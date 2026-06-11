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
                    } else if matches!(
                        refined_ty,
                        Ty::String | Ty::Array(_) | Ty::Map(_, _) | Ty::Set(_)
                    ) {
                        // Built-in heap-owning locals declared in a loop body
                        // must ALSO be freed at the loop back-edge / break /
                        // continue, not only at function scope exit — otherwise
                        // each iteration's allocation leaks and only the final
                        // iteration's value is reclaimed by the scope-exit drop.
                        // `emit_dealloc_loop_locals` selects the type-correct
                        // free callee (`ruxen_string_free` / `ruxen_vec_free` /
                        // …). Class/Struct/Enum are registered above (they use
                        // `ruxen_dealloc`); these built-ins need their dedicated
                        // helpers. Surfaced by the borrow-in-loop drop-matrix
                        // pin (a `String` borrowed into a user `&String` fn
                        // inside a `while` leaked iterations 1..n-1).
                        if let Some(frame) = self.loop_stack.last_mut() {
                            frame.body_locals.push(local);
                        }
                    }
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
