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

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // ── Keywords: Variable & Binding ──
    Let,
    Mut,
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

    // ── Operators: Arithmetic ──
    Plus,    // +
    Minus,   // -
    Star,    // *
    Slash,   // /
    Percent, // %

    // ── Operators: Comparison ──
    EqEq,  // ==
    NotEq, // !=
    Lt,    // <
    Gt,    // >
    LtEq,  // <=
    GtEq,  // >=

    // ── Operators: Logical ──
    AmpAmp,   // &&
    PipePipe, // ||
    Bang,     // !

    // ── Operators: Bitwise ──
    Amp,   // &
    Pipe,  // |
    Caret, // ^
    Shl,   // <<
    Shr,   // >>

    // ── Operators: Assignment ──
    Eq,        // =
    PlusEq,    // +=
    MinusEq,   // -=
    StarEq,    // *=
    SlashEq,   // /=
    PercentEq, // %=

    // ── Operators: Range ──
    DotDot,   // ..
    DotDotEq, // ..=

    // ── Operators: Arrow ──
    Arrow,    // ->
    FatArrow, // =>

    // ── Operators: Special ──
    QuestionDot, // ?.
    Question,    // ?
    At,          // @
    ColonColon,  // ::
    AmpMut,      // &mut

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

    // ── Identifiers ──
    Identifier(String),
    TypeIdentifier(String),

    // ── Lifetime ──
    Lifetime(String), // 'a, 'input — lifetime parameters

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
                | TokenKind::Slash
                | TokenKind::Percent
                | TokenKind::Eq
                | TokenKind::PlusEq
                | TokenKind::MinusEq
                | TokenKind::StarEq
                | TokenKind::SlashEq
                | TokenKind::PercentEq
                | TokenKind::EqEq
                | TokenKind::NotEq
                | TokenKind::Lt
                | TokenKind::Gt
                | TokenKind::LtEq
                | TokenKind::GtEq
                | TokenKind::AmpAmp
                | TokenKind::PipePipe
                | TokenKind::Arrow
                | TokenKind::FatArrow
                | TokenKind::Dot
                | TokenKind::QuestionDot
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
                | TokenKind::DotDotEq
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

/// The canonical list of Ruxen keyword spellings — the single source of
/// truth shared with IDE features (e.g. completion) so they cannot drift
/// from the lexer. Every entry MUST be recognized by [`lookup_keyword`];
/// the `keywords_const_matches_lookup` test enforces that.
pub const KEYWORDS: &[&str] = &[
    "let", "move", "ref", "var", "class", "struct", "enum", "mixin", "include", "extension",
    "newtype", "type", "def", "public", "private", "protected", "consume", "inline", "self",
    "Self", "init", "super", "return", "yield", "async", "await", "if", "elsif", "else", "match",
    "while", "for", "in", "loop", "do", "end", "break", "continue", "where", "as", "some", "any",
    "layout", "module", "use", "package", "unsafe", "lib", "nil", "true", "false", "Some", "Ok",
    "Err", "macro", "static", "const", "when", "unless",
];

/// Look up a keyword from an identifier string.
pub fn lookup_keyword(ident: &str) -> Option<TokenKind> {
    match ident {
        // Variable & Binding
        "let" => Some(TokenKind::Let),
        // `mut` removed — use `var` for every writable position.
        "move" => Some(TokenKind::Move),
        "ref" => Some(TokenKind::Ref),
        "var" => Some(TokenKind::Var),

        // Type Definitions
        "class" => Some(TokenKind::Class),
        "struct" => Some(TokenKind::Struct),
        "enum" => Some(TokenKind::Enum),
        // `trait` removed — use `mixin`.
        // `impl` removed — use `include` inside a type body or `extension`
        // for conditional / external method blocks.
        "mixin" => Some(TokenKind::Mixin),
        "include" => Some(TokenKind::Include),
        "extension" => Some(TokenKind::Extension),
        "newtype" => Some(TokenKind::Newtype),
        "type" => Some(TokenKind::Type),

        // Functions & Methods
        "def" => Some(TokenKind::Def),
        // `pub` removed — use `public` section marker or `public` prefix.
        "public" => Some(TokenKind::Public),
        "private" => Some(TokenKind::Private),
        "protected" => Some(TokenKind::Protected),
        "consume" => Some(TokenKind::Consume),
        "inline" => Some(TokenKind::Inline),
        "self" => Some(TokenKind::SelfValue),
        "Self" => Some(TokenKind::SelfType),
        "init" => Some(TokenKind::Init),
        "super" => Some(TokenKind::Super),
        "return" => Some(TokenKind::Return),
        "yield" => Some(TokenKind::Yield),
        "async" => Some(TokenKind::Async),
        "await" => Some(TokenKind::Await),

        // Control Flow
        "if" => Some(TokenKind::If),
        "elsif" => Some(TokenKind::Elsif),
        "else" => Some(TokenKind::Else),
        "match" => Some(TokenKind::Match),
        "while" => Some(TokenKind::While),
        "for" => Some(TokenKind::For),
        "in" => Some(TokenKind::In),
        "loop" => Some(TokenKind::Loop),
        "do" => Some(TokenKind::Do),
        "end" => Some(TokenKind::End),
        "break" => Some(TokenKind::Break),
        "continue" => Some(TokenKind::Continue),

        // Type System
        "where" => Some(TokenKind::Where),
        "as" => Some(TokenKind::As),
        // `dyn` removed — use `any Mixin` for dyn-trait positions.
        "some" => Some(TokenKind::SomeBound),
        "any" => Some(TokenKind::AnyBound),
        // `derive` removed — auto-include replaces it.
        "layout" => Some(TokenKind::Layout),

        // Modules
        "module" => Some(TokenKind::Module),
        "use" => Some(TokenKind::Use),
        "package" => Some(TokenKind::Package),

        // Safety
        "unsafe" => Some(TokenKind::Unsafe),

        // FFI & Interop
        "lib" => Some(TokenKind::Lib),
        // `null` removed — use `nil` (raw-pointer null literal).
        "nil" => Some(TokenKind::Nil),

        // Literals
        "true" => Some(TokenKind::True),
        "false" => Some(TokenKind::False),
        // `None` removed — use `nil` (Option::None literal).
        "Some" => Some(TokenKind::SomeKw),
        "Ok" => Some(TokenKind::OkKw),
        "Err" => Some(TokenKind::ErrKw),

        // Reserved
        "macro" => Some(TokenKind::Macro),
        // `crate` removed — use `package`.
        // `extern` removed — use `lib`.
        "static" => Some(TokenKind::Static),
        "const" => Some(TokenKind::Const),
        "when" => Some(TokenKind::When),
        "unless" => Some(TokenKind::Unless),

        _ => None,
    }
}
