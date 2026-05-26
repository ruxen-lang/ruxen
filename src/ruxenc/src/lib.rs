//! ruxenc library surface.
//!
//! The `ruxenc` binary lives in `main.rs`; everything reusable (for benches,
//! integration tests, downstream crates, and the unified `ruxen` binary) is
//! re-exported here.
//!
//! Each CLI subcommand has a dedicated module with a `pub fn run(args: &[String])
//! -> Result<(), String>` entry point. The two binaries (`ruxenc` and `ruxen`)
//! both dispatch into these modules so there is exactly one implementation per
//! command.

pub mod bench;
pub mod cache;
pub mod clean;
pub mod compile;
pub mod fmt;
pub mod test_output;
pub mod test_runner;
