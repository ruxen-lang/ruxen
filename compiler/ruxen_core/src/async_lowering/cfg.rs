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
use crate::parser::ast::{Block, Expr, ExprKind, Pattern, Statement};

use super::{block_contains_await, expr_contains_await};

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

/// Analyse an async fn `body` into a [`Cfg`], or `None` if the body uses a
/// shape outside the supported subset.
///
/// This is the single analysis that subsumes the three old recognizers:
///   * `segment_body` — straight-line `pre | [await-let]* | tail` (incl. the
///     degenerate no-await body, which the 2A path handled),
///   * `recognize_while_single_await` — one top-level `while` with one `.await`
///     in its body,
///   * `recognize_while_multi_await` — the same with N≥2 `.await`s.
///
/// The single-vs-multi-await distinction vanishes: a loop body with N awaits is
/// N segment-suspends and one back-edge. The accepted set is the EXACT union of
/// what the three recognizers accepted — every rejection below is ported from a
/// named source so the E1115 negatives (which fire for the rejected shapes) are
/// unchanged. Phase 3 unifies the implementation; it does not widen the
/// allowlist.
///
/// Sources folded in:
///   * loop detection + single-await rejections ← `recognize_while_single_await`,
///   * N-await loop-body segmentation ← `recognize_while_multi_await`,
///   * straight-line segmentation ← `segment_body`.
pub fn segment_cfg(body: &Block) -> Option<Cfg> {
    // 1. No `.await` anywhere → degenerate single terminal segment whose tail
    //    is the whole body. (The old 2A `lower_one_async_fn` path.)
    if !block_contains_await(body) {
        let cfg = Cfg {
            segments: vec![Segment {
                id: 0,
                stmts: Vec::new(),
                suspend: None,
            }],
            edges: Vec::new(),
            tail: body.statements.clone(),
            span: body.span.clone(),
        };
        return cfg.validate().ok().map(|()| cfg);
    }

    // 2. A single top-level awaiting `while` loop → loop Cfg with a back-edge.
    if let Some(cfg) = try_loop_cfg(body) {
        return Some(cfg);
    }

    // 3. Straight-line `pre | [await-let]* | tail`.
    try_linear_cfg(body)
}

/// Port of `recognize_while_single_await` + `recognize_while_multi_await`,
/// collapsed: detect exactly one top-level `Statement::Expression(While)` whose
/// body contains `.await`, reject any other top-level await, segment the loop
/// body into N≥1 suspends, and wire one `Edge::Loop` back-edge.
fn try_loop_cfg(body: &Block) -> Option<Cfg> {
    // Find the single awaiting top-level `while`; reject any other top-level
    // statement that contains `.await` (← recognize_while_*: the "any other
    // stmt must not contain await" guard).
    let mut while_idx: Option<usize> = None;
    for (i, stmt) in body.statements.iter().enumerate() {
        if let Statement::Expression(e) = stmt {
            if let ExprKind::While(w) = &e.kind {
                if block_contains_await(&w.body) {
                    if while_idx.is_some() {
                        return None; // more than one awaiting loop
                    }
                    while_idx = Some(i);
                    continue;
                }
            }
        }
        match stmt {
            Statement::Let(lb) => {
                if let Some(v) = &lb.value {
                    if expr_contains_await(v) {
                        return None;
                    }
                }
            }
            Statement::Expression(e) => {
                if expr_contains_await(e) {
                    return None;
                }
            }
        }
    }
    let widx = while_idx?;
    let pre_loop_stmts: Vec<Statement> = body.statements[..widx].to_vec();
    let post_loop_stmts: Vec<Statement> = body.statements[widx + 1..].to_vec();

    let (loop_cond, w_body) = match &body.statements[widx] {
        Statement::Expression(e) => match &e.kind {
            ExprKind::While(w) => ((*w.condition).clone(), w.body.clone()),
            _ => return None,
        },
        _ => return None,
    };
    // ← recognize_while_*: await in the loop condition is rejected.
    if expr_contains_await(&loop_cond) {
        return None;
    }

    // Segment the loop body into phases: each phase is (pre_stmts, await-let).
    // ← recognize_while_multi_await's current_pre/phases/post_last_await loop,
    //   generalized to N≥1 (no `phases.len() < 2` reject).
    let mut phase_pre: Vec<Vec<Statement>> = Vec::new();
    let mut phase_binding: Vec<String> = Vec::new();
    let mut phase_awaitee: Vec<Expr> = Vec::new();
    let mut current_pre: Vec<Statement> = Vec::new();
    let mut post_last_await_stmts: Vec<Statement> = Vec::new();
    let mut last_was_await = false;

    for stmt in &w_body.statements {
        let mut is_await_let = false;
        if let Statement::Let(lb) = stmt {
            if let Some(v) = &lb.value {
                if let ExprKind::Await(inner) = &v.kind {
                    // ← recognize_while_*: await-let pattern must be a plain
                    //   Identifier.
                    let name = match &lb.pattern {
                        Pattern::Identifier { name, .. } => name.clone(),
                        _ => return None,
                    };
                    is_await_let = true;
                    phase_pre.push(std::mem::take(&mut current_pre));
                    phase_binding.push(name);
                    phase_awaitee.push((**inner).clone());
                    last_was_await = true;
                } else if expr_contains_await(v) {
                    return None;
                }
            }
        } else if let Statement::Expression(e) = stmt {
            if expr_contains_await(e) {
                return None;
            }
        }
        if !is_await_let {
            current_pre.push(stmt.clone());
            last_was_await = false;
        }
    }
    if !last_was_await {
        post_last_await_stmts = current_pre;
    }
    // Must have at least one await suspend in the loop body.
    let n = phase_binding.len();
    if n == 0 {
        return None;
    }

    // Assemble segments:
    //   seg0      : pre_loop_stmts + phase0.pre  → suspend on await0
    //   seg i     : phase_i.pre                  → suspend on await_i   (1<=i<N)
    //   seg N     : post_last_await_stmts        → terminal (loop tail) with
    //               the back-edge re-entering seg0 (loop head) when cond holds.
    //
    // The terminal segment's `suspend` is None; the post-loop statements become
    // `Cfg.tail`, and the back-edge's `else_to` points at this terminal segment
    // (cond false → fall through to the post-loop tail).
    let mut segments: Vec<Segment> = Vec::with_capacity(n + 1);
    for i in 0..n {
        let mut stmts = if i == 0 {
            pre_loop_stmts.clone()
        } else {
            Vec::new()
        };
        stmts.extend(phase_pre[i].iter().cloned());
        segments.push(Segment {
            id: i,
            stmts,
            suspend: Some(Suspend {
                binding: phase_binding[i].clone(),
                awaitee: phase_awaitee[i].clone(),
            }),
        });
    }
    // Terminal segment: the post-last-await stmts (which run on the final
    // phase's Ready before looping back), then the loop terminates and the
    // tail produces the return value.
    segments.push(Segment {
        id: n,
        stmts: post_last_await_stmts,
        suspend: None,
    });

    // Straight-line Next edges chain the in-loop suspends, then one Loop
    // back-edge from the last suspend's successor (the terminal) re-enters the
    // loop head (seg0) when the cond holds, else falls through to the terminal.
    let mut edges: Vec<Edge> = Vec::new();
    for i in 0..n.saturating_sub(1) {
        edges.push(Edge::Next { from: i, to: i + 1 });
    }
    // Back-edge from the last in-loop suspend (seg n-1) to the loop head.
    edges.push(Edge::Loop {
        from: n - 1,
        cond: loop_cond,
        to: 0,
        else_to: n,
    });

    let cfg = Cfg {
        segments,
        edges,
        tail: post_loop_stmts,
        span: body.span.clone(),
    };
    cfg.validate().ok().map(|()| cfg)
}

/// Port of `segment_body`: straight-line `pre_await | [await-let]* | tail`.
/// Rejects the same shapes (pre-await stmt with nested await, non-Identifier
/// await-let pattern, bare `expr.await` statement, await nested in a non-direct
/// expression).
fn try_linear_cfg(body: &Block) -> Option<Cfg> {
    let mut pre_await: Vec<Statement> = Vec::new();
    let mut suspends: Vec<(String, Expr)> = Vec::new();
    let mut tail: Vec<Statement> = Vec::new();
    let mut seen_first_await = false;
    let mut in_tail = false;

    for stmt in &body.statements {
        if in_tail {
            // Post-last-await: forbid further awaits (unsupported v1).
            if let Statement::Let(lb) = stmt {
                if let Some(v) = &lb.value {
                    if expr_contains_await(v) {
                        return None;
                    }
                }
            }
            if let Statement::Expression(e) = stmt {
                if expr_contains_await(e) {
                    return None;
                }
            }
            tail.push(stmt.clone());
            continue;
        }

        match stmt {
            Statement::Let(lb) => {
                let value = lb.value.as_ref()?;
                if let ExprKind::Await(inner) = &value.kind {
                    let name = match &lb.pattern {
                        Pattern::Identifier { name, .. } => name.clone(),
                        _ => return None,
                    };
                    suspends.push((name, (**inner).clone()));
                    seen_first_await = true;
                } else if expr_contains_await(value) {
                    return None;
                } else if !seen_first_await {
                    pre_await.push(stmt.clone());
                } else {
                    in_tail = true;
                    tail.push(stmt.clone());
                }
            }
            Statement::Expression(e) => {
                if expr_contains_await(e) {
                    return None;
                }
                if !seen_first_await {
                    pre_await.push(stmt.clone());
                } else {
                    in_tail = true;
                    tail.push(stmt.clone());
                }
            }
        }
    }

    // A body with no straight-line await-let here means the only awaits were in
    // a shape the loop path rejected (or nested) — not a linear shape.
    if suspends.is_empty() {
        return None;
    }

    // If the body ended with an await-let and no tail stmt, the bound value's
    // identifier is the implicit tail expression — match `segment_body`'s
    // `self.<binding>` synthesis (handled in lowering via the binding field),
    // here we leave `tail` empty and let `build_poll_body` emit the final
    // binding read. To keep the Cfg self-describing, synthesise the
    // `self.<binding>` read as the tail when empty.
    if tail.is_empty() {
        if let Some((last_binding, last_awaitee)) = suspends.last() {
            let span = last_awaitee.span.clone();
            tail.push(Statement::Expression(Expr {
                kind: ExprKind::FieldAccess {
                    object: Box::new(Expr {
                        kind: ExprKind::SelfRef,
                        span: span.clone(),
                    }),
                    field: last_binding.clone(),
                },
                span,
            }));
        }
    }

    // Assemble segments: seg0 carries pre_await + suspend0; seg i carries
    // suspend_i; the final terminal segment carries the tail.
    let n = suspends.len();
    let mut segments: Vec<Segment> = Vec::with_capacity(n + 1);
    for (i, (binding, awaitee)) in suspends.into_iter().enumerate() {
        let stmts = if i == 0 {
            std::mem::take(&mut pre_await)
        } else {
            Vec::new()
        };
        segments.push(Segment {
            id: i,
            stmts,
            suspend: Some(Suspend { binding, awaitee }),
        });
    }
    segments.push(Segment {
        id: n,
        stmts: Vec::new(),
        suspend: None,
    });

    let edges: Vec<Edge> = (0..n).map(|i| Edge::Next { from: i, to: i + 1 }).collect();

    let cfg = Cfg {
        segments,
        edges,
        tail,
        span: body.span.clone(),
    };
    cfg.validate().ok().map(|()| cfg)
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

    /// Parse `src` and return the body of its FIRST top-level function.
    fn parse_fn_body(src: &str) -> Block {
        use crate::parser::ast::TopLevelItem;
        let mut lx = crate::lexer::Lexer::new(src);
        let toks = lx.tokenize().expect("lex");
        let mut p = crate::parser::Parser::new(toks);
        let prog = p.parse().expect("parse");
        for item in prog.items {
            if let TopLevelItem::Function(f) = item {
                return f.body;
            }
        }
        panic!("no top-level function in source");
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

    // ─── segment_cfg shape tests (one per old recognizer shape) ─────

    #[test]
    fn segment_cfg_no_await_is_single_terminal_segment() {
        let body = parse_fn_body("def f\n  1 + 2\nend");
        let cfg = segment_cfg(&body).expect("no-await body is a valid (degenerate) Cfg");
        assert_eq!(cfg.segments.len(), 1);
        assert!(cfg.segments[0].suspend.is_none());
        assert!(cfg.edges.is_empty());
        // The whole body becomes the tail.
        assert_eq!(cfg.tail.len(), 1);
    }

    #[test]
    fn segment_cfg_linear_two_await_has_three_segments_two_next_edges() {
        let body =
            parse_fn_body("async def f\n  let a = g().await\n  let b = h().await\n  a + b\nend");
        let cfg = segment_cfg(&body).unwrap();
        assert_eq!(cfg.segments.len(), 3); // seg0 .await g, seg1 .await h, seg2 tail
        assert!(matches!(
            cfg.edges.as_slice(),
            [Edge::Next { .. }, Edge::Next { .. }]
        ));
        assert_eq!(cfg.segments[0].suspend.as_ref().unwrap().binding, "a");
        assert_eq!(cfg.segments[1].suspend.as_ref().unwrap().binding, "b");
        assert!(cfg.segments[2].suspend.is_none());
    }

    #[test]
    fn segment_cfg_linear_one_await_has_two_segments_one_next_edge() {
        let body = parse_fn_body("async def f\n  let a = g().await\n  a\nend");
        let cfg = segment_cfg(&body).unwrap();
        assert_eq!(cfg.segments.len(), 2);
        assert!(matches!(cfg.edges.as_slice(), [Edge::Next { .. }]));
    }

    #[test]
    fn segment_cfg_while_single_await_has_back_edge() {
        let body =
            parse_fn_body("async def f\n  while keep()\n    let x = step().await\n  end\n  0\nend");
        let cfg = segment_cfg(&body).unwrap();
        assert!(cfg.edges.iter().any(|e| matches!(e, Edge::Loop { .. })));
        // one in-loop suspend + a terminal segment
        assert_eq!(
            cfg.segments.iter().filter(|s| s.suspend.is_some()).count(),
            1
        );
    }

    #[test]
    fn segment_cfg_while_multi_await_has_n_segments_one_back_edge() {
        let body = parse_fn_body(
            "async def f\n  while keep()\n    let a = r().await\n    let b = w().await\n  end\n  0\nend",
        );
        let cfg = segment_cfg(&body).unwrap();
        assert_eq!(
            cfg.edges
                .iter()
                .filter(|e| matches!(e, Edge::Loop { .. }))
                .count(),
            1
        );
        // two suspends inside the loop body
        assert_eq!(
            cfg.segments.iter().filter(|s| s.suspend.is_some()).count(),
            2
        );
    }

    #[test]
    fn segment_cfg_rejects_await_in_loop_condition() {
        let body =
            parse_fn_body("async def f\n  while c().await\n    let x = s().await\n  end\n  0\nend");
        // same rejection as recognize_while_single_await
        assert!(segment_cfg(&body).is_none());
    }

    #[test]
    fn segment_cfg_rejects_await_nested_in_non_let_expr() {
        // bare `g().await` expression statement (not let-bound) — segment_body
        // rejects this; it isn't a loop either.
        let body = parse_fn_body("async def f\n  g().await\n  0\nend");
        assert!(segment_cfg(&body).is_none());
    }

    #[test]
    fn segment_cfg_rejects_two_top_level_awaiting_loops() {
        let body = parse_fn_body(
            "async def f\n  while a()\n    let x = s().await\n  end\n  while b()\n    let y = t().await\n  end\n  0\nend",
        );
        assert!(segment_cfg(&body).is_none());
    }

    /// The accepted set must be the EXACT union of the three old recognizers.
    /// This pins it: for each fixture, `segment_cfg(..).is_some()` equals the
    /// old ladder's acceptance — where the ladder accepts an awaiting body iff
    /// `recognize_while_multi_await` OR `recognize_while_single_await` OR
    /// `super::segment_body` returns `Some` (and any no-await body is always
    /// accepted by the 2A path). The recognizers are still present (Task 5
    /// deletes them) so this cross-check is exact.
    #[test]
    fn segment_cfg_acceptance_matches_old_ladder_union() {
        let cases = [
            // (src, comment)
            "async def f\n  1 + 2\nend",                                            // no-await
            "async def f\n  let a = g().await\n  a\nend",                            // linear-1
            "async def f\n  let a = g().await\n  let b = h().await\n  a + b\nend",   // linear-2
            "async def f\n  let p = setup()\n  let a = g().await\n  a\nend",         // pre-await + linear
            "async def f\n  while keep()\n    let x = s().await\n  end\n  0\nend",   // while-single
            "async def f\n  while keep()\n    let a = r().await\n    let b = w().await\n  end\n  0\nend", // while-multi
            // Rejections:
            "async def f\n  while c().await\n    let x = s().await\n  end\n  0\nend", // await in cond
            "async def f\n  g().await\n  0\nend",                                    // bare await stmt
            "async def f\n  let (a, b) = g().await\n  a\nend",                       // non-ident await pattern
            "async def f\n  while a()\n    let x = s().await\n  end\n  while b()\n    let y = t().await\n  end\n  0\nend", // two awaiting loops
            "async def f\n  let a = if cond then g().await else h().await end\n  a\nend", // await nested in non-let expr
        ];
        for src in cases {
            let body = parse_fn_body(src);
            let old_accepts = if !block_contains_await(&body) {
                true
            } else {
                super::super::recognize_while_multi_await(&body).is_some()
                    || super::super::recognize_while_single_await(&body).is_some()
                    || super::super::segment_body(&body).is_some()
            };
            let new_accepts = segment_cfg(&body).is_some();
            assert_eq!(
                new_accepts, old_accepts,
                "acceptance mismatch for src:\n{src}\n(old={old_accepts}, new={new_accepts})"
            );
        }
    }
}
