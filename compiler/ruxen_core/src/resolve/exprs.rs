use crate::diagnostics::Diagnostic;
use crate::hir::nodes::*;
use crate::hir::types::{MoveSemantics, Ty};
use crate::parser::ast::{self, Visibility};

use super::implicit_block_params::split_trailing_block_pass;
use super::scope::ScopeKind;
use super::symbols::*;
use super::{PatternSpan, Resolver};

impl Resolver {
    /// The type of a type name used as a value (the receiver of
    /// `Point.new(...)`, `Color.Red`, `Outer.Inner.method(...)`): the
    /// class, struct or enum itself, else a fresh inference variable.
    fn type_name_value_ty(&mut self, def_id: DefId, name: &str) -> Ty {
        let name = name.to_string();
        match self.symbols.get(def_id).map(|d| &d.kind) {
            Some(DefKind::Class { .. }) => Ty::Class {
                name,
                generic_args: vec![],
            },
            Some(DefKind::Struct { .. }) => Ty::Struct {
                name,
                generic_args: vec![],
            },
            Some(DefKind::Enum { .. }) => Ty::Enum {
                name,
                generic_args: vec![],
            },
            _ => self.type_context.fresh_type_var(),
        }
    }

    pub(super) fn resolve_expr(&mut self, expr: &ast::Expr) -> HirExpr {
        let span = expr.span.clone();
        match &expr.kind {
            ast::ExprKind::IntLiteral(val, suffix) => {
                let ty = self.int_literal_type(*suffix);
                HirExpr {
                    kind: HirExprKind::IntLiteral(*val),
                    ty,
                    span,
                }
            }
            ast::ExprKind::FloatLiteral(val, suffix) => {
                let ty = self.float_literal_type(*suffix);
                HirExpr {
                    kind: HirExprKind::FloatLiteral(*val),
                    ty,
                    span,
                }
            }
            ast::ExprKind::StringLiteral(s) => HirExpr {
                // One-string-type ADR (docs/decisions/one-string-type.md): a
                // string literal is born `Ty::String` (owned). MIR's
                // `emit_owned_string_literal` heap-copies the `.rodata` pointer
                // through `ruxen_string_from`, so scope-exit drop is safe. The
                // old `Ty::Str` (`&str`) birth + the Q38/Q39 owned-position
                // promotion patches are gone — there is no `&str` to promote.
                kind: HirExprKind::StringLiteral(s.clone()),
                ty: Ty::String,
                span,
            },
            ast::ExprKind::InterpolatedString(parts) => {
                let hir_parts: Vec<HirInterpolationPart> = parts
                    .iter()
                    .map(|p| {
                        match p {
                            ast::StringPart::Literal(s) => HirInterpolationPart::Literal(s.clone()),
                            // The format spec (Phase 2 #06.B) is threaded through
                            // unchanged to MIR — Phase C/D consume it.
                            ast::StringPart::Expr { expr, spec } => HirInterpolationPart::Expr {
                                expr: self.resolve_expr(expr),
                                spec: spec.clone(),
                            },
                        }
                    })
                    .collect();
                HirExpr {
                    kind: HirExprKind::Interpolation { parts: hir_parts },
                    ty: Ty::String, // interpolated strings produce owned Strings
                    span,
                }
            }
            ast::ExprKind::CharLiteral(c) => HirExpr {
                kind: HirExprKind::CharLiteral(*c),
                ty: Ty::Char,
                span,
            },
            ast::ExprKind::RegexLiteral { pattern, flags } => HirExpr {
                kind: HirExprKind::RegexLiteral {
                    pattern: pattern.clone(),
                    flags: flags.clone(),
                },
                // Typed as `Regex` class — the std.regex bootstrap
                // declares `class Regex` so downstream method
                // resolution finds `.is_match`, `.find`, `.scan`,
                // `.replace`, `.replace_all`, `.split`, etc.
                ty: Ty::Class {
                    name: "Regex".to_string(),
                    generic_args: vec![],
                },
                span,
            },
            ast::ExprKind::BoolLiteral(b) => HirExpr {
                kind: HirExprKind::BoolLiteral(*b),
                ty: Ty::Bool,
                span,
            },
            ast::ExprKind::UnitLiteral => HirExpr {
                kind: HirExprKind::UnitLiteral,
                ty: Ty::Unit,
                span,
            },
            ast::ExprKind::Identifier(name) => {
                if name == "super" {
                    self.check_super_context(&span);
                }
                // `block_defined?` / `block_given?` (alias) — Ruby-block-semantics
                // ADR D6. A no-receiver builtin predicate that is `true` iff the
                // caller passed a block. Desugars to `__block != nil` (the null
                // closure-pair-pointer sentinel, ADR D1) using existing nodes —
                // no new HIR variant. Only meaningful inside a function that
                // declares a block slot; outside one it falls through to the
                // ordinary undefined-variable path.
                if let Some(block_def) = (name == "block_defined?" || name == "block_given?")
                    .then(|| self.scopes.lookup("__block"))
                    .flatten()
                {
                    let block_ty = self.symbols.def_ty(block_def).unwrap_or(Ty::Error);
                    let lhs = HirExpr {
                        kind: HirExprKind::VarRef(block_def),
                        ty: block_ty,
                        span: span.clone(),
                    };
                    let rhs = HirExpr {
                        kind: HirExprKind::NullLiteral,
                        ty: Ty::Int,
                        span: span.clone(),
                    };
                    return HirExpr {
                        kind: HirExprKind::BinaryOp {
                            op: ast::BinOp::NotEq,
                            left: Box::new(lhs),
                            right: Box::new(rhs),
                        },
                        ty: Ty::Bool,
                        span,
                    };
                }
                if let Some((def_id, def_scope_id)) = self.scopes.lookup_with_scope(name) {
                    // If the identifier resolves to an enum variant (e.g.
                    // bare `None`, `Color.Red`), lower it as an
                    // EnumVariant construction rather than a VarRef so
                    // codegen allocates and tags it correctly.
                    if let Some(def) = self.symbols.get(def_id) {
                        if let DefKind::EnumVariant {
                            parent,
                            variant_idx,
                            ..
                        } = def.kind
                        {
                            let parent_name = self
                                .symbols
                                .get(parent)
                                .map(|p| p.name.clone())
                                .unwrap_or_default();
                            return HirExpr {
                                kind: HirExprKind::EnumVariant {
                                    type_def: parent,
                                    type_name: parent_name.clone(),
                                    variant_name: name.clone(),
                                    variant_idx,
                                    fields: vec![],
                                },
                                ty: Ty::Enum {
                                    name: parent_name,
                                    generic_args: vec![],
                                },
                                span,
                            };
                        }
                    }
                    self.record_capture_if_needed(def_id, def_scope_id);
                    // Phase 2 stdlib (#06): Class/Struct identifiers
                    // imported via `use std.process.Command` (or
                    // otherwise reached through the value-scope
                    // `lookup` rather than `lookup_type`) must surface
                    // as their own `Ty::Class { name }` /
                    // `Ty::Struct { name }` so a subsequent
                    // `.new(...)` MethodCall sees a concrete receiver
                    // and dispatches through the collection-ctor fast
                    // path in mir/lower.rs. Without this branch,
                    // `def_ty` returns None for Class kinds
                    // (intentionally — see
                    // `def_ty_returns_none_for_class` in symbols.rs)
                    // and the receiver collapses to a fresh inference
                    // variable, which means `.arg/.status/etc.` would
                    // never resolve to the right `builtin_method_type`
                    // arm.
                    //
                    // Enum is intentionally NOT promoted here: enum
                    // identifiers reach this path only as the receiver
                    // for `EnumName.Variant(...)` constructor calls,
                    // which are parsed as their own AST shape
                    // (`ExprKind::EnumVariant`) and reach
                    // `resolve_expr` through a different arm. Limiting
                    // the promotion to Class/Struct also keeps the
                    // 226-fixture e2e baseline stable (only the pre-
                    // existing `95_error_into_conversion` typecheck
                    // failure remains; everything else passes).
                    let ty = match self.symbols.get(def_id).map(|d| &d.kind) {
                        Some(DefKind::Class { .. }) => Ty::Class {
                            name: name.clone(),
                            generic_args: vec![],
                        },
                        Some(DefKind::Struct { .. }) => Ty::Struct {
                            name: name.clone(),
                            generic_args: vec![],
                        },
                        _ => self
                            .symbols
                            .def_ty(def_id)
                            .unwrap_or_else(|| self.type_context.fresh_type_var()),
                    };
                    HirExpr {
                        kind: HirExprKind::VarRef(def_id),
                        ty,
                        span,
                    }
                } else if let Some(def_id) = self.scopes.lookup_type(name) {
                    // Type name used as a value — needed for constructor calls
                    // like Point.new(...), Color.Red, etc.
                    let ty = self.type_name_value_ty(def_id, name);
                    HirExpr {
                        kind: HirExprKind::VarRef(def_id),
                        ty,
                        span,
                    }
                } else if name.contains('.') {
                    // #06.93 Phase 3: dotted identifier produced by the
                    // parser for `Outer.Inner.method(args)` shape — the
                    // parser emits `Identifier("Outer.Inner")` and
                    // postfix builds a MethodCall on top. Look up the
                    // qualified name in `type_registry` (populated by
                    // Phase 1's nested-module registration). If found
                    // as a Class/Struct/Enum, emit a Class-typed
                    // expression so the trailing `.method(args)`
                    // dispatches as a regular class-method call.
                    if let Some(&def_id) = self.type_registry.get(name) {
                        let ty = self.type_name_value_ty(def_id, name);
                        HirExpr {
                            kind: HirExprKind::VarRef(def_id),
                            ty,
                            span,
                        }
                    } else {
                        self.error(format!("undefined qualified type `{}`", name), &span);
                        self.unresolved_names.push(name.to_string());
                        HirExpr {
                            kind: HirExprKind::Error,
                            ty: Ty::Error,
                            span,
                        }
                    }
                } else {
                    self.error_with_code(format!("undefined variable `{}`", name), &span, "E0734");
                    self.unresolved_names.push(name.to_string());
                    HirExpr {
                        kind: HirExprKind::Error,
                        ty: Ty::Error,
                        span,
                    }
                }
            }
            ast::ExprKind::GenericTypeRef { name, ty } => {
                // `Hash[String, Int].new` — a type in value position with the
                // generic arguments the author wrote. Resolve the written
                // path through ordinary type resolution (which already knows
                // `Hash[K, V]` is `Ty::Map`, `Set[T]` is `Ty::Set`, and how to
                // handle a user generic), and give the receiver THAT type
                // instead of the constructor entry's declared one.
                //
                // This is the whole fix: typeck's `substitute_generics_in_return`
                // binds the class's declared params against the receiver's
                // generic args, so a receiver of `Map(String, Int)` turns the
                // declared `-> Hash[K, V]` into `Hash[String, Int]`. With the
                // old `Identifier` receiver it bound `{K → K}` and the call's
                // type stayed abstract forever.
                // Only the BUILTIN type constructors need refining here.
                // `register_type_constructors` registers `Array`/`Hash`/`Set`/
                // … in VALUE scope as a `DefKind::Variable` holding the
                // declared generic type, and that entry is the thing whose
                // rigid parameters leak. A user generic class is a
                // `DefKind::Class` reached through type scope, its `.new` goes
                // down the constructor path, and `Pair[Int, String].new(42,
                // "hi")` already infers its parameters from the constructor
                // arguments. Refining those here would swap a `DefKind::Class`
                // receiver for a synthetic `DefKind::Variable` and break
                // constructor resolution, so they keep the path they have.
                let is_builtin_ctor = self
                    .scopes
                    .lookup(name)
                    .and_then(|id| self.symbols.get(id).map(|d| &d.kind))
                    .is_some_and(|k| matches!(k, DefKind::Variable { .. }));
                if !is_builtin_ctor {
                    // Resolve exactly as the bare type name would, so every
                    // non-builtin shape behaves precisely as before this node
                    // existed — including the diagnostics for an undefined one.
                    let bare = ast::Expr {
                        kind: ast::ExprKind::Identifier(name.clone()),
                        span: span.clone(),
                    };
                    return self.resolve_expr(&bare);
                }
                let resolved_ty = self.resolve_type_expr(ty);
                // Setting `expr.ty` here would NOT survive: typeck's `VarRef`
                // arm re-derives an expression's type from its `DefId`
                // (`typeck/infer/expr.rs:125-130`) and would put the
                // uninstantiated `Hash[K, V]` straight back. So register the
                // INSTANTIATED constructor as its own def, which is exactly
                // the shape `resolve/stdlib/type_constructors.rs` uses for the
                // uninstantiated one. The type then survives by construction
                // rather than by every later stage agreeing not to clobber it.
                //
                // Method lookup is unaffected: static-method resolution and
                // MIR's callee mangling both key off the receiver's TYPE
                // (`method_home_key(Ty::Map) == "Hash"`), never off this
                // `DefId`. The def is deliberately NOT inserted into any
                // scope — it is not name-addressable, it exists only to carry
                // this occurrence's type. That means one extra symbol-table
                // entry per written `Type[Args].method` expression, which is
                // the same order as the temporaries every expression already
                // allocates.
                let def_id = self.symbols.define(
                    name.clone(),
                    DefKind::Variable {
                        mutable: false,
                        ty: resolved_ty.clone(),
                    },
                    Visibility::Public,
                    span.clone(),
                );
                HirExpr {
                    kind: HirExprKind::VarRef(def_id),
                    ty: resolved_ty,
                    span,
                }
            }
            ast::ExprKind::SelfRef => {
                if let Some(def_id) = self.scopes.lookup("self") {
                    let ty = self.current_self_ty.clone().unwrap_or(Ty::Error);
                    HirExpr {
                        kind: HirExprKind::VarRef(def_id),
                        ty,
                        span,
                    }
                } else {
                    self.error("`self` used outside of method context".to_string(), &span);
                    HirExpr {
                        kind: HirExprKind::Error,
                        ty: Ty::Error,
                        span,
                    }
                }
            }
            ast::ExprKind::SelfType => {
                if let Some(ref ty) = self.current_self_ty {
                    let def_id = self.scopes.lookup_type("Self").unwrap_or(UNRESOLVED_DEF);
                    HirExpr {
                        kind: HirExprKind::VarRef(def_id),
                        ty: ty.clone(),
                        span,
                    }
                } else {
                    self.error("`Self` used outside of type context".to_string(), &span);
                    HirExpr {
                        kind: HirExprKind::Error,
                        ty: Ty::Error,
                        span,
                    }
                }
            }
            ast::ExprKind::BinaryOp { left, op, right } => {
                let left_hir = self.resolve_expr(left);
                let right_hir = self.resolve_expr(right);
                let result_ty = self.type_context.fresh_type_var();
                HirExpr {
                    kind: HirExprKind::BinaryOp {
                        op: *op,
                        left: Box::new(left_hir),
                        right: Box::new(right_hir),
                    },
                    ty: result_ty,
                    span,
                }
            }
            ast::ExprKind::UnaryOp { op, operand } => {
                let operand_hir = self.resolve_expr(operand);
                let result_ty = self.type_context.fresh_type_var();
                HirExpr {
                    kind: HirExprKind::UnaryOp {
                        op: *op,
                        operand: Box::new(operand_hir),
                    },
                    ty: result_ty,
                    span,
                }
            }
            ast::ExprKind::Borrow(inner) => {
                let inner_hir = self.resolve_expr(inner);
                let ty = Ty::Ref(Box::new(inner_hir.ty.clone()));
                HirExpr {
                    kind: HirExprKind::Borrow {
                        mutable: false,
                        expr: Box::new(inner_hir),
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::BorrowMut(inner) => {
                let inner_hir = self.resolve_expr(inner);
                let ty = Ty::RefMut(Box::new(inner_hir.ty.clone()));
                HirExpr {
                    kind: HirExprKind::Borrow {
                        mutable: true,
                        expr: Box::new(inner_hir),
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::FieldAccess { object, field, .. } => {
                let obj_hir = self.resolve_expr(object);
                let ty = self.type_context.fresh_type_var();
                HirExpr {
                    kind: HirExprKind::FieldAccess {
                        object: Box::new(obj_hir),
                        field_name: field.clone(),
                        field_idx: 0, // resolved during type checking
                        safe_nav: false,
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::MethodCall {
                parens,
                object,
                method,
                generic_args,
                args,
                block,
            } => {
                let obj_hir = self.resolve_expr(object);
                // Block Tier-2: a trailing `&:sym` / `&block` / `&` argument is
                // hoisted out of the positional args into the block slot.
                let (pos_args, trailing_bp) = split_trailing_block_pass(args);
                let mut args_hir: Vec<HirExpr> =
                    pos_args.iter().map(|a| self.resolve_expr(a)).collect();
                let block_hir = if let Some(bp) = trailing_bp {
                    Some(Box::new(self.resolve_block_pass(bp, &span)))
                } else {
                    block.as_ref().map(|b| Box::new(self.resolve_expr(b)))
                };
                if block_hir.is_none() {
                    if let HirExprKind::VarRef(module_id) = &obj_hir.kind {
                        if let Some(module_def) = self.symbols.get(*module_id) {
                            let module_name = module_def.name.clone();
                            if let DefKind::Module { items } = &module_def.kind {
                                let items = items.clone();
                                let candidates: Vec<DefId> = items
                                    .iter()
                                    .copied()
                                    .filter(|item_id| {
                                        self.symbols
                                            .get(*item_id)
                                            .map(|def| {
                                                def.name == *method
                                                    || def.name.starts_with(&format!(
                                                        "{}__overload",
                                                        method
                                                    ))
                                            })
                                            .unwrap_or(false)
                                    })
                                    .collect();
                                if let Some(def_id) =
                                    self.select_overload_candidate_by_args(&candidates, &args_hir)
                                {
                                    self.append_default_args(def_id, &mut args_hir);
                                    let child_name = self
                                        .symbols
                                        .get(def_id)
                                        .map(|d| d.name.clone())
                                        .unwrap_or_else(|| method.clone());
                                    let callee_name =
                                        format!("{}_{}", module_name.replace('.', "_"), child_name);
                                    return HirExpr {
                                        kind: HirExprKind::FnCall {
                                            callee: def_id,
                                            callee_name,
                                            args: args_hir,
                                        },
                                        ty: self.type_context.fresh_type_var(),
                                        span,
                                    };
                                }
                            }
                        }
                    }
                }
                let generic_args_hir = generic_args
                    .iter()
                    .map(|a| self.resolve_type_expr(a))
                    .collect();
                let ty = self.type_context.fresh_type_var();
                HirExpr {
                    kind: HirExprKind::MethodCall {
                        object: Box::new(obj_hir),
                        method: UNRESOLVED_DEF, // resolved during type checking
                        resolved: UNRESOLVED_DEF,
                        method_name: method.clone(),
                        generic_args: generic_args_hir,
                        args: args_hir,
                        block: block_hir,
                        safe_nav: false,
                        parens: *parens,
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::Call {
                parens,
                callee,
                args,
                block,
            } => {
                // Block Tier-2: hoist a trailing `&:sym` / `&block` / `&`.
                let (pos_args, trailing_bp) = split_trailing_block_pass(args);
                let mut args_hir: Vec<HirExpr> =
                    pos_args.iter().map(|a| self.resolve_expr(a)).collect();
                let mut block_hir = if let Some(bp) = trailing_bp {
                    Some(Box::new(self.resolve_block_pass(bp, &span)))
                } else {
                    block.as_ref().map(|b| Box::new(self.resolve_expr(b)))
                };

                // E1112 (docs/specs/stdlib/executor.spec.md B6) is
                // detected in `async_lowering::collect_block_on_in_async_diagnostics`,
                // which runs BEFORE the async-fn rewrite while the
                // original `block_on(...)` call is still attached to
                // its async parent. By the time the resolver runs,
                // the call has either been erased by the block_on
                // rewriter (sync scopes) or pushed inside a
                // synthesised state-machine `poll` method (async
                // scopes), making async_scope_depth here unreliable.

                // Try to resolve the callee
                match &callee.kind {
                    ast::ExprKind::Identifier(name) => {
                        if name == "super" {
                            self.check_super_context(&span);
                        }
                        if let Some(orig_def_id) = self.scopes.lookup(name) {
                            // A trailing `do…end` block on a call whose callee
                            // is a FUNCTION (a `def`, including yield-based
                            // implicit-block fns) is block-as-arg sugar:
                            // `runit do…end` == `runit({ ||…end })`. Forward
                            // the block as the last argument and emit a plain
                            // `FnCall`. The `.call` MethodCall path below is
                            // ONLY for a closure-typed VARIABLE identifier
                            // (`let f = {…}; f do…end`). (Q3: previously every
                            // block-bearing call took the `.call` path, so
                            // `runit do…end` treated the function `runit` as a
                            // closure value and passed a bad closure → segfault.)
                            let callee_is_function = matches!(
                                self.symbols.get(orig_def_id).map(|d| &d.kind),
                                Some(DefKind::Function { .. }) | Some(DefKind::OverloadSet { .. })
                            );
                            if callee_is_function {
                                if let Some(blk) = block_hir.take() {
                                    args_hir.push(*blk);
                                }
                                let def_id = self.select_overload_by_args(orig_def_id, &args_hir);
                                self.append_default_args(def_id, &mut args_hir);
                                let callee_name = self
                                    .symbols
                                    .get(def_id)
                                    .map(|d| d.name.clone())
                                    .unwrap_or_else(|| name.clone());
                                let ty = self.type_context.fresh_type_var();
                                return HirExpr {
                                    kind: HirExprKind::FnCall {
                                        callee: def_id,
                                        callee_name,
                                        args: args_hir,
                                    },
                                    ty,
                                    span,
                                };
                            }

                            let def_id = self.select_overload_by_args(orig_def_id, &args_hir);
                            self.append_default_args(def_id, &mut args_hir);
                            let resolved_callee_name = self
                                .symbols
                                .get(def_id)
                                .map(|d| d.name.clone())
                                .unwrap_or_else(|| name.clone());
                            let ty = self.type_context.fresh_type_var();
                            // Closure-typed variable identifier: a trailing
                            // block invokes it via `.call`.
                            let kind = match block_hir {
                                Some(blk) => HirExprKind::MethodCall {
                                    object: Box::new(HirExpr {
                                        kind: HirExprKind::VarRef(def_id),
                                        ty: self.symbols.def_ty(def_id).unwrap_or(Ty::Error),
                                        span: callee.span.clone(),
                                    }),
                                    method: UNRESOLVED_DEF,
                                    resolved: UNRESOLVED_DEF,
                                    method_name: "call".to_string(),
                                    generic_args: vec![],
                                    args: args_hir,
                                    block: Some(blk),
                                    safe_nav: false,
                                    parens: *parens,
                                },
                                None => HirExprKind::FnCall {
                                    callee: def_id,
                                    callee_name: resolved_callee_name,
                                    args: args_hir,
                                },
                            };
                            HirExpr { kind, ty, span }
                        } else if let Some(type_def_id) = self.scopes.lookup_type(name) {
                            // `Name(arg)` where `Name` is the name of a type.
                            // For a zero-cost `newtype Meters(Float)` wrapper
                            // this desugars to a single-field Construct that
                            // can later be read back via `.0`.
                            if let Some(def) = self.symbols.get(type_def_id) {
                                if let DefKind::Newtype { inner } = &def.kind {
                                    let inner_ty = inner.clone();
                                    if args_hir.len() != 1 {
                                        self.error(
                                            format!(
                                                "newtype `{}` expects exactly 1 argument, got {}",
                                                name,
                                                args_hir.len(),
                                            ),
                                            &span,
                                        );
                                        return HirExpr {
                                            kind: HirExprKind::Error,
                                            ty: Ty::Error,
                                            span,
                                        };
                                    }
                                    let arg = args_hir.into_iter().next().unwrap();
                                    let ty = Ty::Newtype {
                                        name: name.clone(),
                                        inner: Box::new(inner_ty),
                                    };
                                    return HirExpr {
                                        kind: HirExprKind::Construct {
                                            type_def: type_def_id,
                                            type_name: name.clone(),
                                            fields: vec![("0".to_string(), arg)],
                                        },
                                        ty,
                                        span,
                                    };
                                }
                            }
                            self.error_with_code(
                                format!("undefined function `{}`", name),
                                &span,
                                "E0735",
                            );
                            self.unresolved_names.push(name.to_string());
                            HirExpr {
                                kind: HirExprKind::Error,
                                ty: Ty::Error,
                                span,
                            }
                        } else {
                            // Could be a type constructor: Type.new(...)
                            self.error_with_code(
                                format!("undefined function `{}`", name),
                                &span,
                                "E0735",
                            );
                            self.unresolved_names.push(name.to_string());
                            HirExpr {
                                kind: HirExprKind::Error,
                                ty: Ty::Error,
                                span,
                            }
                        }
                    }
                    // FieldAccess could be a static method call: Type.method(...)
                    ast::ExprKind::FieldAccess { object, field, .. } => {
                        let obj_hir = self.resolve_expr(object);
                        let ty = self.type_context.fresh_type_var();
                        HirExpr {
                            kind: HirExprKind::MethodCall {
                                object: Box::new(obj_hir),
                                method: UNRESOLVED_DEF,
                                resolved: UNRESOLVED_DEF,
                                method_name: field.clone(),
                                generic_args: vec![],
                                args: args_hir,
                                block: block_hir,
                                safe_nav: false,
                                parens: *parens,
                            },
                            ty,
                            span,
                        }
                    }
                    _ => {
                        let callee_hir = self.resolve_expr(callee);
                        let ty = self.type_context.fresh_type_var();
                        HirExpr {
                            kind: HirExprKind::MethodCall {
                                object: Box::new(callee_hir),
                                method: UNRESOLVED_DEF,
                                resolved: UNRESOLVED_DEF,
                                method_name: "call".to_string(),
                                generic_args: vec![],
                                args: args_hir,
                                block: block_hir,
                                safe_nav: false,
                                parens: *parens,
                            },
                            ty,
                            span,
                        }
                    }
                }
            }
            ast::ExprKind::Index { object, index } => {
                let obj_hir = self.resolve_expr(object);
                let idx_hir = self.resolve_expr(index);
                let ty = self.type_context.fresh_type_var();
                HirExpr {
                    kind: HirExprKind::Index {
                        object: Box::new(obj_hir),
                        index: Box::new(idx_hir),
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::Assign { target, value } => {
                let target_hir = self.resolve_expr(target);
                let value_hir = self.resolve_expr(value);
                HirExpr {
                    kind: HirExprKind::Assign {
                        target: Box::new(target_hir),
                        value: Box::new(value_hir),
                        semantics: MoveSemantics::Move, // determined during type checking
                    },
                    ty: Ty::Unit,
                    span,
                }
            }
            ast::ExprKind::MultiAssign {
                targets,
                values,
                splat_target,
            } => {
                let targets_hir = targets.iter().map(|t| self.resolve_expr(t)).collect();
                let values_hir = values.iter().map(|v| self.resolve_expr(v)).collect();
                HirExpr {
                    kind: HirExprKind::MultiAssign {
                        targets: targets_hir,
                        values: values_hir,
                        splat_target: *splat_target,
                    },
                    ty: Ty::Unit,
                    span,
                }
            }
            ast::ExprKind::CompoundAssign { target, op, value } => {
                let target_hir = self.resolve_expr(target);
                let value_hir = self.resolve_expr(value);
                HirExpr {
                    kind: HirExprKind::CompoundAssign {
                        target: Box::new(target_hir),
                        op: *op,
                        value: Box::new(value_hir),
                    },
                    ty: Ty::Unit,
                    span,
                }
            }
            ast::ExprKind::If(if_expr) => self.resolve_if(if_expr),
            ast::ExprKind::IfLet(if_let) => self.resolve_if_let(if_let),
            ast::ExprKind::Match(match_expr) => self.resolve_match(match_expr),
            ast::ExprKind::While(while_expr) => {
                let cond = self.resolve_expr(&while_expr.condition);
                self.scopes.push(ScopeKind::Loop);
                let body = self.resolve_block_as_expr(&while_expr.body);
                self.scopes.pop();
                HirExpr {
                    kind: HirExprKind::While {
                        condition: Box::new(cond),
                        body: Box::new(body),
                    },
                    ty: Ty::Unit,
                    span,
                }
            }
            ast::ExprKind::WhileLet(wl) => {
                // Desugar while-let to loop + match
                let value = self.resolve_expr(&wl.value);
                self.scopes.push(ScopeKind::Loop);
                let pattern = self.resolve_pattern(&wl.pattern);
                let body = self.resolve_block_as_expr(&wl.body);
                self.scopes.pop();
                let break_expr = HirExpr {
                    kind: HirExprKind::Break(None),
                    ty: Ty::Never,
                    span: span.clone(),
                };
                HirExpr {
                    kind: HirExprKind::Loop {
                        body: Box::new(HirExpr {
                            kind: HirExprKind::Match {
                                scrutinee: Box::new(value),
                                arms: vec![
                                    HirMatchArm {
                                        pattern,
                                        guard: None,
                                        body: Box::new(body),
                                        span: span.clone(),
                                    },
                                    HirMatchArm {
                                        pattern: HirPattern::Wildcard { span: span.clone() },
                                        guard: None,
                                        body: Box::new(break_expr),
                                        span: span.clone(),
                                    },
                                ],
                            },
                            ty: Ty::Unit,
                            span: span.clone(),
                        }),
                    },
                    ty: Ty::Unit,
                    span,
                }
            }
            ast::ExprKind::For(for_expr) => {
                let iterable = self.resolve_expr(&for_expr.iterable);
                self.scopes.push(ScopeKind::Loop);
                let binding_name = self.pattern_binding_name(&for_expr.pattern);
                let pattern = self.resolve_pattern(&for_expr.pattern);
                // A simple `for x in` (or `for &x in`) binds `x` directly.
                // Any other pattern gets an unscoped def for the whole
                // element, and a tuple pattern's sub-bindings are handed to
                // the MIR lowerer.
                let mut tuple_bindings = Vec::new();
                let binding_def = match &pattern {
                    HirPattern::Binding { def_id, .. } | HirPattern::Ref { def_id, .. } => *def_id,
                    _ => {
                        if let HirPattern::Tuple { elements, .. } = &pattern {
                            for elem in elements {
                                if let HirPattern::Binding { def_id, name, .. } = elem {
                                    tuple_bindings.push((*def_id, name.clone()));
                                }
                            }
                        }
                        let binding_ty = self.type_context.fresh_type_var();
                        self.symbols.define(
                            binding_name.clone(),
                            DefKind::Variable {
                                mutable: false,
                                ty: binding_ty,
                            },
                            Visibility::Private,
                            for_expr.pattern.span().clone(),
                        )
                    }
                };
                let body = self.resolve_block_as_expr(&for_expr.body);
                self.scopes.pop();
                HirExpr {
                    kind: HirExprKind::For {
                        binding: binding_def,
                        binding_name,
                        iterable: Box::new(iterable),
                        body: Box::new(body),
                        tuple_bindings,
                    },
                    ty: Ty::Unit,
                    span,
                }
            }
            ast::ExprKind::Loop(loop_expr) => {
                self.scopes.push(ScopeKind::Loop);
                let body = self.resolve_block_as_expr(&loop_expr.body);
                self.scopes.pop();
                HirExpr {
                    kind: HirExprKind::Loop {
                        body: Box::new(body),
                    },
                    ty: self.type_context.fresh_type_var(),
                    span,
                }
            }
            ast::ExprKind::Block(block) => self.resolve_block_as_expr(block),
            ast::ExprKind::Closure(closure) => self.resolve_closure(closure, &span),
            ast::ExprKind::Return(value) => {
                let value_hir = value.as_ref().map(|v| Box::new(self.resolve_expr(v)));
                HirExpr {
                    kind: HirExprKind::Return(value_hir),
                    ty: Ty::Never,
                    span,
                }
            }
            ast::ExprKind::Break(value) => {
                if !self.scopes.in_loop() {
                    self.error("`break` used outside of loop".to_string(), &span);
                }
                let value_hir = value.as_ref().map(|v| Box::new(self.resolve_expr(v)));
                HirExpr {
                    kind: HirExprKind::Break(value_hir),
                    ty: Ty::Never,
                    span,
                }
            }
            ast::ExprKind::Continue => {
                if !self.scopes.in_loop() {
                    self.error("`continue` used outside of loop".to_string(), &span);
                }
                HirExpr {
                    kind: HirExprKind::Continue,
                    ty: Ty::Never,
                    span,
                }
            }
            ast::ExprKind::Next(value) => {
                // Block Tier-2: `next [expr]` is the block's value for this
                // iteration. The parser only produces it inside a closure body,
                // so no scope check is needed here; it lowers to an early
                // return from the block's closure.
                let value_hir = value.as_ref().map(|v| Box::new(self.resolve_expr(v)));
                HirExpr {
                    kind: HirExprKind::Next(value_hir),
                    ty: Ty::Never,
                    span,
                }
            }
            ast::ExprKind::Range {
                start,
                end,
                inclusive,
            } => {
                let start_hir = start.as_ref().map(|s| Box::new(self.resolve_expr(s)));
                let end_hir = end.as_ref().map(|e| Box::new(self.resolve_expr(e)));
                let ty = self.type_context.fresh_type_var();
                HirExpr {
                    kind: HirExprKind::Range {
                        start: start_hir,
                        end: end_hir,
                        inclusive: *inclusive,
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::ArrayLiteral(elems) => {
                let elems_hir: Vec<HirExpr> = elems.iter().map(|e| self.resolve_expr(e)).collect();
                let elem_ty = if elems_hir.is_empty() {
                    self.type_context.fresh_type_var()
                } else {
                    elems_hir[0].ty.clone()
                };
                let ty = Ty::Array(Box::new(elem_ty));
                HirExpr {
                    kind: HirExprKind::ArrayLiteral(elems_hir),
                    ty,
                    span,
                }
            }
            ast::ExprKind::MapLiteral(entries) => {
                let entries_hir: Vec<(HirExpr, HirExpr)> = entries
                    .iter()
                    .map(|(k, v)| (self.resolve_expr(k), self.resolve_expr(v)))
                    .collect();
                let (k_ty, v_ty) = if let Some((k, v)) = entries_hir.first() {
                    (k.ty.clone(), v.ty.clone())
                } else {
                    (
                        self.type_context.fresh_type_var(),
                        self.type_context.fresh_type_var(),
                    )
                };
                let ty = Ty::Map(Box::new(k_ty), Box::new(v_ty));
                HirExpr {
                    kind: HirExprKind::MapLiteral(entries_hir),
                    ty,
                    span,
                }
            }
            ast::ExprKind::ArrayFill { value, count } => {
                let value_hir = self.resolve_expr(value);
                let count_hir = self.resolve_expr(count);
                let elem_ty = value_hir.ty.clone();
                // Try to extract count as a usize
                let count_val = match &count_hir.kind {
                    HirExprKind::IntLiteral(n) => *n as usize,
                    _ => 0, // will be validated during type checking
                };
                HirExpr {
                    kind: HirExprKind::ArrayFill {
                        value: Box::new(value_hir),
                        count: count_val,
                    },
                    ty: Ty::FixedArray(
                        Box::new(elem_ty),
                        crate::hir::types::ConstExpr::Lit(count_val as u64),
                    ),
                    span,
                }
            }
            ast::ExprKind::TupleLiteral(elems) => {
                let elems_hir: Vec<HirExpr> = elems.iter().map(|e| self.resolve_expr(e)).collect();
                let tys: Vec<Ty> = elems_hir.iter().map(|e| e.ty.clone()).collect();
                HirExpr {
                    kind: HirExprKind::Tuple(elems_hir),
                    ty: Ty::Tuple(tys),
                    span,
                }
            }
            ast::ExprKind::Cast {
                expr: inner,
                target_type,
            } => {
                let inner_hir = self.resolve_expr(inner);
                let target = self.resolve_type_expr(target_type);
                HirExpr {
                    kind: HirExprKind::Cast {
                        expr: Box::new(inner_hir),
                        target: target.clone(),
                    },
                    ty: target,
                    span,
                }
            }
            ast::ExprKind::Await(inner) => {
                if self.async_scope_depth == 0 {
                    self.diagnostics.push(Diagnostic::error_with_code(
                        "`.await` is only valid inside `async def` or `async { }`",
                        span.clone(),
                        "E1110",
                    ));
                }
                let inner_hir = self.resolve_expr(inner);
                let ty = self.type_context.fresh_type_var();
                HirExpr {
                    kind: HirExprKind::MethodCall {
                        object: Box::new(inner_hir),
                        method: UNRESOLVED_DEF,
                        resolved: UNRESOLVED_DEF,
                        method_name: "await".to_string(),
                        generic_args: vec![],
                        args: vec![],
                        block: None,
                        safe_nav: false,
                        parens: false,
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::Try(inner) => {
                // Desugar `expr?` to match + early return
                let inner_hir = self.resolve_expr(inner);
                let result_ty = self.type_context.fresh_type_var();
                // For now, represent as a method call to a special `try_unwrap` operation
                // The type checker will handle the actual desugaring
                HirExpr {
                    kind: HirExprKind::MethodCall {
                        object: Box::new(inner_hir),
                        method: UNRESOLVED_DEF,
                        resolved: UNRESOLVED_DEF,
                        method_name: "try_op".to_string(),
                        generic_args: vec![],
                        args: vec![],
                        block: None,
                        safe_nav: false,
                        parens: false,
                    },
                    ty: result_ty,
                    span,
                }
            }
            // `x&.name`: typeck decides field or method and wraps the
            // result in `T?`; MIR branches on the receiver's tag.
            ast::ExprKind::SafeNav { object, field } => {
                let obj_hir = self.resolve_expr(object);
                let ty = self.type_context.fresh_type_var();
                HirExpr {
                    kind: HirExprKind::FieldAccess {
                        object: Box::new(obj_hir),
                        field_name: field.clone(),
                        field_idx: 0,
                        safe_nav: true,
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::SafeNavCall {
                object,
                method,
                args,
            } => {
                let obj_hir = self.resolve_expr(object);
                let args_hir: Vec<HirExpr> = args.iter().map(|a| self.resolve_expr(a)).collect();
                let ty = self.type_context.fresh_type_var();
                HirExpr {
                    kind: HirExprKind::MethodCall {
                        object: Box::new(obj_hir),
                        method: UNRESOLVED_DEF,
                        resolved: UNRESOLVED_DEF,
                        method_name: method.clone(),
                        generic_args: vec![],
                        args: args_hir,
                        block: None,
                        safe_nav: true,
                        parens: true,
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::MacroCall { name, args, .. } => {
                let args_hir: Vec<HirExpr> = args.iter().map(|a| self.resolve_expr(a)).collect();
                // B87: no unrecognised macro may survive past this point. It
                // used to fall through to a fresh type var here and then to a
                // `_ => Ok(None)` catch-all in MIR, which silently deleted the
                // whole call. The parser only builds a `MacroCall` for a name
                // in `parser::macros::BUILTIN_MACRO_NAMES`; of those, the
                // retired collection macros (ruby-naming.spec.md §10a) are
                // rejected here with a migration message.
                let message = if crate::parser::macros::RETIRED_MACRO_NAMES.contains(&name.as_str())
                {
                    Some(format!(
                        "macro `{name}!` is retired: use the literal form \
                         (`[a, b]`, `{{ k => v }}`, `[a, b].to_set`)"
                    ))
                } else if !crate::parser::macros::is_builtin_macro(name) {
                    Some(format!(
                        "unknown macro `{name}!` — the compiler has no lowering for it, so \
                         the call would silently do nothing"
                    ))
                } else {
                    None
                };
                if let Some(message) = message {
                    self.error_with_code(message, &span, "E1126");
                    return HirExpr {
                        kind: HirExprKind::Error,
                        ty: Ty::Error,
                        span,
                    };
                }
                let ty = match name.as_str() {
                    "panic" => Ty::Never,
                    _ => self.type_context.fresh_type_var(),
                };
                HirExpr {
                    kind: HirExprKind::MacroCall {
                        name: name.clone(),
                        args: args_hir,
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::EnumVariant {
                type_path,
                variant,
                args,
            } => {
                let type_name = type_path.join(".");
                let composite = format!("{}.{}", type_name, variant);
                let variant_def = self.scopes.lookup(&composite).unwrap_or(UNRESOLVED_DEF);
                let mut type_def = self
                    .type_registry
                    .get(&type_name)
                    .copied()
                    .unwrap_or(UNRESOLVED_DEF);

                // For bare variants (Ok, Err, Some, None) where type_path is empty,
                // look up the parent enum from the variant definition
                let mut resolved_type_name = type_name.clone();
                if type_def == UNRESOLVED_DEF && variant_def != UNRESOLVED_DEF {
                    if let Some(def) = self.symbols.get(variant_def) {
                        if let DefKind::EnumVariant { parent, .. } = &def.kind {
                            type_def = *parent;
                            if let Some(parent_def) = self.symbols.get(*parent) {
                                resolved_type_name = parent_def.name.clone();
                            }
                        }
                    }
                }

                // Extract variant_idx first to avoid borrow conflicts
                let variant_idx = if variant_def != UNRESOLVED_DEF {
                    self.symbols
                        .get(variant_def)
                        .and_then(|def| {
                            if let DefKind::EnumVariant { variant_idx, .. } = &def.kind {
                                Some(*variant_idx)
                            } else {
                                None
                            }
                        })
                        .unwrap_or(0)
                } else {
                    self.error(
                        format!("undefined enum variant `{}.{}`", type_name, variant),
                        &span,
                    );
                    0
                };

                let fields_hir: Vec<(String, HirExpr)> = args
                    .iter()
                    .map(|fa| {
                        (
                            fa.name.clone().unwrap_or_default(),
                            self.resolve_expr(&fa.value),
                        )
                    })
                    .collect();

                let ty = if type_def != UNRESOLVED_DEF {
                    Ty::Enum {
                        name: resolved_type_name.clone(),
                        generic_args: vec![],
                    }
                } else {
                    Ty::Error
                };

                HirExpr {
                    kind: HirExprKind::EnumVariant {
                        type_def,
                        type_name: resolved_type_name,
                        variant_name: variant.clone(),
                        variant_idx,
                        fields: fields_hir,
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::ClosureCall { callee, args } => {
                let callee_hir = self.resolve_expr(callee);
                let args_hir: Vec<HirExpr> = args.iter().map(|a| self.resolve_expr(a)).collect();
                let ty = self.type_context.fresh_type_var();
                HirExpr {
                    kind: HirExprKind::MethodCall {
                        object: Box::new(callee_hir),
                        method: UNRESOLVED_DEF,
                        resolved: UNRESOLVED_DEF,
                        method_name: "call".to_string(),
                        generic_args: vec![],
                        args: args_hir,
                        block: None,
                        safe_nav: false,
                        parens: true,
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::Yield(args) => {
                let args_hir: Vec<HirExpr> = args.iter().map(|a| self.resolve_expr(a)).collect();
                // `yield VALUE …` desugars to `BLOCK.(VALUE …)`, encoded as
                // a MethodCall with method_name == "call" on the enclosing
                // function's block parameter.  Prefer the synthetic
                // `__block` inserted for implicit-block functions; fall
                // back to the explicit `&block` parameter name that the
                // older `Block(…)` syntax produces.  If neither is in
                // scope (e.g. a `yield` sitting inside a nested closure
                // whose enclosing method has no block), we keep the old
                // unresolved-FnCall shape so downstream passes can report
                // a clearer error.
                let block_def = self
                    .scopes
                    .lookup("__block")
                    .or_else(|| self.scopes.lookup("&block"));
                if let Some(block_def) = block_def {
                    let block_ty = self.symbols.def_ty(block_def).unwrap_or(Ty::Error);
                    let callee = HirExpr {
                        kind: HirExprKind::VarRef(block_def),
                        ty: block_ty,
                        span: span.clone(),
                    };
                    let ty = self.type_context.fresh_type_var();
                    HirExpr {
                        kind: HirExprKind::MethodCall {
                            object: Box::new(callee),
                            method: UNRESOLVED_DEF,
                            resolved: UNRESOLVED_DEF,
                            method_name: "call".to_string(),
                            generic_args: vec![],
                            args: args_hir,
                            block: None,
                            safe_nav: false,
                            parens: true,
                        },
                        ty,
                        span,
                    }
                } else {
                    let ty = self.type_context.fresh_type_var();
                    HirExpr {
                        kind: HirExprKind::FnCall {
                            callee: UNRESOLVED_DEF,
                            callee_name: "yield".to_string(),
                            args: args_hir,
                        },
                        ty,
                        span,
                    }
                }
            }
            ast::ExprKind::BlockPass(_) => {
                // A `&`-form block argument is only valid as the LAST argument
                // of a call (where the MethodCall/Call arms hoist it into the
                // block slot). Anywhere else it is malformed.
                self.diagnostics.push(Diagnostic::error_with_code(
                    "a `&`-block argument (`&:sym`, `&block`, `&`) must be the last \
                     argument of a call",
                    span.clone(),
                    "E1124",
                ));
                HirExpr {
                    kind: HirExprKind::Error,
                    ty: Ty::Error,
                    span,
                }
            }
            ast::ExprKind::UnsafeBlock(block) => {
                // Same body resolution as a regular block (including bare
                // `x = v` first-assignment bindings), rewrapped as unsafe.
                let resolved = self.resolve_block_as_expr(block);
                let HirExprKind::Block(stmts, tail_expr) = resolved.kind else {
                    unreachable!("resolve_block_as_expr always yields HirExprKind::Block")
                };
                HirExpr {
                    kind: HirExprKind::UnsafeBlock(stmts, tail_expr),
                    ty: resolved.ty,
                    span,
                }
            }
            ast::ExprKind::NullLiteral => {
                HirExpr {
                    kind: HirExprKind::NullLiteral,
                    ty: Ty::UInt64, // null is a zero-valued pointer; for now UInt64
                    span,
                }
            }
        }
    }

    // ─── Block Resolution ───────────────────────────────────────────
}

#[cfg(test)]
mod tests {
    use crate::lexer::Lexer;
    use crate::parser::Parser;
    use crate::resolve::Resolver;

    fn resolve_codes(source: &str) -> Vec<Option<String>> {
        let tokens = Lexer::new(source).tokenize().expect("lexer failed");
        let program = Parser::new(tokens).parse().expect("parser failed");
        Resolver::new()
            .resolve(&program)
            .diagnostics
            .into_iter()
            .map(|d| d.code)
            .collect()
    }

    #[test]
    fn unsafe_block_binds_a_bare_assignment() {
        let codes = resolve_codes("def main\n  unsafe\n    x = 5\n    x\n  end\nend\n");
        assert!(codes.is_empty(), "{codes:?}");
    }

    #[test]
    fn undefined_variable_is_e0734() {
        let codes = resolve_codes("def main\n  totl\nend\n");
        assert_eq!(codes, vec![Some("E0734".to_string())]);
    }

    #[test]
    fn undefined_function_is_e0735() {
        let codes = resolve_codes("def main\n  greet(1)\nend\n");
        assert_eq!(codes, vec![Some("E0735".to_string())]);
    }
}
