//! Ruby-style block builder DSL over `yield self`.
//!
//! Regression cover for the three fixes that make a nested builder DSL
//! (`widget do |w| w.text "x" end`) compile in v1:
//!
//!   1. `yield self` infers the trailing block's parameter type as the
//!      enclosing class (resolve: synthesize the `__block` parameter whose
//!      `self` positions are typed as the class, not a fresh var — methods
//!      went through a cloned signature that lost the link).
//!   2. A method that `yield`s anywhere but its tail expression keeps the
//!      synthetic `__block` return type resolvable (defaulted to Unit; v1
//!      never consumes a block's value).
//!   3. Paren-less method calls with a string argument (`s.line "a"`) parse
//!      as a call, Ruby command-call style.
//!
//! This pin stays in Rust because the fixture prints with `print` (no
//! trailing newline). The single-level `yield self` and paren-less string
//! argument cases are release-e2e fixture 2055.

use crate::common;

fn rx(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruxen")
        .join(format!("{name}.rx"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

/// The full nested builder: `yield self` with inferred block params, a method
/// that yields mid-body, and paren-less string-argument calls — the exact
/// shape quiver's widget DSL relies on.
#[test]
fn yield_self_nested_builder_dsl() {
    let source = rx("yield_self_block_dsl");
    let (stdout, stderr, code) = common::compile_and_run(&source, "yield_self_block_dsl");
    assert_eq!(code, Some(0), "non-zero exit; stderr: {}", stderr);
    // section "[" -> a -> [nested: "[" -> b -> "]"] -> c -> "]"
    assert_eq!(stdout, "[;a;[;b;];c;];", "stdout was {:?}", stdout);
}
