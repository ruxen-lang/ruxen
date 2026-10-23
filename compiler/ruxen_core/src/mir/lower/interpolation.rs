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

    /// A borrowed scalar (`&Float`, `&var Bool`) carries the scalar's own
    /// word, not an address (see `mir::refmut_abi`), so it is formatted as
    /// its pointee. A float's word is its IEEE-754 bit pattern and is
    /// reinterpreted with `BitsToFloat`, never converted numerically. Any
    /// other type is returned unchanged.
    pub(super) fn deref_borrowed_scalar(
        &mut self,
        val_local: Option<LocalId>,
        ty: &Ty,
    ) -> (Option<LocalId>, Ty) {
        let inner = ty.peel_refs();
        if inner == ty || !inner.is_scalar_primitive() {
            return (val_local, ty.clone());
        }
        let inner = inner.clone();
        match val_local {
            Some(src) if inner.is_float() => {
                let dest = self.new_temp(inner.clone());
                self.emit(MirInst::BitsToFloat { dest, src });
                (Some(dest), inner)
            }
            other => (other, inner),
        }
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

        // The running result and whether this function owns it. A part is
        // owned when a Fresh callee produced it (`Formatter#buffer`, the
        // `to_s` helpers, a `concat`); a literal or a passed-through `String`
        // value is not.
        let mut accumulated: Option<(LocalId, bool)> = None;

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
                    let local_ty = val_local
                        .and_then(|lid| {
                            self.fn_mut().locals.get(lid as usize).map(|l| l.ty.clone())
                        })
                        .unwrap_or_else(|| expr.ty.clone());
                    let effective_ty = if local_ty.is_infer()
                        || local_ty.is_error()
                        || matches!(local_ty, Ty::Unit | Ty::Never | Ty::TypeParam { .. })
                    {
                        self.declared_method_return_ty(expr)
                            .filter(|ty| !ty.is_infer() && !ty.is_error())
                            .unwrap_or(local_ty)
                    } else {
                        local_ty
                    };

                    self.emit_display_to_string(val_local, &effective_ty, spec)
                        .map_err(|e| format!("{e} (at {}:{})", expr.span.line, expr.span.column))?
                }
            };

            let part_owned = self.is_fresh_call_result(part_local);
            accumulated = Some(match accumulated {
                None => (part_local, part_owned),
                Some((prev, prev_owned)) => {
                    let dest = self.new_temp(Ty::String);
                    self.emit(MirInst::Call {
                        dest: Some(dest),
                        callee: "ruxen_string_concat".to_string(),
                        args: vec![MirValue::Use(prev), MirValue::Use(part_local)],
                    });
                    // `concat` copies both operands, so the intermediate
                    // pieces this interpolation built are dead here.
                    if prev_owned {
                        self.free_interpolation_temp(prev);
                    }
                    if part_owned {
                        self.free_interpolation_temp(part_local);
                    }
                    (dest, true)
                }
            });
        }

        Ok(accumulated.map(|(result, _)| result))
    }

    /// True when `local` was just defined, in the current block, by a call
    /// whose result is a fresh allocation the caller owns
    /// (`runtime_abi::result_ownership`).
    fn is_fresh_call_result(&self, local: LocalId) -> bool {
        use crate::mir::lower::runtime_abi::{result_ownership, ResultOwnership};
        self.fn_ref().blocks[self.current_block]
            .instructions
            .iter()
            .rev()
            .find_map(|inst| match inst {
                MirInst::Call {
                    dest: Some(dest),
                    callee,
                    ..
                } if *dest == local => {
                    Some(result_ownership(callee, &self.declared_results) == ResultOwnership::Fresh)
                }
                _ => None,
            })
            .unwrap_or(false)
    }

    /// Free an owned interpolation temporary right after its last use and
    /// zero it. The zero store retires the temp: drop elaboration sees a
    /// non-allocation value and never frees it a second time.
    fn free_interpolation_temp(&mut self, temp: LocalId) {
        self.emit(MirInst::Call {
            dest: None,
            callee: "ruxen_string_free".to_string(),
            args: vec![MirValue::Use(temp)],
        });
        self.emit(MirInst::Assign {
            dest: temp,
            value: MirValue::Literal(Literal::Int(0)),
        });
    }

    /// Render an already-lowered value as a `String` through the Display
    /// ladder, returning the local holding the result.
    ///
    /// SINGLE SOURCE for "turn a value of any type into printable text".
    /// Extracted from `lower_interpolation` so that the two USER-VISIBLE
    /// spellings of the same intent cannot diverge:
    ///
    ///   * `puts "#{x}"` — interpolation, via `lower_interpolation`
    ///   * `puts x`      — a bare argument, via `lower_fn_call`
    ///
    /// They used to diverge, and it was a segfault (B02): interpolation
    /// consulted the MIR local's type and formatted correctly, while the
    /// bare form passed the raw word straight to `ruxen_puts`, which
    /// dereferenced an `Int` as a `char*`. Anything that needs a value as
    /// text must come through here rather than re-deriving a `_fmt`
    /// callee, because a second copy of this ladder is free to drift from
    /// this one exactly the way the bare `puts` path did.
    ///
    /// `ty` must be the EFFECTIVE type — the MIR local's type where there
    /// is one, not the HIR expression's. See the caller in
    /// `lower_interpolation` for why the HIR type is not trustworthy here.
    pub(super) fn emit_display_to_string(
        &mut self,
        val_local: Option<LocalId>,
        effective_ty: &Ty,
        spec: &crate::lexer::token::FormatSpec,
    ) -> Result<LocalId, String> {
        let (val_local, effective_ty) = self.deref_borrowed_scalar(val_local, effective_ty);
        // `self` inside `extension Int` is typed as the method-home class;
        // its value is the primitive.
        let effective_ty = match &effective_ty {
            Ty::Class { name, generic_args } if generic_args.is_empty() => {
                type_helpers::primitive_ty_by_name(name).unwrap_or(effective_ty)
            }
            _ => effective_ty,
        };
        let formatted = {
            // Phase 2 #06.D2.S3 dispatch priority (top-down):
            //   1. string-like                    → pass-through
            //   2. user `impl Display for T`      → `{T}_fmt` via Formatter
            //   3. struct with `derive Debug`     → `{Name}_to_debug` (legacy)
            //   4. enum with `derive Debug`       → `{Name}_to_debug` (legacy)
            //   5. primitive Char/Int/Float/Bool  → synth `{Prim}_fmt` via Formatter
            //   6. abstract type parameter        → synth `Int_fmt` (opaque generic ABI)
            //   7. unresolved inference variable  → abstract callee (pruned or reported)
            //   8. Unit / Never / Error           → lowering error
            //   9. other concrete type            → synth `Int_fmt` (raw word; see the arm)
            //
            // A borrowed scalar is formatted as its pointee
            // (`deref_borrowed_scalar`) before the ladder runs.
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
            if is_string_like(&effective_ty) && spec.is_default() {
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
                    callee: format!("{}_{}", struct_name, self.debug_method()),
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
                    callee: format!("{}_{}", enum_name, self.debug_method()),
                    args: vec![MirValue::Use(src)],
                });
                dest
            } else {
                // Priorities #5 + #6: primitives + opaque generic
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
                let abstract_fmt;
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
                } else if matches!(effective_ty, Ty::TypeParam { .. }) {
                    // Unspecialized generic bodies use the compiler's
                    // uniform-word ABI. Concrete generic call sites are
                    // monomorphized before this dispatch when possible.
                    "Int_fmt"
                } else if effective_ty.is_infer() {
                    // A type inference left open has no formatter yet. In
                    // an opaque generic body that a monomorphised copy
                    // replaces, the abstract callee lets
                    // `prune_unmonomorphised_bodies` drop the body; in a
                    // live body it is reported there as an error.
                    abstract_fmt = format!("{effective_ty}_fmt");
                    &abstract_fmt
                } else if matches!(effective_ty, Ty::Unit | Ty::Never | Ty::Error) {
                    return Err(format!(
                        "cannot interpolate value of type `{effective_ty}`: the type has no Display formatter"
                    ));
                } else {
                    // A concrete type with no formatter (`Array`, `Option`,
                    // a tuple) still prints its word. Making this an error
                    // breaks every generic body that interpolates its `T`
                    // (`Matcher[T]#to_eq` with `T = Array[Int]`) until
                    // those types have a Display formatter.
                    "Int_fmt"
                };
                self.emit_display_dispatch(src, fmt_callee, Some(spec))
            }
        };
        Ok(formatted)
    }
}
