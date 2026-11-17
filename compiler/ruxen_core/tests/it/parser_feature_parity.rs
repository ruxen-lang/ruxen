//! Parser feature-parity tests: constructs the language supports in one
//! context must parse in every context the grammar allows, so the parser
//! (shared by the compiler, the IDE/LSP, and the formatter) never diverges.
//!
//! The repo-wide sweep that every shipped `.rx` file parses is
//! `syntax_parity::axis_compiler_corpus_parses`.

use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use std::path::Path;

fn rx(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruxen")
        .join(format!("{name}.rx"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn parse_diags(src: &str) -> Result<(), String> {
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

/// Doc comments (`##`) must be accepted before FFI `def`s inside a `lib`
/// block, exactly as they are at top level and in class/mixin bodies.
#[test]
fn lib_block_accepts_doc_comments() {
    let src = rx("parse_lib_block_doc_comments");
    assert!(
        parse_diags(&src).is_ok(),
        "lib block with doc comments must parse: {:?}",
        parse_diags(&src)
    );
}

/// `match` is a keyword but also the canonical regex method name — it must
/// be usable as a method/FFI def name and in method-call position.
#[test]
fn keyword_match_usable_as_method_name() {
    let src = rx("parse_keyword_method_name");
    assert!(
        parse_diags(&src).is_ok(),
        "`match` must be usable as a method name: {:?}",
        parse_diags(&src)
    );
}

/// A file that is ENTIRELY module-level doc comments (no item) must parse
/// cleanly — e.g. a doc-only stdlib surface file like std.string's lib.rx.
#[test]
fn doc_only_file_parses() {
    let src = rx("parse_doc_only_file");
    assert!(
        parse_diags(&src).is_ok(),
        "doc-only file must parse: {:?}",
        parse_diags(&src)
    );
}

/// Trailing doc comments after the last item (before EOF) must not crash
/// the top-level parser.
#[test]
fn trailing_doc_comments_parse() {
    let src = rx("parse_trailing_doc_comments");
    assert!(
        parse_diags(&src).is_ok(),
        "trailing doc comments must parse: {:?}",
        parse_diags(&src)
    );
}
