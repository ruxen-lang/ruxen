use super::super::*;

impl<'a> Lowerer<'a> {
    pub(super) fn lower_misc(&mut self, expr: &HirExpr) -> Result<Option<LocalId>, String> {
        match &expr.kind {
            // ── Cast ────────────────────────────────────────────────
            HirExprKind::Cast { expr: inner, .. } => {
                // For now, pass through the inner expression.
                self.lower_expr(inner)
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
