use super::*;

fn lex(input: &str) -> Vec<Token> {
    let mut lexer = Lexer::new(input);
    lexer.tokenize().expect("lexer should succeed")
}

fn lex_kinds(input: &str) -> Vec<TokenKind> {
    lex(input).into_iter().map(|t| t.kind).collect()
}

fn lex_with_errors(input: &str) -> (Vec<Token>, Vec<crate::diagnostics::Diagnostic>) {
    let mut lexer = Lexer::new(input);
    match lexer.tokenize() {
        Ok(tokens) => (tokens, vec![]),
        Err(diags) => (lexer.tokens.clone(), diags),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Keywords
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_all_keywords() {
    let pairs = vec![
        ("let", TokenKind::Let),
        ("mut", TokenKind::Mut),
        ("move", TokenKind::Move),
        ("ref", TokenKind::Ref),
        ("class", TokenKind::Class),
        ("struct", TokenKind::Struct),
        ("enum", TokenKind::Enum),
        ("trait", TokenKind::Trait),
        ("impl", TokenKind::Impl),
        ("newtype", TokenKind::Newtype),
        ("type", TokenKind::Type),
        ("def", TokenKind::Def),
        ("pub", TokenKind::Pub),
        ("protected", TokenKind::Protected),
        ("consume", TokenKind::Consume),
        ("self", TokenKind::SelfValue),
        ("Self", TokenKind::SelfType),
        ("init", TokenKind::Init),
        ("super", TokenKind::Super),
        ("return", TokenKind::Return),
        ("yield", TokenKind::Yield),
        ("async", TokenKind::Async),
        ("await", TokenKind::Await),
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
        ("where", TokenKind::Where),
        ("as", TokenKind::As),
        ("dyn", TokenKind::Dyn),
        ("derive", TokenKind::Derive),
        ("module", TokenKind::Module),
        ("use", TokenKind::Use),
        ("unsafe", TokenKind::Unsafe),
        ("true", TokenKind::True),
        ("false", TokenKind::False),
        ("None", TokenKind::NoneKw),
        ("Some", TokenKind::SomeKw),
        ("Ok", TokenKind::OkKw),
        ("Err", TokenKind::ErrKw),
        ("macro", TokenKind::Macro),
        ("crate", TokenKind::Crate),
        ("extern", TokenKind::Extern),
        ("static", TokenKind::Static),
        ("const", TokenKind::Const),
        ("when", TokenKind::When),
        ("unless", TokenKind::Unless),
    ];

    for (input, expected) in pairs {
        let kinds = lex_kinds(input);
        assert_eq!(
            kinds,
            vec![expected.clone(), TokenKind::Eof],
            "keyword '{}' did not produce expected token",
            input
        );
    }
}

#[test]
fn test_keyword_not_prefix() {
    // "letter" should not be lexed as "let" + "ter"
    let kinds = lex_kinds("letter");
    assert_eq!(
        kinds,
        vec![TokenKind::Identifier("letter".into()), TokenKind::Eof]
    );
}

#[test]
fn test_unreserved_keywords_lex_as_identifiers() {
    // P0.12 / TEC-13: `actor`, `send`, `receive` are no longer reserved and
    // must lex as plain identifiers so users can name their own functions
    // and variables with these words.
    for name in ["actor", "send", "receive"] {
        let kinds = lex_kinds(name);
        assert_eq!(
            kinds,
            vec![TokenKind::Identifier(name.into()), TokenKind::Eof],
            "'{}' should lex as an identifier, not a keyword",
            name
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Operators
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_single_char_operators() {
    let pairs = vec![
        ("+", TokenKind::Plus),
        ("-", TokenKind::Minus),
        ("*", TokenKind::Star),
        ("/", TokenKind::Slash),
        ("%", TokenKind::Percent),
        ("=", TokenKind::Eq),
        ("!", TokenKind::Bang),
        ("<", TokenKind::Lt),
        (">", TokenKind::Gt),
        ("&", TokenKind::Amp),
        ("|", TokenKind::Pipe),
        ("^", TokenKind::Caret),
        (".", TokenKind::Dot),
        ("?", TokenKind::Question),
        ("@", TokenKind::At),
        (":", TokenKind::Colon),
        (";", TokenKind::Semicolon),
        (",", TokenKind::Comma),
        ("(", TokenKind::LParen),
        (")", TokenKind::RParen),
        ("[", TokenKind::LBracket),
        ("]", TokenKind::RBracket),
        ("{", TokenKind::LBrace),
        ("}", TokenKind::RBrace),
    ];

    for (input, expected) in pairs {
        let kinds = lex_kinds(input);
        assert_eq!(
            kinds,
            vec![expected.clone(), TokenKind::Eof],
            "operator '{}' did not produce expected token",
            input
        );
    }
}

#[test]
fn test_multi_char_operators() {
    let pairs = vec![
        ("==", TokenKind::EqEq),
        ("!=", TokenKind::NotEq),
        ("<=", TokenKind::LtEq),
        (">=", TokenKind::GtEq),
        ("&&", TokenKind::AmpAmp),
        ("||", TokenKind::PipePipe),
        ("<<", TokenKind::Shl),
        (">>", TokenKind::Shr),
        ("+=", TokenKind::PlusEq),
        ("-=", TokenKind::MinusEq),
        ("*=", TokenKind::StarEq),
        ("/=", TokenKind::SlashEq),
        ("%=", TokenKind::PercentEq),
        ("..", TokenKind::DotDot),
        ("..=", TokenKind::DotDotEq),
        ("->", TokenKind::Arrow),
        ("?.", TokenKind::QuestionDot),
        ("::", TokenKind::ColonColon),
    ];

    for (input, expected) in pairs {
        let kinds = lex_kinds(input);
        assert_eq!(
            kinds,
            vec![expected.clone(), TokenKind::Eof],
            "operator '{}' did not produce expected token",
            input
        );
    }
}

#[test]
fn test_amp_mut() {
    let kinds = lex_kinds("&mut");
    assert_eq!(kinds, vec![TokenKind::AmpMut, TokenKind::Eof]);
}

#[test]
fn test_amp_mut_not_partial() {
    // &mutable should be & + identifier "mutable"
    let kinds = lex_kinds("&mutable");
    assert_eq!(
        kinds,
        vec![
            TokenKind::Amp,
            TokenKind::Identifier("mutable".into()),
            TokenKind::Eof
        ]
    );
}

#[test]
fn test_amp_mut_with_value() {
    let kinds = lex_kinds("&mut value");
    assert_eq!(
        kinds,
        vec![
            TokenKind::AmpMut,
            TokenKind::Identifier("value".into()),
            TokenKind::Eof,
        ]
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Integer Literals
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_decimal_integers() {
    let kinds = lex_kinds("42");
    assert_eq!(kinds, vec![TokenKind::IntLiteral(42, None), TokenKind::Eof]);
}

#[test]
fn test_integer_with_underscores() {
    let kinds = lex_kinds("1_000_000");
    assert_eq!(
        kinds,
        vec![TokenKind::IntLiteral(1_000_000, None), TokenKind::Eof]
    );
}

#[test]
fn test_hex_literal() {
    let kinds = lex_kinds("0xFF");
    assert_eq!(
        kinds,
        vec![TokenKind::IntLiteral(0xFF, None), TokenKind::Eof]
    );
}

#[test]
fn test_hex_with_underscores() {
    let kinds = lex_kinds("0xFF_FF");
    assert_eq!(
        kinds,
        vec![TokenKind::IntLiteral(0xFFFF, None), TokenKind::Eof]
    );
}

#[test]
fn test_binary_literal() {
    let kinds = lex_kinds("0b1010_0101");
    assert_eq!(
        kinds,
        vec![TokenKind::IntLiteral(0b1010_0101, None), TokenKind::Eof]
    );
}

#[test]
fn test_octal_literal() {
    let kinds = lex_kinds("0o777");
    assert_eq!(
        kinds,
        vec![TokenKind::IntLiteral(0o777, None), TokenKind::Eof]
    );
}

#[test]
fn test_integer_with_suffix() {
    let kinds = lex_kinds("42i8");
    assert_eq!(
        kinds,
        vec![
            TokenKind::IntLiteral(42, Some(NumericSuffix::I8)),
            TokenKind::Eof
        ]
    );
    let kinds = lex_kinds("42u64");
    assert_eq!(
        kinds,
        vec![
            TokenKind::IntLiteral(42, Some(NumericSuffix::U64)),
            TokenKind::Eof
        ]
    );
    let kinds = lex_kinds("42usize");
    assert_eq!(
        kinds,
        vec![
            TokenKind::IntLiteral(42, Some(NumericSuffix::USize)),
            TokenKind::Eof
        ]
    );
    let kinds = lex_kinds("42isize");
    assert_eq!(
        kinds,
        vec![
            TokenKind::IntLiteral(42, Some(NumericSuffix::ISize)),
            TokenKind::Eof
        ]
    );
    let kinds = lex_kinds("42u");
    assert_eq!(
        kinds,
        vec![
            TokenKind::IntLiteral(42, Some(NumericSuffix::U)),
            TokenKind::Eof
        ]
    );
}

#[test]
fn test_zero() {
    let kinds = lex_kinds("0");
    assert_eq!(kinds, vec![TokenKind::IntLiteral(0, None), TokenKind::Eof]);
}

// ═══════════════════════════════════════════════════════════════════════════
// Float Literals
// ═══════════════════════════════════════════════════════════════════════════

#[test]
#[allow(clippy::approx_constant)]
fn test_float_basic() {
    let kinds = lex_kinds("3.14");
    assert_eq!(
        kinds,
        vec![TokenKind::FloatLiteral(3.14, None), TokenKind::Eof]
    );
}

#[test]
fn test_float_scientific() {
    let kinds = lex_kinds("1.0e10");
    assert_eq!(
        kinds,
        vec![TokenKind::FloatLiteral(1.0e10, None), TokenKind::Eof]
    );
}

#[test]
fn test_float_scientific_negative_exponent() {
    let kinds = lex_kinds("1.0e-3");
    assert_eq!(
        kinds,
        vec![TokenKind::FloatLiteral(1.0e-3, None), TokenKind::Eof]
    );
}

#[test]
#[allow(clippy::approx_constant)]
fn test_float_with_suffix() {
    let kinds = lex_kinds("3.14f32");
    assert_eq!(
        kinds,
        vec![
            TokenKind::FloatLiteral(3.14, Some(NumericSuffix::F32)),
            TokenKind::Eof
        ]
    );
    let kinds = lex_kinds("3.14f64");
    assert_eq!(
        kinds,
        vec![
            TokenKind::FloatLiteral(3.14, Some(NumericSuffix::F64)),
            TokenKind::Eof
        ]
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Range vs Float disambiguation
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_range_not_float() {
    // 0..10 must be 0, .., 10 — not a float
    let kinds = lex_kinds("0..10");
    assert_eq!(
        kinds,
        vec![
            TokenKind::IntLiteral(0, None),
            TokenKind::DotDot,
            TokenKind::IntLiteral(10, None),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn test_inclusive_range() {
    let kinds = lex_kinds("0..=10");
    assert_eq!(
        kinds,
        vec![
            TokenKind::IntLiteral(0, None),
            TokenKind::DotDotEq,
            TokenKind::IntLiteral(10, None),
            TokenKind::Eof,
        ]
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// String Literals
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_simple_string() {
    let kinds = lex_kinds(r#""hello""#);
    assert_eq!(
        kinds,
        vec![TokenKind::StringLiteral("hello".into()), TokenKind::Eof]
    );
}

#[test]
fn test_string_with_escapes() {
    let kinds = lex_kinds(r#""hello\nworld""#);
    assert_eq!(
        kinds,
        vec![
            TokenKind::StringLiteral("hello\nworld".into()),
            TokenKind::Eof
        ]
    );
}

#[test]
fn test_string_with_unicode_escape() {
    let kinds = lex_kinds(r#""\u{1F600}""#);
    assert_eq!(
        kinds,
        vec![TokenKind::StringLiteral("\u{1F600}".into()), TokenKind::Eof]
    );
}

#[test]
fn test_string_interpolation() {
    let kinds = lex_kinds(r#""hello #{name}""#);
    match &kinds[0] {
        TokenKind::InterpolatedString(parts) => {
            assert_eq!(parts.len(), 2);
            assert_eq!(parts[0], StringPart::Literal("hello ".into()));
            match &parts[1] {
                StringPart::Expr { tokens, .. } => {
                    assert_eq!(tokens.len(), 1);
                    assert_eq!(tokens[0].kind, TokenKind::Identifier("name".into()));
                }
                _ => panic!("expected expr part"),
            }
        }
        _ => panic!("expected interpolated string, got {:?}", kinds[0]),
    }
}

#[test]
fn test_string_interpolation_with_expression() {
    let kinds = lex_kinds(r#""result: #{a + b}""#);
    match &kinds[0] {
        TokenKind::InterpolatedString(parts) => {
            assert_eq!(parts.len(), 2);
            assert_eq!(parts[0], StringPart::Literal("result: ".into()));
            match &parts[1] {
                StringPart::Expr { tokens, .. } => {
                    assert_eq!(tokens.len(), 3);
                    assert_eq!(tokens[0].kind, TokenKind::Identifier("a".into()));
                    assert_eq!(tokens[1].kind, TokenKind::Plus);
                    assert_eq!(tokens[2].kind, TokenKind::Identifier("b".into()));
                }
                _ => panic!("expected expr part"),
            }
        }
        _ => panic!("expected interpolated string"),
    }
}

#[test]
fn test_escaped_interpolation() {
    let kinds = lex_kinds(r#""\#{not interpolation}""#);
    assert_eq!(
        kinds,
        vec![
            TokenKind::StringLiteral("#{not interpolation}".into()),
            TokenKind::Eof
        ]
    );
}

#[test]
fn test_nested_string_interpolation() {
    // "#{a + "inner"}" — interpolation containing a string
    let input = "\"#{a + \"inner\"}\"";
    let kinds = lex_kinds(input);
    match &kinds[0] {
        TokenKind::InterpolatedString(parts) => {
            assert_eq!(parts.len(), 1);
            match &parts[0] {
                StringPart::Expr { tokens, .. } => {
                    assert_eq!(tokens[0].kind, TokenKind::Identifier("a".into()));
                    assert_eq!(tokens[1].kind, TokenKind::Plus);
                    assert_eq!(tokens[2].kind, TokenKind::StringLiteral("inner".into()));
                }
                _ => panic!("expected expr"),
            }
        }
        _ => panic!("expected interpolated string"),
    }
}

/// Phase 2 #06.B: bare `"#{x}"` carries the default `FormatSpec`.
#[test]
fn test_interpolation_default_spec() {
    let kinds = lex_kinds(r#""val=#{x}""#);
    match &kinds[0] {
        TokenKind::InterpolatedString(parts) => match &parts[1] {
            StringPart::Expr { spec, tokens } => {
                assert!(spec.is_default(), "expected default spec, got {:?}", spec);
                assert_eq!(tokens.len(), 1);
                assert_eq!(tokens[0].kind, TokenKind::Identifier("x".into()));
            }
            _ => panic!("expected Expr part"),
        },
        _ => panic!("expected InterpolatedString"),
    }
}

/// Phase 2 #06.B: `"#{x:?}"` sets the `debug` flag.
#[test]
fn test_interpolation_debug_spec() {
    let kinds = lex_kinds(r#""val=#{x:?}""#);
    match &kinds[0] {
        TokenKind::InterpolatedString(parts) => match &parts[1] {
            StringPart::Expr { spec, tokens } => {
                assert!(spec.debug, "expected debug=true");
                assert!(spec.width.is_none());
                assert_eq!(tokens.len(), 1);
                assert_eq!(tokens[0].kind, TokenKind::Identifier("x".into()));
            }
            _ => panic!("expected Expr part"),
        },
        _ => panic!("expected InterpolatedString"),
    }
}

/// Phase 2 #06.B: `"#{x:>10}"` parses align=`>` and width=10.
#[test]
fn test_interpolation_width_spec() {
    // Use plain string with escapes — `r#""#{...}"#` collides with
    // Rust's raw-string `#` delimiters.
    let kinds = lex_kinds("\"#{x:>10}\"");
    match &kinds[0] {
        TokenKind::InterpolatedString(parts) => match &parts[0] {
            StringPart::Expr { spec, .. } => {
                assert_eq!(spec.align, Some('>'));
                assert_eq!(spec.width, Some(10));
                assert!(spec.precision.is_none());
                assert!(!spec.debug);
            }
            _ => panic!("expected Expr part"),
        },
        _ => panic!("expected InterpolatedString"),
    }
}

/// Phase 2 #06.B: `"#{x:.2}"` parses precision=2 (e.g. for floats).
#[test]
fn test_interpolation_precision_spec() {
    let kinds = lex_kinds("\"#{pi:.2}\"");
    match &kinds[0] {
        TokenKind::InterpolatedString(parts) => match &parts[0] {
            StringPart::Expr { spec, .. } => {
                assert_eq!(spec.precision, Some(2));
                assert!(spec.width.is_none());
                assert!(spec.align.is_none());
            }
            _ => panic!("expected Expr part"),
        },
        _ => panic!("expected InterpolatedString"),
    }
}

/// Phase 2 #06.B regression: `:` inside parens (named arguments) or
/// brackets (generic args) must NOT be treated as a format-spec start.
/// Pre-fix bug: lexer truncated `area(Shape.Circle(radius: 2.0))` at
/// the `:` after `radius`, dropping the rest of the expression and
/// emitting an empty interpolation. Result: `puts "#{...}"` produced
/// no output (assertion fail in
/// rivenc/tests/installed_binary.rs::enum_with_match).
#[test]
fn test_interpolation_named_arg_colon_is_not_spec() {
    let kinds = lex_kinds("\"#{Shape.Circle(radius: 2.0)}\"");
    match &kinds[0] {
        TokenKind::InterpolatedString(parts) => match &parts[0] {
            StringPart::Expr { tokens, spec } => {
                assert!(
                    spec.is_default(),
                    "named-arg `:` must NOT trigger spec mode, got {:?}",
                    spec
                );
                let kinds: Vec<&TokenKind> = tokens.iter().map(|t| &t.kind).collect();
                assert!(
                    kinds
                        .iter()
                        .any(|k| matches!(k, TokenKind::FloatLiteral(_, _))),
                    "expected FloatLiteral 2.0 in tokens, got {:?}",
                    kinds
                );
            }
            _ => panic!("expected Expr part"),
        },
        _ => panic!("expected InterpolatedString"),
    }
}

/// Phase 2 #06.B regression: `::` path qualifier (`Vec::<Int>::new`-
/// style at the lexer level — Riven uses `Vec[Int]::new` but the
/// lexer needs to handle path `::` regardless) must NOT trigger spec
/// mode. The check `peek_at(1) != Some(':')` in the lexer guards
/// this case.
#[test]
fn test_interpolation_path_colon_is_not_spec() {
    let kinds = lex_kinds("\"#{a::b}\"");
    match &kinds[0] {
        TokenKind::InterpolatedString(parts) => match &parts[0] {
            StringPart::Expr { spec, tokens } => {
                assert!(
                    spec.is_default(),
                    "path `::` must NOT trigger spec mode, got {:?}",
                    spec
                );
                let kinds: Vec<&TokenKind> = tokens.iter().map(|t| &t.kind).collect();
                assert!(
                    kinds.iter().any(|k| matches!(k, TokenKind::ColonColon)),
                    "expected ColonColon in tokens, got {:?}",
                    kinds
                );
            }
            _ => panic!("expected Expr part"),
        },
        _ => panic!("expected InterpolatedString"),
    }
}

/// Phase 2 #06.B regression: `:` inside generic brackets (Riven uses
/// `Vec[T]` for generics) must NOT trigger spec mode. Covers nested
/// generic args + a colon (for type bounds, `T: Trait` etc.).
#[test]
fn test_interpolation_bracket_colon_is_not_spec() {
    let kinds = lex_kinds("\"#{f[T: Foo]}\"");
    match &kinds[0] {
        TokenKind::InterpolatedString(parts) => match &parts[0] {
            StringPart::Expr { spec, .. } => {
                assert!(
                    spec.is_default(),
                    "`:` inside [...] must NOT trigger spec mode, got {:?}",
                    spec
                );
            }
            _ => panic!("expected Expr part"),
        },
        _ => panic!("expected InterpolatedString"),
    }
}

/// Phase 2 #06.B: `"#{x:*<10.3?}"` parses fill=`*`, align=`<`,
/// width=10, precision=3, debug=true. Exercises the full spec.
#[test]
fn test_interpolation_full_spec() {
    let kinds = lex_kinds("\"#{x:*<10.3?}\"");
    match &kinds[0] {
        TokenKind::InterpolatedString(parts) => match &parts[0] {
            StringPart::Expr { spec, .. } => {
                assert_eq!(spec.fill, Some('*'));
                assert_eq!(spec.align, Some('<'));
                assert_eq!(spec.width, Some(10));
                assert_eq!(spec.precision, Some(3));
                assert!(spec.debug);
            }
            _ => panic!("expected Expr part"),
        },
        _ => panic!("expected InterpolatedString"),
    }
}

#[test]
fn test_multiline_string() {
    let input = "\"\"\"
  hello
  world
\"\"\"";
    let kinds = lex_kinds(input);
    assert_eq!(
        kinds,
        vec![
            TokenKind::StringLiteral("hello\nworld".into()),
            TokenKind::Eof
        ]
    );
}

#[test]
fn test_raw_string() {
    let kinds = lex_kinds(r#"r"no\escape""#);
    assert_eq!(
        kinds,
        vec![
            TokenKind::StringLiteral(r"no\escape".into()),
            TokenKind::Eof
        ]
    );
}

#[test]
fn test_raw_string_with_hashes() {
    let kinds = lex_kinds(r###"r#"can contain "quotes""#"###);
    assert_eq!(
        kinds,
        vec![
            TokenKind::StringLiteral(r#"can contain "quotes""#.into()),
            TokenKind::Eof
        ]
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Character Literals
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_char_literal() {
    let kinds = lex_kinds("'a'");
    assert_eq!(kinds, vec![TokenKind::CharLiteral('a'), TokenKind::Eof]);
}

#[test]
fn test_char_escape() {
    let kinds = lex_kinds(r"'\n'");
    assert_eq!(kinds, vec![TokenKind::CharLiteral('\n'), TokenKind::Eof]);
}

#[test]
fn test_char_unicode() {
    let kinds = lex_kinds(r"'\u{1F600}'");
    assert_eq!(
        kinds,
        vec![TokenKind::CharLiteral('\u{1F600}'), TokenKind::Eof]
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Boolean Literals
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_booleans() {
    let kinds = lex_kinds("true");
    assert_eq!(kinds, vec![TokenKind::True, TokenKind::Eof]);
    let kinds = lex_kinds("false");
    assert_eq!(kinds, vec![TokenKind::False, TokenKind::Eof]);
}

// ═══════════════════════════════════════════════════════════════════════════
// Identifiers
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_snake_case_identifier() {
    let kinds = lex_kinds("user_name");
    assert_eq!(
        kinds,
        vec![TokenKind::Identifier("user_name".into()), TokenKind::Eof]
    );
}

#[test]
fn test_type_identifier() {
    let kinds = lex_kinds("TaskList");
    assert_eq!(
        kinds,
        vec![TokenKind::TypeIdentifier("TaskList".into()), TokenKind::Eof]
    );
}

#[test]
fn test_identifier_with_question_suffix() {
    // ? is emitted as a separate token; parser combines identifier + ? for method names
    let kinds = lex_kinds("is_empty?");
    assert_eq!(
        kinds,
        vec![
            TokenKind::Identifier("is_empty".into()),
            TokenKind::Question,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn test_identifier_with_bang_suffix() {
    let kinds = lex_kinds("unwrap!");
    assert_eq!(
        kinds,
        vec![TokenKind::Identifier("unwrap!".into()), TokenKind::Eof]
    );
}

#[test]
fn test_identifier_with_underscore_prefix() {
    let kinds = lex_kinds("_unused");
    assert_eq!(
        kinds,
        vec![TokenKind::Identifier("_unused".into()), TokenKind::Eof]
    );
}

#[test]
fn test_single_underscore() {
    let kinds = lex_kinds("_");
    assert_eq!(
        kinds,
        vec![TokenKind::Identifier("_".into()), TokenKind::Eof]
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Comments
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_line_comment() {
    let kinds = lex_kinds("# this is a comment\nx");
    assert_eq!(
        kinds,
        vec![TokenKind::Identifier("x".into()), TokenKind::Eof]
    );
}

#[test]
fn test_block_comment() {
    let kinds = lex_kinds("#= block comment =# x");
    assert_eq!(
        kinds,
        vec![TokenKind::Identifier("x".into()), TokenKind::Eof]
    );
}

#[test]
fn test_nested_block_comment() {
    let kinds = lex_kinds("#= outer #= inner =# still outer =# x");
    assert_eq!(
        kinds,
        vec![TokenKind::Identifier("x".into()), TokenKind::Eof]
    );
}

#[test]
fn test_doc_comment() {
    let kinds = lex_kinds("## This is a doc comment");
    assert_eq!(
        kinds,
        vec![
            TokenKind::DocComment("This is a doc comment".into()),
            TokenKind::Eof
        ]
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Newline Handling
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_newline_as_statement_terminator() {
    let kinds = lex_kinds("a\nb");
    assert_eq!(
        kinds,
        vec![
            TokenKind::Identifier("a".into()),
            TokenKind::Newline,
            TokenKind::Identifier("b".into()),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn test_consecutive_newlines_collapsed() {
    let kinds = lex_kinds("a\n\n\nb");
    assert_eq!(
        kinds,
        vec![
            TokenKind::Identifier("a".into()),
            TokenKind::Newline,
            TokenKind::Identifier("b".into()),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn test_newline_suppressed_after_operator() {
    let kinds = lex_kinds("a +\nb");
    assert_eq!(
        kinds,
        vec![
            TokenKind::Identifier("a".into()),
            TokenKind::Plus,
            TokenKind::Identifier("b".into()),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn test_newline_suppressed_after_comma() {
    let kinds = lex_kinds("a,\nb");
    assert_eq!(
        kinds,
        vec![
            TokenKind::Identifier("a".into()),
            TokenKind::Comma,
            TokenKind::Identifier("b".into()),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn test_newline_suppressed_after_opening_delimiter() {
    let kinds = lex_kinds("(\na\n)");
    assert_eq!(
        kinds,
        vec![
            TokenKind::LParen,
            TokenKind::Identifier("a".into()),
            TokenKind::Newline,
            TokenKind::RParen,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn test_newline_suppressed_after_dot() {
    let kinds = lex_kinds("foo.\nbar");
    assert_eq!(
        kinds,
        vec![
            TokenKind::Identifier("foo".into()),
            TokenKind::Dot,
            TokenKind::Identifier("bar".into()),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn test_newline_suppressed_after_arrow() {
    let kinds = lex_kinds("def f ->\nInt");
    assert_eq!(
        kinds,
        vec![
            TokenKind::Def,
            TokenKind::Identifier("f".into()),
            TokenKind::Arrow,
            TokenKind::TypeIdentifier("Int".into()),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn test_no_leading_newline() {
    let kinds = lex_kinds("\n\na");
    assert_eq!(
        kinds,
        vec![TokenKind::Identifier("a".into()), TokenKind::Eof]
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Error Recovery
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_unterminated_string() {
    let (_, diags) = lex_with_errors(r#""hello"#);
    assert!(!diags.is_empty());
    assert!(diags[0].message.contains("unterminated"));
}

#[test]
fn test_invalid_escape() {
    let (_, diags) = lex_with_errors(r#""\q""#);
    assert!(!diags.is_empty());
    assert!(diags[0].message.contains("invalid escape"));
}

#[test]
fn test_unterminated_block_comment() {
    let (_, diags) = lex_with_errors("#= no close");
    assert!(!diags.is_empty());
    assert!(diags[0].message.contains("unterminated block comment"));
}

#[test]
fn test_invalid_hex_literal() {
    let (_, diags) = lex_with_errors("0x");
    assert!(!diags.is_empty());
    assert!(diags[0].message.contains("no digits"));
}

#[test]
fn test_unexpected_character() {
    let (_, diags) = lex_with_errors("~");
    assert!(!diags.is_empty());
    assert!(diags[0].message.contains("unexpected character"));
}

// ═══════════════════════════════════════════════════════════════════════════
// At symbol (@)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_at_in_constructor() {
    let kinds = lex_kinds("@name");
    assert_eq!(
        kinds,
        vec![
            TokenKind::At,
            TokenKind::Identifier("name".into()),
            TokenKind::Eof,
        ]
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Complex expressions
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_let_binding() {
    let kinds = lex_kinds("let x = 42");
    assert_eq!(
        kinds,
        vec![
            TokenKind::Let,
            TokenKind::Identifier("x".into()),
            TokenKind::Eq,
            TokenKind::IntLiteral(42, None),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn test_method_definition() {
    let kinds = lex_kinds("pub def mut assign(name: String)");
    assert_eq!(
        kinds,
        vec![
            TokenKind::Pub,
            TokenKind::Def,
            TokenKind::Mut,
            TokenKind::Identifier("assign".into()),
            TokenKind::LParen,
            TokenKind::Identifier("name".into()),
            TokenKind::Colon,
            TokenKind::TypeIdentifier("String".into()),
            TokenKind::RParen,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn test_generic_type() {
    let kinds = lex_kinds("Vec[T]");
    assert_eq!(
        kinds,
        vec![
            TokenKind::TypeIdentifier("Vec".into()),
            TokenKind::LBracket,
            TokenKind::TypeIdentifier("T".into()),
            TokenKind::RBracket,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn test_safe_navigation() {
    let kinds = lex_kinds("user?.name");
    assert_eq!(
        kinds,
        vec![
            TokenKind::Identifier("user".into()),
            TokenKind::QuestionDot,
            TokenKind::Identifier("name".into()),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn test_try_operator() {
    let kinds = lex_kinds("result?");
    assert_eq!(
        kinds,
        vec![
            TokenKind::Identifier("result".into()),
            TokenKind::Question,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn test_block_with_pipes() {
    let kinds = lex_kinds("{ |x| x + 1 }");
    assert_eq!(
        kinds,
        vec![
            TokenKind::LBrace,
            TokenKind::Pipe,
            TokenKind::Identifier("x".into()),
            TokenKind::Pipe,
            TokenKind::Identifier("x".into()),
            TokenKind::Plus,
            TokenKind::IntLiteral(1, None),
            TokenKind::RBrace,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn test_match_with_arrow() {
    let kinds = lex_kinds("match x\n  1 -> true\nend");
    assert_eq!(
        kinds,
        vec![
            TokenKind::Match,
            TokenKind::Identifier("x".into()),
            TokenKind::Newline,
            TokenKind::IntLiteral(1, None),
            TokenKind::Arrow,
            TokenKind::True,
            TokenKind::Newline,
            TokenKind::End,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn test_span_tracking() {
    let tokens = lex("let x = 42");
    assert_eq!(tokens[0].span.line, 1);
    assert_eq!(tokens[0].span.column, 1);
    // "x" starts at column 5
    assert_eq!(tokens[1].span.line, 1);
    assert_eq!(tokens[1].span.column, 5);
}

#[test]
fn test_pipe_in_block() {
    // The pipe should be a Pipe, not PipePipe
    let kinds = lex_kinds("|x|");
    assert_eq!(
        kinds,
        vec![
            TokenKind::Pipe,
            TokenKind::Identifier("x".into()),
            TokenKind::Pipe,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn test_class_inheritance() {
    let kinds = lex_kinds("class TimedTask < Task");
    assert_eq!(
        kinds,
        vec![
            TokenKind::Class,
            TokenKind::TypeIdentifier("TimedTask".into()),
            TokenKind::Lt,
            TokenKind::TypeIdentifier("Task".into()),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn test_hash_in_interpolated_string() {
    // "Task ##{id} not found" — literal '#' followed by interpolation
    let kinds = lex_kinds(r#""Task ##{id} not found""#);
    match &kinds[0] {
        TokenKind::InterpolatedString(parts) => {
            // Should be: "Task #", expr(id), " not found"
            assert_eq!(parts.len(), 3);
            assert_eq!(parts[0], StringPart::Literal("Task #".into()));
            match &parts[1] {
                StringPart::Expr { tokens, .. } => {
                    assert_eq!(tokens[0].kind, TokenKind::Identifier("id".into()));
                }
                _ => panic!("expected expr"),
            }
            assert_eq!(parts[2], StringPart::Literal(" not found".into()));
        }
        _ => panic!("expected interpolated string, got {:?}", kinds[0]),
    }
}

#[test]
fn test_semicolons() {
    let kinds = lex_kinds("a; b");
    assert_eq!(
        kinds,
        vec![
            TokenKind::Identifier("a".into()),
            TokenKind::Semicolon,
            TokenKind::Identifier("b".into()),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn test_newline_suppressed_after_pipe() {
    // Pipe is a continuation context (for block parameters)
    let kinds = lex_kinds("{ |\nx\n| x }");
    assert_eq!(
        kinds,
        vec![
            TokenKind::LBrace,
            TokenKind::Pipe,
            TokenKind::Identifier("x".into()),
            TokenKind::Newline,
            TokenKind::Pipe,
            TokenKind::Identifier("x".into()),
            TokenKind::RBrace,
            TokenKind::Eof,
        ]
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Lifetimes
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_lifetime() {
    let kinds = lex_kinds("'a");
    assert_eq!(kinds, vec![TokenKind::Lifetime("a".into()), TokenKind::Eof]);
}

#[test]
fn test_lifetime_long_name() {
    let kinds = lex_kinds("'input");
    assert_eq!(
        kinds,
        vec![TokenKind::Lifetime("input".into()), TokenKind::Eof]
    );
}

#[test]
fn test_char_literal_still_works() {
    // 'a' (with closing quote) is still a char literal
    let kinds = lex_kinds("'a'");
    assert_eq!(kinds, vec![TokenKind::CharLiteral('a'), TokenKind::Eof]);
}

// ═══════════════════════════════════════════════════════════════════════════
// Backslash Continuation
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_backslash_continuation() {
    let kinds = lex_kinds("a + \\\nb");
    assert_eq!(
        kinds,
        vec![
            TokenKind::Identifier("a".into()),
            TokenKind::Plus,
            TokenKind::Identifier("b".into()),
            TokenKind::Eof,
        ]
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Additional Edge Cases
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_empty_string() {
    let kinds = lex_kinds(r#""""#);
    assert_eq!(
        kinds,
        vec![TokenKind::StringLiteral("".into()), TokenKind::Eof]
    );
}

#[test]
fn test_escaped_single_quote_in_char() {
    let kinds = lex_kinds(r"'\''");
    assert_eq!(kinds, vec![TokenKind::CharLiteral('\''), TokenKind::Eof]);
}
