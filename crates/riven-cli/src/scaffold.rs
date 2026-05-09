//! Project scaffolding: `riven new` and `riven init`.

use std::fs;
use std::path::Path;
use std::process::Command;

/// Create a new Riven project in a new directory under the current working directory.
pub fn new_project(name: &str, lib: bool, no_git: bool) -> Result<(), String> {
    let cwd =
        std::env::current_dir().map_err(|e| format!("failed to get current directory: {}", e))?;
    new_project_in(name, lib, no_git, &cwd)
}

/// Create a new project in the given parent directory. (Testable without changing cwd.)
pub fn new_project_in(name: &str, lib: bool, no_git: bool, parent: &Path) -> Result<(), String> {
    crate::manifest::validate_package_name(name)?;

    let project_dir = parent.join(name);
    if project_dir.exists() {
        return Err(format!(
            "destination `{}` already exists",
            project_dir.display()
        ));
    }

    let kind = if lib { "library" } else { "binary" };
    println!("  Creating {} project `{}`", kind, name);

    fs::create_dir_all(project_dir.join("src"))
        .map_err(|e| format!("failed to create project directory: {}", e))?;

    // Generate Riven.toml
    let manifest = generate_manifest(name, lib);
    fs::write(project_dir.join("Riven.toml"), manifest)
        .map_err(|e| format!("failed to write Riven.toml: {}", e))?;
    println!("      Created Riven.toml");

    // Generate source file
    if lib {
        let lib_src = "pub def hello -> String\n  \"Hello from {}!\"\nend\n".replace("{}", name);
        fs::write(project_dir.join("src/lib.rvn"), lib_src)
            .map_err(|e| format!("failed to write src/lib.rvn: {}", e))?;
        println!("      Created src/lib.rvn");
    } else {
        let main_src = "def main { puts(\"Hello, Riven!\") }\n";
        fs::write(project_dir.join("src/main.rvn"), main_src)
            .map_err(|e| format!("failed to write src/main.rvn: {}", e))?;
        println!("      Created src/main.rvn");
    }

    // Generate .gitignore
    let gitignore = generate_gitignore(lib);
    fs::write(project_dir.join(".gitignore"), gitignore)
        .map_err(|e| format!("failed to write .gitignore: {}", e))?;
    println!("      Created .gitignore");

    // Initialize git repository
    if !no_git {
        init_git(&project_dir)?;
    }

    Ok(())
}

/// Initialize a Riven project in the current directory.
pub fn init_project() -> Result<(), String> {
    let cwd =
        std::env::current_dir().map_err(|e| format!("failed to get current directory: {}", e))?;

    if cwd.join("Riven.toml").exists() {
        return Err("Riven.toml already exists in this directory".to_string());
    }

    let name = cwd
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("my-project")
        .to_string();

    // Normalize name: replace invalid chars with hyphens, lowercase
    let name: String = name
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let name = name.trim_matches('-');

    // Determine if lib or binary based on existing files
    let lib = cwd.join("src/lib.rvn").exists() && !cwd.join("src/main.rvn").exists();

    let kind = if lib { "library" } else { "binary" };
    println!("  Initializing {} project `{}`", kind, name);

    // Create src/ if it doesn't exist
    let src_dir = cwd.join("src");
    if !src_dir.exists() {
        fs::create_dir_all(&src_dir)
            .map_err(|e| format!("failed to create src/ directory: {}", e))?;
    }

    // Generate Riven.toml
    let manifest = generate_manifest(name, lib);
    fs::write(cwd.join("Riven.toml"), manifest)
        .map_err(|e| format!("failed to write Riven.toml: {}", e))?;
    println!("      Created Riven.toml");

    // Generate source files if they don't exist
    if lib && !cwd.join("src/lib.rvn").exists() {
        let lib_src = format!("pub def hello -> String\n  \"Hello from {}!\"\nend\n", name);
        fs::write(cwd.join("src/lib.rvn"), lib_src)
            .map_err(|e| format!("failed to write src/lib.rvn: {}", e))?;
        println!("      Created src/lib.rvn");
    } else if !lib && !cwd.join("src/main.rvn").exists() {
        let main_src = "def main { puts(\"Hello, Riven!\") }\n";
        fs::write(cwd.join("src/main.rvn"), main_src)
            .map_err(|e| format!("failed to write src/main.rvn: {}", e))?;
        println!("      Created src/main.rvn");
    }

    // Generate .gitignore if it doesn't exist
    if !cwd.join(".gitignore").exists() {
        let gitignore = generate_gitignore(lib);
        fs::write(cwd.join(".gitignore"), gitignore)
            .map_err(|e| format!("failed to write .gitignore: {}", e))?;
        println!("      Created .gitignore");
    }

    // Initialize git if not already a repo
    if !cwd.join(".git").exists() {
        init_git(&cwd)?;
    }

    Ok(())
}

fn generate_manifest(name: &str, lib: bool) -> String {
    let mut toml = format!(
        "[package]\nname = \"{}\"\nversion = \"0.1.0\"\nedition = \"2026\"\n",
        name
    );
    if lib {
        toml.push_str("\n[build]\ntype = \"library\"\n");
    }
    toml
}

fn generate_gitignore(lib: bool) -> String {
    let mut content = "/target\n".to_string();
    if lib {
        content.push_str("Riven.lock\n");
    }
    content
}

fn init_git(dir: &Path) -> Result<(), String> {
    let status = Command::new("git")
        .args(["init"])
        .current_dir(dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("      Initialized git repository");
            Ok(())
        }
        Ok(_) => {
            // git init failed but that's not fatal
            eprintln!("  warning: failed to initialize git repository");
            Ok(())
        }
        Err(_) => {
            // git not installed, not fatal
            eprintln!("  warning: `git` not found, skipping repository initialization");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_generate_manifest_binary() {
        let manifest = generate_manifest("my-project", false);
        assert!(manifest.contains("name = \"my-project\""));
        assert!(manifest.contains("version = \"0.1.0\""));
        assert!(manifest.contains("edition = \"2026\""));
        assert!(!manifest.contains("[build]"));
    }

    #[test]
    fn test_generate_manifest_library() {
        let manifest = generate_manifest("my-lib", true);
        assert!(manifest.contains("name = \"my-lib\""));
        assert!(manifest.contains("type = \"library\""));
    }

    #[test]
    fn test_generate_gitignore_binary() {
        let gi = generate_gitignore(false);
        assert!(gi.contains("/target"));
        assert!(!gi.contains("Riven.lock"));
    }

    #[test]
    fn test_generate_gitignore_library() {
        let gi = generate_gitignore(true);
        assert!(gi.contains("/target"));
        assert!(gi.contains("Riven.lock"));
    }

    #[test]
    fn test_new_project_creates_structure() {
        let tmp = env::temp_dir().join(format!(
            "riven_test_new_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let result = new_project_in("test-pkg", false, true, &tmp);
        assert!(result.is_ok(), "new_project failed: {:?}", result);

        let project_dir = tmp.join("test-pkg");
        assert!(project_dir.join("Riven.toml").exists());
        assert!(project_dir.join("src/main.rvn").exists());
        assert!(project_dir.join(".gitignore").exists());

        let manifest_content = fs::read_to_string(project_dir.join("Riven.toml")).unwrap();
        assert!(manifest_content.contains("name = \"test-pkg\""));

        let main_content = fs::read_to_string(project_dir.join("src/main.rvn")).unwrap();
        assert!(main_content.contains("def main"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_new_lib_project() {
        let tmp = env::temp_dir().join(format!(
            "riven_test_lib_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let result = new_project_in("my-lib", true, true, &tmp);
        assert!(result.is_ok(), "new_project failed: {:?}", result);

        let project_dir = tmp.join("my-lib");
        assert!(project_dir.join("src/lib.rvn").exists());
        assert!(!project_dir.join("src/main.rvn").exists());

        let gitignore = fs::read_to_string(project_dir.join(".gitignore")).unwrap();
        assert!(gitignore.contains("Riven.lock"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_new_project_rejects_existing() {
        let tmp = env::temp_dir().join(format!(
            "riven_test_exist_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        fs::create_dir(tmp.join("existing")).unwrap();
        let result = new_project_in("existing", false, true, &tmp);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already exists"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_new_project_validates_name() {
        let result = new_project("Invalid-Name", false, true);
        assert!(result.is_err());
    }
}
