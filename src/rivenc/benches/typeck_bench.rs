//! Compile-pipeline benchmark: full type-check (resolve + bootstrap
//! + typeck pre-passes + MixinResolver collect).
//!
//! Run with: `cargo bench -p rivenc --bench typeck_bench`.
//!
//! `typeck::type_check` is the public entry every test + driver
//! flows through, so this bench is what catches the "I added a
//! generic-bound check and now whole-program typeck is 30% slower"
//! class of regression.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use riven_core::lexer::Lexer;
use riven_core::parser::Parser;
use riven_core::typeck::type_check;

fn bench_typeck(c: &mut Criterion) {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/release-e2e/cases/508_command_status.rvn"),
    )
    .expect("read 508 fixture");
    let mut lx = Lexer::new(&source);
    let tokens = lx.tokenize().expect("tokenize");
    let mut p = Parser::new(tokens);
    let program = p.parse().expect("parse");

    c.bench_function("typeck/508_command_status", |b| {
        b.iter(|| {
            let _result = type_check(black_box(&program));
        })
    });
}

criterion_group!(benches, bench_typeck);
criterion_main!(benches);
