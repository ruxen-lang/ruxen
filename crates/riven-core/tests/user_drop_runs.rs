//! Verifies that a class with a user-defined `impl Drop / def drop`
//! actually executes its `drop` body when an instance goes out of scope.
//!
//! Regression: prior to the fix, `MirInst::Drop` was a no-op in both
//! backends, so the user's `def drop` was never called.

use riven_core::codegen;
use riven_core::lexer::Lexer;
use riven_core::mir::lower::Lowerer;
use riven_core::parser::Parser;
use riven_core::typeck;
use std::process::Command;

const SOURCE: &str = r#"
class Holder
  tag: Int

  def init(t: Int)
    self.tag = t
  end

  include Drop

  def drop
    puts "DROP_RAN_tag=#{self.tag}"
  end
end

def main
  let h = Holder.new(7)
  puts "before"
end
"#;

fn workspace_root() -> std::path::PathBuf {
    let crate_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

#[test]
fn user_drop_runs_at_scope_exit() {
    let root = workspace_root();
    let tmp_dir = root.join("tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let bin_path = tmp_dir.join("user_drop_runs.bin");

    let mut lexer = Lexer::new(SOURCE);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == riven_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "typecheck errors: {:?}", errors);

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .expect("MIR lowering");

    codegen::compile(&mir, bin_path.to_str().unwrap()).expect("codegen");

    let output = Command::new(&bin_path).output().expect("run binary");
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
