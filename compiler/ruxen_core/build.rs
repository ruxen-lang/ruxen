//! Bakes the host target triple into the crate so
//! `codegen::find_prebuilt_runtime_archive` can build the per-triple
//! discovery candidate (`<install>/lib/ruxen/<triple>/libruxenrt.a`) via
//! `option_env!("RUXEN_HOST_TARGET")`. `cargo:rustc-env` only affects the
//! emitting crate's own compilation, so this MUST live in `ruxen_core`
//! (where the `option_env!` is read), not a downstream crate.

//!
//! The only input is the `TARGET` env var, which Cargo sets per target and
//! already folds into the build fingerprint. Without a `rerun-if-changed`
//! line Cargo reruns this script whenever ANY file in the package changes
//! (every integration test, every fixture), and the rerun forces a rebuild
//! of `ruxen_core` and a relink of every test binary. Watching only this
//! file keeps an edit to `tests/*.rs` from rebuilding the library.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target = std::env::var("TARGET").unwrap_or_default();
    println!("cargo:rustc-env=RUXEN_HOST_TARGET={target}");
}
