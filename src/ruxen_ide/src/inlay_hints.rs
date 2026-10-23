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

use ruxen_core::hir::nodes::{
    HirExpr, HirExprKind, HirFuncDef, HirPattern, HirProgram, HirStatement,
};
use ruxen_core::hir::types::Ty;
use ruxen_core::hir::visit::{walk_expr, walk_func, walk_stmt, Visit};
use ruxen_core::lexer::token::Span;
use ruxen_core::resolve::symbols::{DefKind, FnSignature, SymbolTable};

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
pub fn inlay_hints(result: &AnalysisResult, range: Range, cfg: &InlayHintConfig) -> Vec<InlayHint> {
    let (Some(program), Some(symbols)) = (result.program.as_ref(), result.symbols.as_ref()) else {
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

// ─── Walk ──────────────────────────────────────────────────────────

fn walk_program(ctx: &Ctx<'_>, program: &HirProgram, out: &mut Vec<InlayHint>) {
    let mut collector = HintCollector { ctx, out };
    for item in &program.items {
        collector.visit_item(item);
    }
}

struct HintCollector<'c, 'a> {
    ctx: &'c Ctx<'a>,
    out: &'c mut Vec<InlayHint>,
}

impl Visit for HintCollector<'_, '_> {
    fn visit_func(&mut self, f: &HirFuncDef) {
        // The `analyze()` pipeline merges bootstrap-loaded stdlib programs
        // (sync.rx, async runtime, etc.) into the same `HirProgram` as the
        // user's source. Their spans index into different `.rx` files but
        // share the same `usize` namespace, so a stdlib `let fd = …` would
        // happily produce a hint indexed at byte N of the user's source.
        //
        // Guard by checking whether the function's span actually resolves
        // to its declaration in `ctx.source`. If not, this function was
        // bootstrap-loaded and we skip it wholesale.
        if !span_belongs_to_user_source(self.ctx.source, &f.span, &f.name) {
            return;
        }
        walk_func(self, f);
    }

    fn visit_stmt(&mut self, stmt: &HirStatement) {
        if let HirStatement::Let {
            pattern, ty, span, ..
        } = stmt
        {
            if self.ctx.cfg.show_type_hints {
                emit_type_hint(self.ctx, pattern, ty, span, self.out);
            }
        }
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &HirExpr) {
        if self.ctx.cfg.show_param_hints {
            let symbols = self.ctx.symbols;
            match &expr.kind {
                HirExprKind::FnCall { callee, args, .. } => {
                    if let Some(sig) = sig_of(symbols, *callee) {
                        emit_param_hints(self.ctx, sig, args, self.out);
                    }
                }
                HirExprKind::MethodCall {
                    object,
                    method,
                    method_name,
                    args,
                    ..
                } => {
                    // The resolver leaves `method` as `UNRESOLVED_DEF` for
                    // user-class method calls (typeck stores the binding in
                    // `TypeContext`, not back on the HIR field). Fall back
                    // to resolving by `(object.ty, method_name)` against the
                    // symbol table when the direct lookup fails.
                    let sig = sig_of(symbols, *method)
                        .or_else(|| resolve_method_via_receiver(symbols, &object.ty, method_name));
                    if let Some(sig) = sig {
                        emit_param_hints(self.ctx, sig, args, self.out);
                    }
                }
                // Only calls take parameter hints.
                _ => {}
            }
        }
        walk_expr(self, expr);
    }
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

fn emit_param_hints(ctx: &Ctx<'_>, sig: &FnSignature, args: &[HirExpr], out: &mut Vec<InlayHint>) {
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

fn sig_of(symbols: &SymbolTable, def_id: ruxen_core::hir::nodes::DefId) -> Option<&FnSignature> {
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
