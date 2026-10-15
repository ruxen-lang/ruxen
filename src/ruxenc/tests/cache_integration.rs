//! Integration tests for the incremental cache.
//!
//! These exercise the driver API end-to-end with a mock `CompileFn`, covering
//! multi-file scenarios that are hard to unit-test in isolation.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use ruxenc::cache::{
    build, BuildOptions, CacheStore, CompileOutput, FileSignature, FileStatus, PublicItem, SigFn,
    SourceFile,
};

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

fn sig_fn(name: &str, ret: &str) -> FileSignature {
    FileSignature {
        items: vec![PublicItem::Function(SigFn {
            name: name.into(),
            generic_params: vec![],
            self_mode: None,
            is_class_method: false,
            params: vec![],
            return_ty: ret.into(),
        })],
    }
}

fn src(path: &str, body: &str) -> SourceFile {
    SourceFile {
        path: path.into(),
        source: body.into(),
    }
}

/// Build a deterministic project of N files `f0.rx` .. `fN-1.rx`.
fn project(n: usize) -> Vec<SourceFile> {
    (0..n)
        .map(|i| src(&format!("f{}.rx", i), &format!("fn f{}() {{}}", i)))
        .collect()
}

#[test]
fn ten_files_cold_then_warm() {
    let (_td, store) = tmp_store();
    let files = project(10);

    let calls = AtomicUsize::new(0);
    let compile = |f: &SourceFile| {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(CompileOutput {
            object_bytes: format!("OBJ<{}>", f.source).into_bytes(),
            signature: sig_fn(&f.path, "Int"),
        })
    };

    // Cold build.
    let r1 = build(files.clone(), &store, &opts(), &compile).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 10);
    assert_eq!(r1.objects.len(), 10);

    // Warm rebuild — no compile invocations.
    calls.store(0, Ordering::SeqCst);
    let r2 = build(files, &store, &opts(), &compile).unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "warm rebuild must not call compile"
    );
    assert!(!r2.any_object_changed);
    for status in r2.statuses.values() {
        assert_eq!(*status, FileStatus::CacheHit);
    }
}

#[test]
fn one_file_edit_recompiles_only_that_file() {
    let (_td, store) = tmp_store();
    let files = project(5);

    let calls = Mutex::new(Vec::<String>::new());
    let compile = |f: &SourceFile| {
        calls.lock().unwrap().push(f.path.clone());
        Ok(CompileOutput {
            object_bytes: format!("OBJ<{}>", f.path).into_bytes(), // bytes depend only on path
            signature: sig_fn(&f.path, "Int"),
        })
    };

    build(files.clone(), &store, &opts(), &compile).unwrap();
    calls.lock().unwrap().clear();

    // Edit one file's source.
    let mut modified = files.clone();
    modified
        .iter_mut()
        .find(|f| f.path == "f4.rx")
        .unwrap()
        .source = "# body-only edit".into();
    build(modified, &store, &opts(), &compile).unwrap();

    let recorded = calls.lock().unwrap().clone();
    assert!(recorded.contains(&"f4.rx".to_string()));
    for other in &["f0.rx", "f1.rx", "f2.rx", "f3.rx"] {
        assert!(
            !recorded.contains(&(*other).to_string()),
            "{} should NOT be recompiled when only f4.rx changed, but it was",
            other
        );
    }
}

#[test]
fn force_recompiles_the_whole_project() {
    let (_td, store) = tmp_store();
    let files = project(5);
    let calls = AtomicUsize::new(0);
    let compile = |f: &SourceFile| {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(CompileOutput {
            object_bytes: format!("OBJ<{}>", f.path).into_bytes(),
            signature: sig_fn(&f.path, "Int"),
        })
    };

    build(files.clone(), &store, &opts(), &compile).unwrap();
    calls.store(0, Ordering::SeqCst);

    let mut forced = opts();
    forced.force = true;
    build(files, &store, &forced, &compile).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 5);
}

#[test]
fn corrupt_object_causes_recompile_without_panic() {
    let (_td, store) = tmp_store();
    let files = project(3);
    let calls = AtomicUsize::new(0);
    let compile = |f: &SourceFile| {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(CompileOutput {
            object_bytes: format!("OBJ<{}>", f.path).into_bytes(),
            signature: sig_fn(&f.path, "Int"),
        })
    };

    let r = build(files.clone(), &store, &opts(), &compile).unwrap();
    let any_obj = &r.objects[0].1;
    // Truncate an object to empty to simulate mid-write interruption.
    std::fs::write(any_obj, b"").unwrap();

    calls.store(0, Ordering::SeqCst);
    let r2 = build(files, &store, &opts(), &compile).unwrap();
    assert!(
        calls.load(Ordering::SeqCst) >= 1,
        "at least the corrupt-object file should be recompiled"
    );
    assert_eq!(r2.objects.len(), 3);
}

// ─── tier 4.02: per-target cache isolation ──────────────────────────────
//
// The cache key already carries a `target` component; making it the real
// triple means the same source compiled for two targets produces two distinct
// cache entries. This pins that the `target` field is load-bearing in the
// content hash — a host object can never be replayed for a cross target (or
// vice versa).
mod target_cache_isolation {
    use ruxenc::cache::{hash_file, CacheKey};

    fn key_for(source: &str, target: &str) -> CacheKey {
        CacheKey::new(
            hash_file(source),
            /* compiler_version */ 42,
            target,
            "debug",
            "backend=default",
        )
    }

    #[test]
    fn same_source_two_targets_distinct_keys() {
        let src = "def main\n  println(\"hi\")\nend\n";
        let host = key_for(src, "aarch64-apple-darwin");
        let linux = key_for(src, "aarch64-unknown-linux-gnu");
        let x64 = key_for(src, "x86_64-apple-darwin");

        // Same source, different targets → different content hashes.
        assert_ne!(
            host.to_hex(),
            linux.to_hex(),
            "host and linux targets must not share a cache entry"
        );
        assert_ne!(host.to_hex(), x64.to_hex());
        assert_ne!(linux.to_hex(), x64.to_hex());
    }

    #[test]
    fn same_source_same_target_stable_key() {
        let src = "def main\n  0\nend\n";
        let a = key_for(src, "x86_64-unknown-linux-gnu");
        let b = key_for(src, "x86_64-unknown-linux-gnu");
        assert_eq!(a.to_hex(), b.to_hex(), "key must be deterministic");
    }

    #[test]
    fn target_only_difference_flips_the_hash() {
        // Hold everything else constant; only the target differs. Proves the
        // target field participates in `content_hash` (no cross-pollution).
        let src = "def main\n  1\nend\n";
        let a = key_for(src, "aarch64-unknown-linux-gnu");
        let b = key_for(src, "x86_64-unknown-linux-gnu");
        assert_ne!(a.to_hex(), b.to_hex());
        assert_eq!(a.source_hash, b.source_hash, "same source bytes");
    }
}
