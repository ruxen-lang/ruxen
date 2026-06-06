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

/// Bug A (no parens): a zero-arg `def self.X` static on a STRUCT, called in
/// paren-less form and immediately chained (`C3.white.val`), must dispatch
/// statically (no phantom `self`) and resolve the static's return type so the
/// chained instance method resolves. Regression: `is_user_static_method` only
/// matched `DefKind::Class`, so a struct static was mis-classified as an
/// instance call and a constant-0 receiver was prepended — tripping the
/// Cranelift arg-count verifier.
#[test]
fn struct_zero_arg_static_chained_no_parens() {
    let source = r##"
struct C3
  r: UInt8

  def self.white -> C3
    C3.new(255u8)
  end

  def val -> Int
    self.r as Int
  end
end

def main
  puts "direct=#{C3.white.val}"
end
"##;
    let out = compile_and_run("struct_static_noparens", source);
    assert!(
        out.contains("direct=255"),
        "expected 'direct=255', got: [{out}]"
    );
}

/// Bug A (parens): the same struct zero-arg static written `C3.white().val`.
/// Regression: `select_class_method` only fired for `Ty::Class`, so a struct
/// static fell through to the lenient fresh-var path and the result type
/// stayed `?T`, so the chained `.val` could not resolve (codegen emitted
/// `?T<n>_val`).
#[test]
fn struct_zero_arg_static_chained_parens() {
    let source = r##"
struct C4
  r: UInt8

  def self.white() -> C4
    C4.new(255u8)
  end

  def val -> Int
    self.r as Int
  end
end

def main
  puts "parens=#{C4.white().val}"
end
"##;
    let out = compile_and_run("struct_static_parens", source);
    assert!(
        out.contains("parens=255"),
        "expected 'parens=255', got: [{out}]"
    );
}

/// Bug B: an INLINE `(small_int as UInt32) << N` term must contribute the
/// widened, shifted value. Regression: the `Cast` MIR lowering passed the
/// inner value through unchanged, so the enclosing `<<` ran at the source
/// (8-bit) width and Cranelift masked the shift amount — `(1u8 as UInt32)
/// << 16` silently became 0. The packed value must equal 0xFF010203.
#[test]
fn inline_cast_then_shift_packs_correctly() {
    let source = r##"
def main
  let x = (255u8 as UInt32) << 24 | ((1u8 as UInt32) << 16) | ((2u8 as UInt32) << 8) | (3u8 as UInt32)
  puts "#{x}"
end
"##;
    let out = compile_and_run("inline_cast_shift", source);
    // 0xFF010203 == 4278256131
    assert!(
        out.contains("4278256131"),
        "expected packed 0xFF010203 (4278256131), got: [{out}]"
    );
}

/// Bug C: an un-annotated `let c = Struct.new(...)` followed by field access
/// inside a closure body must resolve `c`'s type to the struct. Regression:
/// the struct constructor's result type stayed `?T` inside a closure (the
/// same `select_class_method` Struct/Enum gap as Bug A), so `c.a` lowered to
/// `?T<n>_a`. The closure is passed to a user function so it lowers to a
/// standalone closure function (where the bug surfaced).
#[test]
fn unannotated_struct_let_field_access_in_closure() {
    let source = r##"
struct Color
  r: UInt8
  g: UInt8
  b: UInt8
  a: UInt8

  def self.rgb(r: UInt8, g: UInt8, b: UInt8) -> Color
    Color.new(r, g, b, 255u8)
  end
end

def main
  let nums = [1]
  nums.each do |_n|
    let c = Color.new(10u8, 20u8, 30u8, 255u8)
    puts "a=#{c.a} r=#{c.r}"
    let d = Color.rgb(1u8, 2u8, 3u8)
    puts "da=#{d.a} dr=#{d.r}"
  end
end
"##;
    let out = compile_and_run("unannot_struct_closure", source);
    assert!(
        out.contains("a=255 r=10"),
        "expected 'a=255 r=10', got: [{out}]"
    );
    assert!(
        out.contains("da=255 dr=1"),
        "expected 'da=255 dr=1', got: [{out}]"
    );
}
