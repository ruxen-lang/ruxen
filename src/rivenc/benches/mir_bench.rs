//! Compile-pipeline benchmark: MIR lowering (HIR → MIR).
//!
//! Run with: `cargo bench -p rivenc --bench mir_bench`.
//!
//! Typeck is performed once outside the iter loop so the bench
//! measures pure `Lowerer::lower_program` cost. Each iteration
//! constructs a fresh Lowerer + lowers the program, which mirrors
//! exactly what `codegen::compile` does for one TU.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use riven_core::lexer::Lexer;
use riven_core::mir::lower::Lowerer;
use riven_core::parser::Parser;
use riven_core::typeck::type_check;

fn bench_mir(c: &mut Criterion) {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/release-e2e/cases/508_command_status.rvn"),
    )
    .expect("read 508 fixture");
    let mut lx = Lexer::new(&source);
    let tokens = lx.tokenize().expect("tokenize");
    let mut p = Parser::new(tokens);
    let program = p.parse().expect("parse");
    let result = type_check(&program);

    c.bench_function("mir_lower/508_command_status", |b| {
        b.iter(|| {
            let mut lowerer = Lowerer::new(black_box(&result.symbols));
            lowerer.lower_program(black_box(&result.program)).expect("lower")
        })
    });
}

criterion_group!(benches, bench_mir);
criterion_main!(benches);
