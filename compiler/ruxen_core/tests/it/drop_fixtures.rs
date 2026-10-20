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

use crate::blocks_procs_lambdas::borrow_codes;
use crate::common;

use ruxen_core::codegen;
use ruxen_core::driver;
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

fn rx(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruxen")
        .join(format!("{name}.rx"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

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

/// Splice a package-internal header (`#include "x.h"` naming a file next
/// to the `.c`, such as `future/runtime/future_rt.h`) into the body. The
/// unity TU is written to `tmp/`, where such a relative include would not
/// resolve. Each header carries an include guard, so splicing it once per
/// including file is harmless.
fn inline_package_headers(src: &str, runtime_dir: &std::path::Path) -> String {
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        let local = line
            .trim_start()
            .strip_prefix("#include \"")
            .and_then(|rest| rest.strip_suffix('"'))
            .map(|name| runtime_dir.join(name))
            .filter(|path| path.is_file());
        match local {
            Some(path) => out.push_str(
                &std::fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("read {}: {}", path.display(), e)),
            ),
            None => out.push_str(line),
        }
        out.push('\n');
    }
    out
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
        // include fails. The splices target malloc/free patterns in
        // core/string/vec/hash, so the regex package is left out of the
        // unity build and linked as separate objects instead (see
        // `regex_runtime_link_flags`).
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
            let body = inline_package_headers(&strip_runtime_h_include(&body), &runtime_dir);
            out.push_str(&body);
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
    //   * `WRAPNAME_CALLOC` → `ruxen_test_calloc`
    //   * `ORIG_MALLOC(`    → `malloc(`            (call to libc inside wrappers)
    //   * `ORIG_FREE(`      → `free(`
    //   * `ORIG_REALLOC(`   → `realloc(`
    //   * `ORIG_CALLOC(`    → `calloc(`
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
        static void *WRAPNAME_CALLOC(size_t n, size_t size) {\n\
        \x20\x20\x20\x20void *p = ORIG_CALLOC(n, size);\n\
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
    // `calloc` too: the formatter and class allocations come from it, and a
    // block counted on its `free` but not on its allocation reads as an
    // over-free.
    tracked = replace_call_at_word_boundary(&tracked, "calloc(", "ruxen_test_calloc(");
    // 2) Restore the wrapper *definitions'* libc-call sentinels back to
    //    raw libc names so the wrappers don't recurse into themselves.
    tracked = tracked.replace("ORIG_MALLOC(", "malloc(");
    tracked = tracked.replace("ORIG_FREE(", "free(");
    tracked = tracked.replace("ORIG_REALLOC(", "realloc(");
    tracked = tracked.replace("ORIG_CALLOC(", "calloc(");
    // 3) Restore the wrapper *names* (which we hid behind WRAPNAME_*
    //    sentinels) to their final identifiers.
    tracked = tracked.replace("WRAPNAME_MALLOC", "ruxen_test_malloc");
    tracked = tracked.replace("WRAPNAME_FREE", "ruxen_test_free");
    tracked = tracked.replace("WRAPNAME_REALLOC", "ruxen_test_realloc");
    tracked = tracked.replace("WRAPNAME_CALLOC", "ruxen_test_calloc");

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
    // The drop glue (`ruxen_vec_drop`, `ruxen_hash_drop`,
    // `ruxen_set_drop`, `ruxen_elem_drop`) reaches
    // `ruxen_string_ORIG_FREE`, `ruxen_vec_ORIG_FREE`,
    // `ruxen_hash_ORIG_FREE` and `ruxen_set_ORIG_FREE` internally, so
    // the per-kind counters bump transitively without instrumenting
    // the glue itself.
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

/// Objects for the regex package (and its vendored PCRE2), which the
/// tracked unity build leaves out, passed to the link as extra inputs.
/// Compiled once per process.
fn regex_runtime_link_flags() -> &'static [String] {
    static OBJECTS: OnceLock<Vec<String>> = OnceLock::new();
    OBJECTS.get_or_init(|| {
        let sources = common::package_runtime_sources(&["regex"]);
        assert!(!sources.is_empty(), "no regex runtime sources found");
        common::compile_runtime_objects(&sources)
            .into_iter()
            .map(|o| o.to_string_lossy().into_owned())
            .collect()
    })
}

fn compile_and_run_with_tracking(name: &str, source: &str) -> (String, String, Option<i32>) {
    let root = workspace_root();
    let tmp_dir = root.join("tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let uniq = format!("{}-{}-{}", name, std::process::id(), ruxen_unique_id());
    let bin_path = tmp_dir.join(format!("{}.bin", uniq));
    let runtime_path = tmp_dir.join(format!("{}_runtime.c", uniq));

    write_tracking_runtime(&runtime_path);

    // Every fixture is a program `ruxen build` accepts: it type-checks,
    // borrow-checks and lowers with its loan check (I-20).
    let program = driver::parse(source).expect("parse");
    let mut frontend = driver::check(&program, &driver::Options::default());
    let Some(mir) = frontend.mir.take() else {
        panic!("`{name}` does not build:\n{}", frontend.render(None));
    };

    // Link against the tracked runtime in place of the default one,
    // passed by path through the production host link. No process env
    // is touched, so tests sharing this process keep the default runtime.
    codegen::compile_with_runtime_sources(
        &mir,
        bin_path.to_str().unwrap(),
        std::slice::from_ref(&runtime_path),
        regex_runtime_link_flags(),
    )
    .expect("compile and link with the tracked runtime");

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

/// A user `let` whose name starts with `_t` is a user binding, not a compiler
/// temporary: `_total` and `_tally` must be freed at scope exit like any
/// other owned local. Temporary-ness comes from `LocalKind`, never the name.
#[test]
fn underscore_t_user_local_is_freed_like_any_binding() {
    let source = rx("underscore_t_user_local_drop");
    let (stdout, stderr, exit) =
        compile_and_run_with_tracking("underscore_t_user_local_drop", &source);
    assert_eq!(exit, Some(0), "fixture exited non-zero. stderr: {stderr}");
    assert_eq!(stdout, "42\n", "stdout was {stdout:?}");
    let (allocs, frees, outstanding) = parse_leak_marker(&stderr);
    assert_eq!(allocs, 2, "expected two Buffer allocations: {stderr}");
    assert_eq!(frees, 2, "both `_t`-named bindings must be freed: {stderr}");
    assert_eq!(outstanding, 0, "leak: {stderr}");
}

/// A brace closure and a `do...end` block each own a by-value heap capture.
/// Dropping the closure must release the capture, captures block, and pair.
#[test]
fn closure_and_block_captures_are_freed_at_closure_drop() {
    for fixture in ["closure_capture_heap_no_leak", "block_capture_heap_no_leak"] {
        let source = rx(fixture);
        let (stdout, stderr, exit) = compile_and_run_with_tracking(fixture, &source);
        assert_eq!(exit, Some(0), "{fixture} exited non-zero. stderr: {stderr}");
        assert_eq!(stdout, "42", "{fixture} stdout was {stdout:?}");
        let (allocs, frees, outstanding) = parse_leak_marker(&stderr);
        assert_eq!(
            allocs, 3,
            "{fixture}: expected capture + block + pair: {stderr}"
        );
        assert_eq!(allocs, frees, "{fixture}: closure capture leaked: {stderr}");
        assert_eq!(
            outstanding, 0,
            "{fixture}: closure capture leaked: {stderr}"
        );
    }
}

/// Two trailing blocks capture the same String. Blocks borrow their captures,
/// so the String is freed once by `main` and never by a block's drop.
#[test]
fn blocks_sharing_a_capture_free_it_once() {
    let source = rx("blocks_share_string_capture");
    let (stdout, stderr, exit) =
        compile_and_run_with_tracking("blocks_share_string_capture", &source);
    assert_eq!(exit, Some(0), "fixture exited non-zero. stderr: {stderr}");
    assert_eq!(stdout, "p-0\np-1\np-0\np-1\np-\n", "stdout was {stdout:?}");
    let report = parse_leak_report(&stderr);
    assert_eq!(report.outstanding_allocations, 0, "leak: {report:#?}");
}

/// A block borrows its captures, so moving one out goes through `.clone`
/// (E1019 otherwise): the block's value, a binding, a push, an array literal,
/// an owning lambda in the block, and a lambda nested in a lambda (it owns a
/// clone made in the outer body). Every clone and every capture is freed once.
#[test]
fn cloned_block_captures_are_freed_once() {
    let source = rx("block_capture_clone_moves_no_leak");
    assert_eq!(borrow_codes(&source), Vec::<String>::new());
    let (stdout, stderr, exit) =
        compile_and_run_with_tracking("block_capture_clone_moves_no_leak", &source);
    assert_eq!(exit, Some(0), "fixture exited non-zero. stderr: {stderr}");
    assert_eq!(
        stdout, "1\ncap-7\n2\n1\ncap-7\ncap-7\n",
        "stdout was {stdout:?}"
    );
    let report = parse_leak_report(&stderr);
    assert_eq!(report.outstanding_allocations, 0, "leak: {report:#?}");
}

/// A capture-less block forwarded through `&block` runs with its `Fn[...]`
/// parameter types (it segfaulted), and a forwarded capturing block borrows
/// its String: every allocation is freed exactly once.
#[test]
fn forwarded_blocks_run_typed_and_free_once() {
    let source = rx("block_forwarded_capture_less");
    let (stdout, stderr, exit) =
        compile_and_run_with_tracking("block_forwarded_capture_less", &source);
    assert_eq!(exit, Some(0), "fixture exited non-zero. stderr: {stderr}");
    assert_eq!(stdout, "1\n2\n1\ns\n2\ns\ns\n", "stdout was {stdout:?}");
    let report = parse_leak_report(&stderr);
    assert_eq!(report.outstanding_allocations, 0, "leak: {report:#?}");
}

/// A closure nested in an owning lambda makes the lambda own the outer
/// String it reads: the String is freed once, by the lambda's drop. A nested
/// owning lambda owns a clone made in the outer body.
#[test]
fn nested_closure_capture_is_owned_by_the_lambda_and_freed_once() {
    let source = rx("nested_closure_capture_owned_once");
    assert_eq!(borrow_codes(&source), Vec::<String>::new());
    let (stdout, stderr, exit) =
        compile_and_run_with_tracking("nested_closure_capture_owned_once", &source);
    assert_eq!(exit, Some(0), "fixture exited non-zero. stderr: {stderr}");
    assert_eq!(stdout, "3\n3\nt1\n", "stdout was {stdout:?}");
    let report = parse_leak_report(&stderr);
    assert_eq!(report.outstanding_allocations, 0, "leak: {report:#?}");
}

/// A lambda passed to an ordinary `Fn[...]` parameter is borrowed: the caller
/// frees the closure pair and the lambda's owned String after both calls.
#[test]
fn lambda_passed_to_fn_param_is_freed_by_the_caller() {
    let source = rx("lambda_to_fn_param_no_leak");
    let (stdout, stderr, exit) =
        compile_and_run_with_tracking("lambda_to_fn_param_no_leak", &source);
    assert_eq!(exit, Some(0), "fixture exited non-zero. stderr: {stderr}");
    assert_eq!(stdout, "4\n5\n", "stdout was {stdout:?}");
    let report = parse_leak_report(&stderr);
    assert_eq!(report.outstanding_allocations, 0, "leak: {report:#?}");
}

/// The `.clone` forms of the moves E1019 rejects in a closure body (a cast, a
/// match binding, a multi-assign value, and a push or binding in an owning
/// lambda called twice) pass the borrow checker and free every clone once.
#[test]
fn cloned_capture_moves_pass_borrow_check_and_free_once() {
    let source = rx("capture_clone_moves_no_leak");
    assert_eq!(borrow_codes(&source), Vec::<String>::new());
    let (stdout, stderr, exit) =
        compile_and_run_with_tracking("capture_clone_moves_no_leak", &source);
    assert_eq!(exit, Some(0), "fixture exited non-zero. stderr: {stderr}");
    assert_eq!(stdout, "cap-7\n2\n2\n2\ncap-7\n", "stdout was {stdout:?}");
    let report = parse_leak_report(&stderr);
    assert_eq!(report.outstanding_allocations, 0, "leak: {report:#?}");
}

/// A single `if let Some(v) = o` on an owned Option[String] moves the payload
/// into `v`, which frees it once: nothing leaks and nothing is freed twice.
#[test]
fn if_let_binding_owns_the_option_payload() {
    let source = rx("if_let_owned_option_no_leak");
    assert_eq!(borrow_codes(&source), Vec::<String>::new());
    let (stdout, stderr, exit) =
        compile_and_run_with_tracking("if_let_owned_option_no_leak", &source);
    assert_eq!(exit, Some(0), "fixture exited non-zero. stderr: {stderr}");
    assert_eq!(stdout, "opt-7\n", "stdout was {stdout:?}");
    let report = parse_leak_report(&stderr);
    assert_eq!(report.outstanding_allocations, 0, "leak: {report:#?}");
}

/// `Some(_)` on a local leaves the payload in place for a later `if let`, and
/// an `if let` on a field only views the field's payload, so neither frees a
/// String something else still owns (both crashed; the field read is a
/// `&Option[String]`, whose payload match typing peels the reference). Only
/// exit and output are pinned: an `Option` box that no by-value binding
/// emptied is never freed yet (TODO.md, Known issues).
#[test]
fn match_on_a_place_keeps_its_payload() {
    let source = rx("match_place_payload_kept");
    assert_eq!(borrow_codes(&source), Vec::<String>::new());
    let (stdout, stderr, exit) = compile_and_run_with_tracking("match_place_payload_kept", &source);
    assert_eq!(exit, Some(0), "fixture exited non-zero. stderr: {stderr}");
    assert_eq!(stdout, "some\nopt-7\nn-3\nn-3\n", "stdout was {stdout:?}");
}

/// Returning a capture moves it out of the closure. Closure drop frees only
/// its pair and captures block; the escaped Buffer stays live until its new
/// owner, the caller's `escaped`, frees it once (a boxed Copy value a closure
/// returns is the caller's box).
#[test]
fn closure_drop_does_not_free_a_moved_out_capture() {
    let source = rx("closure_capture_moved_out_not_freed");
    let (stdout, stderr, exit) =
        compile_and_run_with_tracking("closure_capture_moved_out_not_freed", &source);
    assert_eq!(exit, Some(0), "fixture exited non-zero. stderr: {stderr}");
    assert_eq!(stdout, "42", "escaped capture was invalid: {stderr}");
    let (allocs, frees, outstanding) = parse_leak_marker(&stderr);
    assert_eq!(allocs, 3, "expected capture + block + pair: {stderr}");
    assert_eq!(frees, 3, "block, pair, then the escaped Buffer: {stderr}");
    assert_eq!(outstanding, 0, "the escaped Buffer leaked: {stderr}");
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
/// owned-typed view of a borrow double-freed the source. (The EXPLICIT
/// `&"literal"` borrow is now zero-copy, see `borrow_of_string_literal_is_zero_copy`
/// below; the BARE-literal-into-`&String`-param case still heap-copies then frees
/// it, leak-free but not yet zero-copy, a filed follow-up in
/// `docs/decisions/one-string-type.md`.)
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

/// Zero-copy `&String` borrow of a string literal (TASKS.md, "zero-copy
/// `&String` borrow of a literal"). `&"x"` in an immutable borrow position now
/// lowers to the raw `.rodata` pointer (`emit_raw_string_literal` /
/// `unaryops.rs` Borrow arm), so it allocates NOTHING: the old lowering
/// heap-copied each literal via `ruxen_string_from` into an owned `String`
/// only to `Ref`-borrow it. Assert zero-copy (`string_frees == 0`: no owned
/// String is ever born for the two `&"literal"` borrows, so there is nothing
/// to free) AND leak-free (`outstanding == 0`). The pre-fix lowering would have
/// produced two owned Strings (each freed at scope exit by drop elaboration, so
/// `string_frees` would be 2, not 0); this test's `string_frees == 0` is what
/// proves the borrow is genuinely zero-copy, not merely leak-free.
#[test]
fn borrow_of_string_literal_is_zero_copy() {
    let source = rx("borrow_string_literal_zerocopy");
    let report = run_fixture_inline("borrow_string_literal_zerocopy", &source);
    assert_eq!(
        report.outstanding_allocations, 0,
        "leak on an `&\"literal\"` borrow: {:#?}",
        report
    );
    assert_eq!(
        report.string_frees, 0,
        "expected zero-copy: no owned String should be created for the \
         `&\"literal\"` borrows, so string_frees must be 0: {:#?}",
        report
    );
}

/// Bare-literal zero-copy: a string literal passed DIRECTLY (no `&`) to a
/// `&String` param (`f("x")`, `puts "x"`) also lowers to the raw `.rodata`
/// pointer, gated by `user_callee_param_is_ref` at the free-fn call-arg site.
/// The escape gate guarantees the slot is an immutable `&`-param with no
/// mutation channel, so handing over the immortal `.rodata` is sound. Like the
/// explicit `&"x"` case: no owned String is born, so `string_frees == 0` and
/// nothing leaks.
#[test]
fn bare_string_literal_arg_to_ref_param_is_zero_copy() {
    let source = rx("bare_literal_arg_zerocopy");
    let report = run_fixture_inline("bare_literal_arg_zerocopy", &source);
    assert_eq!(
        report.outstanding_allocations, 0,
        "leak on a bare `\"literal\"` arg to a `&String` param: {:#?}",
        report
    );
    assert_eq!(
        report.string_frees, 0,
        "expected zero-copy: a bare `\"literal\"` passed to a `&String` param \
         must not create an owned String, so string_frees must be 0: {:#?}",
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
 * The MIR drop-elaboration emits `ruxen_vec_drop` with the element
 * descriptor for `Vec[String]` and `Vec[Vec[T]]` locals. It walks
 * the slots before releasing the spine, so
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
 * F-25: nested containers drop recursively.
 *
 * Every container local gets ONE drop callee (`ruxen_vec_drop`,
 * `ruxen_hash_drop`, `ruxen_set_drop`) plus an element descriptor built
 * from its static type, and `ruxen_elem_drop` recurses through it. The
 * old per-type-pair helpers stopped one level down, so the inner
 * strings and arrays below leaked.
 * ────────────────────────────────────────────────────────────────── */

/// `Hash[String, Array[String]]`: the old selector freed the keys only
/// (`ruxen_hash_drop_string_v`), leaking every value Array and its strings.
#[test]
fn hash_of_string_to_array_of_string_frees_values() {
    let source = r#"
def main
  var m: Hash[String, Array[String]] = Hash.new
  var a: Array[String] = Array.new
  a.push("x")
  a.push("y")
  var b: Array[String] = Array.new
  b.push("z")
  m.insert("first", a)
  m.insert("second", b)
  let _len = m.size
end
"#;
    let report = run_fixture_inline("f25_hash_string_to_array_string", source);
    assert_eq!(report.outstanding_allocations, 0, "leak: {:#?}", report);
    assert!(
        report.string_frees >= 5,
        "expected 2 key + 3 element string frees: {:#?}",
        report
    );
    assert!(
        report.vec_frees >= 2,
        "expected both value Arrays freed: {:#?}",
        report
    );
}

/// `Array[Array[String]]` and a deep copy of its tail (`drop(1)` lowers to
/// `ruxen_vec_skip_elem`): the old `ruxen_vec_drop_vec` freed the inner
/// spines only, leaking every string. The copy and the source must each
/// free their own strings, once.
#[test]
fn array_of_array_of_string_frees_inner_strings() {
    let source = r#"
def main
  var grid: Array[Array[String]] = Array.new
  var row1: Array[String] = Array.new
  row1.push("a")
  row1.push("b")
  var row2: Array[String] = Array.new
  row2.push("c")
  grid.push(row1)
  grid.push(row2)
  let tail = grid.drop(1)
  let _n = tail.size
end
"#;
    let report = run_fixture_inline("f25_array_array_string", source);
    assert_eq!(
        report.outstanding_allocations, 0,
        "leak (or double free): {:#?}",
        report
    );
    assert!(
        report.string_frees >= 4,
        "expected 3 source strings and 1 copied string freed: {:#?}",
        report
    );
}

/// A Hash nested in an Array, with an Array nested in the Hash, plus a
/// deep copy of it (`drop(0)`): the descriptor has to carry the key and
/// then the value past it, for the copy and for both drops.
#[test]
fn array_of_hash_of_string_to_array_frees_every_level() {
    let source = r#"
def main
  var outer: Array[Hash[String, Array[Int]]] = Array.new
  var h: Hash[String, Array[Int]] = Hash.new
  var nums: Array[Int] = Array.new
  nums.push(1)
  h.insert("k", nums)
  outer.push(h)
  let copy = outer.drop(0)
  let _len = copy.size
end
"#;
    let report = run_fixture_inline("f25_array_hash_array", source);
    assert_eq!(
        report.outstanding_allocations, 0,
        "leak (or double free): {:#?}",
        report
    );
    assert!(
        report.hash_frees >= 2,
        "expected the inner Hash and its copy freed: {:#?}",
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

// ───────────────────────────────────────────────────────────────────────────
// Borrowed-`&T`-user-param leak matrix (drop-elaboration Fix 1).
//
// A `String` (or any heap type) passed to a USER function/method whose
// parameter is declared `&T` / `&var T` is BORROWED, not consumed. The source
// local must therefore stay drop-eligible and be freed exactly once at its
// own scope exit. The pre-existing bug: `compute_dealloc_safe_locals` routed
// the call arg through the conservative `callee_ownership` default (user
// callee ⇒ taint every arg), suppressing the source's `ruxen_string_free` →
// LEAK. A BY-VALUE (consuming) param must keep the taint so we do not regress
// into a double-free. These fixtures are the oracle for that boundary.
// ───────────────────────────────────────────────────────────────────────────

/// Borrow a `String` into a user free fn's `&String` param. The source `owned`
/// is still live afterwards and must be freed EXACTLY once at scope exit — no
/// leak, no double-free.
#[test]
fn borrow_string_into_user_fn_param_frees_source_once() {
    let source = r#"
def borrow_len(s: &String) -> USize
  s.size
end

def main
  let owned = "transient"
  let _n = borrow_len(owned)
  puts "ok"
end
"#;
    let report = run_fixture_inline("borrow_string_user_fn", source);
    assert_eq!(
        report.outstanding_allocations, 0,
        "borrowed String into user &String param leaked (or double-freed): {:#?}",
        report
    );
    assert!(
        report.string_frees >= 1,
        "the borrowed source String must still be freed once at scope exit: {:#?}",
        report
    );
}

/// Borrow a `String` into a user METHOD's `&String` param (not just a free fn).
/// Same invariant: source freed exactly once.
#[test]
fn borrow_string_into_user_method_param_frees_source_once() {
    let source = r#"
class Greeter
  def init
  end

  def measure(s: &String) -> USize
    s.size
  end
end

def main
  let owned = "hello"
  let g = Greeter.new
  let _n = g.measure(owned)
  puts "ok"
end
"#;
    let report = run_fixture_inline("borrow_string_user_method", source);
    assert_eq!(
        report.outstanding_allocations, 0,
        "borrowed String into user method &String param leaked (or double-freed): {:#?}",
        report
    );
    assert!(
        report.string_frees >= 1,
        "the borrowed source String must still be freed once at scope exit: {:#?}",
        report
    );
}

/// A user param declared BY VALUE (consuming) must KEEP its taint: Fix 1 only
/// untaints `&T` / `&var T` borrow params. Over-rotating into treating every
/// user-call arg as a borrow would re-introduce the taint for by-value args
/// and risk a double-free where one is genuinely consumed. This pin asserts the
/// DANGER boundary: the by-value path runs cleanly (exit 0, no double-free —
/// `frees <= allocs` and no crash) and is UNCHANGED by Fix 1.
///
/// NOTE: a by-value heap arg into a user fn currently *leaks* (the caller
/// taints the source, and the callee skips its params during drop elaboration,
/// so neither side frees it). That caller-frees-by-value-arg gap is PRE-EXISTING
/// and ORTHOGONAL to the `&String`-param filing Fix 1 closes (it is the same
/// "callee never drops a by-value heap param" shape). It is filed separately;
/// this test deliberately does NOT assert `outstanding == 0` so Fix 1 stays
/// scoped to borrow params and cannot mask that gap.
#[test]
fn by_value_string_into_user_fn_keeps_taint_no_double_free() {
    let source = r#"
def take_str(s: String) -> USize
  s.size
end

def main
  let owned = "consumed"
  let _n = take_str(owned)
  puts "ok"
end
"#;
    let (stdout, stderr, exit) = compile_and_run_with_tracking("by_value_string_user_fn", source);
    assert_eq!(exit, Some(0), "by-value fixture crashed. stderr:\n{stderr}");
    assert_eq!(stdout, "ok\n", "unexpected stdout: {stdout:?}");
    let report = parse_leak_report(&stderr);
    // No DOUBLE free: a double-free would show frees > allocs or crash the run.
    // We assert the raw-heap channel never over-frees.
    let raw_line = stderr
        .lines()
        .find(|l| l.starts_with("RUXEN_TEST_RAW"))
        .expect("raw marker");
    let raw_mallocs = extract_u64(raw_line, "raw_mallocs", &stderr);
    let raw_frees = extract_u64(raw_line, "raw_frees", &stderr);
    assert!(
        raw_frees <= raw_mallocs,
        "double-free on by-value path: raw_mallocs={raw_mallocs} raw_frees={raw_frees}\n{:#?}",
        report
    );
}

/// Multiple `&String` borrow args in one call: every borrowed source stays
/// drop-eligible and is freed once.
#[test]
fn multiple_borrow_args_each_source_freed_once() {
    let source = r#"
def concat_len(a: &String, b: &String) -> USize
  a.size + b.size
end

def main
  let first = "alpha"
  let second = "beta"
  let _n = concat_len(first, second)
  puts "ok"
end
"#;
    let report = run_fixture_inline("multi_borrow_args", source);
    assert_eq!(
        report.outstanding_allocations, 0,
        "multiple borrowed String sources leaked or double-freed: {:#?}",
        report
    );
    assert!(
        report.string_frees >= 2,
        "both borrowed sources must each be freed once (>=2): {:#?}",
        report
    );
}

/// Mixed args: one borrowed `&String` (arg0), one by-value `String` (arg1) in
/// the SAME call. Fix 1 must untaint ONLY the borrow slot: the borrowed source
/// `keep` is freed at the caller's scope exit (a string free fires), while the
/// by-value `give` keeps its taint (its pre-existing caller-free gap is
/// orthogonal — see `by_value_string_into_user_fn_keeps_taint_no_double_free`).
/// The point of this pin is that per-argument ref-ness is resolved POSITIONALLY:
/// untainting the whole call (or none of it) would be wrong.
#[test]
fn mixed_borrow_and_by_value_args_untaint_only_the_borrow() {
    let source = r#"
def mix(borrowed: &String, owned: String) -> USize
  borrowed.size + owned.size
end

def main
  let keep = "keep"
  let give = "give"
  let _n = mix(keep, give)
  puts "ok"
end
"#;
    let (stdout, stderr, exit) = compile_and_run_with_tracking("mixed_borrow_value_args", source);
    assert_eq!(exit, Some(0), "mixed fixture crashed. stderr:\n{stderr}");
    assert_eq!(stdout, "ok\n", "unexpected stdout: {stdout:?}");
    let report = parse_leak_report(&stderr);
    // The borrow source `keep` must be freed (untainted borrow arg), while the
    // by-value `give` leaks (the pre-existing by-value gap, raw_outstanding=1).
    // `keep`/`give` are named locals, so the bare-literal zero-copy does not
    // touch them; only the incidental `puts "ok"` literal changed, from a
    // heap-copied owned String to an immortal `.rodata` borrow that allocates
    // and frees nothing. So `keep` is now the SOLE string free (string_frees>=1);
    // before the bare-literal zero-copy the `"ok"` literal also freed.
    assert!(
        report.string_frees >= 1,
        "the borrow-slot source `keep` must be freed (untainted borrow arg): {:#?}",
        report
    );
    assert_eq!(
        report.raw_outstanding, 1,
        "after Fix 1 only the by-value `give` should leak (the borrow `keep` is \
         freed): {:#?}",
        report
    );
    // No double-free on the raw-heap channel.
    let raw_line = stderr
        .lines()
        .find(|l| l.starts_with("RUXEN_TEST_RAW"))
        .expect("raw marker");
    let raw_mallocs = extract_u64(raw_line, "raw_mallocs", &stderr);
    let raw_frees = extract_u64(raw_line, "raw_frees", &stderr);
    assert!(
        raw_frees <= raw_mallocs,
        "double-free on mixed path: raw_mallocs={raw_mallocs} raw_frees={raw_frees}",
    );
}

/// Borrow, then KEEP USING the source after the call. The source is genuinely
/// alive past the borrow; it must be freed once at scope exit.
#[test]
fn borrow_then_use_after_frees_source_once() {
    let source = r#"
def peek(s: &String) -> USize
  s.size
end

def main
  let owned = "lingering"
  let _a = peek(owned)
  let _b = owned.size
  puts "ok"
end
"#;
    let report = run_fixture_inline("borrow_then_use_after", source);
    assert_eq!(
        report.outstanding_allocations, 0,
        "source used after borrow leaked or double-freed: {:#?}",
        report
    );
    assert!(
        report.string_frees >= 1,
        "the still-live source must be freed once: {:#?}",
        report
    );
}

/// CHARACTERIZATION (accepted-leak class, 2026-06-11). Borrow a fresh
/// per-iteration `String` into a user `&String` reader inside a loop body.
///
/// `0741468` registered built-in heap loop-body locals
/// (`String`/`Array`/`Map`/`Set`) for a loop-back-edge free that did NOT
/// consult whether the local had been MOVED OUT during the iteration (rondo's
/// `captures.insert(name, …)`), and freed what a collection now owned. It was
/// reverted, and per-iteration values then leaked until scope exit.
///
/// The drop pass now frees the value such a local held before each
/// redefinition (`drops::free_prior_value_on_rebind`), only for locals the
/// dealloc-safety analysis proves own every value they hold, so a moved-out
/// local is never freed (pinned by
/// `loop_local_moved_into_hash_is_not_freed_on_rebind`). All three
/// per-iteration Strings are freed, and none twice.
#[test]
fn borrow_in_loop_frees_every_iteration_no_uaf() {
    let source = r#"
def borrow_len(s: &String) -> USize
  s.size
end

def main
  var i = 0
  while i < 3
    let owned = "iter"
    let _n = borrow_len(owned)
    i = i + 1
  end
  puts "ok"
end
"#;
    let (stdout, stderr, exit) = compile_and_run_with_tracking("borrow_in_loop", source);
    assert_eq!(
        exit,
        Some(0),
        "borrow-in-loop fixture crashed. stderr:\n{stderr}"
    );
    assert_eq!(stdout, "ok\n", "unexpected stdout: {stdout:?}");
    let report = parse_leak_report(&stderr);
    // No DOUBLE-FREE / UAF: the raw-heap channel never over-frees. This is the
    // load-bearing soundness assertion — the per-iteration allocations may
    // leak, but none is freed while still referenced.
    let raw_line = stderr
        .lines()
        .find(|l| l.starts_with("RUXEN_TEST_RAW"))
        .expect("raw marker");
    let raw_mallocs = extract_u64(raw_line, "raw_mallocs", &stderr);
    let raw_frees = extract_u64(raw_line, "raw_frees", &stderr);
    assert!(
        raw_frees <= raw_mallocs,
        "double-free/UAF on borrow-in-loop path: raw_mallocs={raw_mallocs} \
         raw_frees={raw_frees}\n{:#?}",
        report
    );
    assert_eq!(
        report.outstanding_allocations, 0,
        "every per-iteration String must be freed: {:#?}",
        report
    );
    assert!(
        report.string_frees >= 3,
        "expected one free per iteration: {:#?}",
        report
    );
}

/// Borrow a heap `Array[Int]` into a user fn's `&Array[Int]` param. The Vec
/// spine + data buffer of the source must be freed once — Fix 1 covers all
/// heap types, not just String.
#[test]
fn borrow_array_into_user_fn_frees_source_once() {
    let source = r#"
def total(xs: &Array[Int]) -> Int
  var sum = 0
  xs.each do |x|
    sum = sum + x
  end
  sum
end

def main
  var owned = Array[Int].new
  owned.push(1)
  owned.push(2)
  let _s = total(owned)
  puts "ok"
end
"#;
    let report = run_fixture_inline("borrow_array_user_fn", source);
    assert_eq!(
        report.outstanding_allocations, 0,
        "borrowed Array source leaked or double-freed: {:#?}",
        report
    );
    assert!(
        report.vec_frees >= 1,
        "the borrowed Array source spine must be freed once: {:#?}",
        report
    );
}

/// Borrow a user CLASS value through a `&self` (`Ref`) method receiver. The
/// receiver instance is owned by the CALLER and borrowed by the method, so it
/// must be freed once at scope exit — Fix 1 untaints a `Ref`-self receiver so a
/// class instance passed to its own read-only method no longer leaks.
///
/// NOTE: the sibling spelling — a free fn with a `&Box` PARAMETER —
/// (`def read_box(b: &Box)`) currently fails typeck for a USER class with
/// "could not infer type for parameter `b`" (the `&UserClass` annotation stays
/// an unresolved `Infer`; a stdlib class like `&Json` resolves fine — see
/// release-e2e 800). That is a typeck inference gap ORTHOGONAL to drop
/// elaboration; the `&self` receiver form below exercises the same
/// class-borrow-frees-source drop path Fix 1 added.
#[test]
fn borrow_class_through_ref_self_frees_receiver_once() {
    let source = r#"
class Box
  value: Int

  def init(@value: Int)
  end

  def read -> Int
    self.value
  end
end

def main
  let owned = Box.new(7)
  let _v = owned.read
  puts "ok"
end
"#;
    let report = run_fixture_inline("borrow_class_ref_self", source);
    assert_eq!(
        report.ruxen_alloc_outstanding, 0,
        "class receiver borrowed through &self leaked or double-freed: {:#?}",
        report
    );
}

/// Class elements of an `Array[UserClass]` and the class values of a
/// `Hash[String, UserClass]` are freed when the container drops, and a
/// class's `def drop` runs for each one. Before the OBJ element descriptor
/// (`RUXEN_ELEM_OBJ` + per-type element glue) every element leaked.
#[test]
fn class_elements_and_hash_class_values_are_freed_with_the_container() {
    let source = r##"
class Point
  x: Int
  y: Int

  def init(@x: Int, @y: Int)
  end
end

class Tracked
  id: Int

  def init(@id: Int)
  end

  def drop
    puts "drop #{self.id}"
  end
end

def main
  var pts = [Point.new(1, 2), Point.new(3, 4)]
  pts.push(Point.new(5, 6))
  var h: Hash[String, Point] = Hash.new
  h["a"] = Point.new(7, 8)
  h["b"] = Point.new(9, 10)
  var ts: Array[Tracked] = Array.new
  ts.push(Tracked.new(1))
  ts.push(Tracked.new(2))
  puts "#{pts.size} #{h.size} #{ts.size}"
end
"##;
    let (stdout, stderr, exit) = compile_and_run_with_tracking("class_elements", source);
    assert_eq!(exit, Some(0), "fixture exited non-zero. stderr:\n{stderr}");
    assert_eq!(stdout, "3 2 2\ndrop 1\ndrop 2\n", "stdout: {stdout:?}");
    let report = parse_leak_report(&stderr);
    assert_eq!(
        report.outstanding_allocations, 0,
        "class elements leaked: {:#?}",
        report
    );
}

/// A read element (`a[0]`, `for q in a`) is an alias of a slot the source
/// still owns. Pushed into another collection, the source must not free it
/// too: the source's drop becomes shallow (`drops::demote_escaping_element_drops`).
/// The String spelling aborted with a double free before; the class spelling
/// would have since class elements are freed.
#[test]
fn an_element_alias_stored_elsewhere_is_not_freed_twice() {
    let source = r##"
class P
  x: Int

  def init(@x: Int)
  end
end

def main
  let a = ["x" + "y", "z" + "w"]
  var b: Array[String] = Array.new
  b.push(a[0])
  let ps = [P.new(1), P.new(2)]
  var qs: Array[P] = Array.new
  qs.push(ps[0])
  for q in ps
    qs.push(q)
  end
  puts "#{b.size} #{qs.size}"
end
"##;
    let (stdout, stderr, exit) = compile_and_run_with_tracking("element_alias_escape", source);
    assert_eq!(
        exit,
        Some(0),
        "double free on an escaped element alias. stderr:\n{stderr}"
    );
    assert_eq!(stdout, "1 3\n");
    let report = parse_leak_report(&stderr);
    let raw_line = stderr
        .lines()
        .find(|l| l.starts_with("RUXEN_TEST_RAW"))
        .expect("raw marker");
    let raw_mallocs = extract_u64(raw_line, "raw_mallocs", &stderr);
    let raw_frees = extract_u64(raw_line, "raw_frees", &stderr);
    assert!(raw_frees <= raw_mallocs, "over-free: {:#?}", report);
}

/// String interpolation in a loop: every formatted part (`Formatter#buffer`,
/// the `Int`/`Float`/`Bool` `_fmt` bodies' `to_s` strings), every
/// intermediate `concat` and every per-iteration result is freed.
#[test]
fn interpolation_temporaries_are_freed_in_a_loop() {
    let source = r##"
def main
  var i = 0
  let name = "n" + "m"
  while i < 3
    puts "i=#{i} f=#{1.5} b=#{i > 1} s=#{name}!"
    let label = "#{i}"
    puts label
    i = i + 1
  end
end
"##;
    let report = run_fixture_inline("interpolation_loop", source);
    assert_eq!(
        report.outstanding_allocations, 0,
        "interpolation temporaries leaked: {:#?}",
        report
    );
}

/// String locals inside monomorphised bodies: a bounded generic fn rebuilds a
/// `String` in a loop (`out = out + …`, the shape of the stdlib
/// `Array__mono__T_join`), and a generic class method keeps a `String`
/// local. Every value `out` held before its last is freed on redefinition,
/// and a receiver passed to a `Box__mono__Int_*` method is still freed by
/// its caller (the mono callee name resolves to the generic method's `&self`).
#[test]
fn string_locals_in_monomorphised_bodies_are_freed() {
    let source = r##"
def joined_size[T: Hashable](v: &T, n: Int) -> Int
  var out = ""
  var i = 0
  while i < n
    out = out + "#{v},"
    i = i + 1
  end
  out.size as Int
end

class Box[T]
  item: T

  def init(@item: T)
  end

  def describe -> Int
    let s = "box"
    let t = s + "!"
    t.size as Int
  end
end

def main
  let k = "kk" + "x"
  puts joined_size(&k, 3)
  puts joined_size(&7, 2)
  let b = Box.new(3)
  puts b.describe
end
"##;
    let report = run_fixture_inline("mono_string_locals", source);
    assert_eq!(
        report.outstanding_allocations, 0,
        "String locals in monomorphised bodies leaked: {:#?}",
        report
    );
}

/// A loop-body String moved into a collection each iteration (rondo's
/// `captures.insert(name, …)`) is owned by the collection after the move:
/// the rebind free must not touch it, and the Hash frees every key once.
#[test]
fn loop_local_moved_into_hash_is_not_freed_on_rebind() {
    let source = r##"
def main
  var h: Hash[String, Int] = Hash.new
  var i = 0
  while i < 3
    let k = "k#{i}"
    h[k] = i
    i = i + 1
  end
  puts h.size
end
"##;
    let report = run_fixture_inline("loop_local_moved", source);
    assert_eq!(
        report.outstanding_allocations, 0,
        "moved loop local leaked or double-freed: {:#?}",
        report
    );
}

/// Run `source` against the tracking runtime and require a clean exit (a
/// double free aborts), the expected stdout and zero outstanding
/// allocations of either flavour.
fn assert_runs_leak_free(name: &str, source: &str, expected_stdout: &str) {
    let (stdout, stderr, exit) = compile_and_run_with_tracking(name, source);
    assert_eq!(exit, Some(0), "`{name}` exited non-zero. stderr:\n{stderr}");
    assert_eq!(stdout, expected_stdout, "`{name}` stdout");
    let report = parse_leak_report(&stderr);
    assert_eq!(
        report.outstanding_allocations, 0,
        "`{name}` leaked: {report:#?}"
    );
}

/// An object owns its fields: when it dies its String, Array[String] and
/// nested-object fields are freed after its `def drop` runs, whether it dies
/// at scope exit, at a loop back-edge, on rebinding, or as an element of a
/// collection.
#[test]
fn an_object_frees_its_fields_when_it_dies() {
    let source = r##"
class Address
  city: String

  def init(@city: String)
  end
end

class User
  name: String
  tags: Array[String]
  home: Address

  def init(@name: String, @tags: Array[String], @home: Address)
  end
end

class Noisy
  label: String

  def init(@label: String)
  end

  def drop
    puts "drop #{@label}"
  end
end

total = 0
i = 0
while i < 20
  u = User.new("u#{i}", ["a#{i}", "b#{i}"], Address.new("c#{i}"))
  total = total + (u.tags.size as Int)
  i = i + 1
end
r = User.new("r1", ["x"], Address.new("y"))
r = User.new("r2", ["z"], Address.new("w"))
users = Array[User].new
users.push(User.new("e", ["f"], Address.new("g")))
n = Noisy.new("n" + "1")
puts "#{total} #{r.home.city} #{users.size}"
"##;
    assert_runs_leak_free("object_fields", source, "40 w 1\ndrop n1\n");
}

/// Storing into a field frees the value it held before: rebuilding a
/// String field in a loop (`rep.text = rep.text + w`) leaks none of the
/// intermediate values.
#[test]
fn storing_into_a_field_frees_the_previous_value() {
    let source = r##"
class Report
  text: String
  words: Array[String]

  def init
    @text = ""
    @words = []
  end
end

rep = Report.new
i = 0
while i < 30
  w = "w#{i}"
  rep.text = rep.text + w
  rep.words.push(w.clone)
  i = i + 1
end
puts "#{rep.text.size} #{rep.words.size}"
"##;
    assert_runs_leak_free("field_overwrite", source, "80 30\n");
}

/// `rep.words = rep.words.select { … }` keeps aliases of the old Array's
/// elements: freeing the old value, and later the field, must not free a
/// kept element twice (the field's drops become shallow,
/// `drops::demote_escaping_element_drops`).
#[test]
fn a_field_element_kept_by_its_replacement_is_not_freed_twice() {
    let source = r##"
class Report
  words: Array[String]

  def init
    @words = []
  end
end

rep = Report.new
i = 0
while i < 30
  rep.words.push("w#{i}")
  i = i + 1
end
rep.words = rep.words.select { |w| w.size < 3 }
puts rep.words.size
"##;
    let (stdout, stderr, exit) = compile_and_run_with_tracking("field_element_alias", source);
    assert_eq!(
        exit,
        Some(0),
        "double free of a kept element. stderr:\n{stderr}"
    );
    assert_eq!(stdout, "10\n");
    let raw_line = stderr
        .lines()
        .find(|l| l.starts_with("RUXEN_TEST_RAW"))
        .expect("raw marker");
    let raw_mallocs = extract_u64(raw_line, "raw_mallocs", &stderr);
    let raw_frees = extract_u64(raw_line, "raw_frees", &stderr);
    assert!(raw_frees <= raw_mallocs, "over-free:\n{stderr}");
}

/// A derived `clone` copies every field deeply, so the original and the
/// copy each free only their own fields, in either order.
#[test]
fn a_derived_clone_owns_independent_fields() {
    let source = r##"
class Inner
  label: String

  def init(@label: String)
  end
end

class Outer
  name: String
  items: Array[String]
  inner: Inner

  def init(@name: String, @items: Array[String], @inner: Inner)
  end
end

a = Outer.new("a" + "b", ["c" + "d"], Inner.new("e" + "f"))
b = a.clone
c = b.clone
puts "#{a.name} #{b.items.size} #{c.inner.label}"
"##;
    assert_runs_leak_free("derived_clone_fields", source, "ab 1 ef\n");
}

/// A generic class instance frees the fields its type arguments make owned,
/// and a subclass frees the String field it inherits from its parent.
#[test]
fn generic_and_inherited_fields_are_freed() {
    let source = r##"
class Box[T]
  item: T
  note: String

  def init(@item: T, @note: String)
  end

end

class Animal
  name: String

  def init(@name: String)
  end
end

class Dog < Animal
  trick: String

  def init(@name: String, @trick: String)
  end
end

b = Box.new("x" + "y", "note")
b.item = "z" + "w"
n = Box.new(7, "seven")
d = Dog.new("rex", "sit")
puts "#{b.note} #{n.note} #{d.name} #{d.trick}"
"##;
    assert_runs_leak_free("generic_inherited_fields", source, "note seven rex sit\n");
}

/// A lambda stored in a field moves into the object, which frees it (its
/// captured String, captures box and pair) exactly once when it dies.
#[test]
fn an_object_frees_the_lambda_in_its_field() {
    let source = r##"
class Holder
  cb: Proc[() -> nil]

  def init(@cb: Proc[() -> nil])
  end

  def fire -> nil
    @cb.()
  end
end

name = "ali" + "ce"
h = Holder.new(-> { puts name })
h.fire
h.fire
"##;
    assert_runs_leak_free("closure_field", source, "alice\nalice\n");
}

/// Lambdas pushed into an Array move into it; the Array's drop frees each
/// one through the pair's own drop slot (the CLOSURE element tag), also
/// after they were called through a `for` loop binding.
#[test]
fn an_array_frees_the_lambdas_it_holds() {
    let source = r##"
hs = Array[Proc[(Int) -> nil]].new
a = "a" + "1"
b = "b" + "2"
c = "c" + "3"
hs.push(->(n: Int) { puts "#{a}-#{n}" })
hs.push(->(n: Int) { puts "#{b}-#{n}" })
hs.push(->(n: Int) { puts "#{c}-#{n}" })
for h in hs
  h.(7)
end
puts hs.size
"##;
    assert_runs_leak_free("closure_array", source, "a1-7\nb2-7\nc3-7\n3\n");
}

/// The Bag shape: a user `each(&block)` iterating its own `@items` with
/// `for` and calling the block only lends each element to the block, so
/// `@items` keeps a deep drop. Two blocks borrow a shared prefix; a lambda
/// and a proc own theirs.
#[test]
fn a_block_called_from_a_for_loop_borrows_the_elements() {
    let source = r##"
class Bag
  items: Array[String]

  def init
    @items = ["a" + "", "b" + ""]
  end

  def each(&block: Fn[(String) -> nil]) -> nil
    for s in @items
      block.(s)
    end
  end
end

prefix = "p-"
bag = Bag.new
bag.each { |n| puts prefix + n }
bag.each { |n| puts prefix + n }
puts prefix
tag = "tag-".clone
f = -> { puts tag }
f.()
g = proc { puts "proc" }
g.()
"##;
    assert_runs_leak_free(
        "bag_each_block",
        source,
        "p-a\np-b\np-a\np-b\np-\ntag-\nproc\n",
    );
}

/// A Hash frees the lambda and proc values it holds, also after one was
/// called through an index read.
#[test]
fn a_hash_frees_the_procs_it_holds() {
    let source = r##"
routes = Hash[String, Proc[(Int) -> nil]].new
x = "x" + "!"
y = "y" + "?"
routes["get"] = ->(n: Int) { puts "#{x}#{n}" }
routes["put"] = proc { |n: Int| puts "#{y}#{n}" }
routes["put"].(3)
puts routes.size
"##;
    assert_runs_leak_free("closure_hash", source, "y?3\n2\n");
}

/// `.clone` of a Proc shares it: every copy (an explicit `.clone` of a
/// field read, a derived clone of the object, a copied Array) holds a
/// counted reference, and the pair is freed once, by its last owner.
#[test]
fn a_shared_proc_is_freed_once_by_its_last_owner() {
    let source = r##"
class Policy
  pred: Proc[(Int) -> Bool]
  hooks: Array[Proc[() -> nil]]

  def init(@pred: Proc[(Int) -> Bool], @hooks: Array[Proc[() -> nil]])
  end
end

limit = "lim" + "it"
hooks = Array[Proc[() -> nil]].new
hooks.push(-> { puts limit })
p = Policy.new(->(n: Int) { n > 2 }, hooks)
q = Policy.new(p.pred.clone, p.hooks.drop(0))
r = q.clone
for h in r.hooks
  h.()
end
puts "#{p.pred.(3)} #{q.pred.(1)} #{r.pred.(5)}"
"##;
    assert_runs_leak_free("shared_proc", source, "limit\ntrue false true\n");
}

/// Heap statement temporaries are freed at the end of their statement:
/// a `String` or class instance returned by a user function and only used
/// inside one statement (`puts(label(i))`, `boxed(i).value`), and a
/// constructor temporary (`Parcel.new(i).value`). User function results used
/// to be treated as borrowed, so every one of them leaked, once per loop
/// iteration.
#[test]
fn heap_statement_temporaries_are_freed() {
    let source = r##"
class Parcel
  n: Int

  def init(@n: Int)
  end

  def value -> Int
    @n
  end
end

def label(n: Int) -> String
  "v#{n}"
end

def boxed(n: Int) -> Parcel
  Parcel.new(n)
end

i = 0
while i < 3
  puts label(i)
  puts boxed(i).value
  puts Parcel.new(i).value
  i = i + 1
end
kept = label(9)
puts kept
"##;
    let (stdout, stderr, exit) = compile_and_run_with_tracking("heap_stmt_temps", source);
    assert_eq!(exit, Some(0), "fixture exited non-zero. stderr:\n{stderr}");
    assert_eq!(stdout, "v0\n0\n0\nv1\n1\n1\nv2\n2\n2\nv9\n");
    let report = parse_leak_report(&stderr);
    assert_eq!(
        report.outstanding_allocations, 0,
        "heap statement temporaries leaked: {report:#?}"
    );
}

/// A temporary of a class with a `def drop` runs its drop at the end of the
/// statement that created it, and its storage is freed; a `let`-bound value
/// is dropped at scope exit. Before, the constructor temporary was dropped
/// at function exit and the function-returned one never.
#[test]
fn drop_class_statement_temporary_drops_at_statement_end() {
    let source = r##"
class Noisy
  n: Int

  def init(@n: Int)
  end

  def drop
    puts "drop #{@n}"
  end

  def value -> Int
    @n
  end
end

def make(n: Int) -> Noisy
  Noisy.new(n)
end

puts make(1).value
puts "after 1"
puts Noisy.new(2).value
puts "after 2"
kept = make(3)
puts "kept #{kept.value}"
"##;
    let (stdout, stderr, exit) = compile_and_run_with_tracking("drop_class_stmt_temp", source);
    assert_eq!(exit, Some(0), "fixture exited non-zero. stderr:\n{stderr}");
    assert_eq!(
        stdout,
        "1\ndrop 1\nafter 1\n2\ndrop 2\nafter 2\nkept 3\ndrop 3\n"
    );
    let (_, _, outstanding) = parse_leak_marker(&stderr);
    assert_eq!(outstanding, 0, "drop-class temporaries leaked: {stderr}");
}

/// A binding that borrows a temporary's field or result (`U.new(..).name`,
/// `mk(4).name`, `U.new(..).label`, `pick(U.new(..))`) keeps
/// the temporary alive while the binding is used, then frees it once, in a
/// loop body every iteration (I-15). The temporary used to be freed at the
/// end of its statement while the binding still pointed into it.
#[test]
fn borrow_of_a_temporary_keeps_it_alive_then_frees_it() {
    let source = rx("temporary_root_borrow_kept_alive");
    let (stdout, stderr, exit) =
        compile_and_run_with_tracking("temporary_root_borrow_kept_alive", &source);
    assert_eq!(exit, Some(0), "fixture exited non-zero. stderr:\n{stderr}");
    assert_eq!(
        stdout,
        "tmp-3\nmade-4\nlbl-3\nin-fn-5\np-3\nloop-0\nloop-1\nloop-2\n"
    );
    let report = parse_leak_report(&stderr);
    assert_eq!(report.outstanding_allocations, 0, "leak: {report:#?}");
}

/// A `&var String` parameter is the caller's slot: `push_str` and an
/// assignment in the callee replace the caller's buffer, the old buffer is
/// freed exactly once, and the caller frees the final one. Reading the
/// parameter used to treat the slot address as the String.
#[test]
fn var_string_param_writes_neither_leak_nor_double_free() {
    // A field or element argument, explicit (`&var u.name`) or auto-borrowed
    // (`u.name`), is evaluated once and written back without freeing the
    // value the callee already replaced through the slot (I-17, i1).
    let source = r##"
def grow(s: &var String) -> nil
  s.push_str("!")
  puts "#{s} #{s.size}"
end

def replace(s: &var String) -> nil
  s = "new"
end

class U
  name: String

  def init(@name: String)
  end
end

s = "hi"
grow(s)
grow(s)
replace(s)
puts s

k = 3
u = U.new("old-#{k}")
replace(&var u.name)
puts u.name
replace(u.name)
puts u.name
grow(&var u.name)
grow(u.name)

a = ["e-#{k}", "f-#{k}"]
replace(&var a[0])
replace(a[1])
puts a[0]
puts a[1]
grow(&var a[0])
grow(a[1])
"##;
    let (stdout, stderr, exit) = compile_and_run_with_tracking("var_string_param", source);
    assert_eq!(exit, Some(0), "fixture exited non-zero. stderr:\n{stderr}");
    assert_eq!(
        stdout,
        "hi! 3\nhi!! 4\nnew\nnew\nnew\nnew! 4\nnew!! 5\nnew\nnew\nnew! 4\nnew! 4\n"
    );
    let report = parse_leak_report(&stderr);
    assert_eq!(
        report.outstanding_allocations, 0,
        "&var String writes leaked or double-freed: {report:#?}"
    );
}

/// A `def var` String method on a field borrowed with `&var` grows the
/// field's own buffer through the address of the field's slot (P-8,
/// `FieldAddr`), so the field never keeps the freed buffer.
#[test]
fn var_field_receiver_grows_the_field_in_place() {
    let source = rx("var_field_receiver_writeback");
    let (stdout, stderr, exit) =
        compile_and_run_with_tracking("var_field_receiver_writeback", &source);
    assert_eq!(exit, Some(0), "fixture exited non-zero. stderr:\n{stderr}");
    assert_eq!(
        stdout,
        "old-3-a-sufficiently-long-suffix-to-force-realloc-of-the-buffer\n64\n"
    );
    let report = parse_leak_report(&stderr);
    assert_eq!(report.outstanding_allocations, 0, "leak: {report:#?}");
}

/// Writes to a place once the borrows of it have ended (the loan matrix's
/// "borrow dead before the write" rows): `u.name += t` grows the field's
/// own buffer through the address of its slot, a parallel
/// assignment to a field, rebinding an object after a closure literal (the
/// old object is freed: the closure body no longer drops the enclosing
/// function's heap locals), a `for` loop writing a field it does not walk,
/// and two `&var` arguments on disjoint fields of one object.
#[test]
fn place_writes_after_borrows_neither_leak_nor_double_free() {
    let source = rx("place_writes_after_borrows");
    let (stdout, stderr, exit) =
        compile_and_run_with_tracking("place_writes_after_borrows", &source);
    assert_eq!(exit, Some(0), "fixture exited non-zero. stderr:\n{stderr}");
    assert_eq!(
        stdout,
        "old-3\nold-3-more\nmulti-3 2\nmulti-3\nold-5\ne0-5\ne1-5\n2\nboth-s both-t\n"
    );
    let report = parse_leak_report(&stderr);
    assert_eq!(report.outstanding_allocations, 0, "leak: {report:#?}");
}

/// A closure stored through a type parameter (`Holder.new(-> { 7 })`) is a
/// stored proc in the specialized body (`Ty::as_type_argument`): the
/// constructor owns it and the field frees it once. The specialized `T`
/// parameter used to read as a borrowed block slot, so the caller freed the
/// closure as well (a double free under Guard Malloc, review probe w2).
#[test]
fn a_closure_stored_through_a_type_parameter_is_freed_once() {
    let source = rx("generic_class_stores_lambda");
    let (stdout, stderr, exit) =
        compile_and_run_with_tracking("generic_class_stores_lambda", &source);
    assert_eq!(exit, Some(0), "fixture exited non-zero. stderr:\n{stderr}");
    assert_eq!(stdout, "x\n7\n");
    let report = parse_leak_report(&stderr);
    assert_eq!(report.outstanding_allocations, 0, "leak: {report:#?}");
}

fn ruxen_unique_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// A `&var` place points at the real storage (I-24, P-11): a `&var` field
/// argument, a `r = &var u.name` binding and its forwarding, a scalar field
/// borrowed `&var`, and a `&var` of a captured variable inside a block, a
/// lambda or a proc (explicit, auto-borrowed, or a `def var` receiver) all
/// write the variable itself. No detached copy is freed under the real
/// storage, and nothing leaks.
#[test]
fn var_places_point_at_the_real_storage() {
    let source = rx("var_places_point_at_storage");
    let (stdout, stderr, exit) =
        compile_and_run_with_tracking("var_places_point_at_storage", &source);
    assert_eq!(exit, Some(0), "fixture exited non-zero. stderr:\n{stderr}");
    let long = "replaced-a-long-enough-string-to-realloc";
    let more = "-more-more-more-more-more-more-more";
    assert_eq!(
        stdout,
        format!(
            "bound-3\n{long}\n21\n{long}\n{long}\npush-3{more}\n5\n{long}\n{long}\nlpush-3{more}\n{long}\n"
        )
    );
    let report = parse_leak_report(&stderr);
    assert_eq!(report.outstanding_allocations, 0, "leak: {report:#?}");
}

/// `op=` on an Array element updates the element in its own slot (P-12; it
/// used to be a silent no-op): a String element appends in place, an Int
/// or Float element is read, combined and stored back, and a `def var`
/// method on an element mutates it. Nothing leaks.
#[test]
fn compound_assignment_updates_an_array_element() {
    let source = rx("element_compound_assign");
    let (stdout, stderr, exit) = compile_and_run_with_tracking("element_compound_assign", &source);
    assert_eq!(exit, Some(0), "fixture exited non-zero. stderr:\n{stderr}");
    let s = "x-3-more-more-more-more-more-more";
    assert_eq!(stdout, format!("{s}\n{s}!\n6\n6\n2.5\n"));
    let report = parse_leak_report(&stderr);
    assert_eq!(report.outstanding_allocations, 0, "leak: {report:#?}");
}

/// `op=` on a place that cannot be updated in place (a Hash value) is a
/// lowering error, never a silent no-op (P-12).
#[test]
fn compound_assignment_to_a_hash_value_is_rejected() {
    let source = "h = {\"a\" => 1}\nh[\"a\"] += 1\nputs(h[\"a\"])\n";
    let program = Parser::new(Lexer::new(source).tokenize().expect("lex"))
        .parse()
        .expect("parse");
    let result = typeck::type_check(&program);
    let err = Lowerer::new(&result.symbols)
        .lower_program(&result.program)
        .expect_err("a compound assignment to a Hash value must not lower");
    assert!(err.contains("cannot update this place in place"), "{err}");
}
