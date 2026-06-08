//! AST node definitions for the Ruxen programming language.
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
    /// `mixin M ... end` — was `Trait` pre-Ruby-naming migration.
    /// See docs/specs/syntax/ruby-naming.spec.md.
    Mixin(MixinDef),
    Impl(ImplBlock),
    Function(FuncDef),
    Use(UseDecl),
    TypeAlias(TypeAliasDef),
    Newtype(NewtypeDef),
    Const(ConstDef),
    Lib(LibDecl),
    Extern(ExternBlock),
    /// A top-level expression statement — e.g. a call with a trailing
    /// `do…end` block at module scope (`Tester.describe("…") do … end`).
    /// The compiler's normal pipeline does NOT execute top-level
    /// statements directly: `ruxen test` HOISTS top-level items and wraps
    /// the remaining statements in a synthesised `def main` before
    /// compiling, so this variant only ever survives to `resolve` when a
    /// raw file is compiled directly — where it is rejected with a clear
    /// E0728. Its purpose is to let the SHARED parser (compiler + LSP +
    /// `ruxen fmt`) ACCEPT the test-file surface so the formatter can
    /// round-trip it instead of erroring at 1:1 (Q23b).
    Expr(Expr),
}

// ─── Type Expressions ────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct TypePath {
    pub segments: Vec<String>,
    pub generic_args: Option<Vec<TypeExpr>>,
    pub span: Span,
    /// `true` if the type path was written with a leading `::` —
    /// e.g. `::Foo` or `::Outer.Inner`.  Set by the parser
    /// (`parse_type_path`) when the first token is `ColonColon`.
    /// Consumed by the resolver to bypass inner module scopes and
    /// look the name up in the global `type_registry` directly
    /// (#06.93 Phase 2 — root anchor).
    ///
    /// Default `false`; expression-position `::` is not in scope
    /// for #06.93.
    pub rooted: bool,
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
    /// `some M` — static-dispatch mixin reference (was `impl Trait`
    /// pre-Ruby-naming; see docs/specs/syntax/ruby-naming.spec.md).
    SomeMixin {
        bounds: Vec<MixinBound>,
        span: Span,
    },
    /// `any M` — dynamic-dispatch mixin reference (was `dyn Trait`
    /// pre-Ruby-naming).
    AnyMixin {
        bounds: Vec<MixinBound>,
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
    /// kind-mismatch diagnostic (E0704) when it lands against a type
    /// parameter.  The variant uses `i64` so the parser can carry
    /// negative literals through faithfully even though const
    /// parameters bound to unsigned types will reject them later.
    ConstLit {
        value: i64,
        span: Span,
    },
    /// Tier-2 const generics S8.S3: an arithmetic const expression
    /// appearing in a generic-argument position (e.g. `2 + 3` in
    /// `Vector[Int, 2 + 3]`).  Stored as a parser `Expr` so the
    /// existing arithmetic precedence parser does the work; resolve
    /// folds the result into a HIR `ConstExpr` via
    /// `lower_const_expr_from_expr`.  Triggered when the lookahead
    /// after an `IntLiteral` token is a binary arithmetic op
    /// (`+ - * /`).  Bare literals still emit `ConstLit` for
    /// backwards compatibility with existing call sites.
    ConstExprArg {
        expr: Box<Expr>,
        span: Span,
    },
}

// ─── Mixin Bounds & Generics ─────────────────────────────────────────
// Was "Trait Bounds" pre-Ruby-naming migration. See
// docs/specs/syntax/ruby-naming.spec.md.

#[derive(Debug, Clone, PartialEq)]
pub struct MixinBound {
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
        bounds: Vec<MixinBound>,
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
    /// Tier-2 const generics (T2.02 §B9 / S9 parser cut):
    /// `where N > 0`, `where N == M`, `where N + M == 8`.
    /// Parser captures these as full expressions; enforcement (per-
    /// instantiation eval + `E-CONST-WHERE-FALSE` diagnostic) lands
    /// with the deeper S7 binding-threading follow-up.
    pub const_predicates: Vec<ConstPredicate>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WherePredicate {
    pub type_expr: TypeExpr,
    pub bounds: Vec<MixinBound>,
    pub span: Span,
}

/// Tier-2 const generics (T2.02 §B9 / S9 parser cut): a const-level
/// predicate in a `where` clause.  Triggered when a where-clause
/// item starts with `Identifier op …` where the op is a comparison
/// (`> < >= <= == !=`) or arithmetic (`+ - * /`) — distinct from the
/// trait-bound form (`Identifier : TraitName`).
///
/// The captured expression is the raw parser `Expr`; resolve will
/// lower the comparison + arithmetic into a HIR predicate (S9 work)
/// and monomorphization will evaluate it per instantiation.  Today
/// this is parser-only and is silently dropped by downstream
/// passes — the AST round-trips correctly so the syntax can be
/// reviewed in source before the runtime story lands.
#[derive(Debug, Clone, PartialEq)]
pub struct ConstPredicate {
    pub expr: Box<Expr>,
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
    /// `{ k => v, k => v, ... }` — Map literal per ruby-naming.spec.md §10a.
    MapLiteral(Vec<(Expr, Expr)>),

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

    /// `/pat/flags` regex literal (std.regex). Typed as
    /// `Ty::Class { name: "Regex" }` after typeck and lowered to a
    /// `ruxen_regex_compile_const` call by MIR.
    RegexLiteral {
        pattern: String,
        flags: String,
    },
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
    /// Regex match: `s ~= /pat/flags`. Desugars to `pat.is_match(s)`
    /// at MIR-lower time. Equality-tier precedence (same as `==`).
    MatchOp,
}

impl BinOp {
    /// The method name this operator desugars to, for the MIGRATED
    /// operator families only (Task OP, Step 3): arithmetic (`+ - * / %`)
    /// and bitwise (`& | ^ << >>`). Comparison / equality / logical /
    /// regex-match return `None` — they are deliberately EXCLUDED from the
    /// operator-as-method wave (they stay on the existing binop paths and
    /// the later `Comparable` increment), so a `None` here means "do not
    /// desugar; use the structural lowering". This is the SINGLE source of
    /// the operator→method-name map, shared by typeck (`infer_binop`
    /// routing) and MIR (`lower_binops`).
    pub fn method_name(self) -> Option<&'static str> {
        Some(match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Mod => "%",
            BinOp::BitAnd => "&",
            BinOp::BitOr => "|",
            BinOp::BitXor => "^",
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
            BinOp::Eq
            | BinOp::NotEq
            | BinOp::Lt
            | BinOp::Gt
            | BinOp::LtEq
            | BinOp::GtEq
            | BinOp::And
            | BinOp::Or
            | BinOp::MatchOp => return None,
        })
    }
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
    pub default: Option<Box<Expr>>,
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
    /// In-body `layout <kind>` directive args captured at parse time.
    /// Wave 1 (#06.8 T0c) recognises `flat_heap_struct` here, marking a
    /// class whose runtime instances follow the flat-heap-struct C
    /// layout (`RuxenFile`, `RuxenTcpStream` pattern). Other tokens are
    /// accepted by the lexer but rejected by the parser arm.
    pub layout: Vec<String>,
    /// In-body `lib "X" ... end` FFI blocks (#06.8 follow-up). Each block
    /// binds C library functions whose Ruxen-side names are scoped to
    /// this class — e.g. `class File ... lib "rt" def open as "..." end end`
    /// exposes `File.open` as a class method. Parser-only plumbing for
    /// now; resolver wiring (registering each `FfiFunction` as a class
    /// method) lands in the follow-up commit.
    pub lib_decls: Vec<LibDecl>,
    /// Captured `##` doc comments preceding the class (P0.13).
    pub doc_comments: Vec<String>,
    /// T2.02 S9: where-clause predicates (`where T: Display, N > 0`).
    /// Trait bounds land on the matching generic param at resolve;
    /// const predicates land on `ClassInfo::const_predicates` and are
    /// evaluated at every `Ty::Class` instantiation.
    pub where_clause: Option<WhereClause>,
    pub span: Span,
}

// ─── Struct ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct StructDef {
    pub name: String,
    pub generic_params: Option<GenericParams>,
    pub fields: Vec<FieldDecl>,
    /// Inline method definitions (post Ruby-naming migration). Structs
    /// now accept `def`s in their body the same way classes do; the
    /// resolver lowers them through the same per-type method tables.
    pub methods: Vec<FuncDef>,
    /// Inline `impl Trait for Self` blocks and bare `include Mixin`
    /// directives inside the struct body. Mirrors `ClassDef::inner_impls`.
    pub inner_impls: Vec<InnerImpl>,
    /// Traits declared via `@[derive(...)]` at top level or via an in-body
    /// `derive Trait1, Trait2` line. Does NOT include `layout` args —
    /// those live on `layout`.
    pub derive_traits: Vec<String>,
    /// In-body `layout <kind>` directive args (ruby-naming.spec.md §3.5):
    /// `c`, `packed`, `transparent`. Kept as raw argument strings for v1;
    /// a real `Layout` enum arrives with the tier-4 stable-ABI / cbindgen
    /// work.
    pub layout: Vec<String>,
    /// Captured `##` doc comments preceding the struct (P0.13).
    pub doc_comments: Vec<String>,
    /// T2.02 S9: see `ClassDef::where_clause`.
    pub where_clause: Option<WhereClause>,
    pub span: Span,
}

// ─── Enum ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct EnumDef {
    pub name: String,
    pub generic_params: Option<GenericParams>,
    pub variants: Vec<Variant>,
    /// Inline method definitions (post Ruby-naming migration). Enums
    /// now accept `def`s in their body the same way classes do.
    pub methods: Vec<FuncDef>,
    /// Inline `impl Trait for Self` blocks and bare `include Mixin`
    /// directives inside the enum body. Mirrors `ClassDef::inner_impls`.
    pub inner_impls: Vec<InnerImpl>,
    /// Traits declared via `@[derive(...)]` or in-body `derive Trait` line.
    pub derive_traits: Vec<String>,
    /// In-body `layout <kind>` directive args captured at parse time.
    /// Wave 1 (#06.8 T0c) recognises `tagged` here, which pins variant
    /// declaration order as the runtime tag assignment (the pattern
    /// `IoError`/`IoErrorKind` already follow). Other tokens are
    /// rejected at the parser arm.
    pub layout: Vec<String>,
    /// Captured `##` doc comments preceding the enum (P0.13).
    pub doc_comments: Vec<String>,
    /// T2.02 S9: see `ClassDef::where_clause`.
    pub where_clause: Option<WhereClause>,
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

// ─── Mixin ───────────────────────────────────────────────────────────
// Was "Trait" pre-Ruby-naming migration. See
// docs/specs/syntax/ruby-naming.spec.md.

#[derive(Debug, Clone, PartialEq)]
pub struct MixinDef {
    pub name: String,
    pub generic_params: Option<GenericParams>,
    pub super_traits: Vec<MixinBound>,
    pub items: Vec<MixinItem>,
    /// In-body `lib "X" ... end` FFI blocks (#06.8 follow-up). Mirrors
    /// `ClassDef::lib_decls`. Parser-only plumbing for now.
    pub lib_decls: Vec<LibDecl>,
    pub doc_comments: Vec<String>,
    /// Spec — `docs/specs/types/mixin_vtables.spec.md` §B1. `Static` is
    /// the default; `Runtime` opts the mixin into per-implementor
    /// vtable dispatch and unlocks `&Mixin` / `&var Mixin` parameter
    /// types. Source surface: `mixin Foo dispatch runtime ... end`.
    /// Phase A wires the field through parser → HIR → resolve →
    /// typeck; codegen is unchanged (Phase B).
    pub dispatch_mode: DispatchMode,
    pub span: Span,
}

/// Mixin dispatch policy. `Static` (default) means every method call
/// resolves at the call site from the receiver's concrete class.
/// `Runtime` means the compiler will (Phase B) emit a per-implementor
/// vtable and route `&Mixin` / `&var Mixin` calls through it.
///
/// Spec: `docs/specs/types/mixin_vtables.spec.md` §B1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchMode {
    /// Default: static dispatch only. No runtime overhead.
    Static,
    /// `mixin Foo dispatch runtime` — opt in to runtime dispatch.
    Runtime,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MixinItem {
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
    /// `include Mixin` directive inside an `extension` body
    /// (ruby-naming.spec.md §3.4a).
    Include {
        is_unsafe: bool,
        negative_trait: bool,
        trait_name: TypePath,
        span: Span,
    },
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
    /// `true` when the decl was spelled `def self.NAME(...)` — the Ruxen
    /// convention for class methods (no implicit `self` parameter). `false`
    /// means `def NAME(...)`, which is the instance-method form. For FFI
    /// decls inside a class/mixin body, the resolver uses this flag to
    /// flip `FnSignature.is_class_method`. Top-level FFI decls are always
    /// `false` here — they're free functions, not methods on a type.
    pub is_class_method: bool,
    /// Optional C-symbol alias declared via `def name as "<c-symbol>"(...)`.
    /// When `None`, the Ruxen-side name (`name`) is the linked C symbol —
    /// the historical behaviour of `lib "X" ... end` blocks. When `Some`,
    /// the Ruxen name and the C symbol differ; this is the per-decl
    /// rename surface used by stdlib self-hosting (#06.8) so that a
    /// Ruxen method like `File.open` can bind to `ruxen_file_open`.
    /// Resolver/MIR/codegen wiring is deferred to a follow-up commit;
    /// today this field is parsed and stored but not yet consumed.
    pub c_symbol: Option<String>,
    pub params: Vec<FfiParam>,
    pub return_type: Option<TypeExpr>,
    /// Optional `where T: Bound` clause. The ONLY supported form today is a
    /// receiver-element bound: a predicate on the ENCLOSING class's generic
    /// (e.g. `class Array[T]`'s `def sum -> Int where T: Add`). The
    /// resolver (`ffi_registration.rs`) threads such a predicate into the
    /// registered signature's `generic_params` so the call-site bound seam
    /// can enforce it against the receiver's concrete element. Predicates on
    /// names that aren't a class generic are dropped (FFI defs have no own
    /// generics).
    pub where_clause: Option<WhereClause>,
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
