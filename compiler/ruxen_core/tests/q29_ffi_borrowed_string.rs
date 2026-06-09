//! Q29 — a BORROWED `&String` (owned by the caller) passed into a `lib "C"`
//! FFI function forwards the correct data POINTER and a recoverable LENGTH.
//!
//! Status (2026-06-09 audit, feat/drop-elaboration): **NOT A BUG.** The old
//! ledger claim — "measure_text forwards a char count, not the string; a
//! borrowed `&String` passes the wrong pointer" — described the LEGACY
//! `measure_text_n_raw(n: Int)` char-count fallback, a workaround that existed
//! because `&String` FFI was distrusted. The real `&String` path is correct:
//!
//!   - Ruxen's `String` ABI is a bare NUL-terminated `char*` (no length header;
//!     `library/std/string/runtime/string.c` — `ruxen_string_from`,
//!     `ruxen_string_len` are all `(const char *)`). A `String` VALUE *is* the
//!     `char*`.
//!   - `MirInst::Ref` (the `&` of `&String`) is by-value in both backends
//!     (`codegen/cranelift/emit.rs` `MirInst::Ref`), so it forwards the `char*`
//!     unchanged. The C side recovers length via `strlen`.
//!
//! This pin exercises the boundary through `String` stdlib methods that take a
//! `needle: &String` and run pointer/length-sensitive byte ops on it
//! (`include?`→strstr, `find`→byte offset, `replace`, `starts_with`). A
//! regression to forwarding the wrong pointer or a char-count would corrupt
//! these results. Reads the same fixture the release-e2e harness runs
//! (`tests/release-e2e/cases/649_*`).

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

#[test]
fn borrowed_string_arg_forwards_pointer_and_length() {
    let (src, expected) = case("649_ffi_borrowed_string_arg.rx");
    let (stdout, stderr, ok) = compile_and_run(&src, "q29_649");
    assert!(ok, "non-zero exit; stderr: {stderr}");
    assert_eq!(stdout, expected, "stdout was {stdout:?}");
}
