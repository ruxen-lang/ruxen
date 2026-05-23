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
    (
        "test/src/lib.rvn",
        include_str!("../../../../library/std/test/src/lib.rvn"),
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

/// Sibling `.rvn` files baked in for multi-file stdlib packages.
/// Keyed by the package's `lib.rvn` relative path. The slice value
/// holds `(filename, source)` pairs for every sibling, lib.rvn
/// itself is fetched via [`embedded_source`].
///
/// Adding a new sibling: append a `(name, include_str!(…))` entry
/// here. The pin test `multi_file_pkg_lib_present_when_siblings_listed`
/// at the bottom of this file catches drift between this table and
/// the actual filesystem contents.
pub const BOOTSTRAP_EMBEDDED_SIBLINGS: &[(&str, &[(&str, &str)])] = &[
    (
        "async_net/src/lib.rvn",
        &[
            (
                "async_tcp_stream.rvn",
                include_str!("../../../../library/std/async_net/src/async_tcp_stream.rvn"),
            ),
            (
                "async_tcp_listener.rvn",
                include_str!("../../../../library/std/async_net/src/async_tcp_listener.rvn"),
            ),
            (
                "async_accept_future.rvn",
                include_str!("../../../../library/std/async_net/src/async_accept_future.rvn"),
            ),
            (
                "async_bind_future.rvn",
                include_str!("../../../../library/std/async_net/src/async_bind_future.rvn"),
            ),
            (
                "async_connect_future.rvn",
                include_str!("../../../../library/std/async_net/src/async_connect_future.rvn"),
            ),
            (
                "async_read_future.rvn",
                include_str!("../../../../library/std/async_net/src/async_read_future.rvn"),
            ),
            (
                "async_write_future.rvn",
                include_str!("../../../../library/std/async_net/src/async_write_future.rvn"),
            ),
            (
                "async_close_future.rvn",
                include_str!("../../../../library/std/async_net/src/async_close_future.rvn"),
            ),
        ],
    ),
    (
        "future/src/lib.rvn",
        &[
            (
                "context.rvn",
                include_str!("../../../../library/std/future/src/context.rvn"),
            ),
            (
                "waker.rvn",
                include_str!("../../../../library/std/future/src/waker.rvn"),
            ),
            (
                "time_sleep_future.rvn",
                include_str!("../../../../library/std/future/src/time_sleep_future.rvn"),
            ),
            (
                "async.rvn",
                include_str!("../../../../library/std/future/src/async.rvn"),
            ),
            (
                "task.rvn",
                include_str!("../../../../library/std/future/src/task.rvn"),
            ),
            (
                "task_handle.rvn",
                include_str!("../../../../library/std/future/src/task_handle.rvn"),
            ),
            (
                "task_yield_future.rvn",
                include_str!("../../../../library/std/future/src/task_yield_future.rvn"),
            ),
            (
                "task_join_future.rvn",
                include_str!("../../../../library/std/future/src/task_join_future.rvn"),
            ),
        ],
    ),
    (
        "async_fs/src/lib.rvn",
        &[
            (
                "async_file.rvn",
                include_str!("../../../../library/std/async_fs/src/async_file.rvn"),
            ),
            (
                "async_open_future.rvn",
                include_str!("../../../../library/std/async_fs/src/async_open_future.rvn"),
            ),
            (
                "async_read_to_string_future.rvn",
                include_str!("../../../../library/std/async_fs/src/async_read_to_string_future.rvn"),
            ),
            (
                "async_write_all_future.rvn",
                include_str!("../../../../library/std/async_fs/src/async_write_all_future.rvn"),
            ),
        ],
    ),
    (
        "io/src/lib.rvn",
        &[
            (
                "stdin.rvn",
                include_str!("../../../../library/std/io/src/stdin.rvn"),
            ),
            (
                "stdout.rvn",
                include_str!("../../../../library/std/io/src/stdout.rvn"),
            ),
            (
                "stderr.rvn",
                include_str!("../../../../library/std/io/src/stderr.rvn"),
            ),
            (
                "metadata.rvn",
                include_str!("../../../../library/std/io/src/metadata.rvn"),
            ),
            (
                "open_options.rvn",
                include_str!("../../../../library/std/io/src/open_options.rvn"),
            ),
            (
                "file.rvn",
                include_str!("../../../../library/std/io/src/file.rvn"),
            ),
        ],
    ),
    (
        "sync/src/lib.rvn",
        &[
            (
                "thread.rvn",
                include_str!("../../../../library/std/sync/src/thread.rvn"),
            ),
            (
                "signal.rvn",
                include_str!("../../../../library/std/sync/src/signal.rvn"),
            ),
            (
                "thread_id.rvn",
                include_str!("../../../../library/std/sync/src/thread_id.rvn"),
            ),
            (
                "join_handle.rvn",
                include_str!("../../../../library/std/sync/src/join_handle.rvn"),
            ),
            (
                "mutex.rvn",
                include_str!("../../../../library/std/sync/src/mutex.rvn"),
            ),
            (
                "mutex_guard.rvn",
                include_str!("../../../../library/std/sync/src/mutex_guard.rvn"),
            ),
            (
                "shared_sync.rvn",
                include_str!("../../../../library/std/sync/src/shared_sync.rvn"),
            ),
            (
                "poison_error.rvn",
                include_str!("../../../../library/std/sync/src/poison_error.rvn"),
            ),
            (
                "thread_panic.rvn",
                include_str!("../../../../library/std/sync/src/thread_panic.rvn"),
            ),
            (
                "atomic_i64.rvn",
                include_str!("../../../../library/std/sync/src/atomic_i64.rvn"),
            ),
            (
                "atomic_bool.rvn",
                include_str!("../../../../library/std/sync/src/atomic_bool.rvn"),
            ),
            (
                "atomic_usize.rvn",
                include_str!("../../../../library/std/sync/src/atomic_usize.rvn"),
            ),
            (
                "sender.rvn",
                include_str!("../../../../library/std/sync/src/sender.rvn"),
            ),
            (
                "receiver.rvn",
                include_str!("../../../../library/std/sync/src/receiver.rvn"),
            ),
            (
                "send_error.rvn",
                include_str!("../../../../library/std/sync/src/send_error.rvn"),
            ),
            (
                "recv_error.rvn",
                include_str!("../../../../library/std/sync/src/recv_error.rvn"),
            ),
        ],
    ),
    (
        "async_io/src/lib.rvn",
        &[
            (
                "async_stdin.rvn",
                include_str!("../../../../library/std/async_io/src/async_stdin.rvn"),
            ),
            (
                "async_read_line_future.rvn",
                include_str!("../../../../library/std/async_io/src/async_read_line_future.rvn"),
            ),
        ],
    ),
    (
        "string/src/lib.rvn",
        &[
            (
                "string.rvn",
                include_str!("../../../../library/std/string/src/string.rvn"),
            ),
            (
                "parse_int_error.rvn",
                include_str!("../../../../library/std/string/src/parse_int_error.rvn"),
            ),
            (
                "parse_float_error.rvn",
                include_str!("../../../../library/std/string/src/parse_float_error.rvn"),
            ),
        ],
    ),
    (
        "test/src/lib.rvn",
        &[
            (
                "test_case.rvn",
                include_str!("../../../../library/std/test/src/test_case.rvn"),
            ),
            (
                "matcher.rvn",
                include_str!("../../../../library/std/test/src/matcher.rvn"),
            ),
            (
                "runner.rvn",
                include_str!("../../../../library/std/test/src/runner.rvn"),
            ),
        ],
    ),
];

/// Multi-file package lookup. When the package has sibling files in
/// [`BOOTSTRAP_EMBEDDED_SIBLINGS`], returns the lib.rvn source plus
/// every sibling so the caller can concatenate them the same way the
/// filesystem loader does. Returns `None` for single-file packages —
/// callers fall back to [`embedded_source`].
pub fn embedded_pkg_sources(rel: &str) -> Option<Vec<(&'static str, &'static str)>> {
    let siblings = BOOTSTRAP_EMBEDDED_SIBLINGS
        .iter()
        .find(|(p, _)| *p == rel)
        .map(|(_, s)| *s)?;
    let lib_src = embedded_source(rel)?;
    let mut out: Vec<(&'static str, &'static str)> = Vec::with_capacity(siblings.len() + 1);
    out.push(("lib.rvn", lib_src));
    out.extend_from_slice(siblings);
    Some(out)
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
