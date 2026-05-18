use std::env;
use std::path::PathBuf;

fn main() {
    let crate_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let runtime_c = PathBuf::from(&crate_dir)
        .parent() // crates/
        .unwrap()
        .parent() // workspace root
        .unwrap()
        .join("library")
        .join("runtime")
        .join("runtime.c");

    if !runtime_c.exists() {
        panic!("Runtime source not found at {:?}", runtime_c);
    }

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let out_dir = env::var("OUT_DIR").unwrap();

    // The REPL's cranelift-jit resolves runtime symbols via
    // dlsym(RTLD_DEFAULT), which only sees symbols that survived the
    // link.  Without a whole-archive include, the linker strips ~140
    // unused helpers (Hash_*, Set_*, Option_*, Path_*, …) and the REPL
    // panics the moment user code touches one of them.
    //
    // Strategy: let cc compile `runtime.c` into `librivenrt.a` (with
    // its cargo-metadata emission suppressed so we control the link
    // directive ourselves), then emit a `+whole-archive` modifier-form
    // `rustc-link-lib` so the linker pulls in every object.  Without
    // the cargo_metadata suppression cc emits a plain
    // `cargo:rustc-link-lib=static=rivenrt` and our modifier-form
    // directive becomes a *second* link directive for the same
    // archive — on Linux that surfaces as "multiple definition of
    // riven_env_var" because the runtime translation unit ends up in
    // the link twice.
    cc::Build::new()
        .file(&runtime_c)
        .opt_level(2)
        .warnings(true)
        .cargo_metadata(false)
        .compile("rivenrt");

    println!("cargo:rustc-link-search=native={out_dir}");

    if target_os == "macos" || target_os == "ios" {
        // macOS ld doesn't honour `+whole-archive`; use `-force_load`
        // with the absolute archive path instead, and combine with
        // `-export_dynamic` so the surviving symbols enter the dynamic
        // symbol table where dlsym(RTLD_DEFAULT) can see them.
        println!("cargo:rustc-link-arg=-Wl,-force_load,{out_dir}/librivenrt.a");
        println!("cargo:rustc-link-arg=-Wl,-export_dynamic");
        // std.rand on macOS uses SecRandomCopyBytes from the Security
        // framework (see library/runtime/io/rand.c). The full-program
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

    println!("cargo:rerun-if-changed={}", runtime_c.display());
}
