/// Comment collection and attachment for the Riven formatter.
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
pub fn collect_node_spans(program: &crate::parser::ast::Program) -> Vec<(usize, usize)> {
    use crate::parser::ast::*;
    let mut spans = Vec::new();

    fn add_span(spans: &mut Vec<(usize, usize)>, span: &Span) {
        spans.push((span.start, span.end));
    }

    fn visit_type_expr(spans: &mut Vec<(usize, usize)>, ty: &TypeExpr) {
        match ty {
            TypeExpr::Named(tp) => add_span(spans, &tp.span),
            TypeExpr::Reference { span, inner, .. } => {
                add_span(spans, span);
                visit_type_expr(spans, inner);
            }
            TypeExpr::Tuple { span, elements, .. } => {
                add_span(spans, span);
                for e in elements {
                    visit_type_expr(spans, e);
                }
            }
            TypeExpr::Array { span, element, .. } => {
                add_span(spans, span);
                visit_type_expr(spans, element);
            }
            TypeExpr::Function {
                span,
                params,
                return_type,
            } => {
                add_span(spans, span);
                for p in params {
                    visit_type_expr(spans, p);
                }
                visit_type_expr(spans, return_type);
            }
            TypeExpr::ImplTrait { span, .. }
            | TypeExpr::DynTrait { span, .. }
            | TypeExpr::Never { span }
            | TypeExpr::Inferred { span } => add_span(spans, span),
            TypeExpr::RawPointer { span, inner, .. } => {
                add_span(spans, span);
                visit_type_expr(spans, inner);
            }
        }
    }

    fn visit_pattern(spans: &mut Vec<(usize, usize)>, pat: &Pattern) {
        match pat {
            Pattern::Literal { span, expr } => {
                add_span(spans, span);
                visit_expr(spans, expr);
            }
            Pattern::Identifier { span, .. }
            | Pattern::Wildcard { span }
            | Pattern::Rest { span }
            | Pattern::Ref { span, .. } => add_span(spans, span),
            Pattern::Tuple { span, elements } => {
                add_span(spans, span);
                for e in elements {
                    visit_pattern(spans, e);
                }
            }
            Pattern::Enum { span, fields, .. } => {
                add_span(spans, span);
                for f in fields {
                    visit_pattern(spans, f);
                }
            }
            Pattern::Struct { span, fields, .. } => {
                add_span(spans, span);
                for f in fields {
                    add_span(spans, &f.span);
                    visit_pattern(spans, &f.pattern);
                }
            }
            Pattern::Or { span, patterns } => {
                add_span(spans, span);
                for p in patterns {
                    visit_pattern(spans, p);
                }
            }
        }
    }

    fn visit_expr(spans: &mut Vec<(usize, usize)>, expr: &Expr) {
        add_span(spans, &expr.span);
        match &expr.kind {
            ExprKind::BinaryOp { left, right, .. } => {
                visit_expr(spans, left);
                visit_expr(spans, right);
            }
            ExprKind::UnaryOp { operand, .. } => visit_expr(spans, operand),
            ExprKind::Borrow(e) | ExprKind::BorrowMut(e) | ExprKind::Try(e) => visit_expr(spans, e),
            ExprKind::FieldAccess { object, .. } | ExprKind::SafeNav { object, .. } => {
                visit_expr(spans, object)
            }
            ExprKind::MethodCall {
                object,
                args,
                block,
                ..
            } => {
                visit_expr(spans, object);
                for a in args {
                    visit_expr(spans, a);
                }
                if let Some(b) = block {
                    visit_expr(spans, b);
                }
            }
            ExprKind::SafeNavCall { object, args, .. } => {
                visit_expr(spans, object);
                for a in args {
                    visit_expr(spans, a);
                }
            }
            ExprKind::Call {
                callee,
                args,
                block,
            } => {
                visit_expr(spans, callee);
                for a in args {
                    visit_expr(spans, a);
                }
                if let Some(b) = block {
                    visit_expr(spans, b);
                }
            }
            ExprKind::Index { object, index } => {
                visit_expr(spans, object);
                visit_expr(spans, index);
            }
            ExprKind::ClosureCall { callee, args } => {
                visit_expr(spans, callee);
                for a in args {
                    visit_expr(spans, a);
                }
            }
            ExprKind::Assign { target, value } | ExprKind::CompoundAssign { target, value, .. } => {
                visit_expr(spans, target);
                visit_expr(spans, value);
            }
            ExprKind::If(if_expr) => {
                visit_expr(spans, &if_expr.condition);
                visit_block(spans, &if_expr.then_body);
                for elsif in &if_expr.elsif_clauses {
                    add_span(spans, &elsif.span);
                    visit_expr(spans, &elsif.condition);
                    visit_block(spans, &elsif.body);
                }
                if let Some(else_body) = &if_expr.else_body {
                    visit_block(spans, else_body);
                }
            }
            ExprKind::IfLet(if_let) => {
                visit_pattern(spans, &if_let.pattern);
                visit_expr(spans, &if_let.value);
                visit_block(spans, &if_let.then_body);
                if let Some(else_body) = &if_let.else_body {
                    visit_block(spans, else_body);
                }
            }
            ExprKind::Match(match_expr) => {
                visit_expr(spans, &match_expr.subject);
                for arm in &match_expr.arms {
                    add_span(spans, &arm.span);
                    visit_pattern(spans, &arm.pattern);
                    if let Some(g) = &arm.guard {
                        visit_expr(spans, g);
                    }
                    match &arm.body {
                        MatchArmBody::Expr(e) => visit_expr(spans, e),
                        MatchArmBody::Block(b) => visit_block(spans, b),
                    }
                }
            }
            ExprKind::While(w) => {
                visit_expr(spans, &w.condition);
                visit_block(spans, &w.body);
            }
            ExprKind::WhileLet(wl) => {
                visit_pattern(spans, &wl.pattern);
                visit_expr(spans, &wl.value);
                visit_block(spans, &wl.body);
            }
            ExprKind::For(f) => {
                visit_pattern(spans, &f.pattern);
                visit_expr(spans, &f.iterable);
                visit_block(spans, &f.body);
            }
            ExprKind::Loop(l) => visit_block(spans, &l.body),
            ExprKind::Block(b) => visit_block(spans, b),
            ExprKind::Closure(c) => {
                for p in &c.params {
                    add_span(spans, &p.span);
                }
                match &c.body {
                    ClosureBody::Expr(e) => visit_expr(spans, e),
                    ClosureBody::Block(b) => visit_block(spans, b),
                }
            }
            ExprKind::Range { start, end, .. } => {
                if let Some(s) = start {
                    visit_expr(spans, s);
                }
                if let Some(e) = end {
                    visit_expr(spans, e);
                }
            }
            ExprKind::ArrayLiteral(elems) => {
                for e in elems {
                    visit_expr(spans, e);
                }
            }
            ExprKind::ArrayFill { value, count } => {
                visit_expr(spans, value);
                visit_expr(spans, count);
            }
            ExprKind::TupleLiteral(elems) => {
                for e in elems {
                    visit_expr(spans, e);
                }
            }
            ExprKind::Return(Some(e)) | ExprKind::Break(Some(e)) => visit_expr(spans, e),
            ExprKind::Yield(exprs) => {
                for e in exprs {
                    visit_expr(spans, e);
                }
            }
            ExprKind::MacroCall { args, .. } => {
                for a in args {
                    visit_expr(spans, a);
                }
            }
            ExprKind::Cast { expr, .. } => visit_expr(spans, expr),
            ExprKind::EnumVariant { args, .. } => {
                for a in args {
                    add_span(spans, &a.span);
                    visit_expr(spans, &a.value);
                }
            }
            ExprKind::UnsafeBlock(b) => visit_block(spans, b),
            _ => {}
        }
    }

    fn visit_block(spans: &mut Vec<(usize, usize)>, block: &Block) {
        add_span(spans, &block.span);
        for stmt in &block.statements {
            match stmt {
                Statement::Let(l) => {
                    add_span(spans, &l.span);
                    visit_pattern(spans, &l.pattern);
                    if let Some(ty) = &l.type_annotation {
                        visit_type_expr(spans, ty);
                    }
                    if let Some(val) = &l.value {
                        visit_expr(spans, val);
                    }
                }
                Statement::Expression(e) => visit_expr(spans, e),
            }
        }
    }

    fn visit_func(spans: &mut Vec<(usize, usize)>, func: &FuncDef) {
        add_span(spans, &func.span);
        for p in &func.params {
            add_span(spans, &p.span);
            visit_type_expr(spans, &p.type_expr);
        }
        if let Some(rt) = &func.return_type {
            visit_type_expr(spans, rt);
        }
        visit_block(spans, &func.body);
    }

    fn visit_item(spans: &mut Vec<(usize, usize)>, item: &TopLevelItem) {
        match item {
            TopLevelItem::Module(m) => {
                add_span(spans, &m.span);
                for i in &m.items {
                    visit_item(spans, i);
                }
            }
            TopLevelItem::Class(c) => {
                add_span(spans, &c.span);
                for f in &c.fields {
                    add_span(spans, &f.span);
                }
                for m in &c.methods {
                    visit_func(spans, m);
                }
            }
            TopLevelItem::Struct(s) => {
                add_span(spans, &s.span);
                for f in &s.fields {
                    add_span(spans, &f.span);
                }
            }
            TopLevelItem::Enum(e) => {
                add_span(spans, &e.span);
                for v in &e.variants {
                    add_span(spans, &v.span);
                }
            }
            TopLevelItem::Trait(t) => {
                add_span(spans, &t.span);
                for ti in &t.items {
                    match ti {
                        TraitItem::AssocType { span, .. } => add_span(spans, span),
                        TraitItem::MethodSig(ms) => add_span(spans, &ms.span),
                        TraitItem::DefaultMethod(f) => visit_func(spans, f),
                    }
                }
            }
            TopLevelItem::Impl(imp) => {
                add_span(spans, &imp.span);
                for ii in &imp.items {
                    match ii {
                        ImplItem::AssocType { span, .. } => add_span(spans, span),
                        ImplItem::Method(f) => visit_func(spans, f),
                    }
                }
            }
            TopLevelItem::Function(f) => visit_func(spans, f),
            TopLevelItem::Use(u) => add_span(spans, &u.span),
            TopLevelItem::TypeAlias(ta) => {
                add_span(spans, &ta.span);
                visit_type_expr(spans, &ta.type_expr);
            }
            TopLevelItem::Newtype(nt) => {
                add_span(spans, &nt.span);
                visit_type_expr(spans, &nt.inner_type);
            }
            TopLevelItem::Const(c) => {
                add_span(spans, &c.span);
                visit_type_expr(spans, &c.type_expr);
                visit_expr(spans, &c.value);
            }
            TopLevelItem::Lib(l) => {
                add_span(spans, &l.span);
                for f in &l.functions {
                    add_span(spans, &f.span);
                }
            }
            TopLevelItem::Extern(e) => {
                add_span(spans, &e.span);
                for f in &e.functions {
                    add_span(spans, &f.span);
                }
            }
        }
    }

    add_span(&mut spans, &program.span);
    for item in &program.items {
        visit_item(&mut spans, item);
    }

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
