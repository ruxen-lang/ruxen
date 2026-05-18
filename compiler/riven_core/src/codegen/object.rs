//! Object file emission and native linking.

use std::path::{Path, PathBuf};
use std::process::Command;

fn linker_args(sanitize: bool, extra_link_flags: &[String]) -> Vec<String> {
    let mut args = vec![
        "-lc".to_string(),
        "-lm".to_string(),
        "-lpthread".to_string(),
    ];
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

/// Compile the C runtime to an object file, returning its path.
///
/// When `sanitize` is true, the runtime is compiled with AddressSanitizer
/// and UndefinedBehaviorSanitizer instrumentation for testing.
pub fn compile_runtime(runtime_c_path: &Path, sanitize: bool) -> Result<PathBuf, String> {
    // Use a unique temp file per invocation to avoid race conditions in parallel tests
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let runtime_o = std::env::temp_dir().join(format!(
        "riven_runtime_{}_{}.o",
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

    let status = cmd
        .status()
        .map_err(|e| format!("Failed to invoke cc for runtime: {}", e))?;

    if !status.success() {
        return Err("Failed to compile runtime.c".to_string());
    }

    Ok(runtime_o)
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
    runtime_o: &Path,
    output_path: &str,
    sanitize: bool,
    extra_link_flags: &[String],
) -> Result<(), String> {
    let obj_path = format!("{}.o", output_path);

    std::fs::write(&obj_path, object_bytes)
        .map_err(|e| format!("Failed to write object file: {}", e))?;

    let mut cmd = Command::new("cc");
    cmd.arg(&obj_path).arg(runtime_o).arg("-o").arg(output_path);

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
    let _ = std::fs::remove_file(runtime_o);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::linker_args;

    #[test]
    fn linker_args_always_include_pthread() {
        let args = linker_args(false, &[]);
        assert!(args.iter().any(|arg| arg == "-lpthread"));
    }

    #[test]
    fn linker_args_preserve_sanitizers_and_extra_flags() {
        let args = linker_args(true, &[String::from("-lssl"), String::from("-lcrypto")]);
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
