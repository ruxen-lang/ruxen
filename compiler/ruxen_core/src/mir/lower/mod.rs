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
mod key_desc;
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
    /// Source span of the HIR node currently being lowered. `emit` stamps it
    /// onto each instruction's `BasicBlock::inst_spans` slot so the LLVM
    /// backend can attach DWARF line info (P0.14). Set at the top of
    /// `lower_expr` / `lower_statement` and reset per function in
    /// `lower_method`; `None` before any function body is entered.
    current_span: Option<crate::lexer::token::Span>,
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
    /// Universal-`Kernel` call sites recorded during lowering:
    /// `{Type}_{method}` mangled callee → (concrete receiver `Ty`,
    /// canonical Kernel method name). Every Ruxen value implicitly carries
    /// `mixin Kernel`'s defaults (`tap`/`then`), but a primitive / builtin /
    /// non-`include`-ing receiver has no `impl` item to drive the usual
    /// per-implementor default monomorphization. `emit_universal_kernel_bodies`
    /// drains this map after `lower_items`, synthesizing each `{Type}_tap` /
    /// `{Type}_then` body (cloned Kernel default, `Self` → the concrete
    /// receiver) that a real own/`include` body did not already provide.
    universal_kernel_needed: HashMap<String, (Ty, String)>,
    /// Set transiently around a `lower_method` call to force the implicit
    /// `self` param's type instead of re-deriving it from the mangled name.
    /// `emit_universal_kernel_bodies` uses it: a synthesized callee like
    /// `Array[Int]_tap` names no symbol-table class and no primitive, so the
    /// name-based derivation would fall back to `Ty::Unit` (a zero-sized self
    /// codegen elides — the arg-count mismatch at the call site). The synthesis
    /// already knows the concrete receiver `Ty`, so it supplies it here.
    self_ty_override: Option<Ty>,
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
    /// Per-`T` monomorphization of BUILTIN-HEAD generic classes (`Array[T]`,
    /// `Set[T]`, `Hash[K,V]`, `Option[T]`, `Result[T,E]`): the specializations
    /// to emit, recorded by `collect_builtin_head_mono_instances` from the
    /// receiver instantiations seen at call sites. Drained by
    /// `emit_builtin_head_mono_instances` after `lower_items`.
    builtin_head_mono_instances: Vec<monomorphize::BuiltinHeadMonoInstance>,
    /// The mangled symbols the pass above emits (`Array__mono__String_join`). A
    /// receiver-bearing call site is redirected only when its
    /// `(instantiation, method)` is in here; every other call keeps the opaque
    /// `{Class}_{method}` body reached through `lib_body_methods`.
    builtin_head_mono_emitted: HashSet<String>,
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
    /// Q17b: eligible generic METHODS (methods with at least one
    /// mixin-bounded type parameter), keyed by their resolved DefId. The
    /// stored owner-qualified symbol keeps same-named methods on different
    /// receiver types distinct.
    mono_methods: HashMap<DefId, monomorphize::MonoMethod>,
    /// Q17b: method DefId -> distinct fully-concrete generic-arg vectors
    /// recovered from method-call sites.
    method_mono_instances: HashMap<DefId, Vec<monomorphize::MonoKey>>,
    /// Q17b: method DefId -> emitted `(concrete-args, mangled-symbol)` pairs
    /// consulted by method-call lowering.
    method_mono_emitted: HashMap<DefId, Vec<(monomorphize::MonoKey, String)>>,
    /// Tier 4.04: no_std mode. Suppresses the synthesized primitive `*_fmt`
    /// Display helpers (`Int_fmt`/`Char_fmt`/…), which reference hosted runtime
    /// symbols (`ruxen_int_to_string`, `Formatter_write_str`) absent in a
    /// no_std build. Set via [`Lowerer::new_no_std`]; the default `new` leaves
    /// it `false` so every existing caller is unchanged. Also surfaced on
    /// `MirProgram::no_std` so the cranelift backend skips the `ruxen_env_init`
    /// injection into `main`.
    no_std: bool,
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
            current_span: None,
            closure_counter: 0,
            pending_closures: Vec::new(),
            trait_impls: HashMap::new(),
            loop_stack: Vec::new(),
            cell_promoted: HashSet::new(),
            const_values: HashMap::new(),
            into_impls: HashSet::new(),
            trait_default_methods: HashMap::new(),
            universal_kernel_needed: HashMap::new(),
            self_ty_override: None,
            user_drop_classes: HashSet::new(),
            capture_map: HashMap::new(),
            captures_ptr_local: None,
            initialized_heap_locals: HashSet::new(),
            ffi_alias_map: HashMap::new(),
            lib_body_methods: HashSet::new(),
            mono_classes: HashMap::new(),
            mono_instances: HashMap::new(),
            mono_emitted: HashSet::new(),
            builtin_head_mono_instances: Vec::new(),
            builtin_head_mono_emitted: HashSet::new(),
            mono_fns: HashMap::new(),
            fn_mono_instances: HashMap::new(),
            fn_mono_emitted: HashMap::new(),
            fn_mono_needs_opaque: HashSet::new(),
            mono_methods: HashMap::new(),
            method_mono_instances: HashMap::new(),
            method_mono_emitted: HashMap::new(),
            no_std: false,
        }
    }

    /// Construct a no_std lowerer (tier 4.04): suppresses the synthesized
    /// primitive `*_fmt` Display helpers and marks `MirProgram::no_std`.
    pub fn new_no_std(symbols: &'a SymbolTable) -> Self {
        let mut lo = Self::new(symbols);
        lo.no_std = true;
        lo
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

    /// Q17/Q17b residual guard: true when the receiver type (after peeling refs)
    /// is ITSELF a still-abstract, mixin-bound type parameter
    /// (`TypeParam`/`SomeMixin`/`AnyMixin`) with bounds but NO unique
    /// implementor — so a method call on it would mangle to a
    /// bound-placeholder callee (`T: Sized_width`) that link-fails. Normal Q17
    /// free-function and Q17b method specialization makes this concrete first;
    /// a remaining abstract receiver is an unsupported generic-chain residual.
    /// Detected on the receiver SHAPE, not on the
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
                    // A by-value argument satisfies a `&T`/`&mut T` parameter
                    // of the same underlying type (the call site auto-refs), so
                    // a `String` literal/value matches a `&String` parameter —
                    // the arm that lets an `add("static")` call bind the right
                    // overload instead of falling through to the arity-only
                    // fallback and binding the FIRST-declared (closure) overload
                    // (the old `&str`-vs-closure heap-corruption landmine, now
                    // dissolved — there is no `&str` arm to collide). Mirrors
                    // the typeck selector in
                    // `typeck/infer/collect.rs::method_accepts_args`.
                    || matches!(
                        &param.ty,
                        Ty::Ref(inner) | Ty::RefMut(inner) if **inner == arg.ty
                    )
            })
    }

    /// Every method def on `class_name` whose name is `method_name` or one of
    /// its `<method_name>__overloadN` siblings, paired with its signature.
    ///
    /// Q58: the scan runs against the receiver's exact class name first and,
    /// only when that yields nothing, against its generic-free base
    /// (`Hash[String, Int]` → `Hash`). Symbol-table method parents are keyed by
    /// the BARE class name, while a call site on a builtin collection head
    /// carries the receiver's INSTANTIATED spelling (`Ty::Map` Displays as
    /// `Hash[K, V]`, `Ty::Array` as `Array[T]`), so those heads matched nothing
    /// at all on the exact-name pass and every lookup through here came back
    /// empty. This mirrors the full-then-base lookup order
    /// `SymbolTable::canonical_method_name` already uses for the alias map, so
    /// the two agree on what "this class" means.
    ///
    /// A USER generic class does NOT reach here suffixed: its `Ty::Class` keeps
    /// the bare `name` with the generic args in a separate field, so `Box[Int]`
    /// arrives as `Box` and always hit on the exact pass.
    ///
    /// The base pass is strictly a FALLBACK, never a widening: an exact hit
    /// wins outright, so a non-generic receiver resolves exactly as it did
    /// before this existed.
    fn method_defs_on_class(
        &self,
        class_name: &str,
        method_name: &str,
    ) -> Vec<(String, crate::resolve::symbols::FnSignature)> {
        let base = class_name.split('[').next().unwrap_or(class_name);
        let overload_prefix = format!("{}__overload", method_name);
        let scan = |owner: &str| {
            let mut found = Vec::new();
            for def in self.symbols.iter() {
                let crate::resolve::symbols::DefKind::Method { parent, signature } = &def.kind
                else {
                    continue;
                };
                let Some(parent_def) = self.symbols.get(*parent) else {
                    continue;
                };
                if parent_def.name == owner
                    && (def.name == method_name || def.name.starts_with(&overload_prefix))
                {
                    found.push((def.name.clone(), signature.clone()));
                }
            }
            found
        };
        let exact = scan(class_name);
        if !exact.is_empty() || base == class_name {
            return exact;
        }
        scan(base)
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
        let candidates = self.method_defs_on_class(class_name, method_name);
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
        // already-resolved `class_name`). Shares `select_method_symbol_name`'s
        // scan so a generic-suffixed receiver (`Hash[String, Int]`) finds the
        // signature declared on the bare class. Before Q58 this site returned
        // an empty vector for EVERY generic builtin head, so a paren-less call
        // to a method with trailing defaults appended no sentinels and emitted
        // one argument too few, aborting in the Cranelift verifier with
        // `mismatched argument count ... got 1, expected 2`.
        let Some((_, signature)) = self
            .method_defs_on_class(class_name, method_name)
            .into_iter()
            .next()
        else {
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
            .filter_map(|p| p.default.as_ref().map(Self::default_expr_to_sentinel))
            .collect()
    }

    /// Lower a param `default` AST expr to the MIR value the no-arg
    /// field-access path appends. `nil`/`null` → `Literal::Int(0)` (the null
    /// closure-pair-pointer for a `&block` slot, matching `NullLiteral`'s
    /// non-`Option` lowering). Other literal defaults map to their value.
    fn default_expr_to_sentinel(default: &crate::parser::ast::Expr) -> MirValue {
        use crate::parser::ast::ExprKind;
        match &default.kind {
            ExprKind::NullLiteral | ExprKind::UnitLiteral => MirValue::Literal(Literal::Int(0)),
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
                } else if !func.is_class_method && module_path.is_empty() {
                    // Tier 4.03 (WASM): record top-level free defs as wasm
                    // export candidates. A `wasm32-unknown-unknown` module is
                    // a *reactor* (spec §5.3) — it has no `main`; its top-level
                    // free functions ARE the host-callable API surface, so we
                    // export them by source name. The LLVM backend sets the
                    // `export_name` attribute on these ONLY when targeting
                    // wasm32; every other codegen path ignores the field. (Top-
                    // level `def` is `Private` by default in Ruxen and there is
                    // no `pub` at file scope, so a visibility gate would export
                    // nothing — the reactor surface is the whole free-fn set.
                    // A `wasm_export "custom"` rename/opt-in directive is the
                    // staged remainder — see docs/decisions/phase4-no-std-wasm.md.)
                    // Generic free fns are suppressed above (opaque body never
                    // reaches here), so only concrete callables are recorded.
                    mir.wasm_exports.push(mir_fn.name.clone());
                }
                mir.functions.push(mir_fn);
            }
            HirItem::Class(class) => {
                let qualified_class = Self::qualified_item_name(module_path, &class.name);
                let class_symbol = Self::symbol_name(&qualified_class);
                for method in &class.methods {
                    // Q17b: an eligible generic method's abstract body could
                    // only dispatch through a mixin-bound placeholder. Calls
                    // target emitted concrete copies instead.
                    if !self.generic_method_keeps_opaque(method.def_id) {
                        continue;
                    }
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
                // B74: a class gets the same structural derives a struct
                // does. Before this, the arm synthesised Clone alone, so
                // `a == b` compared heap addresses (silently `false`),
                // `"#{a}"` printed one, and `a.hash_code` type-checked
                // and then failed at LINK with an undefined
                // `{Class}_hash_code`.
                //
                // `module_path.is_empty()` matches Clone's existing
                // guard and `key_desc.rs`'s dotted-name skip: the
                // synthesised symbols mangle from the BARE `class.name`
                // while a module-nested use site's `Ty::Class` carries
                // the qualified one, so emitting for a nested class
                // would define a symbol no call site names.
                //
                // Ord / PartialOrd / Default are deliberately NOT here.
                // They would need `{Class}_cmp` / `{Class}_default`
                // bodies, which do not exist; the matching dispatch
                // helpers stay struct-only to keep that a silent gap
                // rather than a link error. Add a synthesiser and its
                // dispatch site together, or neither.
                if module_path.is_empty() {
                    if class_has("Clone") {
                        mir.functions.push(self.synthesize_class_clone(class));
                    }
                    if class_has("Debug") {
                        mir.functions.push(self.synthesize_class_to_debug(class));
                    }
                    if class_has("PartialEq") {
                        mir.functions.push(self.synthesize_class_eq(class));
                    }
                    if class_has("Hashable") || class_has("Hash") {
                        mir.functions.push(self.synthesize_class_hash_code(class));
                    }
                }

                // §4.1 Increment 2: state-indexed drop glue. A synthesised
                // async `Future` state machine owns its awaited sub-futures as
                // `__sub_i` fields; Ruxen has NO automatic per-field drop, so
                // without an explicit `def drop` those sub-futures leak when the
                // future is drained/cancelled (Increment 1 only gave such a
                // future a `_drop_noop`). When this class is a synthesised future
                // state machine owning sub-futures, synthesise its `<Class>_drop`
                // to drop them and register it so `collect_mixin_vtables` points
                // the Drop vtable slot at the REAL drop instead of the no-op.
                //
                // Use the SYMBOL-TABLE class name (`def.name`) for the function
                // symbol + the `user_drop_classes` key — IDENTICAL to what
                // `collect_mixin_vtables` keys on (`def.name`) — so the Drop
                // vtable's `{name}_drop` method symbol and the synthesised
                // function symbol always agree, even for a (hypothetical) module-
                // nested state machine. (Today async state machines are always
                // top-level, so `class.name == def.name == class_symbol`.)
                let sm_symbol = self
                    .symbols
                    .get(class.def_id)
                    .map(|d| d.name.clone())
                    .unwrap_or_else(|| class.name.clone());
                if let Some(drop_fn) = self.synthesize_future_state_machine_drop(class, &sm_symbol)
                {
                    self.user_drop_classes.insert(sm_symbol);
                    mir.functions.push(drop_fn);
                }
            }
            HirItem::Struct(s) => {
                let qualified_struct = Self::qualified_item_name(module_path, &s.name);
                let struct_symbol = Self::symbol_name(&qualified_struct);
                for method in &s.methods {
                    if !self.generic_method_keeps_opaque(method.def_id) {
                        continue;
                    }
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
                    if !self.generic_method_keeps_opaque(method.def_id) {
                        continue;
                    }
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
        mir.no_std = self.no_std;

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

        // Per-`T` monomorphization of the BUILTIN-HEAD generic classes the
        // rule above excludes (`Array[T]` & friends carry `lib` blocks): record
        // one specialization per `(concrete receiver instantiation, method)`
        // for each `.rx` body method that dispatches on its element type, so
        // `["a", "b"].join` formats the elements instead of printing their
        // buffer addresses through the pointer-as-`Int` floor. Must run after
        // `collect_mono_instances` (it reuses the same mangling) and before
        // `lower_items` (call sites consult the emitted set while bodies lower).
        self.collect_builtin_head_mono_instances(program);

        // Q17: record every concrete instantiation of an eligible generic
        // FREE function (mixin-bounded type param) seen at a call site, so a
        // dependency's generic (`paint_all[T: Paintable]`) can be specialized
        // per CONSUMER implementor instead of emitting the bound-placeholder
        // callee (`T: Paintable_fill_rect`) that link-fails. Runs after
        // `collect_mono_instances` (the trait-impl table is already built).
        self.collect_generic_fn_instances(program);

        // Q17b: collect concrete instantiations of generic methods over
        // mixin bounds. This is deliberately separate from generic-class
        // monomorphization: the receiver type is fixed; only the method's own
        // `[T: Mixin]` parameters vary per call.
        self.collect_generic_method_instances(program);

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

        // Per-`T` builtin-head specializations (`Array__mono__String_join`).
        // Same ordering rationale as `emit_mono_instances`: the opaque body
        // every un-specialized instantiation still resolves to was emitted by
        // `lower_items` above.
        self.emit_builtin_head_mono_instances(&mut mir)?;

        // Q17: emit one specialized MIR copy of every eligible generic free
        // function, per recorded concrete instantiation, and record the
        // `(fn, args) → mangled` redirects consulted by `fn_call.rs`. Same
        // ordering rationale as `emit_mono_instances`; eligible opaque generic
        // bodies themselves were deliberately suppressed above.
        self.emit_generic_fn_instances(&mut mir)?;

        // Q17b: emit the specialized method bodies after ordinary items, in
        // parallel with the free-function specialization pass above.
        self.emit_generic_method_instances(&mut mir)?;

        // Emit the primitive Display::fmt synth functions (Phase 2 #06.D2.S1).
        // These are program-level, not per-use. Stage 3 (D2)
        // `lower_interpolation` rewrite assumes they're present in a hosted
        // build. SUPPRESSED in no_std (tier 4.04): each one references hosted
        // runtime (`ruxen_int_to_string`, `Formatter_write_str`) absent in a
        // no_std link, and a no_std unit cannot interpolate anyway (E1400
        // rejects the `String` an interpolation would build).
        if !self.no_std {
            for f in self.synthesize_primitive_fmt_displays() {
                mir.functions.push(f);
            }
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
            // B73: a closure body constructs tables too — same post-pass,
            // same reason. See `key_desc::install_key_descriptors`.
            key_desc::install_key_descriptors(&mut closure_fn, self.symbols);
            crate::mir::lower::drops::insert_drops(
                &mut closure_fn,
                &return_locals,
                self.symbols,
                &self.user_drop_classes,
                &|mangled| self.resolve_ffi_alias_callee(mangled),
            );
            mir.functions.push(closure_fn);
        }

        // Universal `Kernel` (`tap`/`then`): synthesize `{Type}_{method}`
        // bodies for every primitive / builtin / non-`include`-ing receiver
        // that called one — the receivers that have no `impl` item to drive
        // the per-implementor default monomorphization above. Runs AFTER the
        // closure drain so `tap`/`then` calls INSIDE a `do…end` body (recorded
        // while that closure was lowered) are covered too. See
        // `emit_universal_kernel_bodies`.
        self.emit_universal_kernel_bodies(&mut mir)?;

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

        // §4.1 NARROW: runtime-dispatch classes with no user `def drop`
        // that still need a no-op drop fn for their uniform Drop vtable
        // slot. Collected during the class walk, synthesised after it.
        let mut drop_noop_classes: Vec<String> = Vec::new();

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

            // §4.1 NARROW cancellation path: every FUTURE class (a class
            // whose runtime-dispatch includes the `Future` async-protocol
            // mixin — the only runtime-dispatch class the scheduler ever
            // queues + drains) gets an extra `Drop` vtable appended INSIDE
            // its existing class_info struct, at the slot AFTER the real
            // runtime-dispatch mixins (slot 1, after `Future`@0). The Drop
            // vtable's single entry is the concrete `<Class>_drop` when the
            // class defines `def drop`, or a synthesised no-op
            // `<Class>_drop_noop` otherwise.
            //
            // The slot is UNIFORM across all futures so the SINGLE
            // `Drop_dynamic_drop(self)` helper the C drain calls can always
            // read a valid fn pointer from `class_info[1]` — including
            // futures with no user `def drop` (e.g. `PayloadFuture`,
            // `__GFuture`), which the drain would otherwise dereference an
            // out-of-bounds slot for. The no-op makes the drain a safe
            // no-op for those.
            //
            // Gated on the `Future` OR `Stream` async-protocol mixins (NOT all
            // runtime-dispatch classes): a generic non-async runtime-dispatch
            // class — e.g. a `Sized dispatch runtime` user mixin — is never
            // drained/cancelled and is not the inner of an owning adapter, so
            // adding a Drop slot to it would only widen its class_info for no
            // consumer. `Future` and `Stream` are the two async *protocol*
            // markers (structural names async_lowering / stream adapters key
            // on), not per-type stdlib knowledge.
            //   - `Future` (§4.1): the scheduler drains/cancels queued futures
            //     via `Drop_dynamic_drop`.
            //   - `Stream` (§4.5 Increment 4): a stream ADAPTER owns its inner
            //     `any Stream` by value and runs the inner's `def drop` via
            //     `Drop_dynamic_drop` when the adapter drops — so dropping an
            //     adapter chain drops every stage (the spec's leak-free-on-
            //     cancel guarantee). Without a Drop slot on Stream classes the
            //     adapter's inner source would leak its reactor resource.
            //
            // Both put their Drop vtable at the SAME class_info slot — each has
            // exactly ONE real runtime-dispatch mixin (`Future`@0 / `Stream`@0),
            // so Drop lands uniformly at slot 1 (`drop_vtable_slot_for_class`
            // returns `runtime_dispatch_includes.len()` == 1 for both). The
            // single `Drop_dynamic_drop` helper reads that uniform slot; the
            // hard singleton-slot assert (below) stays satisfied.
            //
            // This routes a future's drain/cancel drop AND an adapter's
            // inner-drop through the class_info each already carries, WITHOUT
            // making `mixin Drop` globally `dispatch runtime` and WITHOUT adding
            // a header to any non-runtime-dispatch class.
            // Spec: docs/decisions/zio-comparison-concurrency-adoption.md
            // §4.1 + §4.5; docs/dev/concurrency-impl-plan.md Increment 1 + F5/Inc4.
            let is_async_protocol = info.runtime_dispatch_includes.iter().any(|&m| {
                self.symbols
                    .get(m)
                    .map(|d| d.name == "Future" || d.name == "Stream")
                    .unwrap_or(false)
            });
            if is_async_protocol {
                let class_has_user_drop = self.user_drop_classes.contains(&class_name)
                    || self
                        .user_drop_classes
                        .iter()
                        .any(|q| q.ends_with(&format!(".{}", class_name)));
                let drop_method_sym = if class_has_user_drop {
                    format!("{}_drop", class_name)
                } else {
                    // No user `def drop` → point the slot at a synthesised
                    // no-op so the uniform-slot drain dispatch stays valid.
                    drop_noop_classes.push(class_name.clone());
                    format!("{}_drop_noop", class_name)
                };
                let drop_vt = crate::mir::nodes::MirVtable {
                    mixin_name: "Drop".to_string(),
                    class_name: class_name.clone(),
                    method_symbols: vec![drop_method_sym],
                };
                class_vtable_syms.push(drop_vt.symbol());
                mir.vtables.push(drop_vt);
            }

            mir.class_infos.push(crate::mir::nodes::MirClassInfo {
                class_name,
                vtable_symbols: class_vtable_syms,
            });
        }

        // Synthesise a no-op `<Class>_drop_noop(self) -> nil` for each
        // runtime-dispatch class lacking a user `def drop`, so the Drop
        // vtable slot above resolves to a real (trivial) symbol.
        for class_name in drop_noop_classes {
            mir.functions.push(self.build_drop_noop(&class_name));
        }
    }

    /// Build a trivial `<Class>_drop_noop(self) -> nil` whose body is an
    /// immediate `return`. Backs the uniform Drop vtable slot for a
    /// FUTURE that defines no user `def drop`, so the single
    /// `Drop_dynamic_drop` helper always resolves a valid fn pointer.
    ///
    /// SCOPE LIMIT (Increment 1): the no-op does NOT drop the future's
    /// owned FIELDS / sub-futures. For a future that owns a droppable field
    /// but declares no `def drop` (e.g. a synthesised `async def` state
    /// machine that hoisted a droppable cross-suspend local), draining it
    /// runs this no-op and that field's `def drop` does NOT fire — the same
    /// leak as before this increment (drain previously fired NO drop at
    /// all), so this is no regression. Synthesising per-state field-drop
    /// glue for such futures is **Increment 2** (state-indexed drop glue,
    /// docs/dev/concurrency-impl-plan.md). The no-op is the honest,
    /// documented Increment-1 boundary, not a claim of complete cleanup.
    /// §4.1 NARROW.
    fn build_drop_noop(&self, class_name: &str) -> MirFunction {
        let fn_name = format!("{}_drop_noop", class_name);
        let mut mir_fn = MirFunction::new(&fn_name, Ty::Unit);
        let self_local = mir_fn.new_local(
            "self",
            Ty::Class {
                name: class_name.to_string(),
                generic_args: vec![],
            },
            false,
        );
        mir_fn.params.push(self_local);
        let entry = mir_fn.entry_block;
        mir_fn.blocks[entry].terminator = Terminator::Return(None);
        mir_fn
    }

    /// §4.1 Increment 2 — state-indexed drop glue for a synthesised async
    /// `Future` state machine.
    ///
    /// Returns `Some((symbol_name, <Class>_drop))` when `class` is a synthesised
    /// future state machine that owns droppable `__sub_i` sub-future fields, else
    /// `None` (the caller then leaves the Increment-1 `_drop_noop` in place).
    /// `symbol_name` is the module-qualified class name — IDENTICAL to the name
    /// `collect_mixin_vtables` derives from the symbol table — so the Drop vtable
    /// slot and the emitted `<symbol>_drop` agree even for a (hypothetical)
    /// module-nested state machine.
    ///
    /// Ruxen has NO automatic per-field drop (see drops.rs /
    /// drop-elaboration.md), so this destructor drops each owned sub-future
    /// EXPLICITLY: a `__sub_i` is itself a `Future` (it carries the Drop vtable
    /// slot Increment 1 added), so we drop it via the same
    /// `Drop_dynamic_drop(self.__sub_i)` helper the C drain uses — its concrete
    /// `<SubClass>_drop` runs (idempotent for a completed sub-future, e.g.
    /// TimeSleepFuture guards on `handle != 0`) — then `ruxen_dealloc` frees its
    /// heap block.
    ///
    /// SCOPE (Increment 2): we drop ONLY the `__sub_i` sub-future fields. These
    /// are the resources that actually leak on drain/cancel (a parked sub-future
    /// owns the reactor timer / fd / read-state) and they are the ONLY fields we
    /// can prove are EAGERLY initialised at any `self.__state`: the state machine
    /// `init` constructs every `__sub_i` at `.new()` time (`lower.rs` init body),
    /// so dropping them is safe at every state. We deliberately do NOT drop
    /// bare-named class fields here: a binding-result field (the `r` in
    /// `let r = make().await`) is a non-`__` name with no synthesisable default,
    /// so it holds UNINITIALISED memory until its await resolves — dropping it
    /// while parked at an earlier state is UB (partial-init / spec §4.1 risk 2).
    /// A pre-await crossing local IS eagerly inited and could be dropped, but at
    /// MIR a crossing local is name-indistinguishable from a binding-result
    /// field, so dropping cross-suspend user-drop locals needs the eager-vs-late
    /// metadata threaded from async-lowering — tracked as a follow-up.
    ///
    /// Double-drop (risk 1): `<Class>_drop` is the SOLE dropper of `__sub_i` —
    /// the poll body only BORROWS the field (`(&var self.__sub_i).poll(cx)`); the
    /// pump never drops a completed future (`scheduler.c`); a spawned state
    /// machine is tainted out of scope-exit drop (`task_spawn_move_by_ffi`); a
    /// block_on root is scope-exit-dropped exactly once. So `__state` is not even
    /// needed as a discriminant for the eager `__sub_i` set: each runs once.
    fn synthesize_future_state_machine_drop(
        &self,
        class: &crate::hir::nodes::HirClassDef,
        symbol_name: &str,
    ) -> Option<MirFunction> {
        use crate::resolve::symbols::DefKind;

        // Must be a FUTURE (runtime-dispatch via the `Future` mixin) — the
        // only runtime-dispatch class the scheduler queues/drains.
        let info = match self.symbols.get(class.def_id) {
            Some(d) => match &d.kind {
                DefKind::Class { info } => info,
                _ => return None,
            },
            None => return None,
        };
        let is_future = info.runtime_dispatch_includes.iter().any(|&m| {
            self.symbols
                .get(m)
                .map(|d| d.name == "Future")
                .unwrap_or(false)
        });
        if !is_future {
            return None;
        }
        // Synthesised state machines carry a `__state` field. A hand-written
        // future (TimeSleepFuture, etc.) defines its OWN `def drop` and is not
        // touched here.
        let has_state = class.fields.iter().any(|f| f.name == "__state");
        if !has_state {
            return None;
        }
        let class_has_user_drop_method = class.methods.iter().any(|m| m.name == "drop")
            || class.impl_blocks.iter().any(|b| {
                b.items.iter().any(|it| {
                    matches!(it, crate::hir::nodes::HirImplItem::Method(m) if m.name == "drop")
                })
            });
        if class_has_user_drop_method {
            // A synthesised machine never authors its own `def drop`; if one is
            // present this isn't ours to synthesise — bail.
            return None;
        }

        // Collect the droppable `__sub_i` sub-future fields, in declaration
        // order, with their MIR field index (after the runtime-dispatch
        // class_info header shift). ONLY `__sub_i` futures — see SCOPE note.
        let shift = self.class_field_index_shift(&class.name);
        let mut sub_future_idxs: Vec<usize> = Vec::new();
        for (decl_idx, field) in class.fields.iter().enumerate() {
            let mir_idx = decl_idx + shift;
            let field_class = match &field.ty {
                Ty::Class { name, .. } => name.clone(),
                _ => continue, // primitives / params / binding scalars: nothing to drop
            };
            if field.name.starts_with("__sub_") && self.class_is_future(&field_class) {
                sub_future_idxs.push(mir_idx);
            }
        }

        if sub_future_idxs.is_empty() {
            // No owned sub-futures — the Increment-1 no-op is correct and
            // cheaper. Leave it.
            return None;
        }

        // Build `<symbol>_drop(self) -> nil`. Drop in REVERSE declaration order
        // (LIFO, matching sync RAII / drops.rs).
        //
        // NULL-GUARD per sub-future (partial-init / spec §4.1 risk 2): a `__sub_i`
        // is NOT always live. LINEAR state machines construct every `__sub_i`
        // eagerly in `init`, but LOOP state machines (`lower_loop_via_cfg`)
        // construct `__sub_i` LAZILY inside `poll` (init only zeroes
        // `__sub_ready_i`), so a loop future drained BEFORE its first pump has a
        // ZERO `__sub_i`. Calling `Drop_dynamic_drop(0)` would null-deref the
        // class_info load. We therefore guard each drop on `self.__sub_i != 0`,
        // dropping only constructed sub-futures — sound for BOTH shapes (and
        // matching how hand-written futures guard, e.g. TimeSleepFuture's
        // `if self.handle != 0`). A zero field is an un-constructed sub-future
        // that owns no resource, so skipping it leaks nothing.
        let fn_name = format!("{}_drop", symbol_name);
        let mut mir_fn = MirFunction::new(&fn_name, Ty::Unit);
        let self_local = mir_fn.new_local(
            "self",
            Ty::Class {
                name: symbol_name.to_string(),
                generic_args: vec![],
            },
            false,
        );
        mir_fn.params.push(self_local);

        // Chain a guarded drop block per sub-future. `cur` is the block that
        // performs the next check; it always ends in a Branch into a drop block
        // (taken when the field is non-NULL) or the continue block.
        let mut cur = mir_fn.entry_block;
        for mir_idx in sub_future_idxs.iter().rev() {
            // sub = self.__sub_i  (opaque pointer; Drop_dynamic_drop takes i64)
            let field_local = mir_fn.new_temp(Ty::Int);
            mir_fn.blocks[cur].instructions.push(MirInst::GetField {
                dest: field_local,
                base: self_local,
                field_index: *mir_idx,
            });
            // is_live = (sub != 0)
            let is_live = mir_fn.new_temp(Ty::Bool);
            mir_fn.blocks[cur].instructions.push(MirInst::Compare {
                dest: is_live,
                op: CmpOp::NotEq,
                lhs: MirValue::Use(field_local),
                rhs: MirValue::Literal(Literal::Int(0)),
            });
            let drop_block = mir_fn.new_block();
            let cont_block = mir_fn.new_block();
            mir_fn.blocks[cur].terminator = Terminator::Branch {
                cond: MirValue::Use(is_live),
                then_block: drop_block,
                else_block: cont_block,
            };
            // drop_block: Drop_dynamic_drop(sub) then free its heap block.
            mir_fn.blocks[drop_block].instructions.push(MirInst::Call {
                dest: None,
                callee: "Drop_dynamic_drop".to_string(),
                args: vec![MirValue::Use(field_local)],
            });
            mir_fn.blocks[drop_block].instructions.push(MirInst::Call {
                dest: None,
                callee: "ruxen_dealloc".to_string(),
                args: vec![MirValue::Use(field_local)],
            });
            mir_fn.blocks[drop_block].terminator = Terminator::Goto(cont_block);
            cur = cont_block;
        }

        mir_fn.blocks[cur].terminator = Terminator::Return(None);
        Some(mir_fn)
    }

    /// True when the named class is a `Future` (its `runtime_dispatch_includes`
    /// contains the `Future` mixin). Used to route a sub-future field's drop
    /// through `Drop_dynamic_drop`. §4.1 Increment 2.
    fn class_is_future(&self, class_name: &str) -> bool {
        use crate::resolve::symbols::DefKind;
        for def in self.symbols.iter() {
            if def.name == *class_name {
                if let DefKind::Class { info } = &def.kind {
                    return info.runtime_dispatch_includes.iter().any(|&m| {
                        self.symbols
                            .get(m)
                            .map(|d| d.name == "Future")
                            .unwrap_or(false)
                    });
                }
            }
        }
        false
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

                // The real runtime-dispatch mixins occupy class_info
                // slots 0..N in `runtime_dispatch_includes` order. v1
                // ships exactly one (`Future` at slot 0), so the slot
                // equals the mixin's index in that list. `Future` is
                // always slot 0 today; the lookup keeps this honest if
                // a second real runtime-dispatch mixin is ever added.
                let mixin_slot = self.runtime_dispatch_mixin_slot(mixin_def_id).unwrap_or(0);
                let helper = self.build_dynamic_dispatch_helper(
                    &mixin_name,
                    method_name,
                    mixin_slot,
                    method_idx,
                    &sig,
                );
                mir.functions.push(helper);
            }
        }

        // §4.1 NARROW: synthesise the SINGLE `Drop_dynamic_drop(self)` if
        // ANY future emitted a `Drop` vtable above. The Drop vtable sits at
        // the class_info slot AFTER all real runtime-dispatch mixins — slot
        // 1 for every future (Future at 0). One helper serves all futures
        // ONLY because they share that slot index: today every future has
        // exactly one real runtime-dispatch mixin (`Future`), so the Drop
        // slot is uniformly 1. We compute the set of distinct slots and
        // assert it is a singleton — if a future ever carries a second real
        // runtime-dispatch mixin (Drop@2), one shared helper would dispatch
        // the wrong slot and this assert fires (telling us to key the helper
        // per-slot). The drop method has a uniform `(self) -> nil` shape, so
        // no signature recovery from the symbol table is needed.
        // All `Drop` vtables today belong to FUTURE classes (we only append a
        // Drop slot for futures — `collect_mixin_vtables`). If a non-future Drop
        // vtable were ever added, `drop_vtable_slot_for_class` would return its
        // `runtime_dispatch_includes.len()` (0 for a non-future), diverging from
        // futures' slot 1 → the hard assert below trips loudly. So scanning all
        // Drop vtables is safe: a stray non-future entry cannot silently
        // miscompile, it fails the singleton-slot assert.
        let drop_slots: std::collections::BTreeSet<usize> = mir
            .vtables
            .iter()
            .filter(|vt| vt.mixin_name == "Drop")
            .map(|vt| self.drop_vtable_slot_for_class(&vt.class_name))
            .collect();
        // HARD assert (not debug_assert): a release build must NOT silently
        // miscompile if this invariant is ever violated. One shared
        // `Drop_dynamic_drop` reads a FIXED class_info slot, so it is only
        // correct when every future puts its Drop vtable at the SAME slot.
        // Today every future has exactly one real runtime-dispatch mixin
        // (`Future`@0), so Drop is uniformly @1. If a future ever carries a
        // second runtime-dispatch mixin (v2 — spec "out of scope"), Drop
        // lands at a different slot and the single helper would dispatch the
        // wrong slot (out-of-bounds read on the lower-slot futures). When
        // that day comes, key `Drop_dynamic_drop` per-slot; until then fail
        // loudly rather than miscompile.
        assert!(
            drop_slots.len() <= 1,
            "Drop_dynamic_drop assumes a uniform Drop class_info slot across the \
             async-protocol (Future/Stream) classes, but found multiple: {:?}. A \
             class now carries >1 real runtime-dispatch mixin (e.g. `include \
             Future` AND `include Stream` on one class — semantically incoherent: \
             a value can't be both a one-shot future and a pull stream), so its \
             Drop slot diverges from the uniform slot 1. Reject that combination \
             upstream, or key `Drop_dynamic_drop` per slot \
             (see docs/dev/concurrency-impl-plan.md Increment 1 + F5/Inc4).",
            drop_slots
        );
        if let Some(&drop_slot) = drop_slots.iter().next_back() {
            mir.functions
                .push(self.build_drop_dynamic_helper(drop_slot));
        }
    }

    /// The class_info slot index where the `Drop` vtable lives for a
    /// runtime-dispatch class: after all real runtime-dispatch mixins.
    /// For every async future this is 1 (Future occupies slot 0).
    fn drop_vtable_slot_for_class(&self, class_name: &str) -> usize {
        use crate::resolve::symbols::DefKind;
        for def in self.symbols.iter() {
            if def.name == *class_name {
                if let DefKind::Class { info } = &def.kind {
                    return info.runtime_dispatch_includes.len();
                }
            }
        }
        // Defensive: a Drop vtable was emitted for this class only
        // because it is a runtime-dispatch class, so the lookup should
        // always succeed; fall back to slot 1 (the future layout).
        1
    }

    /// Slot index of a real runtime-dispatch mixin within a class's
    /// class_info. Returns the position of `mixin_def_id` across all
    /// runtime-dispatch classes (the order is stable per spec §B8); v1
    /// ships a single such mixin (`Future`) so this is always 0.
    fn runtime_dispatch_mixin_slot(&self, mixin_def_id: DefId) -> Option<usize> {
        use crate::resolve::symbols::DefKind;
        for def in self.symbols.iter() {
            if let DefKind::Class { info } = &def.kind {
                if let Some(idx) = info
                    .runtime_dispatch_includes
                    .iter()
                    .position(|&m| m == mixin_def_id)
                {
                    return Some(idx);
                }
            }
        }
        None
    }

    /// Build `Drop_dynamic_drop(self) -> nil`: read the Drop vtable from
    /// `self`'s class_info at `drop_slot`, then call its single
    /// `<Class>_drop(self)` entry. Mirrors `build_dynamic_dispatch_helper`
    /// but is hand-rolled because `Drop` is not a runtime-dispatch mixin
    /// (no `DefKind::Method` contract to recover the signature from) and
    /// the drop method shape is the fixed `(self) -> nil`. §4.1 NARROW.
    fn build_drop_dynamic_helper(&self, drop_slot: usize) -> MirFunction {
        let mut mir_fn = MirFunction::new("Drop_dynamic_drop", Ty::Unit);

        let self_local = mir_fn.new_local(
            "self",
            Ty::Class {
                name: "__dyn_self".to_string(),
                generic_args: vec![],
            },
            false,
        );
        mir_fn.params.push(self_local);

        let entry = mir_fn.entry_block;

        // class_info = self[0]
        let class_info = mir_fn.new_temp(Ty::Int);
        mir_fn.blocks[entry].instructions.push(MirInst::GetField {
            dest: class_info,
            base: self_local,
            field_index: 0,
        });

        // drop_vtable = class_info[drop_slot]
        let drop_vtable = mir_fn.new_temp(Ty::Int);
        mir_fn.blocks[entry].instructions.push(MirInst::GetField {
            dest: drop_vtable,
            base: class_info,
            field_index: drop_slot,
        });

        // drop_ptr = drop_vtable[0]  (single-method Drop vtable)
        let drop_ptr = mir_fn.new_temp(Ty::Int);
        mir_fn.blocks[entry].instructions.push(MirInst::GetField {
            dest: drop_ptr,
            base: drop_vtable,
            field_index: 0,
        });

        // drop_ptr(self) — result discarded (drop returns nil).
        mir_fn.blocks[entry]
            .instructions
            .push(MirInst::CallIndirect {
                dest: None,
                callee: drop_ptr,
                args: vec![MirValue::Use(self_local)],
            });

        mir_fn.blocks[entry].terminator = Terminator::Return(None);
        mir_fn
    }

    /// Build one `<Mixin>_dynamic_<method>` MIR function. See spec §B5.
    /// `mixin_slot` is the mixin's position in the class_info struct
    /// (0 for `Future` in v1).
    fn build_dynamic_dispatch_helper(
        &self,
        mixin_name: &str,
        method_name: &str,
        mixin_slot: usize,
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

        // vtable = class_info[mixin_slot]  — v1: `Future` is slot 0.
        let vtable = mir_fn.new_temp(Ty::Int);
        mir_fn.blocks[entry].instructions.push(MirInst::GetField {
            dest: vtable,
            base: class_info,
            field_index: mixin_slot,
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
