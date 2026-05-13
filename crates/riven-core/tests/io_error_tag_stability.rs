//! Pin test for the `IoError` variant-tag stability contract.
//!
//! The runtime (`crates/riven-core/runtime/runtime.c`) and the
//! resolver (`crates/riven-core/src/resolve/mod.rs`) co-document the
//! tag indices for each `IoError` variant.  If they drift, runtime-
//! returned errors will be misinterpreted at the typeck layer and
//! `match` arms will silently miss.  This test grep-extracts the
//! `RIVEN_IO_ERROR_*` defines from the C source and the
//! `io_unit_variants` table from the resolver, then asserts each
//! variant has the same numeric tag in both places.

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
fn io_error_variant_tags_match_runtime_and_resolver() {
    let runtime = read("crates/riven-core/runtime/runtime.c");
    let resolver = read("crates/riven-core/src/resolve/mod.rs");

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

    // Collect (Variant, tag) pairs from the resolver table.
    // Scan for lines like `("NotFound", 0),` inside the
    // `io_unit_variants` array.
    let mut resolver_tags: Vec<(String, usize)> = Vec::new();
    for line in resolver.lines() {
        let trimmed = line.trim();
        if let Some(start) = trimmed.find("(\"") {
            let rest = &trimmed[start + 2..];
            if let Some(end_quote) = rest.find('"') {
                let name = &rest[..end_quote];
                let after = &rest[end_quote + 1..];
                if let Some(comma) = after.find(',') {
                    let tag_str = after[comma + 1..]
                        .trim_start()
                        .trim_end_matches(|c: char| !c.is_ascii_digit())
                        .trim();
                    if let Ok(tag) = tag_str.parse::<usize>() {
                        // Filter to IoError-y names (avoid stray
                        // (\"name\", number) tuples from unrelated
                        // code).  Heuristic: all 8 named variants.
                        const KNOWN: &[&str] = &[
                            "NotFound",
                            "PermissionDenied",
                            "AlreadyExists",
                            "Interrupted",
                            "WouldBlock",
                            "InvalidInput",
                            "UnexpectedEof",
                            "BrokenPipe",
                            "Other",
                        ];
                        if KNOWN.contains(&name) && !resolver_tags.iter().any(|(n, _)| n == name) {
                            resolver_tags.push((name.to_string(), tag));
                        }
                    }
                }
            }
        }
    }
    // The resolver defines `Other` separately (struct variant with
    // a payload); pick it up via a different sentinel.
    for line in resolver.lines() {
        if line.contains("variant_idx: 8") && !resolver_tags.iter().any(|(n, _)| n == "Other") {
            resolver_tags.push(("Other".to_string(), 8));
        }
    }

    runtime_tags.sort_by_key(|(_, t)| *t);
    resolver_tags.sort_by_key(|(_, t)| *t);

    // The unit variants must match 1:1.  `Other` is tag 8 on both
    // sides (struct variant in resolver, OTHER #define in runtime).
    assert_eq!(
        runtime_tags.len(),
        9,
        "expected 9 IoError tag #defines in runtime; got {:?}",
        runtime_tags
    );
    assert_eq!(
        resolver_tags.len(),
        9,
        "expected 9 IoError variants in resolver; got {:?}",
        resolver_tags
    );
    assert_eq!(
        runtime_tags, resolver_tags,
        "IoError variant ↔ tag mapping drifted between runtime and resolver"
    );
}
