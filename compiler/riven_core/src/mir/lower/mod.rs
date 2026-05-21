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
    /// #06.8 Phase 2: map from Riven-side FFI fn name → linked C symbol,
    /// populated at the start of `lower_program` from `HirProgram::ffi_libs`.
    /// Consulted by `lower_fn_call` so that calling a Riven name like
    /// `add_one` whose `lib` block declared `def add_one as
    /// "riven_test_add_one"(...)` emits `MirInst::Call { callee:
    /// "riven_test_add_one", ... }` instead of `add_one`. Without this
    /// rewrite the linker would resolve the call to the wrong symbol
    /// (or fail outright if no `add_one` C symbol exists).
    ffi_alias_map: HashMap<String, String>,
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
    /// `riven_dealloc(L)` followed by `Assign L = 0` so the next
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

    // ── Public entry point ──────────────────────────────────────────────

    pub fn lower_program(&mut self, program: &HirProgram) -> Result<MirProgram, String> {
        let mut mir = MirProgram::new();

        // Gather `impl Trait for Type` edges so method calls on generic
        // type parameters can dispatch to the unique implementor.
        self.collect_trait_impls(program);

        // Collect trait default method bodies so that every `impl` can
        // monomorphise missing methods into a concrete {Type}_{method}.
        self.collect_trait_default_methods(program);

        // Collect `const` initializer expressions so references are
        // substituted with the RHS value at every use site.
        self.collect_const_values(program);

        // Record every class that defines its own `def drop` so that
        // drop-elaboration emits a call to `{ClassName}_drop` before
        // the no-op `MirInst::Drop` cleanup at scope exit.
        self.collect_user_drop_classes(program);

        // #06.8 Phase 2: bridge `HirProgram::ffi_libs` → `MirProgram::ffi_libs`.
        // Each FFI decl becomes a codegen `FfiFuncDecl` whose `name` field is
        // the linked C symbol (alias if present, Riven name otherwise) and
        // whose `riven_name` is the call-site identifier. We also build the
        // `ffi_alias_map` so call-site lowering can rewrite a Riven-named
        // FFI call into a call to the actual C symbol.
        for hir_lib in &program.ffi_libs {
            let mut funcs = Vec::with_capacity(hir_lib.functions.len());
            for hir_fn in &hir_lib.functions {
                let c_name = hir_fn
                    .c_symbol
                    .clone()
                    .unwrap_or_else(|| hir_fn.riven_name.clone());
                if hir_fn.c_symbol.is_some() {
                    self.ffi_alias_map
                        .insert(hir_fn.riven_name.clone(), c_name.clone());
                }
                funcs.push(crate::mir::nodes::FfiFuncDecl {
                    name: c_name,
                    riven_name: hir_fn.riven_name.clone(),
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

        for item in &program.items {
            match item {
                HirItem::Function(func) => {
                    let mir_fn = self.lower_function(func)?;
                    if mir_fn.name == "main" {
                        mir.entry = Some("main".to_string());
                    }
                    mir.functions.push(mir_fn);
                }
                HirItem::Class(class) => {
                    for method in &class.methods {
                        let mangled = format!("{}_{}", class.name, method.name);
                        let mir_fn = self.lower_method(&mangled, method)?;
                        mir.functions.push(mir_fn);
                    }
                    // ruby-naming.spec.md §3.4: a class's own `def` wins
                    // over any default method an included mixin provides.
                    // Track the names so trait-default synthesis below
                    // skips already-defined methods.
                    let outer_methods: HashSet<String> =
                        class.methods.iter().map(|m| m.name.clone()).collect();
                    for impl_block in &class.impl_blocks {
                        self.lower_impl_block_with_outer_methods(
                            impl_block,
                            &class.name,
                            &mut mir,
                            &outer_methods,
                        )?;
                    }
                    // ruby-naming.spec.md §3.6: a class never implicitly
                    // includes `Copy`, but every other structural mixin
                    // (Clone, Debug, Eq, …) is implicit when its field
                    // contract is satisfied. Mirror the struct/enum
                    // branches by routing through `ty_has_derive_trait`.
                    let class_ty = Ty::Class {
                        name: class.name.clone(),
                        generic_args: vec![],
                    };
                    let class_has = |trait_name: &str| -> bool {
                        crate::resolve::symbols::ty_has_derive_trait(
                            &class_ty,
                            self.symbols,
                            trait_name,
                        )
                    };
                    if class_has("Clone") {
                        mir.functions.push(self.synthesize_class_clone(class));
                    }
                }
                HirItem::Struct(s) => {
                    // ruby-naming.spec.md §3.4a: lower inline methods
                    // defined directly inside the struct body.
                    for method in &s.methods {
                        let mangled = format!("{}_{}", s.name, method.name);
                        let mir_fn = self.lower_method(&mangled, method)?;
                        mir.functions.push(mir_fn);
                    }
                    // ruby-naming.spec.md §3.6: structural mixins are
                    // implicitly included when every field structurally
                    // supports them. The synthesis gates therefore
                    // consult `ty_has_derive_trait`, which folds the
                    // explicit `derive_traits` list with the implicit
                    // rule (see resolve::symbols).
                    let struct_ty = Ty::Struct {
                        name: s.name.clone(),
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
                    // ruby-naming.spec.md §3.4a: enums may carry inline
                    // methods directly in their body. Lower each with
                    // the `{EnumName}_{method}` mangling used by method
                    // dispatch.
                    for method in &e.methods {
                        let mangled = format!("{}_{}", e.name, method.name);
                        let mir_fn = self.lower_method(&mangled, method)?;
                        mir.functions.push(mir_fn);
                    }
                    // ruby-naming.spec.md §3.6: structural mixins for
                    // enums also work implicitly when every variant
                    // field structurally supports them. Route through
                    // ty_has_derive_trait, which folds explicit derives
                    // with the implicit rule.
                    let enum_ty = Ty::Enum {
                        name: e.name.clone(),
                        generic_args: vec![],
                    };
                    let has = |trait_name: &str| -> bool {
                        crate::resolve::symbols::ty_has_derive_trait(
                            &enum_ty,
                            self.symbols,
                            trait_name,
                        )
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
                    self.lower_impl_block(impl_block, &type_name, &mut mir)?;
                }
                HirItem::Mixin(_)
                | HirItem::TypeAlias(_)
                | HirItem::Newtype(_)
                | HirItem::Const(_)
                | HirItem::Module(_) => {
                    // These don't produce MIR functions directly.
                }
            }
        }

        // Emit the primitive Display::fmt synth functions unconditionally
        // (Phase 2 #06.D2.S1). These are program-level, not per-use.
        // Stage 3 (D2) `lower_interpolation` rewrite assumes these are always
        // present; conditional emission would require a two-pass approach.
        for f in self.synthesize_primitive_fmt_displays() {
            mir.functions.push(f);
        }

        // Append any closure functions generated during lowering.
        mir.functions.append(&mut self.pending_closures);

        // Mixin vtables Phase B-2/B-3: emit vtable + class_info metadata
        // for every class that includes any `dispatch runtime` mixin.
        // Codegen reads these vectors and emits one data section per
        // vtable + one per class_info. Order is: per (class, mixin)
        // pair, with mixin slots in `runtime_dispatch_includes` order
        // (mixin-include declaration order on the class).
        self.collect_mixin_vtables(&mut mir);

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

    /// Find the parent class name of the function currently being lowered, if
    /// that function belongs to a class (its mangled name is `Class_method`)
    /// and the class has a `< Parent` clause. Used to lower `super(...)` calls
    /// inside child-class constructors.
    fn current_parent_class(&self) -> Option<String> {
        use crate::resolve::symbols::DefKind;
        let fn_name = self.current_fn.as_ref().map(|f| f.name.clone())?;
        let class_name = fn_name.split('_').next().unwrap_or("");
        if class_name.is_empty() {
            return None;
        }
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
    /// when the mangled riven-side name has a registered alias (with
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
        self.lookup_ffi_alias(&mangled).unwrap_or(mangled)
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
        // Collect every Class def matching the base name.
        let mut candidates: Vec<(DefId, Option<DefId>)> = Vec::new();
        for def in self.symbols.iter() {
            if def.name == lookup_name {
                if let DefKind::Class { ref info } = def.kind {
                    candidates.push((def.id, info.parent));
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
        let parent_name: Option<String> = parent_id.and_then(|pid| {
            self.symbols.get(pid).map(|p| p.name.clone())
        });
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
}

// ─── Standalone entry point (backward compat) ───────────────────────────────

/// Convenience function: lower an HIR program to MIR.
pub fn lower_program(program: &HirProgram, symbols: &SymbolTable) -> Result<MirProgram, String> {
    let mut lowerer = Lowerer::new(symbols);
    lowerer.lower_program(program)
}
