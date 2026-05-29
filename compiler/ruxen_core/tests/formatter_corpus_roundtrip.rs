// Corpus round-trip test: the formatter must be in sync with parser semantics.
// For every `.rx` file the parser currently accepts, `format()` must:
//   1. produce no errors,
//   2. produce output that STILL PARSES (semantic sync — no retired syntax),
//   3. be idempotent: format(format(x)) == format(x).
use ruxen_core::formatter;
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use std::path::{Path, PathBuf};

fn parses(src: &str) -> Result<(), String> {
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

    for f in &files {
        let src = match std::fs::read_to_string(f) {
            Ok(s) => s,
            Err(_) => continue,
        };
        // Baseline: only test files the parser already accepts.
        if parses(&src).is_err() {
            continue;
        }
        tested += 1;
        let rel = f.strip_prefix(root).unwrap_or(f).display();

        let r1 = formatter::format(&src);
        if !r1.errors.is_empty() {
            failures.push(format!("{rel}: format errors: {:?}", r1.errors));
            continue;
        }
        // Semantic sync: formatted output must still parse.
        if let Err(e) = parses(&r1.output) {
            failures.push(format!("{rel}: formatted output no longer parses: {e}"));
            continue;
        }
        // Idempotency.
        let r2 = formatter::format(&r1.output);
        if r2.output != r1.output {
            failures.push(format!("{rel}: not idempotent (format twice differs)"));
        }
    }

    eprintln!("formatter corpus: tested {tested} files, {} failures", failures.len());
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
