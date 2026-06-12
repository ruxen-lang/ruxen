//! Operator-precedence model for the formatter, used to decide when a
//! sub-expression must be re-parenthesised so the formatted output
//! re-parses to the SAME tree as the source.
//!
//! # Why this exists (Q34)
//!
//! The parser DISCARDS grouping parentheses — there is no `Paren`/`Grouped`
//! `ExprKind`. `(a + b) * c` and `a + b * c` would both reach the formatter
//! as a `BinaryOp` tree; the first as `(Mul (Add a b) c)`, the second as
//! `(Add a (Mul b c))`. If the formatter re-emitted operands without
//! precedence awareness it would print BOTH as `a + b * c`, silently
//! changing arithmetic (Q34, `docs/dev/gui-stack-v1-issues.md`). The fix is
//! to re-derive the grouping from the tree shape: wrap any child whose
//! precedence is lower than its position requires.
//!
//! # Single source of precedence truth
//!
//! The numeric tiers below are the LEFT binding power of each operator as
//! defined by `parser::expr::infix_binding_power` (assignment 1, `||` 3,
//! `&&` 5, comparison/`~=` 7, range 9, `|` 11, `^` 13, `&` 15, shift 17,
//! `+`/`-` 19, `*`/`/`/`%` 21, cast 23, prefix/unary 25, postfix 27). When
//! that table changes, change these tiers in lock-step. The reparse-identity
//! pin in `tests/syntax_parity.rs` fails if they drift.
//!
//! All Ruxen binary operators are LEFT-associative (verified against the
//! parser: `a - b - c` parses as `((a - b) - c)`), and `as`/range are
//! non-associative. The re-parenthesisation rule is therefore the standard
//! one:
//!   * a LEFT operand needs parens iff its precedence is strictly lower than
//!     the parent's,
//!   * a RIGHT operand needs parens iff its precedence is lower than OR equal
//!     to the parent's (equal-precedence on the right would otherwise
//!     re-associate: `a - (b - c)` must keep its parens).

use crate::parser::ast::{BinOp, Expr, ExprKind};

/// Precedence tier (left binding power) of an expression's top operator, or
/// `None` for an atom / primary that never needs wrapping in operator
/// position (literals, identifiers, calls, indexing, blocks, …). Atoms bind
/// tighter than any infix operator, so they are never parenthesised.
pub fn expr_prec(expr: &Expr) -> Option<u8> {
    match &expr.kind {
        ExprKind::BinaryOp { op, .. } => Some(binop_prec(*op)),
        // Range `..`/`...` — non-associative tier 9.
        ExprKind::Range { .. } => Some(9),
        // `as` cast — tier 23.
        ExprKind::Cast { .. } => Some(23),
        // Prefix unary (`-`/`!`/`*`-deref) and reference operators — tier 25.
        ExprKind::UnaryOp { .. } | ExprKind::Borrow(_) | ExprKind::BorrowMut(_) => Some(25),
        // Assignment / compound-assignment — tier 1 (loosest binding).
        ExprKind::Assign { .. } | ExprKind::CompoundAssign { .. } => Some(1),
        // Everything else is an atom/primary or a postfix chain (call,
        // index, field, method, try, await, closure, control-flow): binds
        // at least as tight as a unary prefix and never needs grouping when
        // it sits as an operand.
        _ => None,
    }
}

/// Left binding power of a binary operator — mirrors
/// `parser::expr::infix_binding_power` (the LEFT element of its `(l, r)`).
pub fn binop_prec(op: BinOp) -> u8 {
    match op {
        BinOp::Or => 3,
        BinOp::And => 5,
        BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::Gt | BinOp::LtEq | BinOp::GtEq => 7,
        // `~=` shares the comparison tier (std.regex spec).
        BinOp::MatchOp => 7,
        BinOp::BitOr => 11,
        BinOp::BitXor => 13,
        BinOp::BitAnd => 15,
        BinOp::Shl | BinOp::Shr => 17,
        BinOp::Add | BinOp::Sub => 19,
        BinOp::Mul | BinOp::Div | BinOp::Mod => 21,
    }
}

/// Side of a binary/range/cast operator an operand sits on. Right operands
/// of a left-associative operator need parens at EQUAL precedence; left
/// operands do not.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// Does `child`, sitting on `side` of a parent operator whose precedence is
/// `parent_prec`, need grouping parentheses to re-parse identically?
pub fn needs_parens(child: &Expr, parent_prec: u8, side: Side) -> bool {
    match expr_prec(child) {
        None => false,
        Some(cp) => match side {
            Side::Left => cp < parent_prec,
            Side::Right => cp <= parent_prec,
        },
    }
}
