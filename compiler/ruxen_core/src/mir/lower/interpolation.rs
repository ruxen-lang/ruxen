use super::*;

impl<'a> Lowerer<'a> {
    /// Phase 2 #06.D4: when `spec` is non-default, the formatter is
    /// constructed via `Formatter_new_with_spec(width, precision, align,
    /// fill)` so the runtime can apply width / align / fill at finalize
    /// and the synth `_fmt` body can read precision via
    /// `Formatter_precision`.
    pub(super) fn emit_display_dispatch(
        &mut self,
        src: LocalId,
        callee_name: &str,
        spec: Option<&crate::lexer::token::FormatSpec>,
    ) -> LocalId {
        let fmt_local = self.new_temp(Ty::Class {
            name: "Formatter".to_string(),
            generic_args: vec![],
        });
        let use_spec = spec.map(|s| !s.is_default()).unwrap_or(false);
        if use_spec {
            let spec = spec.unwrap();
            let (width, precision, align, fill) = encode_format_spec(spec);
            self.emit(MirInst::Call {
                dest: Some(fmt_local),
                callee: "Formatter_new_with_spec".to_string(),
                args: vec![
                    MirValue::Literal(Literal::Int(width)),
                    MirValue::Literal(Literal::Int(precision)),
                    MirValue::Literal(Literal::Int(align)),
                    MirValue::Literal(Literal::Int(fill)),
                ],
            });
        } else {
            self.emit(MirInst::Call {
                dest: Some(fmt_local),
                callee: "Formatter_new".to_string(),
                args: vec![],
            });
        }
        self.emit(MirInst::Call {
            dest: None,
            callee: callee_name.to_string(),
            args: vec![MirValue::Use(src), MirValue::Use(fmt_local)],
        });
        let dest = self.new_temp(Ty::String);
        self.emit(MirInst::Call {
            dest: Some(dest),
            callee: "Formatter_buffer".to_string(),
            args: vec![MirValue::Use(fmt_local)],
        });
        dest
    }

    pub(super) fn lower_interpolation(
        &mut self,
        parts: &[HirInterpolationPart],
        _result_ty: &Ty,
    ) -> Result<Option<LocalId>, String> {
        if parts.is_empty() {
            let dest = self.new_temp(Ty::String);
            self.emit(MirInst::StringLiteral {
                dest,
                value: String::new(),
            });
            return Ok(Some(dest));
        }

        let mut accumulated: Option<LocalId> = None;

        for part in parts {
            let part_local = match part {
                HirInterpolationPart::Literal(s) => {
                    let dest = self.new_temp(Ty::String);
                    self.emit(MirInst::StringLiteral {
                        dest,
                        value: s.clone(),
                    });
                    dest
                }
                HirInterpolationPart::Expr { expr, spec } => {
                    // Phase 2 #06.B: `spec` is captured at lex time
                    // and consumed here. Phase C uses `spec.debug`
                    // to force Debug formatting; Phase D will route
                    // through `Display::fmt` and consume width/
                    // precision/align/fill via `Formatter`.
                    //
                    // Phase C semantics: `"#{x:?}"` always uses the
                    // `{Type}_to_debug` path when the type derives
                    // Debug. Bare `"#{x}"` keeps the legacy behaviour
                    // (struct-with-derive-Debug also lowers to
                    // `_to_debug` — Phase D will switch this to
                    // `Display::fmt` once the canonical interp path
                    // is migrated).
                    let _spec_debug = spec.debug;
                    let val_local = self.lower_expr(expr)?;

                    // Determine the effective type for the interpolation.
                    // Prefer the MIR local's type (which may have been
                    // corrected by enum variant field type lookup) over
                    // the HIR expression type (which may have stale or
                    // unresolved types from type inference).
                    let effective_ty = val_local
                        .and_then(|lid| {
                            self.fn_mut().locals.get(lid as usize).map(|l| l.ty.clone())
                        })
                        .unwrap_or_else(|| expr.ty.clone());

                    // Phase 2 #06.D2.S3 dispatch priority (top-down):
                    //   1. string-like / inferred-string  → pass-through
                    //   2. user `impl Display for T`      → `{T}_fmt` via Formatter
                    //   3. struct with `derive Debug`     → `{Name}_to_debug` (legacy)
                    //   4. enum with `derive Debug`       → `{Name}_to_debug` (legacy)
                    //   5. primitive Char/Int/Float/Bool  → synth `{Prim}_fmt` via Formatter
                    //   6. anything else                  → synth `Int_fmt` (pointer-as-int fallback)
                    //
                    // Priorities 2/5/6 emit the canonical Display dispatch:
                    //     fmt = Formatter_new()
                    //     {T}_fmt(value, fmt)
                    //     buf = Formatter_buffer(fmt)
                    // The synth `{Prim}_fmt` fns (Stage 1) wrap the same
                    // `ruxen_<prim>_to_string` helpers the legacy direct path
                    // used, so output is byte-identical for all fixtures.
                    // `user_has_impl_display` is checked BEFORE the derive-Debug
                    // arms so a user-supplied `impl Display for T` wins over
                    // an auto-derived `Debug` formatter.
                    // Phase 2 #06.D4: when the spec is non-default we
                    // must route strings through `String_fmt` so the
                    // Formatter can apply width / precision / align /
                    // fill — the legacy pass-through skips the formatter
                    // entirely and would silently drop the spec.
                    if (is_string_like(&effective_ty) || is_inferred_string_expr(expr))
                        && spec.is_default()
                    {
                        val_local.unwrap_or_else(|| {
                            let d = self.new_temp(Ty::String);
                            self.emit(MirInst::StringLiteral {
                                dest: d,
                                value: String::new(),
                            });
                            d
                        })
                    } else if let Some(user_t) = self.user_has_impl_display(&effective_ty) {
                        // Priority #2: user `impl Display for T`.
                        let src = val_local.unwrap_or_else(|| {
                            let d = self.new_temp(Ty::String);
                            self.emit(MirInst::StringLiteral {
                                dest: d,
                                value: String::new(),
                            });
                            d
                        });
                        self.emit_display_dispatch(src, &format!("{}_fmt", user_t), Some(spec))
                    } else if let Some(struct_name) = self.struct_with_derive_debug(&effective_ty) {
                        // Priority #3: struct with `derive Debug` (and no user
                        // `impl Display`) — keep the legacy `{Name}_to_debug`
                        // path so bare `"#{x}"` still prints the formatted
                        // struct rather than a raw pointer address.
                        let src = val_local.unwrap_or_else(|| {
                            let d = self.new_temp(Ty::String);
                            self.emit(MirInst::StringLiteral {
                                dest: d,
                                value: String::new(),
                            });
                            d
                        });
                        let dest = self.new_temp(Ty::String);
                        self.emit(MirInst::Call {
                            dest: Some(dest),
                            callee: format!("{}_to_debug", struct_name),
                            args: vec![MirValue::Use(src)],
                        });
                        dest
                    } else if let Some(enum_name) = self.enum_with_derive_debug(&effective_ty) {
                        // Priority #4: enum with `derive Debug` (Phase 2 #06.C2).
                        let src = val_local.unwrap_or_else(|| {
                            let d = self.new_temp(Ty::String);
                            self.emit(MirInst::StringLiteral {
                                dest: d,
                                value: String::new(),
                            });
                            d
                        });
                        let dest = self.new_temp(Ty::String);
                        self.emit(MirInst::Call {
                            dest: Some(dest),
                            callee: format!("{}_to_debug", enum_name),
                            args: vec![MirValue::Use(src)],
                        });
                        dest
                    } else {
                        // Priorities #5 + #6: primitives + last-resort
                        // fallback.  Each dispatches through the canonical
                        // `Formatter_new` → `{Prim}_fmt(value, fmt)` →
                        // `Formatter_buffer(fmt)` sequence.  `Char` must be
                        // checked BEFORE `is_integer()` because `Char` is a
                        // 32-bit codepoint and currently also satisfies the
                        // integer predicate in some lowerings — without this
                        // priority a `Char` would render as a decimal number.
                        let src = val_local.unwrap_or_else(|| {
                            let d = self.new_temp(Ty::String);
                            self.emit(MirInst::StringLiteral {
                                dest: d,
                                value: String::new(),
                            });
                            d
                        });
                        let fmt_callee = if effective_ty == Ty::Char {
                            "Char_fmt"
                        } else if is_string_like(&effective_ty) {
                            // Phase 2 #06.D4: a String value with a
                            // non-default spec falls here (the spec-
                            // default pass-through above is skipped).
                            // Route through `String_fmt` so width /
                            // precision / align / fill all apply.
                            "String_fmt"
                        } else if effective_ty.is_integer() {
                            "Int_fmt"
                        } else if effective_ty.is_float() {
                            "Float_fmt"
                        } else if effective_ty == Ty::Bool {
                            "Bool_fmt"
                        } else {
                            // Unknown type — treat as integer (pointer
                            // value) as a fallback.  This handles USize,
                            // enum tags, and any not-yet-inferred type.
                            // Preserves the pre-Stage-3 default behaviour.
                            "Int_fmt"
                        };
                        self.emit_display_dispatch(src, fmt_callee, Some(spec))
                    }
                }
            };

            accumulated = Some(match accumulated {
                None => part_local,
                Some(prev) => {
                    let dest = self.new_temp(Ty::String);
                    self.emit(MirInst::Call {
                        dest: Some(dest),
                        callee: "ruxen_string_concat".to_string(),
                        args: vec![MirValue::Use(prev), MirValue::Use(part_local)],
                    });
                    dest
                }
            });
        }

        Ok(accumulated)
    }
}
