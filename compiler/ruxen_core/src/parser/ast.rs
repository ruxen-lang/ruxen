//! AST node definitions for the Ruxen programming language.
//!
//! Every node carries a `Span` for error reporting. The AST is untyped —
//! no semantic information is attached at this stage.

use crate::lexer::token::{FormatSpec, NumericSuffix, Span, TokenKind};

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
    /// `alias new_name old_name` at top level / module scope — a Ruby-style
    /// free-function synonym. Both names resolve to ONE callable DefId; no
    /// new body is emitted (ADR: docs/decisions/alias-keyword.md, D2).
    Alias(AliasDef),
    /// A top-level executable expression statement. The compile/type-check
    /// pipeline moves these into an implicit `main` when no explicit `main`
    /// exists. With an explicit `main`, they remain top-level so resolve can
    /// report the ambiguous combination as E0728.
    Expr(Expr),
}

impl TopLevelItem {
    /// Source span of the whole item.
    pub fn span(&self) -> &Span {
        match self {
            TopLevelItem::Module(m) => &m.span,
            TopLevelItem::Class(c) => &c.span,
            TopLevelItem::Struct(s) => &s.span,
            TopLevelItem::Enum(e) => &e.span,
            TopLevelItem::Mixin(t) => &t.span,
            TopLevelItem::Impl(i) => &i.span,
            TopLevelItem::Function(f) => &f.span,
            TopLevelItem::Use(u) => &u.span,
            TopLevelItem::TypeAlias(ta) => &ta.span,
            TopLevelItem::Newtype(nt) => &nt.span,
            TopLevelItem::Const(c) => &c.span,
            TopLevelItem::Lib(l) => &l.span,
            TopLevelItem::Alias(a) => &a.span,
            TopLevelItem::Expr(e) => &e.span,
        }
    }
}

// ─── Type Expressions ────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct TypePath {
    pub segments: Vec<String>,
    pub generic_args: Option<Vec<TypeExpr>>,
    pub span: Span,
    /// `true` when the source wrote the nilable shorthand `T?`, which the
    /// parser builds as `Option[T]`; `ruxen fmt` writes it back as `T?`.
    pub nilable: bool,
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

/// How a function type was written ([`TypeExpr::Function`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FnTypeSpelling {
    /// The paren form `Fn(T…) -> R` (also the legacy `Block(T…) -> R`).
    Paren,
    /// The canonical square-bracket block signature `Fn[(T…) -> R]`
    /// (Ruby-block-semantics ADR D8).
    Bracket,
    /// A bare signature `(T…) -> R` written as a generic argument of any
    /// named type: `Proc[(Int) -> nil]`.
    Signature,
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
        /// Surface spelling, carried so `ruxen fmt` preserves whichever the
        /// author wrote rather than normalizing one to another (the
        /// fmt-destructiveness class — Q23/Q30/Q34). Semantically inert.
        spelling: FnTypeSpelling,
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

impl TypeExpr {
    /// Source span of the whole type expression.
    pub fn span(&self) -> &Span {
        match self {
            TypeExpr::Named(tp) => &tp.span,
            TypeExpr::Reference { span, .. }
            | TypeExpr::Tuple { span, .. }
            | TypeExpr::Array { span, .. }
            | TypeExpr::Function { span, .. }
            | TypeExpr::SomeMixin { span, .. }
            | TypeExpr::AnyMixin { span, .. }
            | TypeExpr::Never { span }
            | TypeExpr::Inferred { span }
            | TypeExpr::RawPointer { span, .. }
            | TypeExpr::ConstLit { span, .. }
            | TypeExpr::ConstExprArg { span, .. } => span,
        }
    }
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

/// One part of an interpolated string literal: literal text, or a parsed
/// `#{expr}` with its optional format spec (`#{x:>8.2}`).
#[derive(Debug, Clone, PartialEq)]
pub enum StringPart {
    Literal(String),
    Expr { expr: Box<Expr>, spec: FormatSpec },
}

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
    /// A type named in VALUE position with explicit generic arguments,
    /// used as a static-call receiver: the `Hash[String, Int]` in
    /// `Hash[String, Int].new`.
    ///
    /// A plain `Hash` receiver is `Identifier("Hash")`, which resolves to
    /// the value-scope constructor entry whose type is the DECLARED
    /// `Hash[K, V]` — rigid parameters and all. That is fine when the call
    /// has arguments to infer from (`Point[Int].new(3)`), and it is not
    /// fine for a no-argument constructor: `Hash[String, Int].new` used to
    /// erase its arguments here and produce a `Hash[K, V]` that nothing
    /// downstream could ever refine. Carrying the written type instead
    /// lets the resolver hand back a concrete receiver type, which is all
    /// `substitute_generics_in_return` needs to type the call properly.
    ///
    /// `name` is the base segment, kept separately because the resolver
    /// still looks it up in value scope to find the constructor's `DefId`;
    /// `ty` is the whole written path, resolved through the ordinary
    /// type-expression path so `Hash` / `Array` / `Set` need no special
    /// casing here.
    GenericTypeRef {
        name: String,
        ty: Box<TypeExpr>,
    },
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
        /// `true` when the source wrote the instance-variable form `@field`,
        /// which the parser builds as `self.field`; `ruxen fmt` writes it
        /// back as `@field`.
        ivar: bool,
    },
    MethodCall {
        object: Box<Expr>,
        method: String,
        generic_args: Vec<TypeExpr>,
        args: Vec<Expr>,
        block: Option<Box<Expr>>,
        /// The source wrote an argument list `(...)`. False for the
        /// command form `x.m "a"`, a block-only call `x.m { }` and a bare
        /// `x.m[T]`; `ruxen fmt` writes the call back the same way.
        parens: bool,
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
        /// The source wrote an argument list `(...)`. False for the
        /// command form `puts "x"` and a block-only call `with_x { }`;
        /// `ruxen fmt` writes the call back the same way.
        parens: bool,
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
    /// Ruby parallel assignment `a, b = 1, 2` (and swap `a, b = b, a`).
    /// Without a splat, `targets` and `values` have equal length. A trailing
    /// assignment splat (`a, *rest = xs`) records the target index and permits
    /// one Array RHS or a value list. All right-hand values are evaluated
    /// before any target is written. Kept as its own node (not desugared in
    /// the parser) so `ruxen fmt` preserves the authored surface.
    MultiAssign {
        targets: Vec<Expr>,
        values: Vec<Expr>,
        splat_target: Option<usize>,
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

    /// Block Tier-2: `next [expr]` inside a block — the block's value for this
    /// iteration is `expr` (or `nil`), and the rest of the block body is
    /// skipped. Recognized only inside a closure body (parser `closure_depth`
    /// gate), so `.next` field access is unaffected. Lowers to an early
    /// return from the block's closure; rejected inside an inlined combinator
    /// block (E1125) so it can never become a non-local return.
    Next(Option<Box<Expr>>),

    /// Block Tier-2: a `&`-form block argument in call position — either the
    /// `&:symbol` to-proc sugar or `&block` / bare `&` block-forwarding. Lives
    /// transiently as a call's trailing argument; `resolve` hoists it into the
    /// call's block slot and desugars it (to a closure or the enclosing block
    /// slot). It never reaches typeck / MIR.
    BlockPass(BlockPassKind),

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
    /// Exponentiation `a ** b` (Ruby power operator, right-associative).
    /// Lowers to a runtime call (`ruxen_int_pow` / `ruxen_float_pow`), not a
    /// machine instruction and not a `def **` method; see `mir/lower/expr/binops.rs`.
    Pow,
    Eq,
    NotEq,
    Lt,
    Gt,
    LtEq,
    GtEq,
    /// Three-way comparison `a <=> b` (spaceship). Returns `Int` (-1 / 0 / 1).
    /// Lowers to a runtime call (`ruxen_int_cmp` / `ruxen_float_cmp` /
    /// `ruxen_str_spaceship`), not a machine instruction; see
    /// `mir/lower/expr/binops.rs`.
    Cmp,
    And,
    Or,
    /// Lazy Option fallback: `Option[T] ?? T -> T`.
    NilCoalesce,
    /// Ruby-style low-precedence logical `and`.
    KeywordAnd,
    /// Ruby-style low-precedence logical `or`.
    KeywordOr,
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
    /// True for the comparison operators (`==`, `!=`, `<`, `>`, `<=`, `>=`),
    /// whose result is a `Bool`.
    pub fn is_comparison(self) -> bool {
        matches!(
            self,
            BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::Gt | BinOp::LtEq | BinOp::GtEq
        )
    }

    /// Every binary operator, in [`Self::spec`] order.
    const ALL: [BinOp; 24] = [
        BinOp::KeywordAnd,
        BinOp::KeywordOr,
        BinOp::NilCoalesce,
        BinOp::Or,
        BinOp::And,
        BinOp::Eq,
        BinOp::NotEq,
        BinOp::Lt,
        BinOp::Gt,
        BinOp::LtEq,
        BinOp::GtEq,
        BinOp::Cmp,
        BinOp::MatchOp,
        BinOp::BitOr,
        BinOp::BitXor,
        BinOp::BitAnd,
        BinOp::Shl,
        BinOp::Shr,
        BinOp::Add,
        BinOp::Sub,
        BinOp::Mul,
        BinOp::Div,
        BinOp::Mod,
        BinOp::Pow,
    ];

    /// The operator table: each binary operator's token, source spelling and
    /// Pratt binding powers `(left, right)`. Higher binds tighter;
    /// `left < right` is left-associative, `left > right` right-associative,
    /// equal powers chain to the right (`??`). The parser, the formatter's
    /// precedence model and the diagnostics that name an operator all read
    /// it from here. Assignment (1, 2), range (9, 10) and cast (23, 24) are
    /// not `BinOp`s; `parser::expr::infix_binding_power` places them.
    fn spec(self) -> (TokenKind, &'static str, (u8, u8)) {
        match self {
            // Ruby keyword `and` / `or`: one level, looser than assignment.
            BinOp::KeywordAnd => (TokenKind::And, "and", (0, 1)),
            BinOp::KeywordOr => (TokenKind::Or, "or", (0, 1)),
            BinOp::NilCoalesce => (TokenKind::QuestionQuestion, "??", (2, 2)),
            BinOp::Or => (TokenKind::PipePipe, "||", (3, 4)),
            BinOp::And => (TokenKind::AmpAmp, "&&", (5, 6)),
            // Comparison and regex match: one non-associative level.
            BinOp::Eq => (TokenKind::EqEq, "==", (7, 8)),
            BinOp::NotEq => (TokenKind::NotEq, "!=", (7, 8)),
            BinOp::Lt => (TokenKind::Lt, "<", (7, 8)),
            BinOp::Gt => (TokenKind::Gt, ">", (7, 8)),
            BinOp::LtEq => (TokenKind::LtEq, "<=", (7, 8)),
            BinOp::GtEq => (TokenKind::GtEq, ">=", (7, 8)),
            BinOp::Cmp => (TokenKind::Spaceship, "<=>", (7, 8)),
            BinOp::MatchOp => (TokenKind::TildeEq, "~=", (7, 8)),
            BinOp::BitOr => (TokenKind::Pipe, "|", (11, 12)),
            BinOp::BitXor => (TokenKind::Caret, "^", (13, 14)),
            BinOp::BitAnd => (TokenKind::Amp, "&", (15, 16)),
            BinOp::Shl => (TokenKind::Shl, "<<", (17, 18)),
            BinOp::Shr => (TokenKind::Shr, ">>", (17, 18)),
            BinOp::Add => (TokenKind::Plus, "+", (19, 20)),
            BinOp::Sub => (TokenKind::Minus, "-", (19, 20)),
            BinOp::Mul => (TokenKind::Star, "*", (21, 22)),
            BinOp::Div => (TokenKind::Slash, "/", (21, 22)),
            BinOp::Mod => (TokenKind::Percent, "%", (21, 22)),
            // `**` binds tighter than `* / %`, cast and unary minus (25),
            // and is right-associative: `2 ** 3 ** 2 == 512`,
            // `-2 ** 2 == -4`. Postfix (27) still binds tighter.
            BinOp::Pow => (TokenKind::StarStar, "**", (26, 25)),
        }
    }

    /// The binary operator `kind` spells, if any.
    pub fn from_token(kind: &TokenKind) -> Option<BinOp> {
        Self::ALL.into_iter().find(|op| op.spec().0 == *kind)
    }

    /// The operator's source spelling (`+`, `<=>`, `and`).
    pub fn symbol(self) -> &'static str {
        self.spec().1
    }

    /// Pratt binding powers `(left, right)`.
    pub fn binding_power(self) -> (u8, u8) {
        self.spec().2
    }

    /// The method name this operator can desugar to. Nominal arithmetic and
    /// bitwise operators always take this route. Comparisons only take it
    /// when the receiver exposes the method, preserving primitive and
    /// auto-derive lowering as fallbacks.
    ///
    /// `!=` deliberately probes `==`: its dispatched result is inverted by
    /// MIR, matching Ruby's default relationship between the two operators.
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
            BinOp::Eq | BinOp::NotEq => "==",
            BinOp::Lt => "<",
            BinOp::Gt => ">",
            BinOp::LtEq => "<=",
            BinOp::GtEq => ">=",
            BinOp::Cmp => "<=>",
            // Power remains a numeric runtime operation rather than an
            // overridable method.
            BinOp::Pow
            | BinOp::And
            | BinOp::Or
            | BinOp::NilCoalesce
            | BinOp::KeywordAnd
            | BinOp::KeywordOr
            | BinOp::MatchOp => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
    /// Ruby-style low-precedence logical `not`.
    KeywordNot,
    /// Bitwise complement: `~expr` (integer surface, primitive-only for now).
    BitNot,
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

/// The `&`-form of a block argument (Block Tier-2). See `ExprKind::BlockPass`.
#[derive(Debug, Clone, PartialEq)]
pub enum BlockPassKind {
    /// `&:symbol` — desugars to `{ |x| x.symbol }` (a 1-arg to-proc closure).
    ToProc(String),
    /// `&block` / bare `&` — forward the enclosing function's received block.
    Forward,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClosureExpr {
    pub is_async: bool,
    pub is_move: bool,
    pub params: Vec<ClosureParam>,
    pub body: ClosureBody,
    pub span: Span,
    /// The source wrote a parameter header (`|x|` or `||`). A block with
    /// no parameters may omit it (`do ... end`, `x.then { 1 }`); `ruxen
    /// fmt` writes the header back only when it was there.
    pub pipes: bool,
    /// The source wrote `do ... end` rather than `{ ... }`; `ruxen fmt`
    /// keeps the author's form.
    pub do_end: bool,
    /// Block, proc or lambda. The parser marks a trailing block `Block`,
    /// `proc`/`Proc.new` `Proc`, and every other literal `Lambda`.
    pub kind: ClosureKind,
    /// How the source spelled the literal; `ruxen fmt` writes it back, except
    /// a `Bare` non-block value, which it prints as a `->` lambda.
    pub spelling: ClosureSpelling,
}

/// What a closure literal is (plan: blocks, procs and lambdas). A block
/// borrows its captures for the call it is attached to; a proc or a lambda
/// owns them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClosureKind {
    Block,
    Proc,
    Lambda,
}

/// The written head of a closure literal. For `Stabby`, `ClosureExpr::pipes`
/// records whether a `(...)` parameter list was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClosureSpelling {
    /// `{ |x| ... }` / `do |x| ... end` with no head.
    Bare,
    /// `->(x) { ... }` / `->(x) do ... end`.
    Stabby,
    /// `lambda { |x| ... }`.
    Lambda,
    /// `proc { |x| ... }`.
    Proc,
    /// `Proc.new { |x| ... }`.
    ProcNew,
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

/// A Ruby-style `alias new_name old_name` item. Carried in the enclosing
/// container (`ClassDef::aliases`, top-level `TopLevelItem::Alias`, etc.).
/// A pure resolver synonym — see `docs/decisions/alias-keyword.md`.
#[derive(Debug, Clone, PartialEq)]
pub struct AliasDef {
    /// The new spelling being introduced.
    pub new_name: String,
    /// The existing method / free-fn name it aliases.
    pub old_name: String,
    pub span: Span,
}

/// A class-body `type Name = Concrete` binding of an included mixin's
/// associated type (ruby-naming.spec.md §3.4). Kept in the AST so the
/// formatter round-trips it; resolve does not read it yet (the class's
/// concrete-return method carries the type through codegen).
#[derive(Debug, Clone, PartialEq)]
pub struct AssocTypeBinding {
    pub name: String,
    pub type_expr: TypeExpr,
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
    /// Ruby-style `alias new old` items declared in the class body
    /// (docs/decisions/alias-keyword.md). Pure synonyms — no method body.
    pub aliases: Vec<AliasDef>,
    /// `type Name = Concrete` associated-type bindings in the class body.
    pub assoc_types: Vec<AssocTypeBinding>,
    pub span: Span,
}

impl ClassDef {
    /// Whether the class body carried `layout <word>`.
    pub fn has_layout(&self, word: &str) -> bool {
        self.layout.iter().any(|s| s == word)
    }
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
    /// In-body `layout <kind>` directive args (ruby-naming.spec.md §3.5):
    /// `c`, `packed`, `transparent`. Kept as raw argument strings for v1;
    /// a real `Layout` enum arrives with the tier-4 stable-ABI / cbindgen
    /// work.
    pub layout: Vec<String>,
    /// Captured `##` doc comments preceding the struct (P0.13).
    pub doc_comments: Vec<String>,
    /// T2.02 S9: see `ClassDef::where_clause`.
    pub where_clause: Option<WhereClause>,
    /// See `ClassDef::aliases`.
    pub aliases: Vec<AliasDef>,
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
    /// See `ClassDef::aliases`.
    pub aliases: Vec<AliasDef>,
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
    AssocType {
        name: String,
        span: Span,
    },
    MethodSig(MethodSig),
    DefaultMethod(FuncDef),
    /// `alias new old` inside a mixin body (docs/decisions/alias-keyword.md).
    Alias(AliasDef),
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
    /// `alias new old` inside an `extension` impl body
    /// (docs/decisions/alias-keyword.md).
    Alias(AliasDef),
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
    /// `true` when the decl was spelled `def var NAME(...)`: an instance
    /// method that mutates its receiver. The receiver crosses the C ABI as
    /// `&var Self`, which for a String is the address of the caller's slot
    /// (`char**`, see `mir/refmut_abi.rs`), so the C symbol can reallocate
    /// the buffer in place.
    pub mutates_self: bool,
    /// `true` when the decl was spelled `def consume NAME(...)`: an instance
    /// method that takes its receiver over, the same spelling as a
    /// consuming-self `def`. On a `def consume drop`, the C symbol releases
    /// the whole handle, its storage included, so drop elaboration emits no
    /// storage free after it (`SymbolTable::drop_consumes_self`).
    pub consumes_self: bool,
    /// `true` when the return type was spelled `-> owned T`: the C function
    /// returns a fresh value the caller owns outright (for a collection, its
    /// spine, buffer and every element), so drop elaboration frees the
    /// result at the caller's scope exit
    /// (`mir::lower::runtime_abi::result_ownership`). Without it a C result
    /// is not assumed owned.
    pub returns_owned: bool,
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
    /// names that aren't a class generic or one of `generic_params` are
    /// dropped.
    pub where_clause: Option<WhereClause>,
    /// `def to_h[K, V] as "..."`: generic parameters of the declaration's
    /// own, named in its parameter and return types.
    pub generic_params: Option<GenericParams>,
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

// ─── REPL Input Types ──────────────────────────────────────────────

/// A single REPL input — may be an expression, statement, or top-level item.
#[derive(Debug, Clone, PartialEq)]
pub enum ReplInput {
    TopLevel(TopLevelItem),
    Statement(Statement),
    Expression(Expr),
}

/// One top-level unit of a script-style file ([`crate::parser::Parser::parse_script_units`]).
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptUnit {
    /// The declaration item (`def`, `class`, `use`, ...), or `None` for a
    /// statement.
    pub item: Option<TopLevelItem>,
    /// The unit's source, without its leading `##` docs.
    pub span: Span,
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
