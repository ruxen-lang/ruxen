use super::*;
use crate::hir::visit::{walk_expr, walk_stmt, Visit};

/// Collect, in first-use order, every def in `outer_defs` that `expr` (a
/// closure body) reads and that is neither one of the closure's own
/// parameters nor bound inside the body.
pub(super) fn collect_captures(
    expr: &HirExpr,
    closure_params: &HashSet<DefId>,
    outer_defs: &HashSet<DefId>,
    out: &mut Vec<DefId>,
    seen: &mut HashSet<DefId>,
) {
    let mut collector = CaptureCollector {
        closure_params: closure_params.clone(),
        outer_defs,
        locally_bound: HashSet::new(),
        out,
        seen,
    };
    collector.visit_expr(expr);
}

/// The scoped walk behind `collect_captures`. `closure_params` grows with a
/// nested closure's parameters for the duration of that closure's body, and
/// `locally_bound` tracks `let` and `for` bindings in the current scope; both
/// are restored on scope exit.
struct CaptureCollector<'a> {
    closure_params: HashSet<DefId>,
    outer_defs: &'a HashSet<DefId>,
    locally_bound: HashSet<DefId>,
    out: &'a mut Vec<DefId>,
    seen: &'a mut HashSet<DefId>,
}

impl Visit for CaptureCollector<'_> {
    fn visit_expr(&mut self, expr: &HirExpr) {
        match &expr.kind {
            HirExprKind::VarRef(def_id) => {
                if !self.closure_params.contains(def_id)
                    && !self.locally_bound.contains(def_id)
                    && self.outer_defs.contains(def_id)
                    && !self.seen.contains(def_id)
                {
                    self.out.push(*def_id);
                    self.seen.insert(*def_id);
                }
            }
            HirExprKind::Block(..) | HirExprKind::UnsafeBlock(..) => {
                let saved_bound = self.locally_bound.clone();
                walk_expr(self, expr);
                self.locally_bound = saved_bound;
            }
            HirExprKind::For {
                iterable,
                body,
                binding,
                tuple_bindings,
                ..
            } => {
                self.visit_expr(iterable);
                let saved_bound = self.locally_bound.clone();
                self.locally_bound.insert(*binding);
                for (d, _) in tuple_bindings {
                    self.locally_bound.insert(*d);
                }
                self.visit_expr(body);
                self.locally_bound = saved_bound;
            }
            HirExprKind::Closure { params, .. } => {
                // A nested closure sees our captured vars too. Merge its
                // parameters into `closure_params` just for the nested walk.
                let saved_params = self.closure_params.clone();
                let saved_bound = self.locally_bound.clone();
                self.closure_params.extend(params.iter().map(|p| p.def_id));
                walk_expr(self, expr);
                self.closure_params = saved_params;
                self.locally_bound = saved_bound;
            }
            // No binding or scope at this node: just recurse. Listed (no `_`)
            // so a new HIR variant that introduces a binding is triaged here.
            HirExprKind::IntLiteral(_)
            | HirExprKind::FloatLiteral(_)
            | HirExprKind::StringLiteral(_)
            | HirExprKind::BoolLiteral(_)
            | HirExprKind::CharLiteral(_)
            | HirExprKind::UnitLiteral
            | HirExprKind::RegexLiteral { .. }
            | HirExprKind::FieldAccess { .. }
            | HirExprKind::MethodCall { .. }
            | HirExprKind::FnCall { .. }
            | HirExprKind::BinaryOp { .. }
            | HirExprKind::UnaryOp { .. }
            | HirExprKind::Borrow { .. }
            | HirExprKind::If { .. }
            | HirExprKind::Match { .. }
            | HirExprKind::Loop { .. }
            | HirExprKind::While { .. }
            | HirExprKind::Assign { .. }
            | HirExprKind::CompoundAssign { .. }
            | HirExprKind::MultiAssign { .. }
            | HirExprKind::Return(_)
            | HirExprKind::Break(_)
            | HirExprKind::Continue
            | HirExprKind::Next(_)
            | HirExprKind::Construct { .. }
            | HirExprKind::EnumVariant { .. }
            | HirExprKind::Tuple(_)
            | HirExprKind::Index { .. }
            | HirExprKind::Cast { .. }
            | HirExprKind::ArrayLiteral(_)
            | HirExprKind::MapLiteral(_)
            | HirExprKind::ArrayFill { .. }
            | HirExprKind::Range { .. }
            | HirExprKind::Interpolation { .. }
            | HirExprKind::MacroCall { .. }
            | HirExprKind::NullLiteral
            | HirExprKind::Error => walk_expr(self, expr),
        }
    }

    fn visit_stmt(&mut self, stmt: &HirStatement) {
        // The initializer is walked BEFORE the binding enters scope, so
        // `let x = x + 1` still captures the outer `x`.
        walk_stmt(self, stmt);
        if let HirStatement::Let { def_id, .. } = stmt {
            self.locally_bound.insert(*def_id);
        }
    }
}

/// Return `true` if the closure body performs any assignment to the given
/// outer-frame `def_id` (used to decide between ByValue and ByRef storage).
pub(super) fn closure_body_mutates(body: &HirExpr, def_id: DefId) -> bool {
    let mut finder = AssignFinder {
        def_id,
        found: false,
    };
    finder.visit_expr(body);
    finder.found
}

/// Finds an `=` or compound assignment whose target is `def_id` itself.
struct AssignFinder {
    def_id: DefId,
    found: bool,
}

impl Visit for AssignFinder {
    fn visit_expr(&mut self, expr: &HirExpr) {
        if self.found {
            return;
        }
        if let HirExprKind::Assign { target, .. } | HirExprKind::CompoundAssign { target, .. } =
            &expr.kind
        {
            if matches!(target.kind, HirExprKind::VarRef(d) if d == self.def_id) {
                self.found = true;
                return;
            }
        }
        walk_expr(self, expr);
    }
}
