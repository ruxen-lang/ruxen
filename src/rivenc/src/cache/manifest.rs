//! Cache header, per-file entries, and manifest (de)serialization.

use serde::{Deserialize, Serialize};

use super::hash::{compiler_version, hash_u64};

/// Magic bytes that start every cache artifact: `b"RVNC"` (Riven Cache).
pub const MAGIC: [u8; 4] = *b"RVNC";

/// Cache format version. **Bump this on ANY breaking change** to a cached
/// structure — a bump forces every cached artifact to be discarded on load.
pub const FORMAT_VERSION: u32 = 1;

/// Header prefixing every cache artifact on disk.
///
/// On load we validate magic + version before trusting any further bytes.
/// A mismatch means the cache was written by a different compiler or format
/// and must be discarded — never migrated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheHeader {
    pub magic: [u8; 4],
    pub version: u32,
    pub compiler_version: u64,
    pub target_triple_hash: u64,
    pub flags_hash: u64,
}

impl CacheHeader {
    /// Build a header for the current compiler, target, and flags.
    pub fn current(target: &str, flags: &str) -> Self {
        Self {
            magic: MAGIC,
            version: FORMAT_VERSION,
            compiler_version: compiler_version(),
            target_triple_hash: hash_u64(target),
            flags_hash: hash_u64(flags),
        }
    }

    /// Check that a loaded header matches the current compiler's expectations.
    pub fn is_compatible(&self, target: &str, flags: &str) -> bool {
        self.magic == MAGIC
            && self.version == FORMAT_VERSION
            && self.compiler_version == compiler_version()
            && self.target_triple_hash == hash_u64(target)
            && self.flags_hash == hash_u64(flags)
    }
}

/// Per-file cache entry recorded in the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedFile {
    /// Relative path from project root (used only for lookup — never hashed).
    pub path: String,
    /// SHA-256 of source contents.
    pub source_hash: [u8; 32],
    /// Full 32-byte cache key digest (derived from `CacheKey::content_hash`).
    pub cache_key: [u8; 32],
    /// Relative path to signature file under `incremental/signatures/`, if any.
    pub signature_file: Option<String>,
    /// Relative path to object file under `incremental/objects/`, if any.
    pub object_file: Option<String>,
    /// Unix timestamp (seconds) of last successful compilation.
    pub last_compiled: u64,
}

/// Project-local cache manifest. One per project, serialized to
/// `target/riven/incremental/manifest.bin` with postcard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheManifest {
    pub header: CacheHeader,
    pub files: Vec<CachedFile>,
    /// Adjacency list encoded as `(file_index, deps_indices)`.
    pub dependency_graph: Vec<(usize, Vec<usize>)>,
}

/// The outcome of loading a manifest from disk.
///
/// Any error path (missing, corrupt, incompatible) must degrade gracefully to
/// a full rebuild — the cache is an optimization, never a correctness input.
#[derive(Debug)]
pub enum ManifestLoadResult {
    /// Manifest loaded and is compatible with the current compiler.
    Loaded(CacheManifest),
    /// No manifest exists yet.
    Missing,
    /// Manifest exists but is corrupt or incompatible — treat as missing and
    /// log `reason` at warn level.
    Discarded(String),
}

impl CacheManifest {
    /// Build an empty manifest for the given target/flags.
    pub fn empty(target: &str, flags: &str) -> Self {
        Self {
            header: CacheHeader::current(target, flags),
            files: Vec::new(),
            dependency_graph: Vec::new(),
        }
    }

    /// Serialize with postcard.
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        postcard::to_allocvec(self).map_err(|e| format!("manifest serialize: {}", e))
    }

    /// Deserialize and validate against the current target/flags.
    ///
    /// Returns `Ok(Loaded)` on success, or an error if parsing fails. The
    /// caller is responsible for treating failures as "discard and recompile".
    pub fn from_bytes(bytes: &[u8], target: &str, flags: &str) -> ManifestLoadResult {
        match postcard::from_bytes::<CacheManifest>(bytes) {
            Ok(m) => {
                if m.header.is_compatible(target, flags) {
                    ManifestLoadResult::Loaded(m)
                } else {
                    ManifestLoadResult::Discarded(format!(
                        "cache header incompatible (magic={:?}, version={}, compiler={})",
                        m.header.magic, m.header.version, m.header.compiler_version
                    ))
                }
            }
            Err(e) => ManifestLoadResult::Discarded(format!("manifest deserialize: {}", e)),
        }
    }

    /// Find a cached entry by logical path.
    pub fn find(&self, path: &str) -> Option<&CachedFile> {
        self.files.iter().find(|f| f.path == path)
    }

    /// Find-or-insert a mutable cached entry, returning its index.
    pub fn upsert(&mut self, entry: CachedFile) -> usize {
        if let Some(idx) = self.files.iter().position(|f| f.path == entry.path) {
            self.files[idx] = entry;
            idx
        } else {
            self.files.push(entry);
            self.files.len() - 1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_magic_is_rvnc() {
        let h = CacheHeader::current("x86_64-linux", "debug");
        assert_eq!(&h.magic, b"RVNC");
        assert_eq!(h.version, FORMAT_VERSION);
    }

    #[test]
    fn header_roundtrip_via_postcard() {
        let h = CacheHeader::current("x86_64-linux", "debug");
        let bytes = postcard::to_allocvec(&h).unwrap();
        let recovered: CacheHeader = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(h, recovered);
    }

    #[test]
    fn header_compatibility_detects_target_drift() {
        let h = CacheHeader::current("x86_64-linux", "debug");
        assert!(h.is_compatible("x86_64-linux", "debug"));
        assert!(!h.is_compatible("aarch64-linux", "debug"));
        assert!(!h.is_compatible("x86_64-linux", "release"));
    }

    #[test]
    fn manifest_roundtrip_via_postcard() {
        let mut m = CacheManifest::empty("x86_64-linux", "debug");
        m.upsert(CachedFile {
            path: "src/main.rvn".into(),
            source_hash: [1u8; 32],
            cache_key: [2u8; 32],
            signature_file: Some("sig".into()),
            object_file: Some("obj".into()),
            last_compiled: 42,
        });
        m.dependency_graph.push((0, vec![]));

        let bytes = m.to_bytes().unwrap();
        match CacheManifest::from_bytes(&bytes, "x86_64-linux", "debug") {
            ManifestLoadResult::Loaded(recovered) => assert_eq!(recovered, m),
            other => panic!("expected Loaded, got {:?}", other),
        }
    }

    #[test]
    fn manifest_rejects_magic_corruption() {
        let m = CacheManifest::empty("t", "debug");
        let mut bytes = m.to_bytes().unwrap();
        // Corrupt the magic bytes.
        bytes[0] ^= 0xFF;
        match CacheManifest::from_bytes(&bytes, "t", "debug") {
            ManifestLoadResult::Discarded(_) => {}
            other => panic!("expected Discarded on corrupt magic, got {:?}", other),
        }
    }

    #[test]
    fn manifest_rejects_truncated_bytes() {
        let m = CacheManifest::empty("t", "debug");
        let bytes = m.to_bytes().unwrap();
        let truncated = &bytes[..bytes.len() / 2];
        match CacheManifest::from_bytes(truncated, "t", "debug") {
            ManifestLoadResult::Discarded(_) => {}
            other => panic!("expected Discarded on truncated input, got {:?}", other),
        }
    }

    #[test]
    fn manifest_upsert_replaces_existing_entry() {
        let mut m = CacheManifest::empty("t", "debug");
        m.upsert(CachedFile {
            path: "a".into(),
            source_hash: [0u8; 32],
            cache_key: [0u8; 32],
            signature_file: None,
            object_file: None,
            last_compiled: 1,
        });
        m.upsert(CachedFile {
            path: "a".into(),
            source_hash: [1u8; 32],
            cache_key: [0u8; 32],
            signature_file: None,
            object_file: None,
            last_compiled: 2,
        });
        assert_eq!(m.files.len(), 1);
        assert_eq!(m.files[0].source_hash, [1u8; 32]);
        assert_eq!(m.files[0].last_compiled, 2);
    }
}
