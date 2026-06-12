# ADR: Cross-compilation linker matrix and two-stage Docker link (tier 4.02)

Status: Accepted (2026-06-12)
Branch: `feat/drop-elaboration`
Spec: `docs/requirements/tier4_02_cross_compilation.md`

## Context

Tier 4.02 teaches the toolchain to accept a `--target <triple>` and emit a
binary for a non-host target. The spec (§5.5, §6 risk #4) leaves the *linker
strategy* open: it suggests `aarch64-linux-gnu-gcc`, `zig cc`, or a manifest
override, but the right choice depends on what is actually installed on the
build host. This ADR records the decisions made for the implementation pass on
the project's build host (macOS arm64, Xcode `cc`/`clang`, Docker running with
a native `linux/arm64` server; **no** `zig`, **no** gnu cross-toolchains).

## Decisions

### 1. Explicit alias table, not `target-lexicon` canonicalization

The spec (§4.1) claims `target-lexicon` canonicalizes short aliases
(`x86_64-macos` → `x86_64-apple-darwin`). **It does not** in 0.13: parsing
`x86_64-macos` yields `operating_system = Unknown`, `binary_format = Elf` — a
silently *wrong* result (macOS is Mach-O), and `aarch64-linux` drops the `gnu`
environment. `.to_string()` does not re-expand. So `codegen::target` keeps an
explicit alias→canonical map and normalizes **before** parsing. Deterministic,
no lossy parse. (Implemented in `compiler/ruxen_core/src/codegen/target.rs`,
pinned by `alias_expansion_fixes_lossy_parse`.)

### 2. Linker matrix as built

| Target | Host (macOS arm64) | Strategy | Verified |
|---|---|---|---|
| `aarch64-apple-darwin` | host | `cranelift_native::builder()` + `cc` | host path, byte-identical |
| `x86_64-apple-darwin` | cross-arch, same OS | `cc -arch x86_64` **locally**; runs under **Rosetta** | acceptance bar (b) — local |
| `aarch64-unknown-linux-gnu` | cross-OS | **two-stage Docker** (`linux/arm64`, native here) | acceptance bar (a) — in-container |
| `x86_64-unknown-linux-gnu` | cross-OS+arch | two-stage Docker (`linux/amd64`, emulated here) OR native cross-gcc on a Linux host | **config-ready, CI-proven-at-push** (canvas ubuntu-latest x64 job exercises this triple natively at push time) |
| `aarch64-linux-android` | cross | NDK `aarch64-linux-android21-clang` (wired, **untested** — no NDK on host) | config-ready, NDK-gated |
| `wasm32-unknown-unknown` | — | LLVM backend, NEXT phase (prompt 16) | out of scope; §5.8 error fires on `--backend=cranelift` |

When a conventional cross gcc (`<arch>-linux-gnu-gcc`) **is** on PATH,
`linker_for` prefers it over the container; the container is the honest
fallback when nothing local can link the target.

### 3. Two-stage Docker link (Linux target, no local cross toolchain)

Rather than require the user to install `zig`/gnu-cross, a Linux target on a
host without a cross linker is linked **inside a target-native container**:

1. Emit the target `.o` locally via the cross Cranelift backend (`isa::lookup`).
2. `docker run --platform linux/<arch> -v <scratch>:/work gcc:13 cc <obj>
   <runtime.c...> -o <out>` — the container's native `cc` + glibc compile the
   stdlib runtime **for the target** and link the final binary.
3. Copy the binary out of the scratch mount; preserve the exec bit.

This satisfies spec §5.6 (runtime compiled *for the target*) without a host
cross-cc, and keeps `ruxen compile --target aarch64-unknown-linux-gnu`
end-to-end functional. The base image `gcc:13` is a deliberate, boring choice
(ships `cc` + glibc + libm). Implemented in
`object::emit_executable_in_container`.

### 4. Per-target runtime = source-compile; HTTP fetch deferred

The spec's preferred path (§5.6/§5.9) fetches a *prebuilt* `runtime.o` from
`releases.ruxen.land/<version>/runtime-<triple>.tar.gz`. That URL does not
exist. We implement the spec's dev fallback (§5.6 item 4): compile the stdlib
runtime `.c` **for the target** (target `cc` locally, or in-container for
Linux), keyed per-triple so a host object never poisons the target cache. The
HTTP fetch + the release-workflow artifact job are deferred to the WASM/CI
phase; `ruxen target add`/`remove` return a **loud `Err`** (anti-silent-no-op
rule), not a fake success. `ruxen target list` enumerates installed runtimes.

### 5. Deferred to tier 4.01 (recorded so we don't paint it in)

The `cfg(...)` *expression parser* and `[target.<triple>.dependencies]`
gating belong to tier 4.01 (package manager), per spec §7. This pass derives
only the `CfgContext` fact table (§5.4) the evaluator will consume.

## Consequences

- The host path is byte-identical to pre-4.02 (no `--target` → no behaviour
  change). This is the back-compat bar; the full gate + sibling spot-check
  guard it.
- Cross builds depend on Docker for Linux targets on this host. The error when
  Docker is absent is actionable (points at the manifest linker override).
- `x86_64-unknown-linux-gnu` gets free verification when the sibling repos' CI
  ubuntu-latest (x64) jobs run at push time — recorded as "config-ready,
  CI-proven-at-push" rather than "untested".
