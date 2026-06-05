//! Pin tests for #06.8 Phase 2: the `def name as "<c-symbol>"` alias
//! is now propagated all the way from parser → AST → resolver →
//! HirProgram::ffi_libs → MIR::ffi_libs → cranelift Pass-0 →
//! Linkage::Import. A call site `add_one(41)` whose `lib` block
//! declared `def add_one as "ruxen_test_extern_add_one"(...)` must
//! emit a direct call to `ruxen_test_extern_add_one`, and the linked
//! binary must run the actual C function (this fixture's runtime
//! symbol is in `library/std/core/runtime/test_extern.c`).

use ruxen_core::codegen;
use ruxen_core::diagnostics::DiagnosticLevel;
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::mir::nodes::MirInst;
use ruxen_core::parser::Parser;
use ruxen_core::resolve::symbols::DefKind;
use ruxen_core::resolve::Resolver;
use ruxen_core::typeck;
use std::path::PathBuf;
use std::process::Command;

fn workspace_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

fn parse_fixture(name: &str) -> ruxen_core::parser::ast::Program {
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
fn c_symbol_alias_propagates_to_fn_signature() {
    let program = parse_fixture("ffi_c_symbol_alias_basic");
    let resolver = Resolver::new();
    let result = resolver.resolve(&program);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "resolver should not produce errors on a valid lib decl; got {:?}",
        errors
    );

    let add_one = result
        .symbols
        .iter()
        .find(|d| d.name == "add_one")
        .expect("add_one should be registered in the symbol table");
    match &add_one.kind {
        DefKind::Function { signature } => {
            assert_eq!(
                signature.c_symbol.as_deref(),
                Some("ruxen_test_extern_add_one"),
                "FnSignature.c_symbol should carry the alias",
            );
        }
        other => panic!("expected DefKind::Function, got {:?}", other),
    }

    // The HirProgram should also carry the FFI lib as a side-channel.
    assert_eq!(
        result.program.ffi_libs.len(),
        1,
        "expected one HirFfiLib on the program"
    );
    let lib = &result.program.ffi_libs[0];
    assert_eq!(lib.name, "ruxen_runtime");
    assert_eq!(lib.functions.len(), 1);
    let f = &lib.functions[0];
    assert_eq!(f.ruxen_name, "add_one");
    assert_eq!(f.c_symbol.as_deref(), Some("ruxen_test_extern_add_one"));
}

#[test]
fn mir_call_to_aliased_ffi_uses_c_symbol() {
    let program = parse_fixture("ffi_c_symbol_alias_called");
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

    // The `main` function should contain a `Call` whose callee is the
    // C symbol, not the Ruxen-side name. This is the load-bearing
    // assertion that proves the MIR-layer rewrite is wired.
    let main_fn = mir
        .functions
        .iter()
        .find(|f| f.name == "main")
        .expect("main fn in MIR");

    let mut found_aliased_call = false;
    let mut found_ruxen_named_call = false;
    for block in &main_fn.blocks {
        for inst in &block.instructions {
            if let MirInst::Call { callee, .. } = inst {
                if callee == "ruxen_test_extern_add_one" {
                    found_aliased_call = true;
                }
                if callee == "add_one" {
                    found_ruxen_named_call = true;
                }
            }
        }
    }
    assert!(
        found_aliased_call,
        "expected MIR Call to use C symbol `ruxen_test_extern_add_one`, got: {:?}",
        main_fn
    );
    assert!(
        !found_ruxen_named_call,
        "Ruxen-side name `add_one` should be rewritten away at MIR level"
    );

    // MirProgram::ffi_libs is now populated (was dead-loaded before
    // Phase 2). With the Wave-2 (#06.8) bootstrap loader running by
    // default the list also contains the stdlib's own FFI libs
    // (`ruxen_runtime` from `library/std/rand/src/lib.rx`, the
    // `bootstrap_smoke/src/lib.rx` proof-of-life libs, …) — so this test
    // asserts that the user's `add_one` ↔ `ruxen_test_extern_add_one`
    // entry is PRESENT, rather than that the list has exactly one
    // entry.
    let mir_fn = mir
        .ffi_libs
        .iter()
        .flat_map(|lib| lib.functions.iter())
        .find(|f| f.ruxen_name == "add_one")
        .expect("FfiFuncDecl for add_one in some MirFfiLib");
    assert_eq!(mir_fn.name, "ruxen_test_extern_add_one");
}

#[test]
fn conflicting_c_symbol_decls_emit_e0722() {
    let program = parse_fixture("ffi_c_symbol_alias_conflict");
    let resolver = Resolver::new();
    let result = resolver.resolve(&program);
    let e0722_count = result
        .diagnostics
        .iter()
        .filter(|d| d.code.as_deref() == Some("E0722"))
        .count();
    assert_eq!(
        e0722_count,
        1,
        "expected exactly one E0722 diagnostic; got {} diags: {:?}",
        result.diagnostics.len(),
        result
            .diagnostics
            .iter()
            .map(|d| (d.code.clone(), d.message.clone()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn end_to_end_link_smoke() {
    // Compile + link + run a Ruxen program that calls an FFI-aliased
    // function. The exit code is 0 iff `ruxen_test_extern_add_one(41)`
    // returned 42 (i.e. the linker resolved the call to the actual C
    // function in `library/std/core/runtime/test_extern.c`, NOT to the Ruxen
    // name `add_one` which doesn't exist as a runtime symbol).
    let program = parse_fixture("ffi_c_symbol_link_smoke");
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

    let bin_path = workspace_root().join(format!(
        "tmp/ffi_c_symbol_link_smoke-{}-{}.bin",
        std::process::id(),
        ruxen_unique_id()
    ));
    let _ = std::fs::create_dir_all(bin_path.parent().unwrap());
    codegen::compile(&mir, bin_path.to_str().unwrap()).expect("codegen");

    let output = Command::new(&bin_path).output().expect("run binary");
    let _ = std::fs::remove_file(&bin_path);
    assert!(
        output.status.success(),
        "binary should exit 0 (add_one(41) == 42); got status {:?} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn ruxen_unique_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}
