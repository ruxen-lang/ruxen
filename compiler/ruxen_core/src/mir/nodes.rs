//! MIR (Mid-level Intermediate Representation) node definitions.
//!
//! MIR is a control-flow graph (CFG) based IR that sits between HIR and
//! machine code. Each function is a set of basic blocks connected by
//! terminators. Instructions within a block are simple three-address code.

use crate::hir::types::Ty;
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
    /// Linker flags (e.g., "-lm").
    pub link_flags: Vec<String>,
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
}

impl MirProgram {
    pub fn new() -> Self {
        MirProgram {
            functions: Vec::new(),
            entry: None,
            ffi_libs: Vec::new(),
            vtables: Vec::new(),
            class_infos: Vec::new(),
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
            next_temp: 0,
        }
    }

    /// Allocate a new local with the given name, type, and mutability.
    /// Returns the `LocalId` of the newly created local.
    pub fn new_local(&mut self, name: impl Into<String>, ty: Ty, mutable: bool) -> LocalId {
        let id = self.locals.len() as LocalId;
        self.locals.push(MirLocal {
            id,
            name: name.into(),
            ty,
            mutable,
        });
        id
    }

    /// Allocate a fresh compiler-generated temporary local.
    /// Names are of the form `_t0`, `_t1`, etc.
    pub fn new_temp(&mut self, ty: Ty) -> LocalId {
        let idx = self.next_temp;
        self.next_temp += 1;
        let name = format!("_t{}", idx);
        self.new_local(name, ty, false)
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
    pub terminator: Terminator,
}

impl BasicBlock {
    pub fn new(id: BlockId) -> Self {
        BasicBlock {
            id,
            instructions: Vec::new(),
            terminator: Terminator::Unreachable,
        }
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
    },
    /// No operation — placeholder / removed instruction.
    Nop,
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
