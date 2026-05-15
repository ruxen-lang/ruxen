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

    /// Record every (trait → concrete-impl-target) mapping in the program.
    /// Used to dispatch method calls on generic type parameters when the
    /// trait bound has a unique implementor.
    fn collect_trait_impls(&mut self, program: &HirProgram) {
        fn visit(
            item: &HirItem,
            map: &mut HashMap<String, Vec<String>>,
            into_map: &mut HashSet<(String, String)>,
        ) {
            match item {
                HirItem::Impl(imp) => {
                    if let Some(ref trait_ref) = imp.trait_ref {
                        let target = type_name_from_ty(&imp.target_ty);
                        map.entry(trait_ref.name.clone())
                            .or_default()
                            .push(target.clone());
                        if trait_ref.name == "Into" {
                            if let Some(arg) = trait_ref.generic_args.first() {
                                let dst = type_name_from_ty(arg);
                                into_map.insert((target, dst));
                            }
                        }
                    }
                }
                HirItem::Class(class) => {
                    for derive_trait in &class.derive_traits {
                        map.entry(derive_trait.clone())
                            .or_default()
                            .push(class.name.clone());
                    }
                    for inner in &class.impl_blocks {
                        if let Some(ref trait_ref) = inner.trait_ref {
                            map.entry(trait_ref.name.clone())
                                .or_default()
                                .push(class.name.clone());
                            if trait_ref.name == "Into" {
                                if let Some(arg) = trait_ref.generic_args.first() {
                                    let dst = type_name_from_ty(arg);
                                    into_map.insert((class.name.clone(), dst));
                                }
                            }
                        }
                    }
                }
                HirItem::Struct(strukt) => {
                    for derive_trait in &strukt.derive_traits {
                        map.entry(derive_trait.clone())
                            .or_default()
                            .push(strukt.name.clone());
                    }
                }
                HirItem::Enum(enm) => {
                    for derive_trait in &enm.derive_traits {
                        map.entry(derive_trait.clone())
                            .or_default()
                            .push(enm.name.clone());
                    }
                }
                HirItem::Module(m) => {
                    for sub in &m.items {
                        visit(sub, map, into_map);
                    }
                }
                _ => {}
            }
        }
        for item in &program.items {
            visit(item, &mut self.trait_impls, &mut self.into_impls);
        }
    }

    /// Walk the program and record every trait's default method bodies,
    /// keyed by `(trait_name, method_name)`.  Impl blocks that don't
    /// override a default method get a monomorphised copy of the body
    /// emitted as a regular `{TypeName}_{method}` MIR function.
    fn collect_trait_default_methods(&mut self, program: &HirProgram) {
        fn visit(item: &HirItem, map: &mut HashMap<String, HashMap<String, HirFuncDef>>) {
            match item {
                HirItem::Mixin(tdef) => {
                    let entry = map.entry(tdef.name.clone()).or_default();
                    for ti in &tdef.items {
                        if let HirMixinItem::DefaultMethod(f) = ti {
                            entry.insert(f.name.clone(), f.clone());
                        }
                    }
                }
                HirItem::Module(m) => {
                    for sub in &m.items {
                        visit(sub, map);
                    }
                }
                _ => {}
            }
        }
        for item in &program.items {
            visit(item, &mut self.trait_default_methods);
        }
    }

    /// Walk the program and record every top-level `const` definition's
    /// initializer so references can be substituted at use sites.
    fn collect_const_values(&mut self, program: &HirProgram) {
        fn visit(item: &HirItem, map: &mut HashMap<DefId, HirExpr>) {
            match item {
                HirItem::Const(c) => {
                    map.insert(c.def_id, c.value.clone());
                }
                HirItem::Module(m) => {
                    for sub in &m.items {
                        visit(sub, map);
                    }
                }
                _ => {}
            }
        }
        for item in &program.items {
            visit(item, &mut self.const_values);
        }
    }

    /// Walk the program and record every class that defines its own
    /// `def drop` method (typically inside an `impl Drop` block, but we
    /// also accept a top-level `def drop` on the class). This drives the
    /// drop-elaboration pass to emit a call to `{ClassName}_drop` before
    /// the no-op `MirInst::Drop` cleanup at scope exit.
    fn collect_user_drop_classes(&mut self, program: &HirProgram) {
        fn class_has_drop_method(class: &HirClassDef) -> bool {
            if class.methods.iter().any(|m| m.name == "drop") {
                return true;
            }
            for impl_block in &class.impl_blocks {
                for item in &impl_block.items {
                    if let HirImplItem::Method(m) = item {
                        if m.name == "drop" {
                            return true;
                        }
                    }
                }
            }
            false
        }

        fn visit(item: &HirItem, set: &mut HashSet<String>) {
            match item {
                HirItem::Class(class) => {
                    if class_has_drop_method(class) {
                        set.insert(class.name.clone());
                    }
                }
                HirItem::Impl(impl_block) => {
                    let target = type_name_from_ty(&impl_block.target_ty);
                    if !target.is_empty() {
                        for inner in &impl_block.items {
                            if let HirImplItem::Method(m) = inner {
                                if m.name == "drop" {
                                    set.insert(target.clone());
                                    break;
                                }
                            }
                        }
                    }
                }
                HirItem::Module(m) => {
                    for sub in &m.items {
                        visit(sub, set);
                    }
                }
                _ => {}
            }
        }

        for item in &program.items {
            visit(item, &mut self.user_drop_classes);
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
                    if class.derive_traits.iter().any(|t| t == "Clone") {
                        mir.functions.push(self.synthesize_class_clone(class));
                    }
                }
                HirItem::Struct(s) => {
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
                    if e.derive_traits.iter().any(|t| t == "Debug") {
                        mir.functions.push(self.synthesize_enum_to_debug(e));
                    }
                    if e.derive_traits.iter().any(|t| t == "Clone") {
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

    // ── Impl block helper ───────────────────────────────────────────────

    fn lower_impl_block(
        &mut self,
        impl_block: &HirImplBlock,
        type_name: &str,
        mir: &mut MirProgram,
    ) -> Result<(), String> {
        self.lower_impl_block_with_outer_methods(impl_block, type_name, mir, &HashSet::new())
    }

    fn lower_impl_block_with_outer_methods(
        &mut self,
        impl_block: &HirImplBlock,
        type_name: &str,
        mir: &mut MirProgram,
        outer_methods: &HashSet<String>,
    ) -> Result<(), String> {
        // Track which method names the impl defines explicitly so we can
        // decide which trait defaults to monomorphise. `outer_methods`
        // covers the enclosing class/extension body's own `def`s —
        // critical for ruby-naming.spec.md §3.4a where `include Mixin`
        // sits beside the methods that satisfy or override it.
        let mut defined_methods: HashSet<String> = outer_methods.clone();
        for item in &impl_block.items {
            match item {
                HirImplItem::Method(method) => {
                    defined_methods.insert(method.name.clone());
                    let mangled = format!("{}_{}", type_name, method.name);
                    let mir_fn = self.lower_method(&mangled, method)?;
                    mir.functions.push(mir_fn);
                }
                HirImplItem::AssocType { .. } => {}
            }
        }

        // For `impl Trait for Type`, emit a monomorphised copy of every
        // default method the impl did not override. The default body is
        // cloned and its `Self` type occurrences are rewritten to the
        // concrete impl target so `self.field` / `self.method` dispatch
        // resolves through the normal class path.
        if let Some(ref trait_ref) = impl_block.trait_ref {
            if let Some(defaults) = self.trait_default_methods.get(&trait_ref.name).cloned() {
                let concrete_self = impl_block.target_ty.clone();
                for (mname, default_fn) in defaults {
                    if defined_methods.contains(&mname) {
                        continue;
                    }
                    let mut cloned = default_fn.clone();
                    rewrite_self_in_func(&mut cloned, &concrete_self);
                    let mangled = format!("{}_{}", type_name, cloned.name);
                    let mir_fn = self.lower_method(&mangled, &cloned)?;
                    mir.functions.push(mir_fn);
                }
            }
        }
        Ok(())
    }

    // ── Function / Method lowering ──────────────────────────────────────

    fn lower_function(&mut self, func: &HirFuncDef) -> Result<MirFunction, String> {
        self.lower_method(&func.name, func)
    }

    fn lower_method(&mut self, name: &str, func: &HirFuncDef) -> Result<MirFunction, String> {
        // Reset per-function state.
        self.def_to_local.clear();
        self.cell_promoted.clear();
        self.initialized_heap_locals.clear();
        let mir_fn = MirFunction::new(name, func.return_ty.clone());
        self.current_block = mir_fn.entry_block;
        self.current_fn = Some(mir_fn);

        // If this method has a self_mode, add self as the first parameter.
        if func.self_mode.is_some() {
            // Derive the self type from the mangled method name (ClassName_method)
            let self_ty = if let Some(class_name) = name.split('_').next() {
                Ty::Class {
                    name: class_name.to_string(),
                    generic_args: vec![],
                }
            } else {
                Ty::Unit
            };
            let local = self.fn_mut().new_local("self", self_ty, true);
            self.fn_mut().params.push(local);
            // Register all SelfValue DefIds in the symbol table so self.field works
            for def in self.symbols.iter() {
                if def.name == "self" {
                    if let crate::resolve::symbols::DefKind::SelfValue { .. } = &def.kind {
                        self.def_to_local.insert(def.id, local);
                    }
                }
            }
        }

        // Create locals for parameters.
        for param in &func.params {
            let local = self
                .fn_mut()
                .new_local(&param.name, param.ty.clone(), false);
            self.fn_mut().params.push(local);
            self.def_to_local.insert(param.def_id, local);
        }

        // Handle auto-assign params (@field) in init methods.
        // Generate SetField for each auto_assign param.
        // The field_index must match the class field order, not the param
        // order, since the class may have fields that aren't auto-assigned
        // (e.g., `status` in Task is set in the init body, not via @param).
        if func.name == "init" && func.self_mode.is_some() {
            // Find the self local (should be local 0 if self_mode is set)
            let self_local = self.def_to_local.values().copied().min().unwrap_or(0);
            // Get class field names from the class name (derived from mangled method name)
            let class_name = name.split('_').next().unwrap_or("");
            let class_fields = self.get_class_field_names(class_name);
            for param in func.params.iter() {
                if param.auto_assign {
                    if let Some(&param_local) = self.def_to_local.get(&param.def_id) {
                        // Look up the field index by name in the class.
                        let field_index = class_fields
                            .iter()
                            .position(|f| f == &param.name)
                            .unwrap_or_else(|| {
                                // Fallback: try to find in the param list by position
                                func.params
                                    .iter()
                                    .position(|p| p.def_id == param.def_id)
                                    .unwrap_or(0)
                            });
                        self.emit(MirInst::SetField {
                            base: self_local,
                            field_index,
                            value: MirValue::Use(param_local),
                        });
                    }
                }
            }
        }

        // Lower the body.
        let result = self.lower_expr(&func.body)?;

        // If the current block's terminator is still Unreachable, add an
        // implicit return.
        if matches!(self.get_terminator(), Terminator::Unreachable) {
            if func.return_ty == Ty::Unit || func.return_ty == Ty::Never {
                self.set_terminator(Terminator::Return(None));
            } else if let Some(local) = result {
                self.set_terminator(Terminator::Return(Some(MirValue::Use(local))));
            } else {
                self.set_terminator(Terminator::Return(None));
            }
        }

        let mut mir_fn = self.current_fn.take().expect("current_fn must be Some");

        // Determine the return-value locals so we don't drop them. A
        // function may return through multiple `Return` terminators (one
        // per match arm, for example), so we collect every distinct local
        // referenced in such a terminator.
        let return_locals = self.find_return_locals(&mir_fn);

        // Insert Drop instructions for Move-type locals before every Return.
        insert_drops(
            &mut mir_fn,
            &return_locals,
            self.symbols,
            &self.user_drop_classes,
        );

        Ok(mir_fn)
    }

    /// Find every local that appears as the value of a `Return` terminator.
    ///
    /// Functions with multiple early-return paths (e.g. each match arm
    /// returning a freshly built `String`) end up with several `Return`
    /// terminators referencing different locals. Drop elaboration must
    /// exclude all of them — otherwise the final scope-exit free would
    /// release the value the caller is about to read.
    fn find_return_locals(&self, func: &MirFunction) -> HashSet<LocalId> {
        let mut out = HashSet::new();
        for block in &func.blocks {
            if let Terminator::Return(Some(MirValue::Use(local))) = &block.terminator {
                out.insert(*local);
            }
        }
        out
    }

    /// Lower a function-call argument, auto-invoking bare zero-arg function
    /// references.  In Riven, `puts greet` is parsed as `puts(greet)` with
    /// `greet` an `Identifier`; resolution turns the identifier into a
    /// `VarRef` even when it points at a function.  Without special handling
    /// the MIR would try to pass the function address as a value and end up
    /// passing `MirValue::Unit` (NULL).  Instead, detect that case and emit
    /// a `Call` that actually invokes the function.
    fn lower_fn_arg(&mut self, arg: &HirExpr) -> Result<Option<LocalId>, String> {
        use crate::resolve::symbols::DefKind;
        if let HirExprKind::VarRef(def_id) = &arg.kind {
            // Only auto-invoke if the DefId is a zero-arg function and the
            // identifier is not already mapped to a local (which would mean
            // it was shadowed by a `let` binding of the same name).
            if !self.def_to_local.contains_key(def_id) {
                if let Some(def) = self.symbols.get(*def_id) {
                    if let DefKind::Function { signature } = &def.kind {
                        if signature.params.is_empty() {
                            let ret_ty = signature.return_ty.clone();
                            let callee_name = def.name.clone();
                            let dest = if ret_ty != Ty::Unit && ret_ty != Ty::Never {
                                Some(self.new_temp(ret_ty))
                            } else {
                                None
                            };
                            self.emit(MirInst::Call {
                                dest,
                                callee: callee_name,
                                args: vec![],
                            });
                            return Ok(dest);
                        }
                    }
                }
            }
        }
        self.lower_expr(arg)
    }

    // ── Expression lowering ─────────────────────────────────────────────
    //
    // Returns `Ok(Some(local))` when the expression produces a value, or
    // `Ok(None)` for unit-typed / statement-like expressions.

    fn lower_expr(&mut self, expr: &HirExpr) -> Result<Option<LocalId>, String> {
        match &expr.kind {
            // ── Literals ────────────────────────────────────────────
            HirExprKind::IntLiteral(n) => {
                let dest = self.new_temp(expr.ty.clone());
                self.emit(MirInst::Assign {
                    dest,
                    value: MirValue::Literal(Literal::Int(*n)),
                });
                Ok(Some(dest))
            }
            HirExprKind::FloatLiteral(n) => {
                let dest = self.new_temp(expr.ty.clone());
                self.emit(MirInst::Assign {
                    dest,
                    value: MirValue::Literal(Literal::Float(*n)),
                });
                Ok(Some(dest))
            }
            HirExprKind::BoolLiteral(b) => {
                let dest = self.new_temp(Ty::Bool);
                self.emit(MirInst::Assign {
                    dest,
                    value: MirValue::Literal(Literal::Bool(*b)),
                });
                Ok(Some(dest))
            }
            HirExprKind::CharLiteral(c) => {
                let dest = self.new_temp(Ty::Char);
                self.emit(MirInst::Assign {
                    dest,
                    value: MirValue::Literal(Literal::Char(*c)),
                });
                Ok(Some(dest))
            }
            HirExprKind::StringLiteral(s) => {
                // P0.7: wrap raw .rodata pointer through riven_string_from so
                // the local owns a heap-allocated String. Without the wrap,
                // String::drop -> free() would double-free a literal pointer.
                let dest = self.emit_owned_string_literal(s);
                Ok(Some(dest))
            }
            HirExprKind::UnitLiteral => Ok(None),

            // ── Variable reference ──────────────────────────────────
            HirExprKind::VarRef(def_id) => {
                // Captured variable inside a closure body: load from the
                // captures pointer.  ByValue → a direct load; ByRef → load
                // the cell pointer and dereference through it.
                if let Some(slot) = self.capture_map.get(def_id).copied() {
                    let cap_ptr = self
                        .captures_ptr_local
                        .expect("capture_map non-empty implies captures_ptr_local is set");
                    match slot.kind {
                        CaptureKind::ByValue => {
                            let dest = self.new_temp(expr.ty.clone());
                            self.emit(MirInst::GetField {
                                dest,
                                base: cap_ptr,
                                field_index: slot.slot_index,
                            });
                            return Ok(Some(dest));
                        }
                        CaptureKind::ByRef => {
                            let cell_ptr = self.new_temp(Ty::Int);
                            self.emit(MirInst::GetField {
                                dest: cell_ptr,
                                base: cap_ptr,
                                field_index: slot.slot_index,
                            });
                            let dest = self.new_temp(expr.ty.clone());
                            self.emit(MirInst::GetField {
                                dest,
                                base: cell_ptr,
                                field_index: 0,
                            });
                            return Ok(Some(dest));
                        }
                    }
                }
                if let Some(&local) = self.def_to_local.get(def_id) {
                    // Cell-promoted locals (mutably captured by a closure
                    // in this frame) hold a pointer to an 8-byte cell;
                    // reads go through the cell.
                    if self.cell_promoted.contains(def_id) {
                        let dest = self.new_temp(expr.ty.clone());
                        self.emit(MirInst::GetField {
                            dest,
                            base: local,
                            field_index: 0,
                        });
                        return Ok(Some(dest));
                    }
                    Ok(Some(local))
                } else if let Some(const_expr) = self.const_values.get(def_id).cloned() {
                    // Reference to a top-level `const` — substitute the
                    // initializer expression inline at this use site.
                    self.lower_expr(&const_expr)
                } else {
                    // Might be a top-level function reference — just return None
                    // for now; calls use the callee_name directly.
                    Ok(None)
                }
            }

            // ── Binary operations ───────────────────────────────────
            HirExprKind::BinaryOp { op, left, right } => {
                // ── derive PartialEq: structural equality on structs ──
                // `a == b` and `a != b` on a struct that derives PartialEq
                // must compare field-by-field. The default `Compare` lowering
                // would compare struct *pointers* (heap addresses), false for
                // two distinct allocations even when their fields match.
                if matches!(op, BinOp::Eq | BinOp::NotEq) {
                    if let Some(struct_name) = struct_name_with_partial_eq(&left.ty, self.symbols) {
                        if let Some(field_info) = struct_field_layout(&struct_name, self.symbols) {
                            return Ok(Some(self.lower_struct_partial_eq(
                                left,
                                right,
                                *op,
                                &field_info,
                            )?));
                        }
                    }
                }

                // ── derive Ord / PartialOrd: route ordering operators to
                // the synthesised `<Type>_cmp` / `<Type>_partial_cmp`
                // helper. The default `Compare` lowering below would
                // compare struct *pointers* (heap addresses), which
                // gives meaningless lex order across allocations. The
                // synthesiser's tuple-style field walk already returns
                // -1 / 0 / +1, so we only need to fold that result
                // through the requested operator.
                if matches!(op, BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq) {
                    if let Some((struct_name, partial)) =
                        struct_name_with_ord(&left.ty, self.symbols)
                    {
                        return Ok(Some(self.lower_struct_ord(
                            left,
                            right,
                            *op,
                            &struct_name,
                            partial,
                        )?));
                    }
                }

                let lhs_local = self.lower_expr(left)?;
                let rhs_local = self.lower_expr(right)?;
                let lhs_val = local_to_value(lhs_local);
                let rhs_val = local_to_value(rhs_local);

                // ── Phase 2 stdlib batch 2 (#02): String + String ──
                // The default `MirInst::BinOp { op: Add, ... }` would
                // treat both operands as integers and codegen would
                // emit an integer-add over heap pointers — undefined
                // behaviour. Route through `riven_string_concat`
                // instead. Matches the existing string-interpolation
                // lowering, which already calls the same runtime fn.
                if matches!(op, BinOp::Add)
                    && matches!(left.ty, Ty::String | Ty::Str)
                    && matches!(right.ty, Ty::String | Ty::Str)
                {
                    let dest = self.new_temp(Ty::String);
                    self.emit(MirInst::Call {
                        dest: Some(dest),
                        callee: "riven_string_concat".to_string(),
                        args: vec![lhs_val, rhs_val],
                    });
                    return Ok(Some(dest));
                }

                // ── Phase 2 stdlib batch 1 (#03): Vec[T] == Vec[T] ──
                // The default integer Compare would compare heap
                // pointers, returning false for any two distinct
                // allocations even when their elements match. Route
                // through `riven_vec_eq` for both `==` and `!=`.
                if matches!(op, BinOp::Eq | BinOp::NotEq)
                    && matches!(left.ty, Ty::Array(_))
                    && matches!(right.ty, Ty::Array(_))
                {
                    let cmp = self.new_temp(Ty::Bool);
                    self.emit(MirInst::Call {
                        dest: Some(cmp),
                        callee: "riven_vec_eq".to_string(),
                        args: vec![lhs_val, rhs_val],
                    });
                    if matches!(op, BinOp::Eq) {
                        return Ok(Some(cmp));
                    }
                    let dest = self.new_temp(Ty::Bool);
                    self.emit(MirInst::Not {
                        dest,
                        operand: MirValue::Use(cmp),
                    });
                    return Ok(Some(dest));
                }

                // ── Phase 2 stdlib (#04): HashMap == HashMap ──
                // Same justification as Vec equality above — default
                // integer compare on the spine pointers is meaningless
                // across allocations. Route through `riven_hash_eq`.
                if matches!(op, BinOp::Eq | BinOp::NotEq)
                    && matches!(left.ty, Ty::Map(_, _))
                    && matches!(right.ty, Ty::Map(_, _))
                {
                    let cmp = self.new_temp(Ty::Bool);
                    self.emit(MirInst::Call {
                        dest: Some(cmp),
                        callee: "riven_hash_eq".to_string(),
                        args: vec![lhs_val, rhs_val],
                    });
                    if matches!(op, BinOp::Eq) {
                        return Ok(Some(cmp));
                    }
                    let dest = self.new_temp(Ty::Bool);
                    self.emit(MirInst::Not {
                        dest,
                        operand: MirValue::Use(cmp),
                    });
                    return Ok(Some(dest));
                }

                // ── Phase 2 stdlib (#04): HashSet == HashSet ──
                if matches!(op, BinOp::Eq | BinOp::NotEq)
                    && matches!(left.ty, Ty::Set(_))
                    && matches!(right.ty, Ty::Set(_))
                {
                    let cmp = self.new_temp(Ty::Bool);
                    self.emit(MirInst::Call {
                        dest: Some(cmp),
                        callee: "riven_set_eq".to_string(),
                        args: vec![lhs_val, rhs_val],
                    });
                    if matches!(op, BinOp::Eq) {
                        return Ok(Some(cmp));
                    }
                    let dest = self.new_temp(Ty::Bool);
                    self.emit(MirInst::Not {
                        dest,
                        operand: MirValue::Use(cmp),
                    });
                    return Ok(Some(dest));
                }

                let dest = self.new_temp(expr.ty.clone());

                if is_comparison(*op) {
                    let cmp_op = binop_to_cmpop(*op);
                    self.emit(MirInst::Compare {
                        dest,
                        op: cmp_op,
                        lhs: lhs_val,
                        rhs: rhs_val,
                    });
                } else {
                    self.emit(MirInst::BinOp {
                        dest,
                        op: *op,
                        lhs: lhs_val,
                        rhs: rhs_val,
                    });
                }
                Ok(Some(dest))
            }

            // ── Unary operations ────────────────────────────────────
            HirExprKind::UnaryOp { op, operand } => {
                let src = self.lower_expr(operand)?;
                let val = local_to_value(src);
                let dest = self.new_temp(expr.ty.clone());
                match op {
                    UnaryOp::Neg => self.emit(MirInst::Negate { dest, operand: val }),
                    UnaryOp::Not => self.emit(MirInst::Not { dest, operand: val }),
                    UnaryOp::Deref => {
                        // `*x` — strip one reference level. In Riven's value
                        // model a reference is represented the same as its
                        // pointee for scalar types, so this is a plain copy
                        // of the underlying value.
                        self.emit(MirInst::Assign { dest, value: val });
                    }
                }
                Ok(Some(dest))
            }

            // ── Block ───────────────────────────────────────────────
            HirExprKind::Block(stmts, tail) => {
                for stmt in stmts {
                    self.lower_statement(stmt)?;
                }
                if let Some(tail_expr) = tail {
                    self.lower_expr(tail_expr)
                } else {
                    Ok(None)
                }
            }

            // ── If / else ───────────────────────────────────────────
            HirExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let cond_local = self.lower_expr(cond)?;
                let cond_val = local_to_value(cond_local);

                let then_block = self.new_block();
                let else_block = self.new_block();
                let merge_block = self.new_block();

                self.set_terminator(Terminator::Branch {
                    cond: cond_val,
                    then_block,
                    else_block,
                });

                // Then branch
                self.current_block = then_block;
                let then_result = self.lower_expr(then_branch)?;
                let then_exit_block = self.current_block;

                // Else branch
                self.current_block = else_block;
                let else_result = if let Some(else_expr) = else_branch {
                    self.lower_expr(else_expr)?
                } else {
                    None
                };
                let else_exit_block = self.current_block;

                // If the expression has a non-unit type, create a phi-like merge.
                let result = if expr.ty != Ty::Unit && expr.ty != Ty::Never {
                    let result_local = self.new_temp(expr.ty.clone());

                    // Assign from then-branch
                    self.current_block = then_exit_block;
                    if matches!(self.get_terminator(), Terminator::Unreachable) {
                        let val = local_to_value(then_result);
                        self.emit(MirInst::Assign {
                            dest: result_local,
                            value: val,
                        });
                        self.set_terminator(Terminator::Goto(merge_block));
                    }

                    // Assign from else-branch
                    self.current_block = else_exit_block;
                    if matches!(self.get_terminator(), Terminator::Unreachable) {
                        let val = local_to_value(else_result);
                        self.emit(MirInst::Assign {
                            dest: result_local,
                            value: val,
                        });
                        self.set_terminator(Terminator::Goto(merge_block));
                    }

                    Some(result_local)
                } else {
                    // Unit-typed: just jump to merge.
                    self.current_block = then_exit_block;
                    if matches!(self.get_terminator(), Terminator::Unreachable) {
                        self.set_terminator(Terminator::Goto(merge_block));
                    }
                    self.current_block = else_exit_block;
                    if matches!(self.get_terminator(), Terminator::Unreachable) {
                        self.set_terminator(Terminator::Goto(merge_block));
                    }
                    None
                };

                self.current_block = merge_block;
                Ok(result)
            }

            // ── While loop ──────────────────────────────────────────
            HirExprKind::While { condition, body } => {
                let header_block = self.new_block();
                let body_block = self.new_block();
                let exit_block = self.new_block();

                // Jump from current block to header.
                self.set_terminator(Terminator::Goto(header_block));

                // Header: evaluate condition, branch.
                self.current_block = header_block;
                let cond_local = self.lower_expr(condition)?;
                let cond_val = local_to_value(cond_local);
                self.set_terminator(Terminator::Branch {
                    cond: cond_val,
                    then_block: body_block,
                    else_block: exit_block,
                });

                // Body: execute, then jump back to header.
                // `continue` inside the body jumps to the header (re-check
                // the condition); `break` jumps to the exit block.
                self.current_block = body_block;
                self.loop_stack.push(LoopFrame {
                    continue_target: header_block,
                    break_target: exit_block,
                    result_local: None,
                    body_entry_block: body_block,
                    body_locals: Vec::new(),
                });
                let _ = self.lower_expr(body)?;
                let frame = self.loop_stack.pop().expect("loop frame");
                if matches!(self.get_terminator(), Terminator::Unreachable) {
                    self.emit_dealloc_loop_locals(&frame.body_locals);
                    self.set_terminator(Terminator::Goto(header_block));
                }
                self.prepend_zero_init_for_body_locals(&frame);

                self.current_block = exit_block;
                Ok(None) // while loops produce Unit
            }

            // ── Loop (infinite) ─────────────────────────────────────
            HirExprKind::Loop { body } => {
                let loop_block = self.new_block();
                let exit_block = self.new_block();

                // If the loop expression yields a value (via `break VALUE`),
                // allocate a result local that every `break` writes into
                // before jumping to the exit block.
                let result_local = if expr.ty != Ty::Unit && expr.ty != Ty::Never {
                    Some(self.new_temp(expr.ty.clone()))
                } else {
                    None
                };

                self.set_terminator(Terminator::Goto(loop_block));

                self.current_block = loop_block;
                self.loop_stack.push(LoopFrame {
                    continue_target: loop_block,
                    break_target: exit_block,
                    result_local,
                    body_entry_block: loop_block,
                    body_locals: Vec::new(),
                });
                let _ = self.lower_expr(body)?;
                let frame = self.loop_stack.pop().expect("loop frame");
                if matches!(self.get_terminator(), Terminator::Unreachable) {
                    self.emit_dealloc_loop_locals(&frame.body_locals);
                    self.set_terminator(Terminator::Goto(loop_block));
                }
                self.prepend_zero_init_for_body_locals(&frame);

                // exit_block is only reachable via break (which we handle below)
                self.current_block = exit_block;
                Ok(result_local)
            }

            // ── Return ──────────────────────────────────────────────
            HirExprKind::Return(value) => {
                let val = if let Some(expr) = value {
                    let local = self.lower_expr(expr)?;
                    Some(local_to_value(local))
                } else {
                    None
                };
                self.set_terminator(Terminator::Return(val));
                // Create a dead block for any code after the return.
                let dead = self.new_block();
                self.current_block = dead;
                Ok(None)
            }

            // ── Function call ───────────────────────────────────────
            HirExprKind::FnCall {
                callee_name, args, ..
            } => {
                // `super(...)` inside an `init` of a subclass: dispatch to the
                // parent class's init, forwarding the child's `self` as the
                // receiver so that the parent's `@field` auto-assigns write
                // into the same object.
                if callee_name == "super" {
                    if let Some(parent_name) = self.current_parent_class() {
                        let self_local = self.fn_mut().params.first().copied().unwrap_or(0);
                        let mut arg_values = Vec::with_capacity(args.len() + 1);
                        arg_values.push(MirValue::Use(self_local));
                        for arg in args {
                            let local = self.lower_expr(arg)?;
                            arg_values.push(local_to_value(local));
                        }
                        self.emit(MirInst::Call {
                            dest: None,
                            callee: format!("{}_init", parent_name),
                            args: arg_values,
                        });
                        return Ok(None);
                    }
                }

                let mut arg_values = Vec::with_capacity(args.len());
                for arg in args {
                    // Auto-invoke bare zero-arg function references used as
                    // arguments.  Riven allows calling a function without
                    // parentheses (`puts greet` ≡ `puts greet()`), so when an
                    // argument is an identifier that resolves to a zero-arg
                    // function, synthesize the invocation rather than passing
                    // the function address as a value.
                    let local = self.lower_fn_arg(arg)?;
                    arg_values.push(local_to_value(local));
                }

                let dest = if expr.ty != Ty::Unit && expr.ty != Ty::Never {
                    Some(self.new_temp(expr.ty.clone()))
                } else {
                    None
                };

                self.emit(MirInst::Call {
                    dest,
                    callee: callee_name.clone(),
                    args: arg_values,
                });
                Ok(dest)
            }

            // ── Method call ─────────────────────────────────────────
            HirExprKind::MethodCall {
                object,
                method_name,
                generic_args: _generic_args,
                args,
                block,
                ..
            } => {
                let type_name = self
                    .receiver_type_name(object)
                    .unwrap_or_else(|| type_name_from_ty(&object.ty));

                // Handle .new() constructor calls: allocate + call init
                if method_name == "new" {
                    // For built-in types (Vec, Hash, Set), call the runtime
                    // constructor directly instead of Alloc + init.
                    let base_type = if let Some(pos) = type_name.find('[') {
                        &type_name[..pos]
                    } else {
                        type_name.as_str()
                    };
                    // Phase 2 #06.D2.S0: `Formatter.new()` dispatches to
                    // the runtime constructor just like Vec/Hash.
                    if matches!(
                        base_type,
                        "Vec" | "Array"
                            | "Hash"
                            | "HashMap"
                            | "Map"
                            | "Set"
                            | "HashSet"
                            | "Formatter"
                    ) {
                        let obj = self.new_temp(expr.ty.clone());
                        // ruby-naming.spec.md §3.11 renames stdlib types
                        // (`Vec` → `Array`, `HashMap` → `Map`, `HashSet` →
                        // `Set`). The runtime C functions keep their
                        // legacy names (`Vec_new`, `Hash_new`, …), so map
                        // the surface base-type back to the runtime
                        // before mangling.
                        let runtime_base = match base_type {
                            "Array" => "Vec",
                            "Map" => "Hash",
                            "HashMap" => "Hash",
                            "Set" => "HashSet",
                            other => other,
                        };
                        // Emit Call to runtime constructor (e.g., Vec_new).
                        // Use the base type so the mangled callee elides the
                        // generic parameter list (`HashMap[K, V]_new` would
                        // not match a real runtime symbol).
                        self.emit(MirInst::Call {
                            dest: Some(obj),
                            callee: format!("{}_new", runtime_base),
                            args: vec![],
                        });
                        return Ok(Some(obj));
                    }
                    // String.new / String.with_capacity — dispatch to the
                    // C runtime directly. The dispatch table in
                    // codegen/runtime.rs maps `String_new` and
                    // `String_with_capacity` to their `riven_string_*`
                    // implementations.
                    if base_type == "String" {
                        let obj = self.new_temp(expr.ty.clone());
                        let mut call_args = Vec::with_capacity(args.len());
                        for arg in args {
                            let local = self.lower_expr(arg)?;
                            call_args.push(local_to_value(local));
                        }
                        self.emit(MirInst::Call {
                            dest: Some(obj),
                            callee: "String_new".to_string(),
                            args: call_args,
                        });
                        return Ok(Some(obj));
                    }

                    // Structs have no user-defined `init`. The positional
                    // arguments map directly onto the declared fields, so
                    // we allocate the backing storage and emit one
                    // SetField per argument — no synthetic init function.
                    if matches!(&object.ty, Ty::Struct { .. }) {
                        let obj = self.new_temp(expr.ty.clone());
                        self.emit(MirInst::Alloc {
                            dest: obj,
                            ty: expr.ty.clone(),
                            size: self.alloc_size(&expr.ty),
                        });
                        for (idx, arg) in args.iter().enumerate() {
                            let local = self.lower_expr(arg)?;
                            self.emit(MirInst::SetField {
                                base: obj,
                                field_index: idx,
                                value: local_to_value(local),
                            });
                        }
                        return Ok(Some(obj));
                    }

                    let layout = crate::codegen::layout::layout_of(&expr.ty, self.symbols);
                    let obj = self.new_temp(expr.ty.clone());
                    self.emit(MirInst::Alloc {
                        dest: obj,
                        ty: expr.ty.clone(),
                        size: self.alloc_size(&expr.ty),
                    });

                    // Call ClassName_init(self, args...)
                    let mut arg_values = vec![MirValue::Use(obj)];
                    for arg in args {
                        let local = self.lower_expr(arg)?;
                        arg_values.push(local_to_value(local));
                    }
                    let _ = layout; // size used by Alloc internally via layout_of in codegen
                    self.emit(MirInst::Call {
                        dest: None,
                        callee: format!("{}_init", type_name),
                        args: arg_values,
                    });
                    return Ok(Some(obj));
                }

                // ── Phase 2 stdlib (#04): HashMap.entry chain ──────────
                // `m.entry(K).or_insert(V)` and `m.entry(K).or_insert_with { || V }`
                // are recognized as a single MIR unit and inlined to:
                //
                //   if !riven_hash_contains_key(map, k) {
                //       riven_hash_insert(map, k, v);   // discard prior value
                //   }
                //
                // Typeck has already verified the chain shape and the V
                // type — see `infer.rs` MethodCall handler. This emission
                // never materializes an `Entry[K,V]` value at runtime.
                if (method_name == "or_insert" || method_name == "or_insert_with")
                    && matches!(
                        &object.kind,
                        HirExprKind::MethodCall { method_name: m, .. } if m == "entry"
                    )
                {
                    let result = self.inline_entry_or_insert(object, method_name, args, block)?;
                    return Ok(result);
                }

                // ── Inline closure-taking methods ──────────────────────
                // When a method like .each, .filter, .find, .position,
                // .map, .partition, .where_matching takes a trailing block
                // (closure), inline the closure body as a loop instead of
                // passing a (null) function pointer.
                if let Some(block_expr) = block {
                    if let Some(result) =
                        self.try_inline_closure_method(expr, object, method_name, args, block_expr)?
                    {
                        return Ok(result);
                    }
                }

                // Phase 2 stdlib (#05 follow-up): built-in
                // `iter.collect[Target]` lowers directly to a runtime
                // constructor over the v1 eager-iterator representation
                // (`RivenVec*`). Typeck has already validated the target
                // and item compatibility, so lowering only picks the
                // concrete helper by the expression's result type.
                if method_name == "collect" {
                    let iter_local = self.lower_expr(object)?;
                    let iter_id = iter_local.unwrap_or_else(|| self.new_temp(Ty::Int));
                    let dest = self.new_temp(expr.ty.clone());
                    let callee = match &expr.ty {
                        Ty::Array(_) => "riven_vec_from_iter",
                        Ty::String | Ty::Str => "riven_string_from_iter",
                        Ty::Map(_, _) => "riven_hash_from_iter",
                        Ty::Set(_) => "riven_set_from_iter",
                        other => {
                            return Err(format!(
                                "unsupported collect target in MIR lowering: {other}"
                            ));
                        }
                    };
                    self.emit(MirInst::Call {
                        dest: Some(dest),
                        callee: callee.to_string(),
                        args: vec![MirValue::Use(iter_id)],
                    });
                    return Ok(Some(dest));
                }

                // ── Inline try_op (? operator) ──────────────────────────
                // The ? operator desugars to .try_op(). For Result types:
                // Ok(x) -> extract x and continue; Err(e) -> return Err(e).
                // For Option types: Some(x) -> x; None -> return Err(err)
                // (only when inside a Result-returning function via ok_or).
                if method_name == "try_op" {
                    let obj_local = self.lower_expr(object)?;
                    let scrut = obj_local.unwrap_or_else(|| self.new_temp(Ty::Int));

                    // Read the tag: 0 = Ok/Some, 1 = Err/None
                    let tag = self.new_temp(Ty::Int32);
                    self.emit(MirInst::GetTag {
                        dest: tag,
                        src: scrut,
                    });

                    let ok_block = self.new_block();
                    let err_block = self.new_block();
                    let merge_block = self.new_block();

                    // tag == 0 means Ok
                    let is_ok = self.new_temp(Ty::Bool);
                    self.emit(MirInst::Compare {
                        dest: is_ok,
                        op: CmpOp::Eq,
                        lhs: MirValue::Use(tag),
                        rhs: MirValue::Literal(Literal::Int(0)),
                    });
                    self.set_terminator(Terminator::Branch {
                        cond: MirValue::Use(is_ok),
                        then_block: ok_block,
                        else_block: err_block,
                    });

                    // Ok block: extract payload
                    let result_local = self.new_temp(expr.ty.clone());
                    self.current_block = ok_block;
                    let payload_ptr = self.new_temp(Ty::Int);
                    self.emit(MirInst::GetPayload {
                        dest: payload_ptr,
                        src: scrut,
                        ty: object.ty.clone(),
                    });
                    self.emit(MirInst::GetField {
                        dest: result_local,
                        base: payload_ptr,
                        field_index: 0,
                    });
                    self.set_terminator(Terminator::Goto(merge_block));

                    // Err block: early return with Err wrapping the error payload.
                    // Allocate a Result tagged union and return it.
                    self.current_block = err_block;
                    let err_result = self.new_temp(Ty::Int);
                    self.emit(MirInst::Alloc {
                        dest: err_result,
                        ty: Ty::Result(Box::new(Ty::Unit), Box::new(Ty::Int)),
                        size: 16,
                    });
                    // Tag 1 = Err
                    self.emit(MirInst::SetTag {
                        dest: err_result,
                        tag: 1,
                    });
                    // Copy error payload from source
                    let err_payload_ptr = self.new_temp(Ty::Int);
                    self.emit(MirInst::GetPayload {
                        dest: err_payload_ptr,
                        src: scrut,
                        ty: object.ty.clone(),
                    });
                    let err_payload = self.new_temp(Ty::Int);
                    self.emit(MirInst::GetField {
                        dest: err_payload,
                        base: err_payload_ptr,
                        field_index: 0,
                    });

                    // If the current function's declared Err type differs
                    // from the source's Err type and an `impl Into[Outer]
                    // for Inner` was registered, insert a call to
                    // `Inner_into(err_payload)` to coerce the error.
                    let final_payload = if let (Ty::Result(_, src_err), Ty::Result(_, dst_err)) =
                        (&object.ty, &self.fn_mut().return_ty.clone())
                    {
                        let src_name = type_name_from_ty(src_err);
                        let dst_name = type_name_from_ty(dst_err);
                        if !src_name.is_empty()
                            && !dst_name.is_empty()
                            && src_name != dst_name
                            && self
                                .into_impls
                                .contains(&(src_name.clone(), dst_name.clone()))
                        {
                            let converted = self.new_temp((**dst_err).clone());
                            self.emit(MirInst::Call {
                                dest: Some(converted),
                                callee: format!("{}_into", src_name),
                                args: vec![MirValue::Use(err_payload)],
                            });
                            MirValue::Use(converted)
                        } else {
                            MirValue::Use(err_payload)
                        }
                    } else {
                        MirValue::Use(err_payload)
                    };

                    self.emit(MirInst::SetField {
                        base: err_result,
                        field_index: 1,
                        value: final_payload,
                    });
                    self.set_terminator(Terminator::Return(Some(MirValue::Use(err_result))));

                    self.current_block = merge_block;
                    return Ok(Some(result_local));
                }

                // ── Inline ok_or (Option -> Result conversion) ───────────
                // option.ok_or(err_val) converts:
                //   Some(x) -> Result::Ok(x) (tag 0)
                //   None    -> Result::Err(err_val) (tag 1)
                if method_name == "ok_or" {
                    let obj_local = self.lower_expr(object)?;
                    let scrut = obj_local.unwrap_or_else(|| self.new_temp(Ty::Int));

                    // Evaluate the error value argument
                    let err_arg = args.first();
                    let err_val = if let Some(err_expr) = err_arg {
                        let local = self.lower_expr(err_expr)?;
                        local_to_value(local)
                    } else {
                        MirValue::Literal(Literal::Int(0))
                    };

                    // Allocate a Result tagged union
                    let result = self.new_temp(expr.ty.clone());
                    self.emit(MirInst::Alloc {
                        dest: result,
                        ty: expr.ty.clone(),
                        size: 16,
                    });

                    // Read the Option tag: 0 = None (in Option), 1 = Some
                    // Note: inline_position uses tag 0 = None, tag 1 = Some
                    let tag = self.new_temp(Ty::Int32);
                    self.emit(MirInst::GetTag {
                        dest: tag,
                        src: scrut,
                    });

                    let some_block = self.new_block();
                    let none_block = self.new_block();
                    let merge_block = self.new_block();

                    // tag == 1 means Some
                    let is_some = self.new_temp(Ty::Bool);
                    self.emit(MirInst::Compare {
                        dest: is_some,
                        op: CmpOp::Eq,
                        lhs: MirValue::Use(tag),
                        rhs: MirValue::Literal(Literal::Int(1)),
                    });
                    self.set_terminator(Terminator::Branch {
                        cond: MirValue::Use(is_some),
                        then_block: some_block,
                        else_block: none_block,
                    });

                    // Some block: Result::Ok(payload) — tag 0
                    self.current_block = some_block;
                    self.emit(MirInst::SetTag {
                        dest: result,
                        tag: 0,
                    }); // Ok
                    let payload_ptr = self.new_temp(Ty::Int);
                    self.emit(MirInst::GetPayload {
                        dest: payload_ptr,
                        src: scrut,
                        ty: object.ty.clone(),
                    });
                    let some_val = self.new_temp(Ty::Int);
                    self.emit(MirInst::GetField {
                        dest: some_val,
                        base: payload_ptr,
                        field_index: 0,
                    });
                    self.emit(MirInst::SetField {
                        base: result,
                        field_index: 1,
                        value: MirValue::Use(some_val),
                    });
                    self.set_terminator(Terminator::Goto(merge_block));

                    // None block: Result::Err(err_val) — tag 1
                    self.current_block = none_block;
                    self.emit(MirInst::SetTag {
                        dest: result,
                        tag: 1,
                    }); // Err
                    self.emit(MirInst::SetField {
                        base: result,
                        field_index: 1,
                        value: err_val,
                    });
                    self.set_terminator(Terminator::Goto(merge_block));

                    self.current_block = merge_block;
                    return Ok(Some(result));
                }

                // Check if this is a static/class method call (no `self`
                // argument needed). Covers built-in static methods as well
                // as user-defined `def self.method` forms on classes.
                let static_dispatch_ty = if matches!(&object.ty, Ty::Infer(_)) {
                    &expr.ty
                } else {
                    &object.ty
                };
                let is_static = is_builtin_static_method(&type_name, method_name)
                    || self.is_user_static_method(&type_name, method_name)
                    || (method_name == "default"
                        && self.type_supports_trait(static_dispatch_ty, "Default"));

                // Regular method call: object becomes the first argument (self).
                let obj_local = self.lower_expr(object)?;

                let mut arg_values = if is_static {
                    // Static method: don't prepend self.
                    Vec::with_capacity(args.len())
                } else {
                    vec![local_to_value(obj_local)]
                };
                for arg in args {
                    let local = self.lower_expr(arg)?;
                    arg_values.push(local_to_value(local));
                }
                // Include trailing block argument if present (closures passed
                // as the last parameter of the method).
                if let Some(block_expr) = block {
                    let block_local = self.lower_expr(block_expr)?;
                    arg_values.push(local_to_value(block_local));
                }

                // Resolve through parent classes for inherited methods.
                // For a generic type parameter or impl/dyn Trait, dispatch
                // to the unique implementor of the trait bound when one
                // exists.
                let resolved_class = match &object.ty {
                    Ty::Class { name, .. } => self.resolve_method_class(name, method_name),
                    Ty::TypeParam { bounds, .. } | Ty::SomeMixin(bounds) | Ty::AnyMixin(bounds) => {
                        self.unique_bound_impl(bounds)
                            .unwrap_or_else(|| type_name.clone())
                    }
                    Ty::Ref(inner)
                    | Ty::RefMut(inner)
                    | Ty::RefLifetime(_, inner)
                    | Ty::RefMutLifetime(_, inner) => match inner.as_ref() {
                        Ty::TypeParam { bounds, .. }
                        | Ty::SomeMixin(bounds)
                        | Ty::AnyMixin(bounds) => self
                            .unique_bound_impl(bounds)
                            .unwrap_or_else(|| type_name.clone()),
                        _ => type_name.clone(),
                    },
                    _ => type_name.clone(),
                };
                let mangled = format!("{}_{}", resolved_class, method_name);

                // `&mut String` detection: when the receiver is a local
                // of type `&mut String` (i.e. the caller passed `&mut s`
                // into a parameter typed `&mut String`), the local holds
                // a pointer-to-`char*`. Mutating methods must read the
                // current buffer via `riven_deref_ptr`, call the string
                // helper, then write the new buffer back via
                // `riven_store_ptr` so the caller observes the update.
                let receiver_is_mut_string_ref = matches!(
                    &object.ty,
                    Ty::RefMut(inner) | Ty::RefMutLifetime(_, inner)
                        if matches!(inner.as_ref(), Ty::String | Ty::Str)
                );

                // Special handling for push_str on String variables:
                // riven_string_push_str returns a new char*, so we need to
                // capture the return value and reassign it to the object variable.
                if method_name == "push_str" {
                    if receiver_is_mut_string_ref {
                        // `self_arg` here is the pointer value (char**).
                        // We need the pointee to feed into push_str, and
                        // we must store the returned buffer back through
                        // the pointer.
                        let ptr_arg = arg_values[0].clone();
                        let tail_args: Vec<MirValue> = arg_values.iter().skip(1).cloned().collect();
                        let cur = self.new_temp(Ty::String);
                        self.emit(MirInst::Call {
                            dest: Some(cur),
                            callee: "riven_deref_ptr".to_string(),
                            args: vec![ptr_arg.clone()],
                        });
                        let new_buf = self.new_temp(Ty::String);
                        let mut call_args = vec![MirValue::Use(cur)];
                        call_args.extend(tail_args);
                        self.emit(MirInst::Call {
                            dest: Some(new_buf),
                            callee: "String_push_str".to_string(),
                            args: call_args,
                        });
                        self.emit(MirInst::Call {
                            dest: None,
                            callee: "riven_store_ptr".to_string(),
                            args: vec![ptr_arg, MirValue::Use(new_buf)],
                        });
                        return Ok(None);
                    }
                    if let HirExprKind::VarRef(def_id) = &object.kind {
                        if let Some(&obj_var) = self.def_to_local.get(def_id) {
                            let tmp = self.new_temp(Ty::String);
                            self.emit(MirInst::Call {
                                dest: Some(tmp),
                                callee: mangled,
                                args: arg_values,
                            });
                            self.emit(MirInst::Assign {
                                dest: obj_var,
                                value: MirValue::Use(tmp),
                            });
                            return Ok(None);
                        }
                    }
                }

                // Special handling for `String.push(char)`: the runtime
                // only exposes `riven_string_push_str`, so we first widen
                // the Char arg to a one-char heap string via
                // `riven_char_to_string`, then hand that to push_str.
                // Without this rewrite every program that calls
                // `s.push('!')` links against a missing `String_push`.
                //
                // When the receiver is `&mut String` (a parameter), we
                // lower to `*s = String_push_str(*s, one_char_str)` using
                // the deref/store runtime helpers so the caller's local
                // is updated in place.  For an owned local String binding
                // we just rebind the variable to the new buffer.
                if method_name == "push" && resolved_class == "String" && arg_values.len() == 2 {
                    // Phase 2 stdlib batch 2 (#02): route through the
                    // dedicated `riven_string_push(s, codepoint)` runtime
                    // fn rather than synthesising
                    // `riven_char_to_string` + `String_push_str` here.
                    // The dedicated fn allocates exactly one fresh
                    // buffer per call and frees its internal char-string
                    // temporary, so we don't leak the codepoint
                    // intermediate. The prior receiver buffer is freed
                    // here explicitly so the rebind doesn't leak it.
                    let char_arg = arg_values[1].clone();
                    let self_arg = arg_values[0].clone();
                    if receiver_is_mut_string_ref {
                        let cur = self.new_temp(Ty::String);
                        self.emit(MirInst::Call {
                            dest: Some(cur),
                            callee: "riven_deref_ptr".to_string(),
                            args: vec![self_arg.clone()],
                        });
                        let new_buf = self.new_temp(Ty::String);
                        self.emit(MirInst::Call {
                            dest: Some(new_buf),
                            callee: "String_push".to_string(),
                            args: vec![MirValue::Use(cur), char_arg],
                        });
                        // Free the prior buffer before overwriting the
                        // pointer slot, otherwise it leaks.
                        self.emit(MirInst::Call {
                            dest: None,
                            callee: "riven_string_free".to_string(),
                            args: vec![MirValue::Use(cur)],
                        });
                        self.emit(MirInst::Call {
                            dest: None,
                            callee: "riven_store_ptr".to_string(),
                            args: vec![self_arg, MirValue::Use(new_buf)],
                        });
                        return Ok(None);
                    }
                    let new_buf = self.new_temp(Ty::String);
                    self.emit(MirInst::Call {
                        dest: Some(new_buf),
                        callee: "String_push".to_string(),
                        args: vec![self_arg.clone(), char_arg],
                    });
                    if let HirExprKind::VarRef(def_id) = &object.kind {
                        if let Some(&obj_var) = self.def_to_local.get(def_id) {
                            // Free the prior buffer first; the local
                            // owns it (we just lowered it as the self
                            // arg above) and the assignment below is
                            // about to overwrite the slot.
                            self.emit(MirInst::Call {
                                dest: None,
                                callee: "riven_string_free".to_string(),
                                args: vec![MirValue::Use(obj_var)],
                            });
                            self.emit(MirInst::Assign {
                                dest: obj_var,
                                value: MirValue::Use(new_buf),
                            });
                        }
                    }
                    return Ok(None);
                }

                // Phase 2 stdlib: mutating String methods that allocate a
                // fresh buffer (insert, insert_str). Same dance as push_str.
                if matches!(method_name.as_str(), "insert" | "insert_str")
                    && resolved_class == "String"
                {
                    if receiver_is_mut_string_ref {
                        let ptr_arg = arg_values[0].clone();
                        let tail_args: Vec<MirValue> = arg_values.iter().skip(1).cloned().collect();
                        let cur = self.new_temp(Ty::String);
                        self.emit(MirInst::Call {
                            dest: Some(cur),
                            callee: "riven_deref_ptr".to_string(),
                            args: vec![ptr_arg.clone()],
                        });
                        let new_buf = self.new_temp(Ty::String);
                        let mut call_args = vec![MirValue::Use(cur)];
                        call_args.extend(tail_args);
                        self.emit(MirInst::Call {
                            dest: Some(new_buf),
                            callee: format!("String_{}", method_name),
                            args: call_args,
                        });
                        self.emit(MirInst::Call {
                            dest: None,
                            callee: "riven_store_ptr".to_string(),
                            args: vec![ptr_arg, MirValue::Use(new_buf)],
                        });
                        return Ok(None);
                    }
                    if let HirExprKind::VarRef(def_id) = &object.kind {
                        if let Some(&obj_var) = self.def_to_local.get(def_id) {
                            let tmp = self.new_temp(Ty::String);
                            self.emit(MirInst::Call {
                                dest: Some(tmp),
                                callee: format!("String_{}", method_name),
                                args: arg_values,
                            });
                            self.emit(MirInst::Assign {
                                dest: obj_var,
                                value: MirValue::Use(tmp),
                            });
                            return Ok(None);
                        }
                    }
                }

                // String.remove(i) — returns the removed Char and
                // simultaneously rewrites the buffer. The runtime returns
                // a 16-byte struct {removed: i64, new_buffer: ptr}; we read
                // .removed for the value and .new_buffer to update the
                // local / &mut String.
                if method_name == "remove" && resolved_class == "String" {
                    let self_arg = arg_values[0].clone();
                    // For &mut String, we must first deref to get the buf.
                    let buf_arg = if receiver_is_mut_string_ref {
                        let cur = self.new_temp(Ty::String);
                        self.emit(MirInst::Call {
                            dest: Some(cur),
                            callee: "riven_deref_ptr".to_string(),
                            args: vec![self_arg.clone()],
                        });
                        MirValue::Use(cur)
                    } else {
                        self_arg.clone()
                    };
                    let tail_args: Vec<MirValue> = arg_values.iter().skip(1).cloned().collect();
                    let result_struct = self.new_temp(Ty::Int);
                    let mut call_args = vec![buf_arg];
                    call_args.extend(tail_args);
                    self.emit(MirInst::Call {
                        dest: Some(result_struct),
                        callee: "String_remove".to_string(),
                        args: call_args,
                    });
                    // Read the removed Char (field 0 of the 16-byte struct).
                    let removed = self.new_temp(Ty::Char);
                    self.emit(MirInst::GetField {
                        dest: removed,
                        base: result_struct,
                        field_index: 0,
                    });
                    // Read the new buffer (field 1).
                    let new_buf = self.new_temp(Ty::String);
                    self.emit(MirInst::GetField {
                        dest: new_buf,
                        base: result_struct,
                        field_index: 1,
                    });
                    if receiver_is_mut_string_ref {
                        self.emit(MirInst::Call {
                            dest: None,
                            callee: "riven_store_ptr".to_string(),
                            args: vec![self_arg, MirValue::Use(new_buf)],
                        });
                    } else if let HirExprKind::VarRef(def_id) = &object.kind {
                        if let Some(&obj_var) = self.def_to_local.get(def_id) {
                            self.emit(MirInst::Assign {
                                dest: obj_var,
                                value: MirValue::Use(new_buf),
                            });
                        }
                    }
                    return Ok(Some(removed));
                }

                // String.clear / truncate — in-place mutation; for &mut
                // String we must deref to the buffer pointer first.
                if matches!(method_name.as_str(), "clear" | "truncate")
                    && resolved_class == "String"
                {
                    if receiver_is_mut_string_ref {
                        let ptr_arg = arg_values[0].clone();
                        let tail_args: Vec<MirValue> = arg_values.iter().skip(1).cloned().collect();
                        let cur = self.new_temp(Ty::String);
                        self.emit(MirInst::Call {
                            dest: Some(cur),
                            callee: "riven_deref_ptr".to_string(),
                            args: vec![ptr_arg],
                        });
                        let mut call_args = vec![MirValue::Use(cur)];
                        call_args.extend(tail_args);
                        self.emit(MirInst::Call {
                            dest: None,
                            callee: format!("String_{}", method_name),
                            args: call_args,
                        });
                        return Ok(None);
                    }
                    // Owned local: pass the buffer pointer directly.
                    self.emit(MirInst::Call {
                        dest: None,
                        callee: format!("String_{}", method_name),
                        args: arg_values,
                    });
                    return Ok(None);
                }

                let dest = if expr.ty != Ty::Unit && expr.ty != Ty::Never {
                    Some(self.new_temp(expr.ty.clone()))
                } else {
                    None
                };

                // For calls on Fn/FnMut/FnOnce types (closure invocation),
                // emit an indirect call through the function pointer instead
                // of a regular named call.
                let is_fn_type = matches!(
                    &object.ty,
                    Ty::Fn { .. } | Ty::FnMut { .. } | Ty::FnOnce { .. }
                );
                let is_ref_fn_type = matches!(&object.ty,
                    Ty::Ref(inner) | Ty::RefMut(inner)
                    if matches!(inner.as_ref(), Ty::Fn { .. } | Ty::FnMut { .. } | Ty::FnOnce { .. })
                );
                let is_fn_call = is_fn_type
                    || is_ref_fn_type
                    || type_name.starts_with("Fn(")
                    || type_name.starts_with("Fn[")
                    || type_name.starts_with("&Fn(")
                    || type_name.starts_with("&Fn[");

                if is_fn_call {
                    // The closure value is a heap pair {fn_ptr, captures_ptr}.
                    // Load both, then call indirectly with captures_ptr
                    // prepended to the user-visible arg list.
                    let pair = obj_local.unwrap_or_else(|| self.new_temp(Ty::Int));
                    let fn_ptr = self.new_temp(Ty::Int);
                    self.emit(MirInst::GetField {
                        dest: fn_ptr,
                        base: pair,
                        field_index: 0,
                    });
                    let cap_ptr = self.new_temp(Ty::Int);
                    self.emit(MirInst::GetField {
                        dest: cap_ptr,
                        base: pair,
                        field_index: 1,
                    });
                    // Drop the self-as-first-arg that method-call lowering
                    // prepended; replace it with captures_ptr.
                    let user_args: Vec<MirValue> = if !is_static && !arg_values.is_empty() {
                        arg_values.into_iter().skip(1).collect()
                    } else {
                        arg_values
                    };
                    let mut indirect_args = Vec::with_capacity(user_args.len() + 1);
                    indirect_args.push(MirValue::Use(cap_ptr));
                    indirect_args.extend(user_args);
                    self.emit(MirInst::CallIndirect {
                        dest,
                        callee: fn_ptr,
                        args: indirect_args,
                    });
                } else {
                    self.emit(MirInst::Call {
                        dest,
                        callee: mangled,
                        args: arg_values,
                    });
                }
                Ok(dest)
            }

            // ── Assignment ──────────────────────────────────────────
            HirExprKind::Assign { target, value, .. } => {
                let val_local = self.lower_expr(value)?;
                let val = local_to_value(val_local);

                match &target.kind {
                    HirExprKind::VarRef(def_id) => {
                        // Captured variable inside a closure body — must be
                        // ByRef (mutation requires cell-shared storage).
                        if let Some(slot) = self.capture_map.get(def_id).copied() {
                            let cap_ptr = self.captures_ptr_local.unwrap();
                            let cell_ptr = self.new_temp(Ty::Int);
                            self.emit(MirInst::GetField {
                                dest: cell_ptr,
                                base: cap_ptr,
                                field_index: slot.slot_index,
                            });
                            self.emit(MirInst::SetField {
                                base: cell_ptr,
                                field_index: 0,
                                value: val,
                            });
                            return Ok(None);
                        }
                        if let Some(&dest) = self.def_to_local.get(def_id) {
                            if self.cell_promoted.contains(def_id) {
                                // Write-through the cell.
                                self.emit(MirInst::SetField {
                                    base: dest,
                                    field_index: 0,
                                    value: val,
                                });
                            } else {
                                // Re-binding a heap-owned local: free the
                                // prior allocation before the new pointer
                                // overwrites it. (P0.2)
                                if self.initialized_heap_locals.contains(&dest) {
                                    self.emit(MirInst::Call {
                                        dest: None,
                                        callee: "riven_dealloc".to_string(),
                                        args: vec![MirValue::Use(dest)],
                                    });
                                }
                                self.emit(MirInst::Assign { dest, value: val });
                                let dest_ty = self
                                    .fn_ref()
                                    .locals
                                    .iter()
                                    .find(|l| l.id == dest)
                                    .map(|l| l.ty.clone());
                                if matches!(
                                    dest_ty,
                                    Some(Ty::Class { .. })
                                        | Some(Ty::Struct { .. })
                                        | Some(Ty::Enum { .. })
                                ) {
                                    self.initialized_heap_locals.insert(dest);
                                }
                            }
                        }
                    }
                    HirExprKind::FieldAccess {
                        object, field_idx, ..
                    } => {
                        let base_local = self.lower_expr(object)?;
                        if let Some(base) = base_local {
                            self.emit(MirInst::SetField {
                                base,
                                field_index: *field_idx,
                                value: val,
                            });
                        }
                    }
                    _ => {
                        // Other assignment targets (index, etc.) — skip for now
                    }
                }
                Ok(None)
            }

            // ── Compound assignment ─────────────────────────────────
            HirExprKind::CompoundAssign { target, op, value } => {
                let rhs_local = self.lower_expr(value)?;
                let rhs_val = local_to_value(rhs_local);

                // ── Phase 2 stdlib batch 2 (#02): String += String ──
                // Lower as `target = String_push_str(target, value)`.
                // The default integer-add path below would treat the
                // heap pointer operands as i64 and corrupt them.
                //
                // Note: we don't emit an explicit free for the prior
                // buffer here. That mirrors the existing `s.push_str(x)`
                // method-call lowering at line ~1546, which also rebinds
                // without freeing. The known temporary leak is shared
                // by both paths and tracked for a future buffer-owning
                // String redesign; closing it here would diverge from
                // push_str semantics and confuse the leak-tracker tests.
                if matches!(op, BinOp::Add)
                    && matches!(target.ty, Ty::String | Ty::Str)
                    && matches!(value.ty, Ty::String | Ty::Str)
                {
                    if let HirExprKind::VarRef(def_id) = &target.kind {
                        if let Some(&dest) = self.def_to_local.get(def_id) {
                            let new_buf = self.new_temp(Ty::String);
                            self.emit(MirInst::Call {
                                dest: Some(new_buf),
                                callee: "String_push_str".to_string(),
                                args: vec![MirValue::Use(dest), rhs_val],
                            });
                            self.emit(MirInst::Assign {
                                dest,
                                value: MirValue::Use(new_buf),
                            });
                            return Ok(None);
                        }
                    }
                }

                match &target.kind {
                    HirExprKind::VarRef(def_id) => {
                        // Captured variable inside a closure body — load
                        // the current value via the cell, apply the op,
                        // store back through the cell.
                        if let Some(slot) = self.capture_map.get(def_id).copied() {
                            let cap_ptr = self.captures_ptr_local.unwrap();
                            let cell_ptr = self.new_temp(Ty::Int);
                            self.emit(MirInst::GetField {
                                dest: cell_ptr,
                                base: cap_ptr,
                                field_index: slot.slot_index,
                            });
                            let cur = self.new_temp(target.ty.clone());
                            self.emit(MirInst::GetField {
                                dest: cur,
                                base: cell_ptr,
                                field_index: 0,
                            });
                            let tmp = self.new_temp(target.ty.clone());
                            if is_comparison(*op) {
                                self.emit(MirInst::Compare {
                                    dest: tmp,
                                    op: binop_to_cmpop(*op),
                                    lhs: MirValue::Use(cur),
                                    rhs: rhs_val,
                                });
                            } else {
                                self.emit(MirInst::BinOp {
                                    dest: tmp,
                                    op: *op,
                                    lhs: MirValue::Use(cur),
                                    rhs: rhs_val,
                                });
                            }
                            self.emit(MirInst::SetField {
                                base: cell_ptr,
                                field_index: 0,
                                value: MirValue::Use(tmp),
                            });
                            return Ok(None);
                        }
                        if let Some(&dest) = self.def_to_local.get(def_id) {
                            // Cell-promoted local: read-modify-write via cell.
                            if self.cell_promoted.contains(def_id) {
                                let cur = self.new_temp(target.ty.clone());
                                self.emit(MirInst::GetField {
                                    dest: cur,
                                    base: dest,
                                    field_index: 0,
                                });
                                let tmp = self.new_temp(target.ty.clone());
                                if is_comparison(*op) {
                                    self.emit(MirInst::Compare {
                                        dest: tmp,
                                        op: binop_to_cmpop(*op),
                                        lhs: MirValue::Use(cur),
                                        rhs: rhs_val,
                                    });
                                } else {
                                    self.emit(MirInst::BinOp {
                                        dest: tmp,
                                        op: *op,
                                        lhs: MirValue::Use(cur),
                                        rhs: rhs_val,
                                    });
                                }
                                self.emit(MirInst::SetField {
                                    base: dest,
                                    field_index: 0,
                                    value: MirValue::Use(tmp),
                                });
                                return Ok(None);
                            }
                            let lhs_val = MirValue::Use(dest);
                            let tmp = self.new_temp(target.ty.clone());
                            if is_comparison(*op) {
                                self.emit(MirInst::Compare {
                                    dest: tmp,
                                    op: binop_to_cmpop(*op),
                                    lhs: lhs_val,
                                    rhs: rhs_val,
                                });
                            } else {
                                self.emit(MirInst::BinOp {
                                    dest: tmp,
                                    op: *op,
                                    lhs: lhs_val,
                                    rhs: rhs_val,
                                });
                            }
                            self.emit(MirInst::Assign {
                                dest,
                                value: MirValue::Use(tmp),
                            });
                        }
                    }
                    HirExprKind::FieldAccess {
                        object, field_idx, ..
                    } => {
                        let base_local = self.lower_expr(object)?;
                        if let Some(base) = base_local {
                            // Load the current field value.
                            let cur = self.new_temp(target.ty.clone());
                            self.emit(MirInst::GetField {
                                dest: cur,
                                base,
                                field_index: *field_idx,
                            });
                            // Perform the operation.
                            let tmp = self.new_temp(target.ty.clone());
                            if is_comparison(*op) {
                                self.emit(MirInst::Compare {
                                    dest: tmp,
                                    op: binop_to_cmpop(*op),
                                    lhs: MirValue::Use(cur),
                                    rhs: rhs_val,
                                });
                            } else {
                                self.emit(MirInst::BinOp {
                                    dest: tmp,
                                    op: *op,
                                    lhs: MirValue::Use(cur),
                                    rhs: rhs_val,
                                });
                            }
                            // Store the result back.
                            self.emit(MirInst::SetField {
                                base,
                                field_index: *field_idx,
                                value: MirValue::Use(tmp),
                            });
                        }
                    }
                    _ => {
                        // Other compound assignment targets (index, etc.) — skip for now
                    }
                }
                Ok(None)
            }

            // ── Construct (struct/class instantiation) ──────────────
            HirExprKind::Construct { fields, .. } => {
                let dest = self.new_temp(expr.ty.clone());
                self.emit(MirInst::Alloc {
                    dest,
                    ty: expr.ty.clone(),
                    size: self.alloc_size(&expr.ty),
                });
                for (idx, (_name, field_expr)) in fields.iter().enumerate() {
                    let val_local = self.lower_expr(field_expr)?;
                    let val = local_to_value(val_local);
                    self.emit(MirInst::SetField {
                        base: dest,
                        field_index: idx,
                        value: val,
                    });
                }
                Ok(Some(dest))
            }

            // ── Enum variant construction ───────────────────────────
            HirExprKind::EnumVariant {
                variant_idx,
                fields,
                ..
            } => {
                let dest = self.new_temp(expr.ty.clone());
                self.emit(MirInst::Alloc {
                    dest,
                    ty: expr.ty.clone(),
                    size: self.alloc_size(&expr.ty),
                });
                self.emit(MirInst::SetTag {
                    dest,
                    tag: *variant_idx as u32,
                });
                // For variants with data, get a pointer to the payload area
                // (offset 8 after the 4-byte tag + 4 bytes padding), then
                // store fields relative to the payload pointer.
                if !fields.is_empty() {
                    let payload_ptr = self.new_temp(expr.ty.clone());
                    self.emit(MirInst::GetPayload {
                        dest: payload_ptr,
                        src: dest,
                        ty: expr.ty.clone(),
                    });
                    for (idx, (_name, field_expr)) in fields.iter().enumerate() {
                        let val_local = self.lower_expr(field_expr)?;
                        let val = local_to_value(val_local);
                        self.emit(MirInst::SetField {
                            base: payload_ptr,
                            field_index: idx,
                            value: val,
                        });
                    }
                }
                Ok(Some(dest))
            }

            // ── Match ───────────────────────────────────────────────
            HirExprKind::Match { scrutinee, arms } => self.lower_match(expr, scrutinee, arms),

            // ── Field access ────────────────────────────────────────
            HirExprKind::FieldAccess {
                object,
                field_name,
                field_idx,
                ..
            } => {
                // Handle safe navigation `?.field` on Option types.
                // The resolver desugars `x?.field` as FieldAccess with object
                // type Option(...) and result type Option(...). We inline
                // an Option match: if Some, extract inner and call method,
                // otherwise produce None.
                if is_option_type(&object.ty)
                    && is_option_type(&expr.ty)
                    && !matches!(
                        field_name.as_str(),
                        "is_some"
                            | "is_none"
                            | "map"
                            | "unwrap_or"
                            | "unwrap_or_else"
                            | "ok_or"
                            | "unwrap!"
                            | "expect!"
                            | "and_then"
                            | "or"
                            | "filter"
                            | "flatten"
                            | "as_ref"
                            | "take"
                            | "replace"
                    )
                {
                    let opt_local = self.lower_expr(object)?;
                    let opt_id = opt_local.unwrap_or_else(|| self.new_temp(Ty::Int));

                    // Allocate result Option
                    let result = self.new_temp(expr.ty.clone());
                    self.emit(MirInst::Alloc {
                        dest: result,
                        ty: expr.ty.clone(),
                        size: 16,
                    });

                    // Check tag
                    let tag = self.new_temp(Ty::Int32);
                    self.emit(MirInst::GetTag {
                        dest: tag,
                        src: opt_id,
                    });
                    let is_some = self.new_temp(Ty::Bool);
                    self.emit(MirInst::Compare {
                        dest: is_some,
                        op: CmpOp::Eq,
                        lhs: MirValue::Use(tag),
                        rhs: MirValue::Literal(Literal::Int(1)),
                    });

                    let some_block = self.new_block();
                    let none_block = self.new_block();
                    let merge_block = self.new_block();

                    self.set_terminator(Terminator::Branch {
                        cond: MirValue::Use(is_some),
                        then_block: some_block,
                        else_block: none_block,
                    });

                    // Some block: extract payload, call method, wrap in Some
                    self.current_block = some_block;
                    let payload = self.new_temp(Ty::Int);
                    self.emit(MirInst::GetField {
                        dest: payload,
                        base: opt_id,
                        field_index: 1,
                    });

                    // Call the method on the extracted inner value
                    let inner_type_name = match &object.ty {
                        Ty::Option(inner) => type_name_from_ty(inner),
                        _ => String::new(),
                    };
                    // Resolve inherited methods
                    let resolved_class = match &object.ty {
                        Ty::Option(inner) => {
                            let inner_ty = match inner.as_ref() {
                                Ty::Ref(r) | Ty::RefMut(r) => r.as_ref(),
                                other => other,
                            };
                            match inner_ty {
                                Ty::Class { name, .. } => {
                                    self.resolve_method_class(name, field_name)
                                }
                                _ => inner_type_name.clone(),
                            }
                        }
                        _ => inner_type_name.clone(),
                    };
                    let mangled = format!("{}_{}", resolved_class, field_name);
                    // Use the inner type of the result Option for the method result.
                    let inner_result_ty = match &expr.ty {
                        Ty::Option(inner) => inner.as_ref().clone(),
                        _ => Ty::Int,
                    };
                    let method_result = self.new_temp(inner_result_ty);
                    self.emit(MirInst::Call {
                        dest: Some(method_result),
                        callee: mangled,
                        args: vec![MirValue::Use(payload)],
                    });

                    // Wrap in Some
                    self.emit(MirInst::SetTag {
                        dest: result,
                        tag: 1,
                    });
                    self.emit(MirInst::SetField {
                        base: result,
                        field_index: 1,
                        value: MirValue::Use(method_result),
                    });
                    self.set_terminator(Terminator::Goto(merge_block));

                    // None block
                    self.current_block = none_block;
                    self.emit(MirInst::SetTag {
                        dest: result,
                        tag: 0,
                    });
                    self.set_terminator(Terminator::Goto(merge_block));

                    self.current_block = merge_block;
                    return Ok(Some(result));
                }

                // Handle `ClassName.new` (no parentheses) as a constructor
                // call.  The parser resolves this as FieldAccess, but it is
                // semantically equivalent to `ClassName.new()`.
                if field_name == "new" {
                    let type_name = type_name_from_ty(&expr.ty);
                    let base_type = if let Some(pos) = type_name.find('[') {
                        &type_name[..pos]
                    } else {
                        type_name.as_str()
                    };
                    // Phase 2 #06.D2.S0: `Formatter.new()` dispatches to
                    // the runtime constructor just like Vec/Hash.
                    if matches!(
                        base_type,
                        "Vec" | "Array"
                            | "Hash"
                            | "HashMap"
                            | "Map"
                            | "Set"
                            | "HashSet"
                            | "Formatter"
                    ) {
                        let obj = self.new_temp(expr.ty.clone());
                        // ruby-naming.spec.md §3.11 renames stdlib types
                        // (`Vec` → `Array`, `HashMap` → `Map`, `HashSet`
                        // → `Set`). The runtime C functions keep their
                        // legacy names, so map back before mangling.
                        let runtime_base = match base_type {
                            "Array" => "Vec",
                            "Map" => "Hash",
                            "HashMap" => "Hash",
                            "Set" => "HashSet",
                            other => other,
                        };
                        // Use the base type so the mangled callee elides the
                        // generic parameter list (`HashMap[K, V]_new` would
                        // not match a real runtime symbol).
                        self.emit(MirInst::Call {
                            dest: Some(obj),
                            callee: format!("{}_new", runtime_base),
                            args: vec![],
                        });
                        return Ok(Some(obj));
                    }
                    // String.new (no parens) — direct dispatch to the
                    // runtime constructor; see #02 stdlib brief.
                    if base_type == "String" {
                        let obj = self.new_temp(expr.ty.clone());
                        self.emit(MirInst::Call {
                            dest: Some(obj),
                            callee: "String_new".to_string(),
                            args: vec![],
                        });
                        return Ok(Some(obj));
                    }

                    let obj = self.new_temp(expr.ty.clone());
                    self.emit(MirInst::Alloc {
                        dest: obj,
                        ty: expr.ty.clone(),
                        size: self.alloc_size(&expr.ty),
                    });

                    // Structs have no synthetic init — zero-arg `.new` on a
                    // struct leaves fields uninitialised (same as C). Emit
                    // just the allocation.
                    if matches!(&expr.ty, Ty::Struct { .. }) {
                        return Ok(Some(obj));
                    }

                    // Call ClassName_init(self) with no extra args
                    self.emit(MirInst::Call {
                        dest: None,
                        callee: format!("{}_init", type_name),
                        args: vec![MirValue::Use(obj)],
                    });
                    return Ok(Some(obj));
                }

                // Determine whether this FieldAccess is actually a no-arg
                // method call.  The parser produces FieldAccess whenever no
                // parentheses follow the dot, but in Riven method calls can
                // omit parens.
                let obj_type_name = self
                    .receiver_type_name(object)
                    .unwrap_or_else(|| type_name_from_ty(&object.ty));
                // Peel through references to find the underlying class type.
                let base_ty = {
                    let mut ty = &object.ty;
                    loop {
                        match ty {
                            Ty::Ref(inner)
                            | Ty::RefMut(inner)
                            | Ty::RefLifetime(_, inner)
                            | Ty::RefMutLifetime(_, inner) => {
                                ty = inner;
                            }
                            _ => break ty,
                        }
                    }
                };
                let is_field = match base_ty {
                    Ty::Class { name, .. } | Ty::Struct { name, .. } => {
                        self.is_real_field(name, field_name)
                    }
                    // Tuple fields (`.0`, `.1`, ...) are always real fields;
                    // the typechecker has already validated the index.
                    Ty::Tuple(_) => field_name.parse::<usize>().is_ok(),
                    // Newtype wrappers expose the inner value via `.0`.
                    Ty::Newtype { .. } => field_name == "0",
                    _ => false,
                };

                if !is_field && !obj_type_name.is_empty() {
                    // This is a no-arg method call, not a field access.
                    // For static/class methods (`def self.foo`), the callee
                    // takes no `self` parameter, so omit the receiver.
                    let is_static_builtin = is_builtin_static_method(&obj_type_name, field_name)
                        || self.is_user_static_method(&obj_type_name, field_name);
                    let obj_local = self.lower_expr(object)?;

                    let dispatch_ty = if matches!(base_ty, Ty::Infer(_)) {
                        &expr.ty
                    } else {
                        base_ty
                    };
                    let is_static = is_static_builtin
                        || (field_name == "default"
                            && self.type_supports_trait(dispatch_ty, "Default"));
                    let arg_values: Vec<MirValue> = if is_static {
                        Vec::new()
                    } else {
                        vec![local_to_value(obj_local)]
                    };

                    // Resolve through parent classes for inherited methods.
                    // Use base_ty (refs peeled) to find the class name.
                    // For a generic type parameter or impl/dyn Trait,
                    // dispatch to the unique implementor of the trait bound.
                    let resolved_class = match dispatch_ty {
                        Ty::Class { name, .. } => self.resolve_method_class(name, field_name),
                        Ty::TypeParam { bounds, .. }
                        | Ty::SomeMixin(bounds)
                        | Ty::AnyMixin(bounds) => self
                            .unique_bound_impl(bounds)
                            .unwrap_or_else(|| obj_type_name.clone()),
                        _ => obj_type_name.clone(),
                    };
                    let mangled = format!("{}_{}", resolved_class, field_name);

                    let dest = if expr.ty != Ty::Unit && expr.ty != Ty::Never {
                        Some(self.new_temp(expr.ty.clone()))
                    } else {
                        None
                    };

                    self.emit(MirInst::Call {
                        dest,
                        callee: mangled,
                        args: arg_values,
                    });
                    return Ok(dest);
                }

                let base_local = self.lower_expr(object)?;
                if let Some(base) = base_local {
                    let dest = self.new_temp(expr.ty.clone());
                    self.emit(MirInst::GetField {
                        dest,
                        base,
                        field_index: *field_idx,
                    });
                    Ok(Some(dest))
                } else {
                    Ok(None)
                }
            }

            // ── Borrow ──────────────────────────────────────────────
            HirExprKind::Borrow {
                mutable,
                expr: inner,
            } => {
                let src_local = self.lower_expr(inner)?;
                if let Some(src) = src_local {
                    let dest = self.new_temp(expr.ty.clone());
                    if *mutable {
                        self.emit(MirInst::RefMut { dest, src });
                    } else {
                        self.emit(MirInst::Ref { dest, src });
                    }
                    Ok(Some(dest))
                } else {
                    Ok(None)
                }
            }

            // ── String interpolation ────────────────────────────────
            HirExprKind::Interpolation { parts } => self.lower_interpolation(parts, &expr.ty),

            // ── Break / Continue ────────────────────────────────────
            HirExprKind::Break(value) => {
                // Look up the innermost loop. If there is no enclosing
                // loop, treat as a no-op (earlier passes should reject).
                if let Some(frame) = self.loop_stack.last().cloned() {
                    // If a value is provided, lower it and assign into
                    // the loop's result local so the loop expression
                    // evaluates to that value at the exit block.
                    if let Some(val_expr) = value {
                        let val_local = self.lower_expr(val_expr)?;
                        if let Some(dest) = frame.result_local {
                            self.emit(MirInst::Assign {
                                dest,
                                value: local_to_value(val_local),
                            });
                        }
                    }
                    // Free heap-owned locals declared in the loop body
                    // before exiting. (P0.2)
                    self.emit_dealloc_loop_locals(&frame.body_locals);
                    self.set_terminator(Terminator::Goto(frame.break_target));
                    // Any code after `break` in this source block is
                    // unreachable — lower it into a fresh dead block so
                    // subsequent emits don't clobber the terminator we
                    // just set.
                    let dead = self.new_block();
                    self.current_block = dead;
                }
                Ok(None)
            }
            HirExprKind::Continue => {
                if let Some(frame) = self.loop_stack.last().cloned() {
                    self.emit_dealloc_loop_locals(&frame.body_locals);
                    self.set_terminator(Terminator::Goto(frame.continue_target));
                    let dead = self.new_block();
                    self.current_block = dead;
                }
                Ok(None)
            }

            // ── For loop ────────────────────────────────────────────
            HirExprKind::For {
                binding,
                binding_name,
                iterable,
                body,
                tuple_bindings,
            } => {
                // Special case: `for i in start..end` (and `start..=end`).
                // Desugar to a counter-based while loop: evaluate `start`
                // and `end` once each into hidden temporaries, then loop
                // while `i < end` (or `i <= end` for inclusive) and
                // increment by one at the end of each iteration.
                if let HirExprKind::Range {
                    start,
                    end,
                    inclusive,
                } = &iterable.kind
                {
                    let start_expr = start
                        .as_ref()
                        .ok_or_else(|| "for-range requires a start bound".to_string())?;
                    let end_expr = end
                        .as_ref()
                        .ok_or_else(|| "for-range requires an end bound".to_string())?;

                    // Evaluate start and end exactly once.
                    let start_local = self.lower_expr(start_expr)?;
                    let start_val = local_to_value(start_local);
                    let end_local = self.lower_expr(end_expr)?;
                    let end_val = local_to_value(end_local);

                    // Stash end in a hidden temp so we re-use it each header
                    // iteration without re-evaluating the expression.
                    let end_tmp = self.new_temp(Ty::Int);
                    self.emit(MirInst::Assign {
                        dest: end_tmp,
                        value: end_val,
                    });

                    // Create the user-visible loop binding `i` as a mutable
                    // Int local and initialise it with `start`.
                    let binding_local = {
                        let func = self.fn_mut();
                        func.new_local(binding_name.clone(), Ty::Int, true)
                    };
                    self.def_to_local.insert(*binding, binding_local);
                    self.emit(MirInst::Assign {
                        dest: binding_local,
                        value: start_val,
                    });

                    // Blocks: header (cond check), body, step (increment +
                    // back-edge, also the `continue` target), exit.
                    let header_block = self.new_block();
                    let body_block = self.new_block();
                    let step_block = self.new_block();
                    let exit_block = self.new_block();

                    self.set_terminator(Terminator::Goto(header_block));

                    // Header: cond = i < end_tmp (exclusive) or i <= end_tmp.
                    self.current_block = header_block;
                    let cond = self.new_temp(Ty::Bool);
                    self.emit(MirInst::Compare {
                        dest: cond,
                        op: if *inclusive { CmpOp::LtEq } else { CmpOp::Lt },
                        lhs: MirValue::Use(binding_local),
                        rhs: MirValue::Use(end_tmp),
                    });
                    self.set_terminator(Terminator::Branch {
                        cond: MirValue::Use(cond),
                        then_block: body_block,
                        else_block: exit_block,
                    });

                    // Body. `continue` jumps to `step_block` so the counter
                    // is still incremented; `break` jumps to `exit_block`.
                    self.current_block = body_block;
                    self.loop_stack.push(LoopFrame {
                        continue_target: step_block,
                        break_target: exit_block,
                        result_local: None,
                        body_entry_block: body_block,
                        body_locals: Vec::new(),
                    });
                    let _ = self.lower_expr(body)?;
                    let frame = self.loop_stack.pop().expect("loop frame");
                    if matches!(self.get_terminator(), Terminator::Unreachable) {
                        self.emit_dealloc_loop_locals(&frame.body_locals);
                        self.set_terminator(Terminator::Goto(step_block));
                    }
                    self.prepend_zero_init_for_body_locals(&frame);

                    // Step: i = i + 1, then jump back to header.
                    self.current_block = step_block;
                    let next = self.new_temp(Ty::Int);
                    self.emit(MirInst::BinOp {
                        dest: next,
                        op: BinOp::Add,
                        lhs: MirValue::Use(binding_local),
                        rhs: MirValue::Literal(Literal::Int(1)),
                    });
                    self.emit(MirInst::Assign {
                        dest: binding_local,
                        value: MirValue::Use(next),
                    });
                    self.set_terminator(Terminator::Goto(header_block));

                    self.current_block = exit_block;
                    return Ok(None);
                }

                // Fallback: iterate over a Vec-like collection.
                //
                // Lower iterable expression (after iterator no-ops, this
                // is typically a Vec pointer).
                let iter_local = self.lower_expr(iterable)?;
                let iter_id = iter_local.unwrap_or_else(|| self.new_temp(Ty::Int));

                // Index counter: _i = 0
                let idx = self.new_temp(Ty::Int);
                self.emit(MirInst::Assign {
                    dest: idx,
                    value: MirValue::Literal(Literal::Int(0)),
                });

                // Length of the collection.
                // All iterator types (VecIter, VecIntoIter, etc.) are
                // runtime no-ops that pass through the underlying Vec
                // pointer, so we always call Vec runtime ops directly.
                let len = self.new_temp(Ty::Int);
                self.emit(MirInst::Call {
                    dest: Some(len),
                    callee: "riven_vec_len".to_string(),
                    args: vec![MirValue::Use(iter_id)],
                });

                // Create blocks: header, body, step, exit
                let header_block = self.fn_mut().new_block();
                let body_block = self.fn_mut().new_block();
                let step_block = self.fn_mut().new_block();
                let exit_block = self.fn_mut().new_block();

                // Jump to header from current block
                self.set_terminator(Terminator::Goto(header_block));
                self.current_block = header_block;

                // Header: cond = idx < len
                let cond = self.new_temp(Ty::Bool);
                self.emit(MirInst::Compare {
                    dest: cond,
                    op: CmpOp::Lt,
                    lhs: MirValue::Use(idx),
                    rhs: MirValue::Use(len),
                });
                self.set_terminator(Terminator::Branch {
                    cond: MirValue::Use(cond),
                    then_block: body_block,
                    else_block: exit_block,
                });

                // Body: binding = vec_get(iter_id, idx)
                self.current_block = body_block;

                // Create the binding variable.
                // Determine element type from the iterable's type.
                let binding_ty = element_type_of(&iterable.ty);
                let binding_local = {
                    let func = self.fn_mut();

                    func.new_local(binding_name.clone(), binding_ty, false)
                };
                self.def_to_local.insert(*binding, binding_local);

                self.emit(MirInst::Call {
                    dest: Some(binding_local),
                    callee: "riven_vec_get".to_string(),
                    args: vec![MirValue::Use(iter_id), MirValue::Use(idx)],
                });

                // For tuple destructuring patterns like (i, result) from
                // .enumerate(), wire up the sub-bindings.
                if !tuple_bindings.is_empty() {
                    for (tb_idx, (tb_def_id, tb_name)) in tuple_bindings.iter().enumerate() {
                        let tb_local = {
                            let func = self.fn_mut();
                            func.new_local(tb_name.clone(), Ty::Int, false)
                        };
                        self.def_to_local.insert(*tb_def_id, tb_local);

                        if tb_idx == 0 {
                            // First element of enumerate tuple = index
                            self.emit(MirInst::Assign {
                                dest: tb_local,
                                value: MirValue::Use(idx),
                            });
                        } else {
                            // Second element = the actual Vec element
                            self.emit(MirInst::Assign {
                                dest: tb_local,
                                value: MirValue::Use(binding_local),
                            });
                        }
                    }
                }

                // Lower body. `continue` jumps to `step_block` so the
                // index is still incremented; `break` jumps to `exit_block`.
                self.loop_stack.push(LoopFrame {
                    continue_target: step_block,
                    break_target: exit_block,
                    result_local: None,
                    body_entry_block: body_block,
                    body_locals: Vec::new(),
                });
                self.lower_expr(body)?;
                let frame = self.loop_stack.pop().expect("loop frame");

                if matches!(self.get_terminator(), Terminator::Unreachable) {
                    self.emit_dealloc_loop_locals(&frame.body_locals);
                    self.set_terminator(Terminator::Goto(step_block));
                }
                self.prepend_zero_init_for_body_locals(&frame);

                // Step: increment index and jump back to header.
                self.current_block = step_block;
                let next_idx = self.new_temp(Ty::Int);
                self.emit(MirInst::BinOp {
                    dest: next_idx,
                    op: BinOp::Add,
                    lhs: MirValue::Use(idx),
                    rhs: MirValue::Literal(Literal::Int(1)),
                });
                self.emit(MirInst::Assign {
                    dest: idx,
                    value: MirValue::Use(next_idx),
                });

                // Jump back to header
                self.set_terminator(Terminator::Goto(header_block));

                // Continue in exit block
                self.current_block = exit_block;

                Ok(None)
            }

            // ── Closure ─────────────────────────────────────────────
            HirExprKind::Closure {
                params,
                body,
                is_move,
                ..
            } => {
                // Closure layout (heap-allocated, 16 bytes):
                //   [0] fn_ptr       — address of the synthesized function
                //   [8] captures_ptr — heap block holding captured values
                //                      (one 8-byte slot per capture). May
                //                      be NULL when the closure captures
                //                      nothing.
                //
                // Each capture slot holds either the value directly
                // (ByValue — move or Copy) or a pointer to a single-slot
                // heap cell shared with the enclosing frame (ByRef —
                // used for `let mut` variables the closure mutates).
                let closure_name = format!("__closure_{}", self.closure_counter);
                self.closure_counter += 1;

                // Collect captured def_ids by walking the body.  A def is
                // captured when it is referenced but not defined inside
                // the closure body or declared as a closure parameter.
                let param_def_ids: HashSet<DefId> = params.iter().map(|p| p.def_id).collect();
                let mut captured_def_ids: Vec<DefId> = Vec::new();
                let mut seen: HashSet<DefId> = HashSet::new();
                collect_captures(
                    body,
                    &param_def_ids,
                    &self.def_to_local,
                    &mut captured_def_ids,
                    &mut seen,
                );

                // Decide storage kind per capture.  Copy-typed values
                // can always be captured by value; moved/Copy values go
                // inline; non-move captures of a mutable local that is
                // assigned inside the closure body go through a cell.
                let mut slots: Vec<(DefId, LocalId, Ty, CaptureKind)> =
                    Vec::with_capacity(captured_def_ids.len());
                for def in &captured_def_ids {
                    let outer_local = *self.def_to_local.get(def).unwrap();
                    let ty = self.fn_mut().locals[outer_local as usize].ty.clone();
                    let mutates = closure_body_mutates(body, *def);
                    let kind = if *is_move || !mutates {
                        CaptureKind::ByValue
                    } else {
                        CaptureKind::ByRef
                    };
                    slots.push((*def, outer_local, ty, kind));
                }

                // Cell-promote any captured `let mut` that will be shared
                // by-reference: load the current value into a fresh cell,
                // then rewrite the outer local to hold the cell pointer.
                // From this point on, reads/writes to the outer local go
                // through the cell (see `cell_promoted`).  We only do
                // this once per local — if it's already been promoted by
                // a previous closure in the same function, reuse it.
                for (def, outer_local, _ty, kind) in &slots {
                    if *kind == CaptureKind::ByRef && !self.cell_promoted.contains(def) {
                        let cell = self.new_temp(Ty::Int);
                        self.emit(MirInst::Alloc {
                            dest: cell,
                            ty: Ty::Int,
                            size: 8,
                        });
                        // Store the current value of the local into the cell.
                        self.emit(MirInst::SetField {
                            base: cell,
                            field_index: 0,
                            value: MirValue::Use(*outer_local),
                        });
                        // Rewrite the outer local to hold the cell pointer.
                        self.emit(MirInst::Assign {
                            dest: *outer_local,
                            value: MirValue::Use(cell),
                        });
                        self.cell_promoted.insert(*def);
                    }
                }

                // Allocate the captures struct (or NULL if no captures).
                let captures_ptr = if slots.is_empty() {
                    None
                } else {
                    let cap = self.new_temp(Ty::Int);
                    let size = (slots.len() * 8).max(8);
                    self.emit(MirInst::Alloc {
                        dest: cap,
                        ty: Ty::Int,
                        size,
                    });
                    for (slot_idx, (_def, outer_local, _ty, kind)) in slots.iter().enumerate() {
                        match kind {
                            CaptureKind::ByValue => {
                                // For already-cell-promoted defs, the outer
                                // local is a cell pointer — load the value
                                // out of the cell before storing.  (This
                                // covers the niche case of a ByValue capture
                                // of a local promoted by an earlier closure.)
                                let src_val = if self.cell_promoted.contains(&slots[slot_idx].0) {
                                    let tmp = self.new_temp(Ty::Int);
                                    self.emit(MirInst::GetField {
                                        dest: tmp,
                                        base: *outer_local,
                                        field_index: 0,
                                    });
                                    MirValue::Use(tmp)
                                } else {
                                    MirValue::Use(*outer_local)
                                };
                                self.emit(MirInst::SetField {
                                    base: cap,
                                    field_index: slot_idx,
                                    value: src_val,
                                });
                            }
                            CaptureKind::ByRef => {
                                // Outer local already holds the cell pointer
                                // (we promoted it above).  Just copy the
                                // pointer into the captures slot.
                                self.emit(MirInst::SetField {
                                    base: cap,
                                    field_index: slot_idx,
                                    value: MirValue::Use(*outer_local),
                                });
                            }
                        }
                    }
                    Some(cap)
                };

                // Build the synthesized closure function.  First parameter
                // is the captures pointer (may be NULL for no captures).
                let ret_ty = body.ty.clone();
                let mut closure_fn = MirFunction::new(&closure_name, ret_ty);
                let cap_param = closure_fn.new_local("__captures".to_string(), Ty::Int, false);
                closure_fn.params.push(cap_param);
                let mut closure_param_ids: Vec<LocalId> = Vec::with_capacity(params.len());
                for param in params {
                    let local_id =
                        closure_fn.new_local(param.name.clone(), param.ty.clone(), false);
                    closure_fn.params.push(local_id);
                    closure_param_ids.push(local_id);
                }

                // Save the current lowerer state, lower the closure body
                // in the context of the new function, then restore.
                let saved_fn = self.current_fn.take();
                let saved_block = self.current_block;
                let saved_defs = self.def_to_local.clone();
                let saved_capture_map = std::mem::take(&mut self.capture_map);
                let saved_captures_ptr = self.captures_ptr_local.take();
                let saved_cell_promoted = std::mem::take(&mut self.cell_promoted);

                self.current_fn = Some(closure_fn);
                self.current_block = 0;
                self.captures_ptr_local = if slots.is_empty() {
                    None
                } else {
                    Some(cap_param)
                };

                // Clear def_to_local: only closure params (and captures
                // via the capture map) should be visible inside the body.
                self.def_to_local.clear();
                self.initialized_heap_locals.clear();
                for (i, param) in params.iter().enumerate() {
                    self.def_to_local.insert(param.def_id, closure_param_ids[i]);
                }
                // Populate the capture map.  ByRef captures are visible
                // as cell-promoted defs inside the closure body too — any
                // read/write on them goes through the cell.
                for (slot_idx, (def, _outer, _ty, kind)) in slots.iter().enumerate() {
                    self.capture_map.insert(
                        *def,
                        CaptureSlot {
                            slot_index: slot_idx,
                            kind: *kind,
                        },
                    );
                    if *kind == CaptureKind::ByRef {
                        self.cell_promoted.insert(*def);
                    }
                }

                // Lower the closure body.
                let body_result = self.lower_expr(body)?;
                let ret_is_unit = matches!(body.ty, Ty::Unit | Ty::Never);
                if body_result.is_some() && !ret_is_unit {
                    let body_val = local_to_value(body_result);
                    self.set_terminator(Terminator::Return(Some(body_val)));
                } else {
                    self.set_terminator(Terminator::Return(None));
                }

                // Extract the completed closure function.
                let completed_fn = self.current_fn.take().unwrap();
                self.pending_closures.push(completed_fn);

                // Restore the parent function state.
                self.current_fn = saved_fn;
                self.current_block = saved_block;
                self.def_to_local = saved_defs;
                self.capture_map = saved_capture_map;
                self.captures_ptr_local = saved_captures_ptr;
                self.cell_promoted = saved_cell_promoted;

                // Build the closure pair {fn_ptr, captures_ptr}.
                let fn_ptr = self.new_temp(Ty::Int);
                self.emit(MirInst::FuncAddr {
                    dest: fn_ptr,
                    func_name: closure_name,
                });
                let pair = self.new_temp(expr.ty.clone());
                self.emit(MirInst::Alloc {
                    dest: pair,
                    ty: expr.ty.clone(),
                    size: 16,
                });
                self.emit(MirInst::SetField {
                    base: pair,
                    field_index: 0,
                    value: MirValue::Use(fn_ptr),
                });
                let cap_val = match captures_ptr {
                    Some(cp) => MirValue::Use(cp),
                    None => MirValue::Literal(Literal::Int(0)),
                };
                self.emit(MirInst::SetField {
                    base: pair,
                    field_index: 1,
                    value: cap_val,
                });
                Ok(Some(pair))
            }

            // ── Tuple ───────────────────────────────────────────────
            HirExprKind::Tuple(elems) => {
                let dest = self.new_temp(expr.ty.clone());
                self.emit(MirInst::Alloc {
                    dest,
                    ty: expr.ty.clone(),
                    size: self.alloc_size(&expr.ty),
                });
                for (idx, elem) in elems.iter().enumerate() {
                    let val_local = self.lower_expr(elem)?;
                    let val = local_to_value(val_local);
                    self.emit(MirInst::SetField {
                        base: dest,
                        field_index: idx,
                        value: val,
                    });
                }
                Ok(Some(dest))
            }

            // ── Index ───────────────────────────────────────────────
            HirExprKind::Index { object, index } => {
                // Fixed-size arrays `[T; N]` are laid out as N consecutive
                // 8-byte slots (the layout used by Alloc + SetField above).
                // When the index is a compile-time integer literal we can
                // lower `a[i]` to a direct `GetField { field_index: i }`.
                if matches!(object.ty, Ty::FixedArray(_, _)) {
                    if let HirExprKind::IntLiteral(n) = &index.kind {
                        let base_local = self.lower_expr(object)?;
                        if let Some(base) = base_local {
                            let dest = self.new_temp(expr.ty.clone());
                            self.emit(MirInst::GetField {
                                dest,
                                base,
                                field_index: *n as usize,
                            });
                            return Ok(Some(dest));
                        }
                    }
                }
                // ── Phase 2 stdlib batch 1 (#03): Vec[i] ──
                // Indexing a Vec at runtime panics on OOB with a
                // descriptive message ("index N out of range, len M").
                // The runtime fn returns the raw 64-bit slot; the
                // typeck-emitted result type pulls out the element T.
                if matches!(object.ty, Ty::Array(_))
                    || matches!(
                        &object.ty,
                        Ty::Ref(inner) | Ty::RefMut(inner)
                            if matches!(inner.as_ref(), Ty::Array(_))
                    )
                {
                    let base_local = self.lower_expr(object)?;
                    let idx_local = self.lower_expr(index)?;
                    let base_val = local_to_value(base_local);
                    let idx_val = local_to_value(idx_local);
                    let dest = self.new_temp(expr.ty.clone());
                    self.emit(MirInst::Call {
                        dest: Some(dest),
                        callee: "riven_vec_get_or_panic".to_string(),
                        args: vec![base_val, idx_val],
                    });
                    return Ok(Some(dest));
                }
                // ── Phase 2 stdlib batch 3 (#04): HashMap[&K] ──
                // `m[k]` panics on missing keys via `riven_hash_index`
                // (mirrors `riven_vec_get_or_panic` for Vec). The
                // surface type is V (set in typeck::infer_index_ty);
                // runtime returns the raw 64-bit value slot.
                if matches!(object.ty, Ty::Map(_, _))
                    || matches!(
                        &object.ty,
                        Ty::Ref(inner) | Ty::RefMut(inner)
                            if matches!(inner.as_ref(), Ty::Map(_, _))
                    )
                {
                    let base_local = self.lower_expr(object)?;
                    let idx_local = self.lower_expr(index)?;
                    let base_val = local_to_value(base_local);
                    let idx_val = local_to_value(idx_local);
                    let dest = self.new_temp(expr.ty.clone());
                    self.emit(MirInst::Call {
                        dest: Some(dest),
                        callee: "riven_hash_index".to_string(),
                        args: vec![base_val, idx_val],
                    });
                    return Ok(Some(dest));
                }
                // Dynamic index / other collection kinds still need runtime
                // support; fall through as a no-op.
                let _ = (object, index);
                Ok(None)
            }

            // ── Cast ────────────────────────────────────────────────
            HirExprKind::Cast { expr: inner, .. } => {
                // For now, pass through the inner expression.
                self.lower_expr(inner)
            }

            // ── Array literal ───────────────────────────────────────
            HirExprKind::ArrayLiteral(elems) => {
                let dest = self.new_temp(expr.ty.clone());
                self.emit(MirInst::Alloc {
                    dest,
                    ty: expr.ty.clone(),
                    size: self.alloc_size(&expr.ty),
                });
                for (idx, elem) in elems.iter().enumerate() {
                    let val_local = self.lower_expr(elem)?;
                    let val = local_to_value(val_local);
                    self.emit(MirInst::SetField {
                        base: dest,
                        field_index: idx,
                        value: val,
                    });
                }
                Ok(Some(dest))
            }

            // ── Macro calls (vec![], hash!{}, etc.) ──────────────────
            HirExprKind::MacroCall { name, args } => {
                // ruby-naming.spec.md §10a renames the collection macros:
                //   `vec![…]` → `array![…]`
                //   `hash!{…}` → `map!{…}`
                //   `set!{…}` (unchanged)
                // Both names lower identically; rename happens at the
                // surface only.
                match name.as_str() {
                    "vec" | "array" => {
                        // Lower `vec![a, b, c]` to:
                        //   let v = Vec.new()
                        //   v.push(a)
                        //   v.push(b)
                        //   v.push(c)
                        let vec_ty = expr.ty.clone();
                        let dest = self.new_temp(vec_ty.clone());
                        // Determine the mangled Vec.new callee name from
                        // the result type.
                        let vec_new_name = format!("{}_new", type_name_from_ty(&vec_ty));
                        self.emit(MirInst::Call {
                            dest: Some(dest),
                            callee: vec_new_name,
                            args: vec![],
                        });

                        let vec_push_name = format!("{}_push", type_name_from_ty(&vec_ty));
                        for arg_expr in args {
                            let arg_local = self.lower_expr(arg_expr)?;
                            let arg_val = local_to_value(arg_local);
                            self.emit(MirInst::Call {
                                dest: None,
                                callee: vec_push_name.clone(),
                                args: vec![MirValue::Use(dest), arg_val],
                            });
                        }
                        Ok(Some(dest))
                    }
                    // Lower `hash!{ k1 => v1, k2 => v2 }` (args flattened to
                    // [k1, v1, k2, v2]) into a Hash.new + repeated inserts.
                    // `map!{…}` (post-rename surface form) shares the same
                    // lowering — see the comment block above.
                    "hash" | "map" => {
                        let hash_ty = expr.ty.clone();
                        let dest = self.new_temp(hash_ty.clone());
                        let hash_new_name = format!("{}_new", type_name_from_ty(&hash_ty));
                        self.emit(MirInst::Call {
                            dest: Some(dest),
                            callee: hash_new_name,
                            args: vec![],
                        });
                        let hash_insert_name = format!("{}_insert", type_name_from_ty(&hash_ty));
                        let mut iter = args.iter();
                        while let (Some(k_expr), Some(v_expr)) = (iter.next(), iter.next()) {
                            let k_local = self.lower_expr(k_expr)?;
                            let v_local = self.lower_expr(v_expr)?;
                            let k_val = local_to_value(k_local);
                            let v_val = local_to_value(v_local);
                            self.emit(MirInst::Call {
                                dest: None,
                                callee: hash_insert_name.clone(),
                                args: vec![MirValue::Use(dest), k_val, v_val],
                            });
                        }
                        Ok(Some(dest))
                    }
                    // Lower `set!{ a, b, c }` into a Set.new + repeated inserts.
                    "set" => {
                        let set_ty = expr.ty.clone();
                        let dest = self.new_temp(set_ty.clone());
                        let set_new_name = format!("{}_new", type_name_from_ty(&set_ty));
                        self.emit(MirInst::Call {
                            dest: Some(dest),
                            callee: set_new_name,
                            args: vec![],
                        });
                        let set_insert_name = format!("{}_insert", type_name_from_ty(&set_ty));
                        for arg_expr in args {
                            let arg_local = self.lower_expr(arg_expr)?;
                            let arg_val = local_to_value(arg_local);
                            self.emit(MirInst::Call {
                                dest: None,
                                callee: set_insert_name.clone(),
                                args: vec![MirValue::Use(dest), arg_val],
                            });
                        }
                        Ok(Some(dest))
                    }
                    // `panic!("msg")` — evaluate the message (which may be
                    // an interpolated string), call `riven_panic(msg)`, and
                    // set the current block's terminator to `Unreachable`
                    // so that no code after the panic is executed.
                    "panic" => {
                        let arg_val = if let Some(first) = args.first() {
                            let local = self.lower_expr(first)?;
                            local_to_value(local)
                        } else {
                            // panic! with no message — pass an empty string.
                            let empty = self.new_temp(Ty::String);
                            self.emit(MirInst::Assign {
                                dest: empty,
                                value: MirValue::Literal(Literal::String(String::new())),
                            });
                            MirValue::Use(empty)
                        };
                        self.emit(MirInst::Call {
                            dest: None,
                            callee: "riven_panic".to_string(),
                            args: vec![arg_val],
                        });
                        self.set_terminator(Terminator::Unreachable);
                        // Create a dead block for any code after the panic.
                        let dead = self.new_block();
                        self.current_block = dead;
                        Ok(None)
                    }
                    _ => Ok(None),
                }
            }

            // ── Unsafe block — lower identically to a regular block ──
            HirExprKind::UnsafeBlock(stmts, tail) => {
                for stmt in stmts {
                    self.lower_statement(stmt)?;
                }
                if let Some(tail_expr) = tail {
                    self.lower_expr(tail_expr)
                } else {
                    Ok(None)
                }
            }

            // ── `nil` literal ─────────────────────────────────────────
            // ruby-naming.spec.md §3.10: `nil` is polymorphic. Lowering
            // splits on the resolved type:
            //   * `Option[T]` → construct the `None` variant (tag 0).
            //   * Anything else (raw pointer, USize, UInt64) → zero
            //     value, matching the legacy `null` semantics.
            HirExprKind::NullLiteral => {
                if let Ty::Option(_) = &expr.ty {
                    let dest = self.new_temp(expr.ty.clone());
                    self.emit(MirInst::Alloc {
                        dest,
                        ty: expr.ty.clone(),
                        size: self.alloc_size(&expr.ty),
                    });
                    self.emit(MirInst::SetTag { dest, tag: 0 });
                    Ok(Some(dest))
                } else {
                    let dest = self.new_temp(expr.ty.clone());
                    self.emit(MirInst::Assign {
                        dest,
                        value: MirValue::Literal(Literal::Int(0)),
                    });
                    Ok(Some(dest))
                }
            }

            // ── Catch-all for unhandled expressions ─────────────────
            HirExprKind::ArrayFill { .. } | HirExprKind::Range { .. } | HirExprKind::Error => {
                Ok(None)
            }
        }
    }

    // ── Statement lowering ──────────────────────────────────────────────

    fn lower_statement(&mut self, stmt: &HirStatement) -> Result<(), String> {
        match stmt {
            HirStatement::Let {
                def_id,
                ty,
                value,
                mutable,
                pattern,
                ..
            } => {
                // Handle tuple destructuring: `let (a, b) = expr`
                if let HirPattern::Tuple { elements, .. } = pattern {
                    // Lower the initializer first
                    let init_local = if let Some(init) = value {
                        self.lower_expr(init)?
                    } else {
                        None
                    };
                    let tuple_id = init_local.unwrap_or_else(|| self.new_temp(ty.clone()));

                    // Create a local for the whole tuple binding
                    let tuple_local = self.new_local_named("_tuple", ty.clone(), *mutable);
                    self.def_to_local.insert(*def_id, tuple_local);
                    self.emit(MirInst::Assign {
                        dest: tuple_local,
                        value: MirValue::Use(tuple_id),
                    });

                    // Extract each element via GetField
                    for (i, elem_pat) in elements.iter().enumerate() {
                        if let HirPattern::Binding {
                            def_id: elem_def,
                            name: elem_name,
                            ..
                        } = elem_pat
                        {
                            let elem_ty = match ty {
                                Ty::Tuple(tys) if i < tys.len() => tys[i].clone(),
                                _ => Ty::Int,
                            };
                            let elem_local = self.new_local_named(elem_name, elem_ty, *mutable);
                            self.def_to_local.insert(*elem_def, elem_local);
                            self.emit(MirInst::GetField {
                                dest: elem_local,
                                base: tuple_id,
                                field_index: i,
                            });
                        }
                    }
                    return Ok(());
                }

                // Extract the name from the pattern (use the binding name if
                // it is a simple Binding pattern, otherwise fall back to the
                // symbol table).
                let name = match pattern {
                    HirPattern::Binding { name, .. } => name.clone(),
                    _ => def_id_name(*def_id, self.symbols),
                };

                // Refine unresolved Infer types: if the initializer is a
                // method call known to return a string, use Ty::String
                // instead.  This ensures correct string interpolation for
                // variables like `let task_name = ... .unwrap_or(String.from(...))`.
                let refined_ty = if matches!(ty, Ty::Infer(_)) {
                    if let Some(init_expr) = value {
                        if is_inferred_string_expr(init_expr) {
                            Ty::String
                        } else {
                            ty.clone()
                        }
                    } else {
                        ty.clone()
                    }
                } else {
                    ty.clone()
                };

                let local = self.new_local_named(&name, refined_ty.clone(), *mutable);
                self.def_to_local.insert(*def_id, local);

                if let Some(init) = value {
                    let val_local = self.lower_expr(init)?;
                    // Owned rebinding of a VarRef must emit `MirInst::Move`
                    // so drop-elaboration tracks ownership correctly.
                    if let (HirExprKind::VarRef(_), Some(src)) = (&init.kind, val_local) {
                        self.emit_transfer(local, src, &init.ty, init.ty.move_semantics());
                    } else {
                        let val = local_to_value(val_local);
                        self.emit(MirInst::Assign {
                            dest: local,
                            value: val,
                        });
                    }
                    if matches!(
                        refined_ty,
                        Ty::Class { .. } | Ty::Struct { .. } | Ty::Enum { .. }
                    ) {
                        self.initialized_heap_locals.insert(local);
                        if let Some(frame) = self.loop_stack.last_mut() {
                            frame.body_locals.push(local);
                        }
                    }
                }
                Ok(())
            }
            HirStatement::Expr(expr) => {
                let _ = self.lower_expr(expr)?;
                Ok(())
            }
        }
    }

    // ── Match lowering ──────────────────────────────────────────────────

    fn lower_match(
        &mut self,
        expr: &HirExpr,
        scrutinee: &HirExpr,
        arms: &[HirMatchArm],
    ) -> Result<Option<LocalId>, String> {
        let scrut_local = self.lower_expr(scrutinee)?;

        // For enum-like types (Enum, Result, Option), use tag-based
        // switch. Also treat unresolved Infer types as enum if any arm
        // uses an Enum pattern (e.g., Ok/Err, Some/None).
        let is_enum = matches!(
            scrutinee.ty,
            Ty::Enum { .. } | Ty::Result(_, _) | Ty::Option(_)
        ) || arms
            .iter()
            .any(|arm| matches!(arm.pattern, HirPattern::Enum { .. }));

        let merge_block = self.new_block();
        let result_local = if expr.ty != Ty::Unit && expr.ty != Ty::Never {
            Some(self.new_temp(expr.ty.clone()))
        } else {
            None
        };

        if is_enum {
            // Get the discriminant tag.
            let scrut = scrut_local.unwrap_or_else(|| {
                // Scrutinee didn't produce a local (e.g. Unit expression).
                // Create a zero-valued temporary as a fallback.
                let tmp = self.new_temp(scrutinee.ty.clone());
                self.emit(MirInst::Assign {
                    dest: tmp,
                    value: MirValue::Literal(Literal::Int(0)),
                });
                tmp
            });
            let tag_local = self.new_temp(Ty::Int32);
            self.emit(MirInst::GetTag {
                dest: tag_local,
                src: scrut,
            });

            // Build switch targets. Every arm gets its own entry block
            // so arms with guards can fall through to the next arm on a
            // failed guard, and multiple arms targeting the same
            // variant can be chained in source order (first matching-
            // and-guard-true arm wins).
            let mut targets: Vec<(i64, BlockId)> = Vec::new();
            let otherwise = self.new_block(); // fallback / wildcard
            let mut seen_variants: HashMap<i64, BlockId> = HashMap::new();

            // Pre-allocate an entry block for every arm. The first
            // wildcard / binding arm lives directly in `otherwise` so
            // the switch can land there without an extra hop.
            let mut arm_entry_blocks: Vec<BlockId> = Vec::with_capacity(arms.len());
            let mut first_wildcard_placed = false;
            for arm in arms.iter() {
                let is_wild = !matches!(arm.pattern, HirPattern::Enum { .. });
                let block = if is_wild && !first_wildcard_placed {
                    first_wildcard_placed = true;
                    otherwise
                } else {
                    self.new_block()
                };
                arm_entry_blocks.push(block);
            }

            // Compute each arm's fallthrough target: where control
            // transfers when the arm's pattern or guard fails. For an
            // enum arm, fallthrough is the next arm whose pattern could
            // still match this variant (same variant index, or a
            // wildcard / binding arm that matches anything). Falling
            // off the end lands on `otherwise`.
            let mut arm_fallthroughs: Vec<BlockId> = Vec::with_capacity(arms.len());
            for (i, arm) in arms.iter().enumerate() {
                let this_variant = match &arm.pattern {
                    HirPattern::Enum { variant_idx, .. } => Some(*variant_idx as i64),
                    _ => None,
                };
                let mut target = otherwise;
                for (j, other) in arms.iter().enumerate().skip(i + 1) {
                    match &other.pattern {
                        HirPattern::Enum { variant_idx, .. } => {
                            if Some(*variant_idx as i64) == this_variant {
                                target = arm_entry_blocks[j];
                                break;
                            }
                        }
                        _ => {
                            // Wildcard/binding — matches any variant.
                            target = arm_entry_blocks[j];
                            break;
                        }
                    }
                }
                arm_fallthroughs.push(target);
            }

            let mut arm_blocks: Vec<(BlockId, &HirMatchArm)> = Vec::new();
            let mut wildcard_arm: Option<(BlockId, usize)> = None;

            for (arm_idx, arm) in arms.iter().enumerate() {
                let arm_block = arm_entry_blocks[arm_idx];
                if let HirPattern::Enum { variant_idx, .. } = &arm.pattern {
                    let disc = *variant_idx as i64;
                    seen_variants.entry(disc).or_insert_with(|| {
                        targets.push((disc, arm_block));
                        arm_block
                    });
                    arm_blocks.push((arm_block, arm));
                } else {
                    // Wildcard / binding — first one lives at
                    // `otherwise`; later ones are reached only via
                    // fallthrough from a preceding arm's failed guard.
                    wildcard_arm = Some((otherwise, arm_idx));
                    arm_blocks.push((arm_block, arm));
                }
            }

            self.set_terminator(Terminator::Switch {
                value: MirValue::Use(tag_local),
                targets,
                otherwise,
            });

            // Lower each arm body.
            for (arm_idx, (arm_block, arm)) in arm_blocks.iter().enumerate() {
                self.current_block = *arm_block;

                // Bind pattern variables if it's an Enum pattern with field bindings.
                if let HirPattern::Enum {
                    type_def,
                    variant_idx,
                    fields,
                    ..
                } = &arm.pattern
                {
                    if !fields.is_empty() {
                        // For Option/Result, derive field types from the scrutinee type
                        // since the variant definitions use TypeParam placeholders.
                        let variant_field_types = match &scrutinee.ty {
                            Ty::Option(inner) if *variant_idx == 0 => {
                                // Some(T) — the field type is the inner type
                                vec![*inner.clone()]
                            }
                            Ty::Result(ok, _err) if *variant_idx == 0 => {
                                // Ok(T) — the field type is the ok type
                                vec![*ok.clone()]
                            }
                            Ty::Result(_ok, err) if *variant_idx == 1 => {
                                // Err(E) — the field type is the error type
                                vec![*err.clone()]
                            }
                            _ => self.lookup_variant_field_types(*type_def, *variant_idx),
                        };

                        // Get the payload pointer (offset 8 from enum base).
                        let payload_ptr = self.new_temp(scrutinee.ty.clone());
                        self.emit(MirInst::GetPayload {
                            dest: payload_ptr,
                            src: scrut,
                            ty: scrutinee.ty.clone(),
                        });

                        for (idx, field_pat) in fields.iter().enumerate() {
                            let binding_info = match field_pat {
                                HirPattern::Binding {
                                    def_id,
                                    name,
                                    mutable,
                                    ..
                                } => Some((*def_id, name.as_str(), *mutable)),
                                HirPattern::Ref {
                                    def_id,
                                    name,
                                    mutable,
                                    ..
                                } => {
                                    // `ref` pattern: bind a reference to
                                    // the field. At runtime references are
                                    // the same representation as values for
                                    // heap types, so treat identically to
                                    // Binding for code generation purposes.
                                    Some((*def_id, name.as_str(), *mutable))
                                }
                                _ => None,
                            };
                            if let Some((def_id, name, mutable)) = binding_info {
                                let field_ty =
                                    variant_field_types.get(idx).cloned().unwrap_or(Ty::Int);
                                let local = self.new_local_named(name, field_ty, mutable);
                                self.def_to_local.insert(def_id, local);
                                self.emit(MirInst::GetField {
                                    dest: local,
                                    base: payload_ptr,
                                    field_index: idx,
                                });
                            }

                            // Handle nested Enum patterns: e.g.
                            // Err(TaskError.NotFound(id)) — the field
                            // pattern itself is an Enum whose fields need
                            // to be bound.
                            if let HirPattern::Enum {
                                type_def: inner_type_def,
                                variant_idx: inner_variant_idx,
                                fields: inner_fields,
                                ..
                            } = field_pat
                            {
                                // Extract the outer field (the inner enum
                                // value) from the payload.
                                let inner_enum_ty =
                                    variant_field_types.get(idx).cloned().unwrap_or(Ty::Int);
                                let inner_enum_local = self.new_temp(inner_enum_ty.clone());
                                self.emit(MirInst::GetField {
                                    dest: inner_enum_local,
                                    base: payload_ptr,
                                    field_index: idx,
                                });

                                if !inner_fields.is_empty() {
                                    let inner_variant_field_types = self
                                        .lookup_variant_field_types(
                                            *inner_type_def,
                                            *inner_variant_idx,
                                        );

                                    // Get the inner payload pointer.
                                    let inner_payload = self.new_temp(inner_enum_ty.clone());
                                    self.emit(MirInst::GetPayload {
                                        dest: inner_payload,
                                        src: inner_enum_local,
                                        ty: inner_enum_ty,
                                    });

                                    for (inner_idx, inner_field_pat) in
                                        inner_fields.iter().enumerate()
                                    {
                                        let inner_binding = match inner_field_pat {
                                            HirPattern::Binding {
                                                def_id,
                                                name,
                                                mutable,
                                                ..
                                            } => Some((*def_id, name.as_str(), *mutable)),
                                            HirPattern::Ref {
                                                def_id,
                                                name,
                                                mutable,
                                                ..
                                            } => Some((*def_id, name.as_str(), *mutable)),
                                            _ => None,
                                        };
                                        if let Some((inner_def_id, inner_name, inner_mutable)) =
                                            inner_binding
                                        {
                                            let inner_field_ty = inner_variant_field_types
                                                .get(inner_idx)
                                                .cloned()
                                                .unwrap_or(Ty::Int);
                                            let inner_local = self.new_local_named(
                                                inner_name,
                                                inner_field_ty,
                                                inner_mutable,
                                            );
                                            self.def_to_local.insert(inner_def_id, inner_local);
                                            self.emit(MirInst::GetField {
                                                dest: inner_local,
                                                base: inner_payload,
                                                field_index: inner_idx,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else if let HirPattern::Binding {
                    def_id,
                    name,
                    mutable,
                    ..
                } = &arm.pattern
                {
                    // Bind the scrutinee value to the variable — use the scrutinee's type.
                    let binding_ty = scrutinee.ty.clone();
                    let local = self.new_local_named(name, binding_ty, *mutable);
                    self.def_to_local.insert(*def_id, local);
                    self.emit(MirInst::Assign {
                        dest: local,
                        value: MirValue::Use(scrut),
                    });
                }

                // Evaluate the guard (if any) after pattern bindings.
                // Pattern bindings are already registered in
                // `def_to_local`, so the guard expression can reference
                // them. On guard failure, control falls through to the
                // next arm that could match (same variant or wildcard).
                if let Some(guard_expr) = &arm.guard {
                    let guard_local = self.lower_expr(guard_expr)?;
                    let guard_val = local_to_value(guard_local);
                    let body_block = self.new_block();
                    self.set_terminator(Terminator::Branch {
                        cond: guard_val,
                        then_block: body_block,
                        else_block: arm_fallthroughs[arm_idx],
                    });
                    self.current_block = body_block;
                }

                let body_result = self.lower_expr(&arm.body)?;
                if matches!(self.get_terminator(), Terminator::Unreachable) {
                    if let Some(dest) = result_local {
                        let val = local_to_value(body_result);
                        self.emit(MirInst::Assign { dest, value: val });
                    }
                    self.set_terminator(Terminator::Goto(merge_block));
                }
            }

            // If no wildcard arm was found, the otherwise block is unreachable.
            if wildcard_arm.is_none() {
                self.current_block = otherwise;
                self.set_terminator(Terminator::Unreachable);
            }
        } else {
            // Non-enum match: cascading branches (if/else chain).
            self.lower_match_cascading(
                scrut_local,
                &scrutinee.ty,
                arms,
                result_local,
                merge_block,
            )?;
        }

        self.current_block = merge_block;
        Ok(result_local)
    }

    fn lower_match_cascading(
        &mut self,
        scrut_local: Option<LocalId>,
        scrut_ty: &Ty,
        arms: &[HirMatchArm],
        result_local: Option<LocalId>,
        merge_block: BlockId,
    ) -> Result<(), String> {
        if arms.is_empty() {
            self.set_terminator(Terminator::Goto(merge_block));
            return Ok(());
        }

        for (i, arm) in arms.iter().enumerate() {
            let is_last = i == arms.len() - 1;
            let arm_body_block = self.new_block();
            let next_block = if is_last {
                merge_block
            } else {
                self.new_block()
            };

            // When a guard is present, the pattern-match target is an
            // intermediate block that evaluates the guard before
            // dispatching to the body or falling through to `next_block`.
            let has_guard = arm.guard.is_some();
            let match_target = if has_guard {
                self.new_block()
            } else {
                arm_body_block
            };

            match &arm.pattern {
                HirPattern::Wildcard { .. }
                | HirPattern::Binding { .. }
                | HirPattern::Ref { .. } => {
                    // Wildcard / binding / ref always matches.
                    let binding_info = match &arm.pattern {
                        HirPattern::Binding {
                            def_id,
                            name,
                            mutable,
                            ..
                        }
                        | HirPattern::Ref {
                            def_id,
                            name,
                            mutable,
                            ..
                        } => Some((*def_id, name.clone(), *mutable)),
                        _ => None,
                    };
                    if let Some((def_id, name, mutable)) = binding_info {
                        if let Some(scrut) = scrut_local {
                            let local = self.new_local_named(&name, scrut_ty.clone(), mutable);
                            self.def_to_local.insert(def_id, local);
                            self.emit(MirInst::Assign {
                                dest: local,
                                value: MirValue::Use(scrut),
                            });
                        }
                    }
                    self.set_terminator(Terminator::Goto(match_target));
                }
                HirPattern::Or { patterns, .. } => {
                    // Or-pattern: matches if any sub-pattern matches. For
                    // v0.1 we restrict or-patterns to literal / wildcard
                    // alternatives (no binding alternatives) — the parser
                    // accepts more, but we guard here.
                    let mut all_literal_or_wild = true;
                    for p in patterns {
                        match p {
                            HirPattern::Literal { .. } | HirPattern::Wildcard { .. } => {}
                            _ => all_literal_or_wild = false,
                        }
                    }
                    if !all_literal_or_wild {
                        // Fall through to the arm body (best-effort) so
                        // typeck/resolve don't crash; emit a diagnostic-
                        // worthy no-op. A future pass can add uniform-
                        // binding validation.
                        self.set_terminator(Terminator::Goto(match_target));
                    } else {
                        // Build a chain of tests across alternatives.
                        self.lower_or_pattern(
                            scrut_local,
                            scrut_ty,
                            patterns,
                            match_target,
                            next_block,
                        )?;
                    }
                }
                HirPattern::Tuple { elements, .. } => {
                    // Tuple pattern: compare each element against the
                    // scrutinee's corresponding field. Literals gate the
                    // match; bindings always accept and introduce a local.
                    if let Some(scrut) = scrut_local {
                        self.lower_tuple_pattern(
                            scrut,
                            scrut_ty,
                            elements,
                            match_target,
                            next_block,
                        )?;
                    } else {
                        self.set_terminator(Terminator::Goto(match_target));
                    }
                }
                HirPattern::Literal { expr: pat_expr, .. } => {
                    // Compare scrutinee to literal.
                    if let Some(scrut) = scrut_local {
                        let lit_local = self.lower_expr(pat_expr)?;
                        let cmp_dest = self.new_temp(Ty::Bool);
                        self.emit(MirInst::Compare {
                            dest: cmp_dest,
                            op: CmpOp::Eq,
                            lhs: MirValue::Use(scrut),
                            rhs: local_to_value(lit_local),
                        });
                        self.set_terminator(Terminator::Branch {
                            cond: MirValue::Use(cmp_dest),
                            then_block: match_target,
                            else_block: next_block,
                        });
                    } else {
                        self.set_terminator(Terminator::Goto(match_target));
                    }
                }
                _ => {
                    // Other patterns — fallthrough to body for now.
                    self.set_terminator(Terminator::Goto(match_target));
                }
            }

            // Evaluate the guard, if any, in the intermediate block.
            // Pattern bindings introduced above are already registered
            // in `def_to_local`, so the guard can reference them.
            if let Some(guard_expr) = &arm.guard {
                self.current_block = match_target;
                let guard_local = self.lower_expr(guard_expr)?;
                let guard_val = local_to_value(guard_local);
                self.set_terminator(Terminator::Branch {
                    cond: guard_val,
                    then_block: arm_body_block,
                    else_block: next_block,
                });
            }

            // Lower arm body.
            self.current_block = arm_body_block;
            let body_result = self.lower_expr(&arm.body)?;
            if matches!(self.get_terminator(), Terminator::Unreachable) {
                if let Some(dest) = result_local {
                    let val = local_to_value(body_result);
                    self.emit(MirInst::Assign { dest, value: val });
                }
                self.set_terminator(Terminator::Goto(merge_block));
            }

            if !is_last {
                self.current_block = next_block;
            }
        }
        Ok(())
    }

    // ── String interpolation lowering ───────────────────────────────────

    /// Phase 2 #06.D2.S3: emit the canonical `Display::fmt` dispatch
    /// sequence at the current interpolation site:
    ///
    /// ```text
    /// fmt = Formatter_new()                      // or _new_with_spec(...)
    /// {T}_fmt(src, fmt)
    /// buf = Formatter_buffer(fmt)
    /// ```
    ///
    /// Returns the `String` local holding the formatted buffer. The
    /// `Formatter_buffer` runtime impl transfers ownership of the buffer
    /// to the returned String and frees the `Formatter` struct in the
    /// same call, so callers do not (and must not) emit an explicit
    /// `Formatter_free`. `callee_name` is the fully-resolved MIR
    /// function name to dispatch into (e.g. `"Int_fmt"`, `"Money_fmt"`).
    ///
    /// Phase 2 #06.D4: when `spec` is non-default, the formatter is
    /// constructed via `Formatter_new_with_spec(width, precision, align,
    /// fill)` so the runtime can apply width / align / fill at finalize
    /// and the synth `_fmt` body can read precision via
    /// `Formatter_precision`.
    fn emit_display_dispatch(
        &mut self,
        src: LocalId,
        callee_name: &str,
        spec: Option<&crate::lexer::token::FormatSpec>,
    ) -> LocalId {
        let fmt_local = self.new_temp(Ty::Class {
            name: "Formatter".to_string(),
            generic_args: vec![],
        });
        let use_spec = spec.map(|s| !s.is_default()).unwrap_or(false);
        if use_spec {
            let spec = spec.unwrap();
            let (width, precision, align, fill) = encode_format_spec(spec);
            self.emit(MirInst::Call {
                dest: Some(fmt_local),
                callee: "Formatter_new_with_spec".to_string(),
                args: vec![
                    MirValue::Literal(Literal::Int(width)),
                    MirValue::Literal(Literal::Int(precision)),
                    MirValue::Literal(Literal::Int(align)),
                    MirValue::Literal(Literal::Int(fill)),
                ],
            });
        } else {
            self.emit(MirInst::Call {
                dest: Some(fmt_local),
                callee: "Formatter_new".to_string(),
                args: vec![],
            });
        }
        self.emit(MirInst::Call {
            dest: None,
            callee: callee_name.to_string(),
            args: vec![MirValue::Use(src), MirValue::Use(fmt_local)],
        });
        let dest = self.new_temp(Ty::String);
        self.emit(MirInst::Call {
            dest: Some(dest),
            callee: "Formatter_buffer".to_string(),
            args: vec![MirValue::Use(fmt_local)],
        });
        dest
    }

    fn lower_interpolation(
        &mut self,
        parts: &[HirInterpolationPart],
        _result_ty: &Ty,
    ) -> Result<Option<LocalId>, String> {
        if parts.is_empty() {
            let dest = self.new_temp(Ty::String);
            self.emit(MirInst::StringLiteral {
                dest,
                value: String::new(),
            });
            return Ok(Some(dest));
        }

        let mut accumulated: Option<LocalId> = None;

        for part in parts {
            let part_local = match part {
                HirInterpolationPart::Literal(s) => {
                    let dest = self.new_temp(Ty::String);
                    self.emit(MirInst::StringLiteral {
                        dest,
                        value: s.clone(),
                    });
                    dest
                }
                HirInterpolationPart::Expr { expr, spec } => {
                    // Phase 2 #06.B: `spec` is captured at lex time
                    // and consumed here. Phase C uses `spec.debug`
                    // to force Debug formatting; Phase D will route
                    // through `Display::fmt` and consume width/
                    // precision/align/fill via `Formatter`.
                    //
                    // Phase C semantics: `"#{x:?}"` always uses the
                    // `{Type}_to_debug` path when the type derives
                    // Debug. Bare `"#{x}"` keeps the legacy behaviour
                    // (struct-with-derive-Debug also lowers to
                    // `_to_debug` — Phase D will switch this to
                    // `Display::fmt` once the canonical interp path
                    // is migrated).
                    let _spec_debug = spec.debug;
                    let val_local = self.lower_expr(expr)?;

                    // Determine the effective type for the interpolation.
                    // Prefer the MIR local's type (which may have been
                    // corrected by enum variant field type lookup) over
                    // the HIR expression type (which may have stale or
                    // unresolved types from type inference).
                    let effective_ty = val_local
                        .and_then(|lid| {
                            self.fn_mut().locals.get(lid as usize).map(|l| l.ty.clone())
                        })
                        .unwrap_or_else(|| expr.ty.clone());

                    // Phase 2 #06.D2.S3 dispatch priority (top-down):
                    //   1. string-like / inferred-string  → pass-through
                    //   2. user `impl Display for T`      → `{T}_fmt` via Formatter
                    //   3. struct with `derive Debug`     → `{Name}_to_debug` (legacy)
                    //   4. enum with `derive Debug`       → `{Name}_to_debug` (legacy)
                    //   5. primitive Char/Int/Float/Bool  → synth `{Prim}_fmt` via Formatter
                    //   6. anything else                  → synth `Int_fmt` (pointer-as-int fallback)
                    //
                    // Priorities 2/5/6 emit the canonical Display dispatch:
                    //     fmt = Formatter_new()
                    //     {T}_fmt(value, fmt)
                    //     buf = Formatter_buffer(fmt)
                    // The synth `{Prim}_fmt` fns (Stage 1) wrap the same
                    // `riven_<prim>_to_string` helpers the legacy direct path
                    // used, so output is byte-identical for all fixtures.
                    // `user_has_impl_display` is checked BEFORE the derive-Debug
                    // arms so a user-supplied `impl Display for T` wins over
                    // an auto-derived `Debug` formatter.
                    // Phase 2 #06.D4: when the spec is non-default we
                    // must route strings through `String_fmt` so the
                    // Formatter can apply width / precision / align /
                    // fill — the legacy pass-through skips the formatter
                    // entirely and would silently drop the spec.
                    if (is_string_like(&effective_ty) || is_inferred_string_expr(expr))
                        && spec.is_default()
                    {
                        val_local.unwrap_or_else(|| {
                            let d = self.new_temp(Ty::String);
                            self.emit(MirInst::StringLiteral {
                                dest: d,
                                value: String::new(),
                            });
                            d
                        })
                    } else if let Some(user_t) = self.user_has_impl_display(&effective_ty) {
                        // Priority #2: user `impl Display for T`.
                        let src = val_local.unwrap_or_else(|| {
                            let d = self.new_temp(Ty::String);
                            self.emit(MirInst::StringLiteral {
                                dest: d,
                                value: String::new(),
                            });
                            d
                        });
                        self.emit_display_dispatch(src, &format!("{}_fmt", user_t), Some(spec))
                    } else if let Some(struct_name) = self.struct_with_derive_debug(&effective_ty) {
                        // Priority #3: struct with `derive Debug` (and no user
                        // `impl Display`) — keep the legacy `{Name}_to_debug`
                        // path so bare `"#{x}"` still prints the formatted
                        // struct rather than a raw pointer address.
                        let src = val_local.unwrap_or_else(|| {
                            let d = self.new_temp(Ty::String);
                            self.emit(MirInst::StringLiteral {
                                dest: d,
                                value: String::new(),
                            });
                            d
                        });
                        let dest = self.new_temp(Ty::String);
                        self.emit(MirInst::Call {
                            dest: Some(dest),
                            callee: format!("{}_to_debug", struct_name),
                            args: vec![MirValue::Use(src)],
                        });
                        dest
                    } else if let Some(enum_name) = self.enum_with_derive_debug(&effective_ty) {
                        // Priority #4: enum with `derive Debug` (Phase 2 #06.C2).
                        let src = val_local.unwrap_or_else(|| {
                            let d = self.new_temp(Ty::String);
                            self.emit(MirInst::StringLiteral {
                                dest: d,
                                value: String::new(),
                            });
                            d
                        });
                        let dest = self.new_temp(Ty::String);
                        self.emit(MirInst::Call {
                            dest: Some(dest),
                            callee: format!("{}_to_debug", enum_name),
                            args: vec![MirValue::Use(src)],
                        });
                        dest
                    } else {
                        // Priorities #5 + #6: primitives + last-resort
                        // fallback.  Each dispatches through the canonical
                        // `Formatter_new` → `{Prim}_fmt(value, fmt)` →
                        // `Formatter_buffer(fmt)` sequence.  `Char` must be
                        // checked BEFORE `is_integer()` because `Char` is a
                        // 32-bit codepoint and currently also satisfies the
                        // integer predicate in some lowerings — without this
                        // priority a `Char` would render as a decimal number.
                        let src = val_local.unwrap_or_else(|| {
                            let d = self.new_temp(Ty::String);
                            self.emit(MirInst::StringLiteral {
                                dest: d,
                                value: String::new(),
                            });
                            d
                        });
                        let fmt_callee = if effective_ty == Ty::Char {
                            "Char_fmt"
                        } else if is_string_like(&effective_ty) {
                            // Phase 2 #06.D4: a String value with a
                            // non-default spec falls here (the spec-
                            // default pass-through above is skipped).
                            // Route through `String_fmt` so width /
                            // precision / align / fill all apply.
                            "String_fmt"
                        } else if effective_ty.is_integer() {
                            "Int_fmt"
                        } else if effective_ty.is_float() {
                            "Float_fmt"
                        } else if effective_ty == Ty::Bool {
                            "Bool_fmt"
                        } else {
                            // Unknown type — treat as integer (pointer
                            // value) as a fallback.  This handles USize,
                            // enum tags, and any not-yet-inferred type.
                            // Preserves the pre-Stage-3 default behaviour.
                            "Int_fmt"
                        };
                        self.emit_display_dispatch(src, fmt_callee, Some(spec))
                    }
                }
            };

            accumulated = Some(match accumulated {
                None => part_local,
                Some(prev) => {
                    let dest = self.new_temp(Ty::String);
                    self.emit(MirInst::Call {
                        dest: Some(dest),
                        callee: "riven_string_concat".to_string(),
                        args: vec![MirValue::Use(prev), MirValue::Use(part_local)],
                    });
                    dest
                }
            });
        }

        Ok(accumulated)
    }

    // ── Inline closure methods ────────────────────────────────────────

    /// Try to inline a closure-taking method call as an explicit loop.
    /// Returns `Ok(Some(Some(local)))` if inlined with a result,
    /// `Ok(Some(None))` if inlined with no result (Unit),
    /// `Ok(None)` if not handled (fall through to normal method call).
    fn try_inline_closure_method(
        &mut self,
        expr: &HirExpr,
        object: &HirExpr,
        method_name: &str,
        args: &[HirExpr],
        block_expr: &HirExpr,
    ) -> Result<Option<Option<LocalId>>, String> {
        // Extract closure params and body from the block expression.
        let (closure_params, closure_body) = match &block_expr.kind {
            HirExprKind::Closure { params, body, .. } => (params, body),
            _ => return Ok(None), // Not a closure — can't inline.
        };

        // Handle Option.map { |x| expr } inline: check tag, transform payload.
        if is_option_type(&object.ty) && method_name == "map" {
            return self.inline_option_map(expr, object, closure_params, closure_body);
        }

        // Result.map / Result.map_err — same shape: branch on tag,
        // run the closure on the matching arm's payload, repackage.
        if is_result_type(&object.ty) {
            match method_name {
                "map" => {
                    return self.inline_result_map(
                        expr,
                        object,
                        closure_params,
                        closure_body,
                        /*on_ok=*/ true,
                    );
                }
                "map_err" => {
                    return self.inline_result_map(
                        expr,
                        object,
                        closure_params,
                        closure_body,
                        /*on_ok=*/ false,
                    );
                }
                _ => {}
            }
        }

        // Result.unwrap_or_else { |e| ... } / Option.unwrap_or_else { |e| ... }
        // — branch on tag, return payload on the success arm, evaluate
        // closure with the error payload otherwise.
        if method_name == "unwrap_or_else" {
            if is_result_type(&object.ty) {
                return self.inline_unwrap_or_else(
                    expr,
                    object,
                    closure_params,
                    closure_body,
                    /*ok_tag=*/ 0,
                );
            }
            if is_option_type(&object.ty) {
                return self.inline_unwrap_or_else(
                    expr,
                    object,
                    closure_params,
                    closure_body,
                    /*ok_tag=*/ 1,
                );
            }
        }

        // Determine the Vec source. For Vec/iterator types, peel through
        // method call chains. For user-defined classes with known
        // collection-wrapping methods (where_matching, display_all,
        // into_filtered, each), access the class's first field (items Vec).
        let vec_id = if is_vec_or_iterator_type(&object.ty) {
            let vec_local = self.lower_vec_source(object)?;
            vec_local.unwrap_or_else(|| self.new_temp(Ty::Int))
        } else if is_collection_method(method_name) {
            // User-defined class: lower the object and access its first
            // field to get the underlying Vec.
            let obj_local = self.lower_expr(object)?;
            let obj_id = obj_local.unwrap_or_else(|| self.new_temp(Ty::Int));
            let items_local = self.new_temp(Ty::Int);
            self.emit(MirInst::GetField {
                dest: items_local,
                base: obj_id,
                field_index: 0,
            });
            items_local
        } else {
            return Ok(None);
        };

        match method_name {
            "each" => {
                // for i in 0..vec.len: item = vec[i]; <body>
                self.inline_each(vec_id, closure_params, closure_body)?;
                Ok(Some(None))
            }
            "filter" | "where_matching" => {
                // result = Vec.new(); for i in 0..vec.len: item = vec[i]; if <pred>: result.push(item)
                let result = self.inline_filter(expr, vec_id, closure_params, closure_body)?;
                Ok(Some(Some(result)))
            }
            "find" => {
                // for i in 0..vec.len: item = vec[i]; if <pred>: return Some(item); return None
                let result = self.inline_find(expr, vec_id, closure_params, closure_body)?;
                Ok(Some(Some(result)))
            }
            "position" => {
                // for i in 0..vec.len: item = vec[i]; if <pred>: return Some(i); return None
                let result = self.inline_position(expr, vec_id, closure_params, closure_body)?;
                Ok(Some(Some(result)))
            }
            "map" => {
                // result = Vec.new(); for i in 0..vec.len: item = vec[i]; result.push(<expr>)
                let result = self.inline_map(expr, vec_id, closure_params, closure_body)?;
                Ok(Some(Some(result)))
            }
            "partition" => {
                // true_vec = Vec.new(); false_vec = Vec.new(); for ...; return (true_vec, false_vec)
                let result = self.inline_partition(expr, vec_id, closure_params, closure_body)?;
                Ok(Some(Some(result)))
            }
            // Phase 2 stdlib batch 2 (#03): closure-takers reuse the
            // same per-element loop machinery as `each` / `filter`.
            //
            //  * `retain { |x| keep? }`    — in-place filter.
            //  * `sort_by { |a, b| ord }`  — comparator-driven insertion sort.
            "retain" => {
                self.inline_retain(vec_id, closure_params, closure_body)?;
                Ok(Some(None))
            }
            "sort_by" => {
                self.inline_sort_by(vec_id, closure_params, closure_body)?;
                Ok(Some(None))
            }
            // Phase 2 stdlib (#05 batch 2): closure-taking eager
            // terminators on `*Iter` receivers. These inline the same
            // `riven_vec_len` + `riven_vec_get` per-element loop as
            // `each` / `find`, but they accumulate (`fold`) or
            // short-circuit on a boolean predicate (`all` / `any`).
            "fold" => {
                let result = self.inline_fold(expr, vec_id, args, closure_params, closure_body)?;
                Ok(Some(Some(result)))
            }
            "all" => {
                let result = self.inline_all_any(
                    expr,
                    vec_id,
                    closure_params,
                    closure_body,
                    /*all=*/ true,
                )?;
                Ok(Some(Some(result)))
            }
            "any" => {
                let result = self.inline_all_any(
                    expr,
                    vec_id,
                    closure_params,
                    closure_body,
                    /*all=*/ false,
                )?;
                Ok(Some(Some(result)))
            }
            _ => Ok(None), // Not a recognized closure method.
        }
    }

    /// Emit an inlined `vec.retain { |item| pred }` — in-place filter.
    /// Read-write cursor walks the backing array; elements where the
    /// closure returns `true` are kept (compacted into the prefix);
    /// elements where it returns `false` are dropped (the slot at
    /// position `read` is overwritten by a future kept element). Final
    /// `len` becomes the count of survivors. The element backing
    /// (e.g. `Vec[String]` slot strings) is NOT freed by this lowering
    /// — v1 documents `retain` as a slot-level forget, the same
    /// contract as `clear` / `truncate` (#03 batch 1).
    fn inline_retain(
        &mut self,
        vec_id: LocalId,
        closure_params: &[HirClosureParam],
        closure_body: &HirExpr,
    ) -> Result<(), String> {
        // read = 0; write = 0
        let read = self.new_temp(Ty::Int);
        self.emit(MirInst::Assign {
            dest: read,
            value: MirValue::Literal(Literal::Int(0)),
        });
        let write = self.new_temp(Ty::Int);
        self.emit(MirInst::Assign {
            dest: write,
            value: MirValue::Literal(Literal::Int(0)),
        });

        let len = self.new_temp(Ty::Int);
        self.emit(MirInst::Call {
            dest: Some(len),
            callee: "riven_vec_len".to_string(),
            args: vec![MirValue::Use(vec_id)],
        });

        let header_block = self.new_block();
        let body_block = self.new_block();
        let keep_block = self.new_block();
        let inc_block = self.new_block();
        let exit_block = self.new_block();

        self.set_terminator(Terminator::Goto(header_block));
        self.current_block = header_block;

        let cond = self.new_temp(Ty::Bool);
        self.emit(MirInst::Compare {
            dest: cond,
            op: CmpOp::Lt,
            lhs: MirValue::Use(read),
            rhs: MirValue::Use(len),
        });
        self.set_terminator(Terminator::Branch {
            cond: MirValue::Use(cond),
            then_block: body_block,
            else_block: exit_block,
        });

        // Body: bind item, evaluate predicate.
        self.current_block = body_block;

        let item_local = if let Some(param) = closure_params.first() {
            let item = self.new_local_named(&param.name, param.ty.clone(), false);
            self.def_to_local.insert(param.def_id, item);
            self.emit(MirInst::Call {
                dest: Some(item),
                callee: "riven_vec_get".to_string(),
                args: vec![MirValue::Use(vec_id), MirValue::Use(read)],
            });
            item
        } else {
            self.new_temp(Ty::Int)
        };

        let pred_result = self.lower_expr(closure_body)?;
        let pred_val = local_to_value(pred_result);

        self.set_terminator(Terminator::Branch {
            cond: pred_val,
            then_block: keep_block,
            else_block: inc_block,
        });

        // Keep: write the slot at `write`, then write++.
        self.current_block = keep_block;
        self.emit(MirInst::Call {
            dest: None,
            callee: "riven_vec_set".to_string(),
            args: vec![
                MirValue::Use(vec_id),
                MirValue::Use(write),
                MirValue::Use(item_local),
            ],
        });
        let next_write = self.new_temp(Ty::Int);
        self.emit(MirInst::BinOp {
            dest: next_write,
            op: BinOp::Add,
            lhs: MirValue::Use(write),
            rhs: MirValue::Literal(Literal::Int(1)),
        });
        self.emit(MirInst::Assign {
            dest: write,
            value: MirValue::Use(next_write),
        });
        self.set_terminator(Terminator::Goto(inc_block));

        // Increment read.
        self.current_block = inc_block;
        let next_read = self.new_temp(Ty::Int);
        self.emit(MirInst::BinOp {
            dest: next_read,
            op: BinOp::Add,
            lhs: MirValue::Use(read),
            rhs: MirValue::Literal(Literal::Int(1)),
        });
        self.emit(MirInst::Assign {
            dest: read,
            value: MirValue::Use(next_read),
        });
        self.set_terminator(Terminator::Goto(header_block));

        // Exit: truncate to `write`.
        self.current_block = exit_block;
        self.emit(MirInst::Call {
            dest: None,
            callee: "riven_vec_truncate".to_string(),
            args: vec![MirValue::Use(vec_id), MirValue::Use(write)],
        });
        Ok(())
    }

    /// Emit an inlined `vec.sort_by { |a, b| order }` — selection sort
    /// on the backing slots driven by the user's comparator. The
    /// comparator returns a signed Int (negative=a-before-b,
    /// positive=b-before-a). For v1 we use selection sort O(n^2) which
    /// is the simplest stable shape; switching to a heapsort or a
    /// merge-sort lands alongside the wider trait-driven sort surface
    /// in #05. Sort is in-place — bitwise slot swap.
    fn inline_sort_by(
        &mut self,
        vec_id: LocalId,
        closure_params: &[HirClosureParam],
        closure_body: &HirExpr,
    ) -> Result<(), String> {
        // i = 0
        let i_idx = self.new_temp(Ty::Int);
        self.emit(MirInst::Assign {
            dest: i_idx,
            value: MirValue::Literal(Literal::Int(0)),
        });

        // len = riven_vec_len(vec)
        let len = self.new_temp(Ty::Int);
        self.emit(MirInst::Call {
            dest: Some(len),
            callee: "riven_vec_len".to_string(),
            args: vec![MirValue::Use(vec_id)],
        });

        let outer_header = self.new_block();
        let outer_body = self.new_block();
        let inner_header = self.new_block();
        let inner_body = self.new_block();
        let swap_block = self.new_block();
        let inner_inc = self.new_block();
        let outer_inc = self.new_block();
        let exit_block = self.new_block();

        self.set_terminator(Terminator::Goto(outer_header));
        self.current_block = outer_header;

        // outer cond: i < len
        let outer_cond = self.new_temp(Ty::Bool);
        self.emit(MirInst::Compare {
            dest: outer_cond,
            op: CmpOp::Lt,
            lhs: MirValue::Use(i_idx),
            rhs: MirValue::Use(len),
        });
        self.set_terminator(Terminator::Branch {
            cond: MirValue::Use(outer_cond),
            then_block: outer_body,
            else_block: exit_block,
        });

        // outer body: j = i + 1
        self.current_block = outer_body;
        let j_idx = self.new_temp(Ty::Int);
        let i_plus_1 = self.new_temp(Ty::Int);
        self.emit(MirInst::BinOp {
            dest: i_plus_1,
            op: BinOp::Add,
            lhs: MirValue::Use(i_idx),
            rhs: MirValue::Literal(Literal::Int(1)),
        });
        self.emit(MirInst::Assign {
            dest: j_idx,
            value: MirValue::Use(i_plus_1),
        });
        self.set_terminator(Terminator::Goto(inner_header));

        // inner cond: j < len
        self.current_block = inner_header;
        let inner_cond = self.new_temp(Ty::Bool);
        self.emit(MirInst::Compare {
            dest: inner_cond,
            op: CmpOp::Lt,
            lhs: MirValue::Use(j_idx),
            rhs: MirValue::Use(len),
        });
        self.set_terminator(Terminator::Branch {
            cond: MirValue::Use(inner_cond),
            then_block: inner_body,
            else_block: outer_inc,
        });

        // inner body: bind closure params a = vec[i], b = vec[j];
        // result = closure(a, b); if result > 0: swap(i, j).
        self.current_block = inner_body;
        let elem_ty = element_type_of(&self.fn_local_ty(vec_id));
        let a_local = if let Some(param) = closure_params.first() {
            let l = self.new_local_named(&param.name, param.ty.clone(), false);
            self.def_to_local.insert(param.def_id, l);
            self.emit(MirInst::Call {
                dest: Some(l),
                callee: "riven_vec_get".to_string(),
                args: vec![MirValue::Use(vec_id), MirValue::Use(i_idx)],
            });
            l
        } else {
            let l = self.new_temp(elem_ty.clone());
            self.emit(MirInst::Call {
                dest: Some(l),
                callee: "riven_vec_get".to_string(),
                args: vec![MirValue::Use(vec_id), MirValue::Use(i_idx)],
            });
            l
        };
        let _b_local = if let Some(param) = closure_params.get(1) {
            let l = self.new_local_named(&param.name, param.ty.clone(), false);
            self.def_to_local.insert(param.def_id, l);
            self.emit(MirInst::Call {
                dest: Some(l),
                callee: "riven_vec_get".to_string(),
                args: vec![MirValue::Use(vec_id), MirValue::Use(j_idx)],
            });
            l
        } else {
            let l = self.new_temp(elem_ty.clone());
            self.emit(MirInst::Call {
                dest: Some(l),
                callee: "riven_vec_get".to_string(),
                args: vec![MirValue::Use(vec_id), MirValue::Use(j_idx)],
            });
            l
        };
        let _ = a_local;

        let cmp_result = self.lower_expr(closure_body)?;
        let cmp_val = local_to_value(cmp_result);
        let zero = MirValue::Literal(Literal::Int(0));
        let need_swap = self.new_temp(Ty::Bool);
        self.emit(MirInst::Compare {
            dest: need_swap,
            op: CmpOp::Gt,
            lhs: cmp_val,
            rhs: zero,
        });
        self.set_terminator(Terminator::Branch {
            cond: MirValue::Use(need_swap),
            then_block: swap_block,
            else_block: inner_inc,
        });

        // swap_block: riven_vec_swap(vec, i, j)
        self.current_block = swap_block;
        self.emit(MirInst::Call {
            dest: None,
            callee: "riven_vec_swap".to_string(),
            args: vec![
                MirValue::Use(vec_id),
                MirValue::Use(i_idx),
                MirValue::Use(j_idx),
            ],
        });
        self.set_terminator(Terminator::Goto(inner_inc));

        // inner_inc: j += 1
        self.current_block = inner_inc;
        let next_j = self.new_temp(Ty::Int);
        self.emit(MirInst::BinOp {
            dest: next_j,
            op: BinOp::Add,
            lhs: MirValue::Use(j_idx),
            rhs: MirValue::Literal(Literal::Int(1)),
        });
        self.emit(MirInst::Assign {
            dest: j_idx,
            value: MirValue::Use(next_j),
        });
        self.set_terminator(Terminator::Goto(inner_header));

        // outer_inc: i += 1
        self.current_block = outer_inc;
        let next_i = self.new_temp(Ty::Int);
        self.emit(MirInst::BinOp {
            dest: next_i,
            op: BinOp::Add,
            lhs: MirValue::Use(i_idx),
            rhs: MirValue::Literal(Literal::Int(1)),
        });
        self.emit(MirInst::Assign {
            dest: i_idx,
            value: MirValue::Use(next_i),
        });
        self.set_terminator(Terminator::Goto(outer_header));

        self.current_block = exit_block;
        Ok(())
    }

    /// Look up the `Ty` of a local in the function being lowered.
    /// Falls back to `Ty::Int` if the local isn't found (defensive — the
    /// element-type extraction in `inline_sort_by` is the only caller
    /// and operates on locals it just allocated).
    fn fn_local_ty(&self, local_id: LocalId) -> Ty {
        self.current_fn
            .as_ref()
            .and_then(|f| f.locals.iter().find(|l| l.id == local_id))
            .map(|l| l.ty.clone())
            .unwrap_or(Ty::Int)
    }

    /// Lower the "vec source" from a method call chain, peeling through
    /// iterator adaptors and passthrough method calls to find the underlying
    /// Vec local. E.g., `self.items.iter.filter { ... }` -> the local for
    /// `self.items`.
    fn lower_vec_source(&mut self, expr: &HirExpr) -> Result<Option<LocalId>, String> {
        match &expr.kind {
            HirExprKind::MethodCall {
                object,
                method_name,
                block,
                ..
            } => {
                match method_name.as_str() {
                    "iter" | "into_iter" | "to_vec" | "enumerate" => {
                        // These are passthrough — recurse into the object.
                        self.lower_vec_source(object)
                    }
                    "filter" | "where_matching" if block.is_some() => {
                        // A filter in the chain: inline it and return the
                        // filtered vec as the source. This handles chained
                        // `.filter { ... }.to_vec`.
                        // For now, just peel through to the base object.
                        self.lower_vec_source(object)
                    }
                    _ => {
                        // Some other method — lower it normally.
                        self.lower_expr(expr)
                    }
                }
            }
            HirExprKind::FieldAccess {
                object: inner_obj,
                field_name,
                ..
            } => {
                // .iter, .into_iter etc. may be parsed as FieldAccess (no parens)
                match field_name.as_str() {
                    "iter" | "into_iter" | "to_vec" | "enumerate" => {
                        self.lower_vec_source(inner_obj)
                    }
                    _ => self.lower_expr(expr),
                }
            }
            _ => self.lower_expr(expr),
        }
    }

    /// Emit the inlined contains_key+insert pattern that backs
    /// `m.entry(K).or_insert(V)` and `m.entry(K).or_insert_with { || V }`.
    ///
    /// `entry_expr` is the receiver of the outer call — it is the inner
    /// `.entry(K)` HIR node, whose `object` is the original HashMap and
    /// whose `args[0]` is the key K. `outer_args` is the (possibly empty)
    /// arg list of the outer `or_insert*` call; for `or_insert(V)` it
    /// holds `[V]`, for `or_insert_with` it is empty and the closure
    /// body is in `outer_block`.
    ///
    /// Emits:
    ///
    /// ```text
    ///   k_local       = lower(K)
    ///   has           = riven_hash_contains_key(map, k_local)
    ///   if has goto MERGE else goto INSERT
    /// INSERT:
    ///   v_local       = lower(V)            // or closure body
    ///   _             = riven_hash_insert(map, k_local, v_local)
    ///   goto MERGE
    /// MERGE:
    /// ```
    ///
    /// The chain returns Unit (no `&mut V` like Rust — see prompt 04
    /// deferred-Entry note: that requires pointer-returning method
    /// dispatch we have not built).
    fn inline_entry_or_insert(
        &mut self,
        entry_expr: &HirExpr,
        method_name: &str,
        outer_args: &[HirExpr],
        outer_block: &Option<Box<HirExpr>>,
    ) -> Result<Option<LocalId>, String> {
        let (map_expr, k_expr) = match &entry_expr.kind {
            HirExprKind::MethodCall {
                object,
                args: entry_args,
                method_name: m,
                ..
            } if m == "entry" => {
                let k = entry_args
                    .first()
                    .ok_or_else(|| "HashMap.entry expects exactly one key argument".to_string())?;
                (object.as_ref(), k)
            }
            _ => unreachable!("inline_entry_or_insert called without entry chain"),
        };

        let map_local_opt = self.lower_expr(map_expr)?;
        let map_local =
            map_local_opt.ok_or_else(|| "HashMap receiver lowered to no value".to_string())?;

        let k_local_opt = self.lower_expr(k_expr)?;
        let k_local =
            k_local_opt.ok_or_else(|| "HashMap.entry key arg lowered to no value".to_string())?;

        // contains_key check.
        let has = self.new_temp(Ty::Bool);
        self.emit(MirInst::Call {
            dest: Some(has),
            callee: "riven_hash_contains_key".to_string(),
            args: vec![MirValue::Use(map_local), MirValue::Use(k_local)],
        });

        let insert_block = self.new_block();
        let merge_block = self.new_block();
        self.set_terminator(Terminator::Branch {
            cond: MirValue::Use(has),
            then_block: merge_block,
            else_block: insert_block,
        });

        // INSERT block: lower V (or closure body), then call insert.
        self.current_block = insert_block;
        let v_local_opt = match method_name {
            "or_insert" => {
                let v_expr = outer_args
                    .first()
                    .ok_or_else(|| "or_insert expects exactly one value argument".to_string())?;
                self.lower_expr(v_expr)?
            }
            "or_insert_with" => {
                let block_expr = outer_block
                    .as_deref()
                    .ok_or_else(|| "or_insert_with expects a closure block".to_string())?;
                let body = match &block_expr.kind {
                    HirExprKind::Closure { body, .. } => body,
                    _ => {
                        return Err("or_insert_with expects a closure block as its body".to_string())
                    }
                };
                self.lower_expr(body)?
            }
            _ => unreachable!(
                "inline_entry_or_insert called for unknown method `{}`",
                method_name
            ),
        };
        let v_local =
            v_local_opt.ok_or_else(|| format!("`{}` value lowered to no value", method_name))?;

        // Discard the Option[V] return — we don't expose the displaced
        // value because typeck pinned this chain's type to Unit.
        self.emit(MirInst::Call {
            dest: None,
            callee: "riven_hash_insert".to_string(),
            args: vec![
                MirValue::Use(map_local),
                MirValue::Use(k_local),
                MirValue::Use(v_local),
            ],
        });
        self.set_terminator(Terminator::Goto(merge_block));

        // Merge: chain's type is Unit, so no result local.
        self.current_block = merge_block;
        Ok(None)
    }

    /// Emit an inlined `.each { |item| body }` loop.
    fn inline_each(
        &mut self,
        vec_id: LocalId,
        closure_params: &[HirClosureParam],
        closure_body: &HirExpr,
    ) -> Result<(), String> {
        // idx = 0
        let idx = self.new_temp(Ty::Int);
        self.emit(MirInst::Assign {
            dest: idx,
            value: MirValue::Literal(Literal::Int(0)),
        });

        // len = riven_vec_len(vec)
        let len = self.new_temp(Ty::Int);
        self.emit(MirInst::Call {
            dest: Some(len),
            callee: "riven_vec_len".to_string(),
            args: vec![MirValue::Use(vec_id)],
        });

        let header_block = self.new_block();
        let body_block = self.new_block();
        let exit_block = self.new_block();

        self.set_terminator(Terminator::Goto(header_block));
        self.current_block = header_block;

        // cond = idx < len
        let cond = self.new_temp(Ty::Bool);
        self.emit(MirInst::Compare {
            dest: cond,
            op: CmpOp::Lt,
            lhs: MirValue::Use(idx),
            rhs: MirValue::Use(len),
        });
        self.set_terminator(Terminator::Branch {
            cond: MirValue::Use(cond),
            then_block: body_block,
            else_block: exit_block,
        });

        // Body
        self.current_block = body_block;

        // Bind the closure parameter: item = vec_get(vec, idx)
        if let Some(param) = closure_params.first() {
            let item_local = self.new_local_named(&param.name, param.ty.clone(), false);
            self.def_to_local.insert(param.def_id, item_local);
            self.emit(MirInst::Call {
                dest: Some(item_local),
                callee: "riven_vec_get".to_string(),
                args: vec![MirValue::Use(vec_id), MirValue::Use(idx)],
            });
        }

        // Lower the closure body
        let _ = self.lower_expr(closure_body)?;

        // idx = idx + 1
        let next_idx = self.new_temp(Ty::Int);
        self.emit(MirInst::BinOp {
            dest: next_idx,
            op: BinOp::Add,
            lhs: MirValue::Use(idx),
            rhs: MirValue::Literal(Literal::Int(1)),
        });
        self.emit(MirInst::Assign {
            dest: idx,
            value: MirValue::Use(next_idx),
        });

        if matches!(self.get_terminator(), Terminator::Unreachable) {
            self.set_terminator(Terminator::Goto(header_block));
        }

        self.current_block = exit_block;
        Ok(())
    }

    /// Emit an inlined `.filter { |item| pred }` loop.
    fn inline_filter(
        &mut self,
        expr: &HirExpr,
        vec_id: LocalId,
        closure_params: &[HirClosureParam],
        closure_body: &HirExpr,
    ) -> Result<LocalId, String> {
        // result = riven_vec_new()
        let result = self.new_temp(expr.ty.clone());
        self.emit(MirInst::Call {
            dest: Some(result),
            callee: "riven_vec_new".to_string(),
            args: vec![],
        });

        // idx = 0
        let idx = self.new_temp(Ty::Int);
        self.emit(MirInst::Assign {
            dest: idx,
            value: MirValue::Literal(Literal::Int(0)),
        });

        // len = riven_vec_len(vec)
        let len = self.new_temp(Ty::Int);
        self.emit(MirInst::Call {
            dest: Some(len),
            callee: "riven_vec_len".to_string(),
            args: vec![MirValue::Use(vec_id)],
        });

        let header_block = self.new_block();
        let body_block = self.new_block();
        let push_block = self.new_block();
        let inc_block = self.new_block();
        let exit_block = self.new_block();

        self.set_terminator(Terminator::Goto(header_block));
        self.current_block = header_block;

        let cond = self.new_temp(Ty::Bool);
        self.emit(MirInst::Compare {
            dest: cond,
            op: CmpOp::Lt,
            lhs: MirValue::Use(idx),
            rhs: MirValue::Use(len),
        });
        self.set_terminator(Terminator::Branch {
            cond: MirValue::Use(cond),
            then_block: body_block,
            else_block: exit_block,
        });

        // Body: bind item, evaluate predicate
        self.current_block = body_block;

        let item_local = if let Some(param) = closure_params.first() {
            let item = self.new_local_named(&param.name, param.ty.clone(), false);
            self.def_to_local.insert(param.def_id, item);
            self.emit(MirInst::Call {
                dest: Some(item),
                callee: "riven_vec_get".to_string(),
                args: vec![MirValue::Use(vec_id), MirValue::Use(idx)],
            });
            item
        } else {
            self.new_temp(Ty::Int)
        };

        // Evaluate predicate
        let pred_result = self.lower_expr(closure_body)?;
        let pred_val = local_to_value(pred_result);

        self.set_terminator(Terminator::Branch {
            cond: pred_val,
            then_block: push_block,
            else_block: inc_block,
        });

        // Push block: result.push(item)
        self.current_block = push_block;
        self.emit(MirInst::Call {
            dest: None,
            callee: "riven_vec_push".to_string(),
            args: vec![MirValue::Use(result), MirValue::Use(item_local)],
        });
        self.set_terminator(Terminator::Goto(inc_block));

        // Increment
        self.current_block = inc_block;
        let next_idx = self.new_temp(Ty::Int);
        self.emit(MirInst::BinOp {
            dest: next_idx,
            op: BinOp::Add,
            lhs: MirValue::Use(idx),
            rhs: MirValue::Literal(Literal::Int(1)),
        });
        self.emit(MirInst::Assign {
            dest: idx,
            value: MirValue::Use(next_idx),
        });
        self.set_terminator(Terminator::Goto(header_block));

        self.current_block = exit_block;
        Ok(result)
    }

    /// Emit an inlined `.find { |item| pred }` loop.
    fn inline_find(
        &mut self,
        expr: &HirExpr,
        vec_id: LocalId,
        closure_params: &[HirClosureParam],
        closure_body: &HirExpr,
    ) -> Result<LocalId, String> {
        // Allocate result as Option (tagged union: 16 bytes)
        // tag=0 -> None, tag=1 -> Some(payload)
        let result = self.new_temp(expr.ty.clone());
        self.emit(MirInst::Alloc {
            dest: result,
            ty: expr.ty.clone(),
            size: 16,
        });
        // Initialize to None (tag=0)
        self.emit(MirInst::SetTag {
            dest: result,
            tag: 0,
        });

        let idx = self.new_temp(Ty::Int);
        self.emit(MirInst::Assign {
            dest: idx,
            value: MirValue::Literal(Literal::Int(0)),
        });

        let len = self.new_temp(Ty::Int);
        self.emit(MirInst::Call {
            dest: Some(len),
            callee: "riven_vec_len".to_string(),
            args: vec![MirValue::Use(vec_id)],
        });

        let header_block = self.new_block();
        let body_block = self.new_block();
        let found_block = self.new_block();
        let inc_block = self.new_block();
        let exit_block = self.new_block();

        self.set_terminator(Terminator::Goto(header_block));
        self.current_block = header_block;

        let cond = self.new_temp(Ty::Bool);
        self.emit(MirInst::Compare {
            dest: cond,
            op: CmpOp::Lt,
            lhs: MirValue::Use(idx),
            rhs: MirValue::Use(len),
        });
        self.set_terminator(Terminator::Branch {
            cond: MirValue::Use(cond),
            then_block: body_block,
            else_block: exit_block,
        });

        // Body
        self.current_block = body_block;

        let item_local = if let Some(param) = closure_params.first() {
            let item = self.new_local_named(&param.name, param.ty.clone(), false);
            self.def_to_local.insert(param.def_id, item);
            self.emit(MirInst::Call {
                dest: Some(item),
                callee: "riven_vec_get".to_string(),
                args: vec![MirValue::Use(vec_id), MirValue::Use(idx)],
            });
            item
        } else {
            self.new_temp(Ty::Int)
        };

        let pred_result = self.lower_expr(closure_body)?;
        let pred_val = local_to_value(pred_result);

        self.set_terminator(Terminator::Branch {
            cond: pred_val,
            then_block: found_block,
            else_block: inc_block,
        });

        // Found: set result to Some(item)
        self.current_block = found_block;
        self.emit(MirInst::SetTag {
            dest: result,
            tag: 1,
        });
        // Store item as payload (offset 8 from base)
        self.emit(MirInst::SetField {
            base: result,
            field_index: 1,
            value: MirValue::Use(item_local),
        });
        self.set_terminator(Terminator::Goto(exit_block));

        // Increment
        self.current_block = inc_block;
        let next_idx = self.new_temp(Ty::Int);
        self.emit(MirInst::BinOp {
            dest: next_idx,
            op: BinOp::Add,
            lhs: MirValue::Use(idx),
            rhs: MirValue::Literal(Literal::Int(1)),
        });
        self.emit(MirInst::Assign {
            dest: idx,
            value: MirValue::Use(next_idx),
        });
        self.set_terminator(Terminator::Goto(header_block));

        self.current_block = exit_block;
        Ok(result)
    }

    /// Emit an inlined `.position { |item| pred }` loop.
    fn inline_position(
        &mut self,
        expr: &HirExpr,
        vec_id: LocalId,
        closure_params: &[HirClosureParam],
        closure_body: &HirExpr,
    ) -> Result<LocalId, String> {
        // Result is Option[USize] — tagged union
        let result = self.new_temp(expr.ty.clone());
        self.emit(MirInst::Alloc {
            dest: result,
            ty: expr.ty.clone(),
            size: 16,
        });
        self.emit(MirInst::SetTag {
            dest: result,
            tag: 0,
        }); // None

        let idx = self.new_temp(Ty::Int);
        self.emit(MirInst::Assign {
            dest: idx,
            value: MirValue::Literal(Literal::Int(0)),
        });

        let len = self.new_temp(Ty::Int);
        self.emit(MirInst::Call {
            dest: Some(len),
            callee: "riven_vec_len".to_string(),
            args: vec![MirValue::Use(vec_id)],
        });

        let header_block = self.new_block();
        let body_block = self.new_block();
        let found_block = self.new_block();
        let inc_block = self.new_block();
        let exit_block = self.new_block();

        self.set_terminator(Terminator::Goto(header_block));
        self.current_block = header_block;

        let cond = self.new_temp(Ty::Bool);
        self.emit(MirInst::Compare {
            dest: cond,
            op: CmpOp::Lt,
            lhs: MirValue::Use(idx),
            rhs: MirValue::Use(len),
        });
        self.set_terminator(Terminator::Branch {
            cond: MirValue::Use(cond),
            then_block: body_block,
            else_block: exit_block,
        });

        self.current_block = body_block;

        if let Some(param) = closure_params.first() {
            let item = self.new_local_named(&param.name, param.ty.clone(), false);
            self.def_to_local.insert(param.def_id, item);
            self.emit(MirInst::Call {
                dest: Some(item),
                callee: "riven_vec_get".to_string(),
                args: vec![MirValue::Use(vec_id), MirValue::Use(idx)],
            });
        }

        let pred_result = self.lower_expr(closure_body)?;
        let pred_val = local_to_value(pred_result);

        self.set_terminator(Terminator::Branch {
            cond: pred_val,
            then_block: found_block,
            else_block: inc_block,
        });

        // Found: set result to Some(idx)
        self.current_block = found_block;
        self.emit(MirInst::SetTag {
            dest: result,
            tag: 1,
        });
        self.emit(MirInst::SetField {
            base: result,
            field_index: 1,
            value: MirValue::Use(idx),
        });
        self.set_terminator(Terminator::Goto(exit_block));

        // Increment
        self.current_block = inc_block;
        let next_idx = self.new_temp(Ty::Int);
        self.emit(MirInst::BinOp {
            dest: next_idx,
            op: BinOp::Add,
            lhs: MirValue::Use(idx),
            rhs: MirValue::Literal(Literal::Int(1)),
        });
        self.emit(MirInst::Assign {
            dest: idx,
            value: MirValue::Use(next_idx),
        });
        self.set_terminator(Terminator::Goto(header_block));

        self.current_block = exit_block;
        Ok(result)
    }

    /// Emit an inlined `.map { |item| expr }` loop.
    fn inline_map(
        &mut self,
        expr: &HirExpr,
        vec_id: LocalId,
        closure_params: &[HirClosureParam],
        closure_body: &HirExpr,
    ) -> Result<LocalId, String> {
        let result = self.new_temp(expr.ty.clone());
        self.emit(MirInst::Call {
            dest: Some(result),
            callee: "riven_vec_new".to_string(),
            args: vec![],
        });

        let idx = self.new_temp(Ty::Int);
        self.emit(MirInst::Assign {
            dest: idx,
            value: MirValue::Literal(Literal::Int(0)),
        });

        let len = self.new_temp(Ty::Int);
        self.emit(MirInst::Call {
            dest: Some(len),
            callee: "riven_vec_len".to_string(),
            args: vec![MirValue::Use(vec_id)],
        });

        let header_block = self.new_block();
        let body_block = self.new_block();
        let exit_block = self.new_block();

        self.set_terminator(Terminator::Goto(header_block));
        self.current_block = header_block;

        let cond = self.new_temp(Ty::Bool);
        self.emit(MirInst::Compare {
            dest: cond,
            op: CmpOp::Lt,
            lhs: MirValue::Use(idx),
            rhs: MirValue::Use(len),
        });
        self.set_terminator(Terminator::Branch {
            cond: MirValue::Use(cond),
            then_block: body_block,
            else_block: exit_block,
        });

        self.current_block = body_block;

        if let Some(param) = closure_params.first() {
            let item = self.new_local_named(&param.name, param.ty.clone(), false);
            self.def_to_local.insert(param.def_id, item);
            self.emit(MirInst::Call {
                dest: Some(item),
                callee: "riven_vec_get".to_string(),
                args: vec![MirValue::Use(vec_id), MirValue::Use(idx)],
            });
        }

        // Evaluate the mapping expression
        let mapped_result = self.lower_expr(closure_body)?;
        let mapped_val = local_to_value(mapped_result);

        // Push mapped value
        let mapped_temp = self.new_temp(Ty::Int);
        self.emit(MirInst::Assign {
            dest: mapped_temp,
            value: mapped_val,
        });
        self.emit(MirInst::Call {
            dest: None,
            callee: "riven_vec_push".to_string(),
            args: vec![MirValue::Use(result), MirValue::Use(mapped_temp)],
        });

        // Increment
        let next_idx = self.new_temp(Ty::Int);
        self.emit(MirInst::BinOp {
            dest: next_idx,
            op: BinOp::Add,
            lhs: MirValue::Use(idx),
            rhs: MirValue::Literal(Literal::Int(1)),
        });
        self.emit(MirInst::Assign {
            dest: idx,
            value: MirValue::Use(next_idx),
        });
        self.set_terminator(Terminator::Goto(header_block));

        self.current_block = exit_block;
        Ok(result)
    }

    /// Emit an inlined `.partition { |item| pred }` loop.
    fn inline_partition(
        &mut self,
        expr: &HirExpr,
        vec_id: LocalId,
        closure_params: &[HirClosureParam],
        closure_body: &HirExpr,
    ) -> Result<LocalId, String> {
        // Allocate a tuple (true_vec, false_vec) — 16 bytes, 2 pointers
        let result = self.new_temp(expr.ty.clone());
        self.emit(MirInst::Alloc {
            dest: result,
            ty: expr.ty.clone(),
            size: 16,
        });

        // true_vec = Vec.new()
        let true_vec = self.new_temp(Ty::Int);
        self.emit(MirInst::Call {
            dest: Some(true_vec),
            callee: "riven_vec_new".to_string(),
            args: vec![],
        });

        // false_vec = Vec.new()
        let false_vec = self.new_temp(Ty::Int);
        self.emit(MirInst::Call {
            dest: Some(false_vec),
            callee: "riven_vec_new".to_string(),
            args: vec![],
        });

        let idx = self.new_temp(Ty::Int);
        self.emit(MirInst::Assign {
            dest: idx,
            value: MirValue::Literal(Literal::Int(0)),
        });

        let len = self.new_temp(Ty::Int);
        self.emit(MirInst::Call {
            dest: Some(len),
            callee: "riven_vec_len".to_string(),
            args: vec![MirValue::Use(vec_id)],
        });

        let header_block = self.new_block();
        let body_block = self.new_block();
        let true_block = self.new_block();
        let false_block = self.new_block();
        let inc_block = self.new_block();
        let exit_block = self.new_block();

        self.set_terminator(Terminator::Goto(header_block));
        self.current_block = header_block;

        let cond = self.new_temp(Ty::Bool);
        self.emit(MirInst::Compare {
            dest: cond,
            op: CmpOp::Lt,
            lhs: MirValue::Use(idx),
            rhs: MirValue::Use(len),
        });
        self.set_terminator(Terminator::Branch {
            cond: MirValue::Use(cond),
            then_block: body_block,
            else_block: exit_block,
        });

        self.current_block = body_block;

        let item_local = if let Some(param) = closure_params.first() {
            let item = self.new_local_named(&param.name, param.ty.clone(), false);
            self.def_to_local.insert(param.def_id, item);
            self.emit(MirInst::Call {
                dest: Some(item),
                callee: "riven_vec_get".to_string(),
                args: vec![MirValue::Use(vec_id), MirValue::Use(idx)],
            });
            item
        } else {
            self.new_temp(Ty::Int)
        };

        let pred_result = self.lower_expr(closure_body)?;
        let pred_val = local_to_value(pred_result);

        self.set_terminator(Terminator::Branch {
            cond: pred_val,
            then_block: true_block,
            else_block: false_block,
        });

        // True block: true_vec.push(item)
        self.current_block = true_block;
        self.emit(MirInst::Call {
            dest: None,
            callee: "riven_vec_push".to_string(),
            args: vec![MirValue::Use(true_vec), MirValue::Use(item_local)],
        });
        self.set_terminator(Terminator::Goto(inc_block));

        // False block: false_vec.push(item)
        self.current_block = false_block;
        self.emit(MirInst::Call {
            dest: None,
            callee: "riven_vec_push".to_string(),
            args: vec![MirValue::Use(false_vec), MirValue::Use(item_local)],
        });
        self.set_terminator(Terminator::Goto(inc_block));

        // Increment
        self.current_block = inc_block;
        let next_idx = self.new_temp(Ty::Int);
        self.emit(MirInst::BinOp {
            dest: next_idx,
            op: BinOp::Add,
            lhs: MirValue::Use(idx),
            rhs: MirValue::Literal(Literal::Int(1)),
        });
        self.emit(MirInst::Assign {
            dest: idx,
            value: MirValue::Use(next_idx),
        });
        self.set_terminator(Terminator::Goto(header_block));

        // Exit: store true_vec and false_vec into the result tuple
        self.current_block = exit_block;
        self.emit(MirInst::SetField {
            base: result,
            field_index: 0,
            value: MirValue::Use(true_vec),
        });
        self.emit(MirInst::SetField {
            base: result,
            field_index: 1,
            value: MirValue::Use(false_vec),
        });
        Ok(result)
    }

    /// Emit an inlined `iter.fold(init) { |acc, item| body }` loop
    /// (Phase 2 stdlib #05 batch 2). The accumulator is seeded from
    /// the lowered `init` argument, then the closure is invoked once
    /// per element with the running accumulator and the current item.
    /// The accumulator's type comes from the inferred result type
    /// (`expr.ty`); we copy `init` into a fresh local so subsequent
    /// closure-body lowerings can both read and write it.
    fn inline_fold(
        &mut self,
        expr: &HirExpr,
        vec_id: LocalId,
        args: &[HirExpr],
        closure_params: &[HirClosureParam],
        closure_body: &HirExpr,
    ) -> Result<LocalId, String> {
        // Lower `init` first; that yields the seed value for the accumulator.
        let init_arg = args
            .first()
            .ok_or_else(|| "fold requires an init argument".to_string())?;
        let init_local = self
            .lower_expr(init_arg)?
            .ok_or_else(|| "fold init argument has no value".to_string())?;

        // The accumulator local takes its name from the closure's first
        // parameter so the closure body's `acc` reference resolves to it.
        // Without a named local, the body's reference would have nowhere
        // to bind. Type comes from the closure-param annotation if
        // present, else falls back to `expr.ty` (the fold result type).
        let acc_ty = closure_params
            .first()
            .map(|p| p.ty.clone())
            .unwrap_or_else(|| expr.ty.clone());
        let acc_local = if let Some(param) = closure_params.first() {
            let l = self.new_local_named(&param.name, acc_ty.clone(), true);
            self.def_to_local.insert(param.def_id, l);
            l
        } else {
            self.new_temp(acc_ty.clone())
        };
        // Seed accumulator with init.
        self.emit_transfer(acc_local, init_local, &acc_ty, MoveSemantics::Copy);

        // Loop counters.
        let idx = self.new_temp(Ty::Int);
        self.emit(MirInst::Assign {
            dest: idx,
            value: MirValue::Literal(Literal::Int(0)),
        });
        let len = self.new_temp(Ty::Int);
        self.emit(MirInst::Call {
            dest: Some(len),
            callee: "riven_vec_len".to_string(),
            args: vec![MirValue::Use(vec_id)],
        });

        let header_block = self.new_block();
        let body_block = self.new_block();
        let exit_block = self.new_block();

        self.set_terminator(Terminator::Goto(header_block));
        self.current_block = header_block;

        let cond = self.new_temp(Ty::Bool);
        self.emit(MirInst::Compare {
            dest: cond,
            op: CmpOp::Lt,
            lhs: MirValue::Use(idx),
            rhs: MirValue::Use(len),
        });
        self.set_terminator(Terminator::Branch {
            cond: MirValue::Use(cond),
            then_block: body_block,
            else_block: exit_block,
        });

        // Body: bind the per-iteration item (closure param 1), invoke
        // the closure body, store the result back into `acc`.
        self.current_block = body_block;
        if let Some(param) = closure_params.get(1) {
            let item_local = self.new_local_named(&param.name, param.ty.clone(), false);
            self.def_to_local.insert(param.def_id, item_local);
            self.emit(MirInst::Call {
                dest: Some(item_local),
                callee: "riven_vec_get".to_string(),
                args: vec![MirValue::Use(vec_id), MirValue::Use(idx)],
            });
        }
        let body_result = self.lower_expr(closure_body)?;
        if let Some(body_id) = body_result {
            // acc = closure_body(acc, item)
            self.emit(MirInst::Assign {
                dest: acc_local,
                value: MirValue::Use(body_id),
            });
        }

        // idx += 1; back to header.
        let next_idx = self.new_temp(Ty::Int);
        self.emit(MirInst::BinOp {
            dest: next_idx,
            op: BinOp::Add,
            lhs: MirValue::Use(idx),
            rhs: MirValue::Literal(Literal::Int(1)),
        });
        self.emit(MirInst::Assign {
            dest: idx,
            value: MirValue::Use(next_idx),
        });
        if matches!(self.get_terminator(), Terminator::Unreachable) {
            self.set_terminator(Terminator::Goto(header_block));
        }

        self.current_block = exit_block;
        Ok(acc_local)
    }

    /// Emit an inlined `iter.all { |x| pred }` / `iter.any { |x| pred }`
    /// loop (Phase 2 stdlib #05 batch 2). Both share the same loop
    /// shape — bind item, evaluate predicate, branch — but they
    /// short-circuit on different boolean values:
    ///   - `all`: stop and return `false` on the first `false`
    ///   - `any`: stop and return `true`  on the first `true`
    ///
    /// The `is_all` flag selects between these two early-exit modes.
    /// On a fully-iterated empty/uneventful sequence the result is
    /// the *vacuous truth* for `all` (`true`) or the *vacuous
    /// falsehood* for `any` (`false`), matching Rust's `Iterator`.
    fn inline_all_any(
        &mut self,
        _expr: &HirExpr,
        vec_id: LocalId,
        closure_params: &[HirClosureParam],
        closure_body: &HirExpr,
        is_all: bool,
    ) -> Result<LocalId, String> {
        // Result lives in a single mutable Bool local; seed with the
        // vacuous answer (true for all, false for any).
        let result = self.new_temp(Ty::Bool);
        self.emit(MirInst::Assign {
            dest: result,
            value: MirValue::Literal(Literal::Bool(is_all)),
        });

        let idx = self.new_temp(Ty::Int);
        self.emit(MirInst::Assign {
            dest: idx,
            value: MirValue::Literal(Literal::Int(0)),
        });
        let len = self.new_temp(Ty::Int);
        self.emit(MirInst::Call {
            dest: Some(len),
            callee: "riven_vec_len".to_string(),
            args: vec![MirValue::Use(vec_id)],
        });

        let header_block = self.new_block();
        let body_block = self.new_block();
        let short_block = self.new_block();
        let inc_block = self.new_block();
        let exit_block = self.new_block();

        self.set_terminator(Terminator::Goto(header_block));
        self.current_block = header_block;

        let cond = self.new_temp(Ty::Bool);
        self.emit(MirInst::Compare {
            dest: cond,
            op: CmpOp::Lt,
            lhs: MirValue::Use(idx),
            rhs: MirValue::Use(len),
        });
        self.set_terminator(Terminator::Branch {
            cond: MirValue::Use(cond),
            then_block: body_block,
            else_block: exit_block,
        });

        self.current_block = body_block;
        if let Some(param) = closure_params.first() {
            let item = self.new_local_named(&param.name, param.ty.clone(), false);
            self.def_to_local.insert(param.def_id, item);
            self.emit(MirInst::Call {
                dest: Some(item),
                callee: "riven_vec_get".to_string(),
                args: vec![MirValue::Use(vec_id), MirValue::Use(idx)],
            });
        }
        let pred_result = self.lower_expr(closure_body)?;
        let pred_val = local_to_value(pred_result);

        // For `all`: predicate=false  → short-circuit (set false, exit)
        // For `any`: predicate=true   → short-circuit (set true,  exit)
        // The branch selects which arm goes to short-circuit vs continue.
        let (then_block, else_block) = if is_all {
            (inc_block, short_block) // pred=true → continue, false → short
        } else {
            (short_block, inc_block) // pred=true → short, false → continue
        };
        self.set_terminator(Terminator::Branch {
            cond: pred_val,
            then_block,
            else_block,
        });

        // Short-circuit: flip result to the opposite seed and exit.
        self.current_block = short_block;
        self.emit(MirInst::Assign {
            dest: result,
            value: MirValue::Literal(Literal::Bool(!is_all)),
        });
        self.set_terminator(Terminator::Goto(exit_block));

        // Increment: idx += 1; back to header.
        self.current_block = inc_block;
        let next_idx = self.new_temp(Ty::Int);
        self.emit(MirInst::BinOp {
            dest: next_idx,
            op: BinOp::Add,
            lhs: MirValue::Use(idx),
            rhs: MirValue::Literal(Literal::Int(1)),
        });
        self.emit(MirInst::Assign {
            dest: idx,
            value: MirValue::Use(next_idx),
        });
        self.set_terminator(Terminator::Goto(header_block));

        self.current_block = exit_block;
        Ok(result)
    }

    /// Inline an `Option.map { |x| expr }` call.
    ///
    /// Generates: if tag == 1 (Some): apply closure to payload, wrap in new Some
    ///            else: return the original None option
    fn inline_option_map(
        &mut self,
        expr: &HirExpr,
        option_expr: &HirExpr,
        closure_params: &[HirClosureParam],
        closure_body: &HirExpr,
    ) -> Result<Option<Option<LocalId>>, String> {
        let opt_local = self.lower_expr(option_expr)?;
        let opt_id = opt_local.unwrap_or_else(|| self.new_temp(Ty::Int));

        // Allocate the result Option (16 bytes: tag + payload)
        let result = self.new_temp(expr.ty.clone());
        self.emit(MirInst::Alloc {
            dest: result,
            ty: expr.ty.clone(),
            size: 16,
        });

        // Get the tag of the input Option
        let tag = self.new_temp(Ty::Int32);
        self.emit(MirInst::GetTag {
            dest: tag,
            src: opt_id,
        });

        // Check if Some (tag == 1)
        let is_some = self.new_temp(Ty::Bool);
        self.emit(MirInst::Compare {
            dest: is_some,
            op: CmpOp::Eq,
            lhs: MirValue::Use(tag),
            rhs: MirValue::Literal(Literal::Int(1)),
        });

        let some_block = self.new_block();
        let none_block = self.new_block();
        let merge_block = self.new_block();

        self.set_terminator(Terminator::Branch {
            cond: MirValue::Use(is_some),
            then_block: some_block,
            else_block: none_block,
        });

        // Some block: extract payload, apply closure, wrap in new Some
        self.current_block = some_block;

        // Get the payload from the input Option
        let payload = self.new_temp(Ty::Int);
        self.emit(MirInst::GetField {
            dest: payload,
            base: opt_id,
            field_index: 1, // payload is at offset 8
        });

        // Bind the closure parameter to the payload.
        // If the parameter type is Infer, refine it using the inner type
        // of the Option being mapped, so that string interpolation and
        // other type-sensitive lowering works correctly.
        if let Some(param) = closure_params.first() {
            let param_ty = if matches!(param.ty, Ty::Infer(_)) {
                match &option_expr.ty {
                    Ty::Option(inner) => inner.as_ref().clone(),
                    _ => param.ty.clone(),
                }
            } else {
                param.ty.clone()
            };
            let param_local = self.new_local_named(&param.name, param_ty, false);
            self.def_to_local.insert(param.def_id, param_local);
            self.emit(MirInst::Assign {
                dest: param_local,
                value: MirValue::Use(payload),
            });
        }

        // Evaluate the closure body to get the transformed value
        let mapped_result = self.lower_expr(closure_body)?;
        let mapped_val = local_to_value(mapped_result);

        // Set result to Some(mapped_value)
        self.emit(MirInst::SetTag {
            dest: result,
            tag: 1,
        });
        self.emit(MirInst::SetField {
            base: result,
            field_index: 1,
            value: mapped_val,
        });
        self.set_terminator(Terminator::Goto(merge_block));

        // None block: set result to None
        self.current_block = none_block;
        self.emit(MirInst::SetTag {
            dest: result,
            tag: 0,
        });
        self.set_terminator(Terminator::Goto(merge_block));

        self.current_block = merge_block;
        Ok(Some(Some(result)))
    }

    /// Inline `result.map { |v| ... }` (when `on_ok = true`) or
    /// `result.map_err { |e| ... }` (when `on_ok = false`). Both variants
    /// branch on the Result tag, run the closure on the matching arm's
    /// payload, and pass the other arm's payload through unchanged.
    fn inline_result_map(
        &mut self,
        expr: &HirExpr,
        result_expr: &HirExpr,
        closure_params: &[HirClosureParam],
        closure_body: &HirExpr,
        on_ok: bool,
    ) -> Result<Option<Option<LocalId>>, String> {
        let res_local = self.lower_expr(result_expr)?;
        let res_id = res_local.unwrap_or_else(|| self.new_temp(Ty::Int));

        let result = self.new_temp(expr.ty.clone());
        self.emit(MirInst::Alloc {
            dest: result,
            ty: expr.ty.clone(),
            size: 16,
        });

        let tag = self.new_temp(Ty::Int32);
        self.emit(MirInst::GetTag {
            dest: tag,
            src: res_id,
        });

        // Result: Ok=0, Err=1.
        let match_tag = if on_ok { 0 } else { 1 };
        let match_block = self.new_block();
        let other_block = self.new_block();
        let merge_block = self.new_block();

        let take_branch = self.new_temp(Ty::Bool);
        self.emit(MirInst::Compare {
            dest: take_branch,
            op: CmpOp::Eq,
            lhs: MirValue::Use(tag),
            rhs: MirValue::Literal(Literal::Int(match_tag)),
        });
        self.set_terminator(Terminator::Branch {
            cond: MirValue::Use(take_branch),
            then_block: match_block,
            else_block: other_block,
        });

        // Matching arm: payload → closure → repackage with same tag.
        self.current_block = match_block;
        let payload = self.new_temp(Ty::Int);
        self.emit(MirInst::GetField {
            dest: payload,
            base: res_id,
            field_index: 1,
        });

        if let Some(param) = closure_params.first() {
            let param_ty = if matches!(param.ty, Ty::Infer(_)) {
                match &result_expr.ty {
                    Ty::Result(ok, err) => {
                        if on_ok {
                            ok.as_ref().clone()
                        } else {
                            err.as_ref().clone()
                        }
                    }
                    _ => param.ty.clone(),
                }
            } else {
                param.ty.clone()
            };
            let param_local = self.new_local_named(&param.name, param_ty, false);
            self.def_to_local.insert(param.def_id, param_local);
            self.emit(MirInst::Assign {
                dest: param_local,
                value: MirValue::Use(payload),
            });
        }

        let mapped = self.lower_expr(closure_body)?;
        let mapped_val = local_to_value(mapped);

        self.emit(MirInst::SetTag {
            dest: result,
            tag: match_tag as u32,
        });
        self.emit(MirInst::SetField {
            base: result,
            field_index: 1,
            value: mapped_val,
        });
        self.set_terminator(Terminator::Goto(merge_block));

        // Other arm: passthrough — same tag, same payload.
        self.current_block = other_block;
        let other_payload = self.new_temp(Ty::Int);
        self.emit(MirInst::GetField {
            dest: other_payload,
            base: res_id,
            field_index: 1,
        });
        let other_tag = if on_ok { 1 } else { 0 };
        self.emit(MirInst::SetTag {
            dest: result,
            tag: other_tag as u32,
        });
        self.emit(MirInst::SetField {
            base: result,
            field_index: 1,
            value: MirValue::Use(other_payload),
        });
        self.set_terminator(Terminator::Goto(merge_block));

        self.current_block = merge_block;
        Ok(Some(Some(result)))
    }

    /// Inline `result.unwrap_or_else { |e| ... }` and
    /// `option.unwrap_or_else { |_| ... }`. `ok_tag` is 0 for Result
    /// (Ok=0), 1 for Option (Some=1). On the success arm the inner
    /// payload is returned; otherwise the closure runs (for Result the
    /// closure binds the Err payload; for Option it binds nothing).
    fn inline_unwrap_or_else(
        &mut self,
        expr: &HirExpr,
        receiver_expr: &HirExpr,
        closure_params: &[HirClosureParam],
        closure_body: &HirExpr,
        ok_tag: i64,
    ) -> Result<Option<Option<LocalId>>, String> {
        let recv_local = self.lower_expr(receiver_expr)?;
        let recv_id = recv_local.unwrap_or_else(|| self.new_temp(Ty::Int));

        let result = self.new_temp(expr.ty.clone());

        let tag = self.new_temp(Ty::Int32);
        self.emit(MirInst::GetTag {
            dest: tag,
            src: recv_id,
        });

        let is_ok = self.new_temp(Ty::Bool);
        self.emit(MirInst::Compare {
            dest: is_ok,
            op: CmpOp::Eq,
            lhs: MirValue::Use(tag),
            rhs: MirValue::Literal(Literal::Int(ok_tag)),
        });

        let ok_block = self.new_block();
        let err_block = self.new_block();
        let merge_block = self.new_block();

        self.set_terminator(Terminator::Branch {
            cond: MirValue::Use(is_ok),
            then_block: ok_block,
            else_block: err_block,
        });

        // Success arm: result = payload.
        self.current_block = ok_block;
        let ok_payload = self.new_temp(expr.ty.clone());
        self.emit(MirInst::GetField {
            dest: ok_payload,
            base: recv_id,
            field_index: 1,
        });
        self.emit(MirInst::Assign {
            dest: result,
            value: MirValue::Use(ok_payload),
        });
        self.set_terminator(Terminator::Goto(merge_block));

        // Error arm: bind closure param to err payload, run body.
        self.current_block = err_block;
        if let Some(param) = closure_params.first() {
            let err_payload = self.new_temp(Ty::Int);
            self.emit(MirInst::GetField {
                dest: err_payload,
                base: recv_id,
                field_index: 1,
            });
            let param_ty = if matches!(param.ty, Ty::Infer(_)) {
                match &receiver_expr.ty {
                    Ty::Result(_, err) => err.as_ref().clone(),
                    _ => param.ty.clone(),
                }
            } else {
                param.ty.clone()
            };
            let param_local = self.new_local_named(&param.name, param_ty, false);
            self.def_to_local.insert(param.def_id, param_local);
            self.emit(MirInst::Assign {
                dest: param_local,
                value: MirValue::Use(err_payload),
            });
        }
        let body_val = self.lower_expr(closure_body)?;
        // If the closure body produces a value, use it as the result;
        // otherwise leave `result` uninitialised (Unit-typed call sites).
        if let Some(v) = body_val {
            self.emit(MirInst::Assign {
                dest: result,
                value: MirValue::Use(v),
            });
        }
        self.set_terminator(Terminator::Goto(merge_block));

        self.current_block = merge_block;
        Ok(Some(Some(result)))
    }

    // ── Helpers ─────────────────────────────────────────────────────────

    /// Get a mutable reference to the current MIR function.
    fn fn_mut(&mut self) -> &mut MirFunction {
        self.current_fn.as_mut().expect("no current function")
    }

    fn fn_ref(&self) -> &MirFunction {
        self.current_fn.as_ref().expect("no current function")
    }

    /// Emit `riven_dealloc(L); L = 0` for each `L`, in the current
    /// block. Used at every loop-exit edge (break, continue, back-edge)
    /// to free heap allocations made inside the body. The zero-store
    /// after dealloc keeps `compute_dealloc_safe_locals` from re-
    /// emitting a function-end dealloc on a stale pointer, and means a
    /// later iteration that bypasses the `let` will dealloc NULL.
    fn emit_dealloc_loop_locals(&mut self, locals: &[LocalId]) {
        for &local in locals {
            self.emit(MirInst::Call {
                dest: None,
                callee: "riven_dealloc".to_string(),
                args: vec![MirValue::Use(local)],
            });
            self.emit(MirInst::Assign {
                dest: local,
                value: MirValue::Literal(Literal::Int(0)),
            });
        }
    }

    /// Insert `Assign L = 0` at the very top of the loop's body-entry
    /// block for every body-local. Runs once before the first iteration
    /// so a path that bypasses a `let` (e.g. inside a nested `if`)
    /// reaches its first dealloc with a NULL value.
    fn prepend_zero_init_for_body_locals(&mut self, frame: &LoopFrame) {
        if frame.body_locals.is_empty() {
            return;
        }
        let block = &mut self.fn_mut().blocks[frame.body_entry_block];
        for (i, &local) in frame.body_locals.iter().enumerate() {
            block.instructions.insert(
                i,
                MirInst::Assign {
                    dest: local,
                    value: MirValue::Literal(Literal::Int(0)),
                },
            );
        }
    }

    /// Push an instruction onto the current basic block.
    fn emit(&mut self, inst: MirInst) {
        let block_id = self.current_block;
        let func = self.current_fn.as_mut().expect("no current function");
        func.blocks[block_id].instructions.push(inst);
    }

    /// Emit a value-transfer instruction selected by the source's move
    /// semantics and the type's effective Copy-ness. Move-bound owned
    /// values become `MirInst::Move` (so drop-elaboration follows the
    /// LIFO order); Copy values become `MirInst::Copy`; everything else
    /// degrades to a plain `Assign`.
    fn emit_transfer(&mut self, dest: LocalId, src: LocalId, ty: &Ty, semantics: MoveSemantics) {
        let inst = match semantics {
            MoveSemantics::Move if !ty_is_effectively_copy(ty, self.symbols) => {
                MirInst::Move { dest, src }
            }
            _ if ty_is_effectively_copy(ty, self.symbols) => MirInst::Copy { dest, src },
            _ => MirInst::Assign {
                dest,
                value: MirValue::Use(src),
            },
        };
        self.emit(inst);
    }

    /// Lower a string literal as a heap-owned `String`. The raw
    /// `MirInst::StringLiteral` produces a pointer into `.rodata`;
    /// dropping such a pointer would call `free()` on a static address.
    /// `riven_string_from` copies it to the heap so `String::drop` is
    /// safe on the result. (P0.7)
    fn emit_owned_string_literal(&mut self, value: &str) -> LocalId {
        let raw = self.new_temp(Ty::Str);
        self.emit(MirInst::StringLiteral {
            dest: raw,
            value: value.to_string(),
        });
        let owned = self.new_temp(Ty::String);
        self.emit(MirInst::Call {
            dest: Some(owned),
            callee: "riven_string_from".to_string(),
            args: vec![MirValue::Use(raw)],
        });
        owned
    }

    /// Phase 2 #06.D2.S1: synthesize a `Display::fmt` MIR function for each
    /// primitive type that participates in string interpolation. Each emitted
    /// function has signature `(self: Prim, fmt: &mut Formatter) -> Unit` and
    /// delegates to the existing `riven_<prim>_to_string` runtime helper
    /// (except `String_fmt`, which writes `self` directly). These functions
    /// are emitted unconditionally at program-lowering time and serve as the
    /// canonical target once `lower_interpolation` is rewired in Stage 3.
    fn synthesize_primitive_fmt_displays(&self) -> Vec<MirFunction> {
        let formatter_ty = Ty::RefMut(Box::new(Ty::Class {
            name: "Formatter".to_string(),
            generic_args: vec![],
        }));

        // (fn_name, self_ty, primitive kind — controls how the value is
        // converted to a string before write_str).  Kind drives which
        // runtime helper is invoked and whether precision is read from
        // the Formatter (Float / String only — Char / Int / Bool ignore
        // precision per Rust semantics).
        enum Kind {
            Char,
            Int,
            Float,
            Bool,
            String_,
        }
        let specs: &[(&str, Ty, Kind)] = &[
            ("Char_fmt", Ty::Char, Kind::Char),
            ("Int_fmt", Ty::Int, Kind::Int),
            ("Float_fmt", Ty::Float, Kind::Float),
            ("Bool_fmt", Ty::Bool, Kind::Bool),
            ("String_fmt", Ty::String, Kind::String_),
        ];

        let mut out = Vec::with_capacity(specs.len());
        for (name, self_ty, kind) in specs {
            let mut mir_fn = MirFunction::new(*name, Ty::Unit);
            let self_local = mir_fn.new_local("self", self_ty.clone(), false);
            mir_fn.params.push(self_local);
            let fmt_local = mir_fn.new_local("fmt", formatter_ty.clone(), true);
            mir_fn.params.push(fmt_local);

            let entry = mir_fn.entry_block;

            // Phase 2 #06.D4: Float and String consult the formatter's
            // precision slot; Char / Int / Bool do not.
            let str_local = match kind {
                Kind::Char => {
                    let dest = mir_fn.new_temp(Ty::String);
                    mir_fn.blocks[entry].instructions.push(MirInst::Call {
                        dest: Some(dest),
                        callee: "riven_char_to_string".to_string(),
                        args: vec![MirValue::Use(self_local)],
                    });
                    dest
                }
                Kind::Int => {
                    let dest = mir_fn.new_temp(Ty::String);
                    mir_fn.blocks[entry].instructions.push(MirInst::Call {
                        dest: Some(dest),
                        callee: "riven_int_to_string".to_string(),
                        args: vec![MirValue::Use(self_local)],
                    });
                    dest
                }
                Kind::Bool => {
                    let dest = mir_fn.new_temp(Ty::String);
                    mir_fn.blocks[entry].instructions.push(MirInst::Call {
                        dest: Some(dest),
                        callee: "riven_bool_to_string".to_string(),
                        args: vec![MirValue::Use(self_local)],
                    });
                    dest
                }
                Kind::Float => {
                    // p = Formatter_precision(fmt)
                    let prec_local = mir_fn.new_temp(Ty::Int);
                    mir_fn.blocks[entry].instructions.push(MirInst::Call {
                        dest: Some(prec_local),
                        callee: "Formatter_precision".to_string(),
                        args: vec![MirValue::Use(fmt_local)],
                    });
                    // s = Float_to_string_prec(self, p)
                    let dest = mir_fn.new_temp(Ty::String);
                    mir_fn.blocks[entry].instructions.push(MirInst::Call {
                        dest: Some(dest),
                        callee: "Float_to_string_prec".to_string(),
                        args: vec![MirValue::Use(self_local), MirValue::Use(prec_local)],
                    });
                    dest
                }
                Kind::String_ => {
                    // p = Formatter_precision(fmt)
                    let prec_local = mir_fn.new_temp(Ty::Int);
                    mir_fn.blocks[entry].instructions.push(MirInst::Call {
                        dest: Some(prec_local),
                        callee: "Formatter_precision".to_string(),
                        args: vec![MirValue::Use(fmt_local)],
                    });
                    // s = String_truncate_chars(self, p)   (p == -1 → copy)
                    let dest = mir_fn.new_temp(Ty::String);
                    mir_fn.blocks[entry].instructions.push(MirInst::Call {
                        dest: Some(dest),
                        callee: "String_truncate_chars".to_string(),
                        args: vec![MirValue::Use(self_local), MirValue::Use(prec_local)],
                    });
                    dest
                }
            };

            // Formatter_write_str(fmt, str_value) — result discarded.
            mir_fn.blocks[entry].instructions.push(MirInst::Call {
                dest: None,
                callee: "Formatter_write_str".to_string(),
                args: vec![MirValue::Use(fmt_local), MirValue::Use(str_local)],
            });

            mir_fn.blocks[entry].terminator = Terminator::Return(None);
            out.push(mir_fn);
        }
        out
    }

    /// Synthesize the body of `{StructName}_to_debug(self) -> String` for a
    /// struct that declares `derive Debug`. Output shape:
    /// `Name { field1: <fmt(field1)>, field2: <fmt(field2)>, ... }`.
    /// v1 limitation: only primitive field types are formatted faithfully;
    /// other struct fields with `derive Debug` recurse; everything else
    /// renders as `<...>` so the formatter never panics.
    fn synthesize_struct_to_debug(&self, s: &HirStructDef) -> MirFunction {
        let fn_name = format!("{}_to_debug", s.name);
        let self_ty = Ty::Struct {
            name: s.name.clone(),
            generic_args: vec![],
        };

        let mut mir_fn = MirFunction::new(&fn_name, Ty::String);
        let self_local = mir_fn.new_local("self", self_ty, false);
        mir_fn.params.push(self_local);

        let entry = mir_fn.entry_block;

        let leading = if s.fields.is_empty() {
            format!("{} {{}}", s.name)
        } else {
            format!("{} {{ ", s.name)
        };

        let leading_local = mir_fn.new_temp(Ty::String);
        mir_fn.blocks[entry]
            .instructions
            .push(MirInst::StringLiteral {
                dest: leading_local,
                value: leading,
            });
        let mut acc = leading_local;

        for (idx, field) in s.fields.iter().enumerate() {
            if idx > 0 {
                let sep = mir_fn.new_temp(Ty::String);
                mir_fn.blocks[entry]
                    .instructions
                    .push(MirInst::StringLiteral {
                        dest: sep,
                        value: ", ".to_string(),
                    });
                let next = mir_fn.new_temp(Ty::String);
                mir_fn.blocks[entry].instructions.push(MirInst::Call {
                    dest: Some(next),
                    callee: "riven_string_concat".to_string(),
                    args: vec![MirValue::Use(acc), MirValue::Use(sep)],
                });
                acc = next;
            }

            let label = mir_fn.new_temp(Ty::String);
            mir_fn.blocks[entry]
                .instructions
                .push(MirInst::StringLiteral {
                    dest: label,
                    value: format!("{}: ", field.name),
                });
            let after_label = mir_fn.new_temp(Ty::String);
            mir_fn.blocks[entry].instructions.push(MirInst::Call {
                dest: Some(after_label),
                callee: "riven_string_concat".to_string(),
                args: vec![MirValue::Use(acc), MirValue::Use(label)],
            });
            acc = after_label;

            let field_local = mir_fn.new_temp(field.ty.clone());
            mir_fn.blocks[entry].instructions.push(MirInst::GetField {
                dest: field_local,
                base: self_local,
                field_index: idx,
            });

            let field_str = if field.ty == Ty::Char {
                let dest = mir_fn.new_temp(Ty::String);
                mir_fn.blocks[entry].instructions.push(MirInst::Call {
                    dest: Some(dest),
                    callee: "riven_char_to_string".to_string(),
                    args: vec![MirValue::Use(field_local)],
                });
                dest
            } else if field.ty.is_integer() {
                let dest = mir_fn.new_temp(Ty::String);
                mir_fn.blocks[entry].instructions.push(MirInst::Call {
                    dest: Some(dest),
                    callee: "riven_int_to_string".to_string(),
                    args: vec![MirValue::Use(field_local)],
                });
                dest
            } else if field.ty.is_float() {
                let dest = mir_fn.new_temp(Ty::String);
                mir_fn.blocks[entry].instructions.push(MirInst::Call {
                    dest: Some(dest),
                    callee: "riven_float_to_string".to_string(),
                    args: vec![MirValue::Use(field_local)],
                });
                dest
            } else if field.ty == Ty::Bool {
                let dest = mir_fn.new_temp(Ty::String);
                mir_fn.blocks[entry].instructions.push(MirInst::Call {
                    dest: Some(dest),
                    callee: "riven_bool_to_string".to_string(),
                    args: vec![MirValue::Use(field_local)],
                });
                dest
            } else if matches!(field.ty, Ty::String | Ty::Str) {
                field_local
            } else if let Some(inner_struct_name) = self.struct_with_derive_debug(&field.ty) {
                let dest = mir_fn.new_temp(Ty::String);
                mir_fn.blocks[entry].instructions.push(MirInst::Call {
                    dest: Some(dest),
                    callee: format!("{}_to_debug", inner_struct_name),
                    args: vec![MirValue::Use(field_local)],
                });
                dest
            } else {
                let dest = mir_fn.new_temp(Ty::String);
                mir_fn.blocks[entry]
                    .instructions
                    .push(MirInst::StringLiteral {
                        dest,
                        value: "<...>".to_string(),
                    });
                dest
            };

            let next = mir_fn.new_temp(Ty::String);
            mir_fn.blocks[entry].instructions.push(MirInst::Call {
                dest: Some(next),
                callee: "riven_string_concat".to_string(),
                args: vec![MirValue::Use(acc), MirValue::Use(field_str)],
            });
            acc = next;
        }

        if !s.fields.is_empty() {
            let trailing = mir_fn.new_temp(Ty::String);
            mir_fn.blocks[entry]
                .instructions
                .push(MirInst::StringLiteral {
                    dest: trailing,
                    value: " }".to_string(),
                });
            let next = mir_fn.new_temp(Ty::String);
            mir_fn.blocks[entry].instructions.push(MirInst::Call {
                dest: Some(next),
                callee: "riven_string_concat".to_string(),
                args: vec![MirValue::Use(acc), MirValue::Use(trailing)],
            });
            acc = next;
        }

        mir_fn.blocks[entry].terminator = Terminator::Return(Some(MirValue::Use(acc)));
        mir_fn
    }

    fn synthesize_struct_eq(&self, s: &HirStructDef) -> MirFunction {
        let fn_name = format!("{}_eq", s.name);
        let self_ty = Ty::Struct {
            name: s.name.clone(),
            generic_args: vec![],
        };

        let mut mir_fn = MirFunction::new(&fn_name, Ty::Bool);
        let self_local = mir_fn.new_local("self", self_ty.clone(), false);
        let other_local = mir_fn.new_local("other", Ty::Ref(Box::new(self_ty)), false);
        mir_fn.params.push(self_local);
        mir_fn.params.push(other_local);

        let entry = mir_fn.entry_block;
        let mut acc = mir_fn.new_temp(Ty::Bool);
        mir_fn.blocks[entry].instructions.push(MirInst::Assign {
            dest: acc,
            value: MirValue::Literal(Literal::Bool(true)),
        });

        for (idx, field) in s.fields.iter().enumerate() {
            let lhs = mir_fn.new_temp(field.ty.clone());
            mir_fn.blocks[entry].instructions.push(MirInst::GetField {
                dest: lhs,
                base: self_local,
                field_index: idx,
            });
            let rhs = mir_fn.new_temp(field.ty.clone());
            mir_fn.blocks[entry].instructions.push(MirInst::GetField {
                dest: rhs,
                base: other_local,
                field_index: idx,
            });

            let field_eq =
                if let Some(inner_name) = self.struct_with_derive_trait(&field.ty, "PartialEq") {
                    let dest = mir_fn.new_temp(Ty::Bool);
                    mir_fn.blocks[entry].instructions.push(MirInst::Call {
                        dest: Some(dest),
                        callee: format!("{}_eq", inner_name),
                        args: vec![MirValue::Use(lhs), MirValue::Use(rhs)],
                    });
                    dest
                } else if matches!(field.ty, Ty::String | Ty::Str) {
                    let dest = mir_fn.new_temp(Ty::Bool);
                    mir_fn.blocks[entry].instructions.push(MirInst::Call {
                        dest: Some(dest),
                        callee: "riven_string_eq".to_string(),
                        args: vec![MirValue::Use(lhs), MirValue::Use(rhs)],
                    });
                    dest
                } else {
                    let dest = mir_fn.new_temp(Ty::Bool);
                    mir_fn.blocks[entry].instructions.push(MirInst::Compare {
                        dest,
                        op: CmpOp::Eq,
                        lhs: MirValue::Use(lhs),
                        rhs: MirValue::Use(rhs),
                    });
                    dest
                };

            let next = mir_fn.new_temp(Ty::Bool);
            mir_fn.blocks[entry].instructions.push(MirInst::BinOp {
                dest: next,
                op: BinOp::And,
                lhs: MirValue::Use(acc),
                rhs: MirValue::Use(field_eq),
            });
            acc = next;
        }

        mir_fn.blocks[entry].terminator = Terminator::Return(Some(MirValue::Use(acc)));
        mir_fn
    }

    fn synthesize_struct_hash_code(&self, s: &HirStructDef) -> MirFunction {
        let fn_name = format!("{}_hash_code", s.name);
        let self_ty = Ty::Struct {
            name: s.name.clone(),
            generic_args: vec![],
        };

        let mut mir_fn = MirFunction::new(&fn_name, Ty::Int);
        let self_local = mir_fn.new_local("self", self_ty, false);
        mir_fn.params.push(self_local);

        let entry = mir_fn.entry_block;
        let mut acc = mir_fn.new_temp(Ty::Int);
        mir_fn.blocks[entry].instructions.push(MirInst::Assign {
            dest: acc,
            value: MirValue::Literal(Literal::Int(1469598103934665603_i64)),
        });

        for (idx, field) in s.fields.iter().enumerate() {
            let field_local = mir_fn.new_temp(field.ty.clone());
            mir_fn.blocks[entry].instructions.push(MirInst::GetField {
                dest: field_local,
                base: self_local,
                field_index: idx,
            });

            let field_hash = if let Some(inner_name) = self
                .struct_with_derive_trait(&field.ty, "Hashable")
                .or_else(|| self.struct_with_derive_trait(&field.ty, "Hash"))
            {
                let dest = mir_fn.new_temp(Ty::Int);
                mir_fn.blocks[entry].instructions.push(MirInst::Call {
                    dest: Some(dest),
                    callee: format!("{}_hash_code", inner_name),
                    args: vec![MirValue::Use(field_local)],
                });
                dest
            } else if matches!(field.ty, Ty::String | Ty::Str) {
                let dest = mir_fn.new_temp(Ty::Int);
                mir_fn.blocks[entry].instructions.push(MirInst::Call {
                    dest: Some(dest),
                    callee: "riven_string_hash".to_string(),
                    args: vec![MirValue::Use(field_local)],
                });
                dest
            } else {
                field_local
            };

            let xored = mir_fn.new_temp(Ty::Int);
            mir_fn.blocks[entry].instructions.push(MirInst::BinOp {
                dest: xored,
                op: BinOp::BitXor,
                lhs: MirValue::Use(acc),
                rhs: MirValue::Use(field_hash),
            });
            let next = mir_fn.new_temp(Ty::Int);
            mir_fn.blocks[entry].instructions.push(MirInst::BinOp {
                dest: next,
                op: BinOp::Mul,
                lhs: MirValue::Use(xored),
                rhs: MirValue::Literal(Literal::Int(1099511628211_i64)),
            });
            acc = next;
        }

        mir_fn.blocks[entry].terminator = Terminator::Return(Some(MirValue::Use(acc)));
        mir_fn
    }

    fn synthesize_struct_default(&self, s: &HirStructDef) -> MirFunction {
        let fn_name = format!("{}_default", s.name);
        let self_ty = Ty::Struct {
            name: s.name.clone(),
            generic_args: vec![],
        };

        let mut mir_fn = MirFunction::new(&fn_name, self_ty.clone());
        let entry = mir_fn.entry_block;
        let obj = mir_fn.new_temp(self_ty.clone());
        mir_fn.blocks[entry].instructions.push(MirInst::Alloc {
            dest: obj,
            ty: self_ty.clone(),
            size: self.alloc_size(&self_ty),
        });

        for (idx, field) in s.fields.iter().enumerate() {
            let value = self.synthesize_default_value(&mut mir_fn, entry, &field.ty);
            mir_fn.blocks[entry].instructions.push(MirInst::SetField {
                base: obj,
                field_index: idx,
                value: MirValue::Use(value),
            });
        }

        mir_fn.blocks[entry].terminator = Terminator::Return(Some(MirValue::Use(obj)));
        mir_fn
    }

    fn synthesize_struct_cmp(&self, s: &HirStructDef, partial: bool) -> MirFunction {
        let method_name = if partial { "partial_cmp" } else { "cmp" };
        let fn_name = format!("{}_{}", s.name, method_name);
        let self_ty = Ty::Struct {
            name: s.name.clone(),
            generic_args: vec![],
        };

        let mut mir_fn = MirFunction::new(&fn_name, Ty::Int);
        let self_local = mir_fn.new_local("self", self_ty.clone(), false);
        let other_local = mir_fn.new_local("other", Ty::Ref(Box::new(self_ty)), false);
        mir_fn.params.push(self_local);
        mir_fn.params.push(other_local);

        let mut current_block = mir_fn.entry_block;
        for (idx, field) in s.fields.iter().enumerate() {
            let lhs = mir_fn.new_temp(field.ty.clone());
            mir_fn.blocks[current_block]
                .instructions
                .push(MirInst::GetField {
                    dest: lhs,
                    base: self_local,
                    field_index: idx,
                });
            let rhs = mir_fn.new_temp(field.ty.clone());
            mir_fn.blocks[current_block]
                .instructions
                .push(MirInst::GetField {
                    dest: rhs,
                    base: other_local,
                    field_index: idx,
                });

            if let Some(inner_name) =
                self.struct_with_derive_trait(&field.ty, if partial { "PartialOrd" } else { "Ord" })
            {
                let cmp = mir_fn.new_temp(Ty::Int);
                mir_fn.blocks[current_block]
                    .instructions
                    .push(MirInst::Call {
                        dest: Some(cmp),
                        callee: format!("{}_{}", inner_name, method_name),
                        args: vec![MirValue::Use(lhs), MirValue::Use(rhs)],
                    });
                let is_eq = mir_fn.new_temp(Ty::Bool);
                mir_fn.blocks[current_block]
                    .instructions
                    .push(MirInst::Compare {
                        dest: is_eq,
                        op: CmpOp::Eq,
                        lhs: MirValue::Use(cmp),
                        rhs: MirValue::Literal(Literal::Int(0)),
                    });
                let next_block = mir_fn.new_block();
                let diff_block = mir_fn.new_block();
                mir_fn.blocks[current_block].terminator = Terminator::Branch {
                    cond: MirValue::Use(is_eq),
                    then_block: next_block,
                    else_block: diff_block,
                };
                mir_fn.blocks[diff_block].terminator = Terminator::Return(Some(MirValue::Use(cmp)));
                current_block = next_block;
                continue;
            }

            if matches!(field.ty, Ty::String | Ty::Str) {
                let cmp = mir_fn.new_temp(Ty::Int);
                mir_fn.blocks[current_block]
                    .instructions
                    .push(MirInst::Call {
                        dest: Some(cmp),
                        callee: "riven_string_cmp".to_string(),
                        args: vec![MirValue::Use(lhs), MirValue::Use(rhs)],
                    });
                let is_eq = mir_fn.new_temp(Ty::Bool);
                mir_fn.blocks[current_block]
                    .instructions
                    .push(MirInst::Compare {
                        dest: is_eq,
                        op: CmpOp::Eq,
                        lhs: MirValue::Use(cmp),
                        rhs: MirValue::Literal(Literal::Int(0)),
                    });
                let next_block = mir_fn.new_block();
                let diff_block = mir_fn.new_block();
                mir_fn.blocks[current_block].terminator = Terminator::Branch {
                    cond: MirValue::Use(is_eq),
                    then_block: next_block,
                    else_block: diff_block,
                };
                mir_fn.blocks[diff_block].terminator = Terminator::Return(Some(MirValue::Use(cmp)));
                current_block = next_block;
                continue;
            }

            let lt = mir_fn.new_temp(Ty::Bool);
            mir_fn.blocks[current_block]
                .instructions
                .push(MirInst::Compare {
                    dest: lt,
                    op: CmpOp::Lt,
                    lhs: MirValue::Use(lhs),
                    rhs: MirValue::Use(rhs),
                });
            let lt_block = mir_fn.new_block();
            let ge_block = mir_fn.new_block();
            mir_fn.blocks[current_block].terminator = Terminator::Branch {
                cond: MirValue::Use(lt),
                then_block: lt_block,
                else_block: ge_block,
            };
            mir_fn.blocks[lt_block].terminator =
                Terminator::Return(Some(MirValue::Literal(Literal::Int(-1))));

            let gt = mir_fn.new_temp(Ty::Bool);
            mir_fn.blocks[ge_block].instructions.push(MirInst::Compare {
                dest: gt,
                op: CmpOp::Gt,
                lhs: MirValue::Use(lhs),
                rhs: MirValue::Use(rhs),
            });
            let gt_block = mir_fn.new_block();
            let next_block = mir_fn.new_block();
            mir_fn.blocks[ge_block].terminator = Terminator::Branch {
                cond: MirValue::Use(gt),
                then_block: gt_block,
                else_block: next_block,
            };
            mir_fn.blocks[gt_block].terminator =
                Terminator::Return(Some(MirValue::Literal(Literal::Int(1))));
            current_block = next_block;
        }

        mir_fn.blocks[current_block].terminator =
            Terminator::Return(Some(MirValue::Literal(Literal::Int(0))));
        mir_fn
    }

    /// Emit MIR that produces a deep clone of the value `src` of type
    /// `field_ty`, returning the local that holds the cloned value.
    /// Inserts instructions into `block`.
    ///
    /// Recipe:
    ///   * Copy types (primitives, references, function pointers,
    ///     `derive Copy` user types) → bitwise reuse of `src`.
    ///   * `String` / `Str`           → `riven_string_from(src)`.
    ///   * `Vec[_]`                   → `riven_vec_clone(src)`.
    ///   * `HashMap[_, _]`            → `riven_hash_clone(src)`.
    ///   * `Set[_]`                   → `riven_set_clone(src)`.
    ///   * Struct/Class/Enum that itself derives Clone → recursive
    ///     `<Type>_clone(src)`.
    ///   * Anything else falls back to a bitwise reuse — drop
    ///     elaboration in `implicit_includes/mod.rs::validate_clone_requirements`
    ///     ensures the fallback only triggers for types with E0610
    ///     already emitted, so the synthesised function still has a
    ///     compilable body for downstream codegen even though the
    ///     program will not link.
    fn synthesize_clone_field(
        &self,
        mir_fn: &mut MirFunction,
        block: BlockId,
        src: LocalId,
        field_ty: &Ty,
    ) -> LocalId {
        if ty_is_effectively_copy(field_ty, self.symbols) {
            return src;
        }
        if matches!(field_ty, Ty::String | Ty::Str) {
            let dest = mir_fn.new_temp(field_ty.clone());
            mir_fn.blocks[block].instructions.push(MirInst::Call {
                dest: Some(dest),
                callee: "riven_string_from".to_string(),
                args: vec![MirValue::Use(src)],
            });
            return dest;
        }
        if matches!(field_ty, Ty::Array(_)) {
            let dest = mir_fn.new_temp(field_ty.clone());
            mir_fn.blocks[block].instructions.push(MirInst::Call {
                dest: Some(dest),
                callee: "riven_vec_clone".to_string(),
                args: vec![MirValue::Use(src)],
            });
            return dest;
        }
        if matches!(field_ty, Ty::Map(_, _)) {
            let dest = mir_fn.new_temp(field_ty.clone());
            mir_fn.blocks[block].instructions.push(MirInst::Call {
                dest: Some(dest),
                callee: "riven_hash_clone".to_string(),
                args: vec![MirValue::Use(src)],
            });
            return dest;
        }
        if matches!(field_ty, Ty::Set(_)) {
            let dest = mir_fn.new_temp(field_ty.clone());
            mir_fn.blocks[block].instructions.push(MirInst::Call {
                dest: Some(dest),
                callee: "riven_set_clone".to_string(),
                args: vec![MirValue::Use(src)],
            });
            return dest;
        }
        if let Some(inner_name) = self.user_type_with_derive_clone(field_ty) {
            let dest = mir_fn.new_temp(field_ty.clone());
            mir_fn.blocks[block].instructions.push(MirInst::Call {
                dest: Some(dest),
                callee: format!("{}_clone", inner_name),
                args: vec![MirValue::Use(src)],
            });
            return dest;
        }
        // Fallback: bitwise reuse. The companion validator in
        // `implicit_includes/mod.rs` will already have surfaced E0610 for this
        // path, so the resulting MIR exists only to keep the rest of
        // codegen consistent during the same compilation unit.
        src
    }

    /// Look up a user-defined type by name and return the type name
    /// when the underlying definition (struct / class / enum) carries
    /// `derive Clone`.
    fn user_type_with_derive_clone(&self, ty: &Ty) -> Option<String> {
        use crate::resolve::symbols::DefKind;
        let name = match ty {
            Ty::Struct { name, .. } | Ty::Class { name, .. } | Ty::Enum { name, .. } => {
                name.clone()
            }
            Ty::Ref(inner)
            | Ty::RefMut(inner)
            | Ty::RefLifetime(_, inner)
            | Ty::RefMutLifetime(_, inner) => return self.user_type_with_derive_clone(inner),
            Ty::Alias { target, .. } => return self.user_type_with_derive_clone(target),
            Ty::Newtype { inner, .. } => return self.user_type_with_derive_clone(inner),
            _ => return None,
        };
        for def in self.symbols.iter() {
            if def.name != name {
                continue;
            }
            let derives = match &def.kind {
                DefKind::Struct { info } => &info.derive_traits,
                DefKind::Class { info } => &info.derive_traits,
                DefKind::Enum { info } => &info.derive_traits,
                _ => continue,
            };
            if derives.iter().any(|t| t == "Clone") {
                return Some(name);
            }
        }
        None
    }

    /// Synthesise `{StructName}_clone(self) -> StructName` for a
    /// struct that declares `derive Clone`. The body allocates a fresh
    /// instance, clones each field according to
    /// [`Self::synthesize_clone_field`], and returns the new value.
    fn synthesize_struct_clone(&self, s: &HirStructDef) -> MirFunction {
        let fn_name = format!("{}_clone", s.name);
        let self_ty = Ty::Struct {
            name: s.name.clone(),
            generic_args: vec![],
        };

        let mut mir_fn = MirFunction::new(&fn_name, self_ty.clone());
        let self_local = mir_fn.new_local("self", self_ty.clone(), false);
        mir_fn.params.push(self_local);
        let entry = mir_fn.entry_block;

        let dest = mir_fn.new_temp(self_ty.clone());
        mir_fn.blocks[entry].instructions.push(MirInst::Alloc {
            dest,
            ty: self_ty.clone(),
            size: self.alloc_size(&self_ty),
        });

        for (idx, field) in s.fields.iter().enumerate() {
            let field_local = mir_fn.new_temp(field.ty.clone());
            mir_fn.blocks[entry].instructions.push(MirInst::GetField {
                dest: field_local,
                base: self_local,
                field_index: idx,
            });
            let cloned = self.synthesize_clone_field(&mut mir_fn, entry, field_local, &field.ty);
            mir_fn.blocks[entry].instructions.push(MirInst::SetField {
                base: dest,
                field_index: idx,
                value: MirValue::Use(cloned),
            });
        }

        mir_fn.blocks[entry].terminator = Terminator::Return(Some(MirValue::Use(dest)));
        mir_fn
    }

    /// Synthesise `{ClassName}_clone(self) -> ClassName` for a class
    /// that declares `derive Clone`. Same body shape as the struct
    /// version; the storage layout (8-byte field slots) is identical
    /// at the MIR level.
    fn synthesize_class_clone(&self, c: &HirClassDef) -> MirFunction {
        let fn_name = format!("{}_clone", c.name);
        let self_ty = Ty::Class {
            name: c.name.clone(),
            generic_args: vec![],
        };

        let mut mir_fn = MirFunction::new(&fn_name, self_ty.clone());
        let self_local = mir_fn.new_local("self", self_ty.clone(), false);
        mir_fn.params.push(self_local);
        let entry = mir_fn.entry_block;

        let dest = mir_fn.new_temp(self_ty.clone());
        mir_fn.blocks[entry].instructions.push(MirInst::Alloc {
            dest,
            ty: self_ty.clone(),
            size: self.alloc_size(&self_ty),
        });

        for (idx, field) in c.fields.iter().enumerate() {
            let field_local = mir_fn.new_temp(field.ty.clone());
            mir_fn.blocks[entry].instructions.push(MirInst::GetField {
                dest: field_local,
                base: self_local,
                field_index: idx,
            });
            let cloned = self.synthesize_clone_field(&mut mir_fn, entry, field_local, &field.ty);
            mir_fn.blocks[entry].instructions.push(MirInst::SetField {
                base: dest,
                field_index: idx,
                value: MirValue::Use(cloned),
            });
        }

        mir_fn.blocks[entry].terminator = Terminator::Return(Some(MirValue::Use(dest)));
        mir_fn
    }

    /// Synthesise `{EnumName}_clone(self) -> EnumName` for an enum
    /// that declares `derive Clone`. Lowering is a switch on the
    /// discriminant: each variant allocates a new enum, copies the
    /// tag, clones every payload field, and goto's a shared join
    /// block that returns the cloned value.
    fn synthesize_enum_clone(&self, e: &HirEnumDef) -> MirFunction {
        let fn_name = format!("{}_clone", e.name);
        let self_ty = Ty::Enum {
            name: e.name.clone(),
            generic_args: vec![],
        };

        let mut mir_fn = MirFunction::new(&fn_name, self_ty.clone());
        let self_local = mir_fn.new_local("self", self_ty.clone(), false);
        mir_fn.params.push(self_local);

        let entry = mir_fn.entry_block;
        let result = mir_fn.new_temp(self_ty.clone());
        mir_fn.blocks[entry].instructions.push(MirInst::Alloc {
            dest: result,
            ty: self_ty.clone(),
            size: self.alloc_size(&self_ty),
        });

        let tag = mir_fn.new_temp(Ty::Int32);
        mir_fn.blocks[entry].instructions.push(MirInst::GetTag {
            dest: tag,
            src: self_local,
        });

        // One block per variant + a shared join block that holds the
        // single Return terminator. Variant 0 doubles as the Switch's
        // `otherwise` target so a malformed tag still reaches a real
        // arm rather than falling off the end.
        let join = mir_fn.new_block();
        let mut targets: Vec<(i64, BlockId)> = Vec::with_capacity(e.variants.len());
        let mut variant_blocks: Vec<BlockId> = Vec::with_capacity(e.variants.len());
        for _ in &e.variants {
            variant_blocks.push(mir_fn.new_block());
        }
        for (i, variant) in e.variants.iter().enumerate() {
            targets.push((variant.index as i64, variant_blocks[i]));
        }
        let otherwise = variant_blocks.first().copied().unwrap_or(join);

        mir_fn.blocks[entry].terminator = Terminator::Switch {
            value: MirValue::Use(tag),
            targets,
            otherwise,
        };

        for (i, variant) in e.variants.iter().enumerate() {
            let block = variant_blocks[i];
            mir_fn.blocks[block].instructions.push(MirInst::SetTag {
                dest: result,
                tag: variant.index as u32,
            });

            let payload_fields: &[HirVariantField] = match &variant.kind {
                HirVariantKind::Unit => &[],
                HirVariantKind::Tuple(fields) | HirVariantKind::Struct(fields) => fields,
            };

            if !payload_fields.is_empty() {
                let self_payload = mir_fn.new_temp(self_ty.clone());
                mir_fn.blocks[block].instructions.push(MirInst::GetPayload {
                    dest: self_payload,
                    src: self_local,
                    ty: self_ty.clone(),
                });
                let dest_payload = mir_fn.new_temp(self_ty.clone());
                mir_fn.blocks[block].instructions.push(MirInst::GetPayload {
                    dest: dest_payload,
                    src: result,
                    ty: self_ty.clone(),
                });
                for (idx, field) in payload_fields.iter().enumerate() {
                    let read = mir_fn.new_temp(field.ty.clone());
                    mir_fn.blocks[block].instructions.push(MirInst::GetField {
                        dest: read,
                        base: self_payload,
                        field_index: idx,
                    });
                    let cloned = self.synthesize_clone_field(&mut mir_fn, block, read, &field.ty);
                    mir_fn.blocks[block].instructions.push(MirInst::SetField {
                        base: dest_payload,
                        field_index: idx,
                        value: MirValue::Use(cloned),
                    });
                }
            }

            mir_fn.blocks[block].terminator = Terminator::Goto(join);
        }

        mir_fn.blocks[join].terminator = Terminator::Return(Some(MirValue::Use(result)));
        mir_fn
    }

    /// Phase 2 #06.C2: synthesize `{EnumName}_to_debug(self) -> String`
    /// for an enum that declares `derive Debug`. Output shape mirrors
    /// Rust's `Debug`:
    ///
    /// * `Unit` variants  → `"Variant"`
    /// * `Tuple(a, b)`    → `"Variant(<a>, <b>)"`
    /// * `Struct{x, y}`   → `"Variant { x: <x>, y: <y> }"`
    ///
    /// Field formatting mirrors `synthesize_struct_to_debug`: primitives
    /// use the `riven_*_to_string` runtime helpers, nested structs with
    /// `derive Debug` recurse, anything else renders as `<...>` so the
    /// formatter never panics.
    fn synthesize_enum_to_debug(&self, e: &HirEnumDef) -> MirFunction {
        let fn_name = format!("{}_to_debug", e.name);
        let self_ty = Ty::Enum {
            name: e.name.clone(),
            generic_args: vec![],
        };

        let mut mir_fn = MirFunction::new(&fn_name, Ty::String);
        let self_local = mir_fn.new_local("self", self_ty.clone(), false);
        mir_fn.params.push(self_local);

        let entry = mir_fn.entry_block;
        let tag = mir_fn.new_temp(Ty::Int32);
        mir_fn.blocks[entry].instructions.push(MirInst::GetTag {
            dest: tag,
            src: self_local,
        });

        // One block per variant. Each block builds the variant's
        // debug string and terminates with its own `Return`, so we
        // don't need a join block. Variant 0 doubles as the Switch's
        // `otherwise` target to mirror `synthesize_enum_clone`.
        let mut variant_blocks: Vec<BlockId> = Vec::with_capacity(e.variants.len());
        for _ in &e.variants {
            variant_blocks.push(mir_fn.new_block());
        }
        let targets: Vec<(i64, BlockId)> = e
            .variants
            .iter()
            .enumerate()
            .map(|(i, v)| (v.index as i64, variant_blocks[i]))
            .collect();
        let otherwise = variant_blocks.first().copied().unwrap_or(entry);

        mir_fn.blocks[entry].terminator = Terminator::Switch {
            value: MirValue::Use(tag),
            targets,
            otherwise,
        };

        for (i, variant) in e.variants.iter().enumerate() {
            let block = variant_blocks[i];

            // Start with the variant name.
            let mut acc = mir_fn.new_temp(Ty::String);
            mir_fn.blocks[block]
                .instructions
                .push(MirInst::StringLiteral {
                    dest: acc,
                    value: variant.name.clone(),
                });

            let payload_fields: &[HirVariantField] = match &variant.kind {
                HirVariantKind::Unit => &[],
                HirVariantKind::Tuple(fields) | HirVariantKind::Struct(fields) => fields,
            };

            if !payload_fields.is_empty() {
                let is_struct_variant = matches!(variant.kind, HirVariantKind::Struct(_));
                let open = if is_struct_variant { " { " } else { "(" };
                let close = if is_struct_variant { " }" } else { ")" };

                acc = self.concat_string_literal(&mut mir_fn, block, acc, open);

                let payload = mir_fn.new_temp(self_ty.clone());
                mir_fn.blocks[block].instructions.push(MirInst::GetPayload {
                    dest: payload,
                    src: self_local,
                    ty: self_ty.clone(),
                });

                for (idx, field) in payload_fields.iter().enumerate() {
                    if idx > 0 {
                        acc = self.concat_string_literal(&mut mir_fn, block, acc, ", ");
                    }
                    if is_struct_variant {
                        if let Some(name) = &field.name {
                            let label = format!("{}: ", name);
                            acc = self.concat_string_literal(&mut mir_fn, block, acc, &label);
                        }
                    }

                    let field_local = mir_fn.new_temp(field.ty.clone());
                    mir_fn.blocks[block].instructions.push(MirInst::GetField {
                        dest: field_local,
                        base: payload,
                        field_index: idx,
                    });

                    let field_str =
                        self.format_field_for_debug(&mut mir_fn, block, field_local, &field.ty);

                    let next = mir_fn.new_temp(Ty::String);
                    mir_fn.blocks[block].instructions.push(MirInst::Call {
                        dest: Some(next),
                        callee: "riven_string_concat".to_string(),
                        args: vec![MirValue::Use(acc), MirValue::Use(field_str)],
                    });
                    acc = next;
                }

                acc = self.concat_string_literal(&mut mir_fn, block, acc, close);
            }

            mir_fn.blocks[block].terminator = Terminator::Return(Some(MirValue::Use(acc)));
        }

        mir_fn
    }

    /// Append a literal `&str` to a String accumulator. Returns the
    /// new accumulator local. Helper for `synthesize_enum_to_debug`.
    fn concat_string_literal(
        &self,
        mir_fn: &mut MirFunction,
        block: BlockId,
        acc: LocalId,
        text: &str,
    ) -> LocalId {
        let lit = mir_fn.new_temp(Ty::String);
        mir_fn.blocks[block]
            .instructions
            .push(MirInst::StringLiteral {
                dest: lit,
                value: text.to_string(),
            });
        let next = mir_fn.new_temp(Ty::String);
        mir_fn.blocks[block].instructions.push(MirInst::Call {
            dest: Some(next),
            callee: "riven_string_concat".to_string(),
            args: vec![MirValue::Use(acc), MirValue::Use(lit)],
        });
        next
    }

    /// Format a single field value for `_to_debug` output. Mirrors
    /// the per-field branch in `synthesize_struct_to_debug`. Phase D
    /// will replace this with a canonical `Display::fmt` dispatch.
    fn format_field_for_debug(
        &self,
        mir_fn: &mut MirFunction,
        block: BlockId,
        field_local: LocalId,
        field_ty: &Ty,
    ) -> LocalId {
        if *field_ty == Ty::Char {
            let dest = mir_fn.new_temp(Ty::String);
            mir_fn.blocks[block].instructions.push(MirInst::Call {
                dest: Some(dest),
                callee: "riven_char_to_string".to_string(),
                args: vec![MirValue::Use(field_local)],
            });
            return dest;
        }
        if field_ty.is_integer() {
            let dest = mir_fn.new_temp(Ty::String);
            mir_fn.blocks[block].instructions.push(MirInst::Call {
                dest: Some(dest),
                callee: "riven_int_to_string".to_string(),
                args: vec![MirValue::Use(field_local)],
            });
            return dest;
        }
        if field_ty.is_float() {
            let dest = mir_fn.new_temp(Ty::String);
            mir_fn.blocks[block].instructions.push(MirInst::Call {
                dest: Some(dest),
                callee: "riven_float_to_string".to_string(),
                args: vec![MirValue::Use(field_local)],
            });
            return dest;
        }
        if *field_ty == Ty::Bool {
            let dest = mir_fn.new_temp(Ty::String);
            mir_fn.blocks[block].instructions.push(MirInst::Call {
                dest: Some(dest),
                callee: "riven_bool_to_string".to_string(),
                args: vec![MirValue::Use(field_local)],
            });
            return dest;
        }
        if matches!(field_ty, Ty::String | Ty::Str) {
            return field_local;
        }
        if let Some(inner_struct_name) = self.struct_with_derive_debug(field_ty) {
            let dest = mir_fn.new_temp(Ty::String);
            mir_fn.blocks[block].instructions.push(MirInst::Call {
                dest: Some(dest),
                callee: format!("{}_to_debug", inner_struct_name),
                args: vec![MirValue::Use(field_local)],
            });
            return dest;
        }
        if let Some(inner_enum_name) = self.enum_with_derive_debug(field_ty) {
            let dest = mir_fn.new_temp(Ty::String);
            mir_fn.blocks[block].instructions.push(MirInst::Call {
                dest: Some(dest),
                callee: format!("{}_to_debug", inner_enum_name),
                args: vec![MirValue::Use(field_local)],
            });
            return dest;
        }
        let dest = mir_fn.new_temp(Ty::String);
        mir_fn.blocks[block]
            .instructions
            .push(MirInst::StringLiteral {
                dest,
                value: "<...>".to_string(),
            });
        dest
    }

    fn synthesize_default_value(
        &self,
        mir_fn: &mut MirFunction,
        block: BlockId,
        ty: &Ty,
    ) -> LocalId {
        if ty.is_integer() {
            let dest = mir_fn.new_temp(ty.clone());
            mir_fn.blocks[block].instructions.push(MirInst::Assign {
                dest,
                value: MirValue::Literal(Literal::Int(0)),
            });
            return dest;
        }
        if ty.is_float() {
            let dest = mir_fn.new_temp(ty.clone());
            mir_fn.blocks[block].instructions.push(MirInst::Assign {
                dest,
                value: MirValue::Literal(Literal::Float(0.0)),
            });
            return dest;
        }
        if *ty == Ty::Bool {
            let dest = mir_fn.new_temp(Ty::Bool);
            mir_fn.blocks[block].instructions.push(MirInst::Assign {
                dest,
                value: MirValue::Literal(Literal::Bool(false)),
            });
            return dest;
        }
        if *ty == Ty::Char {
            let dest = mir_fn.new_temp(Ty::Char);
            mir_fn.blocks[block].instructions.push(MirInst::Assign {
                dest,
                value: MirValue::Literal(Literal::Char('\0')),
            });
            return dest;
        }
        if matches!(ty, Ty::String) {
            let raw = mir_fn.new_temp(Ty::Str);
            mir_fn.blocks[block]
                .instructions
                .push(MirInst::StringLiteral {
                    dest: raw,
                    value: String::new(),
                });
            let dest = mir_fn.new_temp(Ty::String);
            mir_fn.blocks[block].instructions.push(MirInst::Call {
                dest: Some(dest),
                callee: "riven_string_from".to_string(),
                args: vec![MirValue::Use(raw)],
            });
            return dest;
        }
        if matches!(ty, Ty::Str) {
            let dest = mir_fn.new_temp(Ty::Str);
            mir_fn.blocks[block]
                .instructions
                .push(MirInst::StringLiteral {
                    dest,
                    value: String::new(),
                });
            return dest;
        }
        if let Some(inner_name) = self.struct_with_derive_trait(ty, "Default") {
            let dest = mir_fn.new_temp(ty.clone());
            mir_fn.blocks[block].instructions.push(MirInst::Call {
                dest: Some(dest),
                callee: format!("{}_default", inner_name),
                args: vec![],
            });
            return dest;
        }
        if matches!(ty, Ty::Array(_) | Ty::Map(_, _) | Ty::Set(_)) {
            let dest = mir_fn.new_temp(ty.clone());
            let type_name = type_name_from_ty(ty);
            let base = type_name.split('[').next().unwrap_or(type_name.as_str());
            mir_fn.blocks[block].instructions.push(MirInst::Call {
                dest: Some(dest),
                callee: format!("{}_new", base),
                args: vec![],
            });
            return dest;
        }
        if let Ty::Option(_) = ty {
            let dest = mir_fn.new_temp(ty.clone());
            mir_fn.blocks[block].instructions.push(MirInst::Alloc {
                dest,
                ty: ty.clone(),
                size: self.alloc_size(ty),
            });
            mir_fn.blocks[block]
                .instructions
                .push(MirInst::SetTag { dest, tag: 0 });
            return dest;
        }

        let dest = mir_fn.new_temp(ty.clone());
        mir_fn.blocks[block].instructions.push(MirInst::Assign {
            dest,
            value: MirValue::Literal(Literal::Int(0)),
        });
        dest
    }

    /// Lower `lhs == rhs` (or `lhs != rhs`) for a struct that derives
    /// `PartialEq`. Compares each field of the two structs in turn and
    /// returns the AND of all field equalities (for `Eq`) or its negation
    /// (for `NotEq`). Both sides must already have the struct shape
    /// described by `fields` (`(index, field_ty)` pairs).
    fn lower_struct_partial_eq(
        &mut self,
        lhs: &HirExpr,
        rhs: &HirExpr,
        op: BinOp,
        fields: &[(usize, Ty)],
    ) -> Result<LocalId, String> {
        let lhs_local = self
            .lower_expr(lhs)?
            .ok_or_else(|| "lhs of struct == has no value".to_string())?;
        let rhs_local = self
            .lower_expr(rhs)?
            .ok_or_else(|| "rhs of struct == has no value".to_string())?;

        if fields.is_empty() {
            let dest = self.new_temp(Ty::Bool);
            self.emit(MirInst::Assign {
                dest,
                value: MirValue::Literal(Literal::Bool(matches!(op, BinOp::Eq))),
            });
            return Ok(dest);
        }

        let mut acc: Option<LocalId> = None;
        for (idx, field_ty) in fields {
            let lf = self.new_temp(field_ty.clone());
            self.emit(MirInst::GetField {
                dest: lf,
                base: lhs_local,
                field_index: *idx,
            });
            let rf = self.new_temp(field_ty.clone());
            self.emit(MirInst::GetField {
                dest: rf,
                base: rhs_local,
                field_index: *idx,
            });

            let field_eq = self.new_temp(Ty::Bool);
            self.emit(MirInst::Compare {
                dest: field_eq,
                op: CmpOp::Eq,
                lhs: MirValue::Use(lf),
                rhs: MirValue::Use(rf),
            });

            acc = Some(match acc {
                None => field_eq,
                Some(prev) => {
                    let combined = self.new_temp(Ty::Bool);
                    self.emit(MirInst::BinOp {
                        dest: combined,
                        op: BinOp::And,
                        lhs: MirValue::Use(prev),
                        rhs: MirValue::Use(field_eq),
                    });
                    combined
                }
            });
        }

        let eq_result = acc.expect("non-empty fields handled above");
        if matches!(op, BinOp::NotEq) {
            let negated = self.new_temp(Ty::Bool);
            self.emit(MirInst::Not {
                dest: negated,
                operand: MirValue::Use(eq_result),
            });
            Ok(negated)
        } else {
            Ok(eq_result)
        }
    }

    /// Lower `<` / `<=` / `>` / `>=` on a struct that derives `Ord`
    /// (or `PartialOrd`) by calling the synthesised
    /// `<Type>_cmp` / `<Type>_partial_cmp`, then comparing its
    /// `-1 / 0 / +1` result to `0` according to `op`.
    fn lower_struct_ord(
        &mut self,
        lhs: &HirExpr,
        rhs: &HirExpr,
        op: BinOp,
        struct_name: &str,
        partial: bool,
    ) -> Result<LocalId, String> {
        let lhs_local = self
            .lower_expr(lhs)?
            .ok_or_else(|| "lhs of struct ordering has no value".to_string())?;
        let rhs_local = self
            .lower_expr(rhs)?
            .ok_or_else(|| "rhs of struct ordering has no value".to_string())?;

        let method_name = if partial { "partial_cmp" } else { "cmp" };
        let cmp = self.new_temp(Ty::Int);
        self.emit(MirInst::Call {
            dest: Some(cmp),
            callee: format!("{}_{}", struct_name, method_name),
            args: vec![MirValue::Use(lhs_local), MirValue::Use(rhs_local)],
        });

        let cmp_op = binop_to_cmpop(op);
        let dest = self.new_temp(Ty::Bool);
        self.emit(MirInst::Compare {
            dest,
            op: cmp_op,
            lhs: MirValue::Use(cmp),
            rhs: MirValue::Literal(Literal::Int(0)),
        });
        Ok(dest)
    }

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

    /// Returns Some(struct_name) if `ty` (peeling refs/aliases) is a struct
    /// whose declaration includes `derive Debug`; otherwise None.
    fn struct_with_derive_debug(&self, ty: &Ty) -> Option<String> {
        self.struct_with_derive_trait(ty, "Debug")
    }

    /// Phase 2 #06.C2: like `struct_with_derive_debug` but for enums.
    /// Returns the enum's name when `ty` resolves (through refs/
    /// aliases/newtypes) to an enum whose `derive_traits` contains
    /// `Debug`.
    fn enum_with_derive_debug(&self, ty: &Ty) -> Option<String> {
        self.enum_with_derive_trait(ty, "Debug")
    }

    fn enum_with_derive_trait(&self, ty: &Ty, trait_name: &str) -> Option<String> {
        use crate::resolve::symbols::DefKind;
        let name = match ty {
            Ty::Enum { name, .. } => name.clone(),
            Ty::Ref(inner)
            | Ty::RefMut(inner)
            | Ty::RefLifetime(_, inner)
            | Ty::RefMutLifetime(_, inner) => {
                return self.enum_with_derive_trait(inner, trait_name)
            }
            Ty::Alias { target, .. } => return self.enum_with_derive_trait(target, trait_name),
            Ty::Newtype { inner, .. } => return self.enum_with_derive_trait(inner, trait_name),
            _ => return None,
        };
        for def in self.symbols.iter() {
            if def.name == name {
                if let DefKind::Enum { info } = &def.kind {
                    if info.derive_traits.iter().any(|t| t == trait_name) {
                        return Some(name);
                    }
                }
            }
        }
        None
    }

    fn receiver_type_name(&self, expr: &HirExpr) -> Option<String> {
        use crate::resolve::symbols::DefKind;

        let HirExprKind::VarRef(def_id) = expr.kind else {
            return None;
        };
        let def = self.symbols.get(def_id)?;
        match &def.kind {
            DefKind::Class { .. } | DefKind::Struct { .. } | DefKind::Enum { .. } => {
                Some(def.name.clone())
            }
            DefKind::TypeAlias { target } => Some(type_name_from_ty(target)),
            _ => None,
        }
    }

    fn type_supports_trait(&self, ty: &Ty, trait_name: &str) -> bool {
        if self.struct_with_derive_trait(ty, trait_name).is_some() {
            return true;
        }
        match ty {
            Ty::TypeParam { bounds, .. } | Ty::SomeMixin(bounds) | Ty::AnyMixin(bounds) => {
                bounds.iter().any(|bound| bound.name == trait_name)
            }
            Ty::Ref(inner)
            | Ty::RefMut(inner)
            | Ty::RefLifetime(_, inner)
            | Ty::RefMutLifetime(_, inner) => self.type_supports_trait(inner, trait_name),
            Ty::Alias { target, .. } => self.type_supports_trait(target, trait_name),
            Ty::Newtype { inner, .. } => self.type_supports_trait(inner, trait_name),
            _ => false,
        }
    }

    fn struct_with_derive_trait(&self, ty: &Ty, trait_name: &str) -> Option<String> {
        // Peel reference/alias/newtype layers, then consult
        // `ty_has_derive_trait` so implicit-include structural mixins
        // (spec §3.6) are honoured alongside explicit `derive_traits`.
        let mut cur = ty;
        loop {
            match cur {
                Ty::Ref(inner)
                | Ty::RefMut(inner)
                | Ty::RefLifetime(_, inner)
                | Ty::RefMutLifetime(_, inner) => cur = inner,
                Ty::Alias { target, .. } => cur = target,
                Ty::Newtype { inner, .. } => cur = inner,
                Ty::Struct { name, .. } => {
                    if crate::resolve::symbols::ty_has_derive_trait(
                        cur,
                        self.symbols,
                        trait_name,
                    ) {
                        return Some(name.clone());
                    }
                    return None;
                }
                _ => return None,
            }
        }
    }

    /// Set the terminator of the current basic block.
    fn set_terminator(&mut self, term: Terminator) {
        let block_id = self.current_block;
        let func = self.current_fn.as_mut().expect("no current function");
        func.blocks[block_id].terminator = term;
    }

    /// Read the terminator of the current basic block.
    fn get_terminator(&self) -> &Terminator {
        let block_id = self.current_block;
        let func = self.current_fn.as_ref().expect("no current function");
        &func.blocks[block_id].terminator
    }

    /// Create a new basic block in the current function.
    fn new_block(&mut self) -> BlockId {
        self.current_fn
            .as_mut()
            .expect("no current function")
            .new_block()
    }

    /// Create a new temporary local.
    fn new_temp(&mut self, ty: Ty) -> LocalId {
        self.current_fn
            .as_mut()
            .expect("no current function")
            .new_temp(ty)
    }

    /// Compute the allocation size for a type using the layout system.
    ///
    /// Classes and structs are stored field-by-field using fixed 8-byte
    /// slots (see cranelift.rs `SetField`/`GetField`), so a struct of
    /// N declared fields needs at least `N * 8` bytes regardless of the
    /// C layout size — a 3xUInt8 struct has layout.size == 3 but we still
    /// write UInt8s at offsets 0, 8, 16 when setting its fields.
    fn alloc_size(&self, ty: &Ty) -> usize {
        use crate::resolve::symbols::DefKind;
        let layout = crate::codegen::layout::layout_of(ty, self.symbols);
        let base = layout.size.max(8);
        if let Ty::Class { name, .. } | Ty::Struct { name, .. } = ty {
            let mut total_fields = 0usize;
            let mut cur = Some(name.clone());
            while let Some(n) = cur.take() {
                for def in self.symbols.iter() {
                    if def.name == n {
                        match &def.kind {
                            DefKind::Class { info } => {
                                total_fields += info.fields.len();
                                cur = info
                                    .parent
                                    .and_then(|p| self.symbols.get(p).map(|d| d.name.clone()));
                                break;
                            }
                            DefKind::Struct { info } => {
                                total_fields += info.fields.len();
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }
            return base.max(total_fields * 8).max(8);
        }
        base
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
    fn lower_or_pattern(
        &mut self,
        scrut_local: Option<LocalId>,
        _scrut_ty: &Ty,
        patterns: &[HirPattern],
        match_target: BlockId,
        next_block: BlockId,
    ) -> Result<(), String> {
        let scrut = match scrut_local {
            Some(s) => s,
            None => {
                self.set_terminator(Terminator::Goto(match_target));
                return Ok(());
            }
        };
        for (i, pat) in patterns.iter().enumerate() {
            let is_last = i + 1 == patterns.len();
            let fail_block = if is_last {
                next_block
            } else {
                self.new_block()
            };
            match pat {
                HirPattern::Wildcard { .. } => {
                    self.set_terminator(Terminator::Goto(match_target));
                    return Ok(());
                }
                HirPattern::Literal { expr: pat_expr, .. } => {
                    let lit_local = self.lower_expr(pat_expr)?;
                    let cmp_dest = self.new_temp(Ty::Bool);
                    self.emit(MirInst::Compare {
                        dest: cmp_dest,
                        op: CmpOp::Eq,
                        lhs: MirValue::Use(scrut),
                        rhs: local_to_value(lit_local),
                    });
                    self.set_terminator(Terminator::Branch {
                        cond: MirValue::Use(cmp_dest),
                        then_block: match_target,
                        else_block: fail_block,
                    });
                }
                _ => {
                    self.set_terminator(Terminator::Goto(match_target));
                    return Ok(());
                }
            }
            if !is_last {
                self.current_block = fail_block;
            }
        }
        Ok(())
    }

    /// Lower a tuple pattern by comparing literal elements and binding
    /// non-literal elements to the corresponding tuple field.
    fn lower_tuple_pattern(
        &mut self,
        scrut: LocalId,
        scrut_ty: &Ty,
        elements: &[HirPattern],
        match_target: BlockId,
        next_block: BlockId,
    ) -> Result<(), String> {
        let elem_tys: Vec<Ty> = match scrut_ty {
            Ty::Tuple(ts) => ts.clone(),
            _ => {
                self.set_terminator(Terminator::Goto(match_target));
                return Ok(());
            }
        };
        for (idx, pat) in elements.iter().enumerate() {
            let elem_ty = elem_tys.get(idx).cloned().unwrap_or(Ty::Unit);
            let elem_local = self.new_temp(elem_ty.clone());
            self.emit(MirInst::GetField {
                dest: elem_local,
                base: scrut,
                field_index: idx,
            });
            match pat {
                HirPattern::Wildcard { .. } => {}
                HirPattern::Binding {
                    def_id,
                    name,
                    mutable,
                    ..
                } => {
                    let local = self.new_local_named(name, elem_ty, *mutable);
                    self.def_to_local.insert(*def_id, local);
                    self.emit(MirInst::Assign {
                        dest: local,
                        value: MirValue::Use(elem_local),
                    });
                }
                HirPattern::Literal { expr: pat_expr, .. } => {
                    let lit_local = self.lower_expr(pat_expr)?;
                    let cmp_dest = self.new_temp(Ty::Bool);
                    self.emit(MirInst::Compare {
                        dest: cmp_dest,
                        op: CmpOp::Eq,
                        lhs: MirValue::Use(elem_local),
                        rhs: local_to_value(lit_local),
                    });
                    let ok_block = self.new_block();
                    self.set_terminator(Terminator::Branch {
                        cond: MirValue::Use(cmp_dest),
                        then_block: ok_block,
                        else_block: next_block,
                    });
                    self.current_block = ok_block;
                }
                _ => {
                    // Unsupported nested patterns: fall through to match.
                }
            }
        }
        self.set_terminator(Terminator::Goto(match_target));
        Ok(())
    }

    /// Get the ordered list of field names for a class.
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

// ─── Free utility functions ─────────────────────────────────────────────────

/// Check if a type is an Option type (including via references and inferred types).
fn is_option_type(ty: &Ty) -> bool {
    match ty {
        Ty::Option(_) => true,
        Ty::Ref(inner)
        | Ty::RefMut(inner)
        | Ty::RefLifetime(_, inner)
        | Ty::RefMutLifetime(_, inner) => is_option_type(inner),
        Ty::Class { name, .. } => name.starts_with("Option"),
        _ => false,
    }
}

fn is_result_type(ty: &Ty) -> bool {
    match ty {
        Ty::Result(_, _) => true,
        Ty::Ref(inner)
        | Ty::RefMut(inner)
        | Ty::RefLifetime(_, inner)
        | Ty::RefMutLifetime(_, inner) => is_result_type(inner),
        Ty::Class { name, .. } => name.starts_with("Result"),
        _ => false,
    }
}

/// Check if a method name is a known collection operation that takes a closure
/// and can be inlined by accessing the class's underlying Vec (first field).
fn is_collection_method(method_name: &str) -> bool {
    matches!(
        method_name,
        "each"
            | "filter"
            | "where_matching"
            | "find"
            | "position"
            | "map"
            | "partition"
            | "into_filtered"
            | "display_all"
    )
}

/// Check if a type is a Vec, iterator, or similar collection type
/// that supports closure inlining (as opposed to a user-defined class
/// like Repository or TaskList).
fn is_vec_or_iterator_type(ty: &Ty) -> bool {
    match ty {
        Ty::Array(_) => true,
        Ty::Ref(inner)
        | Ty::RefMut(inner)
        | Ty::RefLifetime(_, inner)
        | Ty::RefMutLifetime(_, inner) => is_vec_or_iterator_type(inner),
        Ty::Class { name, .. } => {
            let base = if let Some(pos) = name.find('[') {
                &name[..pos]
            } else {
                name.as_str()
            };
            matches!(
                base,
                "Vec" | "VecIter" | "VecIntoIter" | "SplitIter" | "HashIter" | "SetIter"
            )
        }
        // For inferred types, check if the type name suggests a collection.
        Ty::Infer(_) => false,
        _ => false,
    }
}

/// Check if a method on a built-in type is a static/class method
/// (no `self` argument). These are methods like `String.from(...)`,
/// `Vec.new()`, etc. that are called on the type itself.
fn is_builtin_static_method(type_name: &str, method_name: &str) -> bool {
    // Handle both exact matches and generic type names (e.g., "Vec[T]").
    let base_type = if let Some(pos) = type_name.find('[') {
        &type_name[..pos]
    } else {
        type_name
    };
    match base_type {
        "String" => matches!(method_name, "from" | "new" | "with_capacity" | "from_iter"),
        // `Vec.with_capacity(n)` is a stateless static constructor — like
        // `Vec.new` but takes one Int arg. Phase 2 stdlib batch 1 (#03).
        // `Vec.from_iter(iter)` (#03 batch 2) takes any iterator-producing
        // expression and treats it as a fresh allocation.
        "Vec" => matches!(method_name, "new" | "with_capacity" | "from_iter"),
        // Phase 2 stdlib (#04): full HashMap[K,V] / HashSet[T] surface.
        // The `Hash` and `HashMap` aliases both reach here for the
        // `HashMap.new` / `HashMap.with_capacity(n)` constructors; same
        // for `Set` / `HashSet`.
        "Hash" | "HashMap" => matches!(method_name, "new" | "with_capacity" | "from_iter"),
        "Set" | "HashSet" => matches!(method_name, "new" | "with_capacity" | "from_iter"),
        "Thread" => matches!(method_name, "spawn" | "current" | "sleep" | "yield_now"),
        "Mutex" => matches!(method_name, "new"),
        "Arc" => matches!(method_name, "new"),
        _ => false,
    }
}

/// Extract the element type from a collection or iterator type.
///
/// For `Vec[T]`, returns `T`. For iterator wrappers like `VecIter[T]`,
/// `VecIntoIter[T]`, returns `T`. For references to collections, unwraps
/// the reference first. Falls back to `Ty::Int` for unrecognized types.
fn element_type_of(ty: &Ty) -> Ty {
    match ty {
        Ty::Array(inner) => *inner.clone(),
        Ty::Ref(inner)
        | Ty::RefMut(inner)
        | Ty::RefLifetime(_, inner)
        | Ty::RefMutLifetime(_, inner) => element_type_of(inner),
        Ty::Class { name, generic_args } => {
            // Iterator wrapper types: VecIter[T], VecIntoIter[T], etc.
            if (name == "VecIter" || name == "VecIntoIter" || name == "SplitIter")
                && !generic_args.is_empty()
            {
                return generic_args[0].clone();
            }
            // Fall back to I64 (pointer-sized, covers most cases).
            Ty::Int
        }
        _ => Ty::Int,
    }
}

/// Phase 2 #06.D4: encode a `FormatSpec` into the four i64 arguments
/// accepted by `riven_fmt_formatter_new_with_spec`.
///
/// * `width`:     0  = unset
/// * `precision`: -1 = unset
/// * `align`:     0  = default, 1 = left ('<'), 2 = center ('^'), 3 = right ('>')
/// * `fill`:      -1 = unset (runtime treats as ' ')
fn encode_format_spec(spec: &crate::lexer::token::FormatSpec) -> (i64, i64, i64, i64) {
    let width = spec.width.map(|w| w as i64).unwrap_or(0);
    let precision = spec.precision.map(|p| p as i64).unwrap_or(-1);
    let align = match spec.align {
        Some('<') => 1,
        Some('^') => 2,
        Some('>') => 3,
        _ => 0,
    };
    let fill = spec.fill.map(|c| c as i64).unwrap_or(-1);
    (width, precision, align, fill)
}

/// Returns true if the type is a string-like type whose runtime representation
/// is already a `char*` and needs no conversion for string interpolation.
fn is_string_like(ty: &Ty) -> bool {
    match ty {
        Ty::String | Ty::Str => true,
        Ty::Ref(inner)
        | Ty::RefMut(inner)
        | Ty::RefLifetime(_, inner)
        | Ty::RefMutLifetime(_, inner) => is_string_like(inner),
        _ => false,
    }
}

/// Returns true if the expression's type is unresolved but the expression
/// is a method call that likely returns a string at runtime. This handles
/// cases where type inference left Infer(...) types unresolved for methods
/// like `to_display`, `message`, `summary`, `clone` on string types, etc.
fn is_inferred_string_expr(expr: &HirExpr) -> bool {
    if !matches!(expr.ty, Ty::Infer(_)) {
        return false;
    }
    // Known string-returning method names.
    let string_methods = [
        "to_display",
        "to_string",
        "message",
        "summary",
        "serialize",
        "clone",
        "title_ref",
        "deadline_ref",
        "to_lower",
        "trim",
        "push_str",
        "unwrap_or",
        "unwrap_or_else",
    ];

    match &expr.kind {
        HirExprKind::MethodCall { method_name, .. } => {
            string_methods.contains(&method_name.as_str())
        }
        // FieldAccess can also be a no-arg method call.
        HirExprKind::FieldAccess { field_name, .. } => {
            string_methods.contains(&field_name.as_str())
        }
        _ => false,
    }
}

/// Extract a user-visible type name from a `Ty` for method mangling.
pub fn type_name_from_ty(ty: &Ty) -> String {
    match ty {
        Ty::Class { name, .. } => name.clone(),
        Ty::Struct { name, .. } => name.clone(),
        Ty::Enum { name, .. } => name.clone(),
        Ty::Ref(inner) | Ty::RefMut(inner) => type_name_from_ty(inner),
        Ty::RefLifetime(_, inner) | Ty::RefMutLifetime(_, inner) => type_name_from_ty(inner),
        other => other.type_name(),
    }
}

/// Get the name of a definition from the symbol table.
pub fn def_id_name(def_id: DefId, symbols: &SymbolTable) -> String {
    symbols
        .get(def_id)
        .map(|d| d.name.clone())
        .unwrap_or_else(|| format!("_unknown_{}", def_id))
}

/// Convert an `Option<LocalId>` to a `MirValue`. If None, returns `MirValue::Unit`.
fn local_to_value(local: Option<LocalId>) -> MirValue {
    match local {
        Some(id) => MirValue::Use(id),
        None => MirValue::Unit,
    }
}

/// Check if a BinOp is a comparison operator.
fn is_comparison(op: BinOp) -> bool {
    matches!(
        op,
        BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::Gt | BinOp::LtEq | BinOp::GtEq
    )
}

/// Convert a comparison BinOp to the corresponding CmpOp.
fn binop_to_cmpop(op: BinOp) -> CmpOp {
    match op {
        BinOp::Eq => CmpOp::Eq,
        BinOp::NotEq => CmpOp::NotEq,
        BinOp::Lt => CmpOp::Lt,
        BinOp::Gt => CmpOp::Gt,
        BinOp::LtEq => CmpOp::LtEq,
        BinOp::GtEq => CmpOp::GtEq,
        _ => unreachable!("not a comparison op: {:?}", op),
    }
}

/// If `ty` (after peeling reference layers) is a struct that derives —
/// explicitly or implicitly per ruby-naming.spec.md §3.6 — `PartialEq`,
/// return the struct's name. Otherwise return `None`.
fn struct_name_with_partial_eq(ty: &Ty, symbols: &SymbolTable) -> Option<String> {
    let mut cur = ty;
    loop {
        match cur {
            Ty::Ref(inner) | Ty::RefMut(inner) => cur = inner,
            Ty::RefLifetime(_, inner) | Ty::RefMutLifetime(_, inner) => cur = inner,
            Ty::Struct { name, .. } => {
                if crate::resolve::symbols::ty_has_derive_trait(cur, symbols, "PartialEq") {
                    return Some(name.clone());
                }
                return None;
            }
            _ => return None,
        }
    }
}

/// Return `Some((struct_name, partial))` when `ty` (peeling refs and
/// aliases) is a struct that declares `derive Ord` or `derive
/// PartialOrd`. The boolean is `true` only when the struct derives
/// `PartialOrd` *without* `Ord`, in which case the BinaryOp lowering
/// must dispatch to `<Type>_partial_cmp` rather than `<Type>_cmp`.
fn struct_name_with_ord(ty: &Ty, symbols: &SymbolTable) -> Option<(String, bool)> {
    use crate::resolve::symbols::DefKind;
    let mut cur = ty;
    loop {
        match cur {
            Ty::Ref(inner) | Ty::RefMut(inner) => cur = inner,
            Ty::RefLifetime(_, inner) | Ty::RefMutLifetime(_, inner) => cur = inner,
            Ty::Struct { name, .. } => {
                for def in symbols.iter() {
                    if def.name == *name {
                        if let DefKind::Struct { ref info } = def.kind {
                            let has_ord = info.derive_traits.iter().any(|t| t == "Ord");
                            let has_partial = info.derive_traits.iter().any(|t| t == "PartialOrd");
                            if has_ord {
                                return Some((name.clone(), false));
                            }
                            if has_partial {
                                return Some((name.clone(), true));
                            }
                        }
                        return None;
                    }
                }
                return None;
            }
            _ => return None,
        }
    }
}

/// Return the ordered `(field_index, field_ty)` list for a struct, or
/// `None` if the name doesn't refer to a known struct.
fn struct_field_layout(name: &str, symbols: &SymbolTable) -> Option<Vec<(usize, Ty)>> {
    use crate::resolve::symbols::DefKind;
    for def in symbols.iter() {
        if def.name == name {
            if let DefKind::Struct { ref info } = def.kind {
                let mut out = Vec::with_capacity(info.fields.len());
                for &fid in &info.fields {
                    let field_def = symbols.get(fid)?;
                    if let DefKind::Field { index, ref ty, .. } = field_def.kind {
                        out.push((index, ty.clone()));
                    }
                }
                return Some(out);
            }
        }
    }
    None
}

// ─── Drop insertion ────────────────────────────────────────────────────────

/// Insert `MirInst::Drop` instructions for all locals that have Move semantics
/// before every `Terminator::Return` in the function.
///
/// Drops are inserted in **reverse declaration order** (LIFO: last declared,
/// first dropped). We skip:
/// - Copy types (primitives, references, bools, etc.)
/// - Parameters (owned by the caller)
/// - Any local that appears as the value of a `Return` terminator
///   (returning the value, not dropping it).
fn insert_drops(
    func: &mut MirFunction,
    return_locals: &HashSet<LocalId>,
    symbols: &SymbolTable,
    user_drop_classes: &HashSet<String>,
) {
    // Avoid recursing into a class's own `drop` method: if `Holder_drop`
    // takes `self: Holder`, drop-elaboration on `self` would emit a call
    // back to `Holder_drop`, recursing forever at runtime.
    let in_user_drop_method = func
        .name
        .strip_suffix("_drop")
        .map(|prefix| user_drop_classes.contains(prefix))
        .unwrap_or(false);

    // Build a set of parameter locals to skip.
    let param_set: HashSet<LocalId> = func.params.iter().copied().collect();

    // Determine which locals' value provenance is a fresh allocation that
    // this frame owns exclusively. We need this set BEFORE building
    // `drop_locals` because String / Vec / HashMap intermediates that are
    // technically compiler temporaries (`_t…` names) can still own heap
    // (e.g. the implicit `riven_string_from` wrap inserted around a string
    // literal whose owner is an outer `String.from(_)` call). Without
    // dropping such intermediates, every interpolated literal leaks the
    // owned-string copy of itself.
    let dealloc_safe = compute_dealloc_safe_locals(func);

    // Locals whose pointer transitively flows into a `Return` value via
    // `Assign`/`Copy`/`Move`. The dealloc-safety analysis processes blocks
    // in linear order, so a forward edge such as `block 4: Assign 2 = 5`
    // (where `5` is initialized later in `block 5`) leaves `2` tainted
    // and `5` alloc-rooted. Without this fixpoint, dropping `5` at the
    // common return block would free the value the caller is about to
    // read through `2`. We compute the set iteratively from the seed
    // `return_locals` and exclude every member from `drop_locals` below.
    let return_alias_chain = compute_return_alias_chain(func, return_locals);

    // Collect locals that need dropping: Move types, not params, not the
    // return value. Collect in declaration order.
    //
    // We always drop user-declared locals (`let` bindings) of an owning
    // type. Compiler-generated temporaries (`_t0`, `_t1`, …) are dropped
    // only when:
    //   * the temp's type is a built-in heap-owning type
    //     (`String`, `Vec`, `HashMap`), AND
    //   * the dealloc-safety analysis classified the temp as alloc-rooted
    //     (its value chain traces to a fresh-alloc callee like
    //     `riven_string_from` / `riven_vec_new` / `riven_hash_new`, with
    //     no aliasing or transfer).
    //
    // Class / Struct / Enum temps are still skipped: they almost always
    // arise from `MirInst::Alloc` followed by an `Assign` to a named
    // local, and the dealloc-safety pass already moves ownership to the
    // named local — dropping the temp would double-free.
    let drop_locals: Vec<LocalId> = func
        .locals
        .iter()
        .filter(|local| {
            // Must be a Move type. Honors `derive Copy` on user-declared
            // structs/classes/enums by consulting the symbol table.
            if ty_is_effectively_copy(&local.ty, symbols) {
                return false;
            }
            // Must not be a parameter.
            if param_set.contains(&local.id) {
                return false;
            }
            // Must not be the return value (any block) or alias to one.
            if return_locals.contains(&local.id) || return_alias_chain.contains(&local.id) {
                return false;
            }
            // Drop types that own heap memory:
            //   * Class/Struct/Enum — always heap-allocated via `riven_alloc`.
            //   * String/Vec/HashMap — heap-allocated via the built-in
            //     constructors (`riven_string_from`, `riven_vec_new`,
            //     `riven_hash_new`). Each gets a dedicated free helper at
            //     drop time (see drop callee dispatch below).
            //
            // The dealloc-safety analysis (`compute_dealloc_safe_locals`)
            // is what guards against double-free for locals whose value
            // came from a non-fresh source (e.g. a function-call return
            // pointing into the caller's heap).
            if !matches!(
                local.ty,
                Ty::Class { .. }
                    | Ty::Struct { .. }
                    | Ty::Enum { .. }
                    | Ty::String
                    | Ty::Array(_)
                    | Ty::Map(_, _)
                    | Ty::Set(_)
            ) {
                return false;
            }
            // Compiler temporaries: only drop heap-owning built-ins
            // whose value is a fresh allocation (see comment above).
            if local.name.starts_with("_t") {
                let is_builtin_heap = matches!(
                    local.ty,
                    Ty::String | Ty::Array(_) | Ty::Map(_, _) | Ty::Set(_)
                );
                return is_builtin_heap && dealloc_safe.contains(&local.id);
            }
            true
        })
        .map(|local| local.id)
        .collect();

    if drop_locals.is_empty() {
        return;
    }

    // Pre-compute, for each drop-eligible local, the user-drop class name
    // so we can emit `{ClassName}_drop(self)` immediately before the
    // dealloc + no-op cleanup. Indexed for cheap lookup inside the loop.
    let drop_callees: HashMap<LocalId, String> = drop_locals
        .iter()
        .filter_map(|&id| {
            let local = func.locals.iter().find(|l| l.id == id)?;
            if let Ty::Class { name, .. } = &local.ty {
                if user_drop_classes.contains(name) {
                    return Some((id, format!("{}_drop", name)));
                }
            }
            None
        })
        .collect();

    // For each block that ends with a Return terminator, insert Drop
    // instructions (in reverse declaration order) before the return.
    for block in &mut func.blocks {
        if matches!(block.terminator, Terminator::Return(_)) {
            // Insert drops in reverse declaration order (LIFO).
            for &local_id in drop_locals.iter().rev() {
                // 1) User-defined `def drop` runs first (if any), so the
                //    body still sees the live allocation. Skip when we're
                //    already lowering that class's own drop method to
                //    avoid infinite self-recursion.
                if !in_user_drop_method {
                    if let Some(callee) = drop_callees.get(&local_id) {
                        block.instructions.push(MirInst::Call {
                            dest: None,
                            callee: callee.clone(),
                            args: vec![MirValue::Use(local_id)],
                        });
                    }
                }
                // 2) Heap-allocated values need their backing memory
                // freed at scope exit. `MirInst::Drop` itself remains a
                // no-op in both backends — the free call we emit just
                // below is what releases memory.
                //
                // Only emit the free when we can prove the local owns a
                // fresh allocation (see `compute_dealloc_safe_locals`).
                // The callee depends on the local's type:
                //   * Class/Struct/Enum     → `riven_dealloc`
                //   * String                → `riven_string_free`
                //   * Vec[_]                → `riven_vec_free` (spine only)
                //   * HashMap[_, _]         → `riven_hash_free` (spine only)
                if dealloc_safe.contains(&local_id) {
                    let local = func
                        .locals
                        .iter()
                        .find(|l| l.id == local_id)
                        .expect("drop_locals references a missing local");
                    let drop_callee = match &local.ty {
                        Ty::String => "riven_string_free",
                        // Phase 2 stdlib batch 2 (#03): pick the
                        // element-aware drop helper for Vec types whose
                        // element owns heap. `riven_vec_drop_string`
                        // walks slots as `char*` and frees each before
                        // releasing the spine; `riven_vec_drop_vec`
                        // walks slots as `RivenVec*` and recurses one
                        // level. Anything else (primitive elements,
                        // HashMap-of-Vec, deeper nesting) falls back to
                        // the spine-only `riven_vec_free`. The deeper
                        // shapes will land alongside the trait-driven
                        // drop dispatch in #05.
                        Ty::Array(elem) => match elem.as_ref() {
                            Ty::String => "riven_vec_drop_string",
                            Ty::Array(_) => "riven_vec_drop_vec",
                            _ => "riven_vec_free",
                        },
                        // Phase 2 stdlib batch 2 (#04): pick the
                        // element-aware drop helper for HashMap types
                        // whose key and/or value owns heap. The selector
                        // mirrors the Vec one above: a four-way table
                        // over `(K is heap, V is heap)`. Heap-owned in
                        // v1 means String or Vec[_]; the deeper Trie of
                        // nested heap (HashMap-in-HashMap, Set-in-V) is
                        // a follow-up alongside the trait-driven drop
                        // dispatch in #05 and is documented in
                        // CHANGELOG known limitations.
                        Ty::Map(k, v) => {
                            let k_string = matches!(k.as_ref(), Ty::String);
                            let v_string = matches!(v.as_ref(), Ty::String);
                            let v_vec = matches!(v.as_ref(), Ty::Array(_));
                            match (k_string, v_string, v_vec) {
                                (true, true, _) => "riven_hash_drop_string_string",
                                (true, false, _) => "riven_hash_drop_string_v",
                                (false, true, _) => "riven_hash_drop_v_string",
                                (false, false, true) => "riven_hash_drop_v_vec",
                                _ => "riven_hash_free",
                            }
                        }
                        // Phase 2 stdlib batch 2 (#04): HashSet[T] —
                        // spine free is `riven_set_free`; if T is a
                        // String the per-element drop selector walks
                        // slots before delegating.
                        Ty::Set(elem) => match elem.as_ref() {
                            Ty::String => "riven_set_drop_string",
                            _ => "riven_set_free",
                        },
                        Ty::Class { .. } | Ty::Struct { .. } | Ty::Enum { .. } => "riven_dealloc",
                        // Unreachable: the drop_locals filter above
                        // restricts to exactly the variants matched here.
                        // We use unimplemented! rather than a silent
                        // fallback so a future widening of the filter
                        // surfaces immediately.
                        other => {
                            unimplemented!("insert_drops: no drop callee for type {:?}", other)
                        }
                    };
                    block.instructions.push(MirInst::Call {
                        dest: None,
                        callee: drop_callee.to_string(),
                        args: vec![MirValue::Use(local_id)],
                    });
                }
                block.instructions.push(MirInst::Drop { local: local_id });
            }
        }
    }
}

/// Compute the set of locals whose value provenance is a fresh
/// `MirInst::Alloc` (or a chain of `Assign`/`Copy`/`Move` from one) AND
/// whose ownership of that allocation has not been aliased away to
/// another local or transferred to a callee/aggregate.
///
/// These are the locals safe to pass to `riven_dealloc` at scope exit:
/// freeing them releases an allocation that this frame owns exclusively.
///
/// A local is excluded (NOT dealloc-safe) when:
///
///   * Its provenance is not a fresh `Alloc` — for example `GetField` /
///     `GetPayload` (pointer into a parent struct), a function-call
///     return value (caller-owned semantics), `Ref`/`RefMut`, or numeric
///     /string literal output.
///   * Ownership escapes the current frame: the local is passed as a
///     `Use(local)` argument to any `Call`/`CallIndirect`, or stored
///     into another aggregate via `SetField`. After such a transfer the
///     callee/aggregate may have copied or freed the pointer, so a
///     scope-exit dealloc could double-free or use-after-free.
///   * Its value flows (via `Assign`/`Copy`/`Move`) into another local.
///     The MIR doesn't deep-copy structs/classes/enums on assignment —
///     the destination receives the same pointer — so dealloc'ing both
///     would double-free the shared allocation. We move the dealloc
///     responsibility to the last local in each propagation chain.
fn compute_dealloc_safe_locals(func: &MirFunction) -> std::collections::HashSet<LocalId> {
    use std::collections::HashSet;

    // `alloc_rooted`: locals whose value currently traces back to a fresh
    // `MirInst::Alloc` and that have not yet been aliased away or
    // transferred. We process instructions in a single forward pass over
    // a flattened block order; an instruction may both *grant* a local
    // alloc-rooted status (e.g. `Alloc dest=L`) and *revoke* it (e.g.
    // `Use(L)` as a Call argument later in the same block).
    let mut alloc_rooted: HashSet<LocalId> = HashSet::new();
    // `tainted_perm`: once a local was passed to a callee/aggregate or
    // its value was propagated to another local, it can never become
    // dealloc-safe. Re-allocations into the same id (rare) would still
    // honor this — we conservatively keep the local out of the safe set.
    let mut tainted_perm: HashSet<LocalId> = HashSet::new();

    // Single forward pass — block order is the lowering order, which
    // matches program execution order for the cases we care about.
    // Loops/back-edges are not modeled; back-edges only matter when an
    // alloc-rooted local is mutated mid-loop, which the lowerer
    // currently never produces for user `let` bindings of Class/Struct
    // /Enum types.
    for block in &func.blocks {
        for inst in &block.instructions {
            match inst {
                MirInst::Alloc { dest, .. } | MirInst::StackAlloc { dest, .. } => {
                    if !tainted_perm.contains(dest) {
                        alloc_rooted.insert(*dest);
                    }
                }
                MirInst::Assign { dest, value } => {
                    if let MirValue::Use(src) = value {
                        if alloc_rooted.contains(src) && !tainted_perm.contains(dest) {
                            // Pointer-copy: dest now aliases src's
                            // allocation. Hand dealloc responsibility to
                            // dest by tainting src permanently.
                            tainted_perm.insert(*src);
                            alloc_rooted.remove(src);
                            alloc_rooted.insert(*dest);
                            continue;
                        }
                    }
                    // Literal or non-alloc-rooted source → permanently
                    // exclude dest. (We also drop any prior alloc-root
                    // status, since the local is being overwritten.)
                    tainted_perm.insert(*dest);
                    alloc_rooted.remove(dest);
                }
                MirInst::Copy { dest, src } | MirInst::Move { dest, src } => {
                    if alloc_rooted.contains(src) && !tainted_perm.contains(dest) {
                        tainted_perm.insert(*src);
                        alloc_rooted.remove(src);
                        alloc_rooted.insert(*dest);
                    } else {
                        tainted_perm.insert(*dest);
                        alloc_rooted.remove(dest);
                    }
                }
                // Every other instruction that defines a local produces
                // a value that is NOT a fresh allocation owned by this
                // frame — taint the destination.
                MirInst::BinOp { dest, .. }
                | MirInst::Negate { dest, .. }
                | MirInst::Not { dest, .. }
                | MirInst::Compare { dest, .. }
                | MirInst::GetField { dest, .. }
                | MirInst::GetTag { dest, .. }
                | MirInst::GetPayload { dest, .. }
                | MirInst::Ref { dest, .. }
                | MirInst::RefMut { dest, .. }
                | MirInst::StringLiteral { dest, .. }
                | MirInst::FuncAddr { dest, .. } => {
                    tainted_perm.insert(*dest);
                    alloc_rooted.remove(dest);
                }
                MirInst::Call { dest, callee, args } => {
                    // Whitelist of callees that produce a fresh, heap
                    // allocation owned exclusively by `dest`. The result
                    // behaves like `MirInst::Alloc { dest }` — the local
                    // can be freed at scope exit by the matching helper.
                    //
                    // Rule: the callee must allocate new heap that is
                    // returned to the caller, with no pre-existing alias
                    // on entry. `riven_string_from` / `riven_string_concat`
                    // both return a fresh `malloc`'d buffer; `riven_vec_new`
                    // and `riven_hash_new` return fresh struct heap.
                    // The whitelist covers both the MIR-level mangled
                    // names emitted by the lowerer (e.g. `Vec_new`,
                    // `HashMap_new`, `String_from`) and the runtime-
                    // level names used at internal emit sites (e.g.
                    // `riven_string_from` in literal materialisation).
                    // Both forms reach the codegen as `Call { callee }`
                    // and both produce fresh heap.
                    const FRESH_ALLOC_CALLEES: &[&str] = &[
                        "String_from",
                        "Vec_new",
                        "Hash_new",
                        "HashMap_new",
                        "riven_string_from",
                        "riven_string_concat",
                        "riven_vec_new",
                        "riven_hash_new",
                        // Phase 2 stdlib batch 2 (#02): `into_bytes`
                        // returns a freshly-allocated Vec[U8] whose
                        // ownership is exclusive to the dest local —
                        // drop-elaborate it like any other Vec.
                        "String_into_bytes",
                        "riven_string_into_bytes",
                        // `push` allocates a fresh char buffer and the
                        // surrounding mir lowering rebinds the receiver
                        // local to it; the local must remain alloc-rooted
                        // so scope-exit drop emits `riven_string_free`.
                        "String_push",
                        "riven_string_push",
                        // Phase 2 stdlib batch 2 (#03): `Vec.from_iter`
                        // takes ownership of the source iter (which IS a
                        // RivenVec* in v1) and re-emerges as a fresh
                        // owning Vec. Drop-elaborate as a fresh alloc; the
                        // source iter local is tainted via the consume
                        // helper match below.
                        "Vec_from_iter",
                        "riven_vec_from_iter",
                        "String_from_iter",
                        "HashMap_from_iter",
                        "Hash_from_iter",
                        "Set_from_iter",
                        "HashSet_from_iter",
                        // Phase 2 stdlib (#05 follow-up): built-in
                        // `collect[Target]` helpers materialise fresh
                        // heap-owned collections / strings from the
                        // eager v1 iterator representation.
                        "riven_string_from_iter",
                        "riven_hash_from_iter",
                        "riven_set_from_iter",
                        // Phase 2 stdlib (#04): HashMap[K,V] / HashSet[T]
                        // surface. Each of these returns a freshly-
                        // allocated heap object whose ownership is the
                        // caller's drop frame. Without this whitelist
                        // the dest local would be tainted on the very
                        // first instruction (default Call rule) and the
                        // scope-exit drop pass would skip the spine
                        // free, leaking the allocation.
                        // The bare constructors are emitted via the
                        // mangled `Hash_new` / `HashMap_new` / `Set_new`
                        // / `HashSet_new` callee names; without them
                        // here the dest local would be tainted by the
                        // default Call rule and the spine would leak.
                        "Set_new",
                        "HashSet_new",
                        "riven_set_new",
                        "HashMap_with_capacity",
                        "Hash_with_capacity",
                        "riven_hash_with_capacity",
                        "HashMap_keys",
                        "HashMap_values",
                        "HashMap_iter",
                        "Hash_keys",
                        "Hash_values",
                        "Hash_iter",
                        "riven_hash_keys",
                        "riven_hash_values",
                        "riven_hash_iter",
                        // `riven_hash_remove` returns a fresh
                        // 16-byte tagged Option allocated via
                        // `riven_alloc` — drop-elaborate as a fresh
                        // allocation so the temp is freed on scope exit.
                        "HashMap_remove",
                        "Hash_remove",
                        "riven_hash_remove",
                        "HashSet_with_capacity",
                        "Set_with_capacity",
                        "riven_set_with_capacity",
                        "HashSet_iter",
                        "Set_iter",
                        "riven_set_iter",
                        // Set ops (`union` / `intersection` /
                        // `difference`) each materialise a brand-new
                        // RivenSet. The two source sets are borrowed.
                        "HashSet_union",
                        "HashSet_intersection",
                        "HashSet_difference",
                        "Set_union",
                        "Set_intersection",
                        "Set_difference",
                        "riven_set_union",
                        "riven_set_intersection",
                        "riven_set_difference",
                        // Phase 2 stdlib (#05 batch 3): `chain(other)` and
                        // `zip(other)` materialise into a fresh
                        // `RivenVec*` (and, for `zip`, fresh per-pair
                        // tuple cells). The destination local owns the
                        // spine so the scope-exit drop pass must emit
                        // a `riven_vec_free` on it. Both source iters
                        // are *borrowed* (not consumed) — they remain
                        // owned by their caller frames, mirroring
                        // `clone` / `take` / `skip`.
                        "riven_vec_chain",
                        "riven_vec_zip",
                    ];
                    let returns_fresh_alloc = FRESH_ALLOC_CALLEES.contains(&callee.as_str());
                    if let Some(d) = dest {
                        if returns_fresh_alloc && !tainted_perm.contains(d) {
                            alloc_rooted.insert(*d);
                        } else {
                            tainted_perm.insert(*d);
                            alloc_rooted.remove(d);
                        }
                    }
                    // Args passed as `Use(L)` may have their ownership
                    // transferred to the callee — taint L by default.
                    // Structural exceptions (callee borrows rather than
                    // consumes its first arg, so the local stays
                    // dealloc-safe afterwards):
                    //
                    //  * `{Type}_init`: constructor pseudo-call that only
                    //    initializes fields of `self` (first arg).
                    //  * `riven_dealloc` / `riven_string_free` /
                    //    `riven_vec_free` / `riven_hash_free`: emitted by
                    //    drop elaboration itself; treating their arg as a
                    //    transfer would cancel the free we are about to
                    //    insert.
                    //  * `riven_*` runtime helpers (other than the four
                    //    above): in v1 every runtime built-in borrows its
                    //    pointer arguments. `riven_vec_push(v, item)`,
                    //    `riven_hash_insert(h, k, v)`, `riven_string_len(s)`
                    //    and friends mutate or read in place — they never
                    //    free the pointer, so passing a heap-owned local
                    //    must not invalidate its dealloc-safety.
                    //
                    // User-defined callees fall through to the
                    // conservative default and taint every `Use(L)` arg.
                    let borrows_first_arg = callee.ends_with("_init")
                        || callee == "riven_dealloc"
                        || callee == "riven_string_free"
                        || callee == "riven_vec_free"
                        || callee == "riven_vec_drop_string"
                        || callee == "riven_vec_drop_vec"
                        || callee == "riven_hash_free"
                        // Phase 2 stdlib batch 2 (#04): set spine +
                        // hash/set per-element drop selectors are
                        // emitted by the drop pass itself; treating
                        // their first arg as a transfer would cancel
                        // the free we are about to insert.
                        || callee == "riven_set_free"
                        || callee == "riven_hash_drop_string_v"
                        || callee == "riven_hash_drop_v_string"
                        || callee == "riven_hash_drop_string_string"
                        || callee == "riven_hash_drop_v_vec"
                        || callee == "riven_set_drop_string";
                    // A callee borrows its pointer args (does not transfer
                    // ownership) when it is a runtime helper. Two forms
                    // appear in MIR:
                    //
                    //  1. The literal `riven_*` runtime symbol (emitted at
                    //     internal lowering sites that bypass the method
                    //     dispatch path).
                    //  2. The MIR-level mangled name (`Vec_push`,
                    //     `HashMap_insert`, `String_len`, …) which is
                    //     translated to a `riven_*` symbol by
                    //     `codegen::runtime::runtime_name`. The mangled
                    //     name uses one of the known built-in type prefixes,
                    //     so we recognise it by prefix.
                    //
                    // The four free helpers are excluded — they are the
                    // ONLY runtime helpers that consume their pointer
                    // arg, but the drop pass inserts those itself, so the
                    // analysis must stop before they appear.
                    let is_runtime_consume_helper = matches!(
                        callee.as_str(),
                        "riven_dealloc"
                            | "riven_string_free"
                            | "riven_vec_free"
                            | "riven_vec_drop_string"
                            | "riven_vec_drop_vec"
                            | "riven_hash_free"
                            // Phase 2 stdlib batch 2 (#04): set spine +
                            // hash/set per-element drop helpers consume
                            // their first arg the same way the rest of
                            // the free family does. Excluded from the
                            // borrow-helper analysis below so the drop
                            // pass that emits the call doesn't get the
                            // arg double-tainted.
                            | "riven_set_free"
                            | "riven_hash_drop_string_v"
                            | "riven_hash_drop_v_string"
                            | "riven_hash_drop_string_string"
                            | "riven_hash_drop_v_vec"
                            | "riven_set_drop_string"
                            // Phase 2 stdlib batch 2 (#02): `into_bytes`
                            // is the consuming variant of `bytes` — its
                            // runtime fn frees the source `char*`
                            // internally. Treat it as a transfer so the
                            // drop pass does NOT also emit a
                            // `riven_string_free` on the receiver
                            // (which would double-free).
                            | "riven_string_into_bytes"
                            | "String_into_bytes"
                            // Phase 2 stdlib batch 2 (#03): `Vec.from_iter`
                            // and `Vec.into_iter` both consume their
                            // source iterator/Vec. The runtime
                            // representation is identity so the destination
                            // owns the same backing array as the source
                            // — taint the source local so scope-exit
                            // drop does not emit a second
                            // `riven_vec_free` (double-free).
                            | "riven_vec_from_iter"
                            | "Vec_from_iter"
                    );
                    let is_runtime_borrow_helper = !is_runtime_consume_helper
                        && (callee.starts_with("riven_")
                            || callee.starts_with("Vec_")
                            || callee.starts_with("Vec[")
                            || callee.starts_with("Hash_")
                            || callee.starts_with("Hash[")
                            || callee.starts_with("HashMap_")
                            || callee.starts_with("HashMap[")
                            || callee.starts_with("Set_")
                            || callee.starts_with("Set[")
                            || callee.starts_with("HashSet_")
                            || callee.starts_with("HashSet[")
                            || callee.starts_with("String_")
                            || callee.starts_with("&str_"));
                    // `riven_store_ptr(p, v)` writes `v` into `*p`,
                    // transferring `v`'s allocation through the pointer
                    // to whatever owns `*p`. The store is the moment
                    // ownership leaves this frame, so `v` must be
                    // tainted even though the helper itself starts
                    // with `riven_`. Without this special-case the
                    // mir-emitted free at scope-exit on the temp would
                    // race the caller's free on the same pointer
                    // (P0.7 double-free).
                    let is_pointer_store_helper = callee == "riven_store_ptr";
                    // Phase 2 stdlib batch 2 (#03): `riven_vec_push(v, item)`
                    // is normally a borrow helper (mutates `v` in place,
                    // returns nothing). But for `Vec[String]` /
                    // `Vec[Vec[_]]` / `Vec[HashMap[_,_]]`, the item slot
                    // takes ownership of the heap allocation behind
                    // `item`. Now that `riven_vec_drop_string` /
                    // `riven_vec_drop_vec` walk those slots and free the
                    // contents at scope exit (#03 batch 2 drop selector
                    // wiring), the pushed temp must be tainted so the
                    // drop pass does NOT also emit a `riven_string_free`
                    // / `riven_vec_free` on it (double-free).
                    //
                    // Same logic for `riven_vec_insert(v, idx, item)` and
                    // `riven_hash_insert(h, k, v)` whose value slots also
                    // take ownership.
                    // Match both the un-mangled runtime name and the
                    // MIR-level mangled `Vec[T]_push` form. The mangled
                    // form is what `mangled = format!("{}_{}", type_name,
                    // method_name)` produces (line ~1545); type_name for
                    // a `Vec[String]` receiver is the full `"Vec[String]"`
                    // string. Use `extract_method_name` to pull the
                    // method off the mangled form so we don't have to
                    // enumerate every `Vec[T]` instantiation.
                    // Returns the indices of arguments whose ownership
                    // is transferred by this callee (set of arg
                    // positions). For Vec.push / Vec.insert the slot
                    // is single (1 or 2); for HashMap.insert and
                    // HashSet.insert BOTH the key and the value can
                    // own heap, so we taint each independently. The
                    // dynamic check on element type was considered
                    // and rejected: at this point in the analysis we
                    // don't carry per-arg static types, but tainting
                    // a primitive temp is a no-op (it has no heap to
                    // double-free), so over-tainting is safe.
                    let transfer_indices: &[usize] = {
                        use crate::codegen::runtime::extract_method_name;
                        let m = extract_method_name(callee.as_str());
                        let is_vec_method =
                            callee.starts_with("Vec_") || callee.starts_with("Vec[");
                        let is_hash_method = callee.starts_with("Hash_")
                            || callee.starts_with("Hash[")
                            || callee.starts_with("HashMap_")
                            || callee.starts_with("HashMap[");
                        let is_set_method = callee.starts_with("Set_")
                            || callee.starts_with("Set[")
                            || callee.starts_with("HashSet_")
                            || callee.starts_with("HashSet[");
                        match (is_vec_method, is_hash_method, is_set_method, m) {
                            (true, _, _, "push") => &[1],
                            (true, _, _, "insert") => &[2],
                            // HashMap.insert(self, K, V) — both K (1)
                            // and V (2) can own heap.
                            (_, true, _, "insert") => &[1, 2],
                            // HashSet.insert(self, T) — T (1) can own
                            // heap.
                            (_, _, true, "insert") => &[1],
                            _ if callee == "riven_vec_push" => &[1],
                            _ if callee == "riven_vec_insert" => &[2],
                            _ if callee == "riven_hash_insert" => &[1, 2],
                            _ if callee == "riven_set_insert" => &[1],
                            _ => &[],
                        }
                    };
                    for (idx, arg) in args.iter().enumerate() {
                        if let MirValue::Use(l) = arg {
                            if borrows_first_arg && idx == 0 {
                                continue;
                            }
                            if is_pointer_store_helper && idx == 1 {
                                tainted_perm.insert(*l);
                                alloc_rooted.remove(l);
                                continue;
                            }
                            if transfer_indices.contains(&idx) {
                                tainted_perm.insert(*l);
                                alloc_rooted.remove(l);
                                continue;
                            }
                            if is_runtime_borrow_helper {
                                continue;
                            }
                            tainted_perm.insert(*l);
                            alloc_rooted.remove(l);
                        }
                    }
                }
                MirInst::CallIndirect { dest, args, .. } => {
                    if let Some(d) = dest {
                        tainted_perm.insert(*d);
                        alloc_rooted.remove(d);
                    }
                    for arg in args {
                        if let MirValue::Use(l) = arg {
                            tainted_perm.insert(*l);
                            alloc_rooted.remove(l);
                        }
                    }
                }
                MirInst::SetField { value, .. } => {
                    // Storing a local into another aggregate transfers
                    // ownership (the aggregate now references it).
                    if let MirValue::Use(l) = value {
                        tainted_perm.insert(*l);
                        alloc_rooted.remove(l);
                    }
                }
                // No-op for instructions that don't define or move a
                // local.
                MirInst::SetTag { .. } | MirInst::Drop { .. } | MirInst::Nop => {}
            }
        }
    }

    alloc_rooted
}

/// Compute the transitive set of locals whose value flows into a `Return`
/// terminator via `Assign` / `Copy` / `Move`.
///
/// Seed the set with the locals that appear directly in any
/// `Return(Some(Use(L)))` terminator, then iterate to a fixpoint: every
/// `Assign { dest: D, value: Use(S) }` (and `Copy`/`Move`) where `D` is
/// already in the set adds `S` as well — `S` aliases the same heap as
/// `D`, so freeing it would corrupt the returned value.
///
/// We need this because `compute_dealloc_safe_locals` walks blocks in
/// linear order. A function whose return block precedes the producer
/// block (typical for `match`-arm-as-tail-expression) leaves the
/// intermediate alloc-rooted, which would otherwise be picked up as a
/// drop candidate. Excluding the alias chain keeps the dealloc safe.
fn compute_return_alias_chain(
    func: &MirFunction,
    seed_locals: &HashSet<LocalId>,
) -> HashSet<LocalId> {
    use std::collections::HashSet;
    let mut chain: HashSet<LocalId> = seed_locals.clone();
    loop {
        let before = chain.len();
        for block in &func.blocks {
            for inst in &block.instructions {
                match inst {
                    MirInst::Assign { dest, value } => {
                        if chain.contains(dest) {
                            if let MirValue::Use(src) = value {
                                chain.insert(*src);
                            }
                        }
                    }
                    MirInst::Copy { dest, src } | MirInst::Move { dest, src } => {
                        if chain.contains(dest) {
                            chain.insert(*src);
                        }
                    }
                    _ => {}
                }
            }
        }
        if chain.len() == before {
            break;
        }
    }
    chain
}

// ─── Closure capture analysis ───────────────────────────────────────────────

/// Walk a closure body and collect the `DefId`s of free variables that must
/// be captured from the enclosing frame.  A variable is captured when:
///
///  * it is referenced inside the body, AND
///  * it is not a parameter of the closure, AND
///  * it was not introduced by a `let` inside the body, AND
///  * it has a known enclosing-frame local (i.e. it lives in `def_to_local`).
///
/// Duplicates are removed while preserving first-occurrence order so the
/// slot indices in the captures struct are deterministic.
fn collect_captures(
    expr: &HirExpr,
    closure_params: &HashSet<DefId>,
    outer_defs: &HashMap<DefId, LocalId>,
    out: &mut Vec<DefId>,
    seen: &mut HashSet<DefId>,
) {
    let mut locally_bound: HashSet<DefId> = HashSet::new();
    collect_captures_inner(
        expr,
        closure_params,
        outer_defs,
        &mut locally_bound,
        out,
        seen,
    );
}

fn collect_captures_inner(
    expr: &HirExpr,
    closure_params: &HashSet<DefId>,
    outer_defs: &HashMap<DefId, LocalId>,
    locally_bound: &mut HashSet<DefId>,
    out: &mut Vec<DefId>,
    seen: &mut HashSet<DefId>,
) {
    match &expr.kind {
        HirExprKind::VarRef(def_id) => {
            if !closure_params.contains(def_id)
                && !locally_bound.contains(def_id)
                && outer_defs.contains_key(def_id)
                && !seen.contains(def_id)
            {
                out.push(*def_id);
                seen.insert(*def_id);
            }
        }
        HirExprKind::FieldAccess { object, .. } => {
            collect_captures_inner(object, closure_params, outer_defs, locally_bound, out, seen);
        }
        HirExprKind::MethodCall {
            object,
            args,
            block,
            ..
        } => {
            collect_captures_inner(object, closure_params, outer_defs, locally_bound, out, seen);
            for a in args {
                collect_captures_inner(a, closure_params, outer_defs, locally_bound, out, seen);
            }
            if let Some(b) = block {
                collect_captures_inner(b, closure_params, outer_defs, locally_bound, out, seen);
            }
        }
        HirExprKind::FnCall { args, .. } => {
            for a in args {
                collect_captures_inner(a, closure_params, outer_defs, locally_bound, out, seen);
            }
        }
        HirExprKind::BinaryOp { left, right, .. } => {
            collect_captures_inner(left, closure_params, outer_defs, locally_bound, out, seen);
            collect_captures_inner(right, closure_params, outer_defs, locally_bound, out, seen);
        }
        HirExprKind::UnaryOp { operand, .. } => {
            collect_captures_inner(
                operand,
                closure_params,
                outer_defs,
                locally_bound,
                out,
                seen,
            );
        }
        HirExprKind::Borrow { expr: inner, .. } => {
            collect_captures_inner(inner, closure_params, outer_defs, locally_bound, out, seen);
        }
        HirExprKind::Block(stmts, tail) | HirExprKind::UnsafeBlock(stmts, tail) => {
            let saved_bound = locally_bound.clone();
            for s in stmts {
                collect_captures_in_stmt(s, closure_params, outer_defs, locally_bound, out, seen);
            }
            if let Some(t) = tail {
                collect_captures_inner(t, closure_params, outer_defs, locally_bound, out, seen);
            }
            *locally_bound = saved_bound;
        }
        HirExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_captures_inner(cond, closure_params, outer_defs, locally_bound, out, seen);
            collect_captures_inner(
                then_branch,
                closure_params,
                outer_defs,
                locally_bound,
                out,
                seen,
            );
            if let Some(e) = else_branch {
                collect_captures_inner(e, closure_params, outer_defs, locally_bound, out, seen);
            }
        }
        HirExprKind::Match { scrutinee, arms } => {
            collect_captures_inner(
                scrutinee,
                closure_params,
                outer_defs,
                locally_bound,
                out,
                seen,
            );
            for arm in arms {
                if let Some(g) = &arm.guard {
                    collect_captures_inner(g, closure_params, outer_defs, locally_bound, out, seen);
                }
                collect_captures_inner(
                    &arm.body,
                    closure_params,
                    outer_defs,
                    locally_bound,
                    out,
                    seen,
                );
            }
        }
        HirExprKind::While { condition, body } => {
            collect_captures_inner(
                condition,
                closure_params,
                outer_defs,
                locally_bound,
                out,
                seen,
            );
            collect_captures_inner(body, closure_params, outer_defs, locally_bound, out, seen);
        }
        HirExprKind::Loop { body } => {
            collect_captures_inner(body, closure_params, outer_defs, locally_bound, out, seen);
        }
        HirExprKind::For {
            iterable,
            body,
            binding,
            tuple_bindings,
            ..
        } => {
            collect_captures_inner(
                iterable,
                closure_params,
                outer_defs,
                locally_bound,
                out,
                seen,
            );
            let saved_bound = locally_bound.clone();
            locally_bound.insert(*binding);
            for (d, _) in tuple_bindings {
                locally_bound.insert(*d);
            }
            collect_captures_inner(body, closure_params, outer_defs, locally_bound, out, seen);
            *locally_bound = saved_bound;
        }
        HirExprKind::Assign { target, value, .. }
        | HirExprKind::CompoundAssign { target, value, .. } => {
            collect_captures_inner(target, closure_params, outer_defs, locally_bound, out, seen);
            collect_captures_inner(value, closure_params, outer_defs, locally_bound, out, seen);
        }
        HirExprKind::Return(Some(inner)) | HirExprKind::Break(Some(inner)) => {
            collect_captures_inner(inner, closure_params, outer_defs, locally_bound, out, seen);
        }
        HirExprKind::Tuple(elems) | HirExprKind::ArrayLiteral(elems) => {
            for e in elems {
                collect_captures_inner(e, closure_params, outer_defs, locally_bound, out, seen);
            }
        }
        HirExprKind::Index { object, index } => {
            collect_captures_inner(object, closure_params, outer_defs, locally_bound, out, seen);
            collect_captures_inner(index, closure_params, outer_defs, locally_bound, out, seen);
        }
        HirExprKind::Construct { fields, .. } => {
            for (_n, v) in fields {
                collect_captures_inner(v, closure_params, outer_defs, locally_bound, out, seen);
            }
        }
        HirExprKind::EnumVariant { fields, .. } => {
            for (_n, v) in fields {
                collect_captures_inner(v, closure_params, outer_defs, locally_bound, out, seen);
            }
        }
        HirExprKind::Interpolation { parts } => {
            for p in parts {
                if let crate::hir::nodes::HirInterpolationPart::Expr { expr: e, .. } = p {
                    collect_captures_inner(e, closure_params, outer_defs, locally_bound, out, seen);
                }
            }
        }
        HirExprKind::MacroCall { args, .. } => {
            for a in args {
                collect_captures_inner(a, closure_params, outer_defs, locally_bound, out, seen);
            }
        }
        HirExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                collect_captures_inner(s, closure_params, outer_defs, locally_bound, out, seen);
            }
            if let Some(e) = end {
                collect_captures_inner(e, closure_params, outer_defs, locally_bound, out, seen);
            }
        }
        HirExprKind::ArrayFill { value, .. } => {
            collect_captures_inner(value, closure_params, outer_defs, locally_bound, out, seen);
        }
        HirExprKind::Closure {
            body: nested,
            params: nested_params,
            ..
        } => {
            // A nested closure sees our captured vars too.  Merge its
            // parameters into `closure_params` just for the nested walk.
            let mut merged = closure_params.clone();
            for p in nested_params {
                merged.insert(p.def_id);
            }
            let saved_bound = locally_bound.clone();
            collect_captures_inner(nested, &merged, outer_defs, locally_bound, out, seen);
            *locally_bound = saved_bound;
        }
        HirExprKind::Cast { expr: inner, .. } => {
            collect_captures_inner(inner, closure_params, outer_defs, locally_bound, out, seen);
        }
        // Leaf expressions — nothing to traverse.
        _ => {}
    }
}

fn collect_captures_in_stmt(
    stmt: &HirStatement,
    closure_params: &HashSet<DefId>,
    outer_defs: &HashMap<DefId, LocalId>,
    locally_bound: &mut HashSet<DefId>,
    out: &mut Vec<DefId>,
    seen: &mut HashSet<DefId>,
) {
    match stmt {
        HirStatement::Let { def_id, value, .. } => {
            if let Some(v) = value {
                collect_captures_inner(v, closure_params, outer_defs, locally_bound, out, seen);
            }
            locally_bound.insert(*def_id);
        }
        HirStatement::Expr(e) => {
            collect_captures_inner(e, closure_params, outer_defs, locally_bound, out, seen);
        }
    }
}

/// Return `true` if the closure body performs any assignment to the given
/// outer-frame `def_id` (used to decide between ByValue and ByRef storage).
fn closure_body_mutates(body: &HirExpr, def_id: DefId) -> bool {
    match &body.kind {
        HirExprKind::Assign { target, value, .. }
        | HirExprKind::CompoundAssign { target, value, .. } => {
            if let HirExprKind::VarRef(d) = &target.kind {
                if *d == def_id {
                    return true;
                }
            }
            closure_body_mutates(target, def_id) || closure_body_mutates(value, def_id)
        }
        HirExprKind::FieldAccess { object, .. } => closure_body_mutates(object, def_id),
        HirExprKind::MethodCall {
            object,
            args,
            block,
            ..
        } => {
            closure_body_mutates(object, def_id)
                || args.iter().any(|a| closure_body_mutates(a, def_id))
                || block
                    .as_ref()
                    .is_some_and(|b| closure_body_mutates(b, def_id))
        }
        HirExprKind::FnCall { args, .. } => args.iter().any(|a| closure_body_mutates(a, def_id)),
        HirExprKind::BinaryOp { left, right, .. } => {
            closure_body_mutates(left, def_id) || closure_body_mutates(right, def_id)
        }
        HirExprKind::UnaryOp { operand, .. } => closure_body_mutates(operand, def_id),
        HirExprKind::Borrow { expr, .. } => closure_body_mutates(expr, def_id),
        HirExprKind::Block(stmts, tail) | HirExprKind::UnsafeBlock(stmts, tail) => {
            for s in stmts {
                if stmt_mutates(s, def_id) {
                    return true;
                }
            }
            tail.as_ref()
                .is_some_and(|t| closure_body_mutates(t, def_id))
        }
        HirExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            closure_body_mutates(cond, def_id)
                || closure_body_mutates(then_branch, def_id)
                || else_branch
                    .as_ref()
                    .is_some_and(|e| closure_body_mutates(e, def_id))
        }
        HirExprKind::Match { scrutinee, arms } => {
            if closure_body_mutates(scrutinee, def_id) {
                return true;
            }
            arms.iter().any(|arm| {
                arm.guard
                    .as_ref()
                    .is_some_and(|g| closure_body_mutates(g, def_id))
                    || closure_body_mutates(&arm.body, def_id)
            })
        }
        HirExprKind::While { condition, body } => {
            closure_body_mutates(condition, def_id) || closure_body_mutates(body, def_id)
        }
        HirExprKind::Loop { body } => closure_body_mutates(body, def_id),
        HirExprKind::For { iterable, body, .. } => {
            closure_body_mutates(iterable, def_id) || closure_body_mutates(body, def_id)
        }
        HirExprKind::Tuple(elems) | HirExprKind::ArrayLiteral(elems) => {
            elems.iter().any(|e| closure_body_mutates(e, def_id))
        }
        HirExprKind::Index { object, index } => {
            closure_body_mutates(object, def_id) || closure_body_mutates(index, def_id)
        }
        HirExprKind::Construct { fields, .. } | HirExprKind::EnumVariant { fields, .. } => {
            fields.iter().any(|(_, v)| closure_body_mutates(v, def_id))
        }
        HirExprKind::Interpolation { parts } => parts.iter().any(|p| match p {
            crate::hir::nodes::HirInterpolationPart::Expr { expr: e, .. } => {
                closure_body_mutates(e, def_id)
            }
            _ => false,
        }),
        HirExprKind::MacroCall { args, .. } => args.iter().any(|a| closure_body_mutates(a, def_id)),
        HirExprKind::Range { start, end, .. } => {
            start
                .as_ref()
                .is_some_and(|s| closure_body_mutates(s, def_id))
                || end
                    .as_ref()
                    .is_some_and(|e| closure_body_mutates(e, def_id))
        }
        HirExprKind::ArrayFill { value, .. } => closure_body_mutates(value, def_id),
        HirExprKind::Return(Some(inner)) | HirExprKind::Break(Some(inner)) => {
            closure_body_mutates(inner, def_id)
        }
        HirExprKind::Closure { body: nested, .. } => closure_body_mutates(nested, def_id),
        HirExprKind::Cast { expr, .. } => closure_body_mutates(expr, def_id),
        _ => false,
    }
}

fn stmt_mutates(stmt: &HirStatement, def_id: DefId) -> bool {
    match stmt {
        HirStatement::Let { value: Some(v), .. } => closure_body_mutates(v, def_id),
        HirStatement::Let { .. } => false,
        HirStatement::Expr(e) => closure_body_mutates(e, def_id),
    }
}

// ─── Trait-default Self substitution ───────────────────────────────────────

/// Rewrite every occurrence of `Ty::TypeParam { name == "Self" }` inside a
/// cloned trait default method's body/params/return type to point at the
/// concrete `impl` target. This is how we monomorphise a default method for
/// each implementor so that `self.field` / `self.other_method` dispatch
/// resolves through the normal `{ConcreteType}_{method}` path.
fn rewrite_self_in_func(func: &mut HirFuncDef, concrete: &Ty) {
    rewrite_self_in_ty(&mut func.return_ty, concrete);
    for p in &mut func.params {
        rewrite_self_in_ty(&mut p.ty, concrete);
    }
    rewrite_self_in_expr(&mut func.body, concrete);
}

fn rewrite_self_in_ty(ty: &mut Ty, concrete: &Ty) {
    match ty {
        Ty::TypeParam { name, .. } if name == "Self" => {
            *ty = concrete.clone();
        }
        Ty::Ref(inner)
        | Ty::RefMut(inner)
        | Ty::RefLifetime(_, inner)
        | Ty::RefMutLifetime(_, inner) => rewrite_self_in_ty(inner, concrete),
        Ty::Tuple(elems) => {
            for e in elems {
                rewrite_self_in_ty(e, concrete);
            }
        }
        Ty::FixedArray(inner, _) => rewrite_self_in_ty(inner, concrete),
        Ty::Option(inner) => rewrite_self_in_ty(inner, concrete),
        Ty::Result(ok, err) => {
            rewrite_self_in_ty(ok, concrete);
            rewrite_self_in_ty(err, concrete);
        }
        _ => {}
    }
}

fn rewrite_self_in_expr(expr: &mut HirExpr, concrete: &Ty) {
    rewrite_self_in_ty(&mut expr.ty, concrete);
    match &mut expr.kind {
        HirExprKind::FieldAccess { object, .. } => {
            rewrite_self_in_expr(object, concrete);
        }
        HirExprKind::MethodCall {
            object,
            args,
            block,
            ..
        } => {
            rewrite_self_in_expr(object, concrete);
            for a in args {
                rewrite_self_in_expr(a, concrete);
            }
            if let Some(b) = block {
                rewrite_self_in_expr(b, concrete);
            }
        }
        HirExprKind::FnCall { args, .. } => {
            for a in args {
                rewrite_self_in_expr(a, concrete);
            }
        }
        HirExprKind::BinaryOp { left, right, .. } => {
            rewrite_self_in_expr(left, concrete);
            rewrite_self_in_expr(right, concrete);
        }
        HirExprKind::UnaryOp { operand, .. } => {
            rewrite_self_in_expr(operand, concrete);
        }
        HirExprKind::Borrow { expr: inner, .. } => {
            rewrite_self_in_expr(inner, concrete);
        }
        HirExprKind::Block(stmts, tail) | HirExprKind::UnsafeBlock(stmts, tail) => {
            for s in stmts {
                rewrite_self_in_stmt(s, concrete);
            }
            if let Some(t) = tail {
                rewrite_self_in_expr(t, concrete);
            }
        }
        HirExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            rewrite_self_in_expr(cond, concrete);
            rewrite_self_in_expr(then_branch, concrete);
            if let Some(e) = else_branch {
                rewrite_self_in_expr(e, concrete);
            }
        }
        HirExprKind::Match { scrutinee, arms } => {
            rewrite_self_in_expr(scrutinee, concrete);
            for arm in arms {
                if let Some(g) = &mut arm.guard {
                    rewrite_self_in_expr(g, concrete);
                }
                rewrite_self_in_expr(&mut arm.body, concrete);
            }
        }
        HirExprKind::Loop { body } => rewrite_self_in_expr(body, concrete),
        HirExprKind::While { condition, body } => {
            rewrite_self_in_expr(condition, concrete);
            rewrite_self_in_expr(body, concrete);
        }
        HirExprKind::For { iterable, body, .. } => {
            rewrite_self_in_expr(iterable, concrete);
            rewrite_self_in_expr(body, concrete);
        }
        HirExprKind::Assign { target, value, .. } => {
            rewrite_self_in_expr(target, concrete);
            rewrite_self_in_expr(value, concrete);
        }
        HirExprKind::CompoundAssign { target, value, .. } => {
            rewrite_self_in_expr(target, concrete);
            rewrite_self_in_expr(value, concrete);
        }
        HirExprKind::Return(e) | HirExprKind::Break(e) => {
            if let Some(inner) = e {
                rewrite_self_in_expr(inner, concrete);
            }
        }
        HirExprKind::Closure { body, .. } => {
            rewrite_self_in_expr(body, concrete);
        }
        HirExprKind::Construct { fields, .. } | HirExprKind::EnumVariant { fields, .. } => {
            for (_, e) in fields {
                rewrite_self_in_expr(e, concrete);
            }
        }
        HirExprKind::Tuple(elems) | HirExprKind::ArrayLiteral(elems) => {
            for e in elems {
                rewrite_self_in_expr(e, concrete);
            }
        }
        HirExprKind::Index { object, index } => {
            rewrite_self_in_expr(object, concrete);
            rewrite_self_in_expr(index, concrete);
        }
        HirExprKind::Cast {
            expr: inner,
            target,
        } => {
            rewrite_self_in_expr(inner, concrete);
            rewrite_self_in_ty(target, concrete);
        }
        HirExprKind::ArrayFill { value, .. } => {
            rewrite_self_in_expr(value, concrete);
        }
        HirExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                rewrite_self_in_expr(s, concrete);
            }
            if let Some(e) = end {
                rewrite_self_in_expr(e, concrete);
            }
        }
        HirExprKind::Interpolation { parts } => {
            for p in parts {
                if let HirInterpolationPart::Expr { expr: e, .. } = p {
                    rewrite_self_in_expr(e, concrete);
                }
            }
        }
        HirExprKind::MacroCall { args, .. } => {
            for a in args {
                rewrite_self_in_expr(a, concrete);
            }
        }
        _ => {}
    }
}

fn rewrite_self_in_stmt(stmt: &mut HirStatement, concrete: &Ty) {
    match stmt {
        HirStatement::Let { ty, value, .. } => {
            rewrite_self_in_ty(ty, concrete);
            if let Some(v) = value {
                rewrite_self_in_expr(v, concrete);
            }
        }
        HirStatement::Expr(e) => rewrite_self_in_expr(e, concrete),
    }
}

// ─── Standalone entry point (backward compat) ───────────────────────────────

/// Convenience function: lower an HIR program to MIR.
pub fn lower_program(program: &HirProgram, symbols: &SymbolTable) -> Result<MirProgram, String> {
    let mut lowerer = Lowerer::new(symbols);
    lowerer.lower_program(program)
}
