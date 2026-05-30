//! Microbenchmarks for cache primitives.
//!
//! Run with: `cargo bench -p ruxenc`
//!
//! The targets chosen here correspond to the hot paths the driver walks on
//! every build: file hashing, manifest (de)serialization, and signature
//! comparison. If any of these regress significantly, the no-change-rebuild
//! budget (<100ms) becomes harder to hit.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use ruxenc::cache::{
    hash_file, CacheKey, CacheManifest, CachedFile, FileSignature, ManifestLoadResult, PublicItem,
    SigField, SigFn,
};

fn bench_hash_file(c: &mut Criterion) {
    let small = "def main\n  puts \"hi\"\nend\n".to_string();
    let medium = small.repeat(100);
    let large = small.repeat(2000); // ~50kB, representative of a real file

    c.bench_function("hash_file/small", |b| {
        b.iter(|| hash_file(black_box(&small)))
    });
    c.bench_function("hash_file/medium", |b| {
        b.iter(|| hash_file(black_box(&medium)))
    });
    c.bench_function("hash_file/large", |b| {
        b.iter(|| hash_file(black_box(&large)))
    });
}

fn bench_manifest_roundtrip(c: &mut Criterion) {
    let mut manifest = CacheManifest::empty("x86_64-linux", "debug");
    for i in 0..100 {
        manifest.files.push(CachedFile {
            path: format!("src/file{:04}.rx", i),
            source_hash: [i as u8; 32],
            cache_key: [(i as u8).wrapping_add(1); 32],
            signature_file: Some(format!("{:x}.sig", i)),
            object_file: Some(format!("{:x}.o", i)),
            last_compiled: i as u64,
        });
    }
    let bytes = manifest.to_bytes().unwrap();

    c.bench_function("manifest/serialize_100", |b| {
        b.iter(|| manifest.to_bytes().unwrap())
    });
    c.bench_function("manifest/deserialize_100", |b| {
        b.iter(|| {
            let res = CacheManifest::from_bytes(black_box(&bytes), "x86_64-linux", "debug");
            matches!(res, ManifestLoadResult::Loaded(_))
        })
    });
}

fn bench_signature_comparison(c: &mut Criterion) {
    fn mk(n: usize) -> FileSignature {
        FileSignature {
            items: (0..n)
                .map(|i| {
                    PublicItem::Function(SigFn {
                        name: format!("fn_{}", i),
                        generic_params: vec![],
                        self_mode: None,
                        is_class_method: false,
                        params: (0..3)
                            .map(|j| ruxenc::cache::SigParam {
                                name: format!("p{}", j),
                                ty: "Int".into(),
                            })
                            .collect(),
                        return_ty: "Int".into(),
                    })
                })
                .collect(),
        }
    }
    let a = mk(50);
    let b = mk(50);
    c.bench_function("signature_eq/50_unchanged", |bb| {
        bb.iter(|| ruxenc::cache::interface_changed(black_box(&a), black_box(&b)))
    });

    let c_ref = mk(50);
    let mut d = mk(50);
    if let PublicItem::Function(f) = &mut d.items[0] {
        f.return_ty = "String".into();
    }
    c.bench_function("signature_eq/50_one_diff", |bb| {
        bb.iter(|| ruxenc::cache::interface_changed(black_box(&c_ref), black_box(&d)))
    });
}

fn bench_cache_key(c: &mut Criterion) {
    let sh = hash_file("fn main() {}");
    c.bench_function("cache_key/to_hex", |b| {
        b.iter(|| CacheKey::new(black_box(sh), 1, "x86_64-linux", "debug").to_hex())
    });
}

fn bench_signature_roundtrip(c: &mut Criterion) {
    let sig = FileSignature {
        items: (0..20)
            .map(|i| PublicItem::Struct {
                name: format!("S{}", i),
                generic_params: vec![],
                fields: (0..5)
                    .map(|j| SigField {
                        name: format!("f{}", j),
                        ty: "Int".into(),
                        public: true,
                    })
                    .collect(),
            })
            .collect(),
    };
    let bytes = sig.to_bytes().unwrap();
    c.bench_function("signature/deserialize_20", |b| {
        b.iter(|| FileSignature::from_bytes(black_box(&bytes)).unwrap())
    });
    c.bench_function("signature/serialize_20", |b| {
        b.iter(|| sig.to_bytes().unwrap())
    });
}

criterion_group!(
    benches,
    bench_hash_file,
    bench_manifest_roundtrip,
    bench_signature_comparison,
    bench_cache_key,
    bench_signature_roundtrip,
);
criterion_main!(benches);
