//! Pin tests for #06.8 Phase 3b: class-body `lib` FFI decls now
//! register as class methods (the Ruxen equivalent of `def self.foo`)
//! and call sites `ClassName.method(...)` flow through the same
//! Phase-2 c_symbol alias rewrite that top-level FFI calls use.

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
fn class_lib_method_registered_as_class_method() {
    let program = parse_fixture("lib_class_method_basic");
    let resolver = Resolver::new();
    let result = resolver.resolve(&program);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "unexpected resolver errors: {:?}",
        errors
    );

    // The FFI method was registered under the PLAIN method name `bar`
    // (matching the convention `def self.bar(...)` would use) as a
    // `DefKind::Method` with is_class_method=true. The mangled
    // `Foo_bar` form lives only in the MIR-callee/alias-map layer.
    let foo_bar = result
        .symbols
        .iter()
        .find(|d| d.name == "bar" && matches!(&d.kind, DefKind::Method { .. }))
        .expect("bar method should be in the symbol table");
    match &foo_bar.kind {
        DefKind::Method {
            parent: _,
            signature,
        } => {
            assert!(
                signature.is_class_method,
                "class-body lib decl must register with is_class_method=true"
            );
            assert!(
                signature.self_mode.is_none(),
                "class-body lib decl must have no `self` mode"
            );
            assert_eq!(
                signature.c_symbol.as_deref(),
                Some("ruxen_test_extern_add_one"),
                "c_symbol must propagate through to the method signature"
            );
        }
        other => panic!("expected DefKind::Method, got {:?}", other),
    }

    // The HirProgram side-channel carries the FFI lib so the MIR
    // bridge can populate ffi_alias_map under the mangled name.
    assert_eq!(
        result.program.ffi_libs.len(),
        1,
        "expected one HirFfiLib for the class-body lib block"
    );
    let lib = &result.program.ffi_libs[0];
    assert_eq!(lib.functions.len(), 1);
    assert_eq!(lib.functions[0].ruxen_name, "Foo_bar");
    assert_eq!(
        lib.functions[0].c_symbol.as_deref(),
        Some("ruxen_test_extern_add_one")
    );
}

#[test]
fn class_lib_method_mir_callee_is_c_symbol() {
    let program = parse_fixture("lib_class_method_called");
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

    let main_fn = mir
        .functions
        .iter()
        .find(|f| f.name == "main")
        .expect("main fn in MIR");

    let mut found_aliased = false;
    let mut found_mangled = false;
    for block in &main_fn.blocks {
        for inst in &block.instructions {
            if let MirInst::Call { callee, .. } = inst {
                if callee == "ruxen_test_extern_add_one" {
                    found_aliased = true;
                }
                if callee == "Foo_bar" {
                    found_mangled = true;
                }
            }
        }
    }
    assert!(
        found_aliased,
        "MIR Call should use C symbol `ruxen_test_extern_add_one`; got {:?}",
        main_fn
    );
    assert!(
        !found_mangled,
        "mangled `Foo_bar` should be rewritten away at MIR-lowering time"
    );
}

#[test]
fn instance_lib_method_end_to_end_link_smoke() {
    // Instance-method counterpart to the class-method link smoke
    // below. Verifies that `def NAME(self, ...)` inside a class-body
    // `lib` block correctly registers as an instance method, that
    // `obj.method(args)` lowers with `self` prepended as the first
    // arg, and that the linked C symbol receives the receiver.
    let program = parse_fixture("lib_instance_method_link_smoke");
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

    let bin_path = workspace_root().join("tmp/lib_instance_method_link_smoke.bin");
    let _ = std::fs::create_dir_all(bin_path.parent().unwrap());
    codegen::compile(&mir, bin_path.to_str().unwrap()).expect("codegen");

    let output = Command::new(&bin_path).output().expect("run binary");
    assert!(
        output.status.success(),
        "binary should exit 0 (c.tick(41) == 42); got status {:?} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn class_lib_method_end_to_end_link_smoke() {
    let program = parse_fixture("lib_class_method_link_smoke");
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

    let bin_path = workspace_root().join("tmp/lib_class_method_link_smoke.bin");
    let _ = std::fs::create_dir_all(bin_path.parent().unwrap());
    codegen::compile(&mir, bin_path.to_str().unwrap()).expect("codegen");

    let output = Command::new(&bin_path).output().expect("run binary");
    assert!(
        output.status.success(),
        "binary should exit 0 (Foo.bar(41) == 42); got status {:?} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}
