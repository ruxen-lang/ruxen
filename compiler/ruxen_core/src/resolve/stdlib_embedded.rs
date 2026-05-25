//! Stdlib `.rx` sources embedded into the ruxen binary via
//! `include_str!`. Makes the toolchain self-contained — no need to
//! ship `library/std/` next to the executable or set
//! `RUXEN_STDLIB_PATH` after `cargo install`.
//!
//! The table MUST stay in lockstep with [`BOOTSTRAP_FILES`] in
//! `bootstrap.rs`: same paths, same order. The unit test at the
//! bottom of this file pin-checks that invariant; adding a new
//! bootstrap file requires editing both lists.
//!
//! Filesystem reads still work — `bootstrap.rs::run_bootstrap_with_files`
//! prefers `RUXEN_STDLIB_PATH` / a passed-in path override when one
//! is provided, so stdlib hackers can edit `.rx` files and re-run
//! without recompiling the compiler.

/// Embedded stdlib sources, keyed by the same relative path the
/// filesystem loader uses (`"<pkg>/src/lib.rx"`).
///
/// Build-time invariant: every entry in `BOOTSTRAP_FILES` has a
/// matching entry here with the same key.
pub const BOOTSTRAP_EMBEDDED: &[(&str, &str)] = &[
    (
        "core/src/lib.rx",
        include_str!("../../../../library/std/core/src/lib.rx"),
    ),
    (
        "bootstrap_smoke/src/lib.rx",
        include_str!("../../../../library/std/bootstrap_smoke/src/lib.rx"),
    ),
    (
        "io/src/lib.rx",
        include_str!("../../../../library/std/io/src/lib.rx"),
    ),
    (
        "rand/src/lib.rx",
        include_str!("../../../../library/std/rand/src/lib.rx"),
    ),
    (
        "path/src/lib.rx",
        include_str!("../../../../library/std/path/src/lib.rx"),
    ),
    (
        "env/src/lib.rx",
        include_str!("../../../../library/std/env/src/lib.rx"),
    ),
    (
        "iter/src/lib.rx",
        include_str!("../../../../library/std/iter/src/lib.rx"),
    ),
    (
        "hash/src/lib.rx",
        include_str!("../../../../library/std/hash/src/lib.rx"),
    ),
    (
        "fmt/src/lib.rx",
        include_str!("../../../../library/std/fmt/src/lib.rx"),
    ),
    (
        "net/src/lib.rx",
        include_str!("../../../../library/std/net/src/lib.rx"),
    ),
    (
        "bufio/src/lib.rx",
        include_str!("../../../../library/std/bufio/src/lib.rx"),
    ),
    (
        "process/src/lib.rx",
        include_str!("../../../../library/std/process/src/lib.rx"),
    ),
    (
        "time/src/lib.rx",
        include_str!("../../../../library/std/time/src/lib.rx"),
    ),
    (
        "fs/src/lib.rx",
        include_str!("../../../../library/std/fs/src/lib.rx"),
    ),
    (
        "sync/src/lib.rx",
        include_str!("../../../../library/std/sync/src/lib.rx"),
    ),
    (
        "future/src/lib.rx",
        include_str!("../../../../library/std/future/src/lib.rx"),
    ),
    (
        "async_fs/src/lib.rx",
        include_str!("../../../../library/std/async_fs/src/lib.rx"),
    ),
    (
        "async_net/src/lib.rx",
        include_str!("../../../../library/std/async_net/src/lib.rx"),
    ),
    (
        "async_io/src/lib.rx",
        include_str!("../../../../library/std/async_io/src/lib.rx"),
    ),
    (
        "executor/src/lib.rx",
        include_str!("../../../../library/std/executor/src/lib.rx"),
    ),
    (
        "string/src/lib.rx",
        include_str!("../../../../library/std/string/src/lib.rx"),
    ),
    (
        "option_result/src/lib.rx",
        include_str!("../../../../library/std/option_result/src/lib.rx"),
    ),
    (
        "array/src/lib.rx",
        include_str!("../../../../library/std/array/src/lib.rx"),
    ),
    (
        "map/src/lib.rx",
        include_str!("../../../../library/std/map/src/lib.rx"),
    ),
    (
        "set/src/lib.rx",
        include_str!("../../../../library/std/set/src/lib.rx"),
    ),
    (
        "json/src/lib.rx",
        include_str!("../../../../library/std/json/src/lib.rx"),
    ),
    (
        "foobar/src/lib.rx",
        include_str!("../../../../library/std/foobar/src/lib.rx"),
    ),
    (
        "bench/src/lib.rx",
        include_str!("../../../../library/std/bench/src/lib.rx"),
    ),
    (
        "test/src/lib.rx",
        include_str!("../../../../library/std/test/src/lib.rx"),
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

/// Sibling `.rx` files baked in for multi-file stdlib packages.
/// Keyed by the package's `lib.rx` relative path. The slice value
/// holds `(filename, source)` pairs for every sibling, lib.rx
/// itself is fetched via [`embedded_source`].
///
/// Adding a new sibling: append a `(name, include_str!(…))` entry
/// here. The pin test `multi_file_pkg_lib_present_when_siblings_listed`
/// at the bottom of this file catches drift between this table and
/// the actual filesystem contents.
pub const BOOTSTRAP_EMBEDDED_SIBLINGS: &[(&str, &[(&str, &str)])] = &[
    (
        "async_net/src/lib.rx",
        &[
            (
                "async_tcp_stream.rx",
                include_str!("../../../../library/std/async_net/src/async_tcp_stream.rx"),
            ),
            (
                "async_tcp_listener.rx",
                include_str!("../../../../library/std/async_net/src/async_tcp_listener.rx"),
            ),
            (
                "async_accept_future.rx",
                include_str!("../../../../library/std/async_net/src/async_accept_future.rx"),
            ),
            (
                "async_bind_future.rx",
                include_str!("../../../../library/std/async_net/src/async_bind_future.rx"),
            ),
            (
                "async_connect_future.rx",
                include_str!("../../../../library/std/async_net/src/async_connect_future.rx"),
            ),
            (
                "async_read_future.rx",
                include_str!("../../../../library/std/async_net/src/async_read_future.rx"),
            ),
            (
                "async_read_with_timeout_future.rx",
                include_str!(
                    "../../../../library/std/async_net/src/async_read_with_timeout_future.rx"
                ),
            ),
            (
                "async_write_future.rx",
                include_str!("../../../../library/std/async_net/src/async_write_future.rx"),
            ),
            (
                "async_close_future.rx",
                include_str!("../../../../library/std/async_net/src/async_close_future.rx"),
            ),
        ],
    ),
    (
        "future/src/lib.rx",
        &[
            (
                "context.rx",
                include_str!("../../../../library/std/future/src/context.rx"),
            ),
            (
                "waker.rx",
                include_str!("../../../../library/std/future/src/waker.rx"),
            ),
            (
                "time_sleep_future.rx",
                include_str!("../../../../library/std/future/src/time_sleep_future.rx"),
            ),
            (
                "async.rx",
                include_str!("../../../../library/std/future/src/async.rx"),
            ),
            (
                "task.rx",
                include_str!("../../../../library/std/future/src/task.rx"),
            ),
            (
                "task_handle.rx",
                include_str!("../../../../library/std/future/src/task_handle.rx"),
            ),
            (
                "task_yield_future.rx",
                include_str!("../../../../library/std/future/src/task_yield_future.rx"),
            ),
            (
                "task_join_future.rx",
                include_str!("../../../../library/std/future/src/task_join_future.rx"),
            ),
        ],
    ),
    (
        "async_fs/src/lib.rx",
        &[
            (
                "async_file.rx",
                include_str!("../../../../library/std/async_fs/src/async_file.rx"),
            ),
            (
                "async_open_future.rx",
                include_str!("../../../../library/std/async_fs/src/async_open_future.rx"),
            ),
            (
                "async_read_to_string_future.rx",
                include_str!("../../../../library/std/async_fs/src/async_read_to_string_future.rx"),
            ),
            (
                "async_write_all_future.rx",
                include_str!("../../../../library/std/async_fs/src/async_write_all_future.rx"),
            ),
        ],
    ),
    (
        "io/src/lib.rx",
        &[
            (
                "stdin.rx",
                include_str!("../../../../library/std/io/src/stdin.rx"),
            ),
            (
                "stdout.rx",
                include_str!("../../../../library/std/io/src/stdout.rx"),
            ),
            (
                "stderr.rx",
                include_str!("../../../../library/std/io/src/stderr.rx"),
            ),
            (
                "metadata.rx",
                include_str!("../../../../library/std/io/src/metadata.rx"),
            ),
            (
                "open_options.rx",
                include_str!("../../../../library/std/io/src/open_options.rx"),
            ),
            (
                "file.rx",
                include_str!("../../../../library/std/io/src/file.rx"),
            ),
        ],
    ),
    (
        "sync/src/lib.rx",
        &[
            (
                "thread.rx",
                include_str!("../../../../library/std/sync/src/thread.rx"),
            ),
            (
                "signal.rx",
                include_str!("../../../../library/std/sync/src/signal.rx"),
            ),
            (
                "thread_id.rx",
                include_str!("../../../../library/std/sync/src/thread_id.rx"),
            ),
            (
                "join_handle.rx",
                include_str!("../../../../library/std/sync/src/join_handle.rx"),
            ),
            (
                "mutex.rx",
                include_str!("../../../../library/std/sync/src/mutex.rx"),
            ),
            (
                "mutex_guard.rx",
                include_str!("../../../../library/std/sync/src/mutex_guard.rx"),
            ),
            (
                "shared_sync.rx",
                include_str!("../../../../library/std/sync/src/shared_sync.rx"),
            ),
            (
                "poison_error.rx",
                include_str!("../../../../library/std/sync/src/poison_error.rx"),
            ),
            (
                "thread_panic.rx",
                include_str!("../../../../library/std/sync/src/thread_panic.rx"),
            ),
            (
                "atomic_i64.rx",
                include_str!("../../../../library/std/sync/src/atomic_i64.rx"),
            ),
            (
                "atomic_bool.rx",
                include_str!("../../../../library/std/sync/src/atomic_bool.rx"),
            ),
            (
                "atomic_usize.rx",
                include_str!("../../../../library/std/sync/src/atomic_usize.rx"),
            ),
            (
                "sender.rx",
                include_str!("../../../../library/std/sync/src/sender.rx"),
            ),
            (
                "receiver.rx",
                include_str!("../../../../library/std/sync/src/receiver.rx"),
            ),
            (
                "send_error.rx",
                include_str!("../../../../library/std/sync/src/send_error.rx"),
            ),
            (
                "recv_error.rx",
                include_str!("../../../../library/std/sync/src/recv_error.rx"),
            ),
        ],
    ),
    (
        "async_io/src/lib.rx",
        &[
            (
                "async_stdin.rx",
                include_str!("../../../../library/std/async_io/src/async_stdin.rx"),
            ),
            (
                "async_read_line_future.rx",
                include_str!("../../../../library/std/async_io/src/async_read_line_future.rx"),
            ),
        ],
    ),
    (
        "string/src/lib.rx",
        &[
            (
                "string.rx",
                include_str!("../../../../library/std/string/src/string.rx"),
            ),
            (
                "parse_int_error.rx",
                include_str!("../../../../library/std/string/src/parse_int_error.rx"),
            ),
            (
                "parse_float_error.rx",
                include_str!("../../../../library/std/string/src/parse_float_error.rx"),
            ),
        ],
    ),
    (
        "test/src/lib.rx",
        &[
            (
                "test_case.rx",
                include_str!("../../../../library/std/test/src/test_case.rx"),
            ),
            (
                "matcher.rx",
                include_str!("../../../../library/std/test/src/matcher.rx"),
            ),
            (
                "tester.rx",
                include_str!("../../../../library/std/test/src/tester.rx"),
            ),
            (
                "runner.rx",
                include_str!("../../../../library/std/test/src/runner.rx"),
            ),
        ],
    ),
];

/// Multi-file package lookup. When the package has sibling files in
/// [`BOOTSTRAP_EMBEDDED_SIBLINGS`], returns the lib.rx source plus
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
    out.push(("lib.rx", lib_src));
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
        assert!(embedded_source("core/src/lib.rx").is_some());
        assert!(embedded_source("io/src/lib.rx").is_some());
        assert!(embedded_source("nonexistent/src/lib.rx").is_none());
    }
}
