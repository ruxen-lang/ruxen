//! Build driver: orchestrates cache lookups, invalidation, and parallel
//! compilation via the file-level dependency graph.
//!
//! The driver is **policy-only** — it does not know how to compile a file.
//! Callers pass a `CompileFn` closure that produces object bytes + a
//! `FileSignature` for a given source. This keeps the cache decoupled from
//! the compiler pipeline and makes multi-file scenarios easy to unit test.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use super::graph::{DependencyGraph, FileId};
use super::hash::{hash_bytes, hash_file, to_hex, CacheKey};
use super::manifest::{CacheManifest, CachedFile, ManifestLoadResult};
use super::signature::{interface_changed, FileSignature};
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
    /// Public signature extracted from the typed HIR, used for dep-aware
    /// invalidation. Callers that don't yet extract a signature can pass an
    /// empty `FileSignature { items: vec![] }`.
    pub signature: FileSignature,
    /// Files that this file depends on (logical paths). Used to build the
    /// dependency graph.
    pub dependencies: Vec<String>,
}

/// The compilation callback passed to the driver.
///
/// Must be `Send + Sync` so it can be invoked from rayon worker threads when
/// parallel compilation is enabled.
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
    /// If true, compile independent files in parallel with rayon.
    pub parallel: bool,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            force: false,
            verbose: false,
            target: default_target().to_string(),
            opt_level: "debug".to_string(),
            flags: String::new(),
            parallel: false,
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
    /// Recompiled because an upstream interface changed. As above, downstream
    /// dependents are only invalidated when `output_changed` is true.
    InvalidatedByDependency { output_changed: bool },
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
/// 3. For misses, invokes `compile_fn` in topological order (parallel within
///    a level when `options.parallel` is set).
/// 4. Applies dep-aware invalidation: a file whose public signature is
///    unchanged does NOT invalidate its dependents.
/// 5. Applies content convergence: a file whose *output bytes* are unchanged
///    does NOT invalidate dependents even if its signature changed.
/// 6. Writes the updated manifest atomically.
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
    // Assign a FileId per input, ordered by path for determinism.
    let mut inputs = files;
    inputs.sort_by(|a, b| a.path.cmp(&b.path));
    let path_to_id: HashMap<String, FileId> = inputs
        .iter()
        .enumerate()
        .map(|(i, f)| (f.path.clone(), i))
        .collect();
    let source_hashes: Vec<[u8; 32]> = inputs.iter().map(|f| hash_file(&f.source)).collect();

    // ─── Build per-file WorkItem state ───────────────────────────────
    struct WorkItem {
        source: SourceFile,
        source_hash: [u8; 32],
        cache_key: CacheKey,
        /// Prior cached entry (if any) — used for cache-hit decisions and
        /// for signature comparison after recompilation.
        prior: Option<CachedFile>,
        prior_signature: Option<FileSignature>,
        /// Dependencies discovered during this build (logical paths).
        deps: Vec<String>,
        /// Object bytes in use for this file — either cached or freshly
        /// produced.
        object_bytes: Option<Vec<u8>>,
        /// New signature produced during this build.
        new_signature: Option<FileSignature>,
        /// Hash of the final object bytes; drives content convergence.
        object_hash: Option<[u8; 32]>,
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
            );
            let prior_entry = prior.as_ref().and_then(|m| m.find(&src.path).cloned());
            let prior_sig = prior_entry.as_ref().and_then(|e| {
                let filename = e.signature_file.as_ref()?;
                let hex = filename.strip_suffix(".sig").unwrap_or(filename);
                let bytes = store.load_signature(hex)?;
                FileSignature::from_bytes(&bytes).ok()
            });
            WorkItem {
                source: src,
                source_hash: sh,
                cache_key: key,
                prior: prior_entry,
                prior_signature: prior_sig,
                deps: Vec::new(),
                object_bytes: None,
                new_signature: None,
                object_hash: None,
                status: None,
                object_path: None,
            }
        })
        .collect();

    // ─── First pass: classify cache hits vs misses ───────────────────
    // A cache hit requires: prior entry exists, source hash matches, the
    // object file still exists on disk, and we are not in --force mode.
    let mut needs_compile: HashSet<FileId> = HashSet::new();
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
            Some(bytes) => {
                item.object_hash = Some(hash_bytes(&bytes));
                item.object_bytes = Some(bytes);
                item.status = Some(FileStatus::CacheHit);
                item.object_path = Some(store.object_path(hex));
                item.new_signature = item.prior_signature.clone();
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

    // ─── Build a preliminary dependency graph by pre-parsing deps ────
    // We need dependencies to order compilation. For files we haven't
    // compiled yet in this run, we eagerly invoke compile_fn once to learn
    // their dependencies. (Cost: one compile per dirty file, amortized
    // against the actual compile we need to do anyway.)
    //
    // A cleaner long-term fix is a cheap `parse_uses(source)` helper, but
    // the driver is policy-only — we don't want to reach into the parser
    // here. Callers that know their deps up-front can skip the second
    // compile by caching output.

    // For the MVP, we take a pragmatic approach:
    //   - For cache hits, reuse the previously recorded deps from the
    //     prior manifest.
    //   - For misses, compile eagerly (sequentially for the first pass),
    //     then re-order the level-parallel compilation using the real
    //     graph.
    //
    // This means cache misses pay for a single invocation of compile_fn.
    // Downstream parallel levels simply reuse the produced object bytes.

    // Reconstruct prior deps keyed by path.
    let prior_deps: HashMap<String, Vec<String>> = if let Some(m) = &prior {
        let path_for = |id: FileId| m.files.get(id).map(|f| f.path.clone());
        m.dependency_graph
            .iter()
            .filter_map(|(from, to_list)| {
                let from_path = path_for(*from)?;
                let paths: Vec<String> = to_list.iter().filter_map(|&t| path_for(t)).collect();
                Some((from_path, paths))
            })
            .collect()
    } else {
        HashMap::new()
    };

    for item in &mut items {
        if item.status == Some(FileStatus::CacheHit) {
            if let Some(d) = prior_deps.get(&item.source.path) {
                item.deps = d.clone();
            }
        }
    }

    // ─── Compile dirty files ─────────────────────────────────────────
    //
    // Each file's compile is independent (the callback sees only the source
    // string), so we can run this pass in parallel when the caller opts in.
    // Cache writes go to distinct paths keyed by content hash, so they are
    // safe to issue concurrently. We collect results first and apply them
    // sequentially to keep manifest mutation deterministic.

    let dirty_order = collect_sorted(&needs_compile);
    let outputs: Vec<(FileId, Result<CompileOutput, String>)> = if options.parallel {
        use rayon::prelude::*;
        dirty_order
            .par_iter()
            .map(|&idx| {
                let input = items[idx].source.clone();
                (idx, compile_fn(&input))
            })
            .collect()
    } else {
        dirty_order
            .iter()
            .map(|&idx| {
                let input = items[idx].source.clone();
                (idx, compile_fn(&input))
            })
            .collect()
    };

    for (idx, res) in outputs {
        let out = res?;
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
        item.object_hash = Some(obj_hash);
        item.object_bytes = Some(out.object_bytes);
        item.new_signature = Some(out.signature);
        item.deps = out.dependencies;
        item.status = Some(FileStatus::Recompiled { output_changed });
        // Update the prior entry in place so later dependents see the fresh
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

    // ─── Build the real dependency graph from discovered deps ────────
    let mut graph = DependencyGraph::new();
    for (i, item) in items.iter().enumerate() {
        graph.add_file(i);
        for dep_path in &item.deps {
            if let Some(&to) = path_to_id.get(dep_path) {
                graph.add_edge(i, to);
            }
            // Unknown paths are silently ignored — they refer to external
            // modules (stdlib, deps) which are out of scope for Phase 1.
        }
    }
    graph
        .check_acyclic()
        .map_err(|e| format!("circular import: {:?}", e))?;

    // ─── Cascade invalidation on signature changes ───────────────────
    //
    // A dirty file with a changed signature AND a changed object hash
    // invalidates its transitive dependents. We recompile those dependents
    // here (sequentially in this pass; parallel mode kicks in below).

    let mut invalidated: HashSet<FileId> = HashSet::new();

    // Seed with every file whose recompile changed either signature or output.
    let mut seeds: Vec<FileId> = Vec::new();
    for (idx, item) in items.iter().enumerate() {
        if matches!(item.status, Some(FileStatus::Recompiled { .. })) {
            let sig_changed = match (&item.prior_signature, &item.new_signature) {
                (Some(old), Some(new)) => interface_changed(old, new),
                // No prior signature -> treat as changed (forces first-time cascade).
                _ => true,
            };
            let output_changed = matches!(
                item.status,
                Some(FileStatus::Recompiled {
                    output_changed: true
                })
            );
            if sig_changed && output_changed {
                seeds.push(idx);
            }
        }
    }

    // BFS over dependents; stop expanding once a recompilation produces
    // byte-identical output (content convergence).
    let mut queue: Vec<FileId> = seeds;
    while let Some(origin) = queue.pop() {
        if let Some(deps) = graph.dependents_of(origin) {
            let to_process: Vec<FileId> = deps.iter().copied().collect();
            for dependent in to_process {
                if invalidated.contains(&dependent) || needs_compile.contains(&dependent) {
                    continue;
                }
                invalidated.insert(dependent);
                // Recompile the dependent.
                let item = &mut items[dependent];
                let input = item.source.clone();
                let out = compile_fn(&input)?;
                let obj_hash = hash_bytes(&out.object_bytes);
                let prev_hash = item.object_hash;
                let output_changed = prev_hash.map(|h| h != obj_hash).unwrap_or(true);

                let key_hex = item.cache_key.to_hex();
                let obj_name = store.store_object(&key_hex, &out.object_bytes)?;
                let sig_bytes = out.signature.to_bytes()?;
                let src_hex = to_hex(&item.source_hash);
                let sig_name = store.store_signature(&src_hex, &sig_bytes)?;

                let sig_changed = match (&item.prior_signature, &out.signature) {
                    (Some(old), new) => interface_changed(old, new),
                    _ => true,
                };

                item.object_path = Some(store.object_path(&key_hex));
                item.object_hash = Some(obj_hash);
                item.object_bytes = Some(out.object_bytes);
                item.new_signature = Some(out.signature);
                item.deps = out.dependencies;
                item.status = Some(FileStatus::InvalidatedByDependency { output_changed });
                item.prior = Some(CachedFile {
                    path: item.source.path.clone(),
                    source_hash: item.source_hash,
                    cache_key: item.cache_key.content_hash(),
                    signature_file: Some(sig_name),
                    object_file: Some(obj_name),
                    last_compiled: unix_now_secs(),
                });

                // Only cascade if *both* signature and output changed.
                if sig_changed && output_changed {
                    queue.push(dependent);
                } else if options.verbose {
                    eprintln!(
                        "[cache] {}: signature or output unchanged, cascade stops here",
                        item.source.path
                    );
                }
            }
        }
    }

    // ─── Persist the manifest ────────────────────────────────────────
    let mut new_manifest = CacheManifest::empty(&options.target, &options.flags);
    for item in &items {
        if let Some(entry) = item.prior.clone() {
            new_manifest.files.push(entry);
        }
    }
    // Reindex the dependency graph against the new manifest ordering.
    let new_index: HashMap<String, FileId> = new_manifest
        .files
        .iter()
        .enumerate()
        .map(|(i, f)| (f.path.clone(), i))
        .collect();
    for item in items.iter() {
        let Some(from_entry) = new_manifest
            .files
            .iter()
            .position(|f| f.path == item.source.path)
        else {
            continue;
        };
        let deps: Vec<FileId> = item
            .deps
            .iter()
            .filter_map(|p| new_index.get(p).copied())
            .collect();
        new_manifest.dependency_graph.push((from_entry, deps));
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
            FileStatus::Recompiled { output_changed }
            | FileStatus::InvalidatedByDependency { output_changed } => {
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

fn collect_sorted(set: &HashSet<FileId>) -> Vec<FileId> {
    let mut v: Vec<FileId> = set.iter().copied().collect();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

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
            parallel: false,
        }
    }

    fn src(path: &str, s: &str) -> SourceFile {
        SourceFile {
            path: path.into(),
            source: s.into(),
        }
    }

    // A deterministic fake compiler: object bytes = source bytes, signature
    // = empty, deps pulled from a caller-provided map.
    fn compile_identity<'a>(
        deps_map: &'a HashMap<String, Vec<String>>,
        call_count: &'a AtomicUsize,
    ) -> impl Fn(&SourceFile) -> Result<CompileOutput, String> + Send + Sync + 'a {
        move |f: &SourceFile| {
            call_count.fetch_add(1, Ordering::SeqCst);
            Ok(CompileOutput {
                object_bytes: format!("OBJ<{}>:{}", f.path, f.source).into_bytes(),
                signature: FileSignature { items: vec![] },
                dependencies: deps_map.get(&f.path).cloned().unwrap_or_default(),
            })
        }
    }

    #[test]
    fn first_build_compiles_all_files() {
        let (_td, store) = tmp_store();
        let deps = HashMap::new();
        let calls = AtomicUsize::new(0);
        let f = compile_identity(&deps, &calls);

        let files = vec![src("a.rx", "fn a() {}"), src("b.rx", "fn b() {}")];
        let r = build(files, &store, &opts(), &f).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(r.objects.len(), 2);
        assert!(r.any_object_changed);
    }

    #[test]
    fn no_change_rebuild_is_all_cache_hits() {
        let (_td, store) = tmp_store();
        let deps = HashMap::new();
        let calls = AtomicUsize::new(0);
        let f = compile_identity(&deps, &calls);

        // First build.
        let files = vec![src("a.rx", "1"), src("b.rx", "2")];
        build(files.clone(), &store, &opts(), &f).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        // Second build, same sources.
        let calls2 = AtomicUsize::new(0);
        let f2 = compile_identity(&deps, &calls2);
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
        let deps = HashMap::new();
        let calls = AtomicUsize::new(0);
        let f = compile_identity(&deps, &calls);

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
        let deps = HashMap::new();
        let calls = AtomicUsize::new(0);
        let f = compile_identity(&deps, &calls);
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
        let deps = HashMap::new();
        let calls = AtomicUsize::new(0);
        let f = compile_identity(&deps, &calls);
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
        let deps = HashMap::new();
        let calls = AtomicUsize::new(0);
        let f = compile_identity(&deps, &calls);
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
        let deps = HashMap::new();
        let calls = AtomicUsize::new(0);
        let f = compile_identity(&deps, &calls);

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
        let deps = HashMap::new();
        let calls = AtomicUsize::new(0);
        let f = compile_identity(&deps, &calls);
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
    fn body_only_change_does_not_cascade_to_dependents() {
        let (_td, store) = tmp_store();
        // a depends on b. b changes in a way that produces identical output,
        // so a should stay cached.
        let mut deps_map: HashMap<String, Vec<String>> = HashMap::new();
        deps_map.insert("a.rx".into(), vec!["b.rx".into()]);

        // Custom compiler: b's object bytes are a constant, regardless of source
        // (simulates "body change with no interface change and identical output").
        let calls = Mutex::new(Vec::<String>::new());
        let compile = |f: &SourceFile| {
            calls.lock().unwrap().push(f.path.clone());
            let bytes: Vec<u8> = match f.path.as_str() {
                "b.rx" => b"stable-b-output".to_vec(),
                other => format!("OBJ<{}>:{}", other, f.source).into_bytes(),
            };
            Ok(CompileOutput {
                object_bytes: bytes,
                signature: FileSignature { items: vec![] },
                dependencies: deps_map.get(&f.path).cloned().unwrap_or_default(),
            })
        };

        let files_v1 = vec![src("a.rx", "a-src"), src("b.rx", "b-v1")];
        build(files_v1, &store, &opts(), &compile).unwrap();
        calls.lock().unwrap().clear();

        // Change b's source (not its output bytes or signature).
        let files_v2 = vec![src("a.rx", "a-src"), src("b.rx", "b-v2")];
        let r = build(files_v2, &store, &opts(), &compile).unwrap();
        let compiled: Vec<String> = calls.lock().unwrap().clone();
        assert!(
            compiled.contains(&"b.rx".to_string()),
            "b must be recompiled when its source changes"
        );
        assert!(
            !compiled.contains(&"a.rx".to_string()),
            "a must not be recompiled when b's signature + output are unchanged"
        );
        assert_eq!(r.statuses.get("a.rx"), Some(&FileStatus::CacheHit));
    }

    #[test]
    fn signature_change_cascades_to_dependents() {
        use crate::cache::signature::{PublicItem, SigFn};
        let (_td, store) = tmp_store();
        let mut deps_map: HashMap<String, Vec<String>> = HashMap::new();
        deps_map.insert("a.rx".into(), vec!["b.rx".into()]);

        // b exposes a function whose signature changes between v1 and v2.
        let version = Mutex::new(1u32);
        let calls = Mutex::new(Vec::<String>::new());
        let compile = |f: &SourceFile| {
            calls.lock().unwrap().push(f.path.clone());
            let v = *version.lock().unwrap();
            let sig = if f.path == "b.rx" {
                FileSignature {
                    items: vec![PublicItem::Function(SigFn {
                        name: "foo".into(),
                        generic_params: vec![],
                        self_mode: None,
                        is_class_method: false,
                        params: vec![],
                        return_ty: if v == 1 {
                            "Int".into()
                        } else {
                            "String".into()
                        },
                    })],
                }
            } else {
                FileSignature { items: vec![] }
            };
            // Object bytes differ by version to guarantee `output_changed`.
            let bytes = format!("{}-v{}", f.path, v).into_bytes();
            Ok(CompileOutput {
                object_bytes: bytes,
                signature: sig,
                dependencies: deps_map.get(&f.path).cloned().unwrap_or_default(),
            })
        };

        let files = vec![src("a.rx", "a"), src("b.rx", "b")];
        build(files.clone(), &store, &opts(), &compile).unwrap();
        calls.lock().unwrap().clear();

        // Bump b's signature.
        *version.lock().unwrap() = 2;
        let files_v2 = vec![src("a.rx", "a"), src("b.rx", "b-v2")];
        build(files_v2, &store, &opts(), &compile).unwrap();

        let recorded = calls.lock().unwrap().clone();
        assert!(recorded.contains(&"b.rx".to_string()));
        assert!(
            recorded.contains(&"a.rx".to_string()),
            "a must be recompiled when b's signature changes"
        );
    }

    #[test]
    fn content_convergence_stops_cascade_when_output_unchanged() {
        let (_td, store) = tmp_store();
        let mut deps_map: HashMap<String, Vec<String>> = HashMap::new();
        deps_map.insert("a.rx".into(), vec!["b.rx".into()]);

        // b's signature changes, but b's output bytes are pinned to a
        // constant. The cascade should stop at b and never touch a.
        let compile = |f: &SourceFile| {
            use crate::cache::signature::{PublicItem, SigFn};
            let sig = if f.path == "b.rx" {
                FileSignature {
                    items: vec![PublicItem::Function(SigFn {
                        name: f.source.clone(), // signature depends on source
                        generic_params: vec![],
                        self_mode: None,
                        is_class_method: false,
                        params: vec![],
                        return_ty: "Int".into(),
                    })],
                }
            } else {
                FileSignature { items: vec![] }
            };
            let bytes: Vec<u8> = match f.path.as_str() {
                "b.rx" => b"stable-b-output".to_vec(),
                _ => format!("OBJ<{}>:{}", f.path, f.source).into_bytes(),
            };
            Ok(CompileOutput {
                object_bytes: bytes,
                signature: sig,
                dependencies: deps_map.get(&f.path).cloned().unwrap_or_default(),
            })
        };

        let files_v1 = vec![src("a.rx", "a-src"), src("b.rx", "b-v1")];
        build(files_v1, &store, &opts(), &compile).unwrap();

        // Change b's source -> changes b's signature, but b's output stays the same.
        let files_v2 = vec![src("a.rx", "a-src"), src("b.rx", "b-v2")];
        let r = build(files_v2, &store, &opts(), &compile).unwrap();

        // a must stay a cache hit because b's output bytes did not change.
        assert_eq!(r.statuses.get("a.rx"), Some(&FileStatus::CacheHit));
    }

    #[test]
    fn circular_import_returns_error() {
        let (_td, store) = tmp_store();
        let mut deps_map: HashMap<String, Vec<String>> = HashMap::new();
        deps_map.insert("a.rx".into(), vec!["b.rx".into()]);
        deps_map.insert("b.rx".into(), vec!["a.rx".into()]);

        let calls = AtomicUsize::new(0);
        let f = compile_identity(&deps_map, &calls);
        let files = vec![src("a.rx", "a"), src("b.rx", "b")];
        let r = build(files, &store, &opts(), &f);
        assert!(
            matches!(r, Err(ref e) if e.contains("circular")),
            "expected circular-import error, got {:?}",
            r.as_ref().err()
        );
    }

    #[test]
    fn any_object_changed_is_false_for_warm_cache() {
        let (_td, store) = tmp_store();
        let deps = HashMap::new();
        let calls = AtomicUsize::new(0);
        let f = compile_identity(&deps, &calls);
        let files = vec![src("a.rx", "x")];
        build(files.clone(), &store, &opts(), &f).unwrap();
        let r = build(files, &store, &opts(), &f).unwrap();
        assert!(!r.any_object_changed);
    }

    #[test]
    fn parallel_build_produces_identical_output() {
        let (_td, store_seq) = tmp_store();
        let (_td2, store_par) = tmp_store();
        let mut deps_map: HashMap<String, Vec<String>> = HashMap::new();
        deps_map.insert("a.rx".into(), vec!["b.rx".into(), "c.rx".into()]);
        let calls = AtomicUsize::new(0);
        let f = compile_identity(&deps_map, &calls);

        let files = vec![
            src("a.rx", "a-src"),
            src("b.rx", "b-src"),
            src("c.rx", "c-src"),
        ];

        let seq = build(files.clone(), &store_seq, &opts(), &f).unwrap();
        let mut par_opts = opts();
        par_opts.parallel = true;
        let par = build(files, &store_par, &par_opts, &f).unwrap();

        // Object byte sets must match (order-independent).
        let read = |items: &[(String, PathBuf)]| -> Vec<(String, Vec<u8>)> {
            let mut out: Vec<(String, Vec<u8>)> = items
                .iter()
                .map(|(p, path)| (p.clone(), std::fs::read(path).unwrap()))
                .collect();
            out.sort();
            out
        };
        assert_eq!(read(&seq.objects), read(&par.objects));
    }
}
