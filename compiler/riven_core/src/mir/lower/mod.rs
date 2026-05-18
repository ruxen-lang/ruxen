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
mod collect;
mod derive;
mod drops;
mod emit;
mod interpolation;
mod impl_block;
mod function;
mod statement;
mod match_arms;
mod closure_inline;
mod expr;
mod trait_default;
mod type_helpers;
mod util;
pub use type_helpers::{def_id_name, type_name_from_ty};
use captures::*;
use drops::*;
use trait_default::*;
use type_helpers::*;
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

        Ok(mir)
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
        // Find the class def.
        let mut class_def_id: Option<DefId> = None;
        let mut parent_name: Option<String> = None;
        for def in self.symbols.iter() {
            if def.name == base {
                if let DefKind::Class { ref info } = def.kind {
                    class_def_id = Some(def.id);
                    if let Some(parent_id) = info.parent {
                        if let Some(p) = self.symbols.get(parent_id) {
                            parent_name = Some(p.name.clone());
                        }
                    }
                    break;
                }
            }
        }
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
