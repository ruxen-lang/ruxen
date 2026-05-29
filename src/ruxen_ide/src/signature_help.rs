//! LSP signature help — phase 1D per `docs/requirements/tier3_01_lsp.md`
//! (the `signature_help` capability sketched at §5.1 and triggered
//! after `(` / `,` per §5.4 paragraph 4).
//!
//! Behaviour:
//!
//! - Determine whether the cursor sits inside a call's argument list by
//!   walking BACKWARD through the source text balancing delimiters
//!   (`()`, `[]`, `{}`, string literals). The walk terminates at the
//!   first unmatched `(`. If none is found, return `None`.
//! - Find the innermost `HirExprKind::FnCall` / `MethodCall` whose
//!   span surrounds that unmatched `(` — the unique HIR call that
//!   *owns* the open paren we just located.
//! - Look up the callee's `FnSignature` in the symbol table.
//! - Build a `SignatureInformation` (label + parameter list) and set
//!   `active_parameter` to the number of top-level commas between the
//!   `(` and the cursor.
//!
//! Returns `None` cleanly whenever any of those steps fails — an
//! unresolved callee, a parser-incomplete call site, or a cursor that
//! lives outside any call. No panics.

use lsp_types::{
    Documentation, ParameterInformation, ParameterLabel, Position, SignatureHelp,
    SignatureInformation,
};
use ruxen_core::hir::nodes::{DefId, HirExpr, HirExprKind, HirItem, HirProgram, HirStatement};
use ruxen_core::resolve::symbols::{DefKind, FnSignature, SymbolTable};

use crate::analysis::AnalysisResult;

/// Public entry. Returns a `SignatureHelp` when the cursor lives
/// inside a call we can resolve; `None` otherwise.
pub fn signature_help(result: &AnalysisResult, position: Position) -> Option<SignatureHelp> {
    let symbols = result.symbols.as_ref()?;
    let program = result.program.as_ref()?;
    let source = result.source.as_str();
    let cursor = result.line_index.byte_offset_of(position);

    // Step 1: find the enclosing call's `(` and the active-param index.
    let ctx = enclosing_call_context(source, cursor)?;

    // Step 2: locate the HIR call node whose span owns that `(`.
    let (callee_name, signature) = lookup_call_signature(program, symbols, ctx.paren_offset)?;

    // Step 3: build the LSP shape.
    let label = format_signature(&callee_name, &signature);
    let parameters = build_parameter_information(&label, &callee_name, &signature);
    let active_parameter = clamp_active_param(ctx.active_param, parameters.len());

    let info = SignatureInformation {
        label,
        documentation: signature_doc(&signature),
        parameters: Some(parameters),
        active_parameter: Some(active_parameter as u32),
    };

    Some(SignatureHelp {
        signatures: vec![info],
        active_signature: Some(0),
        active_parameter: Some(active_parameter as u32),
    })
}

// ─── Source-text context walker ────────────────────────────────────

struct CallContext {
    /// Byte offset of the `(` that opens the enclosing call.
    paren_offset: usize,
    /// Zero-based index of the argument the cursor currently sits in.
    active_param: usize,
}

/// Walk backward from `cursor` through balanced delimiters. The first
/// unmatched `(` we hit is the open-paren of the enclosing call. While
/// walking we count the top-level commas — they tell us which
/// argument the cursor lives in.
///
/// String/char literals and line comments are treated as opaque so a
/// `,` inside `"a, b"` doesn't shift the active parameter.
fn enclosing_call_context(source: &str, cursor: usize) -> Option<CallContext> {
    let bytes = source.as_bytes();
    let cursor = cursor.min(bytes.len());

    // Brackets we balance while walking BACKWARD: when we see a
    // closing `)`/`]`/`}` going backward, we're stepping out of an
    // inner pair and must skip its matching open. Tracked as a depth
    // counter per kind.
    let mut paren_depth: i32 = 0;
    let mut square_depth: i32 = 0;
    let mut brace_depth: i32 = 0;

    // Commas counted at the candidate call's top level. We accumulate
    // them while walking back, then commit when we find the unmatched
    // `(`. Resets if a deeper `(` would otherwise mask the count.
    let mut commas: usize = 0;

    let mut i = cursor;
    while i > 0 {
        i -= 1;
        let b = bytes[i];

        // Skip the body of a string / char literal — a `,` or `(`
        // inside text mustn't count. We detect the closing quote
        // FIRST (since we walk backward) then scan back to its
        // matching opener.
        if b == b'"' || b == b'\'' {
            if let Some(open) = scan_back_to_string_open(bytes, i, b) {
                i = open; // landing here, next loop iter decrements past it
                continue;
            } else {
                // Unterminated literal — bail; can't classify safely.
                return None;
            }
        }

        match b {
            b')' => paren_depth += 1,
            b']' => square_depth += 1,
            b'}' => brace_depth += 1,
            b'(' => {
                if paren_depth == 0 {
                    // Unmatched open paren — this is our call's `(`.
                    return Some(CallContext {
                        paren_offset: i,
                        active_param: commas,
                    });
                }
                paren_depth -= 1;
            }
            b'[' => {
                if square_depth == 0 {
                    // Cursor is inside an array/index literal, not a
                    // call argument list. Stop classifying.
                    return None;
                }
                square_depth -= 1;
            }
            b'{' => {
                if brace_depth == 0 {
                    // Cursor is inside a block/struct/closure literal
                    // — not a call argument list.
                    return None;
                }
                brace_depth -= 1;
            }
            b',' => {
                if paren_depth == 0 && square_depth == 0 && brace_depth == 0 {
                    commas += 1;
                }
            }
            // Newlines are legal inside arg lists in Ruxen, so we do
            // NOT bail here. Ruxen uses no `;` statement separator.
            b'\n' => {}
            _ => {}
        }
    }

    // Walked all the way to the start of the file without finding an
    // unmatched `(` — cursor is not in any call.
    None
}

/// Given the position of a closing `"` or `'`, find the matching opener
/// to its left. Handles `\"` / `\'` escapes naively (skip the prev byte
/// when the char before it isn't a backslash run of even length —
/// for v1 we accept "good enough" since this only governs whether to
/// skip the literal body).
fn scan_back_to_string_open(bytes: &[u8], close: usize, quote: u8) -> Option<usize> {
    let mut j = close;
    while j > 0 {
        j -= 1;
        if bytes[j] == quote && !is_escaped(bytes, j) {
            return Some(j);
        }
    }
    None
}

fn is_escaped(bytes: &[u8], at: usize) -> bool {
    let mut backslashes = 0usize;
    let mut k = at;
    while k > 0 && bytes[k - 1] == b'\\' {
        backslashes += 1;
        k -= 1;
    }
    backslashes % 2 == 1
}

// ─── HIR lookup ────────────────────────────────────────────────────

/// Find the innermost call expression whose span CONTAINS `paren_offset`
/// (i.e. the call owns this `(`). Returns the callee's display name and
/// its `FnSignature`.
///
/// For `FnCall`, the DefId stored in the HIR is the resolved callee —
/// we look it up directly. For `MethodCall`, the HIR keeps the
/// `method` field as `UNRESOLVED_DEF` (typeck doesn't write it back),
/// so we resolve the method by walking the receiver's `Ty` to its
/// class and searching the class's method list by name — same shape
/// the after-`.` completion uses.
fn lookup_call_signature(
    program: &HirProgram,
    symbols: &SymbolTable,
    paren_offset: usize,
) -> Option<(String, FnSignature)> {
    let mut finder = CallFinder {
        target: paren_offset,
        best: None,
    };
    finder.visit_program(program);
    let candidate = finder.best?;
    match candidate {
        Callee::Fn { def_id, name } => {
            let def = symbols.get(def_id)?;
            match &def.kind {
                DefKind::Function { signature } => Some((name, signature.clone())),
                _ => None,
            }
        }
        Callee::Method {
            receiver_ty,
            method_name,
        } => {
            let class_name = class_name_of(&receiver_ty)?;
            for def in symbols.iter() {
                if def.name != method_name {
                    continue;
                }
                let DefKind::Method { parent, signature } = &def.kind else {
                    continue;
                };
                let Some(parent_def) = symbols.get(*parent) else {
                    continue;
                };
                if parent_def.name == class_name {
                    return Some((method_name, signature.clone()));
                }
            }
            None
        }
    }
}

/// Pulls the class/struct/enum name out of a `Ty`. Peels `Ref` /
/// `RefMut` / aliases / newtypes — mirrors `completion::class_name_of`
/// (kept private to this module for the same self-contained reason).
fn class_name_of(ty: &ruxen_core::hir::types::Ty) -> Option<String> {
    use ruxen_core::hir::types::Ty;
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

/// Discriminated record of how to resolve the callee's signature.
/// Free `FnCall` carries a resolved DefId; `MethodCall` carries the
/// receiver's `Ty` + method name (HIR doesn't keep the resolved method
/// DefId, so we walk by name + parent class).
enum Callee {
    Fn {
        def_id: DefId,
        name: String,
    },
    Method {
        receiver_ty: ruxen_core::hir::types::Ty,
        method_name: String,
    },
}

struct CallFinder {
    target: usize,
    /// Best match so far — pick the smallest enclosing call (innermost).
    best: Option<Callee>,
}

impl CallFinder {
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
                for imp in &class.impl_blocks {
                    for it in &imp.items {
                        if let ruxen_core::hir::nodes::HirImplItem::Method(func) = it {
                            self.visit_expr(&func.body);
                        }
                    }
                }
            }
            HirItem::Impl(imp) => {
                for it in &imp.items {
                    if let ruxen_core::hir::nodes::HirImplItem::Method(func) = it {
                        self.visit_expr(&func.body);
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
        // Only descend if the target falls inside this expression's
        // span — saves us walking every sibling subtree.
        if expr.span.start > self.target || expr.span.end <= self.target {
            return;
        }

        match &expr.kind {
            HirExprKind::FnCall {
                callee,
                callee_name,
                args,
            } => {
                // Record this call as the best so far — a later, more
                // deeply-nested call will overwrite if it also
                // contains `target`. (We're walking outside-in, so the
                // last write wins.)
                if *callee != ruxen_core::hir::nodes::UNRESOLVED_DEF {
                    self.best = Some(Callee::Fn {
                        def_id: *callee,
                        name: callee_name.clone(),
                    });
                }
                for a in args {
                    self.visit_expr(a);
                }
            }
            HirExprKind::MethodCall {
                object,
                method_name,
                args,
                block,
                ..
            } => {
                // The receiver's `Ty` was filled in by typeck. We carry
                // it (rather than a DefId) because the MethodCall HIR
                // node doesn't store the resolved method DefId — see
                // `lookup_call_signature` for the by-name resolution.
                self.best = Some(Callee::Method {
                    receiver_ty: object.ty.clone(),
                    method_name: method_name.clone(),
                });
                self.visit_expr(object);
                for a in args {
                    self.visit_expr(a);
                }
                if let Some(b) = block {
                    self.visit_expr(b);
                }
            }
            HirExprKind::FieldAccess { object, .. } => self.visit_expr(object),
            HirExprKind::BinaryOp { left, right, .. } => {
                self.visit_expr(left);
                self.visit_expr(right);
            }
            HirExprKind::UnaryOp { operand, .. } => self.visit_expr(operand),
            HirExprKind::Borrow { expr: inner, .. } => self.visit_expr(inner),
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
                for (_n, v) in fields {
                    self.visit_expr(v);
                }
            }
            HirExprKind::Tuple(elems) | HirExprKind::ArrayLiteral(elems) => {
                for e in elems {
                    self.visit_expr(e);
                }
            }
            HirExprKind::MapLiteral(entries) => {
                for (k, v) in entries {
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
                for part in parts {
                    if let ruxen_core::hir::nodes::HirInterpolationPart::Expr { expr: e, .. } = part
                    {
                        self.visit_expr(e);
                    }
                }
            }
            HirExprKind::MacroCall { args, .. } => {
                for a in args {
                    self.visit_expr(a);
                }
            }
            HirExprKind::UnsafeBlock(stmts, tail) => {
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
            // Terminal kinds — nothing to recurse into.
            HirExprKind::VarRef(_)
            | HirExprKind::IntLiteral(_)
            | HirExprKind::FloatLiteral(_)
            | HirExprKind::StringLiteral(_)
            | HirExprKind::BoolLiteral(_)
            | HirExprKind::CharLiteral(_)
            | HirExprKind::UnitLiteral
            | HirExprKind::NullLiteral
            | HirExprKind::RegexLiteral { .. }
            | HirExprKind::Continue
            | HirExprKind::Return(None)
            | HirExprKind::Break(None)
            | HirExprKind::Error => {}
        }
    }
}

// ─── Signature formatting ──────────────────────────────────────────

/// Render the call's label exactly as a user would write it. We keep
/// it close to `format_signature` in `completion.rs` so the two
/// surfaces look consistent.
fn format_signature(name: &str, sig: &FnSignature) -> String {
    // Delegate to the shared renderer so signature help, hover, and
    // `ruxen fmt` always agree on signature shape. `build_parameter_information`
    // still locates each `name: type` substring in this label for active-param
    // highlighting, which the shared renderer preserves.
    crate::signature_render::render(name, sig)
}

/// Build per-parameter `ParameterInformation` entries whose `label`
/// field carries the precise byte range into the signature label.
/// LSP clients render the active param bolded by reading these
/// ranges, so they must match the substring exactly.
fn build_parameter_information(
    label: &str,
    name: &str,
    sig: &FnSignature,
) -> Vec<ParameterInformation> {
    let mut out = Vec::with_capacity(sig.params.len());

    // Locate the `(` past the function name to anchor the scan. If we
    // can't find it (shouldn't happen with the format above) fall
    // back to label-start.
    let scan_start = label
        .find(&format!("{}(", name))
        .map(|i| i + name.len() + 1)
        .unwrap_or(0);
    let mut cursor = scan_start;

    for p in &sig.params {
        let needle = format!("{}: {}", p.name, p.ty);
        if let Some(rel) = label[cursor..].find(&needle) {
            let start = cursor + rel;
            let end = start + needle.len();
            out.push(ParameterInformation {
                label: ParameterLabel::LabelOffsets([start as u32, end as u32]),
                documentation: None,
            });
            cursor = end;
        } else {
            // Defensive fallback — shouldn't be reached unless the
            // format helper diverges from this one. Use a simple
            // String label so we still surface the param name.
            out.push(ParameterInformation {
                label: ParameterLabel::Simple(needle),
                documentation: None,
            });
        }
    }
    out
}

/// Optional doc string — for v1 we don't surface anything (no
/// doc-comment harvesting yet), but the `Documentation` field is
/// where it would go. `None` keeps the signature panel uncluttered.
fn signature_doc(_sig: &FnSignature) -> Option<Documentation> {
    None
}

/// Clamp the active-param index to `params.len() - 1` so a trailing
/// comma past the last declared param (e.g. variadic-style usage)
/// doesn't push the index out of range. An empty param list yields
/// `0` — the LSP spec allows it even with no parameters.
fn clamp_active_param(commas_before_cursor: usize, param_count: usize) -> usize {
    if param_count == 0 {
        0
    } else if commas_before_cursor >= param_count {
        param_count - 1
    } else {
        commas_before_cursor
    }
}
