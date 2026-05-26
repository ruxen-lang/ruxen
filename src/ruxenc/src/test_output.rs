//! Output renderers for `ruxen test`: TAP and JSON.
//!
//! The pretty (default) renderer is co-located with `test_runner::run`
//! because it shares the summary aggregation; TAP and JSON are
//! data-only emitters that consume the same `&[TestFileResult]` slice.

use crate::test_runner::TestFileResult;

/// TAP version 13. One `ok N - <test_path>` per passing test, one
/// `not ok N - <test_path>` per failing test, one
/// `ok N - <test_path> # SKIP pending` per xit/pending case.
///
/// We emit `passed + failed + pending` lines per file rather than one
/// line per file because the spec calls out per-test granularity (TAP
/// consumers like `prove` expect that).
pub fn render_tap(results: &[TestFileResult]) {
    let total: u32 = results
        .iter()
        .map(|r| r.passed + r.failed + r.pending)
        .sum();
    println!("TAP version 13");
    println!("1..{}", total);

    let mut n = 0u32;
    for r in results {
        for _ in 0..r.passed {
            n += 1;
            println!("ok {} - {}", n, r.test_path);
        }
        for _ in 0..r.failed {
            n += 1;
            println!("not ok {} - {}", n, r.test_path);
            println!("  ---");
            println!("  message: \"see captured stderr for {}\"", r.test_path);
            println!("  ...");
        }
        for _ in 0..r.pending {
            n += 1;
            println!("ok {} - {} # SKIP pending", n, r.test_path);
        }
    }
}

/// One JSON object per line — newline-delimited JSON, matching
/// `cargo test --format json`'s shape. Test-name strings are escaped
/// with Rust's `{:?}` (Debug) format, which produces valid JSON
/// strings for the characters we expect in `test_path` (alphanumerics,
/// '.', '_', '-'). If `test_path` ever contains control characters
/// the Debug escaping diverges slightly from strict JSON; we accept
/// that for v1.
pub fn render_json(results: &[TestFileResult]) {
    for r in results {
        for _ in 0..r.passed {
            println!(
                "{{\"type\":\"test\",\"event\":\"ok\",\"name\":{:?}}}",
                r.test_path
            );
        }
        for _ in 0..r.failed {
            println!(
                "{{\"type\":\"test\",\"event\":\"failed\",\"name\":{:?}}}",
                r.test_path
            );
        }
        for _ in 0..r.pending {
            println!(
                "{{\"type\":\"test\",\"event\":\"ignored\",\"name\":{:?}}}",
                r.test_path
            );
        }
    }
    let total_passed: u32 = results.iter().map(|r| r.passed).sum();
    let total_failed: u32 = results.iter().map(|r| r.failed).sum();
    let total_pending: u32 = results.iter().map(|r| r.pending).sum();
    println!(
        "{{\"type\":\"suite\",\"event\":{:?},\"passed\":{},\"failed\":{},\"ignored\":{}}}",
        if total_failed == 0 { "ok" } else { "failed" },
        total_passed,
        total_failed,
        total_pending
    );
}
