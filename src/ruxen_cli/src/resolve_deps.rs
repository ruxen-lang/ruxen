//! Dependency resolution — graph building, topological sort, cycle detection,
//! git/path dependency fetching.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::lock::{LockFile, LockedPiece};
use crate::manifest::{Dependency, DependencyDetail, Manifest};
use crate::rlib;

// ─── Security-boundary validators ──────────────────────────────────────────
//
// Every field of `DependencyDetail` is attacker-controlled: a malicious
// `Ruxen.toml` shipped from a registry / git source can inject arbitrary
// argv into the `git` binary (`--upload-pack=…` style RCE,
// CVE-2017-1000117-class) or read arbitrary filesystem paths through
// path-dep traversal. These helpers gate every untrusted string before
// it crosses the `Command::new("git")` / `Path::new(…)` boundary.

/// Reject URLs starting with `-` (would be misread as a `git` flag) and
/// restrict to known-safe schemes. The `--` argv separator added at the
/// call site is the second line of defence; this check is the first.
fn validate_git_url(url: &str) -> Result<(), String> {
    if url.starts_with('-') {
        return Err(format!(
            "git URL `{}` starts with `-` and would be interpreted as a flag by git",
            url
        ));
    }
    let scheme_ok = url.starts_with("https://")
        || url.starts_with("http://")
        || url.starts_with("ssh://")
        || url.starts_with("git://")
        || url.starts_with("git@")
        || url.starts_with("file://");
    if !scheme_ok {
        return Err(format!(
            "git URL `{}` must use https://, http://, ssh://, git://, file://, or `git@` scheme",
            url
        ));
    }
    Ok(())
}

/// Validate a git ref (branch / tag / rev). Conservative allowlist —
/// matches the character class git itself accepts for branch names plus
/// the `~`/`^` suffix syntax. Rejects anything that could be mistaken
/// for a flag.
fn validate_git_ref(r: &str) -> Result<(), String> {
    if r.is_empty() {
        return Err("git ref must not be empty".into());
    }
    if r.starts_with('-') {
        return Err(format!(
            "git ref `{}` starts with `-` and would be interpreted as a flag",
            r
        ));
    }
    // Allow [A-Za-z0-9_./+@~^-] — covers branch names, tags, commit
    // SHAs, and ~/^ rev suffixes. Anything else is rejected.
    if !r.chars().all(|c| {
        c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '+' | '@' | '~' | '^' | '-')
    }) {
        return Err(format!(
            "git ref `{}` contains characters outside [A-Za-z0-9_./+@~^-]",
            r
        ));
    }
    Ok(())
}

/// Decide whether a path dep is safe to consume without an explicit
/// `--allow-external-path` opt-in.
///
/// Threat model: a malicious upstream package ships a `Ruxen.toml` with
/// `path = "/etc/shadow"` or similar absolute path. Refusing absolute
/// paths blocks that. Relative paths (including `..` traversal) stay
/// allowed because legitimate sibling-workspace patterns
/// (`apps/foo` depending on `../../libs/bar`) need them and the
/// project_dir-confined check that would block those is too strict for
/// real layouts.
#[allow(dead_code)] // referenced from resolve_path_dep behind an env-var gate
fn validate_path_dep(name: &str, raw: &str, _project_dir: &Path) -> Result<(), String> {
    if Path::new(raw).is_absolute() {
        return Err(format!(
            "path dependency `{}` uses absolute path `{}` — only relative paths are allowed by default (set `RUXEN_ALLOW_EXTERNAL_PATH=1` to override)",
            name, raw
        ));
    }
    Ok(())
}

/// A resolved dependency with all information needed to compile it.
#[derive(Debug, Clone)]
pub struct ResolvedDep {
    pub name: String,
    pub version: String,
    pub source_dir: PathBuf,
    pub is_path: bool,
    pub checksum: Option<String>,
    /// Names of dependencies this piece depends on.
    pub dependencies: Vec<String>,
}

/// The result of dependency resolution.
#[derive(Debug)]
pub struct ResolveResult {
    /// Dependencies in topological order (leaves first, root last).
    pub deps: Vec<ResolvedDep>,
    /// Generated lock file.
    pub lock: LockFile,
}

/// Resolve all dependencies for a project.
pub fn resolve(
    project_dir: &Path,
    manifest: &Manifest,
    existing_lock: Option<&LockFile>,
) -> Result<ResolveResult, String> {
    let deps_dir = project_dir.join("target").join("deps");
    std::fs::create_dir_all(&deps_dir)
        .map_err(|e| format!("failed to create deps directory: {}", e))?;

    let mut resolved: BTreeMap<String, ResolvedDep> = BTreeMap::new();
    let mut in_flight: HashSet<String> = HashSet::new();

    // Resolve each direct dependency (cycle detection happens inline)
    for (name, dep) in &manifest.dependencies {
        resolve_dep(
            name,
            dep,
            project_dir,
            &deps_dir,
            existing_lock,
            &mut resolved,
            &mut in_flight,
        )?;
    }

    // Topological sort
    let sorted = topological_sort(&resolved)?;

    // Build lock file
    let lock = build_lock_file(&sorted, project_dir, manifest)?;

    Ok(ResolveResult { deps: sorted, lock })
}

/// Resolve a single dependency recursively.
///
/// `in_flight` tracks packages currently being resolved up the call stack
/// to detect cycles before they cause infinite recursion.
fn resolve_dep(
    name: &str,
    dep: &Dependency,
    project_dir: &Path,
    deps_dir: &Path,
    existing_lock: Option<&LockFile>,
    resolved: &mut BTreeMap<String, ResolvedDep>,
    in_flight: &mut HashSet<String>,
) -> Result<(), String> {
    // Skip if already resolved
    if resolved.contains_key(name) {
        return Ok(());
    }

    // Cycle detection: if this package is already being resolved up the
    // call stack, we have a circular dependency.
    if !in_flight.insert(name.to_string()) {
        let cycle: Vec<_> = in_flight.iter().cloned().collect();
        return Err(format!(
            "Circular dependency detected:\n  {} -> {}",
            cycle.join(" -> "),
            name
        ));
    }

    let (source_dir, version, is_path, checksum) = match dep {
        Dependency::Version(_ver) => {
            in_flight.remove(name);
            return Err(registry_deferred_error(name));
        }
        Dependency::Detailed(detail) => {
            if let Some(path) = &detail.path {
                resolve_path_dep(name, path, project_dir)?
            } else if let Some(git_url) = &detail.git {
                resolve_git_dep(name, git_url, detail, deps_dir, existing_lock)?
            } else if detail.version.is_some() {
                in_flight.remove(name);
                return Err(registry_deferred_error(name));
            } else {
                in_flight.remove(name);
                return Err(format!(
                    "dependency `{}` has no source (specify `path = \"...\"` or `git = \"...\"`)",
                    name
                ));
            }
        }
    };

    // Read the dependency's manifest to find transitive dependencies
    let dep_manifest = if source_dir.join("Ruxen.toml").exists() {
        Some(Manifest::load(&source_dir)?)
    } else {
        None
    };

    let mut dep_names = Vec::new();

    // Resolve transitive dependencies
    if let Some(ref dm) = dep_manifest {
        for (trans_name, trans_dep) in &dm.dependencies {
            dep_names.push(trans_name.clone());
            resolve_dep(
                trans_name,
                trans_dep,
                &source_dir,
                deps_dir,
                existing_lock,
                resolved,
                in_flight,
            )?;
        }
    }

    in_flight.remove(name);

    resolved.insert(
        name.to_string(),
        ResolvedDep {
            name: name.to_string(),
            version,
            source_dir,
            is_path,
            checksum,
            dependencies: dep_names,
        },
    );

    Ok(())
}

/// Error emitted when a registry-style dependency is encountered.
///
/// Per CEO ruling TEC-13, the v1 registry model is git-URL only (Go-style):
/// there is no centralized registry. Users must declare deps as
/// `foo = { git = "https://..." }`. We accept the parsed registry form so
/// downstream tooling that only reads the manifest still works, but reject
/// it at resolve time with a clear migration path.
fn registry_deferred_error(name: &str) -> String {
    format!(
        "registry dependencies are not supported in v1 (piece `{}`).\n\
         Ruxen v1 uses Go-style git-URL dependencies — there is no centralized registry.\n\
         Replace the version string with a git URL, for example:\n\
         \n    [dependencies]\n    {} = {{ git = \"https://github.com/user/{}.git\" }}\n\n\
         You can also pin to a tag, branch, or revision via `tag = \"...\"`, \
         `branch = \"...\"`, or `rev = \"...\"`. See CEO ruling TEC-13.",
        name, name, name
    )
}

/// Resolve a path dependency.
fn resolve_path_dep(
    name: &str,
    path: &str,
    project_dir: &Path,
) -> Result<(PathBuf, String, bool, Option<String>), String> {
    // Validate the path dep against the security boundary unless the
    // caller has explicitly opted into external paths.
    let allow_external = std::env::var("RUXEN_ALLOW_EXTERNAL_PATH")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if !allow_external {
        validate_path_dep(name, path, project_dir)?;
    }

    let dep_dir = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        project_dir.join(path)
    };

    let dep_dir = dep_dir
        .canonicalize()
        .map_err(|e| format!("path dependency `{}` not found at `{}`: {}", name, path, e))?;

    if !dep_dir.exists() {
        return Err(format!(
            "path dependency `{}` not found at `{}`",
            name,
            dep_dir.display()
        ));
    }

    // Read version from the dependency's manifest
    let version = if dep_dir.join("Ruxen.toml").exists() {
        let m = Manifest::load(&dep_dir)?;
        m.package.version.clone()
    } else {
        "0.0.0".to_string()
    };

    Ok((dep_dir, version, true, None))
}

/// Resolve a git dependency by cloning/fetching.
fn resolve_git_dep(
    name: &str,
    git_url: &str,
    detail: &DependencyDetail,
    deps_dir: &Path,
    existing_lock: Option<&LockFile>,
) -> Result<(PathBuf, String, bool, Option<String>), String> {
    // Determine the ref to check out
    let git_ref = if let Some(rev) = &detail.rev {
        rev.clone()
    } else if let Some(tag) = &detail.tag {
        tag.clone()
    } else if let Some(branch) = &detail.branch {
        branch.clone()
    } else {
        "HEAD".to_string()
    };

    // Check if we have a locked revision
    let locked_rev = existing_lock.and_then(|l| {
        l.find(name)
            .and_then(|p| p.git_rev().map(|s| s.to_string()))
    });

    // Use locked revision if available and no specific ref override
    let effective_ref = if detail.rev.is_none() && detail.tag.is_none() {
        locked_rev.as_deref().unwrap_or(&git_ref)
    } else {
        &git_ref
    };

    // Compute cache key
    let cache_key = rlib::hash_bytes(format!("{}:{}", git_url, effective_ref).as_bytes());
    let short_hash = &cache_key[7..15]; // skip "sha256:" prefix, take 8 chars
    let clone_dir = deps_dir.join(format!("{}-{}", name, short_hash));

    // Security boundary: every field of `DependencyDetail` is
    // attacker-controlled. Validate the URL + ref + insert `--` before
    // every user-controlled argv slot so a `git_url` like
    // `--upload-pack=…` can't reach git as a flag.
    validate_git_url(git_url)?;
    validate_git_ref(effective_ref)?;

    if !clone_dir.exists() {
        // Clone the repository
        println!("    Fetching piece `{}` from {}", name, git_url);
        let status = Command::new("git")
            .args([
                "clone",
                "--quiet",
                "--",
                git_url,
                &clone_dir.to_string_lossy(),
            ])
            .status()
            .map_err(|e| format!("failed to run git clone: {}", e))?;

        if !status.success() {
            return Err(format!("failed to clone `{}` from {}", name, git_url));
        }
    }

    // Checkout the specific ref
    if effective_ref != "HEAD" {
        // For `git checkout`, `--` *follows* the ref ("no paths after");
        // putting `--` before the ref would tell git the argument is a
        // path, not a ref. The strict `validate_git_ref` allowlist above
        // is the primary defence; the trailing `--` is belt-and-braces.
        let status = Command::new("git")
            .args(["checkout", "--quiet", effective_ref, "--"])
            .current_dir(&clone_dir)
            .status()
            .map_err(|e| format!("failed to checkout ref: {}", e))?;

        if !status.success() {
            return Err(format!(
                "failed to checkout `{}` at ref `{}`",
                name, effective_ref
            ));
        }
    }

    // Get the resolved revision
    let rev_output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&clone_dir)
        .output()
        .map_err(|e| format!("failed to get git revision: {}", e))?;

    let _resolved_rev = String::from_utf8_lossy(&rev_output.stdout)
        .trim()
        .to_string();

    // Read version from the dependency's manifest
    let version = if clone_dir.join("Ruxen.toml").exists() {
        let m = Manifest::load(&clone_dir)?;
        m.package.version.clone()
    } else {
        "0.0.0".to_string()
    };

    // Compute source checksum
    let checksum = if clone_dir.join("src").exists() {
        Some(rlib::hash_sources(&clone_dir)?)
    } else {
        None
    };

    Ok((clone_dir, version, false, checksum))
}

/// Topologically sort dependencies (leaves first).
fn topological_sort(deps: &BTreeMap<String, ResolvedDep>) -> Result<Vec<ResolvedDep>, String> {
    let mut visited = HashSet::new();
    let mut result = Vec::new();

    for name in deps.keys() {
        topo_visit(name, deps, &mut visited, &mut result)?;
    }

    Ok(result)
}

fn topo_visit(
    name: &str,
    deps: &BTreeMap<String, ResolvedDep>,
    visited: &mut HashSet<String>,
    result: &mut Vec<ResolvedDep>,
) -> Result<(), String> {
    if visited.contains(name) {
        return Ok(());
    }
    visited.insert(name.to_string());

    if let Some(dep) = deps.get(name) {
        for child in &dep.dependencies {
            topo_visit(child, deps, visited, result)?;
        }
        result.push(dep.clone());
    }

    Ok(())
}

/// Build a lock file from resolved dependencies.
fn build_lock_file(
    deps: &[ResolvedDep],
    project_dir: &Path,
    _manifest: &Manifest,
) -> Result<LockFile, String> {
    let mut lock = LockFile::new();

    for dep in deps {
        let source = if dep.is_path {
            // Store relative path from project dir
            let rel = dep
                .source_dir
                .strip_prefix(project_dir)
                .unwrap_or(&dep.source_dir);
            format!("path+{}", rel.display())
        } else {
            // For git deps, we already have the source URL in the clone dir name
            // Read git remote URL from the cloned repo
            let url = get_git_remote_url(&dep.source_dir).unwrap_or_default();
            let rev = get_git_head_rev(&dep.source_dir).unwrap_or_default();
            format!("git+{}?rev={}", url, rev)
        };

        lock.pieces.push(LockedPiece {
            name: dep.name.clone(),
            version: dep.version.clone(),
            source,
            checksum: dep.checksum.clone(),
            dependencies: dep.dependencies.clone(),
        });
    }

    Ok(lock)
}

fn get_git_remote_url(dir: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(dir)
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

fn get_git_head_rev(dir: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cycle_detection_real_projects() {
        // Create two projects that depend on each other
        let tmp = test_tmp_dir("cycle");
        std::fs::create_dir_all(tmp.join("cycle-a/src")).unwrap();
        std::fs::create_dir_all(tmp.join("cycle-b/src")).unwrap();

        std::fs::write(tmp.join("cycle-a/src/lib.rx"), "pub def a\nend\n").unwrap();
        std::fs::write(tmp.join("cycle-b/src/lib.rx"), "pub def b\nend\n").unwrap();

        // A depends on B (use relative path: the security validator
        // refuses absolute paths in manifests by default).
        std::fs::write(
            tmp.join("cycle-a/Ruxen.toml"),
            "[package]\nname = \"cycle-a\"\nversion = \"0.1.0\"\n\n[dependencies]\ncycle-b = { path = \"../cycle-b\" }\n",
        ).unwrap();

        // B depends on A → cycle!
        std::fs::write(
            tmp.join("cycle-b/Ruxen.toml"),
            "[package]\nname = \"cycle-b\"\nversion = \"0.1.0\"\n\n[dependencies]\ncycle-a = { path = \"../cycle-a\" }\n",
        ).unwrap();

        let manifest = crate::manifest::Manifest::load(&tmp.join("cycle-a")).unwrap();
        let result = resolve(&tmp.join("cycle-a"), &manifest, None);
        assert!(result.is_err(), "expected cycle detection error");
        let err = result.unwrap_err();
        assert!(
            err.contains("Circular dependency detected"),
            "expected 'Circular dependency detected', got: {}",
            err,
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_topological_sort() {
        let mut deps = BTreeMap::new();
        deps.insert(
            "app".to_string(),
            ResolvedDep {
                name: "app".to_string(),
                version: "0.1.0".to_string(),
                source_dir: PathBuf::from("/tmp/app"),
                is_path: true,
                checksum: None,
                dependencies: vec!["http".to_string(), "json".to_string()],
            },
        );
        deps.insert(
            "http".to_string(),
            ResolvedDep {
                name: "http".to_string(),
                version: "1.0.0".to_string(),
                source_dir: PathBuf::from("/tmp/http"),
                is_path: true,
                checksum: None,
                dependencies: vec!["tls".to_string()],
            },
        );
        deps.insert(
            "json".to_string(),
            ResolvedDep {
                name: "json".to_string(),
                version: "2.0.0".to_string(),
                source_dir: PathBuf::from("/tmp/json"),
                is_path: true,
                checksum: None,
                dependencies: vec![],
            },
        );
        deps.insert(
            "tls".to_string(),
            ResolvedDep {
                name: "tls".to_string(),
                version: "1.0.0".to_string(),
                source_dir: PathBuf::from("/tmp/tls"),
                is_path: true,
                checksum: None,
                dependencies: vec![],
            },
        );

        let sorted = topological_sort(&deps).unwrap();
        let names: Vec<&str> = sorted.iter().map(|d| d.name.as_str()).collect();

        // Leaves (tls, json) must come before their dependents (http, app)
        let tls_idx = names.iter().position(|&n| n == "tls").unwrap();
        let http_idx = names.iter().position(|&n| n == "http").unwrap();
        let json_idx = names.iter().position(|&n| n == "json").unwrap();
        let app_idx = names.iter().position(|&n| n == "app").unwrap();

        assert!(tls_idx < http_idx);
        assert!(http_idx < app_idx);
        assert!(json_idx < app_idx);
    }

    /// Build a temp dir keyed on the test name (the caller's
    /// `function_path!()`). Thread IDs alone are not unique across cargo's
    /// test worker pool — a second test landing on the same worker would
    /// share the path and `remove_dir_all` from one would race the other's
    /// `create_dir_all`. Embedding the test name keeps paths disjoint and
    /// reproducible.
    fn test_tmp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ruxen_test_{}_{}_{:?}",
            name,
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn test_registry_version_dep_parses_but_rejects_at_resolve() {
        // Bare version string: foo = "0.1.0"
        let tmp = test_tmp_dir("registry_v");
        std::fs::create_dir_all(tmp.join("src")).unwrap();
        std::fs::write(tmp.join("src/main.rx"), "def main\nend\n").unwrap();
        std::fs::write(
            tmp.join("Ruxen.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nfoo = \"0.1.0\"\n",
        )
        .unwrap();

        // Parse must succeed — downstream tooling reads manifests of all shapes.
        let manifest = crate::manifest::Manifest::load(&tmp).unwrap();
        assert!(manifest.dependencies.contains_key("foo"));

        // Resolve must fail with the deferred-feature diagnostic.
        let result = resolve(&tmp, &manifest, None);
        assert!(result.is_err(), "expected resolve to fail for registry dep");
        let err = result.unwrap_err();
        assert!(
            err.contains("TEC-13"),
            "diagnostic should cite TEC-13, got: {}",
            err
        );
        assert!(
            err.contains("git ="),
            "diagnostic should mention `git = \"...\"` migration, got: {}",
            err
        );
        assert!(
            err.contains("foo"),
            "diagnostic should name the offending piece, got: {}",
            err
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_registry_detailed_version_dep_rejected_at_resolve() {
        // Detailed table form with only `version`: foo = { version = "0.1.0" }
        let tmp = test_tmp_dir("registry_d");
        std::fs::create_dir_all(tmp.join("src")).unwrap();
        std::fs::write(tmp.join("src/main.rx"), "def main\nend\n").unwrap();
        std::fs::write(
            tmp.join("Ruxen.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nbar = { version = \"0.1.0\" }\n",
        )
        .unwrap();

        let manifest = crate::manifest::Manifest::load(&tmp).unwrap();
        let result = resolve(&tmp, &manifest, None);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("TEC-13"));
        assert!(err.contains("git ="));
        assert!(err.contains("bar"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_path_dep_still_resolves() {
        // Regression check: the fix for registry deps must not break path deps.
        let tmp = test_tmp_dir("path_ok");
        std::fs::create_dir_all(tmp.join("app/src")).unwrap();
        std::fs::create_dir_all(tmp.join("lib/src")).unwrap();
        std::fs::write(tmp.join("app/src/main.rx"), "def main\nend\n").unwrap();
        std::fs::write(tmp.join("lib/src/lib.rx"), "pub def hi\nend\n").unwrap();

        std::fs::write(
            tmp.join("lib/Ruxen.toml"),
            "[package]\nname = \"lib\"\nversion = \"0.2.0\"\n",
        )
        .unwrap();
        // Relative path: the security validator refuses absolute paths
        // in manifests by default. From `app`'s project_dir, the sibling
        // `lib` directory is `../lib`.
        std::fs::write(
            tmp.join("app/Ruxen.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nlib = { path = \"../lib\" }\n",
        )
        .unwrap();

        let manifest = crate::manifest::Manifest::load(&tmp.join("app")).unwrap();
        let result = resolve(&tmp.join("app"), &manifest, None).expect("path dep should resolve");
        assert_eq!(result.deps.len(), 1);
        assert_eq!(result.deps[0].name, "lib");
        assert_eq!(result.deps[0].version, "0.2.0");
        assert!(result.deps[0].is_path);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_topological_sort_single() {
        let mut deps = BTreeMap::new();
        deps.insert(
            "only".to_string(),
            ResolvedDep {
                name: "only".to_string(),
                version: "1.0.0".to_string(),
                source_dir: PathBuf::from("/tmp/only"),
                is_path: true,
                checksum: None,
                dependencies: vec![],
            },
        );

        let sorted = topological_sort(&deps).unwrap();
        assert_eq!(sorted.len(), 1);
        assert_eq!(sorted[0].name, "only");
    }
}
