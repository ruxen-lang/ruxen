//! REPL JIT parity: every fixture in ruxen_core's
//! `tests/common/parity_cases.rs` is JIT-compiled as a whole program and
//! its `main` run, and its stdout and exit code must match the Cranelift
//! executable built from the same source. (ruxen_core's `backend_parity`
//! compares Cranelift with LLVM over the same list.)
//!
//! The JIT half runs in a child process: this test binary re-executes
//! itself, filtered to the one test, with `JIT_CHILD_ENV` naming the
//! fixture, so a program that exits, panics or crashes ends the child, not
//! the test run. It lives here because only `ruxen_repl`'s build script
//! links the C runtime (`libruxenrt.a`) into a test binary the JIT can
//! resolve symbols from.

use std::ffi::{c_char, CString};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use ruxen_repl::jit::JITCodeGen;

use crate::common::{compile_program, lower, run_binary};

/// Set on the child process to the fixture path it JIT-runs.
const JIT_CHILD_ENV: &str = "RUXEN_JIT_PARITY_FIXTURE";
/// Printed by the child right before `main` runs; the parent keeps only
/// the stdout after it, dropping the test harness's own lines.
const OUTPUT_MARKER: &str = "\u{0}jit-parity-output\u{0}\n";

fn fixture_path(fixture: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../compiler/ruxen_core/tests/fixtures")
        .join(fixture)
}

fn read_fixture(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Child side: JIT-compile the program, run its `main`, and exit with
/// `main`'s status. Never returns.
fn run_jit_child(path: &Path) -> ! {
    let mir = lower(&read_fixture(path));
    let mut jit = JITCodeGen::new().expect("create JIT");
    jit.compile_program(&mir).expect("JIT-compile program");
    let main_ptr = jit.function_ptr("main").expect("JIT main");
    let program_name = CString::new("jit_parity").expect("program name");
    let argv: [*const c_char; 2] = [program_name.as_ptr(), std::ptr::null()];

    print!("{OUTPUT_MARKER}");
    std::io::stdout().flush().expect("flush marker");
    // SAFETY: `main_ptr` is the finalized JIT entry of the program's
    // `main`, whose signature is the C `main(argc: i32, argv) -> i32`
    // (`abi::FnAbi` gives the `is_entry` function the C entry ABI). `jit` owns the code and
    // outlives the call; `argv` is a NULL-terminated array of one live
    // C string.
    let main: extern "C" fn(i32, *const *const c_char) -> i32 =
        unsafe { std::mem::transmute(main_ptr) };
    let status = main(1, argv.as_ptr());
    // `process::exit` runs libc `exit`, which flushes the runtime's stdio.
    std::process::exit(status)
}

/// Parent side: run this test again in a child that JIT-runs `fixture`,
/// and return its program stdout and exit code.
fn run_jit(test_name: &str, path: &Path) -> (String, Option<i32>) {
    let output = Command::new(std::env::current_exe().expect("test binary path"))
        .args([test_name, "--exact", "--nocapture", "--test-threads=1"])
        .env(JIT_CHILD_ENV, path)
        .stdin(Stdio::null())
        .output()
        .expect("spawn JIT child");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let program_out = stdout
        .split_once(OUTPUT_MARKER)
        .map(|(_, out)| out)
        .unwrap_or_else(|| {
            panic!(
                "JIT child never reached main\nstdout:\n{stdout}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
    (program_out.to_string(), output.status.code())
}

/// The test body for one fixture: in the child, JIT-run it; in the parent,
/// compare the child's run with the Cranelift executable's.
fn assert_jit_matches_cranelift(test_name: &str, fixture: &str) {
    if let Some(path) = std::env::var_os(JIT_CHILD_ENV) {
        run_jit_child(Path::new(&path));
    }
    let path = fixture_path(fixture);
    let (cl_out, _, cl_exit) = run_binary(&compile_program(&read_fixture(&path), "jit_parity"));
    let (jit_out, jit_exit) = run_jit(test_name, &path);
    assert_eq!(
        (&cl_out, cl_exit),
        (&jit_out, jit_exit),
        "{fixture}: Cranelift and the REPL JIT differ\nCranelift (exit {cl_exit:?}):\n{cl_out}\nJIT (exit {jit_exit:?}):\n{jit_out}"
    );
}

macro_rules! parity_cases {
    ($($name:ident => $fixture:literal,)*) => {
        $(
            #[test]
            fn $name() {
                assert_jit_matches_cranelift(concat!("jit_parity::", stringify!($name)), $fixture);
            }
        )*
    };
}

include!("../../../../compiler/ruxen_core/tests/common/parity_cases.rs");
