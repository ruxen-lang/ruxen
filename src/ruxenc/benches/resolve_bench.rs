//! Compile-pipeline benchmark: resolver (name + scope resolution).
//!
//! Run with: `cargo bench -p ruxenc --bench resolve_bench`.
//!
//! Bootstrap programs (~25 stdlib packages) are loaded once and
//! cloned per iteration so we measure pure resolve cost, not
//! filesystem I/O. The user program is the 508_command_status
//! fixture — medium-sized, exercises module + use + Command +
//! Result destructuring.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use ruxen_core::resolve::bootstrap::run_bootstrap_with_package_names;
use ruxen_core::resolve::Resolver;

fn bench_resolve(c: &mut Criterion) {
    let mut bootstrap_diags: Vec<_> = Vec::new();
    let bootstrap = run_bootstrap_with_package_names(&mut bootstrap_diags);
    assert!(
        bootstrap_diags.is_empty(),
        "bootstrap had diagnostics: {:?}",
        bootstrap_diags
    );

    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/release-e2e/cases/508_command_status.rx"),
    )
    .expect("read 508 fixture");
    let mut lx = Lexer::new(&source);
    let tokens = lx.tokenize().expect("tokenize");
    let mut p = Parser::new(tokens);
    let program = p.parse().expect("parse");

    c.bench_function("resolve/508_command_status", |b| {
        b.iter(|| {
            let resolver = Resolver::new();
            let _result = resolver
                .resolve_with_bootstrap_packages(black_box(&program), black_box(&bootstrap));
        })
    });
}

criterion_group!(benches, bench_resolve);
criterion_main!(benches);
