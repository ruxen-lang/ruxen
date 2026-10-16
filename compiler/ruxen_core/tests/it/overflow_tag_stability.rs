//! Pin test for the `Overflow` enum variant-tag stability contract
//! (concurrency §4.4 — the async bounded `Queue`).
//!
//! `enum Overflow { Suspend; DropOldest; DropNewest }` lives in
//! `library/std/future/src/queue.rx`. A no-payload Ruxen enum crosses
//! to C as a HEAP POINTER whose int32 tag at offset 0 the queue
//! constructor reads (`*(int32_t *)overflow`) and switches on against
//! the `#define RUXEN_OVERFLOW_*` tags in
//! `library/std/future/runtime/queue.c` (Suspend=0, DropOldest=1,
//! DropNewest=2). If the enum declaration order and the C `#define`s
//! drift, the constructor picks the wrong overflow policy (or hits its
//! unknown-tag panic) silently. This pin surfaces the drift directly —
//! the e2e pins exercise each tag end-to-end, but only this snapshots
//! the ABI contract. Mirrors `shutdown_tag_stability.rs`.

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

#[test]
fn overflow_tag_values_match_runtime_and_stdlib_source() {
    let runtime = read("library/std/future/runtime/queue.c");
    let stdlib_source = read("library/std/future/src/queue.rx");

    // Runtime side: scan for `#define RUXEN_OVERFLOW_<NAME>  <tag>`.
    let mut runtime_tags: Vec<(String, usize)> = Vec::new();
    for line in runtime.lines() {
        let trimmed = line.trim();
        let prefix = "#define RUXEN_OVERFLOW_";
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            let mut parts = rest.split_whitespace();
            let name = parts.next().unwrap_or("");
            let tag: usize = parts.next().unwrap_or("").parse().unwrap_or(usize::MAX);
            if !name.is_empty() && tag != usize::MAX {
                // Map the SCREAMING_SNAKE define suffix to the CamelCase
                // enum variant name.
                let camel = match name {
                    "SUSPEND" => "Suspend".to_string(),
                    "DROP_OLDEST" => "DropOldest".to_string(),
                    "DROP_NEWEST" => "DropNewest".to_string(),
                    other => panic!("unrecognised Overflow #define: RUXEN_OVERFLOW_{}", other),
                };
                runtime_tags.push((camel, tag));
            }
        }
    }

    // Stdlib-source side: variants under `enum Overflow ... end` in
    // declaration order; the tag is the zero-based position (Ruxen's
    // VariantKind::Unit enum lowering preserves source order).
    let mut stdlib_tags: Vec<(String, usize)> = Vec::new();
    let mut in_block = false;
    let mut next_tag: usize = 0;
    for line in stdlib_source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("enum Overflow") {
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
        // A bare unit-variant line: the identifier on its own (an inline
        // `# comment` after it is fine — split on whitespace takes the
        // leading identifier).
        let name = trimmed.split_whitespace().next().unwrap_or("");
        assert!(
            name.chars().next().is_some_and(|c| c.is_ascii_uppercase()),
            "unexpected line inside `enum Overflow` body: {:?}",
            line
        );
        stdlib_tags.push((name.to_string(), next_tag));
        next_tag += 1;
    }

    runtime_tags.sort_by_key(|(_, t)| *t);
    stdlib_tags.sort_by_key(|(_, t)| *t);

    assert_eq!(
        runtime_tags.len(),
        3,
        "expected 3 RUXEN_OVERFLOW_* defines in queue.c; got {:?}",
        runtime_tags
    );
    assert_eq!(
        stdlib_tags.len(),
        3,
        "expected 3 Overflow variants in library/std/future/src/queue.rx; got {:?}",
        stdlib_tags
    );
    assert_eq!(
        runtime_tags, stdlib_tags,
        "Overflow variant ↔ tag mapping drifted between queue.c and queue.rx"
    );

    // Canonical mapping — pin so a simultaneous re-order on both sides
    // (which would slip past the cross-check above) still fails.
    assert_eq!(runtime_tags[0], ("Suspend".to_string(), 0));
    assert_eq!(runtime_tags[1], ("DropOldest".to_string(), 1));
    assert_eq!(runtime_tags[2], ("DropNewest".to_string(), 2));
}
