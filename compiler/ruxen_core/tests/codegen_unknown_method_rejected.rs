//! P0.5 regression guard: codegen must reject generic method calls that
//! have no real runtime symbol, instead of silently mapping them to the
//! historical `ruxen_noop_passthrough` stub.
//!
//! Background: `runtime_name()` used to fall through to
//! `ruxen_noop_passthrough` for any unrecognised `?T_xxx_method` mangled
//! name, masking unimplemented stdlib methods (`.fold`, `.sum`, `.count`,
//! `.collect`, `.map_err`, `.ok_or`, …) behind a no-op that happened to
//! produce the expected output for some fixtures. The fallback is gone;
//! these tests pin that behavior in place.

use ruxen_core::codegen;
use ruxen_core::codegen::runtime::runtime_name;
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

fn rx(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruxen")
        .join(format!("{name}.rx"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

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
    let tmp = std::env::temp_dir().join(format!("ruxen_p05_reject_{}", std::process::id()));
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

// (The `.iter.flat_map` codegen-rejection canary was removed with the
// iterator layer — `.iter` no longer exists, so `flat_map` is rejected at
// typeck as "no method" rather than reaching codegen. General unknown-
// method rejection stays covered by
// `runtime_name_rejects_unknown_inferred_method` above.)
