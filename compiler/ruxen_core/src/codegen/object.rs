//! Object file emission and native linking.

use std::path::{Path, PathBuf};
use std::process::Command;

fn linker_args(sanitize: bool, extra_link_flags: &[String]) -> Vec<String> {
    // B3 of `docs/specs/system/zero_rust_stdlib_classes.spec.md`:
    // `-lc / -lm / -lpthread` are now declared in per-package
    // `Ruxen.toml` `[system_libs]` tables (`library/std/core/Ruxen.toml`,
    // `library/std/sync/Ruxen.toml`). `codegen::compile_with_options`
    // aggregates them into `extra_link_flags` BEFORE calling this
    // function, so they arrive here in the caller-supplied slice
    // alongside FFI-attribute flags from `@[link("...")]`. Only
    // sanitizer + macOS framework flags remain compiler-resident
    // because they are not package-specific.
    let mut args: Vec<String> = Vec::new();
    // Phase 2 #06.5 T8: std::rand on macOS uses SecRandomCopyBytes from
    // the Security framework. The runtime's `#include <Security/...>`
    // covers the header side; the linker still needs the framework
    // flag to resolve the symbol at link time.
    if cfg!(target_os = "macos") {
        args.push("-framework".to_string());
        args.push("Security".to_string());
    }
    if sanitize {
        args.push("-fsanitize=address,undefined".to_string());
    }
    args.extend(extra_link_flags.iter().cloned());
    args
}

/// Compile a single C runtime source to an object file.
///
/// When `sanitize` is true, the file is compiled with AddressSanitizer
/// and UndefinedBehaviorSanitizer instrumentation. Kept as the primitive
/// shared by both the singleton path (legacy unity-build callers via
/// [`compile_runtime`]) and the multi-source path
/// ([`compile_runtime_sources`]) introduced in #06.95 Phase B-2.
pub fn compile_runtime(runtime_c_path: &Path, sanitize: bool) -> Result<PathBuf, String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stem = runtime_c_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("runtime");
    let runtime_o = std::env::temp_dir().join(format!(
        "ruxen_{}_{}_{}.o",
        stem,
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
    ));

    let mut cmd = Command::new("cc");
    cmd.arg("-c").arg(runtime_c_path).arg("-o").arg(&runtime_o);

    if sanitize {
        cmd.arg("-fsanitize=address,undefined")
            .arg("-g")
            .arg("-fno-omit-frame-pointer");
    } else {
        cmd.arg("-O2");
    }

    let status = cmd.status().map_err(|e| {
        format!(
            "Failed to invoke cc for {}: {}",
            runtime_c_path.display(),
            e
        )
    })?;

    if !status.success() {
        return Err(format!("Failed to compile {}", runtime_c_path.display()));
    }

    Ok(runtime_o)
}

/// Compile every per-package C runtime source to its own object file.
///
/// After #06.95 Phase B-2, each stdlib package's `.c` files are
/// standalone translation units (they `#include` the shared
/// `library/std/core/runtime/runtime.h` for cross-package type and
/// function declarations). This walks the source list, compiles each
/// to a uniquely named `.o`, and returns the full set for the linker.
///
/// On failure, any objects already produced are best-effort removed.
pub fn compile_runtime_sources(
    sources: &[PathBuf],
    sanitize: bool,
) -> Result<Vec<PathBuf>, String> {
    let mut objects: Vec<PathBuf> = Vec::with_capacity(sources.len());
    for src in sources {
        match compile_runtime(src, sanitize) {
            Ok(o) => objects.push(o),
            Err(e) => {
                for partial in &objects {
                    let _ = std::fs::remove_file(partial);
                }
                return Err(e);
            }
        }
    }
    Ok(objects)
}

/// Write object bytes to a file and link with the runtime into an executable.
///
/// When `sanitize` is true, the linker is invoked with sanitizer flags so that
/// the sanitizer runtime is linked into the final binary.
///
/// `extra_link_flags` provides additional linker flags (e.g., `-lfoo` from
/// `@[link("foo")]` FFI attributes).
pub fn emit_executable(
    object_bytes: &[u8],
    runtime_objects: &[PathBuf],
    output_path: &str,
    sanitize: bool,
    extra_link_flags: &[String],
) -> Result<(), String> {
    let obj_path = format!("{}.o", output_path);

    std::fs::write(&obj_path, object_bytes)
        .map_err(|e| format!("Failed to write object file: {}", e))?;

    let mut cmd = Command::new("cc");
    cmd.arg(&obj_path);
    for runtime_o in runtime_objects {
        cmd.arg(runtime_o);
    }
    cmd.arg("-o").arg(output_path);

    for arg in linker_args(sanitize, extra_link_flags) {
        cmd.arg(arg);
    }

    let status = cmd
        .status()
        .map_err(|e| format!("Failed to invoke linker: {}", e))?;

    if !status.success() {
        return Err(format!("Linking failed for '{}'", output_path));
    }

    let _ = std::fs::remove_file(&obj_path);
    for runtime_o in runtime_objects {
        let _ = std::fs::remove_file(runtime_o);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::linker_args;

    // B3 of docs/specs/system/zero_rust_stdlib_classes.spec.md moved
    // `-lc / -lm / -lpthread` out of `linker_args` into per-package
    // Ruxen.toml `[system_libs]` tables. The aggregation now happens
    // upstream in `codegen::compile_with_options` and arrives here
    // through the `extra_link_flags` argument — so `linker_args(_,
    // &["-lpthread", "-lc", "-lm"])` is now the right contract to
    // assert. See `compiler/ruxen_core/tests/linker_system_libs.rs`
    // for the package-walk pin.
    #[test]
    fn linker_args_propagate_pthread_through_extra_flags() {
        let args = linker_args(false, &[String::from("-lpthread")]);
        assert!(
            args.iter().any(|arg| arg == "-lpthread"),
            "extra_link_flags must reach the final arg list; got {args:?}"
        );
    }

    #[test]
    fn linker_args_preserve_sanitizers_and_extra_flags() {
        let args = linker_args(
            true,
            &[
                String::from("-lpthread"),
                String::from("-lssl"),
                String::from("-lcrypto"),
            ],
        );
        assert!(args.iter().any(|arg| arg == "-lpthread"));
        assert!(args.iter().any(|arg| arg == "-fsanitize=address,undefined"));
        assert!(args.iter().any(|arg| arg == "-lssl"));
        assert!(args.iter().any(|arg| arg == "-lcrypto"));
    }

    /// Phase 2 #06.5 T8: macOS link must include `-framework Security`
    /// so SecRandomCopyBytes (used by std::rand) resolves at link
    /// time. The flag is `cfg!(target_os = "macos")`-gated; this test
    /// only enforces the contract on macOS hosts.
    #[cfg(target_os = "macos")]
    #[test]
    fn linker_args_include_security_framework_on_macos() {
        let args = linker_args(false, &[]);
        let security_pair = args
            .windows(2)
            .any(|w| w[0] == "-framework" && w[1] == "Security");
        assert!(
            security_pair,
            "macOS linker_args must emit `-framework Security` as a pair; got {args:?}"
        );
    }
}
