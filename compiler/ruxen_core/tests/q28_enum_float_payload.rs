//! Q28 — a `Float32` struct field / enum VARIANT payload must round-trip
//! through construction and field-read / `match` with no precision loss.
//!
//! Context: `canvas/src/event.rx` was forced to carry pointer coordinates as
//! `Int` logical pixels with a TODO ("return to Float32 payloads once enum
//! float payloads work"), costing sub-pixel precision on every pointer event.
//!
//! REOPENED 2026-06-09: the earlier "already sound" verdict was WRONG. It was
//! tested only against INLINE `f32`-suffixed literals (`120.5f32`), which the
//! lowering narrows in place — that path always worked. On the real shapes the
//! `SetField`/`GetField` slot path was width-BLIND:
//!   - a bare `Float` (f64) literal/local placed into a `Float32` field stored
//!     8 bytes into a 4-byte slot, and the later f32 `GetField` read 0;
//!   - an uncast f64 local placed into the payload could even crash.
//! Only the inline `120.5f32` literal and the `expr as Float32` cast (and a
//! `Float32` fn-param coercion) produced an f32-typed SSA value before the
//! store, which is why those worked and masked the bug.
//!
//! FIX (`mir/lower/expr/constructors.rs` + `method_call.rs`): before each
//! width-blind `SetField` in a struct/enum/tuple constructor, the value is
//! coerced to the FIELD's declared width via a target-typed `Assign`
//! (`coerce_to_field_ty` → the shared `coerce_value` fdemote/fpromote/fcvt
//! path the `as`-cast already used). The store now happens at the slot width,
//! and `GetField` reads it back at the same width. Backend-agnostic — the
//! coercion is in shared MIR lowering, so Cranelift and LLVM agree.
//!
//! These pins read the SAME fixtures the release-e2e harness runs
//! (`tests/release-e2e/cases/647_*`, `648_*`, `650_*`, `651_*`) and — the
//! non-negotiable part of the reopen — they COMPILE + RUN the binary and assert
//! its exact stdout. 647/648 cover the inline-literal path; 650/651 cover the
//! LOAD-FROM-LOCAL and BARE-f64-LITERAL shapes that actually regressed, so the
//! real codegen path (not just compile success) is guarded and cannot silently
//! revert to a width-blind store again.

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

/// REOPEN pin — a `Float32` STRUCT field stored from a value bound to a LOCAL
/// (the canvas `let ia = event_a() as Float32; P.new(ia, ib)` shape), a bare
/// f64 literal into an f32 field, and an uncast f64 local — all must read the
/// field back as 204.75, not 0 (and must not crash). RUNS + asserts stdout.
#[test]
fn f32_field_store_via_local_round_trips() {
    let (src, expected) = case("650_f32_field_store_via_local.rx");
    let (stdout, stderr, ok) = compile_and_run(&src, "q28_650");
    assert!(ok, "non-zero exit; stderr: {stderr}");
    assert_eq!(stdout, expected, "stdout was {stdout:?}");
}

/// REOPEN pin — same load-from-local / bare-f64 / uncast-f64 shapes but for an
/// enum `Float32` PAYLOAD (the `GetPayload` + SetField path). RUNS + asserts
/// stdout (every line 204.75).
#[test]
fn f32_payload_store_via_local_round_trips() {
    let (src, expected) = case("651_enum_f32_payload_via_local.rx");
    let (stdout, stderr, ok) = compile_and_run(&src, "q28_651");
    assert!(ok, "non-zero exit; stderr: {stderr}");
    assert_eq!(stdout, expected, "stdout was {stdout:?}");
}
