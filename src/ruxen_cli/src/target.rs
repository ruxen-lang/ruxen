//! `ruxen target list/add/remove` — cross-compilation runtime management
//! (tier 4.02).
//!
//! In this release, per-target runtimes are **compiled from source for the
//! target** by `ruxen compile/build --target <triple>` (locally for a Darwin
//! cross, or via the two-stage Docker flow for Linux). The prebuilt-runtime
//! HTTP fetch from a release URL (spec §5.9) is deferred to the WASM/CI phase —
//! see `docs/decisions/cross-compilation-linker-matrix.md` §4.
//!
//! So `add`/`remove` here return a **clear `Err`** rather than a silent no-op
//! (project rule: no silent no-ops for user-callable surface). `list`
//! enumerates whatever runtimes are installed under
//! `~/.ruxen/lib/runtime/<triple>/`.

use std::path::PathBuf;

use crate::cli::TargetAction;

/// The set of first-class triples this release recognizes (for `list --all`).
/// Mirrors `codegen::target`'s accepted set; Android is config-ready (NDK),
/// wasm is the next phase.
const KNOWN_TARGETS: &[(&str, &str)] = &[
    ("aarch64-apple-darwin", "host / Apple Silicon (verified)"),
    ("x86_64-apple-darwin", "cross-arch, Rosetta (verified)"),
    (
        "aarch64-unknown-linux-gnu",
        "cross-OS, two-stage Docker (verified)",
    ),
    (
        "x86_64-unknown-linux-gnu",
        "config-ready, CI-proven-at-push",
    ),
    (
        "aarch64-linux-android",
        "config-ready, NDK-gated (untested)",
    ),
    ("wasm32-unknown-unknown", "next phase (LLVM backend)"),
];

pub fn run(action: TargetAction) -> Result<(), String> {
    match action {
        TargetAction::List { all } => list(all),
        TargetAction::Add { triple } => add(&triple),
        TargetAction::Remove { triple } => remove(&triple),
    }
}

/// `~/.ruxen/lib/runtime/` — the installed per-target runtime root.
fn runtime_root() -> Option<PathBuf> {
    // Prefer RUXEN_HOME, else ~/.ruxen.
    if let Ok(home) = std::env::var("RUXEN_HOME") {
        return Some(PathBuf::from(home).join("lib").join("runtime"));
    }
    let home = std::env::var("HOME").ok()?;
    Some(
        PathBuf::from(home)
            .join(".ruxen")
            .join("lib")
            .join("runtime"),
    )
}

fn list(all: bool) -> Result<(), String> {
    if all {
        println!("Known targets:");
        for (triple, note) in KNOWN_TARGETS {
            println!("  {:<32} {}", triple, note);
        }
        println!(
            "\nRuntimes are compiled from source on demand by \
             `ruxen compile/build --target <triple>`.\nSee docs/CROSS_COMPILE.md."
        );
        return Ok(());
    }

    let Some(root) = runtime_root() else {
        return Err("could not determine ~/.ruxen/lib/runtime (set RUXEN_HOME or HOME)".into());
    };
    if !root.is_dir() {
        println!(
            "No installed target runtimes (looked in {}).",
            root.display()
        );
        println!(
            "Cross builds compile the runtime from source for the target — \
             no install step is required for the verified targets. \
             See docs/CROSS_COMPILE.md."
        );
        return Ok(());
    }

    let mut found = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&root) {
        for e in rd.flatten() {
            if e.path().is_dir() {
                if let Some(name) = e.file_name().to_str() {
                    found.push(name.to_string());
                }
            }
        }
    }
    found.sort();
    if found.is_empty() {
        println!("No installed target runtimes in {}.", root.display());
    } else {
        println!("Installed target runtimes ({}):", root.display());
        for t in found {
            println!("  {}", t);
        }
    }
    Ok(())
}

fn add(triple: &str) -> Result<(), String> {
    // Validate the triple so a typo is caught here, not later.
    let resolved = ruxen_core::codegen::target::ResolvedTarget::resolve(Some(triple))?;
    Err(format!(
        "`ruxen target add {}` is not implemented in this release.\n  \
         Per-target runtimes are compiled from source automatically by \
         `ruxen compile --target {}` (locally for a macOS cross, or via the \
         two-stage Docker flow for Linux — Docker required).\n  \
         The prebuilt-runtime fetch from a release URL is deferred to the \
         WASM/CI phase. See docs/CROSS_COMPILE.md.",
        resolved.canonical(),
        resolved.canonical()
    ))
}

fn remove(triple: &str) -> Result<(), String> {
    let resolved = ruxen_core::codegen::target::ResolvedTarget::resolve(Some(triple))?;
    let Some(root) = runtime_root() else {
        return Err("could not determine ~/.ruxen/lib/runtime (set RUXEN_HOME or HOME)".into());
    };
    let dir = root.join(resolved.canonical());
    if !dir.is_dir() {
        return Err(format!(
            "no installed runtime for '{}' at {} — nothing to remove.",
            resolved.canonical(),
            dir.display()
        ));
    }
    std::fs::remove_dir_all(&dir)
        .map_err(|e| format!("failed to remove {}: {}", dir.display(), e))?;
    println!("Removed runtime for '{}'.", resolved.canonical());
    Ok(())
}
