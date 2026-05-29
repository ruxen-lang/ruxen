//! Parser feature-parity tests: constructs the language supports in one
//! context must parse in every context the grammar allows, so the parser
//! (shared by the compiler, the IDE/LSP, and the formatter) never diverges.
//!
//! Also includes a repo-wide sweep asserting every stdlib/example `.rx`
//! SOURCE file the toolchain ships actually parses — a divergence between
//! "the compiler accepts it" and "the IDE/formatter reject it" is exactly a
//! parser gap, and this sweep surfaces them as a worklist.

use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use std::path::{Path, PathBuf};

fn rx(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruxen")
        .join(format!("{name}.rx"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn parse_diags(src: &str) -> Result<(), String> {
    let mut lexer = Lexer::new(src);
    let tokens = lexer
        .tokenize()
        .map_err(|d| format!("lex: {:?}", d.iter().map(|e| e.to_string()).collect::<Vec<_>>()))?;
    let mut parser = Parser::new(tokens);
    parser
        .parse()
        .map(|_| ())
        .map_err(|d| format!("parse: {:?}", d.iter().map(|e| e.to_string()).collect::<Vec<_>>()))
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

fn collect_rx(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect_rx(&p, out);
            } else if p.extension().and_then(|s| s.to_str()) == Some("rx") {
                out.push(p);
            }
        }
    }
}

/// Every shipped stdlib and example `.rx` source must parse. These are the
/// files the compiler bootstraps and the IDE/formatter open, so any parse
/// failure here is a cross-binary divergence.
#[test]
fn shipped_stdlib_and_examples_all_parse() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let mut files = Vec::new();
    collect_rx(&root.join("library/std"), &mut files);
    collect_rx(&root.join("examples"), &mut files);
    files.sort();

    let mut failures = Vec::new();
    for f in &files {
        let rel = f.strip_prefix(root).unwrap_or(f).display().to_string();
        let Ok(src) = std::fs::read_to_string(f) else {
            continue;
        };
        if let Err(e) = parse_diags(&src) {
            failures.push(format!("{rel}: {e}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} shipped .rx file(s) fail to parse (parser/IDE/compiler divergence):\n{}",
        failures.len(),
        failures.join("\n")
    );
}
