//! Implicit block parameters — Ruby's numbered params (`_1`..`_9`) and the
//! single implicit `it`. Block Tier-2 (block ADR, staged).
//!
//! A closure written with NO explicit `|params|` may still reference the
//! implicit parameters `_1`..`_9` or the bare `it`. When it does, `resolve`
//! synthesizes the matching closure parameters so those references bind (see
//! `control_flow.rs::resolve_closure`). Doing this at RESOLVE time — not in the
//! parser — keeps the authored surface (`{ _1 * 2 }` / `{ it * 2 }`) intact for
//! `ruxen fmt`; the formatter never sees a rewritten `{ |_1| _1 * 2 }`.
//!
//! Rules (matching Ruby):
//!   - `_1`..`_9`: the highest index used sets the arity; params `_1`..`_max`
//!     are synthesized. `_2` alone therefore yields two params (`_1`, `_2`).
//!   - `it`: a lone implicit parameter, synthesized only when NO numbered
//!     parameter is present (numbered wins if both appear).
//!   - The scan stops at a nested closure boundary: `_1` / `it` inside a nested
//!     `do…end` / `{ }` belong to THAT closure, not the enclosing one (Ruby's
//!     numbered params do not cross block boundaries).
//!   - `it` is NOT counted when it is the direct callee of a `Call`
//!     (`it("desc") do … end` is a method call — e.g. the tester DSL — not the
//!     implicit parameter). `it` as a value (`it`, `it.foo`, `f(it)`) counts.

use crate::parser::ast;

/// Split a call's argument list into (positional args, trailing block-pass).
/// A `&`-form block argument (`&:sym` / `&block` / `&`) is only meaningful as
/// the LAST argument; when the final argument is an `ExprKind::BlockPass`, it is
/// returned separately so `resolve` can hoist it into the call's block slot. A
/// block-pass anywhere else is left in `args` and diagnosed (E1120) by the
/// generic `resolve_expr` arm.
pub(super) fn split_trailing_block_pass(
    args: &[ast::Expr],
) -> (&[ast::Expr], Option<&ast::BlockPassKind>) {
    if let Some(last) = args.last() {
        if let ast::ExprKind::BlockPass(kind) = &last.kind {
            return (&args[..args.len() - 1], Some(kind));
        }
    }
    (args, None)
}

/// Return the implicit parameter names a paramless closure body requires, in
/// order. Empty when the body references neither `_1`..`_9` nor `it`.
pub(super) fn implicit_param_names(body: &ast::ClosureBody) -> Vec<String> {
    let mut scan = Scan {
        max_numbered: 0,
        uses_it: false,
    };
    match body {
        ast::ClosureBody::Expr(e) => scan.expr(e),
        ast::ClosureBody::Block(b) => scan.block(b),
    }
    if scan.max_numbered > 0 {
        (1..=scan.max_numbered).map(|i| format!("_{i}")).collect()
    } else if scan.uses_it {
        vec!["it".to_string()]
    } else {
        Vec::new()
    }
}

struct Scan {
    max_numbered: u8,
    uses_it: bool,
}

impl Scan {
    fn ident(&mut self, name: &str) {
        if name == "it" {
            self.uses_it = true;
            return;
        }
        // `_1`..`_9` — a single underscore followed by one digit 1-9.
        let bytes = name.as_bytes();
        if bytes.len() == 2 && bytes[0] == b'_' && (b'1'..=b'9').contains(&bytes[1]) {
            let n = bytes[1] - b'0';
            if n > self.max_numbered {
                self.max_numbered = n;
            }
        }
    }

    fn block(&mut self, b: &ast::Block) {
        for stmt in &b.statements {
            match stmt {
                ast::Statement::Let(binding) => {
                    if let Some(v) = &binding.value {
                        self.expr(v);
                    }
                }
                ast::Statement::Expression(e) => self.expr(e),
            }
        }
    }

    fn expr(&mut self, e: &ast::Expr) {
        use ast::ExprKind::*;
        match &e.kind {
            Identifier(name) => self.ident(name),
            BinaryOp { left, right, .. } => {
                self.expr(left);
                self.expr(right);
            }
            UnaryOp { operand, .. } => self.expr(operand),
            Borrow(x) | BorrowMut(x) => self.expr(x),
            FieldAccess { object, .. } | SafeNav { object, .. } => self.expr(object),
            MethodCall {
                object,
                args,
                block,
                ..
            } => {
                self.expr(object);
                for a in args {
                    self.expr(a);
                }
                if let Some(b) = block {
                    self.expr(b);
                }
            }
            SafeNavCall { object, args, .. } => {
                self.expr(object);
                for a in args {
                    self.expr(a);
                }
            }
            Call {
                parens: _,
                callee,
                args,
                block,
            } => {
                // Skip a bare `it` callee: `it(…)` is a method call, not the
                // implicit parameter. Any other callee is descended normally.
                if !matches!(&callee.kind, Identifier(n) if n == "it") {
                    self.expr(callee);
                }
                for a in args {
                    self.expr(a);
                }
                if let Some(b) = block {
                    self.expr(b);
                }
            }
            Index { object, index } => {
                self.expr(object);
                self.expr(index);
            }
            ClosureCall { callee, args } => {
                self.expr(callee);
                for a in args {
                    self.expr(a);
                }
            }
            Try(x) | Await(x) | Cast { expr: x, .. } => self.expr(x),
            Assign { target, value } => {
                self.expr(target);
                self.expr(value);
            }
            CompoundAssign { target, value, .. } => {
                self.expr(target);
                self.expr(value);
            }
            If(ife) => {
                self.expr(&ife.condition);
                self.block(&ife.then_body);
                for c in &ife.elsif_clauses {
                    self.expr(&c.condition);
                    self.block(&c.body);
                }
                if let Some(eb) = &ife.else_body {
                    self.block(eb);
                }
            }
            IfLet(ile) => {
                self.expr(&ile.value);
                self.block(&ile.then_body);
                if let Some(eb) = &ile.else_body {
                    self.block(eb);
                }
            }
            Match(me) => {
                self.expr(&me.subject);
                for arm in &me.arms {
                    match &arm.body {
                        ast::MatchArmBody::Expr(x) => self.expr(x),
                        ast::MatchArmBody::Block(b) => self.block(b),
                    }
                }
            }
            While(we) => {
                self.expr(&we.condition);
                self.block(&we.body);
            }
            WhileLet(wle) => {
                self.expr(&wle.value);
                self.block(&wle.body);
            }
            For(fe) => {
                self.expr(&fe.iterable);
                self.block(&fe.body);
            }
            Loop(le) => self.block(&le.body),
            Block(b) | UnsafeBlock(b) => self.block(b),
            Range { start, end, .. } => {
                if let Some(s) = start {
                    self.expr(s);
                }
                if let Some(en) = end {
                    self.expr(en);
                }
            }
            ArrayLiteral(elems) | TupleLiteral(elems) => {
                for x in elems {
                    self.expr(x);
                }
            }
            ArrayFill { value, count } => {
                self.expr(value);
                self.expr(count);
            }
            MapLiteral(entries) => {
                for (k, v) in entries {
                    self.expr(k);
                    self.expr(v);
                }
            }
            // `Next` carries the same `Option<Box<Expr>>` payload as `Return` /
            // `Break` and must be scanned identically. Omitting it meant the two
            // headline Block Tier-2 features did not compose: `xs.map { next _1 * 100 }`
            // failed with `undefined variable '_1'`, because the numbered-param
            // scan never descended into the `next` operand. The existing test
            // used `next` and `_1` in two SEPARATE blocks, so it did not catch this.
            Return(x) | Break(x) | Next(x) => {
                if let Some(v) = x {
                    self.expr(v);
                }
            }
            Yield(args) => {
                for a in args {
                    self.expr(a);
                }
            }
            MacroCall { args, .. } => {
                for a in args {
                    self.expr(a);
                }
            }
            EnumVariant { args, .. } => {
                for fa in args {
                    self.expr(&fa.value);
                }
            }
            // A nested closure has its OWN implicit-parameter scope; do not
            // descend (`Closure(_) => {}`), matching the yield-scan boundary.
            Closure(_) => {}
            // Leaf / no-sub-expression forms (literals, SelfRef, etc.).
            _ => {}
        }
    }
}
