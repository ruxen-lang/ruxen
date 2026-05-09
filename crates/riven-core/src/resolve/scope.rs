//! Scope management for name resolution.
//!
//! Scopes form a tree: each scope has an optional parent, and name lookup
//! walks up the tree until a binding is found or the root is reached.

use std::collections::HashMap;

use crate::hir::nodes::DefId;

/// Unique identifier for a scope.
pub type ScopeId = u32;

/// The kind of scope — determines what names are available and what
/// control flow constructs are valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind {
    /// Top-level module scope
    Module,
    /// Inside a class body
    Class,
    /// Inside a trait body
    Trait,
    /// Inside an impl block
    Impl,
    /// Inside a function body
    Function,
    /// A generic block `do...end` or `{ }`
    Block,
    /// Inside a loop (enables `break` and `continue`)
    Loop,
    /// Inside a match expression
    Match,
    /// Inside a closure
    Closure,
}

/// A single lexical scope.
#[derive(Debug)]
pub struct Scope {
    pub id: ScopeId,
    pub parent: Option<ScopeId>,
    pub kind: ScopeKind,
    /// Name → DefId bindings in this scope.
    bindings: HashMap<String, DefId>,
    /// Type name → DefId bindings (for classes, structs, enums, traits, type aliases).
    type_bindings: HashMap<String, DefId>,
}

impl Scope {
    fn new(id: ScopeId, parent: Option<ScopeId>, kind: ScopeKind) -> Self {
        Self {
            id,
            parent,
            kind,
            bindings: HashMap::new(),
            type_bindings: HashMap::new(),
        }
    }

    /// Insert a value binding into this scope.
    /// Returns the previous DefId if the name was already bound (shadowing).
    pub fn insert(&mut self, name: String, def_id: DefId) -> Option<DefId> {
        self.bindings.insert(name, def_id)
    }

    /// Insert a type binding into this scope.
    pub fn insert_type(&mut self, name: String, def_id: DefId) -> Option<DefId> {
        self.type_bindings.insert(name, def_id)
    }

    /// Look up a value name in this scope only (not parents).
    pub fn lookup_local(&self, name: &str) -> Option<DefId> {
        self.bindings.get(name).copied()
    }

    /// Look up a type name in this scope only.
    pub fn lookup_type_local(&self, name: &str) -> Option<DefId> {
        self.type_bindings.get(name).copied()
    }
}

/// The scope stack manages all scopes during name resolution.
#[derive(Debug)]
pub struct ScopeStack {
    scopes: Vec<Scope>,
    /// The current (innermost) scope.
    current: ScopeId,
}

impl ScopeStack {
    pub fn new() -> Self {
        // Start with a global module scope
        let global = Scope::new(0, None, ScopeKind::Module);
        Self {
            scopes: vec![global],
            current: 0,
        }
    }

    /// Push a new child scope and make it current.
    pub fn push(&mut self, kind: ScopeKind) -> ScopeId {
        let id = self.scopes.len() as ScopeId;
        let scope = Scope::new(id, Some(self.current), kind);
        self.scopes.push(scope);
        self.current = id;
        id
    }

    /// Pop the current scope, returning to the parent.
    pub fn pop(&mut self) {
        if let Some(parent) = self.scopes[self.current as usize].parent {
            self.current = parent;
        }
    }

    /// Get the current scope ID.
    pub fn current_id(&self) -> ScopeId {
        self.current
    }

    /// Get the current scope kind.
    pub fn current_kind(&self) -> ScopeKind {
        self.scopes[self.current as usize].kind
    }

    /// Insert a value binding into the current scope.
    pub fn insert(&mut self, name: String, def_id: DefId) -> Option<DefId> {
        self.scopes[self.current as usize].insert(name, def_id)
    }

    /// Insert a type binding into the current scope.
    pub fn insert_type(&mut self, name: String, def_id: DefId) -> Option<DefId> {
        self.scopes[self.current as usize].insert_type(name, def_id)
    }

    /// Look up a value name, walking up the scope chain.
    pub fn lookup(&self, name: &str) -> Option<DefId> {
        self.lookup_with_scope(name).map(|(def_id, _)| def_id)
    }

    /// Look up a value name, returning both the DefId and the scope that owns it.
    pub fn lookup_with_scope(&self, name: &str) -> Option<(DefId, ScopeId)> {
        let mut scope_id = Some(self.current);
        while let Some(id) = scope_id {
            let scope = &self.scopes[id as usize];
            if let Some(def_id) = scope.lookup_local(name) {
                return Some((def_id, id));
            }
            scope_id = scope.parent;
        }
        None
    }

    /// Look up a type name, walking up the scope chain.
    pub fn lookup_type(&self, name: &str) -> Option<DefId> {
        let mut scope_id = Some(self.current);
        while let Some(id) = scope_id {
            let scope = &self.scopes[id as usize];
            if let Some(def_id) = scope.lookup_type_local(name) {
                return Some(def_id);
            }
            scope_id = scope.parent;
        }
        None
    }

    /// Check if we're currently inside a loop (for break/continue validation).
    pub fn in_loop(&self) -> bool {
        let mut scope_id = Some(self.current);
        while let Some(id) = scope_id {
            let scope = &self.scopes[id as usize];
            if scope.kind == ScopeKind::Loop {
                return true;
            }
            // Stop at function boundary — loops don't cross functions
            if scope.kind == ScopeKind::Function || scope.kind == ScopeKind::Closure {
                return false;
            }
            scope_id = scope.parent;
        }
        false
    }

    /// Check if we're inside a function (for return validation).
    pub fn in_function(&self) -> bool {
        let mut scope_id = Some(self.current);
        while let Some(id) = scope_id {
            let scope = &self.scopes[id as usize];
            if scope.kind == ScopeKind::Function || scope.kind == ScopeKind::Closure {
                return true;
            }
            scope_id = scope.parent;
        }
        false
    }

    /// Find the nearest enclosing class/impl scope's DefId for `self` resolution.
    pub fn enclosing_type_scope(&self) -> Option<ScopeKind> {
        let mut scope_id = Some(self.current);
        while let Some(id) = scope_id {
            let scope = &self.scopes[id as usize];
            if matches!(
                scope.kind,
                ScopeKind::Class | ScopeKind::Impl | ScopeKind::Trait
            ) {
                return Some(scope.kind);
            }
            scope_id = scope.parent;
        }
        None
    }

    /// Get a reference to a scope by ID.
    pub fn get_scope(&self, id: ScopeId) -> &Scope {
        &self.scopes[id as usize]
    }

    /// Returns true when `scope_id` is the same scope as `ancestor_id` or is
    /// nested somewhere underneath it.
    pub fn is_within_scope(&self, scope_id: ScopeId, ancestor_id: ScopeId) -> bool {
        let mut current = Some(scope_id);
        while let Some(id) = current {
            if id == ancestor_id {
                return true;
            }
            current = self.scopes[id as usize].parent;
        }
        false
    }
}

impl Default for ScopeStack {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_starts_with_module_scope() {
        let stack = ScopeStack::new();
        assert_eq!(stack.current_id(), 0);
        assert_eq!(stack.current_kind(), ScopeKind::Module);
    }

    #[test]
    fn default_matches_new() {
        let s = ScopeStack::default();
        assert_eq!(s.current_id(), 0);
        assert_eq!(s.current_kind(), ScopeKind::Module);
    }

    #[test]
    fn lookup_finds_binding_in_current_scope() {
        let mut stack = ScopeStack::new();
        stack.insert("foo".to_string(), 42);
        assert_eq!(stack.lookup("foo"), Some(42));
    }

    #[test]
    fn lookup_returns_none_for_missing_name() {
        let stack = ScopeStack::new();
        assert_eq!(stack.lookup("nope"), None);
    }

    #[test]
    fn lookup_walks_up_parent_scopes() {
        let mut stack = ScopeStack::new();
        stack.insert("outer".to_string(), 10);
        stack.push(ScopeKind::Block);
        // `outer` is not in this scope directly, but must be found via parent
        assert_eq!(stack.lookup("outer"), Some(10));
    }

    #[test]
    fn shadowing_inner_scope_hides_outer_binding() {
        let mut stack = ScopeStack::new();
        stack.insert("x".to_string(), 1);
        stack.push(ScopeKind::Block);
        stack.insert("x".to_string(), 2);
        assert_eq!(stack.lookup("x"), Some(2));
        stack.pop();
        assert_eq!(stack.lookup("x"), Some(1));
    }

    #[test]
    fn pop_returns_to_parent_and_keeps_outer_bindings() {
        let mut stack = ScopeStack::new();
        stack.insert("global".to_string(), 100);
        let inner = stack.push(ScopeKind::Function);
        stack.insert("local".to_string(), 200);
        assert_eq!(stack.current_id(), inner);
        stack.pop();
        assert_eq!(stack.current_id(), 0);
        assert_eq!(stack.lookup("global"), Some(100));
        assert_eq!(stack.lookup("local"), None);
    }

    #[test]
    fn pop_at_module_root_is_a_noop() {
        let mut stack = ScopeStack::new();
        // Root scope has no parent; popping must not panic and must remain at 0.
        stack.pop();
        assert_eq!(stack.current_id(), 0);
        stack.pop();
        assert_eq!(stack.current_id(), 0);
    }

    #[test]
    fn names_are_case_sensitive() {
        let mut stack = ScopeStack::new();
        stack.insert("Foo".to_string(), 1);
        stack.insert("foo".to_string(), 2);
        stack.insert("FOO".to_string(), 3);
        assert_eq!(stack.lookup("Foo"), Some(1));
        assert_eq!(stack.lookup("foo"), Some(2));
        assert_eq!(stack.lookup("FOO"), Some(3));
        assert_eq!(stack.lookup("fOo"), None);
    }

    #[test]
    fn insert_same_name_returns_previous_defid() {
        let mut stack = ScopeStack::new();
        assert_eq!(stack.insert("x".to_string(), 1), None);
        // Overwriting in the *same* scope returns the previous DefId.
        assert_eq!(stack.insert("x".to_string(), 2), Some(1));
        assert_eq!(stack.lookup("x"), Some(2));
    }

    #[test]
    fn value_and_type_bindings_are_independent() {
        let mut stack = ScopeStack::new();
        stack.insert("Foo".to_string(), 1);
        stack.insert_type("Foo".to_string(), 2);
        assert_eq!(stack.lookup("Foo"), Some(1));
        assert_eq!(stack.lookup_type("Foo"), Some(2));
    }

    #[test]
    fn lookup_type_walks_up_parent_scopes() {
        let mut stack = ScopeStack::new();
        stack.insert_type("Vector".to_string(), 7);
        stack.push(ScopeKind::Function);
        stack.push(ScopeKind::Block);
        assert_eq!(stack.lookup_type("Vector"), Some(7));
    }

    #[test]
    fn push_increments_scope_ids_and_parents_correctly() {
        let mut stack = ScopeStack::new();
        let a = stack.push(ScopeKind::Function);
        let b = stack.push(ScopeKind::Block);
        assert_eq!(a, 1);
        assert_eq!(b, 2);
        assert_eq!(stack.current_id(), b);
        assert_eq!(stack.get_scope(b).parent, Some(a));
        assert_eq!(stack.get_scope(a).parent, Some(0));
        assert_eq!(stack.get_scope(0).parent, None);
    }

    #[test]
    fn in_loop_true_inside_loop_scope() {
        let mut stack = ScopeStack::new();
        assert!(!stack.in_loop());
        stack.push(ScopeKind::Function);
        stack.push(ScopeKind::Loop);
        assert!(stack.in_loop());
        stack.push(ScopeKind::Block);
        // Still inside the loop
        assert!(stack.in_loop());
    }

    #[test]
    fn in_loop_false_when_function_boundary_is_crossed() {
        let mut stack = ScopeStack::new();
        stack.push(ScopeKind::Loop);
        stack.push(ScopeKind::Function); // crosses function boundary
        stack.push(ScopeKind::Block);
        // The loop is outside the function — break/continue must NOT escape.
        assert!(!stack.in_loop());
    }

    #[test]
    fn in_loop_false_at_module_root() {
        let stack = ScopeStack::new();
        assert!(!stack.in_loop());
    }

    #[test]
    fn in_function_detects_function_and_closure() {
        let mut stack = ScopeStack::new();
        assert!(!stack.in_function());
        stack.push(ScopeKind::Function);
        assert!(stack.in_function());
        stack.pop();
        stack.push(ScopeKind::Closure);
        assert!(stack.in_function());
    }

    #[test]
    fn enclosing_type_scope_finds_nearest_class_impl_or_trait() {
        let mut stack = ScopeStack::new();
        assert_eq!(stack.enclosing_type_scope(), None);
        stack.push(ScopeKind::Class);
        stack.push(ScopeKind::Function);
        assert_eq!(stack.enclosing_type_scope(), Some(ScopeKind::Class));

        let mut stack2 = ScopeStack::new();
        stack2.push(ScopeKind::Trait);
        assert_eq!(stack2.enclosing_type_scope(), Some(ScopeKind::Trait));

        let mut stack3 = ScopeStack::new();
        stack3.push(ScopeKind::Impl);
        stack3.push(ScopeKind::Function);
        stack3.push(ScopeKind::Block);
        assert_eq!(stack3.enclosing_type_scope(), Some(ScopeKind::Impl));
    }

    #[test]
    fn lookup_local_does_not_walk_parents() {
        let mut stack = ScopeStack::new();
        stack.insert("outer".to_string(), 1);
        stack.push(ScopeKind::Block);
        let current = stack.current_id();
        let scope = stack.get_scope(current);
        // `outer` is in the module scope, not the current block.
        assert_eq!(scope.lookup_local("outer"), None);
    }

    #[test]
    fn lookup_type_local_does_not_walk_parents() {
        let mut stack = ScopeStack::new();
        stack.insert_type("T".to_string(), 1);
        stack.push(ScopeKind::Function);
        let scope = stack.get_scope(stack.current_id());
        assert_eq!(scope.lookup_type_local("T"), None);
    }

    #[test]
    fn scope_kind_equality() {
        assert_eq!(ScopeKind::Module, ScopeKind::Module);
        assert_ne!(ScopeKind::Module, ScopeKind::Class);
        assert_ne!(ScopeKind::Function, ScopeKind::Closure);
        assert_ne!(ScopeKind::Loop, ScopeKind::Block);
    }

    #[test]
    fn lookup_prefers_innermost_binding_through_multiple_levels() {
        let mut stack = ScopeStack::new();
        stack.insert("x".to_string(), 1);
        stack.push(ScopeKind::Function);
        stack.insert("x".to_string(), 2);
        stack.push(ScopeKind::Block);
        stack.insert("x".to_string(), 3);
        assert_eq!(stack.lookup("x"), Some(3));
        stack.pop();
        assert_eq!(stack.lookup("x"), Some(2));
        stack.pop();
        assert_eq!(stack.lookup("x"), Some(1));
    }
}
