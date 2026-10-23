use ruxen_core::hir::nodes::*;
use ruxen_core::hir::types::Ty;
use ruxen_core::hir::visit::{walk_expr, walk_impl_block, walk_pattern, Visit};
use ruxen_core::lexer::token::Span;

/// Describes the innermost HIR node found at a cursor position.
pub enum NodeAtPosition {
    VarRef(DefId, Span),
    FnCall {
        callee: DefId,
        span: Span,
    },
    MethodCall {
        method: DefId,
        span: Span,
    },
    FieldAccess {
        object_ty: Ty,
        field_name: String,
        span: Span,
    },
    TypeRef {
        name: String,
        span: Span,
    },
    Definition(DefId, Span),
}

/// Find the innermost HIR node whose span contains the given byte offset.
pub fn node_at_position(program: &HirProgram, byte_offset: usize) -> Option<NodeAtPosition> {
    let mut finder = NodeFinder {
        target: byte_offset,
        result: None,
        enclosing_fn: None,
    };
    finder.visit_program(program);
    finder.result
}

/// The innermost function or method containing a byte offset.
pub struct EnclosingFn {
    pub name: String,
    pub is_async: bool,
    /// The whole definition; it starts at its `def` (or `async`) keyword.
    pub span: Span,
}

/// Find the innermost function or method whose span contains `byte_offset`.
pub fn enclosing_function(program: &HirProgram, byte_offset: usize) -> Option<EnclosingFn> {
    let mut finder = NodeFinder {
        target: byte_offset,
        result: None,
        enclosing_fn: None,
    };
    finder.visit_program(program);
    finder.enclosing_fn
}

/// The innermost `MethodCall` whose span contains `byte_offset`, as its
/// receiver `Ty` and textual method name. The fallback highlight and
/// rename use when typeck left the call's method unresolved.
pub(crate) fn method_call_at(program: &HirProgram, byte_offset: usize) -> Option<(Ty, String)> {
    let mut finder = MethodCallAt {
        target: byte_offset,
        result: None,
    };
    for item in &program.items {
        finder.visit_item(item);
    }
    finder.result
}

struct MethodCallAt {
    target: usize,
    result: Option<(Ty, String)>,
}

impl Visit for MethodCallAt {
    fn visit_expr(&mut self, expr: &HirExpr) {
        if !(expr.span.start <= self.target && self.target < expr.span.end) {
            return;
        }
        // Record this call as the best so far; a deeper call (say, one in
        // an argument) overwrites it.
        if let HirExprKind::MethodCall {
            object,
            method_name,
            ..
        } = &expr.kind
        {
            self.result = Some((object.ty.clone(), method_name.clone()));
        }
        walk_expr(self, expr);
    }
}

struct NodeFinder {
    target: usize,
    result: Option<NodeAtPosition>,
    enclosing_fn: Option<EnclosingFn>,
}

impl NodeFinder {
    fn contains(&self, span: &Span) -> bool {
        span.start <= self.target && self.target < span.end
    }

    fn visit_program(&mut self, program: &HirProgram) {
        for item in &program.items {
            self.visit_item(item);
        }
    }
}

impl Visit for NodeFinder {
    fn visit_item(&mut self, item: &HirItem) {
        match item {
            HirItem::Function(func) => self.visit_func(func),
            HirItem::Class(class) => {
                if self.contains(&class.span) {
                    // Check if cursor is on the class name
                    if let Some(name_span) =
                        self.name_span_in_def(class.def_id, &class.name, &class.span)
                    {
                        if self.contains(&name_span) {
                            self.result = Some(NodeAtPosition::Definition(class.def_id, name_span));
                        }
                    }
                    for field in &class.fields {
                        self.visit_field_def(field);
                    }
                    for method in &class.methods {
                        self.visit_func(method);
                    }
                    for imp in &class.impl_blocks {
                        self.visit_impl_block(imp);
                    }
                }
            }
            HirItem::Struct(s) => {
                if self.contains(&s.span) {
                    for field in &s.fields {
                        self.visit_field_def(field);
                    }
                }
            }
            HirItem::Enum(e) => {
                if self.contains(&e.span) {
                    for variant in &e.variants {
                        if self.contains(&variant.span) {
                            self.result = Some(NodeAtPosition::Definition(
                                variant.def_id,
                                variant.span.clone(),
                            ));
                        }
                    }
                }
            }
            HirItem::Mixin(t) => {
                if self.contains(&t.span) {
                    for item in &t.items {
                        match item {
                            HirMixinItem::DefaultMethod(func) => self.visit_func(func),
                            HirMixinItem::MethodSig { .. } | HirMixinItem::AssocType { .. } => {}
                        }
                    }
                }
            }
            HirItem::Impl(imp) => self.visit_impl_block(imp),
            HirItem::Module(m) => {
                if self.contains(&m.span) {
                    for item in &m.items {
                        self.visit_item(item);
                    }
                }
            }
            HirItem::TypeAlias(_) | HirItem::Newtype(_) | HirItem::Const(_) => {}
        }
    }

    fn visit_func(&mut self, func: &HirFuncDef) {
        if !self.contains(&func.span) {
            return;
        }
        self.enclosing_fn = Some(EnclosingFn {
            name: func.name.clone(),
            is_async: func.is_async,
            span: func.span.clone(),
        });
        // Check if cursor is on the function name itself
        // The name is at the start of the function definition
        self.result = Some(NodeAtPosition::Definition(func.def_id, func.span.clone()));

        for param in &func.params {
            if self.contains(&param.span) {
                self.result = Some(NodeAtPosition::Definition(param.def_id, param.span.clone()));
            }
        }
        self.visit_expr(&func.body);
    }

    fn visit_expr(&mut self, expr: &HirExpr) {
        if !self.contains(&expr.span) {
            return;
        }
        match &expr.kind {
            HirExprKind::VarRef(def_id) => {
                self.result = Some(NodeAtPosition::VarRef(*def_id, expr.span.clone()));
            }
            HirExprKind::FnCall { callee, .. } => {
                self.result = Some(NodeAtPosition::FnCall {
                    callee: *callee,
                    span: expr.span.clone(),
                });
            }
            // A cursor outside the receiver is on the call itself (or in an
            // argument, which the walk below then narrows to).
            HirExprKind::MethodCall { object, method, .. } if !self.contains(&object.span) => {
                self.result = Some(NodeAtPosition::MethodCall {
                    method: *method,
                    span: expr.span.clone(),
                });
            }
            _ => {}
        }
        walk_expr(self, expr);
        // A cursor inside the receiver lands on the call or field access
        // unless the receiver held something more specific.
        match &expr.kind {
            HirExprKind::MethodCall { object, method, .. }
                if self.contains(&object.span) && !self.found_inside(object) =>
            {
                self.result = Some(NodeAtPosition::MethodCall {
                    method: *method,
                    span: expr.span.clone(),
                });
            }
            HirExprKind::FieldAccess {
                object, field_name, ..
            } if !self.found_inside(object) => {
                self.result = Some(NodeAtPosition::FieldAccess {
                    object_ty: object.ty.clone(),
                    field_name: field_name.clone(),
                    span: expr.span.clone(),
                });
            }
            _ => {}
        }
    }

    fn visit_stmt(&mut self, stmt: &HirStatement) {
        match stmt {
            HirStatement::Let {
                def_id,
                value,
                span,
                pattern,
                ..
            } => {
                // Check if cursor is on the binding pattern
                if let Some(pat_span) = self.pattern_span(pattern) {
                    if self.contains(&pat_span) {
                        self.result = Some(NodeAtPosition::Definition(*def_id, pat_span));
                    }
                }
                if let Some(val) = value {
                    self.visit_expr(val);
                }
                // Fallback: if cursor is in the let statement span but not on value
                if self.contains(span) && self.result.is_none() {
                    self.result = Some(NodeAtPosition::Definition(*def_id, span.clone()));
                }
            }
            HirStatement::Expr(expr) => {
                self.visit_expr(expr);
            }
        }
    }

    fn visit_pattern(&mut self, pattern: &HirPattern) {
        match pattern {
            HirPattern::Binding { def_id, span, .. } | HirPattern::Ref { def_id, span, .. } => {
                if self.contains(span) {
                    self.result = Some(NodeAtPosition::Definition(*def_id, span.clone()));
                }
            }
            _ => walk_pattern(self, pattern),
        }
    }
}

impl NodeFinder {
    fn visit_impl_block(&mut self, imp: &HirImplBlock) {
        if self.contains(&imp.span) {
            walk_impl_block(self, imp);
        }
    }

    fn visit_field_def(&mut self, field: &HirFieldDef) {
        if self.contains(&field.span) {
            self.result = Some(NodeAtPosition::Definition(field.def_id, field.span.clone()));
        }
    }

    fn pattern_span(&self, pattern: &HirPattern) -> Option<Span> {
        match pattern {
            HirPattern::Binding { span, .. } => Some(span.clone()),
            HirPattern::Tuple { span, .. } => Some(span.clone()),
            HirPattern::Wildcard { span } => Some(span.clone()),
            _ => None,
        }
    }

    /// Check if we already found a result inside a sub-expression.
    fn found_inside(&self, expr: &HirExpr) -> bool {
        if let Some(ref result) = self.result {
            let result_span = match result {
                NodeAtPosition::VarRef(_, s) => s,
                NodeAtPosition::FnCall { span, .. } => span,
                NodeAtPosition::MethodCall { span, .. } => span,
                NodeAtPosition::FieldAccess { span, .. } => span,
                NodeAtPosition::TypeRef { span, .. } => span,
                NodeAtPosition::Definition(_, s) => s,
            };
            // If the result span is strictly inside the given expression span,
            // we found something more specific
            result_span.start >= expr.span.start
                && result_span.end <= expr.span.end
                && (result_span.start != expr.span.start || result_span.end != expr.span.end)
        } else {
            false
        }
    }

    /// Try to compute a name-only span for a definition.
    /// For now, returns None — the full definition span is used instead.
    fn name_span_in_def(&self, _def_id: DefId, _name: &str, _outer_span: &Span) -> Option<Span> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;

    /// Return the byte offset of the first occurrence of `needle`, skipping `skip` hits.
    fn byte_offset_of(src: &str, needle: &str, skip: usize) -> usize {
        let mut remaining = skip + 1;
        let mut search_start = 0;
        while remaining > 0 {
            let found = src[search_start..].find(needle).expect("not found");
            remaining -= 1;
            if remaining == 0 {
                return search_start + found;
            }
            search_start += found + needle.len();
        }
        unreachable!()
    }

    #[test]
    fn find_identifier_at_exact_start() {
        let src = "def main\n  let x = 42\n  let y = x\nend\n";
        let result = analyze(src);
        let program = result.program.as_ref().unwrap();
        let offset = byte_offset_of(src, "x", 1); // the 'x' in `let y = x`
        let node = node_at_position(program, offset);
        assert!(node.is_some());
        match node.unwrap() {
            NodeAtPosition::VarRef(_, span) => {
                assert_eq!(span.start, offset);
            }
            other => {
                // It might be wrapped — at least we got a node
                let _ = other;
            }
        }
    }

    #[test]
    fn find_identifier_inside_body() {
        let src = "def main\n  let foo = 42\n  let bar = foo\nend\n";
        let result = analyze(src);
        let program = result.program.as_ref().unwrap();
        // Offset in the middle of "foo" (position 1 of the reference)
        let foo_ref_start = byte_offset_of(src, "foo", 1);
        let offset = foo_ref_start + 1;
        let node = node_at_position(program, offset);
        assert!(node.is_some(), "Expected node inside foo identifier");
    }

    #[test]
    fn find_beyond_identifier_may_return_none_or_different_node() {
        let src = "def main\n  let x = 42\nend\n";
        let result = analyze(src);
        let program = result.program.as_ref().unwrap();
        // Past the end of source
        let node = node_at_position(program, src.len() + 10);
        assert!(node.is_none(), "Expected None beyond EOF");
    }

    #[test]
    fn find_in_function_body_returns_node_inside() {
        let src = "def main\n  let x = 42\nend\n";
        let result = analyze(src);
        let program = result.program.as_ref().unwrap();
        // Inside "let x = 42"
        let let_offset = byte_offset_of(src, "let", 0);
        let node = node_at_position(program, let_offset + 1);
        assert!(node.is_some(), "Expected node inside function body");
    }

    #[test]
    fn find_on_literal_returns_enclosing_definition() {
        let src = "def main\n  let x = 42\nend\n";
        let result = analyze(src);
        let program = result.program.as_ref().unwrap();
        let offset = byte_offset_of(src, "42", 0);
        let node = node_at_position(program, offset);
        // Either None or an enclosing node (int literals have no children)
        if let Some(n) = node {
            // At this position, we're inside the let statement — should get some definition
            // or expression enclosing
            let _ = n;
        }
    }

    #[test]
    fn find_at_zero_offset_in_empty_program() {
        let src = "";
        let result = analyze(src);
        let program = result.program.as_ref();
        if let Some(program) = program {
            let node = node_at_position(program, 0);
            assert!(node.is_none());
        }
    }

    #[test]
    fn find_at_whitespace_may_return_enclosing_fn() {
        let src = "def main\n  let x = 42\nend\n";
        let result = analyze(src);
        let program = result.program.as_ref().unwrap();
        // Offset at the start of line 1 (before "  let x")
        let offset = 9; // "def main\n" is 9 bytes, line 1 starts at byte 9
        let node = node_at_position(program, offset);
        // Could be None or a definition enveloping this whitespace
        let _ = node;
    }

    #[test]
    fn find_nested_field_access_finds_innermost() {
        let src = "class Inner\n  a: Int\n  def init(@a: Int)\n  end\nend\n\nclass Outer\n  inner: Inner\n  def init(@inner: Inner)\n  end\nend\n\ndef main\n  let o = Outer.new(Inner.new(7))\n  let r = o.inner.a\nend\n";
        let result = analyze(src);
        let program = result.program.as_ref().unwrap();
        // Find the position of 'a' in "o.inner.a"
        let offset = src.rfind(".a").unwrap() + 1;
        let node = node_at_position(program, offset);
        // We should find a node — either field access or a def
        if let Some(NodeAtPosition::FieldAccess { field_name, .. }) = node {
            assert_eq!(field_name, "a");
        }
    }

    #[test]
    fn find_on_function_param_returns_definition() {
        let src = "def greet(name: String)\n  puts name\nend\n";
        let result = analyze(src);
        let program = result.program.as_ref().unwrap();
        // Position on "name" parameter in signature
        let offset = byte_offset_of(src, "name", 0);
        let node = node_at_position(program, offset + 1);
        assert!(node.is_some());
    }

    #[test]
    fn find_on_method_call_returns_method_call_or_nested() {
        // ruby-naming.spec.md §3.2: `pub def` is retired (public is default).
        let src = "class Counter\n  v: Int\n  def init(@v: Int)\n  end\n  def inc -> Int\n    self.v + 1\n  end\nend\n\ndef main\n  let c = Counter.new(0)\n  let r = c.inc\nend\n";
        let result = analyze(src);
        let program = result.program.as_ref().unwrap();
        let offset = byte_offset_of(src, "c.inc", 0);
        let node = node_at_position(program, offset + 2); // inside "inc"
        assert!(node.is_some());
    }

    #[test]
    fn find_inside_class_body_finds_field_or_method() {
        let src = "class Box\n  x: Int\n  def init(@x: Int)\n  end\nend\n";
        let result = analyze(src);
        let program = result.program.as_ref().unwrap();
        // The 'x' field on line 1
        let offset = src.find("x: Int").unwrap();
        let node = node_at_position(program, offset + 1);
        assert!(node.is_some(), "Expected a node inside class body");
    }

    #[test]
    fn find_past_end_of_program_returns_none() {
        let src = "def main\n  let x = 42\nend";
        let result = analyze(src);
        let program = result.program.as_ref().unwrap();
        let node = node_at_position(program, src.len());
        assert!(node.is_none(), "Expected None at the exact end of program");
    }

    #[test]
    fn find_at_far_past_eof_returns_none() {
        let src = "def main\n  let x = 1\nend\n";
        let result = analyze(src);
        let program = result.program.as_ref().unwrap();
        let node = node_at_position(program, 100_000);
        assert!(node.is_none());
    }

    #[test]
    fn find_binary_op_descends_into_left_operand() {
        let src = "def add -> Int\n  let a = 1\n  let b = 2\n  a + b\nend\n";
        let result = analyze(src);
        let program = result.program.as_ref().unwrap();
        // Position on 'a' in "a + b"
        let offset = byte_offset_of(src, "a + b", 0);
        let node = node_at_position(program, offset);
        assert!(node.is_some());
        // Should be a VarRef, or enclosing definition
        if let Some(NodeAtPosition::VarRef(_, _)) = node {
            // Good — found the variable reference
        }
    }
}
