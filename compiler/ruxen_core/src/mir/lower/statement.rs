use super::*;

impl<'a> Lowerer<'a> {
    // ── Statement lowering ──────────────────────────────────────────────

    pub(super) fn lower_statement(&mut self, stmt: &HirStatement) -> Result<(), String> {
        match stmt {
            HirStatement::Let {
                def_id,
                ty,
                value,
                mutable,
                pattern,
                ..
            } => {
                // Handle tuple destructuring: `let (a, b) = expr`
                if let HirPattern::Tuple { elements, .. } = pattern {
                    // Lower the initializer first
                    let init_local = if let Some(init) = value {
                        self.lower_expr(init)?
                    } else {
                        None
                    };
                    let tuple_id = init_local.unwrap_or_else(|| self.new_temp(ty.clone()));

                    // Create a local for the whole tuple binding
                    let tuple_local = self.new_local_named("_tuple", ty.clone(), *mutable);
                    self.def_to_local.insert(*def_id, tuple_local);
                    self.emit(MirInst::Assign {
                        dest: tuple_local,
                        value: MirValue::Use(tuple_id),
                    });

                    // Extract each element via GetField
                    for (i, elem_pat) in elements.iter().enumerate() {
                        if let HirPattern::Binding {
                            def_id: elem_def,
                            name: elem_name,
                            ..
                        } = elem_pat
                        {
                            let elem_ty = match ty {
                                Ty::Tuple(tys) if i < tys.len() => tys[i].clone(),
                                _ => Ty::Int,
                            };
                            let elem_local = self.new_local_named(elem_name, elem_ty, *mutable);
                            self.def_to_local.insert(*elem_def, elem_local);
                            self.emit(MirInst::GetField {
                                dest: elem_local,
                                base: tuple_id,
                                field_index: i,
                            });
                        }
                    }
                    return Ok(());
                }

                // Extract the name from the pattern (use the binding name if
                // it is a simple Binding pattern, otherwise fall back to the
                // symbol table).
                let name = match pattern {
                    HirPattern::Binding { name, .. } => name.clone(),
                    _ => def_id_name(*def_id, self.symbols),
                };

                // Refine unresolved Infer types: if the initializer is a
                // method call known to return a string, use Ty::String
                // instead.  This ensures correct string interpolation for
                // variables like `let task_name = ... .unwrap_or(other.clone)`.
                let refined_ty = if matches!(ty, Ty::Infer(_)) {
                    if let Some(init_expr) = value {
                        if is_inferred_string_expr(init_expr) {
                            Ty::String
                        } else {
                            ty.clone()
                        }
                    } else {
                        ty.clone()
                    }
                } else {
                    ty.clone()
                };

                let local = self.new_local_named(&name, refined_ty.clone(), *mutable);
                self.def_to_local.insert(*def_id, local);

                if let Some(init) = value {
                    let val_local = self.lower_expr(init)?;
                    // Owned rebinding of a VarRef must emit `MirInst::Move`
                    // so drop-elaboration tracks ownership correctly.
                    if let (HirExprKind::VarRef(_), Some(src)) = (&init.kind, val_local) {
                        self.emit_transfer(local, src, &init.ty, init.ty.move_semantics());
                    } else {
                        let val = local_to_value(val_local);
                        self.emit(MirInst::Assign {
                            dest: local,
                            value: val,
                        });
                    }
                    if matches!(
                        refined_ty,
                        Ty::Class { .. } | Ty::Struct { .. } | Ty::Enum { .. }
                    ) {
                        self.initialized_heap_locals.insert(local);
                        if let Some(frame) = self.loop_stack.last_mut() {
                            frame.body_locals.push(local);
                        }
                    }
                    // NOTE (soundness, 2026-06-11): `0741468` ALSO registered
                    // built-in heap loop-body locals (`String`/`Array`/`Map`/
                    // `Set`) here for a loop-back-edge free, to stop a fresh
                    // per-iteration allocation leaking iterations 1..n-1. That
                    // free is emitted UNCONDITIONALLY at lower time
                    // (`emit_dealloc_loop_locals`) and does NOT see whether the
                    // local was MOVED OUT during the iteration — into an
                    // escaping collection (`captures.insert(name, …)`), a
                    // returned value, or a stored field. When it was, the
                    // back-edge frees an allocation the collection/return now
                    // owns → dangling pointer → use-after-free (rondo's
                    // path-param `<none>` reads; nondeterministic heap
                    // corruption across the whole dispatch path). The scope-exit
                    // drop pass (`compute_dealloc_safe_locals`) tracks those
                    // moves correctly via the `arg_transfer` taint, so freeing
                    // built-in heap loop-body locals ONLY at scope exit is
                    // sound. Reverted to that behavior: a genuinely-owned
                    // per-iteration built-in heap local LEAKS until scope exit
                    // (the accepted leak class filed in TASKS.md) rather than
                    // dangling. Soundness strictly beats leak-fixing; the
                    // move-aware back-edge free needs the dealloc-safe analysis
                    // run BEFORE the loop edge is emitted, which is the filed
                    // follow-up. (Class/Struct/Enum loop locals stay registered
                    // above — they use `ruxen_dealloc` and the W15 user-`def
                    // drop` path depends on the back-edge call.)
                }
                Ok(())
            }
            HirStatement::Expr(expr) => {
                let _ = self.lower_expr(expr)?;
                Ok(())
            }
        }
    }
}
