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
//! | E1600-E1699   | package manager               |
//! | E1700-E1799   | std.regex                     |

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
    // `crates/ruxen-core/tests/implicit_codes_registered.rs`.
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
    // Numeric arithmetic operand mismatch. Ruxen performs no implicit
    // numeric coercion in binary arithmetic (`+ - * / %`); both operands
    // must already share a type. Emitted from
    // `typeck::infer::ops::infer_binop` when the two numeric operand types
    // fail to unify (e.g. `Int - Float`, `Int + Int64`). Catching it here
    // turns what was a silent miscompile (a Cranelift verifier crash:
    // `isub.i64 ... arg has type f64`) into a source-spanned diagnostic.
    CodeInfo {
        code: "E0707",
        title: "binary operator applied to mismatched numeric types",
    },
    // Phase 2 #06.5 T1: IoError variant constructor arity. Reserved
    // for the diagnostic emitted when a user constructs an IoError
    // variant with the wrong field set (e.g. `IoError.ConnectionRefused()`
    // with no `message:` arg, or `IoError.NotFound("oops")` on a
    // unit variant).
    CodeInfo {
        code: "E0710",
        title: "IoError variant constructor wrong arity",
    },
    // Phase 2 #06.5 T2: File / OpenOptions / SeekFrom diagnostics.
    // E0711 fires at runtime when `OpenOptions` is passed to
    // `File.open_options(path, opts)` without any of read/write/append
    // set — the open() syscall would otherwise be ambiguous. We surface
    // it as Result::Err(IoError::InvalidInput) at the runtime layer;
    // static detection of the static OpenOptions chain is deferred
    // until a const-folding pass is wired up (the chain is value-level
    // so a single-AST-pass static analysis can miss conditional sets).
    CodeInfo {
        code: "E0711",
        title: "OpenOptions requires at least one of read/write/append",
    },
    // E0712 fires at runtime when `File.seek(SeekFrom.Start(n))` is
    // called with n<0 — a position before byte 0 is meaningless. Same
    // deferral note as E0711: literal-only static detection is doable
    // but the more common runtime-computed offset case demands a
    // runtime check, so we ship the runtime check first and let the
    // static check piggy-back on a future const-folding pass.
    CodeInfo {
        code: "E0712",
        title: "SeekFrom arg out of range (negative offset on Start)",
    },
    // Phase 2 #06.5 T6: BufReader[R] / BufWriter[W] v1 restrict the
    // inner type to the closed set {File, TcpStream}. The full
    // Read/Write trait/mixin story is deferred to v1.5 with the
    // iterator-trait work; until then any other R/W is rejected at
    // typeck with E0714 (see typeck::method_resolvers::builtin_method_type).
    CodeInfo {
        code: "E0714",
        title: "BufReader/BufWriter inner type must be File or TcpStream (v1)",
    },
    // #06.8 Phase 2: cross-decl FFI signature conflict — two
    // `lib`/`extern` declarations target the same C symbol with
    // incompatible signatures. Keyed on the LINKED symbol (alias if
    // present, Ruxen name otherwise) so an alias mismatch trips this
    // before codegen can produce a mis-typed call.
    CodeInfo {
        code: "E0722",
        title: "conflicting FFI declarations for the same C symbol",
    },
    // #06.8 Wave 1 Task 0c: in-body `layout tagged` directive on an
    // enum pins variant declaration order as the runtime tag
    // assignment. The resolver tracks tagged-enum names per scope at
    // forward-declaration time and emits this code on the second
    // declaration of the same name. E0724 is intentionally reserved
    // (link-time `flat_heap_struct` layout-mismatch check) but is NOT
    // registered yet — the runtime `ruxen_<class>_layout_check`
    // symbol that gates it does not exist until a real stdlib class
    // adopts the marker.
    CodeInfo {
        code: "E0723",
        title: "duplicate `layout tagged` enum in scope",
    },
    // E0724 reserved for `#[repr(flat_heap_struct)]` /
    // `layout flat_heap_struct` link-time layout-mismatch — see the
    // note on E0723 above. Intentionally absent from REGISTRY until
    // a real consumer is wired.
    // #06.8 Wave 1 Task 0b: stdlib bootstrap loader. Every error path
    // through `resolve::bootstrap` carries this code so a contributor
    // diagnosing a broken stdlib `.rx` file lands on the same docs
    // page regardless of whether the failure is io, lexer, or parser.
    CodeInfo {
        code: "E0725",
        title: "stdlib bootstrap failed (file missing / lex / parse error)",
    },
    // Type-directed auto-call: a bare reference to a function/method that
    // requires arguments, used in a value position whose expected type is
    // not a `Fn` type, cannot be auto-called with zero arguments. The
    // diagnostic names both escape routes (call it, or annotate a `Fn`
    // type to reference it).
    CodeInfo {
        code: "E0726",
        title: "function reference needs arguments; call it or annotate a `Fn` type",
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
    // E1015 — general declared trait-bound enforcement (Feature B). Fires
    // when a `[T: Bound]` / `where T: Bound` generic param is instantiated
    // (or a `some`/`any Mixin` parameter is passed) with a concrete type
    // that does not satisfy `Bound`, for any Bound other than the ones with
    // a dedicated code (Send → E1011, Sync → E1012, the numeric `Add` on
    // `sum` → E0700, the Send-payload construction sites → E1101/E1102).
    // Emitted by `typeck::infer::ops::check_declared_bounds` /
    // `check_generic_param_bounds`.
    CodeInfo {
        code: "E1015",
        title: "type does not satisfy declared mixin bound",
    },
    // Send/Sync enforcement at thread-boundary construction sites
    // (docs/specs/ownership/send_sync_enforcement.spec.md). E1011 / E1012
    // (above) fire when a Send/Sync-bounded generic param sees a
    // non-satisfying type. E1100–E1102 fire at construction sites where
    // the safety boundary is implicit in the class semantics rather
    // than declared as a generic bound:
    //
    // * E1100 — `Thread.spawn` closure capture is not Send.
    // * E1101 — `Mutex.new(value)` / `Sender[T]` / `Receiver[T]` /
    //   `channel[T]()` constructed with non-Send T.
    // * E1102 — `SharedSync.new(value)` constructed with non-Send T.
    CodeInfo {
        code: "E1100",
        title: "captured value across thread boundary is not Send",
    },
    CodeInfo {
        code: "E1101",
        title: "Mutex / Sender / Receiver requires Send payload",
    },
    CodeInfo {
        code: "E1102",
        title: "SharedSync requires Send payload",
    },
    // Async lowering — sub-phase 2 (docs/specs/syntax/async_lowering.spec.md).
    // E1110 fires at every `.await` site whose enclosing function or
    // closure is not marked `async`. Detected during name resolution
    // (`resolve/exprs.rs`).
    CodeInfo {
        code: "E1110",
        title: "`.await` outside `async` function or closure",
    },
    // E1112 fires at every `block_on(...)` call site whose enclosing
    // function or closure IS marked `async`. Symmetric to E1110 —
    // calling block_on from inside an async context would deadlock
    // the single-threaded executor (the outer block_on waits for the
    // inner future, but the inner future can never make progress
    // because the outer call holds the only thread). Detected during
    // name resolution (`resolve/exprs.rs`). Async sub-phase 3
    // (docs/specs/stdlib/executor.spec.md B6).
    CodeInfo {
        code: "E1112",
        title: "`block_on` called inside `async` context (would deadlock)",
    },
    // E1115 fires when `.await` appears inside the body of a `while` /
    // `for` loop. v1 lowering does not handle loop suspension; the
    // shape requires a state per iteration. Deferred to v2 per the
    // async-lowering spec's "out of scope" list.
    CodeInfo {
        code: "E1115",
        title: "`.await` inside `while` / `for` body not yet supported",
    },
    // Task scheduler (`docs/specs/stdlib/task_spawn.spec.md`).
    // E1116 fires when high-level `Task.spawn(...)` appears in a sync
    // scope. Runtime-level `Task.spawn_raw(...)` is allowed for code
    // that establishes an executor by driving `block_on` itself.
    // Polarity-inverted twin of E1112: E1112 = block_on inside async;
    // E1116 = Task.spawn outside async. Detected during the same
    // async-lowering pre-pass (`collect_task_spawn_outside_
    // async_diagnostics`).
    CodeInfo {
        code: "E1116",
        title: "`Task.spawn` called outside `async` context",
    },
    // Mixin vtables (`docs/specs/types/mixin_vtables.spec.md`). Phase A
    // surfaces the codes; codegen of the actual vtables is Phase B/C.
    // E1117 fires when a class `include`s a mixin marked
    // `dispatch runtime` but does not implement all of its required
    // methods. Distinct from the existing structural-satisfaction
    // diagnostic because runtime-dispatch mixins MUST have a complete
    // method table — there is no compile-time fallback path.
    CodeInfo {
        code: "E1117",
        title: "class includes a `dispatch runtime` mixin but is missing required methods",
    },
    // E1118 fires when a `&Mixin` / `&var Mixin` parameter or field
    // type references a mixin that does NOT have `dispatch runtime`.
    // The `&Mixin` shape requires a vtable at runtime; only mixins
    // that opt in carry one.
    CodeInfo {
        code: "E1118",
        title: "`&Mixin` references a non-`dispatch runtime` mixin",
    },
    // ── Package manager (E1600-E1699) ────────────────────────────────
    // Emitted from `ruxen_cli`. Spans don't apply (these are toolchain
    // errors, not compile errors), but the codes follow the same
    // numbering scheme so `ruxen explain E1600` returns a hit.
    CodeInfo {
        code: "E1600",
        title: "workspace member not found",
    },
    CodeInfo {
        code: "E1601",
        title: "circular path dependency between workspace members",
    },
    CodeInfo {
        code: "E1602",
        title: "published version tag already exists at remote",
    },
    // ── Regex (E1700-E1799) ──────────────────────────────────────────
    // Lexer/parser/typeck errors for the std.regex package's
    // `/pat/flags` literal syntax and the `~=` match operator.
    // Spec: `docs/superpowers/specs/2026-05-29-std-regex-design.md`.
    CodeInfo {
        code: "E1700",
        title: "unrecognised regex flag",
    },
    CodeInfo {
        code: "E1701",
        title: "unterminated regex literal",
    },
    CodeInfo {
        code: "E1702",
        title: "`~=` operand type mismatch",
    },
    CodeInfo {
        code: "E1703",
        title: "empty regex pattern",
    },
    CodeInfo {
        code: "E1704",
        title: "invalid regex pattern",
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
