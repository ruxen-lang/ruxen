use crate::hir::nodes::DefId;
use crate::lexer::token::Span;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub enum BindingState {
    Owned,
    Moved { target: Option<DefId>, span: Span },
    MovedIntoCall { callee: String, span: Span },
}

#[derive(Debug, Clone)]
pub struct OwnershipState {
    bindings: HashMap<DefId, BindingState>,
}

impl Default for OwnershipState {
    fn default() -> Self {
        Self::new()
    }
}

impl OwnershipState {
    pub fn new() -> Self {
        Self {
            bindings: HashMap::new(),
        }
    }

    pub fn declare(&mut self, def_id: DefId) {
        self.bindings.insert(def_id, BindingState::Owned);
    }

    pub fn record_move(&mut self, source: DefId, target: Option<DefId>, span: Span) {
        self.bindings
            .insert(source, BindingState::Moved { target, span });
    }

    pub fn record_move_into_call(&mut self, source: DefId, callee: String, span: Span) {
        self.bindings
            .insert(source, BindingState::MovedIntoCall { callee, span });
    }

    pub fn reinitialize(&mut self, def_id: DefId) {
        self.bindings.insert(def_id, BindingState::Owned);
    }

    pub fn is_owned(&self, def_id: DefId) -> bool {
        matches!(self.bindings.get(&def_id), Some(BindingState::Owned))
    }

    pub fn is_moved(&self, def_id: DefId) -> bool {
        matches!(
            self.bindings.get(&def_id),
            Some(BindingState::Moved { .. } | BindingState::MovedIntoCall { .. })
        )
    }

    pub fn snapshot(&self) -> Self {
        self.clone()
    }

    /// Conservative merge: if a binding is moved on ANY branch, it's moved after.
    pub fn merge(branches: Vec<Self>) -> Self {
        if branches.is_empty() {
            return Self::new();
        }
        let mut result = branches[0].clone();
        for branch in &branches[1..] {
            for (def_id, state) in &branch.bindings {
                if !matches!(state, BindingState::Owned) && result.is_owned(*def_id) {
                    result.bindings.insert(*def_id, state.clone());
                }
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::token::Span;

    fn span() -> Span {
        Span::new(0, 1, 1, 1)
    }

    #[test]
    fn new_binding_is_owned() {
        let mut state = OwnershipState::new();
        state.declare(10);
        assert!(state.is_owned(10));
        assert!(!state.is_moved(10));
    }

    #[test]
    fn move_invalidates_source() {
        let mut state = OwnershipState::new();
        state.declare(10);
        state.record_move(10, Some(20), span());
        assert!(state.is_moved(10));
        assert!(!state.is_owned(10));
    }

    #[test]
    fn move_into_call_invalidates() {
        let mut state = OwnershipState::new();
        state.declare(10);
        state.record_move_into_call(10, "consume".into(), span());
        assert!(state.is_moved(10));
    }

    #[test]
    fn snapshot_and_merge_conservative() {
        let mut state = OwnershipState::new();
        state.declare(10);
        state.declare(20);
        let snapshot = state.snapshot();

        // Branch A: move 10
        state.record_move(10, Some(30), span());
        let branch_a = state.clone();

        // Branch B: nothing (restore from snapshot)
        let branch_b = snapshot.clone();

        // Merge
        state = OwnershipState::merge(vec![branch_a, branch_b]);
        assert!(
            state.is_moved(10),
            "moved on any branch → moved after merge"
        );
        assert!(state.is_owned(20), "untouched → still owned");
    }
}
