//! `textDocument/references` — find every place a symbol is mentioned.
//!
//! Spec: `docs/requirements/tier3_01_lsp.md` §5.6.
//!
//! Strategy: resolve the cursor to a `DefId` via `node_finder`, then
//! look up the pre-built `UseIndex` instead of re-walking the HIR. The
//! `UseIndex` contract guarantees `uses[def]` is `[decl_span, use1,
//! use2, ...]`, so honoring `include_declaration` is a single
//! conditional skip of the first entry.
//!
//! Method calls are special: typeck leaves
//! `HirExprKind::MethodCall::method` as `UNRESOLVED_DEF` (resolution is
//! deferred to MIR — see `project_riven_mir_two_dispatch_paths.md`).
//! When the cursor lands on such a call, the `NodeAtPosition::MethodCall`
//! carries `method = UNRESOLVED_DEF`. We recover the real DefId the
//! same way `signature_help` / `use_index` do: walk the receiver's
//! resolved `Ty` to its class and find the method by name.

use lsp_types::{Location, Position, Url};

use riven_core::hir::nodes::{DefId, HirExpr, HirExprKind, HirProgram, UNRESOLVED_DEF};
use riven_core::hir::types::Ty;
use riven_core::lexer::token::Span;
use riven_core::resolve::symbols::{DefKind, SymbolTable};

use crate::analysis::AnalysisResult;
use crate::node_finder::{node_at_position, NodeAtPosition};

/// Find every reference to the symbol at `position`.
///
/// Returns an empty `Vec` when:
/// - the cursor doesn't land on a resolvable name,
/// - the resolved def is synthetic / `__`-prefixed,
/// - analysis stopped before typeck (no `use_index`).
///
/// Each `Location.uri` is a placeholder (`file:///__placeholder__`).
/// The LSP handler rewrites it to the actual document URI — references
/// in v1 are single-file (workspace-wide ref hunt is a Phase 2 item).
pub fn references(
    result: &AnalysisResult,
    position: Position,
    include_declaration: bool,
) -> Vec<Location> {
    let program = match result.program.as_ref() {
        Some(p) => p,
        None => return Vec::new(),
    };
    let symbols = match result.symbols.as_ref() {
        Some(s) => s,
        None => return Vec::new(),
    };
    let use_index = match result.use_index.as_ref() {
        Some(u) => u,
        None => return Vec::new(),
    };

    let byte_offset = result.line_index.byte_offset_of(position);
    let node = match node_at_position(program, byte_offset) {
        Some(n) => n,
        None => return Vec::new(),
    };

    let def_id = match resolve_node_def(&node, program, symbols, byte_offset) {
        Some(id) => id,
        None => return Vec::new(),
    };

    if def_id == UNRESOLVED_DEF {
        return Vec::new();
    }

    // Guard against synthetic / compiler-internal defs leaking through.
    // The UseIndex already drops `__`-prefixed and zero-span defs at
    // build time, so an entry here means it's user-visible — but we
    // double-check the name to keep this layer self-contained.
    if let Some(def) = symbols.get(def_id) {
        if def.name.starts_with("__") {
            return Vec::new();
        }
    }

    let spans = match use_index.uses.get(&def_id) {
        Some(v) => v.as_slice(),
        None => return Vec::new(),
    };

    let start = if include_declaration { 0 } else { 1 };
    let placeholder_uri = Url::parse("file:///__placeholder__").expect("placeholder URL parses");

    spans
        .iter()
        .skip(start)
        .filter(|s| !is_synthetic_span(s))
        .map(|s| Location {
            uri: placeholder_uri.clone(),
            range: result.line_index.span_to_range(s),
        })
        .collect()
}

/// Pull a `DefId` out of a `NodeAtPosition`, falling back to a
/// name-based method lookup when the HIR didn't resolve the method.
///
/// Returns `None` for nodes that carry no def (whitespace, field
/// access, type ref).
fn resolve_node_def(
    node: &NodeAtPosition,
    program: &HirProgram,
    symbols: &SymbolTable,
    byte_offset: usize,
) -> Option<DefId> {
    match node {
        NodeAtPosition::VarRef(id, _) => Some(*id),
        NodeAtPosition::FnCall { callee, .. } => Some(*callee),
        NodeAtPosition::MethodCall { method, span } => {
            if *method != UNRESOLVED_DEF {
                return Some(*method);
            }
            // Recover the method DefId by finding the enclosing
            // MethodCall expression and walking its receiver type. This
            // mirrors signature_help's fallback path.
            resolve_method_call_def(program, symbols, byte_offset, span)
        }
        NodeAtPosition::Definition(id, _) => Some(*id),
        NodeAtPosition::TypeRef { .. } | NodeAtPosition::FieldAccess { .. } => None,
    }
}

/// Walk the HIR to find the innermost `MethodCall` whose span contains
/// `target` and whose own span matches `expected_span`, then resolve
/// its method by name against the receiver's resolved class.
fn resolve_method_call_def(
    program: &HirProgram,
    symbols: &SymbolTable,
    target: usize,
    expected_span: &Span,
) -> Option<DefId> {
    let mut finder = MethodCallFinder {
        target,
        expected_span: expected_span.clone(),
        result: None,
    };
    finder.visit_program(program);
    let (receiver_ty, method_name) = finder.result?;
    lookup_method(symbols, &receiver_ty, &method_name)
}

/// Tiny HIR visitor that finds a `MethodCall` matching a span and
/// captures its receiver type + method name.
struct MethodCallFinder {
    target: usize,
    expected_span: Span,
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

    fn visit_item(&mut self, item: &riven_core::hir::nodes::HirItem) {
        use riven_core::hir::nodes::{HirImplItem, HirItem, HirMixinItem};
        match item {
            HirItem::Function(f) => self.visit_expr(&f.body),
            HirItem::Class(c) => {
                for m in &c.methods {
                    self.visit_expr(&m.body);
                }
                for b in &c.impl_blocks {
                    for it in &b.items {
                        if let HirImplItem::Method(m) = it {
                            self.visit_expr(&m.body);
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
                        if let HirImplItem::Method(m) = it {
                            self.visit_expr(&m.body);
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
                        if let HirImplItem::Method(m) = it {
                            self.visit_expr(&m.body);
                        }
                    }
                }
            }
            HirItem::Mixin(t) => {
                for it in &t.items {
                    if let HirMixinItem::DefaultMethod(m) = it {
                        self.visit_expr(&m.body);
                    }
                }
            }
            HirItem::Impl(b) => {
                for it in &b.items {
                    if let HirImplItem::Method(m) = it {
                        self.visit_expr(&m.body);
                    }
                }
            }
            HirItem::Module(m) => {
                for it in &m.items {
                    self.visit_item(it);
                }
            }
            HirItem::Const(c) => self.visit_expr(&c.value),
            HirItem::TypeAlias(_) | HirItem::Newtype(_) => {}
        }
    }

    fn visit_expr(&mut self, expr: &HirExpr) {
        if self.result.is_some() {
            return;
        }
        if !self.contains(&expr.span) {
            return;
        }
        if let HirExprKind::MethodCall {
            object,
            method_name,
            args,
            block,
            ..
        } = &expr.kind
        {
            // Recurse into the object & args first to find the
            // innermost match.
            self.visit_expr(object);
            for a in args {
                self.visit_expr(a);
            }
            if let Some(b) = block {
                self.visit_expr(b);
            }
            if self.result.is_some() {
                return;
            }
            if expr.span.start == self.expected_span.start
                && expr.span.end == self.expected_span.end
            {
                self.result = Some((object.ty.clone(), method_name.clone()));
            }
            return;
        }
        // Descend into all child expressions generically.
        self.descend_children(expr);
    }

    fn descend_children(&mut self, expr: &HirExpr) {
        use riven_core::hir::nodes::{HirInterpolationPart, HirStatement};
        match &expr.kind {
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
            HirExprKind::Borrow { expr: inner, .. } => self.visit_expr(inner),
            HirExprKind::Block(stmts, tail) | HirExprKind::UnsafeBlock(stmts, tail) => {
                for s in stmts {
                    match s {
                        HirStatement::Let { value, pattern, .. } => {
                            self.visit_pattern(pattern);
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
                    self.visit_pattern(&arm.pattern);
                    if let Some(g) = &arm.guard {
                        self.visit_expr(g);
                    }
                    self.visit_expr(&arm.body);
                }
            }
            HirExprKind::Loop { body } => self.visit_expr(body),
            HirExprKind::While { condition, body } => {
                self.visit_expr(condition);
                self.visit_expr(body);
            }
            HirExprKind::For { iterable, body, .. } => {
                self.visit_expr(iterable);
                self.visit_expr(body);
            }
            HirExprKind::Assign { target, value, .. }
            | HirExprKind::CompoundAssign { target, value, .. } => {
                self.visit_expr(target);
                self.visit_expr(value);
            }
            HirExprKind::Return(Some(e)) | HirExprKind::Break(Some(e)) => self.visit_expr(e),
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
            HirExprKind::ArrayFill { value, .. } => self.visit_expr(value),
            HirExprKind::Index { object, index } => {
                self.visit_expr(object);
                self.visit_expr(index);
            }
            HirExprKind::Cast { expr: inner, .. } => self.visit_expr(inner),
            HirExprKind::FieldAccess { object, .. } => self.visit_expr(object),
            HirExprKind::Range { start, end, .. } => {
                if let Some(s) = start {
                    self.visit_expr(s);
                }
                if let Some(e) = end {
                    self.visit_expr(e);
                }
            }
            HirExprKind::Interpolation { parts } => {
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
            _ => {}
        }
    }

    fn visit_pattern(&mut self, pattern: &riven_core::hir::nodes::HirPattern) {
        use riven_core::hir::nodes::HirPattern;
        match pattern {
            HirPattern::Literal { expr, .. } => self.visit_expr(expr),
            HirPattern::Tuple { elements, .. }
            | HirPattern::Or {
                patterns: elements, ..
            } => {
                for el in elements {
                    self.visit_pattern(el);
                }
            }
            HirPattern::Enum { fields, .. } => {
                for f in fields {
                    self.visit_pattern(f);
                }
            }
            HirPattern::Struct { fields, .. } => {
                for (_, p) in fields {
                    self.visit_pattern(p);
                }
            }
            _ => {}
        }
    }
}

/// Resolve a method by walking the receiver's `Ty` past wrappers to a
/// class / struct, then scanning the symbol table for a `Method` with
/// matching name whose parent matches the resolved class.
fn lookup_method(symbols: &SymbolTable, receiver_ty: &Ty, method_name: &str) -> Option<DefId> {
    let class_name = peel_to_class_name(receiver_ty)?;
    for def in symbols.iter() {
        if def.name != method_name {
            continue;
        }
        let DefKind::Method { parent, .. } = &def.kind else {
            continue;
        };
        if let Some(parent_def) = symbols.get(*parent) {
            if parent_def.name == class_name {
                return Some(def.id);
            }
        }
    }
    None
}

/// Peel `Ref`, `RefMut`, lifetime variants, raw pointers, alias,
/// newtype to reach the underlying class / struct / enum name.
fn peel_to_class_name(ty: &Ty) -> Option<String> {
    let mut cur = ty;
    loop {
        match cur {
            Ty::Class { name, .. } | Ty::Struct { name, .. } | Ty::Enum { name, .. } => {
                return Some(name.clone());
            }
            Ty::Ref(inner)
            | Ty::RefMut(inner)
            | Ty::RefLifetime(_, inner)
            | Ty::RefMutLifetime(_, inner)
            | Ty::RawPtr(inner)
            | Ty::RawPtrMut(inner)
            | Ty::Newtype { inner, .. }
            | Ty::Alias { target: inner, .. } => cur = inner,
            _ => return None,
        }
    }
}

/// Drop spans seeded by built-ins / bootstrap shims (start == end ==
/// line == 0). The `UseIndex` already filters these at build time but
/// the LSP handler costs nothing to reassert.
fn is_synthetic_span(span: &Span) -> bool {
    span.start == 0 && span.end == 0 && span.line == 0
}
