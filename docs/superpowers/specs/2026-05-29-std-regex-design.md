# `std.regex` — Design Specification

**Date:** 2026-05-29
**Status:** Approved (brainstorm → spec). Awaiting implementation plan.

## Goal

Add a regex package to the Ruxen stdlib with first-class JS/Ruby-style syntax:

```ruxen
let pattern = /\d+/i              # auto-recognised regex literal
if line ~= /error: (\d+)/         # ~= reads as "matched?"
  let m = line.match(/error: (\d+)/).unwrap!
  puts "code: #{m.group(1).unwrap!}"
end
let codes = /\d+/.scan(text)      # all matches
let masked = /\bsecret\b/g.replace_all(input, "***")
```

`Regex.new(pattern, flags)` remains available for runtime-built patterns.

## Engine choice

**PCRE2** (`libpcre2-8`), declared via the regex package's `Ruxen.toml [system_libs]`. Reasoning: users reaching for the `/pat/flags` syntax expect Perl-compatible semantics — `\d`, `\w`, `\s`, non-greedy quantifiers, lookahead, named groups, backreferences. POSIX ERE would silently mismatch those expectations; a hand-rolled engine is multi-week scope creep.

PCRE2 is the same dependency pattern `library/std/net` uses for system libs. Pre-installed on virtually all Linux/macOS dev systems; minimal CI containers add `libpcre2-dev`/`pcre2` via the standard package list.

## Architecture and file layout

### New package: `library/std/regex/`

| File | Responsibility |
|---|---|
| `Ruxen.toml` | Declares the package; `[system_libs] libs = ["pcre2-8"]`. |
| `runtime/regex.c` | Thin C wrapper around PCRE2 (`pcre2_compile_8` / `pcre2_match_8` / `pcre2_substitute_8`). Exports `ruxen_regex_*` and `ruxen_match_*` symbols. |
| `src/lib.rx` | Declares `class Regex`, `class Match`, `class RegexError` with per-method FFI alias to the C symbols. |
| `tests/` | Per-method unit tests (Ruxen `.rx`). |

### Compiler changes (all in `compiler/ruxen_core/src/`)

| Area | Change |
|---|---|
| `lexer/tokens.rs` | New `TokenKind::RegexLiteral { pattern: String, flags: String }`. New `TokenKind::TildeEq` operator. Add `prev_token_starts_expr_context()` lookback helper for `/`-disambiguation. |
| `parser/ast.rs` | New `ExprKind::RegexLiteral { pattern: String, flags: String }`. New `BinOp::MatchOp` for `~=`. |
| `parser/expr/atoms.rs` | Consume `RegexLiteral` token, emit the AST node. |
| `parser/expr/binops.rs` (or wherever binop precedence is defined) | Add `~=` at the same precedence as `==` / `!=`. |
| `hir/types.rs` | **No** new `Ty::Regex` primitive. `Regex` / `Match` / `RegexError` are `Ty::Class { name: "..." }` registered via the existing class-bootstrap path that `String`/`Array`/`HashMap` use. |
| `typeck/` | `/pat/flags` types as `Ty::Class { name: "Regex" }`. `s ~= r` typechecks `s: String, r: Regex` → `Bool`. Mismatched operands → E1702 (per the diagnostic registry below). |
| `mir/lower/expr/` | `RegexLiteral` lowers to a `Call(Regex.compile_const, [literal_pattern, literal_flags])` hoisted to module-init: each literal compiles **once at program start**, the compiled handle lives in the data section as a static `ptr` — never re-compiled per evaluation. Same pattern as `String.from(literal)` const folding. |
| `diagnostics/codes.rs` | Register E1700, E1701, E1702, E1703 (see "Error registry" below). |
| `implicit_includes/` | Auto-synth `Copy`/`Clone`/`Drop` for the `Regex` and `Match` classes per existing rules. `Regex.drop` calls `pcre2_code_free_8`; `Match.drop` frees the subject-copy and ovector-copy (see "Match handle ownership" below). |

### Existing patterns this follows

| Pattern | Source of truth in repo |
|---|---|
| Per-package C runtime + lib.rx FFI aliasing | `library/std/string/src/lib.rx` (especially the ABI-notes header comment) |
| System-lib declaration | `library/std/net/Ruxen.toml` |
| Hand-rolled `Result[T,E]` returns from C | `library/std/io/runtime/file.c::ruxen_file_create` |
| Returning `Array[T]` of pointer values from C | `library/std/fs/runtime/fs.c::ruxen_fs_read_dir` |

## Lexer specification

### Regex literal token

```
RegexLiteral { pattern: String, flags: String }
```

Recognised when **both** of:
1. Current byte is `/`, AND
2. The most recently emitted non-trivia token (skipping `Newline`, line comments, and block comments) belongs to the expression-context set below — OR there is no previous token (start of file).

**Expression-context token set** (where `/` opens a regex literal):

```
Eof
Newline
LParen LBracket LBrace
Comma Semicolon Colon FatArrow Arrow
Eq EqEq NotEq Lt Gt LtEq GtEq
Plus Minus Star SlashEq Percent
AmpAmp PipePipe Bang
TildeEq                                       # new this work
Keywords: if while match return when then else elsif in do unless
```

Anything else → `/` is `Slash` (division) or `SlashEq`.

### Pattern body scan

From the opening `/`, the lexer advances byte-by-byte. The body terminates at the next `/` that is **all of**:

- not the immediately-following byte (empty pattern `//` is E1703 below)
- not preceded by an unescaped `\` (so `\/` does NOT close the literal — the `\/` is kept verbatim in the pattern body and forwarded to PCRE2)
- not inside an open `[ … ]` character class (the lexer tracks bracket depth; a `/` inside `[…]` does not close the literal)

If end-of-file or end-of-line is reached before the closing `/`, emit **E1701 "unterminated regex literal"** and recover at the next newline.

### Flag suffix

After the closing `/`, consume contiguous ASCII letters as flag chars. Allowed set: `i m s g x`. Each may appear at most once. Any other ASCII letter → **E1700 "unrecognised regex flag"** pointing at the offending character. Trailing non-letter characters end the flag suffix normally.

### `~=` operator

`Tilde` already exists in the token table as unused. Add `TildeEq` as a paired two-character operator. Lex `~=` greedily before falling through to `~`.

## Parser / typeck

### AST nodes

```rust
// parser/ast.rs
pub enum ExprKind {
    // ... existing ...
    RegexLiteral { pattern: String, flags: String },
}

pub enum BinOp {
    // ... existing ...
    MatchOp,        // ~=
}
```

### Precedence

`~=` at the same precedence as `==` / `!=` (the equality tier). Left-associative.

### Typing rules

| Construct | Typing |
|---|---|
| `/pat/flags` | `Ty::Class { name: "Regex", generic_args: [] }`. The pattern + flags strings are validated at compile time (call into PCRE2 from the typecheck pass to catch invalid patterns before MIR lowering — error E1704). |
| `s ~= r` where `s: String, r: Regex` | `Ty::Bool`. Lowered to `r.is_match(s)`. |
| `s ~= r` with any other operand types | **E1702 "`~=` operands must be String and Regex"** at the operator's source span. |

### MIR lowering

Each literal `RegexLiteral { pattern, flags }` lowers to a `Call(Regex.compile_const, [...])`. Codegen recognises the `compile_const` form and:

1. Synthesises a unique static-pattern symbol (`__regex_lit_<hash>`) at module-init.
2. Emits a per-program init call into the module's `_init` (or equivalent constructor section) that runs `ruxen_regex_new(pattern, flags)` once and stashes the resulting handle into the static slot.
3. Replaces the call site with a load of the static slot.

This is the same shape as `String.from(literal)` const folding lives today; reuse the existing helper if practical.

## `library/std/regex/src/lib.rx` — public API surface

```text
## std::regex — PCRE2-backed regex with first-class /pat/flags literals
## and the `~=` match operator.

lib "pcre2-8"

class RegexError
  ## Description from PCRE2's compile-time error.
  def message as "ruxen_regex_error_message" -> String
  ## Byte offset into the pattern where compilation failed.
  def offset as "ruxen_regex_error_offset" -> Int
end

class Regex
  ## Runtime-built pattern. For compile-time-known patterns use the
  ## /pat/flags literal — those are compiled once at program start.
  def self.new as "ruxen_regex_new"(pattern: &String, flags: &String) -> Result[Regex, RegexError]

  ## Predicate. The `~=` operator desugars to this.
  def is_match as "ruxen_regex_is_match"(text: &String) -> Bool

  ## First match, with capture data, or None.
  ## Named `find` (not `match`) because `match` is a reserved
  ## block-opener keyword in Ruxen. C symbol unchanged.
  def find as "ruxen_regex_match"(text: &String) -> Option[Match]

  ## All matches, left-to-right, non-overlapping.
  def scan as "ruxen_regex_scan"(text: &String) -> Array[Match]

  ## Replace the FIRST match. Replacement supports PCRE2 substitution
  ## syntax (`$1`, `${name}` for back-references).
  def replace as "ruxen_regex_replace"(text: &String, replacement: &String) -> String

  ## Replace ALL matches. Same replacement syntax as `replace`.
  def replace_all as "ruxen_regex_replace_all"(text: &String, replacement: &String) -> String

  ## Split `text` at every match. Empty trailing-match segments
  ## are dropped (matches Ruby / JS behaviour).
  def split as "ruxen_regex_split"(text: &String) -> Array[String]
end

class Match
  ## The entire matched substring.
  def matched as "ruxen_match_matched" -> String

  ## Byte offset (inclusive) into the subject where the match begins.
  ## `start` parses fine (not a keyword), but renamed to start_pos
  ## for symmetric pairing with end_pos.
  def start_pos as "ruxen_match_start" -> Int

  ## Byte offset (exclusive) into the subject where the match ends.
  ## `end` is Ruxen's block-terminator keyword, can't be a method
  ## name. Renamed to end_pos. C symbol unchanged.
  def end_pos as "ruxen_match_end" -> Int

  ## Numbered capture group. `group(0)` returns the whole match for
  ## symmetry with JS `match[0]`. Returns None if `n` exceeds the
  ## group count or the group didn't participate in the match.
  def group as "ruxen_match_group"(n: Int) -> Option[String]

  ## Named capture from `(?<name>...)`. None if no such group or it
  ## didn't participate.
  def named as "ruxen_match_named"(name: &String) -> Option[String]

  ## All numbered groups in declaration order. Index 0 is the whole
  ## match; index 1+ are parenthesised captures.
  def groups as "ruxen_match_groups" -> Array[Option[String]]

  ## All successful named captures.
  def named_groups as "ruxen_match_named_groups" -> HashMap[String, String]
end
```

## C runtime ABI (`library/std/regex/runtime/regex.c`)

### Handle types

```c
/* Wire form of a Regex value: opaque pointer to a PCRE2 compiled pattern.
 * Cast to int64_t at the FFI boundary, same as RuxenHashMap / RuxenVec. */
typedef pcre2_code_8 * RuxenRegex;

/* Wire form of a Match. Owns its copy of the subject string + ovector
 * so the original `String` can be dropped after `regex.match(s)`
 * returns. */
typedef struct RuxenMatch {
    char *subject_copy;          /* malloc'd; freed in ruxen_match_drop */
    int   subject_len;
    PCRE2_SIZE *ovector_copy;    /* malloc'd */
    int   ovector_count;         /* pairs */
    char *named_table_copy;      /* PCRE2 named-group name table; malloc'd */
    int   named_count;
    int   named_entry_size;
} RuxenMatch;
```

### Match handle ownership

Each call to `ruxen_regex_match` / `ruxen_regex_scan` copies the subject + ovector into a fresh `RuxenMatch`. The caller (Ruxen code) owns the `RuxenMatch` handle and drops it via the auto-synth'd `Match.drop` which calls `ruxen_match_drop` (frees the three buffers + struct).

Reason for copying: a session can do `let s = read_input(); let m = /pat/.match(s)` and let `s` go out of scope before reading `m.group(1)`. Without the copy, the ovector would index into freed memory.

### Function inventory

```c
/* Construction */
void *ruxen_regex_new(const char *pattern, const char *flags);
       /* Returns Result[Regex, RegexError] via ruxen_result_*. */

/* Compile-time literal init (called from module-init for each /pat/flags) */
RuxenRegex ruxen_regex_compile_const(const char *pattern, const char *flags);
       /* PANICS via ruxen_panic if PCRE2 rejects — patterns in
        * /pat/flags MUST be valid; typeck already pre-checks. */

/* Queries */
int64_t   ruxen_regex_is_match(RuxenRegex r, const char *text);
void     *ruxen_regex_match(RuxenRegex r, const char *text);   /* Option[Match] */
void     *ruxen_regex_scan(RuxenRegex r, const char *text);    /* Array[Match] */

/* Mutating */
char     *ruxen_regex_replace(RuxenRegex r, const char *text, const char *repl);
char     *ruxen_regex_replace_all(RuxenRegex r, const char *text, const char *repl);
void     *ruxen_regex_split(RuxenRegex r, const char *text);   /* Array[String] */

/* Match accessors */
char     *ruxen_match_matched(RuxenMatch *m);
int64_t   ruxen_match_start(RuxenMatch *m);
int64_t   ruxen_match_end(RuxenMatch *m);
void     *ruxen_match_group(RuxenMatch *m, int64_t n);             /* Option[String] */
void     *ruxen_match_named(RuxenMatch *m, const char *name);      /* Option[String] */
void     *ruxen_match_groups(RuxenMatch *m);                       /* Array[Option[String]] */
void     *ruxen_match_named_groups(RuxenMatch *m);                 /* HashMap[String,String] */

/* Drops */
void      ruxen_regex_drop(RuxenRegex r);
void      ruxen_match_drop(RuxenMatch *m);

/* Error accessors (RuxenRegexError = struct with copies of PCRE2 msg + offset) */
char     *ruxen_regex_error_message(void *err);
int64_t   ruxen_regex_error_offset(void *err);
```

All `ruxen_regex_*` symbols added to the `runtime_signature` table in
`compiler/ruxen_core/src/codegen/cranelift/runtime_sigs.rs` (pointer-or-i64 signatures, matching the wire ABI above).

### Replay-flag handling

Per Task 3 of the REPL state refactor, every non-idempotent runtime function early-returns when `ruxen_repl_is_replaying` is set. `ruxen_regex_*` is **idempotent** (pure compile + match + replace, no side effects) so it ignores the flag and always executes. Documented inline in `regex.c`.

## Error registry

| Code | Diagnostic | Trigger | Long-form doc |
|---|---|---|---|
| E1700 | `unrecognised regex flag '<c>'` | Lexer sees an ASCII letter after the closing `/` that isn't `i m s g x`. | `docs/errors/E1700.md` |
| E1701 | `unterminated regex literal` | Lexer reaches newline/EOF before a closing `/`. | `docs/errors/E1701.md` |
| E1702 | `\`~=\` operands must be String and Regex` | Typecheck sees `~=` with operand types other than `String` and `Regex`. | `docs/errors/E1702.md` |
| E1703 | `empty regex pattern` | Lexer sees `//` (zero-byte pattern). | `docs/errors/E1703.md` |
| E1704 | `invalid regex pattern: <pcre2-message>` | Typecheck runs `pcre2_compile_8` on a literal and PCRE2 rejects. | `docs/errors/E1704.md` |

E170x range chosen to fit the existing namespace allocation:
- E1011–1099 mixin
- E16xx package manager
- **E17xx — regex** (new, allocated by this work)

## Edge cases (the spec must cover)

| Case | Behaviour |
|---|---|
| `//` (zero-byte pattern) | E1703 at lex time. |
| `/[/]/` | The `/` inside the character class is part of the pattern. Lexer bracket-depth counter ignores it. |
| `/\\//` | The `\/` is escape + slash. The `/` it would close is consumed by the escape; the next bare `/` closes the literal. PCRE2 receives the raw `\/` and treats it as a literal `/`. |
| `/pat/iimg` | E1700 on the second `i` ("flag 'i' repeated"). |
| Single-line regex spanning bracket depth | The bracket-depth tracking only applies to `[…]`. `()` and `{…}` inside the pattern don't affect terminator detection — they're regex syntax handled by PCRE2. |
| `result/2` (intended as division) | The previous token is `Identifier` which is NOT in the expression-context set → `/` lexes as `Slash`. Division. |
| `let r = /foo/` | Previous token is `Eq` → expression context → regex literal. |
| `f(/foo/)` | Previous token is `LParen` → expression context → regex literal. |
| `arr[/foo/]` | Previous token is `LBracket` → expression context → regex literal. |
| `/foo/g.scan(s)` | Regex literal followed by method call. Once the regex literal is one token, the `.` chain parses normally. |
| `s ~= /foo/g` | `TildeEq` is in the expression-context set → `/foo/g` lexes as regex literal. |
| `match.match(/pat/)` (method named `match`) | Identifier `match` is followed by `.match` (field/method access). The `(` after the inner `match` puts the lexer in expression context. Regex literal parses. |

## Testing strategy

### Lexer unit tests (`compiler/ruxen_core/src/lexer/tests.rs`)

One test per expression-context token kind: `<prev_token> /pat/i` → `RegexLiteral`. One negative test per non-expression-context kind: `<prev_token> /pat/i` → `Slash` + parse error. Plus the edge-case table above (`/[/]/`, `/\\//`, etc.).

### Parser unit tests

- `~=` precedence vs `==`, `&&`, `||`.
- AST shape for `let r = /foo/g`.
- AST shape for `s ~= /foo/`.

### Typeck unit tests

- E1702 emitted for `5 ~= /foo/`, `s ~= 3`, `s ~= "string"`.
- E1704 emitted for `let r = /pat[/` (unterminated character class — PCRE2 compile error).
- `s ~= r` infers `Bool`.

### Stdlib unit tests (`library/std/regex/tests/*.rx`)

| File | Coverage |
|---|---|
| `tests/basic_match.rx` | `is_match`, `match`, `~=` for simple patterns. |
| `tests/groups.rx` | numbered + named captures, `group`/`named`/`groups`/`named_groups`. |
| `tests/scan.rx` | `scan` over text with 0, 1, many matches. |
| `tests/replace.rx` | `replace`, `replace_all`, `$1` / `${name}` substitutions. |
| `tests/split.rx` | `split` with leading/trailing/empty segments. |
| `tests/flags.rx` | `i m s x` flag effects (`g` is a no-op — pinned). |

### e2e fixtures (`tests/release-e2e/cases/9xx_regex_*.rx`)

| Fixture | Surface tested |
|---|---|
| `900_regex_literal_match.rx` | `/error/.is_match("error: foo")` → `true` |
| `901_regex_tilde_eq.rx` | `if "x42y" ~= /\d+/; puts "yes"; else; puts "no"; end` |
| `902_regex_groups.rx` | named + numbered capture round trip |
| `903_regex_scan.rx` | `scan` returning Array of Match |
| `904_regex_replace_all.rx` | `replace_all` with `$1` back-reference |
| `905_regex_runtime_compile.rx` | `Regex.new(pattern, flags)` Ok/Err paths |
| `906_regex_invalid_literal.rx` | compile-time E1704 (negative test fixture) |

## Out of scope (v1)

- `Regex.new` accepting `Array[u8]` for non-UTF-8 patterns.
- Compile-time-derived `Match` literals.
- Iterator-based `scan` (returns `Array` for simplicity; `Iterator[Match]` deferred until iterators are a finished surface across the stdlib).
- `?`-suffix predicate method names (would require lexer ambiguity work outside this spec's scope; using `is_match` instead per existing convention).
- `=` setter methods (would require parser change). Not needed for regex.
- A `RegexBuilder` / fluent-config API. Just `Regex.new(pattern, flags)`.
- PCRE2 JIT compilation hooks. Use the default interpreter; revisit if benchmarks show it.
- Unicode property escapes (`\p{Letter}` etc.) — PCRE2 supports them by default; no Ruxen-side work needed but tested only at smoke-test depth.
