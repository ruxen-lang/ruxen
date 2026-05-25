//! Build script for `ruxen_cli`.
//!
//! Why this exists: the `ruxen_repl` crate's build script compiles
//! every per-package `runtime/*.c` source into a single
//! `libruxenrt.a` and emits `cargo:rustc-link-arg=-Wl,-force_load,…`
//! so its OWN `cdylib`/`bin` targets link the whole archive.
//!
//! Cargo, by design, does NOT propagate `rustc-link-arg` directives
//! across package boundaries — they apply only to artifacts in the
//! emitting package. The unified `ruxen` binary lives here in
//! `ruxen_cli`, so without this build script its link command
//! drops the runtime archive and the linker errors out with
//! "_ruxen_vec_new" / "_ruxen_string_concat" / etc. undefined.
//!
//! Fix: `ruxen_repl/Cargo.toml` declares `links = "ruxenrt"` and its
//! build.rs publishes the OUT_DIR via `cargo:lib_dir=…`. Cargo turns
//! that into the env var `DEP_RUXENRT_LIB_DIR` for THIS build script,
//! which we use to re-emit the platform-appropriate link args for the
//! `ruxen` bin.

use std::env;

fn main() {
    // ruxen_repl publishes its libruxenrt.a OUT_DIR through the
    // `links = "ruxenrt"` channel. If it's missing, fail loudly —
    // silently skipping would produce a fresh symbol-undefined wall
    // of linker errors that takes ten minutes to diagnose.
    let archive_dir = env::var("DEP_RUXENRT_LIB_DIR").expect(
        "DEP_RUXENRT_LIB_DIR not set — ensure ruxen_repl/Cargo.toml has \
         `links = \"ruxenrt\"` and its build.rs prints `cargo:lib_dir=…`",
    );
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    // The bin lives in this crate, so the link args take effect here.
    // Mirror the platform branch in ruxen_repl/build.rs exactly — same
    // archive, same dlsym(RTLD_DEFAULT) requirement.
    if target_os == "macos" || target_os == "ios" {
        println!("cargo:rustc-link-arg-bin=ruxen=-Wl,-force_load,{archive_dir}/libruxenrt.a");
        println!("cargo:rustc-link-arg-bin=ruxen=-Wl,-export_dynamic");
        // std.rand's runtime/rand.c calls SecRandomCopyBytes.
        println!("cargo:rustc-link-arg-bin=ruxen=-framework");
        println!("cargo:rustc-link-arg-bin=ruxen=Security");
    } else {
        // GNU ld / lld: re-issue the static lib with whole-archive
        // applied, scoped to the `ruxen` bin only. The
        // `rustc-link-search` is already in scope because cargo's
        // dependency-link propagation forwards `cargo:rustc-link-search`
        // from ruxen_repl's build.rs.
        println!("cargo:rustc-link-arg-bin=ruxen=-Wl,--whole-archive");
        println!("cargo:rustc-link-arg-bin=ruxen=-lruxenrt");
        println!("cargo:rustc-link-arg-bin=ruxen=-Wl,--no-whole-archive");
        if target_os == "linux" || target_os == "android" {
            println!("cargo:rustc-link-arg-bin=ruxen=-Wl,--export-dynamic");
        }
    }
}
