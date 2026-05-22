//! LSP `textDocument/foldingRange` for Riven.
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

use lsp_types::{FoldingRange, FoldingRangeKind};
use riven_core::hir::nodes::{
    HirClassDef, HirEnumDef, HirExpr, HirExprKind, HirFuncDef, HirImplBlock, HirImplItem, HirItem,
    HirMixinDef, HirMixinItem, HirModule, HirProgram, HirStatement, HirStructDef,
};
use riven_core::lexer::token::Span;

use crate::analysis::AnalysisResult;
use crate::line_index::LineIndex;

/// Collect every foldable range for the given analysed document.
pub fn folding_ranges(result: &AnalysisResult) -> Vec<FoldingRange> {
    let mut ranges = Vec::new();

    if let Some(program) = result.program.as_ref() {
        let mut walker = HirWalker {
            line_index: &result.line_index,
            ranges: &mut ranges,
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
}

impl<'a> HirWalker<'a> {
    fn push_region(&mut self, span: &Span) {
        if let Some(range) = region_from_span(span, self.line_index) {
            self.ranges.push(range);
        }
    }

    fn walk_program(&mut self, program: &HirProgram) {
        for item in &program.items {
            self.walk_item(item);
        }
    }

    fn walk_item(&mut self, item: &HirItem) {
        match item {
            HirItem::Module(m) => self.walk_module(m),
            HirItem::Class(c) => self.walk_class(c),
            HirItem::Struct(s) => self.walk_struct(s),
            HirItem::Enum(e) => self.walk_enum(e),
            HirItem::Mixin(m) => self.walk_mixin(m),
            HirItem::Impl(i) => self.walk_impl(i),
            HirItem::Function(f) => self.walk_func(f),
            HirItem::TypeAlias(_) | HirItem::Newtype(_) | HirItem::Const(_) => {}
        }
    }

    fn walk_module(&mut self, m: &HirModule) {
        self.push_region(&m.span);
        for it in &m.items {
            self.walk_item(it);
        }
    }

    fn walk_class(&mut self, c: &HirClassDef) {
        self.push_region(&c.span);
        for f in &c.methods {
            self.walk_func(f);
        }
        for ib in &c.impl_blocks {
            self.walk_impl(ib);
        }
    }

    fn walk_struct(&mut self, s: &HirStructDef) {
        self.push_region(&s.span);
        for f in &s.methods {
            self.walk_func(f);
        }
        for ib in &s.impl_blocks {
            self.walk_impl(ib);
        }
    }

    fn walk_enum(&mut self, e: &HirEnumDef) {
        self.push_region(&e.span);
        for f in &e.methods {
            self.walk_func(f);
        }
        for ib in &e.impl_blocks {
            self.walk_impl(ib);
        }
    }

    fn walk_mixin(&mut self, m: &HirMixinDef) {
        self.push_region(&m.span);
        for item in &m.items {
            if let HirMixinItem::DefaultMethod(f) = item {
                self.walk_func(f);
            }
        }
    }

    fn walk_impl(&mut self, i: &HirImplBlock) {
        self.push_region(&i.span);
        for item in &i.items {
            if let HirImplItem::Method(f) = item {
                self.walk_func(f);
            }
        }
    }

    fn walk_func(&mut self, f: &HirFuncDef) {
        // The function span covers `def … end`. The function body is
        // a Block expr whose own span covers the same lines, so we
        // skip walking the *outer* block and descend straight into
        // its children to avoid emitting a duplicate range.
        self.push_region(&f.span);
        self.walk_func_body(&f.body);
    }

    /// Walk the body of a function, descending into nested blocks
    /// without emitting an extra Region for the body itself.
    fn walk_func_body(&mut self, body: &HirExpr) {
        match &body.kind {
            HirExprKind::Block(stmts, tail) => {
                for s in stmts {
                    self.walk_stmt(s);
                }
                if let Some(t) = tail {
                    self.walk_expr(t);
                }
            }
            _ => self.walk_expr(body),
        }
    }

    fn walk_stmt(&mut self, stmt: &HirStatement) {
        match stmt {
            HirStatement::Let { value, .. } => {
                if let Some(v) = value {
                    self.walk_expr(v);
                }
            }
            HirStatement::Expr(e) => self.walk_expr(e),
        }
    }

    fn walk_expr(&mut self, expr: &HirExpr) {
        match &expr.kind {
            HirExprKind::Block(stmts, tail) => {
                // Bare `do … end` (or any block expr) — fold it.
                self.push_region(&expr.span);
                for s in stmts {
                    self.walk_stmt(s);
                }
                if let Some(t) = tail {
                    self.walk_expr(t);
                }
            }
            HirExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.push_region(&expr.span);
                self.walk_expr(cond);
                self.walk_func_body(then_branch);
                if let Some(eb) = else_branch {
                    // If else_branch is itself another `If`, recursing
                    // through walk_expr will fold the `elsif` chain as
                    // its own region. A pure `else` Block we descend
                    // without an extra fold — the parent `If` already
                    // covers the closing `end`.
                    match &eb.kind {
                        HirExprKind::If { .. } => self.walk_expr(eb),
                        _ => self.walk_func_body(eb),
                    }
                }
            }
            HirExprKind::Match { scrutinee, arms } => {
                self.push_region(&expr.span);
                self.walk_expr(scrutinee);
                for arm in arms {
                    if let Some(g) = arm.guard.as_deref() {
                        self.walk_expr(g);
                    }
                    self.walk_func_body(&arm.body);
                }
            }
            HirExprKind::Loop { body } => {
                self.push_region(&expr.span);
                self.walk_func_body(body);
            }
            HirExprKind::While { condition, body } => {
                self.push_region(&expr.span);
                self.walk_expr(condition);
                self.walk_func_body(body);
            }
            HirExprKind::For { iterable, body, .. } => {
                self.push_region(&expr.span);
                self.walk_expr(iterable);
                self.walk_func_body(body);
            }
            HirExprKind::Closure { body, .. } => {
                // Closure bodies can have nested blocks; the closure
                // itself is usually inline so we don't fold it, but
                // we do descend.
                self.walk_expr(body);
            }
            HirExprKind::UnsafeBlock(stmts, tail) => {
                self.push_region(&expr.span);
                for s in stmts {
                    self.walk_stmt(s);
                }
                if let Some(t) = tail {
                    self.walk_expr(t);
                }
            }
            // Recurse through subexpressions.
            HirExprKind::FieldAccess { object, .. } => self.walk_expr(object),
            HirExprKind::MethodCall {
                object,
                args,
                block,
                ..
            } => {
                self.walk_expr(object);
                for a in args {
                    self.walk_expr(a);
                }
                if let Some(b) = block {
                    self.walk_expr(b);
                }
            }
            HirExprKind::FnCall { args, .. } => {
                for a in args {
                    self.walk_expr(a);
                }
            }
            HirExprKind::BinaryOp { left, right, .. } => {
                self.walk_expr(left);
                self.walk_expr(right);
            }
            HirExprKind::UnaryOp { operand, .. } => self.walk_expr(operand),
            HirExprKind::Borrow { expr, .. } => self.walk_expr(expr),
            HirExprKind::Assign { target, value, .. } => {
                self.walk_expr(target);
                self.walk_expr(value);
            }
            HirExprKind::CompoundAssign { target, value, .. } => {
                self.walk_expr(target);
                self.walk_expr(value);
            }
            HirExprKind::Return(opt) | HirExprKind::Break(opt) => {
                if let Some(e) = opt {
                    self.walk_expr(e);
                }
            }
            HirExprKind::Construct { fields, .. } | HirExprKind::EnumVariant { fields, .. } => {
                for (_, e) in fields {
                    self.walk_expr(e);
                }
            }
            HirExprKind::Tuple(es) | HirExprKind::ArrayLiteral(es) => {
                for e in es {
                    self.walk_expr(e);
                }
            }
            HirExprKind::Index { object, index } => {
                self.walk_expr(object);
                self.walk_expr(index);
            }
            HirExprKind::Cast { expr, .. } => self.walk_expr(expr),
            HirExprKind::MapLiteral(pairs) => {
                for (k, v) in pairs {
                    self.walk_expr(k);
                    self.walk_expr(v);
                }
            }
            HirExprKind::ArrayFill { value, .. } => self.walk_expr(value),
            HirExprKind::Range { start, end, .. } => {
                if let Some(s) = start {
                    self.walk_expr(s);
                }
                if let Some(e) = end {
                    self.walk_expr(e);
                }
            }
            HirExprKind::Interpolation { parts } => {
                for part in parts {
                    if let riven_core::hir::nodes::HirInterpolationPart::Expr { expr, .. } = part {
                        self.walk_expr(expr);
                    }
                }
            }
            HirExprKind::MacroCall { args, .. } => {
                for a in args {
                    self.walk_expr(a);
                }
            }
            // Leaves — nothing to descend.
            HirExprKind::IntLiteral(_)
            | HirExprKind::FloatLiteral(_)
            | HirExprKind::StringLiteral(_)
            | HirExprKind::BoolLiteral(_)
            | HirExprKind::CharLiteral(_)
            | HirExprKind::UnitLiteral
            | HirExprKind::NullLiteral
            | HirExprKind::VarRef(_)
            | HirExprKind::Continue
            | HirExprKind::Error => {}
        }
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
