//! Filesystem-based module discovery.
//!
//! Walks `src/` recursively and maps `.rvn` files to module paths.
//! `src/http/client.rvn` → `Http.Client`

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A node in the module tree.
#[derive(Debug, Clone)]
pub struct ModuleNode {
    /// Module name in UpperCamelCase.
    pub name: String,
    /// The `.rvn` file for this module, if any.
    pub file: Option<PathBuf>,
    /// Child modules.
    pub children: BTreeMap<String, ModuleNode>,
}

/// The module tree for a project.
#[derive(Debug, Clone)]
pub struct ModuleTree {
    /// The root node (represents the crate itself).
    pub root: ModuleNode,
    /// All discovered module files, in the order they should be compiled.
    pub files: Vec<(String, PathBuf)>,
}

impl ModuleTree {
    /// Discover modules from the `src/` directory under `project_root`.
    pub fn discover(project_root: &Path) -> Result<Self, String> {
        let src_dir = project_root.join("src");
        if !src_dir.exists() {
            return Err(format!("source directory not found: {}", src_dir.display()));
        }

        let mut root = ModuleNode {
            name: String::new(),
            file: None,
            children: BTreeMap::new(),
        };

        let mut files = Vec::new();

        discover_recursive(&src_dir, &src_dir, &mut root, &mut files)?;

        Ok(ModuleTree { root, files })
    }

    /// Get the module path for a given file path relative to src/.
    pub fn module_path_for_file(rel_path: &Path) -> String {
        let stem = rel_path.with_extension("");
        stem.components()
            .map(|c| to_upper_camel_case(&c.as_os_str().to_string_lossy()))
            .collect::<Vec<_>>()
            .join(".")
    }

    /// Find a module node by its dotted path (e.g. "Http.Client").
    pub fn find(&self, path: &str) -> Option<&ModuleNode> {
        if path.is_empty() {
            return Some(&self.root);
        }
        let segments: Vec<&str> = path.split('.').collect();
        let mut node = &self.root;
        for seg in segments {
            node = node.children.get(seg)?;
        }
        Some(node)
    }

    /// Get all files that need compilation (excluding main.rvn and lib.rvn entry points).
    pub fn module_files(&self) -> Vec<(&str, &Path)> {
        self.files
            .iter()
            .filter(|(name, _)| !name.is_empty())
            .map(|(name, path)| (name.as_str(), path.as_path()))
            .collect()
    }
}

fn discover_recursive(
    src_dir: &Path,
    current_dir: &Path,
    parent_node: &mut ModuleNode,
    files: &mut Vec<(String, PathBuf)>,
) -> Result<(), String> {
    let mut entries: Vec<_> = std::fs::read_dir(current_dir)
        .map_err(|e| format!("failed to read directory {}: {}", current_dir.display(), e))?
        .filter_map(|e| e.ok())
        .collect();

    // Sort for deterministic ordering
    entries.sort_by_key(|e| e.file_name());

    // Separate files and directories
    let mut rvn_files = Vec::new();
    let mut subdirs = Vec::new();

    for entry in &entries {
        let path = entry.path();
        if path.is_dir() {
            subdirs.push(path);
        } else if path.extension().is_some_and(|ext| ext == "rvn") {
            rvn_files.push(path);
        }
    }

    // Process .rvn files
    for file_path in &rvn_files {
        let file_name = file_path.file_stem().unwrap().to_string_lossy().to_string();

        let rel_path = file_path
            .strip_prefix(src_dir)
            .map_err(|e| format!("path error: {}", e))?;

        if file_name == "main" || file_name == "lib" {
            // Entry point — belongs to the root, not a named module
            parent_node.file = Some(file_path.clone());
            files.push((String::new(), file_path.clone()));
            continue;
        }

        let module_name = to_upper_camel_case(&file_name);
        let module_path = ModuleTree::module_path_for_file(&rel_path.with_extension(""));

        // If there's a matching directory, this file is the module root
        let node = parent_node
            .children
            .entry(module_name.clone())
            .or_insert_with(|| ModuleNode {
                name: module_name,
                file: None,
                children: BTreeMap::new(),
            });
        node.file = Some(file_path.clone());
        files.push((module_path, file_path.clone()));
    }

    // Process subdirectories
    for dir_path in &subdirs {
        let dir_name = dir_path.file_name().unwrap().to_string_lossy().to_string();

        // Skip hidden directories
        if dir_name.starts_with('.') {
            continue;
        }

        let module_name = to_upper_camel_case(&dir_name);

        let node = parent_node
            .children
            .entry(module_name.clone())
            .or_insert_with(|| ModuleNode {
                name: module_name,
                file: None,
                children: BTreeMap::new(),
            });

        discover_recursive(src_dir, dir_path, node, files)?;
    }

    Ok(())
}

/// Convert a snake_case or kebab-case name to UpperCamelCase.
///
/// Examples:
/// - `"http"` → `"Http"`
/// - `"http_client"` → `"HttpClient"`
/// - `"my-utils"` → `"MyUtils"`
pub fn to_upper_camel_case(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut capitalize_next = true;

    for c in s.chars() {
        if c == '_' || c == '-' {
            capitalize_next = true;
        } else if capitalize_next {
            result.extend(c.to_uppercase());
            capitalize_next = false;
        } else {
            result.push(c);
        }
    }

    result
}

/// Convert a package name (with hyphens) to a module import name (with underscores).
///
/// `"my-utils"` → `"my_utils"`
pub fn package_to_import_name(name: &str) -> String {
    name.replace('-', "_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_to_upper_camel_case() {
        assert_eq!(to_upper_camel_case("http"), "Http");
        assert_eq!(to_upper_camel_case("http_client"), "HttpClient");
        assert_eq!(to_upper_camel_case("my-utils"), "MyUtils");
        assert_eq!(to_upper_camel_case("main"), "Main");
        assert_eq!(to_upper_camel_case("a"), "A");
        assert_eq!(to_upper_camel_case("HTTP"), "HTTP");
    }

    #[test]
    fn test_package_to_import_name() {
        assert_eq!(package_to_import_name("my-utils"), "my_utils");
        assert_eq!(package_to_import_name("http"), "http");
        assert_eq!(package_to_import_name("a-b-c"), "a_b_c");
    }

    #[test]
    fn test_module_path_for_file() {
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

    fn create_test_project(dir: &Path) {
        let src = dir.join("src");
        fs::create_dir_all(src.join("http")).unwrap();

        fs::write(src.join("main.rvn"), "def main\nend\n").unwrap();
        fs::write(src.join("utils.rvn"), "pub def helper\nend\n").unwrap();
        fs::write(src.join("http.rvn"), "# http module root\n").unwrap();
        fs::write(src.join("http/client.rvn"), "pub class Client\nend\n").unwrap();
        fs::write(src.join("http/server.rvn"), "pub class Server\nend\n").unwrap();
    }

    #[test]
    fn test_discover_modules() {
        let tmp = std::env::temp_dir().join(format!("riven_mod_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        create_test_project(&tmp);

        let tree = ModuleTree::discover(&tmp).unwrap();

        // Root should have the main.rvn file
        assert!(tree.root.file.is_some());

        // Should find Utils module
        let utils = tree.find("Utils");
        assert!(utils.is_some());
        assert!(utils.unwrap().file.is_some());

        // Should find Http module
        let http = tree.find("Http");
        assert!(http.is_some());
        assert!(http.unwrap().file.is_some());

        // Should find Http.Client and Http.Server
        let client = tree.find("Http.Client");
        assert!(client.is_some());

        let server = tree.find("Http.Server");
        assert!(server.is_some());

        // Module files should not include the entry point
        let mod_files = tree.module_files();
        assert!(!mod_files.is_empty());

        // All module files should have non-empty module names
        for (name, _) in &mod_files {
            assert!(!name.is_empty());
        }

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_discover_nested_modules() {
        let tmp = std::env::temp_dir().join(format!("riven_nested_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let src = tmp.join("src");
        fs::create_dir_all(src.join("http/client")).unwrap();

        fs::write(src.join("main.rvn"), "").unwrap();
        fs::write(src.join("http/client/pool.rvn"), "").unwrap();

        let tree = ModuleTree::discover(&tmp).unwrap();

        // Http.Client.Pool should exist
        let pool = tree.find("Http.Client.Pool");
        assert!(pool.is_some());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_discover_empty_src() {
        let tmp = std::env::temp_dir().join(format!("riven_empty_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("src")).unwrap();

        let tree = ModuleTree::discover(&tmp).unwrap();
        assert!(tree.root.file.is_none());
        assert!(tree.root.children.is_empty());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_discover_no_src() {
        let tmp = std::env::temp_dir().join(format!("riven_nosrc_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let result = ModuleTree::discover(&tmp);
        assert!(result.is_err());

        let _ = fs::remove_dir_all(&tmp);
    }
}
