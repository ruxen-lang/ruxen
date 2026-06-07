//! Build script for `ruxen_cli`.
//!
//! Why this exists: the `ruxen_repl` crate's build script compiles
//! every per-package `runtime/*.c` source into a single
//! `libruxenrt.a` and tells the linker to whole-archive it. On macOS
//! it does so via `cargo:rustc-link-arg=-Wl,-force_load,…`, which
//! cargo does NOT propagate across package boundaries — so the
//! unified `ruxen` binary that lives in this crate would link
//! without the runtime and fail with "_ruxen_vec_new" /
//! "_ruxen_string_concat" / etc. undefined.
//!
//! Fix: `ruxen_repl/Cargo.toml` declares `links = "ruxenrt"` and its
//! build.rs publishes the OUT_DIR via `cargo:lib_dir=…`. Cargo turns
//! that into the env var `DEP_RUXENRT_LIB_DIR` for THIS build script,
//! which we use to re-emit the macOS-specific link args for the
//! `ruxen` bin.
//!
//! On Linux / other GNU-ld targets the REPL emits
//! `cargo:rustc-link-lib=static:+whole-archive=ruxenrt`, which DOES
//! propagate to dependent crates. Re-emitting the link directive
//! here would inject `-lruxenrt` a second time and the archive would
//! get pulled into the link command twice — surfacing as a wall of
//! "duplicate symbol" errors from rust-lld. So on non-Darwin we only
//! emit `--export-dynamic` (a non-propagating link-arg that the bin
//! needs for the REPL's dlsym(RTLD_DEFAULT) lookups), and leave the
//! archive itself to the propagated link-lib directive.

use std::env;

fn main() {
    // Auto-generate the embedded error-explanation table by scanning
    // docs/errors/*.md, so `explain.rs` never hand-mirrors the folder.
    generate_error_docs_table();

    // ruxen_repl publishes its libruxenrt.a OUT_DIR through the
    // `links = "ruxenrt"` channel. If it's missing, fail loudly —
    // silently skipping would produce a fresh symbol-undefined wall
    // of linker errors that takes ten minutes to diagnose.
    let archive_dir = env::var("DEP_RUXENRT_LIB_DIR").expect(
        "DEP_RUXENRT_LIB_DIR not set — ensure ruxen_repl/Cargo.toml has \
         `links = \"ruxenrt\"` and its build.rs prints `cargo:lib_dir=…`",
    );
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    // Place the prebuilt runtime archive next to the `ruxen` binary
    // (target/<profile>/libruxenrt.a) so the compiler's
    // `find_prebuilt_runtime_archive()` auto-discovers it in dev builds,
    // and so `install.sh --from-source` has a known path to stage into
    // `~/.ruxen/lib/`. OUT_DIR = target/<profile>/build/ruxen_cli-<hash>/out,
    // so the profile dir is its 3rd ancestor (out -> ruxen_cli-* -> build).
    {
        let src = std::path::Path::new(&archive_dir).join("libruxenrt.a");
        if src.is_file() {
            if let Ok(out_dir) = env::var("OUT_DIR") {
                if let Some(profile_dir) = std::path::Path::new(&out_dir).ancestors().nth(3) {
                    let dest = profile_dir.join("libruxenrt.a");
                    let _ = std::fs::copy(&src, &dest);
                }
            }
            println!("cargo:rerun-if-changed={}", src.display());
        }
    }

    if target_os == "macos" || target_os == "ios" {
        // macOS ld doesn't honour `+whole-archive`; ruxen_repl uses
        // `-force_load` via `cargo:rustc-link-arg`, which doesn't
        // propagate. Re-emit the same args scoped to the `ruxen` bin.
        println!("cargo:rustc-link-arg-bin=ruxen=-Wl,-force_load,{archive_dir}/libruxenrt.a");
        println!("cargo:rustc-link-arg-bin=ruxen=-Wl,-export_dynamic");
        // std.rand's runtime/rand.c calls SecRandomCopyBytes.
        println!("cargo:rustc-link-arg-bin=ruxen=-framework");
        println!("cargo:rustc-link-arg-bin=ruxen=Security");
    } else if target_os == "linux" || target_os == "android" {
        // The archive itself is pulled in by the propagating
        // `rustc-link-lib=static:+whole-archive=ruxenrt` from
        // ruxen_repl. We only need `--export-dynamic` here so the
        // REPL's cranelift-jit can dlsym(RTLD_DEFAULT) the runtime
        // symbols at runtime — that's a `link-arg`, which does NOT
        // propagate, so it must be re-emitted in this crate.
        println!("cargo:rustc-link-arg-bin=ruxen=-Wl,--export-dynamic");
    }
}

/// Generate `explain.rs`'s `EXPLAINS` table by scanning every
/// `docs/errors/*.md` at build time. The hand-maintained `include_str!` list
/// drifted from the folder (a registered code's `.md` existed but its table
/// line was forgotten), so the table is now derived from the folder — adding
/// `docs/errors/E####.md` is enough; it is embedded automatically. Emits to
/// `$OUT_DIR/error_docs_table.rs`, which `explain.rs` `include!`s.
fn generate_error_docs_table() {
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    // src/ruxen_cli -> workspace-root docs/errors
    let errors_dir = std::path::Path::new(&manifest).join("../../docs/errors");
    let errors_dir = errors_dir
        .canonicalize()
        .unwrap_or_else(|e| panic!("docs/errors not found at {}: {e}", errors_dir.display()));
    // Regenerate on file add/remove (dir mtime) and content edits (per file).
    println!("cargo:rerun-if-changed={}", errors_dir.display());

    let mut codes: Vec<String> = std::fs::read_dir(&errors_dir)
        .expect("read docs/errors")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|x| x == "md").unwrap_or(false))
        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .collect();
    codes.sort();

    let mut body = String::from(
        "// @generated by build.rs from docs/errors/*.md — DO NOT EDIT.\n\
         static EXPLAINS: &[(&str, &str)] = &[\n",
    );
    for code in &codes {
        let path = errors_dir.join(format!("{code}.md"));
        println!("cargo:rerun-if-changed={}", path.display());
        // {:?} emits a valid escaped Rust string literal for the absolute
        // path; include_str! accepts absolute paths and build.rs reruns per
        // machine, so the baked path is always correct for this build.
        body.push_str(&format!(
            "    ({:?}, include_str!({:?})),\n",
            code,
            path.to_string_lossy()
        ));
    }
    body.push_str("];\n");

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR");
    let dest = std::path::Path::new(&out_dir).join("error_docs_table.rs");
    std::fs::write(&dest, body).expect("write error_docs_table.rs");
}
