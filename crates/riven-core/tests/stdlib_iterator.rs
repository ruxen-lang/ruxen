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
    let source = r#"
def main
  let v = vec![1, 2, 3, 4]
  let total = v.iter.fold(0) { |acc, n| acc + n }
  let s = total.to_string
  puts "total=#{s}"
end
"#;
    assert_compiles(source);
}

#[test]
fn iter_all_compiles_returning_bool() {
    let source = r#"
def main
  let v = vec![2, 4, 6]
  let ok = v.iter.all { |n| n % 2 == 0 }
  if ok
    puts "all even"
  else
    puts "mixed"
  end
end
"#;
    assert_compiles(source);
}

#[test]
fn iter_any_compiles_returning_bool() {
    let source = r#"
def main
  let v = vec![1, 2, 3]
  let has = v.iter.any { |n| n > 2 }
  if has
    puts "yes"
  else
    puts "no"
  end
end
"#;
    assert_compiles(source);
}

// ─── Lazy combinators ───────────────────────────────────────────────

#[test]
fn iter_take_compiles() {
    let source = r#"
def main
  let v = vec![1, 2, 3, 4, 5]
  let n = v.iter.take(2).count
  let s = n.to_string
  puts "n=#{s}"
end
"#;
    assert_compiles(source);
}

#[test]
fn iter_skip_compiles() {
    let source = r#"
def main
  let v = vec![10, 20, 30, 40]
  let n = v.iter.skip(2).count
  let s = n.to_string
  puts "n=#{s}"
end
"#;
    assert_compiles(source);
}

#[test]
fn iter_take_then_sum_compiles() {
    let source = r#"
def main
  let v = vec![1, 2, 3, 4, 5]
  let s = v.iter.take(3).sum
  let out = s.to_string
  puts "s=#{out}"
end
"#;
    assert_compiles(source);
}

#[test]
fn iter_skip_then_sum_compiles() {
    let source = r#"
def main
  let v = vec![1, 2, 3, 4]
  let s = v.iter.skip(2).sum
  let out = s.to_string
  puts "s=#{out}"
end
"#;
    assert_compiles(source);
}

#[test]
fn iter_enumerate_passthrough_compiles() {
    // `enumerate` is a no-op passthrough at the runtime layer; the
    // for-loop lowering recognises the `(i, x)` tuple binding shape
    // and synthesises the index counter directly. As a terminator
    // chain, `.enumerate.count` is equivalent to `.count`.
    let source = r#"
def main
  let v = vec![10, 20, 30]
  let n = v.iter.enumerate.count
  let s = n.to_string
  puts "n=#{s}"
end
"#;
    assert_compiles(source);
}

// ─── Existing (batch 1) terminators — sanity guard ──────────────────

#[test]
fn iter_sum_still_compiles() {
    let source = r#"
def main
  let v = vec![1, 2, 3]
  let s = v.iter.sum
  let out = s.to_string
  puts "s=#{out}"
end
"#;
    assert_compiles(source);
}

#[test]
fn iter_count_still_compiles() {
    let source = r#"
def main
  let v = vec![1, 2, 3]
  let n = v.iter.count
  let s = n.to_string
  puts "n=#{s}"
end
"#;
    assert_compiles(source);
}

// ─── Chaining: lazy + lazy + eager ──────────────────────────────────

#[test]
fn iter_skip_then_take_then_count_compiles() {
    let source = r#"
def main
  let v = vec![1, 2, 3, 4, 5, 6]
  let n = v.iter.skip(1).take(3).count
  let s = n.to_string
  puts "n=#{s}"
end
"#;
    assert_compiles(source);
}

// ─── Mixed-shape chains ─────────────────────────────────────────────

#[test]
fn iter_take_then_fold_compiles() {
    // Lazy combinator → closure terminator.
    let source = r#"
def main
  let v = vec![10, 20, 30, 40]
  let total = v.iter.take(2).fold(0) { |acc, n| acc + n }
  let s = total.to_string
  puts "total=#{s}"
end
"#;
    assert_compiles(source);
}

#[test]
fn iter_skip_then_all_compiles() {
    // Lazy combinator → boolean short-circuit terminator.
    let source = r#"
def main
  let v = vec![1, 2, 3, 4, 5]
  let ok = v.iter.skip(2).all { |n| n > 2 }
  if ok
    puts "ok=true"
  else
    puts "ok=false"
  end
end
"#;
    assert_compiles(source);
}

// ─── Eager-materialising lazy combinators (#05 batch 3) ─────────────

#[test]
fn iter_chain_compiles() {
    // `chain` eager-materialises the concatenation of two iterators
    // into a fresh `RivenVec*` and continues the iter-style chain. The
    // surface type is the receiver's iter class so downstream
    // terminators (`sum`, `count`, `fold`, …) all keep working.
    let source = r#"
def main
  let a = vec![1, 2, 3]
  let b = vec![4, 5]
  let n = a.iter.chain(b.iter).count
  let s = n.to_string
  puts "n=#{s}"
end
"#;
    assert_compiles(source);
}

#[test]
fn iter_chain_then_sum_compiles() {
    let source = r#"
def main
  let a = vec![10, 20]
  let b = vec![30, 40]
  let total = a.iter.chain(b.iter).sum
  let s = total.to_string
  puts "total=#{s}"
end
"#;
    assert_compiles(source);
}

#[test]
fn iter_zip_then_count_compiles() {
    // `zip` materialises pairs into a fresh `Vec[(T,U)]`. Chaining
    // `count` on the result is the simplest sanity that the receiver
    // stays an Iter-shape value compatible with the rest of the
    // surface; the pair element layout is exercised by the e2e
    // fixture (607_iter_zip.rvn).
    let source = r#"
def main
  let a = vec![1, 2, 3, 4]
  let b = vec![10, 20, 30]
  let n = a.iter.zip(b.iter).count
  let s = n.to_string
  puts "n=#{s}"
end
"#;
    assert_compiles(source);
}

// ─── `collect_vec` v1 shorthand for `collect[Vec[T]]` ───────────────

#[test]
fn iter_collect_vec_compiles() {
    // `collect_vec` is v1's type-specific shorthand for
    // `collect[Vec[T]]`. Since every `*Iter` in the v1 runtime is
    // already a `RivenVec*` (`riven_iter_to_vec`), `collect_vec` is
    // the same identity passthrough — but the typeck arm pins the
    // result to `Vec[T]` so users can chain Vec methods on it.
    let source = r#"
def main
  let v = vec![1, 2, 3]
  let out = v.iter.take(2).collect_vec
  let n = out.len
  let s = n.to_string
  puts "n=#{s}"
end
"#;
    assert_compiles(source);
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
    let source = r#"
def main
  let v: Vec[String] = Vec.new
  let _ = v.iter.sum
end
"#;
    let diags = typecheck_diagnostics(source);
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

/// Keep the prior `Vec[Int].iter.sum` happy-path covered alongside
/// the new String-rejection guard so a future tightening doesn't
/// over-broadly reject numeric Items.
#[test]
fn sum_on_int_iter_still_compiles_after_tightening() {
    let source = r#"
def main
  let v: Vec[Int] = vec![1, 2, 3]
  let _ = v.iter.sum
end
"#;
    let diags = typecheck_diagnostics(source);
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
