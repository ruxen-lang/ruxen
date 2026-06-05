/// Comment collection and attachment for the Ruxen formatter.
///
/// The lexer discards line comments and block comments (only doc comments
/// are emitted as tokens). The formatter needs all comments, so we re-scan
/// the raw source text to extract them, then attach each comment to the
/// nearest AST node by byte position.
use std::collections::HashMap;

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
    /// Ranges where formatting is suppressed via `# fmt: off` / `# fmt: on`.
    pub fmt_off_ranges: Vec<FmtOffRange>,
}

impl CommentMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn leading_comments(&self, span_start: usize) -> &[Comment] {
        self.leading.get(&span_start).map_or(&[], |v| v.as_slice())
    }

    pub fn trailing_comments(&self, span_start: usize) -> &[Comment] {
        self.trailing.get(&span_start).map_or(&[], |v| v.as_slice())
    }

    pub fn dangling_comments(&self, span_start: usize) -> &[Comment] {
        self.dangling.get(&span_start).map_or(&[], |v| v.as_slice())
    }

    /// Check if a byte position falls within a `# fmt: off` range.
    pub fn is_fmt_off(&self, byte_pos: usize) -> bool {
        self.fmt_off_ranges
            .iter()
            .any(|r| byte_pos >= r.start_byte && r.end_byte.is_none_or(|end| byte_pos < end))
    }
}

// ─── Comment Collector ──────────────────────────────────────────────

/// Scans raw source text and extracts all comments with their positions.
pub struct CommentCollector<'a> {
    source: &'a str,
    chars: Vec<char>,
    pos: usize,
    byte_pos: usize,
    line: u32,
    column: u32,
    comments: Vec<Comment>,
    fmt_off_ranges: Vec<FmtOffRange>,
}

impl<'a> CommentCollector<'a> {
    pub fn new(source: &'a str) -> Self {
        Self {
            source,
            chars: source.chars().collect(),
            pos: 0,
            byte_pos: 0,
            line: 1,
            column: 1,
            comments: Vec::new(),
            fmt_off_ranges: Vec::new(),
        }
    }

    pub fn collect(mut self) -> (Vec<Comment>, Vec<FmtOffRange>) {
        while !self.is_at_end() {
            let ch = self.current();
            match ch {
                '#' => self.handle_hash(),
                '"' => self.skip_string(),
                '\'' => self.skip_char(),
                'r' if self.peek_at(1) == Some('"') || self.peek_at(1) == Some('#') => {
                    self.skip_raw_string()
                }
                '\n' => {
                    self.advance();
                }
                _ => {
                    self.advance();
                }
            }
        }

        (self.comments, self.fmt_off_ranges)
    }

    fn handle_hash(&mut self) {
        let start_byte = self.byte_pos;
        let start_line = self.line;
        let start_col = self.column;

        if self.peek_at(1) == Some('=') {
            // Block comment #= ... =#
            self.collect_block_comment(start_byte, start_line, start_col);
        } else if self.peek_at(1) == Some('#') {
            // Doc comment ##
            self.collect_doc_comment(start_byte, start_line, start_col);
        } else if self.peek_at(1) == Some('{') {
            // String interpolation `#{...}` — not a comment. Skip the `#`.
            self.advance();
        } else {
            // Line comment
            self.collect_line_comment(start_byte, start_line, start_col);
        }
    }

    fn collect_line_comment(&mut self, start_byte: usize, start_line: u32, start_col: u32) {
        self.advance(); // skip `#`

        // Skip optional leading space
        let content_start = self.pos;
        while !self.is_at_end() && self.current() != '\n' {
            self.advance();
        }
        let content: String = self.chars[content_start..self.pos].iter().collect();

        let span = Span::new(start_byte, self.byte_pos, start_line, start_col);

        // Check for fmt: off/on directives
        let trimmed = content.trim();
        if trimmed == "fmt: off" {
            self.fmt_off_ranges.push(FmtOffRange {
                start_byte,
                end_byte: None,
            });
        } else if trimmed == "fmt: on" {
            if let Some(range) = self.fmt_off_ranges.last_mut() {
                if range.end_byte.is_none() {
                    range.end_byte = Some(self.byte_pos);
                }
            }
        }

        // Determine if this is a trailing comment (code before it on same line)
        let is_trailing = self.has_code_before_on_line(start_byte);

        self.comments.push(Comment {
            kind: CommentKind::Line,
            text: content,
            span,
            position: if is_trailing {
                CommentPosition::Trailing
            } else {
                CommentPosition::Leading
            },
        });
    }

    fn collect_doc_comment(&mut self, start_byte: usize, start_line: u32, start_col: u32) {
        self.advance(); // first #
        self.advance(); // second #

        // Skip optional leading space
        if !self.is_at_end() && self.current() == ' ' {
            self.advance();
        }

        let content_start = self.pos;
        while !self.is_at_end() && self.current() != '\n' {
            self.advance();
        }
        let content: String = self.chars[content_start..self.pos].iter().collect();

        let span = Span::new(start_byte, self.byte_pos, start_line, start_col);

        self.comments.push(Comment {
            kind: CommentKind::Doc,
            text: content,
            span,
            position: CommentPosition::Leading,
        });
    }

    fn collect_block_comment(&mut self, start_byte: usize, start_line: u32, start_col: u32) {
        self.advance(); // #
        self.advance(); // =

        let mut depth = 1u32;
        let content_start = self.pos;

        while !self.is_at_end() && depth > 0 {
            if self.current() == '#' && self.peek_at(1) == Some('=') {
                self.advance();
                self.advance();
                depth += 1;
            } else if self.current() == '=' && self.peek_at(1) == Some('#') {
                depth -= 1;
                if depth > 0 {
                    self.advance();
                    self.advance();
                } else {
                    // Don't advance past the closing =# yet
                    break;
                }
            } else {
                self.advance();
            }
        }

        let content: String = self.chars[content_start..self.pos].iter().collect();

        // Skip past closing =#
        if !self.is_at_end() {
            self.advance(); // =
        }
        if !self.is_at_end() {
            self.advance(); // #
        }

        let span = Span::new(start_byte, self.byte_pos, start_line, start_col);

        let is_trailing = self.has_code_before_on_line(start_byte);

        self.comments.push(Comment {
            kind: CommentKind::Block,
            text: content,
            span,
            position: if is_trailing {
                CommentPosition::Trailing
            } else {
                CommentPosition::Leading
            },
        });
    }

    /// Check if there is non-whitespace content before the given byte offset
    /// on the same line.
    fn has_code_before_on_line(&self, byte_offset: usize) -> bool {
        let source_bytes = self.source.as_bytes();
        if byte_offset == 0 {
            return false;
        }
        let mut i = byte_offset - 1;
        loop {
            if i == 0 || source_bytes.get(i) == Some(&b'\n') {
                return false;
            }
            let ch = source_bytes[i];
            if ch != b' ' && ch != b'\t' && ch != b'\r' {
                return true;
            }
            if i == 0 {
                break;
            }
            i -= 1;
        }
        false
    }

    fn skip_string(&mut self) {
        // Check for triple-quoted string
        if self.peek_at(1) == Some('"') && self.peek_at(2) == Some('"') {
            self.skip_triple_string();
            return;
        }

        self.advance(); // opening "
        while !self.is_at_end() {
            match self.current() {
                '\\' => {
                    self.advance(); // backslash
                    if !self.is_at_end() {
                        self.advance(); // escaped char
                    }
                }
                '#' if self.peek_at(1) == Some('{') => {
                    // String interpolation — skip #{...}
                    self.advance(); // #
                    self.advance(); // {
                    self.skip_braces(1);
                }
                '"' => {
                    self.advance(); // closing "
                    return;
                }
                _ => {
                    self.advance();
                }
            }
        }
    }

    fn skip_triple_string(&mut self) {
        self.advance(); // "
        self.advance(); // "
        self.advance(); // "
        while !self.is_at_end() {
            match self.current() {
                '\\' => {
                    self.advance();
                    if !self.is_at_end() {
                        self.advance();
                    }
                }
                '#' if self.peek_at(1) == Some('{') => {
                    self.advance(); // #
                    self.advance(); // {
                    self.skip_braces(1);
                }
                '"' if self.peek_at(1) == Some('"') && self.peek_at(2) == Some('"') => {
                    self.advance(); // "
                    self.advance(); // "
                    self.advance(); // "
                    return;
                }
                _ => {
                    self.advance();
                }
            }
        }
    }

    fn skip_raw_string(&mut self) {
        self.advance(); // r
                        // Count leading # chars
        let mut hashes = 0;
        while !self.is_at_end() && self.current() == '#' {
            hashes += 1;
            self.advance();
        }
        if !self.is_at_end() && self.current() == '"' {
            self.advance(); // opening "
        }
        // Read until closing " followed by same number of #
        while !self.is_at_end() {
            if self.current() == '"' {
                self.advance();
                let mut found_hashes = 0;
                while found_hashes < hashes && !self.is_at_end() && self.current() == '#' {
                    found_hashes += 1;
                    self.advance();
                }
                if found_hashes == hashes {
                    return;
                }
            } else {
                self.advance();
            }
        }
    }

    fn skip_char(&mut self) {
        self.advance(); // opening '
        if !self.is_at_end() && self.current() == '\\' {
            self.advance(); // backslash
            if !self.is_at_end() {
                self.advance(); // escaped char
            }
        } else if !self.is_at_end() {
            // Check if this is a lifetime ('a) vs char literal
            let next = self.current();
            if next.is_alphabetic() || next == '_' {
                // Could be lifetime — check if followed by more ident chars
                self.advance();
                while !self.is_at_end()
                    && (self.current().is_alphanumeric() || self.current() == '_')
                {
                    self.advance();
                }
                // If we hit a ', it's a char literal; otherwise it was a lifetime.
                if !self.is_at_end() && self.current() == '\'' {
                    self.advance();
                }
                return;
            }
            self.advance();
        }
        if !self.is_at_end() && self.current() == '\'' {
            self.advance(); // closing '
        }
    }

    /// Skip past balanced braces, starting with `depth` open braces already consumed.
    fn skip_braces(&mut self, mut depth: u32) {
        while !self.is_at_end() && depth > 0 {
            match self.current() {
                '{' => {
                    depth += 1;
                    self.advance();
                }
                '}' => {
                    depth -= 1;
                    self.advance();
                }
                '"' => self.skip_string(),
                '\'' => self.skip_char(),
                '#' if self.peek_at(1) == Some('{') => {
                    self.advance();
                    self.advance();
                    self.skip_braces(1);
                }
                _ => {
                    self.advance();
                }
            }
        }
    }

    // ── Navigation helpers ──

    fn is_at_end(&self) -> bool {
        self.pos >= self.chars.len()
    }

    fn current(&self) -> char {
        self.chars[self.pos]
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn advance(&mut self) -> char {
        let ch = self.chars[self.pos];
        self.byte_pos += ch.len_utf8();
        self.pos += 1;
        if ch == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        ch
    }
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
        node_spans: &[(usize, usize)],
        fmt_off_ranges: Vec<FmtOffRange>,
    ) -> CommentMap {
        let mut map = CommentMap::new();
        map.fmt_off_ranges = fmt_off_ranges;

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
                    // Attach to the next AST node following this comment.
                    if let Some(&(node_start, _)) =
                        Self::find_following_node(node_spans, comment_start)
                    {
                        map.leading.entry(node_start).or_default().push(comment);
                    } else if let Some(&(node_start, _)) =
                        Self::find_enclosing_node(node_spans, comment_start)
                    {
                        // No following node — dangling in enclosing scope.
                        let mut c = comment;
                        c.position = CommentPosition::Dangling;
                        map.dangling.entry(node_start).or_default().push(c);
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
pub fn collect_node_spans(program: &crate::parser::ast::Program) -> Vec<(usize, usize)> {
    use crate::parser::ast::*;
    use crate::parser::visit::{walk_expr, walk_pattern, Visit};

    struct SpanCollector {
        spans: Vec<(usize, usize)>,
    }

    impl SpanCollector {
        fn add(&mut self, span: &Span) {
            self.spans.push((span.start, span.end));
        }

        fn visit_func(&mut self, func: &FuncDef) {
            self.add(&func.span);
            for p in &func.params {
                self.add(&p.span);
                self.visit_type_expr(&p.type_expr);
            }
            if let Some(rt) = &func.return_type {
                self.visit_type_expr(rt);
            }
            self.visit_block(&func.body);
        }

        fn visit_item(&mut self, item: &TopLevelItem) {
            match item {
                TopLevelItem::Module(m) => {
                    self.add(&m.span);
                    for i in &m.items {
                        self.visit_item(i);
                    }
                }
                TopLevelItem::Class(c) => {
                    self.add(&c.span);
                    for f in &c.fields {
                        self.add(&f.span);
                    }
                    for m in &c.methods {
                        self.visit_func(m);
                    }
                    // Class-body `lib "..." ... end` blocks: register each FFI
                    // def span so `##` doc comments attached above them survive
                    // formatting (otherwise `ruxen fmt` drops the docs).
                    for lib in &c.lib_decls {
                        self.add(&lib.span);
                        for f in &lib.functions {
                            self.add(&f.span);
                        }
                    }
                }
                TopLevelItem::Struct(s) => {
                    self.add(&s.span);
                    for f in &s.fields {
                        self.add(&f.span);
                    }
                }
                TopLevelItem::Enum(e) => {
                    self.add(&e.span);
                    for v in &e.variants {
                        self.add(&v.span);
                    }
                }
                TopLevelItem::Mixin(t) => {
                    self.add(&t.span);
                    for ti in &t.items {
                        match ti {
                            MixinItem::AssocType { span, .. } => self.add(span),
                            MixinItem::MethodSig(ms) => self.add(&ms.span),
                            MixinItem::DefaultMethod(f) => self.visit_func(f),
                        }
                    }
                }
                TopLevelItem::Impl(imp) => {
                    self.add(&imp.span);
                    for ii in &imp.items {
                        match ii {
                            ImplItem::AssocType { span, .. } => self.add(span),
                            ImplItem::Method(f) => self.visit_func(f),
                            ImplItem::Include { span, .. } => self.add(span),
                        }
                    }
                }
                TopLevelItem::Function(f) => self.visit_func(f),
                TopLevelItem::Use(u) => self.add(&u.span),
                TopLevelItem::TypeAlias(ta) => {
                    self.add(&ta.span);
                    self.visit_type_expr(&ta.type_expr);
                }
                TopLevelItem::Newtype(nt) => {
                    self.add(&nt.span);
                    self.visit_type_expr(&nt.inner_type);
                }
                TopLevelItem::Const(c) => {
                    self.add(&c.span);
                    self.visit_type_expr(&c.type_expr);
                    self.visit_expr(&c.value);
                }
                TopLevelItem::Lib(l) => {
                    self.add(&l.span);
                    for f in &l.functions {
                        self.add(&f.span);
                    }
                }
                TopLevelItem::Extern(e) => {
                    self.add(&e.span);
                    for f in &e.functions {
                        self.add(&f.span);
                    }
                }
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
                        self.add(&arm.span);
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
            if let Statement::Let(l) = stmt {
                // The previous walk attached the `let`-statement span; the
                // shared `walk_stmt` does not, so record it here.
                self.add(&l.span);
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

    let mut collector = SpanCollector { spans: Vec::new() };
    collector.add(&program.span);
    for item in &program.items {
        collector.visit_item(item);
    }

    let mut spans = collector.spans;
    spans.sort_by_key(|&(start, _)| start);
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collect_line_comment() {
        let source = "# hello world\n";
        let collector = CommentCollector::new(source);
        let (comments, _) = collector.collect();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].kind, CommentKind::Line);
        assert_eq!(comments[0].text, " hello world");
        assert_eq!(comments[0].position, CommentPosition::Leading);
    }

    #[test]
    fn test_collect_doc_comment() {
        let source = "## A doc comment\n";
        let collector = CommentCollector::new(source);
        let (comments, _) = collector.collect();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].kind, CommentKind::Doc);
        assert_eq!(comments[0].text, "A doc comment");
    }

    #[test]
    fn test_collect_block_comment() {
        let source = "#= block content =#\n";
        let collector = CommentCollector::new(source);
        let (comments, _) = collector.collect();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].kind, CommentKind::Block);
        assert_eq!(comments[0].text, " block content ");
    }

    #[test]
    fn test_nested_block_comment() {
        let source = "#= outer #= inner =# still outer =#\n";
        let collector = CommentCollector::new(source);
        let (comments, _) = collector.collect();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].kind, CommentKind::Block);
    }

    #[test]
    fn test_comment_in_string_ignored() {
        let source = "\"this # is not a comment\"\n";
        let collector = CommentCollector::new(source);
        let (comments, _) = collector.collect();
        assert_eq!(comments.len(), 0);
    }

    #[test]
    fn test_trailing_comment() {
        let source = "let x = 42  # the answer\n";
        let collector = CommentCollector::new(source);
        let (comments, _) = collector.collect();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].position, CommentPosition::Trailing);
    }

    #[test]
    fn test_fmt_off_on() {
        let source = "# fmt: off\ncode here\n# fmt: on\nmore code\n";
        let collector = CommentCollector::new(source);
        let (comments, ranges) = collector.collect();
        assert_eq!(comments.len(), 2);
        assert_eq!(ranges.len(), 1);
        assert!(ranges[0].end_byte.is_some());
    }

    #[test]
    fn test_fmt_off_no_on() {
        let source = "# fmt: off\nrest of file\n";
        let collector = CommentCollector::new(source);
        let (_, ranges) = collector.collect();
        assert_eq!(ranges.len(), 1);
        assert!(ranges[0].end_byte.is_none());
    }

    #[test]
    fn test_interpolation_not_comment() {
        let source = "\"hello #{name}\"\n# real comment\n";
        let collector = CommentCollector::new(source);
        let (comments, _) = collector.collect();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].kind, CommentKind::Line);
    }
}
