use super::super::*;

impl<'a> Lowerer<'a> {
    pub(super) fn lower_blocks(&mut self, expr: &HirExpr) -> Result<Option<LocalId>, String> {
        match &expr.kind {
            // ── Block ───────────────────────────────────────────────
            HirExprKind::Block(stmts, tail) => {
                for stmt in stmts {
                    self.lower_statement(stmt)?;
                }
                if let Some(tail_expr) = tail {
                    self.lower_expr(tail_expr)
                } else {
                    Ok(None)
                }
            }

            // ── If / else ───────────────────────────────────────────

            // ── Unsafe block — lower identically to a regular block ──
            HirExprKind::UnsafeBlock(stmts, tail) => {
                for stmt in stmts {
                    self.lower_statement(stmt)?;
                }
                if let Some(tail_expr) = tail {
                    self.lower_expr(tail_expr)
                } else {
                    Ok(None)
                }
            }

            // ── `nil` literal ─────────────────────────────────────────
            // ruby-naming.spec.md §3.10: `nil` is polymorphic. Lowering
            // splits on the resolved type:
            //   * `Option[T]` → construct the `None` variant (tag 0).
            //   * Anything else (raw pointer, USize, UInt64) → zero
            //     value, matching the legacy `null` semantics.
            _ => unreachable!("lower_blocks: dispatched to wrong helper"),
        }
    }
}
