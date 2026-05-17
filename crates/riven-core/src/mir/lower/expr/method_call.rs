use super::super::*;

impl<'a> Lowerer<'a> {
    pub(super) fn lower_method_call(&mut self, expr: &HirExpr) -> Result<Option<LocalId>, String> {
        match &expr.kind {
            // ── Method call ─────────────────────────────────────────
            HirExprKind::MethodCall {
                object,
                method_name,
                generic_args: _generic_args,
                args,
                block,
                ..
            } => {
                let type_name = self
                    .receiver_type_name(object)
                    .unwrap_or_else(|| type_name_from_ty(&object.ty));

                // Handle .new() / .with_capacity() constructor calls:
                // dispatch directly to the runtime symbol (no self arg).
                let is_collection_ctor = method_name == "new"
                    || (method_name == "with_capacity" && {
                        let bt = if let Some(pos) = type_name.find('[') {
                            &type_name[..pos]
                        } else {
                            type_name.as_str()
                        };
                        matches!(
                            bt,
                            "Vec" | "Array" | "Hash" | "HashMap" | "Map" | "Set" | "HashSet"
                        )
                    });
                if is_collection_ctor {
                    // For built-in types (Vec, Hash, Set), call the runtime
                    // constructor directly instead of Alloc + init.
                    let base_type = if let Some(pos) = type_name.find('[') {
                        &type_name[..pos]
                    } else {
                        type_name.as_str()
                    };
                    // Phase 2 #06.D2.S0: `Formatter.new()` dispatches to
                    // the runtime constructor just like Vec/Hash.
                    // Phase 2 #06 (Command): `Command.new(prog)` joins
                    // the same fast path so it dispatches to
                    // `riven_command_new(prog)` instead of going through
                    // the `Class_init` path (Command has no user-defined
                    // init).
                    if matches!(
                        base_type,
                        "Vec"
                            | "Array"
                            | "Hash"
                            | "HashMap"
                            | "Map"
                            | "Set"
                            | "HashSet"
                            | "Formatter"
                            | "Command"
                    ) {
                        let obj = self.new_temp(expr.ty.clone());
                        // ruby-naming.spec.md §3.11 renames stdlib types
                        // (`Vec` → `Array`, `HashMap` → `Map`, `HashSet` →
                        // `Set`). The runtime C functions keep their
                        // legacy names (`Vec_new`, `Hash_new`, …), so map
                        // the surface base-type back to the runtime
                        // before mangling.
                        let runtime_base = match base_type {
                            "Array" => "Vec",
                            "Map" => "Hash",
                            "HashMap" => "Hash",
                            "Set" => "HashSet",
                            other => other,
                        };
                        // The same fast path also handles `with_capacity`,
                        // which takes a single integer arg and lowers to
                        // e.g. `riven_hash_with_capacity(cap)`.
                        let mut call_args = Vec::with_capacity(args.len());
                        for arg in args {
                            let local = self.lower_expr(arg)?;
                            call_args.push(local_to_value(local));
                        }
                        self.emit(MirInst::Call {
                            dest: Some(obj),
                            callee: format!("{}_{}", runtime_base, method_name),
                            args: call_args,
                        });
                        return Ok(Some(obj));
                    }
                    // String.new / String.with_capacity — dispatch to the
                    // C runtime directly. The dispatch table in
                    // codegen/runtime.rs maps `String_new` and
                    // `String_with_capacity` to their `riven_string_*`
                    // implementations.
                    if base_type == "String" {
                        let obj = self.new_temp(expr.ty.clone());
                        let mut call_args = Vec::with_capacity(args.len());
                        for arg in args {
                            let local = self.lower_expr(arg)?;
                            call_args.push(local_to_value(local));
                        }
                        self.emit(MirInst::Call {
                            dest: Some(obj),
                            callee: "String_new".to_string(),
                            args: call_args,
                        });
                        return Ok(Some(obj));
                    }

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
                        for (idx, arg) in args.iter().enumerate() {
                            let local = self.lower_expr(arg)?;
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

                    // Call ClassName_init(self, args...)
                    let mut arg_values = vec![MirValue::Use(obj)];
                    for arg in args {
                        let local = self.lower_expr(arg)?;
                        arg_values.push(local_to_value(local));
                    }
                    let _ = layout; // size used by Alloc internally via layout_of in codegen
                    self.emit(MirInst::Call {
                        dest: None,
                        callee: format!("{}_init", type_name),
                        args: arg_values,
                    });
                    return Ok(Some(obj));
                }

                // ── Phase 2 stdlib (#04): HashMap.entry chain ──────────
                // `m.entry(K).or_insert(V)` and `m.entry(K).or_insert_with { || V }`
                // are recognized as a single MIR unit and inlined to:
                //
                //   if !riven_hash_contains_key(map, k) {
                //       riven_hash_insert(map, k, v);   // discard prior value
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
                // When a method like .each, .filter, .find, .position,
                // .map, .partition, .where_matching takes a trailing block
                // (closure), inline the closure body as a loop instead of
                // passing a (null) function pointer.
                if let Some(block_expr) = block {
                    if let Some(result) =
                        self.try_inline_closure_method(expr, object, method_name, args, block_expr)?
                    {
                        return Ok(result);
                    }
                }

                // Phase 2 stdlib (#05 follow-up): built-in
                // `iter.collect[Target]` lowers directly to a runtime
                // constructor over the v1 eager-iterator representation
                // (`RivenVec*`). Typeck has already validated the target
                // and item compatibility, so lowering only picks the
                // concrete helper by the expression's result type.
                if method_name == "collect" {
                    let iter_local = self.lower_expr(object)?;
                    let iter_id = iter_local.unwrap_or_else(|| self.new_temp(Ty::Int));
                    let dest = self.new_temp(expr.ty.clone());
                    let callee = match &expr.ty {
                        Ty::Array(_) => "riven_vec_from_iter",
                        Ty::String | Ty::Str => "riven_string_from_iter",
                        Ty::Map(_, _) => "riven_hash_from_iter",
                        Ty::Set(_) => "riven_set_from_iter",
                        other => {
                            return Err(format!(
                                "unsupported collect target in MIR lowering: {other}"
                            ));
                        }
                    };
                    self.emit(MirInst::Call {
                        dest: Some(dest),
                        callee: callee.to_string(),
                        args: vec![MirValue::Use(iter_id)],
                    });
                    return Ok(Some(dest));
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
                    // Note: inline_position uses tag 0 = None, tag 1 = Some
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
                let static_dispatch_ty = if matches!(&object.ty, Ty::Infer(_)) {
                    &expr.ty
                } else {
                    &object.ty
                };
                let is_static = is_builtin_static_method(&type_name, method_name)
                    || self.is_user_static_method(&type_name, method_name)
                    || (method_name == "default"
                        && self.type_supports_trait(static_dispatch_ty, "Default"));

                // Regular method call: object becomes the first argument (self).
                let obj_local = self.lower_expr(object)?;

                let mut arg_values = if is_static {
                    // Static method: don't prepend self.
                    Vec::with_capacity(args.len())
                } else {
                    vec![local_to_value(obj_local)]
                };
                for arg in args {
                    let local = self.lower_expr(arg)?;
                    arg_values.push(local_to_value(local));
                }
                // Include trailing block argument if present (closures passed
                // as the last parameter of the method).
                if let Some(block_expr) = block {
                    let block_local = self.lower_expr(block_expr)?;
                    arg_values.push(local_to_value(block_local));
                }

                // Resolve through parent classes for inherited methods.
                // For a generic type parameter or impl/dyn Trait, dispatch
                // to the unique implementor of the trait bound when one
                // exists.
                let resolved_class = match &object.ty {
                    Ty::Class { name, .. } => self.resolve_method_class(name, method_name),
                    Ty::TypeParam { bounds, .. } | Ty::SomeMixin(bounds) | Ty::AnyMixin(bounds) => {
                        self.unique_bound_impl(bounds)
                            .unwrap_or_else(|| type_name.clone())
                    }
                    Ty::Ref(inner)
                    | Ty::RefMut(inner)
                    | Ty::RefLifetime(_, inner)
                    | Ty::RefMutLifetime(_, inner) => match inner.as_ref() {
                        Ty::TypeParam { bounds, .. }
                        | Ty::SomeMixin(bounds)
                        | Ty::AnyMixin(bounds) => self
                            .unique_bound_impl(bounds)
                            .unwrap_or_else(|| type_name.clone()),
                        _ => type_name.clone(),
                    },
                    _ => type_name.clone(),
                };
                let mangled = format!("{}_{}", resolved_class, method_name);

                // `&mut String` detection: when the receiver is a local
                // of type `&mut String` (i.e. the caller passed `&mut s`
                // into a parameter typed `&mut String`), the local holds
                // a pointer-to-`char*`. Mutating methods must read the
                // current buffer via `riven_deref_ptr`, call the string
                // helper, then write the new buffer back via
                // `riven_store_ptr` so the caller observes the update.
                let receiver_is_mut_string_ref = matches!(
                    &object.ty,
                    Ty::RefMut(inner) | Ty::RefMutLifetime(_, inner)
                        if matches!(inner.as_ref(), Ty::String | Ty::Str)
                );

                // Special handling for push_str on String variables:
                // riven_string_push_str returns a new char*, so we need to
                // capture the return value and reassign it to the object variable.
                if method_name == "push_str" {
                    if receiver_is_mut_string_ref {
                        // `self_arg` here is the pointer value (char**).
                        // We need the pointee to feed into push_str, and
                        // we must store the returned buffer back through
                        // the pointer.
                        let ptr_arg = arg_values[0].clone();
                        let tail_args: Vec<MirValue> = arg_values.iter().skip(1).cloned().collect();
                        let cur = self.new_temp(Ty::String);
                        self.emit(MirInst::Call {
                            dest: Some(cur),
                            callee: "riven_deref_ptr".to_string(),
                            args: vec![ptr_arg.clone()],
                        });
                        let new_buf = self.new_temp(Ty::String);
                        let mut call_args = vec![MirValue::Use(cur)];
                        call_args.extend(tail_args);
                        self.emit(MirInst::Call {
                            dest: Some(new_buf),
                            callee: "String_push_str".to_string(),
                            args: call_args,
                        });
                        self.emit(MirInst::Call {
                            dest: None,
                            callee: "riven_store_ptr".to_string(),
                            args: vec![ptr_arg, MirValue::Use(new_buf)],
                        });
                        return Ok(None);
                    }
                    if let HirExprKind::VarRef(def_id) = &object.kind {
                        if let Some(&obj_var) = self.def_to_local.get(def_id) {
                            let tmp = self.new_temp(Ty::String);
                            self.emit(MirInst::Call {
                                dest: Some(tmp),
                                callee: mangled,
                                args: arg_values,
                            });
                            self.emit(MirInst::Assign {
                                dest: obj_var,
                                value: MirValue::Use(tmp),
                            });
                            return Ok(None);
                        }
                    }
                }

                // Special handling for `String.push(char)`: the runtime
                // only exposes `riven_string_push_str`, so we first widen
                // the Char arg to a one-char heap string via
                // `riven_char_to_string`, then hand that to push_str.
                // Without this rewrite every program that calls
                // `s.push('!')` links against a missing `String_push`.
                //
                // When the receiver is `&mut String` (a parameter), we
                // lower to `*s = String_push_str(*s, one_char_str)` using
                // the deref/store runtime helpers so the caller's local
                // is updated in place.  For an owned local String binding
                // we just rebind the variable to the new buffer.
                if method_name == "push" && resolved_class == "String" && arg_values.len() == 2 {
                    // Phase 2 stdlib batch 2 (#02): route through the
                    // dedicated `riven_string_push(s, codepoint)` runtime
                    // fn rather than synthesising
                    // `riven_char_to_string` + `String_push_str` here.
                    // The dedicated fn allocates exactly one fresh
                    // buffer per call and frees its internal char-string
                    // temporary, so we don't leak the codepoint
                    // intermediate. The prior receiver buffer is freed
                    // here explicitly so the rebind doesn't leak it.
                    let char_arg = arg_values[1].clone();
                    let self_arg = arg_values[0].clone();
                    if receiver_is_mut_string_ref {
                        let cur = self.new_temp(Ty::String);
                        self.emit(MirInst::Call {
                            dest: Some(cur),
                            callee: "riven_deref_ptr".to_string(),
                            args: vec![self_arg.clone()],
                        });
                        let new_buf = self.new_temp(Ty::String);
                        self.emit(MirInst::Call {
                            dest: Some(new_buf),
                            callee: "String_push".to_string(),
                            args: vec![MirValue::Use(cur), char_arg],
                        });
                        // Free the prior buffer before overwriting the
                        // pointer slot, otherwise it leaks.
                        self.emit(MirInst::Call {
                            dest: None,
                            callee: "riven_string_free".to_string(),
                            args: vec![MirValue::Use(cur)],
                        });
                        self.emit(MirInst::Call {
                            dest: None,
                            callee: "riven_store_ptr".to_string(),
                            args: vec![self_arg, MirValue::Use(new_buf)],
                        });
                        return Ok(None);
                    }
                    let new_buf = self.new_temp(Ty::String);
                    self.emit(MirInst::Call {
                        dest: Some(new_buf),
                        callee: "String_push".to_string(),
                        args: vec![self_arg.clone(), char_arg],
                    });
                    if let HirExprKind::VarRef(def_id) = &object.kind {
                        if let Some(&obj_var) = self.def_to_local.get(def_id) {
                            // Free the prior buffer first; the local
                            // owns it (we just lowered it as the self
                            // arg above) and the assignment below is
                            // about to overwrite the slot.
                            self.emit(MirInst::Call {
                                dest: None,
                                callee: "riven_string_free".to_string(),
                                args: vec![MirValue::Use(obj_var)],
                            });
                            self.emit(MirInst::Assign {
                                dest: obj_var,
                                value: MirValue::Use(new_buf),
                            });
                        }
                    }
                    return Ok(None);
                }

                // Phase 2 stdlib: mutating String methods that allocate a
                // fresh buffer (insert, insert_str). Same dance as push_str.
                if matches!(method_name.as_str(), "insert" | "insert_str")
                    && resolved_class == "String"
                {
                    if receiver_is_mut_string_ref {
                        let ptr_arg = arg_values[0].clone();
                        let tail_args: Vec<MirValue> = arg_values.iter().skip(1).cloned().collect();
                        let cur = self.new_temp(Ty::String);
                        self.emit(MirInst::Call {
                            dest: Some(cur),
                            callee: "riven_deref_ptr".to_string(),
                            args: vec![ptr_arg.clone()],
                        });
                        let new_buf = self.new_temp(Ty::String);
                        let mut call_args = vec![MirValue::Use(cur)];
                        call_args.extend(tail_args);
                        self.emit(MirInst::Call {
                            dest: Some(new_buf),
                            callee: format!("String_{}", method_name),
                            args: call_args,
                        });
                        self.emit(MirInst::Call {
                            dest: None,
                            callee: "riven_store_ptr".to_string(),
                            args: vec![ptr_arg, MirValue::Use(new_buf)],
                        });
                        return Ok(None);
                    }
                    if let HirExprKind::VarRef(def_id) = &object.kind {
                        if let Some(&obj_var) = self.def_to_local.get(def_id) {
                            let tmp = self.new_temp(Ty::String);
                            self.emit(MirInst::Call {
                                dest: Some(tmp),
                                callee: format!("String_{}", method_name),
                                args: arg_values,
                            });
                            self.emit(MirInst::Assign {
                                dest: obj_var,
                                value: MirValue::Use(tmp),
                            });
                            return Ok(None);
                        }
                    }
                }

                // String.remove(i) — returns the removed Char and
                // simultaneously rewrites the buffer. The runtime returns
                // a 16-byte struct {removed: i64, new_buffer: ptr}; we read
                // .removed for the value and .new_buffer to update the
                // local / &mut String.
                if method_name == "remove" && resolved_class == "String" {
                    let self_arg = arg_values[0].clone();
                    // For &mut String, we must first deref to get the buf.
                    let buf_arg = if receiver_is_mut_string_ref {
                        let cur = self.new_temp(Ty::String);
                        self.emit(MirInst::Call {
                            dest: Some(cur),
                            callee: "riven_deref_ptr".to_string(),
                            args: vec![self_arg.clone()],
                        });
                        MirValue::Use(cur)
                    } else {
                        self_arg.clone()
                    };
                    let tail_args: Vec<MirValue> = arg_values.iter().skip(1).cloned().collect();
                    let result_struct = self.new_temp(Ty::Int);
                    let mut call_args = vec![buf_arg];
                    call_args.extend(tail_args);
                    self.emit(MirInst::Call {
                        dest: Some(result_struct),
                        callee: "String_remove".to_string(),
                        args: call_args,
                    });
                    // Read the removed Char (field 0 of the 16-byte struct).
                    let removed = self.new_temp(Ty::Char);
                    self.emit(MirInst::GetField {
                        dest: removed,
                        base: result_struct,
                        field_index: 0,
                    });
                    // Read the new buffer (field 1).
                    let new_buf = self.new_temp(Ty::String);
                    self.emit(MirInst::GetField {
                        dest: new_buf,
                        base: result_struct,
                        field_index: 1,
                    });
                    if receiver_is_mut_string_ref {
                        self.emit(MirInst::Call {
                            dest: None,
                            callee: "riven_store_ptr".to_string(),
                            args: vec![self_arg, MirValue::Use(new_buf)],
                        });
                    } else if let HirExprKind::VarRef(def_id) = &object.kind {
                        if let Some(&obj_var) = self.def_to_local.get(def_id) {
                            self.emit(MirInst::Assign {
                                dest: obj_var,
                                value: MirValue::Use(new_buf),
                            });
                        }
                    }
                    return Ok(Some(removed));
                }

                // String.clear / truncate — in-place mutation; for &mut
                // String we must deref to the buffer pointer first.
                if matches!(method_name.as_str(), "clear" | "truncate")
                    && resolved_class == "String"
                {
                    if receiver_is_mut_string_ref {
                        let ptr_arg = arg_values[0].clone();
                        let tail_args: Vec<MirValue> = arg_values.iter().skip(1).cloned().collect();
                        let cur = self.new_temp(Ty::String);
                        self.emit(MirInst::Call {
                            dest: Some(cur),
                            callee: "riven_deref_ptr".to_string(),
                            args: vec![ptr_arg],
                        });
                        let mut call_args = vec![MirValue::Use(cur)];
                        call_args.extend(tail_args);
                        self.emit(MirInst::Call {
                            dest: None,
                            callee: format!("String_{}", method_name),
                            args: call_args,
                        });
                        return Ok(None);
                    }
                    // Owned local: pass the buffer pointer directly.
                    self.emit(MirInst::Call {
                        dest: None,
                        callee: format!("String_{}", method_name),
                        args: arg_values,
                    });
                    return Ok(None);
                }

                let dest = if expr.ty != Ty::Unit && expr.ty != Ty::Never {
                    Some(self.new_temp(expr.ty.clone()))
                } else {
                    None
                };

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
                let is_fn_call = is_fn_type
                    || is_ref_fn_type
                    || type_name.starts_with("Fn(")
                    || type_name.starts_with("Fn[")
                    || type_name.starts_with("&Fn(")
                    || type_name.starts_with("&Fn[");

                if is_fn_call {
                    // The closure value is a heap pair {fn_ptr, captures_ptr}.
                    // Load both, then call indirectly with captures_ptr
                    // prepended to the user-visible arg list.
                    let pair = obj_local.unwrap_or_else(|| self.new_temp(Ty::Int));
                    let fn_ptr = self.new_temp(Ty::Int);
                    self.emit(MirInst::GetField {
                        dest: fn_ptr,
                        base: pair,
                        field_index: 0,
                    });
                    let cap_ptr = self.new_temp(Ty::Int);
                    self.emit(MirInst::GetField {
                        dest: cap_ptr,
                        base: pair,
                        field_index: 1,
                    });
                    // Drop the self-as-first-arg that method-call lowering
                    // prepended; replace it with captures_ptr.
                    let user_args: Vec<MirValue> = if !is_static && !arg_values.is_empty() {
                        arg_values.into_iter().skip(1).collect()
                    } else {
                        arg_values
                    };
                    let mut indirect_args = Vec::with_capacity(user_args.len() + 1);
                    indirect_args.push(MirValue::Use(cap_ptr));
                    indirect_args.extend(user_args);
                    self.emit(MirInst::CallIndirect {
                        dest,
                        callee: fn_ptr,
                        args: indirect_args,
                    });
                } else {
                    self.emit(MirInst::Call {
                        dest,
                        callee: mangled,
                        args: arg_values,
                    });
                }
                Ok(dest)
            }

            // ── Assignment ──────────────────────────────────────────
            _ => unreachable!("lower_method_call: dispatched to wrong helper"),
        }
    }
}
