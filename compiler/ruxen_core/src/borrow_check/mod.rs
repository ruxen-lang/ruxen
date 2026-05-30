pub mod borrows;
pub mod errors;
pub mod lifetimes;
pub mod moves;
pub mod ownership;
pub mod regions;

mod checks;
mod walk;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use crate::hir::nodes::*;
use crate::hir::types::{MoveSemantics, Ty};
use crate::lexer::token::Span;
use crate::resolve::symbols::{ty_is_effectively_copy, DefKind, SymbolTable};

use self::borrows::{BorrowKind, BorrowSet};
use self::errors::{BorrowError, ErrorCode, SpanLabel};
use self::lifetimes::LifetimeChecker;
use self::moves::MoveChecker;
use self::ownership::OwnershipState;
use self::regions::{ScopeKind, ScopeStack};

// ─── Public entry point ────────────────────────────────────────────────

/// Run borrow checking on a typed HIR program.
///
/// Returns a list of all ownership and borrowing violations found.
pub fn borrow_check(program: &HirProgram, symbols: &SymbolTable) -> Vec<BorrowError> {
    let mut checker = BorrowChecker::new(symbols);
    checker.check_program(program);
    checker.errors
}

// ─── BorrowChecker ─────────────────────────────────────────────────────

struct BorrowChecker<'a> {
    symbols: &'a SymbolTable,
    scopes: ScopeStack,
    ownership: OwnershipState,
    borrows: BorrowSet,
    moves: MoveChecker,
    lifetimes: LifetimeChecker,
    /// Tracks whether each DefId is mutable (let mut) or immutable (let).
    mutability: HashMap<DefId, bool>,
    /// Maps reference variables to the place they borrow from.
    /// e.g., `let r = &v` → ref_bindings[r_def_id] = v_def_id
    ref_bindings: HashMap<DefId, DefId>,
    errors: Vec<BorrowError>,
}

impl<'a> BorrowChecker<'a> {
    fn new(symbols: &'a SymbolTable) -> Self {
        Self {
            symbols,
            scopes: ScopeStack::new(),
            ownership: OwnershipState::new(),
            borrows: BorrowSet::new(),
            moves: MoveChecker::new(),
            lifetimes: LifetimeChecker::new(),
            mutability: HashMap::new(),
            ref_bindings: HashMap::new(),
            errors: Vec::new(),
        }
    }

    // ─── Helpers ───────────────────────────────────────────────────

    /// Register a new binding across all sub-analyzers.
    fn register_binding(&mut self, def_id: DefId, ty: &Ty, mutable: bool, span: Span) {
        self.scopes.register_binding(def_id);
        self.ownership.declare(def_id);
        self.moves.declare(def_id, ty.clone(), span);
        self.mutability.insert(def_id, mutable);
    }

    /// Check if a DefId is mutable.
    fn is_mutable(&self, def_id: DefId) -> bool {
        // First check our local mutability map
        if let Some(&m) = self.mutability.get(&def_id) {
            return m;
        }
        // Fall back to the symbol table
        if let Some(def) = self.symbols.get(def_id) {
            match &def.kind {
                DefKind::Variable { mutable, .. } => return *mutable,
                DefKind::Param { .. } => return false,
                DefKind::SelfValue { .. } => return false,
                _ => {}
            }
        }
        false
    }

    /// Get a human-readable name for a DefId.
    fn def_name(&self, def_id: DefId) -> String {
        self.symbols
            .get(def_id)
            .map(|d| d.name.clone())
            .unwrap_or_else(|| format!("_{}", def_id))
    }

    /// Returns `true` if the given value should be copied (not moved) on
    /// assignment or argument passing.  Extends `Ty::is_copy` by consulting
    /// the `derive_traits` list on user-defined structs — a struct with
    /// `derive Copy` behaves like a primitive for move analysis.
    fn ty_is_effectively_copy(&self, ty: &Ty) -> bool {
        ty_is_effectively_copy(ty, self.symbols)
    }
}

fn ty_has_bound(ty: &Ty, bound_name: &str) -> bool {
    match ty {
        Ty::TypeParam { bounds, .. } | Ty::SomeMixin(bounds) | Ty::AnyMixin(bounds) => {
            bounds.iter().any(|bound| bound.name == bound_name)
        }
        _ => false,
    }
}
