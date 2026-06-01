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
            let Ok(f) = f else { continue };
            let p = f.path();
            if p.extension().and_then(|s| s.to_str()) == Some("c") {
                sources.push(p);
            } else if p.is_dir() {
                // One level of recursion: per-package vendored C sources
                // (e.g. library/std/regex/runtime/pcre2/*.c) get picked up.
                // We intentionally do NOT recurse further — keeps the build
                // glob predictable and matches the "vendor in a single
                // subdir" convention this repo uses.
                for sub in std::fs::read_dir(&p).into_iter().flatten() {
                    let Ok(sub) = sub else { continue };
                    let sp = sub.path();
                    if sp.extension().and_then(|s| s.to_str()) != Some("c") {
                        continue;
                    }
                    // PCRE2 ships two ".c" files that are NOT standalone
                    // translation units — they're pulled in via #include
                    // from pcre2_compile.c / pcre2_tables.c. Compiling
                    // them as TUs would fail with missing-include errors.
                    // Filter by basename so the build glob stays a glob.
                    let name = sp.file_name().and_then(|s| s.to_str()).unwrap_or("");
                    if matches!(name, "pcre2_printint.c" | "pcre2_ucptables.c") {
                        continue;
                    }
                    sources.push(sp);
                }
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
    // compiles every one of them into the libruxenrt archive that the
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
    // `libruxenrt.a` archive (with cargo-metadata emission suppressed
    // so we control the link directive ourselves), then emit a
    // `+whole-archive` modifier-form `rustc-link-lib` so the linker
    // pulls in every object.  Without the cargo_metadata suppression
    // cc emits a plain `cargo:rustc-link-lib=static=ruxenrt` and our
    // modifier-form directive becomes a *second* link directive for
    // the same archive — on Linux that surfaces as "multiple
    // definition" errors because the archive ends up in the link
    // twice.
    let mut build = cc::Build::new();
    build.opt_level(2).warnings(true).cargo_metadata(false);
    for src in &runtime_sources {
        build.file(src);
    }
    // The future runtime's scheduler.c forward-declares
    // `Future_dynamic_poll` — the symbol Ruxen's AOT Cranelift backend
    // synthesises for any class that opts into `dispatch runtime` (see
    // `synthesize_dynamic_dispatch_helpers`). The REPL doesn't go
    // through that codegen path, so include a weak stub in the
    // archive so the link succeeds. The stub is overridden whenever a
    // real definition lands at link time.
    build.file(PathBuf::from(&crate_dir).join("runtime_stubs.c"));

    // Vendored PCRE2 lives at library/std/regex/runtime/pcre2/. Its
    // .c files #include "config.h", "pcre2.h", "pcre2_internal.h",
    // etc. via bare filenames, so we add the dir to the include
    // path here. HAVE_CONFIG_H tells PCRE2 to consult our
    // hand-authored config.h; PCRE2_CODE_UNIT_WIDTH=8 selects the
    // 8-bit single-width build (no 16/32-bit variants).
    let pcre2_dir = std_root.join("regex").join("runtime").join("pcre2");
    if pcre2_dir.is_dir() {
        build.include(&pcre2_dir);
        build.define("HAVE_CONFIG_H", None);
        build.define("PCRE2_CODE_UNIT_WIDTH", "8");
        // Suppress noisy warnings from vendored upstream code we
        // don't want to patch.
        build.flag_if_supported("-Wno-sign-compare");
        build.flag_if_supported("-Wno-unused-parameter");
        build.flag_if_supported("-Wno-implicit-fallthrough");
    }

    // Pin a single macOS deployment target so the runtime objects in
    // libruxenrt.a carry the same build-version (11.0) as the Cranelift
    // objects (LC_BUILD_VERSION) and the final link. Without this, cc
    // stamps the current SDK (e.g. 26.4) and ld warns the prebuilt
    // archive was "built for a newer macOS version than being linked".
    if cfg!(target_os = "macos") {
        build.flag("-mmacosx-version-min=11.0");
    }

    build.compile("ruxenrt");

    println!("cargo:rustc-link-search=native={out_dir}");

    // Publish OUT_DIR via the `cargo:` metadata channel keyed off the
    // `links = "ruxenrt"` declaration in Cargo.toml. Downstream build
    // scripts (notably ruxen_cli's, which links the unified `ruxen`
    // binary) read this as `DEP_RUXENRT_LIB_DIR` so they can re-emit
    // the same -force_load / -export_dynamic link args for THEIR bin's
    // link command — `cargo:rustc-link-arg` doesn't propagate across
    // package boundaries, but `cargo:<key>=<val>` metadata does.
    println!("cargo:lib_dir={out_dir}");

    if target_os == "macos" || target_os == "ios" {
        // macOS ld doesn't honour `+whole-archive`; use `-force_load`
        // with the absolute archive path instead, and combine with
        // `-export_dynamic` so the surviving symbols enter the dynamic
        // symbol table where dlsym(RTLD_DEFAULT) can see them.
        println!("cargo:rustc-link-arg=-Wl,-force_load,{out_dir}/libruxenrt.a");
        println!("cargo:rustc-link-arg=-Wl,-export_dynamic");
        // std.rand on macOS uses SecRandomCopyBytes from the Security
        // framework (see library/std/rand/runtime/rand.c). The full-program
        // codegen path emits this in compiler/ruxen_core/src/codegen/object.rs;
        // the REPL's libruxenrt.a link needs the same flag.
        //
        // Emit as a raw `-framework Security` link-arg pair so it lands
        // on the cc invocation for every artifact this crate produces
        // (lib, bins, AND integration test binaries). The `link-lib=
        // framework=...` form alone was not propagating to the
        // tests/repl_tests binary's link command on this toolchain.
        println!("cargo:rustc-link-arg=-framework");
        println!("cargo:rustc-link-arg=Security");
        println!("cargo:rustc-link-lib=framework=Security");
    } else {
        // GNU ld / lld: `+whole-archive` modifier on the link-lib
        // directive expands to `--whole-archive -lruxenrt --no-whole-archive`
        // in the rustc-emitted link command, with no risk of the
        // archive being included twice.
        println!("cargo:rustc-link-lib=static:+whole-archive=ruxenrt");
        if target_os == "linux" || target_os == "android" {
            println!("cargo:rustc-link-arg=-Wl,--export-dynamic");
        }
    }

    for src in &runtime_sources {
        println!("cargo:rerun-if-changed={}", src.display());
    }
    println!("cargo:rerun-if-changed={}", std_root.display());
}
