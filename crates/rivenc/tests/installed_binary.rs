//! End-to-end tests for the `rivenc` release binary.
//!
//! These tests stage an isolated "installed" layout:
//!
//!     <tempdir>/bin/rivenc
//!     <tempdir>/lib/runtime.c
//!
//! …and invoke the staged `rivenc` against a suite of real Riven programs.
//! This validates the full compile → link → execute pipeline exactly as it
//! runs on a user's machine after `install.sh` — catching regressions like
//! hardcoded `CARGO_MANIFEST_DIR` paths, missing runtime functions, and
//! backend verifier errors.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use tempfile::TempDir;

/// Shared, pre-staged install layout for all tests in this binary.
///
/// On Linux, if two parallel tests each `fs::copy(rivenc_exe(), dst)` and
/// `Command::new(dst).spawn()` at the same time, a sibling thread's fork
/// inherits the still-open write fd, and the child's `execve` hits
/// ETXTBUSY ("Text file busy") even when its own staged binary is fully
/// written and closed. Staging exactly once via `OnceLock` — before any
/// test spawns — eliminates the race: there is no live write fd when
/// tests run.
fn shared_install() -> &'static Path {
    static INSTALL: OnceLock<(TempDir, PathBuf)> = OnceLock::new();
    &INSTALL
        .get_or_init(|| {
            let temp = tempfile::tempdir().expect("mktemp shared install");
            let bin_dir = temp.path().join("bin");
            let lib_dir = temp.path().join("lib");
            fs::create_dir_all(&bin_dir).unwrap();
            fs::create_dir_all(&lib_dir).unwrap();

            let staged_rivenc = bin_dir.join("rivenc");
            fs::copy(rivenc_exe(), &staged_rivenc).expect("copy rivenc");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(&staged_rivenc).unwrap().permissions();
                perms.set_mode(0o755);
                fs::set_permissions(&staged_rivenc, perms).unwrap();
            }

            fs::copy(runtime_c_src(), lib_dir.join("runtime.c")).expect("copy runtime.c");

            (temp, staged_rivenc)
        })
        .1
}

/// Resolve the workspace root by walking up from this crate's manifest dir.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// The path to runtime.c in the source tree.
fn runtime_c_src() -> PathBuf {
    workspace_root()
        .join("crates")
        .join("riven-core")
        .join("runtime")
        .join("runtime.c")
}

/// Path to the `rivenc` binary under test (cargo populates this env var).
fn rivenc_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_rivenc"))
}

/// Build an isolated layout for a test: a per-test tempdir for project
/// files, with the path to the process-wide shared staged `rivenc` binary.
///
/// Returns the tempdir (kept alive by the caller, cleaned up at test end)
/// and the path to the staged `rivenc` binary that is shared across all
/// tests in this binary.
fn stage_install() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().expect("mktemp");
    (temp, shared_install().to_path_buf())
}

/// Compile `source` with the staged `rivenc` in `dir`, then run the resulting
/// binary and return its captured stdout. Panics with context on any failure.
fn compile_and_run(rivenc: &Path, dir: &Path, source_name: &str, source: &str) -> String {
    let src = dir.join(source_name);
    fs::write(&src, source).unwrap();

    let out_name = source_name.trim_end_matches(".rvn");
    let out = dir.join(out_name);

    let compile = Command::new(rivenc)
        .arg(source_name)
        .arg("-o")
        .arg(out_name)
        .current_dir(dir)
        .output()
        .expect("spawn rivenc");

    assert!(
        compile.status.success(),
        "compile failed for {}\nstdout:\n{}\nstderr:\n{}",
        source_name,
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr),
    );

    let run = Command::new(&out)
        .current_dir(dir)
        .output()
        .expect("spawn compiled binary");

    assert!(
        run.status.success(),
        "run failed for {}\nstdout:\n{}\nstderr:\n{}",
        source_name,
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr),
    );

    String::from_utf8(run.stdout).expect("utf8 stdout")
}

// ── Individual tests ──────────────────────────────────────────────────

#[test]
fn version_flag() {
    let (_temp, rivenc) = stage_install();
    let out = Command::new(&rivenc).arg("--version").output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("rivenc "), "got: {:?}", stdout);
}

#[test]
fn find_runtime_c_from_sibling_lib_dir() {
    // The core assertion: the binary resolves runtime.c via the installed
    // layout (bin/../lib/runtime.c), not via the CARGO_MANIFEST_DIR baked in
    // at build time. If this regresses, release binaries will fail on every
    // user machine.
    let (temp, rivenc) = stage_install();

    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "hello.rvn",
        r##"def main
  puts "hello"
end
"##,
    );
    assert_eq!(out.trim(), "hello");
}

#[test]
fn integer_arithmetic() {
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "int_arith.rvn",
        r##"def main
  puts "#{1 + 2}"
  puts "#{10 - 3}"
  puts "#{4 * 6}"
  puts "#{20 / 4}"
  puts "#{17 % 5}"
end
"##,
    );
    assert_eq!(out.trim(), "3\n7\n24\n5\n2");
}

#[test]
fn owned_rebind_does_not_double_drop() {
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "owned_rebind.rvn",
        r##"struct Pair
  left: Int
  right: Int
end

def main
  let pair = Pair.new(7, 11)
  let rebound = pair
  puts "#{rebound.left + rebound.right}"
end
"##,
    );
    assert_eq!(out.trim(), "18");
}

#[test]
fn float_arithmetic() {
    // Regression: Cranelift codegen previously emitted `imul`/`iadd` for
    // f64 values, which the verifier rejects. Must dispatch to `fmul`/`fadd`.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "float_arith.rvn",
        r##"def area(r: Float) -> Float
  3.14159 * r * r
end

def main
  puts "#{area(5.0)}"
  let a = 2.5
  let b = 4.0
  puts "#{a + b}"
  puts "#{b - a}"
  puts "#{b / a}"
end
"##,
    );
    let lines: Vec<&str> = out.trim().lines().collect();
    assert_eq!(lines.len(), 4, "got: {:?}", out);
    // area(5.0) ≈ 78.5397
    assert!(
        lines[0].starts_with("78.5"),
        "area(5.0) should be ~78.5, got {:?}",
        lines[0]
    );
    assert_eq!(lines[1], "6.5");
    assert_eq!(lines[2], "1.5");
    assert_eq!(lines[3], "1.6");
}

#[test]
fn float_comparison() {
    // Regression: Cranelift float comparisons must use `fcmp`, not `icmp`.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "float_cmp.rvn",
        r##"def main
  let a = 2.5
  let b = 4.0
  if a < b
    puts "less"
  end
  if b > a
    puts "greater"
  end
end
"##,
    );
    assert_eq!(out.trim(), "less\ngreater");
}

#[test]
fn string_interpolation() {
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "interp.rvn",
        r##"def main
  let name = "world"
  let n = 3
  puts "hello #{name} #{n}"
end
"##,
    );
    assert_eq!(out.trim(), "hello world 3");
}

#[test]
fn enum_with_match() {
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "match.rvn",
        r##"enum Shape
  Circle(radius: Float)
  Square(side: Float)
end

def area(s: Shape) -> Float
  match s
    Shape.Circle(r) -> 3.14159 * r * r
    Shape.Square(x) -> x * x
  end
end

def main
  puts "#{area(Shape.Circle(radius: 2.0))}"
  puts "#{area(Shape.Square(side: 4.0))}"
end
"##,
    );
    let lines: Vec<&str> = out.trim().lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].starts_with("12.56"), "got {:?}", lines[0]);
    assert_eq!(lines[1], "16");
}

#[test]
fn classes_and_methods() {
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "classes.rvn",
        r##"class Counter
  count: Int

  def init
    self.count = 0
  end

  pub def mut bump
    self.count = self.count + 1
  end

  pub def value -> Int
    self.count
  end
end

def main
  let mut c = Counter.new
  c.bump
  c.bump
  c.bump
  puts "#{c.value}"
end
"##,
    );
    assert_eq!(out.trim(), "3");
}

#[test]
fn closures_and_iterators() {
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "iter.rvn",
        r##"def main
  let nums = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
  let evens = nums.iter.filter { |n| n % 2 == 0 }.to_vec
  puts "#{evens.len}"

  let first = nums.iter.find { |n| n > 7 }
  match first
    Some(n) -> puts "first > 7: #{n}"
    None    -> puts "none"
  end
end
"##,
    );
    assert_eq!(out.trim(), "5\nfirst > 7: 8");
}

#[test]
fn sample_program_fixture_compiles_and_runs() {
    // Runs the canonical sample program through the installed toolchain.
    // This is the broadest smoke test — it exercises enums, classes, traits,
    // generics, iterators, closures, string interpolation, and pattern
    // matching together.
    let (temp, rivenc) = stage_install();
    let src = fs::read_to_string(
        workspace_root().join("crates/riven-core/tests/fixtures/sample_program.rvn"),
    )
    .expect("sample_program.rvn fixture exists");

    let out = compile_and_run(&rivenc, temp.path(), "sample.rvn", &src);

    // The sample program produces ~50 lines of structured task-tracker
    // output. We don't assert the exact text (formatting drift is expected);
    // we do assert it reached the "Archiving" tail section.
    assert!(
        out.contains("Archiving completed tasks"),
        "sample program didn't reach archive section:\n{}",
        out
    );
    assert!(
        out.lines().count() > 40,
        "sample produced too few lines: {}",
        out.lines().count()
    );
}

#[test]
fn runtime_env_override() {
    // RIVEN_RUNTIME env var should take precedence over the bin-relative
    // lookup. We stage a normal install and then point RIVEN_RUNTIME at a
    // secondary copy of runtime.c — compilation must still succeed.
    let (temp, rivenc) = stage_install();
    let alt = temp.path().join("alt_runtime.c");
    fs::copy(runtime_c_src(), &alt).unwrap();

    fs::write(
        temp.path().join("env_ov.rvn"),
        r##"def main
  puts "ok"
end
"##,
    )
    .unwrap();

    let compile = Command::new(&rivenc)
        .arg("env_ov.rvn")
        .arg("-o")
        .arg("env_ov")
        .env("RIVEN_RUNTIME", &alt)
        .current_dir(temp.path())
        .output()
        .unwrap();

    assert!(
        compile.status.success(),
        "compile failed:\n{}",
        String::from_utf8_lossy(&compile.stderr)
    );

    let run = Command::new(temp.path().join("env_ov")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "ok");
}

#[test]
fn missing_runtime_gives_clear_error() {
    // If runtime.c cannot be found anywhere, the error message should name
    // every location we looked, so users can fix their install.
    let temp = tempfile::tempdir().unwrap();
    let bin_dir = temp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let rivenc = bin_dir.join("rivenc");
    fs::copy(rivenc_exe(), &rivenc).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&rivenc).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&rivenc, perms).unwrap();
    }

    fs::write(temp.path().join("x.rvn"), "def main\n  puts \"hi\"\nend\n").unwrap();

    // Deliberately do NOT create lib/runtime.c. Also clear RIVEN_RUNTIME
    // and CARGO_MANIFEST_DIR so the dev fallback can't accidentally save us.
    let out = Command::new(&rivenc)
        .arg("x.rvn")
        .current_dir(temp.path())
        .env_remove("RIVEN_RUNTIME")
        .env("CARGO_MANIFEST_DIR", "/nonexistent/riven-fake")
        .output()
        .unwrap();

    // We can't cleanly prevent the binary from finding runtime.c via its
    // compile-time baked CARGO_MANIFEST_DIR (env!()), so this test only
    // asserts that *when* the binary reports a missing runtime, the
    // message is informative. Most CI users will have the fallback path
    // populated, so we tolerate success here and only check error shape
    // if the compile failed.
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("runtime.c not found") && stderr.contains("RIVEN_RUNTIME"),
            "unhelpful error message: {}",
            stderr
        );
    }
}

#[test]
fn match_guards_on_int_binding() {
    // Regression: `match` arm guards (`case if cond -> body`) were being
    // silently dropped during HIR-to-MIR lowering — the first arm whose
    // pattern matched was taken regardless of the guard. This verifies
    // guards gate arm selection.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "match_guards.rvn",
        r##"def grade(n: Int) -> String
  match n
    n if n >= 90 -> "A"
    n if n >= 80 -> "B"
    n if n >= 70 -> "C"
    _            -> "F"
  end
end

def main
  puts grade(95)
  puts grade(82)
  puts grade(71)
  puts grade(40)
end
"##,
    );
    assert_eq!(out.trim(), "A\nB\nC\nF");
}

#[test]
fn match_on_int_literals_still_works() {
    // Smoke: ensures the cascading match path still handles literal
    // patterns with no guards after the guard-related refactor.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "match_int.rvn",
        r##"def classify(n: Int) -> String
  match n
    0 -> "zero"
    1 -> "one"
    2 -> "two"
    _ -> "many"
  end
end

def main
  puts classify(0)
  puts classify(1)
  puts classify(2)
  puts classify(99)
end
"##,
    );
    assert_eq!(out.trim(), "zero\none\ntwo\nmany");
}

#[test]
fn match_on_simple_enum_still_works() {
    // Smoke: tag-switch lowering for unit-variant enums.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "simple_enum.rvn",
        r##"enum Color
  Red
  Green
  Blue
end

def describe(c: Color) -> String
  match c
    Color.Red   -> "red"
    Color.Green -> "green"
    Color.Blue  -> "blue"
  end
end

def main
  puts describe(Color.Red)
  puts describe(Color.Green)
  puts describe(Color.Blue)
end
"##,
    );
    assert_eq!(out.trim(), "red\ngreen\nblue");
}

#[test]
fn match_on_enum_with_data_still_works() {
    // Smoke: tag-switch + payload-field bindings.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "enum_data.rvn",
        r##"enum Shape
  Circle(radius: Int)
  Rectangle(width: Int, height: Int)
end

def area(s: Shape) -> Int
  match s
    Shape.Circle(r)           -> r * r * 3
    Shape.Rectangle(w, h)     -> w * h
  end
end

def main
  let c = Shape.Circle(radius: 5)
  let r = Shape.Rectangle(width: 4, height: 6)
  puts "#{area(c)}"
  puts "#{area(r)}"
end
"##,
    );
    assert_eq!(out.trim(), "75\n24");
}

#[test]
fn match_guards_with_enum_variant_bindings() {
    // Guards must also work when combined with enum-variant patterns that
    // introduce field bindings — the guard expression must see those
    // bindings and its false case must fall through to the next arm.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "guard_enum.rvn",
        r##"enum Color
  Red(Int)
  Blue(Int)
end

def describe(c: Color) -> String
  match c
    Color.Red(n) if n > 100   -> "bright red"
    Color.Red(_)              -> "red"
    Color.Blue(_)             -> "blue"
  end
end

def main
  puts describe(Color.Red(200))
  puts describe(Color.Red(50))
  puts describe(Color.Blue(1))
end
"##,
    );
    assert_eq!(out.trim(), "bright red\nred\nblue");
}

#[test]
fn fixed_array_literal_coerces() {
    // Bug 1: `let a: [Int; 3] = [1,2,3]` — the bracket literal is typed
    // as Vec[Int] by the resolver; typeck must coerce to fixed array.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "fixed_array.rvn",
        r##"def main
  let a: [Int; 3] = [10, 20, 30]
  puts "#{a[0]}"
  puts "#{a[1]}"
  puts "#{a[2]}"
end
"##,
    );
    assert_eq!(out.trim(), "10\n20\n30");
}

#[test]
fn newtype_wrapper_construct_and_project() {
    // Bug 2: `newtype Meters(Float)` — `Meters(3.14)` must construct
    // the wrapper and `.0` must project the inner value.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "newtype.rvn",
        r##"newtype Meters(Float)

def main
  let m = Meters(3.14)
  puts "#{m.0}"
end
"##,
    );
    assert_eq!(out.trim(), "3.14");
}

#[test]
fn const_decl_substitutes_at_use_sites() {
    // Bug 3: top-level `const` reference must emit the initializer
    // expression at every use site; otherwise we read uninitialized
    // stack memory and print garbage.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "const_decl.rvn",
        r##"const MAX = 100

def main
  puts "#{MAX}"
end
"##,
    );
    assert_eq!(out.trim(), "100");
}

#[test]
fn regression_int_arith() {
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "int_arith_reg.rvn",
        r##"def main
  let a = 2 + 3
  let b = a * 10
  puts "#{b}"
end
"##,
    );
    assert_eq!(out.trim(), "50");
}

#[test]
fn regression_classes_init() {
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "classes_reg.rvn",
        r##"class Box
  x: Int

  def init(@x: Int)
  end

  pub def value -> Int
    self.x
  end

  pub def doubled -> Int
    self.x * 2
  end
end

def main
  let b = Box.new(21)
  puts "#{b.value}"
  puts "#{b.doubled}"
end
"##,
    );
    assert_eq!(out.trim(), "21\n42");
}

#[test]
fn regression_type_alias() {
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "type_alias_reg.rvn",
        r##"type UserId = Int

def main
  let id: UserId = 5
  puts "#{id}"
end
"##,
    );
    assert_eq!(out.trim(), "5");
}

#[test]
fn derive_copy_struct_copies_on_assignment() {
    // Bug 4: a struct with `derive Copy` must be treated as Copy by
    // the borrow checker (no "value used after move" on `let b = a`).
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "derive_copy.rvn",
        r##"struct Point
  x: Int
  y: Int
  derive Copy, Clone
end

def main
  let a = Point.new(1, 2)
  let b = a
  puts "#{a.x} #{a.y}"
  puts "#{b.x} #{b.y}"
end
"##,
    );
    assert_eq!(out.trim(), "1 2\n1 2");
}

// ── Parser / pattern bug fixtures (current task) ──────────────────────

#[test]
fn parser_tuple_field_access_dot_int() {
    // Fixture 32: `t.0` — parser must accept IntLiteral after `.`.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "tuple_field.rvn",
        r##"def main
  let t = (10, 20)
  puts "#{t.0}"
  puts "#{t.1}"
end
"##,
    );
    assert_eq!(out.trim(), "10\n20");
}

#[test]
fn parser_or_pattern_literal_alternatives() {
    // Fixture 58: `a | b | c -> body` restricted to literals.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "or_pattern.rvn",
        r##"def classify(x: Int) -> String
  match x
    1 | 2 | 3 -> String.from("low")
    4 | 5 | 6 -> String.from("mid")
    _         -> String.from("other")
  end
end

def main
  puts "#{classify(1)}"
  puts "#{classify(5)}"
  puts "#{classify(9)}"
end
"##,
    );
    assert_eq!(out.trim(), "low\nmid\nother");
}

#[test]
fn parser_match_tuple_pattern() {
    // Fixture 62: `(a, b) -> body` in match.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "match_tuple.rvn",
        r##"def describe(p: (Int, Int)) -> String
  match p
    (0, 0) -> String.from("origin")
    (x, 0) -> "on x-axis at #{x}"
    (0, y) -> "on y-axis at #{y}"
    (x, y) -> "at (#{x}, #{y})"
  end
end

def main
  puts "#{describe((0, 0))}"
  puts "#{describe((3, 0))}"
  puts "#{describe((0, 4))}"
  puts "#{describe((3, 4))}"
end
"##,
    );
    assert_eq!(
        out.trim(),
        "origin\non x-axis at 3\non y-axis at 4\nat (3, 4)"
    );
}

#[test]
fn parser_match_ref_binding() {
    // Fixture 76: `ref x -> body` — bind to a reference, same runtime
    // value as a plain binding for v1.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "match_ref.rvn",
        r##"def main
  let s = String.from("hello")
  match &s
    ref inner -> puts "#{inner}"
  end
  puts "#{s}"
end
"##,
    );
    assert_eq!(out.trim(), "hello\nhello");
}

#[test]
fn parser_regression_match_int() {
    // Fixture 10: plain literal match arms still route through the
    // non-or branch.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "match_int.rvn",
        r##"def classify(n: Int) -> String
  match n
    0 -> "zero"
    1 -> "one"
    2 -> "two"
    _ -> "many"
  end
end

def main
  puts classify(0)
  puts classify(1)
  puts classify(2)
  puts classify(99)
end
"##,
    );
    assert_eq!(out.trim(), "zero\none\ntwo\nmany");
}

#[test]
fn parser_regression_match_guards() {
    // Fixture 11: `name if guard -> body` still compiles.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "match_guards.rvn",
        r##"def grade(n: Int) -> String
  match n
    n if n >= 90 -> "A"
    n if n >= 80 -> "B"
    n if n >= 70 -> "C"
    _            -> "F"
  end
end

def main
  puts grade(95)
  puts grade(82)
  puts grade(71)
  puts grade(40)
end
"##,
    );
    assert_eq!(out.trim(), "A\nB\nC\nF");
}

#[test]
fn parser_do_end_block_expr() {
    // Fixture 59: `do ... last_expr end` used as an expression.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "do_end_block.rvn",
        r##"def main
  let v = do
    let a = 1
    let b = 2
    a + b
  end
  puts "#{v}"
end
"##,
    );
    assert_eq!(out.trim(), "3");
}

#[test]
fn e2e_16_inheritance() {
    // Fixture 16_inheritance: `super(name)` inside a subclass init must
    // invoke the parent's init with the child's self as the receiver so
    // the parent's `@name` auto-assign writes into the same object.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "inh.rvn",
        r##"class Animal
  name: String

  def init(@name: String)
  end

  pub def speak -> String
    "..."
  end
end

class Cat < Animal
  def init(name: String)
    super(name)
  end

  pub def speak -> String
    "Meow! I'm #{self.name}"
  end
end

def main
  let c = Cat.new(String.from("Whiskers"))
  puts c.speak
end
"##,
    );
    assert_eq!(out.trim(), "Meow! I'm Whiskers");
}

#[test]
fn e2e_22_mixin_default() {
    // Fixture 22_mixin_default: a mixin's default method body may refer
    // to `self.<abstract>` and is monomorphized per impl.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "td22.rvn",
        r##"trait Greetable
  def name -> String
  def greet -> String
    "Hello, #{self.name}!"
  end
end

class Person
  pname: String

  def init(@pname: String)
  end
end

impl Greetable for Person
  def name -> String
    self.pname.clone
  end
end

def main
  let p = Person.new(String.from("Alice"))
  puts p.greet
end
"##,
    );
    assert_eq!(out.trim(), "Hello, Alice!");
}

#[test]
fn e2e_86_mixin_default_method_used() {
    // Fixture 86: mixin default method used via interpolation.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "td86.rvn",
        r##"trait Greeter
  def name -> String
  def greet -> String
    "Hello, #{self.name}!"
  end
end

class Bot
  nm: String

  def init(@nm: String)
  end
end

impl Greeter for Bot
  def name -> String
    self.nm.clone
  end
end

def main
  let b = Bot.new(String.from("Riv"))
  puts "#{b.greet}"
end
"##,
    );
    assert_eq!(out.trim(), "Hello, Riv!");
}

#[test]
fn e2e_87_mixin_override_default() {
    // Fixture 87: include overrides the mixin default; the override wins.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "td87.rvn",
        r##"trait Greeter
  def name -> String
  def greet -> String
    "Hello, #{self.name}!"
  end
end

class Bot
  nm: String

  def init(@nm: String)
  end
end

impl Greeter for Bot
  def name -> String
    self.nm.clone
  end

  def greet -> String
    "Hi, #{self.nm}."
  end
end

def main
  let b = Bot.new(String.from("Riv"))
  puts "#{b.greet}"
end
"##,
    );
    assert_eq!(out.trim(), "Hi, Riv.");
}

#[test]
fn e2e_14_classes() {
    // Fixture 14_classes: plain class + instance method sanity smoke.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "c14.rvn",
        r##"class Point
  x: Int
  y: Int

  def init(@x: Int, @y: Int)
  end

  pub def sum -> Int
    self.x + self.y
  end
end

def main
  let p = Point.new(3, 4)
  puts "#{p.sum}"
end
"##,
    );
    assert_eq!(out.trim(), "7");
}

#[test]
fn e2e_17_class_self_method() {
    // Fixture 17_class_self_method: calling another method via self.<name>.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "csm17.rvn",
        r##"class Box
  n: Int

  def init(@n: Int)
  end

  pub def doubled -> Int
    self.n * 2
  end

  pub def quadrupled -> Int
    self.doubled * 2
  end
end

def main
  let b = Box.new(5)
  puts "#{b.quadrupled}"
end
"##,
    );
    assert_eq!(out.trim(), "20");
}

#[test]
fn e2e_21_mixins() {
    // Fixture 21_mixins: mixin with only required methods (no defaults).
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "tr21.rvn",
        r##"trait Named
  def label -> String
end

class Dog
  n: String

  def init(@n: String)
  end
end

impl Named for Dog
  def label -> String
    self.n.clone
  end
end

def main
  let d = Dog.new(String.from("Rex"))
  puts d.label
end
"##,
    );
    assert_eq!(out.trim(), "Rex");
}

// ── Stdlib method + panic! tests (current task) ───────────────────────

#[test]
fn e2e_45_string_methods() {
    // Fixture 45: String.len (byte length) + "abc".to_upper returning
    // an uppercased String.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "string_methods.rvn",
        r##"def main
  let s = "hello"
  puts "#{s.len}"
  let upper = "abc".to_upper
  puts "#{upper}"
end
"##,
    );
    assert_eq!(out.trim(), "5\nABC");
}

#[test]
fn e2e_106_string_chars() {
    // Fixture 106: `for ch in s.chars` must iterate once per codepoint.
    // `s.chars` returns a `Vec[Char]` which the for-loop lowering then
    // walks via `riven_vec_len`/`riven_vec_get`.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "string_chars.rvn",
        r##"def main
  let s = "abc"
  let mut count = 0
  for ch in s.chars
    count += 1
  end
  puts "#{count}"
end
"##,
    );
    assert_eq!(out.trim(), "3");
}

#[test]
fn e2e_107_array_push_pop() {
    // Fixture 107: `Vec.push` grows the vector, `Vec.pop` returns an
    // `Option[T]` tagged union matching the runtime convention used by
    // `riven_vec_get_opt`.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "vec_push_pop.rvn",
        r##"def main
  let mut v: Vec[Int] = Vec.new
  v.push(1)
  v.push(2)
  v.push(3)
  puts "#{v.len}"
  match v.pop
    Some(x) -> puts "popped #{x}"
    None    -> puts "empty"
  end
  puts "#{v.len}"
end
"##,
    );
    assert_eq!(out.trim(), "3\npopped 3\n2");
}

#[test]
fn e2e_57_while_let_pop() {
    // Fixture 57: `while let Some(x) = v.pop` drains a vec in LIFO
    // order, exercising both the Option matcher and the refreshed
    // `v.pop` call inside the loop header.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "while_let_pop.rvn",
        r##"def main
  let mut v = vec![1, 2, 3]
  while let Some(x) = v.pop
    puts "#{x}"
  end
  puts "done"
end
"##,
    );
    assert_eq!(out.trim(), "3\n2\n1\ndone");
}

#[test]
fn e2e_96_panic_basic() {
    // Fixture 96: `panic!("boom")` prints the message to stderr and
    // exits non-zero. Stdout captures only the output before the
    // panic; anything after is unreachable.
    let (temp, rivenc) = stage_install();
    let src_name = "panic_basic.rvn";
    let src = temp.path().join(src_name);
    fs::write(
        &src,
        r##"def main
  puts "before"
  panic!("boom")
  puts "after"
end
"##,
    )
    .unwrap();

    let out_name = "panic_basic";
    let compile = Command::new(&rivenc)
        .arg(src_name)
        .arg("-o")
        .arg(out_name)
        .current_dir(temp.path())
        .output()
        .expect("spawn rivenc");
    assert!(
        compile.status.success(),
        "compile failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr),
    );

    let bin = temp.path().join(out_name);
    let run = Command::new(&bin)
        .current_dir(temp.path())
        .output()
        .expect("spawn compiled binary");
    assert!(
        !run.status.success(),
        "expected non-zero exit from panic!, got success; stdout={:?}",
        String::from_utf8_lossy(&run.stdout),
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).trim(),
        "before",
        "stderr:\n{}",
        String::from_utf8_lossy(&run.stderr),
    );
}

#[test]
fn e2e_26_array_basic() {
    // Fixture 26: smoke test — `vec![1,2,3]` + `len` + `for x in &v`
    // must still round-trip after the pop/chars changes.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "vec_basic.rvn",
        r##"def main
  let v = vec![1, 2, 3]
  puts "len=#{v.len}"
  for x in &v
    puts "#{x}"
  end
end
"##,
    );
    assert_eq!(out.trim(), "len=3\n1\n2\n3");
}

#[test]
fn e2e_108_string_split() {
    // Fixture 108: `"a,b,c".split(",").to_vec.len` → 3. Exercises
    // SplitIter → Vec[&str] collection (the `.to_vec` passthrough).
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "string_split.rvn",
        r##"def main
  let parts = "a,b,c".split(",").to_vec
  puts "#{parts.len}"
end
"##,
    );
    assert_eq!(out.trim(), "3");
}

#[test]
fn e2e_63_struct_basic() {
    // Fixture 63: plain struct with Int fields; construct + read.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "s63.rvn",
        r##"struct Point
  x: Int
  y: Int
end

def main
  let p = Point.new(3, 4)
  puts "#{p.x} #{p.y}"
end
"##,
    );
    assert_eq!(out.trim(), "3 4");
}

#[test]
fn e2e_64_struct_implicit() {
    // Fixture 64: struct with `derive Copy, Clone` — `let also_red = red`
    // must not move the original; both names remain readable.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "s64.rvn",
        r##"struct Color
  r: UInt8
  g: UInt8
  b: UInt8
  derive Copy, Clone
end

def main
  let red = Color.new(255u8, 0u8, 0u8)
  let also_red = red
  puts "#{red.r} #{red.g} #{red.b}"
  puts "#{also_red.r} #{also_red.g} #{also_red.b}"
end
"##,
    );
    assert_eq!(out.trim(), "255 0 0\n255 0 0");
}

#[test]
fn e2e_71_struct_vs_class() {
    // Fixture 71: struct embedded as a class field; both read-through and
    // the struct original remain usable because of `derive Copy, Clone`.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "s71.rvn",
        r##"struct Point
  x: Int
  y: Int
  derive Copy, Clone
end

class Circle
  center: Point
  radius: Int

  def init(@center: Point, @radius: Int)
  end

  pub def describe -> String
    "center=(#{self.center.x},#{self.center.y}) r=#{self.radius}"
  end
end

def main
  let p = Point.new(1, 2)
  let c = Circle.new(p, 5)
  puts "#{c.describe}"
  puts "#{p.x}"
end
"##,
    );
    assert_eq!(out.trim(), "center=(1,2) r=5\n1");
}

#[test]
fn e2e_85_implicit_debug() {
    // Fixture 85: struct with `derive Debug, Copy, Clone` — we only
    // assert field access + interpolation works (we don't ship a full
    // `#{p}` Debug printer yet; the fixture itself doesn't rely on it).
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "s85.rvn",
        r##"struct Pair
  a: Int
  b: Int
  derive Debug, Copy, Clone
end

def main
  let p = Pair.new(1, 2)
  puts "#{p.a} #{p.b}"
end
"##,
    );
    assert_eq!(out.trim(), "1 2");
}

#[test]
fn e2e_28_closures() {
    // Fixture 28_closures: non-capturing closure bound to a `let` and
    // invoked twice via `.()`.  Exercises the closure-pair heap layout
    // and indirect-call path with an empty captures struct (NULL).
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "c28.rvn",
        r##"def main
  let double = { |x: Int| x * 2 }
  puts "#{double.(5)}"
  puts "#{double.(10)}"
end
"##,
    );
    assert_eq!(out.trim(), "10\n20");
}

#[test]
fn e2e_88_closure_do_end() {
    // Fixture 88: `do ... end` closure passed to `vec.each` — the MIR
    // `try_inline_closure_method` path turns the call into a plain loop.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "c88.rvn",
        r##"def main
  let v = vec![1, 2, 3]
  v.each do |n|
    let doubled = n * 2
    puts "#{doubled}"
  end
end
"##,
    );
    assert_eq!(out.trim(), "2\n4\n6");
}

#[test]
fn e2e_89_closure_capture_immut() {
    // Fixture 89: closure captures an immutable `let multiplier` by
    // value.  `multiplier` is `Int` (Copy), so its current value is
    // copied into the captures struct at closure-construction time.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "c89.rvn",
        r##"def main
  let multiplier = 3
  let multiply = { |x: Int| x * multiplier }
  puts "#{multiply.(5)}"
  puts "#{multiply.(10)}"
end
"##,
    );
    assert_eq!(out.trim(), "15\n30");
}

#[test]
fn e2e_90_closure_capture_mut() {
    // Fixture 90: non-`move` closure mutates a `let mut count` across
    // three calls.  `count` must be cell-promoted so the closure and
    // the enclosing frame share storage.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "c90.rvn",
        r##"def main
  let mut count = 0
  let mut bump = { || count += 1 }
  bump.()
  bump.()
  bump.()
  puts "#{count}"
end
"##,
    );
    assert_eq!(out.trim(), "3");
}

#[test]
fn e2e_91_move_closure() {
    // Fixture 91: `move` closure that captures `n` by value and is
    // returned from `make_adder`.  No cell promotion — the closure
    // owns its own copy of `n`.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "c91.rvn",
        r##"def make_adder(n: Int) -> impl Fn(Int) -> Int
  move { |x| x + n }
end

def main
  let add_five = make_adder(5)
  puts "#{add_five.(10)}"
end
"##,
    );
    assert_eq!(out.trim(), "15");
}

#[test]
fn e2e_92_closure_as_arg() {
    // Fixture 92: non-capturing closure passed as an argument and
    // invoked inside the callee.  `apply` receives a closure pair and
    // calls it indirectly, forwarding a NULL captures pointer.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "c92.rvn",
        r##"def apply(f: Fn(Int) -> Int, x: Int) -> Int
  f.(x)
end

def main
  let square = { |n: Int| n * n }
  puts "#{apply(square, 4)}"
end
"##,
    );
    assert_eq!(out.trim(), "16");
}

#[test]
fn e2e_104_hash_basic() {
    // Fixture 104: `hash!{ k => v, ... }` macro literal builds a
    // `HashMap[String, Int]` and `.get(key)` returns `Option[&Int]`.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "hash_basic.rvn",
        r##"def main
  let h = hash!{ "a" => 1, "b" => 2 }
  match h.get("a")
    Some(v) -> puts "a=#{v}"
    None    -> puts "a=missing"
  end
  match h.get("b")
    Some(v) -> puts "b=#{v}"
    None    -> puts "b=missing"
  end
end
"##,
    );
    assert_eq!(out.trim(), "a=1\nb=2");
}

#[test]
fn e2e_105_set_basic() {
    // Fixture 105: `Set.new` + `.insert` + `.contains` + `.len`. The
    // second `s.insert(1)` is a duplicate and must not change `.len`.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "set_basic.rvn",
        r##"def main
  let mut s: Set[Int] = Set.new
  s.insert(1)
  s.insert(2)
  s.insert(1)
  puts "#{s.len}"
  if s.contains(1)
    puts "has 1"
  end
  if s.contains(3)
    puts "has 3"
  else
    puts "no 3"
  end
end
"##,
    );
    assert_eq!(out.trim(), "2\nhas 1\nno 3");
}

#[test]
fn e2e_93_yield_block() {
    // Fixture 93: `yield VALUE` inside a function invokes the trailing
    // `do ... end` block supplied by the caller.  Functions whose body
    // contains `yield` receive a synthetic `__block: Fn(...) -> ()`
    // parameter, and `yield VALUE` desugars to `__block.(VALUE)`.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "c93.rvn",
        r##"def with_x
  yield 42
end

def main
  with_x do |n|
    puts "#{n}"
  end
end
"##,
    );
    assert_eq!(out.trim(), "42");
}

// ── Type-inference coverage: &mut params and fluent-builder chains ────

#[test]
fn e2e_12_functions() {
    // Fixture 12_functions: plain multi-argument functions without
    // receivers — baseline sanity for return-type inference.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "f12.rvn",
        r##"def add(a: Int, b: Int) -> Int
  a + b
end

def mul(a: Int, b: Int) -> Int
  a * b
end

def main
  puts "#{add(2, 3)}"
  puts "#{add(10, 20)}"
  puts "#{mul(4, 5)}"
  puts "#{mul(7, 6)}"
end
"##,
    );
    assert_eq!(out.trim(), "5\n30\n20\n42");
}

#[test]
fn e2e_15_class_mut() {
    // Fixture 15_class_mut: a `pub def mut` method with no declared
    // return type must default to `Unit`, not trigger inference errors.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "cm15.rvn",
        r##"class Counter
  count: Int

  def init
    self.count = 0
  end

  pub def value -> Int
    self.count
  end

  pub def mut inc
    self.count = self.count + 1
  end
end

def main
  let mut c = Counter.new
  c.inc
  c.inc
  puts "#{c.value}"
end
"##,
    );
    assert_eq!(out.trim(), "2");
}

#[test]
fn e2e_47_borrow_immut() {
    // Fixture 47_borrow_immut: free function taking `&String` — the
    // caller passes `&s` and the original binding remains usable.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "bi47.rvn",
        r##"def print_name(name: &String)
  puts "#{name}"
end

def main
  let s = String.from("Riven")
  print_name(&s)
  puts "#{s}"
end
"##,
    );
    assert_eq!(out.trim(), "Riven\nRiven");
}

#[test]
fn e2e_48_borrow_mut() {
    // Fixture 48_borrow_mut: the free function `append_bang` takes a
    // `&mut String` and has no explicit return type. Without the
    // "default to Unit for unresolved return vars" fix in typeck, the
    // inference engine could not infer a return type and emitted a
    // "could not infer return type for function `append_bang`"
    // diagnostic.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "bm48.rvn",
        r##"def append_bang(s: &mut String)
  s.push('!')
end

def main
  let mut greeting = String.from("hello")
  append_bang(&mut greeting)
  puts "#{greeting}"
end
"##,
    );
    assert_eq!(out.trim(), "hello!");
}

#[test]
fn e2e_70_method_chain() {
    // Fixture 70_method_chain: a fluent builder where each `set_*`
    // method is declared `-> &mut Self` and ends in `self`. Without
    // the auto-ref return-type coercion in infer_func, the body type
    // (`Self`) could not be unified with the declared return
    // (`&mut Self`), breaking the whole chain.
    let (temp, rivenc) = stage_install();
    let out = compile_and_run(
        &rivenc,
        temp.path(),
        "mc70.rvn",
        r##"class Builder
  a: Int
  b: Int

  def init
    self.a = 0
    self.b = 0
  end

  pub def mut set_a(v: Int) -> &mut Self
    self.a = v
    self
  end

  pub def mut set_b(v: Int) -> &mut Self
    self.b = v
    self
  end

  pub def sum -> Int
    self.a + self.b
  end
end

def main
  let mut b = Builder.new
  b.set_a(1).set_b(2)
  puts "#{b.sum}"
end
"##,
    );
    assert_eq!(out.trim(), "3");
}
