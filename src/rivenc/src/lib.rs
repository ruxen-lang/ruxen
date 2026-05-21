//! rivenc library surface.
//!
//! The `rivenc` binary lives in `main.rs`; everything reusable (for benches,
//! integration tests, downstream crates, and the unified `riven` binary) is
//! re-exported here.
//!
//! Each CLI subcommand has a dedicated module with a `pub fn run(args: &[String])
//! -> Result<(), String>` entry point. The two binaries (`rivenc` and `riven`)
//! both dispatch into these modules so there is exactly one implementation per
//! command.

pub mod bench;
pub mod cache;
pub mod clean;
pub mod compile;
pub mod fmt;
