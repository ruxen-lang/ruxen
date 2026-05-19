//! Pin test for the `Shutdown` enum variant-tag stability contract
//! introduced in Phase 2 #06.5 T5.
//!
//! The runtime (`library/runtime/net/tcp.c`) and the resolver
//! (`compiler/riven_core/src/resolve/stdlib/mod.rs`) co-document the
//! tag indices for each `Shutdown` variant. `riven_tcp_stream_shutdown`
//! reads the tag at offset 0 of the value pointer and switches on it
//! to pick `SHUT_RD` / `SHUT_WR` / `SHUT_RDWR`. If the resolver and
//! runtime drift the runtime will hit the E0713 ("Shutdown variant
//! unknown") arm and surface InvalidInput for what should be a working
//! call — this pin test surfaces the drift before it can cause a
//! confusing test-time failure.

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

/// Pull every `("Name", N)` tuple out of `line` and append them to
/// `out`. Mirrors the helper in `file_class_layout_stability.rs` so a
/// rustfmt-collapsed `shutdown_variants` table still parses.
fn parse_tuples(line: &str, out: &mut Vec<(String, usize)>) {
    let mut cursor = line;
    while let Some(open) = cursor.find("(\"") {
        let rest = &cursor[open + 2..];
        let Some(end_quote) = rest.find('"') else {
            break;
        };
        let name = &rest[..end_quote];
        let after = &rest[end_quote + 1..];
        let Some(comma) = after.find(',') else {
            break;
        };
        let tail = after[comma + 1..].trim_start();
        let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(tag) = digits.parse::<usize>() {
            out.push((name.to_string(), tag));
        }
        cursor = &after[comma + 1..];
    }
}

#[test]
fn shutdown_tag_values_match_runtime_and_stdlib_source() {
    // Wave 2 (#06.8) moved the Shutdown enum from
    // `compiler/riven_core/src/resolve/stdlib/mod.rs` to the
    // self-hosted `library/std/src/net.rvn`. The variant-tag
    // contract against `RIVEN_SHUTDOWN_*` in
    // `library/runtime/net/tcp.c` is unchanged — only the
    // *resolver-side scan target* moved.
    let runtime = read("library/runtime/net/tcp.c");
    let stdlib_source = read("library/std/src/net.rvn");

    // Runtime side: scan for `#define RIVEN_SHUTDOWN_<NAME>  <tag>`.
    let mut runtime_tags: Vec<(String, usize)> = Vec::new();
    for line in runtime.lines() {
        let trimmed = line.trim();
        let prefix = "#define RIVEN_SHUTDOWN_";
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            let mut parts = rest.split_whitespace();
            let name = parts.next().unwrap_or("");
            let tag: usize = parts.next().unwrap_or("").parse().unwrap_or(usize::MAX);
            if !name.is_empty() && tag != usize::MAX {
                let camel = match name {
                    "READ" => "Read".to_string(),
                    "WRITE" => "Write".to_string(),
                    "BOTH" => "Both".to_string(),
                    other => panic!("unrecognised Shutdown #define: {}", other),
                };
                runtime_tags.push((camel, tag));
            }
        }
    }

    // Stdlib-source side: enum variants are listed in declaration
    // order under `enum Shutdown ... end` in net.rvn. The variant's
    // tag is its zero-based position in that list — Riven's
    // VariantKind::Unit enum lowering preserves source order. We
    // pull each non-comment, non-blank line between the header and
    // `end` as a variant name.
    let mut stdlib_tags: Vec<(String, usize)> = Vec::new();
    let mut in_block = false;
    let mut next_tag: usize = 0;
    for line in stdlib_source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("enum Shutdown") {
            in_block = true;
            next_tag = 0;
            continue;
        }
        if !in_block {
            continue;
        }
        if trimmed == "end" {
            in_block = false;
            break;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // A bare variant line is the identifier on its own — no `(...)`
        // payload, no struct fields — for Shutdown's three unit
        // variants. Defensive: stop on anything that doesn't match.
        let name = trimmed.split_whitespace().next().unwrap_or("");
        assert!(
            name.chars().next().map_or(false, |c| c.is_ascii_uppercase()),
            "unexpected line inside `enum Shutdown` body: {:?}",
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
        "expected 3 RIVEN_SHUTDOWN_* defines in runtime; got {:?}",
        runtime_tags
    );
    assert_eq!(
        stdlib_tags.len(),
        3,
        "expected 3 Shutdown variants in library/std/src/net.rvn; got {:?}",
        stdlib_tags
    );
    assert_eq!(
        runtime_tags, stdlib_tags,
        "Shutdown variant ↔ tag mapping drifted between runtime and stdlib source"
    );

    // Canonical mapping — pin so a simultaneous re-order on both
    // sides (which would slip past the cross-check above) still
    // fails the contract.
    assert_eq!(runtime_tags[0], ("Read".to_string(), 0));
    assert_eq!(runtime_tags[1], ("Write".to_string(), 1));
    assert_eq!(runtime_tags[2], ("Both".to_string(), 2));
}

#[test]
fn riven_tcp_listener_static_assert_is_eight_bytes() {
    let runtime = read("library/runtime/net/tcp.c");
    assert!(
        runtime.contains("_Static_assert(sizeof(RivenTcpListener) == 8"),
        "expected `_Static_assert(sizeof(RivenTcpListener) == 8 ...)` in tcp.c — \
         widening the struct without updating the MIR drop pipeline is a \
         silent leak risk"
    );
}

#[test]
fn riven_tcp_stream_static_assert_is_eight_bytes() {
    let runtime = read("library/runtime/net/tcp.c");
    assert!(
        runtime.contains("_Static_assert(sizeof(RivenTcpStream) == 8"),
        "expected `_Static_assert(sizeof(RivenTcpStream) == 8 ...)` in tcp.c"
    );
}
