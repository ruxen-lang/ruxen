//! Pin test for the `File` / `OpenOptions` / `SeekFrom` wire-layout
//! stability contracts established in Phase 2 #06.5 T2.
//!
//! Three orthogonal invariants are pinned:
//!
//! 1. `RivenFile` is exactly 8 bytes — `{ int32 fd; int32 closed }`.
//!    The MIR scope-exit drop pipeline emits `File_drop(f) +
//!    riven_dealloc(f)` against an 8-byte allocation; widening the
//!    struct without updating that contract would either leak the
//!    extra bytes or, worse, alias into the next heap block.
//!
//! 2. `RivenOpenOptions` is exactly 8 bytes — six u8 flag bytes plus
//!    two pad bytes. The codegen passes the value as a `ptr_ty` arg;
//!    the runtime reads the flag bytes directly. A change in size
//!    (e.g. growing it to 16 bytes to add a `mode_t`) requires the
//!    `riven_open_options_new` allocation site, every Cranelift/LLVM
//!    extern decl, and the runtime accessors to move in lockstep.
//!
//! 3. `SeekFrom` tag values match the runtime's `RIVEN_SEEK_FROM_*`
//!    constants 1:1. The resolver-side enum is `Start(Int) / End(Int)
//!    / Current(Int)` — three single-field struct variants whose
//!    16-byte tagged-value layout is read by `riven_file_seek`.
//!
//! All three pins grep the runtime + resolver sources, so a refactor
//! that drifts either side surfaces here before it can silently
//! break compiled binaries.

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
fn riven_file_static_assert_is_eight_bytes() {
    // The runtime carries the canonical `_Static_assert` — we just
    // grep for it here so deleting the static_assert is caught as a
    // contract regression, not a "well, the C compiler still built
    // somehow" silent change.
    let runtime = read("library/runtime/runtime.c");
    assert!(
        runtime.contains("_Static_assert(sizeof(RivenFile) == 8"),
        "expected `_Static_assert(sizeof(RivenFile) == 8 ...)` in runtime.c — \
         widening the struct without updating the MIR drop pipeline is a \
         silent leak risk"
    );
}

#[test]
fn riven_open_options_static_assert_is_eight_bytes() {
    let runtime = read("library/runtime/runtime.c");
    assert!(
        runtime.contains("_Static_assert(sizeof(RivenOpenOptions) == 8"),
        "expected `_Static_assert(sizeof(RivenOpenOptions) == 8 ...)` in runtime.c"
    );
}

#[test]
fn seek_from_tag_values_match_runtime_and_resolver() {
    let runtime = read("library/runtime/runtime.c");
    let resolver = read("compiler/riven_core/src/resolve/mod.rs");

    // Runtime side: scan for `#define RIVEN_SEEK_FROM_<NAME>  <tag>`.
    let mut runtime_tags: Vec<(String, usize)> = Vec::new();
    for line in runtime.lines() {
        let trimmed = line.trim();
        let prefix = "#define RIVEN_SEEK_FROM_";
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            let mut parts = rest.split_whitespace();
            let name = parts.next().unwrap_or("");
            let tag: usize = parts.next().unwrap_or("").parse().unwrap_or(usize::MAX);
            if !name.is_empty() && tag != usize::MAX {
                // `START` → `Start`, `END` → `End`, `CURRENT` → `Current`.
                let camel = match name {
                    "START" => "Start".to_string(),
                    "END" => "End".to_string(),
                    "CURRENT" => "Current".to_string(),
                    other => panic!("unrecognised SeekFrom #define: {}", other),
                };
                runtime_tags.push((camel, tag));
            }
        }
    }

    // Resolver side: scan for the explicit `seek_from_variants` table.
    // The table appears as `("Start", 0), ("End", 1), ("Current", 2),`
    // so we look for the surrounding identifier to avoid matching
    // unrelated `("Start", n)` tuples elsewhere.
    let mut resolver_tags: Vec<(String, usize)> = Vec::new();
    let mut in_block = false;
    for line in resolver.lines() {
        if line.contains("seek_from_variants") {
            in_block = true;
            continue;
        }
        if in_block && line.trim() == "];" {
            in_block = false;
            continue;
        }
        if !in_block {
            continue;
        }
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
                        resolver_tags.push((name.to_string(), tag));
                    }
                }
            }
        }
    }

    runtime_tags.sort_by_key(|(_, t)| *t);
    resolver_tags.sort_by_key(|(_, t)| *t);

    assert_eq!(
        runtime_tags.len(),
        3,
        "expected 3 SeekFrom #defines in runtime; got {:?}",
        runtime_tags
    );
    assert_eq!(
        resolver_tags.len(),
        3,
        "expected 3 SeekFrom variants in resolver; got {:?}",
        resolver_tags
    );
    assert_eq!(
        runtime_tags, resolver_tags,
        "SeekFrom variant ↔ tag mapping drifted between runtime and resolver"
    );

    // Spot-check the canonical mapping so a re-ordering that
    // simultaneously updates both files (defeating the cross-check
    // above) still fails the contract test below.
    assert_eq!(runtime_tags[0], ("Start".to_string(), 0));
    assert_eq!(runtime_tags[1], ("End".to_string(), 1));
    assert_eq!(runtime_tags[2], ("Current".to_string(), 2));
}
