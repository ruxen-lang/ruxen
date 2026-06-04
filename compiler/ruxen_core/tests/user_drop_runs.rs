//! Verifies that a class with a user-defined `impl Drop / def drop`
//! actually executes its `drop` body when an instance goes out of scope.
//!
//! Regression: prior to the fix, `MirInst::Drop` was a no-op in both
//! backends, so the user's `def drop` was never called.

use ruxen_core::codegen;
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

#[test]
fn user_drop_runs_at_scope_exit() {
    let root = workspace_root();
    let tmp_dir = root.join("tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let bin_path = tmp_dir.join(format!("user_drop_runs-{}-{}.bin", std::process::id(), ruxen_unique_id()));

    let source = rx("user_drop_runs_at_scope_exit");
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "typecheck errors: {:?}", errors);

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .expect("MIR lowering");

    codegen::compile(&mir, bin_path.to_str().unwrap()).expect("codegen");

    let output = Command::new(&bin_path).output().expect("run binary");
    let _ = std::fs::remove_file(&bin_path);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();

    assert!(
        output.status.success(),
        "binary exited non-zero. stdout=[{}] stderr=[{}]",
        stdout,
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("before"),
        "expected 'before' in stdout, got: [{}]",
        stdout
    );
    assert!(
        stdout.contains("DROP_RAN_tag=7"),
        "user-defined drop did not execute. stdout=[{}]",
        stdout
    );
    // Drop must run AFTER the last use of the value, so 'before' precedes the drop.
    let before_idx = stdout.find("before").unwrap();
    let drop_idx = stdout.find("DROP_RAN_tag=7").unwrap();
    assert!(
        before_idx < drop_idx,
        "drop ran before the last use. stdout=[{}]",
        stdout
    );
}

fn ruxen_unique_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}
