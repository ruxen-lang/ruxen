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
fn shutdown_tag_values_match_runtime_and_resolver() {
    let runtime = read("library/runtime/net/tcp.c");
    let resolver = read("compiler/riven_core/src/resolve/stdlib/mod.rs");

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

    // Resolver side: scan for the `shutdown_variants` table. May be
    // multi-line or rustfmt-collapsed onto a single line.
    let mut resolver_tags: Vec<(String, usize)> = Vec::new();
    let mut in_block = false;
    for line in resolver.lines() {
        // Trigger on the table DECLARATION only — see the matching
        // comment in `file_class_layout_stability::
        // seek_from_tag_values_match_runtime_and_resolver` for the
        // failure mode this avoids.
        if line.contains("let shutdown_variants") {
            in_block = true;
        }
        if in_block && line.trim_end().ends_with("];") {
            parse_tuples(line, &mut resolver_tags);
            in_block = false;
            continue;
        }
        if !in_block {
            continue;
        }
        parse_tuples(line, &mut resolver_tags);
    }

    runtime_tags.sort_by_key(|(_, t)| *t);
    resolver_tags.sort_by_key(|(_, t)| *t);

    assert_eq!(
        runtime_tags.len(),
        3,
        "expected 3 RIVEN_SHUTDOWN_* defines in runtime; got {:?}",
        runtime_tags
    );
    assert_eq!(
        resolver_tags.len(),
        3,
        "expected 3 Shutdown variants in resolver; got {:?}",
        resolver_tags
    );
    assert_eq!(
        runtime_tags, resolver_tags,
        "Shutdown variant ↔ tag mapping drifted between runtime and resolver"
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
