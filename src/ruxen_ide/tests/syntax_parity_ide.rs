//! Syntax-parity harness — LSP / IDE axes (ADR
//! `docs/decisions/syntax-parity-harness.md`).
//!
//! USER REQUIREMENT: "none of the lsp/ide/fmt/compiler/repl may diverge on any
//! syntax of ruxen." The compiler / fmt / repl axes are pinned in
//! `compiler/ruxen_core/tests/syntax_parity.rs`; this file pins the LSP + IDE
//! axes, which can only be exercised from a crate that depends on `ruxen_ide`.
//!
//! Both `ruxen_lsp` and `ruxen_ide` funnel every document through
//! `ruxen_ide::analysis::analyze`, which runs lex → parse → typeck → borrow.
//! `analyze` GATES on parse: a lex or parse error returns early with
//! `program: None` (only the lex/parse diagnostics). So the parity contract is
//! exact and needs no diagnostic classification:
//!
//!   if the SHARED core parser accepts a file, `analyze().program` must be
//!   `Some` — i.e. the IDE/LSP parsed the SAME syntax. A `None` here is a
//!   surface divergence: the compiler accepts syntax the IDE/LSP chokes on.
//!
//! Semantic (typeck/borrow) diagnostics are EXPECTED on stdlib fragments and
//! sibling-library files that cannot type-check standalone — they are not a
//! syntax divergence, so this axis only asserts the parse stage matched.

use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use ruxen_ide::analysis::analyze;
use std::path::{Path, PathBuf};

fn ruxen_root() -> &'static Path {
    // src/ruxen_ide → ruxen
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
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

/// Same corpus as the core harness: compiler fixtures + stdlib + examples +
/// the three sibling repos (read-only, by workspace-sibling path).
fn corpus() -> Vec<PathBuf> {
    let root = ruxen_root();
    let workspace = root.parent().unwrap();
    let mut files = Vec::new();
    collect_rx(&root.join("tests/release-e2e/cases"), &mut files);
    collect_rx(&root.join("library/std"), &mut files);
    collect_rx(&root.join("examples"), &mut files);
    for sib in ["canvas/src", "quiver/src", "rondo/src"] {
        let p = workspace.join(sib);
        if p.is_dir() {
            collect_rx(&p, &mut files);
        }
    }
    files.sort();
    files
}

fn rel(p: &Path) -> String {
    let ws = ruxen_root().parent().unwrap();
    p.strip_prefix(ws).unwrap_or(p).display().to_string()
}

/// Does the shared core parser accept this source?
fn core_parses(src: &str) -> bool {
    let Ok(tokens) = Lexer::new(src).tokenize() else {
        return false;
    };
    Parser::new(tokens).parse().is_ok()
}

/// Every corpus file the compiler's shared parser accepts must also reach a
/// parsed AST inside `analyze` (the IDE/LSP path). If `analyze` bails at parse
/// (`program: None`) on syntax the compiler accepts, the LSP/IDE would surface
/// a spurious syntax error the compiler never raises — a surface divergence.
#[test]
fn ide_lsp_parse_matches_compiler_on_corpus() {
    let files = corpus();
    assert!(files.len() > 100, "corpus suspiciously small: {}", files.len());

    let mut failures = Vec::new();
    let mut tested = 0usize;
    for f in &files {
        let Ok(src) = std::fs::read_to_string(f) else {
            continue;
        };
        if !core_parses(&src) {
            // Not accepted by the compiler's parser either → out of scope for
            // a PARITY check (axis 1 in the core harness owns acceptance).
            continue;
        }
        tested += 1;
        let result = analyze(&src);
        if result.program.is_none() {
            // The IDE/LSP failed to parse syntax the compiler accepted.
            let parse_diags: Vec<String> = result
                .diagnostics
                .iter()
                .map(|d| d.to_string())
                .collect();
            failures.push(format!(
                "{}: IDE/LSP `analyze` did not produce an AST for compiler-accepted syntax \
                 (spurious LSP syntax error):\n    {}",
                rel(f),
                if parse_diags.is_empty() {
                    "(no diagnostics — parse silently dropped)".to_string()
                } else {
                    parse_diags.join("\n    ")
                }
            ));
        }
    }

    eprintln!("ide/lsp axis: {tested} compiler-accepted corpus files checked");
    assert!(
        failures.is_empty(),
        "{} LSP/IDE syntax divergence(s) — compiler parses, IDE/LSP does not:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// A focused, self-contained smoke over every top-level construct, asserting
/// the IDE/LSP `analyze` parses each (independent of the file corpus, so it
/// keeps protecting the contract even if the corpus shrinks).
#[test]
fn ide_lsp_parses_every_item_kind() {
    let exemplars: &[(&str, &str)] = &[
        ("function", "def f -> Int\n  1\nend\n"),
        ("async function", "async def g -> Int\n  1\nend\n"),
        ("class", "class C\n  x: Int\nend\n"),
        ("struct", "struct S\n  x: Int\nend\n"),
        ("enum", "enum E\n  A\n  B\nend\n"),
        ("mixin", "mixin M\n  def f -> Int\nend\n"),
        ("module", "module M\n  def f -> Int\n    1\n  end\nend\n"),
        ("use", "use std.io\n"),
        ("use group", "use std.fs.{write, metadata}\n"),
        ("const", "const K: Int = 1\n"),
        ("type alias", "type Ints = Array[Int]\n"),
        ("newtype", "newtype Meters(Float)\n"),
        (
            "extension",
            "extension Int\n  def double -> Int\n    self\n  end\nend\n",
        ),
        ("lib", "lib \"c\"\n  def puts(s: &str) -> Int\nend\n"),
        ("alias", "def a -> Int\n  1\nend\nalias b a\n"),
    ];
    let mut failures = Vec::new();
    for (label, src) in exemplars {
        if analyze(src).program.is_none() {
            failures.push(format!(
                "{label}: IDE/LSP `analyze` failed to parse a basic top-level construct"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "IDE/LSP cannot parse construct(s) the compiler accepts:\n{}",
        failures.join("\n")
    );
}
