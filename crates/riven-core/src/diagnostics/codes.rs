//! Central registry of compiler error codes (T5.04 phase 1).
//!
//! Every code emitted via [`Diagnostic::error_with_code`] must appear in
//! this table. The companion test in `tests/error_code_registry.rs`
//! greps the source tree and fails the build if a code is used at a
//! call site without a registry entry.
//!
//! Namespaces (per ROADMAP.md):
//!
//! | Range         | Owner                         |
//! |---------------|-------------------------------|
//! | E0001-E0099   | lexer                         |
//! | E0601-E0618   | implicit-include / auto-synth |
//! | E0700-E0799   | tier-2 type system            |
//! | E1011-E1099   | mixin / include checking      |

/// Metadata for a single compiler error code.
#[derive(Debug, Clone, Copy)]
pub struct CodeInfo {
    pub code: &'static str,
    pub title: &'static str,
}

/// All compiler error codes in use today. Sorted by code for easy
/// scanning. Add a row here when you introduce a new code.
pub const REGISTRY: &[CodeInfo] = &[
    // ── Lexer (E0001-E0099) ──────────────────────────────────────────
    CodeInfo {
        code: "E0001",
        title: "unterminated block comment",
    },
    CodeInfo {
        code: "E0002",
        title: "unterminated string literal",
    },
    CodeInfo {
        code: "E0003",
        title: "invalid escape sequence",
    },
    CodeInfo {
        code: "E0004",
        title: "invalid numeric literal",
    },
    CodeInfo {
        code: "E0005",
        title: "unterminated character literal",
    },
    CodeInfo {
        code: "E0006",
        title: "unexpected character",
    },
    CodeInfo {
        code: "E0007",
        title: "malformed format spec",
    },
    // ── Reserved diagnostic-test code ────────────────────────────────
    // Used by `diagnostics::tests` to exercise the `code` field.
    CodeInfo {
        code: "E0042",
        title: "diagnostic-fixture sentinel (tests only)",
    },
    // ── Implicit-include / auto-synth (E0601-E0618) ──────────────────
    // Structural mixins (Debug, Clone, Copy, PartialEq, Eq, Hashable,
    // Default, Ord, PartialOrd) are implicitly included on a type
    // whose fields satisfy the relevant bound (see ruby-naming
    // §3.6). When the rule fails, the diagnostic fires at either the
    // explicit `include` site or the first use site of the relevant
    // method. The codes below cover those failures.
    //
    // E0612 and E0614 are intentionally reserved (Copy uses E0602, Eq
    // uses E0604). The reservation is enforced by the test
    // `e0612_and_e0614_remain_reserved_unless_explicitly_unblocked` in
    // `crates/riven-core/tests/implicit_codes_registered.rs`.
    CodeInfo {
        code: "E0601",
        title: "cannot synthesize Copy: field has non-Copy type",
    },
    CodeInfo {
        code: "E0602",
        title: "Copy implies Clone — a Copy type must also support Clone",
    },
    CodeInfo {
        code: "E0603",
        title: "Copy cannot be auto-synthesized on a class; use a struct",
    },
    CodeInfo {
        code: "E0604",
        title: "Eq implies PartialEq — a type must satisfy both together",
    },
    CodeInfo {
        code: "E0605",
        title: "cannot synthesize Default on enum without a `default` variant",
    },
    CodeInfo {
        code: "E0606",
        title: "Ord implies Eq and PartialOrd — synthesize them together",
    },
    CodeInfo {
        code: "E0608",
        title: "unknown mixin requested for auto-synthesis",
    },
    CodeInfo {
        code: "E0609",
        title: "duplicate include of an auto-synthesized mixin",
    },
    CodeInfo {
        code: "E0610",
        title: "auto-synth: field type does not satisfy the synthesized mixin's bound",
    },
    CodeInfo {
        code: "E0611",
        title: "auto-synth Clone: unsupported field shape (e.g. opaque foreign type)",
    },
    CodeInfo {
        code: "E0613",
        title: "auto-synth PartialEq: field type does not satisfy PartialEq",
    },
    CodeInfo {
        code: "E0615",
        title: "auto-synth Hashable: field type is not hashable",
    },
    CodeInfo {
        code: "E0616",
        title: "auto-synth Default: enum's first variant has fields without Default",
    },
    CodeInfo {
        code: "E0617",
        title: "auto-synth Ord: field type does not satisfy Ord",
    },
    CodeInfo {
        code: "E0618",
        title: "auto-synth PartialOrd: field type does not satisfy PartialOrd",
    },
    // ── Tier-2 type system (E0700-E0799) ─────────────────────────────
    //
    // E0700 was originally used by both the iterator-`sum` validator
    // (typeck) and the const-generic kind-mismatch (resolve).  The
    // collision was resolved 2026-05-14: iterator-sum keeps E0700;
    // const-generic kind-mismatch moved to E0704.  The const-generics
    // spec §"Error code reservations" mirrors the assignment.
    CodeInfo {
        code: "E0700",
        title: "iterator `sum` requires an Item that implements `Add`",
    },
    // E0701-E0703 reserved by const-generics spec §"Error code
    // reservations" but not yet emitted; registered so docs/errors/
    // stays in sync and future S8/S9 work can emit them without
    // separate registry plumbing.
    CodeInfo {
        code: "E0701",
        title: "const-generic argument does not fit declared type",
    },
    CodeInfo {
        code: "E0702",
        title: "expression is not a valid v1 const expression",
    },
    CodeInfo {
        code: "E0703",
        title: "const expression overflows or divides by zero during evaluation",
    },
    CodeInfo {
        code: "E0704",
        title: "kind mismatch on const-generic argument",
    },
    CodeInfo {
        code: "E0705",
        title: "const-generic parameter type must be an integer or `Bool`",
    },
    CodeInfo {
        code: "E0706",
        title: "where-clause const predicate is not satisfied at this instantiation",
    },
    // Phase 2 #06.5 T1: IoError variant constructor arity. Reserved
    // for the diagnostic emitted when a user constructs an IoError
    // variant with the wrong field set (e.g. `IoError.ConnectionRefused()`
    // with no `message:` arg, or `IoError.NotFound("oops")` on a
    // unit variant). E0711-E0714 are reserved by later #06.5 tasks
    // (T2 metadata access, T3 path API, T4 fs ops, T5 net surface)
    // and intentionally left unregistered here.
    CodeInfo {
        code: "E0710",
        title: "IoError variant constructor wrong arity",
    },
    // ── Borrow checking + mixin/include (E1001-E1099) ────────────────
    // The borrow checker maintains a parallel `ErrorCode` enum in
    // `borrow_check/errors.rs`; titles below mirror its `title()`
    // method. Keep them in sync when adding new codes.
    CodeInfo {
        code: "E1001",
        title: "value used after move",
    },
    CodeInfo {
        code: "E1002",
        title: "cannot borrow writably — already borrowed read-only",
    },
    CodeInfo {
        code: "E1003",
        title: "cannot borrow read-only — already borrowed writably",
    },
    CodeInfo {
        code: "E1004",
        title: "cannot move out of borrowed reference",
    },
    CodeInfo {
        code: "E1005",
        title: "borrow outlives owner",
    },
    CodeInfo {
        code: "E1006",
        title: "cannot assign to `let` binding",
    },
    CodeInfo {
        code: "E1007",
        title: "cannot borrow `let` binding writably",
    },
    CodeInfo {
        code: "E1008",
        title: "value moved into closure, cannot be used outside",
    },
    CodeInfo {
        code: "E1009",
        title: "cannot move value — currently borrowed",
    },
    CodeInfo {
        code: "E1010",
        title: "returned reference outlives local value",
    },
    CodeInfo {
        code: "E1011",
        title: "type does not satisfy Send",
    },
    CodeInfo {
        code: "E1012",
        title: "type does not satisfy Sync",
    },
    CodeInfo {
        code: "E1013",
        title: "non-`static` value captured by send-required closure",
    },
    CodeInfo {
        code: "E1014",
        title: "invalid `unsafe include` declaration",
    },
];

/// Look up an error code's metadata. Returns `None` if the code is not
/// registered.
pub fn lookup(code: &str) -> Option<&'static CodeInfo> {
    REGISTRY.iter().find(|info| info.code == code)
}

/// Whether `code` is registered.
pub fn is_registered(code: &str) -> bool {
    lookup(code).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_no_duplicate_codes() {
        let mut seen = std::collections::HashSet::new();
        for info in REGISTRY {
            assert!(
                seen.insert(info.code),
                "duplicate registry entry: {}",
                info.code
            );
        }
    }

    #[test]
    fn lookup_returns_known_codes() {
        assert_eq!(
            lookup("E0001").map(|i| i.title),
            Some("unterminated block comment")
        );
        assert_eq!(
            lookup("E1011").map(|i| i.title),
            Some("type does not satisfy Send")
        );
        assert!(lookup("E9999").is_none());
    }

    #[test]
    fn every_entry_has_nonempty_title_and_well_formed_code() {
        for info in REGISTRY {
            assert!(
                info.code.starts_with('E') && info.code.len() >= 5,
                "ill-formed code: {:?}",
                info.code
            );
            assert!(!info.title.is_empty(), "empty title for {}", info.code);
        }
    }
}
