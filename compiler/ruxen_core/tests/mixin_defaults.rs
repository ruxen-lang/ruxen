//! Pin tests for `docs/specs/types/mixin_default_methods.spec.md`.
//!
//! Covers the B2 behaviour: an implementor class that `include`s a
//! mixin inherits the mixin's default method bodies, and the default
//! body resolves `self.<required>` calls against the implementor's
//! own implementation of the required method.
//!
//! Also serves as a regression guard for the bootstrap-class String
//! stomp described in
//! `project_ruxen_resolve_class_stomps_typealias`: the default body
//! interpolates `self.name` into a `String` literal whose declared
//! return type is `String`. Before option (a) landed in
//! `resolve_type_expr`, the typeck unifier raised
//! `expected `String`, found `String`` because one side was
//! `Ty::String` (the interpolation result) and the other was
//! `Ty::Class { name: "String" }` (the declared return type, after
//! `resolve_class` stomped the original `TypeAlias` DefKind).

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
fn mixin_default_body_with_string_interp_typechecks_and_runs() {
    let program = parse_fixture("mixin_default_method_body_with_string_interp");
    let type_result = typeck::type_check(&program);
    let errors: Vec<&Diagnostic> = type_result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "typecheck errors on mixin default-body with String interp: {:?}",
        errors
    );

    let mut lowerer = Lowerer::new(&type_result.symbols);
    let mir = lowerer
        .lower_program(&type_result.program)
        .expect("MIR lowering");

    let bin_path = workspace_root().join(format!(
        "tmp/mixin_default_method_body_with_string_interp-{}-{}.bin",
        std::process::id(),
        ruxen_unique_id()
    ));
    let _ = std::fs::create_dir_all(bin_path.parent().unwrap());
    codegen::compile(&mir, bin_path.to_str().unwrap()).expect("codegen");

    let output = Command::new(&bin_path)
        .output()
        .expect("run compiled binary");
    let _ = std::fs::remove_file(&bin_path);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "binary exited with status {:?}\nstdout: {}\nstderr: {}",
        output.status,
        stdout,
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(
        stdout.as_ref(),
        "Hello, Riv!\n",
        "default mixin body should interpolate Bot.name via self.name"
    );
}

fn ruxen_unique_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}
