//! Per-input compilation and execution pipeline.
//!
//! Each REPL input goes through: lex → parse → typecheck → MIR → JIT → execute.
//!
//! State persistence strategy: the session accumulates every successful
//! `def` and `let` as AST nodes. Each new input is compiled against a
//! program that includes all prior `def`s as top-level items and replays
//! all prior `let`s inside the synthetic wrapper function's body. This
//! gives the typechecker the full scope and lets the JIT resolve
//! previously-defined functions by name (already-compiled functions are
//! skipped on the JIT side via `JITCodeGen::is_declared`).

use ruxen_core::diagnostics::{Diagnostic, DiagnosticLevel};
use ruxen_core::hir::types::Ty;
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::mir::nodes::MirFunction;
use ruxen_core::parser::ast::{
    Block, Expr, ExprKind, FuncDef, LetBinding, Pattern, Program, ReplInput, ReplParseResult,
    Statement, TopLevelItem, Visibility,
};
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

use crate::capture;
use crate::commands::{self, Command};
use crate::display;
use crate::session::ReplSession;

/// Classify whether an expression drives side effects the REPL must
/// replay on every subsequent input. Pure reads (`5 + 3`, a bare
/// identifier, a string literal) get the one-shot `=> value : Ty`
/// display path; everything else is appended to `all_statements` and
/// re-executed from scratch each input so mutations persist.
fn is_side_effect_expr(expr: &Expr) -> bool {
    match &expr.kind {
        // Mutations always count.
        ExprKind::Assign { .. } | ExprKind::CompoundAssign { .. } => true,
        // Control flow that wraps a block can drive mutations inside.
        ExprKind::For(_)
        | ExprKind::While(_)
        | ExprKind::WhileLet(_)
        | ExprKind::Loop(_)
        | ExprKind::If(_)
        | ExprKind::IfLet(_)
        | ExprKind::Match(_)
        | ExprKind::Block(_)
        | ExprKind::UnsafeBlock(_) => true,
        // Calls / method calls may print, mutate, or allocate.
        ExprKind::Call { .. }
        | ExprKind::MethodCall { .. }
        | ExprKind::ClosureCall { .. }
        | ExprKind::SafeNavCall { .. } => true,
        // `c.inc` — zero-arg method call with no parens — parses as
        // FieldAccess and only later resolves to a method call. Treat as
        // side-effecting to avoid dropping mutations.
        ExprKind::FieldAccess { .. } | ExprKind::SafeNav { .. } => true,
        ExprKind::Try(_) => true,
        _ => false,
    }
}

/// The result of evaluating a single REPL input.
pub enum EvalResult {
    /// Successfully evaluated, with optional display output.
    Ok(Option<String>),
    /// Command was executed (output string).
    Command(String),
    /// Quit was requested.
    Quit,
    /// Input is incomplete — need continuation lines.
    Incomplete,
    /// Error during compilation or execution.
    Error(String),
}

/// Evaluate a single REPL input line (or accumulated multi-line input).
pub fn eval_input(session: &mut ReplSession, input: &str) -> EvalResult {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return EvalResult::Ok(None);
    }

    // Step 1: Check for REPL commands
    if trimmed.starts_with(':') {
        return eval_command(session, trimmed);
    }

    // Step 2: Lex
    let mut lexer = Lexer::new(input);
    let tokens = match lexer.tokenize() {
        Ok(tokens) => tokens,
        Err(diagnostics) => {
            return EvalResult::Error(format_diagnostics(&diagnostics));
        }
    };

    // Step 3: Parse (REPL mode)
    let mut parser = Parser::new(tokens);
    let repl_input = parser.parse_repl_input();

    match repl_input {
        ReplParseResult::Incomplete => EvalResult::Incomplete,
        ReplParseResult::Error(diags) => EvalResult::Error(format_diagnostics(&diags)),
        ReplParseResult::Complete(input_node) => eval_parsed_input(session, input, input_node),
    }
}

/// Evaluate a parsed REPL input node.
fn eval_parsed_input(
    session: &mut ReplSession,
    raw_input: &str,
    input_node: ReplInput,
) -> EvalResult {
    match input_node {
        ReplInput::Expression(expr) => eval_expression(session, raw_input, expr),
        ReplInput::Statement(stmt) => eval_statement(session, raw_input, stmt),
        ReplInput::TopLevel(item) => eval_top_level(session, raw_input, item),
    }
}

/// Evaluate an expression by wrapping it in a function, compiling, and executing.
fn eval_expression(session: &mut ReplSession, raw_input: &str, expr: Expr) -> EvalResult {
    let fn_name = session.next_repl_fn_name();
    let span = expr.span.clone();
    let side_effecting = is_side_effect_expr(&expr);

    // Wrapper body: replay the full cumulative statement history first
    // so prior `let mut` bindings, assignments, and mutating method
    // calls all take effect before the new expression runs. For
    // side-effecting inputs, the expression is also appended to the
    // cumulative history on success.
    let mut statements: Vec<Statement> = session.all_statements.clone();
    statements.push(Statement::Expression(expr.clone()));

    let wrapper = build_program(
        &session.func_defs,
        &session.type_items,
        &fn_name,
        statements,
        &span,
    );

    let hook = if side_effecting {
        Some(CompileHook::RecordStatement(Statement::Expression(expr)))
    } else {
        None
    };
    compile_and_execute(session, raw_input, &fn_name, wrapper, true, hook)
}

/// Evaluate a statement (let binding or expression statement).
fn eval_statement(session: &mut ReplSession, raw_input: &str, stmt: Statement) -> EvalResult {
    let fn_name = session.next_repl_fn_name();

    match stmt {
        Statement::Let(binding) => {
            let span = binding.span.clone();
            // Replay the cumulative history, then run the new let, then
            // read out its bound name so the wrapper returns the freshly-
            // bound value (displayed as `=> <value> : <ty>`).
            let mut statements: Vec<Statement> = session.all_statements.clone();
            statements.push(Statement::Let(binding.clone()));

            // Pull the identifier name (we only support simple patterns
            // in this phase) and append it as the tail expression.
            if let Pattern::Identifier { name, .. } = &binding.pattern {
                statements.push(Statement::Expression(Expr {
                    kind: ExprKind::Identifier(name.clone()),
                    span: span.clone(),
                }));
            }

            let wrapper = build_program(
                &session.func_defs,
                &session.type_items,
                &fn_name,
                statements,
                &span,
            );

            // Stash the binding so future inputs can see this variable.
            // We add it *before* compile_and_execute so any failures later
            // could in principle leave us with a registered-but-unused
            // binding — but because our replay executes the let each time,
            // the worst case on subsequent runs is that the binding re-
            // evaluates correctly. Still, only push on success to keep
            // error semantics clean.
            let new_binding = binding;
            compile_and_execute(
                session,
                raw_input,
                &fn_name,
                wrapper,
                true,
                Some(CompileHook::RecordLet(new_binding)),
            )
        }
        Statement::Expression(expr) => {
            // Reuse the expression path — it already handles the
            // side-effect classification and cumulative replay.
            let _ = fn_name; // unused in this branch
            eval_expression(session, raw_input, expr)
        }
    }
}

/// Evaluate a top-level item (def, class, struct, etc.).
fn eval_top_level(session: &mut ReplSession, raw_input: &str, item: TopLevelItem) -> EvalResult {
    let item_span = get_item_span(&item);
    match item {
        TopLevelItem::Function(func_def) => {
            let name = func_def.name.clone();

            // Build a program that includes ALL accumulated defs plus
            // the new one so typecheck can resolve cross-references
            // between them. Type-level items (class/enum/trait/...) are
            // replayed first so methods can resolve their target types.
            let mut items: Vec<TopLevelItem> = session.type_items.clone();
            items.extend(
                session
                    .func_defs
                    .iter()
                    .cloned()
                    .map(TopLevelItem::Function),
            );
            items.push(TopLevelItem::Function(func_def.clone()));

            let program = Program {
                items,
                span: item_span,
            };

            // Type check
            let type_result = typeck::type_check(&program);
            let has_errors = type_result
                .diagnostics
                .iter()
                .any(|d| d.level == DiagnosticLevel::Error);

            // Some defs can't compile in isolation and must wait for a
            // later input. Two cases:
            //
            //  1. Inference can't ground out without a call site — e.g.
            //     `def with_x; yield 42; end` (the block param's type is
            //     free until called).
            //  2. FORWARD REFERENCES — `def write_payload` calls
            //     `finish_write`, which is defined further down the same
            //     file. Fed line-by-line, the REPL sees the caller before
            //     the callee, so resolution reports "undefined function
            //     finish_write". Recording the def (instead of erroring)
            //     lets it compile once the callee arrives: every
            //     subsequent input rebuilds the program from the full
            //     `func_defs` set, and the two-phase declare-then-define
            //     in the compile path resolves the cycle.
            //
            // In both cases we record the def and report it accepted; a
            // genuine typo simply surfaces when the symbol is finally
            // called.
            if has_errors {
                let only_deferrable = type_result
                    .diagnostics
                    .iter()
                    .filter(|d| d.level == DiagnosticLevel::Error)
                    .all(|d| {
                        let m = &d.message;
                        m.contains("could not infer")
                            || m.contains("type mismatch")
                            || m.contains("undefined function")
                            || m.contains("undefined variable")
                    });
                if only_deferrable {
                    session.func_defs.push(func_def);
                    session.record_input(raw_input);
                    return EvalResult::Ok(Some(format!(
                        "\x1b[32m=>\x1b[0m {} \x1b[2m: <deferred>\x1b[0m",
                        name
                    )));
                }
                return EvalResult::Error(format_diagnostics(&type_result.diagnostics));
            }

            // Borrow check
            let borrow_errors =
                ruxen_core::borrow_check::borrow_check(&type_result.program, &type_result.symbols);
            if !borrow_errors.is_empty() {
                let msg = borrow_errors
                    .iter()
                    .map(|e| format!("{}", e))
                    .collect::<Vec<_>>()
                    .join("\n");
                return EvalResult::Error(display::format_error(&msg));
            }

            // Do NOT JIT-compile the body now — only record the def and
            // let the next expression/call site compile the accumulated
            // `func_defs` together (compile_and_execute's two-phase pass).
            //
            // Why lazy: overloaded defs (`def classify(Int)` /
            // `def classify(String)` / `def classify(Bool)`) mangle to
            // `classify`, `classify__overloadN`, … and exactly one keeps
            // the bare `classify`. Which one does depends on how many
            // overloads exist at lowering time. Eager-compiling each def
            // as it arrives froze `classify` to the FIRST overload (Int);
            // when a later overload reassigned the bare name (Bool), the
            // JIT's already-defined `classify` symbol couldn't be
            // redefined, so `classify(true)` dispatched to the stale Int
            // body. Deferring compilation until the whole set is present
            // makes every overload compile once, under its final name.
            // (Type + borrow checking above already ran, so genuine
            // errors are still reported at definition time.)

            // Extract param info for display — look at the just-defined fn
            // (matched by name) in the typechecked HIR.
            let (params, return_ty) = type_result
                .program
                .items
                .iter()
                .filter_map(|item| {
                    if let ruxen_core::hir::nodes::HirItem::Function(f) = item {
                        if f.name == name {
                            let params: Vec<(String, Ty)> = f
                                .params
                                .iter()
                                .map(|p| (p.name.clone(), p.ty.clone()))
                                .collect();
                            return Some((params, f.return_ty.clone()));
                        }
                    }
                    None
                })
                .next()
                .unwrap_or((Vec::new(), Ty::Unit));

            // Accumulate for future inputs
            session.func_defs.push(func_def);
            session.record_input(raw_input);

            let output = display::format_fn_signature(&name, &params, &return_ty);
            EvalResult::Ok(Some(output))
        }
        other => {
            // Type-level item: class / struct / enum / trait / impl / const /
            // type-alias / newtype / module / use / lib / extern.
            // Replay all prior items + type_items + func_defs plus the new
            // one so cross-references resolve, type-check the whole program,
            // lower to MIR, and JIT-compile any newly-introduced functions
            // (e.g. methods on a class or `impl` block).
            let mut items: Vec<TopLevelItem> = session.type_items.clone();
            items.extend(
                session
                    .func_defs
                    .iter()
                    .cloned()
                    .map(TopLevelItem::Function),
            );
            items.push(other.clone());

            let program = Program {
                items,
                span: item_span,
            };

            let type_result = typeck::type_check(&program);
            let has_errors = type_result
                .diagnostics
                .iter()
                .any(|d| d.level == DiagnosticLevel::Error);

            if has_errors {
                return EvalResult::Error(format_diagnostics(&type_result.diagnostics));
            }

            // Borrow check
            let borrow_errors =
                ruxen_core::borrow_check::borrow_check(&type_result.program, &type_result.symbols);
            if !borrow_errors.is_empty() {
                let msg = borrow_errors
                    .iter()
                    .map(|e| format!("{}", e))
                    .collect::<Vec<_>>()
                    .join("\n");
                return EvalResult::Error(display::format_error(&msg));
            }

            // Lower to MIR and JIT any new functions (methods, trait
            // impls, etc.) that aren't already declared.
            let mut lowerer = Lowerer::new(&type_result.symbols);
            let mir_program = match lowerer.lower_program(&type_result.program) {
                Ok(mir) => mir,
                Err(e) => return EvalResult::Error(display::format_error(&e)),
            };
            if let Err(e) = session.jit.declare_program_data(&mir_program) {
                return EvalResult::Error(display::format_error(&e));
            }
            let mut to_define: Vec<&MirFunction> = Vec::new();
            for mir_func in &mir_program.functions {
                if session.jit.is_declared(&mir_func.name) {
                    continue;
                }
                if let Err(e) = session.jit.declare_function(mir_func) {
                    return EvalResult::Error(display::format_error(&e));
                }
                to_define.push(mir_func);
            }
            if let Err(e) = session.jit.define_program_data(&mir_program) {
                return EvalResult::Error(display::format_error(&e));
            }
            for mir_func in to_define {
                if let Err(e) = session.jit.compile_function(mir_func) {
                    return EvalResult::Error(display::format_error(&e));
                }
            }
            if let Err(e) = session.jit.finalize() {
                return EvalResult::Error(display::format_error(&e));
            }

            // Accumulate for future inputs.
            session.type_items.push(other);
            session.record_input(raw_input);
            EvalResult::Ok(Some("\x1b[32m=>\x1b[0m \x1b[2mdefined\x1b[0m".to_string()))
        }
    }
}

/// Optional hook to run after a successful compile+execute — e.g., to
/// persist a new `let` binding only when everything typed/JITed/ran cleanly.
enum CompileHook {
    RecordLet(LetBinding),
    RecordStatement(Statement),
}

/// Compile a wrapper program and execute it via JIT.
fn compile_and_execute(
    session: &mut ReplSession,
    raw_input: &str,
    fn_name: &str,
    program: Program,
    show_result: bool,
    on_success: Option<CompileHook>,
) -> EvalResult {
    // Type check
    let type_result = typeck::type_check(&program);
    let has_errors = type_result
        .diagnostics
        .iter()
        .any(|d| d.level == DiagnosticLevel::Error);

    if has_errors {
        return EvalResult::Error(format_diagnostics(&type_result.diagnostics));
    }

    // Borrow check
    let borrow_errors =
        ruxen_core::borrow_check::borrow_check(&type_result.program, &type_result.symbols);
    if !borrow_errors.is_empty() {
        let msg = borrow_errors
            .iter()
            .map(|e| format!("{}", e))
            .collect::<Vec<_>>()
            .join("\n");
        return EvalResult::Error(display::format_error(&msg));
    }

    // Determine the return type from the type-checked HIR of the wrapper
    // (matched by name). This is the inferred result type of the expression
    // we are about to execute — everything downstream (MIR, Cranelift
    // signature, result transmute) keys off this.
    let return_ty = type_result
        .program
        .items
        .iter()
        .filter_map(|item| {
            if let ruxen_core::hir::nodes::HirItem::Function(f) = item {
                if f.name == fn_name {
                    Some(f.return_ty.clone())
                } else {
                    None
                }
            } else {
                None
            }
        })
        .next()
        .unwrap_or(Ty::Unit);

    // MIR lowering
    let mut lowerer = Lowerer::new(&type_result.symbols);
    let mir_program = match lowerer.lower_program(&type_result.program) {
        Ok(mir) => mir,
        Err(e) => return EvalResult::Error(display::format_error(&e)),
    };
    if let Err(e) = session.jit.declare_program_data(&mir_program) {
        return EvalResult::Error(display::format_error(&e));
    }

    // Compile every synthesized MIR function (closures `__closure_N`,
    // class methods, trait default-method monomorphizations, ...) that
    // the JIT hasn't seen yet. Two-phase so forward references resolve.
    let mut to_define: Vec<&MirFunction> = Vec::new();
    for mir_func in &mir_program.functions {
        if mir_func.name == fn_name {
            continue;
        }
        if session.jit.is_declared(&mir_func.name) {
            continue;
        }
        if let Err(e) = session.jit.declare_function(mir_func) {
            return EvalResult::Error(display::format_error(&e));
        }
        to_define.push(mir_func);
    }
    if let Err(e) = session.jit.define_program_data(&mir_program) {
        return EvalResult::Error(display::format_error(&e));
    }
    for mir_func in to_define {
        if let Err(e) = session.jit.compile_function(mir_func) {
            return EvalResult::Error(display::format_error(&e));
        }
    }

    // Find the REPL wrapper function in MIR
    let mir_func = match mir_program.functions.iter().find(|f| f.name == fn_name) {
        Some(f) => f,
        None => {
            return EvalResult::Error(display::format_error(&format!(
                "Internal error: REPL function '{}' not found in MIR",
                fn_name
            )))
        }
    };

    // JIT compile the wrapper last.
    let code_ptr = match session.jit.compile_repl_input(mir_func) {
        Ok(ptr) => ptr,
        Err(e) => return EvalResult::Error(display::format_error(&e)),
    };

    // Drain any stray capture-buffer contents from earlier work
    // (errors, `:type` checks, ...) so the post-run capture cleanly
    // reflects this wrapper's stdout.
    let _ = capture::take_all();

    // Execute the JIT-compiled function.
    // The transmute must match the Cranelift ABI for the return type —
    // narrow integers (Char, Int32, UInt8, etc.) must be read back at
    // their native width since Cranelift returns them in the low bits of
    // the return register without zero-extending.
    let raw_result: i64 = match &return_ty {
        Ty::Float | Ty::Float64 => unsafe {
            let func: fn() -> f64 = std::mem::transmute(code_ptr);
            let f = func();
            f.to_bits() as i64
        },
        Ty::Float32 => unsafe {
            let func: fn() -> f32 = std::mem::transmute(code_ptr);
            let f = func();
            (f.to_bits() as u64) as i64
        },
        Ty::Unit | Ty::Never => unsafe {
            let func: fn() = std::mem::transmute(code_ptr);
            func();
            0
        },
        Ty::Bool => unsafe {
            let func: fn() -> i8 = std::mem::transmute(code_ptr);
            func() as i64
        },
        Ty::Int8 => unsafe {
            let func: fn() -> i8 = std::mem::transmute(code_ptr);
            func() as i64
        },
        Ty::UInt8 => unsafe {
            let func: fn() -> u8 = std::mem::transmute(code_ptr);
            func() as i64
        },
        Ty::Int16 => unsafe {
            let func: fn() -> i16 = std::mem::transmute(code_ptr);
            func() as i64
        },
        Ty::UInt16 => unsafe {
            let func: fn() -> u16 = std::mem::transmute(code_ptr);
            func() as i64
        },
        Ty::Int32 => unsafe {
            let func: fn() -> i32 = std::mem::transmute(code_ptr);
            func() as i64
        },
        Ty::UInt32 => unsafe {
            let func: fn() -> u32 = std::mem::transmute(code_ptr);
            func() as i64
        },
        Ty::Char => unsafe {
            let func: fn() -> u32 = std::mem::transmute(code_ptr);
            func() as i64
        },
        // All other integer and pointer types return i64
        _ => unsafe {
            let func: fn() -> i64 = std::mem::transmute(code_ptr);
            func()
        },
    };

    session.record_input(raw_input);

    // Apply the post-success hook (e.g., persist a new let binding).
    let hook_applied = on_success.is_some();
    if let Some(hook) = on_success {
        match hook {
            CompileHook::RecordLet(b) => {
                session.all_statements.push(Statement::Let(b.clone()));
                session.let_bindings.push(b);
            }
            CompileHook::RecordStatement(s) => {
                session.all_statements.push(s);
            }
        }
    }

    // Drain the capture buffer produced by this cumulative replay.
    // The newest input's output is whatever the replayed prefix didn't
    // already produce. We locate it by LINE COUNT rather than a byte
    // prefix: replaying a non-idempotent side effect can print
    // different text the second time (e.g. a `symlink` that now finds
    // the link already present emits an error line where it first
    // emitted a success line), which breaks a `starts_with` prefix diff
    // and used to dump the entire replay to stdout. Line-count skipping
    // tolerates that text drift as long as the replayed prefix emits the
    // same NUMBER of lines it did originally — true for the common case
    // (and every fixture), though a prior statement whose output line
    // count itself depends on mutated state could still mis-skip. That
    // residual case is the replay model's inherent limit; the proper fix
    // is persistent execution state (no replay), out of scope here.
    let captured = capture::take_all();
    let prev_line_count = session.prev_captured_output.matches('\n').count();
    let new_output: String = captured
        .split_inclusive('\n')
        .skip(prev_line_count)
        .collect();
    if !new_output.is_empty() {
        use std::io::Write;
        let stdout = std::io::stdout();
        let mut lock = stdout.lock();
        let _ = lock.write_all(new_output.as_bytes());
        let _ = lock.flush();
    }
    // Only advance the recorded prefix when the new input is actually
    // appended to cumulative history — pure reads replay the same
    // prefix so `prev_captured_output` must not grow.
    if hook_applied {
        session.prev_captured_output = captured;
    }

    // Display result
    if show_result {
        match display::format_result(raw_result, &return_ty) {
            Some(output) => EvalResult::Ok(Some(output)),
            None => EvalResult::Ok(None),
        }
    } else {
        EvalResult::Ok(None)
    }
}

/// Handle a REPL command.
fn eval_command(session: &mut ReplSession, input: &str) -> EvalResult {
    match commands::parse_command(input) {
        Some(Command::Help) => EvalResult::Command(commands::help_text().to_string()),
        Some(Command::Quit) => EvalResult::Quit,
        Some(Command::Reset) => {
            match session.reset() {
                // Silent on success — the next prompt makes the effect
                // obvious to interactive users, and scripted sessions
                // expect no visible acknowledgement here.
                Ok(()) => EvalResult::Ok(None),
                Err(e) => EvalResult::Error(display::format_error(&e)),
            }
        }
        Some(Command::Type(expr_str)) => eval_type_command(session, &expr_str),
        Some(Command::Unknown(cmd)) => EvalResult::Error(display::format_error(&format!(
            "Unknown command ':{cmd}'. Type :help for available commands."
        ))),
        None => EvalResult::Error(display::format_error("Invalid command")),
    }
}

/// Handle the :type command — show type without evaluating.
fn eval_type_command(session: &mut ReplSession, expr_str: &str) -> EvalResult {
    if expr_str.is_empty() {
        return EvalResult::Error(display::format_error("Usage: :type <expression>"));
    }

    // Lex and parse the expression
    let mut lexer = Lexer::new(expr_str);
    let tokens = match lexer.tokenize() {
        Ok(t) => t,
        Err(d) => return EvalResult::Error(format_diagnostics(&d)),
    };

    let mut parser = Parser::new(tokens);
    let parse_result = parser.parse_repl_input();

    match parse_result {
        ReplParseResult::Complete(ReplInput::Expression(expr)) => {
            // Special-case: a bare identifier that matches a known
            // user-defined function — render with parameter names, since
            // `Ty::Fn` itself only carries anonymous parameter types.
            if let ExprKind::Identifier(name) = &expr.kind {
                if let Some(f) = session.func_defs.iter().find(|f| &f.name == name) {
                    return EvalResult::Command(display::format_fn_type_for_def(
                        f,
                        &session.func_defs,
                    ));
                }
            }

            let span = expr.span.clone();
            // Build a wrapper that sees all prior defs + lets (so the
            // expression can reference them).
            let mut statements: Vec<Statement> = session
                .let_bindings
                .iter()
                .cloned()
                .map(Statement::Let)
                .collect();
            statements.push(Statement::Expression(expr));
            let wrapper = build_program(
                &session.func_defs,
                &session.type_items,
                "__type_check",
                statements,
                &span,
            );
            let type_result = typeck::type_check(&wrapper);

            let has_errors = type_result
                .diagnostics
                .iter()
                .any(|d| d.level == DiagnosticLevel::Error);
            if has_errors {
                // `:type` is an inspection command — if the expression
                // references an unknown name (e.g. after `:reset`), stay
                // silent rather than spamming a red error. Interactive
                // users can see the problem by just typing the expression
                // without the `:type` prefix.
                return EvalResult::Ok(None);
            }

            let return_ty = type_result
                .program
                .items
                .iter()
                .filter_map(|item| {
                    if let ruxen_core::hir::nodes::HirItem::Function(f) = item {
                        if f.name == "__type_check" {
                            Some(f.return_ty.clone())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                })
                .next()
                .unwrap_or(Ty::Unit);

            EvalResult::Command(display::format_type(&return_ty))
        }
        ReplParseResult::Complete(_) => {
            EvalResult::Error(display::format_error(":type expects an expression"))
        }
        ReplParseResult::Incomplete => {
            EvalResult::Error(display::format_error("Incomplete expression"))
        }
        ReplParseResult::Error(d) => EvalResult::Error(format_diagnostics(&d)),
    }
}

// ── AST wrapper helpers ────────────────────────────────────────────

/// Build a complete program: all accumulated defs at top level, plus a
/// single synthetic wrapper function whose body is the given statement
/// list. The wrapper's return type is left unannotated so the typechecker
/// infers it from the tail expression.
fn build_program(
    func_defs: &[FuncDef],
    type_items: &[TopLevelItem],
    fn_name: &str,
    statements: Vec<Statement>,
    span: &ruxen_core::lexer::token::Span,
) -> Program {
    let wrapper = FuncDef {
        name: fn_name.to_string(),
        visibility: Visibility::Private,
        generic_params: None,
        self_mode: None,
        is_class_method: false,
        is_async: false,
        params: Vec::new(),
        return_type: None,
        where_clause: None,
        body: Block {
            statements,
            span: span.clone(),
        },
        doc_comments: Vec::new(),
        span: span.clone(),
    };

    // Order: type-level items first (so methods/fns can reference them),
    // then function defs, then the wrapper.
    let mut items: Vec<TopLevelItem> = type_items.to_vec();
    items.extend(func_defs.iter().cloned().map(TopLevelItem::Function));
    items.push(TopLevelItem::Function(wrapper));

    Program {
        items,
        span: span.clone(),
    }
}

/// Get the span from a top-level item.
fn get_item_span(item: &TopLevelItem) -> ruxen_core::lexer::token::Span {
    match item {
        TopLevelItem::Function(f) => f.span.clone(),
        TopLevelItem::Class(c) => c.span.clone(),
        TopLevelItem::Struct(s) => s.span.clone(),
        TopLevelItem::Enum(e) => e.span.clone(),
        TopLevelItem::Mixin(t) => t.span.clone(),
        TopLevelItem::Impl(i) => i.span.clone(),
        TopLevelItem::Module(m) => m.span.clone(),
        TopLevelItem::Use(u) => u.span.clone(),
        TopLevelItem::TypeAlias(t) => t.span.clone(),
        TopLevelItem::Newtype(n) => n.span.clone(),
        TopLevelItem::Const(c) => c.span.clone(),
        TopLevelItem::Lib(l) => l.span.clone(),
        TopLevelItem::Extern(e) => e.span.clone(),
    }
}

/// Format diagnostics for REPL display (compact format).
fn format_diagnostics(diagnostics: &[Diagnostic]) -> String {
    diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .map(|d| display::format_error(&d.message))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ruxen_core::lexer::Lexer;
    use ruxen_core::parser::Parser;

    fn parse_expr(src: &str) -> Expr {
        let tokens = Lexer::new(src).tokenize().expect("lex failed");
        let mut p = Parser::new(tokens);
        match p.parse_repl_input() {
            ReplParseResult::Complete(ReplInput::Expression(e)) => e,
            ReplParseResult::Complete(ReplInput::Statement(Statement::Expression(e))) => e,
            other => panic!(
                "expected expression, got {:?}",
                match other {
                    ReplParseResult::Complete(_) => "top-level",
                    ReplParseResult::Incomplete => "incomplete",
                    ReplParseResult::Error(_) => "error",
                }
            ),
        }
    }

    #[test]
    fn literal_int_is_not_side_effecting() {
        assert!(!is_side_effect_expr(&parse_expr("42")));
    }

    #[test]
    fn literal_string_is_not_side_effecting() {
        assert!(!is_side_effect_expr(&parse_expr("\"hi\"")));
    }

    #[test]
    fn identifier_is_not_side_effecting() {
        assert!(!is_side_effect_expr(&parse_expr("x")));
    }

    #[test]
    fn binary_op_is_not_side_effecting() {
        assert!(!is_side_effect_expr(&parse_expr("1 + 2")));
    }

    #[test]
    fn assignment_is_side_effecting() {
        assert!(is_side_effect_expr(&parse_expr("x = 5")));
    }

    #[test]
    fn compound_assign_is_side_effecting() {
        assert!(is_side_effect_expr(&parse_expr("x += 1")));
    }

    #[test]
    fn function_call_is_side_effecting() {
        assert!(is_side_effect_expr(&parse_expr("puts(\"hi\")")));
    }

    #[test]
    fn method_call_is_side_effecting() {
        assert!(is_side_effect_expr(&parse_expr("v.push(1)")));
    }

    #[test]
    fn field_access_is_side_effecting() {
        // `c.inc` (zero-arg method) parses as FieldAccess — treat as
        // side-effecting so mutation-via-no-parens calls persist.
        assert!(is_side_effect_expr(&parse_expr("c.inc")));
    }

    #[test]
    fn if_expression_is_side_effecting() {
        assert!(is_side_effect_expr(&parse_expr("if true\n1\nelse\n2\nend")));
    }

    #[test]
    fn block_expression_is_side_effecting() {
        assert!(is_side_effect_expr(&parse_expr("do\n1\nend")));
    }

    #[test]
    fn match_expression_is_side_effecting() {
        assert!(is_side_effect_expr(&parse_expr("match x\n_ -> 1\nend")));
    }
}
