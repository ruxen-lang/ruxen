//! P0.5 regression guard: codegen must reject generic method calls that
//! have no real runtime symbol, instead of silently mapping them to the
//! historical `riven_noop_passthrough` stub.
//!
//! Background: `runtime_name()` used to fall through to
//! `riven_noop_passthrough` for any unrecognised `?T_xxx_method` mangled
//! name, masking unimplemented stdlib methods (`.fold`, `.sum`, `.count`,
//! `.collect`, `.map_err`, `.ok_or`, …) behind a no-op that happened to
//! produce the expected output for some fixtures. The fallback is gone;
//! these tests pin that behavior in place.

use riven_core::codegen;
use riven_core::codegen::runtime::runtime_name;
use riven_core::lexer::Lexer;
use riven_core::mir::lower::Lowerer;
use riven_core::parser::Parser;
use riven_core::typeck;

fn try_compile(source: &str) -> Result<(), String> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().map_err(|e| format!("lex: {e:?}"))?;
    let mut parser = Parser::new(tokens);
    let program = parser.parse().map_err(|e| format!("parse: {e:?}"))?;
    let result = typeck::type_check(&program);
    // Type errors mask the codegen path we're trying to test, so allow
    // them through here — we only care about the codegen step.
    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .map_err(|e| format!("mir: {e:?}"))?;
    let tmp = std::env::temp_dir().join(format!("riven_p05_reject_{}", std::process::id()));
    codegen::compile(&mir, tmp.to_str().unwrap())?;
    let _ = std::fs::remove_file(&tmp);
    Ok(())
}

/// `runtime_name` directly: an unrecognised `?T_…_method` mangled name
/// must produce an error that names the method, not silently no-op.
#[test]
fn runtime_name_rejects_unknown_inferred_method() {
    let err = runtime_name("?T7_totally_fake_method").unwrap_err();
    assert!(
        err.contains("totally_fake_method"),
        "diagnostic should name the method: {err}"
    );
    assert!(
        err.contains("no runtime symbol"),
        "diagnostic should mention missing runtime symbol: {err}"
    );
}

/// End-to-end: compiling a Riven program that calls `.flat_map` on an
/// iter (still unimplemented after #05 batch 3 — `chain` / `zip` /
/// `collect_vec` landed alongside `fold` / `all` / `any` / `take` /
/// `skip`; `flat_map` / `flatten` / `collect[FromIterator]` remain
/// rejected) must surface a codegen error rather than emit a binary
/// that silently no-ops. Replaces the prior `.zip` canary from #05
/// batch 2 which now compiles via `riven_vec_zip`.
#[test]
fn compile_fails_when_calling_unimplemented_iter_flat_map() {
    let source = r##"
def main
  let a = vec![1, 2, 3]
  let _z = a.iter.flat_map { |x| vec![x, x] }
end
"##;
    let err = try_compile(source)
        .expect_err("expected codegen to refuse `.iter.flat_map` (no runtime symbol)");
    assert!(
        err.contains("no runtime symbol"),
        "diagnostic should mention missing runtime symbol; got: {err}"
    );
    assert!(
        err.contains("flat_map") || err.contains("iter") || err.contains("to_vec"),
        "diagnostic should name the unresolved method; got: {err}"
    );
}
