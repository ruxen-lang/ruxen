use std::env;
use std::path::PathBuf;

fn main() {
    let crate_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let runtime_c = PathBuf::from(&crate_dir)
        .parent()
        .unwrap()
        .join("riven-core")
        .join("runtime")
        .join("runtime.c");

    if !runtime_c.exists() {
        panic!("Runtime source not found at {:?}", runtime_c);
    }

    cc::Build::new()
        .file(&runtime_c)
        .opt_level(2)
        .warnings(true)
        .compile("rivenrt");

    // Force-load the static archive so every `riven_*` symbol is kept
    // in the final binary even when no Rust code references it
    // directly. The REPL's cranelift-jit resolves runtime symbols via
    // dlsym(RTLD_DEFAULT), which only sees symbols that survived the
    // link — without `-force_load` the linker strips ~140 unused
    // helpers (Hash_*, Set_*, Option_*, Path_*, …) and the REPL
    // panics the moment user code touches one of them.
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let out_dir = env::var("OUT_DIR").unwrap();
    if target_os == "macos" || target_os == "ios" {
        // `-force_load` loads every member of the archive even when
        // none of its symbols are referenced from Rust code, but on
        // macOS the linker still dead-strips unreachable globals
        // after that. Combine with `-export_dynamic` so the surviving
        // symbols are exported to the dynamic symbol table — required
        // for dlsym(RTLD_DEFAULT) to find them at REPL run time.
        println!(
            "cargo:rustc-link-arg=-Wl,-force_load,{}/librivenrt.a",
            out_dir
        );
        println!("cargo:rustc-link-arg=-Wl,-export_dynamic");
    } else if target_os == "linux" || target_os == "android" {
        // GNU ld / lld variant: --whole-archive forces inclusion;
        // --export-dynamic puts the resulting symbols into the dynamic
        // table for dlsym.
        println!("cargo:rustc-link-arg=-Wl,--whole-archive");
        println!("cargo:rustc-link-arg=-l:librivenrt.a");
        println!("cargo:rustc-link-arg=-Wl,--no-whole-archive");
        println!("cargo:rustc-link-arg=-Wl,--export-dynamic");
    }

    println!("cargo:rerun-if-changed={}", runtime_c.display());
}
