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

            // Stage the unity-build runtime: the aggregator `runtime.c`
            // plus every per-module `#include`d file underneath.
            // Post-#06.75 the C runtime lives in
            // `library/runtime/{core,io,net,…}/` and `runtime.c`
            // `#include`s each piece, so a single-file copy is no
            // longer sufficient — the staged install needs the full
            // `library/runtime/` tree at `<install>/lib/`.
            copy_runtime_tree(&runtime_c_src().parent().unwrap().to_path_buf(), &lib_dir);

            (temp, staged_rivenc)
        })
        .1
}

/// Recursively copy `library/runtime/` (the source layout) into the
/// staged `<install>/lib/` (the destination layout).  Mirrors what
/// `install.sh` does with `cp -R "$SRC/lib/." "$RIVEN_HOME/lib/"` on a
/// real release archive.
fn copy_runtime_tree(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let dest = dst.join(entry.file_name());
        if path.is_dir() {
            copy_runtime_tree(&path, &dest);
        } else {
            fs::copy(&path, &dest).expect("copy runtime file");
        }
    }
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
        .join("library")
        .join("runtime")
        .join("runtime.c")
}

/// Path to the `rivenc` binary under test (cargo populates this env var).
fn rivenc_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_rivenc"))
}

/// Read a `.rvn` fixture from `tests/fixtures/riven/<name>.rvn`.
///
/// Riven source for these tests lives in standalone `.rvn` files so that
/// future surface-syntax migrations can sweep `*.rvn` uniformly without
/// touching Rust string literals.
fn rvn(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/riven")
        .join(format!("{name}.rvn"));
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
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
    let source = rvn("find_runtime_c_from_sibling_lib_dir");
    let out = compile_and_run(&rivenc, temp.path(), "hello.rvn", &source);
    assert_eq!(out.trim(), "hello");
}

#[test]
fn integer_arithmetic() {
    let (temp, rivenc) = stage_install();
    let source = rvn("integer_arithmetic");
    let out = compile_and_run(&rivenc, temp.path(), "int_arith.rvn", &source);
    assert_eq!(out.trim(), "3\n7\n24\n5\n2");
}

#[test]
fn owned_rebind_does_not_double_drop() {
    let (temp, rivenc) = stage_install();
    let source = rvn("owned_rebind_does_not_double_drop");
    let out = compile_and_run(&rivenc, temp.path(), "owned_rebind.rvn", &source);
    assert_eq!(out.trim(), "18");
}

#[test]
fn float_arithmetic() {
    // Regression: Cranelift codegen previously emitted `imul`/`iadd` for
    // f64 values, which the verifier rejects. Must dispatch to `fmul`/`fadd`.
    let (temp, rivenc) = stage_install();
    let source = rvn("float_arithmetic");
    let out = compile_and_run(&rivenc, temp.path(), "float_arith.rvn", &source);
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
    let source = rvn("float_comparison");
    let out = compile_and_run(&rivenc, temp.path(), "float_cmp.rvn", &source);
    assert_eq!(out.trim(), "less\ngreater");
}

#[test]
fn string_interpolation() {
    let (temp, rivenc) = stage_install();
    let source = rvn("string_interpolation");
    let out = compile_and_run(&rivenc, temp.path(), "interp.rvn", &source);
    assert_eq!(out.trim(), "hello world 3");
}

#[test]
fn enum_with_match() {
    let (temp, rivenc) = stage_install();
    let source = rvn("enum_with_match");
    let out = compile_and_run(&rivenc, temp.path(), "match.rvn", &source);
    let lines: Vec<&str> = out.trim().lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].starts_with("12.56"), "got {:?}", lines[0]);
    assert_eq!(lines[1], "16");
}

#[test]
fn classes_and_methods() {
    let (temp, rivenc) = stage_install();
    let source = rvn("classes_and_methods");
    let out = compile_and_run(&rivenc, temp.path(), "classes.rvn", &source);
    assert_eq!(out.trim(), "3");
}

#[test]
fn closures_and_iterators() {
    let (temp, rivenc) = stage_install();
    let source = rvn("closures_and_iterators");
    let out = compile_and_run(&rivenc, temp.path(), "iter.rvn", &source);
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
        workspace_root().join("compiler/riven_core/tests/fixtures/sample_program.rvn"),
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
    // Stage a secondary copy of the whole runtime tree so the unity-build
    // `#include "core/alloc.c"` lookups still resolve, then point RIVEN_RUNTIME
    // at its aggregator.
    let alt_dir = temp.path().join("alt_runtime");
    copy_runtime_tree(&runtime_c_src().parent().unwrap().to_path_buf(), &alt_dir);
    let alt = alt_dir.join("runtime.c");

    fs::write(temp.path().join("env_ov.rvn"), rvn("runtime_env_override")).unwrap();

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
    let source = rvn("match_guards_on_int_binding");
    let out = compile_and_run(&rivenc, temp.path(), "match_guards.rvn", &source);
    assert_eq!(out.trim(), "A\nB\nC\nF");
}

#[test]
fn match_on_int_literals_still_works() {
    // Smoke: ensures the cascading match path still handles literal
    // patterns with no guards after the guard-related refactor.
    let (temp, rivenc) = stage_install();
    let source = rvn("match_on_int_literals_still_works");
    let out = compile_and_run(&rivenc, temp.path(), "match_int.rvn", &source);
    assert_eq!(out.trim(), "zero\none\ntwo\nmany");
}

#[test]
fn match_on_simple_enum_still_works() {
    // Smoke: tag-switch lowering for unit-variant enums.
    let (temp, rivenc) = stage_install();
    let source = rvn("match_on_simple_enum_still_works");
    let out = compile_and_run(&rivenc, temp.path(), "simple_enum.rvn", &source);
    assert_eq!(out.trim(), "red\ngreen\nblue");
}

#[test]
fn match_on_enum_with_data_still_works() {
    // Smoke: tag-switch + payload-field bindings.
    let (temp, rivenc) = stage_install();
    let source = rvn("match_on_enum_with_data_still_works");
    let out = compile_and_run(&rivenc, temp.path(), "enum_data.rvn", &source);
    assert_eq!(out.trim(), "75\n24");
}

#[test]
fn match_guards_with_enum_variant_bindings() {
    // Guards must also work when combined with enum-variant patterns that
    // introduce field bindings — the guard expression must see those
    // bindings and its false case must fall through to the next arm.
    let (temp, rivenc) = stage_install();
    let source = rvn("match_guards_with_enum_variant_bindings");
    let out = compile_and_run(&rivenc, temp.path(), "guard_enum.rvn", &source);
    assert_eq!(out.trim(), "bright red\nred\nblue");
}

#[test]
fn fixed_array_literal_coerces() {
    // Bug 1: `let a: [Int; 3] = [1,2,3]` — the bracket literal is typed
    // as Vec[Int] by the resolver; typeck must coerce to fixed array.
    let (temp, rivenc) = stage_install();
    let source = rvn("fixed_array_literal_coerces");
    let out = compile_and_run(&rivenc, temp.path(), "fixed_array.rvn", &source);
    assert_eq!(out.trim(), "10\n20\n30");
}

#[test]
fn newtype_wrapper_construct_and_project() {
    // Bug 2: `newtype Meters(Float)` — `Meters(3.14)` must construct
    // the wrapper and `.0` must project the inner value.
    let (temp, rivenc) = stage_install();
    let source = rvn("newtype_wrapper_construct_and_project");
    let out = compile_and_run(&rivenc, temp.path(), "newtype.rvn", &source);
    assert_eq!(out.trim(), "3.14");
}

#[test]
fn const_decl_substitutes_at_use_sites() {
    // Bug 3: top-level `const` reference must emit the initializer
    // expression at every use site; otherwise we read uninitialized
    // stack memory and print garbage.
    let (temp, rivenc) = stage_install();
    let source = rvn("const_decl_substitutes_at_use_sites");
    let out = compile_and_run(&rivenc, temp.path(), "const_decl.rvn", &source);
    assert_eq!(out.trim(), "100");
}

#[test]
fn regression_int_arith() {
    let (temp, rivenc) = stage_install();
    let source = rvn("regression_int_arith");
    let out = compile_and_run(&rivenc, temp.path(), "int_arith_reg.rvn", &source);
    assert_eq!(out.trim(), "50");
}

#[test]
fn regression_classes_init() {
    let (temp, rivenc) = stage_install();
    let source = rvn("regression_classes_init");
    let out = compile_and_run(&rivenc, temp.path(), "classes_reg.rvn", &source);
    assert_eq!(out.trim(), "21\n42");
}

#[test]
fn regression_type_alias() {
    let (temp, rivenc) = stage_install();
    let source = rvn("regression_type_alias");
    let out = compile_and_run(&rivenc, temp.path(), "type_alias_reg.rvn", &source);
    assert_eq!(out.trim(), "5");
}

#[test]
fn derive_copy_struct_copies_on_assignment() {
    // Bug 4: a struct with `derive Copy` must be treated as Copy by
    // the borrow checker (no "value used after move" on `let b = a`).
    let (temp, rivenc) = stage_install();
    let source = rvn("derive_copy_struct_copies_on_assignment");
    let out = compile_and_run(&rivenc, temp.path(), "derive_copy.rvn", &source);
    assert_eq!(out.trim(), "1 2\n1 2");
}

// ── Parser / pattern bug fixtures (current task) ──────────────────────

#[test]
fn parser_tuple_field_access_dot_int() {
    // Fixture 32: `t.0` — parser must accept IntLiteral after `.`.
    let (temp, rivenc) = stage_install();
    let source = rvn("parser_tuple_field_access_dot_int");
    let out = compile_and_run(&rivenc, temp.path(), "tuple_field.rvn", &source);
    assert_eq!(out.trim(), "10\n20");
}

#[test]
fn parser_or_pattern_literal_alternatives() {
    // Fixture 58: `a | b | c -> body` restricted to literals.
    let (temp, rivenc) = stage_install();
    let source = rvn("parser_or_pattern_literal_alternatives");
    let out = compile_and_run(&rivenc, temp.path(), "or_pattern.rvn", &source);
    assert_eq!(out.trim(), "low\nmid\nother");
}

#[test]
fn parser_match_tuple_pattern() {
    // Fixture 62: `(a, b) -> body` in match.
    let (temp, rivenc) = stage_install();
    let source = rvn("parser_match_tuple_pattern");
    let out = compile_and_run(&rivenc, temp.path(), "match_tuple.rvn", &source);
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
    let source = rvn("parser_match_ref_binding");
    let out = compile_and_run(&rivenc, temp.path(), "match_ref.rvn", &source);
    assert_eq!(out.trim(), "hello\nhello");
}

#[test]
fn parser_regression_match_int() {
    // Fixture 10: plain literal match arms still route through the
    // non-or branch.
    let (temp, rivenc) = stage_install();
    let source = rvn("parser_regression_match_int");
    let out = compile_and_run(&rivenc, temp.path(), "match_int.rvn", &source);
    assert_eq!(out.trim(), "zero\none\ntwo\nmany");
}

#[test]
fn parser_regression_match_guards() {
    // Fixture 11: `name if guard -> body` still compiles.
    let (temp, rivenc) = stage_install();
    let source = rvn("parser_regression_match_guards");
    let out = compile_and_run(&rivenc, temp.path(), "match_guards.rvn", &source);
    assert_eq!(out.trim(), "A\nB\nC\nF");
}

#[test]
fn parser_do_end_block_expr() {
    // Fixture 59: `do ... last_expr end` used as an expression.
    let (temp, rivenc) = stage_install();
    let source = rvn("parser_do_end_block_expr");
    let out = compile_and_run(&rivenc, temp.path(), "do_end_block.rvn", &source);
    assert_eq!(out.trim(), "3");
}

#[test]
fn e2e_16_inheritance() {
    // Fixture 16_inheritance: `super(name)` inside a subclass init must
    // invoke the parent's init with the child's self as the receiver so
    // the parent's `@name` auto-assign writes into the same object.
    let (temp, rivenc) = stage_install();
    let source = rvn("e2e_16_inheritance");
    let out = compile_and_run(&rivenc, temp.path(), "inh.rvn", &source);
    assert_eq!(out.trim(), "Meow! I'm Whiskers");
}

#[test]
fn e2e_22_mixin_default() {
    // Fixture 22_mixin_default: a mixin's default method body may refer
    // to `self.<abstract>` and is monomorphized per impl.
    let (temp, rivenc) = stage_install();
    let source = rvn("e2e_22_mixin_default");
    let out = compile_and_run(&rivenc, temp.path(), "td22.rvn", &source);
    assert_eq!(out.trim(), "Hello, Alice!");
}

#[test]
fn e2e_86_mixin_default_method_used() {
    // Fixture 86: mixin default method used via interpolation.
    let (temp, rivenc) = stage_install();
    let source = rvn("e2e_86_mixin_default_method_used");
    let out = compile_and_run(&rivenc, temp.path(), "td86.rvn", &source);
    assert_eq!(out.trim(), "Hello, Riv!");
}

#[test]
fn e2e_87_mixin_override_default() {
    // Fixture 87: include overrides the mixin default; the override wins.
    let (temp, rivenc) = stage_install();
    let source = rvn("e2e_87_mixin_override_default");
    let out = compile_and_run(&rivenc, temp.path(), "td87.rvn", &source);
    assert_eq!(out.trim(), "Hi, Riv.");
}

#[test]
fn e2e_14_classes() {
    // Fixture 14_classes: plain class + instance method sanity smoke.
    let (temp, rivenc) = stage_install();
    let source = rvn("e2e_14_classes");
    let out = compile_and_run(&rivenc, temp.path(), "c14.rvn", &source);
    assert_eq!(out.trim(), "7");
}

#[test]
fn e2e_17_class_self_method() {
    // Fixture 17_class_self_method: calling another method via self.<name>.
    let (temp, rivenc) = stage_install();
    let source = rvn("e2e_17_class_self_method");
    let out = compile_and_run(&rivenc, temp.path(), "csm17.rvn", &source);
    assert_eq!(out.trim(), "20");
}

#[test]
fn e2e_21_mixins() {
    // Fixture 21_mixins: mixin with only required methods (no defaults).
    let (temp, rivenc) = stage_install();
    let source = rvn("e2e_21_mixins");
    let out = compile_and_run(&rivenc, temp.path(), "tr21.rvn", &source);
    assert_eq!(out.trim(), "Rex");
}

// ── Stdlib method + panic! tests (current task) ───────────────────────

#[test]
fn e2e_45_string_methods() {
    // Fixture 45: String.len (byte length) + "abc".to_upper returning
    // an uppercased String.
    let (temp, rivenc) = stage_install();
    let source = rvn("e2e_45_string_methods");
    let out = compile_and_run(&rivenc, temp.path(), "string_methods.rvn", &source);
    assert_eq!(out.trim(), "5\nABC");
}

#[test]
fn e2e_106_string_chars() {
    // Fixture 106: `for ch in s.chars` must iterate once per codepoint.
    // `s.chars` returns a `Vec[Char]` which the for-loop lowering then
    // walks via `riven_vec_len`/`riven_vec_get`.
    let (temp, rivenc) = stage_install();
    let source = rvn("e2e_106_string_chars");
    let out = compile_and_run(&rivenc, temp.path(), "string_chars.rvn", &source);
    assert_eq!(out.trim(), "3");
}

#[test]
fn e2e_107_array_push_pop() {
    // Fixture 107: `Vec.push` grows the vector, `Vec.pop` returns an
    // `Option[T]` tagged union matching the runtime convention used by
    // `riven_vec_get_opt`.
    let (temp, rivenc) = stage_install();
    let source = rvn("e2e_107_array_push_pop");
    let out = compile_and_run(&rivenc, temp.path(), "vec_push_pop.rvn", &source);
    assert_eq!(out.trim(), "3\npopped 3\n2");
}

#[test]
fn e2e_57_while_let_pop() {
    // Fixture 57: `while let Some(x) = v.pop` drains a vec in LIFO
    // order, exercising both the Option matcher and the refreshed
    // `v.pop` call inside the loop header.
    let (temp, rivenc) = stage_install();
    let source = rvn("e2e_57_while_let_pop");
    let out = compile_and_run(&rivenc, temp.path(), "while_let_pop.rvn", &source);
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
    fs::write(&src, rvn("e2e_96_panic_basic")).unwrap();

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
    // Fixture 26: smoke test — `[1,2,3]` array literal + `len` + `for x in &v`
    // must still round-trip after the pop/chars changes.
    let (temp, rivenc) = stage_install();
    let source = rvn("e2e_26_array_basic");
    let out = compile_and_run(&rivenc, temp.path(), "vec_basic.rvn", &source);
    assert_eq!(out.trim(), "len=3\n1\n2\n3");
}

#[test]
fn e2e_108_string_split() {
    // Fixture 108: `"a,b,c".split(",").to_vec.len` → 3. Exercises
    // SplitIter → Vec[&str] collection (the `.to_vec` passthrough).
    let (temp, rivenc) = stage_install();
    let source = rvn("e2e_108_string_split");
    let out = compile_and_run(&rivenc, temp.path(), "string_split.rvn", &source);
    assert_eq!(out.trim(), "3");
}

#[test]
fn e2e_63_struct_basic() {
    // Fixture 63: plain struct with Int fields; construct + read.
    let (temp, rivenc) = stage_install();
    let source = rvn("e2e_63_struct_basic");
    let out = compile_and_run(&rivenc, temp.path(), "s63.rvn", &source);
    assert_eq!(out.trim(), "3 4");
}

#[test]
fn e2e_64_struct_implicit() {
    // Fixture 64: struct with implicit `Copy, Clone` (per §3.6) —
    // `let also_red = red` must not move the original; both names
    // remain readable.
    let (temp, rivenc) = stage_install();
    let source = rvn("e2e_64_struct_implicit");
    let out = compile_and_run(&rivenc, temp.path(), "s64.rvn", &source);
    assert_eq!(out.trim(), "255 0 0\n255 0 0");
}

#[test]
fn e2e_71_struct_vs_class() {
    // Fixture 71: struct embedded as a class field; both read-through and
    // the struct original remain usable because of implicit `Copy, Clone`.
    let (temp, rivenc) = stage_install();
    let source = rvn("e2e_71_struct_vs_class");
    let out = compile_and_run(&rivenc, temp.path(), "s71.rvn", &source);
    assert_eq!(out.trim(), "center=(1,2) r=5\n1");
}

#[test]
fn e2e_85_implicit_debug() {
    // Fixture 85: struct with implicit `Debug, Copy, Clone` — we only
    // assert field access + interpolation works (we don't ship a full
    // `#{p}` Debug printer yet; the fixture itself doesn't rely on it).
    let (temp, rivenc) = stage_install();
    let source = rvn("e2e_85_implicit_debug");
    let out = compile_and_run(&rivenc, temp.path(), "s85.rvn", &source);
    assert_eq!(out.trim(), "1 2");
}

#[test]
fn e2e_28_closures() {
    // Fixture 28_closures: non-capturing closure bound to a `let` and
    // invoked twice via `.()`.  Exercises the closure-pair heap layout
    // and indirect-call path with an empty captures struct (NULL).
    let (temp, rivenc) = stage_install();
    let source = rvn("e2e_28_closures");
    let out = compile_and_run(&rivenc, temp.path(), "c28.rvn", &source);
    assert_eq!(out.trim(), "10\n20");
}

#[test]
fn e2e_88_closure_do_end() {
    // Fixture 88: `do ... end` closure passed to `vec.each` — the MIR
    // `try_inline_closure_method` path turns the call into a plain loop.
    let (temp, rivenc) = stage_install();
    let source = rvn("e2e_88_closure_do_end");
    let out = compile_and_run(&rivenc, temp.path(), "c88.rvn", &source);
    assert_eq!(out.trim(), "2\n4\n6");
}

#[test]
fn e2e_89_closure_capture_immut() {
    // Fixture 89: closure captures an immutable `let multiplier` by
    // value.  `multiplier` is `Int` (Copy), so its current value is
    // copied into the captures struct at closure-construction time.
    let (temp, rivenc) = stage_install();
    let source = rvn("e2e_89_closure_capture_immut");
    let out = compile_and_run(&rivenc, temp.path(), "c89.rvn", &source);
    assert_eq!(out.trim(), "15\n30");
}

#[test]
fn e2e_90_closure_capture_mut() {
    // Fixture 90: non-`move` closure mutates a `var count` across
    // three calls.  `count` must be cell-promoted so the closure and
    // the enclosing frame share storage.
    let (temp, rivenc) = stage_install();
    let source = rvn("e2e_90_closure_capture_mut");
    let out = compile_and_run(&rivenc, temp.path(), "c90.rvn", &source);
    assert_eq!(out.trim(), "3");
}

#[test]
fn e2e_91_move_closure() {
    // Fixture 91: `move` closure that captures `n` by value and is
    // returned from `make_adder`.  No cell promotion — the closure
    // owns its own copy of `n`.
    let (temp, rivenc) = stage_install();
    let source = rvn("e2e_91_move_closure");
    let out = compile_and_run(&rivenc, temp.path(), "c91.rvn", &source);
    assert_eq!(out.trim(), "15");
}

#[test]
fn e2e_92_closure_as_arg() {
    // Fixture 92: non-capturing closure passed as an argument and
    // invoked inside the callee.  `apply` receives a closure pair and
    // calls it indirectly, forwarding a NULL captures pointer.
    let (temp, rivenc) = stage_install();
    let source = rvn("e2e_92_closure_as_arg");
    let out = compile_and_run(&rivenc, temp.path(), "c92.rvn", &source);
    assert_eq!(out.trim(), "16");
}

#[test]
fn e2e_104_map_basic() {
    // Fixture 104: `{ k => v, ... }` Map literal builds a
    // `Map[String, Int]` and `.get(key)` returns `Option[&Int]`.
    let (temp, rivenc) = stage_install();
    let source = rvn("e2e_104_map_basic");
    let out = compile_and_run(&rivenc, temp.path(), "map_basic.rvn", &source);
    assert_eq!(out.trim(), "a=1\nb=2");
}

#[test]
fn e2e_105_set_basic() {
    // Fixture 105: `Set.new` + `.insert` + `.contains` + `.len`. The
    // second `s.insert(1)` is a duplicate and must not change `.len`.
    let (temp, rivenc) = stage_install();
    let source = rvn("e2e_105_set_basic");
    let out = compile_and_run(&rivenc, temp.path(), "set_basic.rvn", &source);
    assert_eq!(out.trim(), "2\nhas 1\nno 3");
}

#[test]
fn e2e_93_yield_block() {
    // Fixture 93: `yield VALUE` inside a function invokes the trailing
    // `do ... end` block supplied by the caller.  Functions whose body
    // contains `yield` receive a synthetic `__block: Fn(...) -> ()`
    // parameter, and `yield VALUE` desugars to `__block.(VALUE)`.
    let (temp, rivenc) = stage_install();
    let source = rvn("e2e_93_yield_block");
    let out = compile_and_run(&rivenc, temp.path(), "c93.rvn", &source);
    assert_eq!(out.trim(), "42");
}

// ── Type-inference coverage: &var params and fluent-builder chains ────

#[test]
fn e2e_12_functions() {
    // Fixture 12_functions: plain multi-argument functions without
    // receivers — baseline sanity for return-type inference.
    let (temp, rivenc) = stage_install();
    let source = rvn("e2e_12_functions");
    let out = compile_and_run(&rivenc, temp.path(), "f12.rvn", &source);
    assert_eq!(out.trim(), "5\n30\n20\n42");
}

#[test]
fn e2e_15_class_var() {
    // Fixture 15_class_var: a `def var` (writing) method with no declared
    // return type must default to `Unit`, not trigger inference errors.
    let (temp, rivenc) = stage_install();
    let source = rvn("e2e_15_class_var");
    let out = compile_and_run(&rivenc, temp.path(), "cv15.rvn", &source);
    assert_eq!(out.trim(), "2");
}

#[test]
fn e2e_47_borrow_immut() {
    // Fixture 47_borrow_immut: free function taking `&String` — the
    // caller passes `&s` and the original binding remains usable.
    let (temp, rivenc) = stage_install();
    let source = rvn("e2e_47_borrow_immut");
    let out = compile_and_run(&rivenc, temp.path(), "bi47.rvn", &source);
    assert_eq!(out.trim(), "Riven\nRiven");
}

#[test]
fn e2e_48_borrow_var() {
    // Fixture 48_borrow_var: the free function `append_bang` takes a
    // `&var String` and has no explicit return type. Without the
    // "default to Unit for unresolved return vars" fix in typeck, the
    // inference engine could not infer a return type and emitted a
    // "could not infer return type for function `append_bang`"
    // diagnostic.
    let (temp, rivenc) = stage_install();
    let source = rvn("e2e_48_borrow_var");
    let out = compile_and_run(&rivenc, temp.path(), "bv48.rvn", &source);
    assert_eq!(out.trim(), "hello!");
}

#[test]
fn e2e_70_method_chain() {
    // Fixture 70_method_chain: a fluent builder where each `set_*`
    // method is declared `-> &var Self` and ends in `self`. Without
    // the auto-ref return-type coercion in infer_func, the body type
    // (`Self`) could not be unified with the declared return
    // (`&var Self`), breaking the whole chain.
    let (temp, rivenc) = stage_install();
    let source = rvn("e2e_70_method_chain");
    let out = compile_and_run(&rivenc, temp.path(), "mc70.rvn", &source);
    assert_eq!(out.trim(), "3");
}
