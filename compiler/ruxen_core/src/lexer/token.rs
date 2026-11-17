/// Byte-offset span in source code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub line: u32,
    pub column: u32,
}

impl Span {
    pub fn new(start: usize, end: usize, line: u32, column: u32) -> Self {
        Self {
            start,
            end,
            line,
            column,
        }
    }
}

/// Phase 2 #06.B: format specification for interpolated `"#{x:spec}"`.
///
/// Subset of Rust's format-spec grammar:
///
/// ```text
/// spec  := [fill align] [width] ['.' precision] ['?']
/// fill  := any char (only meaningful when followed by `align`)
/// align := '<' (left) | '>' (right) | '^' (center)
/// width := <digit>+
/// precision := <digit>+
/// ```
///
/// Examples:
/// - `"#{x:?}"` — Debug formatting (`debug = true`).
/// - `"#{x:>5}"` — right-aligned, width 5.
/// - `"#{x:.2}"` — precision 2 (e.g. for floats).
/// - `"#{x:*^10.2}"` — fill `*`, center-aligned, width 10, precision 2.
///
/// Phase B (this layer) captures the spec syntactically. Phase C
/// wires the `?` flag into the MIR Debug-interpolation path and
/// Phase D threads width/precision/align/fill into `Display::fmt`
/// via the `Formatter` runtime.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FormatSpec {
    pub debug: bool,
    pub width: Option<usize>,
    pub precision: Option<usize>,
    pub align: Option<char>,
    pub fill: Option<char>,
}

impl FormatSpec {
    /// Returns true when no field is set — equivalent to `"#{x}"`.
    pub fn is_default(&self) -> bool {
        !self.debug
            && self.width.is_none()
            && self.precision.is_none()
            && self.align.is_none()
            && self.fill.is_none()
    }
}

/// A part of an interpolated string.
#[derive(Debug, Clone, PartialEq)]
pub enum StringPart {
    /// Literal text segment.
    Literal(String),
    /// An expression span (byte offsets into source) to be parsed
    /// later, plus an optional format spec captured at lex time.
    /// Default `FormatSpec` corresponds to bare `"#{x}"`.
    Expr {
        tokens: Vec<Token>,
        spec: FormatSpec,
    },
}

/// A token produced by the lexer.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    pub fn new(kind: TokenKind, span: Span) -> Self {
        Self { kind, span }
    }
}

/// Numeric type suffix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumericSuffix {
    I8,
    I16,
    I32,
    I64,
    U,
    U8,
    U16,
    U32,
    U64,
    ISize,
    USize,
    F32,
    F64,
}

impl NumericSuffix {
    /// The suffix as written in source (`42i8`, `3.0f32`).
    pub fn as_str(self) -> &'static str {
        match self {
            NumericSuffix::I8 => "i8",
            NumericSuffix::I16 => "i16",
            NumericSuffix::I32 => "i32",
            NumericSuffix::I64 => "i64",
            NumericSuffix::U => "u",
            NumericSuffix::U8 => "u8",
            NumericSuffix::U16 => "u16",
            NumericSuffix::U32 => "u32",
            NumericSuffix::U64 => "u64",
            NumericSuffix::ISize => "isize",
            NumericSuffix::USize => "usize",
            NumericSuffix::F32 => "f32",
            NumericSuffix::F64 => "f64",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // ── Keywords: Variable & Binding ──
    Let,
    Move,
    Ref,
    Var, // `var` — mutable binding (replaces `let mut`)

    // ── Keywords: Type Definitions ──
    Class,
    Struct,
    Enum,
    Mixin,     // `mixin` — replaces `trait`
    Include,   // `include` — replaces `impl Trait for X`
    Extension, // `extension` — replaces conditional `impl[T: B] C[T]`
    Newtype,
    Type,

    // ── Keywords: Functions & Methods ──
    Def,
    Public,    // `public` — section marker; switches subsequent decls to public
    Private,   // `private` — section marker; replaces `pub` prefix model
    Protected, // already a section marker
    Consume,
    Inline,    // `inline` — modifier on `def`
    SelfValue, // `self`
    SelfType,  // `Self`
    Init,
    Super,
    Return,
    Yield,
    Async,
    Await,

    // ── Keywords: Control Flow ──
    If,
    Elsif,
    Else,
    Match,
    While,
    For,
    In,
    Loop,
    Do,
    End,
    Break,
    Continue,

    // ── Keywords: Type System ──
    Where,
    As,
    SomeBound, // lowercase `some` — `some Mixin` type position
    AnyBound,  // lowercase `any`  — `any Mixin` type position
    Layout,    // `layout c` / `layout packed` / `layout transparent`

    // ── Keywords: Modules ──
    Module,
    Use,
    Package, // `package` — replaces `crate`

    // ── Keywords: Safety ──
    Unsafe,

    // ── Keywords: Literals ──
    True,
    False,
    SomeKw, // `Some` constructor for Option
    OkKw,
    ErrKw,
    Nil, // `nil` — replaces both `null` (raw pointer) and `None` (Option).

    // ── Keywords: FFI & Interop ──
    Lib,

    // ── Keywords: Reserved ──
    Macro,
    Static,
    Const,
    When,
    Unless,
    Until,
    Case, // `case … when/in … end` (Ruby case dispatch + pattern match)
    Then, // `when x then …` clause separator

    // ── Operators: Arithmetic ──
    Plus,     // +
    Minus,    // -
    Star,     // *
    StarStar, // ** power operator (exponentiation), right-associative
    Slash,    // /
    Percent,  // %

    // ── Operators: Comparison ──
    EqEq,      // ==
    NotEq,     // !=
    Lt,        // <
    Gt,        // >
    LtEq,      // <=
    GtEq,      // >=
    Spaceship, // <=> three-way comparison, returns -1 / 0 / 1

    // ── Operators: Regex Match ──
    TildeEq, // ~= — `s ~= /pat/` regex-match operator (E17xx).

    // ── Operators: Logical ──
    And,              // `and` low-precedence logical AND
    Or,               // `or` low-precedence logical OR
    Not,              // `not` low-precedence logical negation
    AmpAmp,           // &&
    PipePipe,         // ||
    QuestionQuestion, // ?? lazy Option fallback
    Bang,             // !

    // ── Operators: Bitwise ──
    Amp,   // &
    Pipe,  // |
    Caret, // ^
    Shl,   // <<
    Shr,   // >>

    // ── Operators: Assignment ──
    Eq,         // =
    PlusEq,     // +=
    MinusEq,    // -=
    StarEq,     // *=
    SlashEq,    // /=
    PercentEq,  // %=
    PipePipeEq, // ||= conditional-assign; desugars to `a || (a = b)`
    AmpAmpEq,   // &&= conditional-assign; desugars to `a && (a = b)`
    ShlEq,      // <<= compound-assign; desugars to `a = a << b`
    ShrEq,      // >>= compound-assign; desugars to `a = a >> b`
    AmpEq,      // &= compound-assign; desugars to `a = a & b`
    PipeEq,     // |= compound-assign; desugars to `a = a | b`
    CaretEq,    // ^= compound-assign; desugars to `a = a ^ b`
    Tilde,      // ~ bitwise complement (prefix unary)

    // ── Operators: Range (ruby-naming.spec.md §3.10b) ──
    // Ruby semantics: `..` is INCLUSIVE, `...` is EXCLUSIVE. There is no
    // `..=` token — the Rust inclusive form is rejected at lex time (E0009).
    DotDot,    // ..  (inclusive)
    DotDotDot, // ... (exclusive)

    // ── Operators: Arrow ──
    Arrow,    // ->
    FatArrow, // =>

    // ── Operators: Special ──
    AmpDot,     // &.
    Question,   // ?
    At,         // @
    ColonColon, // ::
    AmpMut,     // &mut

    // ── Delimiters ──
    LParen,   // (
    RParen,   // )
    LBracket, // [
    RBracket, // ]
    LBrace,   // {
    RBrace,   // }

    // ── Punctuation ──
    Dot,       // .
    Comma,     // ,
    Colon,     // :
    Semicolon, // ;

    // ── Literals ──
    IntLiteral(i64, Option<NumericSuffix>),
    FloatLiteral(f64, Option<NumericSuffix>),
    StringLiteral(String),
    InterpolatedString(Vec<StringPart>),
    CharLiteral(char),
    /// `/pat/flags` regex literal. Recognised by the lexer's `/`
    /// dispatcher only in expression-context positions per the JS/Ruby
    /// positional rule (see `prev_token_starts_expr_context`). Flag
    /// set: `i m s g x`. Lifecycle: lifted by Phase-2 stdlib bootstrap
    /// to a `Ty::Class { name: "Regex" }` after typeck — codegen lowers
    /// to a `ruxen_regex_compile_const` call.
    RegexLiteral {
        pattern: String,
        flags: String,
    },

    // ── Identifiers ──
    Identifier(String),
    TypeIdentifier(String),

    // ── Comments ──
    DocComment(String),

    // ── Structure ──
    Newline,
    Eof,
}

impl TokenKind {
    /// Returns true if this token kind implies line continuation
    /// (i.e., suppress newline after it).
    pub fn continues_line(&self) -> bool {
        matches!(
            self,
            TokenKind::Plus
                | TokenKind::Minus
                | TokenKind::Star
                | TokenKind::StarStar
                | TokenKind::Slash
                | TokenKind::Percent
                | TokenKind::Eq
                | TokenKind::PlusEq
                | TokenKind::MinusEq
                | TokenKind::StarEq
                | TokenKind::SlashEq
                | TokenKind::PercentEq
                | TokenKind::PipePipeEq
                | TokenKind::AmpAmpEq
                | TokenKind::ShlEq
                | TokenKind::ShrEq
                | TokenKind::AmpEq
                | TokenKind::PipeEq
                | TokenKind::CaretEq
                | TokenKind::EqEq
                | TokenKind::NotEq
                | TokenKind::Lt
                | TokenKind::Gt
                | TokenKind::LtEq
                | TokenKind::GtEq
                | TokenKind::Spaceship
                | TokenKind::And
                | TokenKind::Or
                | TokenKind::AmpAmp
                | TokenKind::PipePipe
                | TokenKind::QuestionQuestion
                | TokenKind::Arrow
                | TokenKind::FatArrow
                | TokenKind::Dot
                | TokenKind::AmpDot
                | TokenKind::Comma
                | TokenKind::LParen
                | TokenKind::LBracket
                | TokenKind::LBrace
                | TokenKind::Pipe
                | TokenKind::Amp
                | TokenKind::AmpMut
                | TokenKind::Caret
                | TokenKind::Shl
                | TokenKind::Shr
                | TokenKind::DotDot
                | TokenKind::DotDotDot
                | TokenKind::Colon
                | TokenKind::ColonColon
        )
    }

    pub fn is_opening_delimiter(&self) -> bool {
        matches!(
            self,
            TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace
        )
    }
}

/// The keyword table: every reserved word's spelling and the token it lexes
/// to. The lexer ([`lookup_keyword`]), IDE completion, IDE rename and REPL
/// completion all read it, so none of them can drift from the lexer.
/// Contextual words (`alias`, `exclude`) lex as identifiers and are not here.
pub static KEYWORD_TABLE: [(&str, TokenKind); 65] = [
    // Variable & Binding
    ("let", TokenKind::Let),
    // `mut` removed — use `var` for every writable position.
    ("move", TokenKind::Move),
    ("ref", TokenKind::Ref),
    ("var", TokenKind::Var),
    // Type Definitions
    ("class", TokenKind::Class),
    ("struct", TokenKind::Struct),
    ("enum", TokenKind::Enum),
    // `trait` removed — use `mixin`.
    // `impl` removed — use `include` inside a type body or `extension`
    // for conditional / external method blocks.
    ("mixin", TokenKind::Mixin),
    ("include", TokenKind::Include),
    ("extension", TokenKind::Extension),
    ("newtype", TokenKind::Newtype),
    ("type", TokenKind::Type),
    // Functions & Methods
    ("def", TokenKind::Def),
    // `pub` removed — use `public` section marker or `public` prefix.
    ("public", TokenKind::Public),
    ("private", TokenKind::Private),
    ("protected", TokenKind::Protected),
    ("consume", TokenKind::Consume),
    ("inline", TokenKind::Inline),
    ("self", TokenKind::SelfValue),
    ("Self", TokenKind::SelfType),
    ("init", TokenKind::Init),
    ("super", TokenKind::Super),
    ("return", TokenKind::Return),
    ("yield", TokenKind::Yield),
    ("async", TokenKind::Async),
    ("await", TokenKind::Await),
    // Control Flow
    ("if", TokenKind::If),
    ("elsif", TokenKind::Elsif),
    ("else", TokenKind::Else),
    ("match", TokenKind::Match),
    ("while", TokenKind::While),
    ("for", TokenKind::For),
    ("in", TokenKind::In),
    ("loop", TokenKind::Loop),
    ("do", TokenKind::Do),
    ("end", TokenKind::End),
    ("break", TokenKind::Break),
    ("continue", TokenKind::Continue),
    // Type System
    ("where", TokenKind::Where),
    ("as", TokenKind::As),
    // `dyn` removed — use `any Mixin` for dyn-trait positions.
    ("some", TokenKind::SomeBound),
    ("any", TokenKind::AnyBound),
    // `derive` removed — auto-include replaces it.
    ("layout", TokenKind::Layout),
    // Modules
    ("module", TokenKind::Module),
    ("use", TokenKind::Use),
    ("package", TokenKind::Package),
    // Safety
    ("unsafe", TokenKind::Unsafe),
    // FFI & Interop
    ("lib", TokenKind::Lib),
    // `null` removed — use `nil` (raw-pointer null literal).
    ("nil", TokenKind::Nil),
    // Literals
    ("true", TokenKind::True),
    ("false", TokenKind::False),
    // `None` removed — use `nil` (Option::None literal).
    ("Some", TokenKind::SomeKw),
    ("Ok", TokenKind::OkKw),
    ("Err", TokenKind::ErrKw),
    // Reserved
    ("macro", TokenKind::Macro),
    // `crate` removed — use `package`.
    // `extern` removed — use `lib`.
    ("static", TokenKind::Static),
    ("const", TokenKind::Const),
    ("when", TokenKind::When),
    ("unless", TokenKind::Unless),
    ("until", TokenKind::Until),
    ("case", TokenKind::Case),
    ("then", TokenKind::Then),
    // Low-precedence logical operators
    ("and", TokenKind::And),
    ("or", TokenKind::Or),
    ("not", TokenKind::Not),
];

/// Every keyword spelling, in table order.
pub fn keywords() -> impl Iterator<Item = &'static str> {
    KEYWORD_TABLE.iter().map(|(spelling, _)| *spelling)
}

/// Look up a keyword from an identifier string.
pub fn lookup_keyword(ident: &str) -> Option<TokenKind> {
    KEYWORD_TABLE
        .iter()
        .find(|(spelling, _)| *spelling == ident)
        .map(|(_, kind)| kind.clone())
}
