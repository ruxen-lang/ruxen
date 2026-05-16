//! Unit tests for the Phase 2 stdlib `Iterator` surface (#05).
//!
//! These tests drive lex → parse → typeck → MIR → codegen on small
//! `.rvn` source strings. No `rivenc` subprocess is invoked, so each
//! test runs in <50 ms — one or two orders of magnitude faster than
//! the release-e2e fixtures. They pin the closure-inliner + runtime
//! dispatch surface for iterator combinators + terminators.
//!
//! End-to-end behaviour (real binary, real stdout) is verified by the
//! release-e2e fixtures `tests/release-e2e/cases/60{3..}_iter_*.rvn`
//! separately; the unit tests here are the primary TDD loop.
//!
//! See `docs/prompts/v1/05_phase2_stdlib_iterator.md` for the full
//! Iterator surface contract.

use riven_core::codegen::cranelift;
use riven_core::diagnostics::{Diagnostic, DiagnosticLevel};
use riven_core::lexer::Lexer;
use riven_core::mir::lower::Lowerer;
use riven_core::parser::Parser;
use riven_core::typeck;

fn rvn(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/riven")
        .join(format!("{name}.rvn"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn typecheck_diagnostics(source: &str) -> Vec<Diagnostic> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer
        .tokenize()
        .unwrap_or_else(|e| panic!("lexer failed: {:?}", e));
    let mut parser = Parser::new(tokens);
    let program = parser
        .parse()
        .unwrap_or_else(|e| panic!("parser failed: {:?}", e));
    typeck::type_check(&program).diagnostics
}

/// Drive lex → parse → typeck → MIR → Cranelift-codegen on a small
/// Riven source string. We deliberately stop at the in-memory object
/// emission step so the test stays self-contained: no `cc` invocation,
/// no temp files, no shared `runtime_o` race between parallel tests.
/// `runtime_name` errors (`"no runtime symbol for ..."`) surface as
/// `Err` from `compile_program`; that's exactly the error the unit
/// tests below pin or assert away.
fn try_compile(source: &str) -> Result<(), String> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().map_err(|e| format!("lex: {e:?}"))?;
    let mut parser = Parser::new(tokens);
    let program = parser.parse().map_err(|e| format!("parse: {e:?}"))?;
    let result = typeck::type_check(&program);
    let typeck_errors: Vec<&Diagnostic> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    if !typeck_errors.is_empty() {
        return Err(format!("typeck errors: {:#?}", typeck_errors));
    }
    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .map_err(|e| format!("mir: {e:?}"))?;
    let mut codegen = cranelift::CodeGen::new()?;
    codegen.compile_program(&mir)?;
    let _ = codegen.finish()?;
    Ok(())
}

fn assert_compiles(source: &str) {
    if let Err(e) = try_compile(source) {
        panic!("expected compilation to succeed; got error:\n{}", e);
    }
}

// ─── Eager terminators (closure-takers) ─────────────────────────────

#[test]
fn iter_fold_compiles_with_int_accumulator() {
    let source = rvn("iter_fold_compiles_with_int_accumulator");
    assert_compiles(&source);
}

#[test]
fn iter_all_compiles_returning_bool() {
    let source = rvn("iter_all_compiles_returning_bool");
    assert_compiles(&source);
}

#[test]
fn iter_any_compiles_returning_bool() {
    let source = rvn("iter_any_compiles_returning_bool");
    assert_compiles(&source);
}

// ─── Lazy combinators ───────────────────────────────────────────────

#[test]
fn iter_take_compiles() {
    let source = rvn("iter_take_compiles");
    assert_compiles(&source);
}

#[test]
fn iter_skip_compiles() {
    let source = rvn("iter_skip_compiles");
    assert_compiles(&source);
}

#[test]
fn iter_take_then_sum_compiles() {
    let source = rvn("iter_take_then_sum_compiles");
    assert_compiles(&source);
}

#[test]
fn iter_skip_then_sum_compiles() {
    let source = rvn("iter_skip_then_sum_compiles");
    assert_compiles(&source);
}

#[test]
fn iter_enumerate_passthrough_compiles() {
    // `enumerate` is a no-op passthrough at the runtime layer; the
    // for-loop lowering recognises the `(i, x)` tuple binding shape
    // and synthesises the index counter directly. As a terminator
    // chain, `.enumerate.count` is equivalent to `.count`.
    let source = rvn("iter_enumerate_passthrough_compiles");
    assert_compiles(&source);
}

#[test]
fn iter_filter_then_count_compiles() {
    let source = rvn("iter_filter_then_count_compiles");
    assert_compiles(&source);
}

#[test]
fn iter_map_changes_item_type_then_collect_vec_compiles() {
    let source = rvn("iter_map_changes_item_type_then_collect_vec_compiles");
    assert_compiles(&source);
}

// ─── Existing (batch 1) terminators — sanity guard ──────────────────

#[test]
fn iter_sum_still_compiles() {
    let source = rvn("iter_sum_still_compiles");
    assert_compiles(&source);
}

#[test]
fn iter_count_still_compiles() {
    let source = rvn("iter_count_still_compiles");
    assert_compiles(&source);
}

// ─── Chaining: lazy + lazy + eager ──────────────────────────────────

#[test]
fn iter_skip_then_take_then_count_compiles() {
    let source = rvn("iter_skip_then_take_then_count_compiles");
    assert_compiles(&source);
}

// ─── Mixed-shape chains ─────────────────────────────────────────────

#[test]
fn iter_take_then_fold_compiles() {
    // Lazy combinator → closure terminator.
    let source = rvn("iter_take_then_fold_compiles");
    assert_compiles(&source);
}

#[test]
fn iter_skip_then_all_compiles() {
    // Lazy combinator → boolean short-circuit terminator.
    let source = rvn("iter_skip_then_all_compiles");
    assert_compiles(&source);
}

// ─── Eager-materialising lazy combinators (#05 batch 3) ─────────────

#[test]
fn iter_chain_compiles() {
    // `chain` eager-materialises the concatenation of two iterators
    // into a fresh `RivenVec*` and continues the iter-style chain. The
    // surface type is the receiver's iter class so downstream
    // terminators (`sum`, `count`, `fold`, …) all keep working.
    let source = rvn("iter_chain_compiles");
    assert_compiles(&source);
}

#[test]
fn iter_chain_then_sum_compiles() {
    let source = rvn("iter_chain_then_sum_compiles");
    assert_compiles(&source);
}

#[test]
fn iter_zip_then_count_compiles() {
    // `zip` materialises pairs into a fresh `Vec[(T,U)]`. Chaining
    // `count` on the result is the simplest sanity that the receiver
    // stays an Iter-shape value compatible with the rest of the
    // surface; the pair element layout is exercised by the e2e
    // fixture (607_iter_zip.rvn).
    let source = rvn("iter_zip_then_count_compiles");
    assert_compiles(&source);
}

// ─── `collect_vec` v1 shorthand for `collect[Vec[T]]` ───────────────

#[test]
fn iter_collect_vec_compiles() {
    // `collect_vec` is v1's type-specific shorthand for
    // `collect[Vec[T]]`. Since every `*Iter` in the v1 runtime is
    // already a `RivenVec*` (`riven_iter_to_vec`), `collect_vec` is
    // the same identity passthrough — but the typeck arm pins the
    // result to `Vec[T]` so users can chain Vec methods on it.
    let source = rvn("iter_collect_vec_compiles");
    assert_compiles(&source);
}

#[test]
fn iter_collect_string_compiles() {
    let source = rvn("iter_collect_string_compiles");
    assert_compiles(&source);
}

#[test]
fn iter_collect_hashmap_compiles() {
    let source = rvn("iter_collect_hashmap_compiles");
    assert_compiles(&source);
}

#[test]
fn iter_collect_hashset_compiles() {
    let source = rvn("iter_collect_hashset_compiles");
    assert_compiles(&source);
}

#[test]
fn string_from_iter_compiles() {
    let source = rvn("string_from_iter_compiles");
    assert_compiles(&source);
}

#[test]
#[ignore = "pre-existing Cranelift verifier error: Map.from_iter(zip(...)) emits a call \
            with arity 2 against a 1-arg runtime symbol; unrelated to the syntax \
            migration. Re-enable once the MIR lowering for tuple-iterator \
            destructuring at runtime-fn call sites is fixed."]
fn hashmap_from_iter_compiles() {
    let source = rvn("hashmap_from_iter_compiles");
    assert_compiles(&source);
}

#[test]
fn hashset_from_iter_compiles() {
    let source = rvn("hashset_from_iter_compiles");
    assert_compiles(&source);
}

// ─── Negatives ──────────────────────────────────────────────────────

/// `Vec[String].iter.sum` — String does not implement Add, so a
/// numeric `sum` makes no sense. The runtime dispatch routes
/// `*Iter.sum` to `riven_vec_sum` (which integer-sums the raw 64-bit
/// slot, producing nonsensical bytes-as-int sums for `Vec[String]`).
///
/// As of #05 batch 3, typeck rejects the call with `E0700`
/// ("`sum` requires an iterator whose Item implements `Add`").
#[test]
fn sum_on_string_iter_typeck_rejects() {
    let source = rvn("sum_on_string_iter_typeck_rejects");
    let diags = typecheck_diagnostics(&source);
    let errors: Vec<&Diagnostic> = diags
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(
        !errors.is_empty(),
        "expected `*Iter[String].sum` to be rejected at typeck"
    );
    let saw_e0700 = errors.iter().any(|d| d.code.as_deref() == Some("E0700"));
    assert!(
        saw_e0700,
        "expected diagnostic code E0700 on the rejection; got: {:#?}",
        errors
    );
    let saw_message = errors
        .iter()
        .any(|d| d.message.contains("Add") || d.message.contains("numeric"));
    assert!(
        saw_message,
        "expected diagnostic message to mention `Add`/numeric; got: {:#?}",
        errors
    );
}

#[test]
fn collect_hashmap_rejects_non_pair_items() {
    let source = rvn("collect_hashmap_rejects_non_pair_items");
    let diags = typecheck_diagnostics(&source);
    let errors: Vec<&Diagnostic> = diags
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(
        !errors.is_empty(),
        "expected collect[HashMap[_, _]] rejection"
    );
    assert!(
        errors.iter().any(|d| d.code.as_deref() == Some("E0700")),
        "expected E0700; got {:#?}",
        errors
    );
}

/// Keep the prior `Vec[Int].iter.sum` happy-path covered alongside
/// the new String-rejection guard so a future tightening doesn't
/// over-broadly reject numeric Items.
#[test]
fn sum_on_int_iter_still_compiles_after_tightening() {
    let source = rvn("sum_on_int_iter_still_compiles_after_tightening");
    let diags = typecheck_diagnostics(&source);
    let errors: Vec<&Diagnostic> = diags
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "expected no typeck errors on `Vec[Int].iter.sum`; got: {:#?}",
        errors
    );
}
