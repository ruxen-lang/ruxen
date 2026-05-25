# Changelog

All notable changes to this project will be documented here. The
format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
once 1.0.0 ships.

## [Unreleased]

### Added
- **#06.8 stdlib self-hosting — Waves 1.5–2 + collection migrations
  (T#13/14/15/16/17/21).** Every named stdlib module and every
  collection-method dispatch table that previously lived as Rust
  registrations in `compiler/ruxen_core/src/resolve/stdlib/mod.rs`
  and `codegen/runtime_table/mod.rs` now lives in `library/std/src/
  *.rx`. The bootstrap loader (`resolve/bootstrap.rs`) parses
  these files at compiler startup before user code, the resolver's
  **namespace-anchor mode** reuses the existing type-scope bindings
  for builtin names (so `Ty::String` / `Ty::Array(_)` etc. stay
  canonical for codegen), and MIR's **FFI alias map** rewrites the
  mangled call-site callee (`Array[Int]_push`, `Option[String]_unwrap_or`,
  ...) to the verbatim C symbol BEFORE codegen consults
  `runtime_table`. A generic-stripping fallback peels surface
  `[...]` args so the parent-name-keyed alias entries
  (`Array_push`, `Option_unwrap_or`) match every call site. 94
  collection methods migrated across String / Option / Result /
  Array / Map / Set; ~12 self-hosted .rx modules under
  `library/std/src/`. Two outliers retained in `runtime_table` for
  documented architectural reasons (`String_clone` aliases a C
  symbol with a different wire shape and trips E0722; `String_from_iter`
  has no surface method to attach to).
- **#06.5 Phase 2 sync I/O completeness.** TcpListener/TcpStream
  classes (incl. binary-safe read + socket timeouts), BufReader[R] /
  BufWriter[W] over the closed inner set {File, TcpStream}, std.rand
  (kernel CSPRNG), Command + Instant promotion to the canonical
  surface (legacy `process_run` / `now_ns` free-fns retired).
  Mixin/trait-druxen Read/Write surface is deferred to v1.5; closed-set
  inner check at typeck (E0714) keeps the runtime kind-tag honest.

### Fixed
- **compiler: `any Fn(...)` closures are now dispatchable via
  indirect call (#06.9).** Works in let-binding, array storage,
  class field, and return-position contexts. Unblocks Router /
  event-bus / observer-pattern code. Three sites changed
  (`typeck/unify.rs` lifts the closure → dyn-Fn coercion out of
  arg-only position; `mir/lower/expr/method_call.rs` recognises
  `Ty::AnyMixin([Fn])` receivers, falls back to the local's MIR
  type when typeck left the for-loop binding as `Ty::Infer`, and
  emits the same indirect-call shape the concrete-`Fn` path
  already uses; `codegen/runtime_table/mod.rs` adds a
  belt-and-suspenders fast-path for `any Fn[...]_call` and `?T*_call`
  manglings). 5 pin tests in
  `compiler/ruxen_core/tests/closures_dyn_dispatch.rs`; e2e
  fixture `tests/release-e2e/cases/600_closure_handler_dispatch.rx`.

### Changed
- **Repo restructured to rust-lang-style layout (#06.75).** The
  single `crates/ruxen-core` crate (≈26 KLOC, owning lexer / parser /
  hir / resolve / typeck / mir / borrow_check / codegen / formatter /
  the C runtime / every stdlib registration) is unpacked into a
  `compiler/` + `library/` + `src/` + `tests/` top-level tree:

  - `library/runtime/` — the C runtime, carved per module
    (`core/{alloc,vec,string,hash}.c`, `io/{io_error,stdio,file}.c`,
    `fs.c`, `net/tcp.c`, `time.c`, `signal.c`, `process.c`, `fmt.c`,
    `env.c`).  Top-level `library/runtime/runtime.c` `#include`s each
    module so the build product is still a single translation unit
    with identical link symbols.
  - `library/std/src/` — the `.rx`-source side of the stdlib
    (currently just `iter.rx` as declarative documentation).
  - `compiler/ruxen_core/` — the (still-single) compiler crate,
    relocated from `crates/`.  Internal stdlib registrations now live
    under `compiler/ruxen_core/src/resolve/stdlib/` (carved out of the
    7 153-LOC `resolve/mod.rs`), method resolvers under
    `compiler/ruxen_core/src/typeck/method_resolvers/` (carved out of
    `typeck/infer.rs`), and the runtime-name table under
    `compiler/ruxen_core/src/codegen/runtime_table/` (carved out of
    `codegen/runtime.rs`).
  - `compiler/ruxen_driver/` — a thin `pub use ruxen_core::*` shim,
    placeholder for the future per-phase crate split (`ruxen_lexer`,
    `ruxen_parser`, `ruxen_resolve`, `ruxen_typeck`, …) which is
    deferred to a follow-up prompt.
  - `src/` — every driver crate (`ruxenc`, `ruxen_cli`, `ruxen_lsp`,
    `ruxen_ide`, `ruxen_repl`), relocated from `crates/`.
    Kebab-case package names switched to snake_case to match their
    lib names — `use ruxen_core::…` import sites unchanged.
  - `tests/` — workspace-level integration root (currently houses
    `release-e2e/`; per-crate Cargo integration tests stay under
    `compiler/ruxen_core/tests/` since Cargo's integration-test
    convention is per-crate).

  Workspace `members` is now `["compiler/*", "src/*"]`.  C-side symbol
  names are byte-identical; no language surface changes; no behavior
  changes.  Architecture overview: `docs/architecture/repo-layout.md`.

### Added
- Phase 2 stdlib #06: `std::process::Command` builder API closes the
  last #06 v1 gap. `Command.new(prog).arg("a").args(["b","c"])
  .env("K","V").current_dir("/tmp").status()` returns
  `Result[ExitStatus, IoError]`; `.output()` returns
  `Result[Output, IoError]` with captured stdout/stderr. `Output`
  exposes `.status` / `.stdout` / `.stderr`; `ExitStatus` exposes
  `.code` / `.success`. Pre-flight `access(F_OK)` turns typo'd binary
  paths into `Result::Err(IoError::NotFound)` rather than the
  indistinguishable `Ok(ExitStatus(127))` that the lower-level
  `process_run` returns. Mirrors the `fs.metadata` flat-heap-struct
  pattern with one extension: `Command` is in `user_drop_classes` so
  builder-pattern temporaries are reclaimed via `Command_drop` +
  `ruxen_dealloc` at scope exit. 9 pin tests in
  `stdlib_process.rs::command_*`; e2e fixtures `508_command_status`
  and `512_command_output`. `Command.spawn -> Child` (the async-style
  handle with `.wait/.kill/.try_wait`) is explicitly DEFERRED to v2
  per the prompt.
- Phase 2 stdlib #06: `std::fs::metadata(path) -> Result[Metadata,
  IoError]` returning a flat heap-allocated `Metadata` struct with
  `len` / `modified` / `is_file` / `is_dir` / `is_symlink` accessors.
  Backed by `lstat(2)` so symlinks are reported as Symlink rather than
  followed. The on-wire layout (3 × int64: size / modified-secs /
  kind-tag) is packed by the runtime so the FFI surface is independent
  of libc's `struct stat`. Pin tests in `stdlib_fs.rs::fs_metadata_*`
  (positive file, positive dir, negative missing-path); e2e fixture
  `507_fs_metadata`.

### Added
- Phase 3 #07.S9 parser cut: `where`-clause const predicates.  The
  parser now accepts `where N > 0`, `where N == M`,
  `where N + M == 8` (and any mix of `> < >= <= == !=` comparisons +
  `+ - * /` arithmetic) on const-generic functions / types.  Each
  predicate is captured as a raw parser `Expr` on the new
  `WhereClause::const_predicates: Vec<ConstPredicate>` field;
  existing `WhereClause::predicates` (mixin bounds) is untouched, so
  existing consumers see no shape change.

  Disambiguation: a one-token lookahead past the leading identifier
  picks the path.  If the next token is a comparison or arithmetic
  op, parse as a const predicate; otherwise fall through to the
  historic mixin-bound parser (which expects `:`).  Mixed clauses
  (`where T: Display, N > 0`) split correctly into the two lists.

  Spec stage map S9 entry updated to "in flight" with the parser-
  cut commit; per-instantiation enforcement +
  `E-CONST-WHERE-FALSE` diagnostic remain pending the deeper S7
  binding-threading work.

  Pin tests in `const_generics.rs`:
  `parse_where_clause_const_predicate_n_gt_zero`,
  `parse_where_clause_const_predicate_n_eq_m`,
  `parse_where_clause_const_predicate_arithmetic_eq`
  (`N + M == 8` — verifies the LHS itself is an arithmetic tree),
  `parse_where_clause_mixed_trait_bound_and_const_predicate`
  (both forms in one clause split correctly),
  `parse_where_clause_trait_bound_alone_still_works` (regression
  gate for the historic shape).

### Added
- E2E harness supports a `RUXEN_E2E_CASES` env-var case filter for
  selective runs.  Comma-separated case stems (filenames without
  `.rx`); absent var preserves the existing behaviour (full
  sweep).  Whitespace is trimmed; empty entries ignored; unknown
  cases fail fast with a clear message so typos don't silently
  skip the case the developer was trying to verify.

  New workflow:
  - **Per-commit**: skip e2e entirely (`#[ignore]`-gated as before).
  - **New / changed fixture**:
    `RUXEN_E2E_CASES=NAME cargo test --test release_e2e_smoke
     -- --ignored` runs just that one in ~1s.
  - **Phase / tier completion**: `cargo test --test
     release_e2e_smoke -- --ignored` runs the full ~3-min sweep.

  Documented inline at `tests/release_e2e_smoke.rs::release_e2e_
  all_fixtures`.

### Fixed
- **Const-generic class instantiation parses correctly at value-
  expression call sites.**  Before this fix, `Counter[10].new(42)`
  silently misparsed as `Counter` indexed by `10` followed by
  `.new(42)`, producing an `<error>` receiver type and a downstream
  linker failure (`Undefined symbols: _<error>_init`).  Root cause
  was the `looks_like_type_args` lookahead in `parser/expr.rs`
  rejecting `IntLiteral` as a potential first token after `[` — it
  only accepted type-token kinds (`TypeIdentifier`, `SelfType`,
  `Lifetime`, `Amp`, `AmpMut`).  Adding `TokenKind::IntLiteral(_, _)`
  to the accepted set lets `Counter[10].new(42)` correctly parse as
  a const-generic type application followed by a method call.

  Pin test:
  `parse_const_arg_int_literal_at_call_site_is_type_application`
  asserts the receiver of `.new(42)` is a `MethodCall`, NOT an
  `Index` expression (the regression shape).

  E2E fixture:
  `tests/release-e2e/cases/073_const_generic_class_instantiation.rx`
  exercises `Counter[10].new(1)`, `Counter[1000].new(2)`, and
  `Pair[Int, 7].new(99)` through full compile + run, confirming
  multi-instantiation and mixed type/const generics all work.
  Release-e2e harness now runs 223 fixtures (was 222).

  Spec stage map S6 entry updated: runtime construction of
  const-generic classes whose body doesn't reference the const
  param is now shipped; per-instantiation MIR lowering for bodies
  that *do* use the const param (e.g. `class Buf[const N: USize] {
   data: [Int; N] }`) remains a v1.next follow-up tracked in the
  S7 follow-up section.

### Added
- `tests/release-e2e/cases/072_const_generic_array_arithmetic.rx`:
  new end-to-end fixture exercising the S8.S2/S8.S4 array-size
  arithmetic path through full compile-and-run.  Covers literal
  arithmetic (`2 + 1`), paren-grouped precedence
  (`(1 + 1) * 2`), and identity folding (`5 + 0`).  Verifies that
  the const evaluator + normal-form rewriter agree with the
  codegen layout pass — three arrays of sizes 3 / 4 / 5
  initialise correctly and read back the expected elements.

  The release-e2e harness (`release_e2e_all_fixtures`) now runs 222
  fixtures (was 221; +1 const-generic case).  Note: const-generic
  *class* / *struct* instantiation through codegen
  (`Counter[10].new(...)`) still hits the per-key MIR lowering
  gap tracked as the S6 follow-up; that's a separate workstream
  from the array-size form which this fixture pins.

### Changed
- `docs/tutorial/21-const-generics.md` rewritten end-to-end to
  match the shipped reality.  Previously the status block said
  "Stage 1+2 shipped, 3–9 pending" and the body taught only literal
  use sites.  Updated to cover: bare literals, `+ - * /`
  arithmetic in both array-size and const-arg positions, the
  normal-form rewriter (`2 + 2`, `4 * 1`, `4 + 0` all unify with
  the bare `4` form), the four shipped diagnostics
  (E0702 / E0703 / E0704 / E0705) with cross-links to their
  detailed pages, distinct-types semantics, S5 soundness fix,
  and the two genuinely-remaining v1 follow-ups (S7
  binding-threading, S9 where-clause predicates).  Stage table
  removed in favour of a "what's pending" callout — the spec is
  the canonical stage map.  Examples chosen to mirror the
  pin-test bodies in `tests/const_generics.rs`.

### Added
- Phase 3 #07 follow-up: **E0702 non-const-expression diagnostic**
  (spec §B8 E-CONST-NONCONST).  The lowerer
  `lower_const_expr_from_expr` already produced a
  `ConstExpr::Error` marker for AST shapes outside the v1 const
  language (unsupported binary ops like `%` / `<` / `<<` / `&&`,
  function calls, method calls, field access, runtime variable
  references); resolve now walks the lowered tree and surfaces
  the first `Error` it finds as **E0702** at the construction
  site's span.  One diagnostic per site — nested markers don't
  compound.

  Helpers:
  - New `contains_const_expr_error(&ConstExpr) -> bool` walks the
    tree.
  - New `Resolver::check_const_expr_for_non_const(&mut, &Span)`
    emits the diagnostic; called at the same two sites as
    `check_const_expr_eval_errors` (array size + const-arg
    position).

  Registry / docs / spec coordination:
  - `codes::REGISTRY` title for E0702 dropped its "(reserved)"
    qualifier and now reads "expression is not a valid v1 const
    expression".
  - `docs/errors/E0702.md` rewritten with concrete examples
    (`%`, comparison, function call), a fix matrix (rewrite, lift
    to const param, declare a `const`), and notes referencing
    NG-OQ-3 (no const-arg inference).
  - Spec §"Error code reservations" updated — E0702 marked
    shipped; E0701 remains the lone reserved code.

  Pin tests in `const_generics.rs`:
  `array_size_unsupported_op_emits_e0702` (`5 % 2`),
  `array_size_comparison_op_emits_e0702` (`3 < 4`),
  `array_size_clean_arithmetic_does_not_emit_e0702`,
  `array_size_param_reference_does_not_emit_e0702`.  The existing
  `resolve_array_size_unsupported_op_becomes_error` (HIR-shape
  pin) continues to pass — E0702 is additive, not a replacement.
- Phase 3 #07 follow-up: **E0705 const-generic parameter bad-type
  diagnostic** (spec §B8 `E-CONST-BAD-TYPE`).  `Resolver::
  collect_generic_params` now validates the resolved type of every
  `const NAME: TY` parameter against the v1 allow-list (integer
  family + `Bool`).  Float* / String / user class / Array / tuple /
  every other shape surfaces as **E0705** with the parameter name
  and the rejected type in the message.  `Ty::Error` is treated as
  valid so the diagnostic doesn't stack on top of an upstream
  "unknown type" error.

  New helper `is_valid_const_param_ty(&Ty)` placed alongside the
  existing `ty_is_valid_hash_key` in `resolve/mod.rs`.  Registry
  entry, `EXPLAINS` row, and `docs/errors/E0705.md` page added per
  the existing three-way sync invariant; the page documents the
  full rejected-types list with NG2 / NG3 / OQ-3 cross-references.

  Pin tests in `const_generics.rs`:
  - `const_param_float_type_emits_e0705` — NG2 (NaN ≠ NaN).
  - `const_param_string_type_emits_e0705` — NG3.
  - `const_param_user_class_type_emits_e0705` — user aggregate
    types.
  - `const_param_integer_types_do_not_emit_e0705` — pins every
    integer family + `Bool` as accepted.
  - `const_param_bad_type_does_not_stack_on_unresolved_type` —
    `Bogus` (unresolved) doesn't compound diagnostics.

  Spec §"Error code reservations" updated to mark E0703/E0704/E0705
  shipped (E0701/E0702 remain reserved pending the const-arg-vs-
  param-type and non-const-expr emit sites).
- Phase 3 #07 follow-up: **E0703 surfacing for pure-literal const
  overflow / division-by-zero**.  New
  `Resolver::check_const_expr_eval_errors` runs immediately after
  the S8.S4 normal-form pass at every `TypeExpr::Array { size }`
  and `TypeExpr::ConstExprArg` resolve site.  It calls
  `eval(empty)` on the normalised tree and maps
  `ConstEvalError::Overflow` and `ConstEvalError::DivisionByZero`
  to **E0703** with the source span of the const expression.

  Pure-literal sub-trees whose eval failed survive the normaliser
  as `Op(Lit, _, Lit)` (the successful pure-literal `Op` cases
  collapse to `Lit`), so detection is local and accurate.  Trees
  that mention a const-generic parameter return
  `Err(Unresolved(name))` from eval and are silently skipped —
  their overflow status depends on the instantiation, so the
  check defers to the (still-pending) monomorphization-side pass
  that needs the per-instantiation binding-threading prep work.

  The registry / explain / docs trio refreshed:
  - `codes::REGISTRY` title for E0703 dropped its "(reserved)"
    qualifier and now reads "const expression overflows or
    divides by zero during evaluation".
  - `docs/errors/E0703.md` rewritten with concrete examples for
    `+` overflow, `/0`, nested multiplication overflow, and a
    note on why param-bearing trees aren't flagged.

  Pin tests in `const_generics.rs`:
  `array_size_overflow_emits_e0703` (`i64::MAX * 4` overflows
  u64), `array_size_division_by_zero_emits_e0703`,
  `const_arg_position_overflow_emits_e0703`,
  `array_size_param_arithmetic_does_not_emit_e0703`
  (`N + 1` deferred), `array_size_bare_literal_does_not_emit_e0703`,
  `array_size_clean_arithmetic_does_not_emit_e0703`.
- Phase 3 #07.S8.S4: `ConstExpr::normal_form()` rewriter canonicalises
  arithmetic trees so two source-level forms that denote the same
  compile-time integer produce the same `Ty::ConstArg`.  Rules
  applied bottom-up: pure `Lit ⊙ Lit` constant-folds via the S8.S1
  evaluator (overflow / div-zero leave the `Op` shape intact so a
  later E0703 surfacing pass keeps both spans); identity rewrites
  collapse `x + 0`, `0 + x`, `x - 0`, `x * 1`, `1 * x`, `x / 1`
  to `x`; `x * 0` and `0 * x` to `0`.  Applied at the resolve
  construction sites for both `TypeExpr::Array { size }` and
  `TypeExpr::ConstExprArg`, so `[T; N + 0]` and `[T; N]` compare
  equal through derived `PartialEq`.  Not handled (spec §B8
  intentional limit): distributive rewrites (`N*(M+1)` vs.
  `N*M + N`), commutative reordering of mixed `Param`/`Lit`,
  associative reassociation — v2 will surface
  `E-CONST-NORMAL-FORM` at the kind-check when two instantiations
  differ only by a form the rewriter can't canonicalise.

  Pin tests in `const_generics.rs`:
  `const_expr_normal_form_identity_rewrites` (all eight rules),
  `const_expr_normal_form_folds_pure_arithmetic` (basic + nested),
  `const_expr_normal_form_preserves_op_on_overflow` (`u64::MAX + 1`
  and `7 / 0` keep the `Op` shape),
  `const_expr_normal_form_recurses_into_children` (`(N + 0) * 1 = N`),
  `resolve_normalises_array_size_n_plus_zero_equals_n` (full
  parse-→-resolve round trip pinning `[T; N + 0] == [T; N]`),
  `resolve_normalises_const_arg_arithmetic_with_one_factor`
  (`Vector[Int, 4 * 1] == Vector[Int, 4]`).

  Four pre-existing S8.S2 tests (`resolve_array_size_lowers_*`)
  were rewritten to pin the post-fold `ConstExpr::Lit(n)` shape
  rather than the unfolded `Op` shape — eval round-trip
  assertions retained so any future representation change is
  still caught.  Spec stage map updated to mark S8 shipped;
  remaining tier-2 work is the S9 `where`-clause const
  predicates.
- Phase 3 #07.S8.S3: arithmetic in const-arg position
  (`Vector[Int, 2 + 3]`).  Parser's `parse_generic_arg` looks one
  token ahead after an `IntLiteral`; if a binary arithmetic op
  (`+ - * /`) follows, the whole expression is captured via
  `parse_expression` and emitted as the new
  `TypeExpr::ConstExprArg { expr: Box<Expr>, span }` AST variant.
  Bare literals (`Vector[Int, 4]`) continue to emit `ConstLit`
  unchanged — backwards-compat fast path preserved.  Resolve folds
  `ConstExprArg` through the existing
  `lower_const_expr_from_expr` helper that S8.S2 added for
  `[T; expr]` array sizes, so const-arg arithmetic produces
  identically-shaped `Ty::ConstArg(ConstExpr::Op(...))` HIR.  The
  kind-check now treats both `ConstLit` and `ConstExprArg` as
  const-kind args; a kind mismatch (const arithmetic landing in a
  type slot) still emits E0704 with the diagnostic message
  refined to say "found const expression" rather than "found const
  literal".  Pin tests in `const_generics.rs`:
  `parse_const_arg_arithmetic_emits_const_expr_arg`,
  `parse_const_arg_bare_literal_still_emits_const_lit`,
  `resolve_const_arg_arithmetic_lowers_to_const_expr_op` (full
  parse→resolve→typeck round trip through a function parameter
  annotation), and
  `const_arg_arithmetic_against_type_param_emits_e0704`.  Spec
  stage map updated to reflect S8.S3 shipped; S8 still in flight
  pending the normal-form rewriter (`[T; N + 0] = [T; N]`).
- Three formatter / printer arms also extended so the new
  `ConstExprArg` variant doesn't crash exhaustive matches in
  `parser/printer.rs`, `formatter/comments.rs`, and
  `formatter/format_type.rs`.  The Doc-native printer routes
  through `format_expr_short` for now — a custom Doc layout for
  const-arg arithmetic can come when formatting that surface gets
  style attention.

### Changed
- **Breaking diagnostic-code rename:** const-generic kind-mismatch
  diagnostic moved from **E0700** to **E0704**.  E0700 was originally
  given to this slot by `docs/specs/types/const-generics.spec.md`
  §"Error code reservations", but the typeck iterator-`sum`
  validator already emits E0700 with the "requires `Add`" framing
  — the codes were colliding.  Spec amended to use E0704;
  iterator-`sum` keeps E0700.  Affects:
  - `crates/ruxen-core/src/resolve/mod.rs`: kind-mismatch emit site
    now writes `"E0704"`.
  - `crates/ruxen-core/src/diagnostics/codes.rs`: new `CodeInfo`
    for E0704 ("kind mismatch on const-generic argument") with a
    header comment recording the collision resolution.
  - `crates/ruxen-cli/src/explain.rs`: new `include_str!` row so
    `ruxen explain E0704` works.
  - `docs/errors/E0704.md`: full Why / Example / Fix / Notes /
    Related stub with historical note.
  - Pin test renamed:
    `const_lit_against_type_param_emits_e0700` →
    `const_lit_against_type_param_emits_e0704`.

### Added
- Tier-5 / T5.04 phase 1 follow-up: register reserved const-generic
  diagnostic codes **E0701** (wrong const-arg type), **E0702**
  (non-const expression in const-arg position), and **E0703**
  (const-arg expression overflows during evaluation) per
  `docs/specs/types/const-generics.spec.md` §"Error code
  reservations".  Each gets a `CodeInfo` entry in
  `codes::REGISTRY`, an `include_str!` row in
  `ruxen-cli/src/explain.rs::EXPLAINS`, and a
  `docs/errors/<code>.md` page describing the (planned) trigger,
  example, fix, and relationship to sibling codes.  No emit site
  yet — the reservations document where future S8 / S9 work will
  surface eval errors (overflow / div-zero — see the §B8 mapping
  noted in `E0703.md`) and where the S5 kind-mismatch path will
  split into separate kind vs. type diagnostics.

  Also adds two reciprocal pin tests to
  `tests/error_code_registry.rs` so the registry, the
  `EXPLAINS` table, and `docs/errors/` stay in three-way sync:
  `every_registered_error_code_has_a_docs_page` (no registry
  entry without a docs page) and
  `every_docs_error_page_has_a_registry_entry` (no orphan
  docs page without a registry entry).  Catches the drift
  pattern that left E0700 with two simultaneous meanings (an
  open follow-up noted in the registry header).

  Stale `codegen/layout.rs::layout_of` comment updated to reflect
  S8.S1 evaluator semantics — `Op` arithmetic now folds via
  checked `u64` ops, and overflow / div-zero fold to size 0 here
  pending the monomorphization-side wiring that will surface them
  as E0703.
- Phase 3 stdlib remainder closure: docs/specs/stdlib/hash.spec.md
  backfilled.  Spec covers the `Hashable` mixin surface, the
  auto-synth path for `Hashable`, the `T: Hashable` generic bound
  dispatch (working today for user types), the `Map` /
  `Set` key-validity gate (`ty_is_valid_hash_key`), and the
  primitive runtime hashing (`ruxen_hash_bits`, `ruxen_hash_str`).
  Documents the v2 gap on user-callable `.hash_code` for primitives
  and on `T: Hashable` monomorphisation for primitive `T` (link
  fails today with `_T: Hashable_hash_code`); a new `#[ignore]`
  pin test in `implicit_mixin_dispatch.rs::primitive_int_and_string_
  dispatch_through_hashable_bound` documents the gap so future v2
  work can flip the `#[ignore]` off.

  Phase 3 stdlib task 236fa21f's other listed items are deferred to
  v2 per their canonical specs: `fs::metadata` (needs a struct
  surface, `fs.spec.md` §"Out of scope"), `env::set_var` (write-side
  helpers, `env.spec.md` §"Out of scope"), and the full
  `Command` builder (`process.spec.md` §"Out of scope").  v1 ships
  `process_run` for one-shot invocations.
- Phase 3 #07.S8.S2: source-level arithmetic in `[T; <expr>]`
  array-size positions.  The parser already accepted any expression
  in that slot via `parse_expression`; resolve now folds
  `BinaryOp { op: Add | Sub | Mul | Div, ... }` recursively into
  `ConstExpr::Op` trees rather than collapsing them to
  `ConstExpr::Error`.  `Identifier(name)` legs preserve as
  `ConstExpr::Param` (S3 wiring) so `[T; N + 1]` works inside a
  `const N: USize` parameter scope.  Other binary ops (`%`,
  comparisons, `&&`, bit / shift ops) still fall through to
  `ConstExpr::Error`, surfacing as `ConstEvalError::Malformed` if
  evaluated — the spec reserves `+ - * /` for v1 const generics.
  Pin tests in `const_generics.rs`: parse/resolve of `[Int; 2+3]`,
  every operator individually with eval round-trip, paren grouping
  vs. operator precedence (`(2+3)*4 = 20` vs `2 + 3*4 = 14`), const
  param reference inside arithmetic (`[T; N + 1]`), and the `%`
  fallback path.  Const-arg position (`Vector[Int, 2 + 3]`) is the
  S8.S3 follow-up — `parse_generic_arg` still only accepts a bare
  integer literal.
- Phase 3 #07.S8.S1: `ConstExpr::eval` now implements the `Op` branch
  for `+ - * /` against `u64` bindings.  Inner sub-trees are recursed
  on first, so unresolved-param / parser-recovery errors propagate
  unchanged.  Checked arithmetic on `+ - *` surfaces wrap-around as
  `ConstEvalError::Overflow`; `u64` borrow on `Sub` (`0 - 1`) also
  surfaces as `Overflow` — the spec reserves a single
  `E-CONST-OVERFLOW` slot.  `_ / 0` surfaces as
  `ConstEvalError::DivisionByZero` (E-CONST-DIV-ZERO).  Pin tests in
  `const_generics.rs` cover basic add/sub/mul/div, mixed
  param-on-left, nested precedence (`(2+3)*4` and right-grouped
  `2+(3*4)`), every overflow direction, both error propagations,
  and `[Int; 2+2]` round-tripping through `layout_of`.  Parser
  acceptance of `+ - * /` in const-arg position is the S8.S2
  follow-up.
- Phase 2 #06.D4: `FormatSpec` (width / align / fill / precision) is
  applied at runtime.  `lower_interpolation` emits
  `Formatter_new_with_spec(w, p, a, f)` when the lex-captured spec is
  non-default; `Formatter_buffer` pads with the requested fill char /
  alignment at finalize.  Float precision routes through
  `Float_to_string_prec` (snprintf `%.*f`); String precision routes
  through `String_truncate_chars` (UTF-8 char-count truncate); Int /
  Bool / Char ignore precision per Rust semantics.  Strings with a
  non-default spec are no longer short-circuited by the legacy
  string-like pass-through — they fall through to `String_fmt` so
  width / precision / align / fill all apply.  Covered by 7 new
  `stdlib_fmt_runtime.rs` tests and E2E fixture
  `tests/release-e2e/cases/071_interp_format_specs.rx`.  Out of
  scope (deferred to v2): width on `"#{x:?}"` debug-spec
  interpolation (debug path still bypasses the Formatter); sign /
  `#` alternate / `0` zero-pad / radix flags.
- Phase 2 #06.D2.S4: end-to-end fixture
  `tests/release-e2e/cases/070_interp_display_dispatch.rx` proves the
  Display dispatch path runs at runtime — a `class Money` whose body
  `include Display`s and supplies `def fmt` interpolates via `"#{m}"`,
  while inner `"#{self.cents}"` routes through synth `Int_fmt`.
  Mirrored as an inline cargo test
  (`stdlib_fmt_runtime.rs::interpolation_user_impl_display_money_round_trips`)
  so the default `cargo test --workspace` run exercises it without
  waiting for the `--ignored` `release_e2e_all_fixtures` harness.
- Phase 2 #06.D2.S3: route string interpolation through `Display::fmt`
  dispatch — `Formatter_new` → `{T}_fmt(value, fmt)` → `Formatter_buffer`
  for primitives (Char / Int / Float / Bool) and any type whose body
  `include Display`s with a user `def fmt`. Output is byte-identical
  to the legacy `ruxen_*_to_string` path because the Stage 1 synth
  fns wrap the same helpers. Auto-`Debug`-only types still fall back
  to `{Name}_to_debug` until users provide their own `Display`
  implementation. Closes prompt 06's "string interpolation routes
  through Display::fmt" DoD bullet.

  Also lands a small Cranelift-codegen fix: `coerce_call_args` now
  consults a `user_fn_param_tys` side-table (populated during Pass 0/1)
  when computing the call-site argument coercion. Without this, a narrow
  `Bool`(i8) argument to the new synth `Bool_fmt` would be unconditionally
  widened to i64 by the default fallback rule and fail Cranelift IR
  verification. Runtime helpers still flow through `runtime_signature`.
- Phase 2 #06.D2.S2: add `user_has_impl_display(ty) -> Option<String>`
  helper on the MIR lowerer for the Stage-3 interpolation rewrite.
  Walks the HIR program's `impl_blocks` (via the `trait_impls` map
  collected at lowering start; both are the Rust-side internal name
  for the user-visible `include` directive's record) and resolves
  through Ref / RefMut / Alias / Newtype. No call site uses the
  helper yet — pure plumbing.

  Also bundles four S1 review follow-ups: harden seven `functions[0]`
  index lookups in `mir/tests.rs` to `.find(|f| f.name == ...)`;
  strengthen `synth_primitive_fmt_functions_emitted` to assert
  `params.len() == 2` + `return_ty == Ty::Unit` + non-empty body;
  document "why unconditional emission" + transparent String pass
  in `mir/lower.rs`.
- Phase 2 #06.D2.S1: synthesize primitive `Display::fmt` MIR
  functions — `Char_fmt` / `Int_fmt` / `Float_fmt` / `Bool_fmt` /
  `String_fmt` are emitted at program lowering and wrap the existing
  `ruxen_<prim>_to_string` helpers via `Formatter_write_str`. No
  interpolation call site is rewritten yet (Stage 3 will switch
  `lower_interpolation`). Bundles three S0 follow-ups: leak-tracker
  visibility for `ruxen_fmt_formatter_free`; isolated pin tests for
  `Formatter.write_char` (ASCII) + `Formatter.len`; phase-designator
  alignment in the `write_char` placeholder.
- Phase 2 #06.D2.S0: land the `ruxen_fmt_formatter_*` C runtime
  implementations (`new` / `free` / `write_str` / `write_char` /
  `buffer` / `len`) that Phase A's CHANGELOG referenced but never
  committed. `_free` uses `_ORIG_FREE` sentinel + asm-label rebind
  (same pattern as `ruxen_string_free` / `ruxen_vec_free`); `_buffer`
  transfers buffer ownership and self-frees the Formatter struct.
  Also adds `"Formatter"` to the MIR lowerer's built-in constructor
  special-case list so `Formatter.new()` emits `Formatter_new` rather
  than `Alloc + Formatter_init`. Pin test: `stdlib_fmt_runtime.rs` —
  round-trips `Formatter.new()` + `.write_str` + `.buffer` end-to-end.
- Phase 2 stdlib `std::fmt` foundation surface (#06 fmt MVP, plan
  in `docs/superpowers/plans/2026-05-10-stdlib-fmt.md`):
  - **Phase A** — Display/Debug formal mixins registered with a
    reading `def fmt(f: &mut Formatter) -> Result[(), FmtError]`
    signature. `Formatter` and `FmtError` registered as built-in
    classes. Runtime: `RuxenFormatter { char* buf; size_t len, cap }`
    plus `ruxen_fmt_formatter_{new,free,write_str,write_char,buffer,len}`
    helpers; v1 always returns `Ok(0)` from write_*. A user
    `class T ... include Display ... def fmt ... end ... end`
    parses and typechecks.
  - **Phase B** — Format-spec lexing for `"#{x:spec}"`. The lexer
    captures `[fill align] [width] ['.' precision] ['?']` into a
    `FormatSpec` struct on `StringPart::Expr`, threaded through HIR
    `HirInterpolationPart::Expr { expr, spec }` and into MIR.
    Examples: `"#{x:?}"`, `"#{x:>10}"`, `"#{pi:.2}"`,
    `"#{x:*<10.3?}"`. Five lexer pin-tests; printer + formatter
    round-trip non-default specs.
  - **Phase C+D MVP** — `:?` typechecks for auto-synth `Debug` types
    and width/precision/align specs typecheck on numeric types; existing
    `_to_debug` synthesis on auto-Debug structs already produces
    the expected output. Full `lower_interpolation` refactor through
    `Display::fmt` (Phase D2) deferred to a follow-up session.
- Phase 2 stdlib `Iterator` typing for closure-based `*Iter.map` /
  `*Iter.filter` chains (#05 follow-up). Typeck now seeds closure
  parameters from iterator `Item` for `*Iter.map` and `*Iter.filter`,
  and `*Iter.map` rewrites the iterator item type to the closure-body
  result so downstream `.collect_vec` and `Array` methods see the
  mapped element type instead of the source one. New focused unit
  coverage in `crates/ruxen-core/tests/stdlib_iterator.rs` pins
  `v.iter.filter { ... }.count` and cross-type
  `v.iter.map { |n| "#{n}" }.collect_vec.join(",")`.
- Phase 2 stdlib `std::io` no-Result print conveniences (#06.1 partial):
  `Stdout.print(s)`, `Stdout.println(s)`, `Stderr.eprint(s)`, and
  `Stderr.eprintln(s)`. The `*ln` variants emit a literal `\n` after the
  user's text. Failures are silently swallowed — Rust-style
  panic-on-broken-pipe is a v1 simplification omission, the
  matching `write_str` / `flush` on the same handles still surface the
  IoError when explicit handling is desired. Four new C runtime fns
  (`ruxen_stdout_print`, `ruxen_stdout_println`, `ruxen_stderr_eprint`,
  `ruxen_stderr_eprintln`) wired through `codegen/runtime.rs` plus
  matching method-type entries in `typeck/infer.rs`. New integration
  tests in `crates/ruxen-core/tests/stdlib_io.rs` (4 tests) pin the
  per-stream stdout / stderr routing and the with/without-newline
  contract. `IoError` enum migration (the second half of prompt 06.1)
  remains deferred — it is a runtime layout change touching every
  Result-returning fn.
- Phase 2 stdlib `std::env` / `std::fs` additions (#06 partial): `env.vars()`
  snapshots the process environment into `Map[String, String]` (walks
  `extern char **environ`, splits at first `=`, heap-copies both halves
  via `ruxen_string_from`); `env.current_dir()` returns
  `Result[String, IoError]` via `getcwd` with a growing buffer;
  `fs.is_file(path)` and `fs.is_dir(path)` consult `stat()` and return
  `Bool` (matching `fs.exists`'s "false on error" convention so they slot
  into `if` predicates without `?`); `fs.read_dir(path)` returns
  `Result[Array[String], IoError]` of the directory entry names, skipping
  `.` and `..`. Five new C runtime fns (`ruxen_env_vars`,
  `ruxen_env_current_dir`, `ruxen_fs_is_file`, `ruxen_fs_is_dir`,
  `ruxen_fs_read_dir`) wired through `codegen/runtime.rs` and registered
  in the `std::env` / `std::fs` builtin modules. New integration tests in
  `crates/ruxen-core/tests/stdlib_env.rs` (3 tests) and
  `crates/ruxen-core/tests/stdlib_fs.rs` (4 tests) compile inline Ruxen
  programs and run them against staged temp dirs / sentinel env vars.
  Outstanding from prompt 06: `env.vars` value extraction via
  `Option[&String].get` interpolates the raw pointer (pre-existing v1
  limitation — slated for the `Display` interpolation refactor in 06.2);
  `fs.metadata` deferred (needs a struct surface).
- Phase 2 stdlib `Map[K, V]` Entry API: `m.entry(K).or_insert(V)` and
  `m.entry(K).or_insert_with { || V }` (#04 final batch). The chain is
  detected and inlined as a single MIR unit — there is no real
  `Entry[K, V]` runtime value, sidestepping the pointer-returning
  two-variant dispatch the prompt-04 deferred note flagged. Lowering
  emits `if !ruxen_hash_contains_key(m, k) { ruxen_hash_insert(m, k, v); }`
  so the lazy-default contract of `or_insert_with` is honoured: the
  closure body only runs on the missing-key path. The chain returns
  `Unit` (v1 simplification — Rust's `&mut V` return is deferred). Typeck
  rejects splitting the chain across statements (`let e = m.entry(k); e.or_insert(v)`)
  with a clear error so users do not silently fall through the lenient
  unknown-method path. New release-e2e fixtures
  `510_map_entry_or_insert.rx` and `511_map_entry_or_insert_with.rx`
  cover the populated, empty, and lazy-default paths; positive
  type-check tests in `crates/ruxen-core/tests/stdlib_map.rs` and
  matching negative tests in `stdlib_map_negatives.rs`.
- Phase 2 stdlib `Iterator` eager terminators on `*Iter` classes
  (#05 batch 1). `vec.iter.sum` and `vec.iter.count` now type-check
  and dispatch to the existing `ruxen_vec_sum` / `ruxen_vec_count`
  runtime helpers — the type-checker previously knew only
  `*Iter.filter` / `*Iter.map`, so any other terminator on an iter
  receiver was rejected by typeck before reaching codegen. New entries
  in `typeck/infer.rs` cover `sum` (returns the iter element type, so
  `Array[Int].iter.sum -> Int` and a future `Array[UInt64].iter.sum -> UInt64`
  flows the right type through use-sites) and `count` (returns
  `USize`, mirroring `Array.count`). Both helpers were already wired
  through `codegen/runtime.rs` for `VecIter` / `VecIntoIter` /
  `SplitIter`, so this is pure typeck plumbing — no new runtime fns,
  no new error codes. New release-e2e fixtures
  `tests/release-e2e/cases/601_iter_sum.rx` and
  `602_iter_count.rx` exercise both the populated and empty-iter
  paths; the empty case verifies the additive-identity surprise
  check from the prompt brief (sum of empty = 0).
  The remaining Iterator surface (`fold`, `all`, `any`, `take(n)`,
  `skip(n)`, `chain`, `zip`, `enumerate`, `collect[FromIterator]`,
  full `mixin Iterator` in stdlib source) is deferred to a follow-up:
  closing it requires either per-method MIR inliners (for the
  closure-takers, mirroring the `inline_each` / `inline_filter`
  template) or new `*Iter`-specific runtime helpers — verifying
  either path requires a 140 s round-trip through the
  `cargo test --test p05_e2e_check` e2e probe (the agent sandbox
  forbids invoking `target/release/ruxenc` directly), which is too
  slow to land the full surface in a single batch.
- Phase 2 stdlib `Iterator` closure terminators + lazy combinators
  on `*Iter` classes (#05 batch 2). Six new methods land:
  closure-taking eager terminators `fold(init) { |acc, item| body }`
  / `all { |item| pred }` / `any { |item| pred }`, and the
  non-closure lazy combinators `take(n: Int)` / `skip(n: Int)` /
  (already-passthrough) `enumerate`. The closure terminators inline
  at MIR via three new helpers in `mir/lower.rs`:
  `inline_fold` (Section 3.7 — seeds an accumulator local from the
  init expression, walks the vec with `ruxen_vec_len` /
  `ruxen_vec_get`, and reassigns the closure's return value back to
  `acc` each step), and `inline_all_any` (Section 3.8 — seeds the
  result with the vacuous answer for the operator and short-circuits
  on the first counter-example, mirroring Rust's `Iterator::all` /
  `::any` semantics — empty iter → `all=true`, `any=false`). The
  lazy combinators `take` / `skip` eager-materialise into a fresh
  `RuxenVec*` via two new C runtime fns `ruxen_vec_take` /
  `ruxen_vec_skip` in `crates/ruxen-core/runtime/runtime.c`
  (clamped n; shallow element copy, matching `ruxen_vec_clone`),
  registered in `RUNTIME_FUNCTIONS` and the LLVM `runtime_decl.rs`
  (Cranelift infers the sig from the call site). New typeck arms
  in `typeck/infer.rs::builtin_method_type` cover all five —
  `fold` returns the resolved init type, `all` / `any` return
  `Bool`, `take` / `skip` return the same `*Iter` for chaining.
  The pre-existing rejection in `codegen/runtime.rs::runtime_name`
  for `take` / `skip` was lifted; `fold` / `all` / `any` stay in
  the rejection list because they never reach codegen (the inliner
  short-circuits before mangle). Updated
  `crates/ruxen-core/tests/codegen_unknown_method_rejected.rs` —
  the canary test that pinned `iter.fold` rejection now pins
  `iter.zip` rejection (still unimplemented — `chain` / `zip` /
  `collect[FromIterator]` are the deferred surface for #05 batch 3).
  New unit-test file `crates/ruxen-core/tests/stdlib_iterator.rs`
  drives the full lex → parse → typeck → MIR → Cranelift codegen
  pipeline in-process (no `cc` link, no temp files, no `ruxenc`
  subprocess) and runs in ~30 ms total across 14 tests; this is
  the primary TDD loop for #05 (the e2e probe remains the
  end-to-end confirmation). Three new release-e2e fixtures
  `603_iter_fold.rx` / `604_iter_all_any.rx` /
  `605_iter_take_skip.rx` confirm runtime behaviour
  (`PASS=208 / 208` on `release_e2e_smoke`). Still deferred to a
  later batch: `chain` / `zip` (need real iterator structs holding
  two sources), `collect[C: FromIterator]` (needs the
  `FromIterator` mixin + include machinery; a v1 `iter.collect_vec`
  shorthand is the planned escape hatch), and lifting the surface
  into a real `.rx` `mixin Iterator` source (needs a stdlib
  loader, not yet built).
- Phase 2 stdlib `Map[K, V]` indexing operator and Hash-key
  constraint negatives (#04 batch 3). `m[k]` now lowers through
  `ruxen_hash_index` and panics with `"hashmap index: missing key"`
  on miss (mirrors `Array[i]` / `ruxen_vec_get_or_panic`). The MIR
  `Index` handler in `mir/lower.rs` was extended to recognise
  `Ty::HashMap(_, _)` and `Ty::Ref(HashMap)` receivers; `infer_index_ty`
  in `typeck/infer.rs` was changed from `Ty::Option(V)` to `V` to
  match the panicking-index surface. Resolver-time validation in
  `resolve/mod.rs::ty_is_valid_hash_key` rejects compound containers
  (`Array`, `Set`, `Map`) as `Map` keys / `Set` elements, emitting
  `E0615` at the type-construction site (parallel to the per-field
  auto-synth validator in `implicit_includes/mod.rs`). New release-e2e fixture
  `tests/release-e2e/cases/509_map_index_op.rx` exercises
  the hit path; six new negative tests in
  `crates/ruxen-core/tests/stdlib_map_negatives.rs`
  (`hashmap_with_non_hash_key_emits_e0615`,
  `hashset_with_non_hash_element_emits_e0615`,
  `hashmap_with_nested_compound_key_emits_e0615`,
  `hashset_of_hashmap_emits_e0615`, plus two accept-path sanity
  checks) pin the typeck-level diagnostic.
- Phase 2 stdlib `Map[K, V]` + `Set[T]` per-element drop
  selectors (#04 batch 2). Five new runtime helpers
  (`ruxen_hash_drop_string_v`, `ruxen_hash_drop_v_string`,
  `ruxen_hash_drop_string_string`, `ruxen_hash_drop_v_vec`,
  `ruxen_set_drop_string`) walk the bucket chains and release the
  heap-owned key/value/element before delegating to the spine free.
  New runtime helper `ruxen_set_free` (paired with `ruxen_set_new`)
  closes the `Set` spine-leak gap that batch 1 deferred. The MIR
  drop-elaboration in `mir/lower.rs::insert_drops` now dispatches on
  `Ty::HashMap(K, V)` and `Ty::Set(T)` to pick the right helper based
  on whether K/V/T own heap. Push-time ownership transfer extended
  to taint BOTH the key (idx 1) and value (idx 2) of
  `ruxen_hash_insert` (and the value of `ruxen_set_insert`) so source
  `String.from(...)` / `Array.new` temps don't double-free with the
  per-element drop walk. Four new leak regression tests in
  `crates/ruxen-core/tests/drop_fixtures.rs`
  (`p04_hashmap_string_to_int_releases_every_key`,
  `p04_hashmap_int_to_string_releases_every_value`,
  `p04_hashmap_string_to_vec_int_releases_every_value`,
  `p04_hashset_string_releases_every_element`).
- Phase 2 stdlib `Map[K, V]` + `Set[T]` full surface (#04). New
  `Map` methods (runtime + Cranelift sig + LLVM extern + dispatch):
  `with_capacity(Int)`, `remove(&K) -> Option[V]`, `clear`, `keys ->
  Array[&K]`, `values -> Array[&V]`, `iter -> Array[&K]`, plus `==` /
  `!=` routed through `ruxen_hash_eq` (mirrors `ruxen_vec_eq` from
  #03). New `Set` methods: `with_capacity`, `remove(&T) -> Bool`,
  `clear`, `iter -> Array[&T]`, set operations
  `union(&Self) -> Set[T]`, `intersection(&Self) -> Set[T]`,
  `difference(&Self) -> Set[T]`, plus `==` via `ruxen_set_eq`.
  Set-op helpers and `Map`
  container-returning helpers (`keys`, `values`, `iter`, `remove`,
  `with_capacity`) are added to the `FRESH_ALLOC_CALLEES` whitelist
  in `mir/lower.rs` so their fresh allocations are dropped at scope
  exit. 13 new release-e2e fixtures at
  `tests/release-e2e/cases/50[1-6]_map_*.rx` and
  `52[1-7]_set_*.rx`; new typecheck-level pin tests in
  `crates/ruxen-core/tests/stdlib_map.rs` and `stdlib_set.rs`.
- Phase 2 stdlib `Array[T]` surface batch 2 (#03): closes the closure
  surface and wires the per-element drop selector. New methods:
  `Array.from_iter(I)` static constructor (runtime fn
  `ruxen_vec_from_iter`, registered as an `Array` static method
  alongside `new` / `with_capacity`); `dedup` (runtime fn
  `ruxen_vec_dedup`, removes consecutive bitwise-equal slots);
  `sort_by(closure)` and `retain(closure)` (inlined at MIR via the
  existing closure-method machinery; `sort_by` lowers to a
  selection-sort druxen by the user comparator, `retain` lowers to
  a read-write cursor loop using the new runtime helper
  `ruxen_vec_set`). MIR drop-elaboration now selects per-element
  drop helpers based on element type: `Array[String]` →
  `ruxen_vec_drop_string`, `Array[Array[T]]` → `ruxen_vec_drop_vec`,
  primitives still use the spine-only `ruxen_vec_free`. Push-time
  ownership transfer (`ruxen_vec_push` / `ruxen_vec_insert` /
  `ruxen_hash_insert`) now taints the value-arg local so the drop
  pass does not double-free the heap that the receiving slot
  inherits. Three new release-e2e fixture pairs at
  `tests/release-e2e/cases/40[9-11]_*` plus two new drop-leak
  regression tests in `crates/ruxen-core/tests/drop_fixtures.rs`
  (`vec_of_string_releases_every_element`,
  `vec_of_vec_int_releases_every_inner_vec`). New typeck negatives
  pinning `dedup`, `retain`, `sort_by`, and `from_iter` contracts.
  New developer-facing reference at
  `docs/dev/array_iter_borrow_rules.md` documenting the receiver-mode
  rules that statically reject iterator–mutator interleavings.
- Phase 2 stdlib `Array[T]` surface batch 1 (#03): new constructors and
  inspectors `Array.with_capacity(Int)`, `capacity`; new mutators
  `clear`, `truncate(Int)`, `swap(Int, Int)`, `insert(Int, T)`,
  `remove(Int) -> T`, `extend(&Array[T])`; new conversions / iter
  surface stubs `as_slice`, `iter_mut` (both passthrough on the v1
  RuxenVec representation); operator wiring `Array[T] == Array[T]` /
  `!=` (routed through `ruxen_vec_eq`, replacing the prior
  pointer-compare); indexing `v[i]` (routed through
  `ruxen_vec_get_or_panic`, panic message `"index N out of range,
  len M"`). Per-element drop helpers `ruxen_vec_drop_string` and
  `ruxen_vec_drop_vec` ship as runtime fns ready for the drop
  selector wiring (closes the runtime half of the `Array[String]`
  spine-only limitation; full MIR selector lands in batch 2). Eight
  new release-e2e fixture pairs at `tests/release-e2e/cases/40[1-8]_*`.
  New negatives suite `crates/ruxen-core/tests/stdlib_array_negatives.rs`
  pinning the typecheck contract for `Array[i]`, `Array.pop -> Option`,
  `Array[T] == Array[T] -> Bool`, plus two TODO-tagged tests recording
  current typeck laxness for `Array.from(_)` / non-Int args to
  `with_capacity`.
- Phase 2 stdlib `String` surface batch 2 (#02): closes the surface
  gap left by batch 1. New runtime fns `ruxen_string_split`,
  `ruxen_string_push`, `ruxen_string_into_bytes` (registered in the
  Cranelift signatures, LLVM externs, and `RUNTIME_FUNCTIONS`
  dispatch). New language wiring: `String.split(&str) -> Array[String]`
  (standalone, distinct from `splitn`); `String.push(Char)` (now
  routed through the dedicated runtime fn so the codepoint
  intermediate doesn't leak); `String.into_bytes -> Array[UInt8]` (the
  consuming variant of `bytes` — frees the source spine internally
  and the dealloc-safety analysis taints the receiver to avoid a
  double-free); `String + String` (concat owned, reuses
  `ruxen_string_concat`); `String += String` (push_str-style
  in-place, reuses `ruxen_string_push_str`). Five new release-e2e
  fixture pairs at `tests/release-e2e/cases/31[4-8]_*`. Three new
  drop-leak regression tests in `crates/ruxen-core/tests/drop_fixtures.rs`
  (`string_push_does_not_leak`, `string_into_bytes_transfers_ownership`,
  `string_plus_op_frees_both_operands`). New negatives suite
  `crates/ruxen-core/tests/stdlib_string_negatives.rs` pinning the
  typecheck gaps around static-method arg validation and
  borrow-after-move on owned String args.
- Phase 2 stdlib `String` surface batch 1 (#02): new constructors
  `String.new` and `String.with_capacity(Int)`; new inspectors
  `as_str`, `bytes`, `find(&str) -> Option[Int]`, `splitn(Int, &str)`,
  `trim_start`, `trim_end`; new mutators `clear`, `truncate(Int)`,
  `insert(Int, Char)`, `insert_str(Int, &str)`, `remove(Int) -> Char`;
  new conversions `to_string`, `parse[Int] -> Result[Int, ParseIntError]`,
  `parse[Float] -> Result[Float, ParseFloatError]`. Each method has a
  matching `ruxen_string_<op>` runtime fn (declared in
  `runtime.c`, registered in Cranelift+LLVM signatures and
  `RUNTIME_FUNCTIONS`), MIR dispatch in `mir/lower.rs`, and a
  release-e2e fixture pair under `tests/release-e2e/cases/3NN_*`.
  Surface gap remaining for batch 2: `split` (standalone), `push(Char)`,
  `into_bytes`, `+` and `+=` operators.
- `ruxen explain ECODE` subcommand — looks up a compiler error code
  in the central registry and prints its title (T5.04 phase 2).
- Long-form `ruxen explain ECODE` output: every registered code now
  has a markdown file under `docs/errors/<code>.md` (Why / Example /
  Fix sections) embedded into the binary via `include_str!`. The
  CLI prints the full explanation when available and falls back to
  title-only with a note when not. A registry-coverage test enforces
  that the markdown table stays in sync with
  `ruxen_core::diagnostics::codes::REGISTRY` (T5.04 phase 3, #01-C).
- `crates/ruxen-core/src/diagnostics/codes.rs` — central registry
  for every emitted compiler error code (T5.04 phase 1).
- CI workflow with build/test/MSRV gating + advisory lint job
  (T4.06 phase 1).
- `CONTRIBUTING.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md` (T4.08).
- Drop elaboration: heap-owned locals freed on reassignment and at
  every loop-exit edge (break, continue, back-edge) (P0.2).
- Registered auto-synth error codes E0601, E0603, E0605, E0608, E0609
  (previously emitted by `implicit_includes/mod.rs` but missing from the public
  registry) and reserved E0610, E0611, E0613, E0615, E0616, E0617,
  E0618 ahead of T1.05 auto-synth work (#01-B1).
- Auto-synthesized `Clone` now produces a working `<Type>_clone` for
  structs, classes, and enums (including variants with payloads).
  Per-field clone dispatches to bitwise copy for primitives,
  `ruxen_string_clone` for `String`, `ruxen_vec_clone` /
  `ruxen_hash_clone` / `ruxen_set_clone` for built-in containers, and
  recursively to `<Inner>_clone` for user types that themselves
  satisfy `Clone`. Non-`Clone` field/payload types are rejected with
  `E0610` at validation time (T1.05 #01-B2).
- Remaining built-in auto-synth mixins — `Debug`, `PartialEq`, `Eq`,
  `Hash`, `Default`, `Ord`, `PartialOrd`, `Copy` — now produce
  working code rather than validate-only skeletons (T1.05 #01-B3):
  struct ordering operators (`<`, `<=`, `>`, `>=`) dispatch through
  the synthesised `<Type>_cmp` / `<Type>_partial_cmp` so multi-field
  structs are ordered lex-tuple style instead of pointer-compared.
  Per-field bound checks emit `E0613` (PartialEq), `E0615` (Hash),
  `E0617` (Ord), and `E0618` (PartialOrd) when an inner field type
  doesn't satisfy the mixin, and `E0616` fires when `Default` is
  auto-synthesized on an empty enum. Six new release-e2e fixtures
  (`tests/release-e2e/cases/201`, `206`, `207`, `208`, `209`)
  exercise the green path; five negatives in
  `crates/ruxen-core/tests/implicit_negatives.rs` pin the red path.

### Fixed
- Heap-owned `String`, `Array`, and `Map` locals are now freed on
  scope exit (P0.7). Previously these types leaked until program
  exit; drop elaboration only released `Class`/`Struct`/`Enum`
  storage. Three new runtime helpers (`ruxen_string_free`,
  `ruxen_vec_free`, `ruxen_hash_free`) release the spine of each
  owning type. (#01-A)
- Process-level argv copy from `ruxen_env_init` is released via
  `atexit` so leak-tracking test harnesses see a clean exit ledger.

### Known limitations
- `Array[String]` and `Map[K, V]` with heap-owned element types
  only free the spine (data buffer + outer struct). Element heap is
  leaked; recursive element drops are deferred to a future prompt.
  *Update (#04 batch 2):* `Map[String, V]`, `Map[K, String]`,
  `Map[String, String]`, `Map[K, Array[T]]`, and
  `Set[String]` now release every owned key/value/element via
  the per-element drop selectors `ruxen_hash_drop_string_v` /
  `ruxen_hash_drop_v_string` / `ruxen_hash_drop_string_string` /
  `ruxen_hash_drop_v_vec` / `ruxen_set_drop_string`. Deeper nesting
  (Map-in-Map, Set-in-V, etc.) is still spine-only and
  lands with the mixin-druxen drop dispatch in #05.

### Changed
- `LoopFrame` now tracks `body_locals` for the drop pass.
- Un-reserved keyword `spawn` (`actor`, `send`, `receive` were
  un-reserved in pre-Phase-1 work). Decision: ship the async-only
  path; actors are deferred to v2 as a library, then language v2.0.
  `async` / `await` remain reserved for Phase 4. (P0.12)

## [0.1.0] — 2026-04-23

Initial public preview. See `docs/requirements/ROADMAP.md` for the
roadmap to 1.0.
