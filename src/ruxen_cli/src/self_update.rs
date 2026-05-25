//! `ruxen upgrade` — self-update entry points.
//!
//! Two paths:
//!   * `from_source(path)`  — rebuild + reinstall from a local ruxen
//!     checkout by invoking that checkout's `install.sh --from-source`.
//!   * `from_release(tag)` — fetch and run the canonical installer
//!     script from GitHub (`curl -fsSL <URL> | bash`), optionally
//!     pinning a release tag.
//!
//! Implementation strategy: in both cases the install.sh shell script
//! is the single source of truth for the install layout — duplicating
//! its build + copy + rename logic in Rust would diverge over time.
//! These wrappers just preflight (path / `curl` / `bash` checks) and
//! shell out, forwarding stdio so users see live installer output.

use std::path::PathBuf;
use std::process::Command;

/// Canonical install.sh URL — matches the one printed by the README
/// and the help text in install.sh itself.
const INSTALL_SH_URL: &str =
    "https://raw.githubusercontent.com/ruxen-lang/ruxen/master/install.sh";

/// Returns Err if `tool` isn't found on PATH. Used as preflight before
/// shelling out — gives a grounded error instead of "No such file or
/// directory" from `Command::status`.
fn require_on_path(tool: &str) -> Result<(), String> {
    // `which`-style lookup without taking a dependency: walk PATH and
    // check each entry for an executable file named `tool`. Honors
    // PATHEXT-style suffixes implicitly via Unix-only is_file check;
    // this CLI doesn't run on Windows today.
    let path_var = std::env::var_os("PATH").ok_or_else(|| "PATH is unset".to_string())?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(tool);
        if candidate.is_file() {
            return Ok(());
        }
    }
    Err(format!(
        "`ruxen upgrade`: required tool `{}` not found on PATH",
        tool
    ))
}

pub fn from_source(path: &str) -> Result<(), String> {
    require_on_path("bash")?;
    // Resolve to an absolute path so any cd in the script doesn't
    // break relative argument resolution.
    let abs_src = std::fs::canonicalize(path).map_err(|e| {
        format!(
            "`ruxen upgrade --from-source {}`: path not found ({})",
            path, e
        )
    })?;

    // Surface checks before we shell out — keeps the error message
    // grounded in the user's command, not a downstream cargo/bash
    // error string.
    if !abs_src.join("Cargo.toml").is_file() {
        return Err(format!(
            "`ruxen upgrade --from-source {}`: not a ruxen checkout — no Cargo.toml at {}",
            path,
            abs_src.display()
        ));
    }
    if !abs_src.join("compiler/ruxen_core").is_dir() {
        return Err(format!(
            "`ruxen upgrade --from-source {}`: doesn't look like ruxen — missing compiler/ruxen_core/ at {}",
            path,
            abs_src.display()
        ));
    }
    let install_sh: PathBuf = abs_src.join("install.sh");
    if !install_sh.is_file() {
        return Err(format!(
            "`ruxen upgrade --from-source {}`: install.sh not found at {}",
            path,
            install_sh.display()
        ));
    }

    // Forward stdout/stderr so the user sees the install.sh progress
    // (cargo build output + per-binary install ticks) live, not as a
    // captured blob on failure.
    let status = Command::new("bash")
        .arg(&install_sh)
        .arg("--from-source")
        .arg(&abs_src)
        .arg("--no-modify-path") // we're already on PATH if this command ran
        .status()
        .map_err(|e| format!("failed to invoke {}: {}", install_sh.display(), e))?;

    if !status.success() {
        return Err(format!(
            "install.sh exited with status {} — toolchain not updated",
            status.code().unwrap_or(-1)
        ));
    }

    Ok(())
}

/// Fetch the canonical `install.sh` over HTTPS and pipe it through
/// bash. When `version` is `Some`, pass `-s -- --version <tag>` so the
/// installer pins that release; otherwise the installer's own default
/// (`latest`) applies.
///
/// We use a single `bash -c "curl ... | bash"` invocation so the shell
/// handles the pipe — implementing the pipe in Rust would require
/// either spawning two children with a stitched-together pipe or
/// pulling in a reqwest-class dependency. Shelling out keeps this a
/// dozen lines and avoids the dep, at the cost of requiring `curl` and
/// `bash` on PATH (both already prerequisites for a from-release
/// install — install.sh itself uses them).
pub fn from_release(version: Option<&str>) -> Result<(), String> {
    require_on_path("curl")?;
    require_on_path("bash")?;

    // Build the pipeline. We deliberately keep this as a single string
    // for `bash -c` rather than spawning two processes and wiring
    // pipes — install.sh exits non-zero on its own error paths and
    // bash propagates that through `set -o pipefail` (`bash -c` enables
    // pipefail-equivalent error propagation: a non-zero exit from the
    // last command in the pipeline becomes the script's exit code).
    //
    // We pass `-s --` to bash so anything after it goes to install.sh
    // as positional args, matching the documented one-liner in
    // install.sh's own header comment.
    let mut pipeline = format!("set -o pipefail; curl -fsSL {} | bash -s --", INSTALL_SH_URL);
    if let Some(tag) = version {
        // install.sh accepts `--version <tag>`. We let bash quote the
        // tag — pass it as its own positional arg so any odd characters
        // don't trip up parsing.
        validate_version_tag(tag)?;
        pipeline.push_str(" --version ");
        pipeline.push_str(tag);
    }

    let status = Command::new("bash")
        .arg("-c")
        .arg(&pipeline)
        .status()
        .map_err(|e| format!("failed to invoke bash for upgrade: {e}"))?;

    if !status.success() {
        return Err(format!(
            "ruxen upgrade: installer exited with status {} — toolchain not updated",
            status.code().unwrap_or(-1)
        ));
    }

    Ok(())
}

/// Reject shell-metacharacters in the `--version` argument before we
/// hand it to `bash -c`. We're not implementing full quoting because
/// the input space is tiny — release tags are `vMAJOR.MINOR.PATCH`
/// (optionally with `-pre.N` style suffixes). Anything outside
/// `[A-Za-z0-9._-]` is rejected with a clear error rather than
/// silently re-interpreted by the shell.
fn validate_version_tag(tag: &str) -> Result<(), String> {
    if tag.is_empty() {
        return Err("`--version`: empty tag".to_string());
    }
    if !tag
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(format!(
            "`--version {}`: tag may only contain ASCII letters, digits, '.', '_', '-'",
            tag
        ));
    }
    Ok(())
}

