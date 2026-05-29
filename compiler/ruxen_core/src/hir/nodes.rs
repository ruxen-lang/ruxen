//! HIR (High-level Intermediate Representation) node definitions.
//!
//! HIR nodes mirror AST nodes but with semantic information attached:
//! - Every expression has a resolved `Ty`
//! - Names are resolved to `DefId`s (not strings)
//! - Syntactic sugar is desugared
//! - Copy/Move annotations on all value transfers

use crate::hir::types::{MixinRef, MoveSemantics, Ty};
use crate::lexer::token::Span;
use crate::parser::ast::{BinOp, UnaryOp, Visibility};

/// Unique identifier for every definition in the program.
pub type DefId = u32;

/// A sentinel DefId indicating an unresolved reference (for error recovery).
pub const UNRESOLVED_DEF: DefId = u32::MAX;

// ─── HIR Program ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HirProgram {
    pub items: Vec<HirItem>,
    pub span: Span,
    /// #06.8 Phase 2: FFI library declarations harvested from top-level
    /// `lib "X" ... end` and `extern "C" ... end` blocks. Each entry
    /// carries the link name plus every declared function with its
    /// Ruxen-side name AND optional `c_symbol` alias. MIR lowering uses
    /// this to populate `MirProgram::ffi_libs`, and call-site lowering
    /// uses it to rewrite a Ruxen name → its C symbol when an alias is
    /// present. Top-level `Lib`/`Extern` items themselves do not become
    /// `HirItem`s (they emit no executable code), so we surface them
    /// here as a side-channel on the program.
    pub ffi_libs: Vec<HirFfiLib>,
}

/// #06.8 Phase 2: a single FFI library declaration carried on `HirProgram`.
#[derive(Debug, Clone)]
pub struct HirFfiLib {
    /// Link name (e.g. `"m"` for `lib "m" ... end`). Empty for anonymous
    /// `extern "C"` blocks.
    pub name: String,
    /// Linker flags collected from `@[link]` attributes on this block.
    pub link_flags: Vec<String>,
    pub functions: Vec<HirFfiFunc>,
}

/// #06.8 Phase 2: a single FFI function declaration carried on `HirFfiLib`.
#[derive(Debug, Clone)]
pub struct HirFfiFunc {
    /// The Ruxen-side identifier — what callers spell at the call site.
    pub ruxen_name: String,
    /// Optional C-symbol alias. `None` means `ruxen_name` *is* the linked
    /// C symbol (historical behaviour). `Some(c)` means the linker
    /// resolves the call to symbol `c`.
    pub c_symbol: Option<String>,
    /// #06.8 Phase 3b: optional parent class / mixin name. When `Some`,
    /// this FFI decl was authored inside a class- or mixin-body `lib`
    /// block and the MIR bridge must register a `ParentName_ruxen_name`
    /// alias-map entry so `MethodCall` dispatch (which mangles to
    /// `ClassName_method`) routes to the C symbol. `None` is the
    /// historical top-level `lib`/`extern` case.
    pub parent_type: Option<String>,
    pub param_types: Vec<crate::hir::types::Ty>,
    pub return_type: Option<crate::hir::types::Ty>,
    pub is_variadic: bool,
}

#[derive(Debug, Clone)]
pub enum HirItem {
    Module(HirModule),
    Class(HirClassDef),
    Struct(HirStructDef),
    Enum(HirEnumDef),
    /// `mixin M ... end` — was `Trait` pre-Ruby-naming migration.
    /// See docs/specs/syntax/ruby-naming.spec.md.
    Mixin(HirMixinDef),
    Impl(HirImplBlock),
    Function(HirFuncDef),
    TypeAlias(HirTypeAlias),
    Newtype(HirNewtype),
    Const(HirConst),
}

// ─── Expressions ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HirExpr {
    pub kind: HirExprKind,
    pub ty: Ty,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum HirExprKind {
    // Literals
    IntLiteral(i64),
    FloatLiteral(f64),
    StringLiteral(String),
    BoolLiteral(bool),
    CharLiteral(char),
    UnitLiteral,
    /// `/pat/flags` regex literal (std.regex). Typed as
    /// `Ty::Class { name: "Regex" }`; MIR lowers it to a
    /// `ruxen_regex_compile_const(pattern, flags)` call.
    RegexLiteral {
        pattern: String,
        flags: String,
    },

    /// Resolved variable reference
    VarRef(DefId),

    /// Field access with resolved field index
    FieldAccess {
        object: Box<HirExpr>,
        field_name: String,
        field_idx: usize,
    },

    /// Method call with resolved method DefId
    MethodCall {
        object: Box<HirExpr>,
        method: DefId,
        method_name: String,
        generic_args: Vec<Ty>,
        args: Vec<HirExpr>,
        /// Trailing block argument (desugared closure)
        block: Option<Box<HirExpr>>,
    },

    /// Resolved function call
    FnCall {
        callee: DefId,
        callee_name: String,
        args: Vec<HirExpr>,
    },

    /// Binary operation
    BinaryOp {
        op: BinOp,
        left: Box<HirExpr>,
        right: Box<HirExpr>,
    },

    /// Unary operation
    UnaryOp {
        op: UnaryOp,
        operand: Box<HirExpr>,
    },

    /// Borrow expression: `&x` or `&mut x`
    Borrow {
        mutable: bool,
        expr: Box<HirExpr>,
    },

    /// Block expression (statements + optional tail expression)
    Block(Vec<HirStatement>, Option<Box<HirExpr>>),

    /// If expression (all branches typed)
    If {
        cond: Box<HirExpr>,
        then_branch: Box<HirExpr>,
        else_branch: Option<Box<HirExpr>>,
    },

    /// Match expression (desugared from `if let`, `?.`, etc.)
    Match {
        scrutinee: Box<HirExpr>,
        arms: Vec<HirMatchArm>,
    },

    /// Loop (while/for/loop all desugar to this + break/continue)
    Loop {
        body: Box<HirExpr>,
    },

    /// While loop (kept distinct for clarity)
    While {
        condition: Box<HirExpr>,
        body: Box<HirExpr>,
    },

    /// For loop (desugars to iterator protocol, but kept for clarity)
    For {
        binding: DefId,
        binding_name: String,
        iterable: Box<HirExpr>,
        body: Box<HirExpr>,
        /// For tuple destructuring patterns like `for (i, result) in ...`,
        /// stores the sub-binding DefIds. Empty for simple bindings.
        tuple_bindings: Vec<(DefId, String)>,
    },

    /// Assignment
    Assign {
        target: Box<HirExpr>,
        value: Box<HirExpr>,
        semantics: MoveSemantics,
    },

    /// Compound assignment (+=, -=, etc.)
    CompoundAssign {
        target: Box<HirExpr>,
        op: BinOp,
        value: Box<HirExpr>,
    },

    /// Return from function
    Return(Option<Box<HirExpr>>),

    /// Break from loop
    Break(Option<Box<HirExpr>>),

    /// Continue to next loop iteration
    Continue,

    /// Closure expression
    Closure {
        params: Vec<HirClosureParam>,
        body: Box<HirExpr>,
        captures: Vec<Capture>,
        is_async: bool,
        is_move: bool,
    },

    /// Struct/class construction
    Construct {
        type_def: DefId,
        type_name: String,
        fields: Vec<(String, HirExpr)>,
    },

    /// Enum variant construction
    EnumVariant {
        type_def: DefId,
        type_name: String,
        variant_name: String,
        variant_idx: usize,
        fields: Vec<(String, HirExpr)>,
    },

    /// Tuple literal
    Tuple(Vec<HirExpr>),

    /// Index access: `obj[idx]`
    Index {
        object: Box<HirExpr>,
        index: Box<HirExpr>,
    },

    /// Type cast: `expr as Type`
    Cast {
        expr: Box<HirExpr>,
        target: Ty,
    },

    /// Array literal
    ArrayLiteral(Vec<HirExpr>),

    /// Map literal: `{ k => v, k => v, ... }` (ruby-naming.spec.md §10a).
    MapLiteral(Vec<(HirExpr, HirExpr)>),

    /// Array fill: `[value; count]`
    ArrayFill {
        value: Box<HirExpr>,
        count: usize,
    },

    /// Range: `start..end` or `start..=end`
    Range {
        start: Option<Box<HirExpr>>,
        end: Option<Box<HirExpr>>,
        inclusive: bool,
    },

    /// String interpolation (desugared to format-like calls)
    Interpolation {
        parts: Vec<HirInterpolationPart>,
    },

    /// Macro call (vec![], hash!{}) — treated as special form
    MacroCall {
        name: String,
        args: Vec<HirExpr>,
    },

    /// Unsafe block — safety is enforced at type-check time;
    /// codegen is identical to a regular block.
    UnsafeBlock(Vec<HirStatement>, Option<Box<HirExpr>>),

    /// Null literal — assignable to any raw pointer type.
    NullLiteral,

    /// Error placeholder for recovery
    Error,
}

/// A part of an interpolated string.
#[derive(Debug, Clone)]
pub enum HirInterpolationPart {
    Literal(String),
    /// Phase 2 #06.B: interpolated expression with optional format
    /// spec (default = bare `"#{x}"`). The spec is captured at lex
    /// time and threaded through to MIR for Phase C (`:?` debug
    /// dispatch) and Phase D (Display routing with width/precision).
    Expr {
        expr: HirExpr,
        spec: crate::lexer::token::FormatSpec,
    },
}

// ─── Match Arms ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HirMatchArm {
    pub pattern: HirPattern,
    pub guard: Option<Box<HirExpr>>,
    pub body: Box<HirExpr>,
    pub span: Span,
}

// ─── Patterns ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum HirPattern {
    /// Bind to a variable
    Binding {
        def_id: DefId,
        name: String,
        mutable: bool,
        span: Span,
    },
    /// Wildcard `_`
    Wildcard { span: Span },
    /// Literal pattern
    Literal { expr: Box<HirExpr>, span: Span },
    /// Tuple destructuring
    Tuple {
        elements: Vec<HirPattern>,
        span: Span,
    },
    /// Enum variant pattern
    Enum {
        type_def: DefId,
        variant_idx: usize,
        variant_name: String,
        fields: Vec<HirPattern>,
        span: Span,
    },
    /// Struct destructuring
    Struct {
        type_def: DefId,
        fields: Vec<(String, HirPattern)>,
        rest: bool,
        span: Span,
    },
    /// Or-pattern: `a | b`
    Or {
        patterns: Vec<HirPattern>,
        span: Span,
    },
    /// Reference pattern: `ref x`, `ref var x`
    Ref {
        mutable: bool,
        name: String,
        def_id: DefId,
        span: Span,
    },
    /// Rest pattern: `..`
    Rest { span: Span },
}

// ─── Statements ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum HirStatement {
    /// `let [mut] name [: Type] = value`
    Let {
        def_id: DefId,
        pattern: HirPattern,
        ty: Ty,
        value: Option<HirExpr>,
        mutable: bool,
        span: Span,
    },
    /// Expression statement (value discarded)
    Expr(HirExpr),
}

// ─── Function Parameters ────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HirParam {
    pub def_id: DefId,
    pub name: String,
    pub ty: Ty,
    pub auto_assign: bool,
    pub default: Option<crate::parser::ast::Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct HirClosureParam {
    pub def_id: DefId,
    pub name: String,
    pub ty: Ty,
    pub span: Span,
}

/// Describes what a closure captures from its environment.
/// A value captured by a closure.
///
/// `ty` is a SNAPSHOT taken at closure-resolve time. For captures of
/// values whose type wasn't pinned yet (typeck runs AFTER resolve, so
/// loop-bound or shadowed locals routinely have `Ty::Infer(_)` at
/// capture time), this field stays stale forever — re-running typeck
/// against `def_id` updates the symbol table but not this field.
/// Memory: `project_ruxen_closure_capture_ty_stale.md`.
///
/// Consumers that need the current ty (borrow-check Send/Copy/mut-ref
/// queries, method-resolver dispatch) MUST use [`Self::current_ty`]
/// instead of reading `ty` directly — the helper re-fetches from the
/// symbol table and falls back to the snapshot only when the def has
/// no live entry.
#[derive(Debug, Clone)]
pub struct Capture {
    pub def_id: DefId,
    pub name: String,
    pub by_move: bool,
    pub ty: Ty,
}

impl Capture {
    /// Re-fetch the capture's type from the symbol table. Consumers
    /// that run AFTER typeck should always call this rather than read
    /// `self.ty` (which is the resolve-time snapshot — see struct
    /// docs).
    pub fn current_ty(&self, symbols: &crate::resolve::symbols::SymbolTable) -> Ty {
        symbols
            .def_ty(self.def_id)
            .unwrap_or_else(|| self.ty.clone())
    }
}

// ─── Self Mode ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HirSelfMode {
    /// `&self` — immutable borrow
    Ref,
    /// `&mut self` or `mut` method
    RefMut,
    /// `consume self` — takes ownership
    Consuming,
}

// ─── Function Definition ────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HirFuncDef {
    pub def_id: DefId,
    pub name: String,
    pub visibility: Visibility,
    pub is_async: bool,
    pub self_mode: Option<HirSelfMode>,
    pub is_class_method: bool,
    pub generic_params: Vec<HirGenericParam>,
    pub params: Vec<HirParam>,
    pub return_ty: Ty,
    pub body: Box<HirExpr>,
    pub doc_comments: Vec<String>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct HirGenericParam {
    pub name: String,
    pub bounds: Vec<MixinRef>,
    pub span: Span,
}

// ─── Class Definition ───────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HirClassDef {
    pub def_id: DefId,
    pub name: String,
    pub generic_params: Vec<HirGenericParam>,
    pub parent: Option<DefId>,
    pub fields: Vec<HirFieldDef>,
    pub methods: Vec<HirFuncDef>,
    pub impl_blocks: Vec<HirImplBlock>,
    pub derive_traits: Vec<String>,
    pub doc_comments: Vec<String>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct HirFieldDef {
    pub def_id: DefId,
    pub name: String,
    pub ty: Ty,
    pub visibility: Visibility,
    pub index: usize,
    pub span: Span,
}

// ─── Struct Definition ──────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HirStructDef {
    pub def_id: DefId,
    pub name: String,
    pub generic_params: Vec<HirGenericParam>,
    pub fields: Vec<HirFieldDef>,
    /// Inline methods defined in the struct body
    /// (ruby-naming.spec.md §3.4a).
    pub methods: Vec<HirFuncDef>,
    /// Inline `include Mixin` directives in the struct body
    /// (ruby-naming.spec.md §3.4a).
    pub impl_blocks: Vec<HirImplBlock>,
    pub derive_traits: Vec<String>,
    /// In-body `layout <kind>` directive args (ruby-naming.spec.md §3.5),
    /// e.g. `["c"]`, `["packed"]`, `["transparent"]`.
    pub layout: Vec<String>,
    pub doc_comments: Vec<String>,
    pub span: Span,
}

// ─── Enum Definition ────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HirEnumDef {
    pub def_id: DefId,
    pub name: String,
    pub generic_params: Vec<HirGenericParam>,
    pub variants: Vec<HirVariant>,
    /// Inline methods defined in the enum body
    /// (ruby-naming.spec.md §3.4a).
    pub methods: Vec<HirFuncDef>,
    /// Inline `include Mixin` directives in the enum body
    /// (ruby-naming.spec.md §3.4a).
    pub impl_blocks: Vec<HirImplBlock>,
    pub derive_traits: Vec<String>,
    pub doc_comments: Vec<String>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct HirVariant {
    pub def_id: DefId,
    pub name: String,
    pub kind: HirVariantKind,
    pub index: usize,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum HirVariantKind {
    Unit,
    Tuple(Vec<HirVariantField>),
    Struct(Vec<HirVariantField>),
}

#[derive(Debug, Clone)]
pub struct HirVariantField {
    pub name: Option<String>,
    pub ty: Ty,
    pub span: Span,
}

// ─── Mixin Definition ───────────────────────────────────────────────
// Was "Trait Definition" pre-Ruby-naming migration. See
// docs/specs/syntax/ruby-naming.spec.md.

#[derive(Debug, Clone)]
pub struct HirMixinDef {
    pub def_id: DefId,
    pub name: String,
    pub generic_params: Vec<HirGenericParam>,
    pub super_traits: Vec<MixinRef>,
    pub items: Vec<HirMixinItem>,
    pub doc_comments: Vec<String>,
    /// Mixin vtables spec §B1 — `Static` for ordinary mixins,
    /// `Runtime` for `mixin Foo dispatch runtime`. The HIR mirrors the
    /// AST flag verbatim; downstream (typeck, MIR, codegen) keys off
    /// this and the `MixinInfo.dispatch_mode` copy.
    pub dispatch_mode: crate::parser::ast::DispatchMode,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum HirMixinItem {
    AssocType {
        name: String,
        span: Span,
    },
    MethodSig {
        name: String,
        self_mode: Option<HirSelfMode>,
        is_class_method: bool,
        params: Vec<HirParam>,
        return_ty: Ty,
        span: Span,
    },
    DefaultMethod(HirFuncDef),
}

// ─── Impl Block ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HirImplBlock {
    pub generic_params: Vec<HirGenericParam>,
    pub is_unsafe: bool,
    pub negative_trait: bool,
    pub trait_ref: Option<MixinRef>,
    pub target_ty: Ty,
    pub items: Vec<HirImplItem>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum HirImplItem {
    AssocType {
        name: String,
        ty: Ty,
        span: Span,
    },
    Method(HirFuncDef),
    /// `include Mixin` directive in an extension body
    /// (ruby-naming.spec.md §3.4a).
    Include {
        is_unsafe: bool,
        negative_trait: bool,
        trait_name: String,
        span: Span,
    },
}

// ─── Module ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HirModule {
    pub def_id: DefId,
    pub name: String,
    pub items: Vec<HirItem>,
    pub span: Span,
}

// ─── Type Alias & Newtype ───────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HirTypeAlias {
    pub def_id: DefId,
    pub name: String,
    pub ty: Ty,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct HirNewtype {
    pub def_id: DefId,
    pub name: String,
    pub inner_ty: Ty,
    pub span: Span,
}

// ─── Const ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HirConst {
    pub def_id: DefId,
    pub name: String,
    pub ty: Ty,
    pub value: HirExpr,
    pub doc_comments: Vec<String>,
    pub span: Span,
}
