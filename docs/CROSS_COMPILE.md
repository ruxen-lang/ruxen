# Cross-compilation (tier 4.02)

Ruxen can compile a binary for a target other than the host. Pass a target
triple with `--target`:

```bash
ruxen compile hello.rx --target x86_64-apple-darwin -o hello_x64
ruxen build --target aarch64-unknown-linux-gnu
```

No `--target` → the host, byte-identical to the pre-4.02 behaviour.

## Accepted targets

| Triple | Aliases | Status on a macOS-arm64 dev host |
|---|---|---|
| `aarch64-apple-darwin` | `aarch64-macos`, `arm64-darwin` | host — verified |
| `x86_64-apple-darwin` | `x86_64-macos`, `x86_64-darwin` | cross-arch, links locally, runs under **Rosetta** — **verified** |
| `aarch64-unknown-linux-gnu` | `aarch64-linux` | cross-OS, **two-stage Docker** link — **verified** (runs in a `linux/arm64` container) |
| `x86_64-unknown-linux-gnu` | `x86_64-linux` | cross-OS+arch — **config-ready, CI-proven-at-push** (the sibling repos' ubuntu-latest x64 CI exercises this triple natively at push time) |
| `aarch64-linux-android` | `android`, `aarch64-android` | **config-ready, NDK-gated** — linker logic wired, untested (no NDK on this host) |
| `wasm32-unknown-unknown` | `wasm32`, `wasm` | **next phase** (LLVM backend, prompt 16). Cranelift can't emit wasm; building one errors with a pointer to `--backend=llvm`. |

Short aliases are normalized to the canonical triple before parsing.
(Note: `target-lexicon` 0.13 does *not* canonicalize these itself — it parses
`x86_64-macos` lossily — so Ruxen keeps an explicit alias table. See the ADR.)

## Linker matrix — what links where

| Target | Linker strategy | Requires |
|---|---|---|
| host | `cranelift_native` + `cc` | — |
| Darwin → Darwin (any arch) | `cc -arch <arch>` **locally** | macOS host + Xcode `cc` |
| Linux, host-arch, on a Linux host | plain `cc` | — |
| Linux, with `<arch>-linux-gnu-gcc` on PATH | that cross gcc | the cross toolchain |
| Linux, no local cross toolchain | **two-stage Docker** (`docker run --platform linux/<arch> gcc:13`) | Docker |
| Android | `<arch>-linux-android21-clang` | the Android NDK (untested here) |

### The two-stage Docker flow

When a Linux target has no local cross toolchain (the common case on a macOS
dev machine without `zig`/gnu-cross), Ruxen:

1. emits the target object locally via the cross Cranelift backend
   (`isa::lookup`), then
2. mounts the stdlib runtime tree read-only into a target-native container,
   compiles the runtime **for the target** with the container's native `cc`,
   and links the final binary there.

This needs **Docker** running. When Docker is absent, the error points you at
the `[target.<triple>].linker` manifest override.

## Per-target runtime

The stdlib C runtime is compiled **from source for the target** (locally for a
Darwin cross; in-container for Linux), keyed per-triple so a host object never
poisons a target build (and vice versa). There is no host-object cache
poisoning across targets.

`ruxen target add <triple>` (HTTP-fetch of a *prebuilt* runtime from a release
URL — spec §5.9) is **deferred** to the WASM/CI phase; it currently returns a
clear error rather than a silent no-op. `ruxen target list [--all]` enumerates
installed/known targets.

## Output layout

A cross build writes to `target/<triple>/<profile>/` (e.g.
`target/aarch64-unknown-linux-gnu/debug/myapp`). The host build keeps the
un-prefixed `target/<profile>/` (matches Cargo).

## `ruxen run --target`

`ruxen run --target <non-host>` errors: it does not launch an emulator (spec
non-goal). Build with `ruxen build --target <triple>` and run on the target
(or in a container).

## Backends

Cranelift (default, debug) handles x86_64 / aarch64 / s390x / riscv64. wasm and
embedded targets require the LLVM backend, which auto-engages for `--release`
or when the target demands it; `--backend=cranelift` with such a target errors
with a specific message.

## Verifying

`scripts/cross_verify.sh` runs both acceptance bars (aarch64-linux in Docker,
x86_64-darwin under Rosetta) and SKIPs (never fails) when Docker/Rosetta is
unavailable.

## See also

- ADR: `docs/decisions/cross-compilation-linker-matrix.md`
- Spec: `docs/requirements/tier4_02_cross_compilation.md`
