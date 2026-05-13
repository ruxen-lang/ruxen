//! AST node definitions for the Riven programming language.
//!
//! Every node carries a `Span` for error reporting. The AST is untyped —
//! no semantic information is attached at this stage.

use crate::lexer::token::{NumericSuffix, Span, StringPart};

// ─── Visibility ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Private,
    Public,
    Protected,
}

// ─── Program ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub items: Vec<TopLevelItem>,
    pub span: Span,
}

// ─── Top-Level Items ─────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum TopLevelItem {
    Module(ModuleDef),
    Class(ClassDef),
    Struct(StructDef),
    Enum(EnumDef),
    Trait(TraitDef),
    Impl(ImplBlock),
    Function(FuncDef),
    Use(UseDecl),
    TypeAlias(TypeAliasDef),
    Newtype(NewtypeDef),
    Const(ConstDef),
    Lib(LibDecl),
    Extern(ExternBlock),
}

// ─── Type Expressions ────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct TypePath {
    pub segments: Vec<String>,
    pub generic_args: Option<Vec<TypeExpr>>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeExpr {
    Named(TypePath),
    Reference {
        lifetime: Option<String>,
        mutable: bool,
        inner: Box<TypeExpr>,
        span: Span,
    },
    Tuple {
        elements: Vec<TypeExpr>,
        span: Span,
    },
    Array {
        element: Box<TypeExpr>,
        size: Option<Box<Expr>>,
        span: Span,
    },
    Function {
        params: Vec<TypeExpr>,
        return_type: Box<TypeExpr>,
        span: Span,
    },
    ImplTrait {
        bounds: Vec<TraitBound>,
        span: Span,
    },
    DynTrait {
        bounds: Vec<TraitBound>,
        span: Span,
    },
    Never {
        span: Span,
    },
    Inferred {
        span: Span,
    },
    /// Raw pointer type: `*T` or `*mut T`
    RawPointer {
        mutable: bool,
        inner: Box<TypeExpr>,
        span: Span,
    },
    /// Tier-2 const generics: an integer literal appearing in a
    /// generic-argument position (e.g. the `4` in `Vector[Int, 4]`).
    ///
    /// Phase 02a stage 2 — parser only.  Resolve will promote this
    /// to `ConstExpr::Lit` for const parameters in S3 and emit a
    /// kind-mismatch diagnostic (E0700) when it lands against a type
    /// parameter.  The variant uses `i64` so the parser can carry
    /// negative literals through faithfully even though const
    /// parameters bound to unsigned types will reject them later.
    ConstLit {
        value: i64,
        span: Span,
    },
}

// ─── Trait Bounds & Generics ─────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct TraitBound {
    pub path: TypePath,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GenericParams {
    pub params: Vec<GenericParam>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GenericParam {
    Lifetime {
        name: String,
        span: Span,
    },
    Type {
        name: String,
        bounds: Vec<TraitBound>,
        span: Span,
    },
    /// Tier-2 const generics (`const N: USize`).
    ///
    /// Phase 02a stage 1 — parser only.  `ty` carries the user-written
    /// type annotation as a `TypeExpr`; resolve will validate that it
    /// is a built-in integer or `Bool` in S3 and surface E-CONST-BAD-TYPE
    /// otherwise.  No semantic effects yet — every downstream pass
    /// treats `Const` as a no-op until S3-S6 wire it through.
    Const {
        name: String,
        ty: TypeExpr,
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct WhereClause {
    pub predicates: Vec<WherePredicate>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WherePredicate {
    pub type_expr: TypeExpr,
    pub bounds: Vec<TraitBound>,
    pub span: Span,
}

// ─── Patterns ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    Literal {
        expr: Box<Expr>,
        span: Span,
    },
    Identifier {
        mutable: bool,
        name: String,
        span: Span,
    },
    Wildcard {
        span: Span,
    },
    Tuple {
        elements: Vec<Pattern>,
        span: Span,
    },
    Enum {
        path: Vec<String>,
        variant: String,
        fields: Vec<Pattern>,
        span: Span,
    },
    Struct {
        path: Vec<String>,
        fields: Vec<PatternField>,
        rest: bool,
        span: Span,
    },
    Or {
        patterns: Vec<Pattern>,
        span: Span,
    },
    Ref {
        mutable: bool,
        name: String,
        span: Span,
    },
    Rest {
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct PatternField {
    pub name: Option<String>,
    pub pattern: Pattern,
    pub span: Span,
}

// ─── Literals ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum LiteralValue {
    Int(i64, Option<NumericSuffix>),
    Float(f64, Option<NumericSuffix>),
    String(String),
    Char(char),
    Bool(bool),
}

// ─── Expressions ─────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    // Literals
    IntLiteral(i64, Option<NumericSuffix>),
    FloatLiteral(f64, Option<NumericSuffix>),
    StringLiteral(String),
    InterpolatedString(Vec<StringPart>),
    CharLiteral(char),
    BoolLiteral(bool),
    UnitLiteral,

    // Identifiers
    Identifier(String),
    SelfRef,
    SelfType,

    // Operators
    BinaryOp {
        left: Box<Expr>,
        op: BinOp,
        right: Box<Expr>,
    },
    UnaryOp {
        op: UnaryOp,
        operand: Box<Expr>,
    },

    // Borrowing
    Borrow(Box<Expr>),
    BorrowMut(Box<Expr>),

    // Field / method access
    FieldAccess {
        object: Box<Expr>,
        field: String,
    },
    MethodCall {
        object: Box<Expr>,
        method: String,
        generic_args: Vec<TypeExpr>,
        args: Vec<Expr>,
        block: Option<Box<Expr>>,
    },
    SafeNav {
        object: Box<Expr>,
        field: String,
    },
    SafeNavCall {
        object: Box<Expr>,
        method: String,
        args: Vec<Expr>,
    },

    // Calls & indexing
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        block: Option<Box<Expr>>,
    },
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
    },
    ClosureCall {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },

    // Try operator
    Try(Box<Expr>),

    // Async
    Await(Box<Expr>),

    // Assignment
    Assign {
        target: Box<Expr>,
        value: Box<Expr>,
    },
    CompoundAssign {
        target: Box<Expr>,
        op: BinOp,
        value: Box<Expr>,
    },

    // Control flow
    If(IfExpr),
    IfLet(IfLetExpr),
    Match(MatchExpr),
    While(WhileExpr),
    WhileLet(WhileLetExpr),
    For(ForExpr),
    Loop(LoopExpr),

    // Blocks & closures
    Block(Block),
    Closure(ClosureExpr),

    // Range
    Range {
        start: Option<Box<Expr>>,
        end: Option<Box<Expr>>,
        inclusive: bool,
    },

    // Collection literals
    ArrayLiteral(Vec<Expr>),
    ArrayFill {
        value: Box<Expr>,
        count: Box<Expr>,
    },
    TupleLiteral(Vec<Expr>),

    // Jump expressions
    Return(Option<Box<Expr>>),
    Break(Option<Box<Expr>>),
    Continue,

    // Yield
    Yield(Vec<Expr>),

    // Macros
    MacroCall {
        name: String,
        args: Vec<Expr>,
        delimiter: MacroDelimiter,
    },

    // Cast
    Cast {
        expr: Box<Expr>,
        target_type: TypeExpr,
    },

    // Enum variant construction
    EnumVariant {
        type_path: Vec<String>,
        variant: String,
        args: Vec<FieldArg>,
    },

    // Unsafe block: `unsafe ... end`
    UnsafeBlock(Block),

    // Null literal (for raw pointer types)
    NullLiteral,
}

// ─── Field Argument (for struct/enum construction) ───────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct FieldArg {
    pub name: Option<String>,
    pub value: Expr,
    pub span: Span,
}

// ─── Operators ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    NotEq,
    Lt,
    Gt,
    LtEq,
    GtEq,
    And,
    Or,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
    /// Dereference: `*expr` — strips one level of reference.
    Deref,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacroDelimiter {
    Paren,
    Bracket,
    Brace,
}

// ─── Control Flow Expressions ────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct IfExpr {
    pub condition: Box<Expr>,
    pub then_body: Block,
    pub elsif_clauses: Vec<ElsifClause>,
    pub else_body: Option<Block>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ElsifClause {
    pub condition: Box<Expr>,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IfLetExpr {
    pub pattern: Pattern,
    pub value: Box<Expr>,
    pub then_body: Block,
    pub else_body: Option<Block>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchExpr {
    pub subject: Box<Expr>,
    pub arms: Vec<MatchArm>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub guard: Option<Box<Expr>>,
    pub body: MatchArmBody,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MatchArmBody {
    Expr(Expr),
    Block(Block),
}

#[derive(Debug, Clone, PartialEq)]
pub struct WhileExpr {
    pub condition: Box<Expr>,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WhileLetExpr {
    pub pattern: Pattern,
    pub value: Box<Expr>,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ForExpr {
    pub pattern: Pattern,
    pub iterable: Box<Expr>,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoopExpr {
    pub body: Block,
    pub span: Span,
}

// ─── Closures ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct ClosureExpr {
    pub is_async: bool,
    pub is_move: bool,
    pub params: Vec<ClosureParam>,
    pub body: ClosureBody,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClosureParam {
    pub name: String,
    pub type_expr: Option<TypeExpr>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClosureBody {
    Expr(Box<Expr>),
    Block(Block),
}

// ─── Blocks & Statements ────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub statements: Vec<Statement>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    Let(LetBinding),
    Expression(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct LetBinding {
    pub mutable: bool,
    pub pattern: Pattern,
    pub type_annotation: Option<TypeExpr>,
    pub value: Option<Box<Expr>>,
    pub span: Span,
}

// ─── Self Mode ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfMode {
    Immutable,
    Mutable,
    Consuming,
}

// ─── Field Declaration ──────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct FieldDecl {
    pub visibility: Visibility,
    pub name: String,
    pub type_expr: TypeExpr,
    pub span: Span,
}

// ─── Functions ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct FuncDef {
    pub visibility: Visibility,
    pub is_async: bool,
    pub self_mode: Option<SelfMode>,
    pub is_class_method: bool,
    pub name: String,
    pub generic_params: Option<GenericParams>,
    pub params: Vec<Param>,
    pub return_type: Option<TypeExpr>,
    pub where_clause: Option<WhereClause>,
    pub body: Block,
    pub doc_comments: Vec<String>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub auto_assign: bool,
    pub name: String,
    pub type_expr: TypeExpr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MethodSig {
    pub is_async: bool,
    pub self_mode: Option<SelfMode>,
    pub is_class_method: bool,
    pub name: String,
    pub generic_params: Option<GenericParams>,
    pub params: Vec<Param>,
    pub return_type: Option<TypeExpr>,
    pub span: Span,
}

// ─── Class ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct ClassDef {
    pub name: String,
    pub generic_params: Option<GenericParams>,
    pub parent: Option<TypePath>,
    pub fields: Vec<FieldDecl>,
    pub methods: Vec<FuncDef>,
    pub inner_impls: Vec<InnerImpl>,
    /// Traits declared via `@[derive(...)]` at class top level or via an
    /// in-body `derive Trait1, Trait2` line. Consumed by the tier-1 derive
    /// expander (pillar 05 phase 5a+). Empty for v1 classes that do not
    /// opt in.
    pub derive_traits: Vec<String>,
    /// Captured `##` doc comments preceding the class (P0.13).
    pub doc_comments: Vec<String>,
    pub span: Span,
}

// ─── Struct ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct StructDef {
    pub name: String,
    pub generic_params: Option<GenericParams>,
    pub fields: Vec<FieldDecl>,
    /// Traits declared via `@[derive(...)]` at top level or via an in-body
    /// `derive Trait1, Trait2` line. Does NOT include `@[repr(...)]` args —
    /// those live on `repr`.
    pub derive_traits: Vec<String>,
    /// Representation hints from `@[repr(...)]` (e.g. `C`, `packed`). Kept
    /// as raw argument strings for v1; a real `Repr` enum arrives with the
    /// tier-4 stable-ABI/cbindgen work.
    pub repr: Vec<String>,
    /// Captured `##` doc comments preceding the struct (P0.13).
    pub doc_comments: Vec<String>,
    pub span: Span,
}

// ─── Enum ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct EnumDef {
    pub name: String,
    pub generic_params: Option<GenericParams>,
    pub variants: Vec<Variant>,
    /// Traits declared via `@[derive(...)]` or in-body `derive Trait` line.
    pub derive_traits: Vec<String>,
    /// Captured `##` doc comments preceding the enum (P0.13).
    pub doc_comments: Vec<String>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    pub name: String,
    pub fields: VariantKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum VariantKind {
    Unit,
    Tuple(Vec<VariantField>),
    Struct(Vec<VariantField>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct VariantField {
    pub name: Option<String>,
    pub type_expr: TypeExpr,
    pub span: Span,
}

// ─── Trait ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct TraitDef {
    pub name: String,
    pub generic_params: Option<GenericParams>,
    pub super_traits: Vec<TraitBound>,
    pub items: Vec<TraitItem>,
    pub doc_comments: Vec<String>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TraitItem {
    AssocType { name: String, span: Span },
    MethodSig(MethodSig),
    DefaultMethod(FuncDef),
}

// ─── Impl Blocks ─────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct ImplBlock {
    pub generic_params: Option<GenericParams>,
    pub is_unsafe: bool,
    pub negative_trait: bool,
    pub trait_name: Option<TypePath>,
    pub target_type: TypeExpr,
    pub items: Vec<ImplItem>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ImplItem {
    AssocType {
        name: String,
        type_expr: TypeExpr,
        span: Span,
    },
    Method(FuncDef),
}

#[derive(Debug, Clone, PartialEq)]
pub struct InnerImpl {
    pub is_unsafe: bool,
    pub negative_trait: bool,
    pub trait_name: TypePath,
    pub items: Vec<ImplItem>,
    pub span: Span,
}

// ─── Module ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct ModuleDef {
    pub name: String,
    pub items: Vec<TopLevelItem>,
    pub span: Span,
}

// ─── Use Declaration ─────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct UseDecl {
    pub path: Vec<String>,
    pub kind: UseKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UseKind {
    Simple,
    Alias(String),
    Group(Vec<String>),
}

// ─── Type Alias & Newtype ────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct TypeAliasDef {
    pub name: String,
    pub generic_params: Option<GenericParams>,
    pub type_expr: TypeExpr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewtypeDef {
    pub name: String,
    pub inner_type: TypeExpr,
    pub span: Span,
}

// ─── Const ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct ConstDef {
    pub name: String,
    pub type_expr: TypeExpr,
    pub value: Expr,
    pub doc_comments: Vec<String>,
    pub span: Span,
}

// ─── FFI Declarations ───────────────────────────────────────────────

/// A `lib Name ... end` block binding C library functions.
#[derive(Debug, Clone, PartialEq)]
pub struct LibDecl {
    pub name: String,
    pub functions: Vec<FfiFunction>,
    pub link_attrs: Vec<LinkAttr>,
    pub span: Span,
}

/// An `extern "C" ... end` block (anonymous lib).
#[derive(Debug, Clone, PartialEq)]
pub struct ExternBlock {
    pub abi: String,
    pub functions: Vec<FfiFunction>,
    pub span: Span,
}

/// A single FFI function declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct FfiFunction {
    pub name: String,
    pub params: Vec<FfiParam>,
    pub return_type: Option<TypeExpr>,
    pub is_variadic: bool,
    pub span: Span,
}

/// A parameter in an FFI function declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct FfiParam {
    pub name: String,
    pub type_expr: TypeExpr,
    pub span: Span,
}

/// A `@[link]` attribute for library linking.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkAttr {
    pub name: String,
    pub kind: LinkKind,
}

/// How to link a library.
#[derive(Debug, Clone, PartialEq)]
pub enum LinkKind {
    Dynamic,
    Static,
    Framework,
}

// ─── Attributes ────────────────────────────────────────────────────

/// A single argument inside an `@[name(args)]` attribute list.
///
/// Widened from `String` in tier-1 B2 so downstream passes can distinguish
/// identifiers (`C` in `@[repr(C)]`) from string literals (`"libc"` in
/// `@[link("libc")]`). Tier-2 may extend this with key/value and nested
/// forms; for v1 only the two leaf shapes exist.
#[derive(Debug, Clone, PartialEq)]
pub enum AttrArg {
    /// Bare identifier: `@[repr(C)]` → `Ident("C")`.
    Ident(String, Span),
    /// String literal: `@[link("libc")]` → `Str("libc")`.
    Str(String, Span),
}

impl AttrArg {
    pub fn as_str(&self) -> &str {
        match self {
            AttrArg::Ident(s, _) | AttrArg::Str(s, _) => s,
        }
    }

    pub fn span(&self) -> &Span {
        match self {
            AttrArg::Ident(_, s) | AttrArg::Str(_, s) => s,
        }
    }
}

/// A general attribute: `@[name(args)]`
#[derive(Debug, Clone, PartialEq)]
pub struct Attribute {
    pub name: String,
    pub args: Vec<AttrArg>,
    pub span: Span,
}

// ─── REPL Input Types ──────────────────────────────────────────────

/// A single REPL input — may be an expression, statement, or top-level item.
#[derive(Debug, Clone, PartialEq)]
pub enum ReplInput {
    TopLevel(TopLevelItem),
    Statement(Statement),
    Expression(Expr),
}

/// Result of attempting to parse REPL input.
#[derive(Debug)]
pub enum ReplParseResult {
    /// Successfully parsed a complete input.
    Complete(ReplInput),
    /// Input is incomplete — unclosed delimiters, need more lines.
    Incomplete,
    /// Parse error(s) in complete input.
    Error(Vec<crate::diagnostics::Diagnostic>),
}
