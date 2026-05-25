//! Ruxen.toml manifest parsing and serialization.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// The full Ruxen.toml manifest.
#[derive(Debug, Deserialize, Serialize)]
pub struct Manifest {
    pub package: Package,
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

/// [package] section.
#[derive(Debug, Deserialize, Serialize)]
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
        parse_edition(manifest.package.edition.as_deref())?;
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

    /// Validate the manifest for common errors.
    pub fn validate(&self) -> Result<(), String> {
        validate_package_name(&self.package.name)?;

        // Validate version
        crate::version::SemVer::parse(&self.package.version)
            .map_err(|_| format!("invalid package version: '{}'", self.package.version))?;

        // Validate edition
        parse_edition(self.package.edition.as_deref())?;

        // Validate keywords count
        if self.package.keywords.len() > 5 {
            return Err("too many keywords (max 5)".to_string());
        }

        Ok(())
    }

    /// Return the resolved edition for this package, defaulting to "2026"
    /// when the field is omitted. Returns an error for unknown editions.
    pub fn edition(&self) -> Result<&'static str, String> {
        parse_edition(self.package.edition.as_deref())
    }
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
        assert_eq!(manifest.package.name, "my-project");
        assert_eq!(manifest.package.version, "0.1.0");
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
        assert_eq!(manifest.package.name, "my-project");
        assert_eq!(manifest.package.edition.as_deref(), Some("2026"));
        assert_eq!(manifest.package.authors.len(), 1);
        assert_eq!(manifest.package.keywords, vec!["web", "http"]);
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
        assert_eq!(manifest.package.edition, None);
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
        assert_eq!(reparsed.package.name, "test-pkg");
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
