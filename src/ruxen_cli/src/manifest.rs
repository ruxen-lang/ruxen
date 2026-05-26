//! Ruxen.toml manifest parsing and serialization.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// The full Ruxen.toml manifest.
#[derive(Debug, Deserialize, Serialize)]
pub struct Manifest {
    /// `[package]` table. Absent on a virtual workspace root (a
    /// Ruxen.toml that has only `[workspace]`). Every place that
    /// consumes the package should call [`Manifest::require_package`]
    /// so the "this manifest must be a package" precondition is
    /// surfaced uniformly.
    #[serde(default)]
    pub package: Option<Package>,
    /// `[workspace]` table. Present on a workspace root. Mirrors
    /// Cargo's behaviour: a root may carry both `[package]` and
    /// `[workspace]` ("non-virtual" root) or just `[workspace]`
    /// ("virtual" root).
    #[serde(default)]
    pub workspace: Option<Workspace>,
    #[serde(default)]
    pub dependencies: BTreeMap<String, Dependency>,
    #[serde(default, rename = "dev-dependencies")]
    pub dev_dependencies: BTreeMap<String, Dependency>,
    #[serde(default)]
    pub build: Option<BuildConfig>,
    #[serde(default)]
    pub bin: Vec<BinTarget>,
    #[serde(default)]
    pub profile: Option<Profiles>,
}

/// `[workspace]` section.
///
/// `members` is the authoritative list of workspace member packages.
/// Each entry is either a literal relative directory (`"pkg-a"`) or a
/// glob with a trailing `*` (`"examples/*"`). Anything richer than a
/// trailing `*` falls outside v1 — we hand-roll the expansion in
/// [`expand_workspace_members`] so we avoid the `glob` crate.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct Workspace {
    #[serde(default)]
    pub members: Vec<String>,
    /// Optional subset of `members` used when no specific package
    /// is passed to the build/run driver. Not yet consumed by the
    /// build pipeline; reserved so the field survives a parse →
    /// serialize round-trip.
    #[serde(default, rename = "default-members", skip_serializing_if = "Option::is_none")]
    pub default_members: Option<Vec<String>>,
}

/// [package] section.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Package {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub edition: Option<String>,
    /// Minimum compiler version requirement.
    #[serde(default)]
    pub ruxen: Option<String>,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub readme: Option<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
}

/// A dependency can be a simple version string or a detailed table.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Dependency {
    /// Registry version string: "1.2.0", "^2.0", "~1.2.3"
    Version(String),
    /// Table with source details
    Detailed(DependencyDetail),
}

/// Detailed dependency specification.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct DependencyDetail {
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub git: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub rev: Option<String>,
}

/// [build] section.
#[derive(Debug, Deserialize, Serialize)]
pub struct BuildConfig {
    /// "binary" or "library"
    #[serde(default, rename = "type")]
    pub build_type: Option<String>,
    /// Entry point file
    #[serde(default)]
    pub entry: Option<String>,
    /// C libraries to link (-l flags)
    #[serde(default)]
    pub link: Vec<String>,
    /// Library search paths (-L flags)
    #[serde(default, rename = "link-search")]
    pub link_search: Vec<String>,
}

/// [[bin]] target.
#[derive(Debug, Deserialize, Serialize)]
pub struct BinTarget {
    pub name: String,
    pub entry: String,
}

/// [profile.*] sections.
#[derive(Debug, Deserialize, Serialize)]
pub struct Profiles {
    #[serde(default)]
    pub debug: Option<ProfileConfig>,
    #[serde(default)]
    pub release: Option<ProfileConfig>,
}

/// Configuration for a single build profile.
#[derive(Debug, Deserialize, Serialize)]
pub struct ProfileConfig {
    #[serde(default, rename = "opt-level")]
    pub opt_level: Option<u8>,
    #[serde(default)]
    pub debug: Option<bool>,
    #[serde(default)]
    pub lto: Option<bool>,
}

impl Manifest {
    /// Read and parse a Ruxen.toml from the given directory.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let manifest_path = dir.join("Ruxen.toml");
        if !manifest_path.exists() {
            return Err(format!(
                "could not find `Ruxen.toml` in `{}`",
                dir.display()
            ));
        }
        let content = std::fs::read_to_string(&manifest_path)
            .map_err(|e| format!("failed to read Ruxen.toml: {}", e))?;
        Self::from_str(&content)
    }

    /// Parse a Ruxen.toml from a string.
    pub fn from_str(s: &str) -> Result<Self, String> {
        let manifest: Self =
            toml::from_str(s).map_err(|e| format!("failed to parse Ruxen.toml: {}", e))?;
        // Edition is the only field validated at parse time so unknown
        // editions are rejected uniformly across every load path (build,
        // dependency resolution, lockfile, etc.). Other fields are still
        // validated lazily by `Manifest::validate`.
        //
        // A virtual workspace root has no `[package]`; skip the edition
        // check in that case — the field can't be set anyway.
        if let Some(pkg) = manifest.package.as_ref() {
            parse_edition(pkg.edition.as_deref())?;
        }
        if manifest.package.is_none() && manifest.workspace.is_none() {
            return Err(
                "Ruxen.toml must contain a `[package]` or `[workspace]` section".to_string(),
            );
        }
        Ok(manifest)
    }

    /// Serialize the manifest back to TOML.
    pub fn to_toml_string(&self) -> Result<String, String> {
        toml::to_string_pretty(self).map_err(|e| format!("failed to serialize Ruxen.toml: {}", e))
    }

    /// Write the manifest to Ruxen.toml in the given directory.
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        let content = self.to_toml_string()?;
        let manifest_path = dir.join("Ruxen.toml");
        std::fs::write(&manifest_path, content)
            .map_err(|e| format!("failed to write Ruxen.toml: {}", e))
    }

    /// Borrow the `[package]` section. Errors when the manifest is a
    /// virtual workspace root, with a message that points the caller
    /// to the operation that doesn't apply at the workspace level.
    pub fn require_package(&self) -> Result<&Package, String> {
        self.package.as_ref().ok_or_else(|| {
            "this Ruxen.toml is a virtual workspace root (no [package]); \
             run this command from inside a member package"
                .to_string()
        })
    }

    /// True when this manifest declares a `[workspace]` section.
    pub fn is_workspace_root(&self) -> bool {
        self.workspace.is_some()
    }

    /// Determine the build type ("binary" or "library").
    pub fn build_type(&self) -> &str {
        self.build
            .as_ref()
            .and_then(|b| b.build_type.as_deref())
            .unwrap_or("binary")
    }

    /// Determine the entry point file.
    pub fn entry_point(&self) -> &str {
        self.build
            .as_ref()
            .and_then(|b| b.entry.as_deref())
            .unwrap_or_else(|| {
                if self.build_type() == "library" {
                    "src/lib.rx"
                } else {
                    "src/main.rx"
                }
            })
    }

    /// Validate the manifest for common errors. On a virtual workspace
    /// root (no `[package]`) this only validates the workspace section.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(pkg) = self.package.as_ref() {
            validate_package_name(&pkg.name)?;

            // Validate version
            crate::version::SemVer::parse(&pkg.version)
                .map_err(|_| format!("invalid package version: '{}'", pkg.version))?;

            // Validate edition
            parse_edition(pkg.edition.as_deref())?;

            // Validate keywords count
            if pkg.keywords.len() > 5 {
                return Err("too many keywords (max 5)".to_string());
            }
        }

        Ok(())
    }

    /// Return the resolved edition for this package, defaulting to "2026"
    /// when the field is omitted. Returns an error for unknown editions
    /// or when the manifest is a virtual workspace root.
    pub fn edition(&self) -> Result<&'static str, String> {
        let pkg = self.require_package()?;
        parse_edition(pkg.edition.as_deref())
    }
}

/// Walk upward from `start` looking for a Ruxen.toml that declares
/// `[workspace]`. Returns the directory containing that file, or
/// `None` if no ancestor is a workspace root.
///
/// Used so any `ruxen build` / `ruxen run` invoked from a member
/// subdir can locate the shared `target/` at the workspace root.
pub fn find_workspace_root(start: &Path) -> Option<std::path::PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        let manifest_path = dir.join("Ruxen.toml");
        if manifest_path.exists() {
            if let Ok(text) = std::fs::read_to_string(&manifest_path) {
                if let Ok(m) = Manifest::from_str(&text) {
                    if m.is_workspace_root() {
                        return Some(dir);
                    }
                }
            }
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Expand the `members` list of a workspace into resolved sibling
/// directories under `workspace_root`. Each entry is either a literal
/// path (`"pkg-a"`) or a glob with a trailing `*` (`"examples/*"`).
///
/// Returns one (member-directory, member-name-from-package-name) pair
/// per resolved entry. The member-name comes from the member's own
/// `[package].name`, NOT the directory name — that's the identity
/// other manifests refer to when they declare `pkg-b = "0.1.0"`.
///
/// Emits an error tagged `E1600` when an entry resolves to a directory
/// that does not exist or does not contain a Ruxen.toml. The literal
/// `"E1600"` token in the message is load-bearing: the docs-pin test
/// links the code to its docs page through string occurrence.
pub fn expand_workspace_members(
    workspace_root: &Path,
    members: &[String],
) -> Result<Vec<(std::path::PathBuf, String)>, String> {
    let mut out: Vec<(std::path::PathBuf, String)> = Vec::new();
    let mut seen: std::collections::BTreeSet<std::path::PathBuf> = Default::default();

    for entry in members {
        // Trailing-`*` glob: `"examples/*"`. Anything richer (multiple
        // `*`, embedded `*`, `**`, `?`) is out of scope for v1 — we
        // want zero new deps for this path.
        if let Some(prefix) = entry.strip_suffix("/*") {
            let parent = workspace_root.join(prefix);
            if !parent.exists() {
                return Err(format!(
                    "error[E1600]: workspace glob `{}` resolves to `{}`, which does not exist",
                    entry,
                    parent.display()
                ));
            }
            let read = std::fs::read_dir(&parent).map_err(|e| {
                format!(
                    "error[E1600]: failed to read workspace glob dir `{}`: {}",
                    parent.display(),
                    e
                )
            })?;
            // BTreeSet for deterministic order; the OS `read_dir`
            // iteration order is not guaranteed.
            let mut dirs: std::collections::BTreeSet<std::path::PathBuf> = Default::default();
            for child in read.flatten() {
                let p = child.path();
                if p.is_dir() && p.join("Ruxen.toml").exists() {
                    dirs.insert(p);
                }
            }
            for d in dirs {
                if seen.insert(d.clone()) {
                    let name = read_member_package_name(&d)?;
                    out.push((d, name));
                }
            }
        } else if entry.contains('*') {
            return Err(format!(
                "error[E1600]: workspace member glob `{}` is not supported (only trailing `/*` is allowed in v1)",
                entry
            ));
        } else {
            let member_dir = workspace_root.join(entry);
            if !member_dir.exists() || !member_dir.join("Ruxen.toml").exists() {
                return Err(format!(
                    "error[E1600]: workspace member `{}` declared in [workspace] members, but `{}/Ruxen.toml` does not exist",
                    entry,
                    member_dir.display()
                ));
            }
            if seen.insert(member_dir.clone()) {
                let name = read_member_package_name(&member_dir)?;
                out.push((member_dir, name));
            }
        }
    }

    Ok(out)
}

/// Read just the `[package].name` field from a member's Ruxen.toml.
/// A member without `[package]` is rejected — virtual nested workspaces
/// are not supported in v1, and silently skipping the entry would let
/// `pkg-b = "0.1.0"` intra-workspace resolution miss it without a
/// helpful diagnostic.
fn read_member_package_name(member_dir: &Path) -> Result<String, String> {
    let m = Manifest::load(member_dir)?;
    let pkg = m.package.ok_or_else(|| {
        format!(
            "error[E1600]: workspace member at `{}` has no [package] section (nested virtual workspaces are not supported in v1)",
            member_dir.display()
        )
    })?;
    Ok(pkg.name)
}

/// The default Ruxen edition when the manifest omits `edition`.
pub const DEFAULT_EDITION: &str = "2026";

/// Validate and normalize a manifest edition value. Accepts `None` (defaults
/// to `DEFAULT_EDITION`) or `Some("2026")`. Rejects every other value.
pub fn parse_edition(value: Option<&str>) -> Result<&'static str, String> {
    match value {
        None => Ok(DEFAULT_EDITION),
        Some("2026") => Ok("2026"),
        Some(other) => Err(format!(
            "unknown edition \"{}\": valid values are \"2026\"",
            other
        )),
    }
}

/// Validate a package name: [a-z][a-z0-9_-]*, max 64 chars.
pub fn validate_package_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("package name cannot be empty".to_string());
    }
    if name.len() > 64 {
        return Err(format!(
            "package name '{}' is too long (max 64 characters)",
            name
        ));
    }
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => {
            return Err(format!(
                "package name '{}' must start with a lowercase letter",
                name
            ));
        }
    }
    for c in chars {
        if !c.is_ascii_lowercase() && !c.is_ascii_digit() && c != '_' && c != '-' {
            return Err(format!(
                "package name '{}' contains invalid character '{}' (allowed: a-z, 0-9, _, -)",
                name, c
            ));
        }
    }
    Ok(())
}

impl Dependency {
    /// Get the version string, if any.
    pub fn version_str(&self) -> Option<&str> {
        match self {
            Dependency::Version(v) => Some(v),
            Dependency::Detailed(d) => d.version.as_deref(),
        }
    }

    /// Check if this is a git dependency.
    pub fn is_git(&self) -> bool {
        matches!(self, Dependency::Detailed(d) if d.git.is_some())
    }

    /// Check if this is a path dependency.
    pub fn is_path(&self) -> bool {
        matches!(self, Dependency::Detailed(d) if d.path.is_some())
    }

    /// Get the git URL if this is a git dependency.
    pub fn git_url(&self) -> Option<&str> {
        match self {
            Dependency::Detailed(d) => d.git.as_deref(),
            _ => None,
        }
    }

    /// Get the path if this is a path dependency.
    pub fn dep_path(&self) -> Option<&str> {
        match self {
            Dependency::Detailed(d) => d.path.as_deref(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_minimal_manifest() {
        let toml = r#"
[package]
name = "my-project"
version = "0.1.0"
"#;
        let manifest = Manifest::from_str(toml).unwrap();
        assert_eq!(manifest.package.as_ref().unwrap().name, "my-project");
        assert_eq!(manifest.package.as_ref().unwrap().version, "0.1.0");
        assert!(manifest.dependencies.is_empty());
        assert!(manifest.dev_dependencies.is_empty());
    }

    #[test]
    fn test_full_manifest() {
        let toml = r#"
[package]
name = "my-project"
version = "0.1.0"
edition = "2026"
ruxen = ">=0.2.0"
authors = ["Alice <alice@example.com>"]
license = "MIT"
description = "A short summary"
repository = "https://github.com/user/project"
homepage = "https://example.com"
readme = "README.md"
keywords = ["web", "http"]

[dependencies]
http = "1.2.0"
json = "^2.0"
utils = { git = "https://github.com/user/utils.git", tag = "v1.0.0" }
local_lib = { path = "../local_lib" }
crypto = { git = "https://github.com/user/crypto.git", branch = "main" }

[dev-dependencies]
test_helpers = "0.1.0"

[build]
type = "binary"
entry = "src/main.rx"
link = ["ssl", "crypto"]
link-search = ["/usr/local/lib"]

[[bin]]
name = "my-cli"
entry = "src/bin/cli.rx"

[[bin]]
name = "my-server"
entry = "src/bin/server.rx"

[profile.debug]
opt-level = 0
debug = true

[profile.release]
opt-level = 3
debug = false
lto = true
"#;
        let manifest = Manifest::from_str(toml).unwrap();
        assert_eq!(manifest.package.as_ref().unwrap().name, "my-project");
        assert_eq!(manifest.package.as_ref().unwrap().edition.as_deref(), Some("2026"));
        assert_eq!(manifest.package.as_ref().unwrap().authors.len(), 1);
        assert_eq!(manifest.package.as_ref().unwrap().keywords, vec!["web", "http"]);
        assert_eq!(manifest.dependencies.len(), 5);
        assert_eq!(manifest.dev_dependencies.len(), 1);
        assert_eq!(manifest.bin.len(), 2);

        // Check dependency types
        assert!(matches!(
            manifest.dependencies.get("http"),
            Some(Dependency::Version(v)) if v == "1.2.0"
        ));
        assert!(manifest.dependencies.get("utils").unwrap().is_git());
        assert!(manifest.dependencies.get("local_lib").unwrap().is_path());

        // Check build config
        let build = manifest.build.as_ref().unwrap();
        assert_eq!(build.build_type.as_deref(), Some("binary"));
        assert_eq!(build.link, vec!["ssl", "crypto"]);

        // Check profiles
        let profiles = manifest.profile.as_ref().unwrap();
        assert_eq!(profiles.release.as_ref().unwrap().opt_level, Some(3));
        assert_eq!(profiles.release.as_ref().unwrap().lto, Some(true));
    }

    #[test]
    fn test_unknown_edition_is_rejected() {
        let toml = r#"
[package]
name = "my-project"
version = "0.1.0"
edition = "2099"
"#;
        let err = Manifest::from_str(toml).unwrap_err();
        assert!(
            err.contains(r#"unknown edition "2099": valid values are "2026""#),
            "expected edition error, got: {}",
            err
        );
    }

    #[test]
    fn test_default_edition_is_2026() {
        let toml = r#"
[package]
name = "my-project"
version = "0.1.0"
"#;
        let manifest = Manifest::from_str(toml).unwrap();
        assert_eq!(manifest.package.as_ref().unwrap().edition, None);
        assert_eq!(manifest.edition().unwrap(), "2026");
    }

    #[test]
    fn test_validate_package_name() {
        assert!(validate_package_name("my-project").is_ok());
        assert!(validate_package_name("http").is_ok());
        assert!(validate_package_name("a").is_ok());
        assert!(validate_package_name("my_lib_2").is_ok());

        assert!(validate_package_name("").is_err());
        assert!(validate_package_name("My-Project").is_err());
        assert!(validate_package_name("1foo").is_err());
        assert!(validate_package_name("foo bar").is_err());
        assert!(validate_package_name("foo.bar").is_err());

        let long_name = "a".repeat(65);
        assert!(validate_package_name(&long_name).is_err());
    }

    #[test]
    fn test_manifest_roundtrip() {
        let toml = r#"
[package]
name = "test-pkg"
version = "1.0.0"

[dependencies]
http = "1.0.0"
"#;
        let manifest = Manifest::from_str(toml).unwrap();
        let serialized = manifest.to_toml_string().unwrap();
        let reparsed = Manifest::from_str(&serialized).unwrap();
        assert_eq!(reparsed.package.as_ref().unwrap().name, "test-pkg");
        assert!(reparsed.dependencies.contains_key("http"));
    }

    #[test]
    fn test_entry_point_defaults() {
        let binary_toml = r#"
[package]
name = "bin-project"
version = "0.1.0"
"#;
        let m = Manifest::from_str(binary_toml).unwrap();
        assert_eq!(m.build_type(), "binary");
        assert_eq!(m.entry_point(), "src/main.rx");

        let lib_toml = r#"
[package]
name = "lib-project"
version = "0.1.0"

[build]
type = "library"
"#;
        let m = Manifest::from_str(lib_toml).unwrap();
        assert_eq!(m.build_type(), "library");
        assert_eq!(m.entry_point(), "src/lib.rx");
    }

    #[test]
    fn test_dependency_detail_git() {
        let toml = r#"
[package]
name = "test"
version = "0.1.0"

[dependencies]
http = { git = "https://github.com/user/http.git", tag = "v1.0.0" }
"#;
        let manifest = Manifest::from_str(toml).unwrap();
        let dep = manifest.dependencies.get("http").unwrap();
        assert!(dep.is_git());
        assert_eq!(dep.git_url(), Some("https://github.com/user/http.git"));
        match dep {
            Dependency::Detailed(d) => {
                assert_eq!(d.tag.as_deref(), Some("v1.0.0"));
            }
            _ => panic!("expected detailed dependency"),
        }
    }

    #[test]
    fn test_dependency_detail_path() {
        let toml = r#"
[package]
name = "test"
version = "0.1.0"

[dependencies]
utils = { path = "../utils" }
"#;
        let manifest = Manifest::from_str(toml).unwrap();
        let dep = manifest.dependencies.get("utils").unwrap();
        assert!(dep.is_path());
        assert_eq!(dep.dep_path(), Some("../utils"));
    }
}
