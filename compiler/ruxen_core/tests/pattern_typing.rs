//! Pin tests for task #22 — preserve String typing on `String`-typed
//! parameters and on payloads bound by destructure patterns so that
//! `==` lowers to `ruxen_string_eq` (strcmp) rather than integer
//! pointer-eq.
//!
//! Background: `resolve/stdlib/mod.rs::register_builtins` inserts
//! `String` as `DefKind::TypeAlias { target: Ty::String }`. The
//! bootstrap merge anchors the user-side `class String`
//! (`library/std/string/src/lib.rx`) onto the same DefId so the FFI
//! method decls (`String.from`, `String.new`, …) hang off it. The
//! class-resolution pass in `resolve/items.rs::resolve_class` then
//! rewrites that DefId's `DefKind` from `TypeAlias` to `Class` —
//! after which `resolve_type_expr` returns
//! `Ty::Class { name: "String", .. }` for any annotated `s: String`
//! parameter. Inferred locals (`let x = String.from(…)`) still get
//! `Ty::String` from the inference rules; only the annotation path
//! produces the Class form.
//!
//! The codegen-side `is_string_mir_ty` helper used to only recognise
//! the canonical `Ty::String` / `Ty::Str` forms. The `Compare`
//! emitter therefore fell back to pointer-eq whenever BOTH compare
//! operands were annotated `String` parameters (one side was enough
//! when the other was an inferred `Ty::String` local). Byte-identical
//! heap Strings compared unequal silently — fatal for every URL
//! match, header lookup, and response comparison in real server code.
//!
//! The fix extends `is_string_mir_ty` in BOTH the cranelift and LLVM
//! backends to also recognise `Ty::Class { name: "String", .. }`. The
//! pin tests below exercise compile-and-run on the two surface shapes
//! that previously broke:
//!   * `def f(s: String, needle: String) ... s == needle ...` — both
//!     compare operands carry the Class form.
//!   * `match Result.Ok(s) -> if s == needle ...` where `needle` is
//!     a `String` parameter — destructured payload from a
//!     `Result[String, _]` plus a Class-form parameter.

use ruxen_core::codegen;
use ruxen_core::diagnostics::DiagnosticLevel;
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;
use std::process::Command;

fn rx(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruxen")
        .join(format!("{name}.rx"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn workspace_root() -> std::path::PathBuf {
    let crate_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

/// Compile `source` through the in-process pipeline and run the
/// resulting binary. Returns `(stdout, exit_ok)`.
fn compile_and_run(source: &str, basename: &str) -> (String, bool) {
    let root = workspace_root();
    let tmp_dir = root.join("tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let bin_path = tmp_dir.join(format!("{}-{}-{}.bin", basename, std::process::id(), ruxen_unique_id()));

    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "typecheck errors: {:?}", errors);

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .expect("MIR lowering");
    codegen::compile(&mir, bin_path.to_str().unwrap()).expect("codegen");

    let output = Command::new(&bin_path).output().expect("run binary");
    let _ = std::fs::remove_file(&bin_path);
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        output.status.success(),
    )
}

/// Two String-typed parameters compared via `==` must lower to
/// `ruxen_string_eq`. Before the fix, both operands were
/// `Ty::Class { name: "String" }` and the codegen helper missed the
/// match, falling back to pointer-eq.
#[test]
fn string_eq_between_two_string_params_uses_strcmp() {
    let (stdout, ok) = compile_and_run(
        &rx("string_eq_param_typing"),
        "task22_string_eq_param_typing",
    );
    assert!(ok, "binary did not exit cleanly. stdout: {}", stdout);
    assert_eq!(
        stdout.trim(),
        "match",
        "two byte-identical heap Strings compared unequal — Compare \
         fell back to pointer-eq on Ty::Class {{ name: \"String\" }} \
         operands. stdout: {:?}",
        stdout
    );
}

/// A String flowing out of `match Result.Ok(s) -> ...` compared
/// against a String parameter must lower to `ruxen_string_eq`. This
/// exercises the destructure-binding path AND the Class-form
/// parameter path together — the shape that motivated task #22.
#[test]
fn string_eq_after_result_ok_destructure_uses_strcmp() {
    let (stdout, ok) = compile_and_run(
        &rx("string_eq_after_result_destructure"),
        "task22_string_eq_after_result_destructure",
    );
    assert!(ok, "binary did not exit cleanly. stdout: {}", stdout);
    assert_eq!(
        stdout.trim(),
        "match",
        "String destructured from Result.Ok compared unequal to a \
         byte-identical String parameter — pointer-eq fallback. \
         stdout: {:?}",
        stdout
    );
}

fn ruxen_unique_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}
