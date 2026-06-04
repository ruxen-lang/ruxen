//! Async state-machine lowering — the AST→AST pass that turns each
//! non-loop `async def` into a synthesised `Future` state-machine
//! class (via the await-delimited [`super::cfg`] CFG) and each loop-
//! shaped one into a dedicated while/multi-phase builder, plus the
//! `block_on` poll-loop rewriter. Public entry points are re-exported
//! from the [`super`] facade; `super::cfg` and `super::diagnostics`
//! reach the shared await-scan + loop recognizers via `pub(crate)`
//! re-exports on the facade.

use std::collections::HashMap;

use crate::lexer::token::Span;
use crate::parser::ast::*;
use crate::parser::visit::{walk_expr, walk_expr_mut, Visit, VisitMut};

use super::cfg;

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
/// `project_ruxen_async_compiler_gaps.md` (#2) and unblocks the two
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
    // `TimeSleepFuture` (library/std/future/src/lib.rx).
    let mut class_static_returns: HashMap<(String, String), TypeExpr> = HashMap::new();
    let mut class_instance_returns: HashMap<(String, String), TypeExpr> = HashMap::new();
    let mut future_outputs: HashMap<String, TypeExpr> = HashMap::new();
    collect_class_static_returns_into(program, &mut class_static_returns);
    collect_class_instance_returns_into(program, &mut class_instance_returns);
    collect_future_outputs_into(program, &mut future_outputs);
    for bp in bootstrap_programs {
        collect_class_static_returns_into(bp, &mut class_static_returns);
        collect_class_instance_returns_into(bp, &mut class_instance_returns);
        collect_future_outputs_into(bp, &mut future_outputs);
    }

    let mut new_classes: Vec<TopLevelItem> = Vec::new();

    for item in program.items.iter_mut() {
        if let TopLevelItem::Function(func) = item {
            if !func.is_async || func.is_class_method {
                continue;
            }
            if block_contains_await(&func.body) {
                // Multi-await loop body (2+ awaits per iteration).
                // Tried FIRST because the single-await recognizer
                // explicitly rejects N>=2; ordering only matters for
                // the (degenerate) N==1 case where multi falls
                // through. Spec: §multi-await extension in
                // docs/specs/syntax/async_lowering_loop_await.spec.md.
                if let Some(multi_shape) = recognize_while_multi_await(&func.body) {
                    if let Some((rewritten, sm_class)) = lower_async_fn_while_multi_await(
                        func,
                        &multi_shape,
                        &async_fn_returns,
                        &class_static_returns,
                        &class_instance_returns,
                        &future_outputs,
                    ) {
                        *func = rewritten;
                        new_classes.push(TopLevelItem::Class(sm_class));
                        continue;
                    }
                }
                // Single-await loop body (exactly 1 await). Spec:
                // docs/specs/syntax/async_lowering_loop_await.spec.md.
                // Falls through to the linear-await path if the body
                // doesn't match the loop shape.
                if let Some(while_shape) = recognize_while_single_await(&func.body) {
                    if let Some((rewritten, sm_class)) = lower_async_fn_while_single_await(
                        func,
                        &while_shape,
                        &async_fn_returns,
                        &class_static_returns,
                        &class_instance_returns,
                        &future_outputs,
                    ) {
                        *func = rewritten;
                        new_classes.push(TopLevelItem::Class(sm_class));
                        continue;
                    }
                }
                // Phase 3 Path A: the non-loop `.await` shape (linear-N)
                // lowers through the unified Cfg path. It returns `None`
                // for loop-shaped or unsupported bodies; loop-shaped
                // bodies were already handled by the recognizers above,
                // and any remaining unsupported shape (e.g. `.await`
                // nested in a non-let expr) is left in its pre-lowering
                // state for the resolver/typeck to diagnose (E1110 /
                // E1115). The old linear/no-await builders this subsumes
                // were deleted in Phase 3 step 4.
                if let Some((rewritten, sm_class)) = lower_async_fn_via_cfg(
                    func,
                    &async_fn_returns,
                    &class_static_returns,
                    &class_instance_returns,
                    &future_outputs,
                ) {
                    *func = rewritten;
                    new_classes.push(TopLevelItem::Class(sm_class));
                }
            } else {
                // Phase 3 Path A: the no-await shape routes through the
                // same unified Cfg path (degenerate single-segment Cfg).
                if let Some((rewritten, sm_class)) = lower_async_fn_via_cfg(
                    func,
                    &async_fn_returns,
                    &class_static_returns,
                    &class_instance_returns,
                    &future_outputs,
                ) {
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

/// Twin of [`collect_class_static_returns_into`] for INSTANCE methods —
/// `def X(...) -> T` (no `self.` prefix). Drives Shape 3 in
/// `describe_await`: `<obj>.<method>(args).await` where the receiver's
/// class is known from a `(receiver_name → class_name)` map built by
/// the caller (`lower_async_fn_via_cfg`).
///
/// The receiver-type map is the missing piece — it's built from the
/// outer fn's param annotations + simple let-binding chains, which is
/// enough to cover the canonical server shape:
///
///   def handle(stream: AsyncTcpStream)
///     let n = stream.read(buf).await   # Shape 3 — works
///   end
///
/// Cases that fall outside (`Result.Ok(x)` pattern-bound x, x coming
/// from another async fn's Output, etc.) still need
/// `block_on(method())`. Closing those is a separate task.
fn collect_class_instance_returns_into(
    program: &Program,
    into: &mut HashMap<(String, String), TypeExpr>,
) {
    for item in &program.items {
        let class = match item {
            TopLevelItem::Class(c) => c,
            _ => continue,
        };
        // Hand-written instance methods — `def name(self, ...) -> T`.
        for m in &class.methods {
            if m.is_class_method {
                continue;
            }
            if let Some(ret) = &m.return_type {
                into.insert((class.name.clone(), m.name.clone()), ret.clone());
            }
        }
        // FFI shells inside `lib "..." ... end` declared as instance
        // methods (`def name as "..."(self, ...) -> T`). The stdlib's
        // AsyncTcpStream / AsyncTcpListener instance API lives here.
        for lib in &class.lib_decls {
            for f in &lib.functions {
                if f.is_class_method {
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

/// Phase 3 Path A — unified lowering for the NON-LOOP async-fn shapes
/// (no-await + linear-N-await), driven by the await-delimited
/// [`cfg::Cfg`]. Subsumes `lower_one_async_fn` (no-await) and
/// `lower_one_async_fn_with_await` (linear-N) behind one
/// `self.__state`-indexed poll skeleton built by [`build_poll_body_cfg`].
///
/// Returns `None` when the body is loop-shaped (the Cfg carries an
/// [`cfg::Edge::Loop`]) — those still flow through the three dedicated
/// loop builders (while-single / while-multi / multi-phase), which this
/// phase deliberately leaves untouched. Also `None` for any body
/// outside the union the Cfg analysis accepts, or when an awaitee /
/// crossing-local fails the same checks the linear path enforced.
fn lower_async_fn_via_cfg(
    func: &FuncDef,
    async_fn_returns: &HashMap<String, (String, TypeExpr)>,
    class_static_returns: &HashMap<(String, String), TypeExpr>,
    class_instance_returns: &HashMap<(String, String), TypeExpr>,
    future_outputs: &HashMap<String, TypeExpr>,
) -> Option<(FuncDef, ClassDef)> {
    let span = func.span.clone();
    let class_name = mangle_future_class_name(&func.name);

    let cfg = cfg::segment_cfg(&func.body)?;

    // Loop-shaped bodies are OUT of scope for Path A — bail so the
    // caller's loop recognizers + builders handle them unchanged.
    if cfg
        .edges
        .iter()
        .any(|e| matches!(e, cfg::Edge::Loop { .. }))
    {
        return None;
    }

    // The terminal segment (suspend: None) produces the return value via
    // `cfg.tail`; every other segment carries exactly one suspend. For
    // the non-loop shapes the suspends are the segments in order, sans
    // the terminal — `cfg::Edge::Next` chains them 0→1→…→N.
    let suspend_segments: Vec<&cfg::Segment> = cfg
        .segments
        .iter()
        .filter(|s| s.suspend.is_some())
        .collect();
    let n_suspends = suspend_segments.len();

    // Pre-await straight-line statements: segment 0's `stmts`. For the
    // no-await case segment 0 IS the terminal (no suspend) and its
    // `stmts` are empty (the whole body is `cfg.tail`). For linear-N,
    // segment 0 carries the pre-await prefix folded in by `segment_cfg`.
    let pre_await_stmts: Vec<Statement> = if n_suspends == 0 {
        Vec::new()
    } else {
        cfg.segments[0].stmts.clone()
    };
    let tail_stmts: Vec<Statement> = cfg.tail.clone();

    // The no-await body has a different return-type policy than the
    // await path: 2A infers when the annotation is absent, 2B requires
    // an explicit return type (it types the terminal `Poll.Ready`
    // fold). Preserve both.
    let return_type = if n_suspends == 0 {
        func.return_type
            .clone()
            .unwrap_or_else(|| TypeExpr::Inferred { span: span.clone() })
    } else {
        func.return_type.clone()?
    };

    // ── Crossing-local analysis (ported verbatim from the linear path).
    //
    // A pre-await `let <name> = ...` becomes a state-machine field IFF
    // `<name>` is read in `tail` (after the last await). Crossing locals
    // need an explicit type annotation; bail otherwise (the resolver
    // diagnostic for the un-lowered body surfaces).
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

    let outer_arg_names: Vec<String> = func.params.iter().map(|p| p.name.clone()).collect();
    let mut outer_field_names: Vec<String> = outer_arg_names.clone();
    outer_field_names.extend(crossing_names.iter().cloned());

    // Receiver-type map for Shape-3 (instance-method awaitee) lookup —
    // ported verbatim from the linear path.
    let mut receiver_types: HashMap<String, String> = HashMap::new();
    for p in &func.params {
        if let Some(name) = single_named_class(&p.type_expr) {
            receiver_types.insert(p.name.clone(), name);
        }
    }
    for s in &pre_await_stmts {
        if let Statement::Let(lb) = s {
            if let Pattern::Identifier { name, .. } = &lb.pattern {
                if let Some(ann) = &lb.type_annotation {
                    if let Some(cls) = single_named_class(ann) {
                        receiver_types.insert(name.clone(), cls);
                        continue;
                    }
                }
                if let Some(val) = &lb.value {
                    if let ExprKind::Identifier(src) = &val.kind {
                        if let Some(src_ty) = receiver_types.get(src).cloned() {
                            receiver_types.insert(name.clone(), src_ty);
                        }
                    }
                }
            }
        }
    }

    // Describe every suspend's awaitee, reusing the exact classifier the
    // linear path used (`describe_awaitee`, factored out of
    // `describe_await`).
    let mut subs: Vec<AwaitSub> = Vec::with_capacity(n_suspends);
    for seg in &suspend_segments {
        let suspend = seg.suspend.as_ref()?;
        let sub = describe_awaitee(
            &suspend.binding,
            &suspend.awaitee,
            async_fn_returns,
            class_static_returns,
            class_instance_returns,
            future_outputs,
            &outer_field_names,
            &receiver_types,
        )?;
        subs.push(sub);
    }

    // ── Fields: __state, outer args, crossing locals, __sub_i, bindings.
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

    // ── Init params + body.
    let mut init_params: Vec<Param> = Vec::new();
    init_params.push(Param {
        auto_assign: true,
        name: "__state".to_string(),
        type_expr: int_type(&span),
        default: None,
        span: span.clone(),
    });
    for p in &func.params {
        init_params.push(Param {
            auto_assign: true,
            name: p.name.clone(),
            type_expr: p.type_expr.clone(),
            default: None,
            span: p.span.clone(),
        });
    }

    let mut init_body_stmts: Vec<Statement> = Vec::new();
    // (1) Pre-await statements (with outer-arg refs → self.<arg>).
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
    // (2) Copy crossing locals into their fields.
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
    for (i, sub) in subs.iter().enumerate() {
        init_body_stmts.push(Statement::Expression(Expr {
            kind: ExprKind::Assign {
                target: Box::new(self_field(&format!("__sub_{i}"), &span)),
                value: Box::new(sub.awaitee_ctor.clone()),
            },
            span: span.clone(),
        }));
    }
    // (4) Default-initialise await-binding fields where synthesisable.
    for sub in &subs {
        if let Some(default) = default_value_for_type(&sub.result_type, &span) {
            init_body_stmts.push(Statement::Expression(Expr {
                kind: ExprKind::Assign {
                    target: Box::new(self_field(&sub.binding_name, &span)),
                    value: Box::new(default),
                },
                span: span.clone(),
            }));
        }
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

    // ── Poll body — unified `self.__state` skeleton.
    let poll_body =
        build_poll_body_cfg(&subs, &tail_stmts, &outer_field_names, &func.params, &span);

    let cx_param = Param {
        auto_assign: false,
        name: "cx".to_string(),
        type_expr: TypeExpr::Reference {
            lifetime: None,
            mutable: true,
            inner: Box::new(named_type("Context", &span)),
            span: span.clone(),
        },
        default: None,
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
            " Compiler-synthesised Future state machine for `{}`. Spec: docs/specs/syntax/async_lowering.spec.md (Path A unified Cfg lowering).",
            func.name
        )],
        where_clause: None,
        span: span.clone(),
    };

    // ── Wrapper fn — constructs the state machine.
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

/// Unified poll-body builder for the non-loop shapes. With `subs`
/// empty it emits the 2A single-state body (`if __state == 0 {
/// __state = 1; Poll.Ready(<tail>) } else { Poll.Pending }`); with N
/// suspends it emits the linear `__state` if/elsif chain identical to
/// the linear `build_linear_state_dispatch`. One algorithm, parameterised
/// on N — the no-await case is just N == 0.
fn build_poll_body_cfg(
    subs: &[AwaitSub],
    tail_stmts: &[Statement],
    outer_field_names: &[String],
    args: &[Param],
    span: &Span,
) -> Block {
    if subs.is_empty() {
        // No-await: 2A single-state body. The tail IS the whole user
        // body; rewrite bare arg refs to `self.<arg>` over `func.params`.
        let arg_names: Vec<String> = args.iter().map(|p| p.name.clone()).collect();
        let mut tail_block = Block {
            statements: tail_stmts.to_vec(),
            span: span.clone(),
        };
        rewrite_arg_refs_in_block(&mut tail_block, &arg_names);
        let body_value_expr = Expr {
            kind: ExprKind::Block(tail_block),
            span: span.clone(),
        };
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
        let poll_pending = Expr {
            kind: ExprKind::EnumVariant {
                type_path: vec!["Poll".to_string()],
                variant: "Pending".to_string(),
                args: Vec::new(),
            },
            span: span.clone(),
        };
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
        return Block {
            statements: vec![Statement::Expression(if_expr)],
            span: span.clone(),
        };
    }

    // Linear-N: the `self.__state` if/elsif dispatch over N suspends.
    build_linear_state_dispatch(subs, tail_stmts, outer_field_names, span)
}

/// Lower an async fn whose body matches [`WhileSingleAwaitShape`] —
/// the v1 closure of E1115 for the canonical server-accept pattern.
///
/// The synth class shape differs from the linear-await path:
///   * `__state: Int` is 0 while looping, 1 once the loop has
///     exited cond-false (the terminal state).
///   * `__sub_ready: Int` flags whether `__sub` is currently in-
///     flight (1) or needs construction (0). Reset to 0 after every
///     successful `Ready` so the next iteration re-constructs.
///   * `__sub: <AwaiteeFutureClass>` is the per-iteration sub-
///     future. Constructed lazily inside `poll`, NOT in `init`.
///   * `<binding>: <ResultTy>` holds the most recent Ready value.
///
/// The synth `poll` body wraps the loop iterations in a Ruxen-level
/// `while keep_iterating` loop: each call to `poll` runs as many
/// iterations as the sub-future allows before either completing all
/// iterations or hitting `Pending`. Spec:
/// `docs/specs/syntax/async_lowering_loop_await.spec.md`.
fn lower_async_fn_while_single_await(
    func: &FuncDef,
    shape: &WhileSingleAwaitShape,
    async_fn_returns: &HashMap<String, (String, TypeExpr)>,
    class_static_returns: &HashMap<(String, String), TypeExpr>,
    class_instance_returns: &HashMap<(String, String), TypeExpr>,
    future_outputs: &HashMap<String, TypeExpr>,
) -> Option<(FuncDef, ClassDef)> {
    let span = func.span.clone();
    let class_name = mangle_future_class_name(&func.name);
    let return_type = func.return_type.clone()?;

    // ── Phase 1 — collect outer-field names ─────────────────────────
    //
    // For the loop-await path, EVERY pre-loop `let`/`var` is
    // promoted to a class field — the post-await body almost
    // certainly references at least one of them (loop condition var,
    // accumulator), and the analysis required to detect this
    // precisely isn't worth the complexity for v1. Untyped pre-loop
    // locals → bail (we need a typed field). The user can add a
    // type annotation.
    let outer_arg_names: Vec<String> = func.params.iter().map(|p| p.name.clone()).collect();
    let mut crossing_locals: Vec<(String, TypeExpr)> = Vec::new();
    for s in &shape.pre_loop_stmts {
        if let Statement::Let(lb) = s {
            if let Pattern::Identifier { name, .. } = &lb.pattern {
                let ty = lb.type_annotation.clone()?;
                crossing_locals.push((name.clone(), ty));
            }
        }
    }
    let crossing_names: Vec<String> = crossing_locals.iter().map(|(n, _)| n.clone()).collect();
    let mut outer_field_names: Vec<String> = outer_arg_names.clone();
    outer_field_names.extend(crossing_names.iter().cloned());

    // Receiver-type map for Shape-3 (instance-method awaitee) lookup.
    let mut receiver_types: HashMap<String, String> = HashMap::new();
    for p in &func.params {
        if let Some(name) = single_named_class(&p.type_expr) {
            receiver_types.insert(p.name.clone(), name);
        }
    }
    for (name, ty) in &crossing_locals {
        if let Some(cls) = single_named_class(ty) {
            receiver_types.insert(name.clone(), cls);
        }
    }

    // ── Phase 2 — describe the body's `.await` ──────────────────────
    let sub = describe_await(
        &shape.loop_await_let,
        async_fn_returns,
        class_static_returns,
        class_instance_returns,
        future_outputs,
        &outer_field_names,
        &receiver_types,
    )?;

    // ── Phase 3 — fields ────────────────────────────────────────────
    //
    // Field order MUST match the linear-await lowering's convention
    // so the post-lowering MIR layout shares the same `(class_info,
    // refcount, fields)` shift pattern: __state first, then outer
    // params, then crossing locals, then __sub_ready + __sub, then
    // the binding. Empirically, putting the await binding BEFORE the
    // crossing locals corrupts the field-offset table the Ruxen
    // class metadata builds during pass-1 — fixture 728c then reads
    // self.i as garbage on first poll.
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
    for (name, ty) in &crossing_locals {
        fields.push(FieldDecl {
            visibility: Visibility::Public,
            name: name.clone(),
            type_expr: ty.clone(),
            span: span.clone(),
        });
    }
    fields.push(FieldDecl {
        visibility: Visibility::Public,
        name: "__sub_ready".to_string(),
        type_expr: int_type(&span),
        span: span.clone(),
    });
    fields.push(FieldDecl {
        visibility: Visibility::Public,
        name: "__sub".to_string(),
        type_expr: named_type(&sub.sub_class_name, &span),
        span: span.clone(),
    });
    fields.push(FieldDecl {
        visibility: Visibility::Public,
        name: sub.binding_name.clone(),
        type_expr: sub.result_type.clone(),
        span: span.clone(),
    });
    // ── Phase 4 — init params + body ───────────────────────────────
    let mut init_params: Vec<Param> = Vec::new();
    init_params.push(Param {
        auto_assign: true,
        name: "__state".to_string(),
        type_expr: int_type(&span),
        default: None,
        span: span.clone(),
    });
    for p in &func.params {
        init_params.push(Param {
            auto_assign: true,
            name: p.name.clone(),
            type_expr: p.type_expr.clone(),
            default: None,
            span: p.span.clone(),
        });
    }

    let mut init_body_stmts: Vec<Statement> = Vec::new();
    // (1) __sub_ready = 0 so the first iteration constructs __sub.
    init_body_stmts.push(Statement::Expression(Expr {
        kind: ExprKind::Assign {
            target: Box::new(self_field("__sub_ready", &span)),
            value: Box::new(Expr {
                kind: ExprKind::IntLiteral(0, None),
                span: span.clone(),
            }),
        },
        span: span.clone(),
    }));
    // (2) Pre-loop stmts: for each `let <name> = <init>` (where
    // <name> is a crossing local promoted to a field), emit
    // `self.<name> = <init>` directly — bypasses the local-vs-field
    // name-collision risk that having BOTH a `let i = …` and a
    // `self.i = i` in the same init body would introduce.
    // Non-let pre-loop stmts (e.g. side-effecting expression
    // statements) run verbatim with arg-ref rewrites.
    let crossing_set: std::collections::HashSet<String> = crossing_names.iter().cloned().collect();
    for s in &shape.pre_loop_stmts {
        match s {
            Statement::Let(lb) => {
                if let Pattern::Identifier { name, .. } = &lb.pattern {
                    if crossing_set.contains(name) {
                        if let Some(init_val) = &lb.value {
                            let mut rewritten_val = init_val.as_ref().clone();
                            rewrite_arg_refs_in_expr(&mut rewritten_val, &outer_arg_names);
                            init_body_stmts.push(Statement::Expression(Expr {
                                kind: ExprKind::Assign {
                                    target: Box::new(self_field(name, &span)),
                                    value: Box::new(rewritten_val),
                                },
                                span: span.clone(),
                            }));
                            continue;
                        }
                    }
                }
                // Non-crossing let — clone as-is with arg-ref
                // rewrites (matches the existing linear-path
                // pre-await handling).
                let mut rewritten = s.clone();
                if let Statement::Let(lbm) = &mut rewritten {
                    if let Some(v) = lbm.value.as_mut() {
                        rewrite_arg_refs_in_expr(v, &outer_arg_names);
                    }
                }
                init_body_stmts.push(rewritten);
            }
            Statement::Expression(e) => {
                let mut rewritten_e = e.clone();
                rewrite_arg_refs_in_expr(&mut rewritten_e, &outer_arg_names);
                init_body_stmts.push(Statement::Expression(rewritten_e));
            }
        }
    }
    // (4) Default the binding field (so typeck sees a value).
    if let Some(default) = default_value_for_type(&sub.result_type, &span) {
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

    // ── Phase 5 — poll body ────────────────────────────────────────
    let poll_body = build_loop_state_machine_poll_body(
        &sub,
        &shape.loop_cond,
        &shape.body_pre_await,
        &shape.body_post_await,
        &shape.post_loop_stmts,
        &outer_field_names,
        &return_type,
        &span,
    );
    let cx_param = Param {
        auto_assign: false,
        name: "cx".to_string(),
        type_expr: TypeExpr::Reference {
            lifetime: None,
            mutable: true,
            inner: Box::new(named_type("Context", &span)),
            span: span.clone(),
        },
        default: None,
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
            " Compiler-synthesised Future state machine for `{}` (while-single-await shape). Spec: docs/specs/syntax/async_lowering_loop_await.spec.md.",
            func.name
        )],
        where_clause: None,
        span: span.clone(),
    };

    // ── Phase 6 — wrapper fn ───────────────────────────────────────
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

/// Lower an async fn whose body matches [`WhileMultiAwaitShape`] —
/// a `while` with 2+ `.await`s per iteration. The state machine
/// adds a `__phase: Int` field so the poll body can resume in
/// the middle of an iteration after a Pending exit. Within one
/// outer iteration we walk through phases 0..N-1; on phase N-1's
/// Ready arm we run the post-last-await stmts and wrap back to
/// phase 0 + loop cond.
fn lower_async_fn_while_multi_await(
    func: &FuncDef,
    shape: &WhileMultiAwaitShape,
    async_fn_returns: &HashMap<String, (String, TypeExpr)>,
    class_static_returns: &HashMap<(String, String), TypeExpr>,
    class_instance_returns: &HashMap<(String, String), TypeExpr>,
    future_outputs: &HashMap<String, TypeExpr>,
) -> Option<(FuncDef, ClassDef)> {
    let span = func.span.clone();
    let class_name = mangle_future_class_name(&func.name);
    let return_type = func.return_type.clone()?;

    // ── Phase 1 — outer-field set + crossing-locals ────────────────
    let outer_arg_names: Vec<String> = func.params.iter().map(|p| p.name.clone()).collect();
    let mut crossing_locals: Vec<(String, TypeExpr)> = Vec::new();
    for s in &shape.pre_loop_stmts {
        if let Statement::Let(lb) = s {
            if let Pattern::Identifier { name, .. } = &lb.pattern {
                let ty = lb.type_annotation.clone()?;
                crossing_locals.push((name.clone(), ty));
            }
        }
    }
    let crossing_names: Vec<String> = crossing_locals.iter().map(|(n, _)| n.clone()).collect();
    let mut outer_field_names: Vec<String> = outer_arg_names.clone();
    outer_field_names.extend(crossing_names.iter().cloned());

    // ── Receiver-type map ─────────────────────────────────────────
    let mut receiver_types: HashMap<String, String> = HashMap::new();
    for p in &func.params {
        if let Some(name) = single_named_class(&p.type_expr) {
            receiver_types.insert(p.name.clone(), name);
        }
    }
    for (name, ty) in &crossing_locals {
        if let Some(cls) = single_named_class(ty) {
            receiver_types.insert(name.clone(), cls);
        }
    }

    // ── Phase 2 — describe each phase's `.await` ──────────────────
    // For phase i, the outer_field_names available to describe_await
    // include outer_field_names PLUS the previous phases' binding
    // names (because phase i's awaitee_ctor may reference binding 0
    // through i-1). We extend incrementally.
    let mut accumulated_fields: Vec<String> = outer_field_names.clone();
    let mut subs: Vec<AwaitSub> = Vec::new();
    for phase in &shape.phases {
        let sub = describe_await(
            &phase.await_let,
            async_fn_returns,
            class_static_returns,
            class_instance_returns,
            future_outputs,
            &accumulated_fields,
            &receiver_types,
        )?;
        accumulated_fields.push(sub.binding_name.clone());
        subs.push(sub);
    }
    let binding_names: Vec<String> = subs.iter().map(|s| s.binding_name.clone()).collect();

    // ── Phase 3 — class fields ────────────────────────────────────
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
    for (name, ty) in &crossing_locals {
        fields.push(FieldDecl {
            visibility: Visibility::Public,
            name: name.clone(),
            type_expr: ty.clone(),
            span: span.clone(),
        });
    }
    fields.push(FieldDecl {
        visibility: Visibility::Public,
        name: "__phase".to_string(),
        type_expr: int_type(&span),
        span: span.clone(),
    });
    // __sub_ready_i for each phase
    for i in 0..subs.len() {
        fields.push(FieldDecl {
            visibility: Visibility::Public,
            name: format!("__sub_ready_{i}"),
            type_expr: int_type(&span),
            span: span.clone(),
        });
    }
    // __sub_i for each phase
    for (i, sub) in subs.iter().enumerate() {
        fields.push(FieldDecl {
            visibility: Visibility::Public,
            name: format!("__sub_{i}"),
            type_expr: named_type(&sub.sub_class_name, &span),
            span: span.clone(),
        });
    }
    // bindings for each phase
    for sub in &subs {
        fields.push(FieldDecl {
            visibility: Visibility::Public,
            name: sub.binding_name.clone(),
            type_expr: sub.result_type.clone(),
            span: span.clone(),
        });
    }

    // ── Phase 4 — init ───────────────────────────────────────────
    let mut init_params: Vec<Param> = Vec::new();
    init_params.push(Param {
        auto_assign: true,
        name: "__state".to_string(),
        type_expr: int_type(&span),
        default: None,
        span: span.clone(),
    });
    for p in &func.params {
        init_params.push(Param {
            auto_assign: true,
            name: p.name.clone(),
            type_expr: p.type_expr.clone(),
            default: None,
            span: p.span.clone(),
        });
    }

    let mut init_body_stmts: Vec<Statement> = Vec::new();
    // __phase = 0
    init_body_stmts.push(Statement::Expression(Expr {
        kind: ExprKind::Assign {
            target: Box::new(self_field("__phase", &span)),
            value: Box::new(Expr {
                kind: ExprKind::IntLiteral(0, None),
                span: span.clone(),
            }),
        },
        span: span.clone(),
    }));
    // __sub_ready_i = 0 for each phase
    for i in 0..subs.len() {
        init_body_stmts.push(Statement::Expression(Expr {
            kind: ExprKind::Assign {
                target: Box::new(self_field(&format!("__sub_ready_{i}"), &span)),
                value: Box::new(Expr {
                    kind: ExprKind::IntLiteral(0, None),
                    span: span.clone(),
                }),
            },
            span: span.clone(),
        }));
    }
    // pre_loop_stmts — direct field assign (same shape as single-await path)
    let crossing_set: std::collections::HashSet<String> = crossing_names.iter().cloned().collect();
    for s in &shape.pre_loop_stmts {
        match s {
            Statement::Let(lb) => {
                if let Pattern::Identifier { name, .. } = &lb.pattern {
                    if crossing_set.contains(name) {
                        if let Some(init_val) = &lb.value {
                            let mut rewritten_val = init_val.as_ref().clone();
                            rewrite_arg_refs_in_expr(&mut rewritten_val, &outer_arg_names);
                            init_body_stmts.push(Statement::Expression(Expr {
                                kind: ExprKind::Assign {
                                    target: Box::new(self_field(name, &span)),
                                    value: Box::new(rewritten_val),
                                },
                                span: span.clone(),
                            }));
                            continue;
                        }
                    }
                }
                let mut rewritten = s.clone();
                if let Statement::Let(lbm) = &mut rewritten {
                    if let Some(v) = lbm.value.as_mut() {
                        rewrite_arg_refs_in_expr(v, &outer_arg_names);
                    }
                }
                init_body_stmts.push(rewritten);
            }
            Statement::Expression(e) => {
                let mut rewritten_e = e.clone();
                rewrite_arg_refs_in_expr(&mut rewritten_e, &outer_arg_names);
                init_body_stmts.push(Statement::Expression(rewritten_e));
            }
        }
    }
    // Default-init each binding
    for sub in &subs {
        if let Some(default) = default_value_for_type(&sub.result_type, &span) {
            init_body_stmts.push(Statement::Expression(Expr {
                kind: ExprKind::Assign {
                    target: Box::new(self_field(&sub.binding_name, &span)),
                    value: Box::new(default),
                },
                span: span.clone(),
            }));
        }
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

    // ── Phase 5 — poll body ──────────────────────────────────────
    let poll_body = build_multi_phase_loop_poll_body(
        &subs,
        &shape.phases,
        &shape.loop_cond,
        &shape.post_last_await_stmts,
        &shape.post_loop_stmts,
        &outer_field_names,
        &binding_names,
        &return_type,
        &span,
    );
    let cx_param = Param {
        auto_assign: false,
        name: "cx".to_string(),
        type_expr: TypeExpr::Reference {
            lifetime: None,
            mutable: true,
            inner: Box::new(named_type("Context", &span)),
            span: span.clone(),
        },
        default: None,
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
            " Compiler-synthesised Future state machine for `{}` (while-multi-await shape, {} phases). Spec: docs/specs/syntax/async_lowering_loop_await.spec.md.",
            func.name,
            subs.len(),
        )],
        where_clause: None,
        span: span.clone(),
    };

    // ── Phase 6 — wrapper fn ─────────────────────────────────────
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

/// One iteration of a multi-await loop body. The phase
/// represents the stmts up to and including ONE `.await`. A
/// loop body with N awaits has N phases plus a tail of
/// post-last-await stmts (see [`WhileMultiAwaitShape`]).
struct LoopPhase {
    /// Statements that run before this phase's `.await`. May
    /// reference earlier-phase bindings (via `self.<binding>`
    /// after rewriting). No further `.await`s allowed.
    pre_stmts: Vec<Statement>,
    /// The `let <ident> = <expr>.await` that ends this phase.
    await_let: LetBinding,
}

/// Output of [`recognize_while_multi_await`] — extends the
/// single-await closure to N>=2 awaits per iteration. Used by
/// e.g. an HTTP handler that reads + writes + closes per loop
/// iteration (3 awaits in one loop body).
///
/// Lowered via [`build_multi_phase_loop_poll_body`] into a state
/// machine with a `__phase: Int` field tracking which await we're
/// currently parked on. Within one outer `while keep_iter` loop,
/// each call to `poll` can advance through multiple phases
/// before hitting Pending (one Ready per await per call). On the
/// final phase's Ready arm, the post-last-await stmts run and
/// `__phase` resets to 0 — looping back to phase 0 of the next
/// iteration.
///
/// Spec: `docs/specs/syntax/async_lowering_loop_await.spec.md`
/// §multi-await extension.
pub(crate) struct WhileMultiAwaitShape {
    pre_loop_stmts: Vec<Statement>,
    loop_cond: Expr,
    phases: Vec<LoopPhase>,
    post_last_await_stmts: Vec<Statement>,
    post_loop_stmts: Vec<Statement>,
}

/// Walk `body` looking for a single top-level `while` whose body
/// contains TWO OR MORE `let <ident> = <expr>.await` statements,
/// each at the top level of the body, with arbitrary non-await
/// stmts before/between/after. Returns `Some(shape)` if the
/// pattern matches with phases.len() >= 2; `None` for shapes
/// outside the supported subset (including 0 or 1 awaits — the
/// 1-await case falls through to [`recognize_while_single_await`]).
pub(crate) fn recognize_while_multi_await(body: &Block) -> Option<WhileMultiAwaitShape> {
    let mut while_idx: Option<usize> = None;
    for (i, stmt) in body.statements.iter().enumerate() {
        if let Statement::Expression(e) = stmt {
            if let ExprKind::While(w) = &e.kind {
                if block_contains_await(&w.body) {
                    if while_idx.is_some() {
                        return None;
                    }
                    while_idx = Some(i);
                    continue;
                }
            }
        }
        match stmt {
            Statement::Let(lb) => {
                if let Some(v) = &lb.value {
                    if expr_contains_await(v) {
                        return None;
                    }
                }
            }
            Statement::Expression(e) => {
                if expr_contains_await(e) {
                    return None;
                }
            }
        }
    }
    let widx = while_idx?;
    let pre_loop_stmts: Vec<Statement> = body.statements[..widx].to_vec();
    let post_loop_stmts: Vec<Statement> = body.statements[widx + 1..].to_vec();

    let (loop_cond, w_body) = match &body.statements[widx] {
        Statement::Expression(e) => match &e.kind {
            ExprKind::While(w) => ((*w.condition).clone(), w.body.clone()),
            _ => return None,
        },
        _ => return None,
    };
    if expr_contains_await(&loop_cond) {
        return None;
    }

    let mut phases: Vec<LoopPhase> = Vec::new();
    let mut current_pre: Vec<Statement> = Vec::new();
    let mut post_last_await_stmts: Vec<Statement> = Vec::new();
    let mut last_was_await = false;

    for stmt in &w_body.statements {
        let mut is_await_let = false;
        if let Statement::Let(lb) = stmt {
            if let Some(v) = &lb.value {
                if let ExprKind::Await(_) = &v.kind {
                    if !matches!(&lb.pattern, Pattern::Identifier { .. }) {
                        return None;
                    }
                    is_await_let = true;
                    phases.push(LoopPhase {
                        pre_stmts: std::mem::take(&mut current_pre),
                        await_let: lb.clone(),
                    });
                    last_was_await = true;
                } else if expr_contains_await(v) {
                    return None;
                }
            }
        } else if let Statement::Expression(e) = stmt {
            if expr_contains_await(e) {
                return None;
            }
        }
        if !is_await_let {
            // Non-await stmt: queue into current_pre for the
            // NEXT phase (if another await follows) OR fold
            // into post_last_await_stmts when we exit the loop.
            current_pre.push(stmt.clone());
            last_was_await = false;
        }
    }
    // Any stmts queued after the last await become the post-tail.
    if !last_was_await {
        post_last_await_stmts = current_pre;
    }
    // Multi-await means 2+ phases. N=1 routes to the single path.
    if phases.len() < 2 {
        return None;
    }
    Some(WhileMultiAwaitShape {
        pre_loop_stmts,
        loop_cond,
        phases,
        post_last_await_stmts,
        post_loop_stmts,
    })
}

/// Output of [`recognize_while_single_await`] — the canonical shape
/// for the v1 closure of E1115:
///
/// ```text
/// async def f(args...):
///   <pre_loop_stmts>           # no .await
///   while <loop_cond>:         # no .await in cond
///     <body_pre_await>         # no .await
///     let <binding> = <expr>.await    # the ONE .await
///     <body_post_await>        # no .await
///   end
///   <post_loop_stmts>          # no .await
/// end
/// ```
///
/// Lowered via [`build_loop_state_machine_poll_body`] into an inner
/// Ruxen `while` inside the synth poll body, with per-iteration
/// re-construction of the awaitee sub-future. Spec:
/// `docs/specs/syntax/async_lowering_loop_await.spec.md`.
pub(crate) struct WhileSingleAwaitShape {
    pre_loop_stmts: Vec<Statement>,
    loop_cond: Expr,
    body_pre_await: Vec<Statement>,
    loop_await_let: LetBinding,
    body_post_await: Vec<Statement>,
    post_loop_stmts: Vec<Statement>,
}

/// Walk `body` looking for the canonical "while with a single
/// `.await` in its body" shape. Returns `Some(shape)` if every v1
/// restriction in `WhileSingleAwaitShape` is met, `None` otherwise.
///
/// Restrictions (rejects to None — falls through to the existing
/// linear-await path or the E1115 pre-pass):
///   * Exactly one `Statement::Expression(While)` whose body
///     contains at least one `.await`.
///   * No other top-level statement contains a `.await`.
///   * The while condition contains no `.await`.
///   * Inside the loop body: exactly one `let <Identifier> =
///     <expr>.await` statement. Pre- and post-await body stmts must
///     contain no further `.await`.
///   * Other while/for/loop forms (`loop {…}`, `while let`, `for`)
///     are out of scope for v1 — those still hit the E1115
///     diagnostic.
pub(crate) fn recognize_while_single_await(body: &Block) -> Option<WhileSingleAwaitShape> {
    let mut while_idx: Option<usize> = None;
    for (i, stmt) in body.statements.iter().enumerate() {
        if let Statement::Expression(e) = stmt {
            if let ExprKind::While(w) = &e.kind {
                if block_contains_await(&w.body) {
                    if while_idx.is_some() {
                        return None;
                    }
                    while_idx = Some(i);
                    continue;
                }
            }
        }
        // Any other stmt must not contain an .await.
        match stmt {
            Statement::Let(lb) => {
                if let Some(v) = &lb.value {
                    if expr_contains_await(v) {
                        return None;
                    }
                }
            }
            Statement::Expression(e) => {
                if expr_contains_await(e) {
                    return None;
                }
            }
        }
    }
    let widx = while_idx?;
    let pre_loop_stmts: Vec<Statement> = body.statements[..widx].to_vec();
    let post_loop_stmts: Vec<Statement> = body.statements[widx + 1..].to_vec();

    let (loop_cond, w_body) = match &body.statements[widx] {
        Statement::Expression(e) => match &e.kind {
            ExprKind::While(w) => ((*w.condition).clone(), w.body.clone()),
            _ => return None,
        },
        _ => return None,
    };

    if expr_contains_await(&loop_cond) {
        return None;
    }

    // Walk the loop body looking for the single .await let.
    let mut body_pre_await: Vec<Statement> = Vec::new();
    let mut body_post_await: Vec<Statement> = Vec::new();
    let mut loop_await_let: Option<LetBinding> = None;

    for stmt in &w_body.statements {
        if loop_await_let.is_none() {
            // Pre-await section. Look for the canonical await-let.
            if let Statement::Let(lb) = stmt {
                if let Some(v) = &lb.value {
                    if let ExprKind::Await(_) = &v.kind {
                        if !matches!(&lb.pattern, Pattern::Identifier { .. }) {
                            return None;
                        }
                        loop_await_let = Some(lb.clone());
                        continue;
                    }
                    if expr_contains_await(v) {
                        return None;
                    }
                }
                body_pre_await.push(stmt.clone());
                continue;
            }
            if let Statement::Expression(e) = stmt {
                if expr_contains_await(e) {
                    return None;
                }
                body_pre_await.push(stmt.clone());
                continue;
            }
        } else {
            // Post-await section — reject any further awaits.
            match stmt {
                Statement::Let(lb) => {
                    if let Some(v) = &lb.value {
                        if expr_contains_await(v) {
                            return None;
                        }
                    }
                }
                Statement::Expression(e) => {
                    if expr_contains_await(e) {
                        return None;
                    }
                }
            }
            body_post_await.push(stmt.clone());
        }
    }

    let loop_await_let = loop_await_let?;
    Some(WhileSingleAwaitShape {
        pre_loop_stmts,
        loop_cond,
        body_pre_await,
        loop_await_let,
        body_post_await,
        post_loop_stmts,
    })
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
/// pattern. Downstream `lower_async_fn_via_cfg` returns `None`
/// for the whole function in that case, and the resolver/typeck pass
/// surfaces a follow-up diagnostic.
/// Extract a single-segment class name from a `TypeExpr::Named` path
/// (e.g. `AsyncTcpStream`). Returns `None` for generics, tuples,
/// references, or multi-segment paths — Shape-3 dispatch only handles
/// bare class identifiers.
fn single_named_class(ty: &TypeExpr) -> Option<String> {
    if let TypeExpr::Named(path) = ty {
        if path.segments.len() == 1 {
            return Some(path.segments[0].clone());
        }
    }
    None
}

fn describe_await(
    lb: &LetBinding,
    async_fn_returns: &HashMap<String, (String, TypeExpr)>,
    class_static_returns: &HashMap<(String, String), TypeExpr>,
    class_instance_returns: &HashMap<(String, String), TypeExpr>,
    future_outputs: &HashMap<String, TypeExpr>,
    outer_arg_names: &[String],
    receiver_types: &HashMap<String, String>,
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
    describe_awaitee(
        &binding_name,
        inner,
        async_fn_returns,
        class_static_returns,
        class_instance_returns,
        future_outputs,
        outer_arg_names,
        receiver_types,
    )
}

/// Core awaitee classifier shared by the `LetBinding`-shaped
/// [`describe_await`] (the still-live loop paths) and the CFG-driven
/// [`lower_async_fn_via_cfg`], which already carries the unwrapped
/// `binding` + inner `awaitee` on each [`cfg::Suspend`]. `inner` is the
/// expression INSIDE the `.await` (`g(args)` / `Class.method(args)`),
/// never an `ExprKind::Await`.
#[allow(clippy::too_many_arguments)]
fn describe_awaitee(
    binding_name: &str,
    inner: &Expr,
    async_fn_returns: &HashMap<String, (String, TypeExpr)>,
    class_static_returns: &HashMap<(String, String), TypeExpr>,
    class_instance_returns: &HashMap<(String, String), TypeExpr>,
    future_outputs: &HashMap<String, TypeExpr>,
    outer_arg_names: &[String],
    receiver_types: &HashMap<String, String>,
) -> Option<AwaitSub> {
    let binding_name = binding_name.to_string();
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
        let receiver_ident = match &object.kind {
            ExprKind::Identifier(n) => n.clone(),
            _ => return None,
        };

        // First try Shape 2 (static method on a class name). Then fall
        // through to Shape 3 (instance method on a typed receiver) if
        // the static lookup misses.
        let (ret_ty, awaitee_receiver, is_static): (TypeExpr, ExprKind, bool) = if let Some(rt) =
            class_static_returns
                .get(&(receiver_ident.clone(), method.clone()))
                .cloned()
        {
            (rt, ExprKind::Identifier(receiver_ident.clone()), true)
        } else if let Some(class_name) = receiver_types.get(&receiver_ident) {
            let rt = class_instance_returns
                .get(&(class_name.clone(), method.clone()))
                .cloned()?;
            // For Shape 3 the awaitee receiver stays as the original
            // identifier; the outer-arg-rewrite below promotes it to
            // `self.<name>` when needed.
            (rt, ExprKind::Identifier(receiver_ident.clone()), false)
        } else {
            return None;
        };
        let _ = is_static; // reserved for future divergence in lowering shapes

        let future_class_name = match &ret_ty {
            TypeExpr::Named(path) if path.segments.len() == 1 => path.segments[0].clone(),
            _ => return None,
        };
        let output_ty = future_outputs.get(&future_class_name)?.clone();

        // Preserve the original `<receiver>.method(args)` expression as
        // the sub-future constructor. Both Shape-2 and Shape-3 receivers
        // pass through `rewrite_arg_refs_in_expr` so any outer-field
        // name on the receiver itself (Shape 3) becomes `self.<name>`,
        // matching how state-machine fields are stored.
        let mut rewritten_receiver = Expr {
            kind: awaitee_receiver,
            span: span.clone(),
        };
        rewrite_arg_refs_in_expr(&mut rewritten_receiver, outer_arg_names);

        let mut rewritten_args: Vec<Expr> = Vec::with_capacity(args.len());
        for a in args {
            let mut rewritten = a.clone();
            rewrite_arg_refs_in_expr(&mut rewritten, outer_arg_names);
            rewritten_args.push(rewritten);
        }
        let awaitee_ctor = Expr {
            kind: ExprKind::MethodCall {
                object: Box::new(rewritten_receiver),
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

/// Emit the linear `self.__state` if/elsif dispatch for N≥1 suspends —
/// the linear-N branch of [`build_poll_body_cfg`]. State i polls
/// `self.__sub_i`; on `Ready(v)` it stores `self.<binding_i> = v`, bumps
/// `__state`, and either re-`Pending`s (i < N-1) or folds the terminal
/// `Poll.Ready(<tail>)` (i == N-1). The `else` arm returns `Poll.Pending`
/// forever (poll-after-Ready, per spec B5).
fn build_linear_state_dispatch(
    subs: &[AwaitSub],
    tail_stmts: &[Statement],
    outer_arg_names: &[String],
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
    for (i, arm) in state_arms.iter().enumerate().take(n).skip(1) {
        elsif_clauses.push(ElsifClause {
            condition: Box::new(make_eq(i as i64)),
            body: to_block(arm.clone()),
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

/// Build the poll body for a [`WhileSingleAwaitShape`] — the inner
/// Ruxen `while keep_iterating` loop that drives per-iteration
/// sub-future construction + poll. Spec
/// `docs/specs/syntax/async_lowering_loop_await.spec.md` §2.5.
///
/// Pseudocode emitted (with `<X>` placeholders filled in from
/// `sub`, `loop_cond`, `body_pre_await`, `body_post_await`,
/// `post_loop_stmts`):
///
/// ```text
/// if self.__state != 0
///   return Poll.Pending
/// end
/// var keep_iterating: Bool = true
/// var pending_exit: Bool = false
/// while keep_iterating
///   if <cond_with_self_refs>
///     if self.__sub_ready == 0
///       <body_pre_await with self.*>
///       self.__sub = <awaitee_ctor>
///       self.__sub_ready = 1
///     end
///     let __p = (&var self.__sub).poll(cx)
///     match __p
///       Poll.Pending ->
///         let _stop = 0
///         pending_exit = true
///         keep_iterating = false
///       Poll.Ready(v) ->
///         let _step = 0
///         self.<binding> = v
///         self.__sub_ready = 0
///         <body_post_await with self.*>
///     end
///   else
///     let _term = 0
///     self.__state = 1
///     keep_iterating = false
///   end
/// end
/// if pending_exit
///   Poll.Pending
/// else
///   <post_loop_stmts with self.*>
///   Poll.Ready(<tail_expr_with_self_refs>)
/// end
/// ```
///
/// The two `var` control flags partition the loop exits: any
/// `Pending` from the sub-future sets `pending_exit = true` so the
/// post-loop conditional returns `Poll.Pending`. Cond going false
/// leaves `pending_exit = false` so the post-loop tail expression
/// runs and we return `Poll.Ready(...)`. Both arms set
/// `keep_iterating = false` to exit the inner while.
fn build_loop_state_machine_poll_body(
    sub: &AwaitSub,
    loop_cond: &Expr,
    body_pre_await: &[Statement],
    body_post_await: &[Statement],
    post_loop_stmts: &[Statement],
    outer_field_names: &[String],
    return_ty: &TypeExpr,
    span: &Span,
) -> Block {
    // Rewrite the cond + body stmts so bare references to outer
    // fields (params + crossing locals) read `self.<name>`. The
    // binding name is also a field, so refs to it (e.g. in
    // body_post_await) get rewritten too.
    let mut field_names = outer_field_names.to_vec();
    field_names.push(sub.binding_name.clone());

    let mut cond = loop_cond.clone();
    rewrite_arg_refs_in_expr(&mut cond, &field_names);

    let mut body_pre_await_rewritten: Vec<Statement> = body_pre_await.to_vec();
    let mut tmp_block = Block {
        statements: body_pre_await_rewritten.clone(),
        span: span.clone(),
    };
    rewrite_arg_refs_in_block(&mut tmp_block, &field_names);
    body_pre_await_rewritten = tmp_block.statements;

    let mut body_post_await_rewritten: Vec<Statement> = body_post_await.to_vec();
    let mut tmp_block = Block {
        statements: body_post_await_rewritten.clone(),
        span: span.clone(),
    };
    rewrite_arg_refs_in_block(&mut tmp_block, &field_names);
    body_post_await_rewritten = tmp_block.statements;

    let mut post_loop_rewritten: Vec<Statement> = post_loop_stmts.to_vec();
    let mut tmp_block = Block {
        statements: post_loop_rewritten.clone(),
        span: span.clone(),
    };
    rewrite_arg_refs_in_block(&mut tmp_block, &field_names);
    post_loop_rewritten = tmp_block.statements;

    // ── Helper: `Poll.Pending` ───────────────────────────────────
    let poll_pending = |span: &Span| Expr {
        kind: ExprKind::EnumVariant {
            type_path: vec!["Poll".to_string()],
            variant: "Pending".to_string(),
            args: Vec::new(),
        },
        span: span.clone(),
    };

    // ── Outer guard: if __state != 0, return Poll.Pending ────────
    let outer_guard_cond = Expr {
        kind: ExprKind::BinaryOp {
            op: BinOp::NotEq,
            left: Box::new(self_field("__state", span)),
            right: Box::new(Expr {
                kind: ExprKind::IntLiteral(0, None),
                span: span.clone(),
            }),
        },
        span: span.clone(),
    };
    let outer_guard = Expr {
        kind: ExprKind::If(IfExpr {
            condition: Box::new(outer_guard_cond),
            then_body: Block {
                statements: vec![Statement::Expression(Expr {
                    kind: ExprKind::Return(Some(Box::new(poll_pending(span)))),
                    span: span.clone(),
                })],
                span: span.clone(),
            },
            elsif_clauses: Vec::new(),
            else_body: None,
            span: span.clone(),
        }),
        span: span.clone(),
    };

    // ── Two control flags ────────────────────────────────────────
    let keep_iter_decl = Statement::Let(LetBinding {
        mutable: true,
        pattern: Pattern::Identifier {
            name: "keep_iterating".to_string(),
            mutable: true,
            span: span.clone(),
        },
        type_annotation: Some(named_type("Bool", span)),
        value: Some(Box::new(Expr {
            kind: ExprKind::BoolLiteral(true),
            span: span.clone(),
        })),
        span: span.clone(),
    });
    let pending_exit_decl = Statement::Let(LetBinding {
        mutable: true,
        pattern: Pattern::Identifier {
            name: "pending_exit".to_string(),
            mutable: true,
            span: span.clone(),
        },
        type_annotation: Some(named_type("Bool", span)),
        value: Some(Box::new(Expr {
            kind: ExprKind::BoolLiteral(false),
            span: span.clone(),
        })),
        span: span.clone(),
    });

    // ── Inner: build the sub-init block (body_pre_await + ctor) ──
    let sub_init_stmts = {
        let mut s: Vec<Statement> = body_pre_await_rewritten.clone();
        // self.__sub = <awaitee_ctor>
        s.push(Statement::Expression(Expr {
            kind: ExprKind::Assign {
                target: Box::new(self_field("__sub", span)),
                value: Box::new(sub.awaitee_ctor.clone()),
            },
            span: span.clone(),
        }));
        // self.__sub_ready = 1
        s.push(Statement::Expression(Expr {
            kind: ExprKind::Assign {
                target: Box::new(self_field("__sub_ready", span)),
                value: Box::new(Expr {
                    kind: ExprKind::IntLiteral(1, None),
                    span: span.clone(),
                }),
            },
            span: span.clone(),
        }));
        s
    };

    // if self.__sub_ready == 0 then <sub_init_stmts> end
    let sub_init_if = Expr {
        kind: ExprKind::If(IfExpr {
            condition: Box::new(Expr {
                kind: ExprKind::BinaryOp {
                    op: BinOp::Eq,
                    left: Box::new(self_field("__sub_ready", span)),
                    right: Box::new(Expr {
                        kind: ExprKind::IntLiteral(0, None),
                        span: span.clone(),
                    }),
                },
                span: span.clone(),
            }),
            then_body: Block {
                statements: sub_init_stmts,
                span: span.clone(),
            },
            elsif_clauses: Vec::new(),
            else_body: None,
            span: span.clone(),
        }),
        span: span.clone(),
    };

    // let __p = (&var self.__sub).poll(cx)
    let sub_borrow = Expr {
        kind: ExprKind::BorrowMut(Box::new(self_field("__sub", span))),
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
    let poll_let = Statement::Let(LetBinding {
        mutable: false,
        pattern: Pattern::Identifier {
            name: "__p".to_string(),
            mutable: false,
            span: span.clone(),
        },
        type_annotation: None,
        value: Some(Box::new(poll_call)),
        span: span.clone(),
    });

    // match __p
    //   Poll.Pending -> { let _stop = 0; pending_exit = true; keep_iterating = false }
    //   Poll.Ready(v) -> { let _step = 0; self.<binding> = v; self.__sub_ready = 0; <body_post_await> }
    // end
    let pending_arm_stmts = vec![
        Statement::Let(LetBinding {
            mutable: false,
            pattern: Pattern::Identifier {
                name: "_stop".to_string(),
                mutable: false,
                span: span.clone(),
            },
            type_annotation: None,
            value: Some(Box::new(Expr {
                kind: ExprKind::IntLiteral(0, None),
                span: span.clone(),
            })),
            span: span.clone(),
        }),
        Statement::Expression(Expr {
            kind: ExprKind::Assign {
                target: Box::new(Expr {
                    kind: ExprKind::Identifier("pending_exit".to_string()),
                    span: span.clone(),
                }),
                value: Box::new(Expr {
                    kind: ExprKind::BoolLiteral(true),
                    span: span.clone(),
                }),
            },
            span: span.clone(),
        }),
        Statement::Expression(Expr {
            kind: ExprKind::Assign {
                target: Box::new(Expr {
                    kind: ExprKind::Identifier("keep_iterating".to_string()),
                    span: span.clone(),
                }),
                value: Box::new(Expr {
                    kind: ExprKind::BoolLiteral(false),
                    span: span.clone(),
                }),
            },
            span: span.clone(),
        }),
    ];

    let mut ready_arm_stmts: Vec<Statement> = Vec::new();
    ready_arm_stmts.push(Statement::Let(LetBinding {
        mutable: false,
        pattern: Pattern::Identifier {
            name: "_step".to_string(),
            mutable: false,
            span: span.clone(),
        },
        type_annotation: None,
        value: Some(Box::new(Expr {
            kind: ExprKind::IntLiteral(0, None),
            span: span.clone(),
        })),
        span: span.clone(),
    }));
    // self.<binding> = v
    ready_arm_stmts.push(Statement::Expression(Expr {
        kind: ExprKind::Assign {
            target: Box::new(self_field(&sub.binding_name, span)),
            value: Box::new(Expr {
                kind: ExprKind::Identifier("v".to_string()),
                span: span.clone(),
            }),
        },
        span: span.clone(),
    }));
    // self.__sub_ready = 0
    ready_arm_stmts.push(Statement::Expression(Expr {
        kind: ExprKind::Assign {
            target: Box::new(self_field("__sub_ready", span)),
            value: Box::new(Expr {
                kind: ExprKind::IntLiteral(0, None),
                span: span.clone(),
            }),
        },
        span: span.clone(),
    }));
    // body_post_await (already self.*-rewritten)
    ready_arm_stmts.extend(body_post_await_rewritten.iter().cloned());

    let match_expr = Expr {
        kind: ExprKind::Match(MatchExpr {
            subject: Box::new(Expr {
                kind: ExprKind::Identifier("__p".to_string()),
                span: span.clone(),
            }),
            arms: vec![
                MatchArm {
                    pattern: Pattern::Enum {
                        path: vec!["Poll".to_string()],
                        variant: "Pending".to_string(),
                        fields: Vec::new(),
                        span: span.clone(),
                    },
                    guard: None,
                    body: MatchArmBody::Block(Block {
                        statements: pending_arm_stmts,
                        span: span.clone(),
                    }),
                    span: span.clone(),
                },
                MatchArm {
                    pattern: Pattern::Enum {
                        path: vec!["Poll".to_string()],
                        variant: "Ready".to_string(),
                        fields: vec![Pattern::Identifier {
                            name: "v".to_string(),
                            mutable: false,
                            span: span.clone(),
                        }],
                        span: span.clone(),
                    },
                    guard: None,
                    body: MatchArmBody::Block(Block {
                        statements: ready_arm_stmts,
                        span: span.clone(),
                    }),
                    span: span.clone(),
                },
            ],
            span: span.clone(),
        }),
        span: span.clone(),
    };

    // The "iteration if-cond is true" block: sub_init_if + poll_let + match_expr
    let cond_true_stmts: Vec<Statement> = vec![
        Statement::Expression(sub_init_if),
        poll_let,
        Statement::Expression(match_expr),
    ];

    // The "cond is false (loop terminating)" block:
    //   let _term = 0; self.__state = 1; keep_iterating = false
    let cond_false_stmts: Vec<Statement> = vec![
        Statement::Let(LetBinding {
            mutable: false,
            pattern: Pattern::Identifier {
                name: "_term".to_string(),
                mutable: false,
                span: span.clone(),
            },
            type_annotation: None,
            value: Some(Box::new(Expr {
                kind: ExprKind::IntLiteral(0, None),
                span: span.clone(),
            })),
            span: span.clone(),
        }),
        Statement::Expression(Expr {
            kind: ExprKind::Assign {
                target: Box::new(self_field("__state", span)),
                value: Box::new(Expr {
                    kind: ExprKind::IntLiteral(1, None),
                    span: span.clone(),
                }),
            },
            span: span.clone(),
        }),
        Statement::Expression(Expr {
            kind: ExprKind::Assign {
                target: Box::new(Expr {
                    kind: ExprKind::Identifier("keep_iterating".to_string()),
                    span: span.clone(),
                }),
                value: Box::new(Expr {
                    kind: ExprKind::BoolLiteral(false),
                    span: span.clone(),
                }),
            },
            span: span.clone(),
        }),
    ];

    let iter_if = Expr {
        kind: ExprKind::If(IfExpr {
            condition: Box::new(cond),
            then_body: Block {
                statements: cond_true_stmts,
                span: span.clone(),
            },
            elsif_clauses: Vec::new(),
            else_body: Some(Block {
                statements: cond_false_stmts,
                span: span.clone(),
            }),
            span: span.clone(),
        }),
        span: span.clone(),
    };

    let while_loop = Expr {
        kind: ExprKind::While(WhileExpr {
            condition: Box::new(Expr {
                kind: ExprKind::Identifier("keep_iterating".to_string()),
                span: span.clone(),
            }),
            body: Block {
                statements: vec![Statement::Expression(iter_if)],
                span: span.clone(),
            },
            span: span.clone(),
        }),
        span: span.clone(),
    };

    // ── Post-loop: if pending_exit then Poll.Pending else <post_loop_stmts>; Poll.Ready(<tail>) ─
    //
    // The tail expression is the last Statement::Expression of
    // post_loop_stmts if any; otherwise the user's async fn body
    // ended at the while and the return type must default-init.
    let (tail_post_stmts, tail_expr): (Vec<Statement>, Option<Expr>) = {
        if post_loop_rewritten.is_empty() {
            (Vec::new(), default_value_for_type(return_ty, span))
        } else {
            // Pull the last statement; if it's a Statement::Expression
            // use it as the Ready arg, otherwise keep all stmts and
            // synthesise a default-init tail expression.
            let mut s = post_loop_rewritten.clone();
            let last = s.pop().unwrap();
            match last {
                Statement::Expression(e) => (s, Some(e)),
                other => {
                    s.push(other);
                    (s, default_value_for_type(return_ty, span))
                }
            }
        }
    };
    let tail_expr = tail_expr.unwrap_or(Expr {
        kind: ExprKind::IntLiteral(0, None),
        span: span.clone(),
    });
    let poll_ready_with_tail = Expr {
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
    let mut ready_branch_stmts: Vec<Statement> = tail_post_stmts;
    ready_branch_stmts.push(Statement::Expression(poll_ready_with_tail));

    let exit_if = Expr {
        kind: ExprKind::If(IfExpr {
            condition: Box::new(Expr {
                kind: ExprKind::Identifier("pending_exit".to_string()),
                span: span.clone(),
            }),
            then_body: Block {
                statements: vec![Statement::Expression(poll_pending(span))],
                span: span.clone(),
            },
            elsif_clauses: Vec::new(),
            else_body: Some(Block {
                statements: ready_branch_stmts,
                span: span.clone(),
            }),
            span: span.clone(),
        }),
        span: span.clone(),
    };

    Block {
        statements: vec![
            Statement::Expression(outer_guard),
            keep_iter_decl,
            pending_exit_decl,
            Statement::Expression(while_loop),
            Statement::Expression(exit_if),
        ],
        span: span.clone(),
    }
}

/// Build the poll body for a [`WhileMultiAwaitShape`] — N>=2
/// `.await`s per iteration. Same outer skeleton as
/// [`build_loop_state_machine_poll_body`] (guard + control vars,
/// `while keep_iter`, and `exit_if`), but the inner-iteration body
/// runs N phase blocks instead of one inline await.
///
/// Each phase block:
/// ```text
/// if self.__phase == i && pending_exit == false
///   if self.__sub_ready_i == 0
///     <phase.pre_stmts with self.*>
///     self.__sub_i = <sub.awaitee_ctor>
///     self.__sub_ready_i = 1
///   end
///   let __pi = (&var self.__sub_i).poll(cx)
///   match __pi
///     Poll.Pending -> pending_exit = true; keep_iter = false
///     Poll.Ready(v) ->
///       self.<binding_i> = v
///       self.__sub_ready_i = 0
///       # last phase only: run post_last_await + reset __phase = 0
///       # else: __phase = i+1
///   end
/// end
/// ```
///
/// On Ready of phase i<N-1, __phase advances and the NEXT phase
/// block executes in the same poll call (no Pending return). On
/// Ready of phase N-1, post_last_await runs + __phase resets to
/// 0 + the OUTER while keep_iter loops back to the cond check.
fn build_multi_phase_loop_poll_body(
    subs: &[AwaitSub],
    phases: &[LoopPhase],
    loop_cond: &Expr,
    post_last_await_stmts: &[Statement],
    post_loop_stmts: &[Statement],
    outer_field_names: &[String],
    binding_names: &[String],
    return_ty: &TypeExpr,
    span: &Span,
) -> Block {
    let mut all_field_names = outer_field_names.to_vec();
    all_field_names.extend(binding_names.iter().cloned());

    let mut cond = loop_cond.clone();
    rewrite_arg_refs_in_expr(&mut cond, &all_field_names);

    let mut post_last_rewritten: Vec<Statement> = post_last_await_stmts.to_vec();
    let mut tmp_block = Block {
        statements: post_last_rewritten.clone(),
        span: span.clone(),
    };
    rewrite_arg_refs_in_block(&mut tmp_block, &all_field_names);
    post_last_rewritten = tmp_block.statements;

    let mut post_loop_rewritten: Vec<Statement> = post_loop_stmts.to_vec();
    let mut tmp_block = Block {
        statements: post_loop_rewritten.clone(),
        span: span.clone(),
    };
    rewrite_arg_refs_in_block(&mut tmp_block, &all_field_names);
    post_loop_rewritten = tmp_block.statements;

    let poll_pending = |span: &Span| Expr {
        kind: ExprKind::EnumVariant {
            type_path: vec!["Poll".to_string()],
            variant: "Pending".to_string(),
            args: Vec::new(),
        },
        span: span.clone(),
    };

    // Outer guard: if __state != 0 return Pending
    let outer_guard = Expr {
        kind: ExprKind::If(IfExpr {
            condition: Box::new(Expr {
                kind: ExprKind::BinaryOp {
                    op: BinOp::NotEq,
                    left: Box::new(self_field("__state", span)),
                    right: Box::new(Expr {
                        kind: ExprKind::IntLiteral(0, None),
                        span: span.clone(),
                    }),
                },
                span: span.clone(),
            }),
            then_body: Block {
                statements: vec![Statement::Expression(Expr {
                    kind: ExprKind::Return(Some(Box::new(poll_pending(span)))),
                    span: span.clone(),
                })],
                span: span.clone(),
            },
            elsif_clauses: Vec::new(),
            else_body: None,
            span: span.clone(),
        }),
        span: span.clone(),
    };

    let keep_iter_decl = Statement::Let(LetBinding {
        mutable: true,
        pattern: Pattern::Identifier {
            name: "keep_iterating".to_string(),
            mutable: true,
            span: span.clone(),
        },
        type_annotation: Some(named_type("Bool", span)),
        value: Some(Box::new(Expr {
            kind: ExprKind::BoolLiteral(true),
            span: span.clone(),
        })),
        span: span.clone(),
    });
    let pending_exit_decl = Statement::Let(LetBinding {
        mutable: true,
        pattern: Pattern::Identifier {
            name: "pending_exit".to_string(),
            mutable: true,
            span: span.clone(),
        },
        type_annotation: Some(named_type("Bool", span)),
        value: Some(Box::new(Expr {
            kind: ExprKind::BoolLiteral(false),
            span: span.clone(),
        })),
        span: span.clone(),
    });

    // Build phase blocks as an if/elsif chain over `__phase` so
    // each inner-while iteration runs AT MOST ONE phase body.
    // This keeps `cx` from being moved-into-poll in two
    // independent if-branches in one poll call (Ruxen's borrow
    // analysis doesn't see that __phase is mutually exclusive
    // across independent ifs). The trade-off: each phase
    // advance costs an extra inner-while iteration; harmless on
    // hot paths because the next-phase poll runs in the same
    // outer block_on iteration (no extra reactor park needed).
    let mut phase_bodies: Vec<Block> = Vec::new();
    for (i, (phase, sub)) in phases.iter().zip(subs.iter()).enumerate() {
        let is_last = i + 1 == phases.len();

        // sub-init: pre_stmts (rewritten) + __sub_i = ctor + __sub_ready_i = 1
        let mut sub_init_inner: Vec<Statement> = Vec::new();
        let mut pre_rewritten: Vec<Statement> = phase.pre_stmts.clone();
        let mut pre_block = Block {
            statements: pre_rewritten.clone(),
            span: span.clone(),
        };
        rewrite_arg_refs_in_block(&mut pre_block, &all_field_names);
        pre_rewritten = pre_block.statements;
        sub_init_inner.extend(pre_rewritten);
        sub_init_inner.push(Statement::Expression(Expr {
            kind: ExprKind::Assign {
                target: Box::new(self_field(&format!("__sub_{i}"), span)),
                value: Box::new(sub.awaitee_ctor.clone()),
            },
            span: span.clone(),
        }));
        sub_init_inner.push(Statement::Expression(Expr {
            kind: ExprKind::Assign {
                target: Box::new(self_field(&format!("__sub_ready_{i}"), span)),
                value: Box::new(Expr {
                    kind: ExprKind::IntLiteral(1, None),
                    span: span.clone(),
                }),
            },
            span: span.clone(),
        }));

        let sub_init_if = Expr {
            kind: ExprKind::If(IfExpr {
                condition: Box::new(Expr {
                    kind: ExprKind::BinaryOp {
                        op: BinOp::Eq,
                        left: Box::new(self_field(&format!("__sub_ready_{i}"), span)),
                        right: Box::new(Expr {
                            kind: ExprKind::IntLiteral(0, None),
                            span: span.clone(),
                        }),
                    },
                    span: span.clone(),
                }),
                then_body: Block {
                    statements: sub_init_inner,
                    span: span.clone(),
                },
                elsif_clauses: Vec::new(),
                else_body: None,
                span: span.clone(),
            }),
            span: span.clone(),
        };

        // poll the sub
        let poll_call = Expr {
            kind: ExprKind::MethodCall {
                object: Box::new(Expr {
                    kind: ExprKind::BorrowMut(Box::new(self_field(&format!("__sub_{i}"), span))),
                    span: span.clone(),
                }),
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
        let poll_let_name = format!("__p{i}");
        let poll_let = Statement::Let(LetBinding {
            mutable: false,
            pattern: Pattern::Identifier {
                name: poll_let_name.clone(),
                mutable: false,
                span: span.clone(),
            },
            type_annotation: None,
            value: Some(Box::new(poll_call)),
            span: span.clone(),
        });

        // Pending arm: pending_exit=true, keep_iterating=false
        let pending_arm_stmts = vec![
            Statement::Let(LetBinding {
                mutable: false,
                pattern: Pattern::Identifier {
                    name: format!("_stop{i}"),
                    mutable: false,
                    span: span.clone(),
                },
                type_annotation: None,
                value: Some(Box::new(Expr {
                    kind: ExprKind::IntLiteral(0, None),
                    span: span.clone(),
                })),
                span: span.clone(),
            }),
            Statement::Expression(Expr {
                kind: ExprKind::Assign {
                    target: Box::new(Expr {
                        kind: ExprKind::Identifier("pending_exit".to_string()),
                        span: span.clone(),
                    }),
                    value: Box::new(Expr {
                        kind: ExprKind::BoolLiteral(true),
                        span: span.clone(),
                    }),
                },
                span: span.clone(),
            }),
            Statement::Expression(Expr {
                kind: ExprKind::Assign {
                    target: Box::new(Expr {
                        kind: ExprKind::Identifier("keep_iterating".to_string()),
                        span: span.clone(),
                    }),
                    value: Box::new(Expr {
                        kind: ExprKind::BoolLiteral(false),
                        span: span.clone(),
                    }),
                },
                span: span.clone(),
            }),
        ];

        // Ready arm
        let mut ready_arm_stmts: Vec<Statement> = Vec::new();
        ready_arm_stmts.push(Statement::Let(LetBinding {
            mutable: false,
            pattern: Pattern::Identifier {
                name: format!("_adv{i}"),
                mutable: false,
                span: span.clone(),
            },
            type_annotation: None,
            value: Some(Box::new(Expr {
                kind: ExprKind::IntLiteral(0, None),
                span: span.clone(),
            })),
            span: span.clone(),
        }));
        ready_arm_stmts.push(Statement::Expression(Expr {
            kind: ExprKind::Assign {
                target: Box::new(self_field(&sub.binding_name, span)),
                value: Box::new(Expr {
                    kind: ExprKind::Identifier("v".to_string()),
                    span: span.clone(),
                }),
            },
            span: span.clone(),
        }));
        ready_arm_stmts.push(Statement::Expression(Expr {
            kind: ExprKind::Assign {
                target: Box::new(self_field(&format!("__sub_ready_{i}"), span)),
                value: Box::new(Expr {
                    kind: ExprKind::IntLiteral(0, None),
                    span: span.clone(),
                }),
            },
            span: span.clone(),
        }));
        if is_last {
            // Run post_last_await stmts + reset __phase to 0
            ready_arm_stmts.extend(post_last_rewritten.iter().cloned());
            ready_arm_stmts.push(Statement::Expression(Expr {
                kind: ExprKind::Assign {
                    target: Box::new(self_field("__phase", span)),
                    value: Box::new(Expr {
                        kind: ExprKind::IntLiteral(0, None),
                        span: span.clone(),
                    }),
                },
                span: span.clone(),
            }));
        } else {
            // Advance __phase to i+1
            ready_arm_stmts.push(Statement::Expression(Expr {
                kind: ExprKind::Assign {
                    target: Box::new(self_field("__phase", span)),
                    value: Box::new(Expr {
                        kind: ExprKind::IntLiteral((i + 1) as i64, None),
                        span: span.clone(),
                    }),
                },
                span: span.clone(),
            }));
        }

        let match_expr = Expr {
            kind: ExprKind::Match(MatchExpr {
                subject: Box::new(Expr {
                    kind: ExprKind::Identifier(poll_let_name.clone()),
                    span: span.clone(),
                }),
                arms: vec![
                    MatchArm {
                        pattern: Pattern::Enum {
                            path: vec!["Poll".to_string()],
                            variant: "Pending".to_string(),
                            fields: Vec::new(),
                            span: span.clone(),
                        },
                        guard: None,
                        body: MatchArmBody::Block(Block {
                            statements: pending_arm_stmts,
                            span: span.clone(),
                        }),
                        span: span.clone(),
                    },
                    MatchArm {
                        pattern: Pattern::Enum {
                            path: vec!["Poll".to_string()],
                            variant: "Ready".to_string(),
                            fields: vec![Pattern::Identifier {
                                name: "v".to_string(),
                                mutable: false,
                                span: span.clone(),
                            }],
                            span: span.clone(),
                        },
                        guard: None,
                        body: MatchArmBody::Block(Block {
                            statements: ready_arm_stmts,
                            span: span.clone(),
                        }),
                        span: span.clone(),
                    },
                ],
                span: span.clone(),
            }),
            span: span.clone(),
        };

        // Collect this phase's body block (sub_init_if + poll +
        // match). We'll thread it into the if/elsif chain below.
        phase_bodies.push(Block {
            statements: vec![
                Statement::Expression(sub_init_if),
                poll_let,
                Statement::Expression(match_expr),
            ],
            span: span.clone(),
        });
    }

    // Assemble: if __phase == 0 { phase_bodies[0] }
    //           elsif __phase == 1 { phase_bodies[1] }
    //           ...
    let phase_eq_for = |i: usize, span: &Span| Expr {
        kind: ExprKind::BinaryOp {
            op: BinOp::Eq,
            left: Box::new(self_field("__phase", span)),
            right: Box::new(Expr {
                kind: ExprKind::IntLiteral(i as i64, None),
                span: span.clone(),
            }),
        },
        span: span.clone(),
    };
    let mut elsif_clauses: Vec<ElsifClause> = Vec::new();
    for (i, body) in phase_bodies.iter().enumerate().skip(1) {
        elsif_clauses.push(ElsifClause {
            condition: Box::new(phase_eq_for(i, span)),
            body: body.clone(),
            span: span.clone(),
        });
    }
    let phase_chain = Expr {
        kind: ExprKind::If(IfExpr {
            condition: Box::new(phase_eq_for(0, span)),
            then_body: phase_bodies[0].clone(),
            elsif_clauses,
            else_body: None,
            span: span.clone(),
        }),
        span: span.clone(),
    };

    // cond_true wraps the phase chain
    let cond_true = Block {
        statements: vec![Statement::Expression(phase_chain)],
        span: span.clone(),
    };

    // cond_false: __state = 1, keep_iterating = false
    let cond_false = Block {
        statements: vec![
            Statement::Let(LetBinding {
                mutable: false,
                pattern: Pattern::Identifier {
                    name: "_term".to_string(),
                    mutable: false,
                    span: span.clone(),
                },
                type_annotation: None,
                value: Some(Box::new(Expr {
                    kind: ExprKind::IntLiteral(0, None),
                    span: span.clone(),
                })),
                span: span.clone(),
            }),
            Statement::Expression(Expr {
                kind: ExprKind::Assign {
                    target: Box::new(self_field("__state", span)),
                    value: Box::new(Expr {
                        kind: ExprKind::IntLiteral(1, None),
                        span: span.clone(),
                    }),
                },
                span: span.clone(),
            }),
            Statement::Expression(Expr {
                kind: ExprKind::Assign {
                    target: Box::new(Expr {
                        kind: ExprKind::Identifier("keep_iterating".to_string()),
                        span: span.clone(),
                    }),
                    value: Box::new(Expr {
                        kind: ExprKind::BoolLiteral(false),
                        span: span.clone(),
                    }),
                },
                span: span.clone(),
            }),
        ],
        span: span.clone(),
    };

    let iter_if = Expr {
        kind: ExprKind::If(IfExpr {
            condition: Box::new(cond),
            then_body: cond_true,
            elsif_clauses: Vec::new(),
            else_body: Some(cond_false),
            span: span.clone(),
        }),
        span: span.clone(),
    };

    let while_loop = Expr {
        kind: ExprKind::While(WhileExpr {
            condition: Box::new(Expr {
                kind: ExprKind::Identifier("keep_iterating".to_string()),
                span: span.clone(),
            }),
            body: Block {
                statements: vec![Statement::Expression(iter_if)],
                span: span.clone(),
            },
            span: span.clone(),
        }),
        span: span.clone(),
    };

    // Post-loop: if pending_exit then Pending else post_loop + Poll.Ready(tail)
    let (tail_post_stmts, tail_expr): (Vec<Statement>, Option<Expr>) = {
        if post_loop_rewritten.is_empty() {
            (Vec::new(), default_value_for_type(return_ty, span))
        } else {
            let mut s = post_loop_rewritten.clone();
            let last = s.pop().unwrap();
            match last {
                Statement::Expression(e) => (s, Some(e)),
                other => {
                    s.push(other);
                    (s, default_value_for_type(return_ty, span))
                }
            }
        }
    };
    let tail_expr = tail_expr.unwrap_or(Expr {
        kind: ExprKind::IntLiteral(0, None),
        span: span.clone(),
    });
    let poll_ready_with_tail = Expr {
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
    let mut ready_branch_stmts: Vec<Statement> = tail_post_stmts;
    ready_branch_stmts.push(Statement::Expression(poll_ready_with_tail));

    let exit_if = Expr {
        kind: ExprKind::If(IfExpr {
            condition: Box::new(Expr {
                kind: ExprKind::Identifier("pending_exit".to_string()),
                span: span.clone(),
            }),
            then_body: Block {
                statements: vec![Statement::Expression(poll_pending(span))],
                span: span.clone(),
            },
            elsif_clauses: Vec::new(),
            else_body: Some(Block {
                statements: ready_branch_stmts,
                span: span.clone(),
            }),
            span: span.clone(),
        }),
        span: span.clone(),
    };

    Block {
        statements: vec![
            Statement::Expression(outer_guard),
            keep_iter_decl,
            pending_exit_decl,
            Statement::Expression(while_loop),
            Statement::Expression(exit_if),
        ],
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
            let scalar = match path.segments[0].as_str() {
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
            if scalar.is_some() {
                return scalar;
            }
            // Generic-wrapped defaults for the canonical Future Output
            // shapes — Option[T] and Result[T, E]. Without these, B2b
            // Shape-3 awaits bail at the (4) default-value step in
            // `lower_async_fn_via_cfg` whenever the awaited
            // future's Output is generic (every IO-style future).
            //
            // Defaults chosen by safety not by semantic meaning: the
            // poll-Ready arm overwrites the binding field with the
            // actual value before any user code reads it. We need a
            // well-typed placeholder, not the "right" value.
            //   - Option[T] → Option.None (no payload to default).
            //   - Result[T, E] → Result.Err(default(E)) IF E defaults,
            //     else Result.Ok(default(T)) IF T defaults. Falls
            //     through to None when neither side defaults (rare —
            //     future Outputs almost always wrap a scalar payload).
            if path.segments[0].as_str() == "Option" {
                return Some(Expr {
                    kind: ExprKind::EnumVariant {
                        type_path: vec!["Option".to_string()],
                        variant: "None".to_string(),
                        args: Vec::new(),
                    },
                    span: span.clone(),
                });
            }
            if path.segments[0].as_str() == "Result" {
                if let Some(args) = &path.generic_args {
                    if args.len() == 2 {
                        if let Some(default_err) = default_value_for_type(&args[1], span) {
                            return Some(Expr {
                                kind: ExprKind::EnumVariant {
                                    type_path: vec!["Result".to_string()],
                                    variant: "Err".to_string(),
                                    args: vec![FieldArg {
                                        name: None,
                                        value: default_err,
                                        span: span.clone(),
                                    }],
                                },
                                span: span.clone(),
                            });
                        }
                        if let Some(default_ok) = default_value_for_type(&args[0], span) {
                            return Some(Expr {
                                kind: ExprKind::EnumVariant {
                                    type_path: vec!["Result".to_string()],
                                    variant: "Ok".to_string(),
                                    args: vec![FieldArg {
                                        name: None,
                                        value: default_ok,
                                        span: span.clone(),
                                    }],
                                },
                                span: span.clone(),
                            });
                        }
                    }
                }
                return None;
            }
            return None;
        }
    }
    None
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
/// Detects whether a block/expr contains a `.await` in THIS function's
/// scope. Nested closures have their own async scope, so we override
/// `visit_expr` to NOT recurse into closure bodies (the old code's
/// deliberate opacity) — every other node recurses via the shared
/// exhaustive `walk_expr`, so the variants the old hand-rolled scan
/// missed under its `_ => false` arm (`EnumVariant`, `UnsafeBlock`,
/// `IfLet`, `SafeNav`/`SafeNavCall`, `Range`, `MapLiteral`, `ArrayFill`,
/// `MacroCall`, `Yield`) are now covered. Bug #1.
///
/// Loop/while/while-let/for bodies still count (they recurse via
/// `walk_expr`): an `.await` inside a loop must trigger the await-aware
/// path so the dedicated E1115 pre-pass can surface a clean diagnostic,
/// rather than falling into the no-await path which would leave the
/// `.await` in place and misreport E1110.
struct AwaitScan {
    found: bool,
}

impl Visit for AwaitScan {
    fn visit_expr(&mut self, e: &Expr) {
        if self.found {
            return;
        }
        match &e.kind {
            ExprKind::Await(_) => self.found = true,
            // Closure bodies are a separate async scope — opaque to this scan.
            ExprKind::Closure(_) => {}
            _ => walk_expr(self, e),
        }
    }
}

pub(crate) fn block_contains_await(block: &Block) -> bool {
    let mut s = AwaitScan { found: false };
    s.visit_block(block);
    s.found
}

pub(crate) fn expr_contains_await(expr: &Expr) -> bool {
    let mut s = AwaitScan { found: false };
    s.visit_expr(expr);
    s.found
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
// — the latter requires a "zero value for arbitrary T" Ruxen doesn't
// have, while the former needs no new infrastructure.
//
// Why break-with-value is safe here: MIR's
// `lower_expr/control.rs::HirExprKind::Break` assigns the break value
// into the loop's `result_local` BEFORE running
// `emit_dealloc_loop_locals`, so move semantics for non-Copy payloads
// are preserved. Fixture `tests/release-e2e/cases/54_loop_break_value.rx`
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
// by Ruxen's standard pipeline (see plan doc for the design choice).

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

/// Rewrites every `block_on(future)` call in a (non-async) scope to the
/// inline poll-loop block via the shared exhaustive `walk_expr_mut`.
///
/// The old hand-rolled `rewrite_block_on_in_expr` matched a fixed set of
/// expression forms and ended with a `_ => {}` arm, so it silently skipped
/// `WhileLet` bodies (and `IfLet`, `SafeNav`/`SafeNavCall`, `Yield`,
/// `MacroCall`) — leaving `block_on(...)` inside them un-rewritten. Bug #2.
///
/// Behaviour preserved exactly: (a) recurse into children FIRST, then
/// transform this node (innermost-out, so `block_on(block_on(x))` rewrites
/// correctly); (b) async closures are opaque — same reasoning as async fns
/// (`block_on` inside async would deadlock; the resolver flags it as E1112).
struct BlockOnRewriter {
    counter: u32,
}

impl VisitMut for BlockOnRewriter {
    fn visit_expr_mut(&mut self, e: &mut Expr) {
        // Async closures are a separate scope and are left untouched so the
        // resolver can flag a `block_on` inside them (E1112), matching the
        // old walker's `if !c.is_async` guard.
        if let ExprKind::Closure(c) = &e.kind {
            if c.is_async {
                return;
            }
        }

        // Children first (innermost-out rewrite order).
        walk_expr_mut(self, e);

        // Then this node: `block_on(EXPR)` with exactly one arg → poll-loop.
        if let Some(future_expr) = take_block_on_argument(e) {
            self.counter += 1;
            let n = self.counter;
            let span = e.span.clone();
            *e = build_block_on_loop(future_expr, n, &span);
        }
    }
}

/// If `expr` is a single-argument `block_on(future)` free call, steal the
/// `future` argument out of it and return it (leaving `expr` as a temporary
/// `NullLiteral` placeholder the caller is expected to overwrite). Returns
/// `None` for any other expression.
fn take_block_on_argument(expr: &mut Expr) -> Option<Expr> {
    let is_target = matches!(
        &expr.kind,
        ExprKind::Call { callee, args, block: None }
            if matches!(&callee.kind, ExprKind::Identifier(name) if name == "block_on")
                && args.len() == 1
    );
    if !is_target {
        return None;
    }
    let span = expr.span.clone();
    let mut owned = std::mem::replace(
        expr,
        Expr {
            kind: ExprKind::NullLiteral,
            span,
        },
    );
    match &mut owned.kind {
        ExprKind::Call { args, .. } => Some(args.remove(0)),
        _ => unreachable!("guarded by is_target above"),
    }
}

fn rewrite_block_on_in_block(block: &mut Block, counter: &mut u32) {
    let mut r = BlockOnRewriter { counter: *counter };
    r.visit_block_mut(block);
    *counter = r.counter;
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
///         Poll.Pending ->
///           let __block_on_pumped_N = ruxen_executor_pump_tasks()
///           let __block_on_has_ready_N = ruxen_executor_ready_nonempty()
///           if __block_on_pumped_N == 0 && __block_on_has_ready_N == 0
///             Thread.yield_now
///           end
///       end
///     end
///   }
fn build_block_on_loop(future_expr: Expr, n: u32, span: &Span) -> Expr {
    let fut_name = format!("__block_on_fut_{n}");
    let ctx_name = format!("__block_on_ctx_{n}");
    let v_name = format!("__block_on_v_{n}");
    let pumped_name = format!("__block_on_pumped_{n}");
    let has_ready_name = format!("__block_on_has_ready_{n}");

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
    // library/std/future/src/lib.rx. Ruxen's parser treats
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

    // Match arm:
    //   Poll.Pending ->
    //     let __block_on_pumped_N = ruxen_executor_pump_tasks()
    //     let __block_on_has_ready_N = ruxen_executor_ready_nonempty()
    //     if __block_on_pumped_N == 0 && __block_on_has_ready_N == 0
    //       Thread.yield_now
    //     end
    //
    // Sub-phase 4A (docs/specs/stdlib/async_io.spec.md B2) replaces
    // the sched_yield-spin inside `Thread.yield_now` itself with a
    // park-on-reactor in `library/std/sync/runtime/thread.c`. The
    // AST emitted here is unchanged — the C-side `ruxen_thread_yield`
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
    // Sub-phase 5 (docs/specs/stdlib/task_spawn.spec.md §B3):
    // pump the spawned-task queue once per iteration. The helper
    // short-circuits via a thread-local null-pointer check when no
    // tasks were ever spawned, so block_on calls that never use
    // Task.spawn pay one extra C call per iteration (a single
    // load + compare + return) — negligible vs. the existing
    // Thread.yield_now / reactor-park cost.
    //
    // Emitted as `ruxen_executor_pump_tasks()` (free-fn lib decl in
    // library/std/future/src/lib.rx). Same mechanism as the
    // Pending arm's `Thread.yield_now` call — identifier-callee
    // synthesis with no implicit self.
    let make_pump_call = || Expr {
        kind: ExprKind::Call {
            callee: Box::new(Expr {
                kind: ExprKind::Identifier("ruxen_executor_pump_tasks".to_string()),
                span: span.clone(),
            }),
            args: Vec::new(),
            block: None,
        },
        span: span.clone(),
    };

    let make_ready_nonempty_call = || Expr {
        kind: ExprKind::Call {
            callee: Box::new(Expr {
                kind: ExprKind::Identifier("ruxen_executor_ready_nonempty".to_string()),
                span: span.clone(),
            }),
            args: Vec::new(),
            block: None,
        },
        span: span.clone(),
    };

    // Pump again immediately before deciding whether to park on
    // Pending. This is required for tasks spawned by the root future
    // during the same poll: the iteration-start pump has already run,
    // so parking here could otherwise sleep before the new task ever
    // gets its first poll. If the pump completed work, or if tasks
    // remain ready, skip Thread.yield_now and continue the block_on
    // loop so the root future and spawned tasks are re-polled. The
    // executor parks when there was no task completion and no ready
    // queued task; reactor fd/timer events or explicit Waker.wake
    // calls mark tasks ready again.
    let let_pumped = Statement::Let(LetBinding {
        mutable: false,
        pattern: Pattern::Identifier {
            mutable: false,
            name: pumped_name.clone(),
            span: span.clone(),
        },
        type_annotation: None,
        value: Some(Box::new(make_pump_call())),
        span: span.clone(),
    });
    let let_has_ready = Statement::Let(LetBinding {
        mutable: false,
        pattern: Pattern::Identifier {
            mutable: false,
            name: has_ready_name.clone(),
            span: span.clone(),
        },
        type_annotation: None,
        value: Some(Box::new(make_ready_nonempty_call())),
        span: span.clone(),
    });
    let no_pumped = Expr {
        kind: ExprKind::BinaryOp {
            left: Box::new(Expr {
                kind: ExprKind::Identifier(pumped_name.clone()),
                span: span.clone(),
            }),
            op: BinOp::Eq,
            right: Box::new(Expr {
                kind: ExprKind::IntLiteral(0, None),
                span: span.clone(),
            }),
        },
        span: span.clone(),
    };
    let no_ready = Expr {
        kind: ExprKind::BinaryOp {
            left: Box::new(Expr {
                kind: ExprKind::Identifier(has_ready_name.clone()),
                span: span.clone(),
            }),
            op: BinOp::Eq,
            right: Box::new(Expr {
                kind: ExprKind::IntLiteral(0, None),
                span: span.clone(),
            }),
        },
        span: span.clone(),
    };
    let should_park = Expr {
        kind: ExprKind::BinaryOp {
            left: Box::new(no_pumped),
            op: BinOp::And,
            right: Box::new(no_ready),
        },
        span: span.clone(),
    };
    let park_if_idle = Expr {
        kind: ExprKind::If(IfExpr {
            condition: Box::new(should_park),
            then_body: Block {
                statements: vec![Statement::Expression(yield_call)],
                span: span.clone(),
            },
            elsif_clauses: Vec::new(),
            else_body: None,
            span: span.clone(),
        }),
        span: span.clone(),
    };
    let pending_arm = MatchArm {
        pattern: pending_pattern,
        guard: None,
        body: MatchArmBody::Block(Block {
            statements: vec![
                let_pumped,
                let_has_ready,
                Statement::Expression(park_if_idle),
            ],
            span: span.clone(),
        }),
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

    // loop ... end — pump first, then poll the top-level future.
    let loop_expr = Expr {
        kind: ExprKind::Loop(LoopExpr {
            body: Block {
                statements: vec![
                    Statement::Expression(make_pump_call()),
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

    fn parse_program(src: &str) -> Program {
        let mut lx = crate::lexer::Lexer::new(src);
        let toks = lx.tokenize().expect("lex");
        let mut p = crate::parser::Parser::new(toks);
        p.parse().expect("parse")
    }

    /// Counts residual `block_on(..)` calls anywhere in a program via
    /// the shared `Visit` traversal.
    struct BlockOnCounter {
        n: usize,
    }
    impl crate::parser::visit::Visit for BlockOnCounter {
        fn visit_expr(&mut self, e: &Expr) {
            if let ExprKind::Call { callee, .. } = &e.kind {
                if matches!(&callee.kind, ExprKind::Identifier(name) if name == "block_on") {
                    self.n += 1;
                }
            }
            crate::parser::visit::walk_expr(self, e);
        }
    }

    fn count_block_on_calls(program: &Program) -> usize {
        let mut c = BlockOnCounter { n: 0 };
        for item in &program.items {
            if let TopLevelItem::Function(f) = item {
                use crate::parser::visit::Visit;
                c.visit_block(&f.body);
            }
        }
        c.n
    }

    #[test]
    fn block_on_inside_while_let_is_rewritten() {
        // `while let Some(x) = it.next() do block_on(f()) end`
        // The OLD rewrite_block_on_in_expr lacked a WhileLet arm, so the
        // inner block_on(...) call was left un-rewritten. Bug #2.
        let mut program = parse_program(
            r#"
            def f() -> Int
                7
            end
            def main() -> Unit
                let var it = [1].iter()
                while let Some(x) = it.next()
                    block_on(f())
                end
            end
        "#,
        );
        // Sanity: the un-rewritten program DOES contain a block_on call.
        assert_eq!(
            count_block_on_calls(&program),
            1,
            "fixture should start with exactly one block_on call"
        );
        rewrite_block_on_calls(&mut program);
        assert_eq!(
            count_block_on_calls(&program),
            0,
            "block_on inside a while-let body must be rewritten away"
        );
    }
}
