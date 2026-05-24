//! LSP completion — phase 1B per `docs/requirements/tier3_01_lsp.md`.
//!
//! Two trigger contexts in v1:
//!
//! 1. **Word-start** — the cursor is inside (or at the end of) a
//!    bare identifier. Returns every visible symbol whose name shares
//!    the typed prefix: top-level functions / classes / structs /
//!    enums / mixins / modules / constants, plus locals + params in
//!    the enclosing scope (resolved via `node_at_position` on the
//!    last fully-parsed analysis).
//!
//! 2. **After `.`** — the cursor is one position past a `.` token.
//!    Locates the receiver expression (the AST node ending exactly at
//!    the dot), reads its resolved `Ty`, and returns methods declared
//!    on that type plus fields when the type is a class/struct.
//!
//! What's deliberately out of scope here (deferred to phase 2 per
//! spec §5.4):
//!
//! - After-`(` / after-`,` argument-position completion + signature
//!   help (signature help is its own module).
//! - Inside `include` mixin-name completion.
//! - Fuzzy / case-insensitive ranking — the client does its own
//!   fuzzy match; we just contribute the candidate list.
//! - Auto-import suggestions.
//!
//! The function is pure (`fn completions(...) -> Vec<CompletionItem>`)
//! and consumes the existing `AnalysisResult` so the LSP handler can
//! call it without re-running the pipeline.

use lsp_types::{CompletionItem, CompletionItemKind, InsertTextFormat, Position};
use riven_core::hir::nodes::{DefId, HirExpr, HirExprKind, HirItem, HirProgram, HirStatement};
use riven_core::hir::types::Ty;
use riven_core::lexer::token::Span;
use riven_core::resolve::symbols::{DefKind, Definition, SymbolTable};

use crate::analysis::AnalysisResult;

/// Top-level entry. Returns the completion list for the cursor at
/// `position`. `_trigger` is the LSP-reported trigger character — we
/// don't need it because we inspect the source ourselves (more
/// reliable when the client triggers without one, e.g. ctrl-space).
pub fn completions(
    result: &AnalysisResult,
    position: Position,
    _trigger: Option<char>,
) -> Vec<CompletionItem> {
    let Some(symbols) = result.symbols.as_ref() else {
        return Vec::new();
    };
    let byte_offset = result.line_index.byte_offset_of(position);
    let source = result.source.as_str();

    let ctx = classify_context(source, byte_offset);

    match ctx {
        Context::AfterDot { dot_offset, prefix } => {
            after_dot_completions(result, symbols, dot_offset, &prefix)
        }
        Context::WordStart { prefix } => {
            word_start_completions(result, symbols, byte_offset, &prefix)
        }
    }
}

// ─── Context classification ────────────────────────────────────────

/// Two trigger shapes covered in v1. `prefix` is the chars typed so
/// far that we filter candidates by.
enum Context {
    AfterDot {
        /// Byte offset of the `.` that triggers method completion.
        dot_offset: usize,
        /// Identifier chars typed after the dot (may be empty when
        /// the user has just pressed `.`).
        prefix: String,
    },
    WordStart {
        prefix: String,
    },
}

/// Decide which context we're in by walking backward from the cursor:
///   1. Skip identifier chars to find where the prefix starts.
///   2. If the char immediately before the prefix is `.`, we're in
///      AfterDot. Otherwise we're typing a bare identifier.
fn classify_context(source: &str, byte_offset: usize) -> Context {
    let bytes = source.as_bytes();
    let cursor = byte_offset.min(bytes.len());

    // Walk back over identifier chars to find the prefix start.
    let mut prefix_start = cursor;
    while prefix_start > 0 && is_ident_byte(bytes[prefix_start - 1]) {
        prefix_start -= 1;
    }
    let prefix = source[prefix_start..cursor].to_string();

    if prefix_start > 0 && bytes[prefix_start - 1] == b'.' {
        Context::AfterDot {
            dot_offset: prefix_start - 1,
            prefix,
        }
    } else {
        Context::WordStart { prefix }
    }
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

// ─── Word-start completion ─────────────────────────────────────────

/// Walk the symbol table for every top-level + in-scope name whose
/// identifier shares `prefix`. For top-level fns/classes/etc. we
/// always include them; for locals/params we only include those whose
/// definition site precedes the cursor (so we don't suggest a name
/// the user hasn't typed yet).
fn word_start_completions(
    result: &AnalysisResult,
    symbols: &SymbolTable,
    byte_offset: usize,
    prefix: &str,
) -> Vec<CompletionItem> {
    let mut out: Vec<CompletionItem> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for def in symbols.iter() {
        // Skip synth state-machine helper names (`__HandlerFuture`,
        // `__AddOneFuture`, …) and the `Self` type-alias the resolver
        // pushes into every class scope.
        if def.name.starts_with("__") || def.name == "Self" {
            continue;
        }
        if !matches_prefix(&def.name, prefix) {
            continue;
        }
        // For locals / params / let-bindings, require the def site to
        // precede the cursor. Without this guard the resolver's
        // top-down walk would surface a `let bar` written below the
        // cursor as a valid candidate.
        if is_local_kind(&def.kind) && !span_precedes(&def.span, byte_offset) {
            continue;
        }
        if !seen.insert(def.name.clone()) {
            continue;
        }
        let kind = item_kind_for(&def.kind);
        let detail = detail_for(def, symbols);
        out.push(CompletionItem {
            label: def.name.clone(),
            kind: Some(kind),
            detail,
            sort_text: Some(sort_text(&def.name, prefix)),
            insert_text: Some(def.name.clone()),
            insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
            ..Default::default()
        });
    }

    // Built-in keywords — ranked low per spec §5.4. Only include
    // when the prefix actually shares an initial letter (saves the
    // client a sort pass when the user is typing a name).
    for kw in KEYWORDS {
        if !matches_prefix(kw, prefix) {
            continue;
        }
        if !seen.insert(kw.to_string()) {
            continue;
        }
        out.push(CompletionItem {
            label: kw.to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            detail: Some("keyword".to_string()),
            // Sort keywords AFTER user identifiers — prefix `z` so
            // they end up last among same-prefix candidates.
            sort_text: Some(format!("z{}", kw)),
            insert_text: Some(kw.to_string()),
            insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
            ..Default::default()
        });
    }

    // Silence the `result` lint — we don't use the analysis program
    // here but the signature carries it so future after-`(` /
    // include-arg phases stay backward-compatible.
    let _ = result;
    out
}

// ─── After-dot completion ──────────────────────────────────────────

/// Find the receiver expression ending at `dot_offset - 1`, look up
/// its `Ty`, and enumerate methods on that type (plus fields when the
/// type is a class/struct). Falls back to an empty list when the
/// receiver can't be resolved — better than guessing wrong methods.
fn after_dot_completions(
    result: &AnalysisResult,
    symbols: &SymbolTable,
    dot_offset: usize,
    prefix: &str,
) -> Vec<CompletionItem> {
    let Some(program) = result.program.as_ref() else {
        return Vec::new();
    };

    // The receiver is the node whose span ENDS at `dot_offset`. Walk
    // the HIR for the deepest expression matching that end position.
    let Some(receiver_ty) = receiver_type_ending_at(program, symbols, dot_offset) else {
        return Vec::new();
    };

    let mut out: Vec<CompletionItem> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Methods declared on a class/struct/enum type — walk every
    // Method def whose parent matches the receiver type's class.
    if let Some(class_name) = class_name_of(&receiver_ty) {
        for def in symbols.iter() {
            let DefKind::Method { parent, signature } = &def.kind else {
                continue;
            };
            let Some(parent_def) = symbols.get(*parent) else {
                continue;
            };
            if parent_def.name != class_name {
                continue;
            }
            if !matches_prefix(&def.name, prefix) {
                continue;
            }
            if !seen.insert(def.name.clone()) {
                continue;
            }
            out.push(CompletionItem {
                label: def.name.clone(),
                kind: Some(CompletionItemKind::METHOD),
                detail: Some(format_signature(&def.name, signature)),
                sort_text: Some(sort_text(&def.name, prefix)),
                insert_text: Some(def.name.clone()),
                insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
                ..Default::default()
            });
        }

        // Fields on classes / structs — walk Field defs with matching
        // parent. Stops short for enums (their "fields" are variant
        // payloads, surfaced via match instead).
        for def in symbols.iter() {
            let DefKind::Field { parent, ty, .. } = &def.kind else {
                continue;
            };
            let Some(parent_def) = symbols.get(*parent) else {
                continue;
            };
            if parent_def.name != class_name {
                continue;
            }
            if !matches_prefix(&def.name, prefix) {
                continue;
            }
            if !seen.insert(def.name.clone()) {
                continue;
            }
            out.push(CompletionItem {
                label: def.name.clone(),
                kind: Some(CompletionItemKind::FIELD),
                detail: Some(format!("{}: {}", def.name, ty)),
                sort_text: Some(sort_text(&def.name, prefix)),
                insert_text: Some(def.name.clone()),
                insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
                ..Default::default()
            });
        }
    }

    // Module-path completion: `std.io.<cursor>` should offer the
    // module's items. The receiver was resolved as a VarRef whose
    // DefKind::Module carries the items list.
    if let Some(items) = module_items_of(symbols, &receiver_ty) {
        for &item_id in items {
            let Some(def) = symbols.get(item_id) else {
                continue;
            };
            if def.name.starts_with("__") {
                continue;
            }
            if !matches_prefix(&def.name, prefix) {
                continue;
            }
            if !seen.insert(def.name.clone()) {
                continue;
            }
            out.push(CompletionItem {
                label: def.name.clone(),
                kind: Some(item_kind_for(&def.kind)),
                detail: detail_for(def, symbols),
                sort_text: Some(sort_text(&def.name, prefix)),
                insert_text: Some(def.name.clone()),
                insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
                ..Default::default()
            });
        }
    }

    out
}

/// Find the deepest HIR expression whose span ends at `end`. Used to
/// recover the receiver of `<expr>.`. Returns the expression's `Ty`
/// resolved against the same symbol table the rest of the analysis
/// used — so `(foo)` parens, `obj.field.field`, method-call chains,
/// etc. all resolve through the regular type-inference path.
fn receiver_type_ending_at(program: &HirProgram, symbols: &SymbolTable, end: usize) -> Option<Ty> {
    let mut finder = ReceiverFinder {
        target: end,
        best: None,
    };
    finder.visit_program(program);
    finder.best.and_then(|expr| ty_of_expr(&expr, symbols))
}

fn ty_of_expr(expr: &HirExpr, symbols: &SymbolTable) -> Option<Ty> {
    // Prefer the expression's own `ty` (filled in by typeck) — if
    // it's `Ty::Infer(_)` we fall back to a symbol-table lookup
    // for the bare VarRef case (a let-binding the inferencer
    // hasn't pinned yet still has its def's `ty` in the symbol
    // table).
    let ty = expr.ty.clone();
    match &ty {
        Ty::Infer(_) => {
            if let HirExprKind::VarRef(def_id) = &expr.kind {
                return symbols.def_ty(*def_id);
            }
            Some(ty)
        }
        _ => Some(ty),
    }
}

/// Pulls the class/struct/enum name out of a `Ty`. Peels `Ref` /
/// `RefMut` / aliases so methods on a `&Bencher` complete just like
/// methods on `Bencher` would.
fn class_name_of(ty: &Ty) -> Option<String> {
    let mut cur = ty;
    loop {
        match cur {
            Ty::Class { name, .. } => return Some(name.clone()),
            Ty::Struct { name, .. } => return Some(name.clone()),
            Ty::Enum { name, .. } => return Some(name.clone()),
            Ty::Ref(inner)
            | Ty::RefMut(inner)
            | Ty::RefLifetime(_, inner)
            | Ty::RefMutLifetime(_, inner) => cur = inner,
            Ty::Alias { target, .. } => cur = target,
            Ty::Newtype { inner, .. } => cur = inner,
            _ => return None,
        }
    }
}

fn module_items_of<'a>(symbols: &'a SymbolTable, ty: &Ty) -> Option<&'a Vec<DefId>> {
    // Module receivers come through as a class-shaped Ty in some
    // call sites; walking the symbol table by class_name finds them.
    let name = class_name_of(ty)?;
    for def in symbols.iter() {
        if def.name == name {
            if let DefKind::Module { items } = &def.kind {
                return Some(items);
            }
        }
    }
    None
}

// ─── Walker for the receiver-finder ────────────────────────────────

struct ReceiverFinder {
    target: usize,
    best: Option<HirExpr>,
}

impl ReceiverFinder {
    fn visit_program(&mut self, program: &HirProgram) {
        for item in &program.items {
            self.visit_item(item);
        }
    }

    fn visit_item(&mut self, item: &HirItem) {
        match item {
            HirItem::Function(func) => self.visit_expr(&func.body),
            HirItem::Class(class) => {
                for method in &class.methods {
                    self.visit_expr(&method.body);
                }
            }
            _ => {}
        }
    }

    fn visit_expr(&mut self, expr: &HirExpr) {
        if expr.span.end == self.target {
            self.best = Some(expr.clone());
        }
        // Recurse so an inner expression whose span end matches wins
        // over an outer expression whose span end also matches.
        match &expr.kind {
            HirExprKind::FieldAccess { object, .. } => self.visit_expr(object),
            HirExprKind::MethodCall { object, args, .. } => {
                self.visit_expr(object);
                for a in args {
                    self.visit_expr(a);
                }
            }
            HirExprKind::FnCall { args, .. } => {
                for a in args {
                    self.visit_expr(a);
                }
            }
            HirExprKind::BinaryOp { left, right, .. } => {
                self.visit_expr(left);
                self.visit_expr(right);
            }
            HirExprKind::UnaryOp { operand, .. } => self.visit_expr(operand),
            HirExprKind::Block(stmts, tail) => {
                for stmt in stmts {
                    match stmt {
                        HirStatement::Let { value, .. } => {
                            if let Some(v) = value {
                                self.visit_expr(v);
                            }
                        }
                        HirStatement::Expr(e) => self.visit_expr(e),
                    }
                }
                if let Some(t) = tail {
                    self.visit_expr(t);
                }
            }
            _ => {}
        }
    }
}

// ─── Symbol → CompletionItem mapping ───────────────────────────────

fn item_kind_for(kind: &DefKind) -> CompletionItemKind {
    match kind {
        DefKind::Function { .. } | DefKind::OverloadSet { .. } => CompletionItemKind::FUNCTION,
        DefKind::Method { .. } => CompletionItemKind::METHOD,
        DefKind::Class { .. } => CompletionItemKind::CLASS,
        DefKind::Struct { .. } => CompletionItemKind::STRUCT,
        DefKind::Enum { .. } => CompletionItemKind::ENUM,
        DefKind::EnumVariant { .. } => CompletionItemKind::ENUM_MEMBER,
        DefKind::Trait { .. } => CompletionItemKind::INTERFACE,
        DefKind::TypeAlias { .. } | DefKind::Newtype { .. } | DefKind::TypeParam { .. } => {
            CompletionItemKind::TYPE_PARAMETER
        }
        DefKind::ConstParam { .. } | DefKind::Const { .. } => CompletionItemKind::CONSTANT,
        DefKind::Module { .. } => CompletionItemKind::MODULE,
        DefKind::Field { .. } => CompletionItemKind::FIELD,
        DefKind::Variable { .. } | DefKind::Param { .. } | DefKind::SelfValue { .. } => {
            CompletionItemKind::VARIABLE
        }
    }
}

fn detail_for(def: &Definition, symbols: &SymbolTable) -> Option<String> {
    match &def.kind {
        DefKind::Function { signature } | DefKind::Method { signature, .. } => {
            Some(format_signature(&def.name, signature))
        }
        DefKind::OverloadSet { candidates } => Some(format!("{} overloads", candidates.len())),
        DefKind::Variable { ty, .. } | DefKind::Param { ty, .. } | DefKind::SelfValue { ty } => {
            Some(format!("{}: {}", def.name, ty))
        }
        DefKind::Const { ty } | DefKind::ConstParam { ty } => Some(format!("{}: {}", def.name, ty)),
        DefKind::Class { .. } => Some(format!("class {}", def.name)),
        DefKind::Struct { .. } => Some(format!("struct {}", def.name)),
        DefKind::Enum { .. } => Some(format!("enum {}", def.name)),
        DefKind::Trait { .. } => Some(format!("mixin {}", def.name)),
        DefKind::TypeAlias { target } => Some(format!("alias {} = {}", def.name, target)),
        DefKind::Newtype { inner } => Some(format!("newtype {}({})", def.name, inner)),
        DefKind::Module { .. } => Some(format!("module {}", def.name)),
        DefKind::Field { ty, .. } => Some(format!("{}: {}", def.name, ty)),
        DefKind::EnumVariant { parent, .. } => symbols
            .get(*parent)
            .map(|p| format!("{}.{}", p.name, def.name)),
        DefKind::TypeParam { .. } => Some(format!("typeparam {}", def.name)),
    }
}

fn format_signature(name: &str, sig: &riven_core::resolve::symbols::FnSignature) -> String {
    let params: Vec<String> = sig
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name, p.ty))
        .collect();
    let ret = if matches!(sig.return_ty, Ty::Unit) {
        String::new()
    } else {
        format!(" -> {}", sig.return_ty)
    };
    format!("def {}({}){}", name, params.join(", "), ret)
}

fn matches_prefix(name: &str, prefix: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }
    name.starts_with(prefix)
}

fn is_local_kind(kind: &DefKind) -> bool {
    matches!(
        kind,
        DefKind::Variable { .. } | DefKind::Param { .. } | DefKind::SelfValue { .. }
    )
}

fn span_precedes(span: &Span, byte_offset: usize) -> bool {
    span.end <= byte_offset
}

/// Sort key per spec §5.4: exact-prefix candidates rank first, then
/// case-insensitive prefix matches, then everything else. We don't
/// implement substring matching here — clients fuzzy-match
/// themselves; we just give them the candidate list.
fn sort_text(name: &str, prefix: &str) -> String {
    if prefix.is_empty() {
        return format!("m{}", name);
    }
    if name.starts_with(prefix) {
        format!("a{}", name)
    } else if name
        .to_ascii_lowercase()
        .starts_with(&prefix.to_ascii_lowercase())
    {
        format!("b{}", name)
    } else {
        format!("m{}", name)
    }
}

// ─── Built-in keywords (spec §5.4 lowest-rank fallback) ────────────

const KEYWORDS: &[&str] = &[
    "def", "class", "struct", "enum", "mixin", "module", "lib", "if", "elsif", "else", "end", "do",
    "loop", "while", "for", "in", "return", "break", "continue", "match", "let", "var", "include",
    "use", "as", "async", "await", "self", "Self", "true", "false", "and", "or", "not", "yield",
];
