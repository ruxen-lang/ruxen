//! Dependency management commands: `ruxen add`, `ruxen remove`, `ruxen update`,
//! `ruxen tree`, `ruxen verify`.

use crate::build::find_project_root;
use crate::lock::LockFile;
use crate::manifest::{Dependency, DependencyDetail, Manifest};
use crate::resolve_deps;

/// `ruxen add <piece> [--version] [--git] [--path] [--dev] [--branch] [--tag] [--rev]`
pub fn add(
    piece: &str,
    version: Option<&str>,
    git: Option<&str>,
    path: Option<&str>,
    dev: bool,
    branch: Option<&str>,
    tag: Option<&str>,
    rev: Option<&str>,
) -> Result<(), String> {
    let project_dir = find_project_root()?;
    let mut manifest = Manifest::load(&project_dir)?;

    // Build the dependency spec
    let dep = if let Some(path) = path {
        Dependency::Detailed(DependencyDetail {
            path: Some(path.to_string()),
            ..Default::default()
        })
    } else if let Some(git_url) = git {
        Dependency::Detailed(DependencyDetail {
            git: Some(git_url.to_string()),
            branch: branch.map(|s| s.to_string()),
            tag: tag.map(|s| s.to_string()),
            rev: rev.map(|s| s.to_string()),
            ..Default::default()
        })
    } else if let Some(ver) = version {
        Dependency::Version(ver.to_string())
    } else {
        return Err(
            "specify --git <url>, --path <path>, or --version <ver> for the dependency".to_string(),
        );
    };

    // Determine source type for display
    let source_type = if path.is_some() {
        "path"
    } else if git.is_some() {
        "git"
    } else {
        "registry"
    };

    let dep_section = if dev {
        "[dev-dependencies]"
    } else {
        "[dependencies]"
    };

    // Check for duplicates
    let target_map = if dev {
        &manifest.dev_dependencies
    } else {
        &manifest.dependencies
    };

    if target_map.contains_key(piece) {
        return Err(format!("piece `{}` is already in {}", piece, dep_section));
    }

    // Add the dependency
    if dev {
        manifest.dev_dependencies.insert(piece.to_string(), dep);
    } else {
        manifest.dependencies.insert(piece.to_string(), dep);
    }

    // Save manifest
    manifest.save(&project_dir)?;

    if dev {
        println!(
            "  Adding piece `{}` ({}) to [dev-dependencies]",
            piece, source_type
        );
    } else {
        println!("  Adding piece `{}` ({})", piece, source_type);
    }
    println!("    Updated Ruxen.toml");

    // Re-resolve dependencies
    if !manifest.dependencies.is_empty() {
        println!("  Resolving dependencies...");
        let existing_lock = LockFile::load(&project_dir).ok();
        let result = resolve_deps::resolve(&project_dir, &manifest, existing_lock.as_ref())?;
        result.lock.save(&project_dir)?;
        println!("    Updated Ruxen.lock");
    }

    Ok(())
}

/// `ruxen remove <piece>`
pub fn remove(piece: &str) -> Result<(), String> {
    let project_dir = find_project_root()?;
    let mut manifest = Manifest::load(&project_dir)?;

    let removed_dep = manifest.dependencies.remove(piece).is_some();
    let removed_dev = manifest.dev_dependencies.remove(piece).is_some();

    if !removed_dep && !removed_dev {
        return Err(format!(
            "piece `{}` not found in [dependencies] or [dev-dependencies]",
            piece
        ));
    }

    manifest.save(&project_dir)?;

    println!("  Removing piece `{}`", piece);
    println!("    Updated Ruxen.toml");

    // Re-resolve dependencies
    if !manifest.dependencies.is_empty() {
        let existing_lock = LockFile::load(&project_dir).ok();
        let result = resolve_deps::resolve(&project_dir, &manifest, existing_lock.as_ref())?;
        result.lock.save(&project_dir)?;
        println!("    Updated Ruxen.lock");
    } else {
        // No dependencies left — remove lock file
        let lock_path = project_dir.join("Ruxen.lock");
        if lock_path.exists() {
            let _ = std::fs::remove_file(&lock_path);
            println!("    Removed Ruxen.lock");
        }
    }

    Ok(())
}

/// `ruxen update [<piece>] [--precise <pkg>=<rev>]`
pub fn update(piece: Option<&str>, precise: Option<&str>) -> Result<(), String> {
    let project_dir = find_project_root()?;
    let manifest = Manifest::load(&project_dir)?;

    if manifest.dependencies.is_empty() {
        println!("  No dependencies to update.");
        return Ok(());
    }

    // --precise rewrites a single lockfile entry to a pinned rev. We
    // never combine it with the broader "re-resolve from scratch" path
    // — pinning is a surgical operation, and the spec mandates a
    // target piece. Rejecting the repo-wide form keeps the contract
    // honest (a user typing `ruxen update --precise foo=abc` expects
    // exactly that to land, not a side-effect-laden refresh of every
    // other dep).
    if let Some(spec) = precise {
        let (target_name, rev) = parse_precise(spec)?;
        // The CLI accepts `ruxen update foo --precise bar=abc` syntactically
        // — but the spec says --precise requires a piece target. Treat
        // `piece` as that target when set; otherwise the pkg portion
        // of <pkg>=<rev> IS the target. Mismatches are a user mistake
        // worth flagging.
        let resolved_target = match piece {
            Some(p) if p != target_name => {
                return Err(format!(
                    "`--precise {}={}` names piece `{}`, but the positional argument was `{}`",
                    target_name, rev, target_name, p
                ));
            }
            _ => target_name,
        };
        return apply_precise(&project_dir, &manifest, resolved_target, rev);
    }

    if let Some(name) = piece {
        if !manifest.dependencies.contains_key(name) {
            return Err(format!("piece `{}` not found in [dependencies]", name));
        }
        println!("  Updating piece `{}`", name);
    } else {
        println!("  Updating all dependencies");
    }

    // Re-resolve from scratch (ignore existing lock file to get latest)
    let result = resolve_deps::resolve(&project_dir, &manifest, None)?;
    result.lock.save(&project_dir)?;
    println!("    Updated Ruxen.lock");

    Ok(())
}

/// Split a `<pkg>=<rev>` spec. Both halves must be non-empty.
fn parse_precise(spec: &str) -> Result<(&str, &str), String> {
    let (pkg, rev) = spec.split_once('=').ok_or_else(|| {
        format!(
            "`--precise` requires the form `<pkg>=<rev>` (got `{}`)",
            spec
        )
    })?;
    if pkg.is_empty() || rev.is_empty() {
        return Err(format!(
            "`--precise` requires both sides non-empty (got `{}`)",
            spec
        ));
    }
    Ok((pkg, rev))
}

/// Rewrite the lockfile entry for `target` to pin its source to `rev`.
/// The source string is rewritten from `git+<url>?rev=<old>` to
/// `git+<url>?rev=<new>`; other source shapes (path+...) are rejected
/// since pinning a path dep to a git rev is meaningless.
fn apply_precise(
    project_dir: &std::path::Path,
    manifest: &Manifest,
    target: &str,
    rev: &str,
) -> Result<(), String> {
    if !manifest.dependencies.contains_key(target) {
        return Err(format!(
            "piece `{}` not found in [dependencies]; cannot --precise an undeclared dep",
            target
        ));
    }
    let mut lock = LockFile::load(&project_dir).map_err(|_| {
        "Ruxen.lock not found; run `ruxen build` first to generate it before --precise".to_string()
    })?;
    let entry = lock
        .pieces
        .iter_mut()
        .find(|p| p.name == target)
        .ok_or_else(|| {
            format!(
                "piece `{}` is in Ruxen.toml but not in Ruxen.lock; run `ruxen build` first",
                target
            )
        })?;

    if !entry.source.starts_with("git+") {
        return Err(format!(
            "piece `{}` is not a git dependency (source: `{}`); --precise only applies to git deps",
            target, entry.source
        ));
    }
    // Source shape: `git+<url>?rev=<old>`. Replace everything from
    // `?rev=` onward with the new rev.
    let url_part = entry
        .source
        .split('?')
        .next()
        .unwrap_or(&entry.source)
        .to_string();
    entry.source = format!("{}?rev={}", url_part, rev);
    lock.save(&project_dir)?;
    println!("  Pinned `{}` to rev `{}` in Ruxen.lock", target, rev);
    Ok(())
}

/// `ruxen tree` — display the dependency tree.
pub fn tree() -> Result<(), String> {
    let project_dir = find_project_root()?;
    let manifest = Manifest::load(&project_dir)?;

    if let Some(pkg) = manifest.package.as_ref() {
        println!("{} v{}", pkg.name, pkg.version);
    } else {
        println!("(virtual workspace root)");
    }

    if manifest.dependencies.is_empty() {
        println!("  (no dependencies)");
        return Ok(());
    }

    let lock = LockFile::load(&project_dir).ok();
    let dep_names: Vec<&String> = manifest.dependencies.keys().collect();

    for (i, name) in dep_names.iter().enumerate() {
        let is_last = i == dep_names.len() - 1;
        let prefix = if is_last { "└── " } else { "├── " };

        let version = lock
            .as_ref()
            .and_then(|l| l.find(name))
            .map(|p| p.version.as_str())
            .unwrap_or("*");

        let source = describe_dep_source(manifest.dependencies.get(*name).unwrap());
        println!("{}{} v{} ({})", prefix, name, version, source);

        // Show transitive deps from the lock file
        if let Some(ref l) = lock {
            if let Some(locked) = l.find(name) {
                let child_prefix = if is_last { "    " } else { "│   " };
                for (j, child) in locked.dependencies.iter().enumerate() {
                    let child_last = j == locked.dependencies.len() - 1;
                    let child_sym = if child_last {
                        "└── "
                    } else {
                        "├── "
                    };
                    let child_ver = l.find(child).map(|p| p.version.as_str()).unwrap_or("*");
                    println!("{}{}{} v{}", child_prefix, child_sym, child, child_ver);
                }
            }
        }
    }

    Ok(())
}

/// `ruxen verify` — verify lock file checksums.
pub fn verify() -> Result<(), String> {
    let project_dir = find_project_root()?;

    // If there is no lock file, that's only an error when the manifest
    // actually declares dependencies. For a fresh project with no deps,
    // there is nothing to verify.
    if !project_dir.join("Ruxen.lock").exists() {
        let manifest = Manifest::load(&project_dir)?;
        if manifest.dependencies.is_empty() {
            println!("  verified: no dependencies");
            return Ok(());
        }
        return Err(
            "Ruxen.lock not found; run `ruxen build` to generate it before verifying".to_string(),
        );
    }

    let lock = LockFile::load(&project_dir)?;
    lock.verify_checksums(&project_dir)?;

    let count = lock.pieces.iter().filter(|p| p.checksum.is_some()).count();
    println!("  Verified checksums for {} piece(s). All OK.", count);

    Ok(())
}

fn describe_dep_source(dep: &Dependency) -> String {
    match dep {
        Dependency::Version(v) => format!("registry: {}", v),
        Dependency::Detailed(d) => {
            if let Some(path) = &d.path {
                format!("path: {}", path)
            } else if let Some(git) = &d.git {
                let mut s = format!("git: {}", git);
                if let Some(tag) = &d.tag {
                    s.push_str(&format!(" tag={}", tag));
                } else if let Some(branch) = &d.branch {
                    s.push_str(&format!(" branch={}", branch));
                } else if let Some(rev) = &d.rev {
                    s.push_str(&format!(" rev={}", rev));
                }
                s
            } else if let Some(ver) = &d.version {
                format!("registry: {}", ver)
            } else {
                "unknown".to_string()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::LockedPiece;
    use std::fs;

    #[test]
    fn test_add_and_remove_path_dep() {
        let tmp = std::env::temp_dir().join(format!("ruxen_add_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("src")).unwrap();

        // Create a project
        fs::write(
            tmp.join("Ruxen.toml"),
            "[package]\nname = \"test\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::write(tmp.join("src/main.rx"), "def main\nend\n").unwrap();

        // Create a dependency project
        let dep_dir = tmp.join("my-dep");
        fs::create_dir_all(dep_dir.join("src")).unwrap();
        fs::write(
            dep_dir.join("Ruxen.toml"),
            "[package]\nname = \"my-dep\"\nversion = \"0.2.0\"\n\n[build]\ntype = \"library\"\n",
        )
        .unwrap();
        fs::write(
            dep_dir.join("src/lib.rx"),
            "pub def greet\n  puts \"hi\"\nend\n",
        )
        .unwrap();

        // Manually add the dep to manifest to test parsing
        let manifest_content = format!(
            "[package]\nname = \"test\"\nversion = \"0.1.0\"\n\n[dependencies]\nmy-dep = {{ path = \"{}\" }}\n",
            dep_dir.display()
        );
        fs::write(tmp.join("Ruxen.toml"), &manifest_content).unwrap();

        let manifest = Manifest::load(&tmp).unwrap();
        assert!(manifest.dependencies.contains_key("my-dep"));

        // Remove the dep manually
        let manifest_content = "[package]\nname = \"test\"\nversion = \"0.1.0\"\n";
        fs::write(tmp.join("Ruxen.toml"), manifest_content).unwrap();

        let manifest = Manifest::load(&tmp).unwrap();
        assert!(manifest.dependencies.is_empty());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn precise_parser_rejects_malformed_specs() {
        assert!(parse_precise("foo").is_err());
        assert!(parse_precise("=abc").is_err());
        assert!(parse_precise("foo=").is_err());
        assert_eq!(parse_precise("foo=abc1234").unwrap(), ("foo", "abc1234"));
    }

    #[test]
    fn precise_rewrites_git_rev_in_lockfile() {
        // Stand up a working project with a git dep + a hand-written
        // lockfile so we can drive apply_precise directly without
        // hitting the network.
        let tmp = std::env::temp_dir().join(format!(
            "ruxen_precise_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("src")).unwrap();
        std::fs::write(
            tmp.join("Ruxen.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nfoo = { git = \"https://example.com/foo.git\" }\n",
        )
        .unwrap();
        std::fs::write(tmp.join("src/main.rx"), "def main\nend\n").unwrap();

        let initial_lock = LockFile {
            version: 1,
            pieces: vec![LockedPiece {
                name: "foo".to_string(),
                version: "0.1.0".to_string(),
                source: "git+https://example.com/foo.git?rev=oldrev1".to_string(),
                checksum: None,
                dependencies: vec![],
            }],
        };
        initial_lock.save(&tmp).unwrap();

        let manifest = Manifest::load(&tmp).unwrap();
        apply_precise(&tmp, &manifest, "foo", "newrev9").expect("precise rewrite");
        let reloaded = LockFile::load(&tmp).unwrap();
        let foo = reloaded.find("foo").unwrap();
        assert_eq!(
            foo.source,
            "git+https://example.com/foo.git?rev=newrev9",
            "lockfile entry must be rewritten to the new rev"
        );

        // --precise on an undeclared piece must fail.
        let err = apply_precise(&tmp, &manifest, "ghost", "abc").unwrap_err();
        assert!(
            err.contains("ghost") && err.contains("not found"),
            "expected undeclared-dep error, got: {}",
            err
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_describe_dep_source() {
        assert_eq!(
            describe_dep_source(&Dependency::Version("1.0.0".to_string())),
            "registry: 1.0.0"
        );
        assert_eq!(
            describe_dep_source(&Dependency::Detailed(DependencyDetail {
                path: Some("../lib".to_string()),
                ..Default::default()
            })),
            "path: ../lib"
        );
        assert_eq!(
            describe_dep_source(&Dependency::Detailed(DependencyDetail {
                git: Some("https://github.com/user/repo.git".to_string()),
                tag: Some("v1.0".to_string()),
                ..Default::default()
            })),
            "git: https://github.com/user/repo.git tag=v1.0"
        );
    }
}
