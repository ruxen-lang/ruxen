use riven_core::lexer::token::TokenKind;
use riven_core::lexer::Lexer;

#[test]
fn test_sample_program_lexes_without_errors() {
    let source = include_str!("fixtures/sample_program.rvn");
    let mut lexer = Lexer::new(source);
    let tokens = lexer
        .tokenize()
        .expect("sample program should lex without errors");

    // A 500-line program should produce a significant number of tokens
    assert!(
        tokens.len() > 500,
        "expected > 500 tokens, got {}",
        tokens.len()
    );

    // Last token should be Eof
    assert_eq!(tokens.last().unwrap().kind, TokenKind::Eof);
}

#[test]
fn test_sample_program_first_tokens() {
    let source = include_str!("fixtures/sample_program.rvn");
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().unwrap();

    // After skipping initial comments, the first significant token should be `enum`
    let first_non_newline = tokens
        .iter()
        .find(|t| !matches!(t.kind, TokenKind::Newline | TokenKind::Eof))
        .unwrap();

    assert_eq!(first_non_newline.kind, TokenKind::Enum);
}

#[test]
fn test_sample_program_contains_key_tokens() {
    let source = include_str!("fixtures/sample_program.rvn");
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().unwrap();

    let kinds: Vec<&TokenKind> = tokens.iter().map(|t| &t.kind).collect();

    // ruby-naming.spec.md: the sample fixture now uses `mixin`,
    // `include`, `public` and `extension` instead of `trait`, `impl`,
    // `pub`. The lexer no longer emits the legacy kinds.
    assert!(kinds.contains(&&TokenKind::Enum));
    assert!(kinds.contains(&&TokenKind::Include));
    assert!(kinds.contains(&&TokenKind::Class));
    assert!(kinds.contains(&&TokenKind::Def));
    assert!(kinds.contains(&&TokenKind::Let));
    assert!(kinds.contains(&&TokenKind::Mut));
    assert!(kinds.contains(&&TokenKind::Match));
    assert!(kinds.contains(&&TokenKind::For));
    assert!(kinds.contains(&&TokenKind::If));
    assert!(kinds.contains(&&TokenKind::Else));
    assert!(kinds.contains(&&TokenKind::End));
    assert!(kinds.contains(&&TokenKind::Return));
    assert!(kinds.contains(&&TokenKind::SelfValue));
    assert!(kinds.contains(&&TokenKind::Arrow));
    assert!(kinds.contains(&&TokenKind::Dot));
    assert!(kinds.contains(&&TokenKind::Colon));
    assert!(kinds.contains(&&TokenKind::Comma));
    assert!(kinds.contains(&&TokenKind::LParen));
    assert!(kinds.contains(&&TokenKind::RParen));
    assert!(kinds.contains(&&TokenKind::LBracket));
    assert!(kinds.contains(&&TokenKind::RBracket));
    assert!(kinds.contains(&&TokenKind::LBrace));
    assert!(kinds.contains(&&TokenKind::RBrace));
    assert!(kinds.contains(&&TokenKind::Pipe));
    assert!(kinds.contains(&&TokenKind::Question));
    assert!(kinds.contains(&&TokenKind::At));
    assert!(kinds.contains(&&TokenKind::AmpMut));
    assert!(kinds.contains(&&TokenKind::Amp));
    assert!(kinds.contains(&&TokenKind::Lt));
    assert!(kinds.contains(&&TokenKind::PlusEq));
    assert!(kinds.contains(&&TokenKind::QuestionDot));
    assert!(kinds.contains(&&TokenKind::Consume));
    assert!(kinds.contains(&&TokenKind::Ref));
    assert!(kinds.contains(&&TokenKind::True));
    assert!(kinds.contains(&&TokenKind::False));
    assert!(kinds.contains(&&TokenKind::Bang));
    assert!(kinds.contains(&&TokenKind::AmpAmp));
    assert!(kinds.contains(&&TokenKind::OkKw));
    assert!(kinds.contains(&&TokenKind::ErrKw));
    assert!(kinds.contains(&&TokenKind::SomeKw));
    // ruby-naming.spec.md: `None` is no longer a keyword — `nil` is the
    // absence literal. The fixture exercises `nil` instead.
    assert!(kinds.contains(&&TokenKind::Nil));

    // Check that interpolated strings exist
    assert!(kinds
        .iter()
        .any(|k| matches!(k, TokenKind::InterpolatedString(_))));

    // Check that string literals exist
    assert!(kinds
        .iter()
        .any(|k| matches!(k, TokenKind::StringLiteral(_))));

    // Check that integer literals exist
    assert!(kinds
        .iter()
        .any(|k| matches!(k, TokenKind::IntLiteral(_, _))));
}
