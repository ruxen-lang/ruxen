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

use std::collections::HashMap;

use crate::lexer::token::Span;
use crate::parser::ast::*;

/// Run the async-lowering pass over `program`. Mutates in place:
/// every `async def` is replaced with a non-async wrapper that
/// constructs the synthesised state-machine class; the synthesised
/// classes are PREPENDED to `program.items` so forward references at
/// resolve pass 1 see the class definitions before the rewritten
/// function that returns one.
///
/// Methods on classes that are marked `async def` are NOT lowered in
/// Milestone 2A/2B — they require generic captures over `self` that
/// the state-machine class doesn't model yet. Async methods keep
/// their sub-phase-1 bridge-mode semantics today.
pub fn lower_async_defs(program: &mut Program) {
    // Build a map of `fn_name -> (synth_class_name, declared_return_type)` so
    // a 2B await on `g()` can name the sub-future field type as
    // `__GFuture` and the post-Ready local's type as the user's
    // declared return on `g`. The map covers only top-level async
    // free fns (which is all 2B supports — async methods deferred).
    let mut async_fn_returns: HashMap<String, (String, TypeExpr)> = HashMap::new();
    for item in &program.items {
        if let TopLevelItem::Function(func) = item {
            if func.is_async && !func.is_class_method {
                if let Some(ret) = &func.return_type {
                    async_fn_returns.insert(
                        func.name.clone(),
                        (mangle_future_class_name(&func.name), ret.clone()),
                    );
                }
            }
        }
    }

    let mut new_classes: Vec<TopLevelItem> = Vec::new();

    for item in program.items.iter_mut() {
        if let TopLevelItem::Function(func) = item {
            if !func.is_async || func.is_class_method {
                continue;
            }
            if block_contains_await(&func.body) {
                // Milestone 2B path: async fn with `.await` suspends.
                // The lowering is restricted to a canonical straight-
                // line shape (one or more `let x = g().await; ...`
                // statements followed by a tail expression). Anything
                // outside the supported shape falls back to leaving
                // the function in its pre-lowering state — the
                // resolver/typeck will surface a follow-up error
                // (E1115 for `.await` in loops; the dedicated
                // diagnostic for if/match arms is deferred). Future
                // work will broaden the supported shape.
                if let Some((rewritten, sm_class)) =
                    lower_one_async_fn_with_await(func, &async_fn_returns)
                {
                    *func = rewritten;
                    new_classes.push(TopLevelItem::Class(sm_class));
                }
            } else {
                // Milestone 2A path: trivial single-state machine.
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

// ─── Milestone 2B — `.await` suspension-point lowering ──────────────
//
// 2B accepts a canonical straight-line shape:
//
//     async def f(args...) -> R
//       let x_1 = g_1(<args from outer/literals>).await
//       let x_2 = g_2(<args from outer/literals>).await
//       ...
//       <tail expr>
//     end
//
// where every awaitee is a direct `Call { callee = Identifier(name) }`
// against another top-level async fn whose return type the lowering
// pass already knows (from `async_fn_returns`). Awaitees whose
// arguments depend on a prior `.await`'s result are NOT supported in
// v1 (sub-futures are constructed eagerly in `init` from outer args /
// constants only). Anything more complex causes the pass to bail and
// leave the fn for downstream stages to diagnose.
//
// Generated class layout for the example above:
//
//     class __FFuture
//       __state: Int
//       <outer_arg_i>: <Ty_i>           # captured at construction
//       __sub_0: __G_1Future            # eagerly constructed
//       __sub_1: __G_2Future            # eagerly constructed
//       x_1: <R_1>                      # placeholder-initialised
//       x_2: <R_2>
//       include Future
//
//       def init(@__state: Int, @<outer_arg_i>: <Ty_i>...)
//         self.__sub_0 = __G_1Future.new(0, <args>...)
//         self.__sub_1 = __G_2Future.new(0, <args>...)
//         self.x_1 = <default of R_1>
//         self.x_2 = <default of R_2>
//       end
//
//       def var poll(cx: &var Context) -> Poll[R]
//         if self.__state == 0
//           match (&var self.__sub_0).poll(cx)
//             Poll.Pending  -> Poll.Pending
//             Poll.Ready(v) ->
//               self.x_1 = v
//               self.__state = 1
//               Poll.Pending
//           end
//         elsif self.__state == 1
//           match (&var self.__sub_1).poll(cx)
//             Poll.Pending  -> Poll.Pending
//             Poll.Ready(v) ->
//               self.x_2 = v
//               self.__state = 2
//               Poll.Ready(<tail with self.x_1/x_2/args>)
//           end
//         else
//           Poll.Pending
//         end
//       end
//     end
//
// Per spec B5, the v1 lowering returns `Poll.Pending` between states
// (instead of eager fall-through) — that costs an extra round-trip
// per state but keeps the lowering pure-functional and trivially
// correct. The eager-progress optimisation arrives with the sub-
// phase 3 executor.
fn lower_one_async_fn_with_await(
    func: &FuncDef,
    async_fn_returns: &HashMap<String, (String, TypeExpr)>,
) -> Option<(FuncDef, ClassDef)> {
    let span = func.span.clone();
    let class_name = mangle_future_class_name(&func.name);

    let return_type = func.return_type.clone()?;

    // ── Phase 1 — segment the body into `(let x = g().await)*  tail`.
    //
    // We require EVERY non-tail statement to be a let-binding whose
    // value is `<call>.await`. The final element is the tail
    // expression. Statements outside that shape cause us to bail.
    let (await_lets, tail_stmts) = segment_body(&func.body)?;
    if await_lets.is_empty() {
        // `block_contains_await` saw an await but the segmenter
        // didn't — the await sits in an unsupported shape (e.g.
        // inside an if-arm or a loop). Bail; the resolver-side
        // E1110 / E1115 checks will surface a diagnostic.
        return None;
    }

    // ── Phase 2 — for each await, locate the awaited fn's synth class.
    let outer_arg_names: Vec<String> = func.params.iter().map(|p| p.name.clone()).collect();
    let mut subs: Vec<AwaitSub> = Vec::new();
    for al in &await_lets {
        let sub = describe_await(al, async_fn_returns, &outer_arg_names)?;
        subs.push(sub);
    }

    // ── Phase 3 — synthesise the state-machine class fields ──────────
    //
    // Field order matters for the `init` body that initialises them
    // (later fields can reference earlier ones via `self.x`).
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
    for (i, sub) in subs.iter().enumerate() {
        fields.push(FieldDecl {
            visibility: Visibility::Public,
            name: format!("__sub_{i}"),
            type_expr: named_type(&sub.sub_class_name, &span),
            span: span.clone(),
        });
    }
    for sub in &subs {
        fields.push(FieldDecl {
            visibility: Visibility::Public,
            name: sub.binding_name.clone(),
            type_expr: sub.result_type.clone(),
            span: span.clone(),
        });
    }

    // ── Phase 4 — init params + body ────────────────────────────────
    //
    // Init takes `@__state` + each outer arg. Inside the body we
    // eagerly construct each sub-future from outer args/constants
    // (the constraint enforced by `describe_await`), and assign each
    // hoisted local to a default of its declared type.
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

    let mut init_body_stmts: Vec<Statement> = Vec::new();
    for (i, sub) in subs.iter().enumerate() {
        // self.__sub_i = <SubFutureClass>.new(0, <args rewritten as self.arg>)
        let mut ctor_args: Vec<Expr> = Vec::new();
        ctor_args.push(Expr {
            kind: ExprKind::IntLiteral(0, None),
            span: span.clone(),
        });
        for a in &sub.awaitee_args {
            let mut rewritten = a.clone();
            rewrite_arg_refs_in_expr(&mut rewritten, &outer_arg_names);
            ctor_args.push(rewritten);
        }
        let ctor = Expr {
            kind: ExprKind::MethodCall {
                object: Box::new(Expr {
                    kind: ExprKind::Identifier(sub.sub_class_name.clone()),
                    span: span.clone(),
                }),
                method: "new".to_string(),
                generic_args: Vec::new(),
                args: ctor_args,
                block: None,
            },
            span: span.clone(),
        };
        init_body_stmts.push(Statement::Expression(Expr {
            kind: ExprKind::Assign {
                target: Box::new(self_field(&format!("__sub_{i}"), &span)),
                value: Box::new(ctor),
            },
            span: span.clone(),
        }));
    }
    for sub in &subs {
        // self.<binding> = <default of declared type>
        let default = default_value_for_type(&sub.result_type, &span)?;
        init_body_stmts.push(Statement::Expression(Expr {
            kind: ExprKind::Assign {
                target: Box::new(self_field(&sub.binding_name, &span)),
                value: Box::new(default),
            },
            span: span.clone(),
        }));
    }

    let init_method = FuncDef {
        visibility: Visibility::Public,
        is_async: false,
        self_mode: None,
        is_class_method: false,
        name: "init".to_string(),
        generic_params: None,
        params: init_params,
        return_type: None,
        where_clause: None,
        body: Block {
            statements: init_body_stmts,
            span: span.clone(),
        },
        doc_comments: Vec::new(),
        span: span.clone(),
    };

    // ── Phase 5 — poll body (chained if/elsif/else over __state) ────
    let poll_body =
        build_multi_state_poll_body(&subs, &tail_stmts, &outer_arg_names, &return_type, &span);

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
            " Compiler-synthesised Future state machine for `{}`. Spec: docs/specs/syntax/async_lowering.spec.md B7-B10.",
            func.name
        )],
        where_clause: None,
        span: span.clone(),
    };

    // ── Phase 6 — rewrite original async fn into a wrapper that
    // constructs the state machine.
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

/// One await suspension point picked out by the segmenter.
struct AwaitSub {
    /// `x` in `let x = g(args).await`. Becomes a field on the sm class.
    binding_name: String,
    /// Awaited fn name (e.g. `g`).
    #[allow(dead_code)]
    awaitee_fn_name: String,
    /// Args passed to the awaitee fn at the await site (e.g. `[]` for
    /// `g().await`, `[a]` for `g(a).await`). Args may reference outer
    /// async-fn args via bare `Identifier`; they're rewritten to
    /// `self.<name>` when emitted into the synth class's init body.
    awaitee_args: Vec<Expr>,
    /// `__GFuture` — the synth state-machine class for the awaitee.
    sub_class_name: String,
    /// User-declared return type of the awaited fn (`Int` for
    /// `async def g() -> Int`). The awaited local's field type is
    /// this type after `Poll.Ready(v)` unwraps.
    result_type: TypeExpr,
}

/// Split an async fn body into `[await let-binding]*  tail-statements`.
/// Returns `None` if the body uses an unsupported shape (e.g. a let
/// without an await on the RHS, an await deeper in an expression, or
/// a statement before the first await that isn't itself `let x = g().await`).
fn segment_body(body: &Block) -> Option<(Vec<LetBinding>, Vec<Statement>)> {
    let mut await_lets: Vec<LetBinding> = Vec::new();
    let mut tail: Vec<Statement> = Vec::new();
    let mut in_tail = false;

    for stmt in &body.statements {
        if in_tail {
            // Post-last-await: forbid further awaits (unsupported v1).
            if let Statement::Let(lb) = stmt {
                if let Some(v) = &lb.value {
                    if expr_contains_await(v) {
                        return None;
                    }
                }
            }
            if let Statement::Expression(e) = stmt {
                if expr_contains_await(e) {
                    return None;
                }
            }
            tail.push(stmt.clone());
            continue;
        }

        match stmt {
            Statement::Let(lb) => {
                // Check if the let RHS is `<expr>.await` — recognise
                // ONLY the outermost-await shape. Anything else (await
                // nested deeper in arithmetic, etc.) bails.
                let value = lb.value.as_ref()?;
                if let ExprKind::Await(_) = &value.kind {
                    // OK — this is `let x = <expr>.await`. Pattern
                    // must be a plain Identifier binding (no
                    // destructuring) for v1.
                    if !matches!(&lb.pattern, Pattern::Identifier { .. }) {
                        return None;
                    }
                    await_lets.push(lb.clone());
                } else if expr_contains_await(value) {
                    // Await nested inside a complex expression — bail.
                    return None;
                } else {
                    // Pre-await straight-line let. v1 doesn't allow
                    // statements before the first await (locals
                    // would need crossing-suspend analysis to know
                    // whether to hoist). Defer.
                    return None;
                }
            }
            Statement::Expression(e) => {
                if expr_contains_await(e) {
                    // Treating a bare `expr.await` as the last
                    // suspension is feasible but uncommon; v1 only
                    // accepts the let-bound form. Bail.
                    return None;
                }
                // First non-await statement marks the start of the
                // tail.
                in_tail = true;
                tail.push(stmt.clone());
            }
        }
    }

    // If the body ended with an await let and no tail stmt, the
    // bound value's identifier is the implicit tail expression. Add
    // a `self.<binding>` reference so state N's Ready arm has
    // something to return.
    if tail.is_empty() {
        if let Some(last) = await_lets.last() {
            if let Pattern::Identifier { name, span, .. } = &last.pattern {
                tail.push(Statement::Expression(Expr {
                    kind: ExprKind::FieldAccess {
                        object: Box::new(Expr {
                            kind: ExprKind::SelfRef,
                            span: span.clone(),
                        }),
                        field: name.clone(),
                    },
                    span: span.clone(),
                }));
            }
        }
    }

    Some((await_lets, tail))
}

/// Describe a single `let x = g(args).await` await site.
fn describe_await(
    lb: &LetBinding,
    async_fn_returns: &HashMap<String, (String, TypeExpr)>,
    _outer_arg_names: &[String],
) -> Option<AwaitSub> {
    let binding_name = match &lb.pattern {
        Pattern::Identifier { name, .. } => name.clone(),
        _ => return None,
    };
    let value = lb.value.as_ref()?;
    let inner = match &value.kind {
        ExprKind::Await(inner) => inner,
        _ => return None,
    };
    // Awaitee must be a direct `name(args)` call on a known top-level
    // async fn.
    let (callee_name, args) = match &inner.kind {
        ExprKind::Call { callee, args, .. } => match &callee.kind {
            ExprKind::Identifier(n) => (n.clone(), args.clone()),
            _ => return None,
        },
        _ => return None,
    };
    let (sub_class_name, result_type) = async_fn_returns.get(&callee_name)?.clone();
    Some(AwaitSub {
        binding_name,
        awaitee_fn_name: callee_name,
        awaitee_args: args,
        sub_class_name,
        result_type,
    })
}

/// Build the multi-state poll body for a 2B lowering.
fn build_multi_state_poll_body(
    subs: &[AwaitSub],
    tail_stmts: &[Statement],
    outer_arg_names: &[String],
    _return_ty: &TypeExpr,
    span: &Span,
) -> Block {
    // Build the chain of `if self.__state == 0 ... elsif self.__state == 1 ... else`.
    //
    // Start from the last state and fold backwards into nested IfExpr.
    // - State i (0 <= i < N): `match (&var self.__sub_i).poll(cx) { Pending -> Pending ; Ready(v) -> self.x_i = v; self.__state = i+1; <next_action> }`.
    // - State N (terminal): `Poll.Ready(<tail expression>)`.
    // - Default (else): `Poll.Pending` (poll-after-Ready returns Pending forever, per spec B5).

    let n = subs.len();

    // Build the tail expression. The tail is a block of statements;
    // promote all references to outer args to `self.<arg>` and all
    // references to await bindings to `self.<binding>` (since those
    // are class fields now).
    let mut tail_block = Block {
        statements: tail_stmts.to_vec(),
        span: span.clone(),
    };
    let binding_names: Vec<String> = subs.iter().map(|s| s.binding_name.clone()).collect();
    let mut all_field_names: Vec<String> = outer_arg_names.to_vec();
    all_field_names.extend(binding_names.iter().cloned());
    rewrite_arg_refs_in_block(&mut tail_block, &all_field_names);
    let tail_expr = Expr {
        kind: ExprKind::Block(tail_block),
        span: span.clone(),
    };

    let terminal_state_idx = n as i64;
    let _ = terminal_state_idx; // kept for readability

    // Final terminal arm: Poll.Ready(tail).
    let terminal_ready = Expr {
        kind: ExprKind::EnumVariant {
            type_path: vec!["Poll".to_string()],
            variant: "Ready".to_string(),
            args: vec![FieldArg {
                name: None,
                value: tail_expr,
                span: span.clone(),
            }],
        },
        span: span.clone(),
    };

    // For each state i (last → first), build the arm body.
    //
    //   match (&var self.__sub_i).poll(cx)
    //     Poll.Ready(v) -> <ready_action_i>
    //     Poll.Pending -> Poll.Pending
    //   end
    //
    // ready_action_i for state i < N-1:
    //   self.<binding_i> = v
    //   self.__state = i+1
    //   Poll.Pending
    //
    // ready_action for state i == N-1 (last await):
    //   self.<binding_{N-1}> = v
    //   self.__state = N
    //   <terminal_ready>
    //
    // The `if/elsif/else` chain then dispatches on `self.__state`.

    // Build state arms back-to-front. Each arm produces an Expr.
    let mut state_arms: Vec<Expr> = Vec::with_capacity(n);
    for (i, sub) in subs.iter().enumerate() {
        let is_last = i + 1 == n;
        // ready arm body
        let assign_local = Expr {
            kind: ExprKind::Assign {
                target: Box::new(self_field(&sub.binding_name, span)),
                value: Box::new(Expr {
                    kind: ExprKind::Identifier("v".to_string()),
                    span: span.clone(),
                }),
            },
            span: span.clone(),
        };
        let bump_state = Expr {
            kind: ExprKind::Assign {
                target: Box::new(self_field("__state", span)),
                value: Box::new(Expr {
                    kind: ExprKind::IntLiteral((i + 1) as i64, None),
                    span: span.clone(),
                }),
            },
            span: span.clone(),
        };
        let trailer = if is_last {
            terminal_ready.clone()
        } else {
            // Poll.Pending (re-poll on next call)
            Expr {
                kind: ExprKind::EnumVariant {
                    type_path: vec!["Poll".to_string()],
                    variant: "Pending".to_string(),
                    args: Vec::new(),
                },
                span: span.clone(),
            }
        };
        let ready_block = Block {
            statements: vec![
                Statement::Expression(assign_local),
                Statement::Expression(bump_state),
                Statement::Expression(trailer),
            ],
            span: span.clone(),
        };

        // (&var self.__sub_i).poll(cx)
        let sub_field = self_field(&format!("__sub_{i}"), span);
        let sub_borrow = Expr {
            kind: ExprKind::BorrowMut(Box::new(sub_field)),
            span: span.clone(),
        };
        let poll_call = Expr {
            kind: ExprKind::MethodCall {
                object: Box::new(sub_borrow),
                method: "poll".to_string(),
                generic_args: Vec::new(),
                args: vec![Expr {
                    kind: ExprKind::Identifier("cx".to_string()),
                    span: span.clone(),
                }],
                block: None,
            },
            span: span.clone(),
        };

        let poll_match = Expr {
            kind: ExprKind::Match(MatchExpr {
                subject: Box::new(poll_call),
                arms: vec![
                    MatchArm {
                        pattern: Pattern::Enum {
                            path: vec!["Poll".to_string()],
                            variant: "Ready".to_string(),
                            fields: vec![Pattern::Identifier {
                                mutable: false,
                                name: "v".to_string(),
                                span: span.clone(),
                            }],
                            span: span.clone(),
                        },
                        guard: None,
                        body: MatchArmBody::Block(ready_block),
                        span: span.clone(),
                    },
                    MatchArm {
                        pattern: Pattern::Enum {
                            path: vec!["Poll".to_string()],
                            variant: "Pending".to_string(),
                            fields: vec![],
                            span: span.clone(),
                        },
                        guard: None,
                        body: MatchArmBody::Expr(Expr {
                            kind: ExprKind::EnumVariant {
                                type_path: vec!["Poll".to_string()],
                                variant: "Pending".to_string(),
                                args: Vec::new(),
                            },
                            span: span.clone(),
                        }),
                        span: span.clone(),
                    },
                ],
                span: span.clone(),
            }),
            span: span.clone(),
        };

        state_arms.push(poll_match);
    }

    // Build the if/elsif/else dispatch on self.__state.
    //
    //   if self.__state == 0
    //     state_arms[0]
    //   elsif self.__state == 1
    //     state_arms[1]
    //   ...
    //   else
    //     Poll.Pending
    //   end
    let else_pending = Expr {
        kind: ExprKind::EnumVariant {
            type_path: vec!["Poll".to_string()],
            variant: "Pending".to_string(),
            args: Vec::new(),
        },
        span: span.clone(),
    };

    let make_eq = |i: i64| Expr {
        kind: ExprKind::BinaryOp {
            left: Box::new(self_field("__state", span)),
            op: BinOp::Eq,
            right: Box::new(Expr {
                kind: ExprKind::IntLiteral(i, None),
                span: span.clone(),
            }),
        },
        span: span.clone(),
    };

    let to_block = |e: Expr| Block {
        statements: vec![Statement::Expression(e)],
        span: span.clone(),
    };

    // First arm: if __state == 0
    let then_body = to_block(state_arms[0].clone());

    // elsif clauses for states 1..n-1
    let mut elsif_clauses: Vec<ElsifClause> = Vec::new();
    for i in 1..n {
        elsif_clauses.push(ElsifClause {
            condition: Box::new(make_eq(i as i64)),
            body: to_block(state_arms[i].clone()),
            span: span.clone(),
        });
    }

    let if_expr = Expr {
        kind: ExprKind::If(IfExpr {
            condition: Box::new(make_eq(0)),
            then_body,
            elsif_clauses,
            else_body: Some(to_block(else_pending)),
            span: span.clone(),
        }),
        span: span.clone(),
    };

    Block {
        statements: vec![Statement::Expression(if_expr)],
        span: span.clone(),
    }
}

/// Return a default-value Expr for a type expression, suitable for
/// placeholder-initialising a hoisted local in `init`. v1 supports
/// numeric and Bool defaults only; other types cause the lowering to
/// bail (the user's await-result type must support a placeholder
/// today — the proper fix is `Option[T]`-wrapping in v2).
fn default_value_for_type(ty: &TypeExpr, span: &Span) -> Option<Expr> {
    if let TypeExpr::Named(path) = ty {
        if path.segments.len() == 1 {
            return match path.segments[0].as_str() {
                "Int" | "I8" | "I16" | "I32" | "I64" | "U8" | "U16" | "U32" | "U64" | "USize"
                | "ISize" => Some(Expr {
                    kind: ExprKind::IntLiteral(0, None),
                    span: span.clone(),
                }),
                "Bool" => Some(Expr {
                    kind: ExprKind::BoolLiteral(false),
                    span: span.clone(),
                }),
                "Float" | "F32" | "F64" => Some(Expr {
                    kind: ExprKind::FloatLiteral(0.0, None),
                    span: span.clone(),
                }),
                _ => None,
            };
        }
    }
    None
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
