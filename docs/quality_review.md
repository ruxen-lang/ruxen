# Ruxen — Whole-Project Quality Review

**Date**: 2026-05-22 (original review)
**Re-audited**: 2026-05-22 evening (after the CI-greening + correctness sweep landed)
**Scope**: Full workspace (7 crates, ~26K LOC Rust) reviewed in 8 parallel passes
**Status overall after fixes**: **WARN** (was FAIL). The Wave 1 build/format/lint chunk is shipped; the Wave 2 security boundary in `resolve_deps`+`bench` and most of the Wave 3 soundness gaps remain.

## Status table (updated 2026-05-22 evening)

| Category | Status | Notes |
|---|---|---|
| Formatting | ✅ GREEN | `cargo fmt --all` sweep (74 files) committed in `ae20296`. |
| Build | ✅ GREEN | `ruxen_repl` `MirInst::DataAddr` arm added; `Future_dynamic_poll` weak stub satisfies the libruxenrt link (`ae20296`). gcc-on-linux `atomic_fetch_and(&atomic_bool, …)` rejection fixed with CAS loops (`79be087`). Workspace + MSRV both green on macOS aarch64 and ubuntu/gcc (verified via docker `rust:1.94-bookworm`). |
| Clippy | WARN | 5 `ruxen_ide` lints auto-fixed (`79be087`); 4 `ruxen_core` lints + ~8 across the rest of the workspace remain. CI lint job stays `continue-on-error: true`. |
| Security | **FAIL** | All 4 `resolve_deps.rs` / `bench.rs` holes still open. Listed in Wave 2 below. |
| Performance | WARN | Unchanged from original review. |
| Architecture | WARN | Unchanged. |
| Correctness | **FAIL** → WARN | 5 of the original 12 CRITICAL findings now closed (see "Findings closed" below); 7 still open. |

## Findings closed this session

Map from review section → commit / file:

| Review § | Finding | Fix landed in |
|---|---|---|
| §1.1 | `ruxen_repl/src/jit.rs:731` non-exhaustive `MirInst` match | `ae20296` |
| §1.3 | `resolve/types.rs:516-548` String DefKind-stomp workaround | **Real fix** in `6fbf5d9` — `resolve/items.rs:332` now preserves `DefKind::TypeAlias` instead of overwriting it. Read-side patches in `types.rs` left for defence-in-depth. |
| §10 | `project_ruxen_resolve_class_stomps_typealias.md` | Same as above — memory marked resolved at the write site. |
| Wave 1 #1 | `MirInst::DataAddr` arm | `ae20296` |
| Wave 1 #2 | `cargo fmt` sweep | `ae20296` |
| Wave 1 #3 | Dead `runtime_c_src` helpers | `79be087` |
| Wave 1 #4 | `cargo clippy --fix -p ruxen_ide` (5 auto-fixable) | `79be087` |
| §8 (line_index UTF-8 char-boundary crash, also v1 STATUS.md follow-up) | `ruxen_ide/src/line_index.rs` | `79be087` — added stable `floor_char_boundary` helper, routed both bounds through it. Unblocks `analysis_of_sample_program`. |
| §7 (E1100-E1118 missing markdown — surfaced on linux) | `src/ruxen_cli/src/explain.rs` | `79be087` — 9 `include_str!` lines added. |
| Cross-cutting | gcc rejects `atomic_fetch_and/or` on `atomic_bool` (C11 only defines those for integer atomics; Clang accepts it as an extension) | `79be087` — CAS-loop rewrite. |
| §5 / §1.3 derivative | Command builder `ruxen_command_arg/args/env/current_dir` double-free at scope exit (intermediate and dest both alloc-rooted, both drop+dealloc) | `a398f72` — force-taint receiver in `mir/lower/drops.rs::compute_dealloc_safe_locals`. |
| §4 / §1.3 | `lookup_method_on_bounds` not reachable through `Ref(TypeParam)` receiver | `b840862` — `typeck/infer/collect.rs::lookup_on_type_param_bounds` peels references. |
| §4 / §1.3 | Hashable/Ord/PartialOrd mixin sigs default to `Unit` return | `b840862` — stamped `-> Int` return types in `library/std/core/src/lib.rx` + `library/std/hash/src/lib.rx`. |
| §2 (extension on primitive) | `extension Int { def to_display }` lowered `self: Ty::Unit` because `class_name_from_mangled` only matched user-defined Class/Struct/Enum | `b6abe74` — `primitive_self_ty_from_mangled` fallback in `mir/lower/type_helpers.rs`. |

## Findings still open (priority order)

1. **Wave 2 security boundary** (`src/ruxen_cli/src/resolve_deps.rs` lines 256/269/187, `src/ruxenc/src/bench.rs:142-150`, `src/ruxen_repl/src/jit.rs:178-188`) — argument-injection on `git clone <url>` / `git checkout <ref>`, path-dep traversal, identifier-splice into synthesised `def main`, open `dlsym(RTLD_DEFAULT)` allowlist. All four CVE-class.
2. **Wave 3 soundness gaps** — see the original §1.3 list, minus the entries marked closed above. The four highest-value remaining items:
   - Tuple-field roundtrip through `f64` in `parser/expr/calls.rs:72-96` (`t.0.10` indexes field 1, not 10).
   - `def __drop` never fires in `mir/lower/collect.rs:223+` (`sync.rx` leaks). Memory `project_ruxen_drop_name_mismatch.md`.
   - `mir/lower/derive.rs::synthesize_class_clone` ignores parent-class fields (inherited fields zero-init).
   - `typeck/unify.rs:294-301` Ref auto-deref unsoundness (`&Int` unifies with `Int`).
3. **Open allowlists** — `FRESH_ALLOC_CALLEES` in `mir/lower/drops.rs:412-652` and the ownership-transfer allowlist at `:786-789`. Make them attribute-druxen from FFI decls.
4. **Wave 5 architecture splits** — five god files >1000 LOC (`async_lowering/mod.rs`, `mir/lower/{derive,expr/method_call,mod}.rs`, `resolve/ffi_registration.rs`). None affect correctness; all gate maintenance velocity.

The full original report follows.

---

## How to use this document

Each finding cites an absolute `file:line` and a one-line proposed fix. Findings are organised by severity within each slice. The **Recommended action order** section at the end groups them into landing waves.

When a finding references a memory entry like `project_ruxen_drop_name_mismatch.md`, that's an existing project memory in `/Users/hassan/.claude/projects/-Users-hassan--projects-ruxen/memory/` — confirmed still load-bearing by this review.

---

# 1. Top CRITICAL findings (cross-slice, prioritised)

## 1.1 Build break (fix first)

- **`src/ruxen_repl/src/jit.rs:731`** — `match inst` is non-exhaustive on `&MirInst`: `MirInst::DataAddr { dest, data_sym }` (added in `compiler/ruxen_core/src/mir/nodes.rs:333`) has no arm. Blocks the test target from compiling.
  **Fix**: add the arm; mirror `compiler/ruxen_core/src/codegen/cranelift/emit.rs:460-477` (declare data, materialise address via `module.declare_data_in_func` + `builder.ins().global_value(I64, …)`). Add a tag-stability pin test that destructures every `MirInst` variant.

## 1.2 Security boundary (fix this week)

- **`src/ruxen_cli/src/resolve_deps.rs:256`** — `git clone <git_url>` invokes git with an unvalidated user URL. A `Ruxen.toml` with `git = "--upload-pack=/usr/bin/curl ..."` is CVE-2017-1000117-class argument-injection RCE.
  **Fix**: reject URLs starting with `-`, restrict scheme to `https://`/`ssh://`/`git@`, and pass `--` separator: `args(["clone", "--quiet", "--", git_url, ...])`.

- **`src/ruxen_cli/src/resolve_deps.rs:269`** — `git checkout --quiet <effective_ref>` accepts `branch`/`tag`/`rev` straight from manifest. Same argument-injection vector on older git.
  **Fix**: insert `--` separator; validate ref against `[A-Za-z0-9_./+-]`, rejecting leading `-`.

- **`src/ruxen_cli/src/resolve_deps.rs:187-202`** — `resolve_path_dep` accepts absolute paths and `..` traversal from `Ruxen.toml`. A malicious `path = "/etc"` or `path = "../../../home/user/.ssh"` is read and linked into the user's binary.
  **Fix**: refuse absolute paths and `..` components that escape `project_dir` unless `--allow-external-path`; document in `docs/security/path-deps.md`.

- **`src/ruxenc/src/bench.rs:142-150`** — synthesised `def main` text-splices user-controlled identifiers into a generated Ruxen runner. Safe today by parser contract, but the contract is unsafe by construction.
  **Fix**: validate `bench_names` against `[A-Za-z_][A-Za-z0-9_]*` before splicing; emit through a synthesised registry array, not raw text in a generated function body.

- **`src/ruxen_repl/src/jit.rs:178-188`** — `dlsym(RTLD_DEFAULT, name)` resolves any symbol in process memory. A `lib def system as "system"` JITs straight to libc.
  **Fix**: allowlist `ruxen_*` plus an explicit small set of legitimate libc names; reject everything else from the dlsym path.

## 1.3 Silent miscompilation / unsoundness (correctness P0)

- **`compiler/ruxen_core/src/parser/expr/calls.rs:72-96`** — tuple field access `t.0.1` calls `format!("{}", val: f64)` and `split('.')`. For `t.0.10`, `0.10` formats to `"0.1"`; parser reads field 1 instead of 10. For `t.10.20`, `10.20` formats to `"10.2"`; reads field 2 instead of 20.
  **Fix**: lex `.<int>.<int>` as three tokens when after a `.`, or carry the original raw text on `FloatLiteral`.

- **`compiler/ruxen_core/src/mir/lower/collect.rs:223, 229, 239, 260, 290`** — drop collector matches only `"drop"`. Memory `project_ruxen_drop_name_mismatch.md` confirmed: `def __drop` never fires; `library/std/sync.rx` leaks.
  **Fix**: either normalise to `drop` (surface E1118 for `__drop`/`drop_`), or accept both spellings here and in `insert_drops`'s `_drop` suffix strip. Lock with a pin test.

- **`compiler/ruxen_core/src/mir/lower/derive.rs:757-793`** — `synthesize_class_clone` iterates only `c.fields`, ignoring parent-class fields. Inherited fields silently zero-init in the synthesised `_clone`.
  **Fix**: replicate the parent-walk loop from `alloc_size` to collect a flattened field list (parent fields first, in layout order, with `class_field_index_shift` applied for runtime-dispatch classes); emit one `GetField`/`SetField` pair per slot.

- **`compiler/ruxen_core/src/mir/lower/mod.rs:803`** — `current_parent_class` uses `fn_name.split('_').next().unwrap_or("")`. For `__HandlerFuture_init` returns `""`; for `Foo_bar_baz` where `Foo_bar` is the class returns `"Foo"`. Memory `project_ruxen_mir_mangled_method_name_parsing.md` documents the correct helper.
  **Fix**: `self.class_name_from_mangled(&fn_name)`.

- **`compiler/ruxen_core/src/typeck/unify.rs:294-301`** — Ref auto-deref in `unify`: `unify(Ty::Ref(T), U)` returns `Ok(Ref(T))` when `unify(T, U)` succeeds. `&Int` unifies with `Int` silently; bypasses invariance.
  **Fix**: remove ref auto-deref from `unify`; push into `unify_or_coerce` where it belongs.

- **`compiler/ruxen_core/src/codegen/cranelift/emit.rs:484-494`** — `MirInst::CallIndirect` builds the signature from `arg_vals` widths and hardcodes return to `types::I64`. For an indirect call whose real return is `I8` (Bool) or `I32` (Char): verifier rejection or garbage upper-bit reads.
  **Fix**: thread the MIR-declared callee signature through `CallIndirect`; use `ty_to_cranelift(&func.locals[dest].ty)` for the return slot.

- **`compiler/ruxen_core/src/codegen/cranelift/helpers.rs:148-191`** — `simple_type_size` hardcodes `Class | Struct => 64` ("up to 8 fields"). `codegen/layout.rs::layout_of` computes real size. Today `MirInst::Alloc.size` is precomputed; if any future emitter forgets, falls through to this for >8-field classes → heap corruption.
  **Fix**: delete the Class/Struct arm; take `&SymbolTable`, delegate to `layout::layout_of`; or `panic!` on zero-size path so the bug surfaces.

- **`compiler/ruxen_core/src/codegen/cranelift/translation_env.rs:86-105`** — generic-method dispatch resolves missing callee via `ends_with("_<method>")` + shortest-name wins. Any class whose mangled name ends with the same suffix wins by chance.
  **Fix**: track receiver class name through MIR (Phase B-5 vtable path already does this); forbid the suffix fallback; emit a hard error citing the unresolved `?T*`.

- **`compiler/ruxen_core/src/codegen/cranelift/translation_env.rs:70-161`** — `get_or_declare_func` caches `FuncId` by name in one map but the signature is fixed by whichever caller declared first. Different call sites silently reuse the wrong sig.
  **Fix**: assert sig matches on every `declare_function`, or key on `(name, sig_hash)` with upstream monomorphization disambiguating.

- **`compiler/ruxen_core/src/typeck/method_resolvers/mod.rs:1134-1140`** — six catch-all wildcard arms `(_, "to_display"|"summary"|"is_actionable"|"is_done"|"serialize"|"message")` accept any receiver type and return `Some(Ty::String|Ty::Bool)`. `42.summary` typechecks. `is_actionable`/`is_done` are domain leaks from a test fixture.
  **Fix**: delete bare-wildcard arms; gate `to_display` on `Displayable` mixin satisfaction if it really is universal; remove the domain-named ones.

- **`compiler/ruxen_core/src/resolve/control_flow.rs:327`** — `record_capture_if_needed` snapshots `self.symbols.def_ty(def_id)` at capture-record time; stores `Ty::Infer`/`Error` permanently on `Capture.ty` for not-yet-typed locals. Memory `project_ruxen_closure_capture_ty_stale.md` confirms still open.
  **Fix**: do not store `ty` on `Capture` at all; require consumers to re-fetch from `symbols.def_ty(cap.def_id)`. Or rename to `ty_at_capture_time` so staleness is at the type level.

## 1.4 Open-by-default allowlists

- **`compiler/ruxen_core/src/mir/lower/drops.rs:412-652`** — `FRESH_ALLOC_CALLEES` is a 240-LOC hardcoded list. Every new stdlib constructor → leak risk if forgotten; every new consumer → double-free risk if mislabeled.
  **Fix**: data-drive via FFI attributes — `def ruxen_string_concat as "..." returns owned` propagated into `FfiFuncDecl`. This function becomes a flag lookup.

- **`compiler/ruxen_core/src/mir/lower/drops.rs:786-789`** — ownership-transfer allowlist hardcoded to `"ruxen_executor_spawn" | "Task_spawn_raw"`. Memory `project_ruxen_task_spawn_ownership_gap.md` is exactly this gap. Anything else that transfers ownership through FFI (futures, channel send, future closure-passing) silently double-frees.
  **Fix**: extend `lib def NAME as "..."` with per-arg `move | borrow`; propagate into `MirProgram::ffi_libs`; consult it here.

---

# 2. Frontend (lexer + parser + hir + diagnostics + implicit_includes)

`compiler/ruxen_core/src/{lexer,parser,hir,diagnostics,implicit_includes}/`

## CRITICAL

- **`parser/expr/calls.rs:72-96`** — tuple-field roundtrip through `f64` (see §1.3).
- **`parser/expr/atoms.rs:413`** — `if j > 256 { return false; }` silently aborts type-args lookahead; long generic arg lists misclassify as indexing. **Fix**: remove the bound or emit a diagnostic when hit.
- **`parser/mod.rs:412-419`** — `expect_terminator` only consumes Newline/Semicolon and silently accepts everything else; "contextual terminators" doc claims more but the check is absent. **Fix**: branch explicitly on terminators; diagnose fall-through.

## HIGH

- **`lexer/tokens.rs:273`** — `try_numeric_suffix` does `let remaining: String = self.chars[self.pos..].iter().collect();` per call. **Fix**: scan up to 5 chars via `peek_at`.
- **`lexer/mod.rs:11-12, 25`** — `chars: Vec<char>` materialises the whole source; identifiers reconstructed via `chars[a..b].iter().collect::<String>()` instead of `&source[a..b]`. **Fix**: keep `source: &'a str` + byte cursor; slice directly.
- **`lexer/mod.rs:72-74`** — `Ok(self.tokens.clone())` clones the entire token vector on exit. **Fix**: take `self` by value, return by move.
- **`parser/expr/atoms.rs` (1104 LOC), `parser/classes.rs` (1127 LOC)** — god files. `parse_primary` is 360 LOC; `parse_type_expr_primary` is 169 LOC; body-loop boilerplate repeats 5× verbatim. **Fix**: split into `literals.rs`/`paths.rs`/`closures.rs` and `class.rs`/`enum.rs`/`struct.rs`/`mixin.rs`; extract `parse_body_items<F>`.
- **`hir/types.rs:1003-1021`** — `nominal_definition` does linear `symbols.iter().find(...)` per Send/Sync recursion step. **Fix**: `HashMap<&str, &Definition>` index keyed by name+kind, built once at end-of-resolve.
- **`parser/classes.rs:937, 1048; parser/expr/atoms.rs:11; parser/items.rs:21`** — every body-loop iteration calls `self.current_kind().clone()` to bind into `match`. For String-carrying variants this is a heap alloc per token examined. **Fix**: match against `&TokenKind`; clone inside the arm only when owned String is needed.

## MEDIUM

- **`parser/ast.rs:43, 937`** — `TopLevelItem::Extern(ExternBlock)` and `pub struct ExternBlock` are dead; no parser produces them. **Fix**: delete.
- **`parser/attributes.rs:11-105`** — `parse_attributes`/`apply_*_attrs` (138 LOC) have zero callers since `@[…]` retirement. **Fix**: delete; keep `Attribute`/`AttrArg` types only if FFI inline still needs them.
- **`parser/ffi.rs:9; items.rs:36; classes.rs:592, 1016`** — `parse_lib_decl(link_attrs: Vec<LinkAttr>)` always called with `vec![]`. **Fix**: drop parameter; drop `LinkAttr`/`LinkKind`.
- **`parser/methods.rs:277-281`** — `let final_self_mode = self_mode.or({ None });` — `Option::or(None)` is identity. **Fix**: delete; if a default was intended, write it.
- **`diagnostics/mod.rs:10, 23-29`** — `DiagnosticLevel::Help` has no constructor; `Diagnostic` has no notes/labels/secondary spans. Every parser error is a flat string + one span. v1 ceiling that will hurt UX. **Fix**: add `notes: Vec<(Span, String)>` and `Diagnostic::help`.
- **`parser/mod.rs:108-114`** — `parse_repl_input` detects incomplete input via substring `"expected {:?}, found Eof"` (literal `{:?}`). Any error reword silently breaks REPL continuation. **Fix**: emit a structured code (e.g. `E0008`); match on `d.code`.
- **`parser/*.rs` (~25 sites)** — diagnostics format with `{:?}` on `TokenKind`. **Fix**: `impl fmt::Display for TokenKind` returning user-visible form; use `{}`.
- **`lexer/mod.rs:165-167`** — `lex_block_comment` consumes newlines silently via `advance()`; the newline-separator invariant the parser depends on is broken across block comments. **Fix**: emit a single `Newline` after a block comment that spanned `\n`.
- **`hir/types.rs:519`** — `Ty::Fn | Ty::FnMut | Ty::FnOnce => true` for `is_send`/`is_sync`. If these ever model closures with captures (not bare fn pointers), unsoundness. **Fix**: document that these are caller-side fn types only; model closures as nominal classes.
- **`parser/mod.rs:198-235`** — `check_incomplete` scans entire token stream from index 0 per call. O(n) per keystroke in REPL/LSP. **Fix**: start from `self.pos`, or compute delimiter depth incrementally.
- **`parser/expr/atoms.rs:534`** — qualified paths encoded as dotted strings via `ExprKind::Identifier(path.join("."))`. **Fix**: add `ExprKind::QualifiedPath(Vec<String>)`.

## LOW

- **`lexer/tokens.rs:268-316`** — suffix table rebuilt per call. **Fix**: `const SUFFIX_TABLE: &[(&str, NumericSuffix, bool)]`.
- **`parser/mod.rs:288, 302, 312, 323`** — `tokens.last().unwrap()` ×4. **Fix**: `expect("lexer always emits Eof")` so the invariant is documented.
- **`lexer/strings.rs:297-322`** — `strip_leading_whitespace` 3-pass. Acceptable; single-pass is straightforward.
- **`hir/types.rs:324-326, 330, 334, 338, 369`** — `std::string::String` fully-qualified suggests a removed in-module shadow. **Fix**: drop qualification.
- **`parser/expr/atoms.rs:115`** — `name.trim_end_matches('!').to_string()` allocates; lexer already constructed the trailing `!`. **Fix**: split in lexer.
- **`diagnostics/codes.rs:26`** — `REGISTRY: &[CodeInfo]` linear lookup. **Fix**: `phf_map!` or sorted slice + `binary_search_by_key`.

## Architectural summary

- **Well-shaped**: keyword table → `TokenKind` lookup; OOM-guard `ensure_loop_progress` + `__progress` capture pattern; `is_send_with`/`is_sync_with` single-entry doctrine in `hir/types.rs:530-563`; grep-enforced error-code registry.
- **Gnarly**: token-kind clone discipline (35+ `current_kind().clone()` sites); twin `chars: Vec<char>` + `byte_pos`; user-facing diagnostics rendered with `{:?}`; dead surfaces (`ExternBlock`, `attributes.rs`, `LinkAttr`) lingering from retired phases.
- **Biggest refactor opportunity**: lexer + parser memory representation. Switch `Lexer::chars: Vec<char>` → byte cursor over `&'a str`; make `TokenKind` carry `&'a str` or interned `Symbol(u32)`; add `impl Display for TokenKind`; return tokens by move. Removes hundreds of clones; halves parser allocation footprint; unlocks better diagnostics.

---

# 3. Resolve (name resolution + symbol table)

`compiler/ruxen_core/src/resolve/`

## CRITICAL

- **`resolve/control_flow.rs:327`** — capture `ty` snapshot stale (see §1.3).
- **`resolve/types.rs:516-548`** — `resolve_type_path` silently rewrites `Ty::Class { "String", … }` to `Ty::String` to paper over `resolve_class`'s `DefKind` stomp. The stomp itself still happens — only the read side is patched. Memory `project_ruxen_resolve_class_stomps_typealias.md`. **Fix**: in `resolve_class` (`items.rs:332`), refuse to overwrite a `DefKind::TypeAlias` whose target is a primitive `Ty::*`; attach methods/fields via a parallel `ClassInfo` keyed by the same DefId or a side map.
- **`resolve/bootstrap_merge.rs:306-325`** — per-package snapshot uses `scopes.lookup(name).or_else(|| scopes.lookup_type(name))`; protects the auto-populate path but every other consumer of `scopes.lookup` (e.g. Pass-2 user code at `bootstrap_merge.rs:202`) still sees last-wins between packages. **Fix**: push a package-anchored scope frame around each `merge_bootstrap_programs` iteration; re-export through `std.<pkg>.items` only.
- **`resolve/funcs.rs:67-84`** — `self_mode` silently inferred to `RefMut` for `def init`, `Ref` otherwise when `current_self_ty` is set; no diagnostic if a garbled AST sets `is_class_method=true` AND `self_mode=Some(...)`. **Fix**: `debug_assert!(!(f.is_class_method && self_mode.is_some()))` at line 85; reject or strip.
- **`resolve/ffi_registration.rs:879-913`** — Mixin arm registers `MixinInfo { generic_params: vec![], ... }` even when `t.generic_params` is non-empty. Generic mixins (`mixin Into[T]`) lose their generic surface at registration. **Fix**: call `collect_generic_param_infos(&t.generic_params)` like the Class/Struct/Enum arms.

## HIGH

- **`resolve/items.rs:273-287`** — `runtime_dispatch_includes` looks up mixin via `symbols.iter().find(|d| d.name == trait_ref.name)` — linear per include per class. **Fix**: `type_registry.get(&trait_ref.name)`.
- **`resolve/items.rs:599`** — enum variant DefIds looked up by `scopes.lookup(&composite_name)` with `format!("{}.{}", e.name, variant.name)`; `unwrap_or_else(|| symbols.define(...))` silently creates a duplicate DefId if pass-1 missed it. **Fix**: `expect(...)`; or surface E0902-style internal-error via `self.diagnostics`.
- **`resolve/bootstrap.rs:264-268`** — `RUXEN_STDLIB_PATH` env var trusted as bootstrap root. Low-likelihood but a trust boundary. **Fix**: canonicalise; refuse roots without a `Ruxen.toml` or sysroot marker.
- **`resolve/ffi_registration.rs:444-1212`** — single ~770-LOC function with one giant `match item`; Class arm alone is 235 LOC. **Fix**: extract `register_class_in`/`register_struct_in`/`register_enum_in`/`register_mixin_in`/`register_module_in`/`register_lib_in`/`register_extern_in`; dispatch becomes ~30 LOC.
- **`resolve/items.rs:308`** — `class_generic_param_infos` recomputed via `collect_generic_param_infos` even though `generic_params` was resolved 200 lines above. Two walks for the same data. **Fix**: have `resolve_generic_params` return both, or store one and synthesise the other.
- **`resolve/helpers.rs:136-159 vs 161-185`** — `resolve_params` and `resolve_and_register_params` differ by one line. 24 lines copy-paste. **Fix**: collapse with a `register_in_scope: bool` parameter.

## MEDIUM

- **`resolve/mod.rs:97-174`** — `Resolver` carries 17 fields of side-channel state. Memory `project_ruxen_caller_identity_as_arg.md` says caller-identity should be a `Copy` ctx struct. `RegistrationCtx` follows the pattern; the rest don't. **Fix**: extract `ResolveContext { current_self_ty, current_class_def, current_return_ty, current_impl_assoc_types, current_trait_context }` as a `Copy`/`Clone` snapshot passed down; move accumulators to a `ResolverScratch`.
- **`resolve/funcs.rs:298-370`** — `resolve_child_in_def` `.clone()`s entire parent `Definition` (full `ClassInfo`/`EnumInfo` with `Vec<DefId>` fields) per use-decl segment. **Fix**: borrow with narrower scope; extract `module_item_names_for(&self, parent: DefId) -> &[DefId]`.
- **`resolve/types.rs:355-402`** — const-predicate evaluation clones `info.const_predicates` and `info.generic_params` twice per type-path resolution. **Fix**: borrow once; clone only predicates that surface diagnostics.
- **`resolve/exprs.rs:188-229`** — `if name.contains('.')` for qualified type lookup. Substring detection. **Fix**: gate on a parser-side flag.
- **`resolve/items.rs:817-822`** — `resolve_impl` only looks up `class_def` for `Class`/`Enum`/`Struct`. `Ty::Newtype`/`Ty::Alias` flow through with `class_def=None` → method registrations become free functions. **Fix**: walk to nominal root (same as `nominal_type_definition_mut`).
- **`resolve/scope.rs:67`** — `insert_type` returns prev DefId on shadow; no caller checks. Type-name shadowing silently allowed (e.g. `class Foo[Foo]`). **Fix**: assert `None` at items.rs:132/144 type-param inserts; diagnose otherwise.

## LOW

- **`resolve/funcs.rs:246`** — `path.last().unwrap()` after `is_empty()` 25 lines away. **Fix**: `let Some(last) = path.last() else { return; };`.
- **`resolve/exprs.rs:455`** — `args_hir.into_iter().next().unwrap()` after a `len != 1` check three lines above. **Fix**: `let [arg] = args_hir.as_slice() else { unreachable!() }`.
- **`resolve/ffi_registration.rs:451-456`** — `let _span_zero = Span { start: 0, ... }` constructed and never read. **Fix**: remove.
- **`resolve/items.rs:1`** + every other resolve file at line 1 — `#![allow(unused_imports)]` blanket. **Fix**: remove; let `cargo fix` clean up.
- **`resolve/control_flow.rs:320`** — `closure.captures.iter().any(|cap| cap.def_id == def_id)` linear dedup. **Fix**: parallel `HashSet<DefId>` in `ClosureCaptureContext`.
- **`resolve/types.rs:613`** — undefined-type error message doesn't distinguish rooted vs unrooted paths. **Fix**: include "rooted path" in the message when `path.rooted`.

## Architectural summary

- **Well-shaped**: `ScopeStack`/`SymbolTable` clean; `RegistrationCtx` is a textbook conversion of side-channel state to explicit args; two-pass forward-decl + body resolution is sound; bootstrap merge correctly snapshots per-package DefIds for the auto-populate path.
- **Gnarly**: `Resolver` god-object with 17 lifetime-spanning fields; `register_top_level_type_with_ffi_in` is 770 LOC mixing seven item kinds; `resolve_class` knowingly overwrites a `TypeAlias` for `String` and the patch is read-side normalisation in three places (resolve+typeck+MIR) that must move in lockstep forever.
- **Biggest refactor opportunity**: `ResolveCtx` `Clone` struct + extract the seven `register_<kind>_in` helpers. Pulls 5 of 17 god-object fields into the type system; shrinks the biggest function by 90%; removes the implicit save/restore-via-`mem::replace` pattern. Follow-up: fix the `String` `DefKind` stomp properly.

---

# 4. Typeck (type inference + method resolution + unification)

`compiler/ruxen_core/src/typeck/`

## CRITICAL

- **`typeck/method_resolvers/mod.rs:1134-1140`** — wildcard catch-all arms (see §1.3).
- **`typeck/infer/expr.rs:564-573`** — `If` arm unify error silently swallowed: on mismatch falls back to `then_ty` with no diagnostic. Users get a wrong inferred type and a confusing downstream error. **Fix**: emit a typed diagnostic (e.g. E0710); suppress only when one side `is_never()`.
- **`typeck/infer/expr.rs:637, 652, 659, 829, 842, 845, 903, 913, 914`** — every `let _ = unify(...)` discards a real type-error. Assigns, compound assigns, return, array/map literals, macro-arg pins all swallow. **Fix**: route each through `self.type_error(...)` or document why suppression is correct (e.g. "macro arg unification is best-effort").
- **`typeck/unify.rs:294-301`** — Ref auto-deref unsoundness (see §1.3).
- **`typeck/infer/ops.rs:130-133`** — `Eq | NotEq | Lt | Gt | LtEq | GtEq` runs `unify` only; never emits a `.eq` method-resolution constraint or marks the binop for MIR string-comparison lowering. Memory `project_ruxen_async_compiler_gaps.md` gap #3 (String== falls back to pointer-eq after `Result` destructuring). **Fix**: when both sides resolve to `Ty::String`/`Ty::Str`, set a side-channel marker on `BinaryOp` or rewrite to a `MethodCall { method: "eq", ... }`.
- **`typeck/infer/expr.rs:560-573`** — `match unify(then, else)` on `Err` doesn't check `is_error()`. Combined with silent `Ty::Error` in `lookup_field_with_parents`, a single bogus `then` branch poisons the whole `if` type. **Fix**: explicitly propagate `Ty::Error` upward when either branch is `is_error()`.

## HIGH

- **`typeck/method_resolvers/mod.rs` (1144 LOC)** — single match on `(Ty, &str)`. The header comment already plans the split. None of the 30+ `Ty::String =>` arms also handle `Ty::Class{name:"String"}` (the `resolve_class` stomp gotcha). **Fix**: split per-namespace; add `normalize_string_class(ty)` at entry; `debug_assert!` no `Ty::Class{"String"}` reaches the match body.
- **`typeck/infer/collect.rs:284, 309, 329, 341, 368, 390, 472`** — `lookup_field`/`lookup_field_with_parents`/`lookup_class_method_return`/`infer_class_generics`/`substitute_generics_in_return` all do `for def in self.symbols.iter()` per `obj.field` and per `MyClass.new(...)`. Quadratic in program size. **Fix**: `HashMap<String, Vec<DefId>>` name index on `SymbolTable`.
- **`typeck/infer/expr.rs:42-216`** — `FieldAccess` arm is ~175 LOC with the same 4-step fallback ladder duplicated three times. Different arms use different error phrasings. **Fix**: extract `try_resolve_field_or_method(name, ty) -> Result<Ty, FieldErr>`.
- **`typeck/infer/collect.rs:67-73`** — `resolve_method_call` for a missing method on a concrete type silently returns `fresh_type_var()`. `vec.pussh(1)` becomes a fresh `?Tn` that masks the real error. **Fix**: emit "unknown method" diagnostic for the `!is_infer && !is_error` case; keep `fresh_type_var` for recovery but with a code.
- **`typeck/method_resolvers/mod.rs:474-478, 510, 532-533, 542`** — `if name.ends_with("Iter")` dispatch. User class named `*Iter` gets free `filter`/`map`/`count`/`sum`/`enumerate`/`partition`/`to_vec` with possibly bogus item types. **Fix**: gate on a structural marker — `info.kind == ClassKind::IteratorAdapter`, mixin satisfaction, or a registry built at bootstrap.
- **`typeck/infer/expr.rs:357-379 + 415-432`** — closure-param seeding for `map`/`filter` hardcoded by method name; copy-pasted into the post-call unification block. Receiver auto-deref happens twice. **Fix**: lift to `seed_and_unify_closure(method_name, obj_ty, block, ret_ty)`.
- **`typeck/unify.rs:33-39`** — `unify` calls `ctx.resolve(a)` and `ctx.resolve(b)` on every recursive call; each `resolve` deep-clones the entire type tree. For `Map[String, Vec[Result[Int, IoError]]]` every layer of unify deep-clones both sides on entry. **Fix**: pass `&Ty`; use `resolve_shallow` (top-level `Ty::Infer` chain only) at each entry; reserve full `resolve` for final return.
- **`typeck/method_resolvers/mod.rs:300-303, 944-955`** — `Thread.spawn` reads `args.first().and_then(InferenceEngine::callable_return_ty)`. If `args.first().ty` is still `Ty::Infer`, returns `None`; `JoinHandle[?Tn]` survives; `.join()` returns `Result[?Tn, ThreadPanic]`. No resolve attempt. **Fix**: `eng.ctx.resolve(&arg.ty)` first.

## MEDIUM

- **`typeck/infer/mod.rs:62-75`** — `InferenceEngine::new` takes `&mut SymbolTable` but most reads in `collect.rs` are immutable. Only mutation is `update_ty`. **Fix**: refactor to `&SymbolTable` + `pending_ty_updates: Vec<(DefId, Ty)>` flushed once after inference.
- **`typeck/infer/expr.rs:945`** — `NullLiteral` returns a fresh inference variable with no recorded "must resolve to nullable shape" constraint. The "backwards compatibility default to `UInt64`" path has no code that actually defaults it. **Fix**: deferred default in `TypeContext`, or store `NullLiteralKind` so MIR can decide.
- **`typeck/infer/expr.rs:722-731, 755, 771`** — `EnumVariant` for `None`/`Ok`/`Err` falls back to `fresh_type_var()` when enclosing `current_return_ty` is not the expected shape. For `Err(e)` inside `async def -> Result[T, E]`, `current_return_ty` is `T` not `Result[T,E]` (async-lowering wraps). **Fix**: peel `Ty::Class{"Future", [inner]}` first.
- **`typeck/infer/expr.rs:622-626`** — `For { iterable, body, .. }` does NOT bind the loop variable's type from the iterable's element type. Memory `project_ruxen_for_loop_infer_gap.md` confirmed. **Fix** (load-bearing): after `infer_expr(iterable)`, match shape (`Array(T)`/`Map(K,V)`/`*Iter[T]`/`Set(T)`/`Range`), look up the binding DefId from `HirExprKind::For`, `unify(binding_ty, elem_ty)`.
- **`typeck/infer/collect.rs:325-353`** — `lookup_class_method_return` returns only the return type, dropping `FnSignature`. Callers can't validate arity or arg types. **Fix**: return `Option<FnSignature>`; arity-check at call site.
- **`typeck/infer/mod.rs:233-238`** — `is_mut_method` allow-list falls back to `Ty::Unit` for hardcoded names `display`/`display_all`/`init`/`drop`. User method `display` for unrelated reasons gets a silently-changed return type. **Fix**: drive off `func.self_mode == HirSelfMode::RefMut`, or require explicit `-> ()`.

## LOW

- **`typeck/unify.rs:44-61`** — both infer-var branches clone `a` and `b` into `TypeError`; already cloned by `ctx.resolve` at 34-35. 2× allocations per failure. **Fix**: `mem::take` or accept owned resolved values.
- **`typeck/method_resolvers/mod.rs:67, 71, 99, 103`** — `class_ty("ParseIntError", vec![])` repeated 4×; every dispatch builds a fresh `String`. **Fix**: `const fn` or `lazy_static` for the few stdlib error classes used in dozens of arms.
- **`typeck/infer/expr.rs:825-833`** — `ArrayLiteral` allocates `fresh_type_var()` even for empty literals; unifies pairs against a moving target. **Fix**: short-circuit on `elems.is_empty()`; unify pairwise.
- **`typeck/infer/collect.rs:107-117`** — `iter_item_ty` matches `name.ends_with("Iter")`. Same brittleness; consolidate.
- **`typeck/mixins.rs:236-242, 247-255`** — `lookup_method` iterates `nominal_impls` twice per method lookup. **Fix**: `HashMap<TypeName, Vec<(TraitName, Methods)>>` for O(1).

## Architectural summary

- **The match-on-name-string pattern is the dominant brittleness vector.** `method_resolvers/mod.rs` matches on method name; `iter_item_ty` matches `ends_with("Iter")`; `infer_func` matches `display|drop|init`; `mixins.rs::type_name` produces strings keyed against other strings. Each is one rename / one `resolve_class` stomp away from silent wrong dispatch. Replace name dispatch with structural markers (`IteratorAdapter`, `BootstrapClass`, `is_string_class`).
- **Error handling has two voices and they disagree.** `unify_or_coerce` + `type_error` is the loud path with diagnostic codes (E0700, E1011, E1100…); the `let _ = unify(...)` sites in `expr.rs` and `helpers.rs::collect_break_types` are silent. Pick one — every unify carries diagnostic context, or document why a site can suppress. The current mix is worst-of-both.
- **`infer/expr.rs` is one match away from un-reviewable.** 948 LOC, deep nesting per arm, repeated `auto_deref`+`resolve` ladders, copy-pasted closure-param seeding. The recent split out of `infer.rs` (commit 200f10d) was right; next cut is per-`HirExprKind`-variant methods (`infer_field_access`, `infer_method_call`, `infer_enum_variant`).

---

# 5. MIR + borrow_check

`compiler/ruxen_core/src/mir/`, `compiler/ruxen_core/src/borrow_check/`

## CRITICAL

- **`src/ruxen_repl/src/jit.rs:731`** — non-exhaustive `MirInst` match (see §1.1; cross-cited here because it indicts MIR's exhaustiveness contract across backends).
- **`mir/lower/collect.rs:223+`** — `__drop` not collected (see §1.3).
- **`mir/lower/derive.rs:757-793`** — `synthesize_class_clone` ignores parent fields (see §1.3).
- **`mir/lower/mod.rs:803`** — `current_parent_class` naive `split('_')` (see §1.3).
- **`mir/lower/drops.rs:786-789`** — ownership-transfer allowlist hardcoded (see §1.4).
- **`mir/lower/expr/method_call.rs:177-394 vs 765-1283`** — two dispatch paths gotcha. Memory `project_ruxen_mir_two_dispatch_paths.md`. The static-ctor fast path consults `lookup_ffi_alias("<base>_<method>")` at 203, then falls back to a hardcoded class allowlist (238-289); the second route's `resolve_ffi_alias_callee` at 372-387 retries with `runtime_base` then `base_type` but does NOT retry the dotted-normalised form symmetrically. **Fix**: extract `resolve_dispatched_callee(receiver_ty, method_name, args) -> Either<DirectCall, ClassInit>` owning dot-normalisation, generic stripping, runtime-base remapping (Array→Vec, HashMap→Hash), and bufio suffix selection.

## HIGH

- **`mir/lower/drops.rs:412-652`** — `FRESH_ALLOC_CALLEES` 240-LOC hardcoded list (see §1.4).
- **`mir/lower/derive.rs:160-260`** — `synthesize_struct_to_debug` emits N right-folding `ruxen_string_concat` calls; O(N²) bytes copied at runtime per `to_debug`. Same for enum debug at 906+. **Fix**: emit a `ruxen_string_builder_*` chain — append into a growing buffer; materialise at the end.
- **`mir/lower/expr/method_call.rs` (1291 LOC, single function)** — `lower_method_call` is 1200+ lines of sequential `if method_name == ...` before generic dispatch. **Fix**: registry of `MethodLoweringStrategy` impls keyed by `(receiver_ty_kind, method_name)`; extract OK/Some/Result helpers and String mutation dance.
- **`mir/lower/drops.rs:336`** — `compute_dealloc_safe_locals` comment says "Loops/back-edges are not modeled". A `loop { x = String.from("b") }` taints `x` on first assign and assumes stays tainted; per-iteration alloc leaks. **Fix**: worklist fixpoint with `alloc_rooted` joined per-block on predecessors (intersection); keep single-pass as fast path.
- **`mir/lower/derive.rs` (1339 LOC, god file)** — Display/Debug/Eq/Hash/Default/Clone (struct/class/enum)/Ord/PartialOrd + field-level recursors. **Fix**: split per-trait into `derive/{clone,debug,eq,ord,default,display}.rs`.
- **`borrow_check/checks.rs:457-473`** — hardcoded mutating-method list (`push|pop|insert|remove|clear|sort|reverse|push_str|truncate|extend|retain|drain|iter_mut|set`). Missing `swap`/`replace`/`splice`/`dedup`/`fill`/`resize`/`set_len`/`swap_remove`/`chunks_exact_mut`. User-defined mutating method not registered as `RefMut` self_mode silently treated as immutable. **Fix**: methods flowing through here without symbol-table hit should diagnose (E1119: "method `X` could not be resolved; conservative immutable-borrow assumed").
- **`borrow_check/borrows.rs` (`BorrowSet`)** — `Vec<BorrowInfo>` never compacted; `kill_after_checkpoint`/`kill_scope` flip `alive: false` but leave entries; `expire_before` iterates all per expression. O(borrows × expressions) per function. **Fix**: compact at scope-pop in `kill_scope`; or partition active/dead vectors.

## MEDIUM

- **`mir/lower/mod.rs` (1068 LOC)** — 13 distinct method clusters in one file. **Fix**: split into `program.rs`/`vtables.rs`/`dispatch_lookup.rs`.
- **`borrow_check/moves.rs:60-79`** — `process_transfer`/`process_call_move` consult `ty.is_copy()`, not `ty_is_effectively_copy(ty, symbols)`. Every call site already gates on the symbol-table-aware variant — defense is dead today. **Fix**: take `&SymbolTable` here; call `is_copy_with`.
- **`borrow_check/walk.rs:440`** — `check_for` registers binding with `Ty::Infer(0)`. Memory `project_ruxen_for_loop_infer_gap.md`. `is_copy(Infer)` returns false → treated as Move → use-after-move fires on `for x in xs.iter()` reading `x` twice. **Fix**: peek `iterable.ty`; compute element type same way typeck does; register with that. Or skip the binding entirely when type unknown.
- **`mir/lower/expr/field_access.rs:22-37`** — hardcoded Option method opt-out list for safe-nav. Adding `xor`/`zip`/`unzip`/`as_deref`/`iter`/`cloned` silently breaks `x?.method`. **Fix**: invert — opt-in for genuine field/accessor; gate on `field_name` resolving to a `Field` not a `Method`.
- **`mir/lower/drops.rs:38, 314`** — `compute_dealloc_safe_locals` and `compute_return_alias_chain` both walk every block twice per function. **Fix**: cache `alloc_rooted` per-block during the forward pass; reuse.
- **`mir/lower/mod.rs:595-617`** — `synthesize_dynamic_dispatch_helpers` does `symbols.iter().find_map(...)` per required method. O(M·K·N). **Fix**: `HashMap<MethodName, Vec<(parent_def_id, signature)>>` once at function entry.
- **`mir/lower/expr/method_call.rs:412-652`** in drops.rs — `callee.starts_with("Vec_") || callee.starts_with("Vec[") || callee.starts_with("Array_") || callee.starts_with("Array[")` chains repeated 4×. **Fix**: `&'static [&str]` + `.iter().any(|p| callee.starts_with(p))`.
- **`borrow_check/walk.rs:344-356`** — `check_if` `ownership_snap.clone()`, `moves.snapshot()`, `borrows.snapshot()` allocate three full state copies per branch per `if`. **Fix**: persistent maps (`im::HashMap`) → O(1) clones; O(log n) merges per differing key.
- **`mir/lower/derive.rs:625-674`** — `synthesize_clone_field` silently falls through to bitwise reuse when no clone helper matches and `user_type_with_derive_clone` returns None. `derive Clone` struct with a `RawPtr` or foreign FFI-type field slips both checks → shallow-copy `_clone` aliasing foreign memory. **Fix**: turn fall-through into `unimplemented!` / internal compiler-error diagnostic.

## LOW

- **`mir/nodes.rs:257` (`MirInst`)** — not `#[non_exhaustive]`. Document: exhaustive matches are load-bearing across backends.
- **`mir/lower/derive.rs:622, 630, 638, 646, 654, 663`** — `mir_fn.new_temp(field_ty.clone())` per field. **Fix**: take `&Ty`; clone only when stored.
- **`borrow_check/moves.rs:113-138`** — `merge` returns silently on empty branches. **Fix**: document.
- **`mir/lower/drops.rs:154`** — `if drop_locals.is_empty() { return; }` is fine; user-drop class path at 193 still consults `dealloc_safe.contains` per local even though we'd already returned. Minor.
- **`mir/lower/expr/method_call.rs:1288`** — `unreachable!("lower_method_call: dispatched to wrong helper")` is right for an exhaustive top-match, but the surrounding shape invites refactor. **Fix**: `if let HirExprKind::MethodCall {…} = &expr.kind { … }` + `debug_assert` on entry.

## Architectural summary

- **Hardcoded string allowlists are the dominant correctness risk.** Three serious gaps — `drops.rs::FRESH_ALLOC_CALLEES`, `method_call.rs` class allowlist, `checks.rs` mutating-method list — all "open by default" or "closed by default" depending on which way the omission cuts. Each new stdlib method requires editing 3-5 unrelated lists or you get a UAF / leak / missed borrow conflict. Make each a declarative attribute on the FFI/method signature, propagated through HIR/symbol-table to MIR.
- **The MIR lowerer's biggest files are doing too much.** `derive.rs` (1339), `method_call.rs` (1291), `mod.rs` (1068). None of the splits would change behaviour. `drops.rs` (1019) has the opposite problem: one function doing four distinct CFG analyses (alloc-rooting, taint propagation, return-aliasing, drop emission) that share state but should be three passes with explicit interfaces.
- **`borrow_check` is structurally clean but quadratic in two spots and conservatively-wrong in one.** `BorrowSet` Vec never compacts (HIGH perf); `check_if`/`check_match` clone full HashMap state per branch (MEDIUM); `check_for` registers bindings with `Ty::Infer(0)` (MEDIUM correctness, compounds with `for_loop_infer_gap`).

---

# 6. Codegen + async_lowering + formatter

`compiler/ruxen_core/src/{codegen,async_lowering,formatter}/`

## CRITICAL

- **`codegen/cranelift/translation_env.rs:86-105`** — generic-method dispatch suffix-match wins by shortest name (see §1.3).
- **`codegen/cranelift/helpers.rs:148-191`** — `simple_type_size` hardcodes Class/Struct = 64 (see §1.3).
- **`codegen/cranelift/translation_env.rs:70-161`** — `get_or_declare_func` cache by name with first-writer-wins signature (see §1.3).
- **`codegen/cranelift/emit.rs:484-494`** — `CallIndirect` hardcodes return to I64 (see §1.3).
- **`codegen/cranelift/emit.rs:438-443`** — `MirInst::Drop` is documented marker-only; future MIR pass that legitimately emits a Drop silently no-ops. **Fix**: treat reaching `MirInst::Drop` in codegen as a hard error (`return Err("Drop reached codegen — insert_drops should have rewritten it")`).
- **`async_lowering/mod.rs:1417-1424`** — `build_multi_state_poll_body` clones `tail_stmts` into `tail_block` and embeds in `terminal_ready`. State-arm building at 1467-1577 then clones the entire tail per state at 1616/1623. 5-await async fn with 100-stmt tail = 5 deep clones of the same tail. **Fix**: hoist the tail into a `let __tail = ...` field on the state machine; reference; or restructure the if/elsif chain.

## HIGH

- **`async_lowering/mod.rs` (3357 LOC, single file, god file)** — contains AST-walking lowering (50-934), state-machine builder (935-1916), three near-identical E1115/E1112/E1116 diagnostic visitors (~900 LOC structural duplication), block_on rewriter (2897-3347). **Fix**: extract `mod visitor` with `trait AsyncAstVisitor` and a single recursion driver; split into `mod.rs` (orchestration), `segment.rs`, `state_machine.rs`, `diagnostics.rs`, `block_on.rs`, `visitor.rs`.
- **`async_lowering/mod.rs:299-578, 578-934`** — 216 `.clone()` calls across the file (`func.return_type.clone()`, `span.clone()` repeated dozens of times per field). **Fix**: make `Span: Copy`; use `std::mem::replace`/`std::mem::take` where the original AST node is replaced immediately after. 30-50% allocation reduction in this pass.
- **`codegen/cranelift/runtime_sigs.rs:13-280`** — `runtime_signature` returns `Option<(Vec<Type>, Option<Type>)>` — fresh `Vec<Type>` per lookup; called from `coerce_call_args` per call site lowered. **Fix**: return `Option<(&'static [Type], Option<Type>)>` backed by `&[types::I64]` static slices.
- **`codegen/object.rs:117`** — `let obj_path = format!("{}.o", output_path);` — concurrent ruxenc invocations targeting the same output collide. Runtime objects use `pid + atomic counter` (41-51); user `.o` does not. TOCTOU race. **Fix**: mirror runtime-object naming or write to `std::env::temp_dir()`.
- **`codegen/object.rs:117-141`** — on link failure the `.o` is NOT removed; the `remove_file` at 141 runs only after `cmd.status()` success. **Fix**: RAII drop guard or `let _g = OnDrop::new(...)`.
- **`codegen/lang_intrinsics.rs:62-66`** — `"super" => Ok("ruxen_noop")`, `"yield" => Ok("ruxen_noop_passthrough")`, `"&str_as_str" => Ok("ruxen_noop_passthrough")`. `ruxen_noop_passthrough` also hits at 145 and 164 as fallback for `Fn(...)_call` and dyn-erased closure dispatch. If MIR closure lowering ever has a bug that drops a real closure call into this path, it silently becomes a no-op returning its first arg. **Fix**: route to `ruxen_indirect_call_panic` runtime helper that aborts with a clear message.
- **`formatter/comments.rs:103-114, 106`** — `CommentCollector` materialises the entire source as `Vec<char>`; duplicates `pos: usize` (char index) with `byte_pos: usize` (UTF-8 byte index). **Fix**: walk `source.char_indices()` once; track only `byte_pos`; eliminate `Vec<char>`.
- **`formatter/comments.rs:546-575`** — `find_preceding_node`/`find_following_node`/`find_enclosing_node` do O(N) linear scan per comment despite `node_spans` being sorted. 5K-LOC file = 25M comparisons. **Fix**: `node_spans.partition_point(...)` for preceding/following; `BTreeMap<usize, usize>` for enclosing. ~100× speedup on large files.
- **`formatter/format_expr.rs:662-704`** — `collect_chain` clones each link's `ExprKind` (668). For a 10-link chain: O(chain² × subtree-size) clones. **Fix**: `Vec<&'a ExprKind>` — `kind` is borrowed for formatting lifetime.
- **`formatter/format_expr.rs:673, 706`** — `format_method_chain` constructs `let comments = CommentMap::new()` and passes the empty map to `format_expr` per link. The real `comments: &CommentMap` is discarded at chain entry. **Inline `# comment` inside method-chain args is silently dropped by the formatter.** **Fix**: thread real `comments` through `collect_chain` and `format_method_chain`.

## MEDIUM

- **`codegen/cranelift/emit.rs:97-510`** — `translate_instruction` 414-LOC match with 17 deeply nested arms passing 7 parameters; Assign/BinOp/Negate/Not/Compare/Call repeat the same "compute dest_ty + coerce + def_local" trailer. **Fix**: extract `store_to_dest(dest, val, func, var_map, stack_slots, builder)`; collapses ~80 LOC duplication.
- **`codegen/cranelift/mod.rs:507-518`** — `is_dynamic_dispatch_helper` by string-match `<Mixin>_dynamic_<method>`. User class `MyType_dynamic_thing` collides. **Fix**: tag dynamic-dispatch helpers in MIR (`MirFunction { kind: FunctionKind::DynDispatchHelper, ... }`).
- **`codegen/cranelift/mod.rs:111-156`** — `compile_program` Pass 0 declares FFI under three names, each `param_tys.clone()`. ~1000 unnecessary `Vec<Type>` clones at startup for 200+ FFI fns. **Fix**: insert once; `HashMap<&str, FuncId>` for aliases.
- **`async_lowering/mod.rs:1650-1700`** — `default_value_for_type` bails the whole lowering when an await-binding type has no default. State machine then falls back to leaving the async fn un-lowered, producing confusing downstream errors. **Fix**: wrap binding field in `Option<T>` automatically (comment at 1650 already says this), or surface a dedicated E11XX with the failing type.
- **`formatter/format_items.rs:15-76`** — `format_program` dedupes comments via `HashSet<usize>` on `span.start`; doc comments often share start positions in groups. **Fix**: dedupe by `(span.start, span.end)` or explicit `comment_id`.

## LOW

- **`codegen/cranelift/emit.rs:584`** — `TrapCode::user(1).unwrap()` — `1` hardcoded with no name. **Fix**: `const RUXEN_UNREACHABLE_TRAP: u8 = 1;` with comment.
- **`codegen/cranelift/emit.rs:640`** — `BinOp::Mod => builder.ins().srem(lhs, rhs)` on floats. Comment admits "will fail verifier if it ever runs". Reachable from user `f64 %`. **Fix**: reject float `%` at typeck with a dedicated error, or implement via `fmod` runtime call.
- **`async_lowering/mod.rs:3120`** — `_ => unreachable!("guarded above")` correct today but the guard at 3102 is 18 lines away. **Fix**: factor match into a fn returning `Option<&mut Vec<Expr>>` so guard and use are adjacent.
- **`codegen/llvm/emit/instructions.rs:310, 349, 410`** — out of slice but three `unsafe` GEP-style address blocks; worth a follow-up. Each needs a doc-comment naming the invariant.

## Architectural summary

- **Cranelift backend in good shape post-split — the real risk is `declared_fns: HashMap<String, FuncId>` shared table coupled with suffix-matching fallback and call-site-inferred sigs.** Three paths can mint a `FuncId` for the same name with three different signatures (suffix fallback / runtime_sigs / call-site inference); first writer wins. Monomorphization upstream should resolve callees by `(DefId, generic_args)` and never hand codegen a `?T*` name. The suffix-match arms in `translation_env.rs:86-135` and `lang_intrinsics.rs:337-369` are scar tissue from generics that don't yet monomorphize cleanly.
- **`async_lowering/mod.rs` is a god file that needs splitting before sub-phase 4 lands.** Three E11XX diagnostic visitors are ~900 LOC of pure boilerplate — a single `AsyncAstVisitor` trait + driver collapses them 10×. State-machine builder deserves its own file with the segmenter contract documented as a state diagram in module-level rustdoc.
- **The formatter's comment subsystem is the buggiest part of the slice.** `comments.rs` rescans source as `Vec<char>` (~4× memory); `find_preceding_node` is O(N·M) instead of O(M·log N); method-chain emission silently drops attached comments (`format_expr.rs:673`); no test for nested-block comments crossing format-off/on. Before v1 freeze: rewrite the attacher over `BTreeMap` of node spans with property tests asserting (a) every collected comment ends in exactly one of leading/trailing/dangling and (b) no comment is dropped between collect and emit.

---

# 7. ruxenc + ruxen_cli

`src/ruxenc/`, `src/ruxen_cli/`

## CRITICAL

- **`src/ruxen_cli/src/resolve_deps.rs:256`** — `git clone <git_url>` argument-injection (see §1.2).
- **`src/ruxen_cli/src/resolve_deps.rs:269`** — `git checkout <ref>` argument-injection (see §1.2).
- **`src/ruxen_cli/src/resolve_deps.rs:187-202`** — path-dep traversal (see §1.2).
- **`src/ruxenc/src/bench.rs:142-150`** — synthesised `def main` text-splicing (see §1.2).

## HIGH

- **`src/ruxenc/src/cache/driver.rs:299-369`** — driver's "pragmatic" first pass invokes `compile_fn` once just to discover dependencies, then again per cascade. The second `compile_fn` inside cascade BFS at line 470 recompiles dependents already in `outputs`, doubling work. **Fix**: thread `out.dependencies` through a single-pass topo; gate cascade compiles by `dirty_order` membership. Add `compiles_per_file: u32` counter to `BuildResult` so tests lock "≤ 1 compile per file per build".
- **`src/ruxenc/src/cache/driver.rs:374-384`** — re-reads previous object bytes from disk to hash them to compute `output_changed`. Hash is already stored — `prior_entry.object_file` keyed by content-addressed `cache_key`. **Fix**: `output_changed = new_key != prior.cache_key`; drop the disk read.
- **`src/ruxenc/src/compile.rs:158`** — `fs::read(obj_path)` re-reads the just-cached object only to hand to `emit_executable`; `compile_fn` already returned `object_bytes`. **Fix**: extend `BuildResult.objects` to carry `Vec<u8>` (or `Either<Vec<u8>, PathBuf>` for cache-hit path).
- **`src/ruxenc/src/compile.rs:240`** — `load_bootstrap_or_err()` called per file via `compile_to_object`; `stdlib_bootstrap::run_bootstrap` re-lexes/re-parses every `.rx` in `library/std/` on each call. O(files × stdlib_size). **Fix**: hoist bootstrap loading to outer `compile::run`; pass `Arc<Vec<Program>>` through `BuildOptions`.
- **`src/ruxenc/src/bench.rs:200-218`** — timeout loop polls `try_wait` every 100 ms; `child.kill()` sends SIGKILL to only the child, not its group; benches that spawn subprocesses leak them. No RSS cap despite the 8-GiB standing requirement (memory `feedback_memory_limits.md`). **Fix**: spawn into a new process group (`Command::process_group(0)`); `kill(-pid, SIGKILL)` on timeout; honour `RUXENC_BENCH_RSS_LIMIT` via `setrlimit(RLIMIT_AS)` `pre_exec`.
- **`src/ruxen_cli/src/build.rs:573`** — `fs::write(&obj_path, &obj_bytes)` writes a dep's `.o` to `target/deps/<name>.o` without atomic rename. Concurrent builds truncate each other mid-link. **Fix**: route through `ruxenc::cache::store::atomic_write` (already public).
- **`src/ruxenc/src/compile.rs:74`** — `output_path = path.replace(".rx", "")` naive: `foo.rx.rx` → `foo.`; `/home/u/.rx/main.rx` → `/home/u/main`. **Fix**: `Path::new(path).with_extension("")`.

## MEDIUM

- **`src/ruxen_cli/src/main.rs:104-110`** — `cli::Command::Test` is a `process::exit(2)` stub; a registered clap subcommand that always fails, bypassing the unified error path. **Fix**: return `Err("ruxen test: not yet implemented …".into())`; remove `process::exit(2)`.
- **`src/ruxen_cli/src/cli.rs:131`** — `Compile { extra: Vec<String> }` with `allow_hyphen_values + trailing_var_arg` handed to `ruxenc::compile::run` is stringly-typed argument forwarding. New flags accepted; unknown silently dropped (see next). **Fix**: shared clap derive struct, or error on unknown `--flag` tokens.
- **`src/ruxenc/src/compile.rs:69-71`** — unknown CLI flags silently dropped (loop falls through to `i += 1`). `ruxenc foo.rx --realese` (typo) silently produces a debug build. **Fix**: `else if args[i].starts_with("--") { return Err(format!("unknown flag: {}", args[i])); }`.
- **`src/ruxenc/src/fmt.rs:130-166`** — `discover_rx_files` unbounded recursion; `target/` symlink loop spins forever. **Fix**: `walkdir::WalkDir` with `follow_links(false)` and depth cap, or visited-canonical-path set. `target/` filtered by basename but not canonical.
- **`src/ruxenc/src/compile.rs:484-497`** — `project_target_ruxen()` walks parents looking for `Cargo.toml` as well as `ruxen.toml`. Nested-rust-crate ancestor anchors ruxen cache wrong. **Fix**: anchor only on `ruxen.toml`; drop `Cargo.toml` fallback.
- **`src/ruxenc/tests/installed_binary.rs:111-118` + `src/ruxen_cli/tests/installed_pkg_manager.rs:25-27`** — `runtime_c_src()` dead per clippy. **Fix**: delete.
- **`src/ruxenc/src/bench.rs:198, 219; src/ruxenc/src/compile.rs:73`** — `cargo fmt --all -- --check` reports unformatted code at these lines. **Fix**: `cargo fmt` and commit.
- **`src/ruxen_cli/src/resolve_deps.rs:88-95`** — cycle-detection iterates a `HashSet` to build cycle path; non-deterministic ordering across runs. **Fix**: track `in_flight` as `Vec<String>` (it's a stack by usage).
- **`src/ruxen_cli/src/resolve_deps.rs:250`** — `let short_hash = &cache_key[7..15]` slices SHA-256 hex with hardcoded "skip `sha256:`" offsets. If prefix changes to `blake3:`, slices into the middle of a hash. **Fix**: `cache_key.strip_prefix("sha256:").unwrap_or(&cache_key)[..8]`.
- **`src/ruxenc/src/cache/mod.rs:29`** — `#![allow(dead_code, unused_imports)]` at module scope hides clippy signal for 2200+ LOC. **Fix**: narrow to specific items.
- **`src/ruxen_cli/src/build.rs:584-585`** — `output_path.to_string_lossy().to_string()` to `codegen::compile_with_options`. Loss of non-UTF8 path bytes on Linux. **Fix**: pass `&Path` through.
- **`src/ruxen_cli/src/build.rs:148`** — `ruxen run` always passes `locked: false`; no `--locked` forwarding. **Fix**: forward `--locked` through `Run`.

## LOW

- **`src/ruxenc/src/cache/hash.rs:106-112`** — `to_hex` per-byte `push_str(&format!("{:02x}", b))`. **Fix**: `write!(s, "{b:02x}")`.
- **`src/ruxen_cli/src/scaffold.rs:40, 45, 115, 120`** — generated stub uses brace syntax `def main { puts(...) }` while docs use `def main\n  …\nend`. **Fix**: pick one canonical form.
- **`src/ruxenc/src/compile.rs:428`** — `_opt_level: u8` underscore-prefixed but used under `#[cfg(feature = "llvm")]`. **Fix**: drop underscore; add `#[allow(unused_variables)]` for non-llvm cfg.
- **`src/ruxenc/src/fmt.rs:168-216`** — hand-rolled unified-diff printer with off-by-one risks. **Fix**: use `similar::TextDiff::from_lines(orig, fmt).unified_diff()`.
- **`src/ruxen_cli/src/lock.rs:54-80`** — hand-built TOML emitter for `Ruxen.lock`. Struct derives `Serialize`. **Fix**: `toml::to_string_pretty`.
- **`src/ruxen_cli/src/deps.rs:89, 93-98`** — interactive prints embedded in business-logic functions; same function that mutates `Ruxen.toml` also prints. Hard to test/silence. **Fix**: thread `quiet: bool` through, or return a `BuildReport`.
- **`src/ruxenc/src/main.rs:33`** — `_ => ruxenc::compile::run(&args)` makes typoed subcommand (`ruxenc buld foo.rx`) parse as positional file path. **Fix**: reject `args[1]` starting with a letter and not ending in `.rx` with a "did you mean?" hint.

## Architectural summary

- **The driver split is a clean library/binary cut.** Both binaries route through `ruxenc::{compile,fmt,bench,clean}::run(&[String])`. The re-marshalling at the binary boundary is the weakest seam: every flag is now stringly-typed across two parsers. Next refactor: shared clap derive struct rather than ad-hoc `while i < args.len()` loops in `compile.rs`/`bench.rs`/`fmt.rs`.
- **The security boundary is `resolve_deps.rs`, and it is currently soft.** Three of four CRITICAL findings live there. The compile/format codepath itself is reasonably hermetic (content-addressed cache keys, atomic writes, no env reads on hot path) — but the dep resolver shells out to `git` with attacker-controlled argv and reads arbitrary filesystem paths. Treat `resolve_deps` as an external-input boundary; every field of `DependencyDetail` is untrusted; `--` separator on every `git` invocation; absolute paths behind explicit opt-in.
- **The incremental cache layer is well-shaped but over-eager on I/O.** Content-addressing, hermetic keys, fail-open corruption handling, atomic writes are all done right and tested. Wasteful: `output_changed` computed by re-reading and re-hashing prior bytes when key-equality suffices; objects written to store then re-read for linking; stdlib bootstrap re-parsed per file. `BuildResult` carrying `Vec<u8>` for fresh artifacts + bootstrap hoisted to `compile::run`'s prelude lands most of the win without changing cache invariants.

---

# 8. ruxen_lsp + ruxen_ide

`src/ruxen_lsp/`, `src/ruxen_ide/`

## CRITICAL

- **`src/ruxen_lsp/src/server.rs:118, 165`** — `ruxen_ide::analysis::analyze(&source)` runs the full lex+parse+typeck+borrow-check pipeline **synchronously inside the async handler**. For a 5–10K LOC file this is hundreds of ms; concurrent didOpen + completion + hover queue on the tokio worker. Editor freezes. **Fix**: `tokio::task::spawn_blocking(move || analyze(&source)).await` (or dedicated bounded worker).
- **`src/ruxen_lsp/src/server.rs:138-152`** — `did_change` mutates `doc.source` but does NOT bump or invalidate `analysis`. Every Wave-1/2 handler reads stale `analysis` against new `doc.source`. `rename.rs:389` (`span.end as usize > source.len()`) is the only thing keeping edits from landing at wildly wrong offsets. **Fix**: on did_change either (a) clear `doc.analysis = None`, or (b) trigger debounced re-analyze. "Analysis happens on didSave" is not an excuse to leave source/analysis desynced; break the invariant explicitly.
- **`src/ruxen_ide/src/goto_def.rs:36` and `src/ruxen_ide/src/type_def.rs:74`** — `Url::parse("file:///placeholder").unwrap()` on production path. The LSP handler at `server.rs:239-242` and `server.rs:388-392` overwrites the URI, so the parse is pure tax per request and an `unwrap` on the LSP path. **Fix**: return `(Range,)` only; let the server build the `Location`. Or `OnceLock`.
- **`src/ruxen_ide/src/references.rs:88`** — same `Url::parse("file:///__placeholder__").expect(...)` pattern, allocated per request, rewritten at `server.rs:425`. **Fix**: same.
- **`src/ruxen_ide/src/hover.rs:176`** — `.expect("needle not found in source")`. Same pattern at **`src/ruxen_ide/src/node_finder.rs:469`**. **Fix**: isolate test helpers into a `pos_helper` mod gated by `#[cfg(test)]`.

## HIGH

- **`src/ruxen_ide/src/completion.rs:128, 219, 249, 281`** — `word_start_completions` + `after_dot_completions` do FOUR linear `symbols.iter()` scans (locals/keywords, methods, fields, module items). With bootstrap-merged stdlib `SymbolTable` is low thousands; single keystroke is `O(|symbols| × 4)` plus `def.name.clone()` allocations. **Fix**: prefix trie or `HashMap<char, Vec<DefId>>` built once per analysis; serves rename/references/highlight too.
- **`src/ruxen_ide/src/semantic_tokens.rs:60-66`** — `collect_lexical_tokens` re-runs the lexer per `semantic-tokens-full` request. **Fix**: cache tokens on `AnalysisResult` (lexer produced them during `analyze`; just don't drop).
- **`src/ruxen_ide/src/workspace_symbols.rs:32, 207`** — `name.to_lowercase().contains(needle_lower)` allocates fresh lowercased String per emitted symbol per document per keystroke. **Fix**: `eq_ignore_ascii_case`-style on bytes, or precompute `name_lc` once per definition.
- **`src/ruxen_lsp/src/server.rs:302-316`** — `symbol()` collects `Vec<(Url, &AnalysisResult)>` per request by cloning every URI; iteration walks every doc's HIR. O(N · |HIR|) per keystroke in the picker. **Fix**: incrementally-updated workspace symbol index on `ServerState`; rebuild only on did_save/did_open/did_close.
- **`src/ruxen_ide/src/code_actions.rs:148-178`** — `find_let_token_for_name` does `rfind("let")` on a string slice. `let` matches **inside string literals and comments** because the lexer's token stream is not consulted; `# let x` in a comment is found. Same at `find_enclosing_def` for `def`. **Fix**: walk the cached token stream; match `TokenKind::Let`/`TokenKind::Def`.
- **`src/ruxen_ide/src/rename.rs:800-832`** — `narrow_to_identifier` does substring search inside host span. Same comment/string-literal hazard. If a method `foo` is called as `c.foo()` and the surrounding span includes `"# foo"` or `"foo"`, first-occurrence branch lands on wrong byte. **Fix**: intersect lexer's `Ident` tokens with host span; take first whose lexeme equals `name`.
- **`src/ruxen_ide/src/rename.rs:139-227` + `highlight.rs` + `references.rs`** — three duplicated `MethodCallFinder` walkers (rename.rs 499-717, references.rs 150-407, highlight.rs same shape). ~200 LOC each with subtle drift (references walks `HirItem::Mixin`, rename does not). **Fix**: hoist a shared `hir_walk` module parameterised by callback or trait.

## MEDIUM

- **`src/ruxen_ide/src/use_index.rs:707-709`** — `resolve_struct_method` is a stub returning `None`. Struct method renames silently no-op because `UseIndex` never records struct method uses. **Fix**: implement (walk struct's `impl_blocks`), or have rename refuse early with an explanation.
- **`src/ruxen_lsp/src/server.rs:154-178`** — `did_save` re-reads `(source, version)` under read lock, drops it, runs `analyze`, re-acquires write lock. Between drop and re-acquire, another did_change can land. Tiny window but real. **Fix**: hold read lock across `analyze` (via spawn_blocking with cloned source), or version-tag analysis and discard on mismatch.
- **`src/ruxen_ide/src/analysis.rs:14-27`** — `AnalysisResult` owns `source: String` (clone of doc source). With `DocumentState.source` server-side, every open doc holds two copies. 100K LOC × 20 buffers = 4 MB waste. **Fix**: `Arc<str>` shared.
- **`src/ruxen_ide/src/code_actions.rs:24`** — `#![allow(dead_code)]` at module scope hides the lint indefinitely. **Fix**: remove blanket allow.
- **`src/ruxen_ide/src/rename.rs:289-333`** — `is_reserved_keyword` hand-maintained list drifts from `ruxen_core::lexer`. **Fix**: expose `lexer::is_keyword(&str) -> bool`; add a pin test that fails when the lexer adds a keyword the LSP doesn't reject.
- **`src/ruxen_ide/src/{inlay_hints,rename,use_index,node_finder,semantic_tokens}.rs`** — five files over 500 LOC each, all containing one giant HIR visitor. Decomposition by feature is correct; inside each, visitor sprawls. **Fix**: shared `hir_walk`; feature visitors become trait impls or callbacks (~100 LOC each).
- **Clippy warnings (5 auto-fixable)**: `code_actions.rs:11`, `document_symbols.rs:86`, `rename.rs:389, 476:8, 476:66, 477`, `use_index.rs:629, 674`. **Fix**: `cargo clippy --fix -p ruxen_ide` and review.

## LOW

- **`src/ruxen_lsp/src/server.rs:282-298`** — `let Some(doc) = … else { return Ok(None); };` repeated 14×. **Fix**: extract `with_analysis(&self, &uri, f)` helper.
- **`src/ruxen_ide/src/completion.rs:548-554`** — `KEYWORDS` hardcoded list duplicating `lexer::is_keyword`; same drift hazard.
- **`src/ruxen_ide/src/line_index.rs:8`** — `source: String` owned again. Third copy. **Fix**: `Arc<str>` or `&str`.
- **`src/ruxen_ide/src/rename.rs:117-126`** — "last-ditch alt lookup" returns `Some(word_span_to_range)` based on the original word span without ever calling rename on `alt`. `prepare_rename` says "yes" but subsequent `rename` resolves to a different def. Surface asymmetry — file a bug or remove.
- **`src/ruxen_ide/src/completion.rs:46-47`** — `_trigger: Option<char>` unused. **Fix**: use it (`if trigger == Some('.')` skip context classification) or remove.

## Architectural summary

- **Decomposition is genuinely good for a 12K-LOC IDE crate**: one module per LSP capability, thin `analysis.rs` owning the pipeline, shared `node_finder` + `use_index` doing heavy lifting once. The LSP server is a clean adapter — handlers are ~10 lines each. Placeholder URIs in the IDE crate + rewrite at the LSP boundary is a smell, but the boundary itself is clean.
- **The biggest soundness gap is source/analysis desync on did_change combined with full pipeline analysis happening synchronously on the async runtime worker.** Together: (a) LSP can freeze the editor on a large file, and (b) every Wave-1/2 feature reads stale data between save events. Fix both in order: spawn_blocking the analyze call, then invalidate-on-edit or debounce-reanalyze.
- **The four HIR visitors duplicated across rename/highlight/references/use_index/completion (~800 LOC repetition) are this crate's biggest maintenance liability.** A single `hir_walk` infrastructure with closure or trait dispatch would cut ~1500 LOC and remove the drift between rename.rs and references.rs.

---

# 9. ruxen_repl

`src/ruxen_repl/`

## CRITICAL

- **`jit.rs:731`** — non-exhaustive `MirInst` match (see §1.1).
- **`jit.rs:178-188`** — `dlsym(RTLD_DEFAULT)` resolves arbitrary process symbols (see §1.2).
- **`session.rs:92`** — `self.jit = JITCodeGen::new()?;` drops old `JITModule` without `free_memory()`. Cranelift's `JITModule::Drop` deliberately does not free executable memory (outstanding pointers might be live). REPL doesn't hand pointers across resets — old executable pages leak on every `:reset`. Unbounded growth in long sessions. **Fix**: before reassign, take ownership of the old module and `unsafe { old_jit.module.free_memory() }`. Document the invariant (no callable function pointers may outlive the JIT module) at `compile_repl_input`'s return site.
- **`eval.rs:529-582`** — fourteen `unsafe { transmute(code_ptr) }` blocks form the trust boundary between typechecker `return_ty` and JIT ABI. Match arms must stay in lockstep with `ty_to_cranelift` (jit.rs:1372). **Fix**: extract `transmute_and_call(code_ptr, &cranelift_ty) -> i64` keyed off the *Cranelift* type (ABI source of truth). Today the two enums are walked twice and the invariant lives in review.
- **`jit.rs:1402`** — `Ty::Tuple(_)` and `Ty::FixedArray(_, _)` both map to `types::I64` (single pointer width). Inline-on-stack tuples are wrong — `(Int, Int)` is 16 bytes. If REPL ever sees a tuple result, silent truncation. **Fix**: return `None` and emit "unsupported in REPL", or implement aggregate return via stack-slot pointer like batch codegen.

## HIGH

- **`eval.rs:133, 162, 219, 282-300`** — every input rebuilds the full program (`session.all_statements.clone()` + every `func_def` + every `type_item`) and re-runs typecheck + borrow-check + MIR lowering. After N inputs: O(N²). Already-typechecked, already-lowered defs not cached. **Fix**: cache typeck and MIR per def, invalidating only when a referenced def changes.
- **`jit.rs:914-916`** — `get_or_declare_func` re-walks `declared_fns.keys().filter(...).min_by_key(len)` for `?`-prefixed and generic-param resolution. O(K) per call per wrapper compile. **Fix**: `HashMap<method_suffix, FuncId>` index alongside `declared_fns`.
- **`capture.rs:18`** — process-wide `Mutex<String>` for capture. Panic inside JIT'd code holding the lock poisons the mutex; surrounding `if let Ok(mut buf) = BUFFER.lock()` arms silently drop output. REPL appears "stuck on no output". **Fix**: use `.lock().unwrap_or_else(|p| p.into_inner())` consistently (matching test pattern at 127, 259); clear on entry.
- **`eval.rs:604-622`** — capture-diff via `captured.starts_with(&session.prev_captured_output)`. If any prior `puts` produced non-deterministic output (timestamp, address, random), prefix match fails and fallback at 608 dumps the entire cumulative capture, double-printing everything. **Fix**: count `puts` invocations (deterministic by source) instead, or stash a per-input marker token via a synthetic prologue.
- **`jit.rs:617-639`** — generic type-param resolution heuristic: "name starts with up-to-2 uppercase ASCII letters". Will mis-resolve user functions starting with `Db_foo`/`Io_read`/`Fs_open`. **Fix**: route generic-param dispatch through symbol-table metadata from `ruxen_core`, or require an explicit marker like `__GENERIC__T_method`.
- **`jit.rs:1286`** — float `%` falls back to `srem` (integer). Comment "will fail verifier if it ever actually runs" — defending on a downstream check is brittle. **Fix**: return explicit `Err("\`%\` not supported on floats — use \`.rem_euclid\`/\`.fract\`")`.
- **`eval.rs:251-253`** — diagnostic message-substring sniffing (`d.message.contains("could not infer") || d.message.contains("type mismatch")`) to decide "defer this def". Couples REPL to typechecker prose. **Fix**: structured error code (`E0282` / similar); key off `d.code`.

## MEDIUM

- **`jit.rs` (1540 LOC)** — mixes `JITCodeGen`, instruction translation, terminator translation, runtime symbol registration, signature tables, Ty↔Cranelift mappings. **Fix**: split into `jit/mod.rs`, `jit/translate.rs`, `jit/runtime_table.rs`, `jit/ty_lower.rs`.
- **`jit.rs:1476-1539`** — `runtime_signature` is a 65-arm `match name` duplicating `extern "C"` declarations at 28-117. Two sources of truth that must agree. Already drifted. **Fix**: single `const RUNTIME: &[(name, &[Type], Option<Type>)]` table; derive both registration loop and signature lookup.
- **`main.rs:41-374`** — `split_repl_chunks` is 333 LOC with a nested simulator, three pass walks, several heuristics. Duplicates work `validate.rs:40-72` does with different rules. **Fix**: expose `Parser::needs_continuation(tokens) -> bool` in `ruxen_core`; call from both.
- **`main.rs:5-17`** — six `#[allow(dead_code)]` attributes. Either dead (rule 35: delete) or test-only (`#[cfg(test)]`). **Fix**: remove blanket suppression; run `cargo check`; delete or gate properly.
- **`env.rs:1-110`** — `ReplEnv` largely unused by the eval pipeline; `mark_moved`/`live_variables`/`all_states` have no non-test caller. Dead code masquerading as future work. **Fix**: delete `ReplEnv` entirely, or write the spec for what it's going to do and add at least one in-tree caller.
- **`eval.rs:74`** — `EvalResult::Error(String)` discards structured diagnostic info (span, level, code). **Fix**: carry `Vec<Diagnostic>`; stringify only at the print boundary.
- **`display.rs:141`** — fallback `_ => format!("{:?}", ty)` for non-exhaustive type rendering means new `Ty` variants silently surface as `Debug` to end users. **Fix**: exhaustive match with explicit "unsupported in REPL" arms; unit test exercising every variant.

## LOW

- **`session.rs:57`** — `Option<&Box<dyn Any>>`. clippy `borrowed_box`. **Fix**: `Option<&dyn Any>`.
- **`main.rs:5-19`** — `mod` ordering interleaved with attributes. **Fix**: one block of mods then `use`.
- **`jit.rs:140-203`** — `JITCodeGen::new` returns `Result<Self, String>` via `format!`. Universal error type is `String` crate-wide. **Fix**: `ReplError` enum (Cranelift, Lex, Parse, Typeck, Runtime, …) — unblocks the HIGH about discarding structured diagnostics.
- **`jit.rs:1153-1158`** — `TrapCode::user(1).unwrap()` — `1` hardcoded; copy-paste of `user(0)` panics. **Fix**: named constant.
- **`eval.rs:822-906`** — test module uses bare `panic!("expected ...")`. **Fix**: `assert!(matches!(...))`.
- **`jit.rs:1407`** — `Ty::ConstArg(_) => None` silently coerces to i64 register. **Fix**: explicit `unreachable!("ConstArg in value position")` with comment.

## Architectural summary

- The REPL's "cumulative replay" strategy (every input re-compiles all prior statements) is correct but O(N²); it papers over the absence of an incremental compilation surface in `ruxen_core`. The real fix lives there; REPL can only memo per-def on top.
- `jit.rs` is a parallel-but-not-shared codegen path beside `compiler/ruxen_core/src/codegen/cranelift.rs`. The non-exhaustive `MirInst` match is the visible symptom; every new MIR variant has to be implemented twice with no compile-time link. Right structural move: factor MIR→Cranelift translation into a `trait Backend { fn module(&mut self) -> &mut dyn Module; … }` consumed by both `ObjectModule` (batch) and `JITModule` (REPL).
- Trust boundaries are loose: `dlsym(RTLD_DEFAULT, ...)` resolves arbitrary process symbols; capture-diff is byte-prefix; type-inference deferral is diagnostic-substring. Each is small but together they make REPL semantics fragile under unrelated changes (libc shadowing, diagnostic rewording, non-deterministic output). Tightening to typed contracts (allowlist, structured diagnostic codes, marker tokens) is the highest-leverage hardening work.

---

# 10. Confirmed-still-open project memories

Cross-referencing `memory/MEMORY.md` against this review — these memories are still load-bearing (the bug hasn't moved):

| Memory | Site confirmed | Severity |
|---|---|---|
| `project_ruxen_resolve_class_stomps_typealias.md` | `resolve/types.rs:516-548` (read-side patch); `items.rs:332` (stomp unchanged) | CRITICAL |
| `project_ruxen_closure_capture_ty_stale.md` | `resolve/control_flow.rs:327` | CRITICAL |
| `project_ruxen_drop_name_mismatch.md` | `mir/lower/collect.rs:223+` | CRITICAL |
| `project_ruxen_mir_mangled_method_name_parsing.md` | `mir/lower/mod.rs:803` | CRITICAL |
| `project_ruxen_task_spawn_ownership_gap.md` | `mir/lower/drops.rs:786-789` | CRITICAL |
| `project_ruxen_mir_two_dispatch_paths.md` | `mir/lower/expr/method_call.rs:177-394 vs 765-1283` | CRITICAL |
| `project_ruxen_for_loop_infer_gap.md` | `typeck/infer/expr.rs:622`; bleeds to `borrow_check/walk.rs:440` | HIGH |
| `project_ruxen_async_compiler_gaps.md` gap #3 (String==) | `typeck/infer/ops.rs:130` | HIGH |
| `feedback_memory_limits.md` (8 GiB RSS cap) | `ruxenc/src/bench.rs:200-218` (no rlimit) | HIGH |
| `project_ruxen_caller_identity_as_arg.md` | `resolve/mod.rs:97-174` (17 fields not yet structured) | MEDIUM |

---

# 11. Recommended action order

## Wave 1 — same day

1. Add `MirInst::DataAddr` arm to `src/ruxen_repl/src/jit.rs:731`.
2. `cargo fmt` — unblocks `--check`.
3. Delete `runtime_c_src` dead helpers (`src/ruxenc/tests/installed_binary.rs:111`, `src/ruxen_cli/tests/installed_pkg_manager.rs:25`).
4. Run `cargo clippy --fix -p ruxen_ide` and review the 5 auto-fixable lints.

## Wave 2 — this week (security)

5. Insert `--` separator on every `git` invocation in `resolve_deps.rs` (lines 256, 269).
6. Reject leading `-` and non-`https/ssh/git@` schemes in dep URLs.
7. Refuse `..`-escaping or absolute path-deps unless `--allow-external-path` is set.
8. Allowlist-gate the REPL `dlsym(RTLD_DEFAULT)` fallback to `ruxen_*` + a small explicit set.
9. Validate `bench_names` against `[A-Za-z_][A-Za-z0-9_]*` before splicing in `bench.rs:142-150`.

## Wave 3 — next sprint (correctness)

10. Fix tuple-float-roundtrip (`parser/expr/calls.rs:72-96`).
11. Fix `__drop`/`drop` collector in `mir/lower/collect.rs:223+`.
12. Route `current_parent_class` through `class_name_from_mangled` (`mir/lower/mod.rs:803`).
13. Restrict `unify`'s Ref auto-deref to `unify_or_coerce` (`typeck/unify.rs:294-301`).
14. Thread real return-types into `CallIndirect` (`codegen/cranelift/emit.rs:484-494`).
15. Fix `derive Clone` parent-field walk (`mir/lower/derive.rs:757-793`).
16. Stop swallowing `unify` errors in `typeck/infer/expr.rs` (8 sites).
17. Fix LSP `did_change` invalidation + move `analyze()` to `spawn_blocking`.
18. Fix `simple_type_size` for Class/Struct (`codegen/cranelift/helpers.rs:148-191`).
19. Fix `JITModule` memory leak on `:reset` (`ruxen_repl/session.rs:92`).

## Wave 4 — mid-term (data-drive the allowlists)

20. Extend FFI decl syntax with `owned | borrow | move` attributes.
21. Replace `FRESH_ALLOC_CALLEES` with attribute lookup.
22. Replace ownership-transfer allowlist with attribute lookup.
23. Tag dynamic-dispatch helpers in MIR (`MirFunction { kind: FunctionKind::DynDispatchHelper }`).
24. Delete typeck wildcard arms (`method_resolvers/mod.rs:1134-1140`).
25. Add structural marker for iterator adapters; remove `ends_with("Iter")`.

## Wave 5 — architecture

26. Split `async_lowering/mod.rs` (3357 LOC) into 6 files; extract `AsyncAstVisitor` trait.
27. Split `mir/lower/expr/method_call.rs` (1291 LOC) into a `MethodLoweringStrategy` registry.
28. Split `mir/lower/derive.rs` (1339 LOC) per-trait.
29. Split `resolve/ffi_registration.rs` (1216 LOC) per item kind.
30. Extract shared `hir_walk` for `ruxen_ide`'s 5 duplicated visitors (~1500 LOC saved).
31. Extract `ResolveCtx` `Clone` struct from `Resolver`'s 17 god-object fields.
32. Factor MIR→Cranelift translation into a `trait Backend` consumed by both batch and REPL backends.

## Wave 6 — perf

33. Lexer over `&'a str` byte cursor; `TokenKind` carries `&'a str` or `Symbol(u32)`; return tokens by move.
34. Name index on `SymbolTable` to kill the 10+ `symbols.iter().find()` sites across resolve + typeck + ide.
35. Cache lexer tokens on `AnalysisResult`; serve `semantic_tokens` from cache.
36. Persistent maps (`im::HashMap`) in `borrow_check` `check_if`/`check_match` branch state.
37. Build `output_changed` from `cache_key` equality in `ruxenc/cache/driver.rs:374-384`.
38. Hoist stdlib bootstrap loading out of per-file `compile_to_object`.
39. Make `Span: Copy`; cut `async_lowering`'s 216 `.clone()` calls 30-50%.
40. Static-slice `runtime_signature` returns in `codegen/cranelift/runtime_sigs.rs`.
