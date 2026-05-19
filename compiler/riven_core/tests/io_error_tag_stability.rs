//! Pin test for the `IoError` variant-tag stability contract.
//!
//! The runtime (`library/std/io/runtime/io_error.c`) and the self-hosted
//! stdlib source (`library/std/io/src/lib.rvn`) co-document the tag
//! indices for each `IoError` variant. If they drift, runtime-
//! returned errors will be misinterpreted at the typeck layer and
//! `match` arms will silently miss. This test grep-extracts the
//! `RIVEN_IO_ERROR_*` defines from the C source and the
//! declaration order from the `enum IoError ... end` block in the
//! .rvn (each variant's tag = its zero-based position), then
//! asserts each variant has the same numeric tag in both places.
//!
//! Wave 2 (#06.8) moved the IoError + IoErrorKind enums from
//! `compiler/riven_core/src/resolve/stdlib/mod.rs` into `io.rvn` —
//! the resolver-side scan target moved with them. The runtime side
//! is unchanged.

use std::fs;
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

fn read(path: &str) -> String {
    fs::read_to_string(workspace_root().join(path))
        .unwrap_or_else(|e| panic!("read {} failed: {}", path, e))
}

/// Snake-case → CamelCase: `NOT_FOUND` → `NotFound`.
fn to_camel(s: &str) -> String {
    let mut out = String::new();
    let mut upper = true;
    for c in s.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.push(c);
            upper = false;
        } else {
            out.push(c.to_ascii_lowercase());
        }
    }
    out
}

#[test]
fn io_error_variant_tags_match_runtime_and_stdlib_source() {
    // #06.95 Phase B-2: the RIVEN_IO_ERROR_* tag macros moved from
    // io_error.c into the shared `library/std/core/runtime/runtime.h`
    // so cross-package `.c` files (fs, net, rand, file, …) can
    // reference them without depending on the unity build.
    let runtime = read("library/std/core/runtime/runtime.h");
    let stdlib_source = read("library/std/io/src/lib.rvn");

    // Collect (Variant, tag) pairs from the runtime #defines.
    let mut runtime_tags: Vec<(String, usize)> = Vec::new();
    for line in runtime.lines() {
        let trimmed = line.trim();
        let prefix = "#define RIVEN_IO_ERROR_";
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            // rest = "NOT_FOUND          0"
            let mut parts = rest.split_whitespace();
            let name = parts.next().unwrap_or("");
            let tag: usize = parts.next().unwrap_or("").parse().unwrap_or(usize::MAX);
            if !name.is_empty() && tag != usize::MAX {
                runtime_tags.push((to_camel(name), tag));
            }
        }
    }

    // Stdlib-source side: walk the `enum IoError ... end` block in
    // io.rvn and assign each non-comment, non-blank line its
    // zero-based position as the tag. Variant lines may be bare
    // identifiers (unit variants) or `Name(payload)` /
    // `Name { payload }` (struct/tuple variants); we strip everything
    // from the first `(` / `{` / whitespace to get the variant name.
    let mut stdlib_tags: Vec<(String, usize)> = Vec::new();
    let mut in_block = false;
    let mut next_tag: usize = 0;
    for line in stdlib_source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("enum IoError") {
            in_block = true;
            next_tag = 0;
            continue;
        }
        if !in_block {
            continue;
        }
        if trimmed == "end" {
            break;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let name: &str = trimmed
            .split(|c: char| c == '(' || c == '{' || c.is_whitespace())
            .next()
            .unwrap_or("");
        assert!(
            name.chars().next().is_some_and(|c| c.is_ascii_uppercase()),
            "unexpected line inside `enum IoError` body: {:?}",
            line
        );
        stdlib_tags.push((name.to_string(), next_tag));
        next_tag += 1;
    }

    runtime_tags.sort_by_key(|(_, t)| *t);
    stdlib_tags.sort_by_key(|(_, t)| *t);

    // The variant set must match 1:1 across runtime + stdlib source.
    assert_eq!(
        runtime_tags.len(),
        20,
        "expected 20 IoError tag #defines in runtime; got {:?}",
        runtime_tags
    );
    assert_eq!(
        stdlib_tags.len(),
        20,
        "expected 20 IoError variants in library/std/io/src/lib.rvn; got {:?}",
        stdlib_tags
    );
    assert_eq!(
        runtime_tags, stdlib_tags,
        "IoError variant ↔ tag mapping drifted between runtime and stdlib source"
    );
}
