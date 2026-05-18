use crate::hir::nodes::DefId;
use crate::hir::types::Ty;
use crate::lexer::token::Span;
use std::collections::HashMap;

#[derive(Debug, Clone)]
struct BindingInfo {
    ty: Ty,
    declared_span: Span,
    state: MoveState,
}

#[derive(Debug, Clone)]
enum MoveState {
    Live,
    Moved { span: Span, callee: Option<String> },
}

#[derive(Debug)]
pub struct UseAfterMoveError {
    pub def_id: DefId,
    pub declared_span: Span,
    pub move_span: Span,
    pub use_span: Span,
    pub callee: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MoveChecker {
    bindings: HashMap<DefId, BindingInfo>,
}

impl Default for MoveChecker {
    fn default() -> Self {
        Self::new()
    }
}

impl MoveChecker {
    pub fn new() -> Self {
        Self {
            bindings: HashMap::new(),
        }
    }

    pub fn declare(&mut self, def_id: DefId, ty: Ty, span: Span) {
        self.bindings.insert(
            def_id,
            BindingInfo {
                ty,
                declared_span: span,
                state: MoveState::Live,
            },
        );
    }

    /// Process a value transfer (assignment or function call argument).
    /// If the type is Copy, this is a no-op. If Move, source is invalidated.
    pub fn process_transfer(&mut self, source: DefId, _target: Option<DefId>, ty: &Ty, span: Span) {
        if ty.is_copy() {
            return;
        }
        if let Some(info) = self.bindings.get_mut(&source) {
            info.state = MoveState::Moved { span, callee: None };
        }
    }

    /// Process a move into a function call with a known callee name.
    pub fn process_call_move(&mut self, source: DefId, callee: String, ty: &Ty, span: Span) {
        if ty.is_copy() {
            return;
        }
        if let Some(info) = self.bindings.get_mut(&source) {
            info.state = MoveState::Moved {
                span,
                callee: Some(callee),
            };
        }
    }

    /// Check if a variable can be used at the given span.
    pub fn check_use(&self, def_id: DefId, use_span: Span) -> Result<(), UseAfterMoveError> {
        if let Some(info) = self.bindings.get(&def_id) {
            if let MoveState::Moved { span, callee } = &info.state {
                return Err(UseAfterMoveError {
                    def_id,
                    declared_span: info.declared_span.clone(),
                    move_span: span.clone(),
                    use_span,
                    callee: callee.clone(),
                });
            }
        }
        Ok(())
    }

    /// Re-initialize a variable (e.g., after reassignment to a moved variable).
    pub fn reinitialize(&mut self, def_id: DefId, span: Span) {
        if let Some(info) = self.bindings.get_mut(&def_id) {
            info.state = MoveState::Live;
            info.declared_span = span;
        }
    }

    /// Is this def_id currently live (not moved)?
    pub fn is_live(&self, def_id: DefId) -> bool {
        self.bindings
            .get(&def_id)
            .map(|i| matches!(i.state, MoveState::Live))
            .unwrap_or(true) // Unknown bindings assumed live
    }

    pub fn snapshot(&self) -> Self {
        self.clone()
    }

    pub fn restore(&mut self, snapshot: &Self) {
        self.bindings = snapshot.bindings.clone();
    }

    /// Conservative merge: if moved on ANY branch, moved after.
    pub fn merge(&mut self, branches: Vec<Self>) {
        if branches.is_empty() {
            return;
        }
        self.bindings = branches[0].bindings.clone();
        for branch in &branches[1..] {
            for (def_id, info) in &branch.bindings {
                if let MoveState::Moved { .. } = &info.state {
                    if let Some(existing) = self.bindings.get_mut(def_id) {
                        if matches!(existing.state, MoveState::Live) {
                            existing.state = info.state.clone();
                        }
                    }
                }
            }
        }
    }

    /// Get the type of a tracked binding.
    pub fn binding_ty(&self, def_id: DefId) -> Option<&Ty> {
        self.bindings.get(&def_id).map(|i| &i.ty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hir::types::Ty;
    use crate::lexer::token::Span;

    fn span(line: u32) -> Span {
        Span::new(0, 1, line, 1)
    }

    #[test]
    fn copy_type_does_not_move() {
        let mut checker = MoveChecker::new();
        checker.declare(10, Ty::Int, span(1));
        let result = checker.check_use(10, span(3));
        assert!(result.is_ok());
        // "Move" a copy type — should not actually invalidate
        checker.process_transfer(10, Some(20), &Ty::Int, span(2));
        let result = checker.check_use(10, span(3));
        assert!(
            result.is_ok(),
            "Copy type should still be usable after transfer"
        );
    }

    #[test]
    fn move_type_invalidates_source() {
        let mut checker = MoveChecker::new();
        checker.declare(10, Ty::String, span(1));
        checker.process_transfer(10, Some(20), &Ty::String, span(2));
        let result = checker.check_use(10, span(3));
        assert!(
            result.is_err(),
            "Move type should be invalid after transfer"
        );
    }

    #[test]
    fn use_after_move_returns_error_info() {
        let mut checker = MoveChecker::new();
        checker.declare(10, Ty::String, span(1));
        checker.process_transfer(10, None, &Ty::String, span(5));
        let err = checker.check_use(10, span(8)).unwrap_err();
        assert_eq!(err.move_span.line, 5);
        assert_eq!(err.use_span.line, 8);
    }

    #[test]
    fn branch_merge_conservative() {
        let mut checker = MoveChecker::new();
        checker.declare(10, Ty::String, span(1));
        let snap = checker.snapshot();
        // Branch A: move
        checker.process_transfer(10, None, &Ty::String, span(3));
        let branch_a = checker.clone();
        // Branch B: no move
        checker.restore(&snap);
        let branch_b = checker.clone();
        // Merge: moved on any → moved
        checker.merge(vec![branch_a, branch_b]);
        let result = checker.check_use(10, span(10));
        assert!(result.is_err(), "moved on any branch → invalid after merge");
    }

    #[test]
    fn reinitialize_after_move() {
        let mut checker = MoveChecker::new();
        checker.declare(10, Ty::String, span(1));
        checker.process_transfer(10, None, &Ty::String, span(2));
        assert!(checker.check_use(10, span(3)).is_err());
        checker.reinitialize(10, span(4));
        assert!(checker.check_use(10, span(5)).is_ok());
    }
}
