//! `riven update --from-source <path>` — rebuild + reinstall the
//! toolchain from a local riven checkout.
//!
//! Implementation strategy: invoke the existing `install.sh
//! --from-source <path>` shell script (the same one a fresh-install
//! user would run). install.sh is the single source of truth for the
//! install layout — duplicating its build + copy + rename logic in
//! Rust would diverge over time. The wrapper just locates the script
//! at `<path>/install.sh`, validates the surface, and shells out.

use std::path::PathBuf;
use std::process::Command;

pub fn from_source(path: &str) -> Result<(), String> {
    // Resolve to an absolute path so any cd in the script doesn't
    // break relative argument resolution.
    let abs_src = std::fs::canonicalize(path).map_err(|e| {
        format!(
            "`riven update --from-source {}`: path not found ({})",
            path, e
        )
    })?;

    // Surface checks before we shell out — keeps the error message
    // grounded in the user's command, not a downstream cargo/bash
    // error string.
    if !abs_src.join("Cargo.toml").is_file() {
        return Err(format!(
            "`riven update --from-source {}`: not a riven checkout — no Cargo.toml at {}",
            path,
            abs_src.display()
        ));
    }
    if !abs_src.join("compiler/riven_core").is_dir() {
        return Err(format!(
            "`riven update --from-source {}`: doesn't look like riven — missing compiler/riven_core/ at {}",
            path,
            abs_src.display()
        ));
    }
    let install_sh: PathBuf = abs_src.join("install.sh");
    if !install_sh.is_file() {
        return Err(format!(
            "`riven update --from-source {}`: install.sh not found at {}",
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
