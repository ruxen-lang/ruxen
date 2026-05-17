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

                // Handle .new() / .with_capacity() constructor calls:
                // dispatch directly to the runtime symbol (no self arg).
                let is_collection_ctor = method_name == "new"
                    || (method_name == "with_capacity" && {
                        let bt = if let Some(pos) = type_name.find('[') {
                            &type_name[..pos]
                        } else {
                            type_name.as_str()
                        };
                        matches!(
                            bt,
                            "Vec" | "Array" | "Hash" | "HashMap" | "Map" | "Set" | "HashSet"
                        )
                    });
                if is_collection_ctor {
                    // For built-in types (Vec, Hash, Set), call the runtime
                    // constructor directly instead of Alloc + init.
                    let base_type = if let Some(pos) = type_name.find('[') {
                        &type_name[..pos]
                    } else {
                        type_name.as_str()
                    };
                    // Phase 2 #06.D2.S0: `Formatter.new()` dispatches to
                    // the runtime constructor just like Vec/Hash.
                    // Phase 2 #06 (Command): `Command.new(prog)` joins
                    // the same fast path so it dispatches to
                    // `riven_command_new(prog)` instead of going through
                    // the `Class_init` path (Command has no user-defined
                    // init).
                    if matches!(
                        base_type,
                        "Vec"
                            | "Array"
                            | "Hash"
                            | "HashMap"
                            | "Map"
                            | "Set"
                            | "HashSet"
                            | "Formatter"
                            | "Command"
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
                        // The same fast path also handles `with_capacity`,
                        // which takes a single integer arg and lowers to
                        // e.g. `riven_hash_with_capacity(cap)`.
                        let mut call_args = Vec::with_capacity(args.len());
                        for arg in args {
                            let local = self.lower_expr(arg)?;
                            call_args.push(local_to_value(local));
                        }
                        self.emit(MirInst::Call {
                            dest: Some(obj),
                            callee: format!("{}_{}", runtime_base, method_name),
                            args: call_args,
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
                    // Phase 2 #06 (Command): `Command.new(prog)` joins
                    // the same fast path so it dispatches to
                    // `riven_command_new(prog)` instead of going through
                    // the `Class_init` path (Command has no user-defined
                    // init).
                    if matches!(
                        base_type,
                        "Vec"
                            | "Array"
                            | "Hash"
                            | "HashMap"
                            | "Map"
                            | "Set"
                            | "HashSet"
                            | "Formatter"
                            | "Command"
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
            // ruby-naming.spec.md §10a: bare `[a, b, c]` is the canonical
            // `Array[T]` constructor (the `array!` macro is retired). When
            // the inferred type is `Ty::Array(_)` we lower to
            // `Array.new` + `Array.push` calls; for `FixedArray[T; N]`
            // contexts we keep the slot-by-slot Alloc form so stack-
            // allocated arrays still work.
            HirExprKind::ArrayLiteral(elems) => {
                if matches!(expr.ty, Ty::Array(_)) {
                    let arr_ty = expr.ty.clone();
                    let dest = self.new_temp(arr_ty.clone());
                    let new_name = format!("{}_new", type_name_from_ty(&arr_ty));
                    self.emit(MirInst::Call {
                        dest: Some(dest),
                        callee: new_name,
                        args: vec![],
                    });
                    let push_name = format!("{}_push", type_name_from_ty(&arr_ty));
                    for elem in elems {
                        let val_local = self.lower_expr(elem)?;
                        let val = local_to_value(val_local);
                        self.emit(MirInst::Call {
                            dest: None,
                            callee: push_name.clone(),
                            args: vec![MirValue::Use(dest), val],
                        });
                    }
                    return Ok(Some(dest));
                }
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

            // ── Map literal ──────────────────────────────────────────
            // `{ k => v, ... }` lowers like `map!{…}` did pre-spec-§10a:
            // construct an empty `Map[K, V]` and emit one `insert` call
            // per entry.
            HirExprKind::MapLiteral(entries) => {
                let map_ty = expr.ty.clone();
                let dest = self.new_temp(map_ty.clone());
                let new_name = format!("{}_new", type_name_from_ty(&map_ty));
                self.emit(MirInst::Call {
                    dest: Some(dest),
                    callee: new_name,
                    args: vec![],
                });
                let insert_name = format!("{}_insert", type_name_from_ty(&map_ty));
                for (k_expr, v_expr) in entries {
                    let k_local = self.lower_expr(k_expr)?;
                    let v_local = self.lower_expr(v_expr)?;
                    let k_val = local_to_value(k_local);
                    let v_val = local_to_value(v_local);
                    self.emit(MirInst::Call {
                        dest: None,
                        callee: insert_name.clone(),
                        args: vec![MirValue::Use(dest), k_val, v_val],
                    });
                }
                Ok(Some(dest))
            }

            // ── Macro calls (panic!, assert!, …) ─────────────────────
            HirExprKind::MacroCall { name, args } => {
                // ruby-naming.spec.md §10a retires the collection macros:
                // `array!` / `vec!` → `[…]` Array literal, `map!` / `hash!`
                // → `{ k => v, … }` Map literal, `set!` → `Set.from_iter([…])`.
                // The remaining macros (panic!, assert!, …) live here.
                match name.as_str() {
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
                    // ruby-naming.spec.md §10a: the collection macros
                    // are retired. Spell `array!` / `vec!` as `[…]`,
                    // `map!` / `hash!` as `{ k => v, … }`, and `set!`
                    // as `Set.from_iter([…])`.
                    "array" | "vec" | "map" | "hash" | "set" => Err(format!(
                        "macro `{name}!` is retired — use the literal form per ruby-naming.spec §10a"
                    )),
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
