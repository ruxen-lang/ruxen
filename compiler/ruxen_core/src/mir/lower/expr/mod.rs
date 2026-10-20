use super::*;

mod assign;
mod binops;
mod blocks;
mod closure;
mod constructors;
mod control;
mod field_access;
mod fn_call;
mod for_loop;
mod index;
mod literals;
mod method_call;
mod misc;
mod refmut_place;
mod unaryops;
mod var_ref;

impl<'a> Lowerer<'a> {
    pub(super) fn lower_expr(&mut self, expr: &HirExpr) -> Result<Option<LocalId>, String> {
        // Stamp the span of every instruction this expression emits (P0.14
        // DWARF lines, MIR diagnostics). A sub-expression stamps its own and
        // restores this one on return, so an instruction emitted after the
        // operands (a call, a store) carries its own expression's span.
        let outer_span = self.current_span.replace(expr.span.clone());
        let lowered = self.lower_expr_node(expr);
        self.current_span = outer_span;
        lowered
    }

    fn lower_expr_node(&mut self, expr: &HirExpr) -> Result<Option<LocalId>, String> {
        // Only the call `lower_refmut_addr` asked for may yield a `&var T`
        // address; a nested expression always yields values.
        let want_refmut_addr = std::mem::take(&mut self.want_refmut_addr);
        match &expr.kind {
            // Literals
            HirExprKind::IntLiteral(_)
            | HirExprKind::FloatLiteral(_)
            | HirExprKind::BoolLiteral(_)
            | HirExprKind::CharLiteral(_)
            | HirExprKind::StringLiteral(_)
            | HirExprKind::UnitLiteral
            | HirExprKind::NullLiteral => self.lower_literals(expr),

            // Variables
            HirExprKind::VarRef(_) => self.lower_var_ref(expr),

            // Operators
            HirExprKind::BinaryOp { .. } => self.lower_binops(expr),
            HirExprKind::UnaryOp { .. } | HirExprKind::Borrow { .. } => self.lower_unaryops(expr),

            // Blocks
            HirExprKind::Block(_, _) | HirExprKind::UnsafeBlock(_, _) => self.lower_blocks(expr),

            // Control flow
            HirExprKind::If { .. }
            | HirExprKind::While { .. }
            | HirExprKind::Loop { .. }
            | HirExprKind::Return(_)
            | HirExprKind::Break(_)
            | HirExprKind::Next(_)
            | HirExprKind::Continue => self.lower_control(expr),
            HirExprKind::For { .. } => self.lower_for_loop(expr),

            // Calls
            HirExprKind::FnCall { .. } | HirExprKind::MethodCall { .. } => {
                let result = if matches!(expr.kind, HirExprKind::FnCall { .. }) {
                    self.lower_fn_call(expr)?
                } else {
                    self.lower_method_call(expr)?
                };
                if want_refmut_addr {
                    Ok(result)
                } else {
                    Ok(self.load_slot_refmut_result(result))
                }
            }

            // Assignment
            HirExprKind::Assign { .. } | HirExprKind::CompoundAssign { .. } => {
                self.lower_assign(expr)
            }
            HirExprKind::MultiAssign {
                targets,
                values,
                splat_target,
            } => self.lower_multiassign(targets, values, *splat_target),

            // Constructors / aggregates
            HirExprKind::Construct { .. }
            | HirExprKind::EnumVariant { .. }
            | HirExprKind::Tuple(_)
            | HirExprKind::ArrayLiteral(_)
            | HirExprKind::MapLiteral(_) => self.lower_constructors(expr),

            // Field access + indexing
            HirExprKind::FieldAccess { .. } => self.lower_field_access(expr),
            HirExprKind::Index { .. } => self.lower_index(expr),

            // Closures
            HirExprKind::Closure { .. } => self.lower_closure(expr),

            // Delegates handled directly (single-line lowerings already in their own modules)
            HirExprKind::Match { scrutinee, arms } => self.lower_match(expr, scrutinee, arms),
            HirExprKind::Interpolation { parts } => self.lower_interpolation(parts, &expr.ty),

            // Cast / macros / leftovers
            HirExprKind::Cast { .. }
            | HirExprKind::MacroCall { .. }
            | HirExprKind::ArrayFill { .. }
            | HirExprKind::Range { .. }
            | HirExprKind::Error => self.lower_misc(expr),

            // `/pat/flags` regex literal — wired into MIR by Phase 6
            // of the std.regex rollout.
            HirExprKind::RegexLiteral { .. } => self.lower_regex_literal(expr),
        }
    }

    /// Lower a `/pat/flags` regex literal to a
    /// `ruxen_regex_compile_const(pattern_ptr, flags_ptr)` call.
    ///
    /// v1 compiles the pattern once per evaluation (no module-init
    /// hoisting yet — see the spec's risk register R4). Both args
    /// cross the FFI as raw `const char *` (`Ty::RawPtr(Char)`, i.e.
    /// `*Char` — the raw `.rodata` pointer `MirInst::StringLiteral`
    /// produces, which is NEVER owned and NEVER dropped, distinct from
    /// an owned `Ty::String`). The returned handle is typed as
    /// `Ty::Class { name: "Regex" }`, matching the HIR-level typeck
    /// assignment from Phase 4.
    pub(super) fn lower_regex_literal(
        &mut self,
        expr: &HirExpr,
    ) -> Result<Option<LocalId>, String> {
        let (pattern, flags) = match &expr.kind {
            HirExprKind::RegexLiteral { pattern, flags } => (pattern, flags),
            _ => unreachable!("lower_regex_literal: dispatched to wrong helper"),
        };

        let pat_raw = self.new_temp(Ty::RawPtr(Box::new(Ty::Char)));
        self.emit(MirInst::StringLiteral {
            dest: pat_raw,
            value: pattern.clone(),
        });
        let flag_raw = self.new_temp(Ty::RawPtr(Box::new(Ty::Char)));
        self.emit(MirInst::StringLiteral {
            dest: flag_raw,
            value: flags.clone(),
        });
        let dest = self.new_temp(expr.ty.clone());
        self.emit(MirInst::Call {
            dest: Some(dest),
            callee: "ruxen_regex_compile_const".to_string(),
            args: vec![MirValue::Use(pat_raw), MirValue::Use(flag_raw)],
        });
        Ok(Some(dest))
    }
}
