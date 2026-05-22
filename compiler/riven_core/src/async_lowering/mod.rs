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
///
/// Convenience wrapper for callers that have no bootstrap context
/// (today: a handful of unit tests). Production callers in
/// [`crate::typeck`] use [`lower_async_defs_with_bootstrap`] so
/// `Class.method(args).await` shapes can resolve awaitee classes
/// declared in `library/std/*` (e.g. `TimeSleepFuture`,
/// `TaskJoinFuture`).
pub fn lower_async_defs(program: &mut Program) {
    lower_async_defs_with_bootstrap(program, &[]);
}

/// Bootstrap-aware variant of [`lower_async_defs`]. The lowering's
/// awaitee classifier (`describe_await`) walks `program` plus every
/// `bootstrap_programs` entry to populate its `class_static_returns`
/// / `future_outputs` tables. Without this, every
/// `<StdlibClass>.method(args).await` form fails to desugar — the
/// stdlib classes live in separately-parsed bootstrap `Program`s,
/// not in `program.items`.
///
/// Closes the gap documented in
/// `project_riven_async_compiler_gaps.md` (#2) and unblocks the two
/// deferred pins (731_class_static_call_await, task_join_await).
pub fn lower_async_defs_with_bootstrap(program: &mut Program, bootstrap_programs: &[&Program]) {
    // Build a map of `fn_name -> (synth_class_name, declared_return_type)` so
    // a 2B await on `g()` can name the sub-future field type as
    // `__GFuture` and the post-Ready local's type as the user's
    // declared return on `g`. The map covers only top-level async
    // free fns (which is all 2B supports — async methods deferred).
    //
    // The async-fn-returns map is intentionally USER-PROGRAM-ONLY:
    // bootstrap stdlib defines no async free fns today (the stdlib
    // ships explicit Future classes instead), and even if it did, a
    // user-program `let x = stdlib_async_fn().await` site would still
    // resolve via shape 2 (class-static-method call returning the
    // synth Future class) once the stdlib's async-fn-wrapper was
    // already lowered in its own translation unit.
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

    // Task #21 + follow-up: class-static-method-call awaitee support
    // (`Class.method(args).await`). We collect two AST-time tables:
    //
    //   1. class_static_returns: (ClassName, MethodName) -> declared return type.
    //      Walks each top-level class's `methods` and `lib_decls`.
    //      Filters to STATIC (class-level) methods only — instance
    //      methods are not supported as awaitee receivers in this
    //      milestone (the receiver's class is not known at
    //      AST-rewrite time without typeck).
    //
    //   2. future_outputs: FutureClassName -> Output type. Walks each
    //      class's `methods` for a `poll(cx) -> Poll[T]` method
    //      (which is the canonical Future-mixin marker). The `T` is
    //      the future's Output. Used to type the hoisted
    //      `<binding>` field for method-call awaitees.
    //
    // Both walks visit the user program AND each bootstrap stdlib
    // program so e.g. `Async.sleep(d).await` resolves to
    // `TimeSleepFuture` (library/std/future/src/lib.rvn).
    let mut class_static_returns: HashMap<(String, String), TypeExpr> = HashMap::new();
    let mut future_outputs: HashMap<String, TypeExpr> = HashMap::new();
    collect_class_static_returns_into(program, &mut class_static_returns);
    collect_future_outputs_into(program, &mut future_outputs);
    for bp in bootstrap_programs {
        collect_class_static_returns_into(bp, &mut class_static_returns);
        collect_future_outputs_into(bp, &mut future_outputs);
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
                if let Some((rewritten, sm_class)) = lower_one_async_fn_with_await(
                    func,
                    &async_fn_returns,
                    &class_static_returns,
                    &future_outputs,
                ) {
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

    // Sub-phase 3 (docs/specs/stdlib/executor.spec.md): rewrite every
    // `block_on(EXPR)` call site into an inline poll loop. The
    // rewriter runs AFTER the async-fn rewrite above so block_on
    // calls inside the original async fn bodies (now sync wrappers
    // returning their state-machine class) are still visible —
    // E1112 forbids that case at resolve time, but if a future
    // version permits it (or if E1112 is deferred) the rewriter
    // produces a working poll loop either way.
    rewrite_block_on_calls(program);
}

/// Populate the `(ClassName, MethodName) -> ReturnType` map with
/// every top-level class's STATIC methods (`def self.X` form) from
/// `program`. Covers both hand-written `methods` and `lib_decls`
/// (FFI shells). Used by `describe_await` to recognise
/// `Class.method(args).await` awaitees — task #21.
///
/// Instance methods are intentionally NOT collected here: the awaitee
/// receiver for an instance call is a value expression whose static
/// type is unknown at this pre-resolve AST pass. Supporting
/// `obj.method().await` requires either deferring .await desugar to
/// post-typeck or annotating the receiver — deferred to a follow-up.
///
/// `into` is appended to (not replaced) so the caller can union the
/// user program with bootstrap-loaded stdlib programs in a single
/// table. Last write wins on a key collision — order callers from
/// least- to most-authoritative.
fn collect_class_static_returns_into(
    program: &Program,
    into: &mut HashMap<(String, String), TypeExpr>,
) {
    for item in &program.items {
        let class = match item {
            TopLevelItem::Class(c) => c,
            _ => continue,
        };
        // Hand-written class methods declared as `def self.X(...) -> T`.
        for m in &class.methods {
            if !m.is_class_method {
                continue;
            }
            if let Some(ret) = &m.return_type {
                into.insert((class.name.clone(), m.name.clone()), ret.clone());
            }
        }
        // FFI shells declared inside `lib "..." ... end` blocks in the
        // class body. `def self.X as "..."(...) -> T` has
        // `FfiFunction.is_class_method == true`.
        for lib in &class.lib_decls {
            for f in &lib.functions {
                if !f.is_class_method {
                    continue;
                }
                if let Some(ret) = &f.return_type {
                    into.insert((class.name.clone(), f.name.clone()), ret.clone());
                }
            }
        }
    }
}

/// Populate the `FutureClassName -> OutputType` map from `program`.
///
/// A class is a Future when it includes the `Future` mixin and
/// declares a `def var poll(cx: &var Context) -> Poll[T]` method.
/// The Output is the `T` parameter in `Poll[T]`. We scan the class's
/// `methods` for `poll` and extract the single generic argument from
/// its return type.
///
/// Note: this misses classes that declare `type Output = X` at the
/// top-level class body (the parser currently discards that
/// declaration — see `parser/classes.rs::parse_class_def`'s
/// `TokenKind::Type` arm) AND don't have an explicit `poll` body in
/// the same file. In practice every Future class today (user and
/// stdlib) declares poll with a concrete `Poll[T]` return so this is
/// sufficient for v1.
///
/// `into` is appended to — see `collect_class_static_returns_into`
/// for the union-walk rationale.
fn collect_future_outputs_into(program: &Program, into: &mut HashMap<String, TypeExpr>) {
    for item in &program.items {
        let class = match item {
            TopLevelItem::Class(c) => c,
            _ => continue,
        };
        for m in &class.methods {
            if m.name != "poll" || m.is_class_method {
                continue;
            }
            let ret = match &m.return_type {
                Some(r) => r,
                None => continue,
            };
            // Extract T from Poll[T].
            if let Some(inner) = extract_poll_output(ret) {
                into.insert(class.name.clone(), inner);
                break;
            }
        }
    }
}

/// If `ty` is `Poll[T]`, return `T`. Otherwise return `None`.
fn extract_poll_output(ty: &TypeExpr) -> Option<TypeExpr> {
    let path = match ty {
        TypeExpr::Named(p) => p,
        _ => return None,
    };
    if path.segments.len() != 1 || path.segments[0] != "Poll" {
        return None;
    }
    let args = path.generic_args.as_ref()?;
    if args.len() != 1 {
        return None;
    }
    Some(args[0].clone())
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
    class_static_returns: &HashMap<(String, String), TypeExpr>,
    future_outputs: &HashMap<String, TypeExpr>,
) -> Option<(FuncDef, ClassDef)> {
    let span = func.span.clone();
    let class_name = mangle_future_class_name(&func.name);

    let return_type = func.return_type.clone()?;

    // ── Phase 1 — segment the body into `pre_await | [let x = g().await]* | tail`.
    //
    // The segmenter accepts side-effect-free pre-await statements
    // (Milestone-2B extension — closes the gap noted in
    // `project_riven_milestone_2b_segmenter_constraints.md`). Each
    // pre-await Statement::Let either becomes a pure init-local (if
    // it's never read after the first `.await`) or a state-machine
    // field (if it IS read in `tail`).
    let segments = segment_body(&func.body)?;
    let Segments {
        pre_await: pre_await_stmts,
        await_lets,
        tail: tail_stmts,
    } = segments;
    if await_lets.is_empty() {
        // `block_contains_await` saw an await but the segmenter
        // didn't — the await sits in an unsupported shape (e.g.
        // inside an if-arm or a loop). Bail; the resolver-side
        // E1110 / E1115 checks will surface a diagnostic.
        return None;
    }

    // ── Phase 1b — classify pre-await locals (crossing vs non-crossing).
    //
    // A pre-await `let <name> = ...` becomes a state-machine field IFF
    // `<name>` is read in `tail_stmts` (i.e., AFTER the last await).
    // Such locals must carry an explicit type annotation so the
    // lowering can declare the field type without typeck integration;
    // we bail on un-annotated crossing locals (the user can either
    // annotate the let or restructure the body — the resolver-side
    // diagnostic for the un-lowered body will surface).
    //
    // Non-crossing locals stay as plain `Statement::Let`s inside the
    // `init` body; the awaitee constructor in `describe_await` runs
    // BEFORE the suspend (eager-init), so any awaitee args that
    // reference them resolve to the in-scope init-local without
    // hoisting.
    let pre_await_let_names = collect_let_names(&pre_await_stmts);
    let mut crossing_locals: Vec<(String, TypeExpr)> = Vec::new();
    for s in &pre_await_stmts {
        if let Statement::Let(lb) = s {
            if let Pattern::Identifier { name, .. } = &lb.pattern {
                if stmts_reference_name(&tail_stmts, name) {
                    let ty = lb.type_annotation.clone()?;
                    crossing_locals.push((name.clone(), ty));
                }
            }
        }
    }
    let crossing_names: Vec<String> = crossing_locals.iter().map(|(n, _)| n.clone()).collect();

    // ── Phase 2 — for each await, locate the awaited fn's synth class.
    //
    // The `outer_field_names` set drives `describe_await`'s arg-ref
    // rewrite: bare references to these names inside awaitee args get
    // promoted to `self.<name>`. The set unions outer fn params with
    // crossing pre-await locals (both end up as `self.*` fields), but
    // NOT non-crossing pre-await locals (those stay init-locals and
    // resolve naturally inside the init scope where the awaitee ctor
    // is later assigned via `self.__sub_i = <ctor>`).
    let outer_arg_names: Vec<String> = func.params.iter().map(|p| p.name.clone()).collect();
    let mut outer_field_names: Vec<String> = outer_arg_names.clone();
    outer_field_names.extend(crossing_names.iter().cloned());
    let mut subs: Vec<AwaitSub> = Vec::new();
    for al in &await_lets {
        let sub = describe_await(
            al,
            async_fn_returns,
            class_static_returns,
            future_outputs,
            &outer_field_names,
        )?;
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
    // Hoisted pre-await locals (those referenced in tail). Field name
    // matches the source `let` so the tail-block rewrite picks them up
    // via the `outer_field_names + binding_names` union pass.
    for (name, ty) in &crossing_locals {
        fields.push(FieldDecl {
            visibility: Visibility::Public,
            name: name.clone(),
            type_expr: ty.clone(),
            span: span.clone(),
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
    // Init takes `@__state` + each outer arg. Inside the body we:
    //   1. Run the user's pre-await statements verbatim, with outer-
    //      arg refs rewritten to `self.<arg>`. Statement::Let bindings
    //      defined here are init-scope locals. Side-effecting RHS
    //      (function calls, allocations) runs eagerly as part of
    //      construction — see B5 future work for moving sub-future
    //      construction to per-state.
    //   2. For each crossing local, copy the init-local into the
    //      state-machine field (`self.<name> = <name>`) so the tail's
    //      `self.<name>` reads have a value.
    //   3. Construct each sub-future eagerly. Awaitee args referencing
    //      crossing locals or outer args are already rewritten to
    //      `self.*` by `describe_await`; awaitee args referencing
    //      non-crossing pre-await locals resolve to those init-locals
    //      directly.
    //   4. Default-initialise each await-binding field so the type
    //      checker sees a value at every read site.
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
    // (1) Pre-await statements (with arg refs → self.<arg>).
    for s in &pre_await_stmts {
        let mut rewritten = s.clone();
        match &mut rewritten {
            Statement::Let(lb) => {
                if let Some(v) = lb.value.as_mut() {
                    rewrite_arg_refs_in_expr(v, &outer_arg_names);
                }
            }
            Statement::Expression(e) => {
                rewrite_arg_refs_in_expr(e, &outer_arg_names);
            }
        }
        init_body_stmts.push(rewritten);
    }
    // (2) Copy crossing locals into their state-machine fields.
    for name in &crossing_names {
        init_body_stmts.push(Statement::Expression(Expr {
            kind: ExprKind::Assign {
                target: Box::new(self_field(name, &span)),
                value: Box::new(Expr {
                    kind: ExprKind::Identifier(name.clone()),
                    span: span.clone(),
                }),
            },
            span: span.clone(),
        }));
    }
    // (3) Sub-future eager construction.
    let _ = pre_await_let_names; // reserved for future shadow-checking
    for (i, sub) in subs.iter().enumerate() {
        init_body_stmts.push(Statement::Expression(Expr {
            kind: ExprKind::Assign {
                target: Box::new(self_field(&format!("__sub_{i}"), &span)),
                value: Box::new(sub.awaitee_ctor.clone()),
            },
            span: span.clone(),
        }));
    }
    // (4) Default values for await bindings.
    for sub in &subs {
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
    //
    // Pass `outer_field_names` (outer params ∪ crossing pre-await
    // locals) so the tail-block rewrite promotes references to either
    // category into `self.<name>` reads. Non-crossing pre-await
    // locals are not in the set — they never appear in the tail by
    // definition (that's the "crossing" criterion), so leaving them
    // out is correct and avoids accidentally rewriting unrelated
    // shadowed identifiers.
    let poll_body =
        build_multi_state_poll_body(&subs, &tail_stmts, &outer_field_names, &return_type, &span);

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
    /// Pre-built constructor expression for the sub-future. The init
    /// body emits `self.__sub_i = <awaitee_ctor>` literally — outer
    /// async-fn arg refs have already been rewritten to `self.<arg>`
    /// in `describe_await`.
    ///
    /// For a free-fn awaitee `g(args).await`, this is
    /// `__GFuture.new(0, args_rewritten...)` — built by `describe_await`
    /// to bypass `g`'s wrapper (`g` post-lowering returns a fresh
    /// `__GFuture` too, but the direct `.new(0, …)` avoids the extra
    /// call).
    ///
    /// For a class-static-method-call awaitee `Class.method(args).await`,
    /// this is the user-written `Class.method(args_rewritten...)`
    /// expression preserved verbatim. The method itself constructs the
    /// future class (e.g. `Async.sleep(d)` returns `TimeSleepFuture.new(d)`).
    awaitee_ctor: Expr,
    /// Future class name (`__GFuture` for free-fn awaitees;
    /// `TimeSleepFuture`, `AsyncReadToStringFuture`, etc. for
    /// class-static-method-call awaitees). Used to type the
    /// `__sub_i` field.
    sub_class_name: String,
    /// Future's Output type — the value bound by the await. For a
    /// free-fn awaitee `async def g() -> T`, this is `T`. For a
    /// class-static awaitee whose method returns `<FutClass>`, this
    /// is the Output associated with `<FutClass>` (read from its
    /// `poll(...) -> Poll[T]` method signature).
    result_type: TypeExpr,
}

/// Output of [`segment_body`] — a Milestone-2B-shaped async-fn body
/// split into three sequential regions:
///
///   * `pre_await` — straight-line statements that execute BEFORE the
///     first `.await`. None of them may contain a nested `.await`.
///     These run inside the state-machine's `init` body verbatim
///     (with outer-arg refs rewritten to `self.<arg>`); any local
///     declared here that is read after the first `.await` (i.e.,
///     referenced in `tail`) is additionally hoisted to a field by
///     [`lower_one_async_fn_with_await`].
///   * `await_lets` — one entry per `.await` suspension point, in
///     source order. Each is a `let <binding> = <awaitee>.await`.
///     Bindings ALWAYS become state-machine fields (existing 2B
///     behaviour — every await result survives the next suspend).
///   * `tail` — the post-last-await statements. References to outer
///     args, await-bindings, and crossing pre-await locals are
///     rewritten to `self.<name>` by the caller.
struct Segments {
    pre_await: Vec<Statement>,
    await_lets: Vec<LetBinding>,
    tail: Vec<Statement>,
}

/// Split an async fn body into `pre_await | [await let-binding]* | tail`.
///
/// Returns `None` if the body uses an unsupported shape:
///   * a pre-await statement whose RHS contains a nested `.await`
///     (the body must reach the first suspend through straight-line
///     statements only — branched/looped `.await`s are E1115 / B11),
///   * an await-let whose RHS isn't a bare `<expr>.await` (await
///     nested deeper in an expression),
///   * an await-let whose pattern isn't a plain `Identifier`
///     (destructuring on await results is deferred),
///   * a bare `expr.await` statement at the top level (only let-
///     bound awaits are supported in v1).
///
/// Milestone-2B (commit f899788) accepted ONLY the
/// `[await let-binding]* tail` shape — no pre-await statements at all.
/// This extension lifts that restriction so the natural server-handler
/// shape (`let parsed = parse(req); let r = service.call(parsed).await; reply(r)`)
/// lowers without a workaround.
fn segment_body(body: &Block) -> Option<Segments> {
    let mut pre_await: Vec<Statement> = Vec::new();
    let mut await_lets: Vec<LetBinding> = Vec::new();
    let mut tail: Vec<Statement> = Vec::new();
    // Phase tracking: 0 = collecting pre-await prefix, 1 = collecting
    // await-let chain, 2 = collecting post-last-await tail.
    //
    // The transitions are one-way: once we see the first `.await` we
    // can no longer accept pre-await statements; once we see a non-
    // await statement after an await-let we move to the tail and
    // forbid further awaits.
    let mut seen_first_await = false;
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
                let value = lb.value.as_ref()?;
                if let ExprKind::Await(_) = &value.kind {
                    // OK — this is `let x = <expr>.await`. Pattern
                    // must be a plain Identifier binding (no
                    // destructuring) for v1.
                    if !matches!(&lb.pattern, Pattern::Identifier { .. }) {
                        return None;
                    }
                    await_lets.push(lb.clone());
                    seen_first_await = true;
                } else if expr_contains_await(value) {
                    // Await nested inside a complex expression — bail.
                    return None;
                } else if !seen_first_await {
                    // Pre-await straight-line let. Side-effecting RHS
                    // is fine (it runs in `init`); the type system
                    // and effects checker handle whatever safety
                    // story applies post-resolve.
                    pre_await.push(stmt.clone());
                } else {
                    // First non-await statement AFTER at least one
                    // await marks the start of the tail.
                    in_tail = true;
                    tail.push(stmt.clone());
                }
            }
            Statement::Expression(e) => {
                if expr_contains_await(e) {
                    // Treating a bare `expr.await` as the last
                    // suspension is feasible but uncommon; v1 only
                    // accepts the let-bound form. Bail.
                    return None;
                }
                if !seen_first_await {
                    // Pre-await expression statement (e.g. a side-
                    // effecting setup call). Runs in `init` verbatim.
                    pre_await.push(stmt.clone());
                } else {
                    // First non-await statement marks the start of
                    // the tail.
                    in_tail = true;
                    tail.push(stmt.clone());
                }
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

    Some(Segments {
        pre_await,
        await_lets,
        tail,
    })
}

/// Names introduced by `let <pat> = ...` statements in `stmts`,
/// considering only plain `Pattern::Identifier` bindings (the
/// segmenter rejects destructuring on the await branch but pre-await
/// lets COULD destructure — for those we leave the name set empty so
/// the caller treats them as pure init-locals with no field hoisting).
fn collect_let_names(stmts: &[Statement]) -> Vec<String> {
    let mut names = Vec::new();
    for s in stmts {
        if let Statement::Let(lb) = s {
            if let Pattern::Identifier { name, .. } = &lb.pattern {
                names.push(name.clone());
            }
        }
    }
    names
}

/// Returns `true` if any subtree of `expr` reads the bare identifier
/// `name`. Conservative: doesn't follow shadowing inside nested blocks
/// (matches the segmenter's name-set rewrites, which already assume
/// shadow-free name usage at this AST pass). Closures' bodies are
/// scanned because a captured pre-await local would still need to be
/// readable when the closure runs — same conservatism as the existing
/// `rewrite_arg_refs_in_expr`.
fn expr_references_name(expr: &Expr, name: &str) -> bool {
    if let ExprKind::Identifier(n) = &expr.kind {
        return n == name;
    }
    match &expr.kind {
        ExprKind::BinaryOp { left, right, .. } => {
            expr_references_name(left, name) || expr_references_name(right, name)
        }
        ExprKind::UnaryOp { operand, .. } => expr_references_name(operand, name),
        ExprKind::Borrow(inner)
        | ExprKind::BorrowMut(inner)
        | ExprKind::Try(inner)
        | ExprKind::Await(inner) => expr_references_name(inner, name),
        ExprKind::FieldAccess { object, .. } => expr_references_name(object, name),
        ExprKind::MethodCall { object, args, .. } => {
            expr_references_name(object, name) || args.iter().any(|a| expr_references_name(a, name))
        }
        ExprKind::Call { callee, args, .. } | ExprKind::ClosureCall { callee, args } => {
            expr_references_name(callee, name) || args.iter().any(|a| expr_references_name(a, name))
        }
        ExprKind::Index { object, index } => {
            expr_references_name(object, name) || expr_references_name(index, name)
        }
        ExprKind::Assign { target, value } | ExprKind::CompoundAssign { target, value, .. } => {
            expr_references_name(target, name) || expr_references_name(value, name)
        }
        ExprKind::If(IfExpr {
            condition,
            then_body,
            elsif_clauses,
            else_body,
            ..
        }) => {
            expr_references_name(condition, name)
                || block_references_name(then_body, name)
                || elsif_clauses.iter().any(|el| {
                    expr_references_name(&el.condition, name)
                        || block_references_name(&el.body, name)
                })
                || else_body
                    .as_ref()
                    .is_some_and(|b| block_references_name(b, name))
        }
        ExprKind::Match(MatchExpr { subject, arms, .. }) => {
            expr_references_name(subject, name)
                || arms.iter().any(|a| {
                    a.guard
                        .as_ref()
                        .is_some_and(|g| expr_references_name(g, name))
                        || match &a.body {
                            MatchArmBody::Expr(e) => expr_references_name(e, name),
                            MatchArmBody::Block(b) => block_references_name(b, name),
                        }
                })
        }
        ExprKind::Block(b) => block_references_name(b, name),
        ExprKind::Return(Some(inner)) | ExprKind::Break(Some(inner)) => {
            expr_references_name(inner, name)
        }
        ExprKind::ArrayLiteral(items) | ExprKind::TupleLiteral(items) => {
            items.iter().any(|e| expr_references_name(e, name))
        }
        ExprKind::ArrayFill { value, count } => {
            expr_references_name(value, name) || expr_references_name(count, name)
        }
        ExprKind::MapLiteral(pairs) => pairs
            .iter()
            .any(|(k, v)| expr_references_name(k, name) || expr_references_name(v, name)),
        ExprKind::Range { start, end, .. } => {
            start
                .as_ref()
                .is_some_and(|s| expr_references_name(s, name))
                || end.as_ref().is_some_and(|e| expr_references_name(e, name))
        }
        ExprKind::Cast { expr: inner, .. } => expr_references_name(inner, name),
        ExprKind::Closure(c) => match &c.body {
            ClosureBody::Expr(e) => expr_references_name(e, name),
            ClosureBody::Block(b) => block_references_name(b, name),
        },
        _ => false,
    }
}

fn block_references_name(block: &Block, name: &str) -> bool {
    block.statements.iter().any(|s| match s {
        Statement::Let(lb) => lb
            .value
            .as_ref()
            .is_some_and(|v| expr_references_name(v, name)),
        Statement::Expression(e) => expr_references_name(e, name),
    })
}

fn stmts_reference_name(stmts: &[Statement], name: &str) -> bool {
    stmts.iter().any(|s| match s {
        Statement::Let(lb) => lb
            .value
            .as_ref()
            .is_some_and(|v| expr_references_name(v, name)),
        Statement::Expression(e) => expr_references_name(e, name),
    })
}

/// Describe a single `let x = <awaitee>.await` await site.
///
/// Supported awaitee shapes:
///
///   1. `name(args)` — direct call of a known top-level async free fn.
///      The compiler has already lowered `name` into a sync wrapper
///      that returns `__NameFuture`; this branch bypasses the wrapper
///      and emits `__NameFuture.new(0, args)` directly. Output type
///      comes from the user's declared return on `name`.
///
///   2. `Class.method(args)` — class-static-method call where
///      `Class` is the AST identifier of a class declared in this
///      translation unit, `method` is one of its `def self.X` /
///      `def self.X as "..."` decls, and `method`'s declared return
///      type is a named Future class (i.e., a class with a
///      `poll(cx) -> Poll[T]` method) — Task #21. The awaitee
///      expression is preserved verbatim as the init-body
///      constructor; Output type comes from the Future class's
///      `Poll[T]`.
///
/// Instance-method-call awaitees (`obj.method().await`) are NOT
/// supported in this milestone — the receiver's static type is not
/// known at this pre-resolve pass. Deferred to a follow-up that
/// either annotates the receiver or re-runs the awaitee classification
/// after typeck.
///
/// Returns `None` if the awaitee shape doesn't match either supported
/// pattern. Downstream `lower_one_async_fn_with_await` returns `None`
/// for the whole function in that case, and the resolver/typeck pass
/// surfaces a follow-up diagnostic.
fn describe_await(
    lb: &LetBinding,
    async_fn_returns: &HashMap<String, (String, TypeExpr)>,
    class_static_returns: &HashMap<(String, String), TypeExpr>,
    future_outputs: &HashMap<String, TypeExpr>,
    outer_arg_names: &[String],
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
    let span = inner.span.clone();

    // Shape 1 — free-fn call: `name(args)`.
    if let ExprKind::Call { callee, args, .. } = &inner.kind {
        if let ExprKind::Identifier(callee_name) = &callee.kind {
            if let Some((sub_class_name, result_type)) = async_fn_returns.get(callee_name).cloned()
            {
                let mut ctor_args: Vec<Expr> = Vec::with_capacity(args.len() + 1);
                ctor_args.push(Expr {
                    kind: ExprKind::IntLiteral(0, None),
                    span: span.clone(),
                });
                for a in args {
                    let mut rewritten = a.clone();
                    rewrite_arg_refs_in_expr(&mut rewritten, outer_arg_names);
                    ctor_args.push(rewritten);
                }
                let awaitee_ctor = Expr {
                    kind: ExprKind::MethodCall {
                        object: Box::new(Expr {
                            kind: ExprKind::Identifier(sub_class_name.clone()),
                            span: span.clone(),
                        }),
                        method: "new".to_string(),
                        generic_args: Vec::new(),
                        args: ctor_args,
                        block: None,
                    },
                    span: span.clone(),
                };
                return Some(AwaitSub {
                    binding_name,
                    awaitee_ctor,
                    sub_class_name,
                    result_type,
                });
            }
            return None;
        }
        return None;
    }

    // Shape 2 — class-static-method call: `Class.method(args)`.
    //
    // The parser folds `Class.method(args)` into
    // `ExprKind::MethodCall { object: Identifier("Class"), method, args }`.
    // We accept it as an awaitee when:
    //   - the receiver is a bare Identifier
    //   - `(Identifier, method)` resolves to a static-method return type
    //     in `class_static_returns`
    //   - the return type is `TypeExpr::Named(SimpleName)` whose name
    //     is a known Future class in `future_outputs`
    //
    // Generic args on the receiver or on the method are not supported
    // in this milestone — the awaitee resolution is name-based.
    if let ExprKind::MethodCall {
        object,
        method,
        generic_args,
        args,
        block,
    } = &inner.kind
    {
        if !generic_args.is_empty() || block.is_some() {
            return None;
        }
        let class_name = match &object.kind {
            ExprKind::Identifier(n) => n.clone(),
            _ => return None,
        };
        let ret_ty = class_static_returns.get(&(class_name.clone(), method.clone()))?;
        let future_class_name = match ret_ty {
            TypeExpr::Named(path) if path.segments.len() == 1 => path.segments[0].clone(),
            _ => return None,
        };
        let output_ty = future_outputs.get(&future_class_name)?.clone();

        // Preserve the original `Class.method(args)` expression as the
        // sub-future constructor; just rewrite outer arg refs inside
        // the args to `self.<arg>` references.
        let mut rewritten_args: Vec<Expr> = Vec::with_capacity(args.len());
        for a in args {
            let mut rewritten = a.clone();
            rewrite_arg_refs_in_expr(&mut rewritten, outer_arg_names);
            rewritten_args.push(rewritten);
        }
        let awaitee_ctor = Expr {
            kind: ExprKind::MethodCall {
                object: Box::new(Expr {
                    kind: ExprKind::Identifier(class_name.clone()),
                    span: span.clone(),
                }),
                method: method.clone(),
                generic_args: Vec::new(),
                args: rewritten_args,
                block: None,
            },
            span: span.clone(),
        };
        return Some(AwaitSub {
            binding_name,
            awaitee_ctor,
            sub_class_name: future_class_name,
            result_type: output_ty,
        });
    }

    None
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
    // Unit type — `()` written as a tuple of zero elements, OR as the
    // bare named type `Unit`. Task #21: enables awaitees whose Future
    // Output is `()` (e.g. TimeSleepFuture, AsyncWriteAllFuture's
    // analogues) to participate in `.await` lowering.
    if let TypeExpr::Tuple { elements, .. } = ty {
        if elements.is_empty() {
            return Some(Expr {
                kind: ExprKind::UnitLiteral,
                span: span.clone(),
            });
        }
    }
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
                "Unit" => Some(Expr {
                    kind: ExprKind::UnitLiteral,
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
fn build_poll_body(user_body: &Block, _return_ty: &TypeExpr, args: &[Param], span: &Span) -> Block {
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
        ExprKind::Try(inner) | ExprKind::Await(inner) => rewrite_arg_refs_in_expr(inner, arg_names),
        ExprKind::Assign { target, value } | ExprKind::CompoundAssign { target, value, .. } => {
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
        ExprKind::Closure(c) => match &mut c.body {
            ClosureBody::Expr(e) => rewrite_arg_refs_in_expr(e, arg_names),
            ClosureBody::Block(b) => rewrite_arg_refs_in_block(b, arg_names),
        },
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
                || elsif_clauses
                    .iter()
                    .any(|el| expr_contains_await(&el.condition) || block_contains_await(&el.body))
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
        ExprKind::Return(Some(inner)) | ExprKind::Break(Some(inner)) => expr_contains_await(inner),
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
        // Loop bodies count: an `.await` inside `loop { ... }`,
        // `while cond { ... }`, `while let pat = expr { ... }`, or
        // `for pat in iter { ... }` is still an await in this fn
        // and must trigger the await-aware lowering path. The
        // segmenter will bail on the actual loop shape (v1 doesn't
        // build per-iteration state machines), but the dedicated
        // E1115 pre-pass below surfaces a clean diagnostic — without
        // this branch, the scan would skip the loop body and the
        // function would fall into `lower_one_async_fn` (no-await
        // path) instead, which wraps the body in a single-state
        // Poll.Ready and leaves the `.await` inside it — producing
        // a misleading E1110 ("`.await` only valid inside async
        // def") downstream rather than the correct E1115.
        ExprKind::Loop(LoopExpr { body, .. }) => block_contains_await(body),
        ExprKind::While(WhileExpr {
            condition, body, ..
        }) => expr_contains_await(condition) || block_contains_await(body),
        ExprKind::WhileLet(WhileLetExpr { value, body, .. }) => {
            expr_contains_await(value) || block_contains_await(body)
        }
        ExprKind::For(ForExpr { iterable, body, .. }) => {
            expr_contains_await(iterable) || block_contains_await(body)
        }
        _ => false,
    }
}

/// Pre-pass E1115 (`.await` inside a loop body): walks every async
/// fn / async closure body and emits a diagnostic at each `.await`
/// site whose enclosing context is a `loop` / `while` / `while let` /
/// `for` body. v1 lowering doesn't build per-iteration state
/// machines (loop suspension requires re-constructing the awaitee
/// future on each iteration and re-entering the right state on
/// resume), so the shape is rejected.
///
/// Without this pre-pass the failure mode is opaque: the segmenter
/// bails (since a `loop` body isn't a valid `[await_let]* tail`
/// shape), the function stays un-lowered with `is_async = true`, and
/// the resolver eventually surfaces E1110 ("`.await` only valid
/// inside `async def` or `async { }`") even though we ARE inside an
/// async def — the misleading code blamed scope when the real cause
/// was an unsupported loop shape.
///
/// Spec: docs/errors/E1115.md. Deferred-to-v2 listing:
/// `docs/specs/types/async_lowering.spec.md` "Out of scope".
pub fn collect_await_in_loop_diagnostics(program: &Program) -> Vec<crate::diagnostics::Diagnostic> {
    let mut diags = Vec::new();
    for item in &program.items {
        collect_e1115_in_item(
            item, /*in_async=*/ false, /*in_loop=*/ false, &mut diags,
        );
    }
    diags
}

fn collect_e1115_in_item(
    item: &TopLevelItem,
    in_async: bool,
    in_loop: bool,
    diags: &mut Vec<crate::diagnostics::Diagnostic>,
) {
    match item {
        TopLevelItem::Function(func) => {
            let scope_async = in_async || func.is_async;
            // Crossing a fn boundary resets the loop context — a
            // closure-free nested async fn doesn't inherit loops
            // from its lexical surroundings (and at this point there
            // are no nested async fn defs anyway, only methods).
            collect_e1115_in_block(&func.body, scope_async, /*in_loop=*/ false, diags);
        }
        TopLevelItem::Class(class) => {
            for m in class.methods.iter() {
                let scope_async = in_async || m.is_async;
                collect_e1115_in_block(&m.body, scope_async, false, diags);
            }
            for inner_impl in class.inner_impls.iter() {
                for inner in inner_impl.items.iter() {
                    if let crate::parser::ast::ImplItem::Method(m) = inner {
                        let scope_async = in_async || m.is_async;
                        collect_e1115_in_block(&m.body, scope_async, false, diags);
                    }
                }
            }
        }
        TopLevelItem::Impl(impl_block) => {
            for inner in impl_block.items.iter() {
                if let crate::parser::ast::ImplItem::Method(m) = inner {
                    let scope_async = in_async || m.is_async;
                    collect_e1115_in_block(&m.body, scope_async, false, diags);
                }
            }
        }
        TopLevelItem::Module(module) => {
            for nested in module.items.iter() {
                collect_e1115_in_item(nested, in_async, in_loop, diags);
            }
        }
        _ => {}
    }
}

fn collect_e1115_in_block(
    block: &Block,
    in_async: bool,
    in_loop: bool,
    diags: &mut Vec<crate::diagnostics::Diagnostic>,
) {
    for stmt in &block.statements {
        match stmt {
            Statement::Let(let_binding) => {
                if let Some(v) = &let_binding.value {
                    collect_e1115_in_expr(v, in_async, in_loop, diags);
                }
            }
            Statement::Expression(e) => collect_e1115_in_expr(e, in_async, in_loop, diags),
        }
    }
}

fn collect_e1115_in_expr(
    expr: &Expr,
    in_async: bool,
    in_loop: bool,
    diags: &mut Vec<crate::diagnostics::Diagnostic>,
) {
    if let ExprKind::Await(_) = &expr.kind {
        if in_async && in_loop {
            diags.push(crate::diagnostics::Diagnostic::error_with_code(
                "`.await` inside a `loop` / `while` / `for` body is not yet \
                 supported; v1 lowering does not build per-iteration state \
                 machines. Hand-poll the future with `match fut.poll(&var cx)` \
                 inside the loop instead, or restructure to chained `.await`s \
                 outside the loop.",
                expr.span.clone(),
                "E1115",
            ));
            // Continue walking — multiple awaits in the same loop
            // each get their own diagnostic so the user sees them
            // all in one pass.
        }
    }

    match &expr.kind {
        ExprKind::BinaryOp { left, right, .. } => {
            collect_e1115_in_expr(left, in_async, in_loop, diags);
            collect_e1115_in_expr(right, in_async, in_loop, diags);
        }
        ExprKind::UnaryOp { operand, .. } => {
            collect_e1115_in_expr(operand, in_async, in_loop, diags)
        }
        ExprKind::Borrow(inner) | ExprKind::BorrowMut(inner) | ExprKind::Try(inner) => {
            collect_e1115_in_expr(inner, in_async, in_loop, diags)
        }
        ExprKind::Await(inner) => collect_e1115_in_expr(inner, in_async, in_loop, diags),
        ExprKind::FieldAccess { object, .. } => {
            collect_e1115_in_expr(object, in_async, in_loop, diags)
        }
        ExprKind::MethodCall { object, args, .. } => {
            collect_e1115_in_expr(object, in_async, in_loop, diags);
            for a in args {
                collect_e1115_in_expr(a, in_async, in_loop, diags);
            }
        }
        ExprKind::Call { callee, args, .. } | ExprKind::ClosureCall { callee, args } => {
            collect_e1115_in_expr(callee, in_async, in_loop, diags);
            for a in args {
                collect_e1115_in_expr(a, in_async, in_loop, diags);
            }
        }
        ExprKind::Assign { target, value } | ExprKind::CompoundAssign { target, value, .. } => {
            collect_e1115_in_expr(target, in_async, in_loop, diags);
            collect_e1115_in_expr(value, in_async, in_loop, diags);
        }
        ExprKind::If(IfExpr {
            condition,
            then_body,
            elsif_clauses,
            else_body,
            ..
        }) => {
            collect_e1115_in_expr(condition, in_async, in_loop, diags);
            collect_e1115_in_block(then_body, in_async, in_loop, diags);
            for el in elsif_clauses {
                collect_e1115_in_expr(&el.condition, in_async, in_loop, diags);
                collect_e1115_in_block(&el.body, in_async, in_loop, diags);
            }
            if let Some(b) = else_body {
                collect_e1115_in_block(b, in_async, in_loop, diags);
            }
        }
        ExprKind::Match(MatchExpr { subject, arms, .. }) => {
            collect_e1115_in_expr(subject, in_async, in_loop, diags);
            for a in arms {
                if let Some(g) = &a.guard {
                    collect_e1115_in_expr(g, in_async, in_loop, diags);
                }
                match &a.body {
                    MatchArmBody::Expr(e) => collect_e1115_in_expr(e, in_async, in_loop, diags),
                    MatchArmBody::Block(b) => collect_e1115_in_block(b, in_async, in_loop, diags),
                }
            }
        }
        ExprKind::Block(b) => collect_e1115_in_block(b, in_async, in_loop, diags),
        ExprKind::Loop(LoopExpr { body, .. }) => {
            // Entering a loop body sets in_loop=true for everything
            // nested inside (including nested if/match/etc.).
            collect_e1115_in_block(body, in_async, /*in_loop=*/ true, diags);
        }
        ExprKind::While(WhileExpr {
            condition, body, ..
        }) => {
            // The condition runs once per iteration but isn't itself
            // a loop body — an `.await` in the condition would still
            // be a per-iteration suspend point, so still in_loop.
            collect_e1115_in_expr(condition, in_async, /*in_loop=*/ true, diags);
            collect_e1115_in_block(body, in_async, true, diags);
        }
        ExprKind::WhileLet(WhileLetExpr { value, body, .. }) => {
            collect_e1115_in_expr(value, in_async, /*in_loop=*/ true, diags);
            collect_e1115_in_block(body, in_async, true, diags);
        }
        ExprKind::For(ForExpr { iterable, body, .. }) => {
            // The iterable is evaluated once OUTSIDE the loop, so
            // `.await` in iterable is NOT in_loop — it's a normal
            // pre-loop suspend that the segmenter can handle.
            collect_e1115_in_expr(iterable, in_async, in_loop, diags);
            collect_e1115_in_block(body, in_async, /*in_loop=*/ true, diags);
        }
        ExprKind::Return(Some(inner)) | ExprKind::Break(Some(inner)) => {
            collect_e1115_in_expr(inner, in_async, in_loop, diags)
        }
        ExprKind::ArrayLiteral(items) | ExprKind::TupleLiteral(items) => {
            for e in items {
                collect_e1115_in_expr(e, in_async, in_loop, diags);
            }
        }
        ExprKind::ArrayFill { value, count } => {
            collect_e1115_in_expr(value, in_async, in_loop, diags);
            collect_e1115_in_expr(count, in_async, in_loop, diags);
        }
        ExprKind::MapLiteral(pairs) => {
            for (k, v) in pairs {
                collect_e1115_in_expr(k, in_async, in_loop, diags);
                collect_e1115_in_expr(v, in_async, in_loop, diags);
            }
        }
        ExprKind::Index { object, index } => {
            collect_e1115_in_expr(object, in_async, in_loop, diags);
            collect_e1115_in_expr(index, in_async, in_loop, diags);
        }
        ExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                collect_e1115_in_expr(s, in_async, in_loop, diags);
            }
            if let Some(e) = end {
                collect_e1115_in_expr(e, in_async, in_loop, diags);
            }
        }
        ExprKind::Cast { expr: inner, .. } => {
            collect_e1115_in_expr(inner, in_async, in_loop, diags)
        }
        ExprKind::Closure(_) => {
            // A nested closure has its own loop / async scope. Don't
            // propagate the outer `in_loop` / `in_async` flags into
            // it — its `.await`s are scoped to the closure body and
            // handled as part of the lowering for THAT body.
        }
        _ => {}
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

// ─── E1112 pre-check (block_on inside async) ──────────────────────
//
// This MUST run BEFORE `lower_async_defs` rewrites async fn bodies
// into synthesised state-machine classes — once the async-fn rewrite
// fires, the original `block_on(...)` call ends up inside the
// generated `poll` method (which is itself NOT marked async), so
// the resolver's async_scope_depth check would not find it.
//
// Implementation: walk every top-level `async def` and every
// `async def` method on a class. For each `block_on(...)` call site
// inside the body, emit an E1112 diagnostic. Recursion mirrors the
// rewriter's walker shape.

/// Walk `program` BEFORE `lower_async_defs` runs and collect a
/// diagnostic for every `block_on(...)` call found inside an async
/// function or async closure. Returns the diagnostics; the caller is
/// responsible for surfacing them via the type-check result.
///
/// Spec: docs/specs/stdlib/executor.spec.md B6.
/// Error doc: docs/errors/E1112.md.
pub fn collect_block_on_in_async_diagnostics(
    program: &Program,
) -> Vec<crate::diagnostics::Diagnostic> {
    let mut diags = Vec::new();
    for item in &program.items {
        collect_e1112_in_item(item, /*in_async=*/ false, &mut diags);
    }
    diags
}

fn collect_e1112_in_item(
    item: &TopLevelItem,
    in_async: bool,
    diags: &mut Vec<crate::diagnostics::Diagnostic>,
) {
    match item {
        TopLevelItem::Function(func) => {
            let scope_async = in_async || func.is_async;
            collect_e1112_in_block(&func.body, scope_async, diags);
        }
        TopLevelItem::Class(class) => {
            for m in class.methods.iter() {
                let scope_async = in_async || m.is_async;
                collect_e1112_in_block(&m.body, scope_async, diags);
            }
            for inner_impl in class.inner_impls.iter() {
                for inner in inner_impl.items.iter() {
                    if let crate::parser::ast::ImplItem::Method(m) = inner {
                        let scope_async = in_async || m.is_async;
                        collect_e1112_in_block(&m.body, scope_async, diags);
                    }
                }
            }
        }
        TopLevelItem::Impl(impl_block) => {
            for inner in impl_block.items.iter() {
                if let crate::parser::ast::ImplItem::Method(m) = inner {
                    let scope_async = in_async || m.is_async;
                    collect_e1112_in_block(&m.body, scope_async, diags);
                }
            }
        }
        TopLevelItem::Module(module) => {
            for nested in module.items.iter() {
                collect_e1112_in_item(nested, in_async, diags);
            }
        }
        _ => {}
    }
}

fn collect_e1112_in_block(
    block: &Block,
    in_async: bool,
    diags: &mut Vec<crate::diagnostics::Diagnostic>,
) {
    for stmt in &block.statements {
        match stmt {
            Statement::Let(let_binding) => {
                if let Some(v) = &let_binding.value {
                    collect_e1112_in_expr(v, in_async, diags);
                }
            }
            Statement::Expression(e) => collect_e1112_in_expr(e, in_async, diags),
        }
    }
}

fn collect_e1112_in_expr(
    expr: &Expr,
    in_async: bool,
    diags: &mut Vec<crate::diagnostics::Diagnostic>,
) {
    // Flag this node if it is `block_on(_)` and we're in an async
    // scope. We don't gate on arg count — even malformed
    // `block_on()` calls should produce E1112 rather than a
    // confusing arity error first.
    if in_async {
        if let ExprKind::Call { callee, .. } = &expr.kind {
            if let ExprKind::Identifier(name) = &callee.kind {
                if name == "block_on" {
                    diags.push(crate::diagnostics::Diagnostic::error_with_code(
                        "`block_on` cannot be called inside an `async` function or closure — use `.await` to await a future in an async context",
                        expr.span.clone(),
                        "E1112",
                    ));
                }
            }
        }
    }

    // Recurse — descending into nested closures may CHANGE the
    // async scope (sync closure inside async fn → inner scope is
    // sync; async closure inside sync fn → inner scope is async).
    match &expr.kind {
        ExprKind::BinaryOp { left, right, .. } => {
            collect_e1112_in_expr(left, in_async, diags);
            collect_e1112_in_expr(right, in_async, diags);
        }
        ExprKind::UnaryOp { operand, .. } => collect_e1112_in_expr(operand, in_async, diags),
        ExprKind::Borrow(inner) | ExprKind::BorrowMut(inner) => {
            collect_e1112_in_expr(inner, in_async, diags);
        }
        ExprKind::FieldAccess { object, .. } => collect_e1112_in_expr(object, in_async, diags),
        ExprKind::MethodCall { object, args, .. } => {
            collect_e1112_in_expr(object, in_async, diags);
            for a in args.iter() {
                collect_e1112_in_expr(a, in_async, diags);
            }
        }
        ExprKind::Call { callee, args, .. } => {
            collect_e1112_in_expr(callee, in_async, diags);
            for a in args.iter() {
                collect_e1112_in_expr(a, in_async, diags);
            }
        }
        ExprKind::Index { object, index } => {
            collect_e1112_in_expr(object, in_async, diags);
            collect_e1112_in_expr(index, in_async, diags);
        }
        ExprKind::ClosureCall { callee, args } => {
            collect_e1112_in_expr(callee, in_async, diags);
            for a in args.iter() {
                collect_e1112_in_expr(a, in_async, diags);
            }
        }
        ExprKind::Try(inner) | ExprKind::Await(inner) => {
            collect_e1112_in_expr(inner, in_async, diags);
        }
        ExprKind::Assign { target, value } | ExprKind::CompoundAssign { target, value, .. } => {
            collect_e1112_in_expr(target, in_async, diags);
            collect_e1112_in_expr(value, in_async, diags);
        }
        ExprKind::If(IfExpr {
            condition,
            then_body,
            elsif_clauses,
            else_body,
            ..
        }) => {
            collect_e1112_in_expr(condition, in_async, diags);
            collect_e1112_in_block(then_body, in_async, diags);
            for el in elsif_clauses.iter() {
                collect_e1112_in_expr(&el.condition, in_async, diags);
                collect_e1112_in_block(&el.body, in_async, diags);
            }
            if let Some(b) = else_body {
                collect_e1112_in_block(b, in_async, diags);
            }
        }
        ExprKind::Match(MatchExpr { subject, arms, .. }) => {
            collect_e1112_in_expr(subject, in_async, diags);
            for a in arms.iter() {
                if let Some(g) = &a.guard {
                    collect_e1112_in_expr(g, in_async, diags);
                }
                match &a.body {
                    MatchArmBody::Expr(e) => collect_e1112_in_expr(e, in_async, diags),
                    MatchArmBody::Block(b) => collect_e1112_in_block(b, in_async, diags),
                }
            }
        }
        ExprKind::Block(b) => collect_e1112_in_block(b, in_async, diags),
        ExprKind::Loop(loop_expr) => collect_e1112_in_block(&loop_expr.body, in_async, diags),
        ExprKind::While(while_expr) => {
            collect_e1112_in_expr(&while_expr.condition, in_async, diags);
            collect_e1112_in_block(&while_expr.body, in_async, diags);
        }
        ExprKind::For(for_expr) => {
            collect_e1112_in_expr(&for_expr.iterable, in_async, diags);
            collect_e1112_in_block(&for_expr.body, in_async, diags);
        }
        ExprKind::Return(Some(inner)) | ExprKind::Break(Some(inner)) => {
            collect_e1112_in_expr(inner, in_async, diags);
        }
        ExprKind::ArrayLiteral(items) | ExprKind::TupleLiteral(items) => {
            for it in items.iter() {
                collect_e1112_in_expr(it, in_async, diags);
            }
        }
        ExprKind::ArrayFill { value, count } => {
            collect_e1112_in_expr(value, in_async, diags);
            collect_e1112_in_expr(count, in_async, diags);
        }
        ExprKind::MapLiteral(pairs) => {
            for (k, v) in pairs.iter() {
                collect_e1112_in_expr(k, in_async, diags);
                collect_e1112_in_expr(v, in_async, diags);
            }
        }
        ExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                collect_e1112_in_expr(s, in_async, diags);
            }
            if let Some(e) = end {
                collect_e1112_in_expr(e, in_async, diags);
            }
        }
        ExprKind::Cast { expr: inner, .. } => collect_e1112_in_expr(inner, in_async, diags),
        ExprKind::Closure(c) => {
            // The closure's async-ness OVERRIDES the surrounding
            // scope. Async closure inside sync fn → its body IS
            // async; sync closure inside async fn → its body is
            // NOT async (the .await ban applies to it, and
            // symmetrically block_on inside it is fine).
            let inner_async = c.is_async;
            match &c.body {
                ClosureBody::Expr(e) => collect_e1112_in_expr(e, inner_async, diags),
                ClosureBody::Block(b) => collect_e1112_in_block(b, inner_async, diags),
            }
        }
        ExprKind::UnsafeBlock(b) => collect_e1112_in_block(b, in_async, diags),
        ExprKind::EnumVariant { args, .. } => {
            for fa in args.iter() {
                collect_e1112_in_expr(&fa.value, in_async, diags);
            }
        }
        _ => {}
    }
}

// ─── E1116 pre-check (Task.spawn outside async) ────────────────────
//
// Spec: docs/specs/stdlib/task_spawn.spec.md §B7.
//
// `Task.spawn(fut)` (today reached via `Task.spawn_raw(...)`) only
// makes sense inside a Riven executor — i.e., inside an `async def`
// or `async { ... }` closure. Calling it from plain sync code means
// there's no executor to enqueue into.
//
// Polarity is inverted vs. E1112: flag the call when in_async ==
// false. The walker reuses the same scope-tracking shape (toggling
// in_async on async function/closure bodies) so a Task.spawn inside
// a sync closure inside an async fn correctly fires (the inner sync
// closure has its own non-async scope, so the call there has no
// executor either).
//
// Surface match: matches both `Task.spawn(...)` (MethodCall with
// receiver `Task`) and `Task.spawn_raw(...)` (same shape). The
// future-typed wrapper that ships in commit 2 (`Task.spawn` with a
// `&var Future` parameter) routes through the same MethodCall AST,
// so this check catches it without additional wiring.
//
// Error doc: docs/errors/E1116.md.
pub fn collect_task_spawn_outside_async_diagnostics(
    program: &Program,
) -> Vec<crate::diagnostics::Diagnostic> {
    let mut diags = Vec::new();
    for item in &program.items {
        collect_e1116_in_item(item, /*in_async=*/ false, &mut diags);
    }
    diags
}

fn collect_e1116_in_item(
    item: &TopLevelItem,
    in_async: bool,
    diags: &mut Vec<crate::diagnostics::Diagnostic>,
) {
    match item {
        TopLevelItem::Function(func) => {
            let scope_async = in_async || func.is_async;
            collect_e1116_in_block(&func.body, scope_async, diags);
        }
        TopLevelItem::Class(class) => {
            for m in class.methods.iter() {
                let scope_async = in_async || m.is_async;
                collect_e1116_in_block(&m.body, scope_async, diags);
            }
            for inner_impl in class.inner_impls.iter() {
                for inner in inner_impl.items.iter() {
                    if let crate::parser::ast::ImplItem::Method(m) = inner {
                        let scope_async = in_async || m.is_async;
                        collect_e1116_in_block(&m.body, scope_async, diags);
                    }
                }
            }
        }
        TopLevelItem::Impl(impl_block) => {
            for inner in impl_block.items.iter() {
                if let crate::parser::ast::ImplItem::Method(m) = inner {
                    let scope_async = in_async || m.is_async;
                    collect_e1116_in_block(&m.body, scope_async, diags);
                }
            }
        }
        TopLevelItem::Module(module) => {
            for nested in module.items.iter() {
                collect_e1116_in_item(nested, in_async, diags);
            }
        }
        _ => {}
    }
}

fn collect_e1116_in_block(
    block: &Block,
    in_async: bool,
    diags: &mut Vec<crate::diagnostics::Diagnostic>,
) {
    for stmt in &block.statements {
        match stmt {
            Statement::Let(let_binding) => {
                if let Some(v) = &let_binding.value {
                    collect_e1116_in_expr(v, in_async, diags);
                }
            }
            Statement::Expression(e) => collect_e1116_in_expr(e, in_async, diags),
        }
    }
}

fn collect_e1116_in_expr(
    expr: &Expr,
    in_async: bool,
    diags: &mut Vec<crate::diagnostics::Diagnostic>,
) {
    // Flag this node if it is `Task.spawn(...)` or `Task.spawn_raw(...)`
    // and we're NOT in an async scope. The parser folds
    // `Task.spawn(x)` into MethodCall { object: Identifier("Task"),
    // method: "spawn", args }.
    if !in_async {
        if let ExprKind::MethodCall { object, method, .. } = &expr.kind {
            if let ExprKind::Identifier(name) = &object.kind {
                if name == "Task" && (method == "spawn" || method == "spawn_raw") {
                    diags.push(crate::diagnostics::Diagnostic::error_with_code(
                        "`Task.spawn` can only be called inside an `async` function or closure — there is no executor to enqueue into in sync context",
                        expr.span.clone(),
                        "E1116",
                    ));
                }
            }
        }
    }

    // Recurse — descending into nested closures CHANGES the async
    // scope (matches E1112's recursion shape).
    match &expr.kind {
        ExprKind::BinaryOp { left, right, .. } => {
            collect_e1116_in_expr(left, in_async, diags);
            collect_e1116_in_expr(right, in_async, diags);
        }
        ExprKind::UnaryOp { operand, .. } => collect_e1116_in_expr(operand, in_async, diags),
        ExprKind::Borrow(inner) | ExprKind::BorrowMut(inner) => {
            collect_e1116_in_expr(inner, in_async, diags);
        }
        ExprKind::FieldAccess { object, .. } => collect_e1116_in_expr(object, in_async, diags),
        ExprKind::MethodCall { object, args, .. } => {
            collect_e1116_in_expr(object, in_async, diags);
            for a in args.iter() {
                collect_e1116_in_expr(a, in_async, diags);
            }
        }
        ExprKind::Call { callee, args, .. } => {
            collect_e1116_in_expr(callee, in_async, diags);
            for a in args.iter() {
                collect_e1116_in_expr(a, in_async, diags);
            }
        }
        ExprKind::Index { object, index } => {
            collect_e1116_in_expr(object, in_async, diags);
            collect_e1116_in_expr(index, in_async, diags);
        }
        ExprKind::ClosureCall { callee, args } => {
            collect_e1116_in_expr(callee, in_async, diags);
            for a in args.iter() {
                collect_e1116_in_expr(a, in_async, diags);
            }
        }
        ExprKind::Try(inner) | ExprKind::Await(inner) => {
            collect_e1116_in_expr(inner, in_async, diags);
        }
        ExprKind::Assign { target, value } | ExprKind::CompoundAssign { target, value, .. } => {
            collect_e1116_in_expr(target, in_async, diags);
            collect_e1116_in_expr(value, in_async, diags);
        }
        ExprKind::If(IfExpr {
            condition,
            then_body,
            elsif_clauses,
            else_body,
            ..
        }) => {
            collect_e1116_in_expr(condition, in_async, diags);
            collect_e1116_in_block(then_body, in_async, diags);
            for el in elsif_clauses.iter() {
                collect_e1116_in_expr(&el.condition, in_async, diags);
                collect_e1116_in_block(&el.body, in_async, diags);
            }
            if let Some(b) = else_body {
                collect_e1116_in_block(b, in_async, diags);
            }
        }
        ExprKind::Match(MatchExpr { subject, arms, .. }) => {
            collect_e1116_in_expr(subject, in_async, diags);
            for a in arms.iter() {
                if let Some(g) = &a.guard {
                    collect_e1116_in_expr(g, in_async, diags);
                }
                match &a.body {
                    MatchArmBody::Expr(e) => collect_e1116_in_expr(e, in_async, diags),
                    MatchArmBody::Block(b) => collect_e1116_in_block(b, in_async, diags),
                }
            }
        }
        ExprKind::Block(b) => collect_e1116_in_block(b, in_async, diags),
        ExprKind::Loop(loop_expr) => collect_e1116_in_block(&loop_expr.body, in_async, diags),
        ExprKind::While(while_expr) => {
            collect_e1116_in_expr(&while_expr.condition, in_async, diags);
            collect_e1116_in_block(&while_expr.body, in_async, diags);
        }
        ExprKind::For(for_expr) => {
            collect_e1116_in_expr(&for_expr.iterable, in_async, diags);
            collect_e1116_in_block(&for_expr.body, in_async, diags);
        }
        ExprKind::Return(Some(inner)) | ExprKind::Break(Some(inner)) => {
            collect_e1116_in_expr(inner, in_async, diags);
        }
        ExprKind::ArrayLiteral(items) | ExprKind::TupleLiteral(items) => {
            for it in items.iter() {
                collect_e1116_in_expr(it, in_async, diags);
            }
        }
        ExprKind::ArrayFill { value, count } => {
            collect_e1116_in_expr(value, in_async, diags);
            collect_e1116_in_expr(count, in_async, diags);
        }
        ExprKind::MapLiteral(pairs) => {
            for (k, v) in pairs.iter() {
                collect_e1116_in_expr(k, in_async, diags);
                collect_e1116_in_expr(v, in_async, diags);
            }
        }
        ExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                collect_e1116_in_expr(s, in_async, diags);
            }
            if let Some(e) = end {
                collect_e1116_in_expr(e, in_async, diags);
            }
        }
        ExprKind::Cast { expr: inner, .. } => collect_e1116_in_expr(inner, in_async, diags),
        ExprKind::Closure(c) => {
            let inner_async = c.is_async;
            match &c.body {
                ClosureBody::Expr(e) => collect_e1116_in_expr(e, inner_async, diags),
                ClosureBody::Block(b) => collect_e1116_in_block(b, inner_async, diags),
            }
        }
        ExprKind::UnsafeBlock(b) => collect_e1116_in_block(b, in_async, diags),
        ExprKind::EnumVariant { args, .. } => {
            for fa in args.iter() {
                collect_e1116_in_expr(&fa.value, in_async, diags);
            }
        }
        _ => {}
    }
}

// ─── block_on intrinsic rewriter ───────────────────────────────────
//
// Sub-phase 3 of the async round (docs/specs/stdlib/executor.spec.md).
// At every `block_on(EXPR)` call site, rewrite to a block of the form:
//
//   {
//     var __block_on_fut_N = EXPR
//     var __block_on_ctx_N = Context.executor
//     loop
//       match (&var __block_on_fut_N).poll(&var __block_on_ctx_N)
//         Poll.Ready(__block_on_v_N) -> break __block_on_v_N
//         Poll.Pending -> Thread.yield_now
//       end
//     end
//   }
//
// The trailing expression of the block is the `loop`. The loop's
// type is inferred by typeck from the `break __block_on_v_N` value
// (see `typeck::infer.rs::HirExprKind::Loop` — it walks the body
// collecting break-value types and unifies them). The break value
// IS the Poll.Ready payload, i.e. the future's Output, so the whole
// block evaluates to `Output`.
//
// Task #20 (2026-05-21): this is a fix for the prior "result-var
// hardcoded to Int(0)" shape, which pinned the block's type to Int
// and erased the future's typed Output. Any future whose Output was
// `Result[T, E]`, `String`, or a user class lost its payload through
// block_on. The fix moves typing into typeck's existing break-value
// loop inference rather than synthesising a typed local at AST time
// — the latter requires a "zero value for arbitrary T" Riven doesn't
// have, while the former needs no new infrastructure.
//
// Why break-with-value is safe here: MIR's
// `lower_expr/control.rs::HirExprKind::Break` assigns the break value
// into the loop's `result_local` BEFORE running
// `emit_dealloc_loop_locals`, so move semantics for non-Copy payloads
// are preserved. Fixture `tests/release-e2e/cases/54_loop_break_value.rvn`
// exercises this end-to-end.
//
// Counter `N` is per-function so repeated block_on calls in the same
// scope get distinct names — guards against shadowing if MIR
// scope-tracking is conservative.
//
// Why AST-level and not MIR-level: the poll-match is a sequence of
// constructs (let, loop, match on Poll, method call) all of which
// already lower cleanly. AST rewriting reuses existing infrastructure
// — no new MIR instruction, no new typeck path. The MIR-level
// alternative was rejected as duplicating concerns already handled
// by Riven's standard pipeline (see plan doc for the design choice).

fn rewrite_block_on_calls(program: &mut Program) {
    let mut counter: u32 = 0;
    for item in program.items.iter_mut() {
        rewrite_block_on_in_item(item, &mut counter);
    }
}

fn rewrite_block_on_in_item(item: &mut TopLevelItem, counter: &mut u32) {
    match item {
        TopLevelItem::Function(func) => {
            // Skip async functions: block_on inside async would
            // deadlock at runtime (E1112). Leaving the call un-
            // rewritten lets the resolver flag it (see
            // resolve/exprs.rs:Call site) before the typeck
            // signature mismatch noise would mask the real issue.
            if func.is_async {
                return;
            }
            rewrite_block_on_in_block(&mut func.body, counter);
        }
        TopLevelItem::Class(class) => {
            for m in class.methods.iter_mut() {
                if m.is_async {
                    continue;
                }
                rewrite_block_on_in_block(&mut m.body, counter);
            }
            // Methods declared via in-body `impl Mixin do ... end`
            // blocks live in inner_impls, not the top-level methods
            // vec — walk them too.
            for inner_impl in class.inner_impls.iter_mut() {
                for inner in inner_impl.items.iter_mut() {
                    if let crate::parser::ast::ImplItem::Method(m) = inner {
                        if m.is_async {
                            continue;
                        }
                        rewrite_block_on_in_block(&mut m.body, counter);
                    }
                }
            }
        }
        TopLevelItem::Impl(impl_block) => {
            for inner in impl_block.items.iter_mut() {
                if let crate::parser::ast::ImplItem::Method(m) = inner {
                    if m.is_async {
                        continue;
                    }
                    rewrite_block_on_in_block(&mut m.body, counter);
                }
            }
        }
        TopLevelItem::Module(module) => {
            for nested in module.items.iter_mut() {
                rewrite_block_on_in_item(nested, counter);
            }
        }
        _ => {}
    }
}

fn rewrite_block_on_in_block(block: &mut Block, counter: &mut u32) {
    for stmt in block.statements.iter_mut() {
        match stmt {
            Statement::Let(let_binding) => {
                if let Some(v) = &mut let_binding.value {
                    rewrite_block_on_in_expr(v, counter);
                }
            }
            Statement::Expression(e) => rewrite_block_on_in_expr(e, counter),
        }
    }
}

fn rewrite_block_on_in_expr(expr: &mut Expr, counter: &mut u32) {
    // Recurse into children FIRST so nested block_on calls
    // (`block_on(block_on(x))` — pathological but legal at parse
    // time) get rewritten innermost-out.
    match &mut expr.kind {
        ExprKind::BinaryOp { left, right, .. } => {
            rewrite_block_on_in_expr(left, counter);
            rewrite_block_on_in_expr(right, counter);
        }
        ExprKind::UnaryOp { operand, .. } => rewrite_block_on_in_expr(operand, counter),
        ExprKind::Borrow(inner) | ExprKind::BorrowMut(inner) => {
            rewrite_block_on_in_expr(inner, counter);
        }
        ExprKind::FieldAccess { object, .. } => rewrite_block_on_in_expr(object, counter),
        ExprKind::MethodCall { object, args, .. } => {
            rewrite_block_on_in_expr(object, counter);
            for a in args.iter_mut() {
                rewrite_block_on_in_expr(a, counter);
            }
        }
        ExprKind::Call { callee, args, .. } => {
            rewrite_block_on_in_expr(callee, counter);
            for a in args.iter_mut() {
                rewrite_block_on_in_expr(a, counter);
            }
        }
        ExprKind::Index { object, index } => {
            rewrite_block_on_in_expr(object, counter);
            rewrite_block_on_in_expr(index, counter);
        }
        ExprKind::ClosureCall { callee, args } => {
            rewrite_block_on_in_expr(callee, counter);
            for a in args.iter_mut() {
                rewrite_block_on_in_expr(a, counter);
            }
        }
        ExprKind::Try(inner) | ExprKind::Await(inner) => {
            rewrite_block_on_in_expr(inner, counter);
        }
        ExprKind::Assign { target, value } | ExprKind::CompoundAssign { target, value, .. } => {
            rewrite_block_on_in_expr(target, counter);
            rewrite_block_on_in_expr(value, counter);
        }
        ExprKind::If(IfExpr {
            condition,
            then_body,
            elsif_clauses,
            else_body,
            ..
        }) => {
            rewrite_block_on_in_expr(condition, counter);
            rewrite_block_on_in_block(then_body, counter);
            for el in elsif_clauses.iter_mut() {
                rewrite_block_on_in_expr(&mut el.condition, counter);
                rewrite_block_on_in_block(&mut el.body, counter);
            }
            if let Some(b) = else_body {
                rewrite_block_on_in_block(b, counter);
            }
        }
        ExprKind::Match(MatchExpr { subject, arms, .. }) => {
            rewrite_block_on_in_expr(subject, counter);
            for a in arms.iter_mut() {
                if let Some(g) = &mut a.guard {
                    rewrite_block_on_in_expr(g, counter);
                }
                match &mut a.body {
                    MatchArmBody::Expr(e) => rewrite_block_on_in_expr(e, counter),
                    MatchArmBody::Block(b) => rewrite_block_on_in_block(b, counter),
                }
            }
        }
        ExprKind::Block(b) => rewrite_block_on_in_block(b, counter),
        ExprKind::Loop(loop_expr) => rewrite_block_on_in_block(&mut loop_expr.body, counter),
        ExprKind::While(while_expr) => {
            rewrite_block_on_in_expr(&mut while_expr.condition, counter);
            rewrite_block_on_in_block(&mut while_expr.body, counter);
        }
        ExprKind::For(for_expr) => {
            rewrite_block_on_in_expr(&mut for_expr.iterable, counter);
            rewrite_block_on_in_block(&mut for_expr.body, counter);
        }
        ExprKind::Return(Some(inner)) | ExprKind::Break(Some(inner)) => {
            rewrite_block_on_in_expr(inner, counter);
        }
        ExprKind::ArrayLiteral(items) | ExprKind::TupleLiteral(items) => {
            for it in items.iter_mut() {
                rewrite_block_on_in_expr(it, counter);
            }
        }
        ExprKind::ArrayFill { value, count } => {
            rewrite_block_on_in_expr(value, counter);
            rewrite_block_on_in_expr(count, counter);
        }
        ExprKind::MapLiteral(pairs) => {
            for (k, v) in pairs.iter_mut() {
                rewrite_block_on_in_expr(k, counter);
                rewrite_block_on_in_expr(v, counter);
            }
        }
        ExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                rewrite_block_on_in_expr(s, counter);
            }
            if let Some(e) = end {
                rewrite_block_on_in_expr(e, counter);
            }
        }
        ExprKind::Cast { expr: inner, .. } => rewrite_block_on_in_expr(inner, counter),
        ExprKind::Closure(c) => {
            // Skip async closures — same reasoning as async fns
            // (see rewrite_block_on_in_item).
            if !c.is_async {
                match &mut c.body {
                    ClosureBody::Expr(e) => rewrite_block_on_in_expr(e, counter),
                    ClosureBody::Block(b) => rewrite_block_on_in_block(b, counter),
                }
            }
        }
        ExprKind::UnsafeBlock(b) => rewrite_block_on_in_block(b, counter),
        ExprKind::EnumVariant { args, .. } => {
            for fa in args.iter_mut() {
                rewrite_block_on_in_expr(&mut fa.value, counter);
            }
        }
        _ => {}
    }

    // Now check this node itself. If it's `block_on(EXPR)` with
    // exactly one argument, replace it with the inline poll-loop
    // block expression.
    let rewrite_target = matches!(
        &expr.kind,
        ExprKind::Call { callee, args, block: None }
            if matches!(&callee.kind, ExprKind::Identifier(name) if name == "block_on")
                && args.len() == 1
    );
    if rewrite_target {
        // Steal the future expression out of the Call.
        let span = expr.span.clone();
        let mut owned = std::mem::replace(
            expr,
            Expr {
                kind: ExprKind::NullLiteral,
                span: span.clone(),
            },
        );
        let future_expr = match &mut owned.kind {
            ExprKind::Call { args, .. } => args.remove(0),
            _ => unreachable!("guarded above"),
        };
        *counter += 1;
        let n = *counter;
        *expr = build_block_on_loop(future_expr, n, &span);
    }
}

/// Build the inline poll-loop block for a `block_on(future_expr)`
/// call. `n` is a fresh counter used to name the introduced locals.
///
/// Shape (task #20 — typed-output via break-with-value):
///   {
///     var __block_on_fut_N = future_expr
///     var __block_on_ctx_N = Context.executor
///     loop
///       match (&var __block_on_fut_N).poll(&var __block_on_ctx_N)
///         Poll.Ready(__block_on_v_N) -> break __block_on_v_N
///         Poll.Pending -> Thread.yield_now
///       end
///     end
///   }
fn build_block_on_loop(future_expr: Expr, n: u32, span: &Span) -> Expr {
    let fut_name = format!("__block_on_fut_{n}");
    let ctx_name = format!("__block_on_ctx_{n}");
    let v_name = format!("__block_on_v_{n}");

    // var __block_on_fut_N = EXPR
    let let_fut = Statement::Let(LetBinding {
        mutable: true,
        pattern: Pattern::Identifier {
            mutable: true,
            name: fut_name.clone(),
            span: span.clone(),
        },
        type_annotation: None,
        value: Some(Box::new(future_expr)),
        span: span.clone(),
    });

    // var __block_on_ctx_N = Context.executor
    //
    // `Context.executor` is a no-arg static method declared in
    // library/std/future/src/lib.rvn. Riven's parser treats
    // `Foo.bar` as a FieldAccess when there are no parens; the
    // resolver lifts static-method-without-parens to a static call
    // (same pattern as `Thread.current_id` in fixture 700).
    let ctx_factory = Expr {
        kind: ExprKind::FieldAccess {
            object: Box::new(Expr {
                kind: ExprKind::Identifier("Context".to_string()),
                span: span.clone(),
            }),
            field: "executor".to_string(),
        },
        span: span.clone(),
    };
    let let_ctx = Statement::Let(LetBinding {
        mutable: true,
        pattern: Pattern::Identifier {
            mutable: true,
            name: ctx_name.clone(),
            span: span.clone(),
        },
        type_annotation: None,
        value: Some(Box::new(ctx_factory)),
        span: span.clone(),
    });

    // (&var __block_on_fut_N).poll(&var __block_on_ctx_N)
    let poll_call = Expr {
        kind: ExprKind::MethodCall {
            object: Box::new(Expr {
                kind: ExprKind::BorrowMut(Box::new(Expr {
                    kind: ExprKind::Identifier(fut_name.clone()),
                    span: span.clone(),
                })),
                span: span.clone(),
            }),
            method: "poll".to_string(),
            generic_args: Vec::new(),
            args: vec![Expr {
                kind: ExprKind::BorrowMut(Box::new(Expr {
                    kind: ExprKind::Identifier(ctx_name.clone()),
                    span: span.clone(),
                })),
                span: span.clone(),
            }],
            block: None,
        },
        span: span.clone(),
    };

    // Match arm: Poll.Ready(__block_on_v_N) -> break __block_on_v_N
    //
    // The break value IS the future's Output (the Poll.Ready payload).
    // Typeck infers the loop expression's type from the union of
    // break-value types (see typeck::infer.rs::HirExprKind::Loop), so
    // the surrounding block evaluates to the correct Output type —
    // Result[T,E], String, a user class, whatever the future declared.
    // This replaces the prior "var __block_on_res_N = 0" shape which
    // pinned the result to Int and silently erased non-Int outputs.
    let ready_pattern = Pattern::Enum {
        path: vec!["Poll".to_string()],
        variant: "Ready".to_string(),
        fields: vec![Pattern::Identifier {
            mutable: false,
            name: v_name.clone(),
            span: span.clone(),
        }],
        span: span.clone(),
    };
    let break_with_value = Expr {
        kind: ExprKind::Break(Some(Box::new(Expr {
            kind: ExprKind::Identifier(v_name.clone()),
            span: span.clone(),
        }))),
        span: span.clone(),
    };
    let ready_arm = MatchArm {
        pattern: ready_pattern,
        guard: None,
        body: MatchArmBody::Expr(break_with_value),
        span: span.clone(),
    };

    // Match arm: Poll.Pending -> Thread.yield_now
    //
    // Sub-phase 4A (docs/specs/stdlib/async_io.spec.md B2) replaces
    // the sched_yield-spin inside `Thread.yield_now` itself with a
    // park-on-reactor in `library/std/sync/runtime/thread.c`. The
    // AST emitted here is unchanged — the C-side `riven_thread_yield`
    // now blocks on the per-thread reactor's wait point when any
    // I/O registration is live, and falls back to `sched_yield`
    // when there is nothing to wait on (matches pre-4A behaviour
    // for the trivial 2A / 2B futures that never touch the reactor).
    let pending_pattern = Pattern::Enum {
        path: vec!["Poll".to_string()],
        variant: "Pending".to_string(),
        fields: Vec::new(),
        span: span.clone(),
    };
    let yield_call = Expr {
        kind: ExprKind::FieldAccess {
            object: Box::new(Expr {
                kind: ExprKind::Identifier("Thread".to_string()),
                span: span.clone(),
            }),
            field: "yield_now".to_string(),
        },
        span: span.clone(),
    };
    let pending_arm = MatchArm {
        pattern: pending_pattern,
        guard: None,
        body: MatchArmBody::Expr(yield_call),
        span: span.clone(),
    };

    let match_expr = Expr {
        kind: ExprKind::Match(MatchExpr {
            subject: Box::new(poll_call),
            arms: vec![ready_arm, pending_arm],
            span: span.clone(),
        }),
        span: span.clone(),
    };

    // Sub-phase 5 (docs/specs/stdlib/task_spawn.spec.md §B3):
    // pump the spawned-task queue once per iteration. The helper
    // short-circuits via a thread-local null-pointer check when no
    // tasks were ever spawned, so block_on calls that never use
    // Task.spawn pay one extra C call per iteration (a single
    // load + compare + return) — negligible vs. the existing
    // Thread.yield_now / reactor-park cost.
    //
    // Emitted as `riven_executor_pump_tasks()` (free-fn lib decl in
    // library/std/future/src/lib.rvn). Same mechanism as the
    // Pending arm's `Thread.yield_now` call — identifier-callee
    // synthesis with no implicit self.
    let pump_call = Expr {
        kind: ExprKind::Call {
            callee: Box::new(Expr {
                kind: ExprKind::Identifier("riven_executor_pump_tasks".to_string()),
                span: span.clone(),
            }),
            args: Vec::new(),
            block: None,
        },
        span: span.clone(),
    };

    // loop ... end — pump first, then poll the top-level future.
    let loop_expr = Expr {
        kind: ExprKind::Loop(LoopExpr {
            body: Block {
                statements: vec![
                    Statement::Expression(pump_call),
                    Statement::Expression(match_expr),
                ],
                span: span.clone(),
            },
            span: span.clone(),
        }),
        span: span.clone(),
    };

    // Final outer block: let_fut, let_ctx, loop (trailing expression).
    //
    // The loop is the block's trailing expression — typeck infers its
    // type from break-with-value, so the block evaluates to the
    // future's Output type.
    let outer_block = Block {
        statements: vec![let_fut, let_ctx, Statement::Expression(loop_expr)],
        span: span.clone(),
    };

    Expr {
        kind: ExprKind::Block(outer_block),
        span: span.clone(),
    }
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
