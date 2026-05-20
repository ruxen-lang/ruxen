//! AST-level async lowering — Milestone 2A of the async sub-phase 2
//! (`docs/specs/syntax/async_lowering.spec.md` B1–B6).
//!
//! This pass runs BEFORE the resolver. For each top-level `async def`
//! it synthesises a state-machine class that includes `Future` +
//! defines `def var poll(...) -> Poll[T]`, and rewrites the original
//! function so its body returns `__<FnName>Future.new(args...)`.
//!
//! By emitting the synthesised class as if it were user-written, the
//! resolver + typeck + MIR pipeline does its normal job — drop
//! elaboration, mixin satisfaction, codegen — without any
//! state-machine special-case downstream.
//!
//! Milestone 2A is the simplest case: an `async def` with NO `.await`
//! lowers to a single-state machine that returns `Poll.Ready(value)`
//! on the first poll, then `Poll.Pending` forever (per spec B5).
//!
//! Milestone 2B (`.await` suspension points) will extend this pass
//! with multi-state generation; the entry point shape stays the same.
//!
//! Naming: the synthesised class is `__<FnName>Future`. `__` prefix
//! marks it as compiler-internal (user code shouldn't reference it
//! directly; the original function name still works as the entry
//! point). Top-level uniqueness within a module is sufficient for v1;
//! if collisions ever happen we'll add a hash suffix.

use crate::lexer::token::Span;
use crate::parser::ast::*;

/// Run the async-lowering pass over `program`. Mutates in place:
/// every `async def` is replaced with a non-async wrapper that
/// constructs the synthesised state-machine class; the class is
/// appended to `program.items`.
///
/// Methods on classes that are marked `async def` are NOT lowered in
/// Milestone 2A — they require generic captures over `self` that the
/// state-machine class doesn't model yet. Async methods are flagged
/// for the resolver to either reject or accept as sync-bridge today.
pub fn lower_async_defs(program: &mut Program) {
    let mut new_classes: Vec<TopLevelItem> = Vec::new();

    for item in program.items.iter_mut() {
        if let TopLevelItem::Function(func) = item {
            // Milestone 2A scope: only lower async fns whose body
            // has no `.await`. Suspension-point lowering is
            // Milestone 2B; an async fn containing `.await` keeps
            // its sub-phase 1 bridge-mode semantics (parses, `.await`
            // elides at resolve time) until 2B lands. Detecting
            // `.await` cheaply at the AST level keeps 2A focused
            // on the structural shape without touching the
            // multi-state suspension machinery.
            if func.is_async && !func.is_class_method && !block_contains_await(&func.body) {
                if let Some((rewritten, sm_class)) = lower_one_async_fn(func) {
                    *func = rewritten;
                    new_classes.push(TopLevelItem::Class(sm_class));
                }
            }
        }
    }

    // The async fn rewriter sets the wrapper's return type to the
    // synthesised state-machine class. Pass 1 of the resolver
    // (`register_top_level_type_with_ffi`) resolves function return
    // types AS IT REGISTERS each top-level item, in source order —
    // so any function referencing a not-yet-registered class would
    // hit "undefined type". Prepend the new classes to `items` so
    // every class is registered BEFORE the rewritten function that
    // references it. (Appending to the end is the more natural shape
    // but means class type registration runs LATER than the function
    // that returns one — concretely fails as `undefined type
    // __MakeIntFuture` at pass-1 signature resolution.)
    if !new_classes.is_empty() {
        let mut combined = new_classes;
        combined.append(&mut program.items);
        program.items = combined;
    }
}

/// Lower a single top-level `async def` into a (rewritten sync fn,
/// generated state-machine class) pair. Returns `None` if the input
/// shouldn't be lowered (e.g. extern bodies, recovery errors).
fn lower_one_async_fn(func: &FuncDef) -> Option<(FuncDef, ClassDef)> {
    let span = func.span.clone();
    let class_name = mangle_future_class_name(&func.name);

    // ── Synthesise the state-machine class ────────────────────────
    //
    // Layout:
    //   class __<FnName>Future
    //     __state: Int
    //     <arg_i>: <Ty_i>            # one field per fn arg
    //     include Future
    //
    //     def init(@__state: Int, @arg_i: Ty_i, ...)
    //     end
    //
    //     def var poll(cx: &var Context) -> Poll[<ret>]
    //       if self.__state == 0
    //         self.__state = 1
    //         Poll.Ready(<original body, with arg refs → self.arg refs>)
    //       else
    //         Poll.Pending
    //       end
    //     end
    //   end
    //
    // `@__state` makes init take the initial state as an argument so
    // we can pass `0` from the wrapper without writing manual
    // assignment code in the body. Same trick for each captured arg.

    let return_type = func
        .return_type
        .clone()
        .unwrap_or_else(|| TypeExpr::Inferred { span: span.clone() });

    // Fields: __state then each fn arg.
    let mut fields: Vec<FieldDecl> = Vec::new();
    fields.push(FieldDecl {
        visibility: Visibility::Public,
        name: "__state".to_string(),
        type_expr: int_type(&span),
        span: span.clone(),
    });
    for p in &func.params {
        fields.push(FieldDecl {
            visibility: Visibility::Public,
            name: p.name.clone(),
            type_expr: p.type_expr.clone(),
            span: p.span.clone(),
        });
    }

    // init params: @__state, then @<arg> for each fn param.
    let mut init_params: Vec<Param> = Vec::new();
    init_params.push(Param {
        auto_assign: true,
        name: "__state".to_string(),
        type_expr: int_type(&span),
        span: span.clone(),
    });
    for p in &func.params {
        init_params.push(Param {
            auto_assign: true,
            name: p.name.clone(),
            type_expr: p.type_expr.clone(),
            span: p.span.clone(),
        });
    }

    let init_method = FuncDef {
        visibility: Visibility::Public,
        is_async: false,
        self_mode: None, // init is special; resolver treats it as constructor
        is_class_method: false,
        name: "init".to_string(),
        generic_params: None,
        params: init_params,
        return_type: None,
        where_clause: None,
        body: Block {
            statements: Vec::new(),
            span: span.clone(),
        },
        doc_comments: Vec::new(),
        span: span.clone(),
    };

    // poll method.
    //
    // The poll body:
    //   if self.__state == 0
    //     self.__state = 1
    //     Poll.Ready(<rewritten user body>)
    //   else
    //     Poll.Pending
    //   end
    let poll_body = build_poll_body(&func.body, &return_type, &func.params, &span);

    let cx_param = Param {
        auto_assign: false,
        name: "cx".to_string(),
        type_expr: TypeExpr::Reference {
            lifetime: None,
            mutable: true,
            inner: Box::new(named_type("Context", &span)),
            span: span.clone(),
        },
        span: span.clone(),
    };

    let poll_method = FuncDef {
        visibility: Visibility::Public,
        is_async: false,
        self_mode: Some(SelfMode::Mutable),
        is_class_method: false,
        name: "poll".to_string(),
        generic_params: None,
        params: vec![cx_param],
        return_type: Some(poll_return_type(&return_type, &span)),
        where_clause: None,
        body: poll_body,
        doc_comments: Vec::new(),
        span: span.clone(),
    };

    // include Future (so the mixin participates in trait resolution).
    let include_future = InnerImpl {
        is_unsafe: false,
        negative_trait: false,
        trait_name: TypePath {
            segments: vec!["Future".to_string()],
            generic_args: None,
            span: span.clone(),
            rooted: false,
        },
        items: Vec::new(),
        span: span.clone(),
    };

    let sm_class = ClassDef {
        name: class_name.clone(),
        generic_params: None,
        parent: None,
        fields,
        methods: vec![init_method, poll_method],
        inner_impls: vec![include_future],
        derive_traits: Vec::new(),
        layout: Vec::new(),
        lib_decls: Vec::new(),
        doc_comments: vec![format!(
            " Compiler-synthesised Future state machine for `{}`. Spec: docs/specs/syntax/async_lowering.spec.md B2.",
            func.name
        )],
        where_clause: None,
        span: span.clone(),
    };

    // ── Rewrite the original async fn body ────────────────────────
    //
    //   def <name>(args...) -> __<Name>Future
    //     __<Name>Future.new(0, args...)
    //   end
    //
    // is_async is cleared so the typeck signature-lift to
    // `Future[T]` (see `wrap_async_return` in typeck/infer.rs) no
    // longer fires; the return is now the concrete state-machine
    // class.

    let mut ctor_args: Vec<Expr> = Vec::new();
    ctor_args.push(Expr {
        kind: ExprKind::IntLiteral(0, None),
        span: span.clone(),
    });
    for p in &func.params {
        ctor_args.push(Expr {
            kind: ExprKind::Identifier(p.name.clone()),
            span: p.span.clone(),
        });
    }

    let ctor_call = Expr {
        kind: ExprKind::MethodCall {
            object: Box::new(Expr {
                kind: ExprKind::Identifier(class_name.clone()),
                span: span.clone(),
            }),
            method: "new".to_string(),
            generic_args: Vec::new(),
            args: ctor_args,
            block: None,
        },
        span: span.clone(),
    };

    let wrapper = FuncDef {
        visibility: func.visibility,
        is_async: false,
        self_mode: func.self_mode,
        is_class_method: func.is_class_method,
        name: func.name.clone(),
        generic_params: func.generic_params.clone(),
        params: func.params.clone(),
        return_type: Some(named_type(&class_name, &span)),
        where_clause: func.where_clause.clone(),
        body: Block {
            statements: vec![Statement::Expression(ctor_call)],
            span: span.clone(),
        },
        doc_comments: func.doc_comments.clone(),
        span: span.clone(),
    };

    Some((wrapper, sm_class))
}

/// Build the poll method body.
///
///   if self.__state == 0
///     self.__state = 1
///     Poll.Ready(<rewritten body>)
///   else
///     Poll.Pending
///   end
///
/// The `<rewritten body>` substitutes references to function args
/// (`a`, `b`, …) with `self.a`, `self.b`, … so they read from the
/// captured fields.
fn build_poll_body(
    user_body: &Block,
    _return_ty: &TypeExpr,
    args: &[Param],
    span: &Span,
) -> Block {
    // Walk the body and rewrite Identifier(arg_name) → self.arg_name.
    let arg_names: Vec<String> = args.iter().map(|p| p.name.clone()).collect();
    let mut rewritten_body = user_body.clone();
    rewrite_arg_refs_in_block(&mut rewritten_body, &arg_names);

    // The body might be a block with a tail expr or statements with
    // a final expression. Turn the whole user body into a single
    // value expression by wrapping in a Block.
    let body_value_expr = Expr {
        kind: ExprKind::Block(rewritten_body),
        span: span.clone(),
    };

    // Poll.Ready(<body value>) — `Type.Variant(args)` parses as
    // `ExprKind::EnumVariant { type_path, variant, args }` in the
    // user-facing grammar (verified via parser introspection); the
    // resolver lowers it directly to the variant constructor.
    let poll_ready = Expr {
        kind: ExprKind::EnumVariant {
            type_path: vec!["Poll".to_string()],
            variant: "Ready".to_string(),
            args: vec![FieldArg {
                name: None,
                value: body_value_expr,
                span: span.clone(),
            }],
        },
        span: span.clone(),
    };

    // Poll.Pending — unit-variant form, same AST node with empty
    // args (matches the existing 720 fixture's `Poll.Pending` arm).
    let poll_pending = Expr {
        kind: ExprKind::EnumVariant {
            type_path: vec!["Poll".to_string()],
            variant: "Pending".to_string(),
            args: Vec::new(),
        },
        span: span.clone(),
    };

    // self.__state == 0
    let state_eq_zero = Expr {
        kind: ExprKind::BinaryOp {
            left: Box::new(self_field("__state", span)),
            op: BinOp::Eq,
            right: Box::new(Expr {
                kind: ExprKind::IntLiteral(0, None),
                span: span.clone(),
            }),
        },
        span: span.clone(),
    };

    // self.__state = 1
    let set_state_one = Expr {
        kind: ExprKind::Assign {
            target: Box::new(self_field("__state", span)),
            value: Box::new(Expr {
                kind: ExprKind::IntLiteral(1, None),
                span: span.clone(),
            }),
        },
        span: span.clone(),
    };

    let then_block = Block {
        statements: vec![
            Statement::Expression(set_state_one),
            Statement::Expression(poll_ready),
        ],
        span: span.clone(),
    };

    let else_block = Block {
        statements: vec![Statement::Expression(poll_pending)],
        span: span.clone(),
    };

    let if_expr = Expr {
        kind: ExprKind::If(IfExpr {
            condition: Box::new(state_eq_zero),
            then_body: then_block,
            elsif_clauses: Vec::new(),
            else_body: Some(else_block),
            span: span.clone(),
        }),
        span: span.clone(),
    };

    Block {
        statements: vec![Statement::Expression(if_expr)],
        span: span.clone(),
    }
}

/// `self.<name>` as a FieldAccess expression.
fn self_field(name: &str, span: &Span) -> Expr {
    Expr {
        kind: ExprKind::FieldAccess {
            object: Box::new(Expr {
                kind: ExprKind::SelfRef,
                span: span.clone(),
            }),
            field: name.to_string(),
        },
        span: span.clone(),
    }
}

/// Rewrite bare `Identifier(arg)` references to `self.arg` throughout
/// a block. Conservative — only touches the exact name; doesn't
/// shadow-detect (since args don't get shadowed by inner lets that
/// reuse the same name in typical async fn bodies).
///
/// Milestone 2A doesn't have `.await` so the body is straight-line;
/// 2B will need a more sophisticated walk that also re-scopes locals
/// across suspends.
fn rewrite_arg_refs_in_block(block: &mut Block, arg_names: &[String]) {
    for stmt in block.statements.iter_mut() {
        match stmt {
            Statement::Let(let_binding) => {
                if let Some(v) = &mut let_binding.value {
                    rewrite_arg_refs_in_expr(v, arg_names);
                }
            }
            Statement::Expression(e) => rewrite_arg_refs_in_expr(e, arg_names),
        }
    }
}

fn rewrite_arg_refs_in_expr(expr: &mut Expr, arg_names: &[String]) {
    // Replace bare-identifier reads of an arg name with `self.<arg>`.
    if let ExprKind::Identifier(name) = &expr.kind {
        if arg_names.iter().any(|a| a == name) {
            expr.kind = ExprKind::FieldAccess {
                object: Box::new(Expr {
                    kind: ExprKind::SelfRef,
                    span: expr.span.clone(),
                }),
                field: name.clone(),
            };
            return;
        }
    }
    // Otherwise recurse.
    match &mut expr.kind {
        ExprKind::BinaryOp { left, right, .. } => {
            rewrite_arg_refs_in_expr(left, arg_names);
            rewrite_arg_refs_in_expr(right, arg_names);
        }
        ExprKind::UnaryOp { operand, .. } => rewrite_arg_refs_in_expr(operand, arg_names),
        ExprKind::Borrow(inner) | ExprKind::BorrowMut(inner) => {
            rewrite_arg_refs_in_expr(inner, arg_names)
        }
        ExprKind::FieldAccess { object, .. } => rewrite_arg_refs_in_expr(object, arg_names),
        ExprKind::MethodCall { object, args, .. } => {
            rewrite_arg_refs_in_expr(object, arg_names);
            for a in args.iter_mut() {
                rewrite_arg_refs_in_expr(a, arg_names);
            }
        }
        ExprKind::Call { callee, args, .. } => {
            rewrite_arg_refs_in_expr(callee, arg_names);
            for a in args.iter_mut() {
                rewrite_arg_refs_in_expr(a, arg_names);
            }
        }
        ExprKind::Index { object, index } => {
            rewrite_arg_refs_in_expr(object, arg_names);
            rewrite_arg_refs_in_expr(index, arg_names);
        }
        ExprKind::ClosureCall { callee, args } => {
            rewrite_arg_refs_in_expr(callee, arg_names);
            for a in args.iter_mut() {
                rewrite_arg_refs_in_expr(a, arg_names);
            }
        }
        ExprKind::Try(inner) | ExprKind::Await(inner) => {
            rewrite_arg_refs_in_expr(inner, arg_names)
        }
        ExprKind::Assign { target, value }
        | ExprKind::CompoundAssign { target, value, .. } => {
            rewrite_arg_refs_in_expr(target, arg_names);
            rewrite_arg_refs_in_expr(value, arg_names);
        }
        ExprKind::If(IfExpr {
            condition,
            then_body,
            elsif_clauses,
            else_body,
            ..
        }) => {
            rewrite_arg_refs_in_expr(condition, arg_names);
            rewrite_arg_refs_in_block(then_body, arg_names);
            for el in elsif_clauses.iter_mut() {
                rewrite_arg_refs_in_expr(&mut el.condition, arg_names);
                rewrite_arg_refs_in_block(&mut el.body, arg_names);
            }
            if let Some(b) = else_body {
                rewrite_arg_refs_in_block(b, arg_names);
            }
        }
        ExprKind::Match(MatchExpr { subject, arms, .. }) => {
            rewrite_arg_refs_in_expr(subject, arg_names);
            for a in arms.iter_mut() {
                if let Some(g) = &mut a.guard {
                    rewrite_arg_refs_in_expr(g, arg_names);
                }
                match &mut a.body {
                    MatchArmBody::Expr(e) => rewrite_arg_refs_in_expr(e, arg_names),
                    MatchArmBody::Block(b) => rewrite_arg_refs_in_block(b, arg_names),
                }
            }
        }
        ExprKind::Block(b) => rewrite_arg_refs_in_block(b, arg_names),
        ExprKind::Return(Some(inner)) | ExprKind::Break(Some(inner)) => {
            rewrite_arg_refs_in_expr(inner, arg_names)
        }
        ExprKind::ArrayLiteral(items) | ExprKind::TupleLiteral(items) => {
            for it in items.iter_mut() {
                rewrite_arg_refs_in_expr(it, arg_names);
            }
        }
        ExprKind::ArrayFill { value, count } => {
            rewrite_arg_refs_in_expr(value, arg_names);
            rewrite_arg_refs_in_expr(count, arg_names);
        }
        ExprKind::MapLiteral(pairs) => {
            for (k, v) in pairs.iter_mut() {
                rewrite_arg_refs_in_expr(k, arg_names);
                rewrite_arg_refs_in_expr(v, arg_names);
            }
        }
        ExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                rewrite_arg_refs_in_expr(s, arg_names);
            }
            if let Some(e) = end {
                rewrite_arg_refs_in_expr(e, arg_names);
            }
        }
        ExprKind::Cast { expr: inner, .. } => rewrite_arg_refs_in_expr(inner, arg_names),
        ExprKind::Closure(c) => {
            match &mut c.body {
                ClosureBody::Expr(e) => rewrite_arg_refs_in_expr(e, arg_names),
                ClosureBody::Block(b) => rewrite_arg_refs_in_block(b, arg_names),
            }
        }
        // Leaf / no inner exprs to rewrite.
        _ => {}
    }
}

/// `Int` named type.
fn int_type(span: &Span) -> TypeExpr {
    named_type("Int", span)
}

/// A simple named type from a single-segment path.
fn named_type(name: &str, span: &Span) -> TypeExpr {
    TypeExpr::Named(TypePath {
        segments: vec![name.to_string()],
        generic_args: None,
        span: span.clone(),
        rooted: false,
    })
}

/// `Poll[<inner>]` type expression for the poll method's return.
fn poll_return_type(inner: &TypeExpr, span: &Span) -> TypeExpr {
    TypeExpr::Named(TypePath {
        segments: vec!["Poll".to_string()],
        generic_args: Some(vec![inner.clone()]),
        span: span.clone(),
        rooted: false,
    })
}

/// Whether a block contains a `.await` expression anywhere in its
/// expression tree. Milestone 2A skips lowering on any async fn
/// whose body has even one suspension point — those land in
/// Milestone 2B.
fn block_contains_await(block: &Block) -> bool {
    for stmt in &block.statements {
        match stmt {
            Statement::Let(let_binding) => {
                if let Some(v) = &let_binding.value {
                    if expr_contains_await(v) {
                        return true;
                    }
                }
            }
            Statement::Expression(e) => {
                if expr_contains_await(e) {
                    return true;
                }
            }
        }
    }
    false
}

fn expr_contains_await(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Await(_) => true,
        ExprKind::BinaryOp { left, right, .. } => {
            expr_contains_await(left) || expr_contains_await(right)
        }
        ExprKind::UnaryOp { operand, .. } => expr_contains_await(operand),
        ExprKind::Borrow(inner) | ExprKind::BorrowMut(inner) => expr_contains_await(inner),
        ExprKind::FieldAccess { object, .. } => expr_contains_await(object),
        ExprKind::MethodCall { object, args, .. } => {
            expr_contains_await(object) || args.iter().any(expr_contains_await)
        }
        ExprKind::Call { callee, args, .. } => {
            expr_contains_await(callee) || args.iter().any(expr_contains_await)
        }
        ExprKind::ClosureCall { callee, args } => {
            expr_contains_await(callee) || args.iter().any(expr_contains_await)
        }
        ExprKind::Try(inner) => expr_contains_await(inner),
        ExprKind::Assign { target, value } | ExprKind::CompoundAssign { target, value, .. } => {
            expr_contains_await(target) || expr_contains_await(value)
        }
        ExprKind::If(IfExpr {
            condition,
            then_body,
            elsif_clauses,
            else_body,
            ..
        }) => {
            expr_contains_await(condition)
                || block_contains_await(then_body)
                || elsif_clauses.iter().any(|el| {
                    expr_contains_await(&el.condition) || block_contains_await(&el.body)
                })
                || else_body.as_ref().is_some_and(block_contains_await)
        }
        ExprKind::Match(MatchExpr { subject, arms, .. }) => {
            expr_contains_await(subject)
                || arms.iter().any(|a| match &a.body {
                    MatchArmBody::Expr(e) => expr_contains_await(e),
                    MatchArmBody::Block(b) => block_contains_await(b),
                })
        }
        ExprKind::Block(b) => block_contains_await(b),
        ExprKind::Return(Some(inner)) | ExprKind::Break(Some(inner)) => {
            expr_contains_await(inner)
        }
        ExprKind::ArrayLiteral(items) | ExprKind::TupleLiteral(items) => {
            items.iter().any(expr_contains_await)
        }
        ExprKind::Index { object, index } => {
            expr_contains_await(object) || expr_contains_await(index)
        }
        ExprKind::Cast { expr: inner, .. } => expr_contains_await(inner),
        ExprKind::Closure(c) => match &c.body {
            // A nested `async {}` closure has its own scope — its
            // `.await` doesn't count against the outer fn. v1 keeps
            // closures lowered separately; treat closure bodies as
            // opaque for the await-scan.
            ClosureBody::Expr(_) | ClosureBody::Block(_) => false,
        },
        _ => false,
    }
}

/// Compile-mangled state-machine class name for an async fn.
fn mangle_future_class_name(fn_name: &str) -> String {
    // Capitalise the fn name's first segment; preserve the rest.
    // `make_int` → `MakeInt`, `fetch` → `Fetch`. Underscores survive
    // (since user code never types these names, readability over
    // strict mangling).
    let mut camel = String::new();
    let mut upper_next = true;
    for ch in fn_name.chars() {
        if ch == '_' {
            upper_next = true;
            continue;
        }
        if upper_next {
            camel.extend(ch.to_uppercase());
            upper_next = false;
        } else {
            camel.push(ch);
        }
    }
    format!("__{}Future", camel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mangling_round_trip() {
        assert_eq!(mangle_future_class_name("make_int"), "__MakeIntFuture");
        assert_eq!(mangle_future_class_name("fetch"), "__FetchFuture");
        assert_eq!(mangle_future_class_name("a_b_c"), "__ABCFuture");
    }
}
