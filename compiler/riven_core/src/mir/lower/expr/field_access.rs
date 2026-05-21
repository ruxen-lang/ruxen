use super::super::*;

impl<'a> Lowerer<'a> {
    pub(super) fn lower_field_access(&mut self, expr: &HirExpr) -> Result<Option<LocalId>, String> {
        match &expr.kind {
            // ── Field access ────────────────────────────────────────
            HirExprKind::FieldAccess {
                object,
                field_name,
                field_idx,
                ..
            } => {
                // Handle safe navigation `?.field` on Option types.
                // The resolver desugars `x?.field` as FieldAccess with object
                // type Option(...) and result type Option(...). We inline
                // an Option match: if Some, extract inner and call method,
                // otherwise produce None.
                if is_option_type(&object.ty)
                    && is_option_type(&expr.ty)
                    && !matches!(
                        field_name.as_str(),
                        "is_some"
                            | "is_none"
                            | "map"
                            | "unwrap_or"
                            | "unwrap_or_else"
                            | "ok_or"
                            | "unwrap!"
                            | "expect!"
                            | "and_then"
                            | "or"
                            | "filter"
                            | "flatten"
                            | "as_ref"
                            | "take"
                            | "replace"
                    )
                {
                    let opt_local = self.lower_expr(object)?;
                    let opt_id = opt_local.unwrap_or_else(|| self.new_temp(Ty::Int));

                    // Allocate result Option
                    let result = self.new_temp(expr.ty.clone());
                    self.emit(MirInst::Alloc {
                        dest: result,
                        ty: expr.ty.clone(),
                        size: 16,
                    });

                    // Check tag
                    let tag = self.new_temp(Ty::Int32);
                    self.emit(MirInst::GetTag {
                        dest: tag,
                        src: opt_id,
                    });
                    let is_some = self.new_temp(Ty::Bool);
                    self.emit(MirInst::Compare {
                        dest: is_some,
                        op: CmpOp::Eq,
                        lhs: MirValue::Use(tag),
                        rhs: MirValue::Literal(Literal::Int(1)),
                    });

                    let some_block = self.new_block();
                    let none_block = self.new_block();
                    let merge_block = self.new_block();

                    self.set_terminator(Terminator::Branch {
                        cond: MirValue::Use(is_some),
                        then_block: some_block,
                        else_block: none_block,
                    });

                    // Some block: extract payload, call method, wrap in Some
                    self.current_block = some_block;
                    let payload = self.new_temp(Ty::Int);
                    self.emit(MirInst::GetField {
                        dest: payload,
                        base: opt_id,
                        field_index: 1,
                    });

                    // Call the method on the extracted inner value
                    let inner_type_name = match &object.ty {
                        Ty::Option(inner) => type_name_from_ty(inner),
                        _ => String::new(),
                    };
                    // Resolve inherited methods
                    let resolved_class = match &object.ty {
                        Ty::Option(inner) => {
                            let inner_ty = match inner.as_ref() {
                                Ty::Ref(r) | Ty::RefMut(r) => r.as_ref(),
                                other => other,
                            };
                            match inner_ty {
                                Ty::Class { name, .. } => {
                                    self.resolve_method_class(name, field_name)
                                }
                                _ => inner_type_name.clone(),
                            }
                        }
                        _ => inner_type_name.clone(),
                    };
                    let mangled = format!("{}_{}", resolved_class, field_name);
                    // Use the inner type of the result Option for the method result.
                    let inner_result_ty = match &expr.ty {
                        Ty::Option(inner) => inner.as_ref().clone(),
                        _ => Ty::Int,
                    };
                    let method_result = self.new_temp(inner_result_ty);
                    // #06.8 T#14: route through alias map (see other
                    // sites in this file).
                    let callee = self.resolve_ffi_alias_callee(mangled);
                    self.emit(MirInst::Call {
                        dest: Some(method_result),
                        callee,
                        args: vec![MirValue::Use(payload)],
                    });

                    // Wrap in Some
                    self.emit(MirInst::SetTag {
                        dest: result,
                        tag: 1,
                    });
                    self.emit(MirInst::SetField {
                        base: result,
                        field_index: 1,
                        value: MirValue::Use(method_result),
                    });
                    self.set_terminator(Terminator::Goto(merge_block));

                    // None block
                    self.current_block = none_block;
                    self.emit(MirInst::SetTag {
                        dest: result,
                        tag: 0,
                    });
                    self.set_terminator(Terminator::Goto(merge_block));

                    self.current_block = merge_block;
                    return Ok(Some(result));
                }

                // Handle `ClassName.new` (no parentheses) as a constructor
                // call.  The parser resolves this as FieldAccess, but it is
                // semantically equivalent to `ClassName.new()`.
                if field_name == "new" {
                    let type_name = type_name_from_ty(&expr.ty);
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
                        // (`Vec` → `Array`, `HashMap` → `Map`, `HashSet`
                        // → `Set`). The runtime C functions keep their
                        // legacy names, so map back before mangling.
                        let runtime_base = match base_type {
                            "Array" => "Vec",
                            "Map" => "Hash",
                            "HashMap" => "Hash",
                            "Set" => "HashSet",
                            other => other,
                        };
                        // Use the base type so the mangled callee elides the
                        // generic parameter list (`HashMap[K, V]_new` would
                        // not match a real runtime symbol).
                        //
                        // #06.8 T#14: `Array.new` / `Hash.new` / etc.
                        // (no-paren form) lowers HERE rather than
                        // through `lower_method_call`. Route through
                        // `resolve_ffi_alias_callee` so the migrated
                        // `class Array do lib def self.new ... end end`
                        // entries in `library/std/array/src/lib.rvn` reach
                        // their C symbols. Try the legacy
                        // `runtime_base` first (`Vec_new` for
                        // backward-compat with any sources still in
                        // runtime_table), then the surface `base_type`
                        // (`Array_new` — where the .rvn anchor lives).
                        let raw_callee = format!("{}_new", runtime_base);
                        let aliased = self.resolve_ffi_alias_callee(raw_callee.clone());
                        let callee = if aliased != raw_callee {
                            aliased
                        } else {
                            self.resolve_ffi_alias_callee(format!("{}_new", base_type))
                        };
                        self.emit(MirInst::Call {
                            dest: Some(obj),
                            callee,
                            args: vec![],
                        });
                        return Ok(Some(obj));
                    }
                    // String.new (no parens) — direct dispatch to the
                    // runtime constructor; see #02 stdlib brief.
                    if base_type == "String" {
                        let obj = self.new_temp(expr.ty.clone());
                        self.emit(MirInst::Call {
                            dest: Some(obj),
                            callee: "String_new".to_string(),
                            args: vec![],
                        });
                        return Ok(Some(obj));
                    }

                    let obj = self.new_temp(expr.ty.clone());
                    self.emit(MirInst::Alloc {
                        dest: obj,
                        ty: expr.ty.clone(),
                        size: self.alloc_size(&expr.ty),
                    });

                    // Structs have no synthetic init — zero-arg `.new` on a
                    // struct leaves fields uninitialised (same as C). Emit
                    // just the allocation.
                    if matches!(&expr.ty, Ty::Struct { .. }) {
                        return Ok(Some(obj));
                    }

                    // Call ClassName_init(self) with no extra args
                    self.emit(MirInst::Call {
                        dest: None,
                        callee: format!("{}_init", type_name),
                        args: vec![MirValue::Use(obj)],
                    });
                    return Ok(Some(obj));
                }

                // Determine whether this FieldAccess is actually a no-arg
                // method call.  The parser produces FieldAccess whenever no
                // parentheses follow the dot, but in Riven method calls can
                // omit parens.
                let obj_type_name = self
                    .receiver_type_name(object)
                    .unwrap_or_else(|| type_name_from_ty(&object.ty));
                // Peel through references to find the underlying class type.
                let base_ty = {
                    let mut ty = &object.ty;
                    loop {
                        match ty {
                            Ty::Ref(inner)
                            | Ty::RefMut(inner)
                            | Ty::RefLifetime(_, inner)
                            | Ty::RefMutLifetime(_, inner) => {
                                ty = inner;
                            }
                            _ => break ty,
                        }
                    }
                };
                let is_field = match base_ty {
                    Ty::Class { name, .. } | Ty::Struct { name, .. } => {
                        self.is_real_field(name, field_name)
                    }
                    // Tuple fields (`.0`, `.1`, ...) are always real fields;
                    // the typechecker has already validated the index.
                    Ty::Tuple(_) => field_name.parse::<usize>().is_ok(),
                    // Newtype wrappers expose the inner value via `.0`.
                    Ty::Newtype { .. } => field_name == "0",
                    _ => false,
                };

                if !is_field && !obj_type_name.is_empty() {
                    // Phase C: if the receiver type is `&Mixin` (a
                    // runtime-dispatch single-bound reference), route
                    // through the dispatch helper instead of mangling
                    // a static `<Class>_<method>`. Spec §B5/§B6.
                    if let Some(mixin_name) = self.dyn_mixin_receiver_name(&object.ty) {
                        let obj_local = self.lower_expr(object)?;
                        let mut arg_vals: Vec<MirValue> = Vec::new();
                        if let Some(s) = obj_local {
                            arg_vals.push(MirValue::Use(s));
                        }
                        let dest = if expr.ty != Ty::Unit && expr.ty != Ty::Never {
                            Some(self.new_temp(expr.ty.clone()))
                        } else {
                            None
                        };
                        self.emit(MirInst::Call {
                            dest,
                            callee: format!("{}_dynamic_{}", mixin_name, field_name),
                            args: arg_vals,
                        });
                        return Ok(dest);
                    }

                    // This is a no-arg method call, not a field access.
                    // For static/class methods (`def self.foo`), the callee
                    // takes no `self` parameter, so omit the receiver.
                    let is_static_builtin = is_builtin_static_method(&obj_type_name, field_name)
                        || self.is_user_static_method(&obj_type_name, field_name);
                    let obj_local = self.lower_expr(object)?;

                    let dispatch_ty = if matches!(base_ty, Ty::Infer(_)) {
                        &expr.ty
                    } else {
                        base_ty
                    };
                    let is_static = is_static_builtin
                        || (field_name == "default"
                            && self.type_supports_trait(dispatch_ty, "Default"));
                    let arg_values: Vec<MirValue> = if is_static {
                        Vec::new()
                    } else {
                        vec![local_to_value(obj_local)]
                    };

                    // Resolve through parent classes for inherited methods.
                    // Use base_ty (refs peeled) to find the class name.
                    // For a generic type parameter or impl/dyn Trait,
                    // dispatch to the unique implementor of the trait bound.
                    let resolved_class = match dispatch_ty {
                        Ty::Class { name, .. } => self.resolve_method_class(name, field_name),
                        Ty::TypeParam { bounds, .. }
                        | Ty::SomeMixin(bounds)
                        | Ty::AnyMixin(bounds) => self
                            .unique_bound_impl(bounds)
                            .unwrap_or_else(|| obj_type_name.clone()),
                        _ => obj_type_name.clone(),
                    };
                    let mangled = format!("{}_{}", resolved_class, field_name);

                    let dest = if expr.ty != Ty::Unit && expr.ty != Ty::Never {
                        Some(self.new_temp(expr.ty.clone()))
                    } else {
                        None
                    };

                    // #06.8 T#14: field-access shorthand (`a.len`,
                    // `s.is_empty`) lowers HERE rather than through
                    // `lower_method_call`. Route the mangled callee
                    // through `resolve_ffi_alias_callee` so migrated
                    // builtin methods reach their C symbol via the
                    // alias map (with the generic-stripping fallback
                    // for `Array[Int]_<m>` → `Array_<m>`).
                    let callee = self.resolve_ffi_alias_callee(mangled);
                    self.emit(MirInst::Call {
                        dest,
                        callee,
                        args: arg_values,
                    });
                    return Ok(dest);
                }

                let base_local = self.lower_expr(object)?;
                if let Some(base) = base_local {
                    let dest = self.new_temp(expr.ty.clone());
                    // Phase B-4: shift field index past class_info_ptr
                    // header for runtime-dispatch classes. Returns 0
                    // for structs / static-only-mixin classes.
                    let shift = self.class_field_shift_for_ty(&object.ty);
                    self.emit(MirInst::GetField {
                        dest,
                        base,
                        field_index: *field_idx + shift,
                    });
                    Ok(Some(dest))
                } else {
                    Ok(None)
                }
            }

            // ── Borrow ──────────────────────────────────────────────
            _ => unreachable!("lower_field_access: dispatched to wrong helper"),
        }
    }
}
