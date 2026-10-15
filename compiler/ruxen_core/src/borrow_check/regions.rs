#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScopeId(pub u32);

/// The stack of lexical scopes the borrow checker is inside. Borrows are
/// tagged with the scope that created them and killed when it pops.
#[derive(Debug)]
pub struct ScopeStack {
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
            stack: Vec::new(),
            next_id: 0,
        }
    }

    pub fn push(&mut self) -> ScopeId {
        let id = ScopeId(self.next_id);
        self.next_id += 1;
        self.stack.push(id);
        id
    }

    pub fn pop(&mut self) -> ScopeId {
        self.stack.pop().expect("cannot pop empty scope stack")
    }

    pub fn current(&self) -> ScopeId {
        *self.stack.last().expect("no current scope")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_pop_scope() {
        let mut stack = ScopeStack::new();
        let root = stack.push();
        assert_eq!(stack.current(), root);
        let inner = stack.push();
        assert_eq!(stack.current(), inner);
        stack.pop();
        assert_eq!(stack.current(), root);
    }
}
