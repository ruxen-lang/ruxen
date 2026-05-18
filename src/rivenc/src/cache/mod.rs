//! Incremental compilation cache for rivenc (Phase 13).
//!
//! # Principles
//!
//! - **Content-addressed:** Cache keys are SHA-256 hashes of all inputs that
//!   affect output. Modification timestamps are never consulted.
//! - **Hermetic:** Two builds with the same `CacheKey` must produce
//!   byte-identical output — no hidden environment inputs.
//! - **Fail-open:** Any cache error (corruption, missing file, version
//!   mismatch) triggers a full recompile with a warning, never a panic.
//! - **Atomic writes:** Every artifact is written to `<path>.tmp` then
//!   renamed into place — interrupted builds never leave torn files.
//!
//! # Layout
//!
//! ```text
//! <project>/target/riven/incremental/
//!   ├── manifest.bin
//!   ├── objects/<hex>.o
//!   └── signatures/<hex>.sig
//! ```
//!
//! The global cache (`~/.cache/riven/`) follows a parallel layout and is
//! managed via `store::global_cache_dir()` / `clear_global_cache()`.

// rivenc is a binary crate, so re-exported items without in-binary consumers
// trip the unused-import lint. The cache module is a coherent library-shaped
// surface; suppress the lint at the module boundary.
#![allow(dead_code, unused_imports)]

pub mod driver;
pub mod graph;
pub mod hash;
pub mod manifest;
pub mod signature;
pub mod store;

pub use driver::{
    build, default_target, BuildOptions, BuildResult, CompileFn, CompileOutput, FileStatus,
    SourceFile,
};
pub use graph::{DependencyGraph, FileId, GraphError};
pub use hash::{compiler_version, hash_bytes, hash_file, to_hex, CacheKey};
pub use manifest::{
    CacheHeader, CacheManifest, CachedFile, ManifestLoadResult, FORMAT_VERSION, MAGIC,
};
pub use signature::{
    extract as extract_signature, interface_changed, EnumVariantSig, FileSignature, MixinItemSig,
    PublicItem, SigField, SigFn, SigParam,
};
pub use store::{
    atomic_write, clear_global_cache, global_cache_dir, unix_now_secs, CacheStore,
    INCREMENTAL_DIRNAME, MANIFEST_FILENAME, OBJECTS_DIRNAME, SIGNATURES_DIRNAME,
};
