//! Shared C-runtime link helper for integration tests that build native
//! binaries outside `codegen::compile`.
//!
//! Sources, compile flags and system libraries come from the same driver
//! entry points the real build uses (`codegen::find_runtime_sources`,
//! `codegen::object::compile_runtime_sources`,
//! `codegen::collect_system_lib_flags`), so a new runtime package, a
//! vendored C tree (PCRE2) or a `[system_libs]` entry (`-lm`) reaches
//! these tests without a per-test source list.
//!
//! Each test binary uses only part of this module.
#![allow(dead_code)]

use ruxen_core::codegen;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

/// Directory for build outputs: Cargo's per-target tmp dir under
/// `target/`, never the system tmpfs.
fn out_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("runtime-link-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create test tmp dir");
    dir
}

fn unique_suffix() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!("{}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// Runtime sources the driver links, limited to the named stdlib
/// packages (including their vendored subdirectories). Reads
/// `RUXEN_RUNTIME` like the driver does, so callers that set it must
/// call this before setting it.
pub fn package_runtime_sources(packages: &[&str]) -> Vec<PathBuf> {
    codegen::find_runtime_sources()
        .expect("find runtime sources")
        .into_iter()
        .filter(|src| {
            packages.iter().any(|pkg| {
                let runtime_dir = Path::new(pkg).join("runtime");
                src.ancestors().any(|a| a.ends_with(&runtime_dir))
            })
        })
        .collect()
}

/// Compile `sources` with the driver's flags and move the objects into
/// the test tmp dir.
pub fn compile_runtime_objects(sources: &[PathBuf]) -> Vec<PathBuf> {
    let dir = out_dir();
    codegen::object::compile_runtime_sources(sources, false)
        .expect("compile runtime sources")
        .into_iter()
        .map(|obj| {
            let dest = dir.join(obj.file_name().expect("object file name"));
            if std::fs::rename(&obj, &dest).is_err() {
                std::fs::copy(&obj, &dest).expect("copy runtime object");
                let _ = std::fs::remove_file(&obj);
            }
            dest
        })
        .collect()
}

/// The whole stdlib runtime, compiled once per test process.
fn all_runtime_objects() -> &'static [PathBuf] {
    static OBJECTS: OnceLock<Vec<PathBuf>> = OnceLock::new();
    OBJECTS.get_or_init(|| {
        let sources = codegen::find_runtime_sources().expect("find runtime sources");
        compile_runtime_objects(&sources)
    })
}

/// Compile a pure-C `source` harness and link it against the full stdlib
/// runtime plus every package's `[system_libs]`. Returns the executable.
pub fn link_c_harness(name: &str, source: &str) -> PathBuf {
    let dir = out_dir();
    let suffix = unique_suffix();
    let harness_c = dir.join(format!("{name}_{suffix}.c"));
    let harness_exe = dir.join(format!("{name}_{suffix}"));

    // `future/runtime/scheduler.c` references `Future_dynamic_poll` and
    // `Drop_dynamic_drop`, which Ruxen codegen synthesises for programs.
    // A pure-C harness never goes through codegen, so weak stubs stand in.
    let stub_prelude = "\
__attribute__((weak)) long long Future_dynamic_poll(long long self, long long ctx) { \
    (void)self; (void)ctx; return 0; \
}\n\
__attribute__((weak)) void Drop_dynamic_drop(long long self) { \
    (void)self; \
}\n";
    std::fs::write(&harness_c, format!("{stub_prelude}{source}")).expect("write harness");

    let mut cmd = Command::new("cc");
    cmd.arg(&harness_c);
    cmd.args(all_runtime_objects());
    cmd.arg("-o").arg(&harness_exe);
    cmd.args(codegen::collect_system_lib_flags().expect("collect system libs"));
    // The runtime's CSPRNG uses `SecRandomCopyBytes` on macOS; the driver
    // adds this framework in `object::linker_args`.
    #[cfg(target_os = "macos")]
    cmd.arg("-framework").arg("Security");
    let status = cmd.status().expect("invoke cc for harness");

    let _ = std::fs::remove_file(&harness_c);
    assert!(status.success(), "failed to link C harness {name}");
    harness_exe
}
