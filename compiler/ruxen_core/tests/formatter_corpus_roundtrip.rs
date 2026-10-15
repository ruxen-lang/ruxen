// Corpus round-trip test: the formatter must be in sync with parser semantics.
// For every `.rx` file the parser currently accepts, `format()` must:
//   1. produce no errors,
//   2. produce output that STILL PARSES (semantic sync — no retired syntax),
//   3. parse to the same AST as the input, ignoring spans,
//   4. be idempotent: format(format(x)) == format(x).
use ruxen_core::formatter;
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use std::path::{Path, PathBuf};

/// Parse `src` and return its AST's `Debug` text with every `Span`
/// replaced by a bare `Span`, so two parses compare by structure only.
fn parses(src: &str) -> Result<String, String> {
    let mut lexer = Lexer::new(src);
    let tokens = lexer.tokenize().map_err(|d| {
        format!(
            "lex: {:?}",
            d.iter().map(|e| e.to_string()).collect::<Vec<_>>()
        )
    })?;
    let mut parser = Parser::new(tokens);
    let program = parser.parse().map_err(|d| {
        format!(
            "parse: {:?}",
            d.iter().map(|e| e.to_string()).collect::<Vec<_>>()
        )
    })?;
    Ok(strip_spans(&format!("{program:?}")))
}

/// `Span` has only integer fields, so each `Span { .. }` ends at the
/// first `}` after it.
fn strip_spans(debug: &str) -> String {
    const OPEN: &str = "Span { start: ";
    let mut out = String::with_capacity(debug.len());
    let mut rest = debug;
    while let Some(pos) = rest.find(OPEN) {
        out.push_str(&rest[..pos]);
        out.push_str("Span");
        let after = &rest[pos..];
        let close = after
            .find('}')
            .expect("unterminated Span in AST debug output");
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// Files whose formatted output parses to a different AST, or no longer
/// parses at all. Audit family F-5 emptied it; it may only stay empty.
const KNOWN_AST_CHANGES: &[&str] = &[];

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

#[test]
fn formatter_roundtrips_corpus() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let mut files = Vec::new();
    collect_rx(&root.join("library/std"), &mut files);
    collect_rx(&root.join("tests/release-e2e/cases"), &mut files);
    files.sort();

    let mut failures: Vec<String> = Vec::new();
    let mut tested = 0usize;
    let mut known_seen: Vec<String> = Vec::new();

    for f in &files {
        let src = match std::fs::read_to_string(f) {
            Ok(s) => s,
            Err(_) => continue,
        };
        // Baseline: only test files the parser already accepts.
        let Ok(ast_before) = parses(&src) else {
            continue;
        };
        tested += 1;
        let rel = f.strip_prefix(root).unwrap_or(f).display();
        let rel_str = rel.to_string();

        let r1 = formatter::format(&src);
        if !r1.errors.is_empty() {
            failures.push(format!("{rel}: format errors: {:?}", r1.errors));
            continue;
        }
        // Semantic sync: formatted output must still parse.
        let ast_after = match parses(&r1.output) {
            Ok(ast) => ast,
            Err(_) if KNOWN_AST_CHANGES.contains(&rel_str.as_str()) => {
                known_seen.push(rel_str);
                continue;
            }
            Err(e) => {
                failures.push(format!("{rel}: formatted output no longer parses: {e}"));
                continue;
            }
        };
        // Semantic sync: formatting must not change the program.
        if ast_after != ast_before {
            if KNOWN_AST_CHANGES.contains(&rel_str.as_str()) {
                // Output is already wrong; idempotency of it says nothing.
                known_seen.push(rel_str);
                continue;
            } else {
                let at = ast_before
                    .bytes()
                    .zip(ast_after.bytes())
                    .position(|(a, b)| a != b)
                    .unwrap_or(ast_before.len().min(ast_after.len()));
                let near = |s: &str| {
                    s.get(at.saturating_sub(40)..(at + 80).min(s.len()))
                        .unwrap_or("")
                        .to_string()
                };
                failures.push(format!(
                "{rel}: formatted output parses to a different AST\n    before: {}\n    after:  {}",
                near(&ast_before),
                near(&ast_after)
            ));
                continue;
            }
        }
        // Idempotency.
        let r2 = formatter::format(&r1.output);
        if r2.output != r1.output {
            failures.push(format!("{rel}: not idempotent (format twice differs)"));
        }
    }

    for known in KNOWN_AST_CHANGES {
        if !known_seen.iter().any(|k| k == known) {
            failures.push(format!(
                "{known}: listed in KNOWN_AST_CHANGES but no longer changes AST; remove it"
            ));
        }
    }

    eprintln!(
        "formatter corpus: tested {tested} files, {} failures",
        failures.len()
    );
    if !failures.is_empty() {
        let shown: Vec<_> = failures.iter().take(200).cloned().collect();
        panic!(
            "{} formatter round-trip failures (showing {}):\n{}",
            failures.len(),
            shown.len(),
            shown.join("\n")
        );
    }
}
