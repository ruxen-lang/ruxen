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

    // Leading comments of the program span, when no item starts there to
    // carry them.
    if !program
        .items
        .iter()
        .any(|item| item.span().start == program.span.start)
    {
        for comment in comments.leading_comments(program.span.start) {
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

        parts.push(with_comments(
            item.span().start,
            format_top_level_item(item, comments),
            comments,
        ));

        prev_kind = Some(current_kind);
    }

    // Comments after the last item, or the rest of a file without items
    // (whose first comment already ended its line above).
    let first_dangling = comments
        .dangling
        .get(&program.span.start)
        .and_then(|c| c.first())
        .map(|c| c.span.start);
    if let Some(tail) = dangling_comments_doc(program.span.start, comments) {
        if !program.items.is_empty() {
            parts.push(hardline());
        }
        if first_dangling.is_some_and(|start| comments.blank_line_directly_above(start)) {
            parts.push(hardline());
        }
        parts.push(tail);
    }

    // Trailing newline
    parts.push(hardline());

    concat(parts)
}

pub(super) fn format_comment(comment: &super::comments::Comment) -> Doc {
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

/// Check if a function body may print in the inline `{ }` form: a single
/// simple statement that the author wrote on the `def` line itself
/// (`def id -> Int { @id }`, the endless `def id = @id`, or
/// `def main; puts "x"; end`). A body written on its own line keeps its
/// `def` / `end` layout: fmt never collapses a multi-line `def`.
/// Control flow (if, match, while, for, loop), blocks, and closures are NOT simple.
fn is_simple_inline_body(block: &Block, def_line: u32) -> bool {
    if block.statements.len() != 1 {
        return false;
    }
    match &block.statements[0] {
        Statement::Let(_) => false,
        Statement::Expression(expr) => expr.span.line == def_line && is_simple_expr(&expr.kind),
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
    with_comments(func.span.start, format_func_def(func, comments), comments)
}

/// The comments left dangling in the scope at `start` (an empty body, or no
/// member to follow), as one body part, if there are any.
pub(super) fn dangling_comments_doc(start: usize, comments: &CommentMap) -> Option<Doc> {
    let dangling = comments.dangling_comments(start);
    if dangling.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    for (i, comment) in dangling.iter().enumerate() {
        if i > 0 {
            parts.push(hardline());
            if comments.blank_line_directly_above(comment.span.start) {
                parts.push(hardline());
            }
        }
        parts.push(format_comment(comment));
    }
    Some(concat(parts))
}

/// `doc` preceded by the comments that lead the node at source offset
/// `start`, followed by the comments trailing it on its last line and the
/// own-line comments after it when it is the last member of its scope.
/// Every body member (statement, field, variant, signature, nested item)
/// goes through this, so no attached comment is left unwritten.
pub(super) fn with_comments(start: usize, doc: Doc, comments: &CommentMap) -> Doc {
    let mut parts: Vec<Doc> = Vec::new();
    let leading = comments.leading_comments(start);
    for (i, comment) in leading.iter().enumerate() {
        // The blank lines the author left between comment groups, and
        // between the comments and the node, stay.
        if i > 0 && comments.blank_line_directly_above(comment.span.start) {
            parts.push(hardline());
        }
        parts.push(format_comment(comment));
        parts.push(hardline());
    }
    if !leading.is_empty() && comments.blank_line_directly_above(start) {
        parts.push(hardline());
    }
    parts.push(doc);
    for comment in comments.trailing_comments(start) {
        parts.push(line_suffix(concat(vec![
            text("  "),
            format_comment(comment),
        ])));
    }
    for comment in comments.after_comments(start) {
        parts.push(hardline());
        if comments.blank_line_directly_above(comment.span.start) {
            parts.push(hardline());
        }
        parts.push(format_comment(comment));
    }
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
    let sig_parts = signature_doc(
        *is_async,
        *self_mode,
        *is_class_method,
        name,
        generic_params.as_ref().map(format_generic_params),
        params.iter().map(|p| format_param(p, comments)).collect(),
        return_type
            .as_ref()
            .map(|rt| format_type_expr(rt, comments)),
    );

    // The where clause sits outside the header group: on its own line it is
    // a hard line break, which must not force the parameter list to break.
    let mut sig_tail = Vec::new();
    if let Some(wc) = where_clause {
        sig_tail.push(format_where_clause(wc, span.line));
    }
    sig_tail.extend(header_trailing_comments(
        params,
        return_type.as_ref(),
        comments,
    ));
    let sig = concat(vec![group(concat(sig_parts)), concat(sig_tail)]);

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
        let body_doc = match dangling_comments_doc(span.start, comments) {
            Some(tail) => concat(vec![format_block_body(body, comments), hardline(), tail]),
            None => format_block_body(body, comments),
        };
        // Single-expression inline body: `def id -> Int { @id }`, kept only
        // when the author wrote it on the `def` line (see
        // `is_simple_inline_body`). A comment inside the body needs its own
        // line, so the body cannot sit on the signature line.
        if is_simple_inline_body(body, span.line) && !comments.any_within(span) {
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
            concat(vec![
                sig,
                nest(INDENT_WIDTH, concat(vec![hardline(), body_doc])),
                hardline(),
                text("end"),
            ])
        }
    }
}

/// The parts of a `def` header, shared by `def`s, mixin signatures and
/// [`super::render_fn_signature`] (IDE hover and signature help): `async`,
/// `def`, the self mode, the `self.` class-method marker, the name, the
/// generics, the parameters (no parens when there are none) and the return
/// type. `def` precedes the self mode: `var def` does not parse.
pub(super) fn signature_doc(
    is_async: bool,
    self_mode: Option<SelfMode>,
    is_class_method: bool,
    name: &str,
    generics: Option<Doc>,
    params: Vec<Doc>,
    return_type: Option<Doc>,
) -> Vec<Doc> {
    let mut parts = Vec::new();
    // Dropping `async` would silently make an async function synchronous.
    if is_async {
        parts.push(text("async "));
    }
    parts.push(text("def "));
    match self_mode {
        Some(SelfMode::Mutable) => parts.push(text("var ")),
        Some(SelfMode::Consuming) => parts.push(text("consume ")),
        Some(SelfMode::Immutable) | None => {}
    }
    if is_class_method {
        parts.push(text("self."));
    }
    parts.push(text(name.to_string()));
    parts.extend(generics);
    if !params.is_empty() {
        parts.push(format_call_args(params));
    }
    if let Some(rt) = return_type {
        parts.push(text(" -> "));
        parts.push(rt);
    }
    parts
}

/// Comments at the end of a `def` header line (`def init(n: String)  # c`).
/// They trail the header's last node: the return type, else the last
/// parameter.
fn header_trailing_comments(
    params: &[Param],
    return_type: Option<&TypeExpr>,
    comments: &CommentMap,
) -> Vec<Doc> {
    let last = match (return_type, params.last()) {
        (Some(rt), _) => rt.span().start,
        (None, Some(p)) => p.span.start,
        (None, None) => return Vec::new(),
    };
    comments
        .trailing_comments(last)
        .iter()
        .map(|c| line_suffix(concat(vec![text("  "), format_comment(c)])))
        .collect()
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

// ─── Type bodies (class, struct, enum) ──────────────────────────────

/// The `layout <kind>` directives that open a type body (ruby-naming.spec.md
/// §3.5), one per line. The parser stores `layout c` as `C`. The AST keeps no
/// span for them, so they always print first.
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

/// One member of a class / struct / enum body. The AST groups members by
/// kind; the formatter merges them back into source order by span start so
/// `ruxen fmt` never moves an `include` below the methods or drops a member
/// kind it forgot to list.
enum BodyMember<'a> {
    Field(&'a FieldDecl),
    Variant(&'a Variant),
    Method(&'a FuncDef),
    Include(&'a InnerImpl),
    Lib(&'a LibDecl),
    Alias(&'a AliasDef),
    AssocType(&'a AssocTypeBinding),
}

impl BodyMember<'_> {
    fn start(&self) -> usize {
        match self {
            BodyMember::Field(f) => f.span.start,
            BodyMember::Variant(v) => v.span.start,
            BodyMember::Method(m) => m.span.start,
            BodyMember::Include(i) => i.span.start,
            BodyMember::Lib(l) => l.span.start,
            BodyMember::Alias(a) => a.span.start,
            BodyMember::AssocType(a) => a.span.start,
        }
    }

    /// The section visibility the member carries, for the members a
    /// `public` / `private` / `protected` marker applies to.
    fn visibility(&self) -> Option<Visibility> {
        match self {
            BodyMember::Field(f) => Some(f.visibility),
            BodyMember::Method(m) => Some(m.visibility),
            BodyMember::Variant(_)
            | BodyMember::Include(_)
            | BodyMember::Lib(_)
            | BodyMember::Alias(_)
            | BodyMember::AssocType(_) => None,
        }
    }

    fn format(&self, comments: &CommentMap) -> Doc {
        match self {
            BodyMember::Field(f) => {
                with_comments(f.span.start, format_field_decl(f, comments), comments)
            }
            BodyMember::Variant(v) => {
                with_comments(v.span.start, format_variant(v, comments), comments)
            }
            BodyMember::Method(m) => format_func_with_leading_comments(m, comments),
            BodyMember::Include(i) => {
                with_comments(i.span.start, format_inner_impl(i, comments), comments)
            }
            // A class-scoped FFI block reuses `format_lib`, so it round-trips
            // identically to a top-level one.
            BodyMember::Lib(l) => with_comments(l.span.start, format_lib(l, comments), comments),
            BodyMember::Alias(a) => with_comments(a.span.start, format_alias(a), comments),
            BodyMember::AssocType(a) => with_comments(
                a.span.start,
                concat(vec![
                    text("type "),
                    text(a.name.clone()),
                    text(" = "),
                    format_type_expr(&a.type_expr, comments),
                ]),
                comments,
            ),
        }
    }
}

/// Render a type body: `layout` directives, then every member in source
/// order, each separated from the one before by the blank line the author
/// left there, or by none (`CommentMap::blank_line_before`).
///
/// Per-member visibility is NOT surface syntax (ruby-naming.spec.md §3.2):
/// a `public` / `private` / `protected` SECTION MARKER is emitted whenever
/// the running visibility of the fields and methods changes, so a `private`
/// method or field re-parses with its visibility intact. The marker takes
/// the member's place in the layout, on its own line above it.
fn format_type_body(
    header: Vec<Doc>,
    layout: &[String],
    mut members: Vec<BodyMember<'_>>,
    body_start: usize,
    comments: &CommentMap,
) -> Doc {
    members.sort_by_key(BodyMember::start);

    let mut parts: Vec<Doc> = Vec::new();
    let layout_doc = format_layout(layout);
    let has_layout = layout_doc.is_some();
    parts.extend(layout_doc);

    let mut running = Visibility::Public;
    for (i, member) in members.iter().enumerate() {
        if i > 0 || has_layout {
            parts.push(hardline());
            if has_layout && i == 0 || comments.blank_line_before(member.start()) {
                parts.push(hardline());
            }
        }
        if let Some(vis) = member.visibility().filter(|vis| *vis != running) {
            running = vis;
            parts.push(text(visibility_marker(vis)));
            parts.push(hardline());
            if comments.blank_line_after_marker(member.start()) {
                parts.push(hardline());
            }
        }
        parts.push(member.format(comments));
    }

    if let Some(tail) = dangling_comments_doc(body_start, comments) {
        if !members.is_empty() || has_layout {
            parts.push(hardline());
            parts.push(hardline());
        }
        parts.push(tail);
    }

    concat(vec![
        concat(header),
        nest(INDENT_WIDTH, concat(vec![hardline(), concat(parts)])),
        hardline(),
        text("end"),
    ])
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
        assoc_types,
        span,
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
        header.push(format_where_clause(wc, span.line));
    }

    let members = fields
        .iter()
        .map(BodyMember::Field)
        .chain(methods.iter().map(BodyMember::Method))
        .chain(inner_impls.iter().map(BodyMember::Include))
        .chain(lib_decls.iter().map(BodyMember::Lib))
        .chain(aliases.iter().map(BodyMember::Alias))
        .chain(assoc_types.iter().map(BodyMember::AssocType))
        .collect();
    format_type_body(header, layout, members, span.start, comments)
}

fn format_field_decl(field: &FieldDecl, comments: &CommentMap) -> Doc {
    // ruby-naming.spec.md §3.2: fields have NO per-field visibility prefix
    // (`pub`/`private`/`protected` keywords before a field name are not valid
    // surface syntax — `pub` is not even a keyword, only `public` is). Field
    // visibility is expressed through the section markers `format_type_body`
    // emits. A field declaration is just `name: Type`.
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

fn visibility_marker(v: Visibility) -> &'static str {
    match v {
        Visibility::Public => "public",
        Visibility::Protected => "protected",
        Visibility::Private => "private",
    }
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
        span,
    } = s;
    let mut header = vec![text("struct "), text(name.clone())];

    if let Some(gp) = generic_params {
        header.push(format_generic_params(gp));
    }

    if let Some(wc) = where_clause {
        header.push(format_where_clause(wc, span.line));
    }

    let members = fields
        .iter()
        .map(BodyMember::Field)
        .chain(methods.iter().map(BodyMember::Method))
        .chain(inner_impls.iter().map(BodyMember::Include))
        .chain(aliases.iter().map(BodyMember::Alias))
        .collect();
    format_type_body(header, layout, members, span.start, comments)
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
        span,
    } = e;
    let mut header = vec![text("enum "), text(name.clone())];

    if let Some(gp) = generic_params {
        header.push(format_generic_params(gp));
    }

    if let Some(wc) = where_clause {
        header.push(format_where_clause(wc, span.line));
    }

    let members = variants
        .iter()
        .map(BodyMember::Variant)
        .chain(methods.iter().map(BodyMember::Method))
        .chain(inner_impls.iter().map(BodyMember::Include))
        .chain(aliases.iter().map(BodyMember::Alias))
        .collect();
    format_type_body(header, layout, members, span.start, comments)
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
        span,
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
    let mut members: Vec<(usize, Doc)> = Vec::new();
    let mut running = Visibility::Public;
    for item in items {
        let start = match item {
            MixinItem::AssocType { span, .. } => span.start,
            MixinItem::MethodSig(sig) => sig.span.start,
            MixinItem::DefaultMethod(func) => func.span.start,
            MixinItem::Alias(a) => a.span.start,
        };
        let mut doc = format_trait_item(item, comments);
        if let MixinItem::DefaultMethod(func) = item {
            if func.visibility != running {
                doc = concat(vec![
                    text(visibility_marker(func.visibility)),
                    hardline(),
                    doc,
                ]);
                running = func.visibility;
            }
        }
        members.push((start, doc));
    }

    // In-body `lib "X" ... end` FFI blocks (#06.8 follow-up). Mirrors
    // the class-body emission so a mixin-scoped FFI block round-trips.
    for lib in lib_decls {
        members.push((
            lib.span.start,
            with_comments(lib.span.start, format_lib(lib, comments), comments),
        ));
    }

    let mut body_parts = vec![join_members(members, comments)];
    if let Some(tail) = dangling_comments_doc(span.start, comments) {
        body_parts.push(tail);
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
        MixinItem::AssocType { name, span } => {
            with_comments(span.start, text(format!("type {}", name)), comments)
        }
        MixinItem::MethodSig(sig) => {
            with_comments(sig.span.start, format_method_sig(sig, comments), comments)
        }
        MixinItem::DefaultMethod(func) => format_func_with_leading_comments(func, comments),
        MixinItem::Alias(a) => with_comments(a.span.start, format_alias(a), comments),
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
    let mut parts = signature_doc(
        *is_async,
        *self_mode,
        *is_class_method,
        name,
        generic_params.as_ref().map(format_generic_params),
        params.iter().map(|p| format_param(p, comments)).collect(),
        return_type
            .as_ref()
            .map(|rt| format_type_expr(rt, comments)),
    );
    parts.extend(header_trailing_comments(
        params,
        return_type.as_ref(),
        comments,
    ));

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
        span,
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

    let members = items
        .iter()
        .map(|item| (impl_item_start(item), format_impl_item(item, comments)))
        .collect();
    let mut body_parts = vec![join_members(members, comments)];
    body_parts.extend(dangling_comments_doc(span.start, comments));
    let body = join(concat(vec![hardline(), hardline()]), body_parts);

    concat(vec![
        concat(header),
        nest(INDENT_WIDTH, concat(vec![hardline(), body])),
        hardline(),
        text("end"),
    ])
}

/// Where an `extension` / inner-impl body member starts in the source.
fn impl_item_start(item: &ImplItem) -> usize {
    match item {
        ImplItem::AssocType { span, .. } | ImplItem::Include { span, .. } => span.start,
        ImplItem::Method(func) => func.span.start,
        ImplItem::Alias(a) => a.span.start,
    }
}

/// An `extension` / inner-impl body member with its comments.
fn format_impl_item(item: &ImplItem, comments: &CommentMap) -> Doc {
    with_comments(
        impl_item_start(item),
        format_impl_item_bare(item, comments),
        comments,
    )
}

/// Body members, each `(source start, doc)`, one per line with the blank
/// line the author left above each kept (`CommentMap::blank_line_before`).
fn join_members(members: Vec<(usize, Doc)>, comments: &CommentMap) -> Doc {
    let mut parts = Vec::new();
    for (i, (start, doc)) in members.into_iter().enumerate() {
        if i > 0 {
            parts.push(hardline());
            if comments.blank_line_before(start) {
                parts.push(hardline());
            }
        }
        parts.push(doc);
    }
    concat(parts)
}

fn format_impl_item_bare(item: &ImplItem, comments: &CommentMap) -> Doc {
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
        ImplItem::Method(func) => format_prefixed_func(func, comments),
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
        span,
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

    let members = items
        .iter()
        .map(|item| (impl_item_start(item), format_impl_item(item, comments)))
        .collect();
    let mut body_parts = vec![join_members(members, comments)];
    body_parts.extend(dangling_comments_doc(span.start, comments));
    let body = join(concat(vec![hardline(), hardline()]), body_parts);

    concat(vec![
        concat(header),
        nest(INDENT_WIDTH, concat(vec![hardline(), body])),
        hardline(),
        text("end"),
    ])
}

// ─── Modules ────────────────────────────────────────────────────────

fn format_module(m: &ModuleDef, comments: &CommentMap) -> Doc {
    let ModuleDef { name, items, span } = m;
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
                item_docs.push(with_comments(
                    f.span.start,
                    format_func_def(f, comments),
                    comments,
                ));
            }
            other => item_docs.push(with_comments(
                other.span().start,
                format_top_level_item(other, comments),
                comments,
            )),
        }
    }
    item_docs.extend(dangling_comments_doc(span.start, comments));
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
        span,
    } = l;
    // Each FFI `def` with the `##` docs above it (stdlib packages document
    // every alias; dropping them would delete documentation the parser
    // accepts), separated by the blank lines the author left.
    let members = functions
        .iter()
        .map(|func| {
            (
                func.span.start,
                with_comments(
                    func.span.start,
                    format_ffi_function(func, comments),
                    comments,
                ),
            )
        })
        .collect();
    let mut body_parts = vec![join_members(members, comments)];
    body_parts.extend(dangling_comments_doc(span.start, comments));
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
        mutates_self,
        c_symbol,
        params,
        return_type,
        where_clause,
        is_variadic,
        span,
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
    if *mutates_self {
        parts.push(text("var "));
    }

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
        parts.push(format_where_clause(wc, span.line));
    }

    group(concat(parts))
}
