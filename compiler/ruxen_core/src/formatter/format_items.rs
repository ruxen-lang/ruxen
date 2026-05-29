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
        TopLevelItem::Extern(e) => &e.span,
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
        TopLevelItem::Function(f) => format_func_def(f, comments),
        TopLevelItem::Use(u) => format_use(u),
        TopLevelItem::TypeAlias(ta) => format_type_alias(ta, comments),
        TopLevelItem::Newtype(nt) => format_newtype(nt, comments),
        TopLevelItem::Const(c) => format_const(c, comments),
        TopLevelItem::Lib(l) => format_lib(l, comments),
        TopLevelItem::Extern(e) => format_extern(e, comments),
    }
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

pub fn format_func_def(func: &FuncDef, comments: &CommentMap) -> Doc {
    let mut sig_parts = Vec::new();

    // Visibility
    // TODO(migration): per-def `pub`/`protected`/`private` prefixes are not
    // valid Ruxen surface syntax (§3.2). Visibility lives in section markers
    // (`private` / `protected` / `public`) inside the type body. Formatter
    // needs to be reworked to emit section markers instead of per-method
    // keywords. For now, emit nothing for Public (which is the default) and
    // emit the legacy keyword for Protected/Private so we round-trip.
    match func.visibility {
        Visibility::Public => {}
        Visibility::Protected => sig_parts.push(text("protected ")),
        Visibility::Private => {}
    }

    sig_parts.push(text("def "));

    // Self mode
    if let Some(self_mode) = &func.self_mode {
        match self_mode {
            SelfMode::Mutable => sig_parts.push(text("var ")),
            SelfMode::Consuming => sig_parts.push(text("consume ")),
            SelfMode::Immutable => {}
        }
    }

    // Class method marker
    if func.is_class_method {
        sig_parts.push(text("self."));
    }

    sig_parts.push(text(func.name.clone()));

    // Generic params
    if let Some(gp) = &func.generic_params {
        sig_parts.push(format_generic_params(gp));
    }

    // Parameters
    if func.params.is_empty() && func.self_mode.is_none() {
        // No params at all — some functions have no parens
    } else {
        let param_docs: Vec<Doc> = func
            .params
            .iter()
            .map(|p| format_param(p, comments))
            .collect();
        if param_docs.is_empty() {
            // Has self_mode but no explicit params — no parens needed
        } else {
            sig_parts.push(format_call_args(param_docs));
        }
    }

    // Return type
    if let Some(rt) = &func.return_type {
        sig_parts.push(text(" -> "));
        sig_parts.push(format_type_expr(rt, comments));
    }

    // Where clause
    if let Some(wc) = &func.where_clause {
        sig_parts.push(format_where_clause(wc));
    }

    let sig = group(concat(sig_parts));

    // Body
    if func.body.statements.is_empty() {
        // Dangling comments in empty body
        let dangling = comments.dangling_comments(func.span.start);
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
        if func.body.statements.len() == 1 && is_simple_inline_body(&func.body) {
            let body_doc = format_block_body(&func.body, comments);
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
            let body = format_block_body(&func.body, comments);
            concat(vec![
                sig,
                nest(INDENT_WIDTH, concat(vec![hardline(), body])),
                hardline(),
                text("end"),
            ])
        }
    }
}

fn format_param(param: &Param, comments: &CommentMap) -> Doc {
    let mut parts = Vec::new();
    if param.auto_assign {
        parts.push(text("@"));
    }
    parts.push(text(param.name.clone()));
    parts.push(text(": "));
    parts.push(format_type_expr(&param.type_expr, comments));
    concat(parts)
}

// ─── Classes ────────────────────────────────────────────────────────

fn format_class(class: &ClassDef, comments: &CommentMap) -> Doc {
    let mut header = vec![text("class "), text(class.name.clone())];

    if let Some(gp) = &class.generic_params {
        header.push(format_generic_params(gp));
    }

    if let Some(parent) = &class.parent {
        header.push(text(" < "));
        header.push(format_type_path(parent));
    }

    let mut body_parts: Vec<Doc> = Vec::new();

    // Fields — one per line, no blank lines between them
    let field_docs = format_field_section(&class.fields, comments);
    if !field_docs.is_empty() {
        body_parts.push(join(hardline(), field_docs));
    }

    // Methods — separated by blank lines
    for method in &class.methods {
        body_parts.push(format_func_def(method, comments));
    }

    // Inner impls
    for imp in &class.inner_impls {
        body_parts.push(format_inner_impl(imp, comments));
    }

    // In-body `lib "X" ... end` FFI blocks (#06.8 follow-up). Reuses
    // `format_lib` so a class-scoped FFI block round-trips identically
    // to a top-level one.
    for lib in &class.lib_decls {
        body_parts.push(format_lib(lib, comments));
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
    concat(vec![
        text(field.name.clone()),
        text(": "),
        format_type_expr(&field.type_expr, comments),
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

// ─── Structs ────────────────────────────────────────────────────────

fn format_struct(s: &StructDef, comments: &CommentMap) -> Doc {
    let mut prelude: Vec<Doc> = Vec::new();
    // Pre-existing formatter output uses the retired `@[repr(...)]` syntax;
    // a separate follow-up will migrate it to in-body `layout <kind>` per
    // ruby-naming.spec.md §3.5. For now only the field access changes name.
    for arg in &s.layout {
        prelude.push(concat(vec![text(format!("@[repr({})]", arg)), hardline()]));
    }

    let mut header = vec![text("struct "), text(s.name.clone())];

    if let Some(gp) = &s.generic_params {
        header.push(format_generic_params(gp));
    }

    // Fields (joined without blank lines), then methods/inner_impls,
    // each separated by a blank line — mirrors `format_class`.
    let mut body_parts: Vec<Doc> = Vec::new();

    let field_docs = format_field_section(&s.fields, comments);
    if !field_docs.is_empty() {
        body_parts.push(join(hardline(), field_docs));
    }

    if !s.derive_traits.is_empty() {
        body_parts.push(concat(vec![
            text("derive "),
            join(
                concat(vec![text(","), space()]),
                s.derive_traits.iter().map(|t| text(t.clone())).collect(),
            ),
        ]));
    }

    for method in &s.methods {
        body_parts.push(format_func_def(method, comments));
    }

    for imp in &s.inner_impls {
        body_parts.push(format_inner_impl(imp, comments));
    }

    let body = join(concat(vec![hardline(), hardline()]), body_parts);

    concat(vec![
        concat(prelude),
        concat(header),
        nest(INDENT_WIDTH, concat(vec![hardline(), body])),
        hardline(),
        text("end"),
    ])
}

// ─── Enums ──────────────────────────────────────────────────────────

fn format_enum(e: &EnumDef, comments: &CommentMap) -> Doc {
    let mut header = vec![text("enum "), text(e.name.clone())];

    if let Some(gp) = &e.generic_params {
        header.push(format_generic_params(gp));
    }

    // Variants joined on hardlines (no blank between), then methods /
    // inner-impls each separated by a blank line — mirrors `format_class`.
    let mut body_parts: Vec<Doc> = Vec::new();

    let variant_docs: Vec<Doc> = e
        .variants
        .iter()
        .map(|v| format_variant(v, comments))
        .collect();
    if !variant_docs.is_empty() {
        body_parts.push(join(hardline(), variant_docs));
    }

    if !e.derive_traits.is_empty() {
        body_parts.push(concat(vec![
            text("derive "),
            join(
                concat(vec![text(","), space()]),
                e.derive_traits.iter().map(|t| text(t.clone())).collect(),
            ),
        ]));
    }

    for method in &e.methods {
        body_parts.push(format_func_def(method, comments));
    }

    for imp in &e.inner_impls {
        body_parts.push(format_inner_impl(imp, comments));
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
    let name = text(variant.name.clone());
    match &variant.fields {
        VariantKind::Unit => name,
        VariantKind::Tuple(fields) => {
            let field_docs: Vec<Doc> = fields
                .iter()
                .map(|f| {
                    if let Some(n) = &f.name {
                        concat(vec![
                            text(n.clone()),
                            text(": "),
                            format_type_expr(&f.type_expr, comments),
                        ])
                    } else {
                        format_type_expr(&f.type_expr, comments)
                    }
                })
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
        VariantKind::Struct(fields) => {
            let field_docs: Vec<Doc> = fields
                .iter()
                .map(|f| {
                    if let Some(n) = &f.name {
                        concat(vec![
                            text(n.clone()),
                            text(": "),
                            format_type_expr(&f.type_expr, comments),
                        ])
                    } else {
                        format_type_expr(&f.type_expr, comments)
                    }
                })
                .collect();
            // Use parentheses for named-field variants (Ruxen convention)
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
    }
}

// ─── Traits ─────────────────────────────────────────────────────────

fn format_trait(t: &MixinDef, comments: &CommentMap) -> Doc {
    let mut header = vec![text("mixin "), text(t.name.clone())];

    if let Some(gp) = &t.generic_params {
        header.push(format_generic_params(gp));
    }

    // Mixin vtables spec §B1: `dispatch runtime` modifier sits after the
    // mixin name + generics, before any `: Super` bounds. Skipped when
    // dispatch_mode is the (default) Static.
    if matches!(t.dispatch_mode, DispatchMode::Runtime) {
        header.push(text(" dispatch runtime"));
    }

    if !t.super_traits.is_empty() {
        header.push(text(": "));
        header.push(format_trait_bounds(&t.super_traits));
    }

    let mut body_parts: Vec<Doc> = t
        .items
        .iter()
        .map(|ti| format_trait_item(ti, comments))
        .collect();

    // In-body `lib "X" ... end` FFI blocks (#06.8 follow-up). Mirrors
    // the class-body emission so a mixin-scoped FFI block round-trips.
    for lib in &t.lib_decls {
        body_parts.push(format_lib(lib, comments));
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
        MixinItem::AssocType { name, .. } => text(format!("type {}", name)),
        MixinItem::MethodSig(sig) => format_method_sig(sig, comments),
        MixinItem::DefaultMethod(func) => format_func_def(func, comments),
    }
}

fn format_method_sig(sig: &MethodSig, comments: &CommentMap) -> Doc {
    let mut parts = Vec::new();

    // Keyword order matches `format_func_def`: `def` first, then the self-mode
    // modifier (`var` / `consume`). Emitting `var def` instead of `def var`
    // does not parse — the mixin-body parser expects `def`/`type` to lead.
    parts.push(text("def "));

    if let Some(self_mode) = &sig.self_mode {
        match self_mode {
            SelfMode::Mutable => parts.push(text("var ")),
            SelfMode::Consuming => parts.push(text("consume ")),
            SelfMode::Immutable => {}
        }
    }

    if sig.is_class_method {
        parts.push(text("self."));
    }

    parts.push(text(sig.name.clone()));

    if let Some(gp) = &sig.generic_params {
        parts.push(format_generic_params(gp));
    }

    let param_docs: Vec<Doc> = sig
        .params
        .iter()
        .map(|p| format_param(p, comments))
        .collect();
    if !param_docs.is_empty() {
        parts.push(format_call_args(param_docs));
    }

    if let Some(rt) = &sig.return_type {
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
    // ruby-naming.spec.md §3.4a: the block opener is `extension` — `impl` was
    // retired and is now lexed as a bare identifier, so emitting `impl ...` no
    // longer parses ("expected top-level declaration, found Identifier").
    let mut header = vec![text("extension ")];

    if imp.negative_trait {
        header.push(text("!"));
    }

    if let Some(gp) = &imp.generic_params {
        header.push(format_generic_params(gp));
        header.push(text(" "));
    }

    if let Some(trait_name) = &imp.trait_name {
        header.push(format_type_path(trait_name));
        header.push(text(" for "));
    }

    header.push(format_type_expr(&imp.target_type, comments));

    let item_docs: Vec<Doc> = imp
        .items
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
            name, type_expr, ..
        } => concat(vec![
            text("type "),
            text(name.clone()),
            text(" = "),
            format_type_expr(type_expr, comments),
        ]),
        ImplItem::Method(func) => format_func_def(func, comments),
        ImplItem::Include {
            is_unsafe,
            negative_trait,
            trait_name,
            ..
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
    }
}

fn format_inner_impl(imp: &InnerImpl, comments: &CommentMap) -> Doc {
    // An InnerImpl with no items represents the bare `include Mixin`
    // directive (see `Parser::parse_include_directive`). Emit the
    // original directive form so the formatter round-trips cleanly.
    if imp.items.is_empty() {
        let mut parts = Vec::new();
        if imp.is_unsafe {
            parts.push(text("unsafe "));
        }
        parts.push(text("include "));
        if imp.negative_trait {
            parts.push(text("!"));
        }
        parts.push(format_type_path(&imp.trait_name));
        return concat(parts);
    }

    let header = vec![text("impl "), format_type_path(&imp.trait_name)];

    let item_docs: Vec<Doc> = imp
        .items
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
    let mut body_parts: Vec<Doc> = Vec::new();
    for (i, item) in m.items.iter().enumerate() {
        if i > 0 {
            body_parts.push(hardline());
            body_parts.push(hardline());
        }
        body_parts.push(format_top_level_item(item, comments));
    }
    let body = concat(body_parts);

    concat(vec![
        text("module "),
        text(m.name.clone()),
        nest(INDENT_WIDTH, concat(vec![hardline(), body])),
        hardline(),
        text("end"),
    ])
}

// ─── Use Declarations ───────────────────────────────────────────────

pub fn format_use(u: &UseDecl) -> Doc {
    let path_str = u.path.join(".");
    match &u.kind {
        UseKind::Simple => concat(vec![text("use "), text(path_str)]),
        UseKind::Alias(alias) => concat(vec![
            text("use "),
            text(path_str),
            text(" as "),
            text(alias.clone()),
        ]),
        UseKind::Group(names) => {
            let mut sorted_names = names.clone();
            sorted_names.sort();
            let name_docs: Vec<Doc> = sorted_names.iter().map(|n| text(n.clone())).collect();
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
    let mut parts = vec![text("type "), text(ta.name.clone())];
    if let Some(gp) = &ta.generic_params {
        parts.push(format_generic_params(gp));
    }
    parts.push(text(" = "));
    parts.push(format_type_expr(&ta.type_expr, comments));
    group(concat(parts))
}

fn format_newtype(nt: &NewtypeDef, comments: &CommentMap) -> Doc {
    concat(vec![
        text("newtype "),
        text(nt.name.clone()),
        text("("),
        format_type_expr(&nt.inner_type, comments),
        text(")"),
    ])
}

// ─── Constants ──────────────────────────────────────────────────────

fn format_const(c: &ConstDef, comments: &CommentMap) -> Doc {
    let mut parts = vec![text("const "), text(c.name.clone())];
    // An omitted const type parses to `TypeExpr::Inferred`. Re-emitting it as
    // `: _` does not parse (`expected type, found _`), so only render the
    // annotation when the source actually wrote one.
    if !matches!(c.type_expr, TypeExpr::Inferred { .. }) {
        parts.push(text(": "));
        parts.push(format_type_expr(&c.type_expr, comments));
    }
    parts.push(text(" = "));
    parts.push(format_expr(&c.value, comments));
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
    let mut body_parts: Vec<Doc> = Vec::new();

    for func in &l.functions {
        body_parts.push(format_ffi_function(func, comments));
    }

    let body = join(hardline(), body_parts);

    let mut header_parts = vec![text("lib "), text(format_lib_name(&l.name))];

    // Link attrs
    for attr in &l.link_attrs {
        let kind_str = match attr.kind {
            LinkKind::Dynamic => "dynamic",
            LinkKind::Static => "static",
            LinkKind::Framework => "framework",
        };
        header_parts.push(hardline());
        header_parts.push(text(format!("  @[link({}, {})]", attr.name, kind_str)));
    }

    concat(vec![
        concat(header_parts),
        nest(INDENT_WIDTH, concat(vec![hardline(), body])),
        hardline(),
        text("end"),
    ])
}

// TODO(migration): the `extern "C" ... end` block is retired (§10a, §3.7).
// The replacement is `lib "<linkname>" ... end`. The AST still distinguishes
// `Lib` and `Extern` variants during the transition; once `Extern` parsing is
// removed, this function and the `ExternBlock` AST node should be deleted.
// Until then we emit the legacy form so a parse → format → parse round-trip
// on an `extern` block doesn't silently change semantics.
fn format_extern(e: &ExternBlock, comments: &CommentMap) -> Doc {
    let mut body_parts: Vec<Doc> = Vec::new();

    for func in &e.functions {
        body_parts.push(format_ffi_function(func, comments));
    }

    let body = join(hardline(), body_parts);

    concat(vec![
        text(format!("extern \"{}\"", e.abi)),
        nest(INDENT_WIDTH, concat(vec![hardline(), body])),
        hardline(),
        text("end"),
    ])
}

fn format_ffi_function(func: &FfiFunction, comments: &CommentMap) -> Doc {
    let param_docs: Vec<Doc> = func
        .params
        .iter()
        .map(|p| {
            concat(vec![
                text(p.name.clone()),
                text(": "),
                format_type_expr(&p.type_expr, comments),
            ])
        })
        .collect();

    let mut parts = vec![text("def ")];

    // `def self.NAME` marks a class method (ruby-naming.spec.md §3.4a). The
    // parser records this in `FfiFunction.is_class_method`; emit it back so a
    // static FFI decl does not silently become an instance method on re-parse.
    if func.is_class_method {
        parts.push(text("self."));
    }
    parts.push(text(func.name.clone()));

    // Preserve the `as "<c-symbol>"` rename clause when present so the
    // formatter round-trips per-decl C-symbol aliases (#06.8).
    if let Some(sym) = &func.c_symbol {
        parts.push(text(" as "));
        parts.push(text(format!("\"{}\"", sym)));
    }

    if !param_docs.is_empty() || func.is_variadic {
        let mut all_params = param_docs;
        if func.is_variadic {
            all_params.push(text("..."));
        }
        parts.push(format_call_args(all_params));
    }

    if let Some(rt) = &func.return_type {
        parts.push(text(" -> "));
        parts.push(format_type_expr(rt, comments));
    }

    group(concat(parts))
}
