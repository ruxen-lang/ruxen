//! Stdlib `.rvn` sources embedded into the riven binary via
//! `include_str!`. Makes the toolchain self-contained — no need to
//! ship `library/std/` next to the executable or set
//! `RIVEN_STDLIB_PATH` after `cargo install`.
//!
//! The table MUST stay in lockstep with [`BOOTSTRAP_FILES`] in
//! `bootstrap.rs`: same paths, same order. The unit test at the
//! bottom of this file pin-checks that invariant; adding a new
//! bootstrap file requires editing both lists.
//!
//! Filesystem reads still work — `bootstrap.rs::run_bootstrap_with_files`
//! prefers `RIVEN_STDLIB_PATH` / a passed-in path override when one
//! is provided, so stdlib hackers can edit `.rvn` files and re-run
//! without recompiling the compiler.

/// Embedded stdlib sources, keyed by the same relative path the
/// filesystem loader uses (`"<pkg>/src/lib.rvn"`).
///
/// Build-time invariant: every entry in `BOOTSTRAP_FILES` has a
/// matching entry here with the same key.
pub const BOOTSTRAP_EMBEDDED: &[(&str, &str)] = &[
    (
        "core/src/lib.rvn",
        include_str!("../../../../library/std/core/src/lib.rvn"),
    ),
    (
        "bootstrap_smoke/src/lib.rvn",
        include_str!("../../../../library/std/bootstrap_smoke/src/lib.rvn"),
    ),
    (
        "io/src/lib.rvn",
        include_str!("../../../../library/std/io/src/lib.rvn"),
    ),
    (
        "rand/src/lib.rvn",
        include_str!("../../../../library/std/rand/src/lib.rvn"),
    ),
    (
        "path/src/lib.rvn",
        include_str!("../../../../library/std/path/src/lib.rvn"),
    ),
    (
        "env/src/lib.rvn",
        include_str!("../../../../library/std/env/src/lib.rvn"),
    ),
    (
        "iter/src/lib.rvn",
        include_str!("../../../../library/std/iter/src/lib.rvn"),
    ),
    (
        "hash/src/lib.rvn",
        include_str!("../../../../library/std/hash/src/lib.rvn"),
    ),
    (
        "fmt/src/lib.rvn",
        include_str!("../../../../library/std/fmt/src/lib.rvn"),
    ),
    (
        "net/src/lib.rvn",
        include_str!("../../../../library/std/net/src/lib.rvn"),
    ),
    (
        "bufio/src/lib.rvn",
        include_str!("../../../../library/std/bufio/src/lib.rvn"),
    ),
    (
        "process/src/lib.rvn",
        include_str!("../../../../library/std/process/src/lib.rvn"),
    ),
    (
        "time/src/lib.rvn",
        include_str!("../../../../library/std/time/src/lib.rvn"),
    ),
    (
        "fs/src/lib.rvn",
        include_str!("../../../../library/std/fs/src/lib.rvn"),
    ),
    (
        "sync/src/lib.rvn",
        include_str!("../../../../library/std/sync/src/lib.rvn"),
    ),
    (
        "future/src/lib.rvn",
        include_str!("../../../../library/std/future/src/lib.rvn"),
    ),
    (
        "async_fs/src/lib.rvn",
        include_str!("../../../../library/std/async_fs/src/lib.rvn"),
    ),
    (
        "async_net/src/lib.rvn",
        include_str!("../../../../library/std/async_net/src/lib.rvn"),
    ),
    (
        "async_io/src/lib.rvn",
        include_str!("../../../../library/std/async_io/src/lib.rvn"),
    ),
    (
        "executor/src/lib.rvn",
        include_str!("../../../../library/std/executor/src/lib.rvn"),
    ),
    (
        "string/src/lib.rvn",
        include_str!("../../../../library/std/string/src/lib.rvn"),
    ),
    (
        "option_result/src/lib.rvn",
        include_str!("../../../../library/std/option_result/src/lib.rvn"),
    ),
    (
        "array/src/lib.rvn",
        include_str!("../../../../library/std/array/src/lib.rvn"),
    ),
    (
        "map/src/lib.rvn",
        include_str!("../../../../library/std/map/src/lib.rvn"),
    ),
    (
        "set/src/lib.rvn",
        include_str!("../../../../library/std/set/src/lib.rvn"),
    ),
    (
        "foobar/src/lib.rvn",
        include_str!("../../../../library/std/foobar/src/lib.rvn"),
    ),
    (
        "bench/src/lib.rvn",
        include_str!("../../../../library/std/bench/src/lib.rvn"),
    ),
];

/// Look up an embedded stdlib source by its relative path. Returns
/// `None` if the file isn't in the table — the caller falls back to
/// filesystem reads.
pub fn embedded_source(rel: &str) -> Option<&'static str> {
    BOOTSTRAP_EMBEDDED
        .iter()
        .find(|(p, _)| *p == rel)
        .map(|(_, src)| *src)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::bootstrap::BOOTSTRAP_FILES;

    #[test]
    fn embedded_table_matches_bootstrap_files_one_to_one() {
        // Every BOOTSTRAP_FILES entry must have a matching embedded
        // entry, in the same order. Adding a new bootstrap file
        // requires editing both lists; this test catches the drift.
        assert_eq!(
            BOOTSTRAP_FILES.len(),
            BOOTSTRAP_EMBEDDED.len(),
            "BOOTSTRAP_FILES and BOOTSTRAP_EMBEDDED are out of sync — \
             one has more entries than the other"
        );
        for (i, rel) in BOOTSTRAP_FILES.iter().enumerate() {
            assert_eq!(
                *rel, BOOTSTRAP_EMBEDDED[i].0,
                "entry {} differs: BOOTSTRAP_FILES has `{}`, BOOTSTRAP_EMBEDDED has `{}`",
                i, rel, BOOTSTRAP_EMBEDDED[i].0
            );
            assert!(
                !BOOTSTRAP_EMBEDDED[i].1.is_empty(),
                "embedded source for `{}` is empty",
                rel
            );
        }
    }

    #[test]
    fn embedded_source_lookup_finds_known_file() {
        assert!(embedded_source("core/src/lib.rvn").is_some());
        assert!(embedded_source("io/src/lib.rvn").is_some());
        assert!(embedded_source("nonexistent/src/lib.rvn").is_none());
    }
}
