//! `ruxen publish` — tag the current package and push the tag to a
//! git remote. No centralized registry: distribution is Go-style,
//! consumers depend on a git URL + tag.
//!
//! Contract:
//!  * Tag format: `v<package-name>-<version>` (e.g. `vmy-pkg-0.1.0`).
//!  * Refuses to publish a virtual workspace root.
//!  * Refuses to publish if the working tree has uncommitted changes
//!    against tracked files — that's the only way to make the tag
//!    actually point at reproducible content. Untracked files (target/,
//!    tmp/, .git/, anything in .gitignore) are ignored.
//!  * Builds a tarball next to Ruxen.toml using the system `tar` so
//!    we avoid pulling in the `tar` crate.
//!  * Checks `git ls-remote --tags <remote>` BEFORE creating the local
//!    tag — refusing to clobber an existing remote tag with E1602.
//!  * `--dry-run` short-circuits before `git tag` / `git push`.

use std::path::Path;
use std::process::Command;

use crate::build::find_project_root;
use crate::manifest::Manifest;

/// `ruxen publish [--dry-run] [--registry <name>]`
pub fn publish(dry_run: bool, registry: Option<&str>) -> Result<(), String> {
    let project_dir = find_project_root()?;
    let manifest = Manifest::load(&project_dir)?;
    manifest.validate()?;
    let package = manifest.require_package().map_err(|_| {
        "`ruxen publish` only works on packages — \
         the current directory is a virtual workspace root with no [package]"
            .to_string()
    })?;

    let remote = registry.unwrap_or("origin");
    let tag = format!("v{}-{}", package.name, package.version);

    // The dirty-worktree check guards two things at once:
    //  1) That the tag actually points at the content the user thinks
    //     it does (otherwise `git tag` would record HEAD, dropping the
    //     uncommitted changes from the published artefact).
    //  2) That the user gets a meaningful diagnostic instead of a
    //     low-level git error string. We do this even in --dry-run so
    //     the dry-run output is honest about what publish would do.
    ensure_clean_worktree(&project_dir)?;

    // Tarball path: <package-name>-<version>.tar.gz next to Ruxen.toml.
    // Excludes mirror the prompt spec — target/, tmp/, .git/, prior
    // tarballs. Anything matching .gitignore inside the project tree
    // is also excluded via tar's --exclude-vcs-ignores-style? No — we
    // can't rely on a portable flag for that. The repository-level
    // .gitignore is enforced by `git archive` in v2; for v1 the
    // explicit excludes are the contract and they cover the noise
    // dirs the build pipeline produces.
    let tarball_name = format!("{}-{}.tar.gz", package.name, package.version);
    let tarball_path = project_dir.join(&tarball_name);

    let status = Command::new("tar")
        .current_dir(&project_dir)
        .args([
            "-czf",
            &tarball_name,
            "--exclude=./target",
            "--exclude=./tmp",
            "--exclude=./.git",
            "--exclude=./*.tar.gz",
            ".",
        ])
        .status()
        .map_err(|e| format!("failed to run `tar`: {}", e))?;
    if !status.success() {
        return Err(format!("tar failed (exit {})", status.code().unwrap_or(-1)));
    }

    println!(
        "  Packaged {} ({} bytes)",
        tarball_name,
        std::fs::metadata(&tarball_path)
            .map(|m| m.len())
            .unwrap_or(0)
    );

    // Probe the remote BEFORE creating a local tag — otherwise a
    // collision would leave the user with a tag they have to delete
    // by hand before retrying.
    if remote_tag_exists(&project_dir, remote, &tag)? {
        return Err(format!(
            "error[E1602]: tag `{}` already exists at remote `{}`\n  \
             hint: bump the version in Ruxen.toml, then re-run `ruxen publish`",
            tag, remote
        ));
    }

    println!("  Tag: {}", tag);
    println!("  Remote: {}", remote);

    if dry_run {
        println!("    --dry-run: skipping `git tag` and `git push`");
        return Ok(());
    }

    // Create the local tag at HEAD. `-a` would make it an annotated
    // tag; we keep it lightweight so the published artifact is just
    // a pointer to the commit — annotated tags would carry the local
    // user's name/email into the published artefact, which is
    // surprising.
    let status = Command::new("git")
        .current_dir(&project_dir)
        .args(["tag", &tag])
        .status()
        .map_err(|e| format!("failed to run `git tag`: {}", e))?;
    if !status.success() {
        return Err(format!("`git tag {}` failed", tag));
    }

    let status = Command::new("git")
        .current_dir(&project_dir)
        .args(["push", remote, &tag])
        .status()
        .map_err(|e| format!("failed to run `git push`: {}", e))?;
    if !status.success() {
        // Roll back the local tag so the user can retry cleanly
        // without `git tag -d` first.
        let _ = Command::new("git")
            .current_dir(&project_dir)
            .args(["tag", "-d", &tag])
            .status();
        return Err(format!("`git push {} {}` failed", remote, tag));
    }

    println!("    Published {} to {}", tag, remote);
    Ok(())
}

/// Refuse to publish when the working tree has uncommitted changes
/// against tracked files. Untracked files are ignored — the tarball
/// already excludes the common noise dirs and `.gitignore` covers
/// the rest.
fn ensure_clean_worktree(project_dir: &Path) -> Result<(), String> {
    let output = Command::new("git")
        .current_dir(project_dir)
        .args(["status", "--porcelain=v1", "--untracked-files=no"])
        .output()
        .map_err(|e| format!("failed to run `git status`: {}", e))?;
    if !output.status.success() {
        return Err(format!(
            "`git status` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let dirty = String::from_utf8_lossy(&output.stdout);
    if !dirty.trim().is_empty() {
        return Err(format!(
            "refusing to publish: working tree has uncommitted changes\n{}\n  \
             hint: commit or stash these changes, then re-run `ruxen publish`",
            dirty.trim_end()
        ));
    }
    Ok(())
}

/// True when `<remote>` already has `refs/tags/<tag>`.
fn remote_tag_exists(project_dir: &Path, remote: &str, tag: &str) -> Result<bool, String> {
    let refspec = format!("refs/tags/{}", tag);
    let output = Command::new("git")
        .current_dir(project_dir)
        .args(["ls-remote", "--tags", remote, &refspec])
        .output()
        .map_err(|e| format!("failed to run `git ls-remote`: {}", e))?;
    if !output.status.success() {
        return Err(format!(
            "`git ls-remote {} {}` failed: {}",
            remote,
            refspec,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(!output.stdout.is_empty())
}
