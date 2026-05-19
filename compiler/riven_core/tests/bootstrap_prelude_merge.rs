//! Pin tests for #06.8 Phase 3: bootstrap prelude merge.
//!
//! Phase 2 wired `c_symbol` aliases end-to-end. Phase 3 ties the
//! `run_bootstrap` loader to the resolver via
//! `Resolver::merge_bootstrap_programs` (and the typeck convenience
//! wrapper `type_check_with_bootstrap`), and ships one proof-of-life
//! stdlib file — `library/std/src/_bootstrap_smoke.rvn` — that
//! declares `bootstrap_smoke_add_one` aliased to the runtime test
//! symbol `riven_test_extern_add_one`.
//!
//! These tests pin the three invariants the Phase-3 brief requires:
//!
//! 1. A parsed bootstrap `Program` injected through the merge path
//!    appears in the user program's resolver scope (i.e. user code
//!    can call its top-level defs without "undefined function"
//!    diagnostics, and the MIR call site carries the aliased C
//!    symbol).
//! 2. The Wave-1.5 smoke file compiles + links + runs end-to-end
//!    against the runtime symbol from `library/runtime/test_extern.c`.
//! 3. A deliberately broken stdlib file produces E0725 diagnostics
//!    through the same loader the driver calls — proving the
//!    "stdlib bootstrap failed → fatal" policy is wired.

use riven_core::codegen;
use riven_core::diagnostics::{Diagnostic, DiagnosticLevel};
use riven_core::lexer::Lexer;
use riven_core::mir::lower::Lowerer;
use riven_core::mir::nodes::MirInst;
use riven_core::parser::ast::Program;
use riven_core::parser::Parser;
use riven_core::resolve::bootstrap::run_bootstrap_with_files;
use riven_core::resolve::symbols::DefKind;
use riven_core::resolve::Resolver;
use riven_core::typeck;
use std::path::{Path, PathBuf};
use std::process::Command;

// ─── Fixture / scratch helpers ──────────────────────────────────────────

fn workspace_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

fn parse_fixture(name: &str) -> Program {
    let path = format!(
        "{}/compiler/riven_core/tests/fixtures/riven/{}.rvn",
        workspace_root().display(),
        name
    );
    let source =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path, e));
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    parser.parse().expect("parse")
}

/// Self-cleaning tempdir, modeled on the helper in `stdlib_bootstrap.rs`.
/// We don't reach for `tempfile` to keep `riven_core`'s dev-deps lean.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let mut base = std::env::temp_dir();
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        static COUNTER: std::sync::atomic::AtomicU64 =
            std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        base.push(format!("riven-bootstrap-prelude-{pid}-{nanos}-{n}"));
        std::fs::create_dir_all(&base).expect("create tempdir");
        TempDir { path: base }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn write_fixture(dir: &Path, rel: &str, contents: &str) {
    let full = dir.join(rel);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).expect("create fixture parent dir");
    }
    std::fs::write(&full, contents).expect("write fixture file");
}

// ─── Test 1: merge injects bootstrap def into user scope ───────────────

#[test]
fn bootstrap_program_injects_def_into_user_scope() {
    // Hand-roll a bootstrap program by parsing a tiny lib-decl fixture,
    // then resolve a user program that calls `foo(41)`. The merge path
    // must register `foo` in the resolver scope BEFORE user pass-1 so
    // the user call resolves cleanly, and MIR must rewrite the callee
    // to the aliased C symbol.
    let bootstrap_program = parse_fixture("bootstrap_prelude_lib_decl");
    let user_program = parse_fixture("bootstrap_prelude_user_calls_foo");

    // Resolver path: no "undefined function" error on `foo`, and the
    // HirProgram carries the FFI lib coming from the bootstrap program.
    let resolver = Resolver::new();
    let result = resolver.resolve_with_bootstrap(&user_program, &[bootstrap_program.clone()]);
    let errors: Vec<&Diagnostic> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "resolver should not produce errors on prelude-merged call; got {:?}",
        errors
    );
    assert_eq!(
        result.program.ffi_libs.len(),
        1,
        "bootstrap-loaded lib block should contribute exactly one HirFfiLib; got {:?}",
        result.program.ffi_libs
    );
    let bootstrap_lib = &result.program.ffi_libs[0];
    assert_eq!(bootstrap_lib.name, "rt");
    assert_eq!(bootstrap_lib.functions.len(), 1);
    let foo = &bootstrap_lib.functions[0];
    assert_eq!(foo.riven_name, "foo");
    assert_eq!(foo.c_symbol.as_deref(), Some("riven_test_extern_add_one"));

    // MIR path: the call to `foo(41)` rewrites to the C symbol.
    let type_result =
        typeck::type_check_with_bootstrap(&user_program, &[bootstrap_program]);
    let type_errors: Vec<&Diagnostic> = type_result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(
        type_errors.is_empty(),
        "typecheck errors on prelude-merged call: {:?}",
        type_errors
    );

    let mut lowerer = Lowerer::new(&type_result.symbols);
    let mir = lowerer
        .lower_program(&type_result.program)
        .expect("MIR lowering");

    let main_fn = mir
        .functions
        .iter()
        .find(|f| f.name == "main")
        .expect("main fn in MIR");
    let mut found_aliased = false;
    let mut found_riven_named = false;
    for block in &main_fn.blocks {
        for inst in &block.instructions {
            if let MirInst::Call { callee, .. } = inst {
                if callee == "riven_test_extern_add_one" {
                    found_aliased = true;
                }
                if callee == "foo" {
                    found_riven_named = true;
                }
            }
        }
    }
    assert!(
        found_aliased,
        "expected MIR Call to use aliased C symbol; main = {:?}",
        main_fn
    );
    assert!(
        !found_riven_named,
        "Riven-side `foo` should not appear in MIR call sites — the alias rewrite is the whole point"
    );
}

// ─── Test 2: end-to-end smoke through `run_bootstrap` ──────────────────

#[test]
fn bootstrap_smoke_e2e_via_runtime_file() {
    // Drive the loader directly against the production stdlib root so
    // this test exercises EXACTLY the path the driver takes (parse
    // `library/std/src/_bootstrap_smoke.rvn`, merge into resolver,
    // compile a user .rvn that calls `bootstrap_smoke_add_one`, link
    // against the runtime, run the binary, expect exit 0). If this
    // test passes, the whole "stdlib self-hosting" architecture is
    // proven for Wave 1.5.
    let stdlib_root = workspace_root().join("library/std/src");
    let mut diags = Vec::<Diagnostic>::new();
    let bootstrap_programs = run_bootstrap_with_files(
        &["_bootstrap_smoke.rvn"],
        Some(&stdlib_root),
        &mut diags,
    );
    assert!(
        diags.is_empty(),
        "_bootstrap_smoke.rvn must parse cleanly; got: {:?}",
        diags
    );
    assert_eq!(
        bootstrap_programs.len(),
        1,
        "expected one parsed bootstrap program"
    );

    let user_program = parse_fixture("bootstrap_smoke_caller");
    let type_result =
        typeck::type_check_with_bootstrap(&user_program, &bootstrap_programs);
    let errors: Vec<&Diagnostic> = type_result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "typecheck errors compiling smoke caller against bootstrap: {:?}",
        errors
    );

    let mut lowerer = Lowerer::new(&type_result.symbols);
    let mir = lowerer
        .lower_program(&type_result.program)
        .expect("MIR lowering");

    let bin_path = workspace_root().join("tmp/bootstrap_smoke_e2e.bin");
    let _ = std::fs::create_dir_all(bin_path.parent().unwrap());
    codegen::compile(&mir, bin_path.to_str().unwrap()).expect("codegen");

    let output = Command::new(&bin_path)
        .output()
        .expect("run bootstrap-smoke binary");
    assert!(
        output.status.success(),
        "binary should exit 0 (bootstrap_smoke_add_one(41) == 42); status={:?} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

// ─── Test 2b: bootstrap-loaded CLASS with class-method FFI ──────────────

#[test]
fn bootstrap_class_method_e2e_via_runtime_file() {
    // Phase 4 proof-of-life: a class declared inside the bootstrap
    // `.rvn` file with class-method FFI bindings is callable from user
    // code with no extra wiring. This is the load-bearing test for
    // "stdlib self-hosting works" — the architecture that's needed
    // for Wave 2+ migrations of real stdlib classes.
    let stdlib_root = workspace_root().join("library/std/src");
    let mut diags = Vec::<Diagnostic>::new();
    let bootstrap_programs = run_bootstrap_with_files(
        &["_bootstrap_smoke.rvn"],
        Some(&stdlib_root),
        &mut diags,
    );
    assert!(diags.is_empty(), "bootstrap parse: {:?}", diags);

    let user_program = parse_fixture("bootstrap_class_method_caller");
    let type_result =
        typeck::type_check_with_bootstrap(&user_program, &bootstrap_programs);
    let errors: Vec<&Diagnostic> = type_result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "typecheck errors calling bootstrap class methods: {:?}",
        errors
    );

    let mut lowerer = Lowerer::new(&type_result.symbols);
    let mir = lowerer
        .lower_program(&type_result.program)
        .expect("MIR lowering");

    let bin_path = workspace_root().join("tmp/bootstrap_class_method_e2e.bin");
    let _ = std::fs::create_dir_all(bin_path.parent().unwrap());
    codegen::compile(&mir, bin_path.to_str().unwrap()).expect("codegen");

    let output = Command::new(&bin_path)
        .output()
        .expect("run bootstrap-class-method binary");
    assert!(
        output.status.success(),
        "binary should exit 0 (BootstrapSmokeClass.add_one(41) == 42 AND \
         BootstrapSmokeClass.double(20) == 40); status={:?} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

// ─── Test 3: broken stdlib file aborts via E0725 ───────────────────────

#[test]
fn bootstrap_failure_aborts_driver() {
    // The driver policy is: bootstrap diagnostics are fatal. We can't
    // `process::exit` from a unit test, but we CAN exercise the same
    // surface the driver does (`run_bootstrap_with_files`) and assert
    // the diagnostic shape — that's what gates the driver's exit. The
    // policy is documented in `rivenc::main::load_bootstrap_or_exit`.
    let tmp = TempDir::new();
    write_fixture(
        tmp.path(),
        "broken.rvn",
        "class Foo\n  def open(self) ->\n    # missing return type and body\nend\n",
    );

    let mut diags = Vec::<Diagnostic>::new();
    let programs =
        run_bootstrap_with_files(&["broken.rvn"], Some(tmp.path()), &mut diags);

    // The loader returns no parsed programs for the broken file…
    assert!(
        programs.is_empty(),
        "broken bootstrap file should not yield a usable Program; got {} programs",
        programs.len()
    );
    // …and emits at least one E0725 — the load-bearing fact the driver
    // branches on.
    assert!(
        diags.iter().any(|d| d.code.as_deref() == Some("E0725")),
        "broken stdlib file should emit E0725; got: {:?}",
        diags
    );
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("library/std/src/broken.rvn")),
        "diagnostic should cite the broken stdlib file path; got: {:?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

// ─── Test 4: bootstrap merge accepts `mixin` items (#06.8 T#19) ────────

#[test]
fn bootstrap_program_with_mixin_registers_trait_in_prelude() {
    // Wave 2 (#06.8 T#19) expanded `is_bootstrap_supported_item` to
    // include `Mixin`. This pin test asserts that a `mixin Foo ... end`
    // block inside a bootstrap-loaded `.rvn` file lands in the
    // resolver's symbol table as a `DefKind::Trait` — the same shape a
    // user-code mixin produces. Without this gate the migration of
    // iter.rvn / hash.rvn / fmt.rvn cannot proceed because their
    // surfaces are mixin-shaped (`mixin Iterator`, `mixin Hashable`,
    // `mixin Display`).
    let mut lexer = Lexer::new(
        "mixin BootstrapPinMixin\n  def pin_method -> Int\nend\n",
    );
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let bootstrap_program = parser.parse().expect("parse");

    // Minimal user program — bootstrap merge runs regardless of
    // whether user code references the mixin; we just need ANY parsable
    // user input.
    let mut user_lexer = Lexer::new("def main\nend\n");
    let user_tokens = user_lexer.tokenize().expect("lex user");
    let mut user_parser = Parser::new(user_tokens);
    let user_program = user_parser.parse().expect("parse user");

    let resolver = Resolver::new();
    let result = resolver.resolve_with_bootstrap(&user_program, &[bootstrap_program]);
    let errors: Vec<&Diagnostic> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "bootstrap merge of a mixin-only program should not produce errors; got {:?}",
        errors
    );

    let mixin = result
        .symbols
        .iter()
        .find(|d| d.name == "BootstrapPinMixin")
        .expect("BootstrapPinMixin should be in the symbol table");
    assert!(
        matches!(mixin.kind, DefKind::Trait { .. }),
        "expected DefKind::Trait for bootstrap-loaded mixin; got {:?}",
        mixin.kind
    );
}
