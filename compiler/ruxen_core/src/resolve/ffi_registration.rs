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

/// Caller-identity arguments threaded into Pass 1 type registration.
///
/// SINGLE ENTRY POINT context for
/// `Resolver::register_top_level_type_with_ffi_in`. Carries the two
/// behaviour-modifying flags that historically lived as side-channel
/// fields on `Resolver` (`merging_bootstrap`, `defer_class_lib_decls`)
/// and were flipped before/after a phase. Making them explicit
/// parameters means call sites declare their intent at the point of
/// invocation, eliminating internal `if self.merging_bootstrap`
/// branching druxen by hidden state.
///
/// Per
/// `docs/specs/system/compiler_consolidation.spec.md` §B3 stop
/// condition: the registration body MAY branch on these — namespace-
/// anchor mode for builtin type names is a real semantic difference —
/// but the branching is on EXPLICIT arguments rather than caller-
/// identity inferred from a resolver field.
#[derive(Copy, Clone, Debug)]
pub(super) struct RegistrationCtx {
    /// True iff this call is part of the bootstrap merge (stdlib
    /// `.rx` programs being folded into the prelude). When set, the
    /// `Class` arm enters "namespace-anchor mode" for already-known
    /// builtin type names (String, Option, Result) and the collection
    /// names (Array, Vec, Map, HashMap, Set, HashSet) — it reuses the
    /// existing DefId / hardcoded-arm dispatch instead of replacing
    /// the type-scope binding.
    pub merging_bootstrap: bool,
    /// True iff class-body lib decls should be skipped on this pass.
    /// The bootstrap merge sets this on the first walk so cross-class
    /// typed returns (`def lock_raw -> MutexGuard[T]` declared inside
    /// `class Mutex[T]` before MutexGuard is registered) can be
    /// resolved on a second walk after every class type is
    /// forward-declared. Always false on the user-program path.
    pub defer_class_lib_decls: bool,
}

impl RegistrationCtx {
    /// Context for the user-program Pass 1 walk: no bootstrap
    /// anchoring, no deferral.
    pub(super) const fn user_program() -> Self {
        Self {
            merging_bootstrap: false,
            defer_class_lib_decls: false,
        }
    }

    /// Context for the bootstrap merge's FIRST walk: anchor mode on,
    /// class-body lib decls deferred to the second walk.
    pub(super) const fn bootstrap_first_walk() -> Self {
        Self {
            merging_bootstrap: true,
            defer_class_lib_decls: true,
        }
    }
}

impl Resolver {
    pub(super) fn register_builtins(&mut self) {
        super::stdlib::register_all(self);
    }

    /// #06.8 Phase 3b: register a single FFI decl from inside a class
    /// (or mixin) body's `lib "X" ... end` block as a CLASS METHOD on
    /// that parent.
    ///
    /// The lib-block syntax is identical inside or outside a class — a
    /// plain `def NAME(params) -> Type` (no `self.` prefix, no body).
    /// What makes it a class method is the parent context: there is no
    /// implicit `self` on FFI calls (they bind to a verbatim C symbol),
    /// so `is_class_method` is always true and `self_mode` always None.
    /// Call sites spell `ClassName.method(...)` — the same surface as
    /// `def self.method(...)`.
    ///
    /// Pushes the `HirFfiFunc` onto `ffi_libs` keyed by the MANGLED
    /// `ClassName_method` so the MIR `ffi_alias_map` (which is keyed
    /// the same way `lower_method_call` builds the callee) can rewrite
    /// the call to the C symbol at lowering time.
    /// #06.93 Phase 3: when a class is nested inside one or more
    /// modules, the FFI alias map's mangled ruxen_name uses the
    /// qualified form (`Outer_Inner_method`) so the MIR call-site
    /// callee, which is also built from the qualified
    /// `Ty::Class { name: "Outer.Inner" }` (normalised dot → `_`),
    /// matches. Top-level classes (empty `module_path`) keep the
    /// existing `Class_method` shape. The C symbol stays whatever
    /// the user declared via `as "..."`.
    pub(super) fn register_class_lib_method_in(
        &mut self,
        parent: DefId,
        parent_name: &str,
        ffi_fn: &ast::FfiFunction,
        hir_fns: &mut Vec<HirFfiFunc>,
        module_path: &[String],
    ) {
        let param_tys: Vec<Ty> = ffi_fn
            .params
            .iter()
            .map(|p| self.resolve_type_expr(&p.type_expr))
            .collect();
        let params: Vec<ParamInfo> = ffi_fn
            .params
            .iter()
            .zip(param_tys.iter().cloned())
            .map(|(p, ty)| ParamInfo {
                name: p.name.clone(),
                ty,
                auto_assign: false,
                default: None,
            })
            .collect();
        let return_ty = ffi_fn
            .return_type
            .as_ref()
            .map(|t| self.resolve_type_expr(t))
            .unwrap_or(Ty::Unit);
        let return_ty_for_hir = if ffi_fn.return_type.is_some() {
            Some(return_ty.clone())
        } else {
            None
        };
        // The class-method vs instance-method distinction is carried on
        // the AST FfiFunction (set by the parser based on whether the
        // decl was `def self.NAME` or plain `def NAME`). Ruxen's
        // ruby-naming.spec.md §3.4a uses the same convention everywhere
        // — FFI decls are no exception. Instance-method FFI decls take
        // an implicit `self` receiver as their first arg to the C
        // symbol; class methods do not.
        // Receiver-element bound: a `where T: Bound` on a class FFI method
        // whose `T` is the ENCLOSING class's generic (e.g. `class Array[T]`'s
        // `def sum -> Int where T: Add`). Thread it into the signature as a
        // synthetic bounded generic param so the call-site seam
        // (`bridge_builtin_method`) can bind `{T → element}` and enforce it.
        // Only predicates naming an actual class generic are kept; FFI defs
        // have no own generics.
        let generic_params = self.ffi_receiver_element_bounds(parent, ffi_fn);
        let signature = FnSignature {
            self_mode: if ffi_fn.is_class_method {
                None
            } else {
                Some(crate::hir::nodes::HirSelfMode::Ref)
            },
            is_class_method: ffi_fn.is_class_method,
            is_async: false,
            generic_params,
            params,
            return_ty,
            c_symbol: ffi_fn.c_symbol.clone(),
        };
        let link_symbol = ffi_fn
            .c_symbol
            .clone()
            .unwrap_or_else(|| ffi_fn.name.clone());
        self.check_ffi_signature_conflict(&link_symbol, &signature, &ffi_fn.span);
        // #06.93 Phase 3: when `parent_name` is a class inside one or
        // more enclosing modules, prefix the mangled ruxen_name with
        // the underscored module path. The MIR call site builds the
        // same shape (`Outer_Inner_method`) by normalising the
        // dotted `Ty::Class { name: "Outer.Inner" }` at lowering
        // time. Top-level classes (empty module_path) keep the
        // existing `Class_method` shape.
        let mangled = if module_path.is_empty() {
            format!("{}_{}", parent_name, ffi_fn.name)
        } else {
            format!("{}_{}_{}", module_path.join("_"), parent_name, ffi_fn.name)
        };
        // Register under the PLAIN method name so MIR's
        // `is_user_static_method(class, method)` lookup — which scans
        // for a `DefKind::Method` whose `def.name == method_name` and
        // whose parent matches the class — finds this method.
        // Registering under the mangled name would hide it from that
        // scan and make method-call lowering prepend `self` (which is
        // wrong for class methods).
        let method_def_id = self.symbols.define(
            ffi_fn.name.clone(),
            DefKind::Method {
                parent,
                signature: signature.clone(),
            },
            Visibility::Public,
            ffi_fn.span.clone(),
        );
        self.extern_symbol_table
            .entry(link_symbol)
            .or_insert_with(|| (signature, ffi_fn.span.clone()));
        self.pass1_class_lib_methods
            .entry(parent)
            .or_default()
            .push(method_def_id);
        // Push onto ffi_libs with the MANGLED ruxen_name so the MIR
        // lowering's `ffi_alias_map` is keyed in the same shape that
        // `lower_method_call` builds the `MirInst::Call::callee` —
        // `format!("{}_{}", class_name, method_name)`. With the alias
        // map populated under that key, the existing Phase 2 rewrite
        // path picks up class-body FFI calls for free.
        // Instance methods receive `self` as the first arg at the C
        // ABI level — the MIR method-call lowering prepends the
        // receiver to arg_values for any non-static method, so the
        // FfiFuncDecl's `param_types` (which drives cranelift's
        // `Linkage::Import` signature) must include the class type
        // at index 0. Without this prepend the linker-side signature
        // would be off-by-one and cranelift would refuse the call
        // with "mismatched argument count".
        let final_param_tys = if ffi_fn.is_class_method {
            param_tys
        } else {
            // The receiver crosses the C ABI as the FIRST arg. Its WIRE
            // WIDTH must match what the C symbol expects. For most
            // method-homes the receiver is a pointer / boxed handle /
            // int64-by-value → a pointer-sized `Ty::Class { name }`
            // (I64), which is correct for `String`, the collections, and
            // the scalar homes whose C symbols take `int64_t`
            // (`Int`/`Bool`/`Char`/`USize`: `ruxen_int_to_string`,
            // `ruxen_bool_to_string(int64_t)`, `ruxen_char_to_string(
            // int64_t)`). The ONE exception is `Float`: its C symbols
            // take a `double` (`ruxen_float_to_string(double)`,
            // `ruxen_float_to_i(double)`), so the receiver must derive
            // F64 — prepend `Ty::Float`, not the I64 class handle. Using
            // the I64 class receiver there would pass the float bits in a
            // GP register and read garbage. The parity guard
            // (`tests/runtime_abi_derivation.rs`) pins each derived width.
            let receiver_ty = primitive_ffi_receiver_ty(parent_name);
            let mut tys = Vec::with_capacity(param_tys.len() + 1);
            tys.push(receiver_ty);
            tys.extend(param_tys);
            tys
        };
        hir_fns.push(HirFfiFunc {
            ruxen_name: mangled,
            c_symbol: ffi_fn.c_symbol.clone(),
            param_types: final_param_tys,
            return_type: return_ty_for_hir,
            is_variadic: ffi_fn.is_variadic,
            // The class/mixin name is encoded in the mangled ruxen_name
            // (`ClassName_method`) so any downstream consumer that wants
            // the parent type can split there. Setting it explicitly
            // here makes that intent visible.
            parent_type: Some(parent_name.to_string()),
        });
    }

    // (free helper below; see `final_param_tys` for the receiver-width
    // contract it implements.)

    /// #06.8 Phase 2: emit **E0722** when a Ruxen `lib`/`extern` block
    /// declares the same C symbol that an earlier block already
    /// declared with an incompatible signature. The first decl wins;
    /// subsequent matching decls are silently allowed (a redundant
    /// restatement is a no-op, not an error). The check is keyed on the
    /// LINKED symbol so two Ruxen names that alias the same C symbol
    /// must agree on its ABI — otherwise codegen would produce a
    /// mis-typed call.
    ///
    /// ## What "match" means: the WIRE shape, not the surface signature
    ///
    /// The conflict is decided on the POST-self-prepend WIRE shape — the
    /// list of ABI register widths the call actually crosses — rather
    /// than on surface-`Ty` equality. This is exactly what the linker
    /// sees: a C symbol carries no type info, so the only thing that can
    /// mis-compile is passing an argument in the wrong register or
    /// reading the return at the wrong width (see `docs/errors/E0722.md`
    /// §Background). Comparing surface `Ty` was a stricter
    /// over-approximation that rejected two BENIGN alias families whose
    /// wire shapes are identical:
    ///
    ///   * `String.from(s: &String) -> String` vs the receiver-style
    ///     `String.clone(&self) -> String`: the explicit `&String` param
    ///     and the implicit `&self` receiver are BOTH pointer-sized
    ///     (I64), so post-prepend both are `(I64) -> I64`.
    ///   * `Array.get(i) -> Option[&T]` vs
    ///     `Array.get_mut(i) -> Option[&var T]`: `&T` and `&var T` differ
    ///     only in surface mutability; both lower to a pointer, and the
    ///     boxed `Option` is one pointer either way — `(I64,I64) -> I64`
    ///     for both.
    ///
    /// The relaxation discards ONLY distinctions codegen itself discards.
    /// A genuine width mismatch — the E0722.md example `(I32) -> I32` vs
    /// `(I32) -> I64`, or an arity mismatch `(Int)` vs `(Int, Int)` — is
    /// still rejected, because `abi_wire_class` returns the same
    /// discriminant codegen's `ty_to_cranelift` uses. With this change
    /// `String.clone` / `Array.get_mut` / `Array.get_var` migrate to
    /// their `.rx` method-home (they no longer trip E0722); previously
    /// they were forced to stay as `lang_intrinsics` residual arms.
    /// Build the synthetic generic-param list for an FFI class method from
    /// its `where` clause: each predicate whose LHS names the ENCLOSING
    /// class's own generic becomes a bounded `GenericParamInfo`. This is
    /// the receiver-element bound seam (`def sum -> Int where T: Add` on
    /// `class Array[T]`). Predicates on non-class-generic names are dropped
    /// — an FFI def carries no own generics, so there is nothing else a
    /// `where` could constrain. Returns an empty vec when there is no
    /// `where` clause (the historical case), so every existing FFI decl is
    /// byte-identical (`generic_params: vec![]`).
    fn ffi_receiver_element_bounds(
        &mut self,
        parent: DefId,
        ffi_fn: &ast::FfiFunction,
    ) -> Vec<GenericParamInfo> {
        let Some(wc) = ffi_fn.where_clause.as_ref() else {
            return vec![];
        };
        // The enclosing class's declared generic-param names.
        let class_param_names: Vec<String> = self
            .symbols
            .get(parent)
            .map(|d| match &d.kind {
                DefKind::Class { info } => {
                    info.generic_params.iter().map(|g| g.name.clone()).collect()
                }
                DefKind::Struct { info } => {
                    info.generic_params.iter().map(|g| g.name.clone()).collect()
                }
                DefKind::Enum { info } => {
                    info.generic_params.iter().map(|g| g.name.clone()).collect()
                }
                _ => vec![],
            })
            .unwrap_or_default();
        let mut out = Vec::new();
        for pred in &wc.predicates {
            let ast::TypeExpr::Named(path) = &pred.type_expr else {
                continue;
            };
            if path.segments.len() != 1 || path.generic_args.is_some() {
                continue;
            }
            let name = &path.segments[0];
            if !class_param_names.contains(name) {
                continue;
            }
            let refs: Vec<MixinRef> = pred
                .bounds
                .iter()
                .map(|bound| MixinRef {
                    name: bound.path.segments.join("."),
                    generic_args: bound
                        .path
                        .generic_args
                        .as_ref()
                        .map(|args| args.iter().map(|a| self.resolve_type_expr(a)).collect())
                        .unwrap_or_default(),
                })
                .collect();
            out.push(GenericParamInfo::type_param(name.clone(), refs));
        }
        out
    }

    pub(super) fn check_ffi_signature_conflict(
        &mut self,
        link_symbol: &str,
        new_sig: &FnSignature,
        new_span: &Span,
    ) {
        if let Some((existing_sig, _existing_span)) = self.extern_symbol_table.get(link_symbol) {
            if Self::ffi_wire_shape(existing_sig) != Self::ffi_wire_shape(new_sig) {
                self.diagnostics.push(Diagnostic::error_with_code(
                    format!(
                        "conflicting FFI declarations for the same C symbol `{}` — \
                         the earlier declaration's signature does not match this one",
                        link_symbol
                    ),
                    new_span.clone(),
                    "E0722",
                ));
            }
        }
    }

    /// The ABI register-class discriminant of a single `Ty`, mirroring
    /// codegen's `ty_to_cranelift` (`codegen/cranelift/helpers.rs`)
    /// WITHOUT depending on the cranelift backend crate from the
    /// resolver. The discriminant is `(width_bits, is_float)`:
    ///   * scalars carry their true width; floats are flagged so an
    ///     integer and a float of the SAME width still conflict (they
    ///     use different ABI register files — GP vs SSE — so a call that
    ///     disagrees on int-vs-float would mis-pass the argument);
    ///   * every pointer-like / heap type (String, Array, Map, Set,
    ///     Ref/RefMut, Option, Result, Class, Struct, Enum, Fn, raw
    ///     pointers, Tuple, FixedArray, TypeParam, Infer, …) is the
    ///     integer-class 64-bit pointer `(64, false)`;
    ///   * `None` for `Unit`/`Never`/`Error` (no value in argument /
    ///     return position).
    /// `Ty::bit_width` is NOT reused: it returns `None` for pointer-like
    /// types, which would make a pointer indistinguishable from a
    /// no-value return.
    fn abi_wire_class(ty: &Ty) -> Option<(u32, bool)> {
        match ty {
            Ty::Bool | Ty::Int8 | Ty::UInt8 => Some((8, false)),
            Ty::Int16 | Ty::UInt16 => Some((16, false)),
            Ty::Int32 | Ty::UInt32 | Ty::Char => Some((32, false)),
            Ty::Int | Ty::Int64 | Ty::UInt | Ty::UInt64 | Ty::ISize | Ty::USize => {
                Some((64, false))
            }
            Ty::Float32 => Some((32, true)),
            Ty::Float | Ty::Float64 => Some((64, true)),
            Ty::Unit | Ty::Never | Ty::Error | Ty::ConstArg(_) => None,
            // Everything else is pointer-like at the C ABI → I64 (GP).
            _ => Some((64, false)),
        }
    }

    /// The full WIRE shape of an FFI signature: the ABI width-class of
    /// every C-ABI argument (the implicit `self` receiver pointer
    /// PREPENDED for instance methods, since it IS a wire arg) followed
    /// by the return width-class. Two decls aliasing one C symbol
    /// conflict iff their wire shapes differ. The receiver, when
    /// present, is always pointer-sized (`Some(64)`) regardless of the
    /// owning class.
    fn ffi_wire_shape(sig: &FnSignature) -> Vec<Option<(u32, bool)>> {
        let mut shape = Vec::with_capacity(sig.params.len() + 2);
        if sig.self_mode.is_some() {
            // Instance-method receiver: one pointer-sized GP arg the MIR
            // method-call lowering prepends (see the receiver-prepend in
            // `register_class_lib_method_in`).
            shape.push(Some((64, false)));
        }
        for p in &sig.params {
            shape.push(Self::abi_wire_class(&p.ty));
        }
        // A sentinel separates the arg list from the return so a
        // trailing arg can never collide with the return slot.
        shape.push(None);
        shape.push(Self::abi_wire_class(&sig.return_ty));
        shape
    }

    /// Typed FFI returns (docs/specs/types/typed_ffi_returns.spec.md):
    /// register all `lib "..."` FFI decls inside a class body, after
    /// pushing the class's generic params + a `Self` alias into a
    /// fresh class scope so structured return types (`-> Mutex[T]`,
    /// `-> MutexGuard[T]`, `-> T`, `-> Self`) resolve.
    pub(super) fn register_class_body_lib_decls(
        &mut self,
        class: &ast::ClassDef,
        id: DefId,
        ffi_libs: &mut Vec<HirFfiLib>,
        module_path: &[String],
    ) {
        if class.lib_decls.is_empty() {
            return;
        }
        self.scopes.push(ScopeKind::Class);
        let mut self_generic_args: Vec<Ty> = Vec::new();
        if let Some(gps) = class.generic_params.as_ref() {
            for p in &gps.params {
                if let ast::GenericParam::Type { name, bounds, span } = p {
                    let bound_refs: Vec<MixinRef> = bounds
                        .iter()
                        .map(|b| MixinRef {
                            name: b.path.segments.join("."),
                            generic_args: vec![],
                        })
                        .collect();
                    let gp_def = self.symbols.define(
                        name.clone(),
                        DefKind::TypeParam {
                            bounds: bound_refs.clone(),
                        },
                        Visibility::Private,
                        span.clone(),
                    );
                    self.scopes.insert_type(name.clone(), gp_def);
                    self_generic_args.push(Ty::TypeParam {
                        name: name.clone(),
                        bounds: bound_refs,
                    });
                }
            }
        }
        let self_ty = Ty::Class {
            name: class.name.clone(),
            generic_args: self_generic_args,
        };
        let self_def = self.symbols.define(
            "Self".to_string(),
            DefKind::TypeAlias { target: self_ty },
            Visibility::Private,
            class.span.clone(),
        );
        self.scopes.insert_type("Self".to_string(), self_def);

        let mut hir_fns: Vec<HirFfiFunc> = Vec::new();
        let mut link_flags: Vec<String> = Vec::new();
        for lib in &class.lib_decls {
            for flag in lib.link_attrs.iter().map(|a| format!("-l{}", a.name)) {
                if !link_flags.contains(&flag) {
                    link_flags.push(flag);
                }
            }
            for ffi_fn in &lib.functions {
                self.register_class_lib_method_in(
                    id,
                    &class.name,
                    ffi_fn,
                    &mut hir_fns,
                    module_path,
                );
            }
        }
        self.scopes.pop();
        if !hir_fns.is_empty() {
            ffi_libs.push(HirFfiLib {
                name: class.name.clone(),
                link_flags,
                functions: hir_fns,
            });
        }
    }

    /// Second-walk driver for typed FFI returns: recurse into
    /// `Module` items and call `register_class_body_lib_decls` for
    /// every `Class` whose lib decls were skipped on the first walk.
    /// The class's DefId is looked up from the type registry using
    /// the same qualified-key shape that `insert_type_qualified`
    /// produced during the first walk.
    pub(super) fn process_deferred_class_lib_decls(
        &mut self,
        item: &ast::TopLevelItem,
        ffi_libs: &mut Vec<HirFfiLib>,
        module_path: &[String],
    ) {
        match item {
            ast::TopLevelItem::Class(class) => {
                let lookup_key = if module_path.is_empty() {
                    class.name.clone()
                } else {
                    format!("{}.{}", module_path.join("."), class.name)
                };
                if let Some(&id) = self.type_registry.get(&lookup_key) {
                    self.register_class_body_lib_decls(class, id, ffi_libs, module_path);
                }
            }
            ast::TopLevelItem::Module(m) => {
                let mut nested = module_path.to_vec();
                nested.push(m.name.clone());
                for sub_item in &m.items {
                    self.process_deferred_class_lib_decls(sub_item, ffi_libs, &nested);
                }
            }
            _ => {}
        }
    }

    // ─── Pass 1: Forward Declaration of Types ───────────────────────

    /// #06.93 Phase 1 + Phase 4: register a type's name.
    ///
    /// At the TOP LEVEL (`module_path` empty), insert the
    /// un-qualified name into both the current type scope and the
    /// global `type_registry` (un-changed behaviour).
    ///
    /// INSIDE a module body (`module_path` non-empty), insert the
    /// un-qualified name into the current type scope ONLY — which,
    /// after Phase 4's module-scope push, is the module's own
    /// frame, not the global scope. This means `let x: Inner` at
    /// top level no longer resolves to a module-nested `Inner`.
    /// Insert the dotted qualified key (e.g. `Outer.Inner`) into
    /// the global `type_registry` so external code can still
    /// reference the type via the qualified path.
    fn insert_type_qualified(&mut self, name: &str, id: DefId, module_path: &[String]) {
        self.scopes.insert_type(name.to_string(), id);
        if module_path.is_empty() {
            self.type_registry.insert(name.to_string(), id);
        } else {
            let qualified = format!("{}.{}", module_path.join("."), name);
            self.type_registry.insert(qualified, id);
        }
    }

    /// Public entry into Pass 1 type registration. Top-level call
    /// sites in the bootstrap merge and the user-program driver pass
    /// through here with an empty module path; the internal
    /// `_in` variant carries the accumulated module-path stack for
    /// nested `module Outer { module Inner { ... } }` registration.
    /// SINGLE ENTRY POINT for Pass 1 type registration. Both call
    /// sites (the bootstrap merge and the user-program driver) pass
    /// through here with an empty module path; the internal `_in`
    /// variant carries the accumulated module-path stack for nested
    /// `module Outer { module Inner { ... } }` registration.
    ///
    /// `ctx` declares caller intent (bootstrap merge vs user program,
    /// deferred-lib-decls vs not). The body branches on `ctx`, NOT on
    /// a side-channel resolver field — see
    /// `docs/specs/system/compiler_consolidation.spec.md` §B3.
    pub(super) fn register_top_level_type_with_ffi(
        &mut self,
        item: &ast::TopLevelItem,
        ffi_libs: &mut Vec<HirFfiLib>,
        ctx: RegistrationCtx,
    ) {
        self.register_top_level_type_with_ffi_in(item, ffi_libs, &[], ctx);
    }

    pub(super) fn register_top_level_type_with_ffi_in(
        &mut self,
        item: &ast::TopLevelItem,
        ffi_libs: &mut Vec<HirFfiLib>,
        module_path: &[String],
        ctx: RegistrationCtx,
    ) {
        let _span_zero = Span {
            start: 0,
            end: 0,
            line: 0,
            column: 0,
        };

        match item {
            ast::TopLevelItem::Class(class) => {
                // T2.02 S5: pre-populate generic_params with kinds so
                // use-site E0700 checks during pass 1 (e.g. inside a
                // forward-declared fn signature that references this
                // class) see the right param kinds.  Without this,
                // const params would still register as `Type` kind.
                let class_gp = self.collect_generic_param_infos(&class.generic_params);
                // #06.8 T0c: capture the `layout flat_heap_struct`
                // marker at forward-declaration time so any pre-pass
                // user (e.g. forward-referenced fn sigs) sees it.
                let flat_heap_struct = class.layout.iter().any(|s| s == "flat_heap_struct");

                // #06.8 T#21: namespace-anchor mode. When the bootstrap
                // is merging a `class Foo` whose name already has a
                // type-scope DefId (e.g. `String` → TypeAlias to
                // Ty::String, `Option`/`Result` → Enum), DO NOT
                // replace the binding. Instead reuse the existing
                // DefId as the parent for class-body `lib` decls.
                // This is the only way to attach FFI methods to a
                // builtin type without changing its `Ty` representation
                // for the whole compilation — see the field doc for
                // `merging_bootstrap` for the catastrophic-failure
                // mode this guards against.
                //
                // #06.8 T#14: extend anchor mode to also cover the
                // collection-type names (`Array`/`Vec`/`Map`/`HashMap`/
                // `Set`/`HashSet`) which are NOT registered in the
                // type scope by `register_builtins` — their type
                // resolution comes from a hardcoded `match name`
                // arm in `resolve_type_expr` that builds
                // `Ty::Array(_)` / `Ty::Map(_,_)` / `Ty::Set(_)`
                // directly. Inserting a fresh `DefKind::Class` for
                // those names would mean `let x: Array[Int]`
                // resolves `Array` via type-scope BEFORE the
                // hardcoded arm runs, producing `Ty::Class { name:
                // "Array", ... }` instead of `Ty::Array(Int)` and
                // breaking the entire collection ABI. The single
                // source of truth for this set is
                // `resolve::types::COLLECTION_BUILTINS`, shared so this
                // membership test and the `resolve_type_expr` arms
                // cannot drift.
                let is_anchor_only_builtin =
                    crate::resolve::types::COLLECTION_BUILTINS.contains(&class.name.as_str());
                // Phase E.E of #06.95: when the class is declared
                // INSIDE a module (`module BufReader { class File }`),
                // anchor-mode must look up the QUALIFIED name in
                // `type_registry`, not the unqualified name in scope.
                // Without this, a `class File` nested in `module
                // BufReader` collides with the unrelated top-level
                // `class File` in io.rx and gets anchored onto the
                // wrong DefId — `type_registry` then never receives
                // `"BufReader.File"`, breaking every `BufReader.File`
                // reference downstream. For top-level classes
                // (`module_path` empty), the historical unqualified
                // scope lookup is preserved.
                let anchor_id: Option<DefId> = if ctx.merging_bootstrap {
                    if module_path.is_empty() {
                        self.scopes.lookup_type(&class.name)
                    } else {
                        let qualified = format!("{}.{}", module_path.join("."), class.name);
                        self.type_registry.get(&qualified).copied()
                    }
                } else {
                    None
                };

                // Q14: a USER (non-bootstrap) top-level class whose name is
                // already a built-in / stdlib type in scope collides in the
                // flat symbol namespace — both would emit the same mangled
                // symbols (e.g. `Signal_clone`). Flag it now with a rename
                // hint (E0727) instead of a late codegen `DuplicateDefinition`.
                if !ctx.merging_bootstrap
                    && module_path.is_empty()
                    && anchor_id.is_none()
                    && !is_anchor_only_builtin
                    && self.scopes.lookup_type(&class.name).is_some()
                {
                    self.diagnostics.push(Diagnostic::error_with_code(
                        format!(
                            "type `{}` collides with a built-in or standard-library type of \
                             the same name. The symbol namespace is currently flat, so both \
                             would emit the same mangled symbols (e.g. `{}_clone`). Rename \
                             your type.",
                            class.name, class.name
                        ),
                        class.span.clone(),
                        "E0727",
                    ));
                }

                let id = if let Some(existing) = anchor_id {
                    existing
                } else if ctx.merging_bootstrap && is_anchor_only_builtin {
                    // Create a parent DefId for `pass1_class_lib_methods`
                    // / `ffi_libs` bookkeeping but DO NOT insert into
                    // type-scope. The hardcoded arm in `resolve_type_expr`
                    // remains authoritative for the type representation.
                    //
                    // BUT: still register in `type_registry` so the
                    // bootstrap's deferred lib-decl walk
                    // (`process_deferred_class_lib_decls`) can resolve
                    // this DefId by class name. Without that entry the
                    // second walk's `type_registry.get(class.name)`
                    // misses, the class's own `lib` block never gets
                    // processed, and `Array_new` / `Array_push` /
                    // `Map_insert` / `Set_contains` / etc. all stay
                    // unregistered in `ffi_alias_map` — every call site
                    // emits the bare mangled name and the linker fails
                    // with `_Array[Int]_push undefined`. `type_registry`
                    // is a separate map from `scopes.insert_type`
                    // (which is what would actually re-anchor the
                    // type-scope arm and break `let x: Array[Int]`);
                    // we keep `scopes.insert_type` off the path here.
                    let new_id = self.symbols.define(
                        class.name.clone(),
                        DefKind::Class {
                            info: ClassInfo {
                                generic_params: class_gp,
                                parent: None,
                                fields: vec![],
                                methods: vec![],
                                derive_traits: class.derive_traits.clone(),
                                opt_out_send: false,
                                opt_out_sync: false,
                                manual_send: false,
                                manual_sync: false,
                                const_predicates: vec![],
                                flat_heap_struct,
                                runtime_dispatch_includes: Vec::new(),
                            },
                        },
                        Visibility::Public,
                        class.span.clone(),
                    );
                    // Top-level anchor-only builtins: register the bare
                    // class name in `type_registry`. The list is closed
                    // (Array/Vec/Map/HashMap/Set/HashSet — none of
                    // which can appear nested inside a `module` block),
                    // so the module-path branch in
                    // `insert_type_qualified` doesn't apply.
                    self.type_registry.insert(class.name.clone(), new_id);
                    new_id
                } else {
                    let new_id = self.symbols.define(
                        class.name.clone(),
                        DefKind::Class {
                            info: ClassInfo {
                                generic_params: class_gp,
                                parent: None,
                                fields: vec![],
                                methods: vec![],
                                derive_traits: class.derive_traits.clone(),
                                opt_out_send: false,
                                opt_out_sync: false,
                                manual_send: false,
                                manual_sync: false,
                                const_predicates: vec![],
                                flat_heap_struct,
                                runtime_dispatch_includes: Vec::new(),
                            },
                        },
                        Visibility::Public,
                        class.span.clone(),
                    );
                    self.insert_type_qualified(&class.name, new_id, module_path);
                    new_id
                };

                // #06.8 Phase 3b: register class-body `lib` FFI decls as
                // class methods on this class. The lib-block syntax is
                // identical inside or outside a class; the parent
                // context is what flips `is_class_method` to true and
                // routes calls through `ClassName.method(...)`.
                //
                // Typed FFI returns (docs/specs/types/typed_ffi_returns.spec.md):
                // when `defer_class_lib_decls` is set, the bootstrap
                // merge has chosen to run lib-decl processing in a
                // second walk after every class TYPE has been
                // forward-declared. This lets a class's lib decl
                // reference sibling classes declared later in the
                // same file (`def lock_raw -> MutexGuard[T]` inside
                // `class Mutex[T]` where `MutexGuard` follows).
                if !ctx.defer_class_lib_decls {
                    self.register_class_body_lib_decls(class, id, ffi_libs, module_path);
                }

                // #06.95 Phase A pre-flight: for every `include Mixin`
                // in this class's body, re-register the mixin's
                // lib_decls under THIS class's name. The mixin's own
                // registration (in the Mixin arm below) keys
                // `Mixin_method → c-symbol` in ffi_alias_map. The MIR
                // call site for `ThisClass.method(...)` builds the
                // callee as `ThisClass_method`, so without a parallel
                // entry the FFI alias rewrite misses and codegen
                // emits a call to the unmangled name — link error
                // at codegen time.
                //
                // The pre-pass at resolve_with_bootstrap start
                // populated `self.mixin_lib_decls` with every mixin
                // in scope (user + bootstrap), so this lookup is
                // O(1) regardless of source order.
                let included_lib_fns: Vec<ast::FfiFunction> = class
                    .inner_impls
                    .iter()
                    .flat_map(|inner| {
                        // #06.93 Phase 5: resolve the included mixin's
                        // lookup key. The user wrote some path
                        // (`Reader` or `M.Reader`); we prefer the
                        // class's enclosing-module-qualified form,
                        // falling back to the literal path. Order:
                        //   1. If the user wrote a multi-segment
                        //      path (`M.Reader`), use it as-is.
                        //   2. Else if the class is inside a module,
                        //      try `<class.module_path>.<written>`
                        //      first (the same-module case — common
                        //      for the module + mixin pattern).
                        //   3. Else fall back to the literal name
                        //      (top-level mixin case).
                        let written = inner.trait_name.segments.join(".");
                        let try_keys: Vec<String> = if inner.trait_name.segments.len() > 1 {
                            vec![written.clone()]
                        } else if !module_path.is_empty() {
                            vec![
                                format!("{}.{}", module_path.join("."), written),
                                written.clone(),
                            ]
                        } else {
                            vec![written.clone()]
                        };
                        try_keys
                            .into_iter()
                            .find_map(|k| self.mixin_lib_decls.get(&k).cloned())
                            .into_iter()
                            .flat_map(|libs| {
                                libs.into_iter().flat_map(|lib| lib.functions.into_iter())
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect();
                if !included_lib_fns.is_empty() {
                    let mut hir_fns: Vec<HirFfiFunc> = Vec::new();
                    for ffi_fn in &included_lib_fns {
                        self.register_class_lib_method_in(
                            id,
                            &class.name,
                            ffi_fn,
                            &mut hir_fns,
                            module_path,
                        );
                    }
                    if !hir_fns.is_empty() {
                        ffi_libs.push(HirFfiLib {
                            name: class.name.clone(),
                            link_flags: Vec::new(),
                            functions: hir_fns,
                        });
                    }
                }
            }
            ast::TopLevelItem::Struct(s) => {
                let struct_gp = self.collect_generic_param_infos(&s.generic_params);
                let id = self.symbols.define(
                    s.name.clone(),
                    DefKind::Struct {
                        info: StructInfo {
                            generic_params: struct_gp,
                            fields: vec![],
                            derive_traits: s.derive_traits.clone(),
                            layout: s.layout.clone(),
                            opt_out_send: false,
                            opt_out_sync: false,
                            manual_send: false,
                            manual_sync: false,
                            const_predicates: vec![],
                        },
                    },
                    Visibility::Public,
                    s.span.clone(),
                );
                self.insert_type_qualified(&s.name, id, module_path);
            }
            ast::TopLevelItem::Enum(e) => {
                // #06.8 T0c: duplicate `layout tagged` enum names in
                // the same scope are E0723. Detection happens here at
                // forward-declaration time so the diagnostic lands on
                // the second declaration's span (the first remains the
                // accepted one — matching the "tags are append-only"
                // invariant). The tracker is a flat HashMap keyed by
                // name, which matches the current top-level-only
                // scoping; nested-module semantics are deferred.
                if e.layout.iter().any(|s| s == "tagged") {
                    if let Some(_first_span) = self.tagged_enums_in_scope.get(&e.name).cloned() {
                        self.diagnostics.push(Diagnostic::error_with_code(
                            format!("duplicate `layout tagged` enum `{}` in scope", e.name),
                            e.span.clone(),
                            "E0723",
                        ));
                    } else {
                        self.tagged_enums_in_scope
                            .insert(e.name.clone(), e.span.clone());
                    }
                }
                let enum_gp = self.collect_generic_param_infos(&e.generic_params);
                let id = self.symbols.define(
                    e.name.clone(),
                    DefKind::Enum {
                        info: EnumInfo {
                            generic_params: enum_gp,
                            variants: vec![],
                            derive_traits: e.derive_traits.clone(),
                            opt_out_send: false,
                            opt_out_sync: false,
                            manual_send: false,
                            manual_sync: false,
                            const_predicates: vec![],
                        },
                    },
                    Visibility::Public,
                    e.span.clone(),
                );
                self.insert_type_qualified(&e.name, id, module_path);

                // Push a scope for the enum's own generic params so that
                // variant field types (e.g. `Some(T)` in
                // `enum MyOpt[T] { Some(T), None }`) can resolve `T` to
                // a `TypeParam` rather than `Error` during this pre-pass.
                let enum_generic_names: Vec<(String, Vec<MixinRef>, Span)> = e
                    .generic_params
                    .as_ref()
                    .map(|gps| {
                        gps.params
                            .iter()
                            .filter_map(|p| match p {
                                ast::GenericParam::Type { name, bounds, span } => {
                                    let trait_refs: Vec<MixinRef> = bounds
                                        .iter()
                                        .map(|b| MixinRef {
                                            name: b.path.segments.join("."),
                                            generic_args: vec![],
                                        })
                                        .collect();
                                    Some((name.clone(), trait_refs, span.clone()))
                                }
                                ast::GenericParam::Lifetime { .. } => None,
                                // Stage 3 of const generics: const
                                // params are registered separately
                                // below as `DefKind::ConstParam`
                                // (not type params), so this filter
                                // (which collects type-generic names
                                // for the enum's HIR) skips them.
                                ast::GenericParam::Const { .. } => None,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let has_generics = !enum_generic_names.is_empty();
                if has_generics {
                    self.scopes.push(ScopeKind::Class);
                    for (name, bounds, span) in &enum_generic_names {
                        let gp_def = self.symbols.define(
                            name.clone(),
                            DefKind::TypeParam {
                                bounds: bounds.clone(),
                            },
                            Visibility::Private,
                            span.clone(),
                        );
                        self.scopes.insert_type(name.clone(), gp_def);
                    }
                }

                // Also register each variant for resolution. Collect the
                // resolved info while the generic-param scope is active
                // (so `T` resolves), then register the composite
                // `Type.Variant` lookup entries after popping the scope
                // so they live on the outer top-level scope where callers
                // look them up.
                let mut pending_registrations: Vec<(String, DefId)> = Vec::new();
                for (idx, variant) in e.variants.iter().enumerate() {
                    let vkind = match &variant.fields {
                        ast::VariantKind::Unit => VariantDefKind::Unit,
                        ast::VariantKind::Tuple(fields) => VariantDefKind::Tuple(
                            fields
                                .iter()
                                .map(|f| self.resolve_type_expr(&f.type_expr))
                                .collect(),
                        ),
                        ast::VariantKind::Struct(fields) => VariantDefKind::Struct(
                            fields
                                .iter()
                                .map(|f| {
                                    (
                                        f.name.clone().unwrap_or_default(),
                                        self.resolve_type_expr(&f.type_expr),
                                    )
                                })
                                .collect(),
                        ),
                    };
                    let vid = self.symbols.define(
                        variant.name.clone(),
                        DefKind::EnumVariant {
                            parent: id,
                            variant_idx: idx,
                            kind: vkind,
                        },
                        Visibility::Public,
                        variant.span.clone(),
                    );
                    pending_registrations.push((format!("{}.{}", e.name, variant.name), vid));
                }

                if has_generics {
                    self.scopes.pop();
                }

                // Async sub-phase 1 (docs/specs/stdlib/async.spec.md B2):
                // bootstrap-loaded generic enums like `Poll[T]` need their
                // EnumInfo.variants populated so typeck's
                // `infer_user_enum_generic_args` (infer.rs ~ line 2064)
                // can walk the variant payload tys and bind `T -> Int`
                // for `Poll.Ready(0)`. The Rust-registered Option/Result
                // historically did this via direct `info.variants =
                // vec![...]` assignments in resolve/stdlib/mod.rs; for
                // every other bootstrap-loaded enum that lifted earlier
                // (IoError, IoErrorKind, SeekFrom, Shutdown) the variants
                // happen to be non-generic, so the empty-`variants` gap
                // went unnoticed. Poll is the first generic enum loaded
                // through this path.
                let variant_ids: Vec<DefId> =
                    pending_registrations.iter().map(|(_, vid)| *vid).collect();
                if let Some(enum_def) = self.symbols.get_mut(id) {
                    if let DefKind::Enum { ref mut info } = enum_def.kind {
                        info.variants = variant_ids;
                    }
                }

                // Register Type.Variant lookup entries on the outer scope.
                for (key, vid) in pending_registrations {
                    self.scopes.insert(key, vid);
                }
            }
            ast::TopLevelItem::Mixin(t) => {
                let mut required = vec![];
                let mut defaults = vec![];
                let mut assoc = vec![];
                let mut mixin_aliases: Vec<ast::AliasDef> = vec![];
                for ti in &t.items {
                    match ti {
                        ast::MixinItem::MethodSig(sig) => required.push(sig.name.clone()),
                        ast::MixinItem::DefaultMethod(f) => defaults.push(f.name.clone()),
                        ast::MixinItem::AssocType { name, .. } => assoc.push(name.clone()),
                        // Ruby `alias new old` inside a mixin body
                        // (docs/decisions/alias-keyword.md). Collected here and
                        // recorded against the mixin's method surface below.
                        ast::MixinItem::Alias(a) => mixin_aliases.push(a.clone()),
                    }
                }
                // Record mixin aliases as synonyms over the mixin's own method
                // surface (required + default method names). Implementors that
                // include the mixin inherit both names.
                if !mixin_aliases.is_empty() {
                    let mut method_names = required.clone();
                    method_names.extend(defaults.iter().cloned());
                    self.record_method_aliases(&t.name, &mixin_aliases, &method_names);
                }

                let id = self.symbols.define(
                    t.name.clone(),
                    DefKind::Trait {
                        info: MixinInfo {
                            generic_params: vec![],
                            super_traits: t
                                .super_traits
                                .iter()
                                .map(|b| MixinRef {
                                    name: b.path.segments.join("."),
                                    generic_args: vec![],
                                })
                                .collect(),
                            required_methods: required,
                            default_methods: defaults,
                            assoc_types: assoc,
                            dispatch_mode: t.dispatch_mode,
                        },
                    },
                    Visibility::Public,
                    t.span.clone(),
                );
                self.insert_type_qualified(&t.name, id, module_path);

                // #06.8 Phase 3b: register mixin-body `lib` FFI decls as
                // class methods on the mixin (parallel to class-body lib
                // handling above). Same semantics: no implicit `self`,
                // `ClassName.method(...)` call surface.
                if !t.lib_decls.is_empty() {
                    let mut hir_fns: Vec<HirFfiFunc> = Vec::new();
                    let mut link_flags: Vec<String> = Vec::new();
                    for lib in &t.lib_decls {
                        for flag in lib.link_attrs.iter().map(|a| format!("-l{}", a.name)) {
                            if !link_flags.contains(&flag) {
                                link_flags.push(flag);
                            }
                        }
                        for ffi_fn in &lib.functions {
                            self.register_class_lib_method_in(
                                id,
                                &t.name,
                                ffi_fn,
                                &mut hir_fns,
                                module_path,
                            );
                        }
                    }
                    if !hir_fns.is_empty() {
                        ffi_libs.push(HirFfiLib {
                            name: t.name.clone(),
                            link_flags,
                            functions: hir_fns,
                        });
                    }
                }
            }
            ast::TopLevelItem::TypeAlias(ta) => {
                let target = self.resolve_type_expr(&ta.type_expr);
                let id = self.symbols.define(
                    ta.name.clone(),
                    DefKind::TypeAlias { target },
                    Visibility::Public,
                    ta.span.clone(),
                );
                self.insert_type_qualified(&ta.name, id, module_path);
            }
            ast::TopLevelItem::Newtype(nt) => {
                let inner = self.resolve_type_expr(&nt.inner_type);
                let id = self.symbols.define(
                    nt.name.clone(),
                    DefKind::Newtype { inner },
                    Visibility::Public,
                    nt.span.clone(),
                );
                self.insert_type_qualified(&nt.name, id, module_path);
            }
            ast::TopLevelItem::Module(m) => {
                // Register module type name in the OUTER scope (so
                // `module M { ... }` is reachable from siblings),
                // then push a fresh scope frame for the module's
                // body and recurse. Types declared inside the
                // module land in that frame, NOT the global scope —
                // implementing inner-first shadowing (#06.93 Phase
                // 4). The qualified `type_registry` entry from
                // Phase 1 is still added by `insert_type_qualified`
                // so external code (`use M.Foo`, `let x: M.Foo`)
                // resolves via the global registry.
                let id = self.symbols.define(
                    m.name.clone(),
                    DefKind::Module { items: vec![] },
                    Visibility::Public,
                    m.span.clone(),
                );
                self.insert_type_qualified(&m.name, id, module_path);

                let mut nested_path: Vec<String> = module_path.to_vec();
                nested_path.push(m.name.clone());
                // Phase 4: push a module-body scope. The recursive
                // `insert_type_qualified` calls below will insert
                // their un-qualified names into THIS frame, not the
                // global scope. From outside this module body, those
                // un-qualified names are invisible; only the
                // qualified `M.Inner` (in type_registry) remains
                // reachable.
                self.scopes.push(ScopeKind::Module);
                for sub_item in &m.items {
                    self.register_top_level_type_with_ffi_in(sub_item, ffi_libs, &nested_path, ctx);
                }
                self.scopes.pop();
            }
            ast::TopLevelItem::Function(f) => {
                // Forward-declare top-level functions so they can be referenced
                // before their definition (e.g. parse_priority called from impl body).
                // Push a temporary scope for generic params
                self.scopes.push(ScopeKind::Function);
                let generic_params = self.resolve_generic_params(&f.generic_params);
                for gp in &generic_params {
                    let gp_def = self.symbols.define(
                        gp.name.clone(),
                        DefKind::TypeParam {
                            bounds: gp.bounds.clone(),
                        },
                        Visibility::Private,
                        gp.span.clone(),
                    );
                    self.scopes.insert_type(gp.name.clone(), gp_def);
                }
                let return_ty = f
                    .return_type
                    .as_ref()
                    .map(|t| self.resolve_type_expr(t))
                    .unwrap_or_else(|| {
                        if f.name == "main" {
                            Ty::Unit
                        } else {
                            self.type_context.fresh_type_var()
                        }
                    });
                let params: Vec<ParamInfo> = f
                    .params
                    .iter()
                    .map(|p| {
                        let ty = self.resolve_type_expr(&p.type_expr);
                        ParamInfo {
                            name: p.name.clone(),
                            ty,
                            auto_assign: p.auto_assign,
                            default: p.default.as_deref().cloned(),
                        }
                    })
                    .collect();
                self.scopes.pop();
                let fn_generic_param_infos = self.collect_generic_param_infos(&f.generic_params);
                let id = self.symbols.define(
                    f.name.clone(),
                    DefKind::Function {
                        signature: FnSignature {
                            self_mode: None,
                            is_class_method: false,
                            is_async: f.is_async,
                            generic_params: fn_generic_param_infos,
                            params,
                            return_ty,
                            c_symbol: None,
                        },
                    },
                    Visibility::Public,
                    f.span.clone(),
                );
                self.scopes.insert(f.name.clone(), id);
            }
            ast::TopLevelItem::Lib(lib) => {
                let mut hir_fns: Vec<HirFfiFunc> = Vec::with_capacity(lib.functions.len());
                for ffi_fn in &lib.functions {
                    let param_tys: Vec<Ty> = ffi_fn
                        .params
                        .iter()
                        .map(|p| self.resolve_type_expr(&p.type_expr))
                        .collect();
                    let params: Vec<ParamInfo> = ffi_fn
                        .params
                        .iter()
                        .zip(param_tys.iter().cloned())
                        .map(|(p, ty)| ParamInfo {
                            name: p.name.clone(),
                            ty,
                            auto_assign: false,
                            default: None,
                        })
                        .collect();
                    let return_ty = ffi_fn
                        .return_type
                        .as_ref()
                        .map(|t| self.resolve_type_expr(t))
                        .unwrap_or(Ty::Unit);
                    let return_ty_for_hir = if ffi_fn.return_type.is_some() {
                        Some(return_ty.clone())
                    } else {
                        None
                    };
                    let signature = FnSignature {
                        self_mode: None,
                        is_class_method: false,
                        is_async: false,
                        generic_params: vec![],
                        params,
                        return_ty,
                        c_symbol: ffi_fn.c_symbol.clone(),
                    };
                    // #06.8 Phase 2: E0722 cross-decl conflict check. Keyed
                    // on the LINKED C symbol (alias if present, Ruxen name
                    // otherwise) so two decls that route to the same linker
                    // symbol with incompatible signatures are caught before
                    // codegen produces a mis-typed call.
                    let link_symbol = ffi_fn
                        .c_symbol
                        .clone()
                        .unwrap_or_else(|| ffi_fn.name.clone());
                    self.check_ffi_signature_conflict(&link_symbol, &signature, &ffi_fn.span);
                    let id = self.symbols.define(
                        ffi_fn.name.clone(),
                        DefKind::Function {
                            signature: signature.clone(),
                        },
                        Visibility::Public,
                        ffi_fn.span.clone(),
                    );
                    self.extern_symbol_table
                        .entry(link_symbol)
                        .or_insert_with(|| (signature, ffi_fn.span.clone()));
                    self.scopes.insert(ffi_fn.name.clone(), id);
                    hir_fns.push(HirFfiFunc {
                        ruxen_name: ffi_fn.name.clone(),
                        c_symbol: ffi_fn.c_symbol.clone(),
                        parent_type: None,
                        param_types: param_tys,
                        return_type: return_ty_for_hir,
                        is_variadic: ffi_fn.is_variadic,
                    });
                }
                ffi_libs.push(HirFfiLib {
                    name: lib.name.clone(),
                    link_flags: lib
                        .link_attrs
                        .iter()
                        .map(|a| format!("-l{}", a.name))
                        .collect(),
                    functions: hir_fns,
                });
            }
            ast::TopLevelItem::Extern(ext) => {
                let mut hir_fns: Vec<HirFfiFunc> = Vec::with_capacity(ext.functions.len());
                for ffi_fn in &ext.functions {
                    let param_tys: Vec<Ty> = ffi_fn
                        .params
                        .iter()
                        .map(|p| self.resolve_type_expr(&p.type_expr))
                        .collect();
                    let params: Vec<ParamInfo> = ffi_fn
                        .params
                        .iter()
                        .zip(param_tys.iter().cloned())
                        .map(|(p, ty)| ParamInfo {
                            name: p.name.clone(),
                            ty,
                            auto_assign: false,
                            default: None,
                        })
                        .collect();
                    let return_ty = ffi_fn
                        .return_type
                        .as_ref()
                        .map(|t| self.resolve_type_expr(t))
                        .unwrap_or(Ty::Unit);
                    let return_ty_for_hir = if ffi_fn.return_type.is_some() {
                        Some(return_ty.clone())
                    } else {
                        None
                    };
                    let signature = FnSignature {
                        self_mode: None,
                        is_class_method: false,
                        is_async: false,
                        generic_params: vec![],
                        params,
                        return_ty,
                        c_symbol: ffi_fn.c_symbol.clone(),
                    };
                    let link_symbol = ffi_fn
                        .c_symbol
                        .clone()
                        .unwrap_or_else(|| ffi_fn.name.clone());
                    self.check_ffi_signature_conflict(&link_symbol, &signature, &ffi_fn.span);
                    let id = self.symbols.define(
                        ffi_fn.name.clone(),
                        DefKind::Function {
                            signature: signature.clone(),
                        },
                        Visibility::Public,
                        ffi_fn.span.clone(),
                    );
                    self.extern_symbol_table
                        .entry(link_symbol)
                        .or_insert_with(|| (signature, ffi_fn.span.clone()));
                    self.scopes.insert(ffi_fn.name.clone(), id);
                    hir_fns.push(HirFfiFunc {
                        ruxen_name: ffi_fn.name.clone(),
                        c_symbol: ffi_fn.c_symbol.clone(),
                        parent_type: None,
                        param_types: param_tys,
                        return_type: return_ty_for_hir,
                        is_variadic: ffi_fn.is_variadic,
                    });
                }
                ffi_libs.push(HirFfiLib {
                    name: ext.abi.clone(),
                    link_flags: vec![],
                    functions: hir_fns,
                });
            }
            _ => {
                // Use, Const — resolved in pass 2
            }
        }
    }

    // ─── Pass 2: Full Resolution ────────────────────────────────────
}

/// The `Ty` to prepend as the implicit `self` receiver of an
/// instance-method FFI decl, chosen so its `ty_to_cranelift` width
/// matches the C symbol's first parameter.
///
/// Default: `Ty::Class { name }` — pointer-sized I64. Correct for every
/// pointer / boxed-handle / `int64_t`-by-value receiver, which is all of
/// `String`, the collections (`Array`/`Set`/`Hash`), the enum homes
/// (`Option`/`Result`), and the scalar homes whose C symbols take
/// `int64_t` (`Int`/`Bool`/`Char`/`USize` → `ruxen_int_to_string`,
/// `ruxen_bool_to_string(int64_t)`, `ruxen_char_to_string(int64_t)`).
///
/// The ONE primitive whose C symbols take the value in a FLOAT register
/// is `Float` (`ruxen_float_to_string(double)` / `ruxen_float_to_i(
/// double)`): its receiver must derive F64, so prepend `Ty::Float`.
/// Without this, the I64 class handle would pass the double's bits in a
/// GP register and the C side would read garbage. The parity guard
/// `tests/runtime_abi_derivation.rs` pins the derived width for every
/// shared symbol, so a wrong receiver here fails the guard immediately.
fn primitive_ffi_receiver_ty(parent_name: &str) -> Ty {
    match parent_name {
        "Float" => Ty::Float,
        _ => Ty::Class {
            name: parent_name.to_string(),
            generic_args: vec![],
        },
    }
}
