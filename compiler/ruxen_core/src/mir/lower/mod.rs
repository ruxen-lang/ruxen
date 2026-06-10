//! HIR-to-MIR lowering.
//!
//! Walks the typed HIR and builds MIR functions with explicit control-flow
//! graphs. Each HIR expression becomes one or more MIR instructions within
//! basic blocks connected by terminators.

use std::collections::{HashMap, HashSet};

use crate::hir::nodes::*;
use crate::hir::types::{MoveSemantics, Ty};
use crate::mir::nodes::*;
use crate::parser::ast::{BinOp, UnaryOp};
use crate::resolve::symbols::{ty_is_effectively_copy, SymbolTable};

mod captures;
mod closure_inline;
mod collect;
mod derive;
mod drops;
mod emit;
mod expr;
mod function;
mod impl_block;
mod interpolation;
mod match_arms;
mod monomorphize;
pub mod runtime_abi;
mod statement;
mod trait_default;
mod type_helpers;
mod util;
use captures::*;
use drops::*;
use trait_default::*;
use type_helpers::*;
pub use type_helpers::{def_id_name, type_name_from_ty};
use util::*;

// ─── Lowerer ────────────────────────────────────────────────────────────────

/// Walks typed HIR and produces MIR functions with explicit CFGs.
pub struct Lowerer<'a> {
    symbols: &'a SymbolTable,
    /// Maps HIR DefIds (variables, params) to MIR LocalIds within the
    /// current function being lowered.
    def_to_local: HashMap<DefId, LocalId>,
    /// The function currently being built.
    current_fn: Option<MirFunction>,
    /// The block we are currently emitting into.
    current_block: BlockId,
    /// Counter for generating unique closure function names.
    closure_counter: u32,
    /// Closure functions generated during lowering (added to the program).
    pending_closures: Vec<MirFunction>,
    /// Map from trait name → list of concrete type names that impl it.
    /// Populated at the start of `lower_program`. Used for method dispatch
    /// on generic parameters with a single-trait bound: when the only
    /// implementor is unambiguous, the call is lowered to that impl.
    trait_impls: HashMap<String, Vec<String>>,
    /// Stack of active loops. The innermost loop is the last element.
    /// `continue_target` is the block `continue` jumps to; `break_target`
    /// is the block `break` jumps to; `result_local` is the local that
    /// `break <value>` should assign its value into before jumping to
    /// the break target (None if the loop expression is Unit-typed).
    loop_stack: Vec<LoopFrame>,
    /// DefIds of `let mut` variables that are mutably captured by some
    /// non-move closure in the current function. These are promoted to
    /// heap cells (8-byte allocations) so the closure can share the
    /// cell with the enclosing frame. Reads and writes to such a local
    /// become loads/stores through the cell pointer stored in the local.
    cell_promoted: HashSet<DefId>,
    /// Map from a `const` definition's DefId to its initializer expression.
    /// References to a constant are substituted with the RHS at every use
    /// site so that `const NAME = 100` emits the literal directly, rather
    /// than reading an uninitialized local.
    const_values: HashMap<DefId, HirExpr>,
    /// Records `(src_type, dst_type)` pairs for every `impl Into[Dst] for Src`
    /// in the program. Consulted by `?`-operator lowering so that a
    /// `Result[_, Inner]` returned via `?` is converted to a
    /// `Result[_, Outer]` by calling `Inner_into(err_payload)` when the
    /// caller declares `-> Result[_, Outer]`.
    into_impls: HashSet<(String, String)>,
    /// trait_name → map of method_name → default method `HirFuncDef`.
    /// Populated from every `HirItem::Mixin` at the start of lowering so
    /// that each `impl Trait for Type` can monomorphize the default body
    /// for `Type` if the impl does not override the method itself.
    trait_default_methods: HashMap<String, HashMap<String, HirFuncDef>>,
    /// Set of class names that have a user-defined `def drop` (typically
    /// inside an `impl Drop` block). Consulted by `insert_drops` so that
    /// scope-exit cleanup of an instance of such a class emits a call to
    /// the user's `{ClassName}_drop` method before the no-op `MirInst::Drop`.
    user_drop_classes: HashSet<String>,
    /// Active inside a closure body during lowering: for each captured
    /// `DefId`, the (slot_index_in_captures_struct, storage_kind). This
    /// lets `VarRef`/`Assign`/`CompoundAssign` on a captured variable
    /// redirect to loads/stores through the captures pointer rather than
    /// accessing a non-existent local in the closure function.
    capture_map: HashMap<DefId, CaptureSlot>,
    /// Local that holds the `captures_ptr` in the current closure function.
    /// `None` when not lowering a closure body (or when the closure has no
    /// captures).
    captures_ptr_local: Option<LocalId>,
    /// Locals that currently hold an initialized, frame-owned heap
    /// allocation (Class/Struct/Enum). Populated when a `let` binds a
    /// heap-typed value and when an `Assign` overwrites such a local.
    /// Consulted by `Assign` lowering so that re-binding a heap-typed
    /// local frees the prior allocation before the new pointer overwrites
    /// it. Cleared per function in `lower_method` and closure entry.
    initialized_heap_locals: HashSet<LocalId>,
    /// #06.8 Phase 2: map from Ruxen-side FFI fn name → linked C symbol,
    /// populated at the start of `lower_program` from `HirProgram::ffi_libs`.
    /// Consulted by `lower_fn_call` so that calling a Ruxen name like
    /// `add_one` whose `lib` block declared `def add_one as
    /// "ruxen_test_add_one"(...)` emits `MirInst::Call { callee:
    /// "ruxen_test_add_one", ... }` instead of `add_one`. Without this
    /// rewrite the linker would resolve the call to the wrong symbol
    /// (or fail outright if no `add_one` C symbol exists).
    ffi_alias_map: HashMap<String, String>,
    /// Generic-class monomorphization (option 1): eligible generic classes
    /// keyed by class name. Populated by `collect_mono_instances`; an entry
    /// exists only for a user-defined generic class with ≥1 user method and
    /// NO FFI `lib` methods (so `Mutex[T]` & friends are excluded).
    mono_classes: HashMap<String, crate::hir::nodes::HirClassDef>,
    /// Class name → distinct fully-concrete generic-arg vectors seen at use
    /// sites (e.g. `Matcher` → `[[String], [Int], [Bool]]`).
    mono_instances: HashMap<String, Vec<monomorphize::MonoKey>>,
    /// Monomorphized bases actually emitted (`Box__mono__String`, …). A
    /// call/construct site is redirected to a specialized callee only when
    /// its receiver instantiation's base is in this set — otherwise the
    /// opaque `{Class}_{method}` fallback is kept.
    mono_emitted: HashSet<String>,
    /// Real (non-FFI) body methods defined directly on an FFI-shell
    /// generic builtin class (`Array[T]`, `Option[T]`, `Result[T,E]`).
    /// Keyed by the generic-STRIPPED mangled name (`Array_map`,
    /// `Option_unwrap_or_else`). These classes are excluded from
    /// monomorphization (gating rule #1) so their body methods emit as a
    /// single opaque `{Class}_{method}` function with type params left
    /// abstract — sound for the closure combinators, which only shuffle
    /// pointer/word-sized values (push / closure-call / inlined `each`)
    /// and never dispatch on the concrete `T`. The call site mangles
    /// `Array[Int]_map`; `resolve_ffi_alias_callee` strips the generic
    /// suffix to reach the opaque body when the stripped name is in here.
    lib_body_methods: HashSet<String>,
    /// Q17: eligible generic FREE functions (≥1 mixin-bounded type param),
    /// keyed by function name. Populated by `collect_generic_fn_instances`.
    mono_fns: HashMap<String, crate::hir::nodes::HirFuncDef>,
    /// Q17: function name → distinct fully-concrete generic-arg vectors
    /// recovered at call sites (e.g. `paint_all` → `[[RecordingSurface],
    /// [TallySurface]]`).
    fn_mono_instances: HashMap<String, Vec<monomorphize::MonoKey>>,
    /// Q17: function name → emitted `(concrete-args, mangled-base)` pairs. A
    /// free-fn call site is redirected to the mangled monomorphic callee only
    /// when its recovered type-args match an entry here.
    fn_mono_emitted: HashMap<String, Vec<(monomorphize::MonoKey, String)>>,
    /// Q17: generic fns that saw at least one call we could NOT monomorphize
    /// (a generic-through-generic shape with a non-concrete leaf). Their
    /// opaque abstract body must still be emitted by the normal `lower_item`
    /// path so that call resolves (and devirtualizes via `unique_bound_impl`,
    /// or surfaces a clear error).
    fn_mono_needs_opaque: HashSet<String>,
}

/// A captured variable's storage inside the captures struct.
#[derive(Debug, Clone, Copy)]
struct CaptureSlot {
    /// Index of the 8-byte slot (captures[slot_index] is at offset 8*idx).
    slot_index: usize,
    /// Whether the slot holds the value directly (`ByValue`) or a pointer
    /// to a single-slot heap cell (`ByRef`).  `ByRef` is used for
    /// `let mut`-bound variables that the closure mutates through a
    /// shared cell with the enclosing frame.
    kind: CaptureKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureKind {
    /// The slot stores the captured value directly (move or copy of a
    /// Copy-typed local).
    ByValue,
    /// The slot stores a pointer to an 8-byte heap cell shared with the
    /// enclosing frame (for non-`move` closures over mutable locals).
    ByRef,
}

/// Where a closure capture's value is read FROM, in the enclosing
/// lowering frame, when the captures struct is filled. Mirrors the two
/// resolution paths `lower_var_ref` uses for a `VarRef`.
#[derive(Debug, Clone, Copy)]
enum CaptureSource {
    /// An outer-frame local (`def_to_local`). The value (or, post
    /// cell-promotion, the cell pointer) lives directly in this LocalId.
    Local(LocalId),
    /// A capture of the ENCLOSING closure (`capture_map`): this closure
    /// literal is nested inside another closure's body and re-captures one
    /// of the outer block's captures. The value is read out of the
    /// enclosing captures pointer at `slot_index` (through the cell when
    /// the enclosing capture is `ByRef`). (Q26.)
    Recapture(CaptureSlot),
}

/// Per-active-loop book-keeping: targets for `continue`/`break`, the
/// optional result local that `break <value>` writes into, and the set
/// of heap-owned locals declared inside the loop body that must be
/// freed at every loop-exit edge (break, continue, back-edge) so they
/// do not leak across iterations.
#[derive(Debug, Clone)]
struct LoopFrame {
    continue_target: BlockId,
    break_target: BlockId,
    result_local: Option<LocalId>,
    /// Block at the top of each iteration. Zero-init prologue for
    /// `body_locals` is prepended here after lowering so a path that
    /// bypasses a `let` can still reach a back-edge with a NULL local.
    body_entry_block: BlockId,
    /// Heap-owned (`Class`/`Struct`/`Enum`) locals declared inside this
    /// loop's body. At every loop-exit edge we emit
    /// `ruxen_dealloc(L)` followed by `Assign L = 0` so the next
    /// iteration sees a NULL slot and `free(NULL)` is a documented
    /// no-op for paths that did not run the `let`. (P0.2)
    body_locals: Vec<LocalId>,
}

impl<'a> Lowerer<'a> {
    pub fn new(symbols: &'a SymbolTable) -> Self {
        Lowerer {
            symbols,
            def_to_local: HashMap::new(),
            current_fn: None,
            current_block: 0,
            closure_counter: 0,
            pending_closures: Vec::new(),
            trait_impls: HashMap::new(),
            loop_stack: Vec::new(),
            cell_promoted: HashSet::new(),
            const_values: HashMap::new(),
            into_impls: HashSet::new(),
            trait_default_methods: HashMap::new(),
            user_drop_classes: HashSet::new(),
            capture_map: HashMap::new(),
            captures_ptr_local: None,
            initialized_heap_locals: HashSet::new(),
            ffi_alias_map: HashMap::new(),
            lib_body_methods: HashSet::new(),
            mono_classes: HashMap::new(),
            mono_instances: HashMap::new(),
            mono_emitted: HashSet::new(),
            mono_fns: HashMap::new(),
            fn_mono_instances: HashMap::new(),
            fn_mono_emitted: HashMap::new(),
            fn_mono_needs_opaque: HashSet::new(),
        }
    }

    /// Given a generic type parameter's bounds, return the unique concrete
    /// implementor if exactly one exists across all the trait bounds
    /// (so that `a.method(...)` on a TypeParam dispatches unambiguously).
    ///
    /// For a multi-bound `T: A + B`, compute the intersection of impl
    /// targets across every bound; if the intersection has exactly one
    /// type, dispatch to it.
    fn unique_bound_impl(&self, bounds: &[crate::hir::types::MixinRef]) -> Option<String> {
        if bounds.is_empty() {
            return None;
        }
        if bounds.len() == 1 {
            let impls = self.trait_impls.get(&bounds[0].name)?;
            if impls.len() == 1 {
                return Some(impls[0].clone());
            }
            return None;
        }

        // Multi-bound: intersect impl-target sets across all bounds.
        let first = self.trait_impls.get(&bounds[0].name)?;
        let mut candidates: Vec<String> = first.clone();
        for b in &bounds[1..] {
            let next = self.trait_impls.get(&b.name)?;
            candidates.retain(|c| next.contains(c));
            if candidates.is_empty() {
                return None;
            }
        }
        // De-duplicate (the same type may be pushed twice for redundant impls).
        candidates.sort();
        candidates.dedup();
        if candidates.len() == 1 {
            Some(candidates.remove(0))
        } else {
            None
        }
    }

    /// Q17 (staged boundary): true when the receiver type (after peeling refs)
    /// is ITSELF a still-abstract, mixin-bound type parameter
    /// (`TypeParam`/`SomeMixin`/`AnyMixin`) with bounds but NO unique
    /// implementor — so a method call on it would mangle to a
    /// bound-placeholder callee (`T: Sized_width`) that link-fails. This is the
    /// generic-METHOD-over-mixin case (not yet monomorphized; generic free
    /// functions ARE handled). Detected on the receiver SHAPE, not on the
    /// stringified callee: a concrete receiver carrying a bounded generic ARG
    /// (`Array[T: Showable]`) is a sound builtin call and must return false.
    fn receiver_is_unresolved_bound(&self, ty: &Ty) -> bool {
        let mut cur = ty;
        loop {
            match cur {
                Ty::Ref(inner)
                | Ty::RefMut(inner)
                | Ty::RefLifetime(_, inner)
                | Ty::RefMutLifetime(_, inner) => cur = inner,
                Ty::TypeParam { bounds, .. } | Ty::SomeMixin(bounds) | Ty::AnyMixin(bounds) => {
                    // Unbounded `[T]` is never dispatched on (a bare type param
                    // with no bounds never names a bound method). A callable
                    // bound (`Fn`/`FnMut`/`FnOnce`, e.g. `any Fn[Fn(T) -> U]`)
                    // is dispatched by the closure `.call` mechanism, not by a
                    // nominal `{Type}_{method}` mangle, so it never produces a
                    // bound-placeholder callee — exclude it. Only a genuine
                    // user-mixin bound with no unique implementor yields the
                    // placeholder this guard rejects.
                    if bounds.is_empty()
                        || bounds
                            .iter()
                            .any(|b| matches!(b.name.as_str(), "Fn" | "FnMut" | "FnOnce"))
                    {
                        return false;
                    }
                    return self.unique_bound_impl(bounds).is_none();
                }
                _ => return false,
            }
        }
    }

    // ── Public entry point ──────────────────────────────────────────────

    fn qualified_item_name(module_path: &[String], name: &str) -> String {
        if module_path.is_empty() {
            name.to_string()
        } else {
            format!("{}.{}", module_path.join("."), name)
        }
    }

    fn symbol_name(name: &str) -> String {
        name.replace('.', "_")
    }

    fn class_has_method(&self, class_name: &str, method_name: &str) -> bool {
        self.symbols.iter().any(|def| {
            let crate::resolve::symbols::DefKind::Method { parent, .. } = &def.kind else {
                return false;
            };
            self.symbols
                .get(*parent)
                .map(|parent_def| {
                    parent_def.name == class_name
                        && (def.name == method_name
                            || def.name.starts_with(&format!("{}__overload", method_name)))
                })
                .unwrap_or(false)
        })
    }

    fn class_has_method_accepting_args(
        &self,
        class_name: &str,
        method_name: &str,
        args: &[HirExpr],
    ) -> bool {
        self.symbols.iter().any(|def| {
            let crate::resolve::symbols::DefKind::Method { parent, signature } = &def.kind else {
                return false;
            };
            self.symbols
                .get(*parent)
                .map(|parent_def| {
                    parent_def.name == class_name
                        && (def.name == method_name
                            || def.name.starts_with(&format!("{}__overload", method_name)))
                        && self.method_signature_accepts_args(signature, args)
                })
                .unwrap_or(false)
        })
    }

    fn method_signature_accepts_args(
        &self,
        signature: &crate::resolve::symbols::FnSignature,
        args: &[HirExpr],
    ) -> bool {
        let required = signature
            .params
            .iter()
            .filter(|p| p.default.is_none())
            .count();
        if args.len() < required || args.len() > signature.params.len() {
            return false;
        }
        args.iter()
            .zip(signature.params.iter())
            .all(|(arg, param)| {
                // Q1: a CALLABLE argument (closure literal, or a value of
                // `Fn`/`any Fn`/`some Fn` type) matches ONLY a callable
                // parameter — never a `&str`/other one. Mirrors the typeck
                // overload selector; without it the MIR symbol selection
                // mangled `text(closure)` to the `text(&str)` overload and
                // stored the closure pointer as a String (heap corruption).
                let arg_is_callable =
                    matches!(arg.kind, HirExprKind::Closure { .. }) || ty_is_callable(&arg.ty);
                if arg_is_callable {
                    return ty_is_callable(&param.ty);
                }
                arg.ty.is_infer()
                    || arg.ty.is_error()
                    || arg.ty == param.ty
                    || matches!((&arg.ty, &param.ty), (Ty::Str, Ty::String))
                    || matches!((&arg.ty, &param.ty), (Ty::String, Ty::Str))
                    || matches!(
                        (&arg.ty, &param.ty),
                        (Ty::Ref(a), Ty::Ref(b)) if matches!((&**a, &**b), (Ty::Str, Ty::String))
                    )
                    // A by-value argument satisfies a `&T`/`&mut T` parameter
                    // of the same underlying type (the call site auto-refs).
                    // `Str` and `String` are unified here so a `"..."` literal
                    // (`Ty::Str`) matches a `&str`/`&String` parameter — the
                    // missing arm that let an `add("static")` call fall through
                    // to the arity-only fallback and bind the FIRST-declared
                    // overload (a closure one), mismatching the symbol name.
                    // Mirrors the typeck selector in
                    // `typeck/infer/collect.rs::method_accepts_args`.
                    || matches!(
                        &param.ty,
                        Ty::Ref(inner) | Ty::RefMut(inner)
                            if **inner == arg.ty
                                || matches!((&arg.ty, &**inner), (Ty::Str, Ty::String) | (Ty::String, Ty::Str))
                    )
            })
    }

    fn select_method_symbol_name(
        &self,
        class_name: &str,
        method_name: &str,
        args: &[HirExpr],
    ) -> Option<String> {
        // Ruby `alias new old` (docs/decisions/alias-keyword.md): rewrite an
        // alias method name to its canonical BEFORE scanning for the emitted
        // symbol, so a call via the alias mangles to the real method's body
        // (`set.member?(x)` → `Set_include?`), never a bodiless `Set_member?`.
        let method_name = self.symbols.canonical_method_name(class_name, method_name);
        let mut candidates = Vec::new();
        for def in self.symbols.iter() {
            let crate::resolve::symbols::DefKind::Method { parent, signature } = &def.kind else {
                continue;
            };
            let Some(parent_def) = self.symbols.get(*parent) else {
                continue;
            };
            if parent_def.name == class_name
                && (def.name == method_name
                    || def.name.starts_with(&format!("{}__overload", method_name)))
            {
                candidates.push((def.name.clone(), signature.clone()));
            }
        }
        candidates
            .iter()
            .find(|(_, sig)| {
                sig.params.len() == args.len() && self.method_signature_accepts_args(sig, args)
            })
            .or_else(|| {
                candidates
                    .iter()
                    .find(|(_, sig)| self.method_signature_accepts_args(sig, args))
            })
            .or_else(|| {
                candidates.iter().find(|(_, sig)| {
                    let required = sig.params.iter().filter(|p| p.default.is_none()).count();
                    args.len() >= required && args.len() <= sig.params.len()
                })
            })
            .map(|(name, _)| name.clone())
    }

    /// Trailing default arguments the resolved method declares that a call
    /// supplying `supplied_user_args` positional arguments (NOT counting the
    /// receiver) did not fill — each materialized as the param's lowered
    /// `default` literal value.
    ///
    /// This is the MIR-side mirror of typeck's `append_method_default_args`
    /// (`infer/collect.rs`). The PARENS method-call path (`MethodCall` HIR
    /// node) gets its trailing defaults appended at typeck, so by the time it
    /// reaches MIR the args vector already carries them. The PAREN-LESS no-arg
    /// method-call path lowers as a `FieldAccess` (no args vector), so typeck
    /// never runs the default-arg pass on it — without this, an optional
    /// `&block` slot (which carries a `nil` default → null closure-pair-pointer
    /// sentinel, Ruby-block-semantics ADR D1/D5) is left unfilled and the call
    /// emits one too few arguments, crashing the MIR/Cranelift arity verifier
    /// (`__closure_*: got 1, expected 2`). Filling it here makes `w.frame` and
    /// `w.frame()` lower IDENTICALLY (the block-slot consistency the blocks
    /// feature filed as a known limitation).
    ///
    /// Each default param's `nil`/`null` lowers to `Literal::Int(0)` — the same
    /// value the `NullLiteral` HIR expr lowers to for a non-`Option` type
    /// (`expr/literals.rs`); for the block slot that is the null
    /// closure-pair-pointer the `emit_block_presence_guard` then tests.
    fn method_trailing_default_sentinels(
        &self,
        class_name: &str,
        method_name: &str,
        supplied_user_args: usize,
    ) -> Vec<MirValue> {
        let method_name = self.symbols.canonical_method_name(class_name, method_name);
        // Find the resolved method's signature, walking parents for inherited
        // methods (mirrors `resolve_method_class`'s ancestor search via the
        // already-resolved `class_name`).
        let signature = self.symbols.iter().find_map(|def| {
            let crate::resolve::symbols::DefKind::Method { parent, signature } = &def.kind else {
                return None;
            };
            let parent_name = self.symbols.get(*parent).map(|p| p.name.as_str())?;
            (parent_name == class_name
                && (def.name == method_name
                    || def.name.starts_with(&format!("{}__overload", method_name))))
            .then(|| signature.clone())
        });
        let Some(signature) = signature else {
            return Vec::new();
        };
        // Skip the params already covered by the supplied user args, then
        // materialize each remaining param that carries a default. Only `nil`
        // / `null` defaults arise on this path (the optional `&block` slot);
        // a non-null literal default on a paren-less-reachable param is also
        // honoured for parity with the parens path.
        signature
            .params
            .iter()
            .skip(supplied_user_args)
            .filter_map(|p| p.default.as_ref().map(|d| Self::default_expr_to_sentinel(d)))
            .collect()
    }

    /// Lower a param `default` AST expr to the MIR value the no-arg
    /// field-access path appends. `nil`/`null` → `Literal::Int(0)` (the null
    /// closure-pair-pointer for a `&block` slot, matching `NullLiteral`'s
    /// non-`Option` lowering). Other literal defaults map to their value.
    fn default_expr_to_sentinel(default: &crate::parser::ast::Expr) -> MirValue {
        use crate::parser::ast::ExprKind;
        match &default.kind {
            ExprKind::NullLiteral | ExprKind::UnitLiteral => {
                MirValue::Literal(Literal::Int(0))
            }
            ExprKind::IntLiteral(v, _) => MirValue::Literal(Literal::Int(*v)),
            ExprKind::BoolLiteral(v) => MirValue::Literal(Literal::Bool(*v)),
            ExprKind::FloatLiteral(v, _) => MirValue::Literal(Literal::Float(*v)),
            // Any other default shape falls back to the null sentinel; the
            // only paren-less-reachable trailing default in practice is the
            // optional `&block` (`nil`), so this is conservative parity.
            _ => MirValue::Literal(Literal::Int(0)),
        }
    }

    fn lower_items(
        &mut self,
        items: &[HirItem],
        mir: &mut MirProgram,
        module_path: &[String],
    ) -> Result<(), String> {
        for item in items {
            self.lower_item(item, mir, module_path)?;
        }
        Ok(())
    }

    fn lower_item(
        &mut self,
        item: &HirItem,
        mir: &mut MirProgram,
        module_path: &[String],
    ) -> Result<(), String> {
        match item {
            HirItem::Function(func) => {
                let mut lowered = func.clone();
                if !module_path.is_empty() {
                    lowered.name =
                        Self::symbol_name(&Self::qualified_item_name(module_path, &lowered.name));
                }
                // Q17: suppress the opaque (un-monomorphized) body of EVERY
                // eligible generic free fn — instantiated or not. Its abstract
                // body can only ever emit bound-placeholder callees
                // (`T: Paintable_fill_rect`) that link-fail, and no valid call
                // resolves to it: a concrete call is redirected to a monomorphic
                // copy (or errors at the call site), and an abstract call lives
                // only inside another generic body that is itself suppressed.
                // A NON-eligible fn (`generic_fn_keeps_opaque` → true) is
                // unaffected. Matched on the un-qualified key the collector
                // recorded (module-qualified names share it).
                if !self.generic_fn_keeps_opaque(&func.name) {
                    return Ok(());
                }
                let mir_fn = self.lower_function(&lowered)?;
                if mir_fn.name == "main" {
                    mir.entry = Some("main".to_string());
                }
                mir.functions.push(mir_fn);
            }
            HirItem::Class(class) => {
                let qualified_class = Self::qualified_item_name(module_path, &class.name);
                let class_symbol = Self::symbol_name(&qualified_class);
                for method in &class.methods {
                    let mangled = format!("{}_{}", class_symbol, method.name);
                    let mir_fn = self.lower_method(&mangled, method)?;
                    mir.functions.push(mir_fn);
                }
                let outer_methods: HashSet<String> =
                    class.methods.iter().map(|m| m.name.clone()).collect();
                for impl_block in &class.impl_blocks {
                    self.lower_impl_block_with_outer_methods(
                        impl_block,
                        &qualified_class,
                        mir,
                        &outer_methods,
                    )?;
                }
                let class_ty = Ty::Class {
                    name: qualified_class,
                    generic_args: vec![],
                };
                let class_has = |trait_name: &str| -> bool {
                    crate::resolve::symbols::ty_has_derive_trait(
                        &class_ty,
                        self.symbols,
                        trait_name,
                    )
                };
                if module_path.is_empty() && class_has("Clone") {
                    mir.functions.push(self.synthesize_class_clone(class));
                }
            }
            HirItem::Struct(s) => {
                let qualified_struct = Self::qualified_item_name(module_path, &s.name);
                let struct_symbol = Self::symbol_name(&qualified_struct);
                for method in &s.methods {
                    let mangled = format!("{}_{}", struct_symbol, method.name);
                    let mir_fn = self.lower_method(&mangled, method)?;
                    mir.functions.push(mir_fn);
                }
                let struct_ty = Ty::Struct {
                    name: qualified_struct,
                    generic_args: vec![],
                };
                let has = |trait_name: &str| -> bool {
                    crate::resolve::symbols::ty_has_derive_trait(
                        &struct_ty,
                        self.symbols,
                        trait_name,
                    )
                };
                if has("Debug") {
                    let dbg_fn = self.synthesize_struct_to_debug(s);
                    mir.functions.push(dbg_fn);
                }
                if has("PartialEq") {
                    mir.functions.push(self.synthesize_struct_eq(s));
                }
                if has("Hashable") || has("Hash") {
                    mir.functions.push(self.synthesize_struct_hash_code(s));
                }
                if has("Default") {
                    mir.functions.push(self.synthesize_struct_default(s));
                }
                if has("Ord") {
                    mir.functions.push(self.synthesize_struct_cmp(s, false));
                }
                if has("PartialOrd") {
                    mir.functions.push(self.synthesize_struct_cmp(s, true));
                }
                if has("Clone") {
                    mir.functions.push(self.synthesize_struct_clone(s));
                }
            }
            HirItem::Enum(e) => {
                let qualified_enum = Self::qualified_item_name(module_path, &e.name);
                let enum_symbol = Self::symbol_name(&qualified_enum);
                for method in &e.methods {
                    let mangled = format!("{}_{}", enum_symbol, method.name);
                    let mir_fn = self.lower_method(&mangled, method)?;
                    mir.functions.push(mir_fn);
                }
                let enum_ty = Ty::Enum {
                    name: qualified_enum,
                    generic_args: vec![],
                };
                let has = |trait_name: &str| -> bool {
                    crate::resolve::symbols::ty_has_derive_trait(&enum_ty, self.symbols, trait_name)
                };
                if has("Debug") {
                    mir.functions.push(self.synthesize_enum_to_debug(e));
                }
                if has("Clone") {
                    mir.functions.push(self.synthesize_enum_clone(e));
                }
            }
            HirItem::Impl(impl_block) => {
                let type_name = type_name_from_ty(&impl_block.target_ty);
                self.lower_impl_block(impl_block, &type_name, mir)?;
            }
            HirItem::Module(m) => {
                if module_path.is_empty() && m.name == "std" {
                    return Ok(());
                }
                let mut child_path = module_path.to_vec();
                child_path.push(m.name.clone());
                self.lower_items(&m.items, mir, &child_path)?;
            }
            HirItem::Mixin(_) | HirItem::TypeAlias(_) | HirItem::Newtype(_) | HirItem::Const(_) => {
            }
        }
        Ok(())
    }

    pub fn lower_program(&mut self, program: &HirProgram) -> Result<MirProgram, String> {
        let mut mir = MirProgram::new();

        // Gather `impl Trait for Type` edges so method calls on generic
        // type parameters can dispatch to the unique implementor.
        self.collect_trait_impls(program);

        // Collect trait default method bodies so that every `impl` can
        // monomorphise missing methods into a concrete {Type}_{method}.
        self.collect_trait_default_methods(program);

        // Generic-class monomorphization (option 1): record every concrete
        // instantiation of an eligible generic class so binop / Display
        // lowering over a type parameter resolves to the concrete type.
        // FFI-shell generic classes (`Mutex[T]`, …) are excluded here.
        self.collect_mono_instances(program);

        // Q17: record every concrete instantiation of an eligible generic
        // FREE function (mixin-bounded type param) seen at a call site, so a
        // dependency's generic (`paint_all[T: Paintable]`) can be specialized
        // per CONSUMER implementor instead of emitting the bound-placeholder
        // callee (`T: Paintable_fill_rect`) that link-fails. Runs after
        // `collect_mono_instances` (the trait-impl table is already built).
        self.collect_generic_fn_instances(program);

        // Collect `const` initializer expressions so references are
        // substituted with the RHS value at every use site.
        self.collect_const_values(program);

        // Record every class that defines its own `def drop` so that
        // drop-elaboration emits a call to `{ClassName}_drop` before
        // the no-op `MirInst::Drop` cleanup at scope exit.
        self.collect_user_drop_classes(program);

        // #06.8 Phase 2: bridge `HirProgram::ffi_libs` → `MirProgram::ffi_libs`.
        // Each FFI decl becomes a codegen `FfiFuncDecl` whose `name` field is
        // the linked C symbol (alias if present, Ruxen name otherwise) and
        // whose `ruxen_name` is the call-site identifier. We also build the
        // `ffi_alias_map` so call-site lowering can rewrite a Ruxen-named
        // FFI call into a call to the actual C symbol.
        for hir_lib in &program.ffi_libs {
            let mut funcs = Vec::with_capacity(hir_lib.functions.len());
            for hir_fn in &hir_lib.functions {
                let c_name = hir_fn
                    .c_symbol
                    .clone()
                    .unwrap_or_else(|| hir_fn.ruxen_name.clone());
                if hir_fn.c_symbol.is_some() {
                    self.ffi_alias_map
                        .insert(hir_fn.ruxen_name.clone(), c_name.clone());
                }
                funcs.push(crate::mir::nodes::FfiFuncDecl {
                    name: c_name,
                    ruxen_name: hir_fn.ruxen_name.clone(),
                    param_types: hir_fn.param_types.clone(),
                    return_type: hir_fn.return_type.clone(),
                    is_variadic: hir_fn.is_variadic,
                });
            }
            mir.ffi_libs.push(crate::mir::nodes::FfiLib {
                name: hir_lib.name.clone(),
                link_flags: hir_lib.link_flags.clone(),
                functions: funcs,
            });
        }

        self.lower_items(&program.items, &mut mir, &[])?;

        // Generic-class monomorphization (option 1): emit a specialized MIR
        // copy of every user-defined method (incl. `init`) of each eligible
        // generic class, once per recorded concrete instantiation. Must run
        // after `lower_items` so the opaque fallback bodies and the symbol
        // table are already in place. Call sites are redirected to these
        // specialized callees in `method_call.rs` via `mono_base_for_ty`.
        self.emit_mono_instances(&mut mir)?;

        // Q17: emit one specialized MIR copy of every eligible generic free
        // function, per recorded concrete instantiation, and record the
        // `(fn, args) → mangled` redirects consulted by `fn_call.rs`. Same
        // ordering rationale as `emit_mono_instances` (opaque fallback bodies
        // and symbols already in place).
        self.emit_generic_fn_instances(&mut mir)?;

        // Emit the primitive Display::fmt synth functions unconditionally
        // (Phase 2 #06.D2.S1). These are program-level, not per-use.
        // Stage 3 (D2) `lower_interpolation` rewrite assumes these are always
        // present; conditional emission would require a two-pass approach.
        for f in self.synthesize_primitive_fmt_displays() {
            mir.functions.push(f);
        }

        // Append any closure functions generated during lowering,
        // running drop-elaboration on each one. Without this, locals
        // declared inside a `do || ... end` body — particularly
        // user-drop classes like `MutexGuard` — never get their
        // scope-exit drop emitted, and the underlying resource (a
        // pthread mutex, an fd, a refcount) leaks per call. The
        // closure body returns either a tail-expr value or unit; the
        // closure_inline path already sets up Return terminators
        // before pushing into `pending_closures`, so insert_drops
        // sees the same shape it sees for top-level methods.
        for mut closure_fn in std::mem::take(&mut self.pending_closures) {
            let return_locals = self.find_return_locals(&closure_fn);
            crate::mir::lower::drops::insert_drops(
                &mut closure_fn,
                &return_locals,
                self.symbols,
                &self.user_drop_classes,
                &|mangled| self.resolve_ffi_alias_callee(mangled),
            );
            mir.functions.push(closure_fn);
        }

        // Mixin vtables Phase B-2/B-3: emit vtable + class_info metadata
        // for every class that includes any `dispatch runtime` mixin.
        // Codegen reads these vectors and emits one data section per
        // vtable + one per class_info. Order is: per (class, mixin)
        // pair, with mixin slots in `runtime_dispatch_includes` order
        // (mixin-include declaration order on the class).
        self.collect_mixin_vtables(&mut mir);

        // Mixin vtables Phase C: synthesize one `<Mixin>_dynamic_<method>`
        // helper per required method of every `dispatch runtime` mixin
        // that has at least one implementor. The helper does:
        //   class_info = self[0]            // class_info_ptr at slot 0
        //   vtable = class_info[mixin_slot] // v1: mixin_slot is always 0
        //   method_ptr = vtable[method_idx] // slot of this method in vtable
        //   result = method_ptr(self, args...)
        //   return result
        self.synthesize_dynamic_dispatch_helpers(&mut mir);

        // Post-lowering fixup: elide the `ruxen_dealloc(R)` that
        // `lower_assign` emits before `Assign { dest: R, value: Use(X) }`
        // when `X` came from a call whose callee returns `self`. The
        // dealloc-then-rebind pattern is a use-after-free whenever
        // the call returns its receiver. See `drops::elide_returns_self_realloc`
        // for the detection details and the minimum reproducer.
        crate::mir::lower::drops::elide_returns_self_realloc(&mut mir);

        Ok(mir)
    }

    /// Phase B-2/B-3: enumerate every class that includes a
    /// `dispatch runtime` mixin and emit a `MirVtable` per `(class,
    /// mixin)` pair plus a single `MirClassInfo` per class.
    ///
    /// The class's `runtime_dispatch_includes` list (populated at
    /// resolve time per spec §B1) is the authoritative ordering. For
    /// each mixin DefId we collect its `MixinInfo.required_methods`
    /// (in declaration order) and emit one `MirVtable` whose
    /// `method_symbols` are the mangled `<ClassName>_<method>` symbols
    /// codegen has already declared.
    ///
    /// Missing-method errors are caught earlier as E1117 in typeck;
    /// reaching here means every required method has a class-side
    /// implementation. If a method symbol is somehow absent, the
    /// linker will surface it; the MIR layer doesn't second-guess.
    fn collect_mixin_vtables(&self, mir: &mut MirProgram) {
        use crate::resolve::symbols::DefKind;

        // Iterate over classes. We need a stable order: walk the
        // symbol table.
        for def in self.symbols.iter() {
            let info = match &def.kind {
                DefKind::Class { info } if !info.runtime_dispatch_includes.is_empty() => info,
                _ => continue,
            };
            let class_name = def.name.clone();
            let mut class_vtable_syms: Vec<String> = Vec::new();

            for &mixin_def_id in &info.runtime_dispatch_includes {
                let mixin_def = match self.symbols.get(mixin_def_id) {
                    Some(d) => d,
                    None => continue, // shouldn't happen; defensive
                };
                let mixin_info = match &mixin_def.kind {
                    DefKind::Trait { info } => info,
                    _ => continue,
                };
                let mixin_name = mixin_def.name.clone();
                let method_symbols: Vec<String> = mixin_info
                    .required_methods
                    .iter()
                    .map(|m| format!("{}_{}", class_name, m))
                    .collect();
                let vt = crate::mir::nodes::MirVtable {
                    mixin_name,
                    class_name: class_name.clone(),
                    method_symbols,
                };
                class_vtable_syms.push(vt.symbol());
                mir.vtables.push(vt);
            }

            mir.class_infos.push(crate::mir::nodes::MirClassInfo {
                class_name,
                vtable_symbols: class_vtable_syms,
            });
        }
    }

    /// Phase C: synthesize one `<Mixin>_dynamic_<method>` helper per
    /// required method on every runtime-dispatch mixin that has at
    /// least one implementor class in this program.
    ///
    /// Each helper does three loads (class_info_ptr, vtable_ptr,
    /// method_ptr) and one indirect call. The mixin's slot within
    /// class_info is fixed at 0 for v1: the spec ships single-mixin
    /// runtime dispatch (Future is the only opt-in), and every
    /// runtime-dispatch class's class_info therefore has exactly one
    /// slot — the mixin's vtable pointer. Multi-mixin runtime
    /// dispatch is v2 (spec "Out of scope").
    ///
    /// Helper signature mirrors the mixin's required method signature
    /// (1:1 param types + return type), so call sites can lower to a
    /// plain `Call <Mixin>_dynamic_<method>(self, args...)` without
    /// type adaptation. The signature is read from the mixin
    /// method's `DefKind::Method` entry in the symbol table.
    fn synthesize_dynamic_dispatch_helpers(&self, mir: &mut MirProgram) {
        use crate::resolve::symbols::DefKind;

        // Collect (mixin_name, mixin_def_id) for every runtime-dispatch
        // mixin that has at least one implementor.
        let mut mixins_to_synth: std::collections::BTreeMap<String, DefId> =
            std::collections::BTreeMap::new();
        for vt in &mir.vtables {
            // `vt.mixin_name` is unique per mixin; look up its DefId.
            if mixins_to_synth.contains_key(&vt.mixin_name) {
                continue;
            }
            for def in self.symbols.iter() {
                if def.name == vt.mixin_name
                    && matches!(&def.kind, DefKind::Trait { info } if matches!(
                        info.dispatch_mode,
                        crate::parser::ast::DispatchMode::Runtime
                    ))
                {
                    mixins_to_synth.insert(vt.mixin_name.clone(), def.id);
                    break;
                }
            }
        }

        for (mixin_name, mixin_def_id) in mixins_to_synth {
            // Look up the mixin's required-methods (in declaration
            // order) — same list that drives vtable layout in
            // `collect_mixin_vtables`.
            let mixin_info = match self.symbols.get(mixin_def_id) {
                Some(d) => match &d.kind {
                    DefKind::Trait { info } => info.clone(),
                    _ => continue,
                },
                None => continue,
            };
            for (method_idx, method_name) in mixin_info.required_methods.iter().enumerate() {
                // Mixin method signatures live in HirMixinDef.items as
                // HirMixinItem::MethodSig, NOT in the symbol table as
                // `DefKind::Method`. The symbol-table-registered
                // methods are class-side implementations (`Circle.size`
                // → `DefKind::Method { parent: Circle, .. }`). Pick
                // any implementor's method as the signature source —
                // E1117 has guaranteed every implementor's signature
                // matches the mixin's contract, so any concrete impl
                // is a valid source.
                let sig = self.symbols.iter().find_map(|d| match &d.kind {
                    DefKind::Method { parent, signature } if d.name == *method_name => {
                        // Verify the parent class actually includes
                        // this mixin (otherwise a same-named method
                        // on an unrelated class would mismatch).
                        let parent_includes_mixin = self
                            .symbols
                            .get(*parent)
                            .map(|p| match &p.kind {
                                DefKind::Class { info } => {
                                    info.runtime_dispatch_includes.contains(&mixin_def_id)
                                }
                                _ => false,
                            })
                            .unwrap_or(false);
                        if parent_includes_mixin {
                            Some(signature.clone())
                        } else {
                            None
                        }
                    }
                    _ => None,
                });
                let Some(sig) = sig else {
                    // No implementor of the mixin in this program —
                    // skip helper synthesis (an unreachable helper
                    // would add link-time weight for no benefit).
                    continue;
                };

                let helper =
                    self.build_dynamic_dispatch_helper(&mixin_name, method_name, method_idx, &sig);
                mir.functions.push(helper);
            }
        }
    }

    /// Build one `<Mixin>_dynamic_<method>` MIR function. See spec §B5.
    fn build_dynamic_dispatch_helper(
        &self,
        mixin_name: &str,
        method_name: &str,
        method_idx: usize,
        sig: &crate::resolve::symbols::FnSignature,
    ) -> MirFunction {
        let fn_name = format!("{}_dynamic_{}", mixin_name, method_name);

        let return_ty = sig.return_ty.clone();
        let mut mir_fn = MirFunction::new(&fn_name, return_ty.clone());

        // Parameter 0: `self` (i64 pointer to a heap object whose
        // slot 0 is the class_info_ptr).
        let self_local = mir_fn.new_local(
            "self",
            Ty::Class {
                name: "__dyn_self".to_string(),
                generic_args: vec![],
            },
            false,
        );
        mir_fn.params.push(self_local);

        // Remaining parameters mirror the mixin method's declared
        // params (excluding self — the mixin signature carries
        // explicit params after self_mode).
        for p in &sig.params {
            let local = mir_fn.new_local(&p.name, p.ty.clone(), false);
            mir_fn.params.push(local);
        }

        let entry = mir_fn.entry_block;

        // class_info = self[0]
        let class_info = mir_fn.new_temp(Ty::Int);
        mir_fn.blocks[entry].instructions.push(MirInst::GetField {
            dest: class_info,
            base: self_local,
            field_index: 0,
        });

        // vtable = class_info[mixin_slot]  — v1: mixin_slot is 0 (single-mixin)
        let vtable = mir_fn.new_temp(Ty::Int);
        mir_fn.blocks[entry].instructions.push(MirInst::GetField {
            dest: vtable,
            base: class_info,
            field_index: 0,
        });

        // method_ptr = vtable[method_idx]
        let method_ptr = mir_fn.new_temp(Ty::Int);
        mir_fn.blocks[entry].instructions.push(MirInst::GetField {
            dest: method_ptr,
            base: vtable,
            field_index: method_idx,
        });

        // Build call args: self followed by the remaining params (all
        // already locals on mir_fn).
        let args: Vec<MirValue> = mir_fn.params.iter().map(|&id| MirValue::Use(id)).collect();

        // Indirect call.
        let has_return = !matches!(return_ty, Ty::Unit);
        let dest = if has_return {
            Some(mir_fn.new_temp(return_ty.clone()))
        } else {
            None
        };
        mir_fn.blocks[entry]
            .instructions
            .push(MirInst::CallIndirect {
                dest,
                callee: method_ptr,
                args,
            });

        mir_fn.blocks[entry].terminator = match dest {
            Some(d) => Terminator::Return(Some(MirValue::Use(d))),
            None => Terminator::Return(None),
        };

        mir_fn
    }

    // ── Helpers ─────────────────────────────────────────────────────────

    /// Phase 2 #06.D2: returns `Some(type_name)` when the HIR program
    /// contains an `impl Display for ty` block whose target resolves to the
    /// given type. Used by `lower_interpolation` (Stage 3) to prefer
    /// user-supplied formatting over the synthesized primitive / derive-Debug
    /// fallbacks.
    ///
    /// Resolves through `Ref` / `RefMut` / `RefLifetime` / `RefMutLifetime`
    /// / `Alias` / `Newtype` so that `&Money` and `Money` both hit the same
    /// impl. Returns `None` for primitive types (Int, Float, …) and any
    /// nominal type whose name is not in the Display impl registry.
    pub(super) fn user_has_impl_display(&self, ty: &Ty) -> Option<String> {
        let name = match ty {
            Ty::Struct { name, .. } | Ty::Class { name, .. } | Ty::Enum { name, .. } => {
                name.clone()
            }
            Ty::Ref(inner)
            | Ty::RefMut(inner)
            | Ty::RefLifetime(_, inner)
            | Ty::RefMutLifetime(_, inner) => return self.user_has_impl_display(inner),
            Ty::Alias { target, .. } => return self.user_has_impl_display(target),
            Ty::Newtype { inner, .. } => return self.user_has_impl_display(inner),
            _ => return None,
        };
        // `self.trait_impls` is populated by `collect_trait_impls` at the
        // start of `lower_program` and maps trait name → Vec<target type name>.
        if let Some(impls) = self.trait_impls.get("Display") {
            if impls.iter().any(|n| n == &name) {
                return Some(name);
            }
        }
        None
    }

    /// Create a named local.
    fn new_local_named(&mut self, name: &str, ty: Ty, mutable: bool) -> LocalId {
        self.current_fn
            .as_mut()
            .expect("no current function")
            .new_local(name, ty, mutable)
    }

    /// Look up the field types for an enum variant from the symbol table.
    ///
    /// Given the enum's DefId and the variant index, returns a vector of
    /// the variant's field types.  For unit variants (no fields), returns
    /// an empty vector.
    fn lookup_variant_field_types(&self, enum_def_id: DefId, variant_idx: usize) -> Vec<Ty> {
        use crate::resolve::symbols::{DefKind, VariantDefKind};
        if let Some(def) = self.symbols.get(enum_def_id) {
            if let DefKind::Enum { ref info } = def.kind {
                if let Some(&variant_def_id) = info.variants.get(variant_idx) {
                    if let Some(variant_def) = self.symbols.get(variant_def_id) {
                        if let DefKind::EnumVariant { ref kind, .. } = variant_def.kind {
                            return match kind {
                                VariantDefKind::Struct(fields) => {
                                    fields.iter().map(|(_, ty)| ty.clone()).collect()
                                }
                                VariantDefKind::Tuple(types) => types.clone(),
                                VariantDefKind::Unit => vec![],
                            };
                        }
                    }
                }
            }
        }
        vec![]
    }

    /// Declared field types of a struct/class, in layout order (parent class
    /// fields prepended, mirroring `get_class_field_names`). Used by the
    /// constructor lowering to coerce each field's initializer value to the
    /// field's declared width BEFORE the width-blind `SetField` store — e.g.
    /// a bare `Float` (f64) literal/local placed into a `Float32` field must
    /// be narrowed to f32 first, or the 8-byte store / 4-byte load disagree
    /// and the slot reads garbage (Q28). Returns an empty Vec for unknown /
    /// non-struct-class type_defs (callers fall back to no coercion).
    fn lookup_construct_field_types(&self, ty: &Ty) -> Vec<Ty> {
        let name = match ty {
            Ty::Struct { name, .. } | Ty::Class { name, .. } => name.clone(),
            _ => return Vec::new(),
        };
        self.construct_field_types_by_name(&name)
    }

    /// Name-keyed field-type walk (mirrors `get_class_field_names`): own field
    /// types in declaration order, with any parent class's fields prepended so
    /// the result is in layout order. Keyed by name rather than DefId because
    /// the same name-based `self.symbols.iter()` walk is how `get_class_field_names`
    /// / `alloc_size` already locate the struct/class definition.
    fn construct_field_types_by_name(&self, name: &str) -> Vec<Ty> {
        use crate::resolve::symbols::DefKind;
        for def in self.symbols.iter() {
            if def.name != name {
                continue;
            }
            match &def.kind {
                DefKind::Struct { info } => {
                    return info
                        .fields
                        .iter()
                        .filter_map(|&fid| self.symbols.def_ty(fid))
                        .collect();
                }
                DefKind::Class { info } => {
                    let mut own: Vec<Ty> = info
                        .fields
                        .iter()
                        .filter_map(|&fid| self.symbols.def_ty(fid))
                        .collect();
                    if let Some(parent_id) = info.parent {
                        if let Some(parent_def) = self.symbols.get(parent_id) {
                            let mut tys = self.construct_field_types_by_name(&parent_def.name);
                            tys.append(&mut own);
                            return tys;
                        }
                    }
                    return own;
                }
                _ => {}
            }
        }
        Vec::new()
    }

    /// Find the parent class name of the function currently being lowered, if
    /// that function belongs to a class (its mangled name is `Class_method`)
    /// and the class has a `< Parent` clause. Used to lower `super(...)` calls
    /// inside child-class constructors.
    fn current_parent_class(&self) -> Option<String> {
        use crate::resolve::symbols::DefKind;
        let fn_name = self.current_fn.as_ref().map(|f| f.name.clone())?;
        // `split('_').next()` is wrong for synth `__`-prefixed classes
        // (returns `""` — the empty prefix) and for class names that
        // themselves carry underscores. `class_name_from_mangled` walks
        // underscore positions right-to-left and returns the longest
        // prefix that names an actual class/struct/enum in the symbol
        // table — the same routine that backs `synthesize_struct_*` /
        // self-typing across the lowerer. Pin:
        // `project_ruxen_mir_mangled_method_name_parsing.md`.
        let class_name = self.class_name_from_mangled(&fn_name)?;
        for def in self.symbols.iter() {
            if def.name == class_name {
                if let DefKind::Class { ref info } = def.kind {
                    let parent_id = info.parent?;
                    let parent_def = self.symbols.get(parent_id)?;
                    return Some(parent_def.name.clone());
                }
            }
        }
        None
    }

    /// Lower an or-pattern made of literal / wildcard alternatives.
    /// Chain equality tests across alternatives; any match jumps to
    /// `match_target`, all failing falls through to `next_block`.
    fn get_class_field_names(&self, class_name: &str) -> Vec<String> {
        use crate::resolve::symbols::DefKind;
        for def in self.symbols.iter() {
            if def.name == class_name {
                if let DefKind::Class { ref info } = def.kind {
                    let mut fields = Vec::new();
                    for &field_id in &info.fields {
                        if let Some(field_def) = self.symbols.get(field_id) {
                            fields.push(field_def.name.clone());
                        }
                    }
                    // Also include parent class fields (prepended, since they come first in layout)
                    if let Some(parent_id) = info.parent {
                        if let Some(parent_def) = self.symbols.get(parent_id) {
                            let mut parent_fields = self.get_class_field_names(&parent_def.name);
                            parent_fields.extend(fields);
                            return parent_fields;
                        }
                    }
                    return fields;
                }
            }
        }
        Vec::new()
    }

    /// Check if `field_name` is an actual field (not a method) on the class
    /// or struct identified by `class_name`.  Returns true only when the
    /// symbol table confirms the field exists.
    fn is_real_field(&self, class_name: &str, field_name: &str) -> bool {
        use crate::resolve::symbols::DefKind;
        // Find the class or struct definition in the symbol table.
        for def in self.symbols.iter() {
            if def.name == class_name {
                match &def.kind {
                    DefKind::Class { info } => {
                        for &field_id in &info.fields {
                            if let Some(field_def) = self.symbols.get(field_id) {
                                if field_def.name == field_name {
                                    return true;
                                }
                            }
                        }
                        // Check parent class fields recursively
                        if let Some(parent_id) = info.parent {
                            if let Some(parent_def) = self.symbols.get(parent_id) {
                                return self.is_real_field(&parent_def.name, field_name);
                            }
                        }
                        return false;
                    }
                    DefKind::Struct { info } => {
                        for &field_id in &info.fields {
                            if let Some(field_def) = self.symbols.get(field_id) {
                                if field_def.name == field_name {
                                    return true;
                                }
                            }
                        }
                        return false;
                    }
                    _ => {}
                }
            }
        }
        false
    }

    /// SINGLE ENTRY POINT for FFI alias-map LOOKUP. Returns `Some(c_symbol)`
    /// when the mangled ruxen-side name has a registered alias (with
    /// the same generic-stripped fallback as
    /// `resolve_ffi_alias_callee`), `None` when no alias exists. Adding
    /// a new call site that needs to know "is this method dispatched
    /// through an FFI symbol?" means calling this function — never
    /// access `self.ffi_alias_map` directly. Spec
    /// `docs/specs/system/compiler_consolidation.spec.md` §B1.
    pub(super) fn lookup_ffi_alias(&self, mangled: &str) -> Option<String> {
        if let Some(direct) = self.ffi_alias_map.get(mangled).cloned() {
            return Some(direct);
        }
        // Generic stripping needs BALANCED bracket matching, not the
        // naive `find('[')` + `find(']')` pair, because nested generics
        // like `Array[any Fn[Fn(Int) -> Int]]_iter` would otherwise
        // peel only the inner `]` and leave a stray `]_iter` tail.
        if let Some(bracket_pos) = mangled.find('[') {
            let bytes = mangled.as_bytes();
            let mut depth: i32 = 0;
            let mut close_pos: Option<usize> = None;
            for (i, &b) in bytes.iter().enumerate().skip(bracket_pos) {
                match b {
                    b'[' => depth += 1,
                    b']' => {
                        depth -= 1;
                        if depth == 0 {
                            close_pos = Some(i + 1);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if let Some(close_pos) = close_pos {
                let stripped = format!("{}{}", &mangled[..bracket_pos], &mangled[close_pos..]);
                if let Some(c) = self.ffi_alias_map.get(&stripped).cloned() {
                    return Some(c);
                }
            }
        }
        None
    }

    /// #06.8 T#17 / T#14: rewrite a mangled builtin-method callee
    /// through `ffi_alias_map`, falling back to a generic-stripped
    /// lookup so call sites that carry surface generic args
    /// (`Array[Int]_push`, `Option[String]_unwrap_or`, ...) reach
    /// the parent-name-keyed alias entries (`Array_push`,
    /// `Option_unwrap_or`) that bootstrap class shells register.
    /// When neither lookup hits, returns the unchanged mangled name —
    /// the same fallback path that lets user-defined non-FFI methods
    /// through unmolested.
    ///
    /// Routes through `lookup_ffi_alias` (the spec-§B1 SINGLE ENTRY
    /// POINT for the lookup). This wrapper preserves the historical
    /// "miss → unchanged-mangled-name" caller surface.
    pub(super) fn resolve_ffi_alias_callee(&self, mangled: String) -> String {
        if let Some(c) = self.lookup_ffi_alias(&mangled) {
            return c;
        }
        // No FFI alias: this may be a real BODY method on an FFI-shell
        // generic builtin class (`Array[Int]_map` → opaque `Array_map`).
        // Strip the balanced generic suffix and, if the stripped name is a
        // recorded lib body method, route to it.
        if let Some(stripped) = strip_generic_suffix(&mangled) {
            if self.lib_body_methods.contains(&stripped) {
                return stripped;
            }
        }
        mangled
    }

    /// Returns `true` if the named method on `class_name` is a static/class
    /// method (declared as `def self.foo`). Checks both inherent methods and
    /// methods defined in impl blocks, then recurses into the parent class.
    fn is_user_static_method(&self, class_name: &str, method_name: &str) -> bool {
        use crate::resolve::symbols::DefKind;
        // Peel generics like `Box[Int]` → `Box`.
        let base = if let Some(pos) = class_name.find('[') {
            &class_name[..pos]
        } else {
            class_name
        };
        // #06.93 Phase 5: a class inside a module is registered in
        // the symbol table under its un-qualified last segment
        // (`File`) but the receiver's `Ty::Class` carries the
        // qualified name (`BufferedThings.File`). Use the LAST
        // dotted segment for the symbol-table scan, and when
        // multiple defs share that base name (the top-level stdlib
        // `class File` + a module-nested `class File`), pick the
        // one that ACTUALLY has the queried method registered
        // under its DefId. Same approach scales to any future
        // collision.
        let lookup_name = if let Some(pos) = base.rfind('.') {
            &base[pos + 1..]
        } else {
            base
        };
        // Collect every Class / Struct / Enum def matching the base name.
        // Structs and enums carry inline `def self.X` statics too
        // (ruby-naming.spec.md §3.4a); their methods are registered as
        // `DefKind::Method` with the struct/enum DefId as parent, exactly
        // like classes. Only classes have an inheritance `parent`, so the
        // struct/enum candidates record `None`. Without including them here,
        // a struct zero-arg static (`C3.white`) was mis-classified as an
        // instance call and a phantom `self` (constant 0) was prepended at
        // the call site, tripping the Cranelift arg-count verifier.
        let mut candidates: Vec<(DefId, Option<DefId>)> = Vec::new();
        for def in self.symbols.iter() {
            if def.name == lookup_name {
                match def.kind {
                    DefKind::Class { ref info } => candidates.push((def.id, info.parent)),
                    DefKind::Struct { .. } | DefKind::Enum { .. } => {
                        candidates.push((def.id, None))
                    }
                    _ => {}
                }
            }
        }
        // Pick the candidate that owns the queried method. If none
        // do, fall back to the first candidate (preserves old
        // behaviour for non-method lookups).
        let (class_def_id, parent_id) = if candidates.is_empty() {
            return false;
        } else if candidates.len() == 1 {
            candidates[0]
        } else {
            candidates
                .iter()
                .copied()
                .find(|(class_id, _)| {
                    self.symbols.iter().any(|m| {
                        m.name == method_name
                            && matches!(&m.kind, DefKind::Method { parent, .. } if parent == class_id)
                    })
                })
                .unwrap_or(candidates[0])
        };
        let parent_name: Option<String> =
            parent_id.and_then(|pid| self.symbols.get(pid).map(|p| p.name.clone()));
        let class_def_id = Some(class_def_id);
        let class_def_id = match class_def_id {
            Some(id) => id,
            None => return false,
        };
        // Scan all methods whose parent matches this class.
        for def in self.symbols.iter() {
            if def.name == method_name {
                if let DefKind::Method {
                    parent,
                    ref signature,
                } = def.kind
                {
                    if parent == class_def_id {
                        return signature.is_class_method;
                    }
                }
            }
        }
        // Walk up the inheritance chain.
        if let Some(parent) = parent_name {
            return self.is_user_static_method(&parent, method_name);
        }
        false
    }

    /// Find the class that owns a given method by searching the class and its
    /// parent chain.  Returns the class name where the method is defined.
    fn resolve_method_class(&self, class_name: &str, method_name: &str) -> String {
        use crate::resolve::symbols::DefKind;
        for def in self.symbols.iter() {
            if def.name == class_name {
                if let DefKind::Class { ref info } = def.kind {
                    // Check methods on this class
                    for &method_id in &info.methods {
                        if let Some(method_def) = self.symbols.get(method_id) {
                            if method_def.name == method_name {
                                return class_name.to_string();
                            }
                        }
                    }
                    // Check parent class
                    if let Some(parent_id) = info.parent {
                        if let Some(parent_def) = self.symbols.get(parent_id) {
                            return self.resolve_method_class(&parent_def.name, method_name);
                        }
                    }
                }
            }
        }
        // Fallback to the original class name
        class_name.to_string()
    }

    /// Find the class that owns the overload selected by the call arguments.
    /// A child overload for `pick(Bool)` must not shadow inherited
    /// `pick(Int)` / `pick(String)` overloads during MIR mangling.
    fn resolve_method_class_with_args(
        &self,
        class_name: &str,
        method_name: &str,
        args: &[HirExpr],
    ) -> String {
        use crate::resolve::symbols::DefKind;

        if self.class_has_method_accepting_args(class_name, method_name, args) {
            return class_name.to_string();
        }

        for def in self.symbols.iter() {
            if def.name == class_name {
                if let DefKind::Class { ref info } = def.kind {
                    if let Some(parent_id) = info.parent {
                        if let Some(parent_def) = self.symbols.get(parent_id) {
                            return self.resolve_method_class_with_args(
                                &parent_def.name,
                                method_name,
                                args,
                            );
                        }
                    }
                }
            }
        }

        self.resolve_method_class(class_name, method_name)
    }
}

/// Strip a single balanced generic suffix from a mangled callee:
/// `Array[Int]_map` → `Array_map`, `Result[Int,IoError]_map` →
/// `Result_map`. Uses balanced bracket matching so nested generics
/// (`Array[any Fn[Fn(Int) -> Int]]_map`) peel the whole outer `[...]`.
/// Returns `None` when there is no `[` to strip.
fn strip_generic_suffix(mangled: &str) -> Option<String> {
    let bracket_pos = mangled.find('[')?;
    let bytes = mangled.as_bytes();
    let mut depth: i32 = 0;
    for (i, &b) in bytes.iter().enumerate().skip(bracket_pos) {
        match b {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(format!("{}{}", &mangled[..bracket_pos], &mangled[i + 1..]));
                }
            }
            _ => {}
        }
    }
    None
}

// ─── Standalone entry point (backward compat) ───────────────────────────────

/// Convenience function: lower an HIR program to MIR.
pub fn lower_program(program: &HirProgram, symbols: &SymbolTable) -> Result<MirProgram, String> {
    let mut lowerer = Lowerer::new(symbols);
    lowerer.lower_program(program)
}
