//! Bakes the host target triple into the crate so
//! `codegen::find_prebuilt_runtime_archive` can build the per-triple
//! discovery candidate (`<install>/lib/ruxen/<triple>/libruxenrt.a`) via
//! `option_env!("RUXEN_HOST_TARGET")`. `cargo:rustc-env` only affects the
//! emitting crate's own compilation, so this MUST live in `ruxen_core`
//! (where the `option_env!` is read), not a downstream crate.

fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    println!("cargo:rustc-env=RUXEN_HOST_TARGET={target}");
}
