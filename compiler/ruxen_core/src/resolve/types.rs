#![allow(unused_imports)]

use std::collections::HashMap;

use crate::diagnostics::Diagnostic;
use crate::hir::context::TypeContext;
use crate::hir::nodes::*;
use crate::hir::types::{MixinRef, MoveSemantics, Ty};
use crate::lexer::token::Span;
use crate::parser::ast::{self, Visibility};

use super::const_helpers;
use super::scope::{ScopeId, ScopeKind, ScopeStack};
use super::symbols::*;
use super::{ClosureCaptureContext, ResolveResult, Resolver};

/// The built-in collection type names. These are NOT registered as classes
/// in the type scope by `register_builtins`; their type resolution is the
/// three hardcoded arms in [`Resolver::resolve_type_expr`] below
/// (`"Array" | "Vec"` → `Ty::Array`, `"Map" | "HashMap"` → `Ty::Map`,
/// `"Set" | "HashSet"` → `Ty::Set`, the last two with E0615 hash-key
/// validation). The const is the single source of truth shared with
/// `ffi_registration.rs`'s anchor-only-builtin check, so the membership
/// test there and the arms here cannot drift apart.
///
/// Invariant: this list must contain EXACTLY the names handled by the three
/// `resolve_type_expr` collection arms. It de-dups the ffi `matches!`; it
/// does NOT (and cannot) collapse those three arms, which each produce a
/// different `Ty` and carry distinct validation.
pub(crate) const COLLECTION_BUILTINS: &[&str] =
    &["Array", "Vec", "Map", "HashMap", "Set", "HashSet"];

impl Resolver {
    pub fn resolve_type_expr(&mut self, type_expr: &ast::TypeExpr) -> Ty {
        match type_expr {
            ast::TypeExpr::Named(path) => self.resolve_type_path(path),
            ast::TypeExpr::Reference {
                lifetime,
                mutable,
                inner,
                span: ref_span,
            } => {
                // Mixin vtables Phase A — `&Mixin` / `&var Mixin`.
                // When `inner` is a bare name that resolves to a mixin
                // (DefKind::Trait), this is a dyn-shape reference. The
                // mixin must have `dispatch runtime`; otherwise emit
                // E1118 (no vtable to dispatch through). When valid,
                // model it as `Ty::Ref(Ty::AnyMixin([MixinRef]))` so
                // downstream typeck reuses the existing dyn-mixin
                // satisfaction path. Codegen of the actual vtable is
                // Phase B/C.
                if let Some(mixin_ty) = self.try_resolve_dyn_mixin_ref(inner, ref_span) {
                    return match (lifetime, mutable) {
                        (Some(lt), true) => Ty::RefMutLifetime(lt.clone(), Box::new(mixin_ty)),
                        (Some(lt), false) => Ty::RefLifetime(lt.clone(), Box::new(mixin_ty)),
                        (None, true) => Ty::RefMut(Box::new(mixin_ty)),
                        (None, false) => Ty::Ref(Box::new(mixin_ty)),
                    };
                }

                let inner_ty = self.resolve_type_expr(inner);
                match (lifetime, mutable) {
                    (Some(lt), true) => Ty::RefMutLifetime(lt.clone(), Box::new(inner_ty)),
                    (Some(lt), false) => Ty::RefLifetime(lt.clone(), Box::new(inner_ty)),
                    (None, true) => Ty::RefMut(Box::new(inner_ty)),
                    (None, false) => Ty::Ref(Box::new(inner_ty)),
                }
            }
            ast::TypeExpr::Tuple { elements, .. } => {
                if elements.is_empty() {
                    Ty::Unit
                } else {
                    Ty::Tuple(elements.iter().map(|e| self.resolve_type_expr(e)).collect())
                }
            }
            ast::TypeExpr::Array { element, size, .. } => {
                let elem_ty = self.resolve_type_expr(element);
                if let Some(size_expr) = size {
                    // Fixed-size array [T; N].  T2.02 stage 4: the
                    // size is captured as a `ConstExpr` rather than
                    // a bare `usize`.  T2.02 stage 8: `+ - * /` and
                    // parens fold into `ConstExpr::Op` trees; S8.S4
                    // normalises identities (`N + 0 = N`, …) so
                    // `[T; N + 0]` and `[T; N]` produce the same
                    // `Ty`.  S8.S4 follow-up: pure-literal subtrees
                    // that overflow or divide by zero surface as
                    // E0703 here — they're invariant across
                    // instantiations.
                    let n = const_helpers::lower_const_expr_from_expr(size_expr).normal_form();
                    self.check_const_expr_for_non_const(&n, &size_expr.span);
                    self.check_const_expr_eval_errors(&n, &size_expr.span);
                    Ty::FixedArray(Box::new(elem_ty), n)
                } else {
                    // Slice [T] — treat as Vec for now
                    Ty::Array(Box::new(elem_ty))
                }
            }
            ast::TypeExpr::Function {
                params,
                return_type,
                ..
            } => Ty::Fn {
                params: params.iter().map(|p| self.resolve_type_expr(p)).collect(),
                ret: Box::new(self.resolve_type_expr(return_type)),
            },
            ast::TypeExpr::SomeMixin { bounds, .. } => Ty::SomeMixin(
                bounds
                    .iter()
                    .map(|b| MixinRef {
                        name: b.path.segments.join("."),
                        generic_args: b
                            .path
                            .generic_args
                            .as_ref()
                            .map(|args| args.iter().map(|a| self.resolve_type_expr(a)).collect())
                            .unwrap_or_default(),
                    })
                    .collect(),
            ),
            ast::TypeExpr::AnyMixin { bounds, .. } => Ty::AnyMixin(
                bounds
                    .iter()
                    .map(|b| MixinRef {
                        name: b.path.segments.join("."),
                        generic_args: b
                            .path
                            .generic_args
                            .as_ref()
                            .map(|args| args.iter().map(|a| self.resolve_type_expr(a)).collect())
                            .unwrap_or_default(),
                    })
                    .collect(),
            ),
            ast::TypeExpr::Never { .. } => Ty::Never,
            ast::TypeExpr::Inferred { .. } => self.type_context.fresh_type_var(),
            ast::TypeExpr::RawPointer { mutable, inner, .. } => {
                let inner_ty = self.resolve_type_expr(inner);
                // Check for *Void and *mut Void
                if matches!(&inner_ty, Ty::Struct { name, .. } | Ty::Class { name, .. } if name == "Void")
                    || matches!(&inner_ty, Ty::Error)
                        && matches!(inner.as_ref(), ast::TypeExpr::Named(p) if p.segments == ["Void"])
                {
                    if *mutable {
                        Ty::RawPtrMutVoid
                    } else {
                        Ty::RawPtrVoid
                    }
                } else if let ast::TypeExpr::Named(p) = inner.as_ref() {
                    if p.segments == ["Void"] {
                        if *mutable {
                            Ty::RawPtrMutVoid
                        } else {
                            Ty::RawPtrVoid
                        }
                    } else if *mutable {
                        Ty::RawPtrMut(Box::new(inner_ty))
                    } else {
                        Ty::RawPtr(Box::new(inner_ty))
                    }
                } else if *mutable {
                    Ty::RawPtrMut(Box::new(inner_ty))
                } else {
                    Ty::RawPtr(Box::new(inner_ty))
                }
            }
            // Stage 2 of const generics — parser only.  A ConstLit in
            // generic-arg position has no type-level meaning yet;
            // resolve currently treats it as `Ty::Error` so that any
            // accidental use against a type parameter degrades safely.
            // S3 will introduce DefKind::ConstParam and promote ConstLit
            // to a real `ConstExpr::Lit` against const params, then
            // emit E0704 against type params.
            // T2.02 S6: a `ConstLit` in a generic-arg position
            // becomes a `Ty::ConstArg(ConstExpr::Lit(v))` so distinct
            // const instantiations of a generic type produce
            // distinct Ty values.  The S5 kind-check (above the call
            // site) emits E0704 when this lands against a Type slot;
            // here we only build the value.
            ast::TypeExpr::ConstLit { value, .. } => {
                Ty::ConstArg(crate::hir::types::ConstExpr::Lit(*value as u64))
            }
            // T2.02 S8.S3: an arithmetic const expression in a
            // generic-arg position folds through the same
            // `lower_const_expr_from_expr` helper that S8.S2 uses
            // for `[T; expr]` array sizes.  The kind-check (above
            // the call site) also treats this as a const-arg slot.
            // S8.S4: rewrite to normal form so `Vector[T, N + 0]`
            // and `Vector[T, N]` produce the same `Ty::ConstArg`.
            // S8.S4 follow-up: surface pure-literal overflow /
            // div-zero as E0703 against the source span.
            ast::TypeExpr::ConstExprArg { expr, span } => {
                let folded = const_helpers::lower_const_expr_from_expr(expr).normal_form();
                self.check_const_expr_for_non_const(&folded, span);
                self.check_const_expr_eval_errors(&folded, span);
                Ty::ConstArg(folded)
            }
        }
    }

    pub(super) fn resolve_type_path(&mut self, path: &ast::TypePath) -> Ty {
        // Handle `Self.AssocName` — an associated-type reference.
        // Inside an impl block where `type AssocName = X` is declared,
        // map to `X` directly; inside a trait body, map to an opaque
        // `TypeParam` placeholder bound by the enclosing trait.
        if path.segments.len() == 2 && path.segments[0] == "Self" {
            let assoc = &path.segments[1];
            if let Some(ty) = self.current_impl_assoc_types.get(assoc) {
                return ty.clone();
            }
            if let Some((trait_name, names)) = &self.current_trait_context {
                if names.iter().any(|n| n == assoc) {
                    return Ty::TypeParam {
                        name: format!("Self::{}", assoc),
                        bounds: vec![MixinRef {
                            name: trait_name.clone(),
                            generic_args: vec![],
                        }],
                    };
                }
            }
            // Fall through to the default error path with the joined name.
        }

        let name = path.segments.join(".");

        // Tier-2 const generics S5: kind-check each generic-arg slot
        // against the declared param kind on the target type before
        // running the generic-arg resolution loop.  A `ConstLit` at
        // a slot whose declared param is `Type` is E0700 (kind
        // mismatch).  We look the target up by name in the type
        // registry; built-in containers (Vec/HashMap/Set/etc.) have
        // no const-param slots, so any ConstLit against them is E0700.
        if let Some(ast_args) = path.generic_args.as_ref() {
            // Snapshot the declared param kinds + the declared const
            // type for each Const slot (None for Type slots).  Type slots
            // → kind-check (E0704); Const slots → value-type-fit check
            // (E0701).
            // Snapshot the declared `GenericParamKind` per slot.  The
            // `Const { ty }` variant carries the declared const-param
            // type, used downstream for the E0701 type-fit check.
            let declared_kinds: Option<Vec<GenericParamKind>> = self
                .type_registry
                .get(&name)
                .copied()
                .and_then(|id| self.symbols.get(id))
                .and_then(|def| match &def.kind {
                    DefKind::Class { info } => Some(
                        info.generic_params
                            .iter()
                            .map(|gp| gp.kind.clone())
                            .collect(),
                    ),
                    DefKind::Struct { info } => Some(
                        info.generic_params
                            .iter()
                            .map(|gp| gp.kind.clone())
                            .collect(),
                    ),
                    DefKind::Enum { info } => Some(
                        info.generic_params
                            .iter()
                            .map(|gp| gp.kind.clone())
                            .collect(),
                    ),
                    _ => None,
                });
            for (idx, arg) in ast_args.iter().enumerate() {
                let is_const_arg = matches!(
                    arg,
                    ast::TypeExpr::ConstLit { .. } | ast::TypeExpr::ConstExprArg { .. }
                );
                if !is_const_arg {
                    continue;
                }
                let declared_kind = declared_kinds
                    .as_ref()
                    .and_then(|ks| ks.get(idx).cloned())
                    .unwrap_or(GenericParamKind::Type);
                if matches!(declared_kind, GenericParamKind::Type) {
                    let arg_span = match arg {
                        ast::TypeExpr::ConstLit { span, .. } => span.clone(),
                        ast::TypeExpr::ConstExprArg { span, .. } => span.clone(),
                        _ => path.span.clone(),
                    };
                    let what = match arg {
                        ast::TypeExpr::ConstLit { .. } => "const literal",
                        ast::TypeExpr::ConstExprArg { .. } => "const expression",
                        _ => "const argument",
                    };
                    self.diagnostics.push(Diagnostic::error_with_code(
                        format!(
                            "expected a type at generic argument position {}, found {}",
                            idx + 1,
                            what
                        ),
                        arg_span,
                        // E0704 — kind mismatch on const-generic arg.  Previously
                        // shared E0700 with the iterator-`sum` validator; spec
                        // §"Error code reservations" was amended to fork them
                        // (iterator-sum keeps E0700; this is E0704).
                        "E0704",
                    ));
                } else if let GenericParamKind::Const { ty: declared_ty } = declared_kind {
                    // E0701 — wrong const-arg type.  Kind matches
                    // (const → const slot), but the literal value
                    // doesn't fit the declared type.  Today reachable
                    // when a Bool const-param is given an int literal
                    // other than 0 / 1; future overflows on tight
                    // unsigned widths would land here too once parser
                    // accepts negative literals or wider arithmetic.
                    let arg_value: Option<i64> = match arg {
                        ast::TypeExpr::ConstLit { value, .. } => Some(*value),
                        ast::TypeExpr::ConstExprArg { expr, .. } => {
                            // Fold via the same path used for
                            // resolution, then read the literal value
                            // if we have one after normalization.
                            let folded =
                                const_helpers::lower_const_expr_from_expr(expr).normal_form();
                            folded.as_lit().map(|v| v as i64)
                        }
                        _ => None,
                    };
                    let arg_span = match arg {
                        ast::TypeExpr::ConstLit { span, .. } => span.clone(),
                        ast::TypeExpr::ConstExprArg { span, .. } => span.clone(),
                        _ => path.span.clone(),
                    };
                    if let Some(v) = arg_value {
                        let fits = match &declared_ty {
                            Ty::Bool => v == 0 || v == 1,
                            // Unsigned families: parser produces non-
                            // negative literals today, but be defensive.
                            Ty::USize
                            | Ty::UInt
                            | Ty::UInt8
                            | Ty::UInt16
                            | Ty::UInt32
                            | Ty::UInt64 => v >= 0,
                            // Signed / other integer families accept
                            // every i64 value the parser can produce.
                            _ => true,
                        };
                        if !fits {
                            self.diagnostics.push(Diagnostic::error_with_code(
                                format!(
                                    "const-generic argument `{}` does not fit declared type `{}`",
                                    v, declared_ty
                                ),
                                arg_span,
                                "E0701",
                            ));
                        }
                    }
                }
            }
        }

        let generic_args: Vec<Ty> = path
            .generic_args
            .as_ref()
            .map(|args| args.iter().map(|a| self.resolve_type_expr(a)).collect())
            .unwrap_or_default();

        // T2.02 S9 enforcement: at every instantiation site with const
        // args, walk the target type's where-clause const predicates
        // against the binding map.  Any predicate that evaluates to
        // false produces E0706.  Predicates that still reference
        // unresolved params (e.g. instantiating with a parent's const
        // param that hasn't been substituted yet) are skipped — they
        // re-evaluate at the outer instantiation.
        if let Some(class_def_id) = self.type_registry.get(&name).copied() {
            // Build the binding map for this instantiation.
            let predicates: Vec<crate::resolve::symbols::HirConstPredicate> = self
                .symbols
                .get(class_def_id)
                .map(|def| match &def.kind {
                    DefKind::Class { info } => info.const_predicates.clone(),
                    DefKind::Struct { info } => info.const_predicates.clone(),
                    DefKind::Enum { info } => info.const_predicates.clone(),
                    _ => vec![],
                })
                .unwrap_or_default();
            if !predicates.is_empty() {
                let declared_params: Vec<crate::resolve::symbols::GenericParamInfo> = self
                    .symbols
                    .get(class_def_id)
                    .map(|def| match &def.kind {
                        DefKind::Class { info } => info.generic_params.clone(),
                        DefKind::Struct { info } => info.generic_params.clone(),
                        DefKind::Enum { info } => info.generic_params.clone(),
                        _ => vec![],
                    })
                    .unwrap_or_default();
                let mut bindings = std::collections::HashMap::new();
                let empty_inner = std::collections::HashMap::new();
                for (param, arg) in declared_params.iter().zip(generic_args.iter()) {
                    if matches!(param.kind, GenericParamKind::Const { .. }) {
                        if let Ty::ConstArg(ce) = arg {
                            if let Ok(v) = ce.eval(&empty_inner) {
                                bindings.insert(param.name.clone(), v);
                            }
                        }
                    }
                }
                for pred in &predicates {
                    if let Some(false) = const_helpers::eval_const_predicate(pred, &bindings) {
                        self.diagnostics.push(Diagnostic::error_with_code(
                            format!(
                                "where-clause predicate is not satisfied at this instantiation of `{}`",
                                name
                            ),
                            pred.span.clone(),
                            "E0706",
                        ));
                    }
                }
            }
        }

        // Check built-in generic types
        match name.as_str() {
            // `Array[T]` was `Vec[T]` pre-Ruby-naming. The legacy spelling
            // is kept as an alias so older sources still resolve while
            // the new vocabulary settles.
            "Array" | "Vec" => {
                let elem = generic_args
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| self.type_context.fresh_type_var());
                return Ty::Array(Box::new(elem));
            }
            // `Map[K, V]` was `HashMap[K, V]` pre-Ruby-naming.
            "Map" | "HashMap" => {
                let mut iter = generic_args.into_iter();
                let k = iter
                    .next()
                    .unwrap_or_else(|| self.type_context.fresh_type_var());
                let v = iter
                    .next()
                    .unwrap_or_else(|| self.type_context.fresh_type_var());
                // K must be Hash + Eq. Reject compound containers
                // (Array/Set/Map) and aggregates that don't derive Hash.
                if !const_helpers::ty_is_valid_hash_key(&k, &self.symbols) {
                    self.diagnostics.push(Diagnostic::error_with_code(
                        format!(
                            "Map key type `{}` is not hashable: K must include Hashable + Eq",
                            k
                        ),
                        path.span.clone(),
                        "E0615",
                    ));
                }
                return Ty::Map(Box::new(k), Box::new(v));
            }
            "Set" | "HashSet" => {
                // `HashSet[T]` is the legacy spelling for `Set[T]`. Both
                // desugar to the same runtime representation; method
                // dispatch in `codegen::runtime::runtime_name` accepts
                // either prefix.
                let elem = generic_args
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| self.type_context.fresh_type_var());
                if !const_helpers::ty_is_valid_hash_key(&elem, &self.symbols) {
                    self.diagnostics.push(Diagnostic::error_with_code(
                        format!(
                            "Set element type `{}` is not hashable: T must include Hashable + Eq",
                            elem
                        ),
                        path.span.clone(),
                        "E0615",
                    ));
                }
                return Ty::Set(Box::new(elem));
            }
            "Option" => {
                let inner = generic_args
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| self.type_context.fresh_type_var());
                return Ty::Option(Box::new(inner));
            }
            "Result" => {
                let mut iter = generic_args.into_iter();
                let ok = iter
                    .next()
                    .unwrap_or_else(|| self.type_context.fresh_type_var());
                let err = iter
                    .next()
                    .unwrap_or_else(|| self.type_context.fresh_type_var());
                return Ty::Result(Box::new(ok), Box::new(err));
            }
            "Box" => {
                let inner = generic_args
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| self.type_context.fresh_type_var());
                return Ty::Class {
                    name: "Box".to_string(),
                    generic_args: vec![inner],
                };
            }
            "Fn" => {
                if let Some((ret, params)) = generic_args.split_last() {
                    return Ty::Fn {
                        params: params.to_vec(),
                        ret: Box::new(ret.clone()),
                    };
                }
            }
            "FnMut" => {
                if let Some((ret, params)) = generic_args.split_last() {
                    return Ty::FnMut {
                        params: params.to_vec(),
                        ret: Box::new(ret.clone()),
                    };
                }
            }
            "Block" => {
                // Block(&T) -> Bool is like Fn
                if let Some((ret, params)) = generic_args.split_last() {
                    return Ty::Fn {
                        params: params.to_vec(),
                        ret: Box::new(ret.clone()),
                    };
                }
            }
            _ => {}
        }

        // Look up in type registry
        if let Some(&def_id) = self.type_registry.get(&name) {
            if let Some(def) = self.symbols.get(def_id) {
                match &def.kind {
                    DefKind::TypeAlias { target } => return target.clone(),
                    DefKind::Class { .. } => {
                        // Root-cause normalisation for the
                        // String-builtin-anchored-onto-stdlib-class case:
                        // `register_builtins` registers `String` as
                        // `DefKind::TypeAlias { target: Ty::String }`. The
                        // bootstrap merge then anchors
                        // `class String` (in `library/std/string/src/lib.rx`)
                        // onto the same `DefId` so FFI methods attach.
                        // `resolve_class` later stomps the `DefKind` to
                        // `Class { name: "String", .. }`. Without this
                        // fast-path, every `String` annotation downstream
                        // resolves to `Ty::Class { "String" }` and every
                        // typeck consumer pattern-matching on the
                        // canonical `Ty::String` silently misses it (see
                        // `project_ruxen_resolve_class_stomps_typealias`
                        // memory). Normalising at the resolve layer keeps
                        // the typeck representation aligned with the
                        // builtin primitive while leaving the class
                        // surface intact for `String.from(...)`-style
                        // class-method dispatch (those go through the
                        // AST path / type-registry lookup, not the
                        // returned `Ty`). Method dispatch on receivers
                        // typed `Ty::String` already resolves through
                        // `typeck/method_resolvers` and the FFI alias
                        // map keyed on the class name.
                        if name == "String" && generic_args.is_empty() {
                            return Ty::String;
                        }
                        return Ty::Class { name, generic_args };
                    }
                    DefKind::Struct { .. } => {
                        return Ty::Struct { name, generic_args };
                    }
                    DefKind::Enum { .. } => {
                        return Ty::Enum { name, generic_args };
                    }
                    DefKind::Trait { .. } => {
                        // A trait used as a type — impl Trait or type param
                        return Ty::TypeParam {
                            name,
                            bounds: vec![],
                        };
                    }
                    DefKind::TypeParam { bounds } => {
                        return Ty::TypeParam {
                            name,
                            bounds: bounds.clone(),
                        };
                    }
                    DefKind::Newtype { inner } => {
                        return Ty::Newtype {
                            name,
                            inner: Box::new(inner.clone()),
                        };
                    }
                    _ => {}
                }
            }
        }

        // Check if it's a generic type parameter or type alias in scope.
        // #06.93 Phase 2: root-anchored paths (`::Name`) bypass this
        // scope walk by design — `::Name` means "resolve from the
        // global type registry" so a generic param `T` shadowed at
        // some inner scope is NOT what `::T` means. Phase 1 + global
        // `type_registry` lookups above already handled the rooted
        // case for class / struct / enum / mixin DefKinds; only the
        // generic-param + type-alias scope fallback below is
        // shadowing-sensitive, and it's the one we must skip.
        if !path.rooted {
            if let Some(def_id) = self.scopes.lookup_type(&name) {
                if let Some(def) = self.symbols.get(def_id) {
                    match &def.kind {
                        DefKind::TypeParam { bounds } => {
                            return Ty::TypeParam {
                                name,
                                bounds: bounds.clone(),
                            };
                        }
                        DefKind::TypeAlias { target } => {
                            return target.clone();
                        }
                        _ => {}
                    }
                }
            }
        }

        // Special case: &str
        if name == "str" {
            return Ty::Str;
        }

        self.error(format!("undefined type `{}`", name), &path.span);
        Ty::Error
    }

    /// Phase A of the mixin-vtables spec
    /// (`docs/specs/types/mixin_vtables.spec.md` §B7).
    ///
    /// When a `&Mixin` / `&var Mixin` reference is parsed, the inner
    /// AST is a single-segment `TypeExpr::Named` whose path resolves
    /// to a `DefKind::Trait`. This helper detects that shape and:
    ///
    /// * Returns `Some(Ty::AnyMixin(...))` when the mixin is marked
    ///   `dispatch runtime` (the dyn-shape reference is valid).
    /// * Emits **E1118** and returns `Some(Ty::Error)` when the mixin
    ///   is statically-dispatched (the reference has no vtable to
    ///   dispatch through).
    /// * Returns `None` when the inner is not a mixin — caller falls
    ///   back to the ordinary `&T` resolution.
    ///
    /// Returning a non-`None` value short-circuits the caller's
    /// `resolve_type_expr` on the inner, which is desired: a plain
    /// `Ty::TypeParam { bounds: [] }` (the current behaviour when a
    /// mixin name is used as a type) would silently drop the
    /// mixin identity at this position.
    fn try_resolve_dyn_mixin_ref(&mut self, inner: &ast::TypeExpr, ref_span: &Span) -> Option<Ty> {
        let path = match inner {
            ast::TypeExpr::Named(p) => p,
            _ => return None,
        };
        // Only bare single-segment names. Module-qualified mixin
        // references go through `resolve_type_path` and would need
        // separate plumbing; Phase A keeps the surface tight.
        if path.segments.len() != 1 {
            return None;
        }
        let name = &path.segments[0];
        let def_id = *self.type_registry.get(name)?;
        let def = self.symbols.get(def_id)?;
        let DefKind::Trait { info } = &def.kind else {
            return None;
        };
        match info.dispatch_mode {
            ast::DispatchMode::Runtime => {
                // Build a single-bound MixinRef. Phase A models the
                // dyn-shape via the existing `Ty::AnyMixin` so the
                // satisfaction machinery doesn't need a new branch.
                let generic_args = path
                    .generic_args
                    .as_ref()
                    .map(|args| {
                        args.iter()
                            .map(|a| self.resolve_type_expr(a))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                Some(Ty::AnyMixin(vec![MixinRef {
                    name: name.clone(),
                    generic_args,
                }]))
            }
            ast::DispatchMode::Static => {
                // Dedupe: forward-declaration registration and the
                // pass-2 walk both call `resolve_type_expr` on the
                // same parameter type, so without a per-span guard
                // the diagnostic would fire twice for one source
                // occurrence.
                let key = (ref_span.start, ref_span.end);
                if self.emitted_e1118_spans.insert(key) {
                    self.diagnostics.push(Diagnostic::error_with_code(
                        format!(
                            "`&{0}` references mixin `{0}`, which does not use runtime dispatch; \
                             add `dispatch runtime` to `mixin {0}` to enable `&{0}` / `&var {0}` \
                             parameter types",
                            name
                        ),
                        ref_span.clone(),
                        "E1118",
                    ));
                }
                Some(Ty::Error)
            }
        }
    }
}
