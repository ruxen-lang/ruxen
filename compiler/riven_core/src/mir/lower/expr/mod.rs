use super::*;

mod literals;
mod var_ref;
mod binops;
mod unaryops;
mod blocks;
mod control;
mod for_loop;
mod fn_call;
mod method_call;
mod assign;
mod constructors;
mod field_access;
mod index;
mod closure;
mod misc;

impl<'a> Lowerer<'a> {
    pub(super) fn lower_expr(&mut self, expr: &HirExpr) -> Result<Option<LocalId>, String> {
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
            | HirExprKind::Continue => self.lower_control(expr),
            HirExprKind::For { .. } => self.lower_for_loop(expr),

            // Calls
            HirExprKind::FnCall { .. } => self.lower_fn_call(expr),
            HirExprKind::MethodCall { .. } => self.lower_method_call(expr),

            // Assignment
            HirExprKind::Assign { .. } | HirExprKind::CompoundAssign { .. } => self.lower_assign(expr),

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
        }
    }
}
