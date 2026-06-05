//! #06.93 Phase 5 pin test: a class inside a module that
//! `include`s a sibling mixin from the SAME module must surface
//! the mixin's `lib` FFI decls on the qualified class name.
//!
//! End-to-end: `BufferedThings.File.add(41)` must produce 42 via
//! the `ruxen_test_extern_add_one` C symbol. Verifies that:
//!
//! 1. The pre-pass `collect_mixin_lib_decls` registers
//!    `BufferedThings.AddMixin` under its qualified key.
//! 2. The Class arm's include-walk for `BufferedThings.File`
//!    resolves the bare `include AddMixin` against the
//!    enclosing-module-qualified key, finds the lib decls.
//! 3. `register_class_lib_method_in` keys the FFI alias under
//!    the fully qualified mangled `BufferedThings_File_add`.
//! 4. The MIR call site for `BufferedThings.File.add(41)` builds
//!    the matching mangled callee.
//!
//! This is the foundational shape for the #06.95 BufReader /
//! BufWriter module + mixin reshape.

use ruxen_core::codegen;
use ruxen_core::diagnostics::{Diagnostic, DiagnosticLevel};
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::parser::ast::Program;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;
use std::path::PathBuf;
use std::process::Command;

fn workspace_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

fn parse_fixture(name: &str) -> Program {
    let path = format!(
        "{}/compiler/ruxen_core/tests/fixtures/ruxen/{}.rx",
        workspace_root().display(),
        name
    );
    let source = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path, e));
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    parser.parse().expect("parse")
}

#[test]
fn qualified_class_includes_qualified_mixin_dispatches_correctly() {
    let program = parse_fixture("qualified_class_includes_qualified_mixin");
    let type_result = typeck::type_check(&program);
    let errors: Vec<&Diagnostic> = type_result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "typecheck errors on BufferedThings.File.add(41): {:#?}",
        errors
    );

    let mut lowerer = Lowerer::new(&type_result.symbols);
    let mir = lowerer
        .lower_program(&type_result.program)
        .expect("MIR lowering");

    let bin_path = workspace_root().join(format!(
        "tmp/qualified_class_includes_qualified_mixin-{}-{}.bin",
        std::process::id(),
        ruxen_unique_id()
    ));
    let _ = std::fs::create_dir_all(bin_path.parent().unwrap());
    codegen::compile(&mir, bin_path.to_str().unwrap()).expect("codegen");

    let output = Command::new(&bin_path)
        .output()
        .expect("run qualified class + qualified mixin binary");
    let _ = std::fs::remove_file(&bin_path);
    assert!(
        output.status.success(),
        "binary should exit 0 (BufferedThings.File.add(41) == 42); status={:?} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn ruxen_unique_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}
