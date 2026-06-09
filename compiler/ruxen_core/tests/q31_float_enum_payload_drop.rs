//! Q31 (S1 memory-safety) — constructing a `Float32`-payload enum variant two
//! or more times BY VALUE in one function must not crash.
//!
//! Root cause: `alloc_size` (`mir/lower/emit.rs`) sized enums to their PACKED
//! layout, but codegen addresses enum payload fields on a fixed 8-byte slot
//! stride (`GetPayload` = base+8, field N at `N*8`, in BOTH backends). For
//! `Move(Float32, Float32)` the packed size is 16, yet codegen writes payload
//! field 1 at offset `8 + 1*8 = 16` — a 4-byte store 4 bytes PAST the 16-byte
//! allocation, corrupting the next heap chunk's metadata. The first
//! construction corrupted the heap silently; the SECOND float-format `malloc`
//! (inside `dtoa`) then dereferenced the corrupted free-list and crashed with
//! `EXC_BAD_ACCESS`. That is why the bug needed >= 2 float-payload
//! constructions, and why `Int` payloads (already 8-byte slots, so the packed
//! size coincidentally matched the slot-addressed footprint) were unaffected.
//!
//! Fix: `alloc_size` now slot-rounds enums the same way it already did for
//! classes/structs — `8` (tag/payload-base slot) + `widest_variant_field_count
//! * 8` — so the allocation matches codegen's field addressing for any payload
//! width. The addressing is shared by Cranelift and LLVM, so both honor it.
//!
//! These pins COMPILE + RUN the binary and assert exact stdout + clean exit
//! (not just compile success): a revert of the fix makes them crash at runtime.

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

/// The canonical Q31 repro (canvas's event-loop shape): two `decode()` calls
/// each returning an `Ev.Move(Float32, Float32)`, each matched in one `main`.
/// Must print `frame1=204.75` / `frame2=204.75` and exit 0. Pre-fix this
/// crashed (EXC_BAD_ACCESS / SIGSEGV) inside the second float format.
#[test]
fn float_payload_enum_double_construct_runs() {
    let (src, expected) = case("652_enum_float_payload_double_construct.rx");
    let (stdout, stderr, ok) = compile_and_run(&src, "q31_652");
    assert!(ok, "non-zero exit (crash); stderr: {stderr}");
    assert_eq!(stdout, expected, "stdout was {stdout:?}");
}

/// A `while` loop constructing many Float32-payload enums over its lifetime
/// (1000 iterations) and matching each — proves the slot-rounded allocation
/// is sound across repeated construction, not just twice. Pre-fix this
/// corrupts the heap on the first overrunning store and crashes. Inlined
/// (not from a fixture file) so the loop shape is self-contained.
#[test]
fn float_payload_enum_loop_runs_clean() {
    let source = r#"
enum Ev
  Move(Float32, Float32)
  Scroll(Float32, Float32)
  Key(Int)
end

def decode(i: Int) -> Ev
  Ev.Move((i as Float) as Float32, ((i + 1) as Float) as Float32)
end

def main
  var i = 0
  var hits = 0
  while i < 1000
    match decode(i)
      Ev.Move(x, y)   -> hits = hits + 1
      Ev.Scroll(x, y) -> nil
      Ev.Key(k)       -> nil
    end
    i = i + 1
  end
  puts "hits=#{hits}"
end
"#;
    let (stdout, stderr, ok) = compile_and_run(source, "q31_loop");
    assert!(ok, "non-zero exit (crash); stderr: {stderr}");
    assert_eq!(stdout, "hits=1000\n", "stdout was {stdout:?}");
}

/// Int-payload control: same double-construct shape but with `Int` payloads
/// (already 8-byte slots, never regressed) — must still print correct sums.
/// Guards against the fix accidentally changing Int enum layout.
#[test]
fn int_payload_enum_double_construct_unaffected() {
    let source = r#"
enum Ev
  Move(Int, Int)
  Scroll(Int, Int)
  Key(Int)
end

def decode -> Ev
  Ev.Move(120, 84)
end

def main
  match decode()
    Ev.Move(x, y)   -> puts "f1=#{x + y}"
    Ev.Scroll(x, y) -> nil
    Ev.Key(k)       -> nil
  end
  match decode()
    Ev.Move(x, y)   -> puts "f2=#{x + y}"
    Ev.Scroll(x, y) -> nil
    Ev.Key(k)       -> nil
  end
end
"#;
    let (stdout, stderr, ok) = compile_and_run(source, "q31_int_ctrl");
    assert!(ok, "non-zero exit; stderr: {stderr}");
    assert_eq!(stdout, "f1=204\nf2=204\n", "stdout was {stdout:?}");
}
