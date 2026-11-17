//! Pin for the `Poll[T]` variant tag layout in
//! `library/std/future/src/lib.rx`: the C runtime
//! (`library/std/future/runtime/schedule.c`) reads `Ready = 0` and
//! `Pending = 1` directly, so the declaration order is append-only.

// ─── B2 (tag) — Poll variant tag layout is pinned ───────────────────

/// Tag layout: `Ready = 0`, `Pending = 1`. Sub-phase 3's executor will
/// inspect the discriminant when polling the user's future at the C
/// boundary; if this order ever drifts the executor would mis-read
/// the variant and silently busy-loop on a "Ready" branch it thought
/// was "Pending" (or vice versa). The test reads the .rx source
/// directly so a refactor that moves the declaration but keeps the
/// order intact still passes; a reorder breaks the test even if the
/// enum still compiles.
#[test]
fn poll_tag_layout_stability() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("library/std/future/src/lib.rx");
    let source =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e));

    // Scan the `enum Poll[T] ... end` block and collect variant names
    // in declaration order. Mirrors `io_error_tag_stability.rs`.
    let mut variants: Vec<(String, usize)> = Vec::new();
    let mut in_block = false;
    let mut next_tag: usize = 0;
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("enum Poll") {
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
            "unexpected line inside `enum Poll` body: {:?}",
            line
        );
        variants.push((name.to_string(), next_tag));
        next_tag += 1;
    }

    assert_eq!(
        variants,
        vec![("Ready".to_string(), 0), ("Pending".to_string(), 1)],
        "Poll variant order drifted — append-only, never reorder. \
         Sub-phase 3's executor reads these tags at the C boundary."
    );
}
