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
use std::collections::HashSet;

use crate::capture;
use crate::commands::{self, Command};
use crate::display;
use crate::session::ReplSession;

extern "C" {
    /// Cross-language linkage to the C runtime's REPL replay-suppression
    /// flag accessor (defined in
    /// `library/std/core/runtime/repl_replay.c`). The REPL calls this
    /// directly from Rust only in tests that assert the flag is
    /// properly cleared between inputs; runtime production traffic
    /// flips the flag through the synthetic
    /// `__repl_set_replaying(1/0)` Ruxen calls emitted inside the
    /// wrapper body.
    #[allow(dead_code)]
    pub fn ruxen_repl_set_replaying(v: i32) -> i32;
    #[allow(dead_code)]
    pub fn ruxen_repl_get_replaying() -> i32;
}

/// Build the synthetic `__repl_set_replaying(0|1)` statement that
/// brackets the replay portion of every wrapper. Lowering to MIR
/// resolves the call through the `repl_slot_lib` FFI block parsed
/// in `session::parse_repl_slot_lib`, which in turn binds to the
/// C symbol `ruxen_repl_set_replaying`. The argument is a literal
/// 1 (entering replay) or 0 (leaving replay).
fn replay_flag_toggle_stmt(enter: bool, span: &ruxen_core::lexer::token::Span) -> Statement {
    let arg = Expr {
        kind: ExprKind::IntLiteral(if enter { 1 } else { 0 }, None),
        span: span.clone(),
    };
    let call = Expr {
        kind: ExprKind::Call {
            callee: Box::new(Expr {
                kind: ExprKind::Identifier("__repl_set_replaying".to_string()),
                span: span.clone(),
            }),
            args: vec![arg],
            block: None,
        },
        span: span.clone(),
    };
    Statement::Expression(call)
}

/// Build the cumulative replay-prefix statement list for the next
/// input's wrapper: all prior let bindings (so previously-named
/// values rebind), followed by every mutation that targets a known
/// session variable. The runtime flag is set around this block in
/// `build_program`, so any embedded side-effects no-op cleanly.
fn collect_replay_statements(session: &ReplSession) -> Vec<Statement> {
    // Filter out `let` bindings for slot-backed variables. The wrapper's
    // synthetic slot-load prefix (from Task 1.2 / 1.3) is the source of
    // truth for those values; replaying the original let-RHS would
    // re-execute side-effecting initializers (`Thread.spawn_raw`,
    // network bind, file open) AND would shadow the slot-loaded
    // binding with a fresh — and potentially wrong — value.
    //
    // For 727_async_tcp_echo specifically: `let handle = Thread.spawn_raw(...)`
    // re-runs the spawn on every replay, the second bind hits
    // EADDRINUSE, server_loop returns 0, the lexical rebind makes
    // `handle = 0`, and the replayed `if handle == 0; puts; return; end`
    // exits the wrapper before the user's actual input runs. That's
    // the 727 hang.
    //
    // Heap-typed lets (String, Array, etc.) are NOT slot-backed today
    // (Task 1.2 gates on Ty::Int only), so they continue to replay —
    // which is correct for state continuity since their constructor
    // RHS is idempotent.
    let slot_names: HashSet<&str> = session
        .var_slots
        .iter()
        .map(|vs| vs.name.as_str())
        .collect();
    session
        .session_var_mutations
        .iter()
        .filter(|s| !is_let_for_slot_backed(s, &slot_names))
        .cloned()
        .collect()
}

fn is_let_for_slot_backed(stmt: &Statement, slot_names: &HashSet<&str>) -> bool {
    match stmt {
        Statement::Let(b) => match &b.pattern {
            Pattern::Identifier { name, .. } => slot_names.contains(name.as_str()),
            // Multi-pattern lets (tuple/struct destructuring) aren't
            // slot-backed today; let them replay normally.
            _ => false,
        },
        // Assignments / compound-assignments to a slot-backed name are
        // dropped too. The slot prefix/suffix already round-trips the
        // value through the persistent slot, so the slot LOAD reflects
        // every prior mutation's stored result. Re-replaying the
        // assignment would re-apply the mutation on top of the already-
        // up-to-date slot value — `counter = counter + 1` would
        // double-count on every subsequent input. (Pre-Phase 2.5 the
        // model worked because the corresponding `Let` in the replay
        // first re-shadowed the binding back to its initial value, so
        // the chronological assignments rebuilt the current value from
        // scratch. Phase 2.5 makes the slot the source of truth and
        // drops both halves of that dance together.)
        Statement::Expression(e) => match &e.kind {
            ExprKind::Assign { target, .. } | ExprKind::CompoundAssign { target, .. } => {
                if let ExprKind::Identifier(n) = &target.kind {
                    slot_names.contains(n.as_str())
                } else {
                    false
                }
            }
            _ => false,
        },
    }
}

/// Walk an expression and collect every base identifier name that
/// appears as the target of a mutation (assignment LHS, compound-
/// assignment LHS, or as the receiver of a method-call). Used by
/// `is_session_var_mutation` to decide whether a statement's effect
/// touches a name the session has bound — and is therefore worth
/// replaying.
/// Shallow inspector: records the mutation/method-call target at
/// `expr` itself (if any). Does NOT recurse into sub-expressions;
/// the caller (`walk_expr_for_targets`) handles recursion. Used as
/// the single-node primitive so we don't have a recursive collector
/// AND a recursive walker fighting each other.
///
/// Currently dead: the replay history (`session_var_mutations`)
/// records every side-effecting expression statement chronologically
/// — the runtime replay-suppression flag handles puts/IO
/// duplication, and Thread.sleep / Instant.now() / `let` RHS
/// evaluation all NEED to replay for state continuity. The
/// classifier remains here as a documented building block for a
/// future per-target optimisation that drops statements whose
/// effects we can prove are local.
#[allow(dead_code)]
fn collect_mutation_targets(expr: &Expr, names: &mut HashSet<String>) {
    match &expr.kind {
        ExprKind::Assign { target, .. } | ExprKind::CompoundAssign { target, .. } => {
            base_identifier(target, names);
        }
        ExprKind::MethodCall { object, .. } | ExprKind::SafeNavCall { object, .. } => {
            // Any method call on a session var is treated as a mutation
            // candidate. Read-only methods (e.g. `arr.len`) replay
            // harmlessly — the runtime flag suppresses any embedded
            // side effect they'd otherwise re-fire.
            base_identifier(object, names);
        }
        ExprKind::FieldAccess { object, .. } | ExprKind::SafeNav { object, .. } => {
            // Zero-arg method call without parens (`c.inc`) parses as
            // FieldAccess — treat as a mutation candidate to keep
            // mutation-via-no-parens calls in the replay set.
            base_identifier(object, names);
        }
        ExprKind::ClosureCall { callee, .. } => {
            // `bump.()` — invoking a closure stored in a session var
            // may mutate captured state. Conservatively treat as a
            // mutation candidate so closure-driven mutations replay.
            base_identifier(callee, names);
        }
        _ => {}
    }
}

/// Pull the base identifier from an l-value-ish expression (variable,
/// field access, index, deref) — what the mutation root would name.
#[allow(dead_code)]
fn base_identifier(expr: &Expr, names: &mut HashSet<String>) {
    let mut cur = expr;
    loop {
        match &cur.kind {
            ExprKind::Identifier(n) => {
                names.insert(n.clone());
                return;
            }
            ExprKind::FieldAccess { object, .. } | ExprKind::SafeNav { object, .. } => {
                cur = object;
            }
            ExprKind::Index { object, .. } => {
                cur = object;
            }
            ExprKind::MethodCall { object, .. } | ExprKind::SafeNavCall { object, .. } => {
                cur = object;
            }
            ExprKind::Try(inner) => cur = inner,
            _ => return,
        }
    }
}

/// Recursively scan a control-flow expression's subtree for mutation
/// targets. Cheap enough — typical block bodies have a handful of
/// statements.
#[allow(dead_code)]
fn walk_expr_for_targets(expr: &Expr, names: &mut HashSet<String>) {
    use ruxen_core::parser::ast::MatchArmBody;
    fn walk_block(b: &Block, names: &mut HashSet<String>) {
        for s in &b.statements {
            match s {
                Statement::Expression(e) => {
                    collect_mutation_targets(e, names);
                    walk_expr_for_targets(e, names);
                }
                Statement::Let(l) => {
                    if let Some(v) = &l.value {
                        walk_expr_for_targets(v, names);
                    }
                }
            }
        }
    }
    // First pull any direct mutation/method-call target from this
    // expression node itself — match scrutinees like `v.pop` need
    // their receiver (`v`) recorded too, not just the bodies.
    collect_mutation_targets(expr, names);
    match &expr.kind {
        ExprKind::Block(b) | ExprKind::UnsafeBlock(b) => walk_block(b, names),
        ExprKind::If(if_expr) => {
            walk_expr_for_targets(&if_expr.condition, names);
            walk_block(&if_expr.then_body, names);
            for clause in &if_expr.elsif_clauses {
                walk_expr_for_targets(&clause.condition, names);
                walk_block(&clause.body, names);
            }
            if let Some(eb) = &if_expr.else_body {
                walk_block(eb, names);
            }
        }
        ExprKind::IfLet(if_let) => {
            walk_expr_for_targets(&if_let.value, names);
            walk_block(&if_let.then_body, names);
            if let Some(eb) = &if_let.else_body {
                walk_block(eb, names);
            }
        }
        ExprKind::Match(m) => {
            walk_expr_for_targets(&m.subject, names);
            for arm in &m.arms {
                match &arm.body {
                    MatchArmBody::Expr(e) => {
                        walk_expr_for_targets(e, names);
                    }
                    MatchArmBody::Block(b) => walk_block(b, names),
                }
            }
        }
        ExprKind::For(f) => {
            walk_expr_for_targets(&f.iterable, names);
            walk_block(&f.body, names);
        }
        ExprKind::While(w) => {
            walk_expr_for_targets(&w.condition, names);
            walk_block(&w.body, names);
        }
        ExprKind::WhileLet(w) => {
            walk_expr_for_targets(&w.value, names);
            walk_block(&w.body, names);
        }
        ExprKind::Loop(l) => walk_block(&l.body, names),
        ExprKind::Call {
            callee,
            args,
            block,
        } => {
            walk_expr_for_targets(callee, names);
            for a in args {
                walk_expr_for_targets(a, names);
            }
            if let Some(b) = block {
                walk_expr_for_targets(b, names);
            }
        }
        ExprKind::MethodCall {
            object,
            args,
            block,
            ..
        } => {
            walk_expr_for_targets(object, names);
            for a in args {
                walk_expr_for_targets(a, names);
            }
            if let Some(b) = block {
                walk_expr_for_targets(b, names);
            }
        }
        ExprKind::SafeNavCall { object, args, .. } => {
            walk_expr_for_targets(object, names);
            for a in args {
                walk_expr_for_targets(a, names);
            }
        }
        ExprKind::Closure(c) => {
            use ruxen_core::parser::ast::ClosureBody;
            match &c.body {
                ClosureBody::Expr(e) => walk_expr_for_targets(e, names),
                ClosureBody::Block(b) => {
                    for s in &b.statements {
                        match s {
                            Statement::Expression(e) => walk_expr_for_targets(e, names),
                            Statement::Let(l) => {
                                if let Some(v) = &l.value {
                                    walk_expr_for_targets(v, names);
                                }
                            }
                        }
                    }
                }
            }
        }
        ExprKind::FieldAccess { object, .. } | ExprKind::SafeNav { object, .. } => {
            walk_expr_for_targets(object, names);
        }
        ExprKind::Index { object, index } => {
            walk_expr_for_targets(object, names);
            walk_expr_for_targets(index, names);
        }
        ExprKind::Assign { target, value } => {
            walk_expr_for_targets(target, names);
            walk_expr_for_targets(value, names);
        }
        ExprKind::CompoundAssign { target, value, .. } => {
            walk_expr_for_targets(target, names);
            walk_expr_for_targets(value, names);
        }
        ExprKind::Try(inner) => walk_expr_for_targets(inner, names),
        // `&var x` / `&mut x` style — the inner identifier is the
        // mutation root, since the borrow lets a downstream callee
        // write through it. Treat as a mutation target so a
        // statement like `append_bang(&var greeting)` lands in the
        // replay set even though no direct `=` assignment appears.
        ExprKind::Borrow(inner) | ExprKind::BorrowMut(inner) => {
            base_identifier(inner, names);
            walk_expr_for_targets(inner, names);
        }
        ExprKind::Await(inner) => walk_expr_for_targets(inner, names),
        ExprKind::UnaryOp { operand, .. } => walk_expr_for_targets(operand, names),
        ExprKind::BinaryOp { left, right, .. } => {
            walk_expr_for_targets(left, names);
            walk_expr_for_targets(right, names);
        }
        ExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                walk_expr_for_targets(s, names);
            }
            if let Some(e) = end {
                walk_expr_for_targets(e, names);
            }
        }
        ExprKind::ClosureCall { callee, args } => {
            walk_expr_for_targets(callee, names);
            for a in args {
                walk_expr_for_targets(a, names);
            }
        }
        _ => {}
    }
}

/// True when `stmt`'s effect touches at least one name in
/// `session_var_names`. Drives the narrow `session_var_mutations`
/// history that gets replayed — statements whose effect is purely
/// local (a one-shot `puts`, a `Command.new(...).status` not bound
/// to anything) are excluded and therefore fire exactly once.
#[allow(dead_code)]
fn is_session_var_mutation(stmt: &Statement, session_var_names: &HashSet<String>) -> bool {
    let expr = match stmt {
        Statement::Expression(e) => e,
        Statement::Let(_) => return false, // let bindings replay separately via let_bindings
    };
    let mut targets: HashSet<String> = HashSet::new();
    walk_expr_for_targets(expr, &mut targets);
    !targets.is_disjoint(session_var_names)
}

/// Classify whether an expression drives side effects the REPL must
/// replay on every subsequent input. Pure reads (`5 + 3`, a bare
/// identifier, a string literal) get the one-shot `=> value : Ty`
/// display path; everything else is appended to `session_var_mutations`
/// (if it touches a session var) so mutations persist. The runtime
/// replay-suppression flag prevents puts/IO inside the replayed
/// mutation from re-firing.
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

    // Build the wrapper body in two slices so the runtime
    // replay-suppression flag can wrap exactly the replay portion:
    //   [replay] = prior let_bindings + session_var_mutations
    //   [user]   = this input's new expression
    // `build_program` injects the toggle calls + slot prefix/suffix.
    let replay_stmts = collect_replay_statements(session);
    let user_stmts: Vec<Statement> = vec![Statement::Expression(expr.clone())];

    let wrapper = build_program(session, &fn_name, replay_stmts, user_stmts, &span);

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
            // Replay portion: prior let_bindings + session_var_mutations
            // — wrapped in the runtime suppression flag so any embedded
            // puts/fs.write/subprocess from the originals do NOT re-fire.
            // User portion: this input's new `let` plus a synthetic
            // identifier read so the wrapper returns the freshly-bound
            // value for display.
            let replay_stmts = collect_replay_statements(session);
            let mut user_stmts: Vec<Statement> = Vec::with_capacity(2);
            user_stmts.push(Statement::Let(binding.clone()));
            if let Pattern::Identifier { name, .. } = &binding.pattern {
                user_stmts.push(Statement::Expression(Expr {
                    kind: ExprKind::Identifier(name.clone()),
                    span: span.clone(),
                }));
            }

            // Phase 2.5: pre-register a slot for the new let BEFORE the
            // wrapper is built so `build_program` includes the matching
            // slot prefix (load — bogus on first run, the user's `let
            // name = …` shadows it) and slot suffix (store — captures
            // the user's freshly-bound value into the persistent slot).
            //
            // Without this, the slot wasn't populated until the SECOND
            // input that references `name` (since `var_slots` was empty
            // at first build_program time, no suffix store was emitted),
            // which combined with Phase 2.5's replay filter — that drops
            // the original `let name = …` so the slot load is the source
            // of truth — left subsequent inputs reading 0 instead of the
            // bound value.
            //
            // To know whether the let is slot-eligible we need its type,
            // which requires a typecheck pass. We typecheck a probe
            // wrapper (no slot ops yet) just to discover the let's
            // inferred type; if it's `Ty::Int` we register the slot,
            // then rebuild the wrapper so the prefix/suffix pair lands
            // around the user's let. The probe's typecheck is cheap
            // (the same body the real path would have typechecked) and
            // is discarded if it errors — the real compile_and_execute
            // will re-surface the same error to the user.
            //
            // Heap-typed lets stay on the existing path: register_var
            // is gated on `Ty::Int` in the RecordLet hook, so a
            // heap-typed binding falls through pre-registration and
            // registers normally (with no slot store, just session
            // history bookkeeping).
            if let Pattern::Identifier {
                name,
                mutable: pat_mut,
                ..
            } = &binding.pattern
            {
                let probe_replay = collect_replay_statements(session);
                let probe_user = vec![
                    Statement::Let(binding.clone()),
                    Statement::Expression(Expr {
                        kind: ExprKind::Identifier(name.clone()),
                        span: span.clone(),
                    }),
                ];
                let probe_fn = format!("__repl_probe_{}", session.input_counter);
                let probe_wrapper =
                    build_program(session, &probe_fn, probe_replay, probe_user, &span);
                let probe_result = typeck::type_check(&probe_wrapper);
                let probe_ok = probe_result
                    .diagnostics
                    .iter()
                    .all(|d| d.level != DiagnosticLevel::Error);
                if probe_ok {
                    if let Some(let_ty) =
                        find_let_type_in_wrapper(&probe_result.program, &probe_fn, name)
                    {
                        if matches!(let_ty, Ty::Int)
                            && session.find_var_slot(name).is_none()
                        {
                            // Either the LetBinding-level `mutable`
                            // (top-level `var name = …` form) or the
                            // pattern's own `mutable` (the inline `let
                            // var name = …` form) is enough to make
                            // the slot mutable — a later input's
                            // `name = expr` is well-formed in both
                            // shapes.
                            let is_mut = binding.mutable || *pat_mut;
                            // Failure here means slot exhaustion —
                            // surface it the same way other compile-
                            // time resource exhaustion would.
                            let _ = session.register_var(name, let_ty, is_mut);
                        }
                    }
                }
            }

            let wrapper = build_program(session, &fn_name, replay_stmts, user_stmts, &span);

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
    if let Some(hook) = on_success {
        match hook {
            CompileHook::RecordLet(b) => {
                // Task 1.2: every successful single-identifier let
                // allocates (or refreshes) its persistent slot so the
                // next input's synthetic prefix can load it. Multi-
                // pattern lets (tuple/struct destructuring) skip slot
                // registration for now — Phase 1 scope is primitives
                // only.
                if let Pattern::Identifier {
                    name,
                    mutable: pat_mut,
                    ..
                } = &b.pattern
                {
                    if let Some(let_ty) =
                        find_let_type_in_wrapper(&type_result.program, fn_name, name)
                    {
                        // Phase 1.2 only emits a slot-load prefix for
                        // Int (see `slot_load_let`). To avoid burning
                        // through `REPL_MAX_SLOTS` on session vars whose
                        // type the prefix can't yet consume, only
                        // register slot-eligible types here. Future
                        // phases that widen `slot_load_let` should
                        // update this gate in lockstep.
                        if matches!(let_ty, Ty::Int) {
                            // Phase 2.5: `eval_statement` already pre-
                            // registers the slot before build_program so
                            // the in-wrapper `__slot_store_i64` suffix
                            // captures the bound value on the very first
                            // input. The call here is idempotent for the
                            // common path (find_var_slot was already Some
                            // pre-registration), but stays as a safety
                            // net for any future caller that lands a
                            // RecordLet hook without going through the
                            // eval_statement Let arm. The mutability
                            // flag is propagated so a `var foo = …`
                            // binding gets a mutable slot-load in the
                            // next input's wrapper (otherwise the user's
                            // subsequent `foo = …` would hit E1006
                            // "cannot assign to `let` binding").
                            //
                            // Mutability lives on the LetBinding wrapper
                            // for the top-level `var name = …` form (the
                            // standard idiom) and on Pattern::Identifier
                            // for the inline `let var name = …` form.
                            // Either flag is enough to make the slot
                            // mutable — `parse_let_binding` consumes
                            // `var` before `parse_pattern`, so `var
                            // counter = 0` is `LetBinding { mutable:
                            // true, pattern: Identifier { mutable:
                            // false, … }, … }`, while `let var counter =
                            // 0` is `LetBinding { mutable: false,
                            // pattern: Identifier { mutable: true } }`.
                            let is_mut = b.mutable || *pat_mut;
                            let _ = session.register_var(name, let_ty, is_mut);
                        }
                    }
                }
                session
                    .session_var_mutations
                    .push(Statement::Let(b.clone()));
                session.let_bindings.push(b);
            }
            CompileHook::RecordStatement(s) => {
                // Task 3: every side-effecting Statement::Expression
                // joins the chronological replay history so future
                // inputs see the same world state — Thread.sleep
                // advancing the clock, fs.write touching the disk,
                // a session-var mutation growing an array, etc.
                // The runtime replay-suppression flag, set around
                // the replay portion of each wrapper, gates the
                // non-idempotent helpers (puts, print, fs.write,
                // Command.status, …) so they no-op on replay even
                // though the statement re-runs. That's why we don't
                // narrow the history to "session-var mutations
                // only" here — narrowing dropped Thread.sleep /
                // Instant.now() chains that the old cumulative
                // replay had been carrying for free, and would
                // regress fixtures like 555 / 725. The classifier
                // (`is_session_var_mutation` + friends) still
                // exists for documentation and as a building block
                // future per-target optimisations may use.
                session.session_var_mutations.push(s);
            }
        }
    }

    // Drain the capture buffer to populate the session's
    // `last_output` snapshot for test harnesses. The capture shims
    // (see `capture::ruxen_repl_puts_shim` & friends) ALREADY write
    // to real stdout/stderr in real time — that's the change that
    // restores correct ordering between `puts` and subprocess
    // stdout (`Command.status` writes the child's fd 1 via the
    // kernel; the prior buffered-then-drained scheme always emitted
    // `puts` after the subprocess output and broke `508_command_status`).
    //
    // With the runtime replay-suppression flag bracketing the
    // replay portion of every wrapper, every replayed `puts` /
    // `print` / `fs.write` / `Command.status` inside the replayed
    // `let_bindings` and `session_var_mutations` no-ops at the
    // C-runtime layer — so both the buffer AND real stdout contain
    // ONLY the user's new statement's output.
    session.last_output = capture::take_all();

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
            // Build a wrapper that sees all prior lets as the replay
            // prefix (so the expression can reference them) and the
            // expression itself as the user portion. The toggle calls
            // bracket the lets — :type never executes anything (it
            // only typechecks) so the toggles are inert here, but
            // including them keeps the wrapper shape identical to the
            // real eval path.
            let replay_stmts: Vec<Statement> = session
                .let_bindings
                .iter()
                .cloned()
                .map(Statement::Let)
                .collect();
            let user_stmts: Vec<Statement> = vec![Statement::Expression(expr)];
            let wrapper = build_program(session, "__type_check", replay_stmts, user_stmts, &span);
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
///
/// Task 1.2 additions:
///   - The REPL-internal `lib "ruxen_repl" ... end` block (declaring
///     `__slot_load_i64` / `__slot_store_i64`) is prepended to the
///     program items so the FFI shims are in scope.
///   - For each session variable with a primitive type that fits in i64
///     (today: `Ty::Int`), a synthetic `let <name>: Int =
///     __slot_load_i64(<addr>)` is PREPENDED to the wrapper body. It
///     binds *before* the replayed `all_statements`, so the replay path
///     still shadows the slot value during Phase 1/1.2 (the store
///     suffix that keeps the slot fresh lands in Task 1.3; once the
///     replay is removed in Phase 3 the prefix becomes the sole source
///     of truth for session-variable reads).
///
/// Task 1.3 additions:
///   - For each slot-eligible session var, a synthetic
///     `__slot_store_i64(<addr>, <name>)` is APPENDED to the wrapper
///     body so the (possibly mutated) value of the binding gets
///     written back to its persistent slot. Combined with the load
///     prefix this closes the read+write loop: a mutation in input N
///     is visible (via the slot) in input N+1.
///   - To keep the wrapper's user-visible return value intact, the
///     trailing tail expression is hoisted into a fresh local
///     (`__ruxen_repl_tail_<fn>`) before the store-suffix and
///     re-emitted after — otherwise the wrapper would return Unit
///     (the type of the last store call) instead of the user's
///     expression value.
///   - Until Phase 3 drops the all_statements replay, the suffix
///     reads the replay-bound `<name>` (whose value is the user's
///     mutated one, because the replay reproduces every assignment).
///     This is the intended shadowing for the dormant slot path.
fn build_program(
    session: &ReplSession,
    fn_name: &str,
    replay_stmts: Vec<Statement>,
    user_stmts: Vec<Statement>,
    span: &ruxen_core::lexer::token::Span,
) -> Program {
    // Task 3 — runtime replay-suppression flag. The replay portion of
    // the wrapper body is bracketed with synthetic
    // `__repl_set_replaying(1)` / `__repl_set_replaying(0)` calls so
    // every non-idempotent C-runtime helper (puts, print, fs.write,
    // Command.status, TcpListener.bind, …) early-returns a benign
    // value during replay. Pure reads (`fs.read*`, `getenv`, …) ignore
    // the flag and execute on both passes — needed so let-RHS values
    // that depend on the world survive replay.
    let mut statements: Vec<Statement> =
        Vec::with_capacity(replay_stmts.len() + user_stmts.len() + 2);
    if !replay_stmts.is_empty() {
        statements.push(replay_flag_toggle_stmt(true, span));
        statements.extend(replay_stmts);
        statements.push(replay_flag_toggle_stmt(false, span));
    }
    // Compute the embedded-`return` check BEFORE moving user_stmts
    // into `statements`. We check BOTH the user's new statements AND
    // the replayed ones: the source_history-replay path re-emits
    // every prior input verbatim (per `collect_replay_statements`),
    // so an `if cond; ...; return; end` recorded in a previous input
    // still ends up in the wrapper body even when this input's user
    // portion is a plain `let`. The tail-preservation transform
    // applied around that replayed `return` produces the same
    // signature mismatch (`arguments of return must match function
    // signature`) the user-portion guard prevents — see the comment
    // block above the `if !slot_vars ... && !body_has_return` branch.
    let user_has_return = statements_contain_return(&user_stmts);
    let replay_has_return = statements_contain_return(&statements);
    let body_has_return = user_has_return || replay_has_return;
    statements.extend(user_stmts);

    // Synthetic slot-load prefix for every primitive session variable.
    // Heap-backed types (String, Array, Option, Result, struct/class
    // instances) are deferred to a follow-up — the ABI is the same
    // (i64 handle), but resolving the right Ruxen type to annotate
    // requires more plumbing than this phase pays for.
    //
    // Task 1.3: collect slot-eligible vars up front so the prefix
    // (load) and suffix (store) iterate the exact same set. A var
    // that gets a prefix MUST get a matching suffix; otherwise a
    // mutation in user code is silently dropped before the next
    // input loads.
    let slot_vars: Vec<&crate::session::VarSlot> = session
        .var_slots
        .iter()
        .filter(|vs| slot_kind_eligible(&vs.ty))
        .collect();

    let mut body: Vec<Statement> = Vec::with_capacity(statements.len() + slot_vars.len() * 2 + 2);
    for vs in &slot_vars {
        if let Some(stmt) = slot_load_let(vs, session, span) {
            body.push(stmt);
        }
    }
    body.extend(statements);

    // Task 1.3: slot-store suffix. For each slot-eligible session var,
    // append `__slot_store_i64(addr, name)` so the current binding's
    // value (whatever the replay + user statements left it at) is
    // written back to the persistent slot. The next input's prefix
    // will load that value.
    //
    // Tail preservation: the wrapper's return type is inferred from
    // the tail expression. A store call is `Unit`-typed, so naïvely
    // appending it would change the wrapper's return type and break
    // the value-display path (`=> 15 : Int` would become `=> () :
    // Unit`). We re-bind the existing tail to a temporary, push the
    // stores, then re-emit the temporary as the new tail.
    //
    // Skip the tail-preservation transform when the user's input
    // contains an embedded `return` along a non-tail path. Applying
    // the transform there produces a wrapper signature that the
    // bare-return exit point can't satisfy: the wrapper's CLIF
    // signature is inferred from the rebound `__ruxen_repl_tail_<fn>`
    // (typed as whatever the original tail expression was — often a
    // primitive Int), but the user's bare `return` lowers to MIR
    // `Terminator::Return(None)` which the JIT emits as
    // `return_(&[])`. Signature expects one operand, the bare return
    // supplies zero → Cranelift's verifier rejects with
    // `arguments of return must match function signature` (the
    // exact failure on 727_async_tcp_echo's third REPL input,
    // `if handle == 0; puts "spawn_fail"; return; end`).
    //
    // Without the transform the user's `return` becomes the actual
    // exit; the wrapper's return type infers naturally from the
    // remaining tail (or Unit when the tail is a control-flow block
    // or itself a return). Slot stores are skipped on this path —
    // an input that bails out via `return` can't be relied on to
    // have mutated state in a meaningful way anyway, and the next
    // input's slot prefix will reload from the persistent slot
    // (i.e. the value the previous successful input wrote).

    // Tail-preserve dance: only when we have stores to emit AND the
    // input doesn't embed a `return`. Otherwise `build_program` is a
    // no-op transform over the input statements (matches Task 1.2
    // behaviour exactly for sessions with no Int vars).
    //
    // Phase 2.5: when `body_has_return` is true we STILL need to emit
    // the slot store suffix — the replayed `return` typically doesn't
    // fire (its condition reads a slot-loaded value whose current
    // state means the if-branch is false), the user's let-RHS runs
    // to completion, and the next input's slot prefix is the sole
    // source of truth for the new value. Without the store the next
    // input observes the previous (or 0) slot value, breaking 727
    // (`let ok = client_flow()` → next input's `if ok == 1` reads
    // stale 0 → echo_fail).
    //
    // What we skip on the body_has_return path is just the
    // tail-preservation rebind — Phase 2's Unit-coercion below
    // strips the wrapper's natural tail and pushes `Block(())`, so
    // the wrapper return type is Unit regardless of whether the
    // store calls leave non-Unit expressions in tail position. The
    // stores themselves are simple `Expression(Call(...))` statements
    // that don't change the wrapper's effective tail because Phase 2
    // re-emits its own Unit tail after them.
    if !slot_vars.is_empty() {
        if !body_has_return {
            // Standard tail-preservation: rebind the natural tail to
            // a temp, emit stores, re-emit temp.
            let tail_name = format!("__ruxen_repl_tail_{}", fn_name);
            let original_tail_expr: Option<Expr> = match body.last() {
                Some(Statement::Expression(_)) => match body.pop() {
                    Some(Statement::Expression(e)) => Some(e),
                    _ => unreachable!(),
                },
                _ => None,
            };
            let had_tail = original_tail_expr.is_some();

            if let Some(tail_expr) = original_tail_expr {
                // `let __ruxen_repl_tail_<fn> = <original tail>`
                body.push(Statement::Let(LetBinding {
                    mutable: false,
                    pattern: Pattern::Identifier {
                        mutable: false,
                        name: tail_name.clone(),
                        span: span.clone(),
                    },
                    type_annotation: None,
                    value: Some(Box::new(tail_expr)),
                    span: span.clone(),
                }));
            }

            for vs in &slot_vars {
                body.push(slot_store_expr_stmt(vs, session, span));
            }

            // Re-emit the temp as the new tail so the wrapper's return
            // type — and thus the user-visible result — is preserved.
            if had_tail {
                body.push(Statement::Expression(Expr {
                    kind: ExprKind::Identifier(tail_name),
                    span: span.clone(),
                }));
            }
        } else {
            // body_has_return path: append stores directly, no tail
            // rebind. Phase 2's coercion below strips the user's
            // natural tail and pushes Block(()), so the wrapper's
            // overall return type is Unit and the bare `return`
            // matches the signature.
            for vs in &slot_vars {
                body.push(slot_store_expr_stmt(vs, session, span));
            }
        }
    }

    // Phase 2 redesign: when the wrapper body contains any `return`
    // (user OR replayed), the wrapper MUST declare a Unit return
    // type so the bare `return`'s default Unit value matches the
    // signature. Phase 1 already skipped the slot-store tail-preserve
    // rebind via `body_has_return`, but skipping the rebind alone is
    // not enough — the wrapper's natural tail (e.g. the synthetic
    // `Identifier(ok)` display read appended after `let ok =
    // client_flow()`) still infers a non-Unit return type. Cranelift's
    // verifier then rejects the replayed `return_(&[])` with
    // "arguments of return must match function signature".
    //
    // Fix: strip the user's natural tail expression IF it's a
    // pure display read (a `Statement::Expression(Identifier(_))`
    // — the synthetic line `eval_statement` appends after a let to
    // surface the bound name) and append an empty Block (which
    // evaluates to Unit). The wrapper unambiguously returns Unit
    // and both the bare `return` and the synthetic tail match.
    //
    // We deliberately do NOT pop side-effecting expression
    // statements like `puts "reached"` — those are the user's
    // actual work and must still run. They typically already type
    // as Unit anyway; the Unit literal we append simply makes the
    // wrapper's tail position unambiguous.
    //
    // User-visible: inputs that contain a `return` no longer surface
    // their natural tail value via `=> <value> : <ty>`. This matches
    // compile-and-run semantics (`def main; …; return; end` returns
    // Unit and has no display value).
    if body_has_return {
        if let Some(Statement::Expression(e)) = body.last() {
            if matches!(e.kind, ExprKind::Identifier(_)) {
                body.pop();
            }
        }
        body.push(Statement::Expression(Expr {
            kind: ExprKind::Block(Block {
                statements: Vec::new(),
                span: span.clone(),
            }),
            span: span.clone(),
        }));
    }

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
            statements: body,
            span: span.clone(),
        },
        doc_comments: Vec::new(),
        span: span.clone(),
    };

    // Order: REPL slot-FFI lib (declares the __slot_load_i64 /
    // __slot_store_i64 symbols), then type-level items (so methods/fns
    // can reference them), then function defs, then the wrapper.
    let mut items: Vec<TopLevelItem> = Vec::with_capacity(session.type_items.len() + 2);
    items.push(session.repl_slot_lib.clone());
    items.extend(session.type_items.iter().cloned());
    items.extend(
        session
            .func_defs
            .iter()
            .cloned()
            .map(TopLevelItem::Function),
    );
    items.push(TopLevelItem::Function(wrapper));

    Program {
        items,
        span: span.clone(),
    }
}

/// Walk the typed wrapper function's HIR body and return the inferred
/// type of the LAST let whose binding pattern is
/// `Pattern::Binding { name }`. Used by `CompileHook::RecordLet` to
/// feed `register_var` with the real (post-inference) type rather than
/// the parser-side annotation (which may be absent or partial).
///
/// The "last" semantics matter: the wrapper body holds three layered
/// bindings for a rebound session var — the synthetic slot-load prefix
/// (always `Ty::Int` today), the cumulative `all_statements` replay
/// (the prior input's type), and the new user-level let we're about to
/// record. Scope-wise the new let shadows the others, so its type is
/// the right answer to register for the slot — picking the first match
/// would clamp every rebind to whatever the prefix said.
///
/// Returns `None` when no such let is found — e.g. multi-pattern
/// destructuring or a name mismatch.
fn find_let_type_in_wrapper(
    program: &ruxen_core::hir::nodes::HirProgram,
    fn_name: &str,
    binding_name: &str,
) -> Option<Ty> {
    use ruxen_core::hir::nodes::{HirExprKind, HirItem, HirPattern, HirStatement};
    fn search_stmts(stmts: &[HirStatement], name: &str) -> Option<Ty> {
        let mut last: Option<Ty> = None;
        for s in stmts {
            if let HirStatement::Let { pattern, ty, .. } = s {
                if let HirPattern::Binding { name: n, .. } = pattern {
                    if n == name {
                        last = Some(ty.clone());
                    }
                }
            }
        }
        last
    }
    for item in &program.items {
        if let HirItem::Function(f) = item {
            if f.name == fn_name {
                if let HirExprKind::Block(stmts, _tail) = &f.body.kind {
                    return search_stmts(stmts, binding_name);
                }
            }
        }
    }
    None
}

/// True for session-variable types the slot prefix/suffix pair currently
/// supports. Phase 1 scope is `Ty::Int` only — Bool/Float/Char and the
/// heap-backed types (String, Array, Option, Result, struct/class
/// instances) fit in i64 the same way but need either a narrowing
/// transmute (Bool/Float/Char) or a Ruxen-side cast (heap handles) to
/// type-check, both deferred to follow-up phases.
///
/// This is the single source of truth shared between `build_program`
/// (which decides whether to inject a load/store pair at all), the
/// prefix builder `slot_load_let`, and the suffix builder
/// `slot_store_expr_stmt`. Widening it without updating both sides
/// would emit a load with no matching store (mutations lost) or vice
/// versa (slot stuck at its initial value).
fn slot_kind_eligible(ty: &Ty) -> bool {
    matches!(ty, Ty::Int)
}

/// Build a synthetic `let <name>: <Ty> = __slot_load_i64(<addr>)` for a
/// primitive session variable. Returns `None` for types this phase
/// doesn't yet handle (Bool/Float/Char/heap types) — heap types fit
/// in i64 the same way but need a Ruxen-side cast on the load to type-
/// check, which is deferred.
fn slot_load_let(
    vs: &crate::session::VarSlot,
    session: &ReplSession,
    span: &ruxen_core::lexer::token::Span,
) -> Option<Statement> {
    use ruxen_core::parser::ast::{LetBinding, Pattern, TypeExpr, TypePath};
    // Phase 1 scope: Int only. Bool/Float/Char/heap deferred — see fn doc.
    let (ty_name, ret_segment): (&str, &str) = match vs.ty {
        Ty::Int => ("Int", "Int"),
        _ => return None,
    };
    let addr = session.slot_addr(vs.idx);
    let call = Expr {
        kind: ExprKind::Call {
            callee: Box::new(Expr {
                kind: ExprKind::Identifier("__slot_load_i64".to_string()),
                span: span.clone(),
            }),
            args: vec![Expr {
                kind: ExprKind::IntLiteral(addr, None),
                span: span.clone(),
            }],
            block: None,
        },
        span: span.clone(),
    };
    let _ = ret_segment;
    let type_annotation = Some(TypeExpr::Named(TypePath {
        segments: vec![ty_name.to_string()],
        generic_args: None,
        span: span.clone(),
        rooted: false,
    }));
    Some(Statement::Let(LetBinding {
        mutable: vs.mutable,
        pattern: Pattern::Identifier {
            mutable: vs.mutable,
            name: vs.name.clone(),
            span: span.clone(),
        },
        type_annotation,
        value: Some(Box::new(call)),
        span: span.clone(),
    }))
}

/// Build a synthetic `__slot_store_i64(<addr>, <name>)` expression
/// statement. Counterpart to `slot_load_let` — emitted at the END of
/// the wrapper body so whatever value the user's statements left the
/// `<name>` binding at is persisted to the slot for the next input.
///
/// Callers must only invoke this for `vs.ty` values that
/// `slot_kind_eligible` returns true for; the FFI signature for
/// `ruxen_repl_slot_store_i64` is `(Int, Int) -> Unit`, so a non-Int
/// value would fail typeck. Until widening hits, the matching
/// load-let above is also Int-typed, so the binding the suffix
/// references is always Int.
fn slot_store_expr_stmt(
    vs: &crate::session::VarSlot,
    session: &ReplSession,
    span: &ruxen_core::lexer::token::Span,
) -> Statement {
    let addr = session.slot_addr(vs.idx);
    let call = Expr {
        kind: ExprKind::Call {
            callee: Box::new(Expr {
                kind: ExprKind::Identifier("__slot_store_i64".to_string()),
                span: span.clone(),
            }),
            args: vec![
                Expr {
                    kind: ExprKind::IntLiteral(addr, None),
                    span: span.clone(),
                },
                Expr {
                    kind: ExprKind::Identifier(vs.name.clone()),
                    span: span.clone(),
                },
            ],
            block: None,
        },
        span: span.clone(),
    };
    Statement::Expression(call)
}

/// Walk a statement list looking for an embedded `return` along any
/// path. Used by `build_program` to decide whether to apply the
/// tail-preservation transform from Task 1.3.
///
/// When the user's input contains a `return`, the transform would
/// produce a wrapper whose CLIF signature is inferred from the
/// rebound tail (e.g. `Int` because the original tail was an Int
/// expression) — but the bare `return` lowers to MIR
/// `Terminator::Return(None)`, which Cranelift's JIT emits as
/// `return_(&[])`. Signature wants one operand, the bare return
/// supplies zero → verifier rejects with
/// `arguments of return must match function signature`. Skip the
/// transform in that case: the user's `return` becomes the actual
/// exit path, the wrapper's return type infers naturally from the
/// remaining tail (or Unit when the tail is a control-flow block
/// or itself a return).
///
/// Conservative: returns true for any nested `return`, even when
/// the user clearly intended only a partial short-circuit. Skipping
/// the transform in those cases is safe — we just stop capturing
/// the tail value into a synthetic let — and dodges the verifier
/// error without trying to reason about which exit type "should
/// win".
fn statements_contain_return(statements: &[Statement]) -> bool {
    statements.iter().any(|s| match s {
        Statement::Expression(e) => expr_contains_return(e),
        Statement::Let(b) => b
            .value
            .as_deref()
            .map(expr_contains_return)
            .unwrap_or(false),
    })
}

/// Recurse through every expression node that can host a `return`.
/// The wildcard arm at the bottom is intentional: if a future
/// `ExprKind` variant gains an `Expr`/`Block` field that can host
/// a return, it must be added here or `build_program` will miss the
/// embedded return and reapply the buggy tail-preservation
/// transform. Audit when `ExprKind` grows.
fn expr_contains_return(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Return(_) => true,
        ExprKind::If(if_expr) => {
            expr_contains_return(&if_expr.condition)
                || block_contains_return(&if_expr.then_body)
                || if_expr
                    .elsif_clauses
                    .iter()
                    .any(|c| expr_contains_return(&c.condition) || block_contains_return(&c.body))
                || if_expr
                    .else_body
                    .as_ref()
                    .is_some_and(block_contains_return)
        }
        ExprKind::IfLet(if_let) => {
            expr_contains_return(&if_let.value)
                || block_contains_return(&if_let.then_body)
                || if_let
                    .else_body
                    .as_ref()
                    .is_some_and(block_contains_return)
        }
        ExprKind::Match(match_expr) => {
            expr_contains_return(&match_expr.subject)
                || match_expr.arms.iter().any(|arm| {
                    let guard_has = arm
                        .guard
                        .as_deref()
                        .map(expr_contains_return)
                        .unwrap_or(false);
                    let body_has = match &arm.body {
                        ruxen_core::parser::ast::MatchArmBody::Expr(e) => expr_contains_return(e),
                        ruxen_core::parser::ast::MatchArmBody::Block(b) => {
                            block_contains_return(b)
                        }
                    };
                    guard_has || body_has
                })
        }
        ExprKind::While(w) => expr_contains_return(&w.condition) || block_contains_return(&w.body),
        ExprKind::WhileLet(w) => {
            expr_contains_return(&w.value) || block_contains_return(&w.body)
        }
        ExprKind::For(f) => {
            expr_contains_return(&f.iterable) || block_contains_return(&f.body)
        }
        ExprKind::Loop(l) => block_contains_return(&l.body),
        ExprKind::Block(b) => block_contains_return(b),
        ExprKind::UnsafeBlock(b) => block_contains_return(b),
        ExprKind::BinaryOp { left, right, .. } => {
            expr_contains_return(left) || expr_contains_return(right)
        }
        ExprKind::UnaryOp { operand, .. } => expr_contains_return(operand),
        ExprKind::Borrow(inner) | ExprKind::BorrowMut(inner) => expr_contains_return(inner),
        ExprKind::FieldAccess { object, .. } | ExprKind::SafeNav { object, .. } => {
            expr_contains_return(object)
        }
        ExprKind::MethodCall { object, args, .. } => {
            expr_contains_return(object) || args.iter().any(expr_contains_return)
        }
        ExprKind::SafeNavCall { object, args, .. } => {
            expr_contains_return(object) || args.iter().any(expr_contains_return)
        }
        ExprKind::Call { callee, args, .. } => {
            expr_contains_return(callee) || args.iter().any(expr_contains_return)
        }
        ExprKind::ClosureCall { callee, args } => {
            expr_contains_return(callee) || args.iter().any(expr_contains_return)
        }
        ExprKind::Index { object, index } => {
            expr_contains_return(object) || expr_contains_return(index)
        }
        ExprKind::Try(inner) | ExprKind::Await(inner) => expr_contains_return(inner),
        ExprKind::Assign { target, value } => {
            expr_contains_return(target) || expr_contains_return(value)
        }
        ExprKind::CompoundAssign { target, value, .. } => {
            expr_contains_return(target) || expr_contains_return(value)
        }
        ExprKind::Range { start, end, .. } => {
            start
                .as_deref()
                .map(expr_contains_return)
                .unwrap_or(false)
                || end.as_deref().map(expr_contains_return).unwrap_or(false)
        }
        ExprKind::ArrayLiteral(items) => items.iter().any(expr_contains_return),
        ExprKind::ArrayFill { value, count } => {
            expr_contains_return(value) || expr_contains_return(count)
        }
        ExprKind::TupleLiteral(items) => items.iter().any(expr_contains_return),
        ExprKind::MapLiteral(pairs) => pairs
            .iter()
            .any(|(k, v)| expr_contains_return(k) || expr_contains_return(v)),
        ExprKind::Break(inner) => inner
            .as_deref()
            .map(expr_contains_return)
            .unwrap_or(false),
        ExprKind::Yield(items) => items.iter().any(expr_contains_return),
        ExprKind::MacroCall { args, .. } => args.iter().any(expr_contains_return),
        ExprKind::Cast { expr, .. } => expr_contains_return(expr),
        ExprKind::EnumVariant { args, .. } => args.iter().any(|f| expr_contains_return(&f.value)),
        ExprKind::Closure(_) => {
            // A closure body is a distinct function — a `return` inside
            // it returns from the closure, not from the wrapper. So we
            // intentionally do NOT recurse into closure bodies; the
            // closure's body is lowered as its own MIR function with
            // its own signature.
            false
        }
        // Pure leaves — literals, identifiers, self/SelfType,
        // Continue, NullLiteral. None can host an embedded return.
        ExprKind::IntLiteral(..)
        | ExprKind::FloatLiteral(..)
        | ExprKind::StringLiteral(_)
        | ExprKind::InterpolatedString(_)
        | ExprKind::CharLiteral(_)
        | ExprKind::BoolLiteral(_)
        | ExprKind::UnitLiteral
        | ExprKind::Identifier(_)
        | ExprKind::SelfRef
        | ExprKind::SelfType
        | ExprKind::Continue
        | ExprKind::NullLiteral => false,
    }
}

fn block_contains_return(block: &Block) -> bool {
    statements_contain_return(&block.statements)
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
