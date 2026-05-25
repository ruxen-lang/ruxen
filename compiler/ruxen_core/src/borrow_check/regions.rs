use crate::hir::nodes::DefId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScopeId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind {
    Function,
    Block,
    Loop,
    IfBranch,
    MatchArm,
    Closure,
}

#[derive(Debug)]
struct Scope {
    _id: ScopeId,
    parent: Option<ScopeId>,
    kind: ScopeKind,
    bindings: Vec<DefId>,
}

#[derive(Debug)]
pub struct ScopeStack {
    scopes: Vec<Scope>,
    stack: Vec<ScopeId>,
    next_id: u32,
}

impl Default for ScopeStack {
    fn default() -> Self {
        Self::new()
    }
}

impl ScopeStack {
    pub fn new() -> Self {
        Self {
            scopes: Vec::new(),
            stack: Vec::new(),
            next_id: 0,
        }
    }

    pub fn push(&mut self, kind: ScopeKind) -> ScopeId {
        let id = ScopeId(self.next_id);
        self.next_id += 1;
        let parent = self.stack.last().copied();
        self.scopes.push(Scope {
            _id: id,
            parent,
            kind,
            bindings: Vec::new(),
        });
        self.stack.push(id);
        id
    }

    pub fn pop(&mut self) -> ScopeId {
        self.stack.pop().expect("cannot pop empty scope stack")
    }

    pub fn current(&self) -> ScopeId {
        *self.stack.last().expect("no current scope")
    }

    pub fn current_kind(&self) -> ScopeKind {
        let id = self.current();
        self.get(id).kind
    }

    pub fn parent_of(&self, id: ScopeId) -> Option<ScopeId> {
        self.get(id).parent
    }

    /// Returns true if `outer` contains `inner` (inner is a descendant of outer).
    pub fn scope_contains(&self, outer: ScopeId, inner: ScopeId) -> bool {
        if outer == inner {
            return true;
        }
        let mut current = inner;
        while let Some(parent) = self.parent_of(current) {
            if parent == outer {
                return true;
            }
            current = parent;
        }
        false
    }

    pub fn register_binding(&mut self, def_id: DefId) {
        let current = self.current();
        self.get_mut(current).bindings.push(def_id);
    }

    pub fn bindings_in(&self, id: ScopeId) -> &[DefId] {
        &self.get(id).bindings
    }

    /// Returns bindings in reverse declaration order (LIFO drop order).
    pub fn drop_order_for_current(&self) -> Vec<DefId> {
        let id = self.current();
        let mut bindings = self.get(id).bindings.clone();
        bindings.reverse();
        bindings
    }

    /// Is the current scope inside a loop?
    pub fn is_in_loop(&self) -> bool {
        for &id in self.stack.iter().rev() {
            if self.get(id).kind == ScopeKind::Loop {
                return true;
            }
        }
        false
    }

    /// Find the nearest enclosing function scope.
    pub fn enclosing_function(&self) -> Option<ScopeId> {
        self.stack
            .iter()
            .rev()
            .find(|&&id| self.get(id).kind == ScopeKind::Function)
            .copied()
    }

    fn get(&self, id: ScopeId) -> &Scope {
        &self.scopes[id.0 as usize]
    }

    fn get_mut(&mut self, id: ScopeId) -> &mut Scope {
        &mut self.scopes[id.0 as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_pop_scope() {
        let mut stack = ScopeStack::new();
        let root = stack.push(ScopeKind::Function);
        assert_eq!(stack.current(), root);
        let inner = stack.push(ScopeKind::Block);
        assert_eq!(stack.current(), inner);
        assert_eq!(stack.parent_of(inner), Some(root));
        stack.pop();
        assert_eq!(stack.current(), root);
    }

    #[test]
    fn scope_contains_checks_ancestry() {
        let mut stack = ScopeStack::new();
        let outer = stack.push(ScopeKind::Function);
        let middle = stack.push(ScopeKind::Block);
        let inner = stack.push(ScopeKind::Block);
        assert!(stack.scope_contains(outer, inner));
        assert!(stack.scope_contains(outer, middle));
        assert!(stack.scope_contains(middle, inner));
        assert!(!stack.scope_contains(inner, outer));
    }

    #[test]
    fn register_binding_in_scope() {
        let mut stack = ScopeStack::new();
        let scope = stack.push(ScopeKind::Function);
        stack.register_binding(42);
        let bindings = stack.bindings_in(scope);
        assert_eq!(bindings, &[42]);
    }

    #[test]
    fn bindings_dropped_in_reverse_order() {
        let mut stack = ScopeStack::new();
        let _scope = stack.push(ScopeKind::Function);
        stack.register_binding(1);
        stack.register_binding(2);
        stack.register_binding(3);
        let drop_order = stack.drop_order_for_current();
        assert_eq!(drop_order, vec![3, 2, 1]);
    }
}
