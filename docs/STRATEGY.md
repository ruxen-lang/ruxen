# Riven — Strategic Positioning (2026-05-17)

This document captures a strategic assessment of Riven written after the
v1 #05/#06 closure session. It is **not** a roadmap; it is a *frame* for
making roadmap decisions. The roadmap itself lives at
`docs/prompts/README.md` and is being reprioritized in light of this
document.

> Read this when deciding what to build next. If the choice doesn't
> ladder up to one of the three wedges below, you are probably building
> "complete language" instead of "complete answer to one question."

---

## 1. Founder's actual goal (clarified 2026-05-17)

Not "displace Rust." Not "compete with tech giants."

> Build fast applications (faster than Ruby) that don't leak memory,
> with Rust's borrow checker discipline — from a Ruby developer's
> ergonomic perspective. The pain motivating this: long-running Ruby
> servers ballooning to 300–700 MB after a few hours of operation.

That goal is achievable. The strategic question is **for whom**, **in
what shape**, and **what to build first** to make it land.

---

## 2. The unique slot Riven occupies

The honest competitive matrix:

|              | Ruby syntax | No GC | Borrow checker | AOT compile |
|--------------|:-----------:|:-----:|:--------------:|:-----------:|
| Ruby (MRI/YJIT) | ✅ | ❌ | ❌ | ❌ |
| Crystal      | ✅ | partial (Boehm GC) | ❌ | ✅ |
| Rust         | ❌ | ✅ | ✅ | ✅ |
| Nim          | ❌ | optional | ❌ | ✅ |
| Zig          | ❌ | ✅ | ❌ | ✅ |
| **Riven**    | ✅ | ✅ | ✅ | ✅ |

That row is genuinely unique. But unique ≠ wanted. The skeptical
question: *does anyone actually want all four at once?*

- Most Rubyists ran *toward* Ruby because they didn't want to think
  about lifetimes. They will not accept a borrow checker as the price
  of Ruby-feel.
- Most Rustaceans like Rust's explicitness. They will not trade
  clarity for `do/end`.

The user base is a thin slice: **senior Rubyists who've done some Rust,
hated the syntax, but loved the guarantees.** That slice is real.
It is also small (think Crystal-sized, not Go-sized).

If Riven competes head-on with Ruby or Crystal on those terms, it
loses or stays small. The growth path is **not** "be a better Ruby" —
it is "occupy a wedge nobody else owns."

---

## 3. Three real wedges (ranked by ROI)

### Wedge #1 — "Faster C extensions for Rubyists" (highest ROI)

The Ruby ecosystem ships thousands of gems with C extensions:
nokogiri, oj, mysql2, ffi, image processors, parsers, crypto. Writing
and maintaining C extensions is miserable. The current alternatives:

- C (awful DX)
- Rust + magnus/rb-sys (better DX, but you're learning Rust)
- Crystal (no clean Ruby FFI story)

**Reposition Riven as: "Write a `.so` Ruby can `require`, with
Ruby-like syntax and Rust-like safety."** Rubyists capture the win
*without* writing a single Riven app — they ship faster gems.

Why this is the best opportunity:
- **Distribution is free** — RubyGems already exists.
- **Adoption is sideways** — no all-or-nothing language switch.
- **The pain is acute and ongoing** — every C-extension maintainer is
  a candidate user.
- **Sympathetic audience** — Rubyists already know you.
- **Viral mechanic** — one fast gem ("nokogiri-rv is 10× faster") and
  you're on Hacker News.

**What Riven needs to add to win here:**
- First-class `extern "Ruby"` ABI calling convention.
- Ergonomic Ruby-object marshaling (VALUE in/out, GC-safe handles).
- A `cargo`-style template `riven new --gem` that scaffolds a
  ruby-callable extension with `Rakefile` + native-extension build
  glue out of the box.
- A canonical example gem in the docs (e.g. `riven-fast-json`).

### Wedge #2 — "Ruby for WebAssembly" (second highest)

WASM is exploding — Cloudflare Workers, Fastly Compute, browser
plugins, Figma plugins, game scripting. The current "Ruby for WASM"
story is:

- **mruby** — interpreted, slow.
- **ruby.wasm** — huge binary, slow cold-start.

**Nobody has shipped a compiled, no-GC, fast-cold-start Ruby-shaped
language for WASM.** This is greenfield.

Why this works:
- **No incumbent.** First credible player wins by default.
- **Cloudflare / Fastly developers love new options.** Their docs
  actively showcase non-mainstream languages.
- **Game scripting** (Lua dominates only because nothing else is small
  + fast enough) is suddenly contestable.

**What Riven needs to add to win here:**
- WASM target as a first-class codegen backend (not "technically
  supported").
- Aggressive dead-code elimination to keep binary size small.
- Demonstrate < 100 KB hello-world WASM binary.
- "Deploy to Cloudflare Workers" / "Deploy to Fastly Compute" docs +
  templates.
- WASI compatibility for the stdlib's fs/process layers.

#### Phase 2 status — sync I/O ≈ 90% (post-#06.5)

`fs.*` + `File` + `OpenOptions` + `SeekFrom`, `Command` + `Output` +
`ExitStatus`, `TcpListener` + `TcpStream` (incl. binary-safe read and
socket timeouts), `BufReader[R]` + `BufWriter[W]` over the closed
`{File, TcpStream}` inner set, `Instant` + `Duration` + `sleep`, and
`std.rand` (kernel CSPRNG) are all shipped. The 10% gap is mostly
`BufReader.lines()` (deferred to v1.5 with the iterator trait), a
formal `Read`/`Write` mixin (also v1.5), `Stdin/Stdout/Stderr` BufReader
flavour, and the `IntoInnerError` recovery type Rust ships on
`BufWriter`.

### Wedge #3 — "A flagship app" (slowest but most defensible)

Rails made Ruby. Phoenix made Elixir. TensorFlow made Python ML.
**Languages don't sell languages — flagship apps do.**

Pick one app shape and build the canonical version in Riven:

- A Sinatra-style web framework that proves the perf story.
- A CLI tool framework that beats Cobra/Click on ergonomics + speed.
- A static site generator that beats Hugo (compiled = fast).
- A test runner that beats RSpec on speed.

Then *every blog post about Riven* is actually about that flagship.
People come for the tool, stay for the language.

This is slower than #1 or #2 because the flagship itself needs
shipping before the marketing engine starts. But once shipped, it
compounds for years.

---

## 4. The single biggest mindset shift

> **Stop building "a complete language." Start building "a complete
> answer to one specific question."**

The current 25-prompt v1 roadmap reads like "everything Rust has, in
Ruby clothes." That's a 5-year project competing with everyone, with
no clear "why use this over Crystal" answer.

Instead: pick **one** wedge above, ship a v0.5 that nails it, and let
the rest of the language grow from real user pull.

If forced to bet, **bet on Wedge #1 (Ruby C-extension replacement).**
The combination of free distribution, sideways adoption, acute pain,
and sympathetic audience makes it the highest-ROI play.

---

## 5. Language-level changes to add (high ROI, missing today)

These are not v1 nice-to-haves; these are what make growth possible.

### Optional GC mode (opt-in borrow checking)

Make borrow checking opt-in. Quick scripts use GC; production code
opts into ownership. Roc does this. It cuts the on-ramp by ~80% —
Rubyists try the language without facing lifetimes on day one, then
graduate.

Without this, every Rubyist hits the borrow checker on their first
"hello world" and bounces. With this, they write Ruby-like code,
notice it's fast, and only later learn the ownership story when they
need the guarantees.

### First-class Ruby FFI

`extern "Ruby" fn foo(x: VALUE) -> VALUE` style. Marshal Ruby objects
in and out cleanly. This is the unlock for Wedge #1.

### WASM as a first-class target

Not "technically supported" — documented, demoed, templated. Move
this *up* the roadmap from where it currently sits (prompt #16, deep
in Phase 4) to the front of Phase 2 if Wedge #2 is the chosen play.

### One-command tooling

`riven new`, `riven build`, `riven run`, `riven test`, `riven publish`,
`riven add foo`. **Cargo is the bar.** Match it *before* v1, not after.
A great language with bad tooling loses to a mediocre language with
great tooling. Every time.

### Error messages that teach

Elm-tier. Every error has a doc link, an example, a fix. This is a
multiplier on every other feature. The reason new Rust users say
"Rust's errors are amazing" is *single-handedly* responsible for a
lot of Rust adoption.

---

## 6. What to de-prioritize (or cut from v1)

Looking at the v1 roadmap with fresh eyes:

### #07 — Const generics (deprioritize, finish in-flight only)

Currently partway through (S1–S9 landed). **Finish what's in flight,
ship it as a CHANGELOG bullet, then stop.** Recognize this is a nerd
feature: Ruby has no equivalent, Rubyists do not ask for it,
Crystal shipped 1.0 without anything like it.

The remaining const-generic polish (closing the DoD checkboxes after
S9) is *fine* to do, but no new investment in const-generic surface
beyond what's already typed-checked / monomorphized.

### #08 — HRTBs + `some` mixin (deprioritize → v1.5 or v2)

Higher-ranked trait bounds are a power-user feature. **Crystal
shipped 1.0 without them and is fine.** Rust users hit HRTBs maybe
once a year. They're not blocking any flagship app, any Ruby FFI
use case, or any WASM use case.

Move to v1.5 or v2 entirely. Free up the engineering time.

### #09 — GATs + `any` mixin (deprioritize → v1.5 or v2)

Same logic as #08. Generic associated types are powerful but esoteric.
Not blocking any of the three wedges.

Move to v1.5 or v2.

### #15 — Async (deprioritize, evaluate after Wedge is chosen)

Hard to get right, easy to over-engineer. Crystal got by with fibers
for years. **Ship great sync I/O first; let users prove they need
async before paying the design cost.**

If Wedge #2 (WASM) is the chosen play, async may be load-bearing for
the Cloudflare Workers story. Re-evaluate then. If Wedge #1
(C-extension) is the chosen play, async is irrelevant.

---

## 7. What to NOT compromise on

- **The borrow checker as default for production code.** That's the
  unique slot. If you cave to "make it like Crystal," you lose the
  differentiator and Crystal eats you.
- **AOT compilation.** Static binaries are the deployment story.
  Interpretation tier (REPL, scripting mode) is fine as an option;
  default is compile-to-binary.
- **Ruby-syntax fidelity.** Don't drift toward Rust syntax under
  engineering pressure. The whole point is "this feels like Ruby."
  Every concession ("Rubyists will accept `let mut` instead of
  `var`") is a death-by-paper-cuts loss of the differentiator.

---

## 8. The brutally honest risk

Riven's biggest risk is **not technical** — the compiler is real, the
roadmap is coherent, the patterns established (the "flat heap struct
+ accessors" mirror pattern from fs.metadata, the runtime-fn
registration triple, the no-shortcut TDD discipline) are sound.

The biggest risk is **strategic**: spending three years building a
complete language and then having **no answer** when someone asks
"why would I use this over Crystal?"

Pick your "why" *now*, before v1 lands, and let it shape what v1
actually contains.

---

## 9. Concrete next decisions

1. **Choose a wedge.** #1 (Ruby FFI), #2 (WASM), or #3 (flagship app).
   Don't pick two. Pick one.
2. **Reorder the v1 roadmap** to put the wedge's enabling work in
   Phase 2/3 instead of Phase 4/5. (For #1: Ruby FFI surface. For #2:
   WASM target.)
3. **Cut or push** #08, #09, and possibly #15 to v1.5.
4. **Audit `cargo-style` tooling** against the "one-command
   everything" bar. Whatever's missing becomes a Phase-3 priority.
5. **Identify one canonical demo project** for the chosen wedge and
   start building it in parallel with v1 — the demo proves the
   language ships before "the language is shipped."

---

## 10. Open questions for the founder

- ~~Which wedge resonates? #1 (Ruby FFI), #2 (WASM), #3 (flagship)?~~
  **Resolved 2026-05-22 — see §11.**
- Is there a fourth wedge specific to your network / industry
  exposure that I'm missing?
- What's the realistic time budget per week — solo, or with the
  approved 3 hires onboarded?
- Is there an opinionated "what I want my Ruby app to look like in
  10 years" sketch somewhere? If yes, that's the flagship app.

---

## 11. Wedge sequencing — locked 2026-05-22

After the 2026-05-22 strategy session (held mid-CLI-consolidation),
the three wedges are sequenced rather than mutually-exclusive:

| Phase | Wedge | Start | Notes |
|---|---|---|---|
| **A** | #1 — Ruby FFI | First | Highest probability of success per §3. Smallest commitment to validate (~2-3 months to MVP). Distribution is free via RubyGems. Failure mode is recoverable. |
| **B** | #2 — WASM | In parallel with A once A's basic ABI surface is in tree | The codegen / MIR work for Wedge #1 retargets cleanly to wasm32 — backend swap + WASI stdlib retarget, not a from-scratch rewrite. Heavier (4-6 months MVP) but the parallel start banks the leverage. |
| **C** | #3 — Flagship app | Same start as Wedge #2 | Picked from one of the candidate shapes in §3 (CLI tool framework / static site generator / test runner). Slowest payback but compounds for years if it ships. Doesn't gate the language — language work proceeds; flagship is a parallel investment. |

### Framing correction (Wedge #1)

The "nokogiri-rv is 10× faster than C" pitch in §3 is unwinnable
and is dropped. Riven won't beat hand-tuned C extensions (oj,
nokogiri, mysql2). Realistic Riven perf vs hand-tuned C is **1.5-3×
slower** for the same workload.

The actual Wedge #1 bet:

> **"Riven extensions are *almost-as-fast-as-C* but written in 10×
> less developer time, with memory safety guarantees, by people who
> already know Ruby."**

The 10× perf claim is **only vs pure-Ruby implementations** — and
there are thousands of pure-Ruby gems in the long tail where this
claim is trivially true. The dev-productivity claim (10× faster to
write than C, vs Rust+magnus which needs 3-6 months for a Rubyist
to become productive in) is the actual moat.

Targets:

- Pure-Ruby gems with known perf complaints (templating, parsing,
  format conversion, crypto helpers, image-shellout replacements)
- Abandoned C extensions where the maintainer wants a memory-safe
  rewrite
- New-extension space (the next ten years of gems)

NOT targets:

- oj, nokogiri, mysql2 (you'll lose; don't ship the comparison)
- Numeric tight loops competing against NumPy-shaped libraries
  (numerical-perf is a separate fight, deferred indefinitely)

### Canonical demo gem — TBD

Not yet picked. Candidates by ease-of-win:

1. **CSV / TSV parser** — Ruby stdlib CSV is pure Ruby; SmarterCSV
   is pure Ruby. Easy 30-50× win.
2. **Mustache / Liquid template engine** — pure-Ruby implementations
   with active perf complaints on large templates.
3. **Markdown parser** targeting kramdown's audience (NOT redcarpet's).
4. **Small primitives**: fast base32/64, fast URL encoding, fast HTML
   entity escaping. Each ships in a week.

Pick one as the canonical demo when Phase A formally starts.

### Phase A entry criteria (what triggers Wedge #1 start)

v1.0 of the language shipped (per `docs/prompts/v1/STATUS.md` — most
items already closed, remaining tail is mostly Phase 4/5 platform +
tooling that doesn't gate FFI work).

### Phase B entry criteria

Wedge #1's basic FFI ABI working (one example gem builds + loads
into MRI Ruby + a simple round-trip works). The codegen layer at
this point is wedge-agnostic; retargeting to wasm32 is a backend
swap.

### Phase C entry criteria

Same as Phase B. Flagship app development is mostly Riven-the-language
work (writing the framework code), so the only requirement is the
language being stable enough for serious app development — which it
is today.

### What this changes about v1.0

Not much. v1.0 ships as planned (release checklist per prompt 25).
The wedges are the v1.x / v2.0 trajectory, not gating v1.0.

What DOES change:

- Prompt 16 (no_std / wasm) is now Phase-B work — promote out of "Not
  started" once Phase A lands.
- A new prompt-set for "Ruby FFI" needs writing (currently no
  v1 prompt covers this — `extern "ruby"` is greenfield).
- Prompt 19 (test framework) decisions are already locked from the
  2026-05-22 CLI session; implementation stays on the v1.x trajectory.

---

*Authored as part of the 2026-05-17 v1 closure session, after
shipping `fs.metadata` and the `Iterator` chain/zip/collect
clarifications. The Command builder is in flight at the time of
writing; once it lands, Phase 2 (#05 + #06) is materially complete.*
