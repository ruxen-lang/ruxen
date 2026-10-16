/// Comment classification and attachment for the Ruxen formatter.
///
/// The lexer keeps line and block comments out of the token stream but
/// records every comment's span ([`crate::lexer::Lexer::comment_spans`]).
/// The formatter classifies those spans, then attaches each comment to the
/// nearest AST node by byte position.
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use crate::lexer::token::Span;

// ─── Comment Types ──────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommentKind {
    /// `# ...` — single-line comment
    Line,
    /// `#= ... =#` — block comment (possibly nested)
    Block,
    /// `## ...` — doc comment
    Doc,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommentPosition {
    /// Comment on a line above a node.
    Leading,
    /// Comment at end of a line after code.
    Trailing,
    /// Comment inside an empty block body or between nodes.
    Dangling,
}

#[derive(Debug, Clone)]
pub struct Comment {
    pub kind: CommentKind,
    pub text: String,
    pub span: Span,
    pub position: CommentPosition,
}

// ─── Format Suppression Ranges ──────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FmtOffRange {
    pub start_byte: usize,
    pub end_byte: Option<usize>, // None = rest of file
}

// ─── Comment Map ────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct CommentMap {
    /// Leading comments keyed by the byte offset of the AST node they precede.
    pub leading: HashMap<usize, Vec<Comment>>,
    /// Trailing comments keyed by the byte offset of the AST node they follow.
    pub trailing: HashMap<usize, Vec<Comment>>,
    /// Dangling comments keyed by the byte offset of the enclosing scope.
    pub dangling: HashMap<usize, Vec<Comment>>,
    /// Own-line comments after the last member of a scope (before its
    /// `end`), keyed by that member's byte offset.
    pub after: HashMap<usize, Vec<Comment>>,
    /// Ranges where formatting is suppressed via `# fmt: off` / `# fmt: on`.
    pub fmt_off_ranges: Vec<FmtOffRange>,
    /// Every source line as `(start byte offset, kind)`, ascending. Read through
    /// [`Self::blank_line_before`].
    pub lines: Vec<(usize, LineKind)>,
    /// Span of every comment in the source, attached or not.
    all: Vec<Span>,
    /// Start offsets of the comments the formatter has written out. Filled
    /// by the accessors below, read by [`Self::dropped`].
    emitted: RefCell<HashSet<usize>>,
}

impl CommentMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Comments leading the node at `span_start`. The caller must write
    /// every comment returned: reading them marks them emitted.
    pub fn leading_comments(&self, span_start: usize) -> &[Comment] {
        self.mark(self.leading.get(&span_start))
    }

    /// Comments trailing the node at `span_start`; marks them emitted.
    pub fn trailing_comments(&self, span_start: usize) -> &[Comment] {
        self.mark(self.trailing.get(&span_start))
    }

    /// Own-line comments following the member at `span_start`; marks them
    /// emitted.
    pub fn after_comments(&self, span_start: usize) -> &[Comment] {
        self.mark(self.after.get(&span_start))
    }

    /// Comments dangling in the scope at `span_start`; marks them emitted.
    pub fn dangling_comments(&self, span_start: usize) -> &[Comment] {
        self.mark(self.dangling.get(&span_start))
    }

    fn mark<'m>(&self, comments: Option<&'m Vec<Comment>>) -> &'m [Comment] {
        let comments = comments.map_or(&[][..], |v| v.as_slice());
        let mut emitted = self.emitted.borrow_mut();
        emitted.extend(comments.iter().map(|c| c.span.start));
        comments
    }

    /// Whether the author left a blank line right above the node starting at
    /// `node_start`, or above the comment lines (`#` comments, `##` docs)
    /// and section marker directly over it. Block statements and type-body
    /// members keep that separation; fmt neither adds nor removes it.
    pub fn blank_line_before(&self, node_start: usize) -> bool {
        // Start from the node's first leading comment: blank lines inside its
        // comment block are the comments' own layout (`with_comments`).
        let top = self
            .leading
            .get(&node_start)
            .and_then(|c| c.iter().map(|c| c.span.start).min())
            .map_or(node_start, |c| c.min(node_start));
        let mut line = self.skip_comments_above(self.line_of(top));
        if let Some(marker) = self.marker_above(line) {
            line = self.skip_comments_above(marker);
        }
        line > 0 && self.lines[line - 1].1 == LineKind::Blank
    }

    /// Whether a blank line separates the node at `node_start` (with its
    /// comments) from a section marker above it (`private` + blank + `def`).
    pub fn blank_line_after_marker(&self, node_start: usize) -> bool {
        let top = self
            .leading
            .get(&node_start)
            .and_then(|c| c.iter().map(|c| c.span.start).min())
            .map_or(node_start, |c| c.min(node_start));
        let line = self.skip_comments_above(self.line_of(top));
        line > 0 && self.lines[line - 1].1 == LineKind::Blank && self.marker_above(line).is_some()
    }

    /// Whether the line right above the one holding `offset` is blank: the
    /// gap the author left between two comment groups, or between a comment
    /// block and the node under it.
    pub fn blank_line_directly_above(&self, offset: usize) -> bool {
        let line = self.line_of(offset);
        line > 0 && self.lines[line - 1].1 == LineKind::Blank
    }

    /// Whether the scope at `scope_start` has dangling comments, without
    /// marking them written.
    pub fn has_dangling(&self, scope_start: usize) -> bool {
        self.dangling
            .get(&scope_start)
            .is_some_and(|c| !c.is_empty())
    }

    /// Index into `lines` of the line holding `offset`.
    fn line_of(&self, offset: usize) -> usize {
        self.lines
            .partition_point(|&(start, _)| start <= offset)
            .saturating_sub(1)
    }

    /// The topmost line of the comment block directly above `line` (or
    /// `line` itself when there is none).
    fn skip_comments_above(&self, mut line: usize) -> usize {
        while line > 0 && self.lines[line - 1].1 == LineKind::Comment {
            line -= 1;
        }
        line
    }

    /// The section-marker line directly above `line`, past blank lines.
    fn marker_above(&self, line: usize) -> Option<usize> {
        let mut above = line;
        while above > 0 && self.lines[above - 1].1 == LineKind::Blank {
            above -= 1;
        }
        (above > 0 && self.lines[above - 1].1 == LineKind::Marker).then(|| above - 1)
    }

    /// Whether any comment starts inside `span`.
    pub fn any_within(&self, span: &Span) -> bool {
        self.all
            .iter()
            .any(|c| span.start <= c.start && c.start < span.end)
    }

    /// Comments the formatter never wrote out. `format` refuses to rewrite a
    /// file while this is non-empty, so `ruxen fmt` never deletes a comment.
    pub fn dropped(&self) -> Vec<&Span> {
        let emitted = self.emitted.borrow();
        self.all
            .iter()
            .filter(|s| !emitted.contains(&s.start))
            .collect()
    }

    /// Check if a byte position falls within a `# fmt: off` range.
    pub fn is_fmt_off(&self, byte_pos: usize) -> bool {
        self.fmt_off_ranges
            .iter()
            .any(|r| byte_pos >= r.start_byte && r.end_byte.is_none_or(|end| byte_pos < end))
    }
}

// ─── Comment Classification ─────────────────────────────────────────

/// Turn the comment spans the lexer recorded ([`crate::lexer::Lexer::comment_spans`])
/// into [`Comment`]s, and collect the `# fmt: off` / `# fmt: on` ranges.
///
/// The lexer decides what is a comment (so `#` inside strings, raw strings
/// and `#{...}` interpolation is never mistaken for one); this only reads
/// each comment's text and whether code precedes it on its line.
pub fn classify_comments(source: &str, spans: &[Span]) -> (Vec<Comment>, Vec<FmtOffRange>) {
    let mut comments = Vec::with_capacity(spans.len());
    let mut fmt_off_ranges: Vec<FmtOffRange> = Vec::new();
    for span in spans {
        let raw = &source[span.start..span.end];
        let (kind, text) = if let Some(rest) = raw.strip_prefix("#=") {
            (CommentKind::Block, rest.strip_suffix("=#").unwrap_or(rest))
        } else if let Some(rest) = raw.strip_prefix("##") {
            (CommentKind::Doc, rest.strip_prefix(' ').unwrap_or(rest))
        } else {
            (CommentKind::Line, &raw[1..])
        };
        if kind == CommentKind::Line {
            match text.trim() {
                "fmt: off" => fmt_off_ranges.push(FmtOffRange {
                    start_byte: span.start,
                    end_byte: None,
                }),
                "fmt: on" => {
                    if let Some(range) = fmt_off_ranges.last_mut() {
                        if range.end_byte.is_none() {
                            range.end_byte = Some(span.end);
                        }
                    }
                }
                _ => {}
            }
        }
        // `##` doc comments always lead the item below them.
        let position = if kind != CommentKind::Doc && has_code_before_on_line(source, span.start) {
            CommentPosition::Trailing
        } else {
            CommentPosition::Leading
        };
        comments.push(Comment {
            kind,
            text: text.to_string(),
            span: span.clone(),
            position,
        });
    }
    (comments, fmt_off_ranges)
}

/// Whether non-whitespace text precedes byte `offset` on its line.
fn has_code_before_on_line(source: &str, offset: usize) -> bool {
    let line_start = source[..offset].rfind('\n').map_or(0, |i| i + 1);
    !source[line_start..offset].trim().is_empty()
}

// ─── Comment Attacher ───────────────────────────────────────────────

/// Given collected comments and AST node spans, attach each comment to the
/// nearest node as leading, trailing, or dangling.
pub struct CommentAttacher;

impl CommentAttacher {
    /// Attach comments to AST nodes. `node_spans` is a sorted list of
    /// (start_byte, end_byte) pairs for every AST node.
    pub fn attach(
        comments: Vec<Comment>,
        nodes: &NodeSpans,
        fmt_off_ranges: Vec<FmtOffRange>,
    ) -> CommentMap {
        let node_spans = nodes.spans.as_slice();
        let mut map = CommentMap::new();
        map.fmt_off_ranges = fmt_off_ranges;
        map.all = comments.iter().map(|c| c.span.clone()).collect();

        for comment in comments {
            let comment_start = comment.span.start;

            match comment.position {
                CommentPosition::Trailing => {
                    // Find the node whose span contains or immediately precedes
                    // the comment on the same line.
                    if let Some(&(node_start, _)) =
                        Self::find_preceding_node(node_spans, comment_start)
                    {
                        map.trailing.entry(node_start).or_default().push(comment);
                    } else {
                        // No preceding node — treat as leading of next node
                        if let Some(&(node_start, _)) =
                            Self::find_following_node(node_spans, comment_start)
                        {
                            let mut c = comment;
                            c.position = CommentPosition::Leading;
                            map.leading.entry(node_start).or_default().push(c);
                        }
                    }
                }
                CommentPosition::Leading => {
                    // Attach to the next AST node, unless that node lies
                    // outside the scope the comment sits in (a comment just
                    // before an `end`): then it follows the scope's last
                    // member, or dangles in the scope when it has none. A
                    // comment after the last item sits in the outermost
                    // node, the program.
                    let following = Self::find_following_node(node_spans, comment_start);
                    let enclosing =
                        Self::find_enclosing_node(node_spans, comment_start).or(node_spans.first());
                    let last_member = enclosing.and_then(|&(scope_start, _)| {
                        Self::find_preceding_node(&nodes.members, comment_start).filter(|m| {
                            m.0 > scope_start || (m.0 == scope_start && m.1 <= comment_start)
                        })
                    });
                    match (following, enclosing, last_member) {
                        (Some(&(node_start, _)), Some(&(_, scope_end)), _)
                            if node_start < scope_end =>
                        {
                            map.leading.entry(node_start).or_default().push(comment);
                        }
                        (Some(&(node_start, _)), None, _) => {
                            map.leading.entry(node_start).or_default().push(comment);
                        }
                        (_, Some(_), Some(&(member_start, _))) => {
                            map.after.entry(member_start).or_default().push(comment);
                        }
                        (_, Some(&(scope_start, _)), None) => {
                            let mut c = comment;
                            c.position = CommentPosition::Dangling;
                            map.dangling.entry(scope_start).or_default().push(c);
                        }
                        (None, None, _) => {}
                    }
                }
                CommentPosition::Dangling => {
                    if let Some(&(node_start, _)) =
                        Self::find_enclosing_node(node_spans, comment_start)
                    {
                        map.dangling.entry(node_start).or_default().push(comment);
                    }
                }
            }
        }

        map
    }

    /// Find the node whose span end is closest to and before `pos`.
    fn find_preceding_node(spans: &[(usize, usize)], pos: usize) -> Option<&(usize, usize)> {
        let mut best: Option<&(usize, usize)> = None;
        for span in spans {
            if span.1 <= pos {
                match best {
                    None => best = Some(span),
                    Some(b) => {
                        if span.1 > b.1 {
                            best = Some(span);
                        }
                    }
                }
            }
        }
        best
    }

    /// Find the node whose span start is closest to and after `pos`.
    fn find_following_node(spans: &[(usize, usize)], pos: usize) -> Option<&(usize, usize)> {
        let mut best: Option<&(usize, usize)> = None;
        for span in spans {
            if span.0 >= pos {
                match best {
                    None => best = Some(span),
                    Some(b) => {
                        if span.0 < b.0 {
                            best = Some(span);
                        }
                    }
                }
            }
        }
        best
    }

    /// Find the smallest node that contains `pos`.
    fn find_enclosing_node(spans: &[(usize, usize)], pos: usize) -> Option<&(usize, usize)> {
        let mut best: Option<&(usize, usize)> = None;
        for span in spans {
            if span.0 <= pos && pos < span.1 {
                match best {
                    None => best = Some(span),
                    Some(b) => {
                        let b_size = b.1 - b.0;
                        let s_size = span.1 - span.0;
                        if s_size < b_size {
                            best = Some(span);
                        }
                    }
                }
            }
        }
        best
    }
}

/// Collect all AST node spans from a program for comment attachment.
///
/// The expression / block / statement / pattern / type-expr recursion is
/// delegated to the shared [`crate::parser::visit::Visit`] walk (one
/// exhaustive match, no `_ =>` arm — a new `ExprKind` variant becomes a
/// compile error). [`SpanCollector`] overrides each `visit_*` to record the
/// node's span and to add the auxiliary spans the formatter also attaches to
/// (elsif / match-arm / closure-param / struct-field-pattern / enum-variant-arg
/// / let-statement). A few arms (`MethodCall`/`Cast` type args, `Named`
/// generic args, `Array` size, `ConstExprArg`) deliberately do NOT recurse
/// into their type-expr / expr children — the previous hand-rolled walk did
/// not collect those spans, and widening the set could shift comment
/// attachment. Item-level traversal (modules/classes/funcs/lib decls) stays
/// hand-rolled: `Visit` covers expressions, not top-level items.
pub fn collect_node_spans(program: &crate::parser::ast::Program) -> NodeSpans {
    use crate::parser::ast::*;
    use crate::parser::visit::{walk_expr, walk_pattern, Visit};

    struct SpanCollector {
        spans: Vec<(usize, usize)>,
        members: Vec<(usize, usize)>,
    }

    impl SpanCollector {
        fn add(&mut self, span: &Span) {
            // The parser gives an empty body block an inverted span (start
            // past end); it encloses nothing, so it cannot carry a comment.
            if span.start <= span.end {
                self.spans.push((span.start, span.end));
            }
        }

        /// A node the formatter writes on its own line(s) through
        /// `with_comments` (item, body member, statement).
        fn member(&mut self, span: &Span) {
            self.add(span);
            self.members.push((span.start, span.end));
        }

        fn visit_func(&mut self, func: &FuncDef) {
            self.member(&func.span);
            for p in &func.params {
                self.add(&p.span);
                self.visit_type_expr(&p.type_expr);
            }
            if let Some(rt) = &func.return_type {
                self.visit_type_expr(rt);
            }
            self.visit_block(&func.body);
        }

        /// The members every type body shares: methods, inner impls and
        /// in-body `lib` blocks.
        fn visit_type_body(&mut self, methods: &[FuncDef], inner_impls: &[InnerImpl]) {
            for m in methods {
                self.visit_func(m);
            }
            for imp in inner_impls {
                self.member(&imp.span);
                self.visit_impl_items(&imp.items);
            }
        }

        fn visit_lib(&mut self, lib: &LibDecl) {
            self.member(&lib.span);
            for f in &lib.functions {
                self.member(&f.span);
            }
        }

        fn visit_impl_items(&mut self, items: &[ImplItem]) {
            for ii in items {
                match ii {
                    ImplItem::AssocType { span, .. } => self.member(span),
                    ImplItem::Method(f) => self.visit_func(f),
                    ImplItem::Include { span, .. } => self.member(span),
                    ImplItem::Alias(a) => self.member(&a.span),
                }
            }
        }

        fn visit_item(&mut self, item: &TopLevelItem) {
            match item {
                TopLevelItem::Module(m) => {
                    self.member(&m.span);
                    for i in &m.items {
                        self.visit_item(i);
                    }
                }
                TopLevelItem::Class(c) => {
                    self.member(&c.span);
                    for f in &c.fields {
                        self.member(&f.span);
                    }
                    self.visit_type_body(&c.methods, &c.inner_impls);
                    // Class-body `lib "..." ... end` blocks: register each FFI
                    // def span so `##` doc comments attached above them survive
                    // formatting (otherwise `ruxen fmt` drops the docs).
                    for lib in &c.lib_decls {
                        self.visit_lib(lib);
                    }
                    for a in &c.aliases {
                        self.member(&a.span);
                    }
                    for a in &c.assoc_types {
                        self.member(&a.span);
                        self.visit_type_expr(&a.type_expr);
                    }
                }
                TopLevelItem::Struct(s) => {
                    self.member(&s.span);
                    for f in &s.fields {
                        self.member(&f.span);
                    }
                    self.visit_type_body(&s.methods, &s.inner_impls);
                    for a in &s.aliases {
                        self.member(&a.span);
                    }
                }
                TopLevelItem::Enum(e) => {
                    self.member(&e.span);
                    for v in &e.variants {
                        self.member(&v.span);
                    }
                    self.visit_type_body(&e.methods, &e.inner_impls);
                    for a in &e.aliases {
                        self.member(&a.span);
                    }
                }
                TopLevelItem::Mixin(t) => {
                    self.member(&t.span);
                    for ti in &t.items {
                        match ti {
                            MixinItem::AssocType { span, .. } => self.member(span),
                            MixinItem::MethodSig(ms) => self.member(&ms.span),
                            MixinItem::DefaultMethod(f) => self.visit_func(f),
                            MixinItem::Alias(a) => self.member(&a.span),
                        }
                    }
                    for lib in &t.lib_decls {
                        self.visit_lib(lib);
                    }
                }
                TopLevelItem::Impl(imp) => {
                    self.member(&imp.span);
                    self.visit_impl_items(&imp.items);
                }
                TopLevelItem::Function(f) => self.visit_func(f),
                TopLevelItem::Use(u) => self.member(&u.span),
                TopLevelItem::TypeAlias(ta) => {
                    self.member(&ta.span);
                    self.visit_type_expr(&ta.type_expr);
                }
                TopLevelItem::Newtype(nt) => {
                    self.member(&nt.span);
                    self.visit_type_expr(&nt.inner_type);
                }
                TopLevelItem::Const(c) => {
                    self.member(&c.span);
                    self.visit_type_expr(&c.type_expr);
                    self.visit_expr(&c.value);
                }
                TopLevelItem::Lib(l) => self.visit_lib(l),
                // Q23b: a top-level expression statement. Register its span
                // (and recurse into the expression) so comments attach to it
                // correctly, the same way an in-body statement expr does.
                TopLevelItem::Expr(e) => {
                    self.member(&e.span);
                    self.visit_expr(e);
                }
                TopLevelItem::Alias(a) => self.member(&a.span),
            }
        }
    }

    impl Visit for SpanCollector {
        fn visit_expr(&mut self, expr: &Expr) {
            self.add(&expr.span);
            match &expr.kind {
                // Arms that record an auxiliary (non-`expr.span`) span the
                // previous walk attached to, then recurse via the shared walk.
                ExprKind::If(if_expr) => {
                    for elsif in &if_expr.elsif_clauses {
                        self.add(&elsif.span);
                    }
                    walk_expr(self, expr);
                }
                ExprKind::Match(match_expr) => {
                    for arm in &match_expr.arms {
                        self.member(&arm.span);
                    }
                    walk_expr(self, expr);
                }
                ExprKind::Closure(c) => {
                    for p in &c.params {
                        self.add(&p.span);
                    }
                    walk_expr(self, expr);
                }
                ExprKind::EnumVariant { args, .. } => {
                    for a in args {
                        self.add(&a.span);
                    }
                    walk_expr(self, expr);
                }
                // `MethodCall`/`Cast` carry type-expr children (generic_args /
                // target_type) the previous walk did NOT visit; recurse only
                // into the value children to keep the collected set identical.
                ExprKind::MethodCall {
                    object,
                    args,
                    block,
                    ..
                } => {
                    self.visit_expr(object);
                    for a in args {
                        self.visit_expr(a);
                    }
                    if let Some(b) = block {
                        self.visit_expr(b);
                    }
                }
                ExprKind::Cast { expr: inner, .. } => self.visit_expr(inner),
                // Everything else recurses through the shared exhaustive walk.
                _ => walk_expr(self, expr),
            }
        }

        fn visit_block(&mut self, block: &Block) {
            self.add(&block.span);
            for stmt in &block.statements {
                self.visit_stmt(stmt);
            }
        }

        fn visit_stmt(&mut self, stmt: &Statement) {
            match stmt {
                // The shared `walk_stmt` does not record the `let`-statement
                // span, so record it here.
                Statement::Let(l) => self.member(&l.span),
                Statement::Expression(e) => self.members.push((e.span.start, e.span.end)),
            }
            crate::parser::visit::walk_stmt(self, stmt);
        }

        fn visit_pattern(&mut self, pat: &Pattern) {
            match pat {
                Pattern::Literal { span, .. }
                | Pattern::Identifier { span, .. }
                | Pattern::Wildcard { span }
                | Pattern::Rest { span }
                | Pattern::Ref { span, .. }
                | Pattern::Tuple { span, .. }
                | Pattern::Enum { span, .. }
                | Pattern::Or { span, .. } => self.add(span),
                Pattern::Struct { span, fields, .. } => {
                    self.add(span);
                    for f in fields {
                        self.add(&f.span);
                    }
                }
            }
            walk_pattern(self, pat);
        }

        fn visit_type_expr(&mut self, ty: &TypeExpr) {
            // The previous walk recorded a span for every type-expr node but
            // did NOT recurse into `Named` generic args, `Array` sizes, or
            // `ConstExprArg` expressions. Preserve that exact set: add this
            // node's span and recurse only into the type-expr children the old
            // walk descended into (Reference / Tuple / Array element /
            // Function params+return / RawPointer inner).
            match ty {
                TypeExpr::Named(tp) => self.add(&tp.span),
                TypeExpr::Reference { span, inner, .. } => {
                    self.add(span);
                    self.visit_type_expr(inner);
                }
                TypeExpr::Tuple { span, elements, .. } => {
                    self.add(span);
                    for e in elements {
                        self.visit_type_expr(e);
                    }
                }
                TypeExpr::Array { span, element, .. } => {
                    self.add(span);
                    self.visit_type_expr(element);
                }
                TypeExpr::Function {
                    span,
                    params,
                    return_type,
                    ..
                } => {
                    self.add(span);
                    for p in params {
                        self.visit_type_expr(p);
                    }
                    self.visit_type_expr(return_type);
                }
                TypeExpr::SomeMixin { span, .. }
                | TypeExpr::AnyMixin { span, .. }
                | TypeExpr::Never { span }
                | TypeExpr::Inferred { span } => self.add(span),
                TypeExpr::RawPointer { span, inner, .. } => {
                    self.add(span);
                    self.visit_type_expr(inner);
                }
                TypeExpr::ConstLit { span, .. } => self.add(span),
                TypeExpr::ConstExprArg { span, .. } => self.add(span),
            }
        }
    }

    let mut collector = SpanCollector {
        spans: Vec::new(),
        members: Vec::new(),
    };
    collector.add(&program.span);
    for item in &program.items {
        collector.visit_item(item);
    }

    let mut spans = collector.spans;
    spans.sort_by_key(|&(start, _)| start);
    let mut members = collector.members;
    members.sort_by_key(|&(start, _)| start);
    NodeSpans { spans, members }
}

/// AST node spans for comment attachment, sorted by start offset.
pub struct NodeSpans {
    /// Every node.
    pub spans: Vec<(usize, usize)>,
    /// The nodes written on their own line(s): items, body members and
    /// statements.
    pub members: Vec<(usize, usize)>,
}

/// What a source line holds, for [`CommentMap::blank_line_before`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Blank,
    /// Only a comment (`# ...` or a `##` doc line).
    Comment,
    /// A `private` / `public` / `protected` section marker.
    Marker,
    Code,
}

/// Every line of `source` as `(start byte offset, kind)` (see
/// [`CommentMap::lines`]).
pub fn source_lines(source: &str) -> Vec<(usize, LineKind)> {
    let mut out = Vec::new();
    let mut offset = 0;
    for line in source.split_inclusive('\n') {
        let trimmed = line.trim();
        let kind = if trimmed.is_empty() {
            LineKind::Blank
        } else if trimmed.starts_with('#') {
            LineKind::Comment
        } else if matches!(trimmed, "private" | "public" | "protected") {
            LineKind::Marker
        } else {
            LineKind::Code
        };
        out.push((offset, kind));
        offset += line.len();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(source: &str) -> (Vec<Comment>, Vec<FmtOffRange>) {
        let mut lexer = crate::lexer::Lexer::new(source);
        lexer.tokenize().expect("fixture lexes");
        classify_comments(source, lexer.comment_spans())
    }

    #[test]
    fn test_collect_line_comment() {
        let (comments, _) = collect("# hello world\n");
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].kind, CommentKind::Line);
        assert_eq!(comments[0].text, " hello world");
        assert_eq!(comments[0].position, CommentPosition::Leading);
    }

    #[test]
    fn test_collect_doc_comment() {
        let (comments, _) = collect("## A doc comment\n");
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].kind, CommentKind::Doc);
        assert_eq!(comments[0].text, "A doc comment");
    }

    #[test]
    fn test_collect_block_comment() {
        let (comments, _) = collect("#= block content =#\n");
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].kind, CommentKind::Block);
        assert_eq!(comments[0].text, " block content ");
    }

    #[test]
    fn test_nested_block_comment() {
        let (comments, _) = collect("#= outer #= inner =# still outer =#\n");
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].kind, CommentKind::Block);
    }

    #[test]
    fn test_comment_in_string_ignored() {
        let (comments, _) = collect("\"this # is not a comment\"\n");
        assert_eq!(comments.len(), 0);
    }

    /// The old text scanner read `'` as a char literal and took the `#` in
    /// a single-quoted raw string for a comment.
    #[test]
    fn test_comment_in_raw_string_ignored() {
        let (comments, _) = collect("x = 'a # b'\n");
        assert_eq!(comments.len(), 0);
    }

    #[test]
    fn test_trailing_comment() {
        let (comments, _) = collect("let x = 42  # the answer\n");
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].position, CommentPosition::Trailing);
    }

    #[test]
    fn test_fmt_off_on() {
        let (comments, ranges) = collect("# fmt: off\ncode here\n# fmt: on\nmore code\n");
        assert_eq!(comments.len(), 2);
        assert_eq!(ranges.len(), 1);
        assert!(ranges[0].end_byte.is_some());
    }

    #[test]
    fn test_fmt_off_no_on() {
        let (_, ranges) = collect("# fmt: off\nrest of file\n");
        assert_eq!(ranges.len(), 1);
        assert!(ranges[0].end_byte.is_none());
    }

    #[test]
    fn test_interpolation_not_comment() {
        let (comments, _) = collect("\"hello #{name}\"\n# real comment\n");
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].kind, CommentKind::Line);
    }
}
