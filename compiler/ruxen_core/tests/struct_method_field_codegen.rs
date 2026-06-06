//! Regression: struct-method receiver field access (`self.<field>`) must
//! load the NAMED field at its real offset and type — not always field 0
//! as i64.
//!
//! Root cause: `typeck::infer` never visited struct inline-method bodies
//! (`HirItem::Struct(_) => {}`), so `self.field` accesses kept
//! `field_idx = 0` and an `Infer` result type. Codegen then loaded field 0
//! as a raw i64. Symptoms:
//!   * Int field reads returned the wrong (first) field's value.
//!   * UInt8 field reads all returned field 0's byte.
//!   * Float / Float32 field reads failed Cranelift verification
//!     (i64 result vs f32/f64 signature).
//!
//! Companion bug: the derived `hash_code` for a float-field struct fed a
//! raw `f64`/`f32` into the integer FNV mix (`bxor`), failing verification.
//! Fixed by reinterpreting the float bits via `MirInst::FloatToBits`.

use ruxen_core::codegen;
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;
use std::process::Command;

fn workspace_root() -> std::path::PathBuf {
    let crate_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

fn unique_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Compile `source` to a native binary, run it, and return its stdout.
/// Panics with the collected diagnostics / stderr on any failure.
fn compile_and_run(label: &str, source: &str) -> String {
    let root = workspace_root();
    let tmp_dir = root.join("tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let bin_path = tmp_dir.join(format!(
        "{label}-{}-{}.bin",
        std::process::id(),
        unique_id()
    ));

    let tokens = Lexer::new(source).tokenize().expect("lex");
    let program = Parser::new(tokens).parse().expect("parse");
    let result = typeck::type_check(&program);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "[{label}] typecheck errors: {:?}", errors);

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer.lower_program(&result.program).expect("MIR lowering");

    codegen::compile(&mir, bin_path.to_str().unwrap())
        .unwrap_or_else(|e| panic!("[{label}] codegen failed: {e}"));

    let output = Command::new(&bin_path).output().expect("run binary");
    let _ = std::fs::remove_file(&bin_path);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        output.status.success(),
        "[{label}] binary exited non-zero. stdout=[{}] stderr=[{}]",
        stdout,
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
}

/// Symptom A: a struct method that reads a non-first Int field must return
/// THAT field, not field 0.
#[test]
fn struct_method_reads_non_first_int_field() {
    let source = r##"
struct Wide
  x: Int
  y: Int

  def gety -> Int
    self.y
  end
end

def main
  let w = Wide.new(10, 20)
  puts "wide y=#{w.gety}"
end
"##;
    let out = compile_and_run("struct_int_field", source);
    assert!(
        out.contains("wide y=20"),
        "expected 'wide y=20' (field 1), got: [{out}]"
    );
}

/// Symptom A (UInt8): each of r/g/b/a read inside a method must return its
/// own byte, not field 0's byte.
#[test]
fn struct_method_reads_uint8_fields() {
    let source = r##"
struct Color
  r: UInt8
  g: UInt8
  b: UInt8
  a: UInt8

  def describe -> String
    "method: r=#{self.r} g=#{self.g} b=#{self.b} a=#{self.a}"
  end
end

def main
  let c = Color.new(1u8, 2u8, 3u8, 255u8)
  puts c.describe
end
"##;
    let out = compile_and_run("struct_uint8_fields", source);
    assert!(
        out.contains("method: r=1 g=2 b=3 a=255"),
        "each UInt8 field must read its own byte, got: [{out}]"
    );
}

/// Symptom B: a struct method that does arithmetic on Float32 fields must
/// type as f32 and pass Cranelift verification (no i64-vs-f32 mismatch).
#[test]
fn struct_method_float32_field_arithmetic() {
    let source = r##"
struct P
  x: Float32
  y: Float32

  def sum -> Float32
    self.x + self.y
  end
end

def main
  let p = P.new(1.5f32, 2.5f32)
  puts "sum=#{p.sum}"
end
"##;
    let out = compile_and_run("struct_f32_sum", source);
    assert!(out.contains("sum=4"), "expected 'sum=4', got: [{out}]");
}

/// Symptom B (Float64): same as above with the 64-bit float type.
#[test]
fn struct_method_float64_field_arithmetic() {
    let source = r##"
struct P
  x: Float
  y: Float

  def sum -> Float
    self.x + self.y
  end
end

def main
  let p = P.new(1.5, 2.5)
  puts "sum=#{p.sum}"
end
"##;
    let out = compile_and_run("struct_f64_sum", source);
    assert!(out.contains("sum=4"), "expected 'sum=4', got: [{out}]");
}

/// Bug 2: a float-field struct (no methods) must compile — its derived
/// `hash_code` must bitcast the float to integer bits before the FNV mix
/// rather than feeding a raw f64 into `bxor`.
#[test]
fn float_field_struct_compiles() {
    let source = r##"
struct P
  x: Float
  y: Float
end

def main
  let p = P.new(1.0, 2.0)
  puts "#{p.x + p.y}"
end
"##;
    let out = compile_and_run("float_struct_hash", source);
    assert!(out.contains('3'), "expected '3.0' sum, got: [{out}]");
}
