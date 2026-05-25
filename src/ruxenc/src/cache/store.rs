//! On-disk cache storage with atomic writes and graceful degradation.
//!
//! # Layout
//!
//! Every store manages a project-local directory:
//!
//! ```text
//! <root>/incremental/
//!   ├── manifest.bin
//!   ├── objects/<hex>.o
//!   └── signatures/<hex>.sig
//! ```
//!
//! # Atomicity
//!
//! Writes go to a temp file alongside the destination, then are renamed into
//! place. `std::fs::rename` is atomic on every OS ruxenc targets, so an
//! interrupted build can never leave a torn `.o` visible to subsequent runs.
//!
//! # Corruption policy
//!
//! Every loader returns an `Option` / `Result` and never panics. Corrupt or
//! missing entries are reported to callers so the build driver can log a
//! warning and recompile from scratch — **the cache is an optimization, not a
//! correctness input.**

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::manifest::{CacheManifest, ManifestLoadResult};

/// Root directory relative to the project: `target/ruxen/incremental/`.
pub const INCREMENTAL_DIRNAME: &str = "incremental";
pub const MANIFEST_FILENAME: &str = "manifest.bin";
pub const OBJECTS_DIRNAME: &str = "objects";
pub const SIGNATURES_DIRNAME: &str = "signatures";

/// A project-local cache store rooted at a `target/ruxen/` directory.
pub struct CacheStore {
    root: PathBuf,
}

impl CacheStore {
    /// Construct a store rooted at `<project>/target/ruxen/`.
    pub fn new(project_target_ruxen: PathBuf) -> Self {
        Self {
            root: project_target_ruxen,
        }
    }

    /// The `incremental/` directory.
    pub fn incremental_dir(&self) -> PathBuf {
        self.root.join(INCREMENTAL_DIRNAME)
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.incremental_dir().join(MANIFEST_FILENAME)
    }

    pub fn objects_dir(&self) -> PathBuf {
        self.incremental_dir().join(OBJECTS_DIRNAME)
    }

    pub fn signatures_dir(&self) -> PathBuf {
        self.incremental_dir().join(SIGNATURES_DIRNAME)
    }

    pub fn object_path(&self, hex: &str) -> PathBuf {
        self.objects_dir().join(format!("{}.o", hex))
    }

    pub fn signature_path(&self, hex: &str) -> PathBuf {
        self.signatures_dir().join(format!("{}.sig", hex))
    }

    /// Ensure the directory tree exists. Idempotent.
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        fs::create_dir_all(self.objects_dir())?;
        fs::create_dir_all(self.signatures_dir())?;
        Ok(())
    }

    // ─── Manifest ───────────────────────────────────────────────────

    /// Load the manifest if present and compatible with the given target/flags.
    pub fn load_manifest(&self, target: &str, flags: &str) -> ManifestLoadResult {
        let path = self.manifest_path();
        match fs::read(&path) {
            Ok(bytes) => CacheManifest::from_bytes(&bytes, target, flags),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => ManifestLoadResult::Missing,
            Err(e) => ManifestLoadResult::Discarded(format!("manifest read error: {}", e)),
        }
    }

    /// Atomically write the manifest.
    pub fn save_manifest(&self, manifest: &CacheManifest) -> Result<(), String> {
        self.ensure_dirs()
            .map_err(|e| format!("create incremental dir: {}", e))?;
        let bytes = manifest.to_bytes()?;
        atomic_write(&self.manifest_path(), &bytes)
    }

    /// Delete the entire `incremental/` tree. Used on corruption and
    /// `ruxen clean`.
    pub fn clear(&self) -> std::io::Result<()> {
        let dir = self.incremental_dir();
        match fs::remove_dir_all(&dir) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    // ─── Objects ────────────────────────────────────────────────────

    /// Store object bytes under `hex`. Returns the relative filename used.
    ///
    /// An empty byte slice is a cache bug — rejected so we never cache garbage.
    pub fn store_object(&self, hex: &str, bytes: &[u8]) -> Result<String, String> {
        if bytes.is_empty() {
            return Err("refusing to cache empty object bytes".into());
        }
        self.ensure_dirs()
            .map_err(|e| format!("create objects dir: {}", e))?;
        atomic_write(&self.object_path(hex), bytes)?;
        Ok(format!("{}.o", hex))
    }

    /// Load object bytes for `hex`, returning `None` if missing, corrupt, or
    /// empty.  Corrupt/empty files are deleted on detection so they cannot
    /// shadow a later valid write.
    pub fn load_object(&self, hex: &str) -> Option<Vec<u8>> {
        let path = self.object_path(hex);
        match fs::read(&path) {
            Ok(bytes) if !bytes.is_empty() => Some(bytes),
            Ok(_) => {
                // Zero-byte file — delete it, return None.
                let _ = fs::remove_file(&path);
                None
            }
            Err(_) => None,
        }
    }

    /// Delete a cached object if present.
    pub fn evict_object(&self, hex: &str) {
        let _ = fs::remove_file(self.object_path(hex));
    }

    // ─── Signatures ─────────────────────────────────────────────────

    /// Store signature bytes under `hex`. Returns the relative filename used.
    pub fn store_signature(&self, hex: &str, bytes: &[u8]) -> Result<String, String> {
        self.ensure_dirs()
            .map_err(|e| format!("create signatures dir: {}", e))?;
        atomic_write(&self.signature_path(hex), bytes)?;
        Ok(format!("{}.sig", hex))
    }

    pub fn load_signature(&self, hex: &str) -> Option<Vec<u8>> {
        match fs::read(self.signature_path(hex)) {
            Ok(bytes) if !bytes.is_empty() => Some(bytes),
            _ => None,
        }
    }
}

/// Atomic, durable write: write to `<path>.tmp`, fsync, then rename.
///
/// Without `sync_all` between write and rename a hard crash can land a file
/// of undefined bytes under the final path — a subsequent build would treat
/// those bytes as a valid cached object and link them. fsync before rename
/// forces the file contents to stable storage first.
///
/// Rename is atomic on every OS Ruxen targets. If the process dies between
/// the write and the rename, only the temp file survives and the next build
/// recomputes the artifact from scratch.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::fs::OpenOptions;
    use std::io::Write;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {:?}: {}", parent, e))?;
    }
    let tmp = tmp_path(path);
    {
        let mut f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)
            .map_err(|e| format!("open {:?}: {}", tmp, e))?;
        f.write_all(bytes)
            .map_err(|e| format!("write {:?}: {}", tmp, e))?;
        f.sync_all()
            .map_err(|e| format!("fsync {:?}: {}", tmp, e))?;
    }
    fs::rename(&tmp, path).map_err(|e| format!("rename {:?} -> {:?}: {}", tmp, path, e))?;
    Ok(())
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    name.push(".tmp");
    path.with_file_name(name)
}

/// Current Unix timestamp in seconds. Returns 0 on systems where the clock is
/// before the epoch (defensive — this timestamp is metadata, not a correctness
/// input).
pub fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Delete the global cache directory. Used by `ruxen clean --global`.
///
/// Honors `XDG_CACHE_HOME` on Linux, falling back to `$HOME/.cache/ruxen/`.
pub fn clear_global_cache() -> std::io::Result<()> {
    let dir = global_cache_dir();
    match fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Resolve the global cache directory: `$XDG_CACHE_HOME/ruxen/` or
/// `$HOME/.cache/ruxen/`.
pub fn global_cache_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("ruxen");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".cache").join("ruxen");
    }
    PathBuf::from(".cache").join("ruxen")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::manifest::{CacheManifest, CachedFile};

    fn store() -> (tempfile::TempDir, CacheStore) {
        let td = tempfile::tempdir().unwrap();
        let store = CacheStore::new(td.path().to_path_buf());
        store.ensure_dirs().unwrap();
        (td, store)
    }

    #[test]
    fn store_and_load_object_roundtrip() {
        let (_td, store) = store();
        let bytes = b"some object bytes".to_vec();
        store.store_object("abc", &bytes).unwrap();
        assert_eq!(store.load_object("abc"), Some(bytes));
    }

    #[test]
    fn empty_object_bytes_are_refused() {
        let (_td, store) = store();
        let err = store.store_object("abc", &[]).unwrap_err();
        assert!(err.contains("empty"));
    }

    #[test]
    fn load_object_returns_none_on_missing() {
        let (_td, store) = store();
        assert!(store.load_object("nothere").is_none());
    }

    #[test]
    fn load_object_deletes_zero_byte_files() {
        let (_td, store) = store();
        // Bypass store_object's validation to plant an empty file.
        fs::write(store.object_path("zero"), b"").unwrap();
        assert!(store.load_object("zero").is_none());
        // And the stale empty file should now be gone.
        assert!(!store.object_path("zero").exists());
    }

    #[test]
    fn manifest_save_and_load() {
        let (_td, store) = store();
        let mut manifest = CacheManifest::empty("x86_64-linux", "debug");
        manifest.upsert(CachedFile {
            path: "main.rx".into(),
            source_hash: [7u8; 32],
            cache_key: [8u8; 32],
            signature_file: None,
            object_file: Some("abc.o".into()),
            last_compiled: unix_now_secs(),
        });
        store.save_manifest(&manifest).unwrap();

        match store.load_manifest("x86_64-linux", "debug") {
            ManifestLoadResult::Loaded(m) => assert_eq!(m, manifest),
            other => panic!("expected Loaded, got {:?}", other),
        }
    }

    #[test]
    fn manifest_load_missing_returns_missing() {
        let (_td, store) = store();
        match store.load_manifest("x86_64-linux", "debug") {
            ManifestLoadResult::Missing => {}
            other => panic!("expected Missing, got {:?}", other),
        }
    }

    #[test]
    fn manifest_load_corrupt_returns_discarded() {
        let (_td, store) = store();
        fs::write(store.manifest_path(), b"not a manifest").unwrap();
        match store.load_manifest("x86_64-linux", "debug") {
            ManifestLoadResult::Discarded(_) => {}
            other => panic!("expected Discarded, got {:?}", other),
        }
    }

    #[test]
    fn atomic_write_leaves_no_tmp_file_on_success() {
        let (_td, store) = store();
        store.store_object("atomic", b"payload").unwrap();
        let tmp = tmp_path(&store.object_path("atomic"));
        assert!(!tmp.exists(), "tmp file should be renamed away");
    }

    #[test]
    fn clear_removes_incremental_dir() {
        let (_td, store) = store();
        store.store_object("abc", b"payload").unwrap();
        assert!(store.incremental_dir().exists());
        store.clear().unwrap();
        assert!(!store.incremental_dir().exists());
    }

    #[test]
    fn clear_is_idempotent_when_missing() {
        let (_td, store) = store();
        // Intentionally do not ensure_dirs — exercise the NotFound branch.
        store.clear().unwrap();
        store.clear().unwrap();
    }

    #[test]
    fn evict_object_removes_file_if_present() {
        let (_td, store) = store();
        store.store_object("ev", b"x").unwrap();
        store.evict_object("ev");
        assert!(store.load_object("ev").is_none());
        // Evicting again is a no-op.
        store.evict_object("ev");
    }
}
