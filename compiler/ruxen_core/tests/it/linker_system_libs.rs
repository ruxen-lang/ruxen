//! B3 pin tests for `docs/specs/system/zero_rust_stdlib_classes.spec.md`
//! — `[system_libs]` aggregation from per-package Ruxen.toml.
//!
//! Pre-B3 the linker hardcoded `-lc / -lm / -lpthread` in
//! `compiler/ruxen_core/src/codegen/object.rs::linker_args`. Adding a
//! new stdlib package needing a fresh link-time dep (`-lssl`, etc.)
//! required editing that Rust function. B3 moves the responsibility
//! into each package's `Ruxen.toml`:
//!
//! ```toml
//! [system_libs]
//! libs = ["pthread", "c", "m"]
//! ```
//!
//! `codegen::collect_system_lib_flags` walks every
//! `library/std/<pkg>/Ruxen.toml`, parses out the `libs` array, and
//! returns the deduplicated union as `-l<name>` flags. The link step
//! prepends them to the linker command line alongside FFI-attribute
//! flags from `@[link("...")]` directives.

use ruxen_core::codegen;

#[test]
fn system_libs_aggregate_from_ruxen_tomls() {
    let flags = codegen::collect_system_lib_flags()
        .expect("collect_system_lib_flags must succeed in the workspace layout");

    // `-lc` / `-lm` come from `library/std/core/Ruxen.toml`.
    assert!(
        flags.iter().any(|f| f == "-lc"),
        "expected -lc from library/std/core/Ruxen.toml [system_libs]; got {:?}",
        flags
    );
    assert!(
        flags.iter().any(|f| f == "-lm"),
        "expected -lm from library/std/core/Ruxen.toml [system_libs]; got {:?}",
        flags
    );
    // `-lpthread` comes from `library/std/sync/Ruxen.toml`.
    assert!(
        flags.iter().any(|f| f == "-lpthread"),
        "expected -lpthread from library/std/sync/Ruxen.toml [system_libs]; got {:?}",
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

/// `ruxen build` and `ruxen test` link a project through the same
/// `LinkInputs::for_project`: the project's and each dependency's runtime
/// shims, each dependency's `libs` once, and the frameworks of all of them.
#[test]
fn link_inputs_for_project_gather_project_and_dependency_needs() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tmp/linker-system-libs")
        .join(format!("link-inputs-{}", std::process::id()));
    let project = root.join("app");
    let dep_a = root.join("dep_a");
    let dep_b = root.join("dep_b");
    for dir in [&project, &dep_a, &dep_b] {
        std::fs::create_dir_all(dir.join("runtime")).unwrap();
    }
    std::fs::write(
        project.join("Ruxen.toml"),
        "[system_libs]\nframeworks = [\"Cocoa\"]\n",
    )
    .unwrap();
    std::fs::write(project.join("runtime/app.c"), "").unwrap();
    std::fs::write(
        dep_a.join("Ruxen.toml"),
        "[system_libs]\nlibs = [\"pthread\", \"ssl\"]\nframeworks = [\"Cocoa\"]\n",
    )
    .unwrap();
    std::fs::write(dep_a.join("runtime/shim.m"), "").unwrap();
    std::fs::write(
        dep_b.join("Ruxen.toml"),
        "[system_libs]\nlibs = [\"ssl\"]\n",
    )
    .unwrap();

    let inputs = codegen::LinkInputs::for_project(&project, &[dep_a.clone(), dep_b]).unwrap();

    assert_eq!(
        inputs.runtime_sources,
        vec![project.join("runtime/app.c"), dep_a.join("runtime/shim.m")]
    );
    assert_eq!(inputs.system_libs, vec!["pthread", "ssl"]);
    assert_eq!(inputs.frameworks, vec!["Cocoa"]);
    let _ = std::fs::remove_dir_all(&root);
}
