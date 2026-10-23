pub mod errors;
pub mod moves;
pub mod ownership;

mod checks;
mod walk;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};

use crate::hir::nodes::*;
use crate::hir::types::{MoveSemantics, Ty};
use crate::lexer::token::Span;
use crate::parser::ast::{BinOp, ClosureKind};
use crate::resolve::symbols::{ty_is_effectively_copy, DefKind, SymbolTable};

use self::errors::{BorrowError, ErrorCode, SpanLabel};
use self::moves::MoveChecker;
use self::ownership::OwnershipState;

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
    ownership: OwnershipState,
    moves: MoveChecker,
    /// Tracks whether each DefId is mutable (let mut) or immutable (let).
    mutability: HashMap<DefId, bool>,
    /// The variables bound to a borrow of another variable (`r = &v`): a
    /// closure capturing one captures a borrow of a stack local (E1013).
    ref_bindings: HashMap<DefId, DefId>,
    /// The parameters of the function being checked typed `Fn[...]` (blocks,
    /// incl. `__block`). A block may only be called, tested against `nil` or
    /// forwarded to another `Fn[...]` parameter; any other use stores it
    /// (E1018).
    block_params: HashSet<DefId>,
    /// The parameters of the function being checked typed `&var T` where a
    /// `&var T` is passed as the handle word (`!Ty::refmut_is_slot_address`),
    /// with their declared type. Assigning to one is E1020: the callee holds
    /// a copy of the caller's handle, so the rebinding never reaches the
    /// caller.
    handle_refmut_params: HashMap<DefId, Ty>,
    /// Set by a parent right before it checks a direct `VarRef` child that
    /// uses a block in an allowed position; the `VarRef` consumes it.
    block_use_allowed: bool,
    /// Inside a closure body, its non-Copy captures. The body only borrows
    /// them, whatever the closure kind: the enclosing scope owns a block's
    /// captures, the closure object owns a proc's or lambda's and frees them
    /// on drop. [`Self::record_move`] rejects a move of one (E1019).
    capture_borrows: HashSet<DefId>,
    /// One frame per loop being walked (innermost last): the move state at
    /// each `break` / `next` / `continue` inside it. Those leave their branch
    /// but not the function, so the code after the loop merges them in
    /// (`exit_loop`). A closure body opens its own frame: `next` there
    /// leaves the closure, not an enclosing loop.
    loop_exits: Vec<Vec<(OwnershipState, MoveChecker)>>,
    errors: Vec<BorrowError>,
}

impl<'a> BorrowChecker<'a> {
    fn new(symbols: &'a SymbolTable) -> Self {
        Self {
            symbols,
            ownership: OwnershipState::new(),
            moves: MoveChecker::new(),
            mutability: HashMap::new(),
            ref_bindings: HashMap::new(),
            block_params: HashSet::new(),
            handle_refmut_params: HashMap::new(),
            block_use_allowed: false,
            capture_borrows: HashSet::new(),
            loop_exits: Vec::new(),
            errors: Vec::new(),
        }
    }

    // ─── Helpers ───────────────────────────────────────────────────

    /// Register a new binding across all sub-analyzers.
    fn register_binding(&mut self, def_id: DefId, mutable: bool, span: Span) {
        self.ownership.declare(def_id);
        self.moves.declare(def_id, span);
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

    /// Get a human-readable name for a DefId (its display name: the block
    /// slot `__block` reads `block`).
    fn def_name(&self, def_id: DefId) -> String {
        self.symbols
            .display_name(def_id)
            .map(str::to_string)
            .unwrap_or_else(|| format!("_{}", def_id))
    }

    /// The single point every move of a variable goes through: `def_id`,
    /// of type `ty`, is used by value at `span` and moves `into`. A Copy
    /// value moves nothing. A capture of the closure body being checked is
    /// only borrowed (`capture_borrows`), so moving it is E1019 and records
    /// no move. Returns whether a move was recorded.
    fn record_move(&mut self, def_id: DefId, ty: &Ty, into: MoveInto<'_>, span: &Span) -> bool {
        if self.ty_is_effectively_copy(ty) {
            return false;
        }
        if self.capture_borrows.contains(&def_id) {
            self.report_capture_move(def_id, span, matches!(into, MoveInto::Closure));
            return false;
        }
        match into {
            MoveInto::Binding(target) => {
                self.moves
                    .process_transfer(def_id, target, ty, span.clone());
                self.ownership.record_move(def_id, target, span.clone());
            }
            MoveInto::Call(callee) => {
                self.moves
                    .process_call_move(def_id, callee.to_string(), ty, span.clone());
                self.ownership
                    .record_move_into_call(def_id, callee.to_string(), span.clone());
            }
            MoveInto::Closure => {
                self.moves
                    .process_call_move(def_id, "closure".to_string(), ty, span.clone());
                self.ownership
                    .record_move_into_call(def_id, "closure".to_string(), span.clone());
            }
        }
        true
    }

    /// Record the move of the variable `expr` yields when `expr` sits in a
    /// by-value position (a binding, an assignment, a by-value argument or
    /// receiver, a literal element or field, a returned or block value, a
    /// pattern scrutinee). The value comes from `expr`'s leaves
    /// ([`value_leaves`]: a block's tail, an `if`'s branches, a `match`'s
    /// arms, a cast's operand); a leaf that is not a variable yields a
    /// temporary and moves nothing.
    fn consume(&mut self, expr: &HirExpr, into: MoveInto<'_>) {
        for leaf in value_leaves(expr) {
            if let HirExprKind::VarRef(def_id) = &leaf.kind {
                self.record_move(*def_id, &leaf.ty, into, &leaf.span);
            }
        }
    }

    /// Returns `true` if the given value should be copied (not moved) on
    /// assignment or argument passing.  Extends `Ty::is_copy` by consulting
    /// the symbol table, so a structurally Copy user struct behaves like a
    /// primitive for move analysis.
    fn ty_is_effectively_copy(&self, ty: &Ty) -> bool {
        ty_is_effectively_copy(ty, self.symbols)
    }
}

/// Where a moved value goes, for the use-after-move note (E1001).
#[derive(Clone, Copy)]
enum MoveInto<'n> {
    /// Into a binding (`Some` when the destination is a variable).
    Binding(Option<DefId>),
    /// Into the call of the named function or method.
    Call(&'n str),
    /// Into an owning closure that captures it.
    Closure,
}

/// The expressions whose value `expr` yields: `expr` itself, or for a
/// block, `if`, `match`/`case` and cast, the values of its tail, branches,
/// arms or operand.
fn value_leaves(expr: &HirExpr) -> Vec<&HirExpr> {
    fn collect<'e>(expr: &'e HirExpr, out: &mut Vec<&'e HirExpr>) {
        match &expr.kind {
            HirExprKind::Block(_, Some(tail)) | HirExprKind::UnsafeBlock(_, Some(tail)) => {
                collect(tail, out)
            }
            HirExprKind::If {
                then_branch,
                else_branch,
                ..
            } => {
                collect(then_branch, out);
                if let Some(else_branch) = else_branch {
                    collect(else_branch, out);
                }
            }
            HirExprKind::Match { arms, .. } => {
                for arm in arms {
                    collect(&arm.body, out);
                }
            }
            HirExprKind::Cast { expr: inner, .. } => collect(inner, out),
            _ => out.push(expr),
        }
    }
    let mut out = Vec::new();
    collect(expr, &mut out);
    out
}

/// Whether assigning the variable `def_id` writes through it rather than
/// rebinding it: a `&var` parameter or binding holds the address of a place
/// (`r = &var u.name; r = v` writes `u.name`).
fn writes_through(symbols: &SymbolTable, def_id: DefId) -> bool {
    symbols.def_ty(def_id).is_some_and(|ty| ty.is_mut_ref())
}

/// Whether a closure owns `capture` (a proc, a lambda, a `move` closure, a
/// by-move capture) rather than borrowing it (a block): the one rule
/// `check_closure` and the Send check read.
fn closure_owns(capture: &Capture, is_move: bool, kind: ClosureKind) -> bool {
    capture.by_move || is_move || kind != ClosureKind::Block
}

fn ty_has_bound(ty: &Ty, bound_name: &str) -> bool {
    match ty {
        Ty::TypeParam { bounds, .. } | Ty::SomeMixin(bounds) | Ty::AnyMixin(bounds) => {
            bounds.iter().any(|bound| bound.name == bound_name)
        }
        _ => false,
    }
}
