//! B3 pin tests for `docs/specs/system/zero_rust_stdlib_classes.spec.md`
//! — `[system_libs]` aggregation from per-package Riven.toml.
//!
//! Pre-B3 the linker hardcoded `-lc / -lm / -lpthread` in
//! `compiler/riven_core/src/codegen/object.rs::linker_args`. Adding a
//! new stdlib package needing a fresh link-time dep (`-lssl`, etc.)
//! required editing that Rust function. B3 moves the responsibility
//! into each package's `Riven.toml`:
//!
//! ```toml
//! [system_libs]
//! libs = ["pthread", "c", "m"]
//! ```
//!
//! `codegen::collect_system_lib_flags` walks every
//! `library/std/<pkg>/Riven.toml`, parses out the `libs` array, and
//! returns the deduplicated union as `-l<name>` flags. The link step
//! prepends them to the linker command line alongside FFI-attribute
//! flags from `@[link("...")]` directives.

use riven_core::codegen;

#[test]
fn system_libs_aggregate_from_riven_tomls() {
    let flags = codegen::collect_system_lib_flags()
        .expect("collect_system_lib_flags must succeed in the workspace layout");

    // `-lc` / `-lm` come from `library/std/core/Riven.toml`.
    assert!(
        flags.iter().any(|f| f == "-lc"),
        "expected -lc from library/std/core/Riven.toml [system_libs]; got {:?}",
        flags
    );
    assert!(
        flags.iter().any(|f| f == "-lm"),
        "expected -lm from library/std/core/Riven.toml [system_libs]; got {:?}",
        flags
    );
    // `-lpthread` comes from `library/std/sync/Riven.toml`.
    assert!(
        flags.iter().any(|f| f == "-lpthread"),
        "expected -lpthread from library/std/sync/Riven.toml [system_libs]; got {:?}",
        flags
    );
}

#[test]
fn system_libs_are_deduplicated() {
    // The aggregation must not yield duplicate entries even if two
    // packages declare the same lib. Stability of the deduplication
    // is the load-bearing property; the exact iteration order is
    // not — package walk is `read_dir` + sort, which is stable for
    // any given workspace layout but not part of the contract.
    let flags = codegen::collect_system_lib_flags().expect("collect_system_lib_flags");
    let mut sorted = flags.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        flags.len(),
        "collect_system_lib_flags must not return duplicates; got {:?}",
        flags
    );
}

#[test]
fn parse_system_libs_extracts_quoted_string_array() {
    let toml = r#"
[package]
name = "std-foo"

[system_libs]
libs = ["pthread", "c", "m"]
"#;
    let libs = codegen::parse_system_libs(toml);
    assert_eq!(libs, vec!["pthread", "c", "m"]);
}

#[test]
fn parse_system_libs_ignores_other_sections() {
    let toml = r#"
[package]
name = "std-foo"

[dependencies]
std-core = "= 0.1.0"

[system_libs]
libs = ["ssl"]

[other]
libs = ["unused"]
"#;
    let libs = codegen::parse_system_libs(toml);
    assert_eq!(libs, vec!["ssl"]);
}

#[test]
fn parse_system_libs_returns_empty_when_absent() {
    let toml = r#"
[package]
name = "std-foo"
"#;
    let libs = codegen::parse_system_libs(toml);
    assert!(libs.is_empty(), "no [system_libs] → no entries");
}

#[test]
fn parse_system_libs_returns_empty_for_empty_array() {
    let toml = r#"
[system_libs]
libs = []
"#;
    let libs = codegen::parse_system_libs(toml);
    assert!(libs.is_empty());
}

#[test]
fn parse_system_libs_tolerates_inline_comments() {
    let toml = r#"
[system_libs]
libs = ["pthread"]  # only -lpthread needed
"#;
    let libs = codegen::parse_system_libs(toml);
    assert_eq!(libs, vec!["pthread"]);
}
