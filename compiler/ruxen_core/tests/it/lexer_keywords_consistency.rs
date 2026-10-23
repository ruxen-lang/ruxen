//! The lexer's `KEYWORD_TABLE` is the single source of truth for keyword
//! spellings: the lexer, IDE completion, IDE rename and REPL completion all
//! read it. Guard that it has no duplicate spellings and that the lexer
//! actually lexes each entry as its token.

use ruxen_core::lexer::token::{keywords, lookup_keyword, KEYWORD_TABLE};
use ruxen_core::lexer::Lexer;

#[test]
fn every_table_entry_lexes_as_its_token() {
    for (spelling, kind) in &KEYWORD_TABLE {
        let tokens = Lexer::new(spelling).tokenize().expect("a keyword lexes");
        assert_eq!(
            &tokens[0].kind, kind,
            "`{spelling}` does not lex as its table token"
        );
    }
}

#[test]
fn keyword_table_has_no_duplicates() {
    let mut seen = std::collections::HashSet::new();
    for kw in keywords() {
        assert!(seen.insert(kw), "duplicate keyword in KEYWORD_TABLE: {kw}");
    }
}

#[test]
fn non_keywords_are_not_in_table() {
    for ident in ["foo", "println", "Regex", "alias", "exclude"] {
        assert!(
            lookup_keyword(ident).is_none(),
            "`{ident}` should not be a keyword"
        );
        assert!(
            !keywords().any(|kw| kw == ident),
            "`{ident}` should not be in KEYWORD_TABLE"
        );
    }
}
