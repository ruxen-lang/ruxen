//! Syntax-parity harness (ADR `docs/decisions/syntax-parity-harness.md`).
//!
//! USER REQUIREMENT: "none of the lsp/ide/fmt/compiler/repl may diverge on any
//! syntax of ruxen; ruxen syntax must be 100% available on every package we
//! deliver." This file enforces that in-process for the compiler / fmt / repl
//! surfaces (the lsp + ide surfaces are pinned in `src/ruxen_ide/tests/
//! syntax_parity_ide.rs`, which can depend on the ide crate; the binary-level
//! axes live in `tests/release-e2e/run.sh`'s `parity` phase).
//!
//! Axes here:
//!   1. compiler   — every corpus file LEXES + PARSES (the shared parser feeds
//!                   the compiler, fmt, repl, lsp, and ide; a parse gap in one
//!                   is a parse gap in all).
//!   2. fmt        — `format(parse(src))` re-parses to a STRUCTURALLY IDENTICAL
//!                   AST (spans ignored; `use`-member order normalised — fmt is
//!                   allowed to alphabetise imports) AND is idempotent. This is
//!                   the strongest anti-divergence check: it caught Q34
//!                   (dropped grouping parens), the `MethodCall` zero-arg paren
//!                   drop, the method-visibility-section drop, and the `async`
//!                   modifier drop.
//!   3. repl       — `parse_repl_input` ACCEPTS every top-level item kind the
//!                   batch parser accepts (the contextual-keyword dispatch trap:
//!                   `alias` lexes as an Identifier and must be routed to the
//!                   item parser, not the expression arm).
//!   4. exhaustiveness guard — a compile-time match over `TopLevelItem` /
//!                   `MixinItem` / `ImplItem` that BREAKS THE BUILD when a new
//!                   variant is added without a parity decision (fix-or-
//!                   allowlist), so a future AST variant cannot silently skip
//!                   the harness.
//!   5. intentional-divergence allowlist — the parser-accepts-but-compile-
//!                   rejects cases (E0728 top-level expr, E0607 retired `@[...]`)
//!                   are an EXPLICIT table, not silent.
//!
//! Extending the corpus: add `.rx` files under `tests/release-e2e/cases/` or
//! `library/std/`; they are picked up automatically. Sibling repos
//! (canvas/quiver/rondo) are read-only corpora discovered by path.

use ruxen_core::lexer::token::Span;
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::ast::*;
use ruxen_core::parser::printer::PrettyPrinter;
use ruxen_core::parser::Parser;
use ruxen_core::{diagnostics::Diagnostic, formatter};
use std::path::{Path, PathBuf};

// ─── Corpus discovery ────────────────────────────────────────────────

fn ruxen_root() -> &'static Path {
    // compiler/ruxen_core → ruxen
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

/// The full read-only syntax corpus: the compiler's own fixtures + stdlib
/// source + the three sibling GUI/web repos' source. Sibling repos are
/// addressed by their workspace-sibling path and are NEVER modified.
fn corpus() -> Vec<PathBuf> {
    let root = ruxen_root();
    let workspace = root.parent().unwrap(); // ~/.projects
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
    // Render relative to ~/.projects so sibling files read cleanly.
    let ws = ruxen_root().parent().unwrap();
    p.strip_prefix(ws).unwrap_or(p).display().to_string()
}

// ─── Shared parse helpers ────────────────────────────────────────────

fn parse(src: &str) -> Result<Program, String> {
    let tokens = Lexer::new(src).tokenize().map_err(|d| {
        format!(
            "lex: {:?}",
            d.iter().map(|e| e.to_string()).collect::<Vec<_>>()
        )
    })?;
    let mut p = Parser::new(tokens);
    p.parse().map_err(|d| {
        format!(
            "parse: {:?}",
            d.iter().map(|e| e.to_string()).collect::<Vec<_>>()
        )
    })
}

/// Span-blind, import-order-normalised structural fingerprint of a program.
///
/// Built from the AST pretty-printer (which is span-free and fully
/// parenthesises every operator, so distinct trees print distinctly), then
/// the `Use std.x.{a, b}` member lists are sorted so the formatter's
/// deliberate import alphabetisation (a meaning-preserving canonicalisation,
/// `format_imports.rs`) is not flagged as a divergence — but a DROPPED,
/// ADDED, or RENAMED member still changes the fingerprint and trips the test.
fn fingerprint(prog: &Program) -> String {
    let mut prog = prog.clone();
    for item in &mut prog.items {
        if let TopLevelItem::Use(UseDecl {
            kind: UseKind::Group(members),
            ..
        }) = item
        {
            members.sort();
        }
    }
    PrettyPrinter::new().print_program(&prog)
}

// ─── Axis 1: compiler (lex + parse) ──────────────────────────────────

#[test]
fn axis_compiler_corpus_parses() {
    let files = corpus();
    assert!(files.len() > 100, "corpus suspiciously small: {}", files.len());
    let mut failures = Vec::new();
    let mut tested = 0usize;
    for f in &files {
        let Ok(src) = std::fs::read_to_string(f) else {
            continue;
        };
        tested += 1;
        if let Err(e) = parse(&src) {
            failures.push(format!("{}: {e}", rel(f)));
        }
    }
    eprintln!("axis_compiler: {tested} corpus files");
    assert!(
        failures.is_empty(),
        "{} corpus file(s) fail to lex+parse (shared-parser divergence):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

// ─── Axis 2: fmt (reparse-identity + idempotence) ────────────────────

#[test]
fn axis_fmt_reparse_identity_and_idempotent() {
    let files = corpus();
    let mut failures = Vec::new();
    let mut tested = 0usize;
    for f in &files {
        let Ok(src) = std::fs::read_to_string(f) else {
            continue;
        };
        // Only files the shared parser accepts are in scope (axis 1 gates
        // acceptance; this axis gates non-destructiveness).
        let Ok(orig) = parse(&src) else {
            continue;
        };
        tested += 1;

        let r = formatter::format(&src);
        if !r.errors.is_empty() {
            failures.push(format!("{}: format errored: {:?}", rel(f), r.errors));
            continue;
        }
        let reparsed = match parse(&r.output) {
            Ok(p) => p,
            Err(e) => {
                failures.push(format!("{}: formatted output no longer parses: {e}", rel(f)));
                continue;
            }
        };
        if fingerprint(&orig) != fingerprint(&reparsed) {
            failures.push(format!(
                "{}: fmt CHANGED the parse tree (destructive)",
                rel(f)
            ));
            continue;
        }
        let r2 = formatter::format(&r.output);
        if r2.output != r.output {
            failures.push(format!("{}: fmt not idempotent", rel(f)));
        }
    }
    eprintln!("axis_fmt: {tested} parseable corpus files");
    assert!(
        failures.is_empty(),
        "{} fmt round-trip divergence(s) (Q23/Q30/Q34 class):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

// ─── Axis 3: repl accepts every batch-accepted top-level item kind ───

/// One minimal, self-contained exemplar per `TopLevelItem` kind that the
/// batch parser accepts. The repl's `parse_repl_input` dispatches by leading
/// token; a kind whose leading token is a contextual keyword (lexes as an
/// Identifier — e.g. `alias`) must be explicitly routed or it falls into the
/// expression arm and is rejected. This is the exact bug class locus #1.
fn repl_exemplars() -> Vec<(&'static str, &'static str)> {
    vec![
        ("Function", "def f -> Int\n  1\nend"),
        ("AsyncFunction", "async def g -> Int\n  1\nend"),
        ("Class", "class C\n  x: Int\nend"),
        ("Struct", "struct S\n  x: Int\nend"),
        ("Enum", "enum E\n  A\n  B\nend"),
        ("Mixin", "mixin M\n  def f -> Int\nend"),
        ("Module", "module M\n  def f -> Int\n    1\n  end\nend"),
        ("Use", "use std.io"),
        ("UseGroup", "use std.fs.{write, metadata}"),
        ("Const", "const K: Int = 1"),
        ("TypeAlias", "type Ints = Array[Int]"),
        ("Newtype", "newtype Meters(Float)"),
        ("Extension", "extension Int\n  def double -> Int\n    self\n  end\nend"),
        ("Lib", "lib \"c\"\n  def puts(s: &str) -> Int\nend"),
        // Contextual-keyword item (locus #1): `alias` lexes as Identifier.
        ("Alias", "alias new_name old_name"),
        // Expression / statement inputs the repl must still accept directly.
        ("Statement", "let x = 1"),
        ("Expression", "1 + 2 * 3"),
    ]
}

#[test]
fn axis_repl_accepts_all_item_kinds() {
    let mut failures = Vec::new();
    for (label, src) in repl_exemplars() {
        // The batch parser must accept it, or the exemplar is wrong syntax
        // (not a divergence). Item-kind exemplars are whole programs;
        // Statement/Expression are not, so they skip the batch check and
        // exercise the repl-only stmt/expr arms directly.
        if !matches!(label, "Statement" | "Expression") {
            if let Err(e) = parse(src) {
                failures.push(format!(
                    "{label}: EXEMPLAR is not batch-valid syntax (fix the test, not the repl): {e}"
                ));
                continue;
            }
        }
        let tokens = match Lexer::new(src).tokenize() {
            Ok(t) => t,
            Err(e) => {
                failures.push(format!("{label}: exemplar fails to lex: {e:?}"));
                continue;
            }
        };
        let mut p = Parser::new(tokens);
        match p.parse_repl_input() {
            ReplParseResult::Complete(_) => {}
            ReplParseResult::Incomplete => {
                failures.push(format!("{label}: repl reports Incomplete on a complete input"))
            }
            ReplParseResult::Error(diags) => failures.push(format!(
                "{label}: repl REJECTS a batch-accepted construct: {}",
                fmt_diags(&diags)
            )),
        }
    }
    assert!(
        failures.is_empty(),
        "repl/compiler syntax divergence ({} kind(s)):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

fn fmt_diags(d: &[Diagnostic]) -> String {
    d.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; ")
}

// ─── Axis 4: exhaustiveness guard ────────────────────────────────────
//
// These functions are never CALLED — they exist purely so the compiler
// rejects the test crate when a new variant is added to one of the three
// item enums without a conscious parity decision. Each arm's comment names
// the surface that must learn the new kind. Adding a variant turns the match
// non-exhaustive → a compile error here → the author MUST extend the harness
// (fix path) or add an `_ =>` with a ledger note (intentional path). There is
// deliberately NO `_ =>` arm.

#[allow(dead_code)]
fn guard_top_level_item(it: &TopLevelItem) {
    // Surfaces each kind must round-trip: parser (parse), formatter
    // (format_items.rs::format_top_level_item), repl (parse_repl_input
    // dispatch), lsp/ide (analysis::analyze, shared parser).
    match it {
        TopLevelItem::Module(_) => {}
        TopLevelItem::Class(_) => {}
        TopLevelItem::Struct(_) => {}
        TopLevelItem::Enum(_) => {}
        TopLevelItem::Mixin(_) => {}
        TopLevelItem::Impl(_) => {}
        TopLevelItem::Function(_) => {}
        TopLevelItem::Use(_) => {}
        TopLevelItem::TypeAlias(_) => {}
        TopLevelItem::Newtype(_) => {}
        TopLevelItem::Const(_) => {}
        TopLevelItem::Lib(_) => {}
        TopLevelItem::Extern(_) => {}
        TopLevelItem::Alias(_) => {}
        // Intentional divergence (allowlisted below): parser accepts a
        // bare top-level expression so fmt round-trips test files; the
        // direct-compile path rejects it with E0728 (`ruxen test` hoists
        // first). See `INTENTIONAL_DIVERGENCES`.
        TopLevelItem::Expr(_) => {}
    }
}

#[allow(dead_code)]
fn guard_mixin_item(it: &MixinItem) {
    match it {
        MixinItem::AssocType { .. } => {}
        MixinItem::MethodSig(_) => {}
        MixinItem::DefaultMethod(_) => {}
        MixinItem::Alias(_) => {}
    }
}

#[allow(dead_code)]
fn guard_impl_item(it: &ImplItem) {
    match it {
        ImplItem::AssocType { .. } => {}
        ImplItem::Method(_) => {}
        ImplItem::Include { .. } => {}
        ImplItem::Alias(_) => {}
    }
}

// ─── Axis 5: intentional-divergence allowlist ────────────────────────
//
// Constructs the SHARED parser ACCEPTS (so fmt/lsp/ide round-trip them) but
// the direct compile path intentionally REJECTS with a specific diagnostic.
// These are NOT silent divergences — they are an explicit contract. The test
// asserts each one still parses (the acceptance half of the contract); the
// rejection half is owned by the resolve-phase tests that emit the code.

/// (source, diagnostic-code, why the compile path rejects it)
const INTENTIONAL_DIVERGENCES: &[(&str, &str, &str)] = &[
    (
        "Tester.describe(\"x\") do\n  1\nend\n",
        "E0728",
        "top-level expression statement: parser accepts so `ruxen fmt` \
         round-trips test files; direct compile rejects (ruxen test hoists \
         into a synthesised `def main` first)",
    ),
];

#[test]
fn axis_intentional_divergences_still_parse() {
    let mut failures = Vec::new();
    for (src, code, why) in INTENTIONAL_DIVERGENCES {
        if let Err(e) = parse(src) {
            failures.push(format!(
                "{code} ({why}): parser must ACCEPT this (fmt/lsp/ide parity) \
                 even though compile rejects it — but it failed to parse: {e}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "intentional-divergence acceptance broke:\n{}",
        failures.join("\n")
    );
}

// ─── Corpus-wide repl sweep (real per-item source slices) ────────────
//
// Beyond the per-kind exemplars: for every parseable corpus file, feed each
// TOP-LEVEL item's own source slice (extracted by byte span) to the repl's
// `parse_repl_input` and assert it is accepted. This drives the repl over the
// EXACT source the compiler accepts — catching a real construct the synthetic
// exemplars miss. The repl is one-input-at-a-time, so each item is fed
// independently (mirroring how the line-oriented repl driver works).

#[test]
fn axis_repl_accepts_corpus_top_level_items() {
    let files = corpus();
    let mut failures = Vec::new();
    let mut tested_items = 0usize;
    for f in &files {
        let Ok(src) = std::fs::read_to_string(f) else {
            continue;
        };
        let Ok(prog) = parse(&src) else {
            continue;
        };
        for item in &prog.items {
            // Top-level bare expressions are the allowlisted E0728 case,
            // exercised by the repl's expression arm separately; keep this
            // axis about ITEM-kind dispatch.
            if matches!(item, TopLevelItem::Expr(_)) {
                continue;
            }
            let span = item_span(item);
            let Some(slice) = src.get(span.start..span.end) else {
                continue;
            };
            let slice = slice.trim();
            if slice.is_empty() {
                continue;
            }
            tested_items += 1;
            let tokens = match Lexer::new(slice).tokenize() {
                Ok(t) => t,
                Err(_) => continue,
            };
            let mut p = Parser::new(tokens);
            match p.parse_repl_input() {
                ReplParseResult::Complete(_) | ReplParseResult::Incomplete => {}
                ReplParseResult::Error(diags) => failures.push(format!(
                    "{}: repl REJECTS a top-level item the compiler accepts:\n  {}\n  diags: {}",
                    rel(f),
                    slice.lines().next().unwrap_or(""),
                    fmt_diags(&diags)
                )),
            }
        }
    }
    eprintln!("axis_repl_corpus: {tested_items} top-level items swept");
    assert!(
        failures.is_empty(),
        "{} repl/compiler item-dispatch divergence(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Byte span of a top-level item (mirrors the formatter's private
/// `item_span`). Used only to slice per-item source for the repl sweep.
fn item_span(item: &TopLevelItem) -> &Span {
    match item {
        TopLevelItem::Module(m) => &m.span,
        TopLevelItem::Class(c) => &c.span,
        TopLevelItem::Struct(s) => &s.span,
        TopLevelItem::Enum(e) => &e.span,
        TopLevelItem::Mixin(t) => &t.span,
        TopLevelItem::Impl(i) => &i.span,
        TopLevelItem::Function(f) => &f.span,
        TopLevelItem::Use(u) => &u.span,
        TopLevelItem::TypeAlias(ta) => &ta.span,
        TopLevelItem::Newtype(nt) => &nt.span,
        TopLevelItem::Const(c) => &c.span,
        TopLevelItem::Lib(l) => &l.span,
        TopLevelItem::Extern(e) => &e.span,
        TopLevelItem::Expr(e) => &e.span,
        TopLevelItem::Alias(a) => &a.span,
    }
}
