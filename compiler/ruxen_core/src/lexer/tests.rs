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
    // Post Ruby-naming migration (docs/specs/syntax/ruby-naming.spec.md):
    // `trait`, `impl`, `pub`, `dyn`, `derive`, `None`, `crate`, `extern`,
    // `null`, and `mut` are no longer keywords — they lex as ordinary
    // identifiers and the lookup_keyword table maps the new replacements
    // (`mixin`, `include`/`extension`, `public`/`private`, `any`/`some`,
    // `var`, …) instead.
    let pairs = vec![
        ("let", TokenKind::Let),
        ("move", TokenKind::Move),
        ("ref", TokenKind::Ref),
        ("var", TokenKind::Var),
        ("class", TokenKind::Class),
        ("struct", TokenKind::Struct),
        ("enum", TokenKind::Enum),
        ("mixin", TokenKind::Mixin),
        ("include", TokenKind::Include),
        ("extension", TokenKind::Extension),
        ("newtype", TokenKind::Newtype),
        ("type", TokenKind::Type),
        ("def", TokenKind::Def),
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
        ("some", TokenKind::SomeBound),
        ("any", TokenKind::AnyBound),
        ("layout", TokenKind::Layout),
        ("module", TokenKind::Module),
        ("use", TokenKind::Use),
        ("package", TokenKind::Package),
        ("unsafe", TokenKind::Unsafe),
        ("true", TokenKind::True),
        ("false", TokenKind::False),
        ("Some", TokenKind::SomeKw),
        ("Ok", TokenKind::OkKw),
        ("Err", TokenKind::ErrKw),
        ("nil", TokenKind::Nil),
        ("lib", TokenKind::Lib),
        ("macro", TokenKind::Macro),
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

#[test]
fn test_ruby_naming_legacy_keywords_lex_as_identifiers() {
    // ruby-naming.spec.md: the lowercase legacy keywords `trait`, `impl`,
    // `pub`, `dyn`, `derive`, `crate`, `extern`, `null` are unreserved and
    // must now lex as ordinary identifiers. Old source using them is
    // expected to fail later (in the parser) — but at the lexer level it
    // is a plain identifier, not a TokenKind for the legacy keyword.
    for name in [
        "trait", "impl", "pub", "dyn", "derive", "crate", "extern", "null",
    ] {
        let kinds = lex_kinds(name);
        assert_eq!(
            kinds,
            vec![TokenKind::Identifier(name.into()), TokenKind::Eof],
            "legacy keyword '{}' should now lex as an identifier",
            name
        );
    }
}

#[test]
fn test_ruby_naming_none_is_forbidden_use_nil() {
    // ruby-naming.spec.md §3.10: `None` is not a valid spelling. The single
    // empty literal is `nil` (Option::None, null, and unit). The lexer
    // rejects the identifier `None` with E0008 so the fix-it surfaces in
    // every position (expression, pattern, type) uniformly.
    let (_tokens, diags) = lex_with_errors("None");
    assert!(
        diags.iter().any(|d| d.code.as_deref() == Some("E0008")),
        "`None` should be rejected with E0008; got: {diags:?}"
    );
}

#[test]
fn test_nil_lexes_as_nil_keyword() {
    // The canonical empty literal lexes as the dedicated `Nil` token.
    let kinds = lex_kinds("nil");
    assert_eq!(kinds, vec![TokenKind::Nil, TokenKind::Eof]);
}

// ═══════════════════════════════════════════════════════════════════════════
// Operators
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_single_char_operators() {
    // For division (`/`) we prefix with an identifier `a ` so the
    // lexer's positional `/`-disambiguation (introduced with the
    // regex-literal token, E17xx) takes the division branch instead
    // of opening a regex literal. Identifier is NOT in the expression-
    // context token set, so `a /` lexes as `Identifier(a)` + `Slash`.
    let standalone_pairs = vec![
        ("+", TokenKind::Plus),
        ("-", TokenKind::Minus),
        ("*", TokenKind::Star),
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

    for (input, expected) in standalone_pairs {
        let kinds = lex_kinds(input);
        assert_eq!(
            kinds,
            vec![expected.clone(), TokenKind::Eof],
            "operator '{}' did not produce expected token",
            input
        );
    }

    // Division — see comment at top of test.
    let kinds = lex_kinds("a /");
    assert_eq!(
        kinds,
        vec![
            TokenKind::Identifier("a".into()),
            TokenKind::Slash,
            TokenKind::Eof
        ]
    );
}

#[test]
fn test_multi_char_operators() {
    // For `/=` we use the same `a /=` prefix trick as
    // `test_single_char_operators`: identifier is not in the
    // expression-context set, so the lexer's positional `/`-
    // disambiguation falls through to the SlashEq branch.
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
        ("%=", TokenKind::PercentEq),
        ("..", TokenKind::DotDot),
        ("...", TokenKind::DotDotDot),
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

    // Compound divide-assign — see comment at top of test.
    let kinds = lex_kinds("a /=");
    assert_eq!(
        kinds,
        vec![
            TokenKind::Identifier("a".into()),
            TokenKind::SlashEq,
            TokenKind::Eof
        ]
    );
}

#[test]
fn test_amp_var() {
    // Post Ruby-naming migration: `&var` is the writable-reference
    // single-token (the AmpMut variant is reused — internal name).
    let kinds = lex_kinds("&var");
    assert_eq!(kinds, vec![TokenKind::AmpMut, TokenKind::Eof]);
}

#[test]
fn test_amp_var_not_partial() {
    // &variable should be & + identifier "variable"
    let kinds = lex_kinds("&variable");
    assert_eq!(
        kinds,
        vec![
            TokenKind::Amp,
            TokenKind::Identifier("variable".into()),
            TokenKind::Eof
        ]
    );
}

#[test]
fn test_amp_var_with_value() {
    let kinds = lex_kinds("&var value");
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
fn test_ruby_ranges_dotdot_inclusive_dotdotdot_exclusive() {
    // ruby-naming.spec.md §3.10b: `..` is inclusive, `...` is exclusive.
    assert_eq!(
        lex_kinds("0..10"),
        vec![
            TokenKind::IntLiteral(0, None),
            TokenKind::DotDot,
            TokenKind::IntLiteral(10, None),
            TokenKind::Eof,
        ]
    );
    assert_eq!(
        lex_kinds("0...10"),
        vec![
            TokenKind::IntLiteral(0, None),
            TokenKind::DotDotDot,
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
/// ruxenc/tests/installed_binary.rs::enum_with_match).
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
/// style at the lexer level — Ruxen uses `Vec[Int]::new` but the
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

/// Phase 2 #06.B regression: `:` inside generic brackets (Ruxen uses
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

/// Phase 2 #06.B3: `.` without precision digits is malformed (E0007).
/// The lexer still recovers — the interpolation closes cleanly — but
/// a diagnostic is emitted.
#[test]
fn test_interpolation_spec_dot_without_precision_is_e0007() {
    let (_, diags) = lex_with_errors("\"#{x:5.}\"");
    assert!(
        diags.iter().any(|d| d.code.as_deref() == Some("E0007")
            && d.message.contains("`.`")
            && d.message.contains("precision")),
        "expected E0007 (`.` without precision), got {:?}",
        diags
    );
}

/// Phase 2 #06.B3: trailing junk after the well-formed prefix is
/// malformed (E0007). `"#{x:5xy}"` has width=5 followed by stray `xy`.
#[test]
fn test_interpolation_spec_trailing_junk_is_e0007() {
    let (_, diags) = lex_with_errors("\"#{x:5xy}\"");
    assert!(
        diags
            .iter()
            .any(|d| d.code.as_deref() == Some("E0007")
                && d.message.contains("unexpected character")),
        "expected E0007 (trailing junk), got {:?}",
        diags
    );
}

/// Phase 2 #06.B3: `?` followed by more characters is malformed —
/// `?` is the terminal flag and nothing should follow it before `}`.
#[test]
fn test_interpolation_spec_chars_after_debug_flag_is_e0007() {
    let (_, diags) = lex_with_errors("\"#{x:?nope}\"");
    assert!(
        diags.iter().any(|d| d.code.as_deref() == Some("E0007")),
        "expected E0007 (chars after ?), got {:?}",
        diags
    );
}

/// Phase 2 #06.B3: a totally unrecognised spec body is malformed.
/// `"#{x:@@@}"` has neither fill+align nor any other valid prefix
/// (the doubled `@` defeats the `fill align` lookahead because `@`
/// isn't an alignment marker).
#[test]
fn test_interpolation_spec_unrecognised_body_is_e0007() {
    let (_, diags) = lex_with_errors("\"#{x:@@@}\"");
    assert!(
        diags.iter().any(|d| d.code.as_deref() == Some("E0007")),
        "expected E0007 (unrecognised body), got {:?}",
        diags
    );
}

/// Phase 2 #06.B3: a *well-formed* spec must still produce zero
/// diagnostics. Regression guard for over-eager error emission.
#[test]
fn test_interpolation_well_formed_spec_no_diagnostics() {
    for src in [
        "\"#{x:?}\"",
        "\"#{x:>5}\"",
        "\"#{x:.2}\"",
        "\"#{x:*^10.2}\"",
        "\"#{x:*<10.3?}\"",
        "\"#{x:5}\"",
    ] {
        let (_, diags) = lex_with_errors(src);
        assert!(
            diags.is_empty(),
            "well-formed spec `{}` produced diagnostics: {:?}",
            src,
            diags
        );
    }
}

/// Phase 2 #06.B3: tolerated whitespace inside the spec must not
/// emit a diagnostic on its own. (Whitespace is the documented
/// tolerance carve-out in `lex_format_spec` step 6.)
#[test]
fn test_interpolation_spec_internal_whitespace_is_tolerated() {
    let (_, diags) = lex_with_errors("\"#{x:5 }\"");
    assert!(
        diags.is_empty(),
        "trailing whitespace inside spec should be tolerated, got {:?}",
        diags
    );
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
fn test_raw_string_single_quote() {
    // ruby-naming.spec.md §3.10a: single quotes are RAW strings — no
    // escape processing. `'no\escape'` keeps the backslash verbatim.
    let kinds = lex_kinds(r"'no\escape'");
    assert_eq!(
        kinds,
        vec![
            TokenKind::StringLiteral(r"no\escape".into()),
            TokenKind::Eof
        ]
    );
}

#[test]
fn test_raw_string_can_hold_double_quotes() {
    // A single-quoted raw string carries embedded double quotes verbatim
    // (the role the retired `r#"…"#` form used to play).
    let kinds = lex_kinds(r#"'can contain "quotes"'"#);
    assert_eq!(
        kinds,
        vec![
            TokenKind::StringLiteral(r#"can contain "quotes""#.into()),
            TokenKind::Eof
        ]
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Character Literals (ruby-naming §3.11: `?a`, `?\n`, `?\u{…}`)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_char_literal() {
    let kinds = lex_kinds("?a");
    assert_eq!(kinds, vec![TokenKind::CharLiteral('a'), TokenKind::Eof]);
}

#[test]
fn test_char_escape() {
    let kinds = lex_kinds(r"?\n");
    assert_eq!(kinds, vec![TokenKind::CharLiteral('\n'), TokenKind::Eof]);
}

#[test]
fn test_char_unicode() {
    let kinds = lex_kinds(r"?\u{1F600}");
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
    // ruby-naming: `pub` and `mut` are no longer reserved; methods are
    // prefixed with `public` / `private` / `protected` (or live under a
    // section marker), and writable receivers use `var`. The canonical
    // lex of a writing method signature is `public def var assign(...)`.
    let kinds = lex_kinds("public def var assign(name: String)");
    assert_eq!(
        kinds,
        vec![
            TokenKind::Public,
            TokenKind::Def,
            TokenKind::Var,
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
fn test_lifetime_sigil_is_rejected_no_sigil_form() {
    // ruby-naming.spec.md §3.3 / G7: there is no `'a` lifetime sigil.
    // A leading `'` with no closing quote on the line is an unterminated
    // raw string (E0002), not a lifetime. Lifetimes are bare lowercase
    // names in the `[...]` parameter slot — see the parser tests.
    let (_t, diags) = lex_with_errors("'a");
    assert!(
        diags.iter().any(|d| d.code.as_deref() == Some("E0002")),
        "`'a` should be rejected (no lifetime sigil); got: {diags:?}"
    );
    let (_t2, diags2) = lex_with_errors("'input");
    assert!(
        diags2.iter().any(|d| d.code.as_deref() == Some("E0002")),
        "`'input` should be rejected (no lifetime sigil); got: {diags2:?}"
    );
}

#[test]
fn test_single_quote_is_raw_string_not_char() {
    // After ruby-naming §3.11, `'a'` is a one-char RAW STRING (char
    // literals moved to `?a`). A bare `'input` with no closing quote
    // on the line stays a lifetime (see test_lifetime_*).
    let kinds = lex_kinds("'a'");
    assert_eq!(
        kinds,
        vec![TokenKind::StringLiteral("a".into()), TokenKind::Eof]
    );
    let kinds = lex_kinds("?a");
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
    // `?\'` — a char literal of a single quote via the `?` form.
    let kinds = lex_kinds(r"?\'");
    assert_eq!(kinds, vec![TokenKind::CharLiteral('\''), TokenKind::Eof]);
}

// ═══════════════════════════════════════════════════════════════════════════
// Regex literal + `~=` operator (E17xx)
// ═══════════════════════════════════════════════════════════════════════════

/// `~=` lexes as a single TildeEq token.
#[test]
fn lex_regex_tilde_eq_operator() {
    let kinds = lex_kinds("~=");
    assert_eq!(kinds[0], TokenKind::TildeEq);
}

/// A lone `~` is still rejected with E0006 — there's no bitwise-not
/// in Ruxen, and the regex op is only the two-char form.
#[test]
fn lex_regex_tilde_alone_is_rejected() {
    let (_, diags) = lex_with_errors("~");
    assert!(
        diags.iter().any(|d| d.code.as_deref() == Some("E0006")),
        "expected E0006 for bare `~`, got {:?}",
        diags
    );
}

/// `/foo/i` after `=` lexes as a RegexLiteral.
#[test]
fn lex_regex_literal_after_eq() {
    let toks = lex("let r = /foo/i");
    let kinds: Vec<&TokenKind> = toks.iter().map(|t| &t.kind).collect();
    let lit = kinds
        .iter()
        .find(|k| matches!(k, TokenKind::RegexLiteral { .. }))
        .expect("expected RegexLiteral token");
    match lit {
        TokenKind::RegexLiteral { pattern, flags } => {
            assert_eq!(pattern, "foo");
            assert_eq!(flags, "i");
        }
        _ => unreachable!(),
    }
}

/// `/` after `Identifier` is division (not a regex literal). The JS
/// positional rule: a `/` opens a regex only after a token that ends
/// an expression-context position.
#[test]
fn lex_regex_slash_after_identifier_is_division() {
    let kinds = lex_kinds("a / 2");
    // [Identifier("a"), Slash, IntLiteral(2, _), Eof]
    assert!(
        kinds.iter().any(|k| matches!(k, TokenKind::Slash)),
        "expected Slash, got {:?}",
        kinds
    );
    assert!(
        !kinds
            .iter()
            .any(|k| matches!(k, TokenKind::RegexLiteral { .. })),
        "no RegexLiteral should be produced for `a / 2`"
    );
}

/// A `/` inside a character class doesn't close the literal.
#[test]
fn lex_regex_literal_with_char_class_containing_slash() {
    let toks = lex("let r = /[/]/");
    let lit = toks
        .iter()
        .find_map(|t| match &t.kind {
            TokenKind::RegexLiteral { pattern, flags } => Some((pattern.clone(), flags.clone())),
            _ => None,
        })
        .expect("expected RegexLiteral");
    assert_eq!(lit.0, "[/]");
    assert_eq!(lit.1, "");
}

/// A `\/` escapes the slash and the next bare `/` closes the literal.
#[test]
fn lex_regex_literal_with_escaped_slash() {
    let toks = lex(r"let r = /a\/b/");
    let lit = toks
        .iter()
        .find_map(|t| match &t.kind {
            TokenKind::RegexLiteral { pattern, flags } => Some((pattern.clone(), flags.clone())),
            _ => None,
        })
        .expect("expected RegexLiteral");
    assert_eq!(lit.0, r"a\/b");
    assert_eq!(lit.1, "");
}

/// An unterminated regex literal (EOL before closing `/`) errors with
/// E1701.
#[test]
fn lex_regex_literal_unterminated_errors_e1701() {
    let (_, diags) = lex_with_errors("let r = /foo\n");
    assert!(
        diags.iter().any(|d| d.code.as_deref() == Some("E1701")),
        "expected E1701, got {:?}",
        diags
    );
}

/// An unknown trailing flag errors with E1700.
#[test]
fn lex_regex_literal_unknown_flag_errors_e1700() {
    let (_, diags) = lex_with_errors("let r = /foo/q");
    assert!(
        diags.iter().any(|d| d.code.as_deref() == Some("E1700")),
        "expected E1700, got {:?}",
        diags
    );
}

/// A repeated flag errors with E1700.
#[test]
fn lex_regex_literal_repeated_flag_errors_e1700() {
    let (_, diags) = lex_with_errors("let r = /foo/ii");
    assert!(
        diags.iter().any(|d| d.code.as_deref() == Some("E1700")),
        "expected E1700, got {:?}",
        diags
    );
}

/// An empty pattern `//` errors with E1703.
#[test]
fn lex_regex_literal_empty_pattern_errors_e1703() {
    let (_, diags) = lex_with_errors("let r = //");
    assert!(
        diags.iter().any(|d| d.code.as_deref() == Some("E1703")),
        "expected E1703, got {:?}",
        diags
    );
}

/// `s ~= /foo/g` — after `~=` the `/` opens a regex literal.
#[test]
fn lex_regex_literal_after_tilde_eq() {
    let toks = lex("s ~= /foo/g");
    let lit = toks
        .iter()
        .find_map(|t| match &t.kind {
            TokenKind::RegexLiteral { pattern, flags } => Some((pattern.clone(), flags.clone())),
            _ => None,
        })
        .expect("expected RegexLiteral after ~=");
    assert_eq!(lit.0, "foo");
    assert_eq!(lit.1, "g");
}
