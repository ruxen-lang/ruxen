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
    let bin_path = root.join(format!("tmp/{}-{}-{}.bin", name, std::process::id(), ruxen_unique_id()));
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
fn derive_ord_and_partial_ord_dispatch_through_trait_bounds() {
    let source = rx("derive_ord_and_partial_ord_dispatch_through_trait_bounds");
    let (stdout, exit) = compile_and_run(&source, "derive_ord_partial_ord_dispatch");
    assert_eq!(exit, Some(0), "non-zero exit; stdout={}", stdout);
    assert_eq!(stdout, "true\ntrue\n");
}

#[test]
fn derive_hashable_dispatches_through_trait_bounds() {
    let source = rx("derive_hashable_dispatches_through_trait_bounds");
    let (stdout, exit) = compile_and_run(&source, "derive_hashable_dispatch");
    assert_eq!(exit, Some(0), "non-zero exit; stdout={}", stdout);
    assert!(
        stdout.trim().parse::<i64>().is_ok(),
        "expected an integer hash, got {:?}",
        stdout
    );
}

/// `docs/specs/stdlib/hash.spec.md` §B2 v2 follow-up: primitives
/// already hash internally for `HashMap` keys, but the v1 surface
/// does not yet expose a user-callable `.hash_code` on primitives.
/// Generic `T: Hashable` dispatch for primitive `T` would mangle to
/// `Int_hash_code` / `String_hash_code` which are not registered in
/// `runtime_name` today, so the linker fails with
/// "Undefined symbols: _T: Hashable_hash_code".  This pin documents
/// the gap; remove the `#[ignore]` once primitive Hashable impls
/// are wired and re-run to confirm the dispatch path closes.
#[test]
#[ignore = "v2: primitive Hashable monomorphisation not wired (spec §B2 out-of-scope)"]
fn primitive_int_and_string_dispatch_through_hashable_bound() {
    let source = rx("primitive_int_and_string_dispatch_through_hashable_bound");
    let (stdout, exit) = compile_and_run(&source, "primitive_hashable_dispatch");
    assert_eq!(exit, Some(0), "non-zero exit; stdout={}", stdout);
}

#[test]
fn derive_default_emits_concrete_static_method() {
    let source = rx("derive_default_emits_concrete_static_method");
    let (stdout, exit) = compile_and_run(&source, "derive_default_concrete");
    assert_eq!(exit, Some(0), "non-zero exit; stdout={}", stdout);
    assert_eq!(stdout, "0\n");
}

fn ruxen_unique_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}
