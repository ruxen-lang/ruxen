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

/// Apply the vendored-PCRE2 include path + width defines to a `cc -c`
/// invocation when `runtime_c_path` is a PCRE2 vendor source.
///
/// PCRE2's `.c` files `#include "config.h"`, `"pcre2.h"`,
/// `"pcre2_internal.h"`, etc. via bare filenames, so the vendor dir goes on
/// the include path. `HAVE_CONFIG_H` selects our hand-authored config.h;
/// `PCRE2_CODE_UNIT_WIDTH=8` selects the 8-bit build (matches the REPL JIT
/// build in `src/ruxen_repl/build.rs`). The detection key is a path component
/// literally named `pcre2` — covers the vendor tree, never the user-authored
/// `library/std/regex/runtime/regex.c`. Shared by the host and cross runtime
/// compile paths so they stay in lockstep.
fn apply_pcre2_flags(cmd: &mut Command, runtime_c_path: &Path) {
    if runtime_c_path
        .components()
        .any(|c| c.as_os_str() == "pcre2")
    {
        if let Some(parent) = runtime_c_path.parent() {
            cmd.arg("-I").arg(parent);
        }
        cmd.arg("-DHAVE_CONFIG_H=1")
            .arg("-DPCRE2_CODE_UNIT_WIDTH=8")
            .arg("-Wno-sign-compare")
            .arg("-Wno-unused-parameter")
            .arg("-Wno-implicit-fallthrough");
    }
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

    // Pin the macOS deployment target to match libruxenrt.a's prebuilt
    // objects (ruxen_repl/build.rs), the Cranelift LC_BUILD_VERSION, and
    // the link — all 11.0 — so ld never reports a version skew.
    #[cfg(target_os = "macos")]
    cmd.arg("-mmacosx-version-min=11.0");

    if sanitize {
        cmd.arg("-fsanitize=address,undefined")
            .arg("-g")
            .arg("-fno-omit-frame-pointer");
    } else {
        cmd.arg("-O2");
    }

    apply_pcre2_flags(&mut cmd, runtime_c_path);

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
    // Match the deployment target of every input object (11.0) so the link
    // target isn't the lower OS-major default, which makes ld flag the
    // prebuilt archive as "built for a newer macOS version".
    #[cfg(target_os = "macos")]
    cmd.arg("-mmacosx-version-min=11.0");

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

// ─────────────────────────────────────────────────────────────────────────
// Cross-compilation linking (tier 4.02)
// ─────────────────────────────────────────────────────────────────────────

use crate::codegen::target::{LinkerSpec, ResolvedTarget};

/// Compile one runtime `.c` for a cross target using a target-aware `cc`.
///
/// `target_args` are the leading flags from the resolved [`LinkerSpec`]
/// (e.g. `-arch x86_64` for a Darwin cross). The same `cc` driver that links
/// also compiles the runtime, so the runtime object matches the target ABI.
/// Only used on the *local* cross path (Darwin→Darwin); the container path
/// compiles the runtime inside the container instead (see
/// [`emit_executable_in_container`]).
pub fn compile_runtime_for_target(
    runtime_c_path: &Path,
    target: &ResolvedTarget,
    target_args: &[String],
) -> Result<PathBuf, String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stem = runtime_c_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("runtime");
    let runtime_o = std::env::temp_dir().join(format!(
        "ruxen_{}_{}_{}_{}.o",
        stem,
        target.canonical().replace('-', "_"),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
    ));

    let mut cmd = Command::new("cc");
    for a in target_args {
        cmd.arg(a);
    }
    cmd.arg("-c").arg(runtime_c_path).arg("-o").arg(&runtime_o);
    // Darwin cross still pins the deployment floor so ld doesn't complain.
    if target.is_darwin() {
        cmd.arg("-mmacosx-version-min=11.0");
    }
    cmd.arg("-O2");
    // Vendored PCRE2 needs the same include path + width defines the host
    // `compile_runtime` applies; without them `pcre2_internal.h` aborts the
    // compile ("PCRE2_CODE_UNIT_WIDTH must be defined").
    apply_pcre2_flags(&mut cmd, runtime_c_path);

    let status = cmd
        .status()
        .map_err(|e| format!("Failed to invoke cc for {}: {}", runtime_c_path.display(), e))?;
    if !status.success() {
        return Err(format!(
            "Failed to compile runtime {} for target {}",
            runtime_c_path.display(),
            target.canonical()
        ));
    }
    Ok(runtime_o)
}

/// Link a cross-compiled executable using a resolved [`LinkerSpec`].
///
/// Dispatches on `spec.needs_container`:
/// - local link → spawn `spec.program` with `spec.target_args` (Darwin cross
///   via `cc -arch`, or a host-arch Linux `cc`, or an on-PATH cross gcc).
/// - container link → the two-stage Docker flow ([`emit_executable_in_container`]).
///
/// `runtime_sources` are the stdlib `.c` paths; on the local path they must
/// already be compiled to `runtime_objects`. On the container path they are
/// compiled *inside* the container (the host has no target cc), so the caller
/// passes the source paths and an empty `runtime_objects`.
#[allow(clippy::too_many_arguments)]
pub fn emit_executable_for_target(
    object_bytes: &[u8],
    runtime_objects: &[PathBuf],
    runtime_sources: &[PathBuf],
    output_path: &str,
    target: &ResolvedTarget,
    spec: &LinkerSpec,
    extra_link_flags: &[String],
) -> Result<(), String> {
    if spec.needs_container {
        return emit_executable_in_container(
            object_bytes,
            runtime_sources,
            output_path,
            target,
            extra_link_flags,
        );
    }

    // Local cross link (Darwin→Darwin, native Linux, or on-PATH cross gcc).
    let obj_path = format!("{}.o", output_path);
    std::fs::write(&obj_path, object_bytes)
        .map_err(|e| format!("Failed to write object file: {}", e))?;

    let mut cmd = Command::new(&spec.program);
    for a in &spec.target_args {
        cmd.arg(a);
    }
    cmd.arg(&obj_path);
    for runtime_o in runtime_objects {
        cmd.arg(runtime_o);
    }
    cmd.arg("-o").arg(output_path);
    if target.is_darwin() {
        cmd.arg("-mmacosx-version-min=11.0");
        // std::rand uses SecRandomCopyBytes; the Security framework must be
        // linked. (Cross-platform Linux targets don't link this.)
        cmd.arg("-framework").arg("Security");
    }
    for flag in extra_link_flags {
        cmd.arg(flag);
    }

    let status = cmd.status().map_err(|e| {
        format!(
            "Failed to invoke cross linker '{}' for {}: {}",
            spec.program,
            target.canonical(),
            e
        )
    })?;
    if !status.success() {
        return Err(format!(
            "Cross-linking failed for '{}' (target {})",
            output_path,
            target.canonical()
        ));
    }

    let _ = std::fs::remove_file(&obj_path);
    for runtime_o in runtime_objects {
        let _ = std::fs::remove_file(runtime_o);
    }
    Ok(())
}

/// Two-stage Docker link: emit the target object locally, then compile the
/// stdlib runtime `.c` *and* link inside a target-native container.
///
/// This is the honest fallback for Linux targets from a macOS host with no
/// cross toolchain installed (`zig`/`aarch64-linux-gnu-gcc` absent). The
/// container's native `cc` + libc compile the runtime for the target and link
/// the final binary — no host cross-cc required.
///
/// Mounting strategy: the stdlib root (`library/std`) is mounted read-only at
/// `/std` **preserving directory structure**, so each runtime `.c`'s relative
/// `#include "../../core/runtime/runtime.h"` resolves exactly as it does on
/// the host. A writable scratch dir is mounted at `/work` for the staged
/// object, per-source `.o`s, and the final binary. PCRE2 vendor sources get
/// the same include/width flags as the host (`apply_pcre2_flags` analogue,
/// inlined into the in-container shell). User-supplied runtime sources (which
/// live outside the stdlib tree) are staged flat into `/work` — they are
/// standalone TUs by the project convention.
///
/// Requires Docker. The error when Docker is missing is actionable (points at
/// the manifest linker override) — never a silent failure.
fn emit_executable_in_container(
    object_bytes: &[u8],
    runtime_sources: &[PathBuf],
    output_path: &str,
    target: &ResolvedTarget,
    extra_link_flags: &[String],
) -> Result<(), String> {
    use crate::codegen::target::docker_platform;

    let platform = docker_platform(target);
    let stdlib_root = super::find_stdlib_root()?;
    let stdlib_root = stdlib_root
        .canonicalize()
        .map_err(|e| format!("Failed to canonicalize stdlib root: {}", e))?;

    let scratch = std::env::temp_dir().join(format!(
        "ruxen_xlink_{}_{}",
        std::process::id(),
        target.canonical().replace('-', "_")
    ));
    std::fs::create_dir_all(&scratch)
        .map_err(|e| format!("Failed to create cross-link scratch dir: {}", e))?;
    let guard = ScratchGuard(scratch.clone());

    let obj_name = "ruxen_main.o";
    std::fs::write(scratch.join(obj_name), object_bytes)
        .map_err(|e| format!("Failed to stage object: {}", e))?;

    // Partition runtime sources: stdlib (under stdlib_root → compiled in place
    // at /std/<rel>) vs. user (outside → staged flat into /work).
    let mut compile_lines: Vec<String> = Vec::new();
    let mut object_names: Vec<String> = Vec::new();
    let mut user_staged = 0usize;
    for src in runtime_sources {
        let canon = src
            .canonicalize()
            .map_err(|e| format!("runtime source {} missing: {}", src.display(), e))?;
        let is_pcre2 = canon.components().any(|c| c.as_os_str() == "pcre2");
        let pcre2_flags = if is_pcre2 {
            // Vendor dir is the .c's own parent inside the container.
            "-I\"$(dirname \"$SRC\")\" -DHAVE_CONFIG_H=1 -DPCRE2_CODE_UNIT_WIDTH=8 \
             -Wno-sign-compare -Wno-unused-parameter -Wno-implicit-fallthrough"
        } else {
            ""
        };

        let (container_src, obj) = if let Ok(rel) = canon.strip_prefix(&stdlib_root) {
            // Stdlib source: compiled in place under the read-only /std mount.
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            let obj = format!("/work/std_{}.o", user_obj_safe(&rel_str));
            (format!("/std/{}", rel_str), obj)
        } else {
            // User source: stage flat into scratch.
            let fname = canon
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or_else(|| format!("bad runtime source path {}", canon.display()))?;
            std::fs::copy(&canon, scratch.join(fname))
                .map_err(|e| format!("Failed to stage user runtime {}: {}", canon.display(), e))?;
            user_staged += 1;
            let obj = format!("/work/user_{}.o", user_obj_safe(fname));
            (format!("/work/{}", fname), obj)
        };

        // `SRC=...; cc -c "$SRC" ...` so the PCRE2 `-I$(dirname $SRC)` resolves.
        compile_lines.push(format!(
            "SRC={}; cc -O2 {} -c \"$SRC\" -o {}",
            shell_quote(&container_src),
            pcre2_flags,
            shell_quote(&obj)
        ));
        object_names.push(obj);
    }
    let _ = user_staged;

    let out_name = Path::new(output_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("a.out");

    // Build the in-container script: compile each runtime source, then link
    // the staged main object + every runtime object into the final binary.
    let mut script = String::from("set -e\n");
    for line in &compile_lines {
        script.push_str(line);
        script.push('\n');
    }
    script.push_str("cc -O2 /work/");
    script.push_str(obj_name);
    for obj in &object_names {
        script.push(' ');
        script.push_str(&shell_quote(obj));
    }
    script.push_str(" -o /work/");
    script.push_str(out_name);
    for flag in extra_link_flags {
        // Drop macOS-only framework flags that don't exist on Linux.
        if flag == "-framework" || flag == "Security" {
            continue;
        }
        script.push(' ');
        script.push_str(&shell_quote(flag));
    }
    script.push('\n');

    let work_mount = format!("{}:/work", scratch.display());
    let std_mount = format!("{}:/std:ro", stdlib_root.display());
    let mut cmd = Command::new("docker");
    cmd.arg("run")
        .arg("--rm")
        .arg("--platform")
        .arg(platform)
        .arg("-v")
        .arg(&work_mount)
        .arg("-v")
        .arg(&std_mount)
        .arg("-w")
        .arg("/work")
        // A small image with a C toolchain. gcc:13 ships cc + glibc + libm.
        .arg("gcc:13")
        .arg("bash")
        .arg("-c")
        .arg(&script);

    let output = cmd.output().map_err(|e| {
        format!(
            "Failed to invoke docker for container cross-link of '{}': {}. \
             Install Docker or set [target.{}].linker in Ruxen.toml.",
            target.canonical(),
            e,
            target.canonical()
        )
    })?;
    if !output.status.success() {
        return Err(format!(
            "Container cross-link failed for '{}' (target {}):\n{}",
            output_path,
            target.canonical(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    // Copy the produced binary out of the scratch mount to the final path.
    std::fs::copy(scratch.join(out_name), output_path).map_err(|e| {
        format!(
            "Cross-link produced no binary for '{}': {}",
            output_path, e
        )
    })?;
    // Preserve the executable bit.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(output_path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o755);
            let _ = std::fs::set_permissions(output_path, perms);
        }
    }
    drop(guard);
    Ok(())
}

/// RAII cleanup for the container-link scratch directory.
struct ScratchGuard(PathBuf);
impl Drop for ScratchGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Turn an arbitrary path-ish string into a flat, filesystem-safe object stem
/// (so two runtime sources with the same basename in different package dirs
/// don't collide in `/work`).
fn user_obj_safe(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// Single-quote a string for a POSIX `bash -c` script (closes/reopens around
/// embedded single quotes). Inputs here are paths/flags we control, but
/// quoting keeps spaces and shell metacharacters inert.
fn shell_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
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
