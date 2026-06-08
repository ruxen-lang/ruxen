//! Content-addressed hashing and cache key construction.
//!
//! The cache key is **hermetic** — its value depends only on inputs that affect
//! the compiled output (source contents, compiler version, target triple, flags).
//! Nothing in this module reads file modification times, environment variables,
//! or any other ambient state.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Compute SHA-256 of a source string.
pub fn hash_file(source: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(source.as_bytes());
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// Compute SHA-256 of arbitrary bytes.
pub fn hash_bytes(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// Current compiler version identifier.
///
/// Derived from `CARGO_PKG_VERSION`. When the ruxenc version changes, every
/// cache entry is invalidated — this protects against stale artifacts produced
/// by older compiler builds with different output formats.
pub fn compiler_version() -> u64 {
    // Splice the crate version with a schema tag so bumping the cache format
    // invalidates caches even without a version bump.
    const SCHEMA: &str = "ruxenc-cache-v1";
    let s = format!("{}|{}", env!("CARGO_PKG_VERSION"), SCHEMA);
    let h = hash_file(&s);
    u64::from_le_bytes(h[..8].try_into().unwrap())
}

/// Hash of a string, reduced to a u64. Used for target triple and flag hashing
/// inside the cache header (8-byte fields for compactness).
pub fn hash_u64(s: &str) -> u64 {
    let h = hash_file(s);
    u64::from_le_bytes(h[..8].try_into().unwrap())
}

/// The full set of inputs that determine compilation output for a single file.
///
/// Two compilations with the same `CacheKey` MUST produce byte-identical object
/// code. This is the content-addressing invariant.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CacheKey {
    /// SHA-256 of the source file contents.
    pub source_hash: [u8; 32],
    /// Hash of the compiler binary version.
    pub compiler_version: u64,
    /// Target triple string (e.g., "x86_64-unknown-linux-gnu").
    pub target: String,
    /// Optimization level ("debug", "release").
    pub opt_level: String,
    /// Free-form build-flags string folded into the key. Carries the
    /// backend/opt overrides, the project `runtime/*.c` fingerprint, AND
    /// the TOOLCHAIN-IDENTITY fingerprint (the running compiler binary's
    /// mtime/size — see `compile.rs::toolchain_fingerprint`). Including it
    /// here is load-bearing for Q24: `compiler_version` is derived only
    /// from `CARGO_PKG_VERSION`, so rebuilding the toolchain FROM SOURCE
    /// (same version — the `ruxen upgrade --from-source` dev loop) or
    /// changing an embedded-stdlib `.rx`/`.c` leaves `compiler_version`
    /// unchanged. Without the fingerprint, the old per-object key still
    /// matches and a stale object — with the OLD compiler's move/borrow
    /// behaviour — is replayed (`E1001`/`E1009` with bogus spans). The
    /// fingerprint forces a miss so the new toolchain re-emits diagnostics.
    pub flags: String,
}

impl CacheKey {
    /// Construct a cache key from its component inputs.
    pub fn new(
        source_hash: [u8; 32],
        compiler_version: u64,
        target: impl Into<String>,
        opt_level: impl Into<String>,
        flags: impl Into<String>,
    ) -> Self {
        Self {
            source_hash,
            compiler_version,
            target: target.into(),
            opt_level: opt_level.into(),
            flags: flags.into(),
        }
    }

    /// Reduce the cache key to a 32-byte content hash suitable for use as a
    /// filesystem path component.
    pub fn content_hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.source_hash);
        hasher.update(self.compiler_version.to_le_bytes());
        hasher.update(self.target.as_bytes());
        hasher.update([0u8]); // separator between variable-length fields
        hasher.update(self.opt_level.as_bytes());
        hasher.update([0u8]); // separator
        hasher.update(self.flags.as_bytes());
        let digest = hasher.finalize();
        let mut out = [0u8; 32];
        out.copy_from_slice(&digest);
        out
    }

    /// Hex-encoded content hash — stable string form for filesystem keys.
    pub fn to_hex(&self) -> String {
        to_hex(&self.content_hash())
    }
}

/// Render a 32-byte hash as a 64-character lowercase hex string.
pub fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_deterministic() {
        assert_eq!(hash_file("hello"), hash_file("hello"));
        assert_ne!(hash_file("hello"), hash_file("world"));
    }

    #[test]
    fn empty_source_has_stable_hash() {
        // SHA-256 of the empty string is well-known — this catches any drift
        // in the underlying hasher configuration.
        let expected = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(to_hex(&hash_file("")), expected);
    }

    #[test]
    fn cache_key_hermetic() {
        let src_hash = hash_file("fn main() {}");
        let k1 = CacheKey::new(src_hash, 1, "x86_64-linux", "debug", "flags");
        let k2 = CacheKey::new(src_hash, 1, "x86_64-linux", "debug", "flags");
        assert_eq!(k1.to_hex(), k2.to_hex());
    }

    #[test]
    fn cache_key_differs_on_source_change() {
        let a = CacheKey::new(hash_file("a"), 1, "t", "debug", "flags");
        let b = CacheKey::new(hash_file("b"), 1, "t", "debug", "flags");
        assert_ne!(a.to_hex(), b.to_hex());
    }

    #[test]
    fn cache_key_differs_on_compiler_version() {
        let src = hash_file("x");
        let a = CacheKey::new(src, 1, "t", "debug", "flags");
        let b = CacheKey::new(src, 2, "t", "debug", "flags");
        assert_ne!(a.to_hex(), b.to_hex());
    }

    #[test]
    fn cache_key_differs_on_target() {
        let src = hash_file("x");
        let a = CacheKey::new(src, 1, "x86_64-linux", "debug", "flags");
        let b = CacheKey::new(src, 1, "aarch64-linux", "debug", "flags");
        assert_ne!(a.to_hex(), b.to_hex());
    }

    #[test]
    fn cache_key_differs_on_opt_level() {
        let src = hash_file("x");
        let a = CacheKey::new(src, 1, "t", "debug", "flags");
        let b = CacheKey::new(src, 1, "t", "release", "flags");
        assert_ne!(a.to_hex(), b.to_hex());
    }

    #[test]
    fn cache_key_roundtrips_through_postcard() {
        let key = CacheKey::new(hash_file("roundtrip"), 42, "x86_64-linux", "debug", "flags");
        let bytes = postcard::to_allocvec(&key).unwrap();
        let recovered: CacheKey = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(key, recovered);
    }

    #[test]
    fn cache_key_hex_is_64_chars() {
        let k = CacheKey::new(hash_file("x"), 1, "t", "debug", "flags");
        assert_eq!(k.to_hex().len(), 64);
    }

    #[test]
    fn compiler_version_is_stable_within_process() {
        assert_eq!(compiler_version(), compiler_version());
    }

    /// Q24: the per-object key must differ when only the flags string differs
    /// (it carries the toolchain-identity fingerprint). Without this, a
    /// toolchain rebuilt at the same `CARGO_PKG_VERSION` would keep the same
    /// key and replay a stale object — surfacing false move/borrow
    /// diagnostics from the OLD compiler.
    #[test]
    fn cache_key_differs_on_flags() {
        let src = hash_file("x");
        let a = CacheKey::new(src, 1, "t", "debug", "toolchain=aaaa");
        let b = CacheKey::new(src, 1, "t", "debug", "toolchain=bbbb");
        assert_ne!(
            a.to_hex(),
            b.to_hex(),
            "a changed toolchain/flags fingerprint must invalidate the per-object key"
        );
    }
}
