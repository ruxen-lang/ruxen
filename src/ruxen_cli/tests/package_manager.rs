//! Integration tests for the Ruxen package manager (CLI).

use std::fs;
use std::path::{Path, PathBuf};

use ruxen_cli::lock::LockFile;
use ruxen_cli::manifest::Manifest;
use ruxen_cli::module_discovery::ModuleTree;
use ruxen_cli::scaffold;
use ruxen_cli::version::{SemVer, VersionReq};

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn temp_dir(test_name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ruxen_integ_{}_{:?}",
        test_name,
        std::thread::current().id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

// ─── Manifest Parsing ──────────────────────────────────────────────

#[test]
fn test_parse_simple_binary_manifest() {
    let manifest = Manifest::load(&fixture_path("simple-binary")).unwrap();
    assert_eq!(manifest.package.as_ref().unwrap().name, "simple-binary");
    assert_eq!(manifest.package.as_ref().unwrap().version, "0.1.0");
    assert_eq!(manifest.build_type(), "binary");
    assert_eq!(manifest.entry_point(), "src/main.rx");
}

#[test]
fn test_parse_simple_lib_manifest() {
    let manifest = Manifest::load(&fixture_path("simple-lib")).unwrap();
    assert_eq!(manifest.package.as_ref().unwrap().name, "simple-lib");
    assert_eq!(manifest.build_type(), "library");
    assert_eq!(manifest.entry_point(), "src/lib.rx");
}

#[test]
fn test_parse_with_path_dep_manifest() {
    let manifest = Manifest::load(&fixture_path("with-path-dep")).unwrap();
    assert_eq!(manifest.dependencies.len(), 1);
    let dep = manifest.dependencies.get("simple-lib").unwrap();
    assert!(dep.is_path());
    assert_eq!(dep.dep_path(), Some("../simple-lib"));
}

#[test]
fn test_invalid_manifest_errors() {
    let result = Manifest::from_str("not valid toml [[[");
    assert!(result.is_err());
}

#[test]
fn test_manifest_validation() {
    let bad = Manifest::from_str("[package]\nname = \"Invalid\"\nversion = \"0.1.0\"\n").unwrap();
    assert!(bad.validate().is_err());

    let good =
        Manifest::from_str("[package]\nname = \"valid-name\"\nversion = \"0.1.0\"\n").unwrap();
    assert!(good.validate().is_ok());
}

// ─── Version Requirements ──────────────────────────────────────────

#[test]
fn test_version_req_caret() {
    let req = VersionReq::parse("1.2.3").unwrap();
    assert!(req.matches(&SemVer::new(1, 2, 3)));
    assert!(req.matches(&SemVer::new(1, 9, 0)));
    assert!(!req.matches(&SemVer::new(2, 0, 0)));
    assert!(!req.matches(&SemVer::new(0, 9, 0)));
}

#[test]
fn test_version_req_tilde() {
    let req = VersionReq::parse("~1.2.3").unwrap();
    assert!(req.matches(&SemVer::new(1, 2, 3)));
    assert!(req.matches(&SemVer::new(1, 2, 9)));
    assert!(!req.matches(&SemVer::new(1, 3, 0)));
}

#[test]
fn test_version_req_exact() {
    let req = VersionReq::parse("=2.0.0").unwrap();
    assert!(req.matches(&SemVer::new(2, 0, 0)));
    assert!(!req.matches(&SemVer::new(2, 0, 1)));
}

#[test]
fn test_version_req_range() {
    let req = VersionReq::parse(">=1.0.0, <2.0.0").unwrap();
    assert!(req.matches(&SemVer::new(1, 5, 0)));
    assert!(!req.matches(&SemVer::new(2, 0, 0)));
    assert!(!req.matches(&SemVer::new(0, 9, 0)));
}

#[test]
fn test_version_req_wildcard() {
    let req = VersionReq::parse("*").unwrap();
    assert!(req.matches(&SemVer::new(0, 0, 0)));
    assert!(req.matches(&SemVer::new(99, 0, 0)));
}

// ─── Module Discovery ──────────────────────────────────────────────

#[test]
fn test_discover_simple_binary() {
    let tree = ModuleTree::discover(&fixture_path("simple-binary")).unwrap();
    assert!(tree.root.file.is_some());
    assert!(tree.root.children.is_empty());
}

#[test]
fn test_discover_multi_file() {
    let tree = ModuleTree::discover(&fixture_path("multi-file")).unwrap();
    assert!(tree.root.file.is_some()); // main.rx

    // utils.rx → Utils
    let utils = tree.find("Utils");
    assert!(utils.is_some(), "Utils module not found");
    assert!(utils.unwrap().file.is_some());

    // http/client.rx → Http.Client
    let client = tree.find("Http.Client");
    assert!(client.is_some(), "Http.Client module not found");
}

#[test]
fn test_module_path_mapping() {
    assert_eq!(
        ModuleTree::module_path_for_file(Path::new("utils")),
        "Utils"
    );
    assert_eq!(
        ModuleTree::module_path_for_file(Path::new("http/client")),
        "Http.Client"
    );
    assert_eq!(
        ModuleTree::module_path_for_file(Path::new("http/client/pool")),
        "Http.Client.Pool"
    );
}

// ─── Project Scaffolding ───────────────────────────────────────────

#[test]
fn test_scaffold_binary_project() {
    let tmp = temp_dir("scaffold_bin");

    scaffold::new_project_in("my-app", false, true, &tmp).unwrap();

    let project = tmp.join("my-app");
    assert!(project.join("Ruxen.toml").exists());
    assert!(project.join("src/main.rx").exists());
    assert!(project.join(".gitignore").exists());

    // Manifest should be parseable
    let manifest = Manifest::load(&project).unwrap();
    assert_eq!(manifest.package.as_ref().unwrap().name, "my-app");
    assert_eq!(manifest.package.as_ref().unwrap().version, "0.1.0");
    assert!(manifest.validate().is_ok());

    // Source should contain a main function
    let src = fs::read_to_string(project.join("src/main.rx")).unwrap();
    assert!(src.contains("def main"));
    assert!(src.contains("Hello, Ruxen!"));

    // .gitignore should include /target but not Ruxen.lock
    let gi = fs::read_to_string(project.join(".gitignore")).unwrap();
    assert!(gi.contains("/target"));
    assert!(!gi.contains("Ruxen.lock"));

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn test_scaffold_library_project() {
    let tmp = temp_dir("scaffold_lib");

    scaffold::new_project_in("my-lib", true, true, &tmp).unwrap();

    let project = tmp.join("my-lib");
    assert!(project.join("src/lib.rx").exists());
    assert!(!project.join("src/main.rx").exists());

    let manifest = Manifest::load(&project).unwrap();
    assert_eq!(manifest.build_type(), "library");

    // .gitignore should include Ruxen.lock for libraries
    let gi = fs::read_to_string(project.join(".gitignore")).unwrap();
    assert!(gi.contains("Ruxen.lock"));

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn test_scaffold_rejects_invalid_name() {
    let tmp = temp_dir("scaffold_invalid");
    let result = scaffold::new_project_in("Invalid-Name", false, true, &tmp);
    assert!(result.is_err());
    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn test_scaffold_rejects_existing_dir() {
    let tmp = temp_dir("scaffold_exist");
    fs::create_dir(tmp.join("already-here")).unwrap();

    let result = scaffold::new_project_in("already-here", false, true, &tmp);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("already exists"));

    let _ = fs::remove_dir_all(&tmp);
}

// ─── Lock File ─────────────────────────────────────────────────────

#[test]
fn test_lock_file_roundtrip() {
    use ruxen_cli::lock::LockedPiece;

    let lock = LockFile {
        version: 1,
        pieces: vec![
            LockedPiece::for_path("utils", "0.1.0", "../utils"),
            LockedPiece::for_git(
                "http",
                "1.0.0",
                "https://github.com/user/http.git",
                "abc123",
                Some("sha256:deadbeef".to_string()),
            ),
        ],
    };

    let toml = lock.to_toml_string().unwrap();
    let reparsed = LockFile::from_str(&toml).unwrap();
    assert_eq!(reparsed.version, 1);
    assert_eq!(reparsed.pieces.len(), 2);

    let utils = reparsed.find("utils").unwrap();
    assert!(utils.is_path());
    assert_eq!(utils.path(), Some("../utils"));

    let http = reparsed.find("http").unwrap();
    assert!(http.is_git());
    assert_eq!(http.git_url(), Some("https://github.com/user/http.git"));
    assert_eq!(http.git_rev(), Some("abc123"));
    assert_eq!(http.checksum.as_deref(), Some("sha256:deadbeef"));
}

#[test]
fn test_lock_file_up_to_date_check() {
    use ruxen_cli::lock::LockedPiece;

    let manifest = Manifest::from_str(
        "[package]\nname = \"test\"\nversion = \"0.1.0\"\n\n[dependencies]\nfoo = { path = \"../foo\" }\n",
    )
    .unwrap();

    let matching_lock = LockFile {
        version: 1,
        pieces: vec![LockedPiece::for_path("foo", "0.1.0", "../foo")],
    };
    assert!(matching_lock.is_up_to_date(&manifest));

    let stale_lock = LockFile {
        version: 1,
        pieces: vec![],
    };
    assert!(!stale_lock.is_up_to_date(&manifest));
}

// ─── .rlib Artifacts ───────────────────────────────────────────────

#[test]
fn test_rlib_create_and_load() {
    use ruxen_cli::rlib::*;

    let tmp = temp_dir("rlib_test");
    let rlib_path = tmp.join("test.rlib");

    let metadata = TypeMetadata {
        compiler_version: COMPILER_VERSION.to_string(),
        name: "test".to_string(),
        version: "1.0.0".to_string(),
        exports: Exports {
            functions: vec![ExportedFunction {
                name: "hello".to_string(),
                params: vec![],
                return_type: "String".to_string(),
                visibility: "public".to_string(),
            }],
            ..Default::default()
        },
    };

    create_rlib(&rlib_path, "test", b"object-code", &metadata, "sha256:abc").unwrap();
    assert!(rlib_path.exists());

    let loaded = load_rlib_metadata(&rlib_path).unwrap();
    assert_eq!(loaded.name, "test");
    assert_eq!(loaded.version, "1.0.0");
    assert_eq!(loaded.exports.functions.len(), 1);
    assert_eq!(loaded.exports.functions[0].name, "hello");

    let obj = extract_object_code(&rlib_path).unwrap();
    assert_eq!(obj, b"object-code");

    let hash = extract_hash(&rlib_path).unwrap();
    assert_eq!(hash, "sha256:abc");

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn test_rmeta_creation() {
    use ruxen_cli::rlib::*;

    let tmp = temp_dir("rmeta_test");
    let rmeta_path = tmp.join("test.rmeta");

    let metadata = TypeMetadata {
        compiler_version: COMPILER_VERSION.to_string(),
        name: "test".to_string(),
        version: "0.1.0".to_string(),
        exports: Exports::default(),
    };

    create_rmeta(&rmeta_path, &metadata).unwrap();
    let loaded = load_rmeta_metadata(&rmeta_path).unwrap();
    assert_eq!(loaded.name, "test");

    let _ = fs::remove_dir_all(&tmp);
}

// ─── Dependency Resolution ─────────────────────────────────────────

#[test]
fn test_cycle_detection() {
    use ruxen_cli::resolve_deps;

    let tmp = temp_dir("cycle_integ");
    fs::create_dir_all(tmp.join("cycle-a/src")).unwrap();
    fs::create_dir_all(tmp.join("cycle-b/src")).unwrap();

    fs::write(tmp.join("cycle-a/src/lib.rx"), "pub def a\nend\n").unwrap();
    fs::write(tmp.join("cycle-b/src/lib.rx"), "pub def b\nend\n").unwrap();

    // A depends on B (use a relative path: the security validator
    // refuses absolute paths in manifests by default).
    fs::write(
        tmp.join("cycle-a/Ruxen.toml"),
        "[package]\nname = \"cycle-a\"\nversion = \"0.1.0\"\n\n[dependencies]\ncycle-b = { path = \"../cycle-b\" }\n",
    ).unwrap();

    // B depends on A → cycle!
    fs::write(
        tmp.join("cycle-b/Ruxen.toml"),
        "[package]\nname = \"cycle-b\"\nversion = \"0.1.0\"\n\n[dependencies]\ncycle-a = { path = \"../cycle-a\" }\n",
    ).unwrap();

    let manifest = Manifest::load(&tmp.join("cycle-a")).unwrap();
    let result = resolve_deps::resolve(&tmp.join("cycle-a"), &manifest, None);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Circular dependency detected"));

    let _ = fs::remove_dir_all(&tmp);
}

// ─── Source Hashing ────────────────────────────────────────────────

#[test]
fn test_source_hash_deterministic() {
    use ruxen_cli::rlib;

    let tmp = temp_dir("hash_determ");
    fs::create_dir_all(tmp.join("src")).unwrap();
    fs::write(tmp.join("src/main.rx"), "def main\nend\n").unwrap();

    let h1 = rlib::hash_sources(&tmp).unwrap();
    let h2 = rlib::hash_sources(&tmp).unwrap();
    assert_eq!(h1, h2);
    assert!(h1.starts_with("sha256:"));

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn test_source_hash_changes_on_modification() {
    use ruxen_cli::rlib;

    let tmp = temp_dir("hash_change");
    fs::create_dir_all(tmp.join("src")).unwrap();
    fs::write(tmp.join("src/main.rx"), "def main\nend\n").unwrap();

    let h1 = rlib::hash_sources(&tmp).unwrap();

    fs::write(
        tmp.join("src/main.rx"),
        "def main\n  puts \"changed\"\nend\n",
    )
    .unwrap();
    let h2 = rlib::hash_sources(&tmp).unwrap();

    assert_ne!(h1, h2);

    let _ = fs::remove_dir_all(&tmp);
}
