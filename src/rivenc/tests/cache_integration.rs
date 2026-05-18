//! Integration tests for the incremental cache.
//!
//! These exercise the driver API end-to-end with a mock `CompileFn`, covering
//! multi-file dependency scenarios that are hard to unit-test in isolation.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use rivenc::cache::{
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
        parallel: false,
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

/// Build a deterministic project of N files arranged as a chain:
/// f0 -> f1 -> ... -> fN-1. The last file has no dependencies.
fn chain_project(n: usize) -> (Vec<SourceFile>, HashMap<String, Vec<String>>) {
    let mut files = Vec::with_capacity(n);
    let mut deps: HashMap<String, Vec<String>> = HashMap::new();
    for i in 0..n {
        let path = format!("f{}.rvn", i);
        files.push(src(&path, &format!("fn f{}() {{}}", i)));
        if i + 1 < n {
            deps.insert(path.clone(), vec![format!("f{}.rvn", i + 1)]);
        }
    }
    (files, deps)
}

#[test]
fn ten_file_chain_cold_then_warm() {
    let (_td, store) = tmp_store();
    let (files, deps_map) = chain_project(10);

    let calls = AtomicUsize::new(0);
    let compile = |f: &SourceFile| {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(CompileOutput {
            object_bytes: format!("OBJ<{}>", f.source).into_bytes(),
            signature: sig_fn(&f.path, "Int"),
            dependencies: deps_map.get(&f.path).cloned().unwrap_or_default(),
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
fn leaf_body_edit_recompiles_only_leaf() {
    let (_td, store) = tmp_store();
    let (files, deps_map) = chain_project(5);

    let calls = Mutex::new(Vec::<String>::new());
    // Signature is stable per-path; body change in leaf doesn't change its
    // signature, and the object bytes are identical (we key bytes on path
    // only). So the cascade must stop at the leaf.
    let compile = |f: &SourceFile| {
        calls.lock().unwrap().push(f.path.clone());
        Ok(CompileOutput {
            object_bytes: format!("OBJ<{}>", f.path).into_bytes(), // bytes depend only on path
            signature: sig_fn(&f.path, "Int"),
            dependencies: deps_map.get(&f.path).cloned().unwrap_or_default(),
        })
    };

    build(files.clone(), &store, &opts(), &compile).unwrap();
    calls.lock().unwrap().clear();

    // Edit the leaf's body (source changes, but signature+bytes don't).
    let mut modified = files.clone();
    modified
        .iter_mut()
        .find(|f| f.path == "f4.rvn")
        .unwrap()
        .source = "# body-only edit".into();
    build(modified, &store, &opts(), &compile).unwrap();

    let recorded = calls.lock().unwrap().clone();
    assert!(recorded.contains(&"f4.rvn".to_string()));
    for other in &["f0.rvn", "f1.rvn", "f2.rvn", "f3.rvn"] {
        assert!(
            !recorded.contains(&(*other).to_string()),
            "{} should NOT be recompiled on a leaf body edit, but it was",
            other
        );
    }
}

#[test]
fn signature_edit_on_leaf_cascades_through_chain() {
    let (_td, store) = tmp_store();
    let (files, deps_map) = chain_project(5);

    let version = Mutex::new(1u32);
    let calls = Mutex::new(Vec::<String>::new());
    let compile = |f: &SourceFile| {
        calls.lock().unwrap().push(f.path.clone());
        let v = *version.lock().unwrap();
        // Leaf f4's signature depends on the global version; every other
        // file has a stable signature. Object bytes depend on both path AND
        // version so output_changed=true propagates.
        let sig = if f.path == "f4.rvn" {
            sig_fn(&f.path, if v == 1 { "Int" } else { "String" })
        } else {
            sig_fn(&f.path, "Int")
        };
        Ok(CompileOutput {
            object_bytes: format!("OBJ<{}-v{}>", f.path, v).into_bytes(),
            signature: sig,
            dependencies: deps_map.get(&f.path).cloned().unwrap_or_default(),
        })
    };

    build(files.clone(), &store, &opts(), &compile).unwrap();
    calls.lock().unwrap().clear();

    *version.lock().unwrap() = 2;
    // Only change f4's source so only f4 is naturally dirty.
    let mut modified = files.clone();
    modified
        .iter_mut()
        .find(|f| f.path == "f4.rvn")
        .unwrap()
        .source = "# v2".into();
    build(modified, &store, &opts(), &compile).unwrap();

    let recorded = calls.lock().unwrap().clone();
    // f4 source changed -> f4 recompiles (signature flipped Int -> String).
    // f3 is the direct dependent of f4 -> gets invalidated and recompiles.
    // f3's OWN signature is stable, so the cascade STOPS at f3 — f2/f1/f0
    // keep their cached artifacts. This is the whole point of dep-aware
    // invalidation: signature-stable intermediaries block transitive cascades.
    assert!(recorded.contains(&"f4.rvn".to_string()));
    assert!(recorded.contains(&"f3.rvn".to_string()));
    for blocked in &["f2.rvn", "f1.rvn", "f0.rvn"] {
        assert!(
            !recorded.contains(&(*blocked).to_string()),
            "{} should NOT cascade through — f3 has a stable signature and blocks further invalidation",
            blocked
        );
    }
}

#[test]
fn force_recompiles_the_whole_project() {
    let (_td, store) = tmp_store();
    let (files, deps_map) = chain_project(5);
    let calls = AtomicUsize::new(0);
    let compile = |f: &SourceFile| {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(CompileOutput {
            object_bytes: format!("OBJ<{}>", f.path).into_bytes(),
            signature: sig_fn(&f.path, "Int"),
            dependencies: deps_map.get(&f.path).cloned().unwrap_or_default(),
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
fn parallel_and_sequential_produce_identical_objects() {
    let (_td1, store_seq) = tmp_store();
    let (_td2, store_par) = tmp_store();
    // Wide, shallow project: one root depending on many independent leaves.
    let mut files = Vec::new();
    let mut deps: HashMap<String, Vec<String>> = HashMap::new();
    let leaves: Vec<String> = (0..8).map(|i| format!("leaf{}.rvn", i)).collect();
    for l in &leaves {
        files.push(src(l, l));
    }
    files.push(src("root.rvn", "root"));
    deps.insert("root.rvn".into(), leaves.clone());

    let compile = |f: &SourceFile| {
        Ok(CompileOutput {
            object_bytes: format!("OBJ<{}>", f.path).into_bytes(),
            signature: sig_fn(&f.path, "Int"),
            dependencies: deps.get(&f.path).cloned().unwrap_or_default(),
        })
    };

    let seq = build(files.clone(), &store_seq, &opts(), &compile).unwrap();
    let mut par_opts = opts();
    par_opts.parallel = true;
    let par = build(files, &store_par, &par_opts, &compile).unwrap();

    let read = |items: &[(String, std::path::PathBuf)]| -> Vec<(String, Vec<u8>)> {
        let mut out: Vec<(String, Vec<u8>)> = items
            .iter()
            .map(|(p, path)| (p.clone(), std::fs::read(path).unwrap()))
            .collect();
        out.sort();
        out
    };
    assert_eq!(read(&seq.objects), read(&par.objects));
}

#[test]
fn corrupt_object_causes_recompile_without_panic() {
    let (_td, store) = tmp_store();
    let (files, deps_map) = chain_project(3);
    let calls = AtomicUsize::new(0);
    let compile = |f: &SourceFile| {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(CompileOutput {
            object_bytes: format!("OBJ<{}>", f.path).into_bytes(),
            signature: sig_fn(&f.path, "Int"),
            dependencies: deps_map.get(&f.path).cloned().unwrap_or_default(),
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
