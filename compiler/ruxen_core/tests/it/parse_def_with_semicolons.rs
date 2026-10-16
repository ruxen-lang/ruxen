use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;

fn parse(src: &str) -> Result<(), String> {
    let mut lexer = Lexer::new(src);
    let tokens = lexer.tokenize().map_err(|d| {
        format!(
            "lex: {:?}",
            d.iter().map(|e| e.to_string()).collect::<Vec<_>>()
        )
    })?;
    let mut parser = Parser::new(tokens);
    parser.parse().map(|_| ()).map_err(|d| {
        format!(
            "parse: {:?}",
            d.iter().map(|e| e.to_string()).collect::<Vec<_>>()
        )
    })
}

#[test]
fn def_with_semicolon_separators_parses() {
    parse("def double(n: Int) -> Int; n * 2; end").expect("should parse");
}

#[test]
fn def_with_only_trailing_semicolon_parses() {
    parse("def noop -> Unit; end").expect("should parse");
}

#[test]
fn def_with_semicolons_and_newlines_mixed_parses() {
    parse("def both\n  let x = 1;\n  x + 1\nend").expect("should parse");
}

#[test]
fn multiline_def_still_parses() {
    parse("def normal(n: Int) -> Int\n  let y = n * 2\n  y\nend").expect("should parse");
}
