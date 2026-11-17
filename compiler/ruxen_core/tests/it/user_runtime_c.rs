//! Pin tests for user-project `runtime/*.c` discovery + linking.
//!
//! Stdlib packages have always had their `library/std/<pkg>/runtime/*.c`
//! auto-compiled and linked via `find_runtime_sources()`. User projects
//! had no such hook — a `lib "runtime/foo.c"` decl in user code would
//! compile, but the symbols would resolve to `undefined reference` at
//! link time because no one was telling `cc` about the user's .c file.
//!
//! This test covers:
//!   1. `find_runtime_sources_in_dir` returns `Ok(vec![])` when no
//!      `runtime/` dir exists (the common case for pure-Ruxen projects).
//!   2. `find_runtime_sources_in_dir` returns only `*.c` files, sorted,
//!      when `runtime/` does exist.
//!   3. The new `extra_runtime_sources` param on `compile_with_options`
//!      ends up compiled and linked into the executable. A Ruxen
//!      program that calls a user-defined C function actually runs.
//!
//! The fixture intentionally uses a plain top-level `lib "ruxen_runtime"`
//! block instead of a class-shaped `lib "runtime/widget.c"`. Either
//! shape exercises the load-bearing claim — that the user's .c file
//! reaches the linker — but the top-level form sidesteps class/method
//! FFI plumbing that's irrelevant to this surface.

use ruxen_core::codegen;
use ruxen_core::diagnostics::DiagnosticLevel;
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;
use std::path::PathBuf;
use std::process::Command;

fn workspace_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

fn unique_tmp_dir(stem: &str) -> PathBuf {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = workspace_root()
        .join("tmp")
        .join("user-runtime-tests")
        .join(format!("{}-{}-{}", stem, pid, nanos));
    std::fs::create_dir_all(&path).expect("create tmp dir");
    path
}

#[test]
fn find_runtime_sources_in_dir_missing_runtime_is_ok_empty() {
    let dir = unique_tmp_dir("missing");
    // No `runtime/` subdir at all — must NOT error.
    let result =
        codegen::find_runtime_sources_in_dir(&dir).expect("missing runtime/ should not error");
    assert!(
        result.is_empty(),
        "expected no sources from a dir without runtime/, got {:?}",
        result
    );
}

#[test]
fn find_runtime_sources_in_dir_empty_runtime_is_ok_empty() {
    let dir = unique_tmp_dir("empty-runtime");
    std::fs::create_dir_all(dir.join("runtime")).expect("mkdir runtime");
    let result =
        codegen::find_runtime_sources_in_dir(&dir).expect("empty runtime/ should not error");
    assert!(
        result.is_empty(),
        "expected no sources from an empty runtime/, got {:?}",
        result
    );
}

#[test]
fn find_runtime_sources_in_dir_picks_up_c_files_only() {
    let dir = unique_tmp_dir("mixed-runtime");
    let runtime = dir.join("runtime");
    std::fs::create_dir_all(&runtime).expect("mkdir runtime");
    std::fs::write(runtime.join("zeta.c"), "/* z */\n").expect("write zeta.c");
    std::fs::write(runtime.join("alpha.c"), "/* a */\n").expect("write alpha.c");
    // Non-.c files must be ignored.
    std::fs::write(runtime.join("notes.txt"), "hi").expect("write notes.txt");
    std::fs::write(runtime.join("widget.h"), "// header").expect("write widget.h");

    let result = codegen::find_runtime_sources_in_dir(&dir).expect("scan");
    let names: Vec<String> = result
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        vec!["alpha.c".to_string(), "zeta.c".to_string()],
        "expected only .c files in sorted order; got {:?}",
        names
    );
}

#[test]
fn user_runtime_c_is_compiled_and_linked() {
    // ── 1. Write the user-supplied runtime/widget.c into a temp dir ──
    // `seed` flows in as int64_t (matches Ruxen `Int`), gets stashed in
    // a malloc'd struct, and the value accessor returns seed*seed.
    // Squaring rather than identity makes the test sensitive to a stale
    // .o being picked up from a previous run.
    let project_dir = unique_tmp_dir("project");
    let runtime_dir = project_dir.join("runtime");
    std::fs::create_dir_all(&runtime_dir).expect("mkdir runtime");
    let widget_c = r#"
#include <stdint.h>
#include <stdlib.h>

typedef struct rx_widget {
    int64_t seed;
} rx_widget;

int64_t rx_widget_new(int64_t seed) {
    rx_widget* w = (rx_widget*)malloc(sizeof(rx_widget));
    if (!w) return 0;
    w->seed = seed;
    return (int64_t)(uintptr_t)w;
}

int64_t rx_widget_value(int64_t handle) {
    rx_widget* w = (rx_widget*)(uintptr_t)handle;
    if (!w) return -1;
    int64_t v = w->seed * w->seed;
    /* Caller owns the handle; for this short-lived test we leak
       intentionally — the process exits immediately after. */
    return v;
}
"#;
    std::fs::write(runtime_dir.join("widget.c"), widget_c).expect("write widget.c");

    // ── 2. Discover via the new helper (this is the contract under test) ──
    let extra_runtime =
        codegen::find_runtime_sources_in_dir(&project_dir).expect("find_runtime_sources_in_dir");
    assert_eq!(
        extra_runtime.len(),
        1,
        "expected exactly one user runtime source, got {:?}",
        extra_runtime
    );

    // ── 3. Parse + typecheck + lower the fixture ──
    let fixture_path =
        workspace_root().join("compiler/ruxen_core/tests/fixtures/ruxen/user_runtime_widget.rx");
    let source = std::fs::read_to_string(&fixture_path)
        .unwrap_or_else(|e| panic!("read fixture {}: {}", fixture_path.display(), e));

    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");

    let type_result = typeck::type_check(&program);
    let errors: Vec<_> = type_result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "typecheck errors: {:?}", errors);

    let mut lowerer = Lowerer::new(&type_result.symbols);
    let mir = lowerer
        .lower_program(&type_result.program)
        .expect("MIR lowering");

    // ── 4. Compile + link, threading user_runtime through the new param ──
    let bin_path = project_dir.join("widget_bin");
    let bin_str = bin_path.to_string_lossy().to_string();
    codegen::compile_with_options(
        &mir,
        &bin_str,
        false,
        &[],
        &extra_runtime,
        codegen::Backend::Cranelift,
    )
    .expect("compile_with_options");

    // ── 5. Run it. Exit 0 iff rx_widget_value(rx_widget_new(7)) == 49 ──
    let output = Command::new(&bin_path).output().expect("run binary");
    assert!(
        output.status.success(),
        "binary should exit 0 (49 == 7*7); got status {:?}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
