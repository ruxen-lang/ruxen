//! LSP inlay hints (spec §5.5).
//!
//! Two kinds for v1:
//!
//! 1. **Type hints on unannotated `let`.** When the source between the
//!    end of the binding pattern and the `=` does NOT contain a colon,
//!    we emit `: <InferredType>` after the pattern.
//! 2. **Parameter name hints at call sites.** For each `FnCall`/
//!    `MethodCall`, we zip the resolved `FnSignature.params` with the
//!    call's `args` and emit `<param_name>:` before each arg. We skip
//!    when the argument source text is already an identifier equal to
//!    the parameter name (Rust convention — no redundant hints).
//!
//! Both kinds are filtered to those whose position lies inside the
//! requested LSP range — the editor only renders what it can see.

use lsp_types::{InlayHint, InlayHintKind, InlayHintLabel, Position, Range};

use riven_core::hir::nodes::{
    HirClassDef, HirEnumDef, HirExpr, HirExprKind, HirFuncDef, HirImplBlock, HirImplItem,
    HirInterpolationPart, HirItem, HirMixinDef, HirMixinItem, HirModule, HirPattern, HirProgram,
    HirStatement, HirStructDef,
};
use riven_core::hir::types::Ty;
use riven_core::lexer::token::Span;
use riven_core::resolve::symbols::{DefKind, FnSignature, SymbolTable};

use crate::analysis::AnalysisResult;
use crate::line_index::LineIndex;

#[derive(Debug, Clone, Copy)]
pub struct InlayHintConfig {
    pub show_type_hints: bool,
    pub show_param_hints: bool,
}

impl Default for InlayHintConfig {
    fn default() -> Self {
        Self {
            show_type_hints: true,
            show_param_hints: true,
        }
    }
}

/// Compute inlay hints for the program, filtered to the given range.
pub fn inlay_hints(
    result: &AnalysisResult,
    range: Range,
    cfg: &InlayHintConfig,
) -> Vec<InlayHint> {
    let (Some(program), Some(symbols)) = (result.program.as_ref(), result.symbols.as_ref())
    else {
        return Vec::new();
    };

    let mut hints = Vec::new();
    let ctx = Ctx {
        source: &result.source,
        line_index: &result.line_index,
        symbols,
        cfg,
    };
    walk_program(&ctx, program, &mut hints);

    // Filter to hints whose position is within the requested range.
    hints.retain(|h| within(h.position, range));
    hints
}

struct Ctx<'a> {
    source: &'a str,
    line_index: &'a LineIndex,
    symbols: &'a SymbolTable,
    cfg: &'a InlayHintConfig,
}

fn within(pos: Position, range: Range) -> bool {
    let after_start = (pos.line > range.start.line)
        || (pos.line == range.start.line && pos.character >= range.start.character);
    let before_end = (pos.line < range.end.line)
        || (pos.line == range.end.line && pos.character <= range.end.character);
    after_start && before_end
}

// ─── Top-level walk ────────────────────────────────────────────────

fn walk_program(ctx: &Ctx<'_>, program: &HirProgram, out: &mut Vec<InlayHint>) {
    for item in &program.items {
        walk_item(ctx, item, out);
    }
}

fn walk_item(ctx: &Ctx<'_>, item: &HirItem, out: &mut Vec<InlayHint>) {
    match item {
        HirItem::Function(f) => walk_func(ctx, f, out),
        HirItem::Class(c) => walk_class(ctx, c, out),
        HirItem::Struct(s) => walk_struct(ctx, s, out),
        HirItem::Enum(e) => walk_enum(ctx, e, out),
        HirItem::Mixin(m) => walk_mixin(ctx, m, out),
        HirItem::Impl(b) => walk_impl_block(ctx, b, out),
        HirItem::Module(m) => walk_module(ctx, m, out),
        HirItem::Const(c) => walk_expr(ctx, &c.value, out),
        // Type aliases / newtypes carry no executable bodies.
        HirItem::TypeAlias(_) | HirItem::Newtype(_) => {}
    }
}

fn walk_module(ctx: &Ctx<'_>, m: &HirModule, out: &mut Vec<InlayHint>) {
    for item in &m.items {
        walk_item(ctx, item, out);
    }
}

fn walk_func(ctx: &Ctx<'_>, f: &HirFuncDef, out: &mut Vec<InlayHint>) {
    // The `analyze()` pipeline merges bootstrap-loaded stdlib programs
    // (sync.rvn, async runtime, etc.) into the same `HirProgram` as the
    // user's source. Their spans index into different `.rvn` files but
    // share the same `usize` namespace, so a stdlib `let fd = …` would
    // happily produce a hint indexed at byte N of the user's source.
    //
    // Guard by checking whether the function's span actually resolves
    // to its declaration in `ctx.source`. If not, this function was
    // bootstrap-loaded and we skip it wholesale.
    if !span_belongs_to_user_source(ctx.source, &f.span, &f.name) {
        return;
    }
    walk_expr(ctx, &f.body, out);
}

/// A function span belongs to the user source if (a) it fits inside
/// the user source bytes, and (b) the slice at that span contains the
/// function's name — bootstrap-loaded stdlib functions either overshoot
/// `source.len()` or land on unrelated text.
fn span_belongs_to_user_source(source: &str, span: &Span, name: &str) -> bool {
    if span.end > source.len() || span.start >= source.len() {
        return false;
    }
    source[span.start..span.end].contains(name)
}

fn walk_class(ctx: &Ctx<'_>, c: &HirClassDef, out: &mut Vec<InlayHint>) {
    for m in &c.methods {
        walk_func(ctx, m, out);
    }
    for b in &c.impl_blocks {
        walk_impl_block(ctx, b, out);
    }
}

fn walk_struct(ctx: &Ctx<'_>, s: &HirStructDef, out: &mut Vec<InlayHint>) {
    for m in &s.methods {
        walk_func(ctx, m, out);
    }
    for b in &s.impl_blocks {
        walk_impl_block(ctx, b, out);
    }
}

fn walk_enum(ctx: &Ctx<'_>, e: &HirEnumDef, out: &mut Vec<InlayHint>) {
    for m in &e.methods {
        walk_func(ctx, m, out);
    }
    for b in &e.impl_blocks {
        walk_impl_block(ctx, b, out);
    }
}

fn walk_mixin(ctx: &Ctx<'_>, m: &HirMixinDef, out: &mut Vec<InlayHint>) {
    for item in &m.items {
        if let HirMixinItem::DefaultMethod(f) = item {
            walk_func(ctx, f, out);
        }
    }
}

fn walk_impl_block(ctx: &Ctx<'_>, b: &HirImplBlock, out: &mut Vec<InlayHint>) {
    for item in &b.items {
        if let HirImplItem::Method(f) = item {
            walk_func(ctx, f, out);
        }
    }
}

// ─── Expression / statement walk ──────────────────────────────────

fn walk_block_body(
    ctx: &Ctx<'_>,
    stmts: &[HirStatement],
    tail: Option<&HirExpr>,
    out: &mut Vec<InlayHint>,
) {
    for stmt in stmts {
        walk_stmt(ctx, stmt, out);
    }
    if let Some(t) = tail {
        walk_expr(ctx, t, out);
    }
}

fn walk_stmt(ctx: &Ctx<'_>, stmt: &HirStatement, out: &mut Vec<InlayHint>) {
    match stmt {
        HirStatement::Let {
            pattern,
            ty,
            value,
            span,
            ..
        } => {
            if ctx.cfg.show_type_hints {
                emit_type_hint(ctx, pattern, ty, span, out);
            }
            if let Some(v) = value {
                walk_expr(ctx, v, out);
            }
        }
        HirStatement::Expr(e) => walk_expr(ctx, e, out),
    }
}

fn walk_expr(ctx: &Ctx<'_>, expr: &HirExpr, out: &mut Vec<InlayHint>) {
    match &expr.kind {
        HirExprKind::FnCall { callee, args, .. } => {
            if ctx.cfg.show_param_hints {
                if let Some(sig) = sig_of(ctx.symbols, *callee) {
                    emit_param_hints(ctx, sig, args, out);
                }
            }
            for a in args {
                walk_expr(ctx, a, out);
            }
        }
        HirExprKind::MethodCall {
            object,
            method,
            method_name,
            args,
            block,
            ..
        } => {
            walk_expr(ctx, object, out);
            if ctx.cfg.show_param_hints {
                // The resolver leaves `method` as `UNRESOLVED_DEF` for
                // user-class method calls (typeck stores the binding in
                // `TypeContext`, not back on the HIR field). Fall back
                // to resolving by `(object.ty, method_name)` against the
                // symbol table when the direct lookup fails.
                let sig = sig_of(ctx.symbols, *method).or_else(|| {
                    resolve_method_via_receiver(ctx.symbols, &object.ty, method_name)
                });
                if let Some(sig) = sig {
                    emit_param_hints(ctx, sig, args, out);
                }
            }
            for a in args {
                walk_expr(ctx, a, out);
            }
            if let Some(b) = block {
                walk_expr(ctx, b, out);
            }
        }
        HirExprKind::Block(stmts, tail) => {
            walk_block_body(ctx, stmts, tail.as_deref(), out);
        }
        HirExprKind::UnsafeBlock(stmts, tail) => {
            walk_block_body(ctx, stmts, tail.as_deref(), out);
        }
        HirExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            walk_expr(ctx, cond, out);
            walk_expr(ctx, then_branch, out);
            if let Some(e) = else_branch {
                walk_expr(ctx, e, out);
            }
        }
        HirExprKind::Match { scrutinee, arms } => {
            walk_expr(ctx, scrutinee, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    walk_expr(ctx, g, out);
                }
                walk_expr(ctx, &arm.body, out);
            }
        }
        HirExprKind::Loop { body } => walk_expr(ctx, body, out),
        HirExprKind::While { condition, body } => {
            walk_expr(ctx, condition, out);
            walk_expr(ctx, body, out);
        }
        HirExprKind::For { iterable, body, .. } => {
            walk_expr(ctx, iterable, out);
            walk_expr(ctx, body, out);
        }
        HirExprKind::BinaryOp { left, right, .. } => {
            walk_expr(ctx, left, out);
            walk_expr(ctx, right, out);
        }
        HirExprKind::UnaryOp { operand, .. } => walk_expr(ctx, operand, out),
        HirExprKind::Borrow { expr, .. } => walk_expr(ctx, expr, out),
        HirExprKind::Assign { target, value, .. } => {
            walk_expr(ctx, target, out);
            walk_expr(ctx, value, out);
        }
        HirExprKind::CompoundAssign { target, value, .. } => {
            walk_expr(ctx, target, out);
            walk_expr(ctx, value, out);
        }
        HirExprKind::Return(e) | HirExprKind::Break(e) => {
            if let Some(e) = e {
                walk_expr(ctx, e, out);
            }
        }
        HirExprKind::Closure { body, .. } => walk_expr(ctx, body, out),
        HirExprKind::Construct { fields, .. } => {
            for (_, v) in fields {
                walk_expr(ctx, v, out);
            }
        }
        HirExprKind::EnumVariant { fields, .. } => {
            for (_, v) in fields {
                walk_expr(ctx, v, out);
            }
        }
        HirExprKind::Tuple(xs) | HirExprKind::ArrayLiteral(xs) => {
            for x in xs {
                walk_expr(ctx, x, out);
            }
        }
        HirExprKind::MapLiteral(pairs) => {
            for (k, v) in pairs {
                walk_expr(ctx, k, out);
                walk_expr(ctx, v, out);
            }
        }
        HirExprKind::Index { object, index } => {
            walk_expr(ctx, object, out);
            walk_expr(ctx, index, out);
        }
        HirExprKind::FieldAccess { object, .. } => walk_expr(ctx, object, out),
        HirExprKind::Cast { expr, .. } => walk_expr(ctx, expr, out),
        HirExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                walk_expr(ctx, s, out);
            }
            if let Some(e) = end {
                walk_expr(ctx, e, out);
            }
        }
        HirExprKind::Interpolation { parts } => {
            for p in parts {
                if let HirInterpolationPart::Expr { expr, .. } = p {
                    walk_expr(ctx, expr, out);
                }
            }
        }
        HirExprKind::ArrayFill { value, .. } => walk_expr(ctx, value, out),
        HirExprKind::MacroCall { args, .. } => {
            for a in args {
                walk_expr(ctx, a, out);
            }
        }
        // Leaves
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

// ─── Hint emitters ────────────────────────────────────────────────

fn emit_type_hint(
    ctx: &Ctx<'_>,
    pattern: &HirPattern,
    ty: &Ty,
    stmt_span: &Span,
    out: &mut Vec<InlayHint>,
) {
    // We only handle the bare-binding case for v1. Destructuring lets
    // (`let (a, b) = ...`) carry their own per-element types and would
    // need composite hints — out of scope here.
    let binding_span = match pattern {
        HirPattern::Binding { span, .. } => span,
        _ => return,
    };

    // Skip when typeck didn't resolve the type — a hint of `: ?` is noise.
    if !ty_is_renderable(ty) {
        return;
    }

    // Inspect source between the end of the binding name and the end
    // of the statement: if there is a `:` before any `=`, the let is
    // already annotated.
    let after_name = binding_span.end.min(ctx.source.len());
    let stmt_end = stmt_span.end.min(ctx.source.len());
    if after_name < stmt_end {
        let tail = &ctx.source[after_name..stmt_end];
        // Take everything before the first `=` (or whole tail if none).
        let header = match tail.find('=') {
            Some(i) => &tail[..i],
            None => tail,
        };
        if header.contains(':') {
            return;
        }
    }

    let position = ctx.line_index.position_of(binding_span.end);
    out.push(InlayHint {
        position,
        label: InlayHintLabel::String(format!(": {}", ty)),
        kind: Some(InlayHintKind::TYPE),
        text_edits: None,
        tooltip: None,
        padding_left: None,
        padding_right: None,
        data: None,
    });
}

fn emit_param_hints(
    ctx: &Ctx<'_>,
    sig: &FnSignature,
    args: &[HirExpr],
    out: &mut Vec<InlayHint>,
) {
    // Zip param-by-param. The signature may carry an implicit `self`
    // for methods — `ParamInfo` only covers the explicit params, so the
    // zip naturally lines up. Skip variadic tails we don't know about.
    let n = sig.params.len().min(args.len());
    for i in 0..n {
        let param = &sig.params[i];
        let arg = &args[i];
        if param.name.is_empty() || param.name.starts_with('_') {
            continue;
        }
        if arg_text_matches_param(ctx.source, &arg.span, &param.name) {
            continue;
        }
        let position = ctx.line_index.position_of(arg.span.start);
        out.push(InlayHint {
            position,
            label: InlayHintLabel::String(format!("{}:", param.name)),
            kind: Some(InlayHintKind::PARAMETER),
            text_edits: None,
            tooltip: None,
            padding_left: None,
            padding_right: Some(true),
            data: None,
        });
    }
}

// ─── Helpers ──────────────────────────────────────────────────────

fn sig_of(symbols: &SymbolTable, def_id: riven_core::hir::nodes::DefId) -> Option<&FnSignature> {
    match &symbols.get(def_id)?.kind {
        DefKind::Function { signature } | DefKind::Method { signature, .. } => Some(signature),
        _ => None,
    }
}

/// Look up a method signature by (receiver type, method name) when the
/// `MethodCall.method` field is unresolved. We walk the symbol table
/// for a `Class` matching the receiver, then iterate its declared
/// methods looking for one whose name matches.
fn resolve_method_via_receiver<'a>(
    symbols: &'a SymbolTable,
    receiver_ty: &Ty,
    method_name: &str,
) -> Option<&'a FnSignature> {
    let type_name = match peel_refs(receiver_ty) {
        Ty::Class { name, .. } => name.clone(),
        // For v1 we only resolve methods on classes — `Struct`/`Enum`
        // method tables aren't surfaced on `StructInfo`/`EnumInfo`, and
        // primitive receivers route through mixin dispatch which is
        // out of scope for inlay hints.
        _ => return None,
    };

    let method_def_ids = symbols.iter().find_map(|def| match &def.kind {
        DefKind::Class { info } if def.name == type_name => Some(info.methods.clone()),
        _ => None,
    })?;

    for mid in method_def_ids {
        let m = symbols.get(mid)?;
        if m.name == method_name {
            if let DefKind::Method { signature, .. } = &m.kind {
                return Some(signature);
            }
        }
    }
    None
}

fn peel_refs(ty: &Ty) -> &Ty {
    match ty {
        Ty::Ref(inner)
        | Ty::RefMut(inner)
        | Ty::RefLifetime(_, inner)
        | Ty::RefMutLifetime(_, inner) => peel_refs(inner),
        _ => ty,
    }
}

/// Does the source text at `span` look like a bare identifier equal to
/// `name`? Used to skip redundant `name: name` hints.
fn arg_text_matches_param(source: &str, span: &Span, name: &str) -> bool {
    let start = span.start.min(source.len());
    let end = span.end.min(source.len());
    if start >= end {
        return false;
    }
    let text = source[start..end].trim();
    text == name
}

fn ty_is_renderable(ty: &Ty) -> bool {
    // Skip the two cases where a hint would be unhelpful or wrong.
    // `Infer` means typeck couldn't resolve; `Error` means upstream
    // diagnostic already fired. Both should suppress the hint.
    !matches!(ty, Ty::Infer(_) | Ty::Error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;

    fn full_range() -> Range {
        Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: u32::MAX,
                character: u32::MAX,
            },
        }
    }

    #[test]
    fn empty_program_has_no_hints() {
        let result = analyze("");
        let hints = inlay_hints(&result, full_range(), &InlayHintConfig::default());
        assert!(hints.is_empty());
    }

    #[test]
    fn parse_error_returns_no_hints() {
        let result = analyze("def\n");
        let hints = inlay_hints(&result, full_range(), &InlayHintConfig::default());
        assert!(hints.is_empty());
    }

    #[test]
    fn within_filters_by_position() {
        let r = Range {
            start: Position {
                line: 1,
                character: 0,
            },
            end: Position {
                line: 2,
                character: 0,
            },
        };
        assert!(within(
            Position {
                line: 1,
                character: 5
            },
            r
        ));
        assert!(!within(
            Position {
                line: 0,
                character: 5
            },
            r
        ));
        assert!(!within(
            Position {
                line: 3,
                character: 5
            },
            r
        ));
    }
}
