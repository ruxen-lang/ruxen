/// AST-to-Doc conversion for top-level items (classes, structs, enums, traits,
/// impls, functions, modules, constants, type aliases, etc.)
use crate::parser::ast::*;

use super::comments::CommentMap;
use super::doc::*;
use super::format_expr::{format_block_body, format_call_args, format_expr};
use super::format_type::{
    format_generic_params, format_trait_bounds, format_type_expr, format_type_path,
    format_where_clause,
};

// ─── Program ────────────────────────────────────────────────────────

pub fn format_program(program: &Program, comments: &CommentMap) -> Doc {
    let mut parts: Vec<Doc> = Vec::new();
    let mut prev_kind: Option<ItemKind> = None;

    // Collect all comment span starts that we emit, to avoid duplication.
    // Comments attached to the program span are also attached to the first item.
    let mut emitted_comment_spans: std::collections::HashSet<usize> =
        std::collections::HashSet::new();

    // Emit leading comments that appear before all items (program-level only)
    let leading = comments.leading_comments(program.span.start);
    for comment in leading {
        if emitted_comment_spans.insert(comment.span.start) {
            parts.push(format_comment(comment));
            parts.push(hardline());
        }
    }

    for (i, item) in program.items.iter().enumerate() {
        let current_kind = classify_item(item);

        if i > 0 {
            // Insert blank line(s) between items
            let needs_blank = !matches!(
                (&prev_kind, &current_kind),
                (Some(ItemKind::Use), ItemKind::Use)
            );
            if needs_blank {
                parts.push(hardline());
            }
            parts.push(hardline());
        }

        // Leading comments on this item
        let item_span_start = item_span(item).start;
        let item_leading = comments.leading_comments(item_span_start);
        for comment in item_leading {
            if emitted_comment_spans.insert(comment.span.start) {
                parts.push(format_comment(comment));
                parts.push(hardline());
            }
        }

        parts.push(format_top_level_item(item, comments));

        // Trailing comments
        let item_trailing = comments.trailing_comments(item_span_start);
        for comment in item_trailing {
            if emitted_comment_spans.insert(comment.span.start) {
                parts.push(text("  "));
                parts.push(format_comment(comment));
            }
        }

        prev_kind = Some(current_kind);
    }

    // Trailing newline
    parts.push(hardline());

    concat(parts)
}

fn format_comment(comment: &super::comments::Comment) -> Doc {
    match comment.kind {
        super::comments::CommentKind::Line => text(format!("#{}", comment.text)),
        super::comments::CommentKind::Doc => text(format!("## {}", comment.text)),
        super::comments::CommentKind::Block => text(format!("#={}=#", comment.text)),
    }
}

// ─── Item Classification ────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
enum ItemKind {
    Use,
    TypeDef,
    Function,
    Impl,
    Other,
}

fn classify_item(item: &TopLevelItem) -> ItemKind {
    match item {
        TopLevelItem::Use(_) => ItemKind::Use,
        TopLevelItem::Class(_)
        | TopLevelItem::Struct(_)
        | TopLevelItem::Enum(_)
        | TopLevelItem::Mixin(_)
        | TopLevelItem::Module(_)
        | TopLevelItem::TypeAlias(_)
        | TopLevelItem::Newtype(_) => ItemKind::TypeDef,
        TopLevelItem::Function(_) => ItemKind::Function,
        TopLevelItem::Impl(_) => ItemKind::Impl,
        _ => ItemKind::Other,
    }
}

fn item_span(item: &TopLevelItem) -> &crate::lexer::token::Span {
    match item {
        TopLevelItem::Module(m) => &m.span,
        TopLevelItem::Class(c) => &c.span,
        TopLevelItem::Struct(s) => &s.span,
        TopLevelItem::Enum(e) => &e.span,
        TopLevelItem::Mixin(t) => &t.span,
        TopLevelItem::Impl(i) => &i.span,
        TopLevelItem::Function(f) => &f.span,
        TopLevelItem::Use(u) => &u.span,
        TopLevelItem::TypeAlias(ta) => &ta.span,
        TopLevelItem::Newtype(nt) => &nt.span,
        TopLevelItem::Const(c) => &c.span,
        TopLevelItem::Lib(l) => &l.span,
        TopLevelItem::Expr(e) => &e.span,
        TopLevelItem::Alias(a) => &a.span,
    }
}

// ─── Top-Level Item Dispatch ────────────────────────────────────────

fn format_top_level_item(item: &TopLevelItem, comments: &CommentMap) -> Doc {
    match item {
        TopLevelItem::Module(m) => format_module(m, comments),
        TopLevelItem::Class(c) => format_class(c, comments),
        TopLevelItem::Struct(s) => format_struct(s, comments),
        TopLevelItem::Enum(e) => format_enum(e, comments),
        TopLevelItem::Mixin(t) => format_trait(t, comments),
        TopLevelItem::Impl(i) => format_impl(i, comments),
        TopLevelItem::Function(f) => format_prefixed_func(f, comments),
        TopLevelItem::Use(u) => format_use(u),
        TopLevelItem::TypeAlias(ta) => format_type_alias(ta, comments),
        TopLevelItem::Newtype(nt) => format_newtype(nt, comments),
        TopLevelItem::Const(c) => format_const(c, comments),
        TopLevelItem::Lib(l) => format_lib(l, comments),
        // A top-level expression statement (Q23b — test-file
        // `Tester.describe(...) do … end`). Format it exactly like an
        // in-body statement expression so the test file round-trips.
        TopLevelItem::Expr(e) => format_expr(e, comments),
        TopLevelItem::Alias(a) => format_alias(a),
    }
}

/// Format a Ruby `alias new_name old_name` item byte-stably: the keyword, the
/// new name, the old name, single-spaced, no normalization
/// (docs/decisions/alias-keyword.md, D10).
fn format_alias(a: &AliasDef) -> Doc {
    let AliasDef {
        new_name,
        old_name,
        span: _,
    } = a;
    text(format!("alias {} {}", new_name, old_name))
}

/// Check if a single-statement function body is simple enough for inline `{ }` form.
/// Control flow (if, match, while, for, loop), blocks, and closures are NOT simple.
fn is_simple_inline_body(block: &Block) -> bool {
    if block.statements.len() != 1 {
        return false;
    }
    match &block.statements[0] {
        Statement::Let(_) => false,
        Statement::Expression(expr) => is_simple_expr(&expr.kind),
    }
}

fn is_simple_expr(kind: &ExprKind) -> bool {
    !matches!(
        kind,
        ExprKind::If(_)
            | ExprKind::IfLet(_)
            | ExprKind::Match(_)
            | ExprKind::While(_)
            | ExprKind::WhileLet(_)
            | ExprKind::For(_)
            | ExprKind::Loop(_)
            | ExprKind::Block(_)
            | ExprKind::Closure(_)
            | ExprKind::UnsafeBlock(_)
    )
}

// ─── Functions ──────────────────────────────────────────────────────

/// Format a method/function WITH any `##` doc comments (and plain leading
/// comments) attached above it. Top-level items get their leading comments
/// emitted by `format_program`; methods NESTED inside a class/struct/enum/
/// impl/mixin body are formatted by direct `format_func_def` calls that
/// bypass that path, so without this their leading docs were silently
/// dropped (Q23a — `ruxen fmt` wiped every `##` doc on a class method).
/// Mirrors the class-body `lib "..."` FFI-def doc handling in `format_lib`.
pub fn format_func_with_leading_comments(func: &FuncDef, comments: &CommentMap) -> Doc {
    with_leading_comments(func.span.start, format_func_def(func, comments), comments)
}

/// `doc` preceded by the comments that lead the source offset `start`.
fn with_leading_comments(start: usize, doc: Doc, comments: &CommentMap) -> Doc {
    let mut parts: Vec<Doc> = Vec::new();
    for comment in comments.leading_comments(start) {
        parts.push(format_comment(comment));
        parts.push(hardline());
    }
    parts.push(doc);
    concat(parts)
}

/// A function or method in a context with no visibility sections (top level,
/// `extension` bodies): `protected` is written as an inline prefix.
fn format_prefixed_func(func: &FuncDef, comments: &CommentMap) -> Doc {
    let def = format_func_def(func, comments);
    match func.visibility {
        Visibility::Protected => concat(vec![text("protected "), def]),
        Visibility::Public | Visibility::Private => def,
    }
}

/// A `def` without any visibility keyword: type, mixin and module bodies
/// express visibility with section markers, other contexts through
/// `format_prefixed_func`.
pub fn format_func_def(func: &FuncDef, comments: &CommentMap) -> Doc {
    let FuncDef {
        visibility: _,
        is_async,
        self_mode,
        is_class_method,
        name,
        generic_params,
        params,
        return_type,
        where_clause,
        body,
        doc_comments: _,
        span,
    } = func;
    let mut sig_parts = Vec::new();

    // `async def` — the async modifier precedes `def` (surface spelling
    // `async def name`). Dropping it silently rewrites an async function into
    // a synchronous one (a semantic change + reparse-identity break).
    if *is_async {
        sig_parts.push(text("async "));
    }

    sig_parts.push(text("def "));

    // Self mode
    if let Some(self_mode) = self_mode {
        match self_mode {
            SelfMode::Mutable => sig_parts.push(text("var ")),
            SelfMode::Consuming => sig_parts.push(text("consume ")),
            SelfMode::Immutable => {}
        }
    }

    // Class method marker
    if *is_class_method {
        sig_parts.push(text("self."));
    }

    sig_parts.push(text(name.clone()));

    // Generic params
    if let Some(gp) = generic_params {
        sig_parts.push(format_generic_params(gp));
    }

    // Parameters
    if params.is_empty() && self_mode.is_none() {
        // No params at all — some functions have no parens
    } else {
        let param_docs: Vec<Doc> = params.iter().map(|p| format_param(p, comments)).collect();
        if param_docs.is_empty() {
            // Has self_mode but no explicit params — no parens needed
        } else {
            sig_parts.push(format_call_args(param_docs));
        }
    }

    // Return type
    if let Some(rt) = return_type {
        sig_parts.push(text(" -> "));
        sig_parts.push(format_type_expr(rt, comments));
    }

    // Where clause
    if let Some(wc) = where_clause {
        sig_parts.push(format_where_clause(wc));
    }

    let sig = group(concat(sig_parts));

    // Body
    if body.statements.is_empty() {
        // Dangling comments in empty body
        let dangling = comments.dangling_comments(span.start);
        if dangling.is_empty() {
            concat(vec![sig, hardline(), text("end")])
        } else {
            let comment_docs: Vec<Doc> = dangling.iter().map(format_comment).collect();
            concat(vec![
                sig,
                nest(
                    INDENT_WIDTH,
                    concat(
                        comment_docs
                            .into_iter()
                            .map(|c| concat(vec![hardline(), c]))
                            .collect(),
                    ),
                ),
                hardline(),
                text("end"),
            ])
        }
    } else {
        // Check for single-expression inline body: `pub def id -> Int { self.id }`
        // Only use inline form for simple expressions (not control flow).
        if body.statements.len() == 1 && is_simple_inline_body(body) {
            let body_doc = format_block_body(body, comments);
            let inline = group(concat(vec![
                sig.clone(),
                text(" { "),
                body_doc.clone(),
                text(" }"),
            ]));

            let expanded = concat(vec![
                sig,
                nest(INDENT_WIDTH, concat(vec![hardline(), body_doc])),
                hardline(),
                text("end"),
            ]);

            // Use IfBreak: try inline first, fall back to expanded
            group(if_break(expanded, inline))
        } else {
            let body_doc = format_block_body(body, comments);
            concat(vec![
                sig,
                nest(INDENT_WIDTH, concat(vec![hardline(), body_doc])),
                hardline(),
                text("end"),
            ])
        }
    }
}

fn format_param(param: &Param, comments: &CommentMap) -> Doc {
    let Param {
        auto_assign,
        name,
        type_expr,
        default,
        span: _,
    } = param;
    let mut parts = Vec::new();
    if *auto_assign {
        parts.push(text("@"));
    }
    parts.push(text(name.clone()));
    parts.push(text(": "));
    parts.push(format_type_expr(type_expr, comments));
    if let Some(default) = default {
        parts.push(text(" = "));
        parts.push(format_expr(default, comments));
    }
    concat(parts)
}

// ─── Classes ────────────────────────────────────────────────────────

/// The `layout <kind>` directives that open a type body (ruby-naming.spec.md
/// §3.5), one per line. The parser stores `layout c` as `C`.
fn format_layout(layout: &[String]) -> Option<Doc> {
    if layout.is_empty() {
        return None;
    }
    let lines = layout
        .iter()
        .map(|kind| {
            let kind = if kind == "C" { "c" } else { kind.as_str() };
            text(format!("layout {kind}"))
        })
        .collect();
    Some(join(hardline(), lines))
}

fn format_class(class: &ClassDef, comments: &CommentMap) -> Doc {
    let ClassDef {
        name,
        generic_params,
        parent,
        fields,
        methods,
        inner_impls,
        layout,
        lib_decls,
        doc_comments: _,
        where_clause,
        aliases,
        span: _,
    } = class;
    let mut header = vec![text("class "), text(name.clone())];

    if let Some(gp) = generic_params {
        header.push(format_generic_params(gp));
    }

    if let Some(parent) = parent {
        header.push(text(" < "));
        header.push(format_type_path(parent));
    }

    if let Some(wc) = where_clause {
        header.push(format_where_clause(wc));
    }

    let mut body_parts: Vec<Doc> = Vec::new();
    body_parts.extend(format_layout(layout));

    // Fields — one per line, no blank lines between them
    let field_docs = format_field_section(fields, comments);
    if !field_docs.is_empty() {
        body_parts.push(join(hardline(), field_docs));
    }

    // Methods — separated by blank lines, with visibility SECTION MARKERS
    // (ruby-naming.spec.md §3.2) so a `private`/`protected` method round-trips
    // with its visibility intact instead of silently re-parsing as public.
    let mut running = Visibility::Public;
    body_parts.extend(format_method_section(methods, comments, &mut running));
    if running != Visibility::Public {
        body_parts.push(text("public"));
    }

    // Inner impls
    for imp in inner_impls {
        body_parts.push(format_inner_impl(imp, comments));
    }

    // In-body `lib "X" ... end` FFI blocks (#06.8 follow-up). Reuses
    // `format_lib` so a class-scoped FFI block round-trips identically
    // to a top-level one.
    for lib in lib_decls {
        body_parts.push(with_leading_comments(
            lib.span.start,
            format_lib(lib, comments),
            comments,
        ));
    }

    // Ruby `alias new old` synonyms (docs/decisions/alias-keyword.md, D10).
    if !aliases.is_empty() {
        body_parts.push(join(
            hardline(),
            aliases.iter().map(format_alias).collect::<Vec<_>>(),
        ));
    }

    // Join sections with blank lines (hardline + hardline = one blank line)
    let body = join(concat(vec![hardline(), hardline()]), body_parts);

    concat(vec![
        concat(header),
        nest(INDENT_WIDTH, concat(vec![hardline(), body])),
        hardline(),
        text("end"),
    ])
}

fn format_field_decl(field: &FieldDecl, comments: &CommentMap) -> Doc {
    // ruby-naming.spec.md §3.2: fields have NO per-field visibility prefix
    // (`pub`/`private`/`protected` keywords before a field name are not valid
    // surface syntax — `pub` is not even a keyword, only `public` is). Field
    // visibility is expressed through section markers in the type body, which
    // `format_field_section` emits. A field declaration is just `name: Type`.
    let FieldDecl {
        visibility: _,
        name,
        type_expr,
        span: _,
    } = field;
    concat(vec![
        text(name.clone()),
        text(": "),
        format_type_expr(type_expr, comments),
    ])
}

/// Render a type's fields, inserting `public` / `private` / `protected`
/// section markers (ruby-naming.spec.md §3.2) whenever the running visibility
/// changes. The body default is `public`, so leading public fields emit no
/// marker. If the run ends on a non-public visibility, a trailing `public`
/// marker is emitted so that methods following the field block (which the
/// formatter groups after fields) keep their default-public visibility rather
/// than silently inheriting the last field's section.
fn format_field_section(fields: &[FieldDecl], comments: &CommentMap) -> Vec<Doc> {
    let mut docs: Vec<Doc> = Vec::new();
    let mut running = Visibility::Public;
    for field in fields {
        if field.visibility != running {
            docs.push(text(visibility_marker(field.visibility)));
            running = field.visibility;
        }
        docs.push(format_field_decl(field, comments));
    }
    if running != Visibility::Public {
        docs.push(text("public"));
    }
    docs
}

fn visibility_marker(v: Visibility) -> &'static str {
    match v {
        Visibility::Public => "public",
        Visibility::Protected => "protected",
        Visibility::Private => "private",
    }
}

/// Render a type's methods as body-parts (each is its own block, blank-line
/// separated by the caller), inserting a `private` / `protected` / `public`
/// SECTION MARKER (ruby-naming.spec.md §3.2) as a standalone part whenever the
/// running visibility changes.
///
/// Per-`def` visibility is NOT valid surface syntax — `format_func_def`
/// deliberately emits no inline `private`/`protected` keyword. Without these
/// markers the reformatted source would re-parse every method as `public`,
/// silently widening a `private` method's visibility (a cross-surface
/// divergence: the compiler/IDE see `private`, `ruxen fmt` rewrites it
/// public). The body default is `public`, so leading public methods emit no
/// marker; the field section already resets to `public` before methods begin.
fn format_method_section(
    methods: &[FuncDef],
    comments: &CommentMap,
    running: &mut Visibility,
) -> Vec<Doc> {
    let mut parts: Vec<Doc> = Vec::new();
    for method in methods {
        if method.visibility != *running {
            parts.push(text(visibility_marker(method.visibility)));
            *running = method.visibility;
        }
        parts.push(format_func_with_leading_comments(method, comments));
    }
    parts
}

// ─── Structs ────────────────────────────────────────────────────────

fn format_struct(s: &StructDef, comments: &CommentMap) -> Doc {
    let StructDef {
        name,
        generic_params,
        fields,
        methods,
        inner_impls,
        layout,
        doc_comments: _,
        where_clause,
        aliases,
        span: _,
    } = s;
    let mut header = vec![text("struct "), text(name.clone())];

    if let Some(gp) = generic_params {
        header.push(format_generic_params(gp));
    }

    if let Some(wc) = where_clause {
        header.push(format_where_clause(wc));
    }

    // Layout, then fields (joined without blank lines), then
    // methods/inner_impls, each separated by a blank line — mirrors
    // `format_class`.
    let mut body_parts: Vec<Doc> = Vec::new();
    body_parts.extend(format_layout(layout));

    let field_docs = format_field_section(fields, comments);
    if !field_docs.is_empty() {
        body_parts.push(join(hardline(), field_docs));
    }

    let mut running = Visibility::Public;
    body_parts.extend(format_method_section(methods, comments, &mut running));
    if running != Visibility::Public {
        body_parts.push(text("public"));
    }

    for imp in inner_impls {
        body_parts.push(format_inner_impl(imp, comments));
    }

    if !aliases.is_empty() {
        body_parts.push(join(
            hardline(),
            aliases.iter().map(format_alias).collect::<Vec<_>>(),
        ));
    }

    let body = join(concat(vec![hardline(), hardline()]), body_parts);

    concat(vec![
        concat(header),
        nest(INDENT_WIDTH, concat(vec![hardline(), body])),
        hardline(),
        text("end"),
    ])
}

// ─── Enums ──────────────────────────────────────────────────────────

fn format_enum(e: &EnumDef, comments: &CommentMap) -> Doc {
    let EnumDef {
        name,
        generic_params,
        variants,
        methods,
        inner_impls,
        layout,
        doc_comments: _,
        where_clause,
        aliases,
        span: _,
    } = e;
    let mut header = vec![text("enum "), text(name.clone())];

    if let Some(gp) = generic_params {
        header.push(format_generic_params(gp));
    }

    if let Some(wc) = where_clause {
        header.push(format_where_clause(wc));
    }

    // Layout, then variants joined on hardlines (no blank between), then
    // methods / inner-impls each separated by a blank line — mirrors
    // `format_class`.
    let mut body_parts: Vec<Doc> = Vec::new();
    body_parts.extend(format_layout(layout));

    let variant_docs: Vec<Doc> = variants
        .iter()
        .map(|v| format_variant(v, comments))
        .collect();
    if !variant_docs.is_empty() {
        body_parts.push(join(hardline(), variant_docs));
    }

    let mut running = Visibility::Public;
    body_parts.extend(format_method_section(methods, comments, &mut running));
    if running != Visibility::Public {
        body_parts.push(text("public"));
    }

    for imp in inner_impls {
        body_parts.push(format_inner_impl(imp, comments));
    }

    if !aliases.is_empty() {
        body_parts.push(join(
            hardline(),
            aliases.iter().map(format_alias).collect::<Vec<_>>(),
        ));
    }

    let body = join(concat(vec![hardline(), hardline()]), body_parts);

    concat(vec![
        concat(header),
        nest(INDENT_WIDTH, concat(vec![hardline(), body])),
        hardline(),
        text("end"),
    ])
}

fn format_variant(variant: &Variant, comments: &CommentMap) -> Doc {
    let Variant {
        name,
        fields,
        span: _,
    } = variant;
    let name = text(name.clone());
    // Tuple and named-field variants are both written with parentheses.
    let field_list = match fields {
        VariantKind::Unit => return name,
        VariantKind::Tuple(fields) | VariantKind::Struct(fields) => fields,
    };
    let field_docs: Vec<Doc> = field_list
        .iter()
        .map(
            |VariantField {
                 name,
                 type_expr,
                 span: _,
             }| {
                let ty = format_type_expr(type_expr, comments);
                match name {
                    Some(n) => concat(vec![text(n.clone()), text(": "), ty]),
                    None => ty,
                }
            },
        )
        .collect();
    group(concat(vec![
        name,
        text("("),
        nest(
            INDENT_WIDTH,
            concat(vec![
                softline(),
                join(concat(vec![text(","), line()]), field_docs),
            ]),
        ),
        softline(),
        text(")"),
    ]))
}

// ─── Traits ─────────────────────────────────────────────────────────

fn format_trait(t: &MixinDef, comments: &CommentMap) -> Doc {
    let MixinDef {
        name,
        generic_params,
        super_traits,
        items,
        lib_decls,
        doc_comments: _,
        dispatch_mode,
        span: _,
    } = t;
    let mut header = vec![text("mixin "), text(name.clone())];

    if let Some(gp) = generic_params {
        header.push(format_generic_params(gp));
    }

    // Mixin vtables spec §B1: `dispatch runtime` modifier sits after the
    // mixin name + generics, before any `: Super` bounds. Skipped when
    // dispatch_mode is the (default) Static.
    match dispatch_mode {
        DispatchMode::Runtime => header.push(text(" dispatch runtime")),
        DispatchMode::Static => {}
    }

    if !super_traits.is_empty() {
        header.push(text(": "));
        header.push(format_trait_bounds(super_traits));
    }

    // Default methods carry visibility; it is written with section markers
    // (ruby-naming.spec.md §3.2), as in class bodies.
    let mut body_parts: Vec<Doc> = Vec::new();
    let mut running = Visibility::Public;
    for item in items {
        if let MixinItem::DefaultMethod(func) = item {
            if func.visibility != running {
                body_parts.push(text(visibility_marker(func.visibility)));
                running = func.visibility;
            }
        }
        body_parts.push(format_trait_item(item, comments));
    }

    // In-body `lib "X" ... end` FFI blocks (#06.8 follow-up). Mirrors
    // the class-body emission so a mixin-scoped FFI block round-trips.
    for lib in lib_decls {
        body_parts.push(with_leading_comments(
            lib.span.start,
            format_lib(lib, comments),
            comments,
        ));
    }

    let body = join(hardline(), body_parts);

    concat(vec![
        concat(header),
        nest(INDENT_WIDTH, concat(vec![hardline(), body])),
        hardline(),
        text("end"),
    ])
}

fn format_trait_item(item: &MixinItem, comments: &CommentMap) -> Doc {
    match item {
        MixinItem::AssocType { name, span: _ } => text(format!("type {}", name)),
        MixinItem::MethodSig(sig) => format_method_sig(sig, comments),
        MixinItem::DefaultMethod(func) => format_func_with_leading_comments(func, comments),
        MixinItem::Alias(a) => format_alias(a),
    }
}

fn format_method_sig(sig: &MethodSig, comments: &CommentMap) -> Doc {
    let MethodSig {
        is_async,
        self_mode,
        is_class_method,
        name,
        generic_params,
        params,
        return_type,
        span: _,
    } = sig;
    let mut parts = Vec::new();
    if *is_async {
        parts.push(text("async "));
    }

    // Keyword order matches `format_func_def`: `def` first, then the self-mode
    // modifier (`var` / `consume`). Emitting `var def` instead of `def var`
    // does not parse — the mixin-body parser expects `def`/`type` to lead.
    parts.push(text("def "));

    if let Some(self_mode) = self_mode {
        match self_mode {
            SelfMode::Mutable => parts.push(text("var ")),
            SelfMode::Consuming => parts.push(text("consume ")),
            SelfMode::Immutable => {}
        }
    }

    if *is_class_method {
        parts.push(text("self."));
    }

    parts.push(text(name.clone()));

    if let Some(gp) = generic_params {
        parts.push(format_generic_params(gp));
    }

    let param_docs: Vec<Doc> = params.iter().map(|p| format_param(p, comments)).collect();
    if !param_docs.is_empty() {
        parts.push(format_call_args(param_docs));
    }

    if let Some(rt) = return_type {
        parts.push(text(" -> "));
        parts.push(format_type_expr(rt, comments));
    }

    group(concat(parts))
}

// ─── Impl Blocks ────────────────────────────────────────────────────
//
// TODO(migration): the `impl Trait for Type ... end` block form is retired
// (§10a). `impl T for U` should now be expressed as `include T` inside `U`'s
// body, and `impl[T: B] C[T] ... end` becomes `extension C[T] where T: B`.
// The AST still uses `ImplBlock`/`InnerImpl` nodes for backward parsing
// while the migration is in flight; this formatter therefore still emits
// the legacy `impl ... end` keyword spelling so it round-trips. When the
// parser stops accepting `impl`, this function should be rewritten to emit
// `include` / `extension` blocks.

fn format_impl(imp: &ImplBlock, comments: &CommentMap) -> Doc {
    let ImplBlock {
        generic_params,
        is_unsafe,
        negative_trait,
        trait_name,
        target_type,
        items,
        span: _,
    } = imp;
    // ruby-naming.spec.md §3.4a: the block opener is `extension` — `impl` was
    // retired and is now lexed as a bare identifier, so emitting `impl ...` no
    // longer parses ("expected top-level declaration, found Identifier").
    let mut header = Vec::new();
    if *is_unsafe {
        header.push(text("unsafe "));
    }
    header.push(text("extension "));

    if *negative_trait {
        header.push(text("!"));
    }

    if let Some(gp) = generic_params {
        header.push(format_generic_params(gp));
        header.push(text(" "));
    }

    if let Some(trait_name) = trait_name {
        header.push(format_type_path(trait_name));
        header.push(text(" for "));
    }

    header.push(format_type_expr(target_type, comments));

    let item_docs: Vec<Doc> = items
        .iter()
        .map(|item| format_impl_item(item, comments))
        .collect();
    let body = join(concat(vec![hardline(), hardline()]), item_docs);

    concat(vec![
        concat(header),
        nest(INDENT_WIDTH, concat(vec![hardline(), body])),
        hardline(),
        text("end"),
    ])
}

fn format_impl_item(item: &ImplItem, comments: &CommentMap) -> Doc {
    match item {
        ImplItem::AssocType {
            name,
            type_expr,
            span: _,
        } => concat(vec![
            text("type "),
            text(name.clone()),
            text(" = "),
            format_type_expr(type_expr, comments),
        ]),
        ImplItem::Method(func) => with_leading_comments(
            func.span.start,
            format_prefixed_func(func, comments),
            comments,
        ),
        ImplItem::Include {
            is_unsafe,
            negative_trait,
            trait_name,
            span: _,
        } => {
            let mut parts = Vec::new();
            if *is_unsafe {
                parts.push(text("unsafe "));
            }
            parts.push(text("include "));
            if *negative_trait {
                parts.push(text("!"));
            }
            parts.push(format_type_path(trait_name));
            concat(parts)
        }
        ImplItem::Alias(a) => format_alias(a),
    }
}

fn format_inner_impl(imp: &InnerImpl, comments: &CommentMap) -> Doc {
    let InnerImpl {
        is_unsafe,
        negative_trait,
        trait_name,
        items,
        span: _,
    } = imp;
    // An InnerImpl with no items represents the bare `include Mixin`
    // directive (see `Parser::parse_include_directive`). Emit the
    // original directive form so the formatter round-trips cleanly.
    if items.is_empty() {
        let mut parts = Vec::new();
        if *is_unsafe {
            parts.push(text("unsafe "));
        }
        parts.push(text("include "));
        if *negative_trait {
            parts.push(text("!"));
        }
        parts.push(format_type_path(trait_name));
        return concat(parts);
    }

    let header = vec![text("impl "), format_type_path(trait_name)];

    let item_docs: Vec<Doc> = items
        .iter()
        .map(|item| format_impl_item(item, comments))
        .collect();
    let body = join(concat(vec![hardline(), hardline()]), item_docs);

    concat(vec![
        concat(header),
        nest(INDENT_WIDTH, concat(vec![hardline(), body])),
        hardline(),
        text("end"),
    ])
}

// ─── Modules ────────────────────────────────────────────────────────

fn format_module(m: &ModuleDef, comments: &CommentMap) -> Doc {
    let ModuleDef {
        name,
        items,
        span: _,
    } = m;
    // Module functions carry their section's visibility (ruby-naming.spec.md
    // §3.2); it is written back with section markers.
    let mut item_docs: Vec<Doc> = Vec::new();
    let mut running = Visibility::Public;
    for item in items {
        match item {
            TopLevelItem::Function(f) => {
                if f.visibility != running {
                    item_docs.push(text(visibility_marker(f.visibility)));
                    running = f.visibility;
                }
                item_docs.push(format_func_def(f, comments));
            }
            other => item_docs.push(format_top_level_item(other, comments)),
        }
    }
    let body = join(concat(vec![hardline(), hardline()]), item_docs);

    concat(vec![
        text("module "),
        text(name.clone()),
        nest(INDENT_WIDTH, concat(vec![hardline(), body])),
        hardline(),
        text("end"),
    ])
}

// ─── Use Declarations ───────────────────────────────────────────────

pub fn format_use(u: &UseDecl) -> Doc {
    let UseDecl {
        path,
        kind,
        span: _,
    } = u;
    let path_str = path.join(".");
    match kind {
        UseKind::Simple => concat(vec![text("use "), text(path_str)]),
        UseKind::Alias(alias) => concat(vec![
            text("use "),
            text(path_str),
            text(" as "),
            text(alias.clone()),
        ]),
        UseKind::Group(names) => {
            // Source order is kept: the names are part of the AST.
            let name_docs: Vec<Doc> = names.iter().map(|n| text(n.clone())).collect();
            group(concat(vec![
                text("use "),
                text(path_str),
                text(".{"),
                nest(
                    INDENT_WIDTH,
                    concat(vec![
                        softline(),
                        join(concat(vec![text(","), line()]), name_docs),
                    ]),
                ),
                softline(),
                text("}"),
            ]))
        }
    }
}

// ─── Type Aliases & Newtypes ────────────────────────────────────────

fn format_type_alias(ta: &TypeAliasDef, comments: &CommentMap) -> Doc {
    let TypeAliasDef {
        name,
        generic_params,
        type_expr,
        span: _,
    } = ta;
    let mut parts = vec![text("type "), text(name.clone())];
    if let Some(gp) = generic_params {
        parts.push(format_generic_params(gp));
    }
    parts.push(text(" = "));
    parts.push(format_type_expr(type_expr, comments));
    group(concat(parts))
}

fn format_newtype(nt: &NewtypeDef, comments: &CommentMap) -> Doc {
    let NewtypeDef {
        name,
        inner_type,
        span: _,
    } = nt;
    concat(vec![
        text("newtype "),
        text(name.clone()),
        text("("),
        format_type_expr(inner_type, comments),
        text(")"),
    ])
}

// ─── Constants ──────────────────────────────────────────────────────

fn format_const(c: &ConstDef, comments: &CommentMap) -> Doc {
    let ConstDef {
        name,
        type_expr,
        value,
        doc_comments: _,
        span: _,
    } = c;
    let mut parts = vec![text("const "), text(name.clone())];
    // An omitted const type parses to `TypeExpr::Inferred`. Re-emitting it as
    // `: _` does not parse (`expected type, found _`), so only render the
    // annotation when the source actually wrote one.
    if !matches!(type_expr, TypeExpr::Inferred { span: _ }) {
        parts.push(text(": "));
        parts.push(format_type_expr(type_expr, comments));
    }
    parts.push(text(" = "));
    parts.push(format_expr(value, comments));
    group(concat(parts))
}

// ─── FFI Declarations ───────────────────────────────────────────────

/// Render a `lib` block's link name. The parser (`parse_lib_decl`) accepts
/// either a string literal (`lib "runtime/foo.c"`) or a bare TypeIdentifier
/// (`lib LibM`), storing only the raw text in `LibDecl.name`. To round-trip we
/// re-quote anything that is not a valid bare TypeIdentifier — paths like
/// `runtime/foo.c` and lowercase names like `c` must keep their quotes, or the
/// re-parse fails ("expected lib name — a string literal or TypeIdentifier").
fn format_lib_name(name: &str) -> String {
    if is_bare_type_identifier(name) {
        name.to_string()
    } else {
        format!("\"{}\"", name)
    }
}

/// A bare TypeIdentifier starts with an uppercase ASCII letter and contains
/// only ASCII alphanumerics or underscores — matching the lexer's TypeIdentifier
/// rule. Anything else (paths, lowercase names, `.`/`/`) must be quoted.
fn is_bare_type_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_uppercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn format_lib(l: &LibDecl, comments: &CommentMap) -> Doc {
    let LibDecl {
        name,
        functions,
        span: _,
    } = l;
    let mut body_parts: Vec<Doc> = Vec::new();

    for func in functions {
        // Emit `##` doc comments attached above each FFI `def` — stdlib
        // packages document every FFI alias, and dropping the comment here
        // would make `ruxen fmt` silently delete documentation the parser
        // accepts (a cross-binary divergence).
        let mut entry: Vec<Doc> = Vec::new();
        for comment in comments.leading_comments(func.span.start) {
            entry.push(format_comment(comment));
            entry.push(hardline());
        }
        entry.push(format_ffi_function(func, comments));
        body_parts.push(concat(entry));
    }

    let body = join(hardline(), body_parts);

    concat(vec![
        text("lib "),
        text(format_lib_name(name)),
        nest(INDENT_WIDTH, concat(vec![hardline(), body])),
        hardline(),
        text("end"),
    ])
}

fn format_ffi_function(func: &FfiFunction, comments: &CommentMap) -> Doc {
    let FfiFunction {
        name,
        is_class_method,
        c_symbol,
        params,
        return_type,
        where_clause,
        is_variadic,
        span: _,
    } = func;
    let param_docs: Vec<Doc> = params
        .iter()
        .map(
            |FfiParam {
                 name,
                 type_expr,
                 span: _,
             }| {
                concat(vec![
                    text(name.clone()),
                    text(": "),
                    format_type_expr(type_expr, comments),
                ])
            },
        )
        .collect();

    let mut parts = vec![text("def ")];

    // `def self.NAME` marks a class method (ruby-naming.spec.md §3.4a). The
    // parser records this in `FfiFunction.is_class_method`; emit it back so a
    // static FFI decl does not silently become an instance method on re-parse.
    if *is_class_method {
        parts.push(text("self."));
    }
    parts.push(text(name.clone()));

    // Preserve the `as "<c-symbol>"` rename clause when present so the
    // formatter round-trips per-decl C-symbol aliases (#06.8).
    if let Some(sym) = c_symbol {
        parts.push(text(" as "));
        parts.push(text(format!("\"{}\"", sym)));
    }

    if !param_docs.is_empty() || *is_variadic {
        let mut all_params = param_docs;
        if *is_variadic {
            all_params.push(text("..."));
        }
        parts.push(format_call_args(all_params));
    }

    if let Some(rt) = return_type {
        parts.push(text(" -> "));
        parts.push(format_type_expr(rt, comments));
    }

    if let Some(wc) = where_clause {
        parts.push(format_where_clause(wc));
    }

    group(concat(parts))
}
