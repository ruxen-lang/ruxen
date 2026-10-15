use super::*;

mod all_any;
mod each;
mod each_with_index;
mod entry_or_insert;
mod filter;
mod find;
mod map;
mod partition;
mod position;
mod retain;

/// Does `body` contain a `next` that belongs to it (Block Tier-2)? Walks the
/// block, descending through control flow (`if`/`match`/`for`/`while`/loops —
/// which use `continue`/`break`, never `next`) but STOPPING at a nested closure
/// boundary (a `next` inside a nested block is that block's, not ours). Used to
/// reject a `next` in a block that is about to be MIR-inlined.
fn block_body_has_own_next(body: &HirExpr) -> bool {
    match &body.kind {
        HirExprKind::Next(_) => true,
        // Nested closure owns its own `next`; `return` is a separate jump.
        HirExprKind::Closure { .. } | HirExprKind::Return(_) => false,
        HirExprKind::Loop { body } => block_body_has_own_next(body),
        HirExprKind::While { condition, body } => {
            block_body_has_own_next(condition) || block_body_has_own_next(body)
        }
        HirExprKind::For { iterable, body, .. } => {
            block_body_has_own_next(iterable) || block_body_has_own_next(body)
        }
        HirExprKind::Block(stmts, tail) => {
            stmts.iter().any(|s| match s {
                HirStatement::Let { value: Some(v), .. } => block_body_has_own_next(v),
                HirStatement::Expr(e) => block_body_has_own_next(e),
                _ => false,
            }) || tail.as_deref().is_some_and(block_body_has_own_next)
        }
        HirExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            block_body_has_own_next(cond)
                || block_body_has_own_next(then_branch)
                || else_branch.as_deref().is_some_and(block_body_has_own_next)
        }
        HirExprKind::Match { scrutinee, arms } => {
            block_body_has_own_next(scrutinee)
                || arms.iter().any(|a| {
                    a.guard.as_deref().is_some_and(block_body_has_own_next)
                        || block_body_has_own_next(&a.body)
                })
        }
        HirExprKind::BinaryOp { left, right, .. } => {
            block_body_has_own_next(left) || block_body_has_own_next(right)
        }
        HirExprKind::UnaryOp { operand, .. } => block_body_has_own_next(operand),
        HirExprKind::Borrow { expr: inner, .. } => block_body_has_own_next(inner),
        HirExprKind::Assign { target, value, .. }
        | HirExprKind::CompoundAssign { target, value, .. } => {
            block_body_has_own_next(target) || block_body_has_own_next(value)
        }
        HirExprKind::FnCall { args, .. } => args.iter().any(block_body_has_own_next),
        HirExprKind::MethodCall {
            object,
            args,
            block,
            ..
        } => {
            block_body_has_own_next(object)
                || args.iter().any(block_body_has_own_next)
                || block.as_deref().is_some_and(block_body_has_own_next)
        }
        HirExprKind::FieldAccess { object, .. } => block_body_has_own_next(object),
        HirExprKind::Interpolation { parts } => parts.iter().any(|p| {
            matches!(p, HirInterpolationPart::Expr { expr, .. } if block_body_has_own_next(expr))
        }),
        _ => false,
    }
}

impl<'a> Lowerer<'a> {
    pub(super) fn try_inline_closure_method(
        &mut self,
        expr: &HirExpr,
        object: &HirExpr,
        method_name: &str,
        // Closure-method args (`reduce`'s `init`) were consumed only by the
        // now-migrated `inline_fold`; retained in the signature for the
        // caller's uniform dispatch but no longer read here.
        _args: &[HirExpr],
        block_expr: &HirExpr,
    ) -> Result<Option<Option<LocalId>>, String> {
        // Extract closure params and body from the block expression.
        let (closure_params, closure_body) = match &block_expr.kind {
            HirExprKind::Closure { params, body, .. } => (params, body),
            _ => return Ok(None), // Not a closure — can't inline.
        };

        // `Option#map` / `Result#map` / `Result#map_err` /
        // `unwrap_or_else` are migrated to real `.rx` bodies
        // (`Option_map` / `Result_map` / `Option_unwrap_or_else` / …,
        // option_result/src/lib.rx). Fall through so the normal
        // method-call path emits a call to the opaque body rather than
        // inlining here. Without this, the `is_collection_method` field-0
        // branch below would dereference an Option/Result box at offset 0
        // as a backing Vec and iterate garbage (segfault).
        if (is_option_type(&object.ty) && matches!(method_name, "map" | "unwrap_or_else"))
            || (is_result_type(&object.ty)
                && matches!(method_name, "map" | "map_err" | "unwrap_or_else"))
        {
            return Ok(None);
        }

        // Builtin collection receivers that are NOT a plain Vec/Array
        // (`Ty::Map` / `Ty::Set`) have their combinators migrated to real
        // `.rx` method bodies (`Hash_each`, `Set_each`, …). Such a receiver
        // is NOT a user-defined Vec-wrapping class, so the
        // `is_collection_method` field-0 branch below would dereference a
        // non-existent backing Vec at offset 0 and iterate garbage (the
        // observed `Hash#each` "yields nothing" bug). Fall through so the
        // normal method-call path emits a real call to the opaque body.
        //
        // The receiver shows up in TWO shapes: structurally as
        // `Ty::Map`/`Ty::Set` at a user call site (`s.map { … }`), and as
        // `Ty::Class { name: "Set"|"Hash"|"Map" }` for the `self` inside an
        // opaque mixin-default combinator body (`Set_map`'s `self.each`),
        // because a class's self-type is `Ty::Class { name }` not the
        // primitive head. Both must fall through to the real `Set_each` /
        // `Hash_each` call — only `Array`'s self IS a backing Vec (handled
        // by `is_vec_or_iterator_type`). Without this, `Set`/`Hash`
        // combinators routed through `Enumerable[T]` "yield nothing".
        {
            let mut recv = &object.ty;
            while let Ty::Ref(inner)
            | Ty::RefMut(inner)
            | Ty::RefLifetime(_, inner)
            | Ty::RefMutLifetime(_, inner) = recv
            {
                recv = inner;
            }
            let is_builtin_non_vec_collection = match recv {
                Ty::Map(_, _) | Ty::Set(_) => true,
                Ty::Class { name, .. } => {
                    let base = name.split('[').next().unwrap_or(name);
                    matches!(base, "Set" | "Hash" | "Map")
                }
                _ => false,
            };
            if is_builtin_non_vec_collection {
                return Ok(None);
            }
        }

        // Determine the Vec we iterate. For Vec/iterator types the receiver is
        // an ordinary expression (a variable, a field, or a chained call whose
        // RESULT is that Vec), so lower it normally. For user-defined classes
        // with known collection-wrapping methods (where_matching, display_all,
        // into_filtered, each), access the class's first field (items Vec).
        //
        // Nothing here may inspect the receiver's own method name. A helper
        // that did (`lower_vec_source`, deleted) matched `select`/`reject`/
        // `where_matching` and recursed into THEIR object, so
        // `a.select { … }.each { … }` dropped the `select` and iterated all of
        // `a`. That was a silent wrong answer, while the bound-first spelling
        // (`let b = a.select { … }; b.each { … }`) was correct (B85, pins
        // 1860-1869).
        let vec_id = if is_vec_or_iterator_type(&object.ty) {
            let vec_local = self.lower_expr(object)?;
            vec_local.unwrap_or_else(|| self.new_temp(Ty::Int))
        } else if is_collection_method(method_name) {
            // User-defined class: lower the object and access its first
            // field to get the underlying Vec.
            let obj_local = self.lower_expr(object)?;
            let obj_id = obj_local.unwrap_or_else(|| self.new_temp(Ty::Int));
            let items_local = self.new_temp(Ty::Int);
            self.emit(MirInst::GetField {
                dest: items_local,
                base: obj_id,
                field_index: 0,
            });
            items_local
        } else {
            return Ok(None);
        };

        // Feature C: `map` / `select` / `reject` / `all?` / `any?` /
        // `partition` are now real `.rx` method bodies on `class Array[T]`
        // (resolved through the builtin bridge, lowered as opaque
        // `Array_<m>` functions). For a genuine `Ty::Array` receiver these
        // must NOT be inlined here — fall through (`Ok(None)`) so the normal
        // method-call path emits the call to the migrated body. The inline
        // path is RETAINED for user-defined collection classes that wrap a
        // Vec (the `is_collection_method` branch above lowered such a
        // receiver's field-0 Vec into `vec_id`): those classes have no
        // `Array_<m>` body to call, so they still need the inline expansion.
        let recv_ty = {
            let mut t = &object.ty;
            loop {
                match t {
                    Ty::Ref(inner)
                    | Ty::RefMut(inner)
                    | Ty::RefLifetime(_, inner)
                    | Ty::RefMutLifetime(_, inner) => t = inner,
                    other => break other,
                }
            }
        };
        let concrete_item_ty = match recv_ty {
            Ty::Array(inner) => Some((**inner).clone()),
            _ => None,
        };
        let array_receiver = matches!(recv_ty, Ty::Array(_))
            || matches!(recv_ty, Ty::Class { name, .. } if {
                let base = name.split('[').next().unwrap_or(name);
                base == "Array"
            });
        if array_receiver
            && matches!(
                method_name,
                "map"
                    | "select"
                    | "reject"
                    | "all?"
                    | "any?"
                    | "partition"
                    | "each_with_index"
                    | "find"
                    | "index"
                    | "sort_by"
                    | "reduce"
            )
        {
            return Ok(None);
        }

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

        match method_name {
            "each" => {
                // for i in 0..vec.len: item = vec[i]; <body>
                self.inline_each(
                    vec_id,
                    closure_params,
                    closure_body,
                    concrete_item_ty.as_ref(),
                )?;
                Ok(Some(None))
            }
            "each_with_index" => {
                // for i in 0..vec.len: item = vec[i]; idx = i; <body>
                self.inline_each_with_index(vec_id, closure_params, closure_body)?;
                Ok(Some(None))
            }
            "select" | "where_matching" => {
                // result = Vec.new(); for i in 0..vec.len: item = vec[i]; if <pred>: result.push(item)
                let result =
                    self.inline_filter(expr, vec_id, closure_params, closure_body, false)?;
                Ok(Some(Some(result)))
            }
            "reject" => {
                // Ruby `reject` — the inverse of `select`: keep elements
                // where the predicate is FALSE. Reuses the filter loop with
                // the predicate negated.
                let result =
                    self.inline_filter(expr, vec_id, closure_params, closure_body, true)?;
                Ok(Some(Some(result)))
            }
            "find" => {
                // for i in 0..vec.len: item = vec[i]; if <pred>: return Some(item); return None
                let result = self.inline_find(expr, vec_id, closure_params, closure_body)?;
                Ok(Some(Some(result)))
            }
            "index" => {
                // for i in 0..vec.len: item = vec[i]; if <pred>: return Some(i); return nil
                let result = self.inline_position(expr, vec_id, closure_params, closure_body)?;
                Ok(Some(Some(result)))
            }
            "map" => {
                // result = Vec.new(); for i in 0..vec.len: item = vec[i]; result.push(<expr>)
                let result = self.inline_map(expr, vec_id, closure_params, closure_body)?;
                Ok(Some(Some(result)))
            }
            "partition" => {
                // true_vec = Vec.new(); false_vec = Vec.new(); for ...; return (true_vec, false_vec)
                let result = self.inline_partition(expr, vec_id, closure_params, closure_body)?;
                Ok(Some(Some(result)))
            }
            // Phase 2 stdlib batch 2 (#03): closure-takers reuse the
            // same per-element loop machinery as `each` / `filter`.
            //
            //  * `retain { |x| keep? }`    — in-place filter.
            //
            // `sort_by` MIGRATED to a real `.rx` body (Feature C); for a
            // `Ty::Array` receiver it is intercepted by the array-receiver
            // fall-through above and never reaches this match.
            "select!" => {
                self.inline_retain(vec_id, closure_params, closure_body)?;
                Ok(Some(None))
            }
            // `reduce` MIGRATED to a real `.rx` body over `each` (Feature C);
            // for a `Ty::Array` receiver it is intercepted by the
            // array-receiver fall-through above and never reaches this match.
            "all?" => {
                let result = self.inline_all_any(
                    expr,
                    vec_id,
                    closure_params,
                    closure_body,
                    /*all=*/ true,
                )?;
                Ok(Some(Some(result)))
            }
            "any?" => {
                let result = self.inline_all_any(
                    expr,
                    vec_id,
                    closure_params,
                    closure_body,
                    /*all=*/ false,
                )?;
                Ok(Some(Some(result)))
            }
            _ => Ok(None), // Not a recognized closure method.
        }
    }
}
