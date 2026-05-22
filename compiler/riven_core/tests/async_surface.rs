//! Pin tests for the async sub-phase 1 stdlib surface
//! (`docs/specs/stdlib/async.spec.md`).
//!
//! Sub-phase 1 ships:
//!   * `mixin Future` with associated `type Output` and `def var poll(cx:
//!     &var Context) -> Poll[Self.Output]` (B1).
//!   * `enum Poll[T] { Ready(T), Pending }` (B2 + B11).
//!   * `class Context` / `class Waker` shells whose lib decls point at
//!     the stubbed `library/std/future/runtime/executor.c` (B3 + B10).
//!
//! These tests check the surface is registered after the bootstrap
//! merge runs. The matching e2e fixture
//! `tests/release-e2e/cases/720_handwritten_future_typechecks.rvn`
//! covers the end-to-end "user class includes Future, declares
//! `type Output = Int`, defines poll returning `Poll[Int]`" round-trip
//! (B4).

use riven_core::diagnostics::Diagnostic;
use riven_core::lexer::Lexer;
use riven_core::parser::Parser;
use riven_core::resolve::symbols::{DefKind, SymbolTable};
use riven_core::typeck;

/// Drive the production resolver+typecheck path so the bootstrap
/// merge runs against the same `library/std/<pkg>/src/lib.rvn`
/// sources the compiler uses. The returned `SymbolTable` carries
/// every bootstrap-registered DefId — Future, Poll, Context, Waker,
/// and the rest of the stdlib surface — exactly as user code sees
/// them at type-check time.
fn symbols_after_bootstrap() -> SymbolTable {
    // Empty user program is enough — we just need the bootstrap merge
    // to populate the symbol table.
    let mut lx = Lexer::new("");
    let toks = lx.tokenize().expect("lex empty program");
    let mut p = Parser::new(toks);
    let prog = p.parse().expect("parse empty program");
    let result = typeck::type_check(&prog);
    let errors: Vec<&Diagnostic> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == riven_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "empty program should typecheck cleanly; bootstrap diags: {:?}",
        errors.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
    result.symbols
}

// ─── B1 — Future mixin shape ────────────────────────────────────────

/// The bootstrap-merged `Future` mixin declares ONE required method
/// (`poll`) and ONE associated type (`Output`). Sub-phase 2 will lift
/// `async def` to a synthesised `include Future` + `type Output =
/// <ret>`; the contract pinned here is the surface those generated
/// includes must satisfy.
#[test]
fn future_mixin_has_associated_output_and_poll() {
    let symbols = symbols_after_bootstrap();
    let future = symbols
        .iter()
        .find(|d| d.name == "Future")
        .expect("expected `Future` mixin from library/std/future/src/lib.rvn");
    let DefKind::Trait { info } = &future.kind else {
        panic!("expected Future to be a Trait, got {:?}", future.kind);
    };
    assert_eq!(
        info.required_methods,
        vec!["poll".to_string()],
        "Future must declare exactly `poll` as a required method"
    );
    assert_eq!(
        info.assoc_types,
        vec!["Output".to_string()],
        "Future must declare exactly `Output` as an associated type"
    );
}

// ─── B2 — Poll enum registered ──────────────────────────────────────

/// `enum Poll[T] { Ready(T), Pending }` lifts from
/// `library/std/future/src/lib.rvn`. Both variants are reachable as
/// `Poll.Ready` / `Poll.Pending` and the enum carries one type
/// parameter.
#[test]
fn poll_enum_registered_with_ready_pending_variants() {
    let symbols = symbols_after_bootstrap();
    let poll = symbols
        .iter()
        .find(|d| d.name == "Poll")
        .expect("expected `Poll` enum from library/std/future/src/lib.rvn");
    let DefKind::Enum { info } = &poll.kind else {
        panic!("expected Poll to be an Enum, got {:?}", poll.kind);
    };
    assert_eq!(
        info.generic_params.len(),
        1,
        "Poll must take exactly one type parameter T"
    );
    assert_eq!(info.generic_params[0].name, "T");
    assert_eq!(
        info.variants.len(),
        2,
        "Poll must have exactly two variants (Ready, Pending)"
    );

    // Both variants must be reachable as DefIds and tagged correctly.
    let ready = symbols.get(info.variants[0]).expect("Ready DefId resolves");
    assert_eq!(ready.name, "Ready");
    let pending = symbols
        .get(info.variants[1])
        .expect("Pending DefId resolves");
    assert_eq!(pending.name, "Pending");
}

// ─── B2 (tag) — Poll variant tag layout is pinned ───────────────────

/// Tag layout: `Ready = 0`, `Pending = 1`. Sub-phase 3's executor will
/// inspect the discriminant when polling the user's future at the C
/// boundary; if this order ever drifts the executor would mis-read
/// the variant and silently busy-loop on a "Ready" branch it thought
/// was "Pending" (or vice versa). The test reads the .rvn source
/// directly so a refactor that moves the declaration but keeps the
/// order intact still passes; a reorder breaks the test even if the
/// enum still compiles.
#[test]
fn poll_tag_layout_stability() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("library/std/future/src/lib.rvn");
    let source =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e));

    // Scan the `enum Poll[T] ... end` block and collect variant names
    // in declaration order. Mirrors `io_error_tag_stability.rs`.
    let mut variants: Vec<(String, usize)> = Vec::new();
    let mut in_block = false;
    let mut next_tag: usize = 0;
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("enum Poll") {
            in_block = true;
            next_tag = 0;
            continue;
        }
        if !in_block {
            continue;
        }
        if trimmed == "end" {
            break;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let name: &str = trimmed
            .split(|c: char| c == '(' || c == '{' || c.is_whitespace())
            .next()
            .unwrap_or("");
        assert!(
            name.chars().next().is_some_and(|c| c.is_ascii_uppercase()),
            "unexpected line inside `enum Poll` body: {:?}",
            line
        );
        variants.push((name.to_string(), next_tag));
        next_tag += 1;
    }

    assert_eq!(
        variants,
        vec![("Ready".to_string(), 0), ("Pending".to_string(), 1)],
        "Poll variant order drifted — append-only, never reorder. \
         Sub-phase 3's executor reads these tags at the C boundary."
    );
}

// ─── B3 — Context and Waker classes resolve ─────────────────────────

/// `class Context` and `class Waker` lift from
/// `library/std/future/src/lib.rvn` as Class DefKinds. Sub-phase 1 ships
/// them with lib decls pointing at the stubbed executor (every method
/// `riven_panic`s); the contract pinned here is just that the class
/// names are reachable so user code can write `def poll(cx: &var
/// Context)` and have it type-check.
#[test]
fn context_and_waker_classes_resolve() {
    let symbols = symbols_after_bootstrap();
    let ctx = symbols
        .iter()
        .find(|d| d.name == "Context" && matches!(d.kind, DefKind::Class { .. }))
        .expect("expected `Context` class from library/std/future/src/lib.rvn");
    let _ = ctx; // already asserted via the find
    let waker = symbols
        .iter()
        .find(|d| d.name == "Waker" && matches!(d.kind, DefKind::Class { .. }))
        .expect("expected `Waker` class from library/std/future/src/lib.rvn");
    let _ = waker;
}
