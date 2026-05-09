# Tier 4.07 — `examples/` Directory

## 1. Summary & Motivation

The tutorial (`docs/tutorial/01..16`) teaches Riven feature-by-feature through small snippets; the test fixtures (`crates/riven-core/tests/fixtures/*.rvn`, 13 files) exercise compiler paths through small programs. **Neither is a complete, runnable project that a newcomer can `git clone && riven run`.** There is no `examples/` directory anywhere in the repo.

"Show me a real program written in this language" is the single most common request a new user has. It drives adoption, validates the language's stdlib, and — critically — keeps the language honest: every example that stops building under a compiler change generates CI noise that forces the change to either fix the example or document the break. Examples are the acceptance test for tier 1 / 2 / 3 work.

This document specifies a curated set of in-tree examples under `examples/`, what each is, what it demonstrates, and how CI keeps them building.

## 2. Current State

### 2.1 Directory listing

```
$ ls /home/sheraz/Documents/riven/
Cargo.lock  Cargo.toml  CLAUDE.md  crates  docs  editors  install.sh  README.md  target  uninstall.sh
```

No `examples/`.

### 2.2 What exists that's example-adjacent

- `crates/riven-core/tests/fixtures/` (13 `.rvn` files, most < 50 lines):
  - `arithmetic.rvn`, `classes.rvn`, `class_methods.rvn`, `control_flow.rvn`, `enum_data.rvn`, `enums.rvn`, `functions.rvn`, `hello.rvn`, `mini_sample.rvn`, `sample_program.rvn`, `simple_class.rvn`, `string_interp.rvn`, `tasklist.rvn`.
  - `tasklist.rvn` and `sample_program.rvn` are the largest — still < 100 lines.
  - These are compiler fixtures (they live in `tests/`). Not user-facing.

- Tutorial code snippets in `docs/tutorial/*.md`. Copy-paste-quality, not complete projects.

### 2.3 Consequences

- **Nowhere to point a newcomer** beyond "read the tutorial."
- **No integration tests across the whole stack.** A stdlib change that breaks `read_line + parse + format` wouldn't be caught by fixture tests.
- **No forcing function for stdlib completeness.** Tier-1 stdlib doc mentions tutorial files use methods that don't exist. An `examples/` entry would fail loudly.
- **The package manager is untested against real projects.** `riven-cli/tests/installed_pkg_manager.rs` uses synthetic `my-dep` fixtures; no real cross-package integration.

## 3. Goals & Non-Goals

### Goals

1. An `examples/` directory at the repo root containing 5-7 curated, runnable projects.
2. Each example is a complete `Riven.toml`-rooted project that `riven build && riven run` builds and runs.
3. Examples cover **breadth**: sync CLI utility, blocking network server, threaded server (concurrency), async server (tier 1.03), WASM target (tier 4.03), a small game (dependency on an external piece, exercises tier 4.01 registry).
4. Every example has a `README.md` explaining: what it does, how to run it, which language features it demonstrates.
5. CI matrix (doc 06 phase 6d) compiles every example. A broken example fails the build.
6. No example depends on network access at runtime (tests/smoke tests must be hermetic).
7. Examples compile with the current shipped stdlib. They land *after* the stdlib features they require, not before.

### Non-Goals

- A comprehensive standard-library cookbook (that's `docs/tutorial/`).
- Examples for every language feature (macros for every trait, every pattern). Less is more.
- Benchmarking examples. Performance is a separate concern.
- GUI examples. No GUI toolkit exists in Riven's stdlib.
- A separate `riven-examples` repository (discoverability loss; doc 00 §"Critical questions" answers: in-tree).
- "Awesome Riven" catalogs. Third-party, community-run.
- Examples written in obsolete Riven syntax (pre-v1). If a breaking language change lands, every example gets updated in the same PR.

## 4. Surface

### 4.1 Directory layout

```
examples/
├── README.md                          # index of examples; when to use each
├── 01-cli-utility/
│   ├── README.md
│   ├── Riven.toml
│   └── src/
│       └── main.rvn
├── 02-tcp-echo-server/
│   ├── README.md
│   ├── Riven.toml
│   └── src/main.rvn
├── 03-threaded-http-server/
│   ├── README.md
│   ├── Riven.toml
│   └── src/main.rvn
├── 04-wasm-hello/
│   ├── README.md
│   ├── Riven.toml
│   ├── index.html                      # JS harness
│   └── src/main.rvn
├── 05-snake-game/                      # (exercises tier 4.01 registry when ready)
│   ├── README.md
│   ├── Riven.toml
│   └── src/main.rvn
├── 06-embedded-qemu/                   # optional — tier 4.04 no_std showcase
│   ├── README.md
│   ├── Riven.toml
│   ├── memory.ld
│   └── src/main.rvn
└── 07-async-echo/                      # (tier 1.03 async)
    ├── README.md
    ├── Riven.toml
    └── src/main.rvn
```

Numbering is a learning path, roughly from easiest to most ambitious. Not all examples need to exist from day one — see §8 phasing.

### 4.2 Recommended set (minimum viable)

The doc 00 overview §"Recommended implementation order" lists three as the minimum:

- **CLI utility** — easiest. Ships on day one; no networking, no concurrency, no stdlib growth required beyond what tutorial 01-11 already expects.
- **Threaded server** — depends on tier 1.02 concurrency. Ships after concurrency lands.
- **WASM toy** — depends on tier 4.03 WASM. Ships with doc 03.

This doc adds:

- **Single-threaded TCP echo** — ships earlier than the threaded server (tier 1.01 `std::net` only).
- **Game (Snake)** — demonstrates the package manager (depends on a hypothetical `termio` piece) + pattern matching + mutable state.

### 4.3 Example detail — `01-cli-utility` (word-count clone)

`Riven.toml`:

```toml
[package]
name = "wc"
version = "0.1.0"
edition = "2026"
description = "A Riven port of the classic `wc` utility."

[build]
type = "binary"
```

`src/main.rvn`:

```riven
use std::env
use std::fs
use std::io::IoError

def main
  let args = env::args()
  if args.len < 2
    eputs "usage: wc <file>"
    std::process::exit(1)
  end

  let path = args.get(1).unwrap!
  match fs::read_to_string(&path)
    Ok(content)  -> count(&content)
    Err(IoError::NotFound(p)) -> eputs "no such file: #{p}"
    Err(err)     -> eputs "error: #{err}"
  end
end

def count(text: &str)
  let mut lines = 0
  let mut words = 0
  let mut bytes = text.len

  for line in text.lines()
    lines += 1
    words += line.split_whitespace().count()
  end

  puts "#{lines} #{words} #{bytes}"
end
```

What it exercises:

- `std::env::args`
- `std::fs::read_to_string`
- `std::io::IoError` pattern matching
- Mutable locals
- `for` over an iterator
- String method chaining (`split_whitespace().count()`)
- `std::process::exit`

Prerequisites:

- Tier 1 phase 1b (I/O, fs, env, process).

### 4.4 Example detail — `02-tcp-echo-server`

`src/main.rvn`:

```riven
use std::net::TcpListener
use std::io

def main
  let listener = TcpListener::bind("127.0.0.1:7878").unwrap!
  puts "listening on 127.0.0.1:7878"

  for conn in listener.incoming()
    match conn
      Ok(mut stream) -> handle(&mut stream)
      Err(e)     -> eputs "connection error: #{e}"
    end
  end
end

def handle(stream: &mut TcpStream)
  let mut buf = [UInt8; 1024]
  loop
    match stream.read(&mut buf)
      Ok(0)  -> break                            # client closed
      Ok(n)  -> stream.write(&buf[0..n]).unwrap!
      Err(_) -> break
    end
  end
end
```

What it exercises:

- `std::net::TcpListener`, `TcpStream`.
- `Result` pattern matching with error arms.
- Mutable slices / fixed-size arrays.
- `.unwrap!` with the danger-suffix convention.

Prerequisites:

- Tier 1 phase 1c (net).

### 4.5 Example detail — `03-threaded-http-server`

Spawn-per-connection with an `Arc<Mutex<HitCounter>>` for request counting. Exercises:

- `std::thread::spawn`
- `std::sync::{Arc, Mutex}`
- `Send`/`Sync` auto-trait inference
- Simple HTTP/1.1 parsing (hand-written)

Prerequisites: tier 1 phase 2.

### 4.6 Example detail — `04-wasm-hello`

Riven side (`src/main.rvn`):

```riven
@[wasm_export("greet")]
pub def greet(count: Int32) -> Int32
  let mut total = 0
  for i in 0..count
    total += i
  end
  total
end
```

`index.html` (JS harness):

```html
<!doctype html>
<html>
  <body>
    <button id="go">Sum to 100</button>
    <pre id="out"></pre>
    <script>
      (async () => {
        const resp = await fetch('target/wasm32-unknown-unknown/release/wasm_hello.wasm');
        const buf = await resp.arrayBuffer();
        const { instance } = await WebAssembly.instantiate(buf, {
          env: { console_log: (p, n) => {} },
        });
        document.getElementById('go').onclick = () => {
          document.getElementById('out').textContent = instance.exports.greet(100);
        };
      })();
    </script>
  </body>
</html>
```

Prerequisites: tier 4.03 phase 3b.

### 4.7 Example detail — `05-snake-game`

Classical terminal Snake. Grid, pattern-matching on input (`W`/`A`/`S`/`D`/`Q`), game state as a pair of `Vec<Point>` (snake body) + `Point` (food).

Depends on a hypothetical `termio` piece (cursor positioning, nonblocking key read):

```toml
[dependencies]
termio = { git = "https://github.com/riven-lang/termio.git", tag = "v0.1.0" }
```

This is the single example that exercises the package manager's external-dep path. Once the tier 4.01 registry lands, change to `termio = "0.1.0"`.

What it exercises:

- Pattern matching on enums (`Direction::Up | Down | Left | Right`)
- Mutable state across a `loop`
- Class definitions with methods
- A cross-package dependency

Prerequisites: tier 1 phases 1a-1c for stdlib collections; doc 01 phase 1a workspace/git-dep resolution (already works).

### 4.8 Example detail — `06-embedded-qemu` (optional)

Bare-metal cortex-m3 "blink" analog — writes a pattern to a QEMU UART. `@[no_std]`, `@[panic_handler]`, `@[global_allocator]` (or a no-alloc design).

Prerequisites: tier 4.04.

### 4.9 Example detail — `07-async-echo`

Single-threaded async TCP echo server using `async def` + `.await` + `block_on`. One reactor thread, many connections.

Prerequisites: tier 1.03 phases 3a-3c.

### 4.10 `examples/README.md`

An index:

```md
# Riven Examples

Curated, complete Riven projects. Each has its own `README.md` and `Riven.toml`; run with `riven run` from inside the example's directory.

## The examples

| # | Example | What it teaches | Prerequisites |
|---|---------|-----------------|---------------|
| 01 | [cli-utility](01-cli-utility/) | stdin, env, fs, error handling | stdlib 1b |
| 02 | [tcp-echo-server](02-tcp-echo-server/) | std::net basics | stdlib 1c |
| 03 | [threaded-http-server](03-threaded-http-server/) | threads + Arc<Mutex<T>> | concurrency |
| 04 | [wasm-hello](04-wasm-hello/) | wasm32 target | tier-4 WASM |
| 05 | [snake-game](05-snake-game/) | Git dependencies, terminal I/O | package-mgr |
| 06 | [embedded-qemu](06-embedded-qemu/) | no_std, panic_handler | tier-4 no_std |
| 07 | [async-echo](07-async-echo/) | async/await, single-threaded | async |

## Running

```bash
cd examples/01-cli-utility
riven run -- README.md
```

Each example's README.md has more detail.
```

## 5. Architecture / Design

### 5.1 How examples are not test fixtures

Fixtures live in `crates/riven-core/tests/fixtures/`. They are small, single-file, designed to exercise a specific compiler path, and compiled through an in-process `Lexer`/`Parser`/`typeck`/`mir` pipeline.

Examples live in `examples/`. They are multi-file projects built by the user-facing `riven` CLI. They exercise the full install path (find stdlib, find runtime, link against it).

Both are valuable. They do not replace each other.

### 5.2 CI strategy

A new `examples` job in `ci.yml` (doc 06 §4.2):

```yaml
  examples:
    name: Build example (${{ matrix.example }})
    runs-on: ubuntu-latest
    strategy:
      fail-fast: false
      matrix:
        example:
          - 01-cli-utility
          - 02-tcp-echo-server
          # - 03-threaded-http-server        # gated on concurrency
          # - 04-wasm-hello                  # gated on wasm target
          # - 05-snake-game                  # gated on git registry
          # - 06-embedded-qemu               # gated on no_std
          # - 07-async-echo                  # gated on async
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Build compiler
        run: cargo build -p rivenc -p riven-cli
      - name: Build example
        working-directory: examples/${{ matrix.example }}
        run: ../../target/debug/riven build
      - name: Smoke-test example
        working-directory: examples/${{ matrix.example }}
        run: |
          if [ -x smoke.sh ]; then ./smoke.sh; fi
```

Each example can have an optional `smoke.sh` that runs the built binary with a canned input and asserts expected output. For interactive examples (snake, echo), `smoke.sh` does a 2-second `timeout` + `pkill` cycle and checks the binary started successfully.

### 5.3 Enabling examples incrementally

Each example has a "prerequisites" line in its README pointing at the tier-1 phase that unblocks it. Until that phase ships, the example is commented out of the CI matrix (see §5.2). The matrix entry is uncommented the same week the underlying feature ships — a discipline that keeps the examples honest.

### 5.4 Licensing on examples

Each example's source files carry a permissive license comment at the top (matching repo's dual license: `SPDX-License-Identifier: MIT OR Apache-2.0`). Users who copy an example into their own code should have clear reuse terms.

### 5.5 Example-to-example isolation

Each example has its own `target/` directory (inside the example's folder, gitignored by the example's `.gitignore`). Examples share the compiler with the rest of the repo via `../../target/debug/riven` in CI. Users who `cd examples/01 && riven build` from a cloned repo use the system-installed `riven` — and that's fine.

## 6. Implementation Plan — files to touch

### New files

- `examples/README.md`.
- `examples/<n>-<name>/README.md`, `Riven.toml`, `src/main.rvn`, and optionally `smoke.sh`, per §4.x.
- `.github/workflows/ci.yml` — the `examples` matrix job (part of doc 06).

### Touched files

- `README.md` — add an "Examples" section pointing at `examples/`.
- `.gitignore` (if one exists at root) — ensure `examples/*/target/` is ignored.
- `crates/riven-cli/src/scaffold.rs:154-159` — nothing to change; examples aren't scaffolded, they're authored.

### Tests

- CI's `examples` matrix (see §5.2) is the test. Each example's `smoke.sh` is an integration test.
- Optional: `crates/riven-cli/tests/examples_build.rs` — runs `riven build` on every `examples/*/` in a temp dir clone. Same as the CI matrix but locally runnable. Recommend: skip, CI is enough.

## 7. Interactions with Other Tiers

- **Tier 1 stdlib (01).** Examples 01, 02, 03 exercise stdlib I/O, fs, env, net. `wc` (example 01) is the acceptance test for phase 1b's `fs::read_to_string`.
- **Tier 1 concurrency (02).** Example 03 is the first place `Thread::spawn` + `Mutex<T>` + `Arc<T>` compose in a user program.
- **Tier 1 async (03).** Example 07 tests the async/await pipeline end-to-end.
- **Tier 4.01 package manager.** Example 05 tests git deps with a real (eventually registry-hosted) piece.
- **Tier 4.02 cross-compilation.** No example targets a non-host triple directly; the `cross` CI job (doc 06) cross-compiles the compiler itself. Could optionally add an `examples/cross/` that demos building 01-cli-utility for aarch64-linux-gnu.
- **Tier 4.03 WASM.** Example 04 is the reference.
- **Tier 4.04 no_std.** Example 06 is the reference.
- **Tier 4.06 CI.** Examples are guarded by CI; a broken example fails master.
- **Tier 4.08 repo hygiene.** Examples carry per-file license headers. The repo's top-level LICENSE applies.

## 8. Phasing

### Phase 7a — `01-cli-utility` (0.5 week, after tier-1 phase 1b)

1. `examples/01-cli-utility/` as per §4.3.
2. `smoke.sh` runs `riven run -- README.md` and checks the output matches `<lines> <words> <bytes>` pattern.
3. `examples/README.md` index.
4. CI entry for this example.
5. **Exit:** `cd examples/01-cli-utility && riven run -- src/main.rvn` prints a plausible word count.

### Phase 7b — `02-tcp-echo-server` (0.5 week, after tier-1 phase 1c)

1. `examples/02-tcp-echo-server/` as per §4.4.
2. `smoke.sh` starts the server in the background, `nc`-pipes "hello", asserts the response is "hello", `pkill`s the server.
3. CI entry.
4. **Exit:** `smoke.sh` succeeds in CI.

### Phase 7c — `03-threaded-http-server` (1 week, after tier-1 phase 2d)

1. `examples/03-threaded-http-server/` — Arc<Mutex<Counter>>, spawn-per-conn.
2. `smoke.sh` drives 10 concurrent `curl`s, asserts the counter reaches 10.
3. CI entry.
4. **Exit:** `smoke.sh` succeeds.

### Phase 7d — `04-wasm-hello` (0.5 week, after doc 03 phase 3b)

1. `examples/04-wasm-hello/` with `src/main.rvn` + `index.html` + `build.sh`.
2. `smoke.sh`: `riven build --target wasm32-unknown-unknown --release`, loads the `.wasm` in `wasmi` (small pure-Rust interpreter), calls `greet(100)`, asserts result is `4950`.
3. CI entry.
4. **Exit:** `smoke.sh` succeeds.

### Phase 7e — `05-snake-game` (1 week)

1. Write a `termio` piece externally (https://github.com/riven-lang/termio) — small cursor + raw-input wrapper over `tcsetattr` + ANSI escapes. Probably ~150 lines of Riven.
2. `examples/05-snake-game/` depending on it.
3. `smoke.sh`: start the game, `printf 'wwwwq' | ./snake` (q = quit), assert exit code 0.
4. CI entry.
5. **Exit:** smoke test is green; newcomer can clone, build, and play the game.

### Phase 7f — `06-embedded-qemu` (1 week, after doc 04 phase 4d)

1. `examples/06-embedded-qemu/` with `@[no_std]` + minimal `@[panic_handler]` + `@[global_allocator]` (or no-alloc design).
2. A `memory.ld` linker script.
3. `smoke.sh`: `riven build --target thumbv7em-none-eabihf` then `qemu-system-arm -M mps2-an386 -nographic -kernel target/…/example`; pipe stdout through `timeout 5`; grep the UART output for the expected string.
4. CI entry.
5. **Exit:** smoke test green.

### Phase 7g — `07-async-echo` (0.5 week, after tier-1 phase 3c)

1. `examples/07-async-echo/` — async TCP echo, single-threaded executor.
2. Reuse example 02's smoke script with minor adjustments.
3. **Exit:** smoke green.

## 9. Open Questions & Risks

1. **Example drift.** As stdlib APIs change, examples break. This is the feature, not the bug — but it requires a discipline: every stdlib-API-renaming PR updates every example that uses the old name. Recommend: CI gating catches it; no-one merges a stdlib API rename without touching the examples.
2. **Third-party `termio` dependency.** Making example 05 depend on an external piece is a risk — if `termio` lags behind language changes, the example breaks. Recommend: keep `termio` in the same GitHub org (`riven-lang`), maintained by the compiler team, so PRs can update both in lockstep. Pin to an exact tag in `Riven.toml`.
3. **Example sprawl.** Seven is a lot to maintain. Recommend: five is a reasonable steady state (drop `06-embedded-qemu` to optional, drop `07-async-echo` if async slips).
4. **Scope creep within examples.** A contributor might want to add a 1000-line "realistic web app" example. Recommend: hard cap at 300 lines per example. If more is needed, break into a separate repo.
5. **HTML harness for WASM example.** `index.html` + inline JS is fragile (CORS, fetch, `file://` restrictions in browsers). Recommend: include a `serve.sh` that runs `python3 -m http.server 8000` so `http://localhost:8000/index.html` works; document this.
6. **Example in the dependency graph of the compiler's own tests?** No — examples are separate. The compiler's `tests/fixtures/` are canonical for compiler testing. Examples are *user-facing*.
7. **Platform differences.** Example 02 (TCP echo) works on Linux and macOS identically; example 06 (embedded) needs `qemu-system-arm` installed in CI. Recommend: document prerequisites per example, skip the CI matrix entry on platforms that don't support it.
8. **Example discoverability.** `examples/` is standard and discoverable. But the README should also point at it. Recommend: README's "Quick Start" section grows a line: "For complete projects, see `examples/`."
9. **SPDX headers vs readability.** Adding `# SPDX-License-Identifier: MIT OR Apache-2.0` at the top of every example's `.rvn` file is noisy for a teaching artifact. Recommend: put it in each example's `README.md` + `LICENSE` (symlink or copy). Skip inline `.rvn` headers.
10. **Examples tested with `--release`?** Debug builds are faster; release builds surface LLVM-only bugs. Recommend: CI builds every example with `--release` once, debug once. ~2× CI time for the examples job but catches regressions.
11. **Versioning examples.** When the language hits 1.0, pin examples at tag-level. Example 05's `termio` dep should pin a tag, not `branch = "main"`.
12. **Example size vs language flexibility.** An obviously-too-verbose example suggests the language is too verbose. Watch for readability regressions as examples grow. Recommend: annual prune.
13. **Community-contributed examples.** Do we accept PRs adding new examples? Recommend: yes, with a curator sign-off — tier-4 can't scale to reviewing every submission. Gate: new examples must be < 300 lines, have a `smoke.sh`, pass CI.

## 10. Acceptance Criteria

- [ ] `examples/README.md` exists and lists every example with a one-line description.
- [ ] `examples/01-cli-utility/` builds and runs with the current stdlib; `smoke.sh` passes.
- [ ] At least one example per major tier-1 phase ships when that phase ships:
  - [ ] tier-1 phase 1b → example 01 lands.
  - [ ] tier-1 phase 1c → example 02 lands.
  - [ ] tier-1 phase 2d → example 03 lands.
  - [ ] tier-1 phase 3c → example 07 lands.
- [ ] Every example has its own `README.md` explaining purpose and run instructions.
- [ ] Every example has a `smoke.sh` that exercises the happy path non-interactively.
- [ ] CI runs every example's `smoke.sh` and fails the workflow on any failure.
- [ ] `README.md` at repo root links to `examples/`.
- [ ] A newcomer clone-and-run flow works: `git clone … && cd riven && cargo build -p riven-cli && cd examples/01-cli-utility && ../../target/debug/riven run -- README.md` prints output matching `<lines> <words> <bytes>`.
- [ ] A stdlib API rename PR that breaks an example also updates that example (enforced by CI gating).
- [ ] No example exceeds 300 lines of Riven code.
- [ ] Every example's `Riven.toml` has `description` + `license = "MIT OR Apache-2.0"` fields.
