//! `ruxen fmt` must PRESERVE `##` doc comments attached to FFI `def`s inside
//! `lib` blocks — both top-level and class-body. Dropping them is a
//! cross-binary divergence (the parser accepts them; fmt must not delete them).

use ruxen_core::formatter;

fn formats_preserving(src: &str, must_contain: &[&str]) {
    let result = formatter::format(src);
    assert!(
        result.errors.is_empty(),
        "format errored: {:?}",
        result.errors
    );
    for needle in must_contain {
        assert!(
            result.output.contains(needle),
            "formatter dropped {needle:?}\n--- output ---\n{}",
            result.output
        );
    }
}

#[test]
fn class_body_lib_block_preserves_doc_comments() {
    let src = "class Regex\n  lib \"runtime/regex.c\"\n    ## First match doc.\n    def match as \"ruxen_regex_match\"(text: &String) -> Int\n  end\nend\n";
    formats_preserving(
        src,
        &["## First match doc.", "def match as \"ruxen_regex_match\""],
    );
}

#[test]
fn top_level_lib_block_preserves_doc_comments() {
    let src = "lib \"pcre2-8\"\n  ## A documented FFI alias.\n  def offset as \"ruxen_regex_error_offset\" -> Int\nend\n";
    formats_preserving(
        src,
        &["## A documented FFI alias.", "def offset as \"ruxen_regex_error_offset\""],
    );
}
