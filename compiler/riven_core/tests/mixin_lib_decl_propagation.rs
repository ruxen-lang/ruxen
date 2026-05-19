//! Pre-flight spike for #06.95 (stdlib packagization).
//!
//! Verifies that lib-decl methods declared on a `mixin` are reachable
//! through `include` from the including class. This is the load-bearing
//! propagation the BufReader / BufWriter module+mixin reshape depends
//! on:
//!
//! ```riven
//! module BufReader
//!   mixin Reader
//!     lib "riven_runtime"
//!       def read_line as "riven_bufreader_read_line"(self) -> ...
//!     end
//!   end
//!   class File
//!     include Reader
//!     ...
//!   end
//! end
//! ```
//!
//! If `Thing.add_one` (declared via `mixin Adder; lib ...; include Adder`
//! on `Thing`) does not resolve, the reshape can't ship as planned —
//! and we need a one-task fix in the resolver to walk
//! `class.includes` for lib-decl methods.

use riven_core::codegen;
use riven_core::diagnostics::{Diagnostic, DiagnosticLevel};
use riven_core::lexer::Lexer;
use riven_core::mir::lower::Lowerer;
use riven_core::parser::ast::Program;
use riven_core::parser::Parser;
use riven_core::typeck;
use std::path::PathBuf;
use std::process::Command;

fn workspace_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

fn parse_fixture(name: &str) -> Program {
    let path = format!(
        "{}/compiler/riven_core/tests/fixtures/riven/{}.rvn",
        workspace_root().display(),
        name
    );
    let source =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path, e));
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    parser.parse().expect("parse")
}

#[test]
fn mixin_lib_decl_propagates_through_include() {
    let program = parse_fixture("mixin_lib_decl_propagates");
    let type_result = typeck::type_check(&program);
    let errors: Vec<&Diagnostic> = type_result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "typecheck errors on `Thing.add_one` (mixin lib decl via include): {:?}",
        errors
    );

    let mut lowerer = Lowerer::new(&type_result.symbols);
    let mir = lowerer
        .lower_program(&type_result.program)
        .expect("MIR lowering");

    let bin_path = workspace_root().join("tmp/mixin_lib_decl_propagates.bin");
    let _ = std::fs::create_dir_all(bin_path.parent().unwrap());
    codegen::compile(&mir, bin_path.to_str().unwrap()).expect("codegen");

    let output = Command::new(&bin_path)
        .output()
        .expect("run mixin-lib-decl propagation binary");
    assert!(
        output.status.success(),
        "binary should exit 0 (Thing.add_one(41) == 42); status={:?} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}
