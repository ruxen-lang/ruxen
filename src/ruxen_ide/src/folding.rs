//! LSP `textDocument/foldingRange` for Ruxen.
//!
//! Spec: `docs/requirements/tier3_01_lsp.md` §5.9.
//!
//! Three sources of folds:
//!
//! 1. **HIR walk** — every block-like node (def/method body, class /
//!    struct / enum / mixin / module body, `if` chain, `match`,
//!    `loop` / `while` / `for`, and bare `do … end` blocks) emits a
//!    `Region` range that runs from the opening keyword's line to the
//!    line of the closing `end`.
//! 2. **Top-of-file `use` run** — the first contiguous block of `use`
//!    declarations collapses into a single `Imports` range.
//! 3. **Comment runs** — any run of three or more consecutive
//!    `#`-comment lines collapses into a `Comment` range. Comments are
//!    not preserved into HIR, so we read `result.source` directly.
//!
//! Folds where `start_line == end_line` are filtered — nothing to
//! collapse on a single-line construct.

use std::collections::HashSet;

use lsp_types::{FoldingRange, FoldingRangeKind};
use ruxen_core::hir::nodes::{HirExpr, HirExprKind, HirFuncDef, HirImplBlock, HirItem, HirProgram};
use ruxen_core::hir::visit::{walk_expr, walk_func, walk_item, Visit};
use ruxen_core::lexer::token::Span;

use crate::analysis::AnalysisResult;
use crate::line_index::LineIndex;

/// Collect every foldable range for the given analysed document.
pub fn folding_ranges(result: &AnalysisResult) -> Vec<FoldingRange> {
    let mut ranges = Vec::new();

    if let Some(program) = result.program.as_ref() {
        let mut walker = HirWalker {
            line_index: &result.line_index,
            ranges: &mut ranges,
            bodies: HashSet::new(),
        };
        walker.walk_program(program);
    }

    collect_use_block(&result.source, &result.line_index, &mut ranges);
    collect_comment_blocks(&result.source, &result.line_index, &mut ranges);

    ranges
}

// ─── HIR walker ─────────────────────────────────────────────────────

struct HirWalker<'a> {
    line_index: &'a LineIndex,
    ranges: &'a mut Vec<FoldingRange>,
    /// Body blocks (of a function, branch, arm or loop) whose lines the
    /// parent's own region already covers, so they get no second fold.
    bodies: HashSet<*const HirExpr>,
}

impl<'a> HirWalker<'a> {
    fn push_region(&mut self, span: &Span) {
        if let Some(range) = region_from_span(span, self.line_index) {
            self.ranges.push(range);
        }
    }

    fn walk_program(&mut self, program: &HirProgram) {
        for item in &program.items {
            self.visit_item(item);
        }
    }

    fn mark_body(&mut self, body: &HirExpr) {
        self.bodies.insert(body as *const HirExpr);
    }

    fn is_body(&self, expr: &HirExpr) -> bool {
        self.bodies.contains(&(expr as *const HirExpr))
    }

    fn push_impl_block_regions(&mut self, blocks: &[HirImplBlock]) {
        for b in blocks {
            self.push_region(&b.span);
        }
    }
}

impl Visit for HirWalker<'_> {
    fn visit_item(&mut self, item: &HirItem) {
        match item {
            HirItem::Module(m) => self.push_region(&m.span),
            HirItem::Class(c) => {
                self.push_region(&c.span);
                self.push_impl_block_regions(&c.impl_blocks);
            }
            HirItem::Struct(s) => {
                self.push_region(&s.span);
                self.push_impl_block_regions(&s.impl_blocks);
            }
            HirItem::Enum(e) => {
                self.push_region(&e.span);
                self.push_impl_block_regions(&e.impl_blocks);
            }
            HirItem::Mixin(m) => self.push_region(&m.span),
            HirItem::Impl(i) => self.push_region(&i.span),
            HirItem::Function(_)
            | HirItem::TypeAlias(_)
            | HirItem::Newtype(_)
            | HirItem::Const(_) => {}
        }
        walk_item(self, item);
    }

    fn visit_func(&mut self, f: &HirFuncDef) {
        // The function span covers `def … end`; its body block spans the
        // same lines, so it gets no fold of its own.
        self.push_region(&f.span);
        self.mark_body(&f.body);
        walk_func(self, f);
    }

    fn visit_expr(&mut self, expr: &HirExpr) {
        match &expr.kind {
            // Bare `do … end` (or any block expr) — fold it, unless it is
            // the body of a construct that already folds.
            HirExprKind::Block(..) => {
                if !self.is_body(expr) {
                    self.push_region(&expr.span);
                }
            }
            HirExprKind::UnsafeBlock(..) => self.push_region(&expr.span),
            HirExprKind::If {
                then_branch,
                else_branch,
                ..
            } => {
                self.push_region(&expr.span);
                self.mark_body(then_branch);
                // An `elsif` (a nested `If`) folds as its own region; a
                // pure `else` block is covered by this `If`'s region.
                if let Some(eb) = else_branch {
                    self.mark_body(eb);
                }
            }
            HirExprKind::Match { arms, .. } => {
                self.push_region(&expr.span);
                for arm in arms {
                    self.mark_body(&arm.body);
                }
            }
            HirExprKind::Loop { body }
            | HirExprKind::While { body, .. }
            | HirExprKind::For { body, .. } => {
                self.push_region(&expr.span);
                self.mark_body(body);
            }
            // Closure bodies can have nested blocks; the closure itself is
            // usually inline so we don't fold it, but the walk descends.
            _ => {}
        }
        walk_expr(self, expr);
    }
}

// ─── Span → FoldingRange ────────────────────────────────────────────

fn region_from_span(span: &Span, line_index: &LineIndex) -> Option<FoldingRange> {
    let start_pos = line_index.position_of(span.start);
    let end_pos = line_index.position_of(span.end);
    if end_pos.line <= start_pos.line {
        return None;
    }
    Some(FoldingRange {
        start_line: start_pos.line,
        start_character: None,
        end_line: end_pos.line,
        end_character: None,
        kind: Some(FoldingRangeKind::Region),
        collapsed_text: None,
    })
}

// ─── Top-of-file `use` run ──────────────────────────────────────────

/// Fold the first contiguous run of `use` statements at the top of
/// the file (preceded only by blank lines or comments) into one
/// Imports range. Subsequent `use` blocks deeper in the file are not
/// folded — the spec only calls out the top-of-file case.
fn collect_use_block(source: &str, line_index: &LineIndex, out: &mut Vec<FoldingRange>) {
    let mut first_use: Option<u32> = None;
    let mut last_use: Option<u32> = None;

    for (i, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("use ") || trimmed == "use" {
            let li = i as u32;
            if first_use.is_none() {
                first_use = Some(li);
            }
            last_use = Some(li);
            continue;
        }
        // Allow a run interrupted only by blank lines? No — spec says
        // "consecutive". A blank line ends the run if we've started.
        if first_use.is_some() {
            break;
        }
        // Before the first `use`, allow blank lines and comments to
        // skip over module-header docstrings.
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // Hit a non-comment, non-blank, non-use line before any `use`
        // — there is no top-of-file import block.
        break;
    }

    if let (Some(start), Some(end)) = (first_use, last_use) {
        if end > start {
            out.push(FoldingRange {
                start_line: start,
                start_character: None,
                end_line: end,
                end_character: None,
                kind: Some(FoldingRangeKind::Imports),
                collapsed_text: None,
            });
        }
    }
    let _ = line_index; // silence unused if the helper is later swapped to byte spans
}

// ─── Comment runs ───────────────────────────────────────────────────

/// Fold any run of three-or-more consecutive `#`-comment lines into
/// a Comment range. Trailing-of-line comments (`x = 1  # note`) are
/// ignored — only lines whose first non-whitespace character is `#`.
fn collect_comment_blocks(source: &str, line_index: &LineIndex, out: &mut Vec<FoldingRange>) {
    let mut run_start: Option<u32> = None;
    let mut run_end: u32 = 0;

    for (i, line) in source.lines().enumerate() {
        let li = i as u32;
        let is_comment = line.trim_start().starts_with('#');
        if is_comment {
            if run_start.is_none() {
                run_start = Some(li);
            }
            run_end = li;
        } else if let Some(start) = run_start.take() {
            push_comment_run(start, run_end, out);
        }
    }
    if let Some(start) = run_start {
        push_comment_run(start, run_end, out);
    }
    let _ = line_index;
}

fn push_comment_run(start: u32, end: u32, out: &mut Vec<FoldingRange>) {
    // "≥ 3 lines" — start..=end inclusive.
    if end + 1 >= start + 3 {
        out.push(FoldingRange {
            start_line: start,
            start_character: None,
            end_line: end,
            end_character: None,
            kind: Some(FoldingRangeKind::Comment),
            collapsed_text: None,
        });
    }
}
