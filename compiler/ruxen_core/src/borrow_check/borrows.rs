use crate::borrow_check::regions::ScopeId;
use crate::hir::nodes::DefId;
use crate::lexer::token::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BorrowId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorrowKind {
    Shared,
    Mutable,
}

#[derive(Debug, Clone)]
pub struct BorrowInfo {
    pub id: BorrowId,
    pub kind: BorrowKind,
    pub borrowed_place: DefId,
    pub borrower: DefId,
    pub created_span: Span,
    pub scope: ScopeId,
    pub last_use: Option<Span>,
    alive: bool,
}

/// Conflict information returned when a borrow check fails.
#[derive(Debug)]
pub struct BorrowConflict {
    pub existing: BorrowInfo,
    pub new_kind: BorrowKind,
}

#[derive(Debug, Clone)]
pub struct BorrowSet {
    borrows: Vec<BorrowInfo>,
    next_id: u32,
}

impl Default for BorrowSet {
    fn default() -> Self {
        Self::new()
    }
}

impl BorrowSet {
    pub fn new() -> Self {
        Self {
            borrows: Vec::new(),
            next_id: 0,
        }
    }

    pub fn create(
        &mut self,
        kind: BorrowKind,
        borrowed_place: DefId,
        borrower: DefId,
        span: Span,
        scope: ScopeId,
    ) -> BorrowId {
        let id = BorrowId(self.next_id);
        self.next_id += 1;
        self.borrows.push(BorrowInfo {
            id,
            kind,
            borrowed_place,
            borrower,
            created_span: span,
            scope,
            last_use: None,
            alive: true,
        });
        id
    }

    pub fn record_use(&mut self, id: BorrowId, span: Span) {
        if let Some(info) = self.borrows.iter_mut().find(|b| b.id == id) {
            info.last_use = Some(span);
        }
    }

    /// Check if a new borrow of `place` with `kind` would conflict.
    pub fn check_new_borrow(&self, kind: BorrowKind, place: DefId) -> Result<(), BorrowConflict> {
        for borrow in self.active_borrows_of(place) {
            match (kind, borrow.kind) {
                (BorrowKind::Shared, BorrowKind::Shared) => {} // OK
                _ => {
                    return Err(BorrowConflict {
                        existing: borrow.clone(),
                        new_kind: kind,
                    });
                }
            }
        }
        Ok(())
    }

    /// Check if moving `place` would conflict with active borrows.
    pub fn check_move(&self, place: DefId) -> Result<(), BorrowConflict> {
        if let Some(borrow) = self.active_borrows_of(place).first() {
            Err(BorrowConflict {
                existing: (*borrow).clone(),
                new_kind: BorrowKind::Mutable,
            })
        } else {
            Ok(())
        }
    }

    /// Check if mutating `place` through its owner would conflict.
    pub fn check_mutation(&self, place: DefId) -> Result<(), BorrowConflict> {
        self.check_move(place)
    }

    pub fn active_borrows_of(&self, place: DefId) -> Vec<&BorrowInfo> {
        self.borrows
            .iter()
            .filter(|b| b.alive && b.borrowed_place == place)
            .collect()
    }

    /// All active borrows where `borrower` is the given DefId.
    pub fn borrows_held_by(&self, borrower: DefId) -> Vec<&BorrowInfo> {
        self.borrows
            .iter()
            .filter(|b| b.alive && b.borrower == borrower)
            .collect()
    }

    /// Returns the current number of borrows (used as a checkpoint for temporary borrows).
    pub fn checkpoint(&self) -> usize {
        self.borrows.len()
    }

    /// Kill all borrows created after a checkpoint (temporary borrows from fn args).
    pub fn kill_after_checkpoint(&mut self, checkpoint: usize) {
        for borrow in &mut self.borrows[checkpoint..] {
            borrow.alive = false;
        }
    }

    /// Kill all borrows created in a given scope.
    pub fn kill_scope(&mut self, scope: ScopeId) {
        for borrow in &mut self.borrows {
            if borrow.alive && borrow.scope == scope {
                borrow.alive = false;
            }
        }
    }

    /// NLL: expire borrows whose last_use is before `current_point`.
    /// Comparison is by source offset (Span.start).
    pub fn expire_before(&mut self, current_point: Span) {
        for borrow in &mut self.borrows {
            if borrow.alive {
                if let Some(ref last_use) = borrow.last_use {
                    if last_use.start < current_point.start {
                        borrow.alive = false;
                    }
                }
            }
        }
    }

    /// Snapshot for branching analysis.
    pub fn snapshot(&self) -> Self {
        self.clone()
    }

    /// Restore from snapshot (used when walking independent branches).
    pub fn restore(&mut self, snapshot: &BorrowSet) {
        self.borrows = snapshot.borrows.clone();
        // Keep next_id at its current value to avoid ID collisions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::token::Span;

    fn span(line: u32, col: u32) -> Span {
        Span::new(0, 1, line, col)
    }

    #[test]
    fn create_shared_borrow() {
        let mut set = BorrowSet::new();
        let scope = ScopeId(0);
        let id = set.create(BorrowKind::Shared, 10, 20, span(1, 1), scope);
        let active = set.active_borrows_of(10);
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, id);
        assert_eq!(active[0].kind, BorrowKind::Shared);
    }

    #[test]
    fn multiple_shared_borrows_allowed() {
        let mut set = BorrowSet::new();
        let scope = ScopeId(0);
        set.create(BorrowKind::Shared, 10, 20, span(1, 1), scope);
        set.create(BorrowKind::Shared, 10, 21, span(2, 1), scope);
        assert!(set.check_new_borrow(BorrowKind::Shared, 10).is_ok());
    }

    #[test]
    fn mutable_borrow_conflicts_with_shared() {
        let mut set = BorrowSet::new();
        let scope = ScopeId(0);
        set.create(BorrowKind::Shared, 10, 20, span(1, 1), scope);
        let result = set.check_new_borrow(BorrowKind::Mutable, 10);
        assert!(result.is_err());
    }

    #[test]
    fn shared_borrow_conflicts_with_mutable() {
        let mut set = BorrowSet::new();
        let scope = ScopeId(0);
        set.create(BorrowKind::Mutable, 10, 20, span(1, 1), scope);
        let result = set.check_new_borrow(BorrowKind::Shared, 10);
        assert!(result.is_err());
    }

    #[test]
    fn double_mutable_borrow_conflicts() {
        let mut set = BorrowSet::new();
        let scope = ScopeId(0);
        set.create(BorrowKind::Mutable, 10, 20, span(1, 1), scope);
        let result = set.check_new_borrow(BorrowKind::Mutable, 10);
        assert!(result.is_err());
    }

    #[test]
    fn move_conflicts_with_active_borrow() {
        let mut set = BorrowSet::new();
        let scope = ScopeId(0);
        set.create(BorrowKind::Shared, 10, 20, span(1, 1), scope);
        let result = set.check_move(10);
        assert!(result.is_err());
    }

    #[test]
    fn kill_scope_removes_borrows() {
        let mut set = BorrowSet::new();
        let scope = ScopeId(0);
        set.create(BorrowKind::Shared, 10, 20, span(1, 1), scope);
        set.kill_scope(scope);
        assert!(set.active_borrows_of(10).is_empty());
    }

    #[test]
    fn nll_expire_dead_borrows() {
        let mut set = BorrowSet::new();
        let scope = ScopeId(0);
        let id = set.create(BorrowKind::Shared, 10, 20, span(1, 1), scope);
        set.record_use(id, span(3, 1)); // last use at line 3
                                        // At line 5, the borrow should be dead
        set.expire_before(Span::new(50, 51, 5, 1));
        assert!(set.active_borrows_of(10).is_empty());
    }

    #[test]
    fn nll_borrow_alive_before_last_use() {
        let mut set = BorrowSet::new();
        let scope = ScopeId(0);
        let id = set.create(BorrowKind::Shared, 10, 20, Span::new(0, 1, 1, 1), scope);
        set.record_use(id, Span::new(50, 55, 5, 1)); // last use at offset 50
                                                     // At offset 30 (before last use), borrow should still be alive
        set.expire_before(Span::new(30, 31, 3, 1));
        assert_eq!(set.active_borrows_of(10).len(), 1);
    }
}
