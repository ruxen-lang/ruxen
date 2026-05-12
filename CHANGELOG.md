# Changelog

All notable changes to this project will be documented here. The
format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
once 1.0.0 ships.

## [Unreleased]

### Added
- Phase 2 #06.D2.S0: land the `riven_fmt_formatter_*` C runtime
  implementations (`new` / `free` / `write_str` / `write_char` /
  `buffer` / `len`) that Phase A's CHANGELOG referenced but never
  committed. `_free` uses `_ORIG_FREE` sentinel + asm-label rebind
  (same pattern as `riven_string_free` / `riven_vec_free`); `_buffer`
  transfers buffer ownership and self-frees the Formatter struct.
  Also adds `"Formatter"` to the MIR lowerer's built-in constructor
  special-case list so `Formatter.new()` emits `Formatter_new` rather
  than `Alloc + Formatter_init`. Pin test: `stdlib_fmt_runtime.rs` —
  round-trips `Formatter.new()` + `.write_str` + `.buffer` end-to-end.
- Phase 2 stdlib `std::fmt` foundation surface (#06 fmt MVP, plan
  in `docs/superpowers/plans/2026-05-10-stdlib-fmt.md`):
  - **Phase A** — Display/Debug formal traits registered with
    `fmt(&self, &mut Formatter) -> Result[(), FmtError]` signature.
    `Formatter` and `FmtError` registered as built-in classes.
    Runtime: `RivenFormatter { char* buf; size_t len, cap }` plus
    `riven_fmt_formatter_{new,free,write_str,write_char,buffer,len}`
    helpers; v1 always returns `Ok(0)` from write_*. User `class T ...
    impl Display ... end ... end` parses and typechecks.
  - **Phase B** — Format-spec lexing for `"#{x:spec}"`. The lexer
    captures `[fill align] [width] ['.' precision] ['?']` into a
    `FormatSpec` struct on `StringPart::Expr`, threaded through HIR
    `HirInterpolationPart::Expr { expr, spec }` and into MIR.
    Examples: `"#{x:?}"`, `"#{x:>10}"`, `"#{pi:.2}"`,
    `"#{x:*<10.3?}"`. Five lexer pin-tests; printer + formatter
    round-trip non-default specs.
  - **Phase C+D MVP** — `:?` typechecks for derive Debug types and
    width/precision/align specs typecheck on numeric types; existing
    `_to_debug` synthesis on derive-Debug structs already produces
    the expected output. Full `lower_interpolation` refactor through
    `Display::fmt` (Phase D2) deferred to a follow-up session.
- Phase 2 stdlib `Iterator` typing for closure-based `*Iter.map` /
  `*Iter.filter` chains (#05 follow-up). Typeck now seeds closure
  parameters from iterator `Item` for `*Iter.map` and `*Iter.filter`,
  and `*Iter.map` rewrites the iterator item type to the closure-body
  result so downstream `.collect_vec` and Vec methods see the mapped
  element type instead of the source one. New focused unit coverage in
  `crates/riven-core/tests/stdlib_iterator.rs` pins
  `v.iter.filter { ... }.count` and cross-type
  `v.iter.map { |n| "#{n}" }.collect_vec.join(",")`.
- Phase 2 stdlib `std::io` no-Result print conveniences (#06.1 partial):
  `Stdout.print(s)`, `Stdout.println(s)`, `Stderr.eprint(s)`, and
  `Stderr.eprintln(s)`. The `*ln` variants emit a literal `\n` after the
  user's text. Failures are silently swallowed — Rust-style
  panic-on-broken-pipe is a v1 simplification omission, the
  matching `write_str` / `flush` on the same handles still surface the
  IoError when explicit handling is desired. Four new C runtime fns
  (`riven_stdout_print`, `riven_stdout_println`, `riven_stderr_eprint`,
  `riven_stderr_eprintln`) wired through `codegen/runtime.rs` plus
  matching method-type entries in `typeck/infer.rs`. New integration
  tests in `crates/riven-core/tests/stdlib_io.rs` (4 tests) pin the
  per-stream stdout / stderr routing and the with/without-newline
  contract. `IoError` enum migration (the second half of prompt 06.1)
  remains deferred — it is a runtime layout change touching every
  Result-returning fn.
- Phase 2 stdlib `std::env` / `std::fs` additions (#06 partial): `env.vars()`
  snapshots the process environment into `HashMap[String, String]` (walks
  `extern char **environ`, splits at first `=`, heap-copies both halves
  via `riven_string_from`); `env.current_dir()` returns
  `Result[String, IoError]` via `getcwd` with a growing buffer;
  `fs.is_file(path)` and `fs.is_dir(path)` consult `stat()` and return
  `Bool` (matching `fs.exists`'s "false on error" convention so they slot
  into `if` predicates without `?`); `fs.read_dir(path)` returns
  `Result[Vec[String], IoError]` of the directory entry names, skipping
  `.` and `..`. Five new C runtime fns (`riven_env_vars`,
  `riven_env_current_dir`, `riven_fs_is_file`, `riven_fs_is_dir`,
  `riven_fs_read_dir`) wired through `codegen/runtime.rs` and registered
  in the `std::env` / `std::fs` builtin modules. New integration tests in
  `crates/riven-core/tests/stdlib_env.rs` (3 tests) and
  `crates/riven-core/tests/stdlib_fs.rs` (4 tests) compile inline Riven
  programs and run them against staged temp dirs / sentinel env vars.
  Outstanding from prompt 06: `env.vars` value extraction via
  `Option[&String].get` interpolates the raw pointer (pre-existing v1
  limitation — slated for the `Display` interpolation refactor in 06.2);
  `fs.metadata` deferred (needs a struct surface).
- Phase 2 stdlib `HashMap[K,V]` Entry API: `m.entry(K).or_insert(V)` and
  `m.entry(K).or_insert_with { || V }` (#04 final batch). The chain is
  detected and inlined as a single MIR unit — there is no real
  `Entry[K,V]` runtime value, sidestepping the pointer-returning
  two-variant dispatch the prompt-04 deferred note flagged. Lowering
  emits `if !riven_hash_contains_key(m, k) { riven_hash_insert(m, k, v); }`
  so the lazy-default contract of `or_insert_with` is honoured: the
  closure body only runs on the missing-key path. The chain returns
  `Unit` (v1 simplification — Rust's `&mut V` return is deferred). Typeck
  rejects splitting the chain across statements (`let e = m.entry(k); e.or_insert(v)`)
  with a clear error so users do not silently fall through the lenient
  unknown-method path. New release-e2e fixtures
  `510_hashmap_entry_or_insert.rvn` and `511_hashmap_entry_or_insert_with.rvn`
  cover the populated, empty, and lazy-default paths; positive
  type-check tests in `crates/riven-core/tests/stdlib_hashmap.rs` and
  matching negative tests in `stdlib_hashmap_negatives.rs`.
- Phase 2 stdlib `Iterator` eager terminators on `*Iter` classes
  (#05 batch 1). `vec.iter.sum` and `vec.iter.count` now type-check
  and dispatch to the existing `riven_vec_sum` / `riven_vec_count`
  runtime helpers — the type-checker previously knew only
  `*Iter.filter` / `*Iter.map`, so any other terminator on an iter
  receiver was rejected by typeck before reaching codegen. New entries
  in `typeck/infer.rs` cover `sum` (returns the iter element type, so
  `Vec[Int].iter.sum -> Int` and a future `Vec[U64].iter.sum -> U64`
  flows the right type through use-sites) and `count` (returns
  `USize`, mirroring `Vec.count`). Both helpers were already wired
  through `codegen/runtime.rs` for `VecIter` / `VecIntoIter` /
  `SplitIter`, so this is pure typeck plumbing — no new runtime fns,
  no new error codes. New release-e2e fixtures
  `tests/release-e2e/cases/601_iter_sum.rvn` and
  `602_iter_count.rvn` exercise both the populated and empty-iter
  paths; the empty case verifies the additive-identity surprise
  check from the prompt brief (sum of empty = 0).
  The remaining Iterator surface (`fold`, `all`, `any`, `take(n)`,
  `skip(n)`, `chain`, `zip`, `enumerate`, `collect[FromIterator]`,
  full `trait Iterator` in stdlib source) is deferred to a follow-up:
  closing it requires either per-method MIR inliners (for the
  closure-takers, mirroring the `inline_each` / `inline_filter`
  template) or new `*Iter`-specific runtime helpers — verifying
  either path requires a 140 s round-trip through the
  `cargo test --test p05_e2e_check` e2e probe (the agent sandbox
  forbids invoking `target/release/rivenc` directly), which is too
  slow to land the full surface in a single batch.
- Phase 2 stdlib `Iterator` closure terminators + lazy combinators
  on `*Iter` classes (#05 batch 2). Six new methods land:
  closure-taking eager terminators `fold(init) { |acc, item| body }`
  / `all { |item| pred }` / `any { |item| pred }`, and the
  non-closure lazy combinators `take(n: Int)` / `skip(n: Int)` /
  (already-passthrough) `enumerate`. The closure terminators inline
  at MIR via three new helpers in `mir/lower.rs`:
  `inline_fold` (Section 3.7 — seeds an accumulator local from the
  init expression, walks the vec with `riven_vec_len` /
  `riven_vec_get`, and reassigns the closure's return value back to
  `acc` each step), and `inline_all_any` (Section 3.8 — seeds the
  result with the vacuous answer for the operator and short-circuits
  on the first counter-example, mirroring Rust's `Iterator::all` /
  `::any` semantics — empty iter → `all=true`, `any=false`). The
  lazy combinators `take` / `skip` eager-materialise into a fresh
  `RivenVec*` via two new C runtime fns `riven_vec_take` /
  `riven_vec_skip` in `crates/riven-core/runtime/runtime.c`
  (clamped n; shallow element copy, matching `riven_vec_clone`),
  registered in `RUNTIME_FUNCTIONS` and the LLVM `runtime_decl.rs`
  (Cranelift infers the sig from the call site). New typeck arms
  in `typeck/infer.rs::builtin_method_type` cover all five —
  `fold` returns the resolved init type, `all` / `any` return
  `Bool`, `take` / `skip` return the same `*Iter` for chaining.
  The pre-existing rejection in `codegen/runtime.rs::runtime_name`
  for `take` / `skip` was lifted; `fold` / `all` / `any` stay in
  the rejection list because they never reach codegen (the inliner
  short-circuits before mangle). Updated
  `crates/riven-core/tests/codegen_unknown_method_rejected.rs` —
  the canary test that pinned `iter.fold` rejection now pins
  `iter.zip` rejection (still unimplemented — `chain` / `zip` /
  `collect[FromIterator]` are the deferred surface for #05 batch 3).
  New unit-test file `crates/riven-core/tests/stdlib_iterator.rs`
  drives the full lex → parse → typeck → MIR → Cranelift codegen
  pipeline in-process (no `cc` link, no temp files, no `rivenc`
  subprocess) and runs in ~30 ms total across 14 tests; this is
  the primary TDD loop for #05 (the e2e probe remains the
  end-to-end confirmation). Three new release-e2e fixtures
  `603_iter_fold.rvn` / `604_iter_all_any.rvn` /
  `605_iter_take_skip.rvn` confirm runtime behaviour
  (`PASS=208 / 208` on `release_e2e_smoke`). Still deferred to a
  later batch: `chain` / `zip` (need real iterator structs holding
  two sources), `collect[C: FromIterator]` (needs the
  `FromIterator` trait + impl machinery; a v1 `iter.collect_vec`
  shorthand is the planned escape hatch), and lifting the surface
  into a real `.rvn` `trait Iterator` source (needs a stdlib
  loader, not yet built).
- Phase 2 stdlib `HashMap[K,V]` indexing operator and Hash-key
  constraint negatives (#04 batch 3). `m[k]` now lowers through
  `riven_hash_index` and panics with `"hashmap index: missing key"`
  on miss (mirrors `Vec[i]` / `riven_vec_get_or_panic`). The MIR
  `Index` handler in `mir/lower.rs` was extended to recognise
  `Ty::HashMap(_, _)` and `Ty::Ref(HashMap)` receivers; `infer_index_ty`
  in `typeck/infer.rs` was changed from `Ty::Option(V)` to `V` to
  match the panicking-index surface. Resolver-time validation in
  `resolve/mod.rs::ty_is_valid_hash_key` rejects compound containers
  (`Vec`, `Set`/`HashSet`, `HashMap`) as `HashMap` keys / `HashSet`
  elements, emitting `E0615` at the type-construction site (parallel
  to the per-field derive validator in `derive/mod.rs`). New release-e2e
  fixture `tests/release-e2e/cases/509_hashmap_index_op.rvn` exercises
  the hit path; six new negative tests in
  `crates/riven-core/tests/stdlib_hashmap_negatives.rs`
  (`hashmap_with_non_hash_key_emits_e0615`,
  `hashset_with_non_hash_element_emits_e0615`,
  `hashmap_with_nested_compound_key_emits_e0615`,
  `hashset_of_hashmap_emits_e0615`, plus two accept-path sanity
  checks) pin the typeck-level diagnostic.
- Phase 2 stdlib `HashMap[K,V]` + `HashSet[T]` per-element drop
  selectors (#04 batch 2). Five new runtime helpers
  (`riven_hash_drop_string_v`, `riven_hash_drop_v_string`,
  `riven_hash_drop_string_string`, `riven_hash_drop_v_vec`,
  `riven_set_drop_string`) walk the bucket chains and release the
  heap-owned key/value/element before delegating to the spine free.
  New runtime helper `riven_set_free` (paired with `riven_set_new`)
  closes the HashSet spine-leak gap that batch 1 deferred. The MIR
  drop-elaboration in `mir/lower.rs::insert_drops` now dispatches on
  `Ty::HashMap(K, V)` and `Ty::Set(T)` to pick the right helper based
  on whether K/V/T own heap. Push-time ownership transfer extended
  to taint BOTH the key (idx 1) and value (idx 2) of
  `riven_hash_insert` (and the value of `riven_set_insert`) so source
  `String.from(...)` / `Vec.new` temps don't double-free with the
  per-element drop walk. Four new leak regression tests in
  `crates/riven-core/tests/drop_fixtures.rs`
  (`p04_hashmap_string_to_int_releases_every_key`,
  `p04_hashmap_int_to_string_releases_every_value`,
  `p04_hashmap_string_to_vec_int_releases_every_value`,
  `p04_hashset_string_releases_every_element`).
- Phase 2 stdlib `HashMap[K,V]` + `HashSet[T]` full surface (#04). New
  HashMap methods (runtime + Cranelift sig + LLVM extern + dispatch):
  `with_capacity(Int)`, `remove(&K) -> Option[V]`, `clear`, `keys ->
  Vec[&K]`, `values -> Vec[&V]`, `iter -> Vec[&K]`, plus `==` /  `!=`
  routed through `riven_hash_eq` (mirrors `riven_vec_eq` from #03).
  New HashSet methods: `with_capacity`, `remove(&T) -> Bool`, `clear`,
  `iter -> Vec[&T]`, set operations `union(&Self) -> HashSet[T]`,
  `intersection(&Self) -> HashSet[T]`, `difference(&Self) -> HashSet[T]`,
  plus `==` via `riven_set_eq`. `HashSet[T]` is a v1 alias for `Set[T]`
  registered in `resolve::mod`; `HashSet.new` / `HashSet.with_capacity`
  reach the runtime through the same dispatch as `Set.new`. Set-op
  helpers and HashMap container-returning helpers (`keys`, `values`,
  `iter`, `remove`, `with_capacity`) are added to the
  `FRESH_ALLOC_CALLEES` whitelist in `mir/lower.rs` so their fresh
  allocations are dropped at scope exit. 13 new release-e2e fixtures
  at `tests/release-e2e/cases/50[1-6]_hashmap_*.rvn` and
  `52[1-7]_hashset_*.rvn`; new typecheck-level pin tests in
  `crates/riven-core/tests/stdlib_hashmap.rs` and `stdlib_hashset.rs`.
- Phase 2 stdlib `Vec[T]` surface batch 2 (#03): closes the closure
  surface and wires the per-element drop selector. New methods:
  `Vec.from_iter(I)` static constructor (runtime fn
  `riven_vec_from_iter`, registered as a `Vec` static method
  alongside `new` / `with_capacity`); `dedup` (runtime fn
  `riven_vec_dedup`, removes consecutive bitwise-equal slots);
  `sort_by(closure)` and `retain(closure)` (inlined at MIR via the
  existing closure-method machinery; `sort_by` lowers to a
  selection-sort driven by the user comparator, `retain` lowers to
  a read-write cursor loop using the new runtime helper
  `riven_vec_set`). MIR drop-elaboration now selects per-element
  drop helpers based on element type: `Vec[String]` →
  `riven_vec_drop_string`, `Vec[Vec[T]]` → `riven_vec_drop_vec`,
  primitives still use the spine-only `riven_vec_free`. Push-time
  ownership transfer (`riven_vec_push` / `riven_vec_insert` /
  `riven_hash_insert`) now taints the value-arg local so the drop
  pass does not double-free the heap that the receiving slot
  inherits. Three new release-e2e fixture pairs at
  `tests/release-e2e/cases/40[9-11]_*` plus two new drop-leak
  regression tests in `crates/riven-core/tests/drop_fixtures.rs`
  (`vec_of_string_releases_every_element`,
  `vec_of_vec_int_releases_every_inner_vec`). New typeck negatives
  pinning `dedup`, `retain`, `sort_by`, and `from_iter` contracts.
  New developer-facing reference at
  `docs/dev/vec_iter_borrow_rules.md` documenting the receiver-mode
  rules that statically reject iterator–mutator interleavings.
- Phase 2 stdlib `Vec[T]` surface batch 1 (#03): new constructors and
  inspectors `Vec.with_capacity(Int)`, `capacity`; new mutators
  `clear`, `truncate(Int)`, `swap(Int, Int)`, `insert(Int, T)`,
  `remove(Int) -> T`, `extend(&Vec[T])`; new conversions / iter
  surface stubs `as_slice`, `iter_mut` (both passthrough on the v1
  RivenVec representation); operator wiring `Vec[T] == Vec[T]` /
  `!=` (routed through `riven_vec_eq`, replacing the prior
  pointer-compare); indexing `v[i]` (routed through
  `riven_vec_get_or_panic`, panic message `"index N out of range,
  len M"`). Per-element drop helpers `riven_vec_drop_string` and
  `riven_vec_drop_vec` ship as runtime fns ready for the drop
  selector wiring (closes the runtime half of the `Vec[String]`
  spine-only limitation; full MIR selector lands in batch 2). Eight
  new release-e2e fixture pairs at `tests/release-e2e/cases/40[1-8]_*`.
  New negatives suite `crates/riven-core/tests/stdlib_vec_negatives.rs`
  pinning the typecheck contract for `Vec[i]`, `Vec.pop -> Option`,
  `Vec[T] == Vec[T] -> Bool`, plus two TODO-tagged tests recording
  current typeck laxness for `Vec.from(_)` / non-Int args to
  `with_capacity`.
- Phase 2 stdlib `String` surface batch 2 (#02): closes the surface
  gap left by batch 1. New runtime fns `riven_string_split`,
  `riven_string_push`, `riven_string_into_bytes` (registered in the
  Cranelift signatures, LLVM externs, and `RUNTIME_FUNCTIONS`
  dispatch). New language wiring: `String.split(&str) -> Vec[String]`
  (standalone, distinct from `splitn`); `String.push(Char)` (now
  routed through the dedicated runtime fn so the codepoint
  intermediate doesn't leak); `String.into_bytes -> Vec[U8]` (the
  consuming variant of `bytes` — frees the source spine internally
  and the dealloc-safety analysis taints the receiver to avoid a
  double-free); `String + String` (concat owned, reuses
  `riven_string_concat`); `String += String` (push_str-style
  in-place, reuses `riven_string_push_str`). Five new release-e2e
  fixture pairs at `tests/release-e2e/cases/31[4-8]_*`. Three new
  drop-leak regression tests in `crates/riven-core/tests/drop_fixtures.rs`
  (`string_push_does_not_leak`, `string_into_bytes_transfers_ownership`,
  `string_plus_op_frees_both_operands`). New negatives suite
  `crates/riven-core/tests/stdlib_string_negatives.rs` pinning the
  typecheck gaps around static-method arg validation and
  borrow-after-move on owned String args.
- Phase 2 stdlib `String` surface batch 1 (#02): new constructors
  `String.new` and `String.with_capacity(Int)`; new inspectors
  `as_str`, `bytes`, `find(&str) -> Option[Int]`, `splitn(Int, &str)`,
  `trim_start`, `trim_end`; new mutators `clear`, `truncate(Int)`,
  `insert(Int, Char)`, `insert_str(Int, &str)`, `remove(Int) -> Char`;
  new conversions `to_string`, `parse[Int] -> Result[Int, ParseIntError]`,
  `parse[Float] -> Result[Float, ParseFloatError]`. Each method has a
  matching `riven_string_<op>` runtime fn (declared in
  `runtime.c`, registered in Cranelift+LLVM signatures and
  `RUNTIME_FUNCTIONS`), MIR dispatch in `mir/lower.rs`, and a
  release-e2e fixture pair under `tests/release-e2e/cases/3NN_*`.
  Surface gap remaining for batch 2: `split` (standalone), `push(Char)`,
  `into_bytes`, `+` and `+=` operators.
- `riven explain ECODE` subcommand — looks up a compiler error code
  in the central registry and prints its title (T5.04 phase 2).
- Long-form `riven explain ECODE` output: every registered code now
  has a markdown file under `docs/errors/<code>.md` (Why / Example /
  Fix sections) embedded into the binary via `include_str!`. The
  CLI prints the full explanation when available and falls back to
  title-only with a note when not. A registry-coverage test enforces
  that the markdown table stays in sync with
  `riven_core::diagnostics::codes::REGISTRY` (T5.04 phase 3, #01-C).
- `crates/riven-core/src/diagnostics/codes.rs` — central registry
  for every emitted compiler error code (T5.04 phase 1).
- CI workflow with build/test/MSRV gating + advisory lint job
  (T4.06 phase 1).
- `CONTRIBUTING.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md` (T4.08).
- Drop elaboration: heap-owned locals freed on reassignment and at
  every loop-exit edge (break, continue, back-edge) (P0.2).
- Registered derive error codes E0601, E0603, E0605, E0608, E0609
  (previously emitted by `derive/mod.rs` but missing from the public
  registry) and reserved E0610, E0611, E0613, E0615, E0616, E0617,
  E0618 ahead of T1.05 derive synthesizer work (#01-B1).
- `derive Clone` now synthesizes a working `<Type>_clone` for structs,
  classes, and enums (including variants with payloads). Per-field
  clone dispatches to bitwise copy for primitives, `riven_string_clone`
  for `String`, `riven_vec_clone` / `riven_hash_clone` / `riven_set_clone`
  for built-in containers, and recursively to `<Inner>_clone` for
  user types that themselves derive Clone. Non-Clone field/payload
  types are rejected with `E0610` at validation time (T1.05 #01-B2).
- Remaining built-in derives — `Debug`, `PartialEq`, `Eq`, `Hash`,
  `Default`, `Ord`, `PartialOrd`, `Copy` — now produce working code
  rather than validate-only skeletons (T1.05 #01-B3): struct
  ordering operators (`<`, `<=`, `>`, `>=`) dispatch through the
  synthesised `<Type>_cmp` / `<Type>_partial_cmp` so multi-field
  structs are ordered lex-tuple style instead of pointer-compared.
  Per-field bound checks emit `E0613` (PartialEq), `E0615` (Hash),
  `E0617` (Ord), and `E0618` (PartialOrd) when an inner field type
  doesn't satisfy the trait, and `E0616` fires when `Default` is
  derived on an empty enum. Six new release-e2e fixtures
  (`tests/release-e2e/cases/201`, `206`, `207`, `208`, `209`)
  exercise the green path; five negatives in
  `crates/riven-core/tests/derive_negatives.rs` pin the red path.

### Fixed
- Heap-owned `String`, `Vec`, and `HashMap` locals are now freed on
  scope exit (P0.7). Previously these types leaked until program
  exit; drop elaboration only released `Class`/`Struct`/`Enum`
  storage. Three new runtime helpers (`riven_string_free`,
  `riven_vec_free`, `riven_hash_free`) release the spine of each
  owning type. (#01-A)
- Process-level argv copy from `riven_env_init` is released via
  `atexit` so leak-tracking test harnesses see a clean exit ledger.

### Known limitations
- `Vec[String]` and `HashMap[K, V]` with heap-owned element types
  only free the spine (data buffer + outer struct). Element heap is
  leaked; recursive element drops are deferred to a future prompt.
  *Update (#04 batch 2):* `HashMap[String, V]`, `HashMap[K, String]`,
  `HashMap[String, String]`, `HashMap[K, Vec[T]]`, and
  `HashSet[String]` now release every owned key/value/element via
  the per-element drop selectors `riven_hash_drop_string_v` /
  `riven_hash_drop_v_string` / `riven_hash_drop_string_string` /
  `riven_hash_drop_v_vec` / `riven_set_drop_string`. Deeper nesting
  (HashMap-in-HashMap, HashSet-in-V, etc.) is still spine-only and
  lands with the trait-driven drop dispatch in #05.

### Changed
- `LoopFrame` now tracks `body_locals` for the drop pass.
- Un-reserved keyword `spawn` (`actor`, `send`, `receive` were
  un-reserved in pre-Phase-1 work). Decision: ship the async-only
  path; actors are deferred to v2 as a library, then language v2.0.
  `async` / `await` remain reserved for Phase 4. (P0.12)

## [0.1.0] — 2026-04-23

Initial public preview. See `docs/requirements/ROADMAP.md` for the
roadmap to 1.0.
