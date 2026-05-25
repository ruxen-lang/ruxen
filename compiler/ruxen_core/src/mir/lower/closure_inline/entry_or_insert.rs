use super::super::*;

impl<'a> Lowerer<'a> {
    pub(crate) fn inline_entry_or_insert(
        &mut self,
        entry_expr: &HirExpr,
        method_name: &str,
        outer_args: &[HirExpr],
        outer_block: &Option<Box<HirExpr>>,
    ) -> Result<Option<LocalId>, String> {
        let (map_expr, k_expr) = match &entry_expr.kind {
            HirExprKind::MethodCall {
                object,
                args: entry_args,
                method_name: m,
                ..
            } if m == "entry" => {
                let k = entry_args
                    .first()
                    .ok_or_else(|| "Map.entry expects exactly one key argument".to_string())?;
                (object.as_ref(), k)
            }
            _ => unreachable!("inline_entry_or_insert called without entry chain"),
        };

        let map_local_opt = self.lower_expr(map_expr)?;
        let map_local =
            map_local_opt.ok_or_else(|| "Map receiver lowered to no value".to_string())?;

        let k_local_opt = self.lower_expr(k_expr)?;
        let k_local =
            k_local_opt.ok_or_else(|| "Map.entry key arg lowered to no value".to_string())?;

        // contains_key check.
        let has = self.new_temp(Ty::Bool);
        self.emit(MirInst::Call {
            dest: Some(has),
            callee: "ruxen_hash_contains_key".to_string(),
            args: vec![MirValue::Use(map_local), MirValue::Use(k_local)],
        });

        let insert_block = self.new_block();
        let merge_block = self.new_block();
        self.set_terminator(Terminator::Branch {
            cond: MirValue::Use(has),
            then_block: merge_block,
            else_block: insert_block,
        });

        // INSERT block: lower V (or closure body), then call insert.
        self.current_block = insert_block;
        let v_local_opt = match method_name {
            "or_insert" => {
                let v_expr = outer_args
                    .first()
                    .ok_or_else(|| "or_insert expects exactly one value argument".to_string())?;
                self.lower_expr(v_expr)?
            }
            "or_insert_with" => {
                let block_expr = outer_block
                    .as_deref()
                    .ok_or_else(|| "or_insert_with expects a closure block".to_string())?;
                let body = match &block_expr.kind {
                    HirExprKind::Closure { body, .. } => body,
                    _ => {
                        return Err("or_insert_with expects a closure block as its body".to_string())
                    }
                };
                self.lower_expr(body)?
            }
            _ => unreachable!(
                "inline_entry_or_insert called for unknown method `{}`",
                method_name
            ),
        };
        let v_local =
            v_local_opt.ok_or_else(|| format!("`{}` value lowered to no value", method_name))?;

        // Discard the Option[V] return — we don't expose the displaced
        // value because typeck pinned this chain's type to Unit.
        self.emit(MirInst::Call {
            dest: None,
            callee: "ruxen_hash_insert".to_string(),
            args: vec![
                MirValue::Use(map_local),
                MirValue::Use(k_local),
                MirValue::Use(v_local),
            ],
        });
        self.set_terminator(Terminator::Goto(merge_block));

        // Merge: chain's type is Unit, so no result local.
        self.current_block = merge_block;
        Ok(None)
    }
}
