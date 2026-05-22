//! Compile-pipeline benchmark: codegen (MIR → linked binary).
//!
//! Run with: `cargo bench -p rivenc --bench codegen_bench`.
//!
//! Lex / parse / typeck / MIR-lower happen once outside the iter
//! loop so the bench measures pure `codegen::compile`: Cranelift
//! IR generation, object file emission, and linker invocation
//! end-to-end. Output is written to a tempfile that is overwritten
//! each iteration — the filesystem write is part of what we measure
//! because it tracks the real driver cost.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use riven_core::codegen;
use riven_core::lexer::Lexer;
use riven_core::mir::lower::Lowerer;
use riven_core::parser::Parser;
use riven_core::typeck::type_check;

fn bench_codegen(c: &mut Criterion) {
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
    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer.lower_program(&result.program).expect("lower");

    let out_path = std::env::temp_dir().join("rivenc-bench-codegen.bin");
    let out_str = out_path.to_string_lossy().into_owned();

    c.bench_function("codegen/508_command_status", |b| {
        b.iter(|| codegen::compile(black_box(&mir), black_box(&out_str)).expect("codegen"))
    });
}

criterion_group!(benches, bench_codegen);
criterion_main!(benches);
