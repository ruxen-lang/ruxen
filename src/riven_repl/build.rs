use std::env;
use std::path::PathBuf;

fn collect_runtime_sources(std_root: &std::path::Path) -> Vec<PathBuf> {
    let mut sources: Vec<PathBuf> = Vec::new();
    for pkg in std::fs::read_dir(std_root).expect("read library/std") {
        let pkg = match pkg {
            Ok(e) => e,
            Err(_) => continue,
        };
        let runtime_dir = pkg.path().join("runtime");
        if !runtime_dir.is_dir() {
            continue;
        }
        for f in std::fs::read_dir(&runtime_dir).expect("read runtime dir") {
            let f = match f {
                Ok(e) => e,
                Err(_) => continue,
            };
            let p = f.path();
            if p.extension().and_then(|s| s.to_str()) == Some("c") {
                sources.push(p);
            }
        }
    }
    sources.sort();
    sources
}

fn main() {
    let crate_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    // #06.95 Phase B-2: per-package C runtime. Each stdlib package's
    // `runtime/*.c` files are standalone translation units that
    // `#include` the shared header at
    // `library/std/core/runtime/runtime.h`. The cc::Build below
    // compiles every one of them into the librivenrt archive that the
    // REPL's cranelift-jit links against.
    let workspace = PathBuf::from(&crate_dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let std_root = workspace.join("library").join("std");
    let runtime_sources = collect_runtime_sources(&std_root);
    if runtime_sources.is_empty() {
        panic!(
            "no `runtime/*.c` sources found under {}",
            std_root.display()
        );
    }

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let out_dir = env::var("OUT_DIR").unwrap();

    // The REPL's cranelift-jit resolves runtime symbols via
    // dlsym(RTLD_DEFAULT), which only sees symbols that survived the
    // link.  Without a whole-archive include, the linker strips ~140
    // unused helpers (Hash_*, Set_*, Option_*, Path_*, …) and the REPL
    // panics the moment user code touches one of them.
    //
    // Strategy: let cc compile every per-package `.c` into the
    // `librivenrt.a` archive (with cargo-metadata emission suppressed
    // so we control the link directive ourselves), then emit a
    // `+whole-archive` modifier-form `rustc-link-lib` so the linker
    // pulls in every object.  Without the cargo_metadata suppression
    // cc emits a plain `cargo:rustc-link-lib=static=rivenrt` and our
    // modifier-form directive becomes a *second* link directive for
    // the same archive — on Linux that surfaces as "multiple
    // definition" errors because the archive ends up in the link
    // twice.
    let mut build = cc::Build::new();
    build
        .opt_level(2)
        .warnings(true)
        .cargo_metadata(false);
    for src in &runtime_sources {
        build.file(src);
    }
    build.compile("rivenrt");

    println!("cargo:rustc-link-search=native={out_dir}");

    if target_os == "macos" || target_os == "ios" {
        // macOS ld doesn't honour `+whole-archive`; use `-force_load`
        // with the absolute archive path instead, and combine with
        // `-export_dynamic` so the surviving symbols enter the dynamic
        // symbol table where dlsym(RTLD_DEFAULT) can see them.
        println!("cargo:rustc-link-arg=-Wl,-force_load,{out_dir}/librivenrt.a");
        println!("cargo:rustc-link-arg=-Wl,-export_dynamic");
        // std.rand on macOS uses SecRandomCopyBytes from the Security
        // framework (see library/std/rand/runtime/rand.c). The full-program
        // codegen path emits this in compiler/riven_core/src/codegen/object.rs;
        // the REPL's librivenrt.a link needs the same flag.
        println!("cargo:rustc-link-lib=framework=Security");
    } else {
        // GNU ld / lld: `+whole-archive` modifier on the link-lib
        // directive expands to `--whole-archive -lrivenrt --no-whole-archive`
        // in the rustc-emitted link command, with no risk of the
        // archive being included twice.
        println!("cargo:rustc-link-lib=static:+whole-archive=rivenrt");
        if target_os == "linux" || target_os == "android" {
            println!("cargo:rustc-link-arg=-Wl,--export-dynamic");
        }
    }

    for src in &runtime_sources {
        println!("cargo:rerun-if-changed={}", src.display());
    }
    println!("cargo:rerun-if-changed={}", std_root.display());
}
