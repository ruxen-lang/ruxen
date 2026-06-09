//! Q28 — an enum VARIANT carrying a `Float32`/`Float` payload must round-trip
//! through construction and `match` with no precision loss.
//!
//! Context: `canvas/src/event.rx` was forced to carry pointer coordinates as
//! `Int` logical pixels with a TODO ("return to Float32 payloads once enum
//! float payloads work"), costing sub-pixel precision on every pointer event.
//! A 2026-06-09 audit (feat/drop-elaboration) found the deviation is STALE: the
//! Q5 numeric-cast fix and the case-218 / 1b6ced0 struct/enum inline-method
//! float-codegen work incidentally made enum float payloads correct. The
//! mechanism, end to end:
//!
//!   - A `Float32` literal (`3.5f32`) lowers to `Assign { dest: <f32 temp>,
//!     value: Literal::Float(_) }`. The Cranelift/LLVM `Assign` handler emits an
//!     f64 const and then `coerce_value`-narrows it to the dest's declared f32,
//!     so the temp local is a real f32 BEFORE it ever reaches the constructor.
//!   - The constructor (`mir/lower/expr/constructors.rs`) stores each payload
//!     field with `SetField { value: Use(temp) }` at slot `idx*8`; codegen
//!     stores the value at its own width (4 bytes for f32, 8 for f64).
//!   - `match` payload extraction loads with `GetField`, whose load type is the
//!     PATTERN BINDING's declared type — f32 for an f32 field — so the slot is
//!     read back at the same width. No bit-pattern mismatch, no truncation.
//!
//! These pins read the SAME fixtures the release-e2e harness runs
//! (`tests/release-e2e/cases/647_*`, `648_*`) so the cargo pin and the e2e case
//! can never drift apart (the `dyn_fn_e2e_600` convention). They are regression
//! guards: if the typed slot path ever regresses to a width-blind store/load
//! (e.g. storing an f64 const directly into an f32 field), the sub-pixel value
//! corrupts and these fail.

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

/// `Float32` payloads (the exact shape canvas's event enum was forced away
/// from): sub-pixel coords 120.5 / 84.25 / 0.125 must survive construction +
/// match, and arithmetic on the extracted f32 (`x + 0.5f32`) must be exact.
#[test]
fn float32_payload_round_trips() {
    let (src, expected) = case("647_enum_float32_payload.rx");
    let (stdout, stderr, ok) = compile_and_run(&src, "q28_647");
    assert!(ok, "non-zero exit; stderr: {stderr}");
    assert_eq!(stdout, expected, "stdout was {stdout:?}");
}

/// `Float` (f64) payloads MIXED with `Int` variants, through a function and an
/// Array: exercises the second payload slot and the Int/Float variant
/// interleave a realistic event enum has.
#[test]
fn float64_mixed_payload_round_trips() {
    let (src, expected) = case("648_enum_float_mixed_payload.rx");
    let (stdout, stderr, ok) = compile_and_run(&src, "q28_648");
    assert!(ok, "non-zero exit; stderr: {stderr}");
    assert_eq!(stdout, expected, "stdout was {stdout:?}");
}
