//! Integration tests for the hardened runtime.
//!
//! These tests link against the compiled C runtime and verify that
//! safety-critical operations behave correctly.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// Per-test unique suffix for files in `std::env::temp_dir()`. Cargo
/// runs `#[test]` fns on a thread pool, so any fixed-name temp file
/// collides across parallel tests; we append `(pid, counter)` to keep
/// each test's intermediate files separate.
fn unique_suffix() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}_{}", std::process::id(), n)
}

/// Compile the C runtime to a fresh `.o` file. Each call produces a
/// distinct path so parallel callers don't race on shared output.
/// Walk `library/std/<pkg>/runtime/*.c` to collect every per-package
/// C source. After #06.95 Phase B-2 each `.c` is a standalone TU; the
/// runtime is no longer a single file.
fn collect_runtime_sources() -> Vec<std::path::PathBuf> {
    let crate_dir = env!("CARGO_MANIFEST_DIR");
    let std_root = Path::new(crate_dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("library")
        .join("std");
    let mut sources: Vec<std::path::PathBuf> = Vec::new();
    for pkg in std::fs::read_dir(&std_root).expect("read library/std") {
        let pkg = match pkg {
            Ok(e) => e,
            Err(_) => continue,
        };
        let runtime_dir = pkg.path().join("runtime");
        if !runtime_dir.is_dir() {
            continue;
        }
        for f in std::fs::read_dir(&runtime_dir).expect("read pkg runtime") {
            let f = match f {
                Ok(e) => e,
                Err(_) => continue,
            };
            let p = f.path();
            if p.extension().and_then(|s| s.to_str()) == Some("c") {
                sources.push(p);
            }
        }
    }
    sources.sort();
    sources
}

/// Compile every per-package `.c` to its own `.o`, returning the
/// list. Caller is responsible for cleanup (best-effort
/// `remove_file` on each).
fn compile_runtime_objects(extra_flags: &[&str]) -> Vec<std::path::PathBuf> {
    let sources = collect_runtime_sources();
    let mut objects: Vec<std::path::PathBuf> = Vec::with_capacity(sources.len());
    for src in &sources {
        let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("rt");
        let obj = std::env::temp_dir().join(format!(
            "riven_{}_{}.o",
            stem,
            unique_suffix()
        ));
        let mut cmd = Command::new("cc");
        cmd.arg("-c").arg(src).arg("-o").arg(&obj);
        for f in extra_flags {
            cmd.arg(f);
        }
        let status = cmd.status().expect("failed to invoke cc");
        assert!(
            status.success(),
            "failed to compile {} with flags {:?}",
            src.display(),
            extra_flags
        );
        objects.push(obj);
    }
    objects
}

fn compile_c_harness(name: &str, source: &str) -> PathBuf {
    let runtime_objects = compile_runtime_objects(&["-O2"]);
    let temp_dir = std::env::temp_dir();
    let suffix = unique_suffix();
    let harness_c = temp_dir.join(format!("{name}_{suffix}.c"));
    let harness_exe = temp_dir.join(format!("{name}_{suffix}"));

    std::fs::write(&harness_c, source).expect("write harness");

    // On macOS the runtime's CSPRNG (`library/std/rand/runtime/rand.c`)
    // pulls in `SecRandomCopyBytes` + `kSecRandomDefault` from
    // `Security.framework`. Without the framework flag the link
    // step fails with "_SecRandomCopyBytes referenced from ...".
    // On Linux the equivalents are statically resolved via libc /
    // getentropy(3) — no extra flag needed.
    let mut cmd = Command::new("cc");
    cmd.arg(&harness_c);
    for o in &runtime_objects {
        cmd.arg(o);
    }
    cmd.arg("-o").arg(&harness_exe);
    #[cfg(target_os = "macos")]
    {
        cmd.arg("-framework").arg("Security");
    }
    let status = cmd.status().expect("failed to invoke cc for harness");

    let _ = std::fs::remove_file(&harness_c);
    for o in &runtime_objects {
        let _ = std::fs::remove_file(o);
    }
    assert!(status.success(), "failed to compile C harness {name}");
    harness_exe
}

#[test]
fn runtime_compiles_with_strict_warnings() {
    let objects = compile_runtime_objects(&["-O2", "-Wall", "-Wextra", "-Werror"]);
    for o in &objects {
        let _ = std::fs::remove_file(o);
    }
}

#[test]
fn runtime_compiles_with_sanitizers() {
    let objects =
        compile_runtime_objects(&["-fsanitize=address,undefined", "-g", "-fno-omit-frame-pointer"]);
    for o in &objects {
        let _ = std::fs::remove_file(o);
    }
}

#[test]
fn runtime_env_init_copies_argv_and_clones_reads() {
    let harness = compile_c_harness(
        "riven_runtime_env_argv",
        r#"
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

void riven_env_init(int argc, char **argv);
int64_t riven_env_args_count(void);
char *riven_env_args_at(int64_t index);

int main(void) {
    char arg0[] = "program";
    char arg1[] = "first";
    char *argv[] = {arg0, arg1};

    riven_env_init(2, argv);
    arg1[0] = 'X';

    if (riven_env_args_count() != 2) {
        return 1;
    }

    char *copy1 = riven_env_args_at(1);
    char *copy2 = riven_env_args_at(1);
    if (!copy1 || !copy2) {
        return 2;
    }
    if (strcmp(copy1, "first") != 0) {
        return 3;
    }
    if (copy1 == copy2) {
        return 4;
    }
    if (riven_env_args_at(99) != NULL) {
        return 5;
    }
    return 0;
}
"#,
    );

    let output = Command::new(&harness).output().expect("run harness");
    let _ = std::fs::remove_file(&harness);

    assert!(
        output.status.success(),
        "argv harness failed with status {:?}",
        output.status.code()
    );
}

#[test]
fn runtime_fs_env_and_process_helpers_match_expected_abi() {
    let harness = compile_c_harness(
        "riven_runtime_fs_env_process",
        r#"
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

void *riven_env_var(const char *name);
void *riven_fs_write(const char *path, const char *contents);
void *riven_fs_read_to_string(const char *path);
int64_t riven_fs_exists(const char *path);
void *riven_fs_rename(const char *from, const char *to);
void *riven_fs_remove_file(const char *path);
void *riven_fs_create_dir(const char *path);
void riven_process_exit(int64_t code);

static int is_ok(void *result) {
    return *(int32_t *)result == 0;
}

static int is_err(void *result) {
    return *(int32_t *)result == 1;
}

static const char *payload_str(void *result) {
    return (const char *)((int64_t *)result)[1];
}

int main(int argc, char **argv) {
    const char *root = argv[1];
    char file_a[1024];
    char file_b[1024];
    char dir_a[1024];

    snprintf(file_a, sizeof(file_a), "%s/a.txt", root);
    snprintf(file_b, sizeof(file_b), "%s/b.txt", root);
    snprintf(dir_a, sizeof(dir_a), "%s/dir", root);

    setenv("RIVEN_RUNTIME_ENV_TEST", "expected-value", 1);

    void *env_ok = riven_env_var("RIVEN_RUNTIME_ENV_TEST");
    if (!is_ok(env_ok) || strcmp(payload_str(env_ok), "expected-value") != 0) {
        return 1;
    }

    void *env_err = riven_env_var("RIVEN_RUNTIME_ENV_MISSING");
    if (!is_err(env_err)) {
        return 2;
    }

    if (riven_fs_exists(file_a) != 0) {
        return 3;
    }
    if (!is_ok(riven_fs_write(file_a, "hello runtime"))) {
        return 4;
    }
    if (riven_fs_exists(file_a) != 1) {
        return 5;
    }

    void *read_back = riven_fs_read_to_string(file_a);
    if (!is_ok(read_back) || strcmp(payload_str(read_back), "hello runtime") != 0) {
        return 6;
    }

    if (!is_ok(riven_fs_rename(file_a, file_b))) {
        return 7;
    }
    if (riven_fs_exists(file_a) != 0 || riven_fs_exists(file_b) != 1) {
        return 8;
    }

    if (!is_ok(riven_fs_create_dir(dir_a))) {
        return 9;
    }
    if (!is_ok(riven_fs_remove_file(file_b))) {
        return 10;
    }
    if (riven_fs_exists(file_b) != 0) {
        return 11;
    }

    riven_process_exit(23);
    return argc;
}
"#,
    );

    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let temp_root = std::env::temp_dir().join(format!(
        "riven_runtime_fs_env_process_{}_{}",
        std::process::id(),
        unique
    ));
    let _ = std::fs::remove_dir_all(&temp_root);
    std::fs::create_dir_all(&temp_root).expect("create temp root");

    let output = Command::new(&harness)
        .arg(&temp_root)
        .output()
        .expect("run harness");

    let _ = std::fs::remove_file(&harness);
    let _ = std::fs::remove_dir_all(&temp_root);

    assert_eq!(
        output.status.code(),
        Some(23),
        "process exit harness failed"
    );
}

// ── Property-based tests via proptest ────────────────────────────────────

#[cfg(test)]
mod proptest_tests {
    use proptest::prelude::*;
    use riven_core::lexer::token::{lookup_keyword, TokenKind};
    use riven_core::lexer::Lexer;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        /// Integer literals round-trip across every supported radix.
        ///
        /// For any non-negative i64 value `n`, the lexer must accept its
        /// decimal, hex (`0x`), octal (`0o`), and binary (`0b`) renderings
        /// and recover the same numeric value as an `IntLiteral(n, _)`.
        ///
        /// Falsifiable: this would break if `lex_prefixed_int` mishandled
        /// any radix (e.g. swapped the parse base, dropped digits, or
        /// emitted an off-by-one i64 value).
        #[test]
        fn int_literal_radix_roundtrip(n in 0i64..=i64::MAX) {
            let renderings = [
                format!("{}", n),
                format!("0x{:x}", n),
                format!("0o{:o}", n),
                format!("0b{:b}", n),
            ];

            for src in &renderings {
                let mut lexer = Lexer::new(src);
                let tokens = lexer
                    .tokenize()
                    .map_err(|d| format!("lex failed for {:?}: {:?}", src, d))
                    .unwrap();

                // Expect exactly: IntLiteral, EOF.
                prop_assert_eq!(
                    tokens.len(), 2,
                    "expected one int token + EOF for {:?}, got {} tokens", src, tokens.len()
                );
                match &tokens[0].kind {
                    TokenKind::IntLiteral(v, _) => prop_assert_eq!(
                        *v, n,
                        "rendering {:?} round-tripped to {} instead of {}", src, v, n
                    ),
                    other => prop_assert!(
                        false, "expected IntLiteral for {:?}, got {:?}", src, other
                    ),
                }
                prop_assert_eq!(tokens[1].kind.clone(), TokenKind::Eof);
            }
        }

        /// Non-keyword identifiers lex back to themselves verbatim.
        ///
        /// For a randomly-generated `[a-z_][a-zA-Z0-9_]*` string that is
        /// NOT a reserved keyword, the lexer must emit exactly one
        /// `Identifier(s)` token whose inner string equals the input.
        ///
        /// Falsifiable: this would break if the identifier lexer
        /// truncated, mutated, or mis-cased the input, or if a
        /// non-keyword string was incorrectly classified as a keyword.
        #[test]
        fn identifier_roundtrip(ident in "[a-z_][a-zA-Z0-9_]{0,31}") {
            // Skip strings that happen to be keywords — those legitimately
            // lex to a different token kind.
            prop_assume!(lookup_keyword(&ident).is_none());

            let mut lexer = Lexer::new(&ident);
            let tokens = lexer
                .tokenize()
                .map_err(|d| format!("lex failed for {:?}: {:?}", ident, d))
                .unwrap();

            prop_assert_eq!(
                tokens.len(), 2,
                "expected one identifier + EOF for {:?}, got {} tokens", ident, tokens.len()
            );
            match &tokens[0].kind {
                TokenKind::Identifier(s) => prop_assert_eq!(
                    s.as_str(), ident.as_str(),
                    "identifier round-trip mismatch: input {:?}, got {:?}", ident, s
                ),
                other => prop_assert!(
                    false, "expected Identifier for {:?}, got {:?}", ident, other
                ),
            }
            prop_assert_eq!(tokens[1].kind.clone(), TokenKind::Eof);
        }
    }
}
