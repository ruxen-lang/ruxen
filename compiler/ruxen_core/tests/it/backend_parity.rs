//! Backend parity: every fixture in `tests/common/parity_cases.rs` is built
//! through the front end, then through Cranelift and through LLVM; both
//! executables run, and their stdout and exit code must match. The REPL JIT
//! lane over the same list lives in `ruxen_repl` (`tests/it/jit_parity.rs`).

use ruxen_core::codegen::Backend;

use crate::common::{compile_with_backend, run_binary};

/// Build and run `fixture` (a path under `tests/fixtures`) with Cranelift
/// and with LLVM at -O0, and assert identical stdout and exit code.
fn assert_backends_identical(fixture: &str) {
    let path = format!("{}/tests/fixtures/{fixture}", env!("CARGO_MANIFEST_DIR"));
    let source = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));

    let (cl_out, _, cl_exit) =
        run_binary(&compile_with_backend(&source, "parity", Backend::Cranelift));
    let (llvm_out, _, llvm_exit) = run_binary(&compile_with_backend(
        &source,
        "parity",
        Backend::Llvm { opt_level: 0 },
    ));

    assert_eq!(
        (&cl_out, cl_exit),
        (&llvm_out, llvm_exit),
        "{fixture}: Cranelift and LLVM differ\nCranelift (exit {cl_exit:?}):\n{cl_out}\nLLVM (exit {llvm_exit:?}):\n{llvm_out}"
    );
}

macro_rules! parity_cases {
    ($($name:ident => $fixture:literal,)*) => {
        $(
            #[test]
            fn $name() {
                assert_backends_identical($fixture);
            }
        )*
    };
}

include!("../common/parity_cases.rs");
