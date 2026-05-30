//! The lexer's canonical `KEYWORDS` list is the single source of truth that
//! IDE completion consumes. Guard that it stays consistent with the actual
//! `lookup_keyword` recognizer so the two can never drift.

use ruxen_core::lexer::token::{lookup_keyword, KEYWORDS};

#[test]
fn every_keyword_const_entry_is_recognized() {
    for kw in KEYWORDS {
        assert!(
            lookup_keyword(kw).is_some(),
            "`{kw}` is in KEYWORDS but lookup_keyword does not recognize it"
        );
    }
}

#[test]
fn keywords_const_has_no_duplicates() {
    let mut seen = std::collections::HashSet::new();
    for kw in KEYWORDS {
        assert!(seen.insert(*kw), "duplicate keyword in KEYWORDS: {kw}");
    }
}

#[test]
fn non_keywords_are_not_in_list() {
    // Identifiers that must NOT be treated as keywords (regression guard for
    // the old completion list, which wrongly included `and`/`or`/`not`).
    for ident in ["and", "or", "not", "foo", "println", "Regex"] {
        assert!(
            lookup_keyword(ident).is_none(),
            "`{ident}` should not be a keyword"
        );
        assert!(
            !KEYWORDS.contains(&ident),
            "`{ident}` should not be in KEYWORDS"
        );
    }
}
