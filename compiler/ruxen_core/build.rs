//! Two jobs:
//!
//! 1. Bakes the host target triple into the crate so
//!    `codegen::find_prebuilt_runtime_archive` can build the per-triple
//!    discovery candidate (`<install>/lib/ruxen/<triple>/libruxenrt.a`) via
//!    `option_env!("RUXEN_HOST_TARGET")`. `cargo:rustc-env` only affects the
//!    emitting crate's own compilation, so this MUST live in `ruxen_core`
//!    (where the `option_env!` is read), not a downstream crate.
//!
//! 2. Generates the stdlib bootstrap tables from [`BOOTSTRAP_PACKAGES`] and
//!    the `library/std/` tree: `BOOTSTRAP_FILES` (`resolve/bootstrap.rs`),
//!    and `BOOTSTRAP_EMBEDDED` plus `BOOTSTRAP_EMBEDDED_SIBLINGS`
//!    (`resolve/stdlib_embedded.rs`). The ordered package list is the only
//!    hand-maintained input; every `.rx` file under `<pkg>/src/` is embedded
//!    automatically, and the build fails when the list and the tree disagree.
//!
//! Rerun triggers: this file and each package's `src/` directory (so adding
//! or removing a sibling `.rx` regenerates the tables). Edits to an embedded
//! file already rebuild the crate through `include_str!`. Watching the whole
//! `library/std/` tree would rerun this script on every runtime `.c` edit and
//! rebuild `ruxen_core` for nothing.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Stdlib packages in bootstrap load order (`library/std/<pkg>/src/lib.rx`).
///
/// Order matters: the bootstrap merge runs Pass-1 forward-decl AND full
/// lib-type resolution in the same loop, so a package that references a type
/// owned by another package must come after it. The hand-maintained order is
/// deliberate (docs/specs/system/zero_rust_stdlib_classes.spec.md, "Out of
/// scope"); the `Ruxen.toml` dependency lists are not complete enough to
/// derive it.
const BOOTSTRAP_PACKAGES: &[&str] = &[
    // std-core ships the builtin mixins (Send / Sync / Clone / Copy /
    // Displayable / Comparable / PartialEq / Eq / Hash / Default / Ord /
    // PartialOrd / Drop / Into[T] / Iterable / Error). Every other package
    // plus user code references them, so core MUST load first.
    "core",
    // io ships IoError + IoErrorKind which rand/env/fs/net reference.
    "io",
    "rand",
    "path",
    "env",
    "hash",
    "fmt",
    "net",
    // bufio depends on BOTH io (File / IoError) and net (TcpStream).
    "bufio",
    "process",
    "time",
    "fs",
    "sync",
    // future depends on core only and ships the `Future` mixin, `Poll[T]`,
    // `Context` and `Waker`.
    "future",
    // async_fs / async_net / async_io `include Future`, so they load after
    // future (fs/net/io load before it and cannot).
    "async_fs",
    "async_net",
    "async_io",
    // executor: `block_on` takes a Future-implementing value.
    "executor",
    "string",
    "option_result",
    "array",
    "map",
    "set",
    // scalar: method-home `class Int` / `class Float`; needs `string` for the
    // `-> String` return type of `to_s` / `to_string`.
    "scalar",
    // json: needs array/map/string for `Array[Json]`, `Map[String, Json]`.
    "json",
    // http: blocking HTTP/1.1 client over net + string/array/option.
    "http",
    "base64",
    // regex: returns `Array[Match]`, `HashMap[String, String]`,
    // `Option[String]`, `Result[Regex, RegexError]`.
    "regex",
    // bench: needs time (Instant.now / .elapsed.as_nanos).
    "bench",
    // test: Tester DSL + Matcher + Runner; needs string/array/option_result/
    // fmt/sync.
    "test",
];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target = std::env::var("TARGET").unwrap_or_default();
    println!("cargo:rustc-env=RUXEN_HOST_TARGET={target}");

    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let std_root = manifest_dir.join("../../library/std");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    generate_bootstrap_tables(&std_root, &out_dir);
}

fn generate_bootstrap_tables(std_root: &Path, out_dir: &Path) {
    check_packages_match_tree(std_root);

    let mut files = String::from("&[\n");
    let mut embedded = String::from("&[\n");
    let mut siblings = String::from("&[\n");
    for pkg in BOOTSTRAP_PACKAGES {
        let rel = format!("{pkg}/src/lib.rx");
        let src_dir = std_root.join(pkg).join("src");
        println!("cargo:rerun-if-changed={}", src_dir.display());

        writeln!(files, "    {rel:?},").unwrap();
        writeln!(
            embedded,
            "    ({rel:?}, include_str!({:?})),",
            path_str(&src_dir.join("lib.rx"))
        )
        .unwrap();

        let names = sibling_names(&src_dir);
        if names.is_empty() {
            continue;
        }
        writeln!(siblings, "    ({rel:?}, &[").unwrap();
        for name in names {
            writeln!(
                siblings,
                "        ({name:?}, include_str!({:?})),",
                path_str(&src_dir.join(&name))
            )
            .unwrap();
        }
        siblings.push_str("    ]),\n");
    }
    files.push(']');
    embedded.push(']');
    siblings.push(']');

    write_if_changed(&out_dir.join("bootstrap_files.rs"), &files);
    write_if_changed(&out_dir.join("bootstrap_embedded.rs"), &embedded);
    write_if_changed(&out_dir.join("bootstrap_embedded_siblings.rs"), &siblings);
}

/// Fail the build when [`BOOTSTRAP_PACKAGES`] and the packages on disk
/// (every `library/std/<pkg>/src/lib.rx`) disagree.
fn check_packages_match_tree(std_root: &Path) {
    let entries =
        std::fs::read_dir(std_root).unwrap_or_else(|e| panic!("read {}: {e}", std_root.display()));
    let mut on_disk: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().join("src/lib.rx").is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    on_disk.sort();
    let mut listed: Vec<String> = BOOTSTRAP_PACKAGES.iter().map(|p| p.to_string()).collect();
    listed.sort();
    assert_eq!(
        listed, on_disk,
        "compiler/ruxen_core/build.rs BOOTSTRAP_PACKAGES must name every \
         library/std/<pkg>/src/lib.rx exactly once"
    );
}

/// Every `.rx` file in a package's `src/` other than `lib.rx`, sorted.
fn sibling_names(src_dir: &Path) -> Vec<String> {
    let entries =
        std::fs::read_dir(src_dir).unwrap_or_else(|e| panic!("read {}: {e}", src_dir.display()));
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.ends_with(".rx") && n != "lib.rx")
        .collect();
    names.sort();
    names
}

fn path_str(path: &Path) -> String {
    path.to_str()
        .unwrap_or_else(|| panic!("non-UTF-8 stdlib path {}", path.display()))
        .to_string()
}

/// Skip the write when the content is unchanged, so a rerun that produces the
/// same tables leaves the file's mtime alone.
fn write_if_changed(path: &Path, content: &str) {
    if std::fs::read_to_string(path).ok().as_deref() == Some(content) {
        return;
    }
    std::fs::write(path, content).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}
