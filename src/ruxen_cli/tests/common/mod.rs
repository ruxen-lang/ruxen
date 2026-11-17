//! Shared helpers for the ruxen_cli integration tests.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tempfile::TempDir;

pub fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// The freshly built `ruxen` (target/, never the installed `~/.ruxen`
/// one), staged once per process into an install layout
/// (`<tmp>/bin/ruxen` plus `<tmp>/lib/std/`). Returns the staged binary.
///
/// On Linux, parallel `fs::copy + Command::new().spawn()` races into
/// ETXTBUSY because a sibling thread's fork inherits the still-open write
/// fd. Staging exactly once, for every test in the process, eliminates
/// the race: every test spawns from a binary that was fully written and
/// closed before any test used it.
pub fn shared_install() -> &'static Path {
    static INSTALL: OnceLock<(TempDir, PathBuf)> = OnceLock::new();
    &INSTALL
        .get_or_init(|| {
            let temp = tempfile::tempdir().expect("mktemp shared install");
            let bin_dir = temp.path().join("bin");
            let lib_dir = temp.path().join("lib");
            fs::create_dir_all(&bin_dir).unwrap();
            fs::create_dir_all(&lib_dir).unwrap();

            let staged = bin_dir.join("ruxen");
            fs::copy(env!("CARGO_BIN_EXE_ruxen"), &staged).expect("copy ruxen");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(&staged).unwrap().permissions();
                perms.set_mode(0o755);
                fs::set_permissions(&staged, perms).unwrap();
            }

            // Mirror the full `library/std/` tree into `<install>/lib/std/`:
            // runtime sources `#include` sibling packages by relative path,
            // so the whole tree must be present at the destination.
            copy_runtime_tree(&workspace_root().join("library/std"), &lib_dir.join("std"));
            (temp, staged)
        })
        .1
}

fn copy_runtime_tree(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let dest = dst.join(entry.file_name());
        if path.is_dir() {
            copy_runtime_tree(&path, &dest);
        } else {
            fs::copy(&path, &dest).expect("copy runtime file");
        }
    }
}
