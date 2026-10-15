//! Build driver: orchestrates cache lookups and recompilation of files
//! whose source or cache key changed.
//!
//! The driver is **policy-only** — it does not know how to compile a file.
//! Callers pass a `CompileFn` closure that produces object bytes + a
//! `FileSignature` for a given source. This keeps the cache decoupled from
//! the compiler pipeline and makes multi-file scenarios easy to unit test.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use super::hash::{hash_bytes, hash_file, to_hex, CacheKey};
use super::manifest::{CacheManifest, CachedFile, ManifestLoadResult};
use super::signature::FileSignature;
use super::store::{unix_now_secs, CacheStore};

/// One source file handed to the driver.
#[derive(Debug, Clone)]
pub struct SourceFile {
    /// Logical path used for manifest lookups (typically relative to project).
    pub path: String,
    /// Full source text. Hashed for cache keying.
    pub source: String,
}

/// What a caller's `CompileFn` must return on success.
pub struct CompileOutput {
    /// Raw object bytes for this file — written atomically to the cache.
    pub object_bytes: Vec<u8>,
    /// Public signature extracted from the typed HIR, stored beside the
    /// object. Callers that don't extract a signature can pass an empty
    /// `FileSignature { items: vec![] }`.
    pub signature: FileSignature,
}

/// The compilation callback passed to the driver.
pub type CompileFn<'a> = &'a (dyn Fn(&SourceFile) -> Result<CompileOutput, String> + Send + Sync);

/// Build-wide configuration.
pub struct BuildOptions {
    /// Ignore all caches; recompile every file but still write results.
    pub force: bool,
    /// Emit `[cache] ...` log lines at stderr.
    pub verbose: bool,
    /// Target triple — part of the cache key.
    pub target: String,
    /// Optimization level label ("debug" / "release") — part of the cache key.
    pub opt_level: String,
    /// Free-form flags string hashed into the cache header.
    pub flags: String,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            force: false,
            verbose: false,
            target: default_target().to_string(),
            opt_level: "debug".to_string(),
            flags: String::new(),
        }
    }
}

/// Returns a reasonable default target triple for the current host.
pub fn default_target() -> &'static str {
    // Kept coarse on purpose — we only need a stable string per host.
    #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
    {
        "x86_64-unknown-linux-gnu"
    }
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    {
        "aarch64-unknown-linux-gnu"
    }
    #[cfg(all(target_arch = "x86_64", target_os = "macos"))]
    {
        "x86_64-apple-darwin"
    }
    #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
    {
        "aarch64-apple-darwin"
    }
    #[cfg(not(any(
        all(target_arch = "x86_64", target_os = "linux"),
        all(target_arch = "aarch64", target_os = "linux"),
        all(target_arch = "x86_64", target_os = "macos"),
        all(target_arch = "aarch64", target_os = "macos"),
    )))]
    {
        "unknown-unknown-unknown"
    }
}

/// Per-file status after the build completes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileStatus {
    /// Cache hit — the cached object code was reused as-is.
    CacheHit,
    /// Source changed and we recompiled. The object bytes may or may not
    /// differ from the previous cached version (see `output_changed`).
    Recompiled { output_changed: bool },
}

/// The outcome of one `build()` call.
pub struct BuildResult {
    /// Absolute paths to each file's object code (cached or freshly written).
    /// Ordered by the logical file path.
    pub objects: Vec<(String, PathBuf)>,
    /// Per-file status keyed by logical path.
    pub statuses: HashMap<String, FileStatus>,
    /// `true` if any object's content changed from the prior cached version
    /// — callers use this to decide whether to re-link.
    pub any_object_changed: bool,
}

/// Run an incremental build over `files`.
///
/// The driver:
/// 1. Loads the manifest (discards it on corruption).
/// 2. Hashes every source file and decides which are cache hits.
/// 3. For misses, invokes `compile_fn` in path order.
/// 4. Writes the updated manifest atomically.
pub fn build(
    files: Vec<SourceFile>,
    store: &CacheStore,
    options: &BuildOptions,
    compile_fn: CompileFn,
) -> Result<BuildResult, String> {
    store
        .ensure_dirs()
        .map_err(|e| format!("ensure cache dirs: {}", e))?;

    // ─── Load or discard the prior manifest ──────────────────────────
    let prior: Option<CacheManifest> = if options.force {
        None
    } else {
        match store.load_manifest(&options.target, &options.flags) {
            ManifestLoadResult::Loaded(m) => Some(m),
            ManifestLoadResult::Missing => None,
            ManifestLoadResult::Discarded(reason) => {
                if options.verbose {
                    eprintln!(
                        "[cache] manifest discarded, performing full rebuild: {}",
                        reason
                    );
                } else {
                    eprintln!("[cache] manifest corrupted, performing full rebuild");
                }
                // Blow away all cached artifacts so stale signatures/objects
                // cannot resurface.
                let _ = store.clear();
                store
                    .ensure_dirs()
                    .map_err(|e| format!("re-create cache dirs: {}", e))?;
                None
            }
        }
    };

    // ─── Index inputs and precompute hashes ──────────────────────────
    // Order inputs by path for determinism.
    let mut inputs = files;
    inputs.sort_by(|a, b| a.path.cmp(&b.path));
    let source_hashes: Vec<[u8; 32]> = inputs.iter().map(|f| hash_file(&f.source)).collect();

    // ─── Build per-file WorkItem state ───────────────────────────────
    struct WorkItem {
        source: SourceFile,
        source_hash: [u8; 32],
        cache_key: CacheKey,
        /// Prior cached entry (if any) — used for cache-hit decisions.
        prior: Option<CachedFile>,
        status: Option<FileStatus>,
        /// Absolute path where the object lives on disk.
        object_path: Option<PathBuf>,
    }

    let mut items: Vec<WorkItem> = inputs
        .into_iter()
        .zip(source_hashes.iter().copied())
        .map(|(src, sh)| {
            let key = CacheKey::new(
                sh,
                super::hash::compiler_version(),
                &options.target,
                &options.opt_level,
                // Fold the full flags string (backend + opt overrides +
                // runtime_c fingerprint + TOOLCHAIN-IDENTITY fingerprint)
                // into the per-object key so a stale object from a different
                // toolchain build cannot be replayed (Q24).
                &options.flags,
            );
            let prior_entry = prior.as_ref().and_then(|m| m.find(&src.path).cloned());
            WorkItem {
                source: src,
                source_hash: sh,
                cache_key: key,
                prior: prior_entry,
                status: None,
                object_path: None,
            }
        })
        .collect();

    // ─── First pass: classify cache hits vs misses ───────────────────
    // A cache hit requires: prior entry exists, source hash matches, the
    // object file still exists on disk, and we are not in --force mode.
    let mut needs_compile: HashSet<usize> = HashSet::new();
    for (idx, item) in items.iter_mut().enumerate() {
        if options.force {
            needs_compile.insert(idx);
            continue;
        }
        let Some(prior_entry) = &item.prior else {
            needs_compile.insert(idx);
            continue;
        };
        if prior_entry.source_hash != item.source_hash
            || prior_entry.cache_key != item.cache_key.content_hash()
        {
            needs_compile.insert(idx);
            if options.verbose {
                eprintln!(
                    "[cache] {}: cache miss (source or key changed)",
                    item.source.path
                );
            }
            continue;
        }
        let Some(obj_name) = prior_entry.object_file.as_ref() else {
            needs_compile.insert(idx);
            continue;
        };
        let hex = obj_name.strip_suffix(".o").unwrap_or(obj_name);
        match store.load_object(hex) {
            Some(_) => {
                item.status = Some(FileStatus::CacheHit);
                item.object_path = Some(store.object_path(hex));
                if options.verbose {
                    eprintln!("[cache] {}: cache hit (object unchanged)", item.source.path);
                }
            }
            None => {
                if options.verbose {
                    eprintln!(
                        "[cache] {}: cache miss (object missing or corrupt)",
                        item.source.path
                    );
                }
                needs_compile.insert(idx);
            }
        }
    }

    // ─── Compile dirty files ─────────────────────────────────────────
    let dirty_order = collect_sorted(&needs_compile);
    for idx in dirty_order {
        let out = compile_fn(&items[idx].source)?;
        let item = &mut items[idx];
        let obj_hash = hash_bytes(&out.object_bytes);
        let output_changed = item
            .prior
            .as_ref()
            .and_then(|p| p.object_file.as_ref())
            .and_then(|f| {
                let hex = f.strip_suffix(".o").unwrap_or(f);
                store.load_object(hex).map(|b| hash_bytes(&b))
            })
            .map(|prev| prev != obj_hash)
            .unwrap_or(true);

        // Persist object and signature.
        let key_hex = item.cache_key.to_hex();
        let obj_name = store.store_object(&key_hex, &out.object_bytes)?;
        let sig_bytes = out.signature.to_bytes()?;
        let src_hex = to_hex(&item.source_hash);
        let sig_name = store.store_signature(&src_hex, &sig_bytes)?;

        item.object_path = Some(store.object_path(&key_hex));
        item.status = Some(FileStatus::Recompiled { output_changed });
        // Update the prior entry in place so the manifest records the fresh
        // object + signature names.
        item.prior = Some(CachedFile {
            path: item.source.path.clone(),
            source_hash: item.source_hash,
            cache_key: item.cache_key.content_hash(),
            signature_file: Some(sig_name),
            object_file: Some(obj_name),
            last_compiled: unix_now_secs(),
        });
    }

    // ─── Persist the manifest ────────────────────────────────────────
    // Carry forward prior entries for files NOT part of this invocation.
    // The manifest is shared per (target, flags), but callers may build
    // subsets across SEPARATE `build()` calls — e.g. `ruxen test` compiles
    // each test file in its own single-file invocation. Without preserving
    // the others' entries, each invocation would clobber the manifest down
    // to just its own file, so every file would miss the cache on the next
    // run (the symptom: a warm `ruxen test` recompiling everything). Stale
    // carried-over entries self-heal: the cache-hit check re-verifies the
    // object exists on disk and recompiles if not.
    let mut new_manifest = CacheManifest::empty(&options.target, &options.flags);
    let current_paths: HashSet<&str> = items.iter().map(|it| it.source.path.as_str()).collect();
    if let Some(prior_m) = &prior {
        for f in &prior_m.files {
            if !current_paths.contains(f.path.as_str()) {
                new_manifest.files.push(f.clone());
            }
        }
    }
    for item in &items {
        if let Some(entry) = item.prior.clone() {
            new_manifest.files.push(entry);
        }
    }
    store.save_manifest(&new_manifest)?;

    // ─── Build the return value ──────────────────────────────────────
    let mut objects: Vec<(String, PathBuf)> = items
        .iter()
        .filter_map(|it| it.object_path.clone().map(|p| (it.source.path.clone(), p)))
        .collect();
    objects.sort_by(|a, b| a.0.cmp(&b.0));

    let mut statuses: HashMap<String, FileStatus> = HashMap::new();
    let mut any_object_changed = false;
    for item in &items {
        let status = item.status.clone().unwrap_or(FileStatus::CacheHit);
        match &status {
            FileStatus::CacheHit => {}
            FileStatus::Recompiled { output_changed } => {
                if *output_changed {
                    any_object_changed = true;
                }
            }
        }
        statuses.insert(item.source.path.clone(), status);
    }

    Ok(BuildResult {
        objects,
        statuses,
        any_object_changed,
    })
}

fn collect_sorted(set: &HashSet<usize>) -> Vec<usize> {
    let mut v: Vec<usize> = set.iter().copied().collect();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn tmp_store() -> (tempfile::TempDir, CacheStore) {
        let td = tempfile::tempdir().unwrap();
        let store = CacheStore::new(td.path().to_path_buf());
        store.ensure_dirs().unwrap();
        (td, store)
    }

    fn opts() -> BuildOptions {
        BuildOptions {
            force: false,
            verbose: false,
            target: "x86_64-linux".into(),
            opt_level: "debug".into(),
            flags: String::new(),
        }
    }

    fn src(path: &str, s: &str) -> SourceFile {
        SourceFile {
            path: path.into(),
            source: s.into(),
        }
    }

    // A deterministic fake compiler: object bytes = source bytes, signature
    // = empty.
    fn compile_identity(
        call_count: &AtomicUsize,
    ) -> impl Fn(&SourceFile) -> Result<CompileOutput, String> + Send + Sync + '_ {
        move |f: &SourceFile| {
            call_count.fetch_add(1, Ordering::SeqCst);
            Ok(CompileOutput {
                object_bytes: format!("OBJ<{}>:{}", f.path, f.source).into_bytes(),
                signature: FileSignature { items: vec![] },
            })
        }
    }

    #[test]
    fn first_build_compiles_all_files() {
        let (_td, store) = tmp_store();
        let calls = AtomicUsize::new(0);
        let f = compile_identity(&calls);

        let files = vec![src("a.rx", "fn a() {}"), src("b.rx", "fn b() {}")];
        let r = build(files, &store, &opts(), &f).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(r.objects.len(), 2);
        assert!(r.any_object_changed);
    }

    #[test]
    fn no_change_rebuild_is_all_cache_hits() {
        let (_td, store) = tmp_store();
        let calls = AtomicUsize::new(0);
        let f = compile_identity(&calls);

        // First build.
        let files = vec![src("a.rx", "1"), src("b.rx", "2")];
        build(files.clone(), &store, &opts(), &f).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        // Second build, same sources.
        let calls2 = AtomicUsize::new(0);
        let f2 = compile_identity(&calls2);
        let r = build(files, &store, &opts(), &f2).unwrap();
        assert_eq!(
            calls2.load(Ordering::SeqCst),
            0,
            "no compile calls expected on warm cache"
        );
        assert!(!r.any_object_changed);
        for s in r.statuses.values() {
            assert_eq!(*s, FileStatus::CacheHit);
        }
    }

    #[test]
    fn source_change_triggers_recompile() {
        let (_td, store) = tmp_store();
        let calls = AtomicUsize::new(0);
        let f = compile_identity(&calls);

        build(vec![src("a.rx", "v1")], &store, &opts(), &f).unwrap();
        calls.store(0, Ordering::SeqCst);

        let r = build(vec![src("a.rx", "v2")], &store, &opts(), &f).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(matches!(
            r.statuses.get("a.rx"),
            Some(FileStatus::Recompiled {
                output_changed: true
            })
        ));
        assert!(r.any_object_changed);
    }

    #[test]
    fn force_ignores_cache_hits() {
        let (_td, store) = tmp_store();
        let calls = AtomicUsize::new(0);
        let f = compile_identity(&calls);
        let files = vec![src("a.rx", "v1")];
        build(files.clone(), &store, &opts(), &f).unwrap();
        calls.store(0, Ordering::SeqCst);

        let mut forced = opts();
        forced.force = true;
        build(files, &store, &forced, &f).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1, "force should recompile");
    }

    #[test]
    fn target_change_invalidates_cache() {
        let (_td, store) = tmp_store();
        let calls = AtomicUsize::new(0);
        let f = compile_identity(&calls);
        let files = vec![src("a.rx", "v1")];

        build(files.clone(), &store, &opts(), &f).unwrap();
        calls.store(0, Ordering::SeqCst);

        let mut different_target = opts();
        different_target.target = "aarch64-linux".into();
        build(files, &store, &different_target, &f).unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "changing target triple must force recompile"
        );
    }

    #[test]
    fn opt_level_change_invalidates_cache() {
        let (_td, store) = tmp_store();
        let calls = AtomicUsize::new(0);
        let f = compile_identity(&calls);
        let files = vec![src("a.rx", "v1")];

        build(files.clone(), &store, &opts(), &f).unwrap();
        calls.store(0, Ordering::SeqCst);

        let mut release = opts();
        release.opt_level = "release".into();
        build(files, &store, &release, &f).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn corrupt_manifest_triggers_full_rebuild_without_panic() {
        let (_td, store) = tmp_store();
        let calls = AtomicUsize::new(0);
        let f = compile_identity(&calls);

        build(vec![src("a.rx", "x")], &store, &opts(), &f).unwrap();
        // Corrupt the manifest.
        std::fs::write(store.manifest_path(), b"garbage").unwrap();
        calls.store(0, Ordering::SeqCst);

        let r = build(vec![src("a.rx", "x")], &store, &opts(), &f).unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "recompile after corrupt manifest"
        );
        assert_eq!(r.objects.len(), 1);
    }

    #[test]
    fn missing_object_triggers_recompile() {
        let (_td, store) = tmp_store();
        let calls = AtomicUsize::new(0);
        let f = compile_identity(&calls);
        let files = vec![src("a.rx", "x")];
        build(files.clone(), &store, &opts(), &f).unwrap();

        // Delete all cached objects.
        for entry in std::fs::read_dir(store.objects_dir()).unwrap() {
            std::fs::remove_file(entry.unwrap().path()).unwrap();
        }
        calls.store(0, Ordering::SeqCst);
        build(files, &store, &opts(), &f).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn any_object_changed_is_false_for_warm_cache() {
        let (_td, store) = tmp_store();
        let calls = AtomicUsize::new(0);
        let f = compile_identity(&calls);
        let files = vec![src("a.rx", "x")];
        build(files.clone(), &store, &opts(), &f).unwrap();
        let r = build(files, &store, &opts(), &f).unwrap();
        assert!(!r.any_object_changed);
    }
}
