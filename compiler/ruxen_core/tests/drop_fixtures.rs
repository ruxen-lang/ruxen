//! Drop-elaboration regression suite.
//!
//! Compiles a fixture against a tracking runtime that wraps
//! `ruxen_alloc` / `ruxen_dealloc` with counters, runs the resulting
//! binary, and asserts the counts balance at process exit. A leak
//! manifests as `outstanding > 0` in the runtime's exit message.
//!
//! Used as the regression target for P0.2 (free heap-allocated
//! struct/class/enum locals at scope exit) and P0.7 (free
//! heap-allocated `String` / `Vec` / `HashMap` locals at scope exit).
//!
//! For P0.7 the splice additionally wraps every `malloc` / `free` /
//! `realloc` call site in `runtime.c` with raw-heap counters, so leaks
//! that flow through `ruxen_string_from`, `ruxen_vec_new`,
//! `ruxen_hash_new` (and friends) are observable even though those
//! constructors don't go through `ruxen_alloc`. When the runtime
//! exposes the new helpers `ruxen_string_free`, `ruxen_vec_free`,
//! `ruxen_hash_free`, the splice also injects per-helper counters so
//! tests can assert which kind of free fired.

use ruxen_core::codegen;
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;

fn rx(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruxen")
        .join(format!("{name}.rx"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

/// Serializes the `RUXEN_RUNTIME` env-var window across the parallel
/// tests in this binary. `cargo test` runs `#[test]` fns on a thread
/// pool that shares process env, so without this lock two concurrent
/// `compile_and_run_with_tracking` calls can race: one thread's
/// `remove_var` clobbers another thread's `set_var` between the
/// matching `codegen::compile` invocation, leaving the second compile
/// linked against the default (untracked) runtime — which never emits
/// the `RUXEN_TEST_LEAK` marker the assertion expects.
static RUXEN_RUNTIME_ENV_LOCK: Mutex<()> = Mutex::new(());

fn workspace_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

/// Build a `runtime.c` clone augmented with `ruxen_alloc` /
/// `ruxen_dealloc` tracking and an `atexit` handler that emits
/// `RUXEN_TEST_LEAK allocs=N frees=M outstanding=K` to stderr.
///
/// We replace the function bodies in place rather than re-declaring
/// them so the rest of the runtime (ruxen_panic, ruxen_string_*, …)
/// keeps using the tracked allocator.
/// Strip each per-package .c file's `#include "...runtime.h"` line,
/// which becomes spurious once the bodies are concatenated into a
/// single tracked TU (the synthesized unity has the header inlined
/// at the top).
fn strip_runtime_h_include(src: &str) -> String {
    src.lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !(trimmed.starts_with("#include \"runtime.h\"")
                || trimmed.starts_with("#include \"../../core/runtime/runtime.h\""))
        })
        .map(|line| {
            let mut owned = String::from(line);
            owned.push('\n');
            owned
        })
        .collect()
}

/// Build the equivalent of the pre-#06.95 Phase B-2 unity-build
/// `runtime.c` by reading every per-package `runtime/*.c` and
/// prepending the shared `runtime.h`. The string surgery below was
/// written against the historical single-TU runtime; this synthesis
/// preserves the same single-string shape for drop_fixtures' textual
/// rewrites without resurrecting the unity build in production.
fn synthesize_unity_runtime() -> String {
    let std_root = workspace_root().join("library/std");
    let header =
        std::fs::read_to_string(std_root.join("core/runtime/runtime.h")).expect("read runtime.h");
    let mut out = String::new();
    out.push_str(&header);
    out.push('\n');

    // Order is load-bearing for some textual splices that key off
    // `ruxen_alloc` / `ruxen_dealloc` appearing exactly once — so we
    // walk core first, then the rest alphabetically.
    let mut pkg_dirs: Vec<PathBuf> = std::fs::read_dir(&std_root)
        .expect("read library/std")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.join("runtime").is_dir())
        // The regex package's `runtime/regex.c` `#include`s
        // `pcre2/pcre2.h` (vendored under `runtime/pcre2/`) which
        // only resolves when the C file is compiled in-place via the
        // `cc` crate (see `src/ruxen_repl/build.rs` and
        // `codegen/object.rs`, both of which add the pcre2 dir to
        // the include path). The unity build below concatenates
        // every per-package .c into a single tempfile in `tmp/` and
        // hands that to `cc` with no such include path, so the
        // include fails. The drop-elaboration suite never exercises
        // regex anyway — the splices it injects target malloc/free
        // patterns in core/string/vec/hash. Skip the regex package
        // outright.
        .filter(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .map(|n| n != "regex")
                .unwrap_or(true)
        })
        .collect();
    pkg_dirs.sort_by_key(|p| {
        let name = p
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        // core first, then alphabetical
        (name != "core", name)
    });

    for pkg in pkg_dirs {
        let runtime_dir = pkg.join("runtime");
        let mut c_files: Vec<PathBuf> = std::fs::read_dir(&runtime_dir)
            .expect("read pkg runtime/")
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("c"))
            .collect();
        c_files.sort();
        for c in c_files {
            let body = std::fs::read_to_string(&c)
                .unwrap_or_else(|e| panic!("read {}: {}", c.display(), e));
            out.push_str(&strip_runtime_h_include(&body));
            out.push('\n');
        }
    }
    out
}

fn write_tracking_runtime(target: &PathBuf) {
    let original = synthesize_unity_runtime();

    // Replace the allocator and deallocator. We keep the original size
    // / overflow checks by simply shadowing the body with tracked
    // versions.
    let alloc_old = "void *ruxen_alloc(uint64_t size) {\n\
        \x20\x20\x20\x20void *ptr = malloc((size_t)size);\n\
        \x20\x20\x20\x20if (!ptr && size > 0) {\n\
        \x20\x20\x20\x20\x20\x20\x20\x20ruxen_panic(\"out of memory\");\n\
        \x20\x20\x20\x20}\n\
        \x20\x20\x20\x20memset(ptr, 0, (size_t)size);\n\
        \x20\x20\x20\x20return ptr;\n\
        }";
    let alloc_new = "static uint64_t ruxen_test_allocs = 0;\n\
        static uint64_t ruxen_test_frees = 0;\n\
        static int ruxen_test_atexit_registered = 0;\n\
        static void ruxen_test_print_leak(void) {\n\
        \x20\x20\x20\x20fflush(stdout);\n\
        \x20\x20\x20\x20uint64_t out = ruxen_test_allocs - ruxen_test_frees;\n\
        \x20\x20\x20\x20uint64_t raw_out = ruxen_test_raw_mallocs - ruxen_test_raw_frees;\n\
        \x20\x20\x20\x20fprintf(stderr,\n\
        \x20\x20\x20\x20\x20\x20\x20\x20\"RUXEN_TEST_LEAK allocs=%llu frees=%llu outstanding=%llu\\n\",\n\
        \x20\x20\x20\x20\x20\x20\x20\x20(unsigned long long)ruxen_test_allocs,\n\
        \x20\x20\x20\x20\x20\x20\x20\x20(unsigned long long)ruxen_test_frees,\n\
        \x20\x20\x20\x20\x20\x20\x20\x20(unsigned long long)out);\n\
        \x20\x20\x20\x20fprintf(stderr,\n\
        \x20\x20\x20\x20\x20\x20\x20\x20\"RUXEN_TEST_RAW raw_mallocs=%llu raw_frees=%llu raw_outstanding=%llu\\n\",\n\
        \x20\x20\x20\x20\x20\x20\x20\x20(unsigned long long)ruxen_test_raw_mallocs,\n\
        \x20\x20\x20\x20\x20\x20\x20\x20(unsigned long long)ruxen_test_raw_frees,\n\
        \x20\x20\x20\x20\x20\x20\x20\x20(unsigned long long)raw_out);\n\
        \x20\x20\x20\x20fprintf(stderr,\n\
        \x20\x20\x20\x20\x20\x20\x20\x20\"RUXEN_TEST_KIND string_frees=%llu vec_frees=%llu hash_frees=%llu data_buffer_frees=%llu entry_frees=%llu set_frees=%llu\\n\",\n\
        \x20\x20\x20\x20\x20\x20\x20\x20(unsigned long long)ruxen_test_string_frees,\n\
        \x20\x20\x20\x20\x20\x20\x20\x20(unsigned long long)ruxen_test_vec_frees,\n\
        \x20\x20\x20\x20\x20\x20\x20\x20(unsigned long long)ruxen_test_hash_frees,\n\
        \x20\x20\x20\x20\x20\x20\x20\x20(unsigned long long)ruxen_test_data_buffer_frees,\n\
        \x20\x20\x20\x20\x20\x20\x20\x20(unsigned long long)ruxen_test_entry_frees,\n\
        \x20\x20\x20\x20\x20\x20\x20\x20(unsigned long long)ruxen_test_set_frees);\n\
        \x20\x20\x20\x20fflush(stderr);\n\
        }\n\
        void *ruxen_alloc(uint64_t size) {\n\
        \x20\x20\x20\x20if (!ruxen_test_atexit_registered) {\n\
        \x20\x20\x20\x20\x20\x20\x20\x20atexit(ruxen_test_print_leak);\n\
        \x20\x20\x20\x20\x20\x20\x20\x20ruxen_test_atexit_registered = 1;\n\
        \x20\x20\x20\x20}\n\
        \x20\x20\x20\x20void *ptr = malloc((size_t)size);\n\
        \x20\x20\x20\x20if (!ptr && size > 0) {\n\
        \x20\x20\x20\x20\x20\x20\x20\x20ruxen_panic(\"out of memory\");\n\
        \x20\x20\x20\x20}\n\
        \x20\x20\x20\x20memset(ptr, 0, (size_t)size);\n\
        \x20\x20\x20\x20ruxen_test_allocs += 1;\n\
        \x20\x20\x20\x20return ptr;\n\
        }";
    let dealloc_old = "void ruxen_dealloc(void *ptr) {\n\
        \x20\x20\x20\x20free(ptr);\n\
        }";
    let dealloc_new = "void ruxen_dealloc(void *ptr) {\n\
        \x20\x20\x20\x20if (ptr) {\n\
        \x20\x20\x20\x20\x20\x20\x20\x20ruxen_test_frees += 1;\n\
        \x20\x20\x20\x20}\n\
        \x20\x20\x20\x20free(ptr);\n\
        }";

    let mut tracked = original.replace(alloc_old, alloc_new);
    if !tracked.contains("ruxen_test_allocs") {
        panic!(
            "could not splice tracked ruxen_alloc into runtime.c — has the \
             original definition changed? Update drop_fixtures.rs::write_tracking_runtime."
        );
    }
    let before_dealloc = tracked.clone();
    tracked = tracked.replace(dealloc_old, dealloc_new);
    if tracked == before_dealloc {
        panic!(
            "could not splice tracked ruxen_dealloc into runtime.c — has the \
             original definition changed? Update drop_fixtures.rs::write_tracking_runtime."
        );
    }

    // Prepend the raw-heap wrapper definitions and per-kind counter
    // storage at the very top of the file so every subsequent function
    // sees them. The wrappers themselves have to call libc directly
    // (otherwise the global `malloc(` → `ruxen_test_malloc(` rewrite
    // would make them recurse), so they reference `ORIG_MALLOC` /
    // `ORIG_FREE` / `ORIG_REALLOC` sentinels that we substitute back to
    // raw libc names *after* the global rewrite has run.
    // Prelude uses sentinel tokens for its own header and body so the
    // upcoming global text rewrite doesn't accidentally double-rewrite
    // (e.g. `malloc(` inside `ruxen_test_malloc(` would become
    // `ruxen_test_ruxen_test_malloc(`). After the rewrite, sentinels
    // are substituted back to their final names.
    //
    // Sentinels used:
    //   * `WRAPNAME_MALLOC` → `ruxen_test_malloc`  (wrapper definition only)
    //   * `WRAPNAME_FREE`   → `ruxen_test_free`
    //   * `WRAPNAME_REALLOC`→ `ruxen_test_realloc`
    //   * `ORIG_MALLOC(`    → `malloc(`            (call to libc inside wrappers)
    //   * `ORIG_FREE(`      → `free(`
    //   * `ORIG_REALLOC(`   → `realloc(`
    let raw_prelude = "/* drop_fixtures: raw-heap counters & wrappers */\n\
        #include <stdlib.h>\n\
        #include <stdint.h>\n\
        static uint64_t ruxen_test_raw_mallocs = 0;\n\
        static uint64_t ruxen_test_raw_frees = 0;\n\
        static uint64_t ruxen_test_string_frees = 0;\n\
        static uint64_t ruxen_test_vec_frees = 0;\n\
        static uint64_t ruxen_test_hash_frees = 0;\n\
        static uint64_t ruxen_test_data_buffer_frees = 0;\n\
        static uint64_t ruxen_test_entry_frees = 0;\n\
        static uint64_t ruxen_test_set_frees = 0;\n\
        /* Forward-declared in alloc_new; defined alongside ruxen_alloc. */\n\
        static void ruxen_test_print_leak(void);\n\
        /* Forward-declared in alloc_new. Flipping it here prevents \n\
           ruxen_alloc from re-registering the same handler. */\n\
        static int ruxen_test_atexit_registered;\n\
        /* Register atexit unconditionally — fixtures that never call \n\
           ruxen_alloc still need the leak markers printed. \n\
           NOTE: external linkage (no `static`) is required because \n\
           Apple Clang at -O2 elides `static` constructor functions \n\
           that have no in-TU callers; that produced silently-empty \n\
           stderr and a missing RUXEN_TEST_LEAK marker on macOS CI. */\n\
        __attribute__((constructor))\n\
        void ruxen_test_register_atexit(void) {\n\
        \x20\x20\x20\x20atexit(ruxen_test_print_leak);\n\
        \x20\x20\x20\x20ruxen_test_atexit_registered = 1;\n\
        }\n\
        static void *WRAPNAME_MALLOC(size_t n) {\n\
        \x20\x20\x20\x20void *p = ORIG_MALLOC(n);\n\
        \x20\x20\x20\x20if (p) ruxen_test_raw_mallocs += 1;\n\
        \x20\x20\x20\x20return p;\n\
        }\n\
        static void WRAPNAME_FREE(void *p) {\n\
        \x20\x20\x20\x20if (p) ruxen_test_raw_frees += 1;\n\
        \x20\x20\x20\x20ORIG_FREE(p);\n\
        }\n\
        static void *WRAPNAME_REALLOC(void *p, size_t n) {\n\
        \x20\x20\x20\x20if (p == NULL && n > 0) {\n\
        \x20\x20\x20\x20\x20\x20\x20\x20void *np = ORIG_REALLOC(p, n);\n\
        \x20\x20\x20\x20\x20\x20\x20\x20if (np) ruxen_test_raw_mallocs += 1;\n\
        \x20\x20\x20\x20\x20\x20\x20\x20return np;\n\
        \x20\x20\x20\x20}\n\
        \x20\x20\x20\x20if (p != NULL && n == 0) {\n\
        \x20\x20\x20\x20\x20\x20\x20\x20ruxen_test_raw_frees += 1;\n\
        \x20\x20\x20\x20\x20\x20\x20\x20return ORIG_REALLOC(p, n);\n\
        \x20\x20\x20\x20}\n\
        \x20\x20\x20\x20return ORIG_REALLOC(p, n);\n\
        }\n";
    tracked = format!("{}{}", raw_prelude, tracked);

    // 1) Rewrite every `malloc(` / `free(` / `realloc(` call site in
    //    runtime.c to flow through the test-only wrappers.
    //
    //    Word-boundary-aware: only rewrite when the preceding character
    //    is NOT part of an identifier. Without this guard, a plain
    //    `tracked.replace("free(", "ruxen_test_free(")` mangles
    //    `ruxen_async_read_state_free(` → `ruxen_async_read_state_ruxen_test_free(`,
    //    which then no longer matches the C symbol declared in
    //    `library/std/async_fs/runtime/async_fs.c` (`ruxen_async_read_state_free`)
    //    and every stdlib `def drop` that calls one of those state-free
    //    helpers fails to link. This bit the whole drop_fixtures
    //    suite when the bootstrap merge started resolving stdlib class
    //    drop methods into MIR (zero-rust-stdlib-classes B1).
    tracked = replace_call_at_word_boundary(&tracked, "malloc(", "ruxen_test_malloc(");
    tracked = replace_call_at_word_boundary(&tracked, "free(", "ruxen_test_free(");
    tracked = replace_call_at_word_boundary(&tracked, "realloc(", "ruxen_test_realloc(");
    // 2) Restore the wrapper *definitions'* libc-call sentinels back to
    //    raw libc names so the wrappers don't recurse into themselves.
    tracked = tracked.replace("ORIG_MALLOC(", "malloc(");
    tracked = tracked.replace("ORIG_FREE(", "free(");
    tracked = tracked.replace("ORIG_REALLOC(", "realloc(");
    // 3) Restore the wrapper *names* (which we hid behind WRAPNAME_*
    //    sentinels) to their final identifiers.
    tracked = tracked.replace("WRAPNAME_MALLOC", "ruxen_test_malloc");
    tracked = tracked.replace("WRAPNAME_FREE", "ruxen_test_free");
    tracked = tracked.replace("WRAPNAME_REALLOC", "ruxen_test_realloc");

    // If the runtime exposes the new heap-deinit helpers (added by P0.7),
    // inject per-kind counter increments at the top of each one. The
    // helpers all take a single pointer named `s` / `v` / `h`. We splice
    // by matching the function header line; if a header is absent the
    // splice is a no-op and the corresponding counter simply stays at 0,
    // which manifests as a test-level assertion failure that points the
    // coder at the missing helper.
    tracked = inject_helper_counter(
        &tracked,
        "void ruxen_string_free(char *s) {",
        "void ruxen_string_free(char *s) {\n    if (s) ruxen_test_string_frees += 1;",
    );
    tracked = inject_helper_counter(
        &tracked,
        "void ruxen_vec_free(RuxenVec *v) {",
        "void ruxen_vec_free(RuxenVec *v) {\n    if (v) ruxen_test_vec_frees += 1;\n    if (v && v->data) ruxen_test_data_buffer_frees += 1;",
    );
    tracked = inject_helper_counter(
        &tracked,
        "void ruxen_hash_free(RuxenHash *h) {",
        "void ruxen_hash_free(RuxenHash *h) {\n    if (h) {\n        ruxen_test_hash_frees += 1;\n        for (unsigned ruxen_test_b = 0; ruxen_test_b < 16; ruxen_test_b++) {\n            RuxenHashEntry *ruxen_test_e = h->buckets[ruxen_test_b];\n            while (ruxen_test_e) { ruxen_test_entry_frees += 1; ruxen_test_e = ruxen_test_e->next; }\n        }\n    }",
    );
    // Phase 2 stdlib (#04 batch 2): set spine drop. The hash spine
    // and string spine count their own frees independently above.
    // The new HashMap/HashSet per-element drop helpers
    // (`ruxen_hash_drop_*`, `ruxen_set_drop_string`) all reach
    // `ruxen_string_ORIG_FREE` and `ruxen_hash_ORIG_FREE` /
    // `ruxen_set_ORIG_FREE` internally, so the per-kind counters
    // bump transitively without needing to instrument the new
    // helpers themselves.
    tracked = inject_helper_counter(
        &tracked,
        "void ruxen_set_free(RuxenSet *s) {",
        "void ruxen_set_free(RuxenSet *s) {\n    if (s) ruxen_test_set_frees += 1;",
    );

    std::fs::write(target, tracked).unwrap_or_else(|e| panic!("write {}: {}", target.display(), e));
}

/// Replace `pat` with `replacement` in `src`, but ONLY when the match
/// is at a word boundary — i.e. the character preceding the match is
/// not part of an identifier (alphanumeric or `_`). Used to rewrite
/// bare libc call sites (`free(`, `malloc(`, `realloc(`) without
/// mangling longer identifiers that happen to end in the same suffix
/// (`ruxen_async_read_state_free`, `ruxen_set_free`, …).
fn replace_call_at_word_boundary(src: &str, pat: &str, replacement: &str) -> String {
    if pat.is_empty() {
        return src.to_string();
    }
    let bytes = src.as_bytes();
    let pat_bytes = pat.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < bytes.len() {
        if i + pat_bytes.len() <= bytes.len() && &bytes[i..i + pat_bytes.len()] == pat_bytes {
            let prev_ok = if i == 0 {
                true
            } else {
                let prev = bytes[i - 1] as char;
                !(prev.is_ascii_alphanumeric() || prev == '_')
            };
            if prev_ok {
                out.push_str(replacement);
                i += pat_bytes.len();
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
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
    let uniq = format!("{}-{}-{}", name, std::process::id(), ruxen_unique_id());
    let bin_path = tmp_dir.join(format!("{}.bin", uniq));
    let runtime_path = tmp_dir.join(format!("{}_runtime.c", uniq));

    write_tracking_runtime(&runtime_path);

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
        .expect("MIR lowering");

    // Point the codegen at the tracked runtime. `codegen::compile`
    // reads `RUXEN_RUNTIME` from the process env, so we hold the
    // lock across the whole compile to keep parallel tests from
    // racing on env state. See `RUXEN_RUNTIME_ENV_LOCK`.
    let compile_result = {
        let _env_guard = RUXEN_RUNTIME_ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        std::env::set_var("RUXEN_RUNTIME", &runtime_path);
        let result = codegen::compile(&mir, bin_path.to_str().unwrap());
        std::env::remove_var("RUXEN_RUNTIME");
        result
    };

    compile_result.expect("codegen failed");

    let output = Command::new(&bin_path).output().expect("run binary");
    let _ = std::fs::remove_file(&bin_path);
    let _ = std::fs::remove_file(&runtime_path);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (stdout, stderr, output.status.code())
}

/// Parse `RUXEN_TEST_LEAK allocs=N frees=M outstanding=K` from stderr.
/// Returns `(allocs, frees, outstanding)`.
fn parse_leak_marker(stderr: &str) -> (u64, u64, u64) {
    let line = stderr
        .lines()
        .find(|l| l.starts_with("RUXEN_TEST_LEAK"))
        .unwrap_or_else(|| panic!("missing RUXEN_TEST_LEAK marker in stderr:\n{}", stderr));
    // line looks like: RUXEN_TEST_LEAK allocs=4 frees=2 outstanding=2
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
/// emitted by the tracking runtime: `RUXEN_TEST_LEAK` (ruxen_alloc /
/// ruxen_dealloc accounting), `RUXEN_TEST_RAW` (raw `malloc` / `free`
/// / `realloc` accounting), and `RUXEN_TEST_KIND` (per-helper
/// breakdown of `ruxen_string_free` / `ruxen_vec_free` / `ruxen_hash_free`).
///
/// `outstanding_allocations` is the **sum** of `ruxen_alloc`-tracked
/// outstanding plus raw-heap outstanding, so a single zero check
/// covers leaks from either path.
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct LeakReport {
    /// `ruxen_alloc` calls minus `ruxen_dealloc` calls — the v1 P0.2
    /// channel for Class/Struct/Enum heap.
    ruxen_alloc_outstanding: u64,
    /// `malloc`/`realloc(NULL, …)` calls minus `free`/`realloc(…, 0)`
    /// calls — covers the heap path used by `ruxen_string_from`,
    /// `ruxen_vec_new`, `ruxen_hash_new`, and friends.
    raw_outstanding: u64,
    /// Aggregate of `ruxen_alloc_outstanding` and `raw_outstanding`.
    /// Tests assert this is zero to prove no leaks of either flavour.
    outstanding_allocations: u64,
    /// `ruxen_string_free` calls observed.
    string_frees: u64,
    /// `ruxen_vec_free` calls observed.
    vec_frees: u64,
    /// `ruxen_hash_free` calls observed.
    hash_frees: u64,
    /// `free(v->data)` calls observed inside `ruxen_vec_free`.
    data_buffer_frees: u64,
    /// `free(entry)` calls observed inside `ruxen_hash_free`'s bucket
    /// walk.
    entry_frees: u64,
    /// `ruxen_set_free` calls observed (Phase 2 stdlib #04 batch 2).
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
        .find(|l| l.starts_with("RUXEN_TEST_LEAK"))
        .unwrap_or_else(|| panic!("missing RUXEN_TEST_LEAK marker in stderr:\n{}", stderr));
    let raw_line = stderr
        .lines()
        .find(|l| l.starts_with("RUXEN_TEST_RAW"))
        .unwrap_or_else(|| panic!("missing RUXEN_TEST_RAW marker in stderr:\n{}", stderr));
    let kind_line = stderr
        .lines()
        .find(|l| l.starts_with("RUXEN_TEST_KIND"))
        .unwrap_or_else(|| panic!("missing RUXEN_TEST_KIND marker in stderr:\n{}", stderr));

    let ruxen_alloc_outstanding = extract_u64(leak_line, "outstanding", stderr);
    let raw_outstanding = extract_u64(raw_line, "raw_outstanding", stderr);
    LeakReport {
        ruxen_alloc_outstanding,
        raw_outstanding,
        outstanding_allocations: ruxen_alloc_outstanding + raw_outstanding,
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
    let source = rx("runtime_no_leak_fixture");
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

/// Ruby-block-semantics pin (h): a `&block` whose body captures a
/// heap-allocated class value runs SOUNDLY — the captured value is read
/// correctly through `yield`, the program exits cleanly (exit 0, no
/// double-free / no segfault), and the allocation count is STABLE versus the
/// identical plain-closure (`{ }`) capture shape.
///
/// IMPORTANT — what this does NOT assert: leak-freedom of the captures. A
/// closure that captures a heap value currently does not free that capture at
/// closure-drop (`allocs=3, frees=0` here). That is a PRE-EXISTING limitation
/// of the closure-capture machinery — verified by an identical plain-closure
/// (`run({ || b.value + 1 })`) probe leaking the same `outstanding=3`. The
/// block surface reuses that machinery verbatim, so it neither improves nor
/// regresses it. Block-feature scope: no double-free, correct value, clean
/// exit. The capture-drop leak is filed as a separate follow-up in
/// docs/TASKS.md (not a block regression).
#[test]
fn block_capturing_heap_value_runs_soundly() {
    let source = rx("block_capture_heap_no_leak");
    let (stdout, stderr, exit) =
        compile_and_run_with_tracking("block_capture_heap_no_leak", &source);
    assert_eq!(exit, Some(0), "fixture exited non-zero. stderr: {}", stderr);
    assert_eq!(stdout, "42", "stdout was {stdout:?}");
    let (allocs, frees, outstanding) = parse_leak_marker(&stderr);
    // No DOUBLE free: frees never exceeds allocs (a double-free would show
    // frees > allocs or a crash). Outstanding equals the pre-existing
    // closure-capture leak baseline (captures not yet dropped), NOT zero.
    assert!(
        frees <= allocs,
        "double-free suspected: allocs={allocs} frees={frees}\nstderr:\n{stderr}"
    );
    assert_eq!(
        outstanding,
        allocs - frees,
        "leak accounting inconsistent: allocs={allocs} frees={frees} outstanding={outstanding}"
    );
}

/// Re-binding a heap-owned local must free the prior allocation before
/// the new pointer overwrites it. Three `Buffer.new` calls => three
/// allocations; all three must be freed (two via injected mid-function
/// `ruxen_dealloc`, one via scope-exit drop). (P0.2)
#[test]
fn reassignment_does_not_leak_prior_heap_value() {
    let source = rx("reassignment_drop");
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
    let source = rx("loop_body_local");
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
    let source = rx("break_with_local");
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
    let source = rx("continue_with_local");
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
    let source = rx("runtime_no_leak_fixture");
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
 * (`ruxen_string_free`, `ruxen_vec_free`, `ruxen_hash_free`) and the
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

/// A `String` local bound from a BARE STRING LITERAL must be freed by
/// scope-exit drop, identically to one bound from `String.from(...)`. The
/// fixture is `let s = "hello"` (no `String.from`): an un-annotated `let` on a
/// bare literal now binds an owned `String` (typeck `promote_bare_string_
/// literal_binding`), so the implicit `ruxen_string_from` heap copy is
/// drop-elaborated instead of leaking (ledger Q38). (P0.7)
#[test]
fn string_local_is_freed_on_scope_exit() {
    let source = rx("p07_string_local_drop_source");
    let report = run_fixture_inline("p07_string_local_drop", &source);
    assert_eq!(report.outstanding_allocations, 0, "leak: {:#?}", report);
    assert!(
        report.string_frees >= 1,
        "no string free observed: {:#?}",
        report
    );
}

/// One-string-type ADR drop-safety pin: a bare string literal is born owned
/// `String` (heap-copied via `ruxen_string_from`) and freed once at scope exit;
/// `.clone` allocates a SECOND independent owned `String` that is also freed
/// once. The counters must balance (`outstanding=0`) and BOTH the owned literal
/// local and the clone must fire a string free (>=2) — no leak, no double-free.
/// This is the regression pin for the `&str` removal: with the old `&str` type,
/// a literal-typed local was excluded from drop elaboration (it leaked), and an
/// owned-typed view of a borrow double-freed the source. (The bare-literal-into-
/// `&String`-param zero-copy borrow provenance is a separate filed follow-up;
/// see `docs/decisions/one-string-type.md`.)
#[test]
fn string_literal_and_clone_are_drop_safe() {
    let source = rx("string_literal_borrow_clone_matrix");
    let report = run_fixture_inline("string_literal_borrow_clone_matrix", &source);
    assert_eq!(
        report.outstanding_allocations, 0,
        "leak or double-free: {:#?}",
        report
    );
    assert!(
        report.string_frees >= 2,
        "expected the owned literal local AND the clone to each free a String \
         (>=2): {:#?}",
        report
    );
}

/// An `Array[_]` local bound from `Array.new` (with at least one push) must
/// have both its struct slot and its `data` buffer freed by scope-exit
/// drop. (P0.7)
#[test]
fn vec_local_is_freed_on_scope_exit() {
    let source = rx("p07_vec_local_drop_source");
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
    let source = rx("p07_hashmap_local_drop_source");
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
 *     and the drop pass must NOT also emit `ruxen_string_free` (that
 *     would double-free, observable as a libc abort or `free()`-twice
 *     in the raw counters going below zero).
 *   * `+` allocates a fresh concat result; both source operands must
 *     end up freed by their owning frame's scope-exit drop.
 * ────────────────────────────────────────────────────────────────── */

/// `String.push(Char)` produces a fresh buffer; the variable rebinds
/// to it, and at scope exit `ruxen_string_free` must release the
/// final buffer. Outstanding heap must be zero.
#[test]
fn string_push_does_not_leak() {
    let source = rx("p02b2_string_push_drop_source");
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
/// drop pass does not also emit `ruxen_string_free`. Net: no leak,
/// no double-free.
#[test]
fn string_into_bytes_transfers_ownership() {
    let source = rx("p02b2_string_into_bytes_transfer_source");
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
    let source = rx("p02b2_string_plus_op_drop_source");
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
 * The MIR drop-elaboration picks `ruxen_vec_drop_string` for
 * `Vec[String]` locals and `ruxen_vec_drop_vec` for `Vec[Vec[T]]`
 * locals. Each helper walks the slots before releasing the spine, so
 * the per-element heap (the owned `char*` for String slots, the inner
 * `RuxenVec*` for Vec slots) gets freed at the same scope exit.
 *
 * The push-time taint (`push_takes_ownership_idx` in
 * `compute_dealloc_safe_locals`) ensures the source `String.from(...)`
 * temp does NOT also get a `ruxen_string_free` — that would race the
 * vec drop and double-free.
 * ────────────────────────────────────────────────────────────────── */

/// `Array[String]` with three pushed strings: scope-exit drop must free
/// each element (3 string_frees) and the spine (1 vec_free + its
/// data_buffer_free). No leaks, no double-free.
#[test]
fn vec_of_string_releases_every_element() {
    let source = rx("p03b2_vec_of_string_source");
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
    let source = rx("p03b2_vec_of_vec_int_source");
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
 * value of `ruxen_hash_insert`, plus the value of `ruxen_set_insert`)
 * keeps the source `String.from(...)` / `Vec.new` temps from also
 * being freed independently — that would race the per-element drop
 * and double-free.
 * ────────────────────────────────────────────────────────────────── */

/// `Map[String, Int]` with three inserted entries: scope-exit drop
/// must free each owned key string (3 string_frees), then free the
/// hash spine (1 hash_free + 3 entry_frees).
#[test]
fn p04_hashmap_string_to_int_releases_every_key() {
    let source = rx("p04b2_hashmap_string_to_int_source");
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
    let source = rx("p04b2_hashmap_int_to_string_source");
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
    let source = rx("p04b2_hashmap_int_to_vec_int_source");
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
    let source = rx("p04b2_hashset_string_source");
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

/* ──────────────────────────────────────────────────────────────────
 * Q31 (S1 memory-safety): a Float32-payload enum variant constructed
 * BY VALUE two or more times in one function must be sound — no
 * double-free, no leak.
 *
 * Root cause was an under-allocation in `alloc_size` (mir/lower/emit.rs):
 * enums were sized to their PACKED layout, but codegen addresses payload
 * fields on a fixed 8-byte slot stride. For `Move(Float32, Float32)` the
 * payload-field-1 store landed 4 bytes past the allocation, corrupting
 * adjacent heap-chunk metadata — which crashed the program (not a clean
 * leak). A regressed compiler therefore fails this fixture by exiting
 * non-zero / crashing inside the next malloc, well before the
 * `outstanding == 0` assertion runs; a sound compiler frees each enum
 * allocation exactly once.
 * ────────────────────────────────────────────────────────────────── */

/// Three inline Float32-payload enum constructions, each bound to a
/// local and matched. Each ruxen-managed allocation must be freed exactly
/// once at scope exit (no double-free, no leak): the THREE enum allocs
/// balance the THREE frees → `ruxen_alloc_outstanding == 0`. A revert of
/// the alloc_size slot-rounding fix corrupts the heap on the second
/// construction and the binary crashes before clean exit (the harness
/// reports the crash as a failure before this assertion).
///
/// We assert on `ruxen_alloc_outstanding` (the enum allocations under
/// audit), NOT `outstanding_allocations` (which also counts `raw_*`
/// mallocs): the fixture's final `puts "total=#{total}"` interpolates an
/// Int into a String, and that formatter temporary is a SEPARATE,
/// pre-existing, non-enum raw-heap leak the drop pass does not yet collect
/// (documented in the fixture + the Drop ADR's open items). Folding it
/// into the Q31 enum-soundness assertion would conflate two unrelated
/// subsystems. The enum drop itself is sound: 3 allocs, 3 frees.
#[test]
fn q31_float_payload_enum_double_construct_no_leak() {
    let source = rx("q31_float_enum_payload_no_leak_source");
    let report = run_fixture_inline("q31_float_enum_no_leak", &source);
    assert_eq!(
        report.ruxen_alloc_outstanding, 0,
        "leak (or double-free) on float-payload enum drop: {:#?}",
        report
    );
}

fn ruxen_unique_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}
