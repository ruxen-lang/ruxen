//! Registry-coverage tests for derive-macro error codes (P1.05/B1).
//!
//! Two surfaces are checked here, both as direct `REGISTRY` membership
//! assertions so a missing code surfaces immediately and pinpoints
//! exactly which entry is absent — the existing scanner in
//! `error_code_registry.rs` has a fixed-width lookahead window that
//! silently masks `error_with_code(...)` calls whose code literal
//! lands more than five lines after the call header (which is the
//! norm in `derive/mod.rs`).
//!
//! ## What this file pins
//!
//! 1. `e0601_through_e0609_emitted_codes_are_registered`
//!    Codes already emitted by `crates/riven-core/src/derive/mod.rs`
//!    (E0601, E0603, E0605, E0608, E0609) must appear in the
//!    registry. They are emitted today but unregistered — fixing this
//!    is the first half of B1.
//!
//! 2. `e0610_through_e0618_derive_codes_are_registered`
//!    The new codes minted for the P1.05 derive-synth refactor
//!    (E0610, E0611, E0613, E0615, E0616, E0617, E0618). E0612 and
//!    E0614 are intentionally absent — they are reserved for Copy /
//!    Eq markers, which reuse E0602 / E0604 respectively.
//!
//! The architect's binding rulings on these mappings are repeated
//! inline next to each `assert!` so the intent survives a future
//! refactor.

use riven_core::diagnostics::codes::{is_registered, REGISTRY};

/// Pre-existing codes emitted by `crates/riven-core/src/derive/mod.rs`
/// must be registered. Currently they are not — this test goes red on
/// the empty registry slots and stays green once B1 lands the missing
/// `CodeInfo` rows.
#[test]
fn e0601_through_e0609_emitted_codes_are_registered() {
    let mut missing = Vec::new();
    for code in ["E0601", "E0603", "E0605", "E0608", "E0609"] {
        if !is_registered(code) {
            missing.push(code);
        }
    }
    assert!(
        missing.is_empty(),
        "the following codes are emitted by `derive/mod.rs` but absent \
         from `diagnostics::codes::REGISTRY`: {:?}.\nAdd a `CodeInfo` \
         row for each in `crates/riven-core/src/diagnostics/codes.rs`.",
        missing
    );
}

/// New derive-synth codes minted for P1.05/B1 must be registered with
/// `CodeInfo` rows. Note that E0612 and E0614 are intentionally absent
/// from this list:
///
///   * E0612 reserved — `derive(Copy)` errors continue to reuse E0602.
///   * E0614 reserved — `derive(Eq)` errors continue to reuse E0604.
///
/// Each tuple here is `(code, conceptual_purpose)`. The purpose
/// string is informational only (used in failure messages); coder
/// chooses the final `title` wording when adding the registry row.
#[test]
fn e0610_through_e0618_derive_codes_are_registered() {
    let expected: &[(&str, &str)] = &[
        ("E0610", "derive_field_does_not_implement"),
        ("E0611", "derive_clone_unsupported_shape"),
        ("E0613", "derive_partial_eq_field_mismatch"),
        ("E0615", "derive_hash_field_not_hashable"),
        ("E0616", "derive_default_enum_no_default_variant"),
        ("E0617", "derive_ord_field_not_ord"),
        ("E0618", "derive_partial_ord_field_not_partial_ord"),
    ];
    let mut missing = Vec::new();
    for (code, purpose) in expected {
        if REGISTRY.iter().find(|e| e.code == *code).is_none() {
            missing.push(format!("{} ({})", code, purpose));
        }
    }
    assert!(
        missing.is_empty(),
        "the following P1.05 derive-synth codes are not yet registered \
         in `diagnostics::codes::REGISTRY`:\n  {}\nAdd a `CodeInfo` \
         row for each in `crates/riven-core/src/diagnostics/codes.rs`.",
        missing.join("\n  ")
    );
}

/// Sanity guard: E0612 and E0614 are deliberately reserved (Copy uses
/// E0602, Eq uses E0604). If a future refactor adds them, that's a
/// signal to revisit the mapping in this file and the architect's
/// ruling — not a silent allowance.
///
/// This test passes today (the codes are absent) and is expected to
/// keep passing through B1. It exists as documentation in executable
/// form so the reservation can't be quietly broken.
#[test]
fn e0612_and_e0614_remain_reserved_unless_explicitly_unblocked() {
    assert!(
        !is_registered("E0612"),
        "E0612 is reserved for the Copy marker (uses E0602). If you \
         intend to allocate E0612 to something else, update the \
         architect's binding ruling in docs/prompts/v1/01_phase1_remainder.md \
         first."
    );
    assert!(
        !is_registered("E0614"),
        "E0614 is reserved for the Eq marker (uses E0604). If you \
         intend to allocate E0614 to something else, update the \
         architect's binding ruling in docs/prompts/v1/01_phase1_remainder.md \
         first."
    );
}
