# `std::fmt` Implementation Plan (Prompt #06 — fmt subset)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver the `std::fmt` portion of `docs/prompts/v1/06_phase2_stdlib_io_fmt.md` — `Display` + `Debug` traits, `Formatter`, `fmt::Error`, interpolation routed through `Display::fmt`, and `"#{x:?}"` debug interpolation — without regressing any existing test.

**Architecture:** Build the trait/type surface first as a non-disruptive addition (Phase A). Then thread a `FormatSpec` through lexer → parser → HIR → MIR (Phase B). Wire the `:?` debug path on top of the existing synthesized `{Name}_to_debug` formatter (Phase C). Finally, refactor the canonical interpolation path so every part goes through `Display::fmt`, synthesizing `Display` impls for primitives and stdlib types (Phase D). Each phase ships green tests + commits independently.

**Tech Stack:** Rust workspace (`riven-core`, `rivenc`, `riven-cli`), C runtime (`crates/riven-core/runtime/runtime.c`), `.rvn` E2E fixtures under `tests/release-e2e/cases/`.

**Universal rules apply** — see `docs/prompts/00_universal_rules.md`. Highlights: TDD red→green→refactor; no `#[ignore]` / `riven_noop_passthrough`; `cargo test --workspace` green at every commit; new error codes go in `crates/riven-core/src/diagnostics/codes.rs::REGISTRY`; CHANGELOG bullet per user-visible change; `##` doc comment on every new public surface.

---

## Phase A — Foundation: trait + type surface (Day 1)

**Goal:** `Display`, `Debug` (formal), `Formatter`, `fmt::Error` are resolvable types/traits. No runtime semantics yet — just plumbing tests parse and typecheck.

### Task A1: Register `Display` built-in trait

**Files:**
- Modify: `crates/riven-core/src/resolve/mod.rs` (built-in trait list ~line 182-202)
- Test: `crates/riven-core/tests/stdlib_fmt.rs` (new)

- [ ] **Step 1: Write failing test**

```rust
// crates/riven-core/tests/stdlib_fmt.rs
//! Phase 2 #06 — `std::fmt` surface tests.

use riven_core::lexer::Lexer;
use riven_core::parser::Parser;
use riven_core::typeck;

fn typecheck(src: &str) -> Vec<riven_core::diagnostics::Diagnostic> {
    let mut lx = Lexer::new(src);
    let toks = lx.tokenize().expect("lex");
    let mut p = Parser::new(toks);
    let prog = p.parse().expect("parse");
    let r = typeck::type_check(&prog);
    r.diagnostics
        .into_iter()
        .filter(|d| d.level == riven_core::diagnostics::DiagnosticLevel::Error)
        .collect()
}

#[test]
fn display_trait_is_resolvable() {
    let src = r#"
        struct Money
          cents: Int
          impl Display
            def fmt(&self, f: &mut Formatter) -> Result[(), fmt::Error]
              Ok(())
            end
          end
        end

        def main
          let _m = Money.new(100)
        end
    "#;
    let errs = typecheck(src);
    assert!(errs.is_empty(), "expected no errors, got {:?}", errs);
}
```

- [ ] **Step 2: Run — expect FAIL (Display unknown)**

```bash
cargo test -p riven-core --test stdlib_fmt display_trait_is_resolvable
```

Expected: error referring to unknown trait `Display` or unknown type `Formatter`.

- [ ] **Step 3: Implement minimal**

In `crates/riven-core/src/resolve/mod.rs::register_builtins`:

```rust
let builtin_traits = [
    ("Displayable", vec!["to_display"]),
    ("Display", vec!["fmt"]),     // ← NEW
    ("Debug", vec!["fmt"]),       // ← UPDATED (was vec![])
    // …existing entries…
];
```

Also register `Formatter` as a built-in class type (`DefKind::Type` with `Ty::Class`) and `fmt::Error` as a placeholder enum or class (a unit struct is fine for v1).

- [ ] **Step 4: Run — expect PASS**

- [ ] **Step 5: Commit**

```
git add crates/riven-core/src/resolve/mod.rs crates/riven-core/tests/stdlib_fmt.rs
git commit -m "feat(stdlib): register Display + Debug + Formatter built-in surface (#06.A1)"
```

### Task A2: Negative test — non-trait `Display::fmt` mismatch

**Files:**
- Modify: `crates/riven-core/tests/stdlib_fmt.rs`

- [ ] **Step 1: Failing test**

```rust
#[test]
fn display_fmt_signature_mismatch_errors() {
    let src = r#"
        struct Money
          cents: Int
          impl Display
            def fmt(&self) -> String  # wrong signature
              "x"
            end
          end
        end
    "#;
    let errs = typecheck(src);
    assert!(!errs.is_empty(), "expected diagnostic for bad Display::fmt signature");
}
```

- [ ] **Step 2: Confirm RED.** May already pass via existing trait-impl signature check; if it does, the test is correctly demonstrating existing behavior — promote to a "behavior-locked" assertion and continue. Otherwise, add the check in `typeck::traits` impl-vs-decl signature compare.

- [ ] **Step 3: GREEN, commit.**

### Task A3: `Formatter` as a class with surface

**Files:**
- Modify: `crates/riven-core/src/resolve/mod.rs` (built-in class registry)
- Modify: `crates/riven-core/src/typeck/infer.rs::builtin_method_type` (add Formatter methods)
- Test: `crates/riven-core/tests/stdlib_fmt.rs`

Surface (per prompt):
- `Formatter::write_str(&mut self, s: &str) -> Result[(), fmt::Error]`
- `Formatter::write_char(&mut self, c: Char) -> Result[(), fmt::Error]`
- read-only fields: `width: Option[USize]`, `precision: Option[USize]`, `align: Char`, `fill: Char`

- [ ] Failing test: program defines `impl Display for Money` whose `fmt` body calls `f.write_str("…")`. Should typecheck.
- [ ] Implement: register methods in `builtin_method_type`. Stub the runtime semantics for now (Formatter buffers into a `String`).
- [ ] Wire `riven_fmt_formatter_*` runtime fns in `runtime.c` (write_str = string concat into internal buf; write_char similarly).
- [ ] Green, commit.

### Task A4: `fmt::Error` enum

**Files:**
- Modify: `crates/riven-core/src/resolve/mod.rs` (enum registry alongside Option/Result)
- Test: `crates/riven-core/tests/stdlib_fmt.rs`

- [ ] Failing test: returning `Err(fmt::Error)` from a `fmt` impl typechecks.
- [ ] Implement: register `fmt::Error` as a unit-variant enum (`fmt::Error` only — no variant payload for v1, matching Rust's API).
- [ ] Green, commit.

---

## Phase B — Format spec lexing (Day 2)

**Goal:** `"#{x:?}"` and `"#{x:>10}"` lex into a `StringPart::Expr { tokens, spec }` carrying an optional `FormatSpec`. No semantics yet — just the syntactic capture.

### Task B1: Add `FormatSpec` to lexer token + `StringPart`

**Files:**
- Modify: `crates/riven-core/src/lexer/token.rs` (extend `StringPart`)
- Modify: `crates/riven-core/src/lexer/mod.rs` (`lex_interpolation_expr`)
- Modify: `crates/riven-core/src/parser/ast.rs`, `printer.rs`
- Modify: `crates/riven-core/src/hir/nodes.rs` (`HirInterpolationPart`)
- Test: `crates/riven-core/src/lexer/tests.rs`

`FormatSpec`:

```rust
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FormatSpec {
    pub debug: bool,                  // `?`
    pub width: Option<usize>,         // `>10`
    pub precision: Option<usize>,     // `.2`
    pub align: Option<char>,          // '<', '>', '^'
    pub fill: Option<char>,
}
```

`StringPart::Expr` becomes `Expr { tokens: Vec<Token>, spec: FormatSpec }`.

- [ ] **Step 1: Failing lexer test**

```rust
#[test]
fn test_interpolation_with_debug_spec() {
    let kinds = lex_kinds(r#""val=#{x:?}""#);
    match &kinds[0] {
        TokenKind::InterpolatedString(parts) => {
            assert_eq!(parts.len(), 2); // "val=", "#{x:?}"
            match &parts[1] {
                StringPart::Expr { tokens, spec } => {
                    assert!(spec.debug, "debug flag should be set");
                    assert!(!tokens.is_empty());
                }
                _ => panic!("expected Expr part"),
            }
        }
        _ => panic!("expected InterpolatedString"),
    }
}
```

- [ ] **Step 2: Confirm RED** (likely fails: existing `StringPart::Expr(Vec<Token>)`).

- [ ] **Step 3: GREEN — implement FormatSpec parser inside `lex_interpolation_expr`.**

The lexer currently consumes tokens until the matching `}`. Track brace depth — if at depth 0 we see `:`, switch to "format spec" mode and consume until `}`. Format spec characters: `[fill][align][#][0][width][.precision][?]` (subset of Rust's grammar; v1 supports `?`, `width`, `.precision`, and `<>^` align with optional fill).

- [ ] **Step 4: GREEN — propagate through parser AST and HIR.**
  - `parser::ast::ExprKind::InterpolatedString` keeps the `Vec<StringPart>` shape — `StringPart` upgrade is transparent.
  - `HirInterpolationPart::Expr` gains a `spec: FormatSpec` companion field, OR we add a new variant `ExprWithSpec(HirExpr, FormatSpec)`. Prefer extending the existing variant: `Expr { expr: HirExpr, spec: FormatSpec }` (default = empty spec).

- [ ] **Step 5: Update lowering callers.** Every place that pattern-matches `HirInterpolationPart::Expr(e)` becomes `HirInterpolationPart::Expr { expr: e, spec }` (or `(e, spec)` tuple). Existing semantics ignore `spec`.

- [ ] **Step 6: Run `cargo test --workspace` — verify all existing tests stay green.**

- [ ] **Step 7: Commit.**

### Task B2: Lexer test — width / align / precision

- [ ] Add lexer fixtures for `"#{x:>10}"`, `"#{x:.2}"`, `"#{x:>10.2}"`, `"#{x:*<5}"` (fill `*`, align left, width 5).
- [ ] Implement parser, green, commit.

### Task B3: Negative — malformed format spec

- [ ] Failing test: `"#{x:zz}"` produces a lexer/parser diagnostic (E0XXX in lexer range — pick from E0001-E0099).
- [ ] Add error code to `diagnostics/codes.rs::REGISTRY`.
- [ ] Green, commit.

---

## Phase C — Debug interpolation `:?` (Day 3)

**Goal:** `"#{x:?}"` dispatches to `{Type}_to_debug` for any type that derives `Debug`. Non-Debug types error at typecheck.

### Task C1: Wire `:?` to existing struct `_to_debug`

**Files:**
- Modify: `crates/riven-core/src/mir/lower.rs::lower_interpolation`
- Test: `crates/riven-core/tests/stdlib_fmt.rs` + `tests/release-e2e/cases/NNN_debug_interp_struct.rvn`

- [ ] **Step 1: Failing E2E fixture**

```riven
# tests/release-e2e/cases/NNN_debug_interp_struct.rvn
struct Point
  x: Int
  y: Int
  derive Debug
end

def main
  let p = Point.new(1, 2)
  puts "raw=#{p:?}"
end
```

Expected stdout: `raw=Point { x: 1, y: 2 }`.

- [ ] **Step 2: Confirm RED** — without `:?` routing, output will be wrong (current code dispatches to `Point_to_debug` even without `:?`, so verify the test SPECIFICALLY exercises spec routing — make a counter-test where `:?` produces Debug and absence of spec produces Display).

- [ ] **Step 3: GREEN — in `lower_interpolation`, when `spec.debug` is true:**

```rust
if spec.debug {
    if let Some(struct_name) = self.struct_with_derive_debug(&effective_ty) {
        // call {Name}_to_debug
    } else if let Some(enum_name) = self.enum_with_derive_debug(&effective_ty) {
        // call {EnumName}_to_debug (synthesize if not yet present)
    } else {
        // primitive: same path as today (riven_int_to_string etc.)
        // — Debug for primitives matches Display in v1
    }
}
```

- [ ] **Step 4: Verify green (cargo test --workspace).** Commit.

### Task C2: Synthesize `to_debug` for `derive Debug` enums

**Files:**
- Modify: `crates/riven-core/src/mir/lower.rs` (new `synthesize_enum_to_debug`)

- [ ] Failing E2E fixture: enum with `derive Debug` formatted as `Variant(payload)`.
- [ ] Implement synthesis (mirror the struct path).
- [ ] Green, commit.

### Task C3: Negative — `:?` on non-Debug type errors

**Files:**
- Modify: `crates/riven-core/src/typeck/infer.rs` (interpolation handling)
- Modify: `crates/riven-core/src/diagnostics/codes.rs` (E0XXX in trait range)

- [ ] Failing typecheck test: a struct without `derive Debug` used with `:?` errors with a clear diagnostic (`E1001`-`E1099` range — borrow/trait/impl).
- [ ] Implement check in typeck for interpolation parts.
- [ ] Green, commit.

---

## Phase D — Route interpolation through `Display::fmt` (Days 4-5)

**Goal:** Every interpolation part — primitive or user type — goes through a `Display::fmt`-like dispatch. User types may `impl Display`. Backwards-compatible default for primitives.

### Task D1: Synthesize `Display` for primitives

**Files:**
- Modify: `crates/riven-core/src/mir/lower.rs`

For each primitive type (`Int`, `Float`, `Bool`, `Char`, `String`/`Str`), synthesize (or treat as built-in) a `Display::fmt` impl that delegates to the existing `riven_X_to_string`. This is a virtual dispatch table — we do not need to actually emit Riven code.

- [ ] Failing test: typecheck `let s: String = format(x); s == x.to_display()` for `x: Int`. (Need `format!` macro? Or use direct trait method call.)

Actually simpler: confirm `Int_fmt` (or equivalent canonical name) is in the codegen runtime dispatch table.

- [ ] Green, commit.

### Task D2: Refactor `lower_interpolation` to dispatch through Display

- [ ] For each non-debug part, replace the type-switch (`is_integer`/`is_float`/etc.) with a single Display-dispatch:
  - Look up `Display::fmt` for the value's type (synthesized for primitives, declared for user types).
  - Emit a call that takes a `Formatter` and produces a `String` chunk.

- [ ] Backwards compat: keep all existing E2E test outputs identical.

- [ ] Negative: type without `impl Display` errors clearly when interpolated without `:?`.

- [ ] Green workspace-wide. Commit.

### Task D3: User `impl Display`

- [ ] Failing test: a user struct with `impl Display` interpolates using user-provided `fmt` body.
- [ ] Implement: typeck/lowering recognizes user `impl Display for T` and dispatches to `T_fmt` instead of synthesized.
- [ ] Green, commit.

### Task D4: Width/align/precision in Display path

- [ ] Failing test: `"#{x:>5}"` for `x: Int = 7` produces `"    7"`.
- [ ] Implement: lowering passes `FormatSpec` into the `Formatter` constructor; runtime applies width/precision when finalizing.
- [ ] Green, commit.

### Task D5: CHANGELOG + flip DoD checkboxes

- [ ] Append `[Unreleased]` bullet to `CHANGELOG.md`: `Added: \`std::fmt\` surface — \`Display\` / \`Debug\` traits, \`Formatter\`, \`fmt::Error\`, \`#{x:?}\` Debug interpolation, format-spec width/precision/align.`
- [ ] In `docs/prompts/v1/06_phase2_stdlib_io_fmt.md` flip `[ ]` → `[x]` for: "String interpolation routes through `Display::fmt`", "`Debug` interpolation `\"#{x:?}\"` works", and the env/fs/fmt portion of "Every listed function has a positive + negative test".
- [ ] Final `cargo test --workspace` + `cargo build --workspace --all-targets` green.
- [ ] Commit + push branch.

---

## Self-review

| Spec line                                                  | Plan task |
|------------------------------------------------------------|-----------|
| `trait Display { def fmt(...) }`                           | A1        |
| `trait Debug` formal                                       | A1        |
| `Formatter` carries width/alignment/precision              | A3, B1, D4 |
| Interpolation `"#{x}"` calls `Display::fmt`                | D1, D2    |
| `Debug` interpolation `"#{x:?}"`                            | C1, C2    |
| Per-fn positive + negative tests                            | A2, A4, B3, C3 |
| `fmt::Error` enum                                          | A4        |
| String-interp `to_string` ad-hoc lowering refactored away   | D2        |
| Negative on non-Display / non-Debug interpolated types      | C3, D2    |

Risk areas:
- **Performance regression on interpolation-heavy code.** Watch `cargo bench` (if benches exist) on D2.
- **Existing `to_display` users.** `Displayable` stays — both traits coexist; `to_display` keeps returning `String` as today. Migration story (deprecate `Displayable`?) is **out of scope for this prompt**.
- **`Formatter` ownership model.** A buffered `Formatter` allocates per interpolation. Acceptable for v1; the optimizer can later inline.

## Execution

This is a multi-day plan. Two execution paths:
1. **Subagent-driven (recommended for D-Phase refactor)** — researcher + tester for each Task, coder + reviewer pipeline, commits between tasks.
2. **Inline** — execute Tasks A1 → D5 sequentially in the current session, checkpointing for the user between phases.
