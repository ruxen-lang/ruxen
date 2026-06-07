use super::super::*;

impl<'a> Lowerer<'a> {
    pub(super) fn lower_misc(&mut self, expr: &HirExpr) -> Result<Option<LocalId>, String> {
        match &expr.kind {
            // ── Cast ────────────────────────────────────────────────
            HirExprKind::Cast {
                expr: inner,
                target,
            } => {
                let inner_local = self.lower_expr(inner)?;
                let Some(src) = inner_local else {
                    // No materialised value (e.g. a Unit/void inner) — nothing
                    // to convert; pass through.
                    return Ok(None);
                };
                // A numeric cast (`v as UInt32`) must materialise a value at
                // the TARGET width, not pass the source through unchanged. The
                // pass-through form left an `Int8`/`UInt8` value flowing into
                // an enclosing `<<` / arithmetic op; Cranelift then performed
                // the op at the source width and masked the shift amount
                // (`x << 16` on an 8-bit value masks to `16 % 8 == 0`), so an
                // inline `(1u8 as UInt32) << 16` term silently contributed 0.
                // Let-binding the cast first hid the bug because the `let`'s
                // declared type forced the widening Assign coercion — do the
                // same here by binding into a fresh target-typed local. (We
                // intentionally reuse the existing Assign `coerce_value` path,
                // so signedness behaviour matches let-bound casts exactly.)
                // Types are already resolved by typeck's finalisation pass,
                // so `target` and the inner's type are concrete here.
                let resolved_target = target.clone();
                // Re-materialise for any NUMERIC→NUMERIC cast — int↔int width
                // changes (Bug B) and, since Q5, int↔float / float↔int /
                // float↔float conversions too. Binding into a target-typed
                // local routes the value through the Assign `coerce_value`
                // path, which now emits the right instruction for each pair:
                // `ireduce`/`extend` (int↔int), `fdemote`/`fpromote`
                // (float↔float), and `fcvt_from_*`/`fcvt_to_*_sat` (int↔float,
                // signedness-correct per direction). Non-numeric casts (e.g.
                // reference reinterpret) stay pass-through.
                let is_int_like = |ty: &Ty| {
                    matches!(
                        ty,
                        Ty::Int
                            | Ty::Int8
                            | Ty::Int16
                            | Ty::Int32
                            | Ty::Int64
                            | Ty::UInt
                            | Ty::UInt8
                            | Ty::UInt16
                            | Ty::UInt32
                            | Ty::UInt64
                            | Ty::ISize
                            | Ty::USize
                            | Ty::Bool
                            | Ty::Char
                    )
                };
                let is_float_like = |ty: &Ty| matches!(ty, Ty::Float | Ty::Float32 | Ty::Float64);
                let is_numeric = |ty: &Ty| is_int_like(ty) || is_float_like(ty);
                if !is_numeric(&resolved_target) || !is_numeric(&inner.ty) {
                    return Ok(Some(src));
                }
                let dest = self.new_temp(resolved_target);
                self.emit(MirInst::Assign {
                    dest,
                    value: MirValue::Use(src),
                });
                Ok(Some(dest))
            }

            // ── Array literal ───────────────────────────────────────
            // ruby-naming.spec.md §10a: bare `[a, b, c]` is the canonical
            // `Array[T]` constructor (the `array!` macro is retired). When
            // the inferred type is `Ty::Array(_)` we lower to
            // `Array.new` + `Array.push` calls; for `FixedArray[T; N]`
            // contexts we keep the slot-by-slot Alloc form so stack-
            // allocated arrays still work.

            // ── Macro calls (panic!, assert!, …) ─────────────────────
            HirExprKind::MacroCall { name, args } => {
                // ruby-naming.spec.md §10a retires the collection macros:
                // `array!` / `vec!` → `[…]` Array literal, `map!` / `hash!`
                // → `{ k => v, … }` Map literal, `set!` → `Set.from_iter([…])`.
                // The remaining macros (panic!, assert!, …) live here.
                match name.as_str() {
                    // `panic!("msg")` — evaluate the message (which may be
                    // an interpolated string), call `ruxen_panic(msg)`, and
                    // set the current block's terminator to `Unreachable`
                    // so that no code after the panic is executed.
                    "panic" => {
                        let arg_val = if let Some(first) = args.first() {
                            let local = self.lower_expr(first)?;
                            local_to_value(local)
                        } else {
                            // panic! with no message — pass an empty string.
                            let empty = self.new_temp(Ty::String);
                            self.emit(MirInst::Assign {
                                dest: empty,
                                value: MirValue::Literal(Literal::String(String::new())),
                            });
                            MirValue::Use(empty)
                        };
                        self.emit(MirInst::Call {
                            dest: None,
                            callee: "ruxen_panic".to_string(),
                            args: vec![arg_val],
                        });
                        self.set_terminator(Terminator::Unreachable);
                        // Create a dead block for any code after the panic.
                        let dead = self.new_block();
                        self.current_block = dead;
                        Ok(None)
                    }
                    // ruby-naming.spec.md §10a: the collection macros
                    // are retired. Spell `array!` / `vec!` as `[…]`,
                    // `map!` / `hash!` as `{ k => v, … }`, and `set!`
                    // as `Set.from_iter([…])`.
                    "array" | "vec" | "map" | "hash" | "set" => Err(format!(
                        "macro `{name}!` is retired — use the literal form per ruby-naming.spec §10a"
                    )),
                    _ => Ok(None),
                }
            }

            // ── Unsafe block — lower identically to a regular block ──

            // ── Catch-all for unhandled expressions ─────────────────
            HirExprKind::ArrayFill { .. } | HirExprKind::Range { .. } | HirExprKind::Error => {
                Ok(None)
            }
            _ => unreachable!("lower_misc: dispatched to wrong helper"),
        }
    }
}
