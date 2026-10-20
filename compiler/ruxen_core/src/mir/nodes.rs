//! MIR (Mid-level Intermediate Representation) node definitions.
//!
//! MIR is a control-flow graph (CFG) based IR that sits between HIR and
//! machine code. Each function is a set of basic blocks connected by
//! terminators. Instructions within a block are simple three-address code.

use super::abi::{CalleeIndex, Coercion};
use crate::hir::types::Ty;
use crate::lexer::token::Span;
use crate::parser::ast::BinOp;

/// A local variable identifier (index into MirFunction::locals).
pub type LocalId = u32;

/// A basic block identifier (index into MirFunction::blocks).
pub type BlockId = usize;

// ─── Program ────────────────────────────────────────────────────────────────

/// Top-level MIR program.
#[derive(Debug, Clone)]
pub struct MirProgram {
    pub functions: Vec<MirFunction>,
    /// Name of the entry-point function (if any).
    pub entry: Option<String>,
    /// FFI library declarations collected from `lib` and `extern "C"` blocks.
    pub ffi_libs: Vec<FfiLib>,
    /// Mixin vtables Phase B-2: one per `(class, mixin)` pair where the
    /// class includes a `dispatch runtime` mixin. Codegen emits one
    /// static data section per entry, populated with function-pointer
    /// addresses for each of the mixin's required methods, pointing at
    /// the class's `<ClassName>_<methodname>` symbols. Empty when no
    /// runtime-dispatch mixins are included anywhere in the program —
    /// the existing static-dispatch path stays untouched.
    /// Spec: docs/specs/types/mixin_vtables.spec.md §B3.
    pub vtables: Vec<MirVtable>,
    /// Mixin vtables Phase B-3: one per class that includes any
    /// `dispatch runtime` mixin. Holds an ordered list of vtable
    /// symbol names (one per runtime-dispatch mixin the class
    /// includes), each pointing at one of `MirProgram::vtables`'s
    /// emitted symbols. Codegen emits a static data section
    /// (`__rx_classinfo_<ClassName>`) holding those pointers in order.
    /// The class's allocation header (Phase B-4/B-5) writes a pointer
    /// to this struct at offset 0 of every instance.
    /// Spec: §B8.
    pub class_infos: Vec<MirClassInfo>,
    /// Public, top-level free function names (not class methods, not
    /// `main`). Tier 4.03 (WASM): the LLVM backend, when targeting
    /// `wasm32-*`, sets the LLVM `export_name` attribute on each of
    /// these so the function is callable from a JS host (a wasm export)
    /// without name mangling. The Cranelift backend ignores this field
    /// entirely — it is wasm-only metadata, populated unconditionally at
    /// lower time and consumed only on the wasm codegen path. v1 exports
    /// every public top-level `def` by its source name; a per-function
    /// `wasm_export "custom"` rename directive is the staged remainder
    /// (`docs/decisions/phase4-no-std-wasm.md`).
    pub wasm_exports: Vec<String>,
    /// Every callee's machine signature by each name a call spells, built by
    /// `mir::coercion::make_coercions_explicit` (the last lowering step)
    /// from the functions and FFI declarations above. The conversions that
    /// step inserts and every backend's declarations read this one index.
    pub callees: CalleeIndex,
}

/// Phase B-2: one static vtable struct per `(class, mixin)` pair.
/// Spec §B3.
#[derive(Debug, Clone)]
pub struct MirVtable {
    /// Name of the runtime-dispatch mixin (e.g., "Future").
    pub mixin_name: String,
    /// Name of the implementing class (e.g., "TimeSleepFuture").
    pub class_name: String,
    /// Function symbols for each of the mixin's required methods, in
    /// declaration order. Each is the mangled `<ClassName>_<method>`
    /// symbol that codegen has already declared.
    pub method_symbols: Vec<String>,
}

impl MirVtable {
    /// The static symbol name codegen exports for this vtable.
    /// `__rx_vtable_<MixinName>_for_<ClassName>` per spec §B3 example.
    pub fn symbol(&self) -> String {
        format!("__rx_vtable_{}_for_{}", self.mixin_name, self.class_name)
    }
}

/// Phase B-3: one class_info struct per class with any runtime-dispatch
/// mixin includes. Spec §B8.
#[derive(Debug, Clone)]
pub struct MirClassInfo {
    /// Name of the implementing class.
    pub class_name: String,
    /// Vtable symbol names — one entry per runtime-dispatch mixin the
    /// class includes, in mixin-inclusion order (matching
    /// `ClassInfo.runtime_dispatch_includes`). Each is the symbol
    /// returned by `MirVtable::symbol()` for the matching `(class,
    /// mixin)` pair.
    pub vtable_symbols: Vec<String>,
}

impl MirClassInfo {
    /// The static symbol name codegen exports for this class-info.
    pub fn symbol(&self) -> String {
        format!("__rx_classinfo_{}", self.class_name)
    }
}

/// An FFI library declaration for codegen.
#[derive(Debug, Clone)]
pub struct FfiLib {
    /// Library name (e.g., "LibM") or empty for anonymous `lib` blocks.
    pub name: String,
    /// Declared functions.
    pub functions: Vec<FfiFuncDecl>,
}

/// A single FFI function declaration for codegen.
#[derive(Debug, Clone)]
pub struct FfiFuncDecl {
    /// The actual C symbol the linker resolves — what cranelift / LLVM
    /// declares as `Linkage::Import`. When a Ruxen `lib` block carried
    /// `def NAME as "<c-symbol>"(...)`, this is `"<c-symbol>"`; when no
    /// alias was given, it is the Ruxen name (historical default).
    pub name: String,
    /// The Ruxen-side identifier — what call sites spell. Equals `name`
    /// when no alias was given. Used to wire `declared_fns` under BOTH
    /// keys so a MIR `Call { callee: <ruxen-name> }` and a MIR `Call {
    /// callee: <c-symbol> }` (after rewrite at lower time) both resolve.
    pub ruxen_name: String,
    /// Parameter types.
    pub param_types: Vec<crate::hir::types::Ty>,
    /// Return type (None for void).
    pub return_type: Option<crate::hir::types::Ty>,
    /// Whether the function is variadic.
    pub is_variadic: bool,
    /// Whether `param_types[0]` is the method receiver rather than a
    /// declared parameter (see `HirFfiFunc::has_receiver`).
    pub has_receiver: bool,
    /// The declaration spelled its result `-> owned T`: the caller owns
    /// the result outright (`runtime_abi::DeclaredResults`).
    pub returns_owned: bool,
    /// How each entry of `param_types` receives its argument (receiver
    /// first when `has_receiver`); see [`ParamMode`].
    pub param_modes: Vec<ParamMode>,
}

/// How a callee receives one argument, recorded on its declaration
/// ([`MirFunction::param_modes`], [`FfiFuncDecl::param_modes`]) so a pass
/// reading a `Call` learns what the call does to each operand without
/// knowing the callee. Derived in one place from the declared signature
/// (`mir::lower::param_modes`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamMode {
    /// The callee only reads it: a `&T` or `Fn[...]` parameter, a `&self`
    /// receiver, or a Copy value.
    Read,
    /// The callee may write through it: a `&var T` parameter or a
    /// `def var` receiver.
    Mut,
    /// The callee takes ownership: a non-Copy by-value parameter or a
    /// `def consume` receiver.
    Move,
}

/// What lowering knows a value it builds may point into, recorded in
/// [`MirFunction::views`] by the local the value is defined in, where
/// lowering makes it (or, for a call result, once every callee's body
/// exists). The loan check and drop elaboration read it; neither
/// reconstructs it from types or instruction shapes. A local with no entry
/// may point into every place its defining instruction reads: a field read
/// views the field, a payload read the payload, a call result every
/// non-scalar argument (and it holds the loans of every argument it was
/// made from).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum View {
    /// A machine scalar (an integer, float, Bool, Char or unit): it points
    /// nowhere and nothing can point into it or be kept in it.
    Scalar,
    /// Points into nothing it does not own: a fresh value, or the old value
    /// a field store moves out of the field to free it.
    Own,
    /// A closure pair this body builds (it owns the pair): `body` is the
    /// closure body it runs, `captures` the captures box it was given.
    Closure {
        body: String,
        captures: Option<LocalId>,
    },
    /// An address lowering built for an `op=` target that a call then
    /// writes through (`s += t` appends through the slot): it views the
    /// place, and passing it assigns that place rather than mutating it.
    Assignee,
    /// A call result that may point into the arguments at these positions
    /// (a reference result, a callee that may return that parameter, a
    /// runtime result the callee does not allocate fresh).
    Args(Vec<usize>),
}

impl MirFunction {
    /// Record what the value `local` is defined with may point into. A
    /// local defined twice with different facts gets none: it may point
    /// into whatever each definition reads.
    pub fn record_view(&mut self, local: LocalId, view: View) {
        let recorded = self
            .views
            .entry(local)
            .or_insert_with(|| Some(view.clone()));
        if recorded.as_ref() != Some(&view) {
            *recorded = None;
        }
    }

    /// The [`View`] recorded for `local`, if exactly one was.
    pub fn view(&self, local: LocalId) -> Option<&View> {
        self.views.get(&local).and_then(Option::as_ref)
    }
}

impl MirProgram {
    pub fn new() -> Self {
        MirProgram {
            functions: Vec::new(),
            entry: None,
            ffi_libs: Vec::new(),
            vtables: Vec::new(),
            class_infos: Vec::new(),
            wasm_exports: Vec::new(),
            callees: CalleeIndex::new(),
        }
    }
}

impl Default for MirProgram {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Local Variable ─────────────────────────────────────────────────────────

/// A local variable (parameter or let-binding) in a MIR function.
#[derive(Debug, Clone)]
pub struct MirLocal {
    pub id: LocalId,
    pub name: String,
    pub ty: Ty,
    pub mutable: bool,
    /// Who introduced the local. Drop elaboration applies different
    /// ownership rules to compiler temporaries; the name never decides it.
    pub kind: LocalKind,
}

/// Whether a [`MirLocal`] is a source binding or a compiler temporary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalKind {
    /// A parameter, `let` binding or other local the program names.
    User,
    /// An intermediate value the lowerer introduced.
    Temp,
}

/// The symbol linkage codegen gives a [`MirFunction`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirLinkage {
    /// Visible only inside the emitted module.
    Local,
    /// Exported: the process entry point, and helpers C runtime code calls
    /// by symbol (the `<Mixin>_dynamic_<method>` dispatch helpers).
    Export,
}

/// What a synthesized [`MirFunction`] is, set where lowering builds it, so
/// later passes recognise it without reading its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MirFnRole {
    /// Any function with no role below: user code, glue, helpers.
    #[default]
    Plain,
    /// A closure literal's body: parameter 0 is its captures box.
    ClosureBody,
    /// A derived `clone`: reads its receiver and returns a new owner.
    DerivedClone,
    /// An object type's field drop glue: frees the fields an instance owns,
    /// ahead of the instance's own free (`drops::heap_free_call`).
    FieldGlue,
    /// `__ruxen_closure_drop(pair)`: releases one owner of a closure pair
    /// through the pair's drop slot.
    ClosureDrop,
}

/// Drop glue attached to the fresh local that owns a closure pair.
#[derive(Debug, Clone)]
pub struct MirClosureDrop {
    pub local: LocalId,
    pub drop_fn: String,
    /// Direct by-value capture slots whose heap value came from a local in
    /// this enclosing function. Drop elaboration uses the source local to
    /// transfer a returned capture back out before running closure cleanup.
    pub owned_capture_slots: Vec<(usize, LocalId)>,
}

/// A `match` arm that moved an owned payload out of an `Option` / `Result`
/// local into a by-value binding (the borrow checker records the move, so
/// the local is not read again). Drop elaboration frees the local's box at
/// the start of `block` when the frame owns it: not a parameter and not a
/// field read or reference.
#[derive(Debug, Clone)]
pub struct MirPayloadMove {
    pub scrutinee: LocalId,
    pub block: BlockId,
}

// ─── Function ───────────────────────────────────────────────────────────────

/// A single function in MIR form.
#[derive(Debug, Clone)]
pub struct MirFunction {
    pub name: String,
    /// Parameter locals (subset of `locals`, in order).
    pub params: Vec<LocalId>,
    pub return_ty: Ty,
    pub locals: Vec<MirLocal>,
    pub blocks: Vec<BasicBlock>,
    pub entry_block: BlockId,
    /// Per-closure destructor roots. Assign/Copy/Move propagation is resolved
    /// by drop elaboration so only the final, still-owned pair is destroyed.
    pub closure_drops: Vec<MirClosureDrop>,
    /// Arms that moved a payload out of a sum-type local; see
    /// [`MirPayloadMove`].
    pub payload_moves: Vec<MirPayloadMove>,
    /// Source span of the function's definition (its `def` line). `None` for
    /// compiler-synthesized functions (derives, drop glue, dynamic-dispatch
    /// helpers) that have no source location. Consumed by the LLVM backend's
    /// debug-info emitter (P0.14) to build the function's `DISubprogram` line;
    /// a `None` here means the function gets no DWARF scope at all.
    pub def_span: Option<Span>,
    /// Symbol linkage. `Local` unless lowering marks the function exported.
    pub linkage: MirLinkage,
    /// True only for the program's `main`, which takes the C entry ABI.
    pub is_entry: bool,
    /// What this function is when lowering synthesized it.
    pub role: MirFnRole,
    /// The field drop glue this body calls, by callee, with the object type
    /// each glue frees. Recorded by the one emitter
    /// (`drops::heap_free_call`), so later passes recognise a glue call by
    /// this record (and define the glue from the recorded type) instead of
    /// spelling its name again.
    pub field_glue_calls: std::collections::BTreeMap<String, Ty>,
    /// The element glue (`RUXEN_ELEM_OBJ`) whose address this body passes in
    /// an element descriptor, by glue name, with the object type it frees
    /// and copies. Recorded wherever a descriptor is emitted
    /// (`drops::record_elem_glue`) and read by `synthesize_elem_glue`, so no
    /// pass recovers a type from a glue name.
    pub elem_glue_types: std::collections::BTreeMap<String, Ty>,
    /// How each of `params` receives its argument, index-aligned; empty for
    /// a compiler-synthesized function (every argument then counts as
    /// [`ParamMode::Read`]).
    pub param_modes: Vec<ParamMode>,
    /// Lowered from the prepended standard library (an item before
    /// `HirProgram::prelude_item_count`, or a closure inside one): trusted
    /// code that source-level checks skip, as the HIR borrow check does.
    pub from_prelude: bool,
    /// What lowering knows the value of a local may point into, by the
    /// local an instruction defines; see [`View`]. A local with no entry,
    /// or `None` (definitions recorded different facts), may point into
    /// whatever its defining instruction reads.
    pub views: std::collections::HashMap<LocalId, Option<View>>,
    /// Counter used by `new_temp()` to generate unique names.
    next_temp: u32,
}

impl MirFunction {
    /// Create a new function with a single entry block already inserted.
    pub fn new(name: impl Into<String>, return_ty: Ty) -> Self {
        let entry_block = BasicBlock::new(0);
        MirFunction {
            name: name.into(),
            params: Vec::new(),
            return_ty,
            locals: Vec::new(),
            blocks: vec![entry_block],
            entry_block: 0,
            closure_drops: Vec::new(),
            payload_moves: Vec::new(),
            def_span: None,
            linkage: MirLinkage::Local,
            is_entry: false,
            role: MirFnRole::Plain,
            field_glue_calls: std::collections::BTreeMap::new(),
            elem_glue_types: std::collections::BTreeMap::new(),
            param_modes: Vec::new(),
            from_prelude: false,
            views: std::collections::HashMap::new(),
            next_temp: 0,
        }
    }

    /// Create a function from pre-built parts (useful for tests and codegen).
    pub fn with_parts(
        name: String,
        params: Vec<LocalId>,
        return_ty: Ty,
        locals: Vec<MirLocal>,
        blocks: Vec<BasicBlock>,
        entry_block: BlockId,
    ) -> Self {
        MirFunction {
            name,
            params,
            return_ty,
            locals,
            blocks,
            entry_block,
            closure_drops: Vec::new(),
            payload_moves: Vec::new(),
            def_span: None,
            linkage: MirLinkage::Local,
            is_entry: false,
            role: MirFnRole::Plain,
            field_glue_calls: std::collections::BTreeMap::new(),
            elem_glue_types: std::collections::BTreeMap::new(),
            param_modes: Vec::new(),
            from_prelude: false,
            views: std::collections::HashMap::new(),
            next_temp: 0,
        }
    }

    /// Allocate a new local with the given name, type, and mutability.
    /// Returns the `LocalId` of the newly created local.
    pub fn new_local(&mut self, name: impl Into<String>, ty: Ty, mutable: bool) -> LocalId {
        self.new_local_of_kind(name, ty, mutable, LocalKind::User)
    }

    /// Allocate a new local of the given [`LocalKind`].
    pub fn new_local_of_kind(
        &mut self,
        name: impl Into<String>,
        ty: Ty,
        mutable: bool,
        kind: LocalKind,
    ) -> LocalId {
        let id = self.locals.len() as LocalId;
        self.locals.push(MirLocal {
            id,
            name: name.into(),
            ty,
            mutable,
            kind,
        });
        id
    }

    /// Allocate a fresh compiler-generated temporary local
    /// ([`LocalKind::Temp`]). Names are of the form `_t0`, `_t1`, etc. and
    /// are for display only.
    pub fn new_temp(&mut self, ty: Ty) -> LocalId {
        let idx = self.next_temp;
        self.next_temp += 1;
        let name = format!("_t{}", idx);
        self.new_local_of_kind(name, ty, false, LocalKind::Temp)
    }

    /// Append a new (empty) basic block to the function.
    /// Returns the `BlockId` of the newly created block.
    pub fn new_block(&mut self) -> BlockId {
        let id = self.blocks.len();
        self.blocks.push(BasicBlock::new(id));
        id
    }
}

// ─── Basic Block ────────────────────────────────────────────────────────────

/// A straight-line sequence of instructions ending with a terminator.
#[derive(Debug, Clone)]
pub struct BasicBlock {
    pub id: BlockId,
    pub instructions: Vec<MirInst>,
    /// Per-instruction source spans, positionally aligned with
    /// `instructions`, for DWARF line info (P0.14). An entry is `None` for a
    /// compiler-synthesized instruction (drop glue, zero-init) that has no
    /// source location.
    ///
    /// Alignment invariant: this vector stays index-aligned with
    /// `instructions` for every block built through `Lowerer::emit` (user
    /// function bodies), because both grow together via [`Self::push_inst`] /
    /// [`Self::insert_inst`] / [`Self::remove_inst`]. Compiler-synthesized
    /// functions (derive bodies, dispatch/drop helpers) push straight onto
    /// `instructions` and leave this empty; those functions carry no
    /// `MirFunction::def_span`, so the debug emitter skips them and never
    /// reads their spans. Reads go through [`Self::span_at`], which tolerates
    /// a shorter/empty vector, so a mismatch degrades to "no line info", never
    /// a panic or a wrong-program bug.
    pub inst_spans: Vec<Option<Span>>,
    pub terminator: Terminator,
}

impl BasicBlock {
    pub fn new(id: BlockId) -> Self {
        BasicBlock {
            id,
            instructions: Vec::new(),
            inst_spans: Vec::new(),
            terminator: Terminator::Unreachable,
        }
    }

    /// Append an instruction, recording its source span in lockstep so
    /// `inst_spans` stays index-aligned with `instructions`.
    pub fn push_inst(&mut self, inst: MirInst, span: Option<Span>) {
        self.instructions.push(inst);
        self.inst_spans.push(span);
    }

    /// Insert an instruction at `idx`, keeping `inst_spans` aligned. Only
    /// called on user-function blocks (loop-body zero-init), which are already
    /// span-aligned; the `min` clamp is defensive against an unexpected caller.
    pub fn insert_inst(&mut self, idx: usize, inst: MirInst, span: Option<Span>) {
        self.instructions.insert(idx, inst);
        let at = idx.min(self.inst_spans.len());
        self.inst_spans.insert(at, span);
    }

    /// Remove the instruction at `idx`, keeping `inst_spans` aligned.
    pub fn remove_inst(&mut self, idx: usize) -> MirInst {
        if idx < self.inst_spans.len() {
            self.inst_spans.remove(idx);
        }
        self.instructions.remove(idx)
    }

    /// The source span recorded for the instruction at `idx`, if any.
    /// Defensive against a shorter/empty `inst_spans` (synthesized blocks).
    pub fn span_at(&self, idx: usize) -> Option<Span> {
        self.inst_spans.get(idx).cloned().flatten()
    }
}

// ─── Instructions ───────────────────────────────────────────────────────────

/// A single MIR instruction (three-address code).
#[derive(Debug, Clone)]
pub enum MirInst {
    /// `dest = value`
    Assign { dest: LocalId, value: MirValue },
    /// `dest = lhs op rhs`
    BinOp {
        dest: LocalId,
        op: BinOp,
        lhs: MirValue,
        rhs: MirValue,
    },
    /// `dest = -operand`
    Negate { dest: LocalId, operand: MirValue },
    /// `dest = !operand`
    Not { dest: LocalId, operand: MirValue },
    /// `dest = lhs cmp rhs`  (result is Bool)
    Compare {
        dest: LocalId,
        op: CmpOp,
        lhs: MirValue,
        rhs: MirValue,
    },
    /// `dest = callee(args...)`
    Call {
        dest: Option<LocalId>,
        callee: String,
        args: Vec<MirValue>,
    },
    /// `dest = alloc(ty)`  — heap allocation
    Alloc {
        dest: LocalId,
        ty: Ty,
        /// Pre-computed allocation size in bytes (from layout_of at lowering time).
        /// If 0, the codegen falls back to `simple_type_size`.
        size: usize,
    },
    /// `dest = stack_alloc(ty)`  — stack slot
    StackAlloc { dest: LocalId, ty: Ty },
    /// `dest = base.field_index`
    GetField {
        dest: LocalId,
        base: LocalId,
        field_index: usize,
    },
    /// `dest = &base.field_index` — the address of field slot
    /// `field_index` of the object (or captures box) `base` points at: a
    /// slot-addressed `&var` of a field or of a closure's own capture,
    /// pointing at the real storage (no copy, no store-back). `dest` is a
    /// `RefMut(T)` word and owns nothing; `base` must outlive it.
    FieldAddr {
        dest: LocalId,
        base: LocalId,
        field_index: usize,
    },
    /// `base.field_index = value`
    SetField {
        base: LocalId,
        field_index: usize,
        value: MirValue,
    },
    /// Write the discriminant tag of an enum local.
    SetTag { dest: LocalId, tag: u32 },
    /// `dest = enum_local.tag`
    GetTag { dest: LocalId, src: LocalId },
    /// `dest = enum_local.payload` (cast to the payload type)
    GetPayload { dest: LocalId, src: LocalId, ty: Ty },
    /// `dest = &src`  — immutable borrow
    Ref { dest: LocalId, src: LocalId },
    /// `dest = &mut src`  — mutable borrow
    RefMut { dest: LocalId, src: LocalId },
    /// `dest = copy src`  — explicit copy (for Copy types)
    Copy { dest: LocalId, src: LocalId },
    /// `dest = move src`  — explicit move (src is invalidated)
    Move { dest: LocalId, src: LocalId },
    /// Run the destructor for `local` (inserted by drop elaboration).
    Drop { local: LocalId },
    /// `dest = "string_data"`
    StringLiteral { dest: LocalId, value: String },
    /// `dest = &func_name` — get address of a named function as a pointer
    FuncAddr { dest: LocalId, func_name: String },
    /// `dest = &data_sym` — get address of a static read-only data
    /// section as an i64 pointer. Phase B-5: used to write a class's
    /// `__rx_classinfo_<Class>` symbol into the class_info_ptr header
    /// slot at every allocation site of a runtime-dispatch class. The
    /// data symbol must already be defined elsewhere in the MIR (today
    /// only mixin-vtables emit such sections — `__rx_vtable_*` and
    /// `__rx_classinfo_*`). Spec: docs/specs/types/mixin_vtables.spec.md §B5.
    DataAddr { dest: LocalId, data_sym: String },
    /// `dest = call_indirect(callee_ptr, args...)` — indirect function call
    CallIndirect {
        dest: Option<LocalId>,
        callee: LocalId,
        args: Vec<MirValue>,
        /// How the callee takes each of `args`, by position: lowering
        /// derives them from the callee's declared signature
        /// (`param_modes::param_mode`); where the signature is unknown,
        /// `param_modes::unknown_param_mode`, never a bare `Read`.
        modes: Vec<ParamMode>,
        /// The closure pair `callee` was read out of, when this calls a
        /// closure (its captures box is `args[0]`).
        pair: Option<LocalId>,
    },
    /// `dest = bit-reinterpret(src)` — reinterpret a float's storage bits
    /// as an integer of the same width (f64→i64, f32→i32) without value
    /// conversion. Used by the derived `Hashable` impl so float fields can
    /// participate in the integer FNV mix (`bxor`/`mul`) — feeding a raw
    /// `f64` into an integer `BinOp` otherwise fails Cranelift verification.
    /// `dest`'s declared MIR type selects the integer width.
    ///
    /// Also the OUTBOUND edge of the closure word-ABI (B01): a closure whose
    /// declared result is a float returns the bit pattern in a uniform word,
    /// because the generic body that invokes it through an erased `Fn` box
    /// can only receive words.
    FloatToBits { dest: LocalId, src: LocalId },
    /// `dest = float-from-bits(src)` — the exact inverse of `FloatToBits`:
    /// recover a float from the uniform word holding its bit pattern, with
    /// no numeric conversion. `dest`'s declared MIR type selects the float
    /// width. Emitted by the closure word-ABI (B01) on the INBOUND edge of a
    /// float closure parameter, and when a caller reads a float closure
    /// result back out of the word.
    BitsToFloat { dest: LocalId, src: LocalId },
    /// `dest = convert(src)` — move a value into a local of another
    /// machine class by the operation `mir::abi` decided (an integer
    /// extension, a numeric conversion, a word reinterpretation, ...).
    /// Inserted only by `mir::coercion::make_coercions_explicit`, the last
    /// step of lowering, so no pass before codegen sees one.
    Convert {
        dest: LocalId,
        src: MirValue,
        coercion: Coercion,
    },
    /// No operation — placeholder / removed instruction.
    Nop,
}

impl MirInst {
    /// The local this instruction defines (overwrites as a whole), if any.
    /// `SetField` and `SetTag` update a value in place, so they define
    /// nothing and read the value they update.
    pub fn dest(&self) -> Option<LocalId> {
        match self {
            MirInst::Assign { dest, .. }
            | MirInst::BinOp { dest, .. }
            | MirInst::Negate { dest, .. }
            | MirInst::Not { dest, .. }
            | MirInst::Compare { dest, .. }
            | MirInst::Alloc { dest, .. }
            | MirInst::StackAlloc { dest, .. }
            | MirInst::GetField { dest, .. }
            | MirInst::FieldAddr { dest, .. }
            | MirInst::GetTag { dest, .. }
            | MirInst::GetPayload { dest, .. }
            | MirInst::Ref { dest, .. }
            | MirInst::RefMut { dest, .. }
            | MirInst::Copy { dest, .. }
            | MirInst::Move { dest, .. }
            | MirInst::StringLiteral { dest, .. }
            | MirInst::FuncAddr { dest, .. }
            | MirInst::FloatToBits { dest, .. }
            | MirInst::BitsToFloat { dest, .. }
            | MirInst::Convert { dest, .. }
            | MirInst::DataAddr { dest, .. } => Some(*dest),
            MirInst::Call { dest, .. } | MirInst::CallIndirect { dest, .. } => *dest,
            MirInst::SetField { .. }
            | MirInst::SetTag { .. }
            | MirInst::Drop { .. }
            | MirInst::Nop => None,
        }
    }

    /// The local [`Self::dest`] names, to retarget the instruction.
    pub fn dest_mut(&mut self) -> Option<&mut LocalId> {
        match self {
            MirInst::Assign { dest, .. }
            | MirInst::BinOp { dest, .. }
            | MirInst::Negate { dest, .. }
            | MirInst::Not { dest, .. }
            | MirInst::Compare { dest, .. }
            | MirInst::Alloc { dest, .. }
            | MirInst::StackAlloc { dest, .. }
            | MirInst::GetField { dest, .. }
            | MirInst::FieldAddr { dest, .. }
            | MirInst::GetTag { dest, .. }
            | MirInst::GetPayload { dest, .. }
            | MirInst::Ref { dest, .. }
            | MirInst::RefMut { dest, .. }
            | MirInst::Copy { dest, .. }
            | MirInst::Move { dest, .. }
            | MirInst::StringLiteral { dest, .. }
            | MirInst::FuncAddr { dest, .. }
            | MirInst::FloatToBits { dest, .. }
            | MirInst::BitsToFloat { dest, .. }
            | MirInst::Convert { dest, .. }
            | MirInst::DataAddr { dest, .. } => Some(dest),
            MirInst::Call { dest, .. } | MirInst::CallIndirect { dest, .. } => dest.as_mut(),
            MirInst::SetField { .. }
            | MirInst::SetTag { .. }
            | MirInst::Drop { .. }
            | MirInst::Nop => None,
        }
    }

    /// Every local this instruction reads, in operand order: values,
    /// bases, sources, an indirect callee, and the value a `SetField`,
    /// `SetTag` or `Drop` acts on.
    pub fn read_locals(&self) -> Vec<LocalId> {
        let values = |values: &[&MirValue]| -> Vec<LocalId> {
            values.iter().filter_map(|value| value.local()).collect()
        };
        match self {
            MirInst::Assign { value, .. } | MirInst::Convert { src: value, .. } => values(&[value]),
            MirInst::BinOp { lhs, rhs, .. } | MirInst::Compare { lhs, rhs, .. } => {
                values(&[lhs, rhs])
            }
            MirInst::Negate { operand, .. } | MirInst::Not { operand, .. } => values(&[operand]),
            MirInst::Call { args, .. } => args.iter().filter_map(MirValue::local).collect(),
            MirInst::CallIndirect { callee, args, .. } => std::iter::once(*callee)
                .chain(args.iter().filter_map(MirValue::local))
                .collect(),
            MirInst::GetField { base, .. } | MirInst::FieldAddr { base, .. } => vec![*base],
            MirInst::SetField { base, value, .. } => {
                std::iter::once(*base).chain(value.local()).collect()
            }
            MirInst::SetTag { dest, .. } => vec![*dest],
            MirInst::GetTag { src, .. }
            | MirInst::GetPayload { src, .. }
            | MirInst::Ref { src, .. }
            | MirInst::RefMut { src, .. }
            | MirInst::Copy { src, .. }
            | MirInst::Move { src, .. }
            | MirInst::FloatToBits { src, .. }
            | MirInst::BitsToFloat { src, .. } => vec![*src],
            MirInst::Drop { local } => vec![*local],
            MirInst::Alloc { .. }
            | MirInst::StackAlloc { .. }
            | MirInst::StringLiteral { .. }
            | MirInst::FuncAddr { .. }
            | MirInst::DataAddr { .. }
            | MirInst::Nop => Vec::new(),
        }
    }
}

// ─── Comparison Operator ─────────────────────────────────────────────────────

/// Comparison operators used by `MirInst::Compare`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
}

// ─── Terminator ──────────────────────────────────────────────────────────────

/// The last instruction of a basic block — controls where control flow goes.
#[derive(Debug, Clone)]
pub enum Terminator {
    /// Return the given value (or `Unit` if omitted) from the function.
    Return(Option<MirValue>),
    /// Unconditional branch to a block.
    Goto(BlockId),
    /// Conditional branch: if `cond` is true go to `then_block`, else `else_block`.
    Branch {
        cond: MirValue,
        then_block: BlockId,
        else_block: BlockId,
    },
    /// Jump table: `target = targets[discriminant]`, fallback to `otherwise`.
    Switch {
        value: MirValue,
        targets: Vec<(i64, BlockId)>,
        otherwise: BlockId,
    },
    /// This block is statically unreachable (e.g. after `Never`-typed expression).
    Unreachable,
}

impl Terminator {
    /// The blocks control may reach next, in terminator order.
    pub fn successors(&self) -> Vec<BlockId> {
        match self {
            Terminator::Goto(target) => vec![*target],
            Terminator::Branch {
                then_block,
                else_block,
                ..
            } => vec![*then_block, *else_block],
            Terminator::Switch {
                targets, otherwise, ..
            } => targets
                .iter()
                .map(|(_, target)| *target)
                .chain(std::iter::once(*otherwise))
                .collect(),
            Terminator::Return(_) | Terminator::Unreachable => Vec::new(),
        }
    }

    /// The local this terminator reads, if any.
    pub fn read_local(&self) -> Option<LocalId> {
        match self {
            Terminator::Return(value) => value.as_ref().and_then(MirValue::local),
            Terminator::Branch { cond: value, .. } | Terminator::Switch { value, .. } => {
                value.local()
            }
            Terminator::Goto(_) | Terminator::Unreachable => None,
        }
    }
}

// ─── Values ──────────────────────────────────────────────────────────────────

/// An r-value operand used inside instructions and terminators.
#[derive(Debug, Clone)]
pub enum MirValue {
    /// A compile-time constant.
    Literal(Literal),
    /// Read the value of a local variable.
    Use(LocalId),
    /// The unit value `()`.
    Unit,
}

impl MirValue {
    /// The local this operand reads, if it reads one.
    pub fn local(&self) -> Option<LocalId> {
        match self {
            MirValue::Use(local) => Some(*local),
            MirValue::Literal(_) | MirValue::Unit => None,
        }
    }
}

// ─── Literals ────────────────────────────────────────────────────────────────

/// Compile-time constant values.
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Int(i64),
    Float(f64),
    Bool(bool),
    Char(char),
    String(String),
}
