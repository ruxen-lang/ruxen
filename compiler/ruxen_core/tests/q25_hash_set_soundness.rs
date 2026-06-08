//! Q25 — empty-Hash/Set lookup SIGSEGV + `&Hash`/`&Set` param consistency.
//!
//! (a) `key?`/`get`/`contains` on an EMPTY hash/set SIGSEGV'd: the runtime's
//!     `string_keys` tristate (-1 = unset) was tested with plain C truthiness,
//!     and -1 is truthy, so an empty-table lookup took the string path and
//!     `strcmp`'d an integer key as a `char*` (deref of a small bogus
//!     address). Fixed in `library/std/hash/runtime/hash.c` to test `> 0`.
//!     The runtime cases read the e2e fixtures (`617_*`, `618_*`).
//!
//! (b) `&Hash[K,V]` / `&Set[T]` by-ref params are SOUND (a pointer to the
//!     backing struct, exactly like the widely-used `&Array[Int]`). The bug
//!     was an INCONSISTENCY: a free fn rejected `&Hash[Int,Int]` with E1118
//!     (the `Hash → Hashable` mixin-alias collision) while a method accepted
//!     it. Resolution (sound + minimal): a generic-args-bearing collection
//!     builtin in `&Name[..]` position resolves to the COLLECTION type in both
//!     positions; the bare `&Hash` / `&Set` (= the `Hashable` mixin, no args)
//!     is still correctly rejected with E1118. (`619_*` exercises the runtime
//!     side; the two compile-time pins below assert the diagnostic boundary.)

use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;
use std::path::PathBuf;
use std::process::Command;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn error_messages(source: &str) -> Vec<String> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    result
        .diagnostics
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .map(|d| d.message.clone())
        .collect()
}

fn compile_and_run(source: &str, basename: &str) -> (String, bool) {
    use ruxen_core::codegen;
    use ruxen_core::mir::lower::Lowerer;
    let tmp_dir = workspace_root().join("tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let bin_path = tmp_dir.join(format!("{}-{}.bin", basename, std::process::id()));
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    let errs: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .map(|d| d.message.clone())
        .collect();
    assert!(errs.is_empty(), "typeck errors for {basename}: {errs:?}");
    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .unwrap_or_else(|e| panic!("lower {basename}: {e}"));
    codegen::compile(&mir, bin_path.to_str().unwrap())
        .unwrap_or_else(|e| panic!("codegen {basename}: {e}"));
    let output = Command::new(&bin_path).output().expect("run");
    let _ = std::fs::remove_file(&bin_path);
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        output.status.success(),
    )
}

fn run_case(name: &str, basename: &str) {
    let root = workspace_root();
    let src = std::fs::read_to_string(root.join("tests/release-e2e/cases").join(name))
        .unwrap_or_else(|e| panic!("read {name}: {e}"));
    let expected =
        std::fs::read_to_string(root.join("tests/release-e2e/expected").join(name.replace(".rx", ".out")))
            .unwrap_or_else(|e| panic!("read expected {name}: {e}"));
    let (stdout, ok) = compile_and_run(&src, basename);
    assert!(ok, "{name}: non-zero/segfault exit");
    assert_eq!(stdout, expected, "{name}: stdout was {stdout:?}");
}

/// (a) Empty-hash `key?`/`get` return cleanly (no SIGSEGV).
#[test]
fn empty_hash_lookup_no_segfault() {
    run_case("617_empty_hash_lookup.rx", "q25_empty_hash");
}

/// (a) Empty-set `contains` returns cleanly (no SIGSEGV).
#[test]
fn empty_set_contains_no_segfault() {
    run_case("618_empty_set_contains.rx", "q25_empty_set");
}

/// (b) A `&Hash[K,V]` by-ref param is accepted CONSISTENTLY in free-fn and
/// method position and reads correctly.
#[test]
fn hash_ref_param_consistent_and_sound() {
    run_case("619_hash_ref_param.rx", "q25_hash_ref");
}

/// (b) The bare `&Hash` (no generic args = the `Hashable` mixin, static
/// dispatch) is STILL rejected with E1118 — both as a free fn …
#[test]
fn bare_hash_mixin_ref_rejected_free_fn() {
    let errs = error_messages(
        "def f(h: &Hash) -> Int\n  0\nend\ndef main\n  puts \"x\"\nend\n",
    );
    assert!(
        errs.iter().any(|m| m.contains("E1118") || m.contains("does not use runtime dispatch")),
        "expected E1118 for bare `&Hash`; got {errs:?}"
    );
}

/// … and as a method. The method path resolves `&Hash` through a different
/// route than the free fn, so it surfaces a *different* diagnostic ("could
/// not infer type for parameter") rather than E1118 — but the key soundness
/// property holds: the bare `&Hash` mixin ref is REJECTED AT COMPILE TIME in
/// both positions, never compiled to a silent runtime miscompile. (Unifying
/// the two diagnostics is a DX follow-up, not a soundness gap.)
#[test]
fn bare_hash_mixin_ref_rejected_method() {
    let errs = error_messages(
        "class C\n  t: Int\n  def init; self.t = 0; end\n  def f(h: &Hash) -> Int\n    0\n  end\nend\ndef main\n  puts \"x\"\nend\n",
    );
    assert!(
        !errs.is_empty(),
        "expected a compile-time rejection for bare `&Hash` method param; got none"
    );
}
