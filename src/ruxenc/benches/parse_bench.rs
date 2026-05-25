//! Compile-pipeline benchmark: lexer + parser.
//!
//! Run with: `cargo bench -p ruxenc --bench parse_bench`.
//!
//! Three corpus shapes, named for what they actually contain (NOT
//! "small/medium/large" — that ordering doesn't hold once a real
//! fixture enters the mix):
//!
//!   * `synth_1def` — `def main; puts "hi"; end` (25 chars, smallest
//!     meaningful program — baseline for parser cold-start cost)
//!   * `fixture_508` — `tests/release-e2e/cases/508_command_status.rx`
//!     (45 lines, ~3 defs, exercises Command builder + match arms +
//!     string interpolation — representative of typical user code)
//!   * `fixture_727_async` — `tests/release-e2e/cases/727_async_tcp_echo.rx`
//!     (133 lines, `async def`, futures, mixins — largest real
//!     fixture, exercises the parser's most-complex paths)
//!
//! Measures combined `Lexer::tokenize` + `Parser::parse` time so a
//! regression in either stage shows up. Splitting them per-bench is
//! straightforward if needed.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;

fn read_fixture(name: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/release-e2e/cases")
            .join(name),
    )
    .unwrap_or_else(|e| panic!("read {}: {}", name, e))
}

fn bench_parse(c: &mut Criterion) {
    let synth_1def = "def main\n  puts \"hi\"\nend\n".to_string();
    let fixture_508 = read_fixture("508_command_status.rx");
    let fixture_727 = read_fixture("727_async_tcp_echo.rx");

    for (name, src) in [
        ("synth_1def", &synth_1def),
        ("fixture_508", &fixture_508),
        ("fixture_727_async", &fixture_727),
    ] {
        c.bench_function(&format!("parse/{}", name), |b| {
            b.iter(|| {
                let mut lx = Lexer::new(black_box(src));
                let toks = lx.tokenize().expect("tokenize");
                let mut p = Parser::new(toks);
                p.parse().expect("parse")
            })
        });
    }
}

criterion_group!(benches, bench_parse);
criterion_main!(benches);
