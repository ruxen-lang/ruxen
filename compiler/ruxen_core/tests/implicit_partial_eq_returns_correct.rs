//! Regression test: `derive PartialEq` on a struct must generate an `eq()`
//! method (and `==` codegen) that returns `true` for structurally equal
//! values and `false` otherwise — not pointer equality, not always-false.
//!
//! Surfaced during Tier-1 e2e probing (see CEO ruling 2026-04-24).

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

fn compile_and_run(source: &str, name: &str) -> (String, Option<i32>) {
    let root = workspace_root();
    let bin_path = root.join(format!(
        "tmp/{}-{}-{}.bin",
        name,
        std::process::id(),
        ruxen_unique_id()
    ));
    let _ = std::fs::create_dir_all(root.join("tmp"));

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
    assert!(errors.is_empty(), "typecheck errors: {:?}", errors);

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .expect("MIR lowering failed");
    codegen::compile(&mir, bin_path.to_str().unwrap()).expect("codegen failed");

    let output = Command::new(&bin_path).output().expect("run binary");
    let _ = std::fs::remove_file(&bin_path);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    (stdout, output.status.code())
}

#[test]
fn derive_partial_eq_compares_fields_struct() {
    let source = rx("derive_partial_eq_compares_fields_struct");
    let (stdout, exit) = compile_and_run(&source, "derive_partial_eq_basic");
    assert_eq!(exit, Some(0), "non-zero exit; stdout={}", stdout);
    assert_eq!(
        stdout, "ab=true\nac=false\n",
        "structurally equal P.new(1,2) instances should compare equal; \
         differing fields should compare unequal"
    );
}

/// Characterization (Phase 6 Task 2): pins the runtime behaviour of the
/// fold-shaped derived methods (`==`, `hash_code`, `.clone`) on a struct
/// with two integer fields and a `String` field — the exact arms the
/// shared `fold_struct_fields` driver folds (primitive Compare/Eq, the
/// FNV hash step, and the per-field clone+SetField). The extraction is
/// byte-identical MIR; this is the green-before-and-after backstop.
#[test]
fn derive_fold_methods_eq_hash_clone_behaviour() {
    let source = "\
struct Rec
  a: Int
  b: Int
  s: String
  include Hashable
end

def hash_it[T: Hashable](x: &T) -> Int
  x.hash_code
end

def main
  let p = Rec.new(1, 2, \"hi\")
  let q = Rec.new(1, 2, \"hi\")
  let r = Rec.new(1, 9, \"hi\")
  puts \"eq=#{p == q}\"
  puts \"neq=#{p == r}\"
  let c = p.clone
  puts \"clone_eq=#{p == c}\"
  puts \"hash_match=#{hash_it(&p) == hash_it(&q)}\"
end
";
    let (stdout, exit) = compile_and_run(source, "derive_fold_methods");
    assert_eq!(exit, Some(0), "non-zero exit; stdout={}", stdout);
    assert_eq!(
        stdout, "eq=true\nneq=false\nclone_eq=true\nhash_match=true\n",
        "fold-shaped derived methods (eq/hash/clone) must agree across \
         structurally-equal instances and a clone"
    );
}

fn ruxen_unique_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}
