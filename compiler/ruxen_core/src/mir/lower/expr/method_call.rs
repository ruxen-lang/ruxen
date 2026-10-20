//! Method call lowering — `expr.method(args)` and `Class.method(args)`.
//!
//! ## Dispatch structure
//!
//! Lowering walks three logical branches in order:
//!
//! 1. **Static-ctor fast path** (`is_collection_ctor`, ~lines 33-365).
//!    Fires when the call is `.new` / `.with_capacity` / specific
//!    builtin static names. The branch FIRST consults
//!    `lookup_ffi_alias` (the §B1 single-entry alias lookup) — if a
//!    `<Base>_<method>` alias is registered, it dispatches directly
//!    with the user's explicit args, no synthetic `self`. Falls back
//!    to a hardcoded class-name list with specialised remapping
//!    (Vec→Hash legacy names, BufReader suffix routing), then
//!    String/Struct special cases, and finally the
//!    `{ClassName}_init(self, args)` synthesis for any user class
//!    with a `def init` body.
//! 2. **`is_static` decision** (~line 667). For non-`.new` calls the
//!    receiver type drives "static vs instance" via four signals:
//!    builtin static methods, user-defined `def self.X`, the
//!    `default` constant for `T: Default`, and "receiver is a class
//!    identifier" (e.g. `JoinHandle.join_raw(handle)` written in
//!    static style). The decision controls whether `self` is
//!    prepended to `arg_values`.
//! 3. **Final-callee resolution** (~line 1179). Builds the mangled
//!    `ResolvedClass_method` name (with the bufio `_file`/`_tcp`
//!    suffix when applicable) and routes it through
//!    `resolve_ffi_alias_callee` — the wrapper that turns an alias
//!    hit into the C symbol and a miss into the unchanged mangled
//!    name (which the linker then resolves against a user-defined
//!    method).
//!
//! ## §B1 consolidation scope
//!
//! Spec `docs/specs/system/compiler_consolidation.spec.md` §B1
//! proposed collapsing the three branches into one
//! `lower_method_call_via_ffi_alias` entry. The §B1 stop condition
//! was triggered: the static-ctor fast path's `Class_init` synthesis
//! (the `format!("{}_init", type_name)` emit at the bottom of
//! Branch 1), the bufio kind-suffix routing, and the receiver-is-
//! class-identifier signal each have semantics that don't cleanly
//! fold into a generic helper without either preserving the existing
//! complexity (no consolidation win) or losing specialised behaviour
//! (regression).
//!
//! What §B1 DID consolidate:
//!
//! - The "is there an FFI alias for this name?" check now flows
//!   through `lookup_ffi_alias` (mir/lower/mod.rs) — the single
//!   entry. The `self.ffi_alias_map.contains_key(...)` direct probe
//!   that lived at Branch 1's top is gone; the symmetric direct
//!   probe in `fn_call.rs` is also gone.
//! - `resolve_ffi_alias_callee` is now a thin wrapper around
//!   `lookup_ffi_alias` (miss → unchanged-mangled), preserving the
//!   pre-§B1 caller surface.
//!
//! Invariant: no caller outside `mir/lower/mod.rs` accesses
//! `ffi_alias_map` directly.

use super::super::*;

impl<'a> Lowerer<'a> {
    pub(super) fn lower_method_call(&mut self, expr: &HirExpr) -> Result<Option<LocalId>, String> {
        match &expr.kind {
            // ── Method call ─────────────────────────────────────────
            HirExprKind::MethodCall {
                object,
                method,
                method_name,
                args,
                block,
                safe_nav,
                resolved,
                ..
            } => {
                if *safe_nav && is_option_type(&object.ty) {
                    return self.lower_safe_nav(expr);
                }
                let receiver_ty = self.mir_refined_expr_ty(object);
                let type_name = self
                    .receiver_type_name(object)
                    .unwrap_or_else(|| type_name_from_ty(&receiver_ty));

                // ── Universal `to_s` on user-defined types (REQ4) ───────
                // `obj.to_s()` on a class / struct / enum that does NOT
                // define its own `to_s` returns the same `String` that
                // `"#{obj}"` would — we route it through the identical
                // interpolation display dispatch so the two are always
                // byte-identical (derive-Debug structs/enums, user
                // `impl Display`, etc.). A user-defined `to_s` wins: when
                // the type declares one we fall through to the normal
                // mangled `<Type>_to_s` call, which links to their method.
                // Scalar primitives are handled by the mangled-name path
                // (`lang_intrinsics`: `Int_to_s` → `ruxen_int_to_string`),
                // so this intercept is intentionally limited to the
                // user-defined aggregate types.
                if method_name == "to_s"
                    && args.is_empty()
                    && block.is_none()
                    && matches!(
                        object.ty.peel_refs(),
                        Ty::Class { .. } | Ty::Struct { .. } | Ty::Enum { .. }
                    )
                    && !self.class_has_method(&type_name, "to_s")
                {
                    let parts = vec![HirInterpolationPart::Expr {
                        expr: (**object).clone(),
                        spec: crate::lexer::token::FormatSpec::default(),
                    }];
                    return self.lower_interpolation(&parts, &Ty::String);
                }

                // `tap` / `then` / `yield_self` are ordinary `.rx` `mixin
                // Kernel` defaults now (universally included), not a compiler
                // interception. They dispatch through the normal method-call
                // path below to a `{Type}_{method}` body synthesized by
                // `emit_universal_kernel_bodies`. No marker handling here.

                // ── Phase C: dynamic dispatch on `&Mixin` / `&var Mixin` ─
                // When the receiver is statically typed as a
                // runtime-dispatch mixin reference (e.g. `f: &Future`),
                // call the per-mixin-method helper instead of mangling
                // a static `<Class>_<method>`. The helper reads
                // class_info_ptr from slot 0 of self, looks up the
                // mixin's vtable, and indirect-calls the concrete
                // method. Spec docs/specs/types/mixin_vtables.spec.md §B5/§B6.
                if let Some(mixin_name) = self.dyn_mixin_receiver_name(&object.ty) {
                    let mut arg_vals = Vec::with_capacity(args.len() + 1);
                    let self_local = self.lower_expr(object)?;
                    if let Some(s) = self_local {
                        arg_vals.push(MirValue::Use(s));
                    }
                    for a in args {
                        let l = self.lower_expr(a)?;
                        arg_vals.push(local_to_value(l));
                    }
                    let dest = if expr.ty != Ty::Unit && expr.ty != Ty::Never {
                        Some(self.new_temp(expr.ty.clone()))
                    } else {
                        None
                    };
                    self.emit(MirInst::Call {
                        dest,
                        callee: format!("{}_dynamic_{}", mixin_name, method_name),
                        args: arg_vals,
                    });
                    return Ok(dest);
                }

                // Static calls: `Type.new(...)` and every declared
                // `def self.NAME` (static-ness comes from the declaration,
                // `is_user_static_method`). A static bound `as "c_symbol"`
                // dispatches straight to that symbol with no synthetic
                // `self` (`lookup_ffi_alias`, the single alias entry).
                // Module-nested classes (`BufReader.File`) mangle with
                // dot -> underscore, the shape `register_class_lib_method_in`
                // keys the alias map on. A `.new` with no FFI binding
                // allocates and runs `Class_init` (or fills a struct's
                // fields) below; any other static without one takes the
                // regular static dispatch further down.
                let is_new = method_name == "new";
                if is_new || self.is_user_static_method(&type_name, method_name) {
                    let base_type = match type_name.find('[') {
                        Some(pos) => &type_name[..pos],
                        None => type_name.as_str(),
                    };
                    let alias_key = format!("{}_{}", base_type.replace('.', "_"), method_name);
                    if let Some(callee) = self.lookup_ffi_alias(&alias_key) {
                        let obj = self.new_temp(expr.ty.clone());
                        let mut call_args = Vec::with_capacity(args.len());
                        for arg in args {
                            let local = self.lower_expr(arg)?;
                            call_args.push(local_to_value(local));
                        }
                        self.emit(MirInst::Call {
                            dest: Some(obj),
                            callee,
                            args: call_args,
                        });
                        return Ok(Some(obj));
                    }
                }
                if is_new {
                    // Structs have no user-defined `init`. The positional
                    // arguments map directly onto the declared fields, so
                    // we allocate the backing storage and emit one
                    // SetField per argument — no synthetic init function.
                    if matches!(&object.ty, Ty::Struct { .. }) {
                        let obj = self.new_temp(expr.ty.clone());
                        self.emit(MirInst::Alloc {
                            dest: obj,
                            ty: expr.ty.clone(),
                            size: self.alloc_size(&expr.ty),
                        });
                        // Coerce each positional arg to the declared field
                        // width before the width-blind SetField store, so a
                        // bare `Float` (f64) value placed into a `Float32`
                        // field is narrowed to f32 first (Q28). Without this,
                        // an 8-byte f64 store into a 4-byte f32 slot makes the
                        // later f32 GetField read garbage (0).
                        let field_tys = self.lookup_construct_field_types(&expr.ty);
                        for (idx, arg) in args.iter().enumerate() {
                            let mut local = self.lower_expr(arg)?;
                            if let Some(field_ty) = field_tys.get(idx).cloned() {
                                local = self.coerce_to_field_ty(local, &field_ty);
                            }
                            self.emit(MirInst::SetField {
                                base: obj,
                                field_index: idx,
                                value: local_to_value(local),
                            });
                        }
                        return Ok(Some(obj));
                    }

                    let layout = crate::codegen::layout::layout_of(&expr.ty, self.symbols);
                    let obj = self.new_temp(expr.ty.clone());
                    self.emit(MirInst::Alloc {
                        dest: obj,
                        ty: expr.ty.clone(),
                        size: self.alloc_size(&expr.ty),
                    });
                    // Phase B-5: write class_info_ptr at slot 0 before
                    // `ClassName_init` runs (init body may already
                    // try to dispatch a `dyn Mixin` method on
                    // `self`).
                    self.emit_class_info_init(&expr.ty, obj);

                    // Call ClassName_init(self, args...)
                    let mut arg_values = vec![MirValue::Use(obj)];
                    for arg in args {
                        let local = self.lower_expr(arg)?;
                        arg_values.push(local_to_value(local));
                    }
                    let _ = layout; // size used by Alloc internally via layout_of in codegen
                                    // Generic-class monomorphization: when this `.new()`
                                    // constructs a concrete instantiation we specialized
                                    // (e.g. `Box[String]`), call the specialized
                                    // `Box__mono__String_init` so the init body sees the
                                    // concrete field type. Otherwise keep the opaque
                                    // `{Class}_init` fallback.
                    let init_callee = match self.mono_base_for_ty(&expr.ty) {
                        Some(base) => format!("{}_init", base),
                        None => format!("{}_init", type_name.replace('.', "_")),
                    };
                    self.emit(MirInst::Call {
                        dest: None,
                        callee: init_callee,
                        args: arg_values,
                    });
                    return Ok(Some(obj));
                }

                // ── Phase 2 stdlib (#04): HashMap.entry chain ──────────
                // `m.entry(K).or_insert(V)` and `m.entry(K).or_insert_with { || V }`
                // are recognized as a single MIR unit and inlined to:
                //
                //   if !ruxen_hash_contains_key(map, k) {
                //       ruxen_hash_insert(map, k, v);   // discard prior value
                //   }
                //
                // Typeck has already verified the chain shape and the V
                // type — see `infer.rs` MethodCall handler. This emission
                // never materializes an `Entry[K,V]` value at runtime.
                if (method_name == "or_insert" || method_name == "or_insert_with")
                    && matches!(
                        &object.kind,
                        HirExprKind::MethodCall { method_name: m, .. } if m == "entry"
                    )
                {
                    let result = self.inline_entry_or_insert(object, method_name, args, block)?;
                    return Ok(result);
                }

                // ── Inline closure-taking methods ──────────────────────
                // `Array#each` and `Array#select!` with a trailing block
                // inline the block body as a loop; every other receiver and
                // method calls the real method body.
                if let Some(block_expr) = block {
                    if let Some(result) =
                        self.try_inline_closure_method(object, method_name, block_expr)?
                    {
                        return Ok(result);
                    }
                }

                // ── Inline try_op (? operator) ──────────────────────────
                // The ? operator desugars to .try_op(). For Result types:
                // Ok(x) -> extract x and continue; Err(e) -> return Err(e).
                // For Option types: Some(x) -> x; None -> return Err(err)
                // (only when inside a Result-returning function via ok_or).
                if method_name == "try_op" {
                    let obj_local = self.lower_expr(object)?;
                    let scrut = obj_local.unwrap_or_else(|| self.new_temp(Ty::Int));

                    // Read the tag: 0 = Ok/Some, 1 = Err/None
                    let tag = self.new_temp(Ty::Int32);
                    self.emit(MirInst::GetTag {
                        dest: tag,
                        src: scrut,
                    });

                    let ok_block = self.new_block();
                    let err_block = self.new_block();
                    let merge_block = self.new_block();

                    // tag == 0 means Ok
                    let is_ok = self.new_temp(Ty::Bool);
                    self.emit(MirInst::Compare {
                        dest: is_ok,
                        op: CmpOp::Eq,
                        lhs: MirValue::Use(tag),
                        rhs: MirValue::Literal(Literal::Int(0)),
                    });
                    self.set_terminator(Terminator::Branch {
                        cond: MirValue::Use(is_ok),
                        then_block: ok_block,
                        else_block: err_block,
                    });

                    // Ok block: extract payload
                    let result_local = self.new_temp(expr.ty.clone());
                    self.current_block = ok_block;
                    let payload_ptr = self.new_temp(Ty::Int);
                    self.emit(MirInst::GetPayload {
                        dest: payload_ptr,
                        src: scrut,
                        ty: object.ty.clone(),
                    });
                    self.emit(MirInst::GetField {
                        dest: result_local,
                        base: payload_ptr,
                        field_index: 0,
                    });
                    self.set_terminator(Terminator::Goto(merge_block));

                    // Err block: early return with Err wrapping the error payload.
                    // Allocate a Result tagged union and return it.
                    self.current_block = err_block;
                    let err_result = self.new_temp(Ty::Int);
                    self.emit(MirInst::Alloc {
                        dest: err_result,
                        ty: Ty::Result(Box::new(Ty::Unit), Box::new(Ty::Int)),
                        size: 16,
                    });
                    // Tag 1 = Err
                    self.emit(MirInst::SetTag {
                        dest: err_result,
                        tag: 1,
                    });
                    // Copy error payload from source
                    let err_payload_ptr = self.new_temp(Ty::Int);
                    self.emit(MirInst::GetPayload {
                        dest: err_payload_ptr,
                        src: scrut,
                        ty: object.ty.clone(),
                    });
                    let err_payload = self.new_temp(Ty::Int);
                    self.emit(MirInst::GetField {
                        dest: err_payload,
                        base: err_payload_ptr,
                        field_index: 0,
                    });

                    // If the current function's declared Err type differs
                    // from the source's Err type and an `impl Into[Outer]
                    // for Inner` was registered, insert a call to
                    // `Inner_into(err_payload)` to coerce the error.
                    let final_payload = if let (Ty::Result(_, src_err), Ty::Result(_, dst_err)) =
                        (&object.ty, &self.fn_mut().return_ty.clone())
                    {
                        let src_name = type_name_from_ty(src_err);
                        let dst_name = type_name_from_ty(dst_err);
                        if !src_name.is_empty()
                            && !dst_name.is_empty()
                            && src_name != dst_name
                            && self
                                .into_impls
                                .contains(&(src_name.clone(), dst_name.clone()))
                        {
                            let converted = self.new_temp((**dst_err).clone());
                            self.emit(MirInst::Call {
                                dest: Some(converted),
                                callee: format!("{}_into", src_name),
                                args: vec![MirValue::Use(err_payload)],
                            });
                            MirValue::Use(converted)
                        } else {
                            MirValue::Use(err_payload)
                        }
                    } else {
                        MirValue::Use(err_payload)
                    };

                    self.emit(MirInst::SetField {
                        base: err_result,
                        field_index: 1,
                        value: final_payload,
                    });
                    self.set_terminator(Terminator::Return(Some(MirValue::Use(err_result))));

                    self.current_block = merge_block;
                    return Ok(Some(result_local));
                }

                // ── Inline ok_or (Option -> Result conversion) ───────────
                // option.ok_or(err_val) converts:
                //   Some(x) -> Result::Ok(x) (tag 0)
                //   None    -> Result::Err(err_val) (tag 1)
                if method_name == "ok_or" {
                    let obj_local = self.lower_expr(object)?;
                    let scrut = obj_local.unwrap_or_else(|| self.new_temp(Ty::Int));

                    // Evaluate the error value argument
                    let err_arg = args.first();
                    let err_val = if let Some(err_expr) = err_arg {
                        let local = self.lower_expr(err_expr)?;
                        local_to_value(local)
                    } else {
                        MirValue::Literal(Literal::Int(0))
                    };

                    // Allocate a Result tagged union
                    let result = self.new_temp(expr.ty.clone());
                    self.emit(MirInst::Alloc {
                        dest: result,
                        ty: expr.ty.clone(),
                        size: 16,
                    });

                    // Read the Option tag: 0 = None (in Option), 1 = Some
                    let tag = self.new_temp(Ty::Int32);
                    self.emit(MirInst::GetTag {
                        dest: tag,
                        src: scrut,
                    });

                    let some_block = self.new_block();
                    let none_block = self.new_block();
                    let merge_block = self.new_block();

                    // tag == 1 means Some
                    let is_some = self.new_temp(Ty::Bool);
                    self.emit(MirInst::Compare {
                        dest: is_some,
                        op: CmpOp::Eq,
                        lhs: MirValue::Use(tag),
                        rhs: MirValue::Literal(Literal::Int(1)),
                    });
                    self.set_terminator(Terminator::Branch {
                        cond: MirValue::Use(is_some),
                        then_block: some_block,
                        else_block: none_block,
                    });

                    // Some block: Result::Ok(payload) — tag 0
                    self.current_block = some_block;
                    self.emit(MirInst::SetTag {
                        dest: result,
                        tag: 0,
                    }); // Ok
                    let payload_ptr = self.new_temp(Ty::Int);
                    self.emit(MirInst::GetPayload {
                        dest: payload_ptr,
                        src: scrut,
                        ty: object.ty.clone(),
                    });
                    let some_val = self.new_temp(Ty::Int);
                    self.emit(MirInst::GetField {
                        dest: some_val,
                        base: payload_ptr,
                        field_index: 0,
                    });
                    self.emit(MirInst::SetField {
                        base: result,
                        field_index: 1,
                        value: MirValue::Use(some_val),
                    });
                    self.set_terminator(Terminator::Goto(merge_block));

                    // None block: Result::Err(err_val) — tag 1
                    self.current_block = none_block;
                    self.emit(MirInst::SetTag {
                        dest: result,
                        tag: 1,
                    }); // Err
                    self.emit(MirInst::SetField {
                        base: result,
                        field_index: 1,
                        value: err_val,
                    });
                    self.set_terminator(Terminator::Goto(merge_block));

                    self.current_block = merge_block;
                    return Ok(Some(result));
                }

                // Check if this is a static/class method call (no `self`
                // argument needed). Covers built-in static methods as well
                // as user-defined `def self.method` forms on classes.
                //
                // Bug-fix (v1-missing-features 2026-05-20):
                // A `Type.method(arg)` call where `Type` is a class
                // identifier (HirExprKind::VarRef → DefKind::Class) MUST
                // be treated as static-style dispatch even if `method` is
                // declared as an instance method (`def method as "..."(self)`
                // in a class lib block). The instance-method FFI registration
                // already prepends the receiver type to `param_types` at
                // registration time (see `register_class_lib_method_in`),
                // so the C symbol's cranelift signature expects exactly
                // (self, user_args...). When the user writes the call in
                // static style — `JoinHandle.join_raw(handle)` — their
                // first explicit arg IS the self handle, and we must not
                // additionally prepend a phantom `Unit` (zero) at the
                // call site. Without this guard the verifier rejects
                // the resulting `ruxen_thread_join(0, handle)` as
                // "got 2, expected 1".
                let static_dispatch_ty = &receiver_ty;
                let receiver_is_class_identifier = self.is_class_identifier(object);
                let is_static = self.is_user_static_method(&type_name, method_name)
                    || (method_name == "default"
                        && self.type_supports_trait(static_dispatch_ty, "Default"))
                    || receiver_is_class_identifier;

                // Regular method call: object becomes the first argument (self).
                // Still lowered (for its side effects) even when its value is
                // dropped as a zero-sized self below.
                //
                // A `def var` String lib method on a `&var String` receiver
                // takes the receiver's slot address (`mir::refmut_abi`), not
                // the String loaded through it.
                let slot_receiver = self.mutates_slot_receiver(*resolved, &object.ty);
                // A receiver with storage of its own (a variable, a field,
                // `(&var u.name)`, P-8) passes its address
                // (`emit_rebinding_self_call`).
                let receiver_is_address = slot_receiver && self.slot_receiver_has_storage(object);
                let obj_local = if slot_receiver {
                    self.lower_slot_receiver(object)?
                } else {
                    self.lower_expr(object)?
                };
                // A borrowed scalar receiver (`x.to_s` on `x: &Float`) passes
                // its pointee, so a float's bit-pattern word is not converted
                // numerically at the call.
                let obj_local = match obj_local {
                    Some(lid) if !is_static && !receiver_is_address => {
                        let local_ty = self.fn_mut().locals[lid as usize].ty.clone();
                        self.deref_borrowed_scalar(obj_local, &local_ty).0
                    }
                    other => other,
                };

                // A zero-sized (`Unit`/`Never`) receiver has no ABI self: codegen
                // elides a `Unit` param from the callee signature
                // (`ty_to_cranelift_type` → None), so prepending the self value
                // (a `0` literal) here would make the call pass one more arg than
                // the callee accepts ("got 2, expected 1"). This arises only for
                // the universal `Kernel` methods on a unit-typed receiver
                // (`puts(x).tap { … }`) — ordinary code never calls an instance
                // method on `Unit`. Drop the self arg to match the param side.
                let self_is_zero_sized = matches!(receiver_ty.peel_refs(), Ty::Unit | Ty::Never);
                let mut arg_values = if is_static || self_is_zero_sized {
                    // Static method (or zero-sized self): don't prepend self.
                    Vec::with_capacity(args.len())
                } else {
                    vec![local_to_value(obj_local)]
                };
                // The arguments are lowered below, once the callee is known:
                // a slot-addressed `&var T` parameter takes an address.

                // Resolve through parent classes for inherited methods.
                // For a generic type parameter or impl/dyn Trait, dispatch
                // to the unique implementor of the trait bound when one
                // exists.
                // A call typeck resolved to a method of the class declared
                // `layout closure` (`Closure#clone` on a `Proc[...]`) is a
                // call of that method, not an invocation of the closure.
                let shell = self.symbols.closure_shell();
                let closure_shell_method = self.symbols.get(*resolved).is_some_and(|def| {
                    matches!(def.kind, crate::resolve::symbols::DefKind::Method { parent, .. }
                        if shell.is_some_and(|shell| shell.id == parent))
                });
                let resolved_class = match (&receiver_ty, receiver_ty.peel_refs()) {
                    _ if closure_shell_method => {
                        receiver_ty.method_home(self.symbols.closure_shell_name())
                    }
                    (Ty::Class { name, .. }, _) => {
                        self.resolve_method_class_with_args(name, method_name, args)
                    }
                    (
                        _,
                        Ty::TypeParam { bounds, .. } | Ty::SomeMixin(bounds) | Ty::AnyMixin(bounds),
                    ) => self
                        .unique_bound_impl(bounds)
                        .unwrap_or_else(|| type_name.clone()),
                    _ => type_name.clone(),
                };
                let resolved_class = if *method != UNRESOLVED_DEF {
                    self.symbols
                        .get(*method)
                        .and_then(|def| match &def.kind {
                            crate::resolve::symbols::DefKind::Method { parent, signature } => {
                                // Only trust the resolver's choice when its
                                // signature actually accepts the call args.
                                // For inherited overloaded methods the
                                // resolver may have latched onto a child's
                                // unrelated overload (e.g. ChildPicker's
                                // `pick(Bool)`) for a call that really wants
                                // the parent's `pick(Int)`.
                                if !self.method_signature_accepts_args(signature, args) {
                                    return None;
                                }
                                self.symbols
                                    .get(*parent)
                                    .map(|parent_def| parent_def.name.clone())
                            }
                            _ => None,
                        })
                        .unwrap_or(resolved_class)
                } else {
                    resolved_class
                };
                // #06.93 Phase 3: module-qualified class names carry a
                // dotted form (`Outer.Inner`). C symbol names can't
                // contain `.`, so normalise to `_` when building the
                // mangled callee — `Outer.Inner_make` becomes
                // `Outer_Inner_make`. The FFI alias map is keyed in
                // the same shape by `register_class_lib_method`.
                let resolved_class_cs = resolved_class.replace('.', "_");
                // Q1 (do-block variant): include a trailing `do…end` block as
                // the last argument for overload-symbol selection so the block
                // (a closure) is matched against the callable parameter — i.e.
                // `c.text do…end` selects the `any Fn` overload, not `&str`.
                let args_for_select: Vec<HirExpr> = match block {
                    Some(b) => {
                        let mut v = args.to_vec();
                        v.push((**b).clone());
                        v
                    }
                    None => args.to_vec(),
                };
                let selected_method_name =
                    self.select_method_symbol_name(&resolved_class, method_name, &args_for_select);
                let lowered_method_name = if let Some(selected) = selected_method_name.as_deref() {
                    selected
                } else if *method != UNRESOLVED_DEF {
                    self.symbols
                        .get(*method)
                        .and_then(|def| match &def.kind {
                            crate::resolve::symbols::DefKind::Method { .. } => {
                                Some(def.name.as_str())
                            }
                            _ => None,
                        })
                        .unwrap_or(method_name.as_str())
                } else {
                    // Ruby `alias new old`: an alias of a method the class
                    // only inherits from a mixin default has no symbol of its
                    // own to select; mangle the canonical name, whose
                    // per-implementor default body is emitted.
                    self.symbols
                        .canonical_method_name(&resolved_class, method_name)
                };
                let mangled = format!("{}_{}", resolved_class_cs, lowered_method_name);

                // Generic-class monomorphization: when the receiver is a
                // concrete instantiation we specialized (e.g. the receiver
                // of `box.eq(...)` is `Box[String]`), redirect the callee to
                // the specialized `Box__mono__String_eq` so the body
                // dispatches `==` / `#{}` through the concrete type.
                // `mono_base_for_ty` returns Some only when a copy was
                // actually emitted (FFI shells / un-specialized
                // instantiations keep the opaque callee).
                let mangled = match self.mono_base_for_ty(&receiver_ty) {
                    Some(base) => format!("{}_{}", base, lowered_method_name),
                    None => mangled,
                };

                // Builtin-head per-`T` monomorphization: the receiver of a
                // collection call is a structural head (`Ty::Array(Float)`),
                // never `Ty::Class`, so `mono_base_for_ty` above never fires
                // for it. Redirect to the specialized body when this exact
                // `(instantiation, method)` was emitted; otherwise keep the
                // opaque callee, which `resolve_ffi_alias_callee` routes to the
                // FFI symbol or the shared `{Class}_{method}` body as before.
                let mangled = match self.builtin_head_mono_callee(&receiver_ty, lowered_method_name)
                {
                    Some(specialized) => specialized,
                    // No copy for an abstract receiver: only an opaque generic
                    // body calls it, and `prune_unmonomorphised_bodies` drops
                    // that body (or reports it when something live reaches it).
                    None if *method != UNRESOLVED_DEF && self.is_specialized_only(*method) => {
                        monomorphize::abstract_callee(&receiver_ty.to_string(), lowered_method_name)
                    }
                    None => mangled,
                };

                // Q17b: specialize a generic METHOD over a mixin bound using
                // the concrete types of its user arguments. The receiver is
                // already the fixed first MIR argument (`self`); it does not
                // participate in recovery of the method's own `[T: Mixin]`.
                let mut method_arg_tys: Vec<Ty> = args.iter().map(|arg| arg.ty.clone()).collect();
                if let Some(block_expr) = block {
                    method_arg_tys.push(block_expr.ty.clone());
                }
                let mangled = if *method != UNRESOLVED_DEF {
                    if let Some(specialized) = self.method_mono_callee(*method, &method_arg_tys) {
                        specialized
                    } else if self.is_generic_mixin_method(*method) {
                        return Err(format!(
                            "cannot monomorphize generic method `{method_name}`: its \
                             mixin-bound type argument did not resolve to a concrete type at \
                             this call site (generic-calling-generic across methods requires \
                             a concrete leaf — see docs/decisions/q17-cross-package-\
                             monomorphization.md)."
                        ));
                    } else {
                        mangled
                    }
                } else {
                    mangled
                };

                // A method call whose RECEIVER is still an abstract,
                // mixin-bound type parameter (`item.width` where
                // `item: &var T, T: Sized`) with no unique implementor has no
                // concrete callee. That only happens in an un-monomorphised
                // generic body (the opaque fallback beside the per-
                // instantiation copies). Detect it on the peeled receiver type,
                // NOT by string-matching the mangled callee: a concrete
                // receiver with a bounded generic ARG (`Array[T: Showable]`) is
                // a sound builtin call. Mark the callee abstract;
                // `prune_unmonomorphised_bodies` drops the body when nothing
                // reachable calls it and rejects the program otherwise.
                // A `some Mixin` / `any Mixin` receiver is not a generic
                // parameter of an enclosing body, so no monomorphised copy
                // replaces it: reject it here.
                let mangled = if !self.receiver_is_unresolved_bound(&receiver_ty) {
                    mangled
                } else if let Ty::TypeParam { name, .. } = receiver_ty.peel_refs() {
                    monomorphize::abstract_callee(name, lowered_method_name)
                } else {
                    return Err(format!(
                        "cannot lower method `{method_name}` on unresolved mixin-bound \
                         receiver `{resolved_class}`: no concrete monomorphization reached \
                         this call (generic-calling-generic requires a concrete leaf — see \
                         docs/decisions/q17-cross-package-monomorphization.md)."
                    ));
                };

                let result_ty = self
                    .declared_method_return_ty(expr)
                    .filter(|ty| !ty.is_infer() && !ty.is_error())
                    .unwrap_or_else(|| expr.ty.clone());
                let dest = if expr.ty != Ty::Unit && expr.ty != Ty::Never {
                    Some(self.new_temp(result_ty))
                } else {
                    None
                };

                // A slot-addressed `&var T` parameter (`n: &var Int`) takes the
                // ADDRESS of the argument's storage (`mir::refmut_abi`), decided
                // on the declared signature of the callee this call names. A
                // zero-sized receiver passes no self, so its slots do not line
                // up with a `self`-taking signature; only `Kernel` methods are
                // called on one, and they take no `&var`.
                let refmut_slots = if self_is_zero_sized && !is_static {
                    Vec::new()
                } else {
                    let preferred = [*method, *resolved]
                        .into_iter()
                        .find(|def| *def != UNRESOLVED_DEF);
                    crate::mir::lower::runtime_abi::user_callee_refmut_slot_params(
                        self.symbols,
                        &mangled,
                        preferred,
                        arg_values.len() + args.len(),
                    )?
                };
                let first_arg_slot = arg_values.len();
                for (index, arg) in args.iter().enumerate() {
                    if refmut_slots
                        .get(first_arg_slot + index)
                        .copied()
                        .unwrap_or(false)
                    {
                        let addr = self.lower_refmut_addr(arg)?;
                        arg_values.push(local_to_value(addr));
                        continue;
                    }
                    let local = self.lower_expr(arg)?;
                    arg_values.push(local_to_value(local));
                }
                // Include trailing block argument if present (closures passed
                // as the last parameter of the method).
                if let Some(block_expr) = block {
                    let block_local = self.lower_expr(block_expr)?;
                    arg_values.push(local_to_value(block_local));
                }

                // A `def var` lib method (`String#push_str`, `#remove`, ...)
                // takes its receiver as `&var Self`.
                if slot_receiver {
                    let callee = self.resolve_ffi_alias_callee(mangled);
                    self.emit_rebinding_self_call(object, callee, arg_values, dest)?;
                    return Ok(dest);
                }

                // For calls on Fn/FnMut/FnOnce types (closure invocation),
                // emit an indirect call through the function pointer instead
                // of a regular named call.
                let is_fn_type = matches!(
                    &object.ty,
                    Ty::Fn { .. } | Ty::FnMut { .. } | Ty::FnOnce { .. }
                );
                let is_ref_fn_type = matches!(&object.ty,
                    Ty::Ref(inner) | Ty::RefMut(inner)
                    if matches!(inner.as_ref(), Ty::Fn { .. } | Ty::FnMut { .. } | Ty::FnOnce { .. })
                );
                // Phase 2 #06.9: dyn-erased `any Fn(...)` receivers
                // dispatch through the same indirect-call path. The
                // physical representation is identical — a closure
                // value is a pointer to the `(fn_ptr, captures_ptr, …)`
                // heap pair (see `drops::CLOSURE_PAIR_SIZE`), and `Ty::AnyMixin` lays out
                // as a 16-byte primitive (see `codegen/layout.rs:445`),
                // so slot 0 / slot 1 line up without a vtable. The
                // typeck-side unification in `typeck/unify.rs` is what
                // gets the closure literal into the dyn slot in the
                // first place.
                let is_any_fn_type = matches!(
                    &object.ty,
                    Ty::AnyMixin(bounds) if bounds.iter().any(crate::hir::types::MixinRef::is_fn_family)
                );
                let is_ref_any_fn_type = matches!(&object.ty,
                    Ty::Ref(inner) | Ty::RefMut(inner)
                    if matches!(inner.as_ref(), Ty::AnyMixin(bounds) if bounds.iter().any(crate::hir::types::MixinRef::is_fn_family))
                );
                // For-loop bindings (and any other receiver whose HIR
                // expression type was left as `Ty::Infer` by typeck —
                // see `typeck/infer.rs::HirExprKind::For` which never
                // unifies the binding with the iterable's element
                // type) carry their real shape in the MIR local's
                // declared `ty`. Peek there as a fallback so a
                // dyn-erased closure dispatched through `for h in
                // hs.iter` reaches the indirect-call path instead of
                // falling through to a named call against a
                // non-existent `?T*_call` symbol.
                let local_ty_is_fn = matches!(&object.ty, Ty::Infer(_))
                    && obj_local
                        .map(|id| {
                            let func = self.fn_mut();
                            func.locals
                                .get(id as usize)
                                .map(|l| l.ty.is_callable())
                                .unwrap_or(false)
                        })
                        .unwrap_or(false);
                let is_fn_call = !closure_shell_method
                    && (is_fn_type
                        || is_ref_fn_type
                        || is_any_fn_type
                        || is_ref_any_fn_type
                        || local_ty_is_fn
                        || type_name.starts_with("Fn(")
                        || type_name.starts_with("Fn[")
                        || type_name.starts_with("&Fn(")
                        || type_name.starts_with("&Fn[")
                        || type_name.starts_with("any Fn(")
                        || type_name.starts_with("any Fn[")
                        || type_name.starts_with("&any Fn(")
                        || type_name.starts_with("&any Fn["));

                if is_fn_call {
                    // The closure value is a heap pair {fn_ptr, captures_ptr}.
                    // Load both, then call indirectly with captures_ptr
                    // prepended to the user-visible arg list.
                    let pair = obj_local.unwrap_or_else(|| self.new_temp(Ty::Int));

                    // Ruby-block-semantics ADR D5: an OPTIONAL `&block` slot is
                    // a null closure-pair-pointer (ADR D1) when the caller
                    // passed no block. Calling it (`yield` / `block.(…)`) would
                    // dereference null and segfault. Guard the dispatch: when
                    // the receiver is the block slot (`__block`), branch on
                    // `pair == 0` and `ruxen_panic` with a LocalJumpError-style
                    // message naming the enclosing function, instead of
                    // crashing. Non-block closure receivers (map/each/stored
                    // closures) skip the guard — they are never optional, so
                    // this adds zero overhead to hot closure paths.
                    let is_block_slot = matches!(
                        &object.kind,
                        HirExprKind::VarRef(def_id)
                            if self.symbols.get(*def_id).map(|d| d.name == "__block").unwrap_or(false)
                    );
                    if is_block_slot {
                        self.emit_block_presence_guard(pair);
                    }

                    let fn_ptr = self.new_temp(Ty::Int);
                    self.emit(MirInst::GetField {
                        dest: fn_ptr,
                        base: pair,
                        field_index: CLOSURE_PAIR_FN,
                    });
                    let cap_ptr = self.new_temp(Ty::Int);
                    self.emit(MirInst::GetField {
                        dest: cap_ptr,
                        base: pair,
                        field_index: CLOSURE_PAIR_CAPTURES,
                    });
                    // Drop the self-as-first-arg that method-call lowering
                    // prepended; replace it with captures_ptr.
                    let user_args: Vec<MirValue> = if !is_static && !arg_values.is_empty() {
                        arg_values.into_iter().skip(1).collect()
                    } else {
                        arg_values
                    };
                    // Closure word-ABI (B01), CALLER side. `lower_closure`
                    // compiles a closure's float parameters and float return
                    // as 64-bit uniform WORDS carrying the IEEE-754 bit
                    // pattern, because a generic body invoking it through the
                    // erased `Fn` box can only pass words. Mirror that here:
                    // bit-reinterpret each float argument on the way in, and
                    // the result on the way out.
                    //
                    // In a GENERIC body (`Array_map`, whose arg/result locals
                    // are `TypeParam`) neither conversion fires — the values
                    // are already words, which is exactly right. It fires at
                    // a CONCRETE call site (`f.call(2.5)` on an
                    // `Fn(Float) -> Float`), keeping both spellings on the
                    // one ABI.
                    let mut indirect_args = Vec::with_capacity(user_args.len() + 1);
                    indirect_args.push(MirValue::Use(cap_ptr));
                    for arg in user_args {
                        indirect_args.push(self.float_arg_to_word(arg));
                    }
                    let (modes, result) =
                        self.closure_call_facts(&object.ty, obj_local, &indirect_args);

                    // A float result comes back in a word, so the
                    // `CallIndirect` destination must be word-typed and the
                    // real destination filled by the inverse reinterpretation.
                    let dest_is_float = dest
                        .and_then(|d| self.fn_mut().locals.get(d as usize))
                        .map(|l| crate::mir::abi::is_float_ty(&l.ty))
                        .unwrap_or(false);
                    let call_dest = if dest_is_float {
                        Some(self.new_temp(Ty::Int))
                    } else {
                        dest
                    };
                    self.emit(MirInst::CallIndirect {
                        dest: call_dest,
                        callee: fn_ptr,
                        args: indirect_args,
                        modes,
                        pair: obj_local,
                    });
                    if let (Some(call_dest), Some(result)) = (call_dest, result) {
                        self.fn_mut().record_view(call_dest, result);
                    }
                    if let (true, Some(word), Some(real)) = (dest_is_float, call_dest, dest) {
                        self.emit(MirInst::BitsToFloat {
                            dest: real,
                            src: word,
                        });
                    }
                } else {
                    // #06.8 Phase 3b: rewrite the mangled `ClassName_method`
                    // callee to the linked C symbol when this method came
                    // from a class-body `lib` block (the alias map was
                    // populated by `register_class_lib_method` keyed on
                    // the same mangled shape). Non-FFI class methods hit
                    // the unwrap_or branch and use the mangled name
                    // unchanged.
                    //
                    // #06.8 T#17: for generic builtin receivers (`Ty::Option(Inner)`,
                    // `Ty::Result(Ok,Err)`, ...) the call-site mangle carries the
                    // surface generic args — `Option[Int]_unwrap_or`. The
                    // alias-map entry from a `class Option do lib ... end end`
                    // bootstrap shell is keyed without args: `Option_unwrap_or`.
                    // After the exact-key lookup misses, peel the `[...]` segment
                    // from `mangled` and retry — that's the generic-stripped
                    // shape the alias map carries.
                    // Universal-`Kernel` call-site recording: if this resolves
                    // to one of `mixin Kernel`'s defaults (`tap`/`then`),
                    // remember the concrete receiver so
                    // `emit_universal_kernel_bodies` can synthesize
                    // `{Type}_{method}` for a primitive / builtin / non-
                    // `include`-ing receiver that has no `impl` item to drive
                    // the usual per-implementor default monomorphization. A
                    // receiver with its OWN body (own `def tap`, or an explicit
                    // `include Kernel`) already emits `{mangled}`; the post-pass
                    // skips any name already present, so an override wins.
                    if self
                        .trait_default_methods
                        .get("Kernel")
                        .is_some_and(|m| m.contains_key(lowered_method_name))
                    {
                        let self_ty = receiver_ty.peel_refs().clone();
                        self.universal_kernel_needed
                            .insert(mangled.clone(), (self_ty, lowered_method_name.to_string()));
                    }
                    let callee = self.resolve_ffi_alias_callee(mangled);
                    self.emit(MirInst::Call {
                        dest,
                        callee,
                        args: arg_values,
                    });
                }
                Ok(dest)
            }

            // ── Assignment ──────────────────────────────────────────
            _ => unreachable!("lower_method_call: dispatched to wrong helper"),
        }
    }

    /// What lowering knows of a closure call from the closure's declared
    /// signature (`object_ty`, else the type of the `pair` local holding
    /// it): how it takes each of `args` (`args[0]` is the pair's own
    /// captures box, which the call only reads), as
    /// `param_modes::unknown_param_mode` where the signature has no
    /// parameter; and the [`View`] of its result, `Own` when the declared
    /// result can hold no reference, `None` (it may point into every
    /// argument) when it can or the signature is unknown.
    fn closure_call_facts(
        &mut self,
        object_ty: &Ty,
        pair: Option<LocalId>,
        args: &[MirValue],
    ) -> (Vec<ParamMode>, Option<View>) {
        let pair_ty = pair.map(|pair| self.fn_mut().locals[pair as usize].ty.clone());
        let signature = [Some(object_ty), pair_ty.as_ref()]
            .into_iter()
            .flatten()
            .find_map(|ty| ty.peel_refs().callable_signature())
            .map(|(params, result)| (params.to_vec(), result.clone()));
        let result = signature.as_ref().and_then(|(_, result)| {
            let generic = result.any(&mut |t: &Ty| matches!(t, Ty::TypeParam { .. }));
            (!generic && !result.holds_reference()).then_some(View::Own)
        });
        let params = signature.map(|(params, _)| params).unwrap_or_default();
        let symbols = self.symbols;
        let func = self.fn_mut();
        let mut modes = vec![ParamMode::Read];
        for (position, arg) in args.iter().enumerate().skip(1) {
            let mode = match (params.get(position - 1), arg.local()) {
                (Some(param), _) => crate::mir::param_modes::param_mode(param, symbols),
                (None, Some(local)) => crate::mir::param_modes::unknown_param_mode(
                    &func.locals[local as usize].ty,
                    symbols,
                ),
                (None, None) => ParamMode::Read,
            };
            modes.push(mode);
        }
        (modes, result)
    }

    /// Whether `resolved` is a `def var` lib method (`String#push_str`) on a
    /// receiver whose `&var` is the address of its slot (a String or a
    /// scalar, see `refmut_abi`): the C symbol then writes the receiver's
    /// slot in place, so the call must pass `&var receiver`, not its value.
    fn mutates_slot_receiver(&self, resolved: DefId, receiver_ty: &Ty) -> bool {
        use crate::resolve::symbols::DefKind;
        resolved != UNRESOLVED_DEF
            && crate::mir::refmut_abi::refmut_is_slot_address(receiver_ty)
            && self.symbols.get(resolved).is_some_and(|def| {
                matches!(&def.kind, DefKind::Method { signature, .. }
                    if signature.c_symbol.is_some()
                        && signature.self_mode == Some(HirSelfMode::RefMut))
            })
    }

    /// Whether the receiver of a `def var` lib method on a String or scalar
    /// is passed as the address of its own storage: a `&var T` receiver
    /// (already the caller's address), and an owned variable (a local, a
    /// cell, a capture) or owned field (`lower_refmut_addr`). A borrowed
    /// String or a temporary has no storage the callee may replace.
    pub(super) fn slot_receiver_has_storage(&self, object: &HirExpr) -> bool {
        if matches!(object.ty, Ty::RefMut(_) | Ty::RefMutLifetime(..)) {
            return true;
        }
        let owned = if object.ty.peel_refs().is_string_form() {
            matches!(object.ty, Ty::String)
        } else {
            !matches!(object.ty, Ty::Ref(_) | Ty::RefLifetime(..))
        };
        owned
            && match &object.kind {
                HirExprKind::VarRef(def_id) => {
                    self.def_to_local.contains_key(def_id) || self.capture_map.contains_key(def_id)
                }
                HirExprKind::FieldAccess { .. } | HirExprKind::Index { .. } => {
                    !object.is_unaddressable_var_place()
                }
                _ => false,
            }
    }

    /// Lower the receiver of a `def var` lib method on a String or scalar:
    /// its storage's address when it has one
    /// ([`Self::slot_receiver_has_storage`]), else its value.
    pub(super) fn lower_slot_receiver(
        &mut self,
        object: &HirExpr,
    ) -> Result<Option<LocalId>, String> {
        if self.slot_receiver_has_storage(object) {
            self.lower_refmut_addr(object)
        } else {
            self.lower_expr(object)
        }
    }

    /// Emit a call to a `def var` lib method on a String or scalar receiver.
    /// `args[0]` holds the receiver lowered by [`Self::lower_slot_receiver`].
    /// The callee writes the new value through a `&var T` slot (a String
    /// callee frees the old buffer), so the slot must be the receiver's own
    /// storage:
    ///
    /// - a receiver with storage already is its address;
    /// - a String buffer this code does not own (a `&String` borrow, a call
    ///   result) is copied first, so the callee never frees someone else's
    ///   buffer. A borrowed variable is rebound to the mutated copy. A
    ///   scalar is a plain copy.
    pub(super) fn emit_rebinding_self_call(
        &mut self,
        object: &HirExpr,
        callee: String,
        mut args: Vec<MirValue>,
        dest: Option<LocalId>,
    ) -> Result<(), String> {
        if self.slot_receiver_has_storage(object) {
            self.emit(MirInst::Call { dest, callee, args });
            return Ok(());
        }
        let pointee = object.ty.peel_refs().clone();
        let slot_ty = Ty::RefMut(Box::new(pointee.clone()));
        let owned = if pointee.is_string_form() {
            matches!(object.ty, Ty::String)
        } else {
            !matches!(object.ty, Ty::Ref(_) | Ty::RefLifetime(..))
        };
        let is_place = matches!(
            object.kind,
            HirExprKind::VarRef(_) | HirExprKind::FieldAccess { .. }
        );
        let buf = self.new_temp(pointee.clone());
        if (owned && is_place) || !pointee.is_string_form() {
            self.emit(MirInst::Assign {
                dest: buf,
                value: args[0].clone(),
            });
        } else {
            self.emit(MirInst::Call {
                dest: Some(buf),
                callee: "ruxen_string_from".to_string(),
                args: vec![args[0].clone()],
            });
        }
        let slot = self.new_temp(slot_ty);
        self.emit(MirInst::RefMut {
            dest: slot,
            src: buf,
        });
        args[0] = MirValue::Use(slot);
        self.emit(MirInst::Call { dest, callee, args });
        if is_place {
            self.store_value_into_target(object, MirValue::Use(buf))?;
        }
        Ok(())
    }
}
