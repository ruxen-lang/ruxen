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

use ruxen_core::codegen::runtime::runtime_name;

// (The `rx` / `try_compile` end-to-end helpers were removed with the
// `.iter.flat_map` codegen-rejection canary — see the note at the bottom
// of this file. The surviving test drives `runtime_name` directly.)

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
