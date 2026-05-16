//! Drop-elaboration regression suite.
//!
//! Compiles a fixture against a tracking runtime that wraps
//! `riven_alloc` / `riven_dealloc` with counters, runs the resulting
//! binary, and asserts the counts balance at process exit. A leak
//! manifests as `outstanding > 0` in the runtime's exit message.
//!
//! Used as the regression target for P0.2 (free heap-allocated
//! struct/class/enum locals at scope exit) and P0.7 (free
//! heap-allocated `String` / `Vec` / `HashMap` locals at scope exit).
//!
//! For P0.7 the splice additionally wraps every `malloc` / `free` /
//! `realloc` call site in `runtime.c` with raw-heap counters, so leaks
//! that flow through `riven_string_from`, `riven_vec_new`,
//! `riven_hash_new` (and friends) are observable even though those
//! constructors don't go through `riven_alloc`. When the runtime
//! exposes the new helpers `riven_string_free`, `riven_vec_free`,
//! `riven_hash_free`, the splice also injects per-helper counters so
//! tests can assert which kind of free fired.

use riven_core::codegen;
use riven_core::lexer::Lexer;
use riven_core::mir::lower::Lowerer;
use riven_core::parser::Parser;
use riven_core::typeck;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;

fn rvn(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/riven")
        .join(format!("{name}.rvn"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

/// Serializes the `RIVEN_RUNTIME` env-var window across the parallel
/// tests in this binary. `cargo test` runs `#[test]` fns on a thread
/// pool that shares process env, so without this lock two concurrent
/// `compile_and_run_with_tracking` calls can race: one thread's
/// `remove_var` clobbers another thread's `set_var` between the
/// matching `codegen::compile` invocation, leaving the second compile
/// linked against the default (untracked) runtime — which never emits
/// the `RIVEN_TEST_LEAK` marker the assertion expects.
static RIVEN_RUNTIME_ENV_LOCK: Mutex<()> = Mutex::new(());

fn workspace_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

/// Build a `runtime.c` clone augmented with `riven_alloc` /
/// `riven_dealloc` tracking and an `atexit` handler that emits
/// `RIVEN_TEST_LEAK allocs=N frees=M outstanding=K` to stderr.
///
/// We replace the function bodies in place rather than re-declaring
/// them so the rest of the runtime (riven_panic, riven_string_*, …)
/// keeps using the tracked allocator.
fn write_tracking_runtime(target: &PathBuf) {
    let runtime_c = workspace_root().join("crates/riven-core/runtime/runtime.c");
    let original = std::fs::read_to_string(&runtime_c)
        .unwrap_or_else(|e| panic!("read {}: {}", runtime_c.display(), e));

    // Replace the allocator and deallocator. We keep the original size
    // / overflow checks by simply shadowing the body with tracked
    // versions.
    let alloc_old = "void *riven_alloc(uint64_t size) {\n\
        \x20\x20\x20\x20void *ptr = malloc((size_t)size);\n\
        \x20\x20\x20\x20if (!ptr && size > 0) {\n\
        \x20\x20\x20\x20\x20\x20\x20\x20riven_panic(\"out of memory\");\n\
        \x20\x20\x20\x20}\n\
        \x20\x20\x20\x20memset(ptr, 0, (size_t)size);\n\
        \x20\x20\x20\x20return ptr;\n\
        }";
    let alloc_new = "static uint64_t riven_test_allocs = 0;\n\
        static uint64_t riven_test_frees = 0;\n\
        static int riven_test_atexit_registered = 0;\n\
        static void riven_test_print_leak(void) {\n\
        \x20\x20\x20\x20fflush(stdout);\n\
        \x20\x20\x20\x20uint64_t out = riven_test_allocs - riven_test_frees;\n\
        \x20\x20\x20\x20uint64_t raw_out = riven_test_raw_mallocs - riven_test_raw_frees;\n\
        \x20\x20\x20\x20fprintf(stderr,\n\
        \x20\x20\x20\x20\x20\x20\x20\x20\"RIVEN_TEST_LEAK allocs=%llu frees=%llu outstanding=%llu\\n\",\n\
        \x20\x20\x20\x20\x20\x20\x20\x20(unsigned long long)riven_test_allocs,\n\
        \x20\x20\x20\x20\x20\x20\x20\x20(unsigned long long)riven_test_frees,\n\
        \x20\x20\x20\x20\x20\x20\x20\x20(unsigned long long)out);\n\
        \x20\x20\x20\x20fprintf(stderr,\n\
        \x20\x20\x20\x20\x20\x20\x20\x20\"RIVEN_TEST_RAW raw_mallocs=%llu raw_frees=%llu raw_outstanding=%llu\\n\",\n\
        \x20\x20\x20\x20\x20\x20\x20\x20(unsigned long long)riven_test_raw_mallocs,\n\
        \x20\x20\x20\x20\x20\x20\x20\x20(unsigned long long)riven_test_raw_frees,\n\
        \x20\x20\x20\x20\x20\x20\x20\x20(unsigned long long)raw_out);\n\
        \x20\x20\x20\x20fprintf(stderr,\n\
        \x20\x20\x20\x20\x20\x20\x20\x20\"RIVEN_TEST_KIND string_frees=%llu vec_frees=%llu hash_frees=%llu data_buffer_frees=%llu entry_frees=%llu set_frees=%llu\\n\",\n\
        \x20\x20\x20\x20\x20\x20\x20\x20(unsigned long long)riven_test_string_frees,\n\
        \x20\x20\x20\x20\x20\x20\x20\x20(unsigned long long)riven_test_vec_frees,\n\
        \x20\x20\x20\x20\x20\x20\x20\x20(unsigned long long)riven_test_hash_frees,\n\
        \x20\x20\x20\x20\x20\x20\x20\x20(unsigned long long)riven_test_data_buffer_frees,\n\
        \x20\x20\x20\x20\x20\x20\x20\x20(unsigned long long)riven_test_entry_frees,\n\
        \x20\x20\x20\x20\x20\x20\x20\x20(unsigned long long)riven_test_set_frees);\n\
        \x20\x20\x20\x20fflush(stderr);\n\
        }\n\
        void *riven_alloc(uint64_t size) {\n\
        \x20\x20\x20\x20if (!riven_test_atexit_registered) {\n\
        \x20\x20\x20\x20\x20\x20\x20\x20atexit(riven_test_print_leak);\n\
        \x20\x20\x20\x20\x20\x20\x20\x20riven_test_atexit_registered = 1;\n\
        \x20\x20\x20\x20}\n\
        \x20\x20\x20\x20void *ptr = malloc((size_t)size);\n\
        \x20\x20\x20\x20if (!ptr && size > 0) {\n\
        \x20\x20\x20\x20\x20\x20\x20\x20riven_panic(\"out of memory\");\n\
        \x20\x20\x20\x20}\n\
        \x20\x20\x20\x20memset(ptr, 0, (size_t)size);\n\
        \x20\x20\x20\x20riven_test_allocs += 1;\n\
        \x20\x20\x20\x20return ptr;\n\
        }";
    let dealloc_old = "void riven_dealloc(void *ptr) {\n\
        \x20\x20\x20\x20free(ptr);\n\
        }";
    let dealloc_new = "void riven_dealloc(void *ptr) {\n\
        \x20\x20\x20\x20if (ptr) {\n\
        \x20\x20\x20\x20\x20\x20\x20\x20riven_test_frees += 1;\n\
        \x20\x20\x20\x20}\n\
        \x20\x20\x20\x20free(ptr);\n\
        }";

    let mut tracked = original.replace(alloc_old, alloc_new);
    if !tracked.contains("riven_test_allocs") {
        panic!(
            "could not splice tracked riven_alloc into runtime.c — has the \
             original definition changed? Update drop_fixtures.rs::write_tracking_runtime."
        );
    }
    let before_dealloc = tracked.clone();
    tracked = tracked.replace(dealloc_old, dealloc_new);
    if tracked == before_dealloc {
        panic!(
            "could not splice tracked riven_dealloc into runtime.c — has the \
             original definition changed? Update drop_fixtures.rs::write_tracking_runtime."
        );
    }

    // Prepend the raw-heap wrapper definitions and per-kind counter
    // storage at the very top of the file so every subsequent function
    // sees them. The wrappers themselves have to call libc directly
    // (otherwise the global `malloc(` → `riven_test_malloc(` rewrite
    // would make them recurse), so they reference `ORIG_MALLOC` /
    // `ORIG_FREE` / `ORIG_REALLOC` sentinels that we substitute back to
    // raw libc names *after* the global rewrite has run.
    // Prelude uses sentinel tokens for its own header and body so the
    // upcoming global text rewrite doesn't accidentally double-rewrite
    // (e.g. `malloc(` inside `riven_test_malloc(` would become
    // `riven_test_riven_test_malloc(`). After the rewrite, sentinels
    // are substituted back to their final names.
    //
    // Sentinels used:
    //   * `WRAPNAME_MALLOC` → `riven_test_malloc`  (wrapper definition only)
    //   * `WRAPNAME_FREE`   → `riven_test_free`
    //   * `WRAPNAME_REALLOC`→ `riven_test_realloc`
    //   * `ORIG_MALLOC(`    → `malloc(`            (call to libc inside wrappers)
    //   * `ORIG_FREE(`      → `free(`
    //   * `ORIG_REALLOC(`   → `realloc(`
    let raw_prelude = "/* drop_fixtures: raw-heap counters & wrappers */\n\
        #include <stdlib.h>\n\
        #include <stdint.h>\n\
        static uint64_t riven_test_raw_mallocs = 0;\n\
        static uint64_t riven_test_raw_frees = 0;\n\
        static uint64_t riven_test_string_frees = 0;\n\
        static uint64_t riven_test_vec_frees = 0;\n\
        static uint64_t riven_test_hash_frees = 0;\n\
        static uint64_t riven_test_data_buffer_frees = 0;\n\
        static uint64_t riven_test_entry_frees = 0;\n\
        static uint64_t riven_test_set_frees = 0;\n\
        /* Forward-declared in alloc_new; defined alongside riven_alloc. */\n\
        static void riven_test_print_leak(void);\n\
        /* Forward-declared in alloc_new. Flipping it here prevents \n\
           riven_alloc from re-registering the same handler. */\n\
        static int riven_test_atexit_registered;\n\
        /* Register atexit unconditionally — fixtures that never call \n\
           riven_alloc still need the leak markers printed. \n\
           NOTE: external linkage (no `static`) is required because \n\
           Apple Clang at -O2 elides `static` constructor functions \n\
           that have no in-TU callers; that produced silently-empty \n\
           stderr and a missing RIVEN_TEST_LEAK marker on macOS CI. */\n\
        __attribute__((constructor))\n\
        void riven_test_register_atexit(void) {\n\
        \x20\x20\x20\x20atexit(riven_test_print_leak);\n\
        \x20\x20\x20\x20riven_test_atexit_registered = 1;\n\
        }\n\
        static void *WRAPNAME_MALLOC(size_t n) {\n\
        \x20\x20\x20\x20void *p = ORIG_MALLOC(n);\n\
        \x20\x20\x20\x20if (p) riven_test_raw_mallocs += 1;\n\
        \x20\x20\x20\x20return p;\n\
        }\n\
        static void WRAPNAME_FREE(void *p) {\n\
        \x20\x20\x20\x20if (p) riven_test_raw_frees += 1;\n\
        \x20\x20\x20\x20ORIG_FREE(p);\n\
        }\n\
        static void *WRAPNAME_REALLOC(void *p, size_t n) {\n\
        \x20\x20\x20\x20if (p == NULL && n > 0) {\n\
        \x20\x20\x20\x20\x20\x20\x20\x20void *np = ORIG_REALLOC(p, n);\n\
        \x20\x20\x20\x20\x20\x20\x20\x20if (np) riven_test_raw_mallocs += 1;\n\
        \x20\x20\x20\x20\x20\x20\x20\x20return np;\n\
        \x20\x20\x20\x20}\n\
        \x20\x20\x20\x20if (p != NULL && n == 0) {\n\
        \x20\x20\x20\x20\x20\x20\x20\x20riven_test_raw_frees += 1;\n\
        \x20\x20\x20\x20\x20\x20\x20\x20return ORIG_REALLOC(p, n);\n\
        \x20\x20\x20\x20}\n\
        \x20\x20\x20\x20return ORIG_REALLOC(p, n);\n\
        }\n";
    tracked = format!("{}{}", raw_prelude, tracked);

    // 1) Rewrite every `malloc(` / `free(` / `realloc(` call site in
    //    runtime.c to flow through the test-only wrappers.
    tracked = tracked.replace("malloc(", "riven_test_malloc(");
    tracked = tracked.replace("free(", "riven_test_free(");
    tracked = tracked.replace("realloc(", "riven_test_realloc(");
    // 2) Restore the wrapper *definitions'* libc-call sentinels back to
    //    raw libc names so the wrappers don't recurse into themselves.
    tracked = tracked.replace("ORIG_MALLOC(", "malloc(");
    tracked = tracked.replace("ORIG_FREE(", "free(");
    tracked = tracked.replace("ORIG_REALLOC(", "realloc(");
    // 3) Restore the wrapper *names* (which we hid behind WRAPNAME_*
    //    sentinels) to their final identifiers.
    tracked = tracked.replace("WRAPNAME_MALLOC", "riven_test_malloc");
    tracked = tracked.replace("WRAPNAME_FREE", "riven_test_free");
    tracked = tracked.replace("WRAPNAME_REALLOC", "riven_test_realloc");

    // If the runtime exposes the new heap-deinit helpers (added by P0.7),
    // inject per-kind counter increments at the top of each one. The
    // helpers all take a single pointer named `s` / `v` / `h`. We splice
    // by matching the function header line; if a header is absent the
    // splice is a no-op and the corresponding counter simply stays at 0,
    // which manifests as a test-level assertion failure that points the
    // coder at the missing helper.
    tracked = inject_helper_counter(
        &tracked,
        "void riven_string_free(char *s) {",
        "void riven_string_free(char *s) {\n    if (s) riven_test_string_frees += 1;",
    );
    tracked = inject_helper_counter(
        &tracked,
        "void riven_vec_free(RivenVec *v) {",
        "void riven_vec_free(RivenVec *v) {\n    if (v) riven_test_vec_frees += 1;\n    if (v && v->data) riven_test_data_buffer_frees += 1;",
    );
    tracked = inject_helper_counter(
        &tracked,
        "void riven_hash_free(RivenHash *h) {",
        "void riven_hash_free(RivenHash *h) {\n    if (h) {\n        riven_test_hash_frees += 1;\n        for (unsigned riven_test_b = 0; riven_test_b < 16; riven_test_b++) {\n            RivenHashEntry *riven_test_e = h->buckets[riven_test_b];\n            while (riven_test_e) { riven_test_entry_frees += 1; riven_test_e = riven_test_e->next; }\n        }\n    }",
    );
    // Phase 2 stdlib (#04 batch 2): set spine drop. The hash spine
    // and string spine count their own frees independently above.
    // The new HashMap/HashSet per-element drop helpers
    // (`riven_hash_drop_*`, `riven_set_drop_string`) all reach
    // `riven_string_ORIG_FREE` and `riven_hash_ORIG_FREE` /
    // `riven_set_ORIG_FREE` internally, so the per-kind counters
    // bump transitively without needing to instrument the new
    // helpers themselves.
    tracked = inject_helper_counter(
        &tracked,
        "void riven_set_free(RivenSet *s) {",
        "void riven_set_free(RivenSet *s) {\n    if (s) riven_test_set_frees += 1;",
    );

    std::fs::write(target, tracked).unwrap_or_else(|e| panic!("write {}: {}", target.display(), e));
}

/// Replace `header_old` with `header_new` if present. Used to inject
/// per-helper counters into the new free helpers added for P0.7.
/// Returns `tracked` unchanged when the header is not yet present
/// (i.e. before the coder has added the helper); in that case the
/// matching counter stays at 0 and the test assertion that depends on
/// it fails — the intended red.
fn inject_helper_counter(tracked: &str, header_old: &str, header_new: &str) -> String {
    if tracked.contains(header_old) {
        tracked.replacen(header_old, header_new, 1)
    } else {
        tracked.to_string()
    }
}

fn compile_and_run_with_tracking(name: &str, source: &str) -> (String, String, Option<i32>) {
    let root = workspace_root();
    let tmp_dir = root.join("tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let bin_path = tmp_dir.join(format!("{}.bin", name));
    let runtime_path = tmp_dir.join(format!("{}_runtime.c", name));

    write_tracking_runtime(&runtime_path);

    let mut lexer = Lexer::new(source);
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

    // Point the codegen at the tracked runtime. `codegen::compile`
    // reads `RIVEN_RUNTIME` from the process env, so we hold the
    // lock across the whole compile to keep parallel tests from
    // racing on env state. See `RIVEN_RUNTIME_ENV_LOCK`.
    let compile_result = {
        let _env_guard = RIVEN_RUNTIME_ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        std::env::set_var("RIVEN_RUNTIME", &runtime_path);
        let result = codegen::compile(&mir, bin_path.to_str().unwrap());
        std::env::remove_var("RIVEN_RUNTIME");
        result
    };

    compile_result.expect("codegen failed");

    let output = Command::new(&bin_path).output().expect("run binary");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (stdout, stderr, output.status.code())
}

/// Parse `RIVEN_TEST_LEAK allocs=N frees=M outstanding=K` from stderr.
/// Returns `(allocs, frees, outstanding)`.
fn parse_leak_marker(stderr: &str) -> (u64, u64, u64) {
    let line = stderr
        .lines()
        .find(|l| l.starts_with("RIVEN_TEST_LEAK"))
        .unwrap_or_else(|| panic!("missing RIVEN_TEST_LEAK marker in stderr:\n{}", stderr));
    // line looks like: RIVEN_TEST_LEAK allocs=4 frees=2 outstanding=2
    let mut allocs = None;
    let mut frees = None;
    let mut outstanding = None;
    for tok in line.split_whitespace() {
        if let Some(rest) = tok.strip_prefix("allocs=") {
            allocs = rest.parse().ok();
        } else if let Some(rest) = tok.strip_prefix("frees=") {
            frees = rest.parse().ok();
        } else if let Some(rest) = tok.strip_prefix("outstanding=") {
            outstanding = rest.parse().ok();
        }
    }
    (
        allocs.expect("allocs missing"),
        frees.expect("frees missing"),
        outstanding.expect("outstanding missing"),
    )
}

/// Structured leak report assembled from the three stderr markers
/// emitted by the tracking runtime: `RIVEN_TEST_LEAK` (riven_alloc /
/// riven_dealloc accounting), `RIVEN_TEST_RAW` (raw `malloc` / `free`
/// / `realloc` accounting), and `RIVEN_TEST_KIND` (per-helper
/// breakdown of `riven_string_free` / `riven_vec_free` / `riven_hash_free`).
///
/// `outstanding_allocations` is the **sum** of `riven_alloc`-tracked
/// outstanding plus raw-heap outstanding, so a single zero check
/// covers leaks from either path.
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct LeakReport {
    /// `riven_alloc` calls minus `riven_dealloc` calls — the v1 P0.2
    /// channel for Class/Struct/Enum heap.
    riven_alloc_outstanding: u64,
    /// `malloc`/`realloc(NULL, …)` calls minus `free`/`realloc(…, 0)`
    /// calls — covers the heap path used by `riven_string_from`,
    /// `riven_vec_new`, `riven_hash_new`, and friends.
    raw_outstanding: u64,
    /// Aggregate of `riven_alloc_outstanding` and `raw_outstanding`.
    /// Tests assert this is zero to prove no leaks of either flavour.
    outstanding_allocations: u64,
    /// `riven_string_free` calls observed.
    string_frees: u64,
    /// `riven_vec_free` calls observed.
    vec_frees: u64,
    /// `riven_hash_free` calls observed.
    hash_frees: u64,
    /// `free(v->data)` calls observed inside `riven_vec_free`.
    data_buffer_frees: u64,
    /// `free(entry)` calls observed inside `riven_hash_free`'s bucket
    /// walk.
    entry_frees: u64,
    /// `riven_set_free` calls observed (Phase 2 stdlib #04 batch 2).
    set_frees: u64,
    /// Full stderr from the fixture run, included for diagnostic
    /// formatting in test failure messages.
    stderr: String,
}

/// Find a `KEY=value` token inside a single stderr line and parse it
/// as a `u64`. Panics with the full stderr included if missing or
/// malformed; failures during a red phase should point clearly at
/// which marker line is incomplete.
fn extract_u64(line: &str, key: &str, all_stderr: &str) -> u64 {
    let needle = format!("{}=", key);
    for tok in line.split_whitespace() {
        if let Some(rest) = tok.strip_prefix(&needle) {
            return rest.parse::<u64>().unwrap_or_else(|e| {
                panic!(
                    "could not parse `{}` value `{}` as u64: {}\nstderr:\n{}",
                    key, rest, e, all_stderr
                )
            });
        }
    }
    panic!(
        "missing `{}=…` token in line `{}`\nstderr:\n{}",
        key, line, all_stderr
    );
}

/// Build a `LeakReport` from the three structured markers in stderr.
fn parse_leak_report(stderr: &str) -> LeakReport {
    let leak_line = stderr
        .lines()
        .find(|l| l.starts_with("RIVEN_TEST_LEAK"))
        .unwrap_or_else(|| panic!("missing RIVEN_TEST_LEAK marker in stderr:\n{}", stderr));
    let raw_line = stderr
        .lines()
        .find(|l| l.starts_with("RIVEN_TEST_RAW"))
        .unwrap_or_else(|| panic!("missing RIVEN_TEST_RAW marker in stderr:\n{}", stderr));
    let kind_line = stderr
        .lines()
        .find(|l| l.starts_with("RIVEN_TEST_KIND"))
        .unwrap_or_else(|| panic!("missing RIVEN_TEST_KIND marker in stderr:\n{}", stderr));

    let riven_alloc_outstanding = extract_u64(leak_line, "outstanding", stderr);
    let raw_outstanding = extract_u64(raw_line, "raw_outstanding", stderr);
    LeakReport {
        riven_alloc_outstanding,
        raw_outstanding,
        outstanding_allocations: riven_alloc_outstanding + raw_outstanding,
        string_frees: extract_u64(kind_line, "string_frees", stderr),
        vec_frees: extract_u64(kind_line, "vec_frees", stderr),
        hash_frees: extract_u64(kind_line, "hash_frees", stderr),
        data_buffer_frees: extract_u64(kind_line, "data_buffer_frees", stderr),
        entry_frees: extract_u64(kind_line, "entry_frees", stderr),
        set_frees: extract_u64(kind_line, "set_frees", stderr),
        stderr: stderr.to_string(),
    }
}

/// Compile `source` against the tracking runtime, run the resulting
/// binary, and parse the structured leak markers from its stderr into
/// a `LeakReport`. Used by the P0.7 string/vec/hash drop tests below.
fn run_fixture_inline(name: &str, source: &str) -> LeakReport {
    let (_stdout, stderr, exit) = compile_and_run_with_tracking(name, source);
    assert_eq!(
        exit,
        Some(0),
        "fixture `{}` exited non-zero. stderr:\n{}",
        name,
        stderr
    );
    parse_leak_report(&stderr)
}

/// Regression: the two `Buffer` structs allocated in `main` must be
/// freed by scope-exit dealloc. Before P0.2 this leaked; afterwards
/// `outstanding` must be `0`.
#[test]
fn runtime_no_leak_fixture_exits_without_tracked_leaks() {
    let source = rvn("runtime_no_leak_fixture");
    let (stdout, stderr, exit) = compile_and_run_with_tracking("runtime_no_leak", &source);
    assert_eq!(exit, Some(0), "fixture exited non-zero. stderr: {}", stderr);
    let (allocs, frees, outstanding) = parse_leak_marker(&stderr);
    assert_eq!(
        outstanding, 0,
        "leak detected: allocs={} frees={} outstanding={}\nstdout:\n{}\nstderr:\n{}",
        allocs, frees, outstanding, stdout, stderr
    );
    assert!(
        allocs >= 2,
        "expected at least 2 Buffer allocations, got allocs={}",
        allocs
    );
}

/// Re-binding a heap-owned local must free the prior allocation before
/// the new pointer overwrites it. Three `Buffer.new` calls => three
/// allocations; all three must be freed (two via injected mid-function
/// `riven_dealloc`, one via scope-exit drop). (P0.2)
#[test]
fn reassignment_does_not_leak_prior_heap_value() {
    let source = rvn("reassignment_drop");
    let (_stdout, stderr, exit) = compile_and_run_with_tracking("reassignment_drop", &source);
    assert_eq!(exit, Some(0), "fixture exited non-zero. stderr: {}", stderr);
    let (allocs, frees, outstanding) = parse_leak_marker(&stderr);
    assert_eq!(
        allocs, 3,
        "expected 3 Buffer allocations, got allocs={} stderr={}",
        allocs, stderr
    );
    assert_eq!(
        frees, 3,
        "expected 3 frees, got frees={} stderr={}",
        frees, stderr
    );
    assert_eq!(outstanding, 0);
}

/// A heap-owned local declared inside a `while` body must be freed at
/// every back-edge so it does not leak across iterations. Three
/// iterations => three allocations => three frees. (P0.2)
#[test]
fn loop_body_local_does_not_leak_across_iterations() {
    let source = rvn("loop_body_local");
    let (_stdout, stderr, exit) = compile_and_run_with_tracking("loop_body_local", &source);
    assert_eq!(exit, Some(0), "fixture exited non-zero. stderr: {}", stderr);
    let (allocs, frees, outstanding) = parse_leak_marker(&stderr);
    assert_eq!(allocs, 3, "expected 3 allocs, got {}", allocs);
    assert_eq!(frees, 3, "expected 3 frees, got {}", frees);
    assert_eq!(outstanding, 0);
}

/// `break` inside a loop body must free heap-owned locals declared in
/// the body before jumping to the exit block. (P0.2)
#[test]
fn break_drops_loop_body_local() {
    let source = rvn("break_with_local");
    let (_stdout, stderr, exit) = compile_and_run_with_tracking("break_with_local", &source);
    assert_eq!(exit, Some(0), "fixture exited non-zero. stderr: {}", stderr);
    let (allocs, frees, outstanding) = parse_leak_marker(&stderr);
    assert_eq!(allocs, 1);
    assert_eq!(frees, 1);
    assert_eq!(outstanding, 0);
}

/// `continue` inside a loop body must free heap-owned locals declared
/// in the body before jumping to the loop header. (P0.2)
#[test]
fn continue_drops_loop_body_local() {
    let source = rvn("continue_with_local");
    let (_stdout, stderr, exit) = compile_and_run_with_tracking("continue_with_local", &source);
    assert_eq!(exit, Some(0), "fixture exited non-zero. stderr: {}", stderr);
    let (allocs, frees, outstanding) = parse_leak_marker(&stderr);
    assert_eq!(allocs, 3);
    assert_eq!(frees, 3);
    assert_eq!(outstanding, 0);
}

/// Sanity: the leak tracker itself reports a non-zero `outstanding`
/// when allocations genuinely leak. We construct that scenario by
/// allocating but never letting the local fall out of scope before
/// exit (using `loop` with no terminating return path is not possible
/// in v1 without diverging types, so we instead use a fixture that
/// exercises the dealloc but verifies the count is correct).
///
/// Specifically: this test validates that when 2 allocations happen
/// AND they're freed, the counter shows allocs == frees. The negative
/// case (real leak) is guarded by the regression test above.
#[test]
fn tracker_reports_balanced_allocs_for_dropped_locals() {
    let source = rvn("runtime_no_leak_fixture");
    let (_stdout, stderr, exit) = compile_and_run_with_tracking("tracker_balanced", &source);
    assert_eq!(exit, Some(0));
    let (allocs, frees, outstanding) = parse_leak_marker(&stderr);
    assert_eq!(allocs, frees, "tracker is itself buggy: {}", stderr);
    assert_eq!(outstanding, 0);
}

/* ──────────────────────────────────────────────────────────────────
 * P0.7: String / Vec / HashMap locals must be freed at scope exit.
 *
 * These tests exercise the new heap-deinit helpers
 * (`riven_string_free`, `riven_vec_free`, `riven_hash_free`) and the
 * widened drop-elaboration filter that emits calls to them for
 * `String` / `Vec[_]` / `HashMap[_, _]` typed locals. Pre-coder, the
 * filter excludes those types, so:
 *
 *   * `outstanding_allocations` is non-zero (raw heap leaks observed
 *     via the malloc/free splice), and
 *   * `string_frees` / `vec_frees` / `hash_frees` are zero because the
 *     helpers don't yet exist (or the dispatch never reaches them).
 *
 * Either failure mode is a valid red.
 * ────────────────────────────────────────────────────────────────── */

/// A `String` local bound from `String.from(...)` must be freed by
/// scope-exit drop. Currently the drop-elaboration filter excludes
/// `Ty::String` so the underlying `malloc` from `riven_string_from`
/// leaks through to process exit. (P0.7)
#[test]
fn string_local_is_freed_on_scope_exit() {
    let source = rvn("p07_string_local_drop_source");
    let report = run_fixture_inline("p07_string_local_drop", &source);
    assert_eq!(report.outstanding_allocations, 0, "leak: {:#?}", report);
    assert!(
        report.string_frees >= 1,
        "no string free observed: {:#?}",
        report
    );
}

/// An `Array[_]` local bound from `Array.new` (with at least one push) must
/// have both its struct slot and its `data` buffer freed by scope-exit
/// drop. (P0.7)
#[test]
fn vec_local_is_freed_on_scope_exit() {
    let source = rvn("p07_vec_local_drop_source");
    let report = run_fixture_inline("p07_vec_local_drop", &source);
    assert_eq!(report.outstanding_allocations, 0, "leak: {:#?}", report);
    assert!(report.vec_frees >= 1, "no vec free observed: {:#?}", report);
    assert!(
        report.data_buffer_frees >= 1,
        "no vec data-buffer free observed: {:#?}",
        report
    );
}

/// A `Map[_, _]` local bound from `Map.new` (with at least one
/// insert) must have both its struct slot and its bucket-chain entries
/// freed by scope-exit drop. (P0.7)
#[test]
fn hashmap_local_is_freed_on_scope_exit() {
    let source = rvn("p07_hashmap_local_drop_source");
    let report = run_fixture_inline("p07_hashmap_local_drop", &source);
    assert_eq!(report.outstanding_allocations, 0, "leak: {:#?}", report);
    assert!(
        report.hash_frees >= 1,
        "no hash free observed: {:#?}",
        report
    );
    assert!(
        report.entry_frees >= 1,
        "no hash entry free observed: {:#?}",
        report
    );
}

/* ──────────────────────────────────────────────────────────────────
 * #02 batch 2: stdlib String mutator / consume / operator drop tests.
 *
 * Each fixture exercises one of the surface gaps closed by batch 2:
 *
 *   * `push(Char)` is a mutator that allocates a fresh char* and
 *     rebinds the variable. The prior buffer must not leak.
 *   * `into_bytes` is consuming: the runtime fn frees the source spine,
 *     and the drop pass must NOT also emit `riven_string_free` (that
 *     would double-free, observable as a libc abort or `free()`-twice
 *     in the raw counters going below zero).
 *   * `+` allocates a fresh concat result; both source operands must
 *     end up freed by their owning frame's scope-exit drop.
 * ────────────────────────────────────────────────────────────────── */

/// `String.push(Char)` produces a fresh buffer; the variable rebinds
/// to it, and at scope exit `riven_string_free` must release the
/// final buffer. Outstanding heap must be zero.
#[test]
fn string_push_does_not_leak() {
    let source = rvn("p02b2_string_push_drop_source");
    let report = run_fixture_inline("p02b2_string_push_drop", &source);
    assert_eq!(report.outstanding_allocations, 0, "leak: {:#?}", report);
    assert!(
        report.string_frees >= 1,
        "no string free observed: {:#?}",
        report
    );
}

/// `into_bytes` transfers ownership: the runtime fn frees the source
/// `char*` itself, and the MIR analysis taints the receiver so the
/// drop pass does not also emit `riven_string_free`. Net: no leak,
/// no double-free.
#[test]
fn string_into_bytes_transfers_ownership() {
    let source = rvn("p02b2_string_into_bytes_transfer_source");
    let report = run_fixture_inline("p02b2_string_into_bytes_transfer", &source);
    assert_eq!(
        report.outstanding_allocations, 0,
        "leak (or double-free): {:#?}",
        report
    );
    // The Vec spine still needs freeing at scope exit.
    assert!(report.vec_frees >= 1, "no vec free observed: {:#?}", report);
}

/// `+` on owned Strings allocates a fresh concat buffer. All three
/// owning locals (`a`, `b`, `c`) must be freed by scope-exit drop —
/// the operator does not consume its operands at the language level
/// in v1 (the borrow checker hasn't been tightened for ownership
/// transfer through operators yet), so each retains its own free.
#[test]
fn string_plus_op_frees_both_operands() {
    let source = rvn("p02b2_string_plus_op_drop_source");
    let report = run_fixture_inline("p02b2_string_plus_op_drop", &source);
    assert_eq!(report.outstanding_allocations, 0, "leak: {:#?}", report);
    assert!(
        report.string_frees >= 3,
        "expected ≥3 string frees (a, b, c), got: {:#?}",
        report
    );
}

/* ──────────────────────────────────────────────────────────────────
 * #03 batch 2: Vec[String] / Vec[Vec[T]] element-drop selector.
 *
 * The MIR drop-elaboration picks `riven_vec_drop_string` for
 * `Vec[String]` locals and `riven_vec_drop_vec` for `Vec[Vec[T]]`
 * locals. Each helper walks the slots before releasing the spine, so
 * the per-element heap (the owned `char*` for String slots, the inner
 * `RivenVec*` for Vec slots) gets freed at the same scope exit.
 *
 * The push-time taint (`push_takes_ownership_idx` in
 * `compute_dealloc_safe_locals`) ensures the source `String.from(...)`
 * temp does NOT also get a `riven_string_free` — that would race the
 * vec drop and double-free.
 * ────────────────────────────────────────────────────────────────── */

/// `Array[String]` with three pushed strings: scope-exit drop must free
/// each element (3 string_frees) and the spine (1 vec_free + its
/// data_buffer_free). No leaks, no double-free.
#[test]
fn vec_of_string_releases_every_element() {
    let source = rvn("p03b2_vec_of_string_source");
    let report = run_fixture_inline("p03b2_vec_of_string", &source);
    assert_eq!(
        report.outstanding_allocations, 0,
        "leak (or double-free): {:#?}",
        report
    );
    assert!(
        report.string_frees >= 3,
        "expected ≥3 string frees (one per pushed element), got: {:#?}",
        report
    );
    assert!(
        report.vec_frees >= 1,
        "expected the vec spine to be freed: {:#?}",
        report
    );
}

/// `Array[Array[Int]]` with two inner Arrays pushed as elements. Scope-exit
/// drop must walk the outer slots, free each inner Array spine, then
/// free the outer spine. The element transfer rule prevents the named
/// locals (`row1`, `row2`) from also being freed independently — they
/// transferred ownership at push time.
#[test]
fn vec_of_vec_int_releases_every_inner_vec() {
    let source = rvn("p03b2_vec_of_vec_int_source");
    let report = run_fixture_inline("p03b2_vec_of_vec_int", &source);
    assert_eq!(
        report.outstanding_allocations, 0,
        "leak (or double-free): {:#?}",
        report
    );
    assert!(
        report.vec_frees >= 3,
        "expected ≥3 vec frees (outer + 2 inner), got: {:#?}",
        report
    );
}

/* ──────────────────────────────────────────────────────────────────
 * #04 batch 2: HashMap[K, V] / HashSet[T] per-element drop selectors.
 *
 * Closes the deferred item from prompt 04 DoD: scope-exit drop walks
 * the bucket chains and frees heap-owned keys/values for the four
 * shapes the v1 surface needs: `HashMap[String, V]`, `HashMap[K, String]`,
 * `HashMap[K, Vec[T]]`, `HashSet[String]`. The push-time taint rule
 * (extended in `compute_dealloc_safe_locals` to cover BOTH key and
 * value of `riven_hash_insert`, plus the value of `riven_set_insert`)
 * keeps the source `String.from(...)` / `Vec.new` temps from also
 * being freed independently — that would race the per-element drop
 * and double-free.
 * ────────────────────────────────────────────────────────────────── */

/// `Map[String, Int]` with three inserted entries: scope-exit drop
/// must free each owned key string (3 string_frees), then free the
/// hash spine (1 hash_free + 3 entry_frees).
#[test]
fn p04_hashmap_string_to_int_releases_every_key() {
    let source = rvn("p04b2_hashmap_string_to_int_source");
    let report = run_fixture_inline("p04b2_hashmap_string_to_int", &source);
    assert_eq!(
        report.outstanding_allocations, 0,
        "leak (or double-free): {:#?}",
        report
    );
    assert!(
        report.string_frees >= 3,
        "expected >=3 string frees (one per key), got: {:#?}",
        report
    );
    assert!(
        report.hash_frees >= 1,
        "expected the hash spine to be freed: {:#?}",
        report
    );
}

/// `Map[Int, String]` with three inserted entries: scope-exit drop
/// must free each owned value string (3 string_frees), then free the
/// hash spine.
#[test]
fn p04_hashmap_int_to_string_releases_every_value() {
    let source = rvn("p04b2_hashmap_int_to_string_source");
    let report = run_fixture_inline("p04b2_hashmap_int_to_string", &source);
    assert_eq!(
        report.outstanding_allocations, 0,
        "leak (or double-free): {:#?}",
        report
    );
    assert!(
        report.string_frees >= 3,
        "expected >=3 string frees (one per value), got: {:#?}",
        report
    );
    assert!(
        report.hash_frees >= 1,
        "expected the hash spine to be freed: {:#?}",
        report
    );
}

/// `Map[Int, Array[Int]]` with two inserted Arrays as values. Scope-
/// exit drop must walk the bucket chains, free each inner Array spine
/// (2 vec_frees), then free the outer hash spine. The push-time taint
/// rule prevents the named `row1` / `row2` locals from also being
/// freed independently.
#[test]
fn p04_hashmap_string_to_vec_int_releases_every_value() {
    let source = rvn("p04b2_hashmap_int_to_vec_int_source");
    let report = run_fixture_inline("p04b2_hashmap_int_to_vec_int", &source);
    assert_eq!(
        report.outstanding_allocations, 0,
        "leak (or double-free): {:#?}",
        report
    );
    assert!(
        report.vec_frees >= 2,
        "expected >=2 vec frees (one per value), got: {:#?}",
        report
    );
    assert!(
        report.hash_frees >= 1,
        "expected the hash spine to be freed: {:#?}",
        report
    );
}

/// `Set[String]` with three inserted strings: scope-exit drop must
/// free each owned element string (3 string_frees), then free the set
/// spine (1 set_free).
#[test]
fn p04_hashset_string_releases_every_element() {
    let source = rvn("p04b2_hashset_string_source");
    let report = run_fixture_inline("p04b2_hashset_string", &source);
    assert_eq!(
        report.outstanding_allocations, 0,
        "leak (or double-free): {:#?}",
        report
    );
    assert!(
        report.string_frees >= 3,
        "expected >=3 string frees (one per element), got: {:#?}",
        report
    );
    assert!(
        report.set_frees >= 1,
        "expected the set spine to be freed: {:#?}",
        report
    );
}
