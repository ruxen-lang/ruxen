use super::*;

mod all_any;
mod each;
mod each_with_index;
mod entry_or_insert;
mod filter;
mod find;
mod fold;
mod map;
mod option_map;
mod partition;
mod position;
mod result_map;
mod retain;
mod sort_by;
mod unwrap_or_else;

impl<'a> Lowerer<'a> {
    pub(super) fn try_inline_closure_method(
        &mut self,
        expr: &HirExpr,
        object: &HirExpr,
        method_name: &str,
        args: &[HirExpr],
        block_expr: &HirExpr,
    ) -> Result<Option<Option<LocalId>>, String> {
        // Extract closure params and body from the block expression.
        let (closure_params, closure_body) = match &block_expr.kind {
            HirExprKind::Closure { params, body, .. } => (params, body),
            _ => return Ok(None), // Not a closure — can't inline.
        };

        // Handle Option.map { |x| expr } inline: check tag, transform payload.
        if is_option_type(&object.ty) && method_name == "map" {
            return self.inline_option_map(expr, object, closure_params, closure_body);
        }

        // Result.map / Result.map_err — same shape: branch on tag,
        // run the closure on the matching arm's payload, repackage.
        if is_result_type(&object.ty) {
            match method_name {
                "map" => {
                    return self.inline_result_map(
                        expr,
                        object,
                        closure_params,
                        closure_body,
                        /*on_ok=*/ true,
                    );
                }
                "map_err" => {
                    return self.inline_result_map(
                        expr,
                        object,
                        closure_params,
                        closure_body,
                        /*on_ok=*/ false,
                    );
                }
                _ => {}
            }
        }

        // Result.unwrap_or_else { |e| ... } / Option.unwrap_or_else { |e| ... }
        // — branch on tag, return payload on the success arm, evaluate
        // closure with the error payload otherwise.
        if method_name == "unwrap_or_else" {
            if is_result_type(&object.ty) {
                return self.inline_unwrap_or_else(
                    expr,
                    object,
                    closure_params,
                    closure_body,
                    /*ok_tag=*/ 0,
                );
            }
            if is_option_type(&object.ty) {
                return self.inline_unwrap_or_else(
                    expr,
                    object,
                    closure_params,
                    closure_body,
                    /*ok_tag=*/ 1,
                );
            }
        }

        // Determine the Vec source. For Vec/iterator types, peel through
        // method call chains. For user-defined classes with known
        // collection-wrapping methods (where_matching, display_all,
        // into_filtered, each), access the class's first field (items Vec).
        let vec_id = if is_vec_or_iterator_type(&object.ty) {
            let vec_local = self.lower_vec_source(object)?;
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
        let array_receiver = matches!(recv_ty, Ty::Array(_))
            || matches!(recv_ty, Ty::Class { name, .. } if {
                let base = name.split('[').next().unwrap_or(name);
                base == "Array"
            });
        if array_receiver
            && matches!(
                method_name,
                "map" | "select" | "reject" | "all?" | "any?" | "partition"
            )
        {
            return Ok(None);
        }

        match method_name {
            "each" => {
                // for i in 0..vec.len: item = vec[i]; <body>
                self.inline_each(vec_id, closure_params, closure_body)?;
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
            //  * `sort_by { |a, b| ord }`  — comparator-druxen insertion sort.
            "select!" => {
                self.inline_retain(vec_id, closure_params, closure_body)?;
                Ok(Some(None))
            }
            "sort_by" => {
                self.inline_sort_by(vec_id, closure_params, closure_body)?;
                Ok(Some(None))
            }
            // Phase 2 stdlib (#05 batch 2): closure-taking eager
            // terminators on `*Iter` receivers. These inline the same
            // `ruxen_vec_len` + `ruxen_vec_get` per-element loop as
            // `each` / `find`, but they accumulate (`fold`) or
            // short-circuit on a boolean predicate (`all` / `any`).
            "reduce" => {
                let result = self.inline_fold(expr, vec_id, args, closure_params, closure_body)?;
                Ok(Some(Some(result)))
            }
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

    /// Emit an inlined `vec.retain { |item| pred }` — in-place filter.
    /// Read-write cursor walks the backing array; elements where the
    /// closure returns `true` are kept (compacted into the prefix);
    /// elements where it returns `false` are dropped (the slot at
    /// position `read` is overwritten by a future kept element). Final
    /// `len` becomes the count of survivors. The element backing
    /// (e.g. `Vec[String]` slot strings) is NOT freed by this lowering
    /// — v1 documents `retain` as a slot-level forget, the same
    /// contract as `clear` / `truncate` (#03 batch 1).
    pub(super) fn fn_local_ty(&self, local_id: LocalId) -> Ty {
        self.current_fn
            .as_ref()
            .and_then(|f| f.locals.iter().find(|l| l.id == local_id))
            .map(|l| l.ty.clone())
            .unwrap_or(Ty::Int)
    }

    /// Lower the "vec source" from a method call chain, peeling through
    /// iterator adaptors and passthrough method calls to find the underlying
    /// Vec local. E.g., `self.items.iter.filter { ... }` -> the local for
    /// `self.items`.
    pub(super) fn lower_vec_source(&mut self, expr: &HirExpr) -> Result<Option<LocalId>, String> {
        match &expr.kind {
            HirExprKind::MethodCall {
                object,
                method_name,
                block,
                ..
            } => {
                match method_name.as_str() {
                    // (The `iter`/`into_iter`/`to_vec`/`enumerate`
                    // passthrough peel was removed with the orphaned
                    // iterator machinery — Phase B / Milestone 2. Nothing
                    // produces those calls, so the `_ =>` arm below covers
                    // any residual shape by lowering normally.)
                    "select" | "reject" | "where_matching" if block.is_some() => {
                        // A filter in the chain: inline it and return the
                        // filtered vec as the source. This handles chained
                        // `.filter { ... }.to_vec`.
                        // For now, just peel through to the base object.
                        self.lower_vec_source(object)
                    }
                    _ => {
                        // Some other method — lower it normally.
                        self.lower_expr(expr)
                    }
                }
            }
            HirExprKind::FieldAccess {
                object: inner_obj,
                field_name,
                ..
            } => {
                // (`.iter`/`.into_iter`/`.to_vec`/`.enumerate` field-access
                // peel removed with the orphaned iterator machinery —
                // Phase B / Milestone 2.)
                let _ = (inner_obj, field_name);
                self.lower_expr(expr)
            }
            _ => self.lower_expr(expr),
        }
    }
}
