use super::*;
use crate::hir::visit::{walk_expr, Visit};

mod each;
mod entry_or_insert;
mod retain;

/// Does `body` contain a `next` that belongs to it (Block Tier-2)? Walks the
/// block, descending through control flow (`if`/`match`/`for`/`while`/loops —
/// which use `continue`/`break`, never `next`) and every other nested
/// expression, but STOPPING at a nested closure boundary (a `next` inside a
/// nested block is that block's, not ours) and at `return`. Used to reject a
/// `next` in a block that is about to be MIR-inlined (E1125).
fn block_body_has_own_next(body: &HirExpr) -> bool {
    let mut finder = OwnNextFinder { found: false };
    finder.visit_expr(body);
    finder.found
}

struct OwnNextFinder {
    found: bool,
}

impl Visit for OwnNextFinder {
    fn visit_expr(&mut self, expr: &HirExpr) {
        if self.found {
            return;
        }
        if matches!(expr.kind, HirExprKind::Next(_)) {
            self.found = true;
            return;
        }
        // A nested closure owns its own `next`; `return` is a separate jump.
        if matches!(
            expr.kind,
            HirExprKind::Closure { .. } | HirExprKind::Return(_)
        ) {
            return;
        }
        walk_expr(self, expr);
    }
}

impl<'a> Lowerer<'a> {
    pub(super) fn try_inline_closure_method(
        &mut self,
        object: &HirExpr,
        method_name: &str,
        block_expr: &HirExpr,
    ) -> Result<Option<Option<LocalId>>, String> {
        // Extract closure params and body from the block expression.
        let (closure_params, closure_body) = match &block_expr.kind {
            HirExprKind::Closure { params, body, .. } => (params, body),
            _ => return Ok(None), // Not a closure — can't inline.
        };

        // Only a backing Vec is inlined: an `Array` receiver (structural, or
        // the `Ty::Class { name: "Array" }` self inside an `Array[T]` stdlib
        // body). Every other receiver, including a user class whose methods
        // happen to be named `each`/`map`/`find`/…, falls through so the
        // normal method-call path calls that receiver's own method body
        // (blocker 5, e2e 2150).
        //
        // Nothing here may inspect the receiver's own method name. A helper
        // that did (`lower_vec_source`, deleted) matched `select`/`reject`/
        // `where_matching` and recursed into THEIR object, so
        // `a.select { … }.each { … }` dropped the `select` and iterated all of
        // `a` (B85, pins 1860-1869).
        if !is_vec_or_iterator_type(&object.ty) {
            return Ok(None);
        }

        // `each` and `select!` are the only combinators still inlined over an
        // `Array`; every other block-taking `Array` method (`map`, `select`,
        // `find`, `partition`, …) is a real `.rx` body on `class Array[T]`
        // and goes through the normal method-call path.
        if !matches!(method_name, "each" | "select!") {
            return Ok(None);
        }
        let concrete_item_ty = match object.ty.peel_refs() {
            Ty::Array(inner) => Some((**inner).clone()),
            _ => None,
        };

        // Block Tier-2: a `next` that belongs to THIS block cannot be inlined —
        // an inlined body shares the enclosing function's frame, so lowering a
        // `next` there (`Terminator::Return`) would return from the ENCLOSING
        // function (the non-local return the ADR rejects), not just this
        // iteration. Reject it with a clear message. `next` still works in every
        // NON-inlined block: a `&block`/`yield` method body, and real `Array`
        // `map`/`select`/`reject`/`find`/`index`/`all?`/`any?`/`partition`/
        // `each_with_index`/`reduce` (which route to real closures over
        // `Enumerable`), where it lowers to a normal early return from the
        // block's own closure. A `next` inside a NESTED closure within this body
        // belongs to that closure and is not flagged.
        if block_body_has_own_next(closure_body) {
            return Err(format!(
                "`next` is not supported inside an inlined `{method_name}` block \
                 (it would become a non-local return). Use `map`/`select`/`find` \
                 (which run the block as a real closure) or restructure with \
                 `if`/`for … continue`."
            ));
        }

        let vec_local = self.lower_expr(object)?;
        let vec_id = vec_local.unwrap_or_else(|| self.new_temp(Ty::Int));
        if method_name == "each" {
            // for i in 0..vec.len: item = vec[i]; <body>
            self.inline_each(
                vec_id,
                closure_params,
                closure_body,
                concrete_item_ty.as_ref(),
            )?;
        } else {
            // `select!`: in-place filter.
            self.inline_retain(vec_id, closure_params, closure_body)?;
        }
        Ok(Some(None))
    }
}
