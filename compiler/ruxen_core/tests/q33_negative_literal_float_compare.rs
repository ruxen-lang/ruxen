//! Q33 — comparing a float value (`Float32` / `Float`) against a NEGATIVE Int
//! literal must evaluate correctly. It silently returned `false` for equality
//! and gave inverted ordering results.
//!
//! Surfaced 2026-06-09 by canvas's `Scroll(-1, 3)` round-trip pin: `f32 == -1`
//! was false even when the stored value was exactly `-1.0`.
//!
//! Root cause (`codegen/cranelift/emit.rs` `Compare`): the `Compare`
//! instruction is width-blind (like `SetField` in Q28). Codegen coerced the
//! rhs to the lhs's SSA type with the signedness-BLIND `coerce_value`, which
//! selects `fcvt_from_uint` for an int->float crossing. A signed `Int(-1)`
//! (i64 `0xFFFF_FFFF_FFFF_FFFF`) therefore became `1.84e19`, so `f == -1`
//! `fcmp`'d false. The ordering operators broke too (`>= -1` false, `< -1`
//! true) — the bogus huge value only accidentally satisfied `<=` / `>`. The
//! literal-on-the-left shape (`-1 == f`) broke symmetrically through
//! `fcvt_to_uint_sat` clamping `-1.0` to `0`.
//!
//! FIX (`mir/lower/expr/binops.rs` `coerce_compare_operands`): before the
//! width-blind `Compare`, a mismatched numeric operand pair is re-materialized
//! to a common float width via a target-typed `Assign`, which invokes codegen's
//! Q5 signedness-aware int<->float path (`fcvt_from_sint` for a signed source) —
//! exactly as a `let`-bound `as Float32` cast already does. Both operands then
//! reach `Compare` at the same float width with the right sign. Mirrors Q28's
//! `coerce_to_field_ty`. Backend-agnostic (shared MIR), so Cranelift and LLVM
//! agree.
//!
//! These pins read the SAME fixtures the release-e2e harness runs
//! (`tests/release-e2e/cases/653_*`, `654_*`) and — the non-negotiable part —
//! they COMPILE + RUN the binary and assert its exact stdout. A pin that only
//! type-checks would have passed against the broken codegen.

use ruxen_core::codegen;
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;
use std::path::PathBuf;
use std::process::Command;

fn workspace_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

fn compile_and_run(source: &str, basename: &str) -> (String, String, bool) {
    let root = workspace_root();
    let tmp_dir = root.join("tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let bin_path = tmp_dir.join(format!("{}-{}.bin", basename, std::process::id()));

    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "typecheck errors for {basename}: {errors:?}"
    );

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .unwrap_or_else(|e| panic!("MIR lowering for {basename}: {e}"));
    codegen::compile(&mir, bin_path.to_str().unwrap())
        .unwrap_or_else(|e| panic!("codegen for {basename}: {e}"));

    let output = Command::new(&bin_path)
        .output()
        .unwrap_or_else(|e| panic!("run {basename}: {e}"));
    let _ = std::fs::remove_file(&bin_path);
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.success(),
    )
}

fn case(name: &str) -> (String, String) {
    let root = workspace_root();
    let src = std::fs::read_to_string(root.join("tests/release-e2e/cases").join(name))
        .unwrap_or_else(|e| panic!("read case {name}: {e}"));
    let out_name = name.replace(".rx", ".out");
    let expected = std::fs::read_to_string(root.join("tests/release-e2e/expected").join(out_name))
        .unwrap_or_else(|e| panic!("read expected for {name}: {e}"));
    (src, expected)
}

/// Full matrix: `eq`/`ne`/`le`/`ge`/`lt`/`gt` of a `Float32` (and a `Float`)
/// against a negative Int literal, the literal-on-the-left shape, and a
/// wider-float common-type pair. Every line must match the expected stdout.
#[test]
fn f32_negative_int_literal_compare_matrix() {
    let (src, expected) = case("653_f32_negative_int_literal_compare.rx");
    let (stdout, stderr, ok) = compile_and_run(&src, "q33_653");
    assert!(ok, "non-zero exit; stderr: {stderr}");
    assert_eq!(stdout, expected, "stdout was {stdout:?}");
}

/// The original surfacing shape: a `Float32` enum payload extracted in a
/// `match`, then compared against a negative Int literal (canvas's
/// `Scroll(-1, 3)` round-trip). RUNS + asserts exact stdout.
#[test]
fn enum_f32_payload_negative_compare() {
    let (src, expected) = case("654_enum_f32_payload_negative_compare.rx");
    let (stdout, stderr, ok) = compile_and_run(&src, "q33_654");
    assert!(ok, "non-zero exit; stderr: {stderr}");
    assert_eq!(stdout, expected, "stdout was {stdout:?}");
}
