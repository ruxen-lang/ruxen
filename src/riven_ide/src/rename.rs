//! LSP `textDocument/prepareRename` + `textDocument/rename` —
//! Wave-2 capability per `docs/requirements/tier3_01_lsp.md` §5.13.
//!
//! Two entry points share a resolver:
//!
//! * `prepare_rename` — given a cursor position, return `Some(range)`
//!   covering JUST the identifier the user would be replacing (so the
//!   editor pops up a textbox over the name, not the entire call
//!   expression). Returns `None` for unrenamable nodes (whitespace,
//!   literals, builtins, `self` / `Self`, synthetic `__*` defs).
//! * `rename` — same resolution path, then iterate every span recorded
//!   in `UseIndex.uses[def_id]`, narrow each from "whole expression"
//!   to "just the identifier" by source-text scan, and emit a
//!   `WorkspaceEdit` with one `TextEdit` per occurrence.
//!
//! ## Why we narrow spans by source-text scan
//!
//! `UseIndex` records use-sites at `HirExpr.span` granularity, which
//! for a `FnCall` covers the whole `add(1, 2)` and for a `MethodCall`
//! covers the whole `c.bump(3)`. Renaming literally would clobber the
//! arguments. The HIR doesn't carry a separate name-only span for the
//! callee, so we slice the source over the recorded span, find the
//! identifier by name with word-boundary checks, and emit a
//! tightened `TextEdit`.
//!
//! For a `VarRef`, `let` decl, or `class` decl the recorded span is
//! often wider than the identifier too (`let x = 42` covers the whole
//! statement). The same source-scan path narrows it.
//!
//! ## Validation rules (per Wave-2 task brief)
//!
//! * **Value bindings** (locals, params, functions, methods, fields,
//!   consts, modules): `^[a-z_][a-zA-Z0-9_]*$`.
//! * **Type bindings** (classes, structs, enums, traits, newtypes,
//!   aliases, type params, enum variants): `^[A-Z][a-zA-Z0-9_]*$`.
//! * Reject Riven keywords regardless of category.
//! * Reject `__`-prefixed names (compiler-internal namespace).
//!
//! ## Exclusions
//!
//! We return `None` for both `prepare_rename` and `rename` when:
//!
//! * The cursor is on a builtin / stdlib name whose def-site span is
//!   synthetic (`start == 0 && end == 0`). The `library/std/io/...`
//!   .rvn file the bootstrap loader merged in isn't currently tracked
//!   per-file in `analyze()`, so a rename would either no-op or
//!   produce a spurious edit at offset 0.
//! * The cursor is on `self` or `Self` — these are language keywords,
//!   not user-controllable bindings.
//! * The resolved def's name starts with `__` — those are compiler
//!   shims (`__drop`, `__poll`, …) and renaming them would break the
//!   bootstrap merge.

use std::collections::HashMap;

use lsp_types::{Position, Range, TextEdit, Url, WorkspaceEdit};
use riven_core::hir::nodes::{
    DefId, HirExpr, HirExprKind, HirImplItem, HirItem, HirProgram, HirStatement, UNRESOLVED_DEF,
};
use riven_core::hir::types::Ty;
use riven_core::lexer::token::Span;
use riven_core::resolve::symbols::{DefKind, SymbolTable};

use crate::analysis::AnalysisResult;
use crate::node_finder::{node_at_position, NodeAtPosition};

// ─── Public surfaces ────────────────────────────────────────────────

/// Return the LSP range the editor should put a rename popup over, or
/// `None` if the cursor isn't on a renamable identifier.
///
/// The returned range covers JUST the identifier (e.g. `bump` inside
/// `c.bump(3)`), not the surrounding expression.
pub fn prepare_rename(result: &AnalysisResult, position: Position) -> Option<Range> {
    let program = result.program.as_ref()?;
    let symbols = result.symbols.as_ref()?;
    let byte_offset = result.line_index.byte_offset_of(position);

    // The user has to actually be pointing at an identifier — not just
    // somewhere inside an enclosing definition's span. Reading the
    // word under the cursor up-front gates everything that follows.
    let word_span = word_under_cursor(&result.source, byte_offset)?;
    let word = &result.source[word_span.0..word_span.1];

    // Reject cursor on `self` / `Self` keywords. The resolver maps
    // `self` to a `SelfValue` DefKind which is otherwise indistinguishable
    // from a renamable local in the use-index.
    if word == "self" || word == "Self" {
        return None;
    }

    // Reject keywords outright — typing on `def` / `let` is not a rename.
    if is_reserved_keyword(word) {
        return None;
    }

    let def_id = match node_at_position(program, byte_offset) {
        Some(node) => def_id_for_node(&node, program, symbols, byte_offset)?,
        // Cursor on a name whose enclosing AST node has no def slot
        // (e.g. on a class declaration name — node_finder doesn't set
        // a `Definition` for the class header itself). Fall back to a
        // symbol-table scan by name.
        None => find_def_by_word(symbols, word, byte_offset)?,
    };
    let def = symbols.get(def_id)?;

    // Synthetic / compiler-internal / not-in-this-file defs aren't renamable.
    if !is_renamable_def(def_id, &def.name, &def.span, &result.source) {
        return None;
    }

    // The cursor's word MUST match the def's name. If they differ,
    // we landed on a use-site of one def but resolved to a different
    // one (e.g. the cursor is inside the body of a function the
    // node_finder fell back to). Refuse — renaming would silently
    // rewrite the wrong identifier.
    if word != def.name {
        // Last-ditch: maybe the symbol-table-by-word lookup finds a
        // def whose name matches.
        let alt = find_def_by_word(symbols, word, byte_offset)?;
        let alt_def = symbols.get(alt)?;
        if !is_renamable_def(alt, &alt_def.name, &alt_def.span, &result.source) {
            return None;
        }
        return Some(word_span_to_range(result, word_span));
    }

    Some(word_span_to_range(result, word_span))
}

/// Build a `WorkspaceEdit` that renames every occurrence of the symbol
/// under the cursor to `new_name`. Returns `None` when the request
/// can't be honoured — invalid identifier, unsupported node, builtin
/// target, etc.
pub fn rename(
    result: &AnalysisResult,
    uri: &Url,
    position: Position,
    new_name: &str,
) -> Option<WorkspaceEdit> {
    let program = result.program.as_ref()?;
    let symbols = result.symbols.as_ref()?;
    let use_index = result.use_index.as_ref()?;
    let byte_offset = result.line_index.byte_offset_of(position);

    // Same identifier-under-cursor gate as `prepare_rename`. Refusing
    // here keeps the two surfaces symmetric — an editor that already
    // got a `Some(range)` from prepare_rename will always get a
    // `Some(WorkspaceEdit)` back, modulo `new_name` validation.
    let word_span = word_under_cursor(&result.source, byte_offset)?;
    let word = &result.source[word_span.0..word_span.1];
    if word == "self" || word == "Self" {
        return None;
    }
    if is_reserved_keyword(word) {
        return None;
    }

    let def_id = match node_at_position(program, byte_offset) {
        Some(node) => {
            let cand = def_id_for_node(&node, program, symbols, byte_offset);
            // If the cursor's word doesn't match the resolved def's
            // name (e.g. node_finder fell back to the enclosing fn),
            // prefer a symbol-table scan keyed on the cursor's word.
            match cand {
                Some(id) => {
                    let name = symbols.get(id).map(|d| d.name.as_str()).unwrap_or("");
                    if name == word {
                        id
                    } else {
                        find_def_by_word(symbols, word, byte_offset)?
                    }
                }
                None => find_def_by_word(symbols, word, byte_offset)?,
            }
        }
        None => find_def_by_word(symbols, word, byte_offset)?,
    };
    let def = symbols.get(def_id)?;

    if !is_renamable_def(def_id, &def.name, &def.span, &result.source) {
        return None;
    }

    // Choose value-vs-type validator from the resolved def kind.
    let kind_class = classify_def_kind(&def.kind);
    if !is_valid_new_name(new_name, kind_class) {
        return None;
    }

    let spans = use_index.uses.get(&def_id)?;
    let mut edits: Vec<TextEdit> = Vec::with_capacity(spans.len());
    let mut seen: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
    for span in spans {
        if is_synthetic(span) {
            continue;
        }
        let Some(name_span) = narrow_to_identifier(&result.source, span, &def.name) else {
            continue;
        };
        // Deduplicate identical narrow ranges (`use_index` can record
        // both a Class def-site span AND a `Construct` use-site that
        // happen to narrow to the same identifier in pathological
        // single-line fixtures).
        let key = (name_span.start as u32, name_span.end as u32);
        if !seen.insert(key) {
            continue;
        }
        let range = result.line_index.span_to_range(&name_span);
        edits.push(TextEdit {
            range,
            new_text: new_name.to_string(),
        });
    }

    if edits.is_empty() {
        return None;
    }

    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    changes.insert(uri.clone(), edits);
    Some(WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    })
}

// ─── Identifier-class validation ────────────────────────────────────

/// Two flavours of identifier shape: value bindings start lowercase,
/// type bindings start uppercase. Enum variants are treated as types
/// (Riven uses PascalCase variants per ruby-naming.spec.md).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum NameClass {
    Value,
    Type,
}

fn classify_def_kind(kind: &DefKind) -> NameClass {
    match kind {
        DefKind::Class { .. }
        | DefKind::Struct { .. }
        | DefKind::Enum { .. }
        | DefKind::EnumVariant { .. }
        | DefKind::Trait { .. }
        | DefKind::TypeAlias { .. }
        | DefKind::Newtype { .. }
        | DefKind::TypeParam { .. } => NameClass::Type,
        DefKind::Variable { .. }
        | DefKind::Function { .. }
        | DefKind::Method { .. }
        | DefKind::Field { .. }
        | DefKind::Param { .. }
        | DefKind::Const { .. }
        | DefKind::ConstParam { .. }
        | DefKind::Module { .. }
        | DefKind::SelfValue { .. } => NameClass::Value,
    }
}

/// Apply the regex-equivalent + keyword + `__`-prefix rules without a
/// regex dependency. Hand-rolled to keep the crate's dep list small.
fn is_valid_new_name(name: &str, class: NameClass) -> bool {
    if name.is_empty() {
        return false;
    }
    if name.starts_with("__") {
        return false;
    }
    if is_reserved_keyword(name) {
        return false;
    }
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    let first_ok = match class {
        NameClass::Value => first.is_ascii_lowercase() || first == '_',
        NameClass::Type => first.is_ascii_uppercase(),
    };
    if !first_ok {
        return false;
    }
    for c in chars {
        if !(c.is_ascii_alphanumeric() || c == '_') {
            return false;
        }
    }
    true
}

/// Full Riven keyword list per the Wave-2 task brief. Kept here rather
/// than reaching into `riven_core::lexer` so the LSP never widens its
/// rejection list silently when the lexer adds a new keyword — that's
/// a deliberate review touchpoint.
fn is_reserved_keyword(name: &str) -> bool {
    matches!(
        name,
        "def"
            | "class"
            | "let"
            | "var"
            | "if"
            | "else"
            | "end"
            | "do"
            | "loop"
            | "while"
            | "for"
            | "in"
            | "return"
            | "break"
            | "continue"
            | "match"
            | "async"
            | "await"
            | "module"
            | "mixin"
            | "include"
            | "use"
            | "as"
            | "lib"
            | "self"
            | "Self"
            | "true"
            | "false"
            | "and"
            | "or"
            | "not"
            | "yield"
    )
}

// ─── DefId extraction (mirrors `highlight.rs`) ──────────────────────

/// Map a `NodeAtPosition` to the def whose use-list we should rewrite.
/// Mirrors `highlight.rs::def_id_for_node` so rename, highlight, and
/// references all agree on resolution — including the
/// `UNRESOLVED_DEF` method-call fallback (`MethodCall.method` is often
/// unresolved at HIR time per `project_riven_mir_two_dispatch_paths.md`).
fn def_id_for_node(
    node: &NodeAtPosition,
    program: &HirProgram,
    symbols: &SymbolTable,
    byte_offset: usize,
) -> Option<DefId> {
    match node {
        NodeAtPosition::VarRef(def_id, _) => Some(*def_id),
        NodeAtPosition::Definition(def_id, _) => Some(*def_id),
        NodeAtPosition::FnCall { callee, .. } => Some(*callee),
        NodeAtPosition::MethodCall { method, .. } => {
            if *method != UNRESOLVED_DEF {
                return Some(*method);
            }
            let (receiver_ty, method_name) = find_method_call_at(program, byte_offset)?;
            resolve_method_def(symbols, &receiver_ty, &method_name)
        }
        NodeAtPosition::FieldAccess { .. } => None,
        NodeAtPosition::TypeRef { .. } => None,
    }
}

/// "Real" defs are renamable. Excludes:
///
/// * `UNRESOLVED_DEF`,
/// * `__`-prefixed compiler internals,
/// * defs whose span sits outside the current source (i.e. the
///   bootstrap loader merged them from another `.rvn` file — Wave 2
///   doesn't do cross-file rename), AND
/// * the all-zero synthetic span (pre-#06.8 builtins like the original
///   `puts`).
fn is_renamable_def(def_id: DefId, name: &str, span: &Span, source: &str) -> bool {
    if def_id == UNRESOLVED_DEF {
        return false;
    }
    if name.starts_with("__") {
        return false;
    }
    if is_synthetic(span) {
        return false;
    }
    // Cross-file source ranges: the bootstrap merge populates defs
    // from stdlib `.rvn` files (e.g. `puts` from io.rvn) into the same
    // symbol table, with their own line numbers — but those bytes
    // aren't in our current source buffer. Rename would either no-op
    // (offsets > source.len()) or produce an edit at an unrelated
    // location. Refuse.
    if span.end > source.len() {
        return false;
    }
    true
}

fn is_synthetic(span: &Span) -> bool {
    span.start == 0 && span.end == 0 && span.line == 0
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// If `byte_offset` is on (or immediately after) an identifier in
/// `source`, return its `(start, end)` byte range. Returns `None`
/// when the cursor is on whitespace, punctuation, or a digit-leading
/// token (we don't recognise numeric literals as identifiers).
fn word_under_cursor(source: &str, byte_offset: usize) -> Option<(usize, usize)> {
    let bytes = source.as_bytes();
    let len = bytes.len();
    if len == 0 {
        return None;
    }
    // Allow the cursor to sit just past the end of an identifier
    // (common when the user types after the last char) by checking
    // byte_offset and byte_offset-1.
    let on_ident_here = byte_offset < len && is_ident_byte(bytes[byte_offset]);
    let on_ident_before = byte_offset > 0 && is_ident_byte(bytes[byte_offset - 1]);
    if !on_ident_here && !on_ident_before {
        return None;
    }
    let mut start = byte_offset.min(len);
    while start > 0 && is_ident_byte(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = byte_offset.min(len);
    if end < len && !is_ident_byte(bytes[end]) && on_ident_before {
        // Cursor is just past the end of the identifier — `end` is
        // already at the boundary, leave it.
    }
    while end < len && is_ident_byte(bytes[end]) {
        end += 1;
    }
    if start == end {
        return None;
    }
    // Reject digit-leading tokens (numeric literals).
    if bytes[start].is_ascii_digit() {
        return None;
    }
    Some((start, end))
}

/// Convert a `(start, end)` byte range into an LSP `Range` via the
/// analysis result's line index.
fn word_span_to_range(result: &AnalysisResult, (start, end): (usize, usize)) -> Range {
    Range {
        start: result.line_index.position_of(start),
        end: result.line_index.position_of(end),
    }
}

/// Symbol-table fallback for cases where `node_at_position` returns
/// `None` or returns a def whose name doesn't match the cursor's
/// word. Picks the def with the tightest non-synthetic span that
/// contains the cursor — and as a final fallback, the first matching
/// real def with a non-synthetic span (for a class-header click,
/// `node_finder` doesn't enter the class but the class def's span
/// does contain the cursor).
fn find_def_by_word(symbols: &SymbolTable, word: &str, byte_offset: usize) -> Option<DefId> {
    let mut best: Option<(DefId, usize)> = None;
    let mut first_real: Option<DefId> = None;
    for def in symbols.iter() {
        if def.name != word {
            continue;
        }
        if def.name.starts_with("__") {
            continue;
        }
        if is_synthetic(&def.span) {
            continue;
        }
        if first_real.is_none() {
            first_real = Some(def.id);
        }
        let span = &def.span;
        if span.start <= byte_offset && byte_offset < span.end {
            let width = span.end - span.start;
            match best {
                None => best = Some((def.id, width)),
                Some((_, prev_w)) if width < prev_w => best = Some((def.id, width)),
                _ => {}
            }
        }
    }
    best.map(|(id, _)| id).or(first_real)
}

// ─── HIR walk for MethodCall fallback (mirrors highlight.rs) ────────

fn find_method_call_at(program: &HirProgram, byte_offset: usize) -> Option<(Ty, String)> {
    let mut finder = MethodCallFinder {
        target: byte_offset,
        result: None,
    };
    finder.visit_program(program);
    finder.result
}

struct MethodCallFinder {
    target: usize,
    result: Option<(Ty, String)>,
}

impl MethodCallFinder {
    fn contains(&self, span: &Span) -> bool {
        span.start <= self.target && self.target < span.end
    }

    fn visit_program(&mut self, program: &HirProgram) {
        for item in &program.items {
            self.visit_item(item);
        }
    }

    fn visit_item(&mut self, item: &HirItem) {
        match item {
            HirItem::Function(f) => self.visit_expr(&f.body),
            HirItem::Class(c) => {
                for m in &c.methods {
                    self.visit_expr(&m.body);
                }
                for b in &c.impl_blocks {
                    for it in &b.items {
                        if let HirImplItem::Method(f) = it {
                            self.visit_expr(&f.body);
                        }
                    }
                }
            }
            HirItem::Struct(s) => {
                for m in &s.methods {
                    self.visit_expr(&m.body);
                }
                for b in &s.impl_blocks {
                    for it in &b.items {
                        if let HirImplItem::Method(f) = it {
                            self.visit_expr(&f.body);
                        }
                    }
                }
            }
            HirItem::Enum(e) => {
                for m in &e.methods {
                    self.visit_expr(&m.body);
                }
                for b in &e.impl_blocks {
                    for it in &b.items {
                        if let HirImplItem::Method(f) = it {
                            self.visit_expr(&f.body);
                        }
                    }
                }
            }
            HirItem::Impl(b) => {
                for it in &b.items {
                    if let HirImplItem::Method(f) = it {
                        self.visit_expr(&f.body);
                    }
                }
            }
            HirItem::Module(m) => {
                for it in &m.items {
                    self.visit_item(it);
                }
            }
            _ => {}
        }
    }

    fn visit_expr(&mut self, expr: &HirExpr) {
        if !self.contains(&expr.span) {
            return;
        }
        match &expr.kind {
            HirExprKind::MethodCall {
                object,
                method_name,
                args,
                block,
                ..
            } => {
                self.result = Some((object.ty.clone(), method_name.clone()));
                self.visit_expr(object);
                for a in args {
                    self.visit_expr(a);
                }
                if let Some(b) = block {
                    self.visit_expr(b);
                }
            }
            HirExprKind::FnCall { args, .. } => {
                for a in args {
                    self.visit_expr(a);
                }
            }
            HirExprKind::FieldAccess { object, .. } => self.visit_expr(object),
            HirExprKind::BinaryOp { left, right, .. } => {
                self.visit_expr(left);
                self.visit_expr(right);
            }
            HirExprKind::UnaryOp { operand, .. } => self.visit_expr(operand),
            HirExprKind::Borrow { expr: inner, .. } => self.visit_expr(inner),
            HirExprKind::Block(stmts, tail) | HirExprKind::UnsafeBlock(stmts, tail) => {
                for stmt in stmts {
                    self.visit_stmt(stmt);
                }
                if let Some(t) = tail {
                    self.visit_expr(t);
                }
            }
            HirExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.visit_expr(cond);
                self.visit_expr(then_branch);
                if let Some(e) = else_branch {
                    self.visit_expr(e);
                }
            }
            HirExprKind::Match { scrutinee, arms } => {
                self.visit_expr(scrutinee);
                for arm in arms {
                    if let Some(g) = &arm.guard {
                        self.visit_expr(g);
                    }
                    self.visit_expr(&arm.body);
                }
            }
            HirExprKind::While { condition, body } => {
                self.visit_expr(condition);
                self.visit_expr(body);
            }
            HirExprKind::For { iterable, body, .. } => {
                self.visit_expr(iterable);
                self.visit_expr(body);
            }
            HirExprKind::Loop { body } => self.visit_expr(body),
            HirExprKind::Assign { target, value, .. }
            | HirExprKind::CompoundAssign { target, value, .. } => {
                self.visit_expr(target);
                self.visit_expr(value);
            }
            HirExprKind::Return(Some(inner)) | HirExprKind::Break(Some(inner)) => {
                self.visit_expr(inner);
            }
            HirExprKind::Closure { body, .. } => self.visit_expr(body),
            HirExprKind::Construct { fields, .. } | HirExprKind::EnumVariant { fields, .. } => {
                for (_, v) in fields {
                    self.visit_expr(v);
                }
            }
            HirExprKind::Tuple(elems) | HirExprKind::ArrayLiteral(elems) => {
                for e in elems {
                    self.visit_expr(e);
                }
            }
            HirExprKind::MapLiteral(pairs) => {
                for (k, v) in pairs {
                    self.visit_expr(k);
                    self.visit_expr(v);
                }
            }
            HirExprKind::Index { object, index } => {
                self.visit_expr(object);
                self.visit_expr(index);
            }
            HirExprKind::Cast { expr: inner, .. } => self.visit_expr(inner),
            HirExprKind::ArrayFill { value, .. } => self.visit_expr(value),
            HirExprKind::Range { start, end, .. } => {
                if let Some(s) = start {
                    self.visit_expr(s);
                }
                if let Some(e) = end {
                    self.visit_expr(e);
                }
            }
            HirExprKind::Interpolation { parts } => {
                use riven_core::hir::nodes::HirInterpolationPart;
                for p in parts {
                    if let HirInterpolationPart::Expr { expr: e, .. } = p {
                        self.visit_expr(e);
                    }
                }
            }
            HirExprKind::MacroCall { args, .. } => {
                for a in args {
                    self.visit_expr(a);
                }
            }
            HirExprKind::VarRef(_)
            | HirExprKind::IntLiteral(_)
            | HirExprKind::FloatLiteral(_)
            | HirExprKind::StringLiteral(_)
            | HirExprKind::BoolLiteral(_)
            | HirExprKind::CharLiteral(_)
            | HirExprKind::UnitLiteral
            | HirExprKind::NullLiteral
            | HirExprKind::Continue
            | HirExprKind::Return(None)
            | HirExprKind::Break(None)
            | HirExprKind::Error => {}
        }
    }

    fn visit_stmt(&mut self, stmt: &HirStatement) {
        match stmt {
            HirStatement::Let { value, .. } => {
                if let Some(v) = value {
                    self.visit_expr(v);
                }
            }
            HirStatement::Expr(e) => self.visit_expr(e),
        }
    }
}

/// Resolve a method DefId from receiver type + method name. Mirrors
/// `highlight.rs::resolve_method_def` + `use_index::resolve_method_def`.
fn resolve_method_def(symbols: &SymbolTable, receiver_ty: &Ty, method_name: &str) -> Option<DefId> {
    let ty = peel_ty(receiver_ty);
    let type_name = match ty {
        Ty::Class { name, .. } | Ty::Struct { name, .. } => name.as_str(),
        _ => return None,
    };
    let type_def_id = lookup_named_type(symbols, type_name)?;
    let methods = match &symbols.get(type_def_id)?.kind {
        DefKind::Class { info } => &info.methods,
        _ => return None,
    };
    methods.iter().copied().find(|id| {
        symbols
            .get(*id)
            .map(|d| d.name == method_name)
            .unwrap_or(false)
    })
}

fn peel_ty(ty: &Ty) -> &Ty {
    let mut cur = ty;
    loop {
        match cur {
            Ty::Ref(inner)
            | Ty::RefMut(inner)
            | Ty::RefLifetime(_, inner)
            | Ty::RefMutLifetime(_, inner)
            | Ty::RawPtr(inner)
            | Ty::RawPtrMut(inner)
            | Ty::Newtype { inner, .. }
            | Ty::Alias { target: inner, .. } => cur = inner,
            _ => return cur,
        }
    }
}

fn lookup_named_type(symbols: &SymbolTable, name: &str) -> Option<DefId> {
    let mut fallback: Option<DefId> = None;
    for def in symbols.iter() {
        if def.name != name {
            continue;
        }
        match def.kind {
            DefKind::Class { .. } | DefKind::Struct { .. } | DefKind::Enum { .. } => {
                return Some(def.id);
            }
            DefKind::Newtype { .. } | DefKind::TypeAlias { .. } | DefKind::Trait { .. } => {
                if fallback.is_none() {
                    fallback = Some(def.id);
                }
            }
            _ => {}
        }
    }
    fallback
}

// ─── Span narrowing ─────────────────────────────────────────────────

/// Slice `source` over `host_span` and return the first occurrence of
/// `name` flanked by non-identifier characters (so we don't match
/// `bump` inside `bumper`). Returns `None` when the span is out of
/// bounds or the name isn't found in it.
///
/// First-occurrence is correct for every kind of use-span we record:
///
/// * **VarRef**: the span equals the identifier — only one occurrence.
/// * **FnCall**: the callee identifier appears at the start; any
///   subsequent occurrence in args is a separately-recorded use.
/// * **MethodCall**: the method identifier appears after the `.`,
///   before the `(` — first occurrence inside the span hits the
///   method name, not the receiver (the receiver is `object.ty`,
///   never a literal `name` token unless the user types
///   `bump.bump(3)` which is a separate VarRef anyway).
/// * **Definition (`let x = …` / `def f` / `class Foo`)**: the name
///   appears at the start of the declaration; the rhs value can't
///   contain a redeclaration of the same name in the same span.
fn narrow_to_identifier(source: &str, host_span: &Span, name: &str) -> Option<Span> {
    if name.is_empty() {
        return None;
    }
    let start = host_span.start.min(source.len());
    let end = host_span.end.min(source.len());
    if start >= end {
        return None;
    }
    let slice = &source[start..end];
    let bytes = slice.as_bytes();
    let name_bytes = name.as_bytes();
    let mut i = 0;
    while i + name_bytes.len() <= bytes.len() {
        if &bytes[i..i + name_bytes.len()] == name_bytes {
            let before_ok = i == 0 || !is_ident_byte(bytes[i - 1]);
            let after_ok =
                i + name_bytes.len() == bytes.len() || !is_ident_byte(bytes[i + name_bytes.len()]);
            if before_ok && after_ok {
                let abs_start = start + i;
                let abs_end = abs_start + name_bytes.len();
                return Some(Span {
                    start: abs_start,
                    end: abs_end,
                    line: host_span.line,
                    column: host_span.column,
                });
            }
        }
        i += 1;
    }
    None
}
