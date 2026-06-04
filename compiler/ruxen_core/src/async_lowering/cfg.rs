//! Await-delimited control-flow graph for an async fn body.
//!
//! Each [`Segment`] is a maximal run of straight-line statements ending at (at
//! most) one `.await` suspension point. [`Edge`]s connect segments; a loop is a
//! back-edge. ONE `build_poll_body` (lower.rs, Task 4) emits the
//! `self.__state`-indexed poll skeleton for ANY topology, subsuming the five
//! hand-specialized lowering paths (`no-await` / `linear-N` / `while-single` /
//! `while-multi` / `multi-phase-loop`) and their three `recognize_*Shape`
//! recognizers.
//!
//! This module is pure AST→data-structure analysis: no codegen, no
//! `Span`-heavy builders. [`segment_cfg`] (Task 3) produces a `Cfg` for the
//! exact union of shapes the three old recognizers accepted; `lower.rs`
//! consumes it.

// The data model is introduced in Task 2 ahead of its consumers: `segment_cfg`
// (Task 3) populates these fields and `build_poll_body` (Task 4) reads them.
// Until then several fields/methods have no non-test reader. The allow is
// removed implicitly once Tasks 3-4 land their consumers.
#![allow(dead_code)]

use crate::lexer::token::Span;
use crate::parser::ast::{Expr, Statement};

/// A basic block in the async CFG: straight-line work, then an optional
/// suspend. Segments are stored in `Cfg::segments` with `id == index`.
pub struct Segment {
    /// 0-based index == the `self.__state` discriminant value for this segment.
    pub id: usize,
    /// Statements that run on entry to this segment, BEFORE its suspend. None
    /// of them may contain a nested `.await` — those split into earlier
    /// segments.
    pub stmts: Vec<Statement>,
    /// The suspension that ends this segment, if any. `None` only for the
    /// terminal segment (whose tail value is produced by [`Cfg::tail`]).
    pub suspend: Option<Suspend>,
}

/// A `let <binding> = <awaitee>.await` suspension point.
pub struct Suspend {
    /// Binding name — becomes a `self.<binding>` field (every await result
    /// survives the next poll, matching current 2B behaviour).
    pub binding: String,
    /// The awaitee expression (`g(args)` / `Class.method(args)`), pre-rewrite.
    /// `lower.rs` builds the sub-future ctor + the `(&var self.__sub_i).poll(cx)`
    /// match from this, exactly as `describe_await` does today.
    pub awaitee: Expr,
}

/// Where control goes after a segment's suspend resolves `Ready`.
///
/// Exhaustive — every edge topology the four old builders produced is either a
/// straight advance (`Next`) or a conditional loop back-edge (`Loop`). No `_`
/// arm: adding an edge kind must be a compile error in `build_poll_body`.
pub enum Edge {
    /// Advance to segment `to` (the straight-line / linear-N case).
    Next { from: usize, to: usize },
    /// Conditional back-edge: re-evaluate `cond`; if true go to `to` (loop
    /// head), else fall through to `else_to`. Models `while cond` loops as a
    /// back-edge, replacing the recognize_while_single/multi + multi-phase-loop
    /// builders.
    Loop {
        from: usize,
        cond: Expr,
        to: usize,
        else_to: usize,
    },
}

impl Edge {
    /// The segment this edge departs from.
    pub fn from(&self) -> usize {
        match self {
            Edge::Next { from, .. } => *from,
            Edge::Loop { from, .. } => *from,
        }
    }
}

/// An await-delimited control-flow graph for one async fn body.
pub struct Cfg {
    pub segments: Vec<Segment>,
    pub edges: Vec<Edge>,
    /// Statements after the loop / after the last suspend that produce the
    /// fn's return value (the terminal `Poll.Ready(<tail>)`).
    pub tail: Vec<Statement>,
    pub span: Span,
}

impl Cfg {
    /// Cheap structural invariant the lowering depends on. A `Cfg` that passes
    /// `validate` lets `build_poll_body` use `unreachable!` on ill-formed
    /// graphs with a load-bearing claim, instead of a silent `_ =>`.
    ///
    /// Checks:
    ///  * segment ids are dense `0..N` (`segments[i].id == i`),
    ///  * exactly one terminal segment (`suspend: None`),
    ///  * every edge endpoint references an existing segment id,
    ///  * no `Edge::Next` departs FROM the terminal segment (the terminal
    ///    produces `Poll.Ready(tail)` and has no successor).
    pub fn validate(&self) -> Result<(), String> {
        let n = self.segments.len();
        if n == 0 {
            return Err("Cfg has no segments".to_string());
        }

        // Dense ids.
        for (i, s) in self.segments.iter().enumerate() {
            if s.id != i {
                return Err(format!("segment at index {i} has id {} (not dense)", s.id));
            }
        }

        // Exactly one terminal.
        let terminals: Vec<usize> = self
            .segments
            .iter()
            .filter(|s| s.suspend.is_none())
            .map(|s| s.id)
            .collect();
        if terminals.len() != 1 {
            return Err(format!(
                "expected exactly one terminal segment (suspend: None), found {}: {:?}",
                terminals.len(),
                terminals
            ));
        }
        let terminal_id = terminals[0];

        // Edge endpoints in range; no Next out of the terminal.
        for edge in &self.edges {
            match edge {
                Edge::Next { from, to } => {
                    if *from >= n {
                        return Err(format!("Edge::Next.from {from} out of range (n={n})"));
                    }
                    if *to >= n {
                        return Err(format!("Edge::Next.to {to} out of range (n={n})"));
                    }
                    if *from == terminal_id {
                        return Err(format!(
                            "Edge::Next departs from the terminal segment {terminal_id}"
                        ));
                    }
                }
                Edge::Loop {
                    from, to, else_to, ..
                } => {
                    if *from >= n {
                        return Err(format!("Edge::Loop.from {from} out of range (n={n})"));
                    }
                    if *to >= n {
                        return Err(format!("Edge::Loop.to {to} out of range (n={n})"));
                    }
                    if *else_to >= n {
                        return Err(format!("Edge::Loop.else_to {else_to} out of range (n={n})"));
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::ast::ExprKind;

    fn dummy_span() -> Span {
        Span::new(0, 0, 1, 1)
    }

    fn dummy_expr() -> Expr {
        Expr {
            kind: ExprKind::Identifier("x".to_string()),
            span: dummy_span(),
        }
    }

    fn suspend(binding: &str) -> Suspend {
        Suspend {
            binding: binding.to_string(),
            awaitee: dummy_expr(),
        }
    }

    /// Hand-build the shape `let a = f().await; let b = g().await; a + b`:
    /// seg0 suspends on f, seg1 suspends on g, seg2 is the terminal tail.
    fn handbuilt_linear_two_await() -> Cfg {
        Cfg {
            segments: vec![
                Segment {
                    id: 0,
                    stmts: vec![],
                    suspend: Some(suspend("a")),
                },
                Segment {
                    id: 1,
                    stmts: vec![],
                    suspend: Some(suspend("b")),
                },
                Segment {
                    id: 2,
                    stmts: vec![],
                    suspend: None,
                },
            ],
            edges: vec![Edge::Next { from: 0, to: 1 }, Edge::Next { from: 1, to: 2 }],
            tail: vec![],
            span: dummy_span(),
        }
    }

    #[test]
    fn cfg_invariants_hold_for_handbuilt_linear_two_await() {
        let cfg = handbuilt_linear_two_await();
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
        assert!(cfg.segments.last().unwrap().suspend.is_none());
        assert!(cfg.segments.iter().enumerate().all(|(i, s)| s.id == i));
    }

    #[test]
    fn cfg_validate_rejects_dangling_edge() {
        let mut cfg = handbuilt_linear_two_await();
        cfg.edges.push(Edge::Next { from: 0, to: 99 });
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn cfg_validate_rejects_two_terminals() {
        let mut cfg = handbuilt_linear_two_await();
        // Drop seg1's suspend so there are two `suspend: None` segments.
        cfg.segments[1].suspend = None;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn cfg_validate_rejects_next_out_of_terminal() {
        let mut cfg = handbuilt_linear_two_await();
        cfg.edges.push(Edge::Next { from: 2, to: 0 });
        assert!(cfg.validate().is_err());
    }
}
