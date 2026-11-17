//! Negative typeck pin for `BufReader` / `BufWriter`: both are modules, so
//! `BufReader.new(..)` is E0714 pointing to `BufReader.File` /
//! `BufReader.Tcp`. The runtime behaviour tests live in
//! `library/std/bufio/tests`, except the `into_inner` panic path, which needs
//! the process exit status.
//!
//! Every Ruxen snippet lives in `tests/fixtures/ruxen/` — no inline
//! `r#"..."#` per the cleanup convention (see commit 70ad1fb).

use ruxen_core::diagnostics::{Diagnostic, DiagnosticLevel};
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

fn rx(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruxen")
        .join(format!("{name}.rx"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn typecheck_diagnostics(source: &str) -> Vec<Diagnostic> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    typeck::type_check(&program).diagnostics
}

/// **Negative E0714**: `BufReader.new(file)` is a typeck error naming the
/// module's classes, not a silently dropped call.
#[test]
fn bufreader_module_new_emits_e0714() {
    let source = rx("bufreader_module_new_emits_e0714");
    let diags = typecheck_diagnostics(&source);
    let errs: Vec<&Diagnostic> = diags
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    let e0714 = errs
        .iter()
        .find(|d| d.code.as_deref() == Some("E0714"))
        .unwrap_or_else(|| panic!("expected E0714; got: {errs:#?}"));
    assert!(
        e0714.message.contains("`BufReader.File.new`")
            && e0714.message.contains("`BufReader.Tcp.new`"),
        "E0714 must point to the module's classes; got: {}",
        e0714.message
    );
}

/// `BufWriter#into_inner` panics (exit 101, a message naming the failed
/// flush) when flushing the buffer fails, here because the inner File is
/// open read-only; nothing after it runs.
#[test]
fn bufwriter_into_inner_panics_when_its_flush_fails() {
    let source = rx("bufwriter_into_inner_flush_failure_panics");
    let bin = crate::common::compile_program(&source, "bufwriter_into_inner_flush_failure");
    let dir = bin.with_extension("dir");
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    let output = std::process::Command::new(&bin)
        .current_dir(&dir)
        .output()
        .expect("run");
    let _ = std::fs::remove_file(&bin);
    let _ = std::fs::remove_dir_all(&dir);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(101), "stderr: {stderr}");
    assert!(
        stderr.contains("BufWriter#into_inner: flush failed"),
        "stderr: {stderr}"
    );
    assert!(output.stdout.is_empty(), "nothing after the panic runs");
}
