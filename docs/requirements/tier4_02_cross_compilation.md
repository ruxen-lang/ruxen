# Tier 4.02 — Cross-Compilation

## 1. Summary & Motivation

Riven can compile only for the host it is running on. The Cranelift backend instantiates an ISA from `cranelift_native::builder()` (`crates/riven-core/src/codegen/cranelift.rs:50`), which hard-codes the current CPU. The LLVM backend calls `TargetMachine::get_default_triple()` (`crates/riven-core/src/codegen/llvm/mod.rs:42`), which returns the host triple at compile time of the *Rust compiler that built `inkwell`*. The linker is unconditionally `cc` with `-lc -lm` (`crates/riven-core/src/codegen/object.rs:20,64-70`). There is no `--target` flag in any CLI.

Making Riven cross-compile is a prerequisite for the WASM target (doc 03), for shipping Linux binaries from macOS developer machines without spinning up QEMU, for ARM64 cloud deployment from an x86_64 laptop, and for the long-tail of embedded targets that no_std (doc 04) opens up. This document specifies how to teach the toolchain to accept a triple, pick the right ISA, pick the right linker, find the right runtime, and produce an artifact that runs on the target.

## 2. Current State

### 2.1 Cranelift backend (`crates/riven-core/src/codegen/cranelift.rs:41-75`)

```rust
pub fn new() -> Result<Self, String> {
    // ...
    let isa_builder = cranelift_native::builder()
        .map_err(|e| format!("Failed to create native ISA builder: {}", e))?;
    let isa = isa_builder
        .finish(settings::Flags::new(flag_builder))
        .map_err(|e| format!("Failed to finish ISA: {}", e))?;
    // ...
}
```

`cranelift_native::builder()` reads `/proc/cpuinfo` (or equivalent) to detect the host. Passing a triple is done through `cranelift_codegen::isa::lookup(triple)` (available in 0.130 — already a dep). Cranelift's supported targets are: `x86_64`, `aarch64`, `s390x`, `riscv64`. Notably **not** `wasm32` (Cranelift has a `wasmtime`-oriented wasm backend but it's for JIT, not for AOT object emission).

### 2.2 LLVM backend (`crates/riven-core/src/codegen/llvm/mod.rs:37-83`)

```rust
Target::initialize_all(&InitializationConfig::default());
let target_triple = TargetMachine::get_default_triple();
module.set_triple(&target_triple);

let target = Target::from_triple(&target_triple)
    .map_err(|e| format!("Unknown target: {}", e))?;
let target_machine = target
    .create_target_machine(
        &target_triple,
        "generic",                    // CPU
        "",                           // features
        match self.opt_level { ... },
        RelocMode::PIC,
        CodeModel::Default,
    )
    .ok_or("Failed to create target machine")?;
```

`Target::initialize_all` already registers every LLVM target built into the linked LLVM 18. Switching to a non-host target is one string change — but none of the CLIs let the user make it.

LLVM's target list is broad: `x86_64`, `aarch64`, `arm`, `riscv32`, `riscv64`, `wasm32`, `wasm64`, `mips`, `powerpc`, `thumbv6m`, `thumbv7em`, etc. `wasm32-unknown-unknown` is fully supported.

### 2.3 Linking (`crates/riven-core/src/codegen/object.rs:10-92`)

```rust
pub fn compile_runtime(runtime_c_path: &Path, sanitize: bool) -> Result<PathBuf, String> {
    let mut cmd = Command::new("cc");
    cmd.arg("-c").arg(runtime_c_path).arg("-o").arg(&runtime_o);
    // ...
}

pub fn emit_executable(...) -> Result<(), String> {
    let mut cmd = Command::new("cc");
    cmd.arg(&obj_path).arg(runtime_o).arg("-o").arg(output_path)
       .arg("-lc").arg("-lm");
    // ...
}
```

Every invocation is `cc` on the host's `PATH`. Nothing lets the user pick `aarch64-linux-gnu-gcc`, `wasm-ld`, or `clang --target=…`.

### 2.4 Runtime (`crates/riven-core/runtime/runtime.c`, 426 lines)

Unconditionally:
- Includes `<stdio.h>`, `<stdlib.h>`, `<string.h>`, `<stdint.h>`.
- Uses `malloc`, `free`, `realloc`, `fprintf(stderr, …)`, `abort()`.
- Compiled once per build into a fresh `.o` via `cc -c runtime.c -o runtime.o` — always host-targeted.

There is no per-target runtime. No precompiled `runtime_x86_64-unknown-linux-gnu.o`.

### 2.5 target-lexicon dependency

`crates/riven-core/Cargo.toml:22` already declares `target-lexicon = "0.13"`. The type is used in Cranelift's internal dependencies but not referenced from Riven source. This gives us a tested, zero-new-dep parser for the `arch-vendor-os[-env]` triple grammar.

### 2.6 Release workflow already targets 4 triples

`.github/workflows/release.yml:23-32`:

```yaml
matrix:
  include:
    - target: x86_64-unknown-linux-gnu
      os: ubuntu-latest
    - target: aarch64-unknown-linux-gnu
      os: ubuntu-latest
      cross: true
    - target: x86_64-apple-darwin
      os: macos-14
    - target: aarch64-apple-darwin
      os: macos-14
```

But this is the Rust toolchain cross-compiling the Riven *compiler binaries* — not Riven cross-compiling a user's project. The `cross` crate (line 53-54) is used only for the aarch64-Linux Rust build. Nothing in `riven-core` or `riven-cli` touches a triple.

### 2.7 Triple awareness elsewhere

- `install.sh:87-103` already detects `OS_TAG` (linux-gnu/apple-darwin) and `ARCH_TAG` (x86_64/aarch64), computes `TARGET="${ARCH_TAG}-${OS_TAG}"`, downloads the matching release asset.
- `.github/workflows/release.yml:79-103` creates `riven-${TAG}-${TARGET}.tar.gz`.

So the release infrastructure already thinks in triples; the compiler does not.

## 3. Goals & Non-Goals

### Goals

1. `--target <triple>` flag on `riven build`, `riven run`, `riven check`, and `rivenc`.
2. Triple parsing via `target-lexicon` → `{ architecture, vendor, operating_system, environment, binary_format }`.
3. Both codegen backends honor the target. Cranelift via `isa::lookup(triple)`, LLVM via `Target::from_triple(...)`.
4. Linker selection per target: `cc`, `aarch64-linux-gnu-gcc`, `wasm-ld` via an override table in `Riven.toml`.
5. Sysroot layout: precompiled `runtime.o` per target, shipped or fetched via `riven target add <triple>`.
6. `cfg(target_os = …, target_arch = …, target_family = …)` evaluated from the resolved triple — hooks into doc 01 §4.1.
7. `target/<triple>/debug/` and `target/<triple>/release/` — per-target output directories, so builds for different targets do not clobber each other.
8. `[target.<triple>.dependencies]` from doc 01 §4.1 wired through.
9. CI matrix that smoke-tests cross-compilation to at least `aarch64-unknown-linux-gnu` from x86_64 runners.

### Non-Goals

- Running the cross-compiled binary on the host (QEMU / Rosetta integration). `riven run --target foo` errors cleanly.
- Windows targets in v1 (MSVC toolchain, PE linking). Defer to v2; the architecture permits it.
- Target-specific optimizations beyond what LLVM/Cranelift already do for a named CPU.
- A full target-triple registry like `rustup target list`. Ship with a fixed set (§4.3) and let the user declare arbitrary triples in `Riven.toml` at their own risk.
- Cross-language linking (`riven-cc` wrapping a cross gcc). Users install cross toolchains themselves.

## 4. Surface

### 4.1 CLI

```
riven build  [--target <triple>] [--release] [--features ...] ...
riven run    [--target <triple>] [--release] ...         # errors if target != host
riven check  [--target <triple>] ...
rivenc       [--target <triple>] [--backend=cranelift|llvm] ...

riven target list                                         # lists installed target runtimes
riven target list --all                                   # lists available targets (from the registry)
riven target add <triple>                                 # fetches & installs pre-built runtime.o
riven target remove <triple>
```

Aliases accepted at the command line (canonicalized to full triples by `target-lexicon`):

| Alias | Canonical |
|---|---|
| `x86_64-linux` | `x86_64-unknown-linux-gnu` |
| `aarch64-linux` | `aarch64-unknown-linux-gnu` |
| `x86_64-macos` / `x86_64-darwin` | `x86_64-apple-darwin` |
| `aarch64-macos` / `aarch64-darwin` | `aarch64-apple-darwin` |
| `wasm32` / `wasm` | `wasm32-unknown-unknown` |
| `wasm32-wasi` | `wasm32-wasi` |

### 4.2 Manifest

```toml
[target.x86_64-unknown-linux-gnu]
linker = "clang"
rustflags = ["-C", "link-arg=-fuse-ld=lld"]                 # passed to LLVM as raw flags
runner = "qemu-x86_64"                                       # optional — used by `riven run` if target != host

[target.aarch64-unknown-linux-gnu]
linker = "aarch64-linux-gnu-gcc"
runner = "qemu-aarch64 -L /usr/aarch64-linux-gnu"

[target.wasm32-unknown-unknown]
linker = "wasm-ld"                                           # or `lld -flavor wasm`
# Dependencies resolved only on this target (doc 01 §4.1)
[target.wasm32-unknown-unknown.dependencies]
wasm-bindgen = "0.2"

# cfg(...) gating (doc 01 §5.2)
[target.'cfg(unix)'.dependencies]
nix = "0.28"
```

The `runner` field mirrors Cargo's `cargo run --target` with `CARGO_TARGET_<TRIPLE>_RUNNER`. If present, `riven run --target <triple>` invokes the runner with the built binary as an argument.

### 4.3 Default supported target list

Tiers adapted from Rust's model.

**Tier 1 (tested in CI, binaries shipped in releases):**

| Triple | Notes |
|---|---|
| `x86_64-unknown-linux-gnu` | Default host for CI Linux runners |
| `aarch64-unknown-linux-gnu` | Via cross compilation |
| `x86_64-apple-darwin` | macOS Intel |
| `aarch64-apple-darwin` | macOS Apple Silicon |

**Tier 2 (tested in CI, no prebuilt host binaries):**

| Triple | Notes |
|---|---|
| `wasm32-unknown-unknown` | See tier4_03 |
| `wasm32-wasi` | See tier4_03 |
| `x86_64-unknown-linux-musl` | Static binaries for Alpine / Docker scratch |

**Tier 3 (best-effort, may break):**

| Triple | Notes |
|---|---|
| `aarch64-apple-ios` | Requires user-supplied SDK paths |
| `thumbv7em-none-eabihf` | Cortex-M, depends on no_std (doc 04) |
| `riscv64gc-unknown-linux-gnu` | |
| `riscv32imac-unknown-none-elf` | no_std |

### 4.4 File layout

```
~/.riven/
├── bin/
├── lib/
│   └── runtime/
│       ├── x86_64-unknown-linux-gnu/
│       │   ├── runtime.o                      # precompiled
│       │   ├── runtime.a                      # static archive variant
│       │   └── version                        # compiler version that built this
│       ├── aarch64-unknown-linux-gnu/runtime.o
│       ├── wasm32-unknown-unknown/runtime.o
│       └── wasm32-wasi/runtime.o
└── toolchains/
    ├── x86_64-unknown-linux-gnu/              # optional — cross toolchain fetched by `riven target add`
    │   └── bin/{gcc,ld,ar,...}
    └── ...

<project>/target/
├── <triple>/
│   ├── debug/
│   │   ├── deps/
│   │   └── myapp
│   └── release/
└── .rustc_info.json
```

## 5. Architecture / Design

### 5.1 Triple data flow

```
CLI (--target)   OR   Riven.toml ([build] default-target)   OR   $RIVEN_TARGET   OR   host triple
       │                                                                                   │
       └───────────────────────────┬───────────────────────────────────────────────────────┘
                                   ▼
                      target_lexicon::Triple (parsed, canonicalized)
                                   │
        ┌─────────────┬────────────┼──────────────┬────────────────┬────────────────────┐
        ▼             ▼            ▼              ▼                ▼                    ▼
   cranelift::isa  llvm::Target  cfg evaluator  linker picker  runtime.o resolver  target/<triple>/
      (5.2)         (5.3)         (5.4)          (5.5)            (5.6)              (5.7)
```

### 5.2 Cranelift target binding

Replace `cranelift_native::builder()` in `codegen/cranelift.rs:50-55` with:

```rust
use cranelift_codegen::isa;
use target_lexicon::Triple;

let triple: Triple = triple_str.parse()
    .map_err(|e| format!("invalid target triple '{}': {}", triple_str, e))?;
let isa_builder = isa::lookup(triple.clone())
    .map_err(|e| format!("unsupported Cranelift target '{}': {}", triple, e))?;
let isa = isa_builder
    .finish(settings::Flags::new(flag_builder))
    .map_err(|e| format!("Failed to finish ISA: {}", e))?;
```

Pass the triple through `CodeGen::new` signature. Cranelift supports only x86_64, aarch64, s390x, riscv64 — so `wasm32` must route to LLVM. Codegen backend selection is bumped to check target compatibility before accepting the user's `--backend` choice (§5.8).

### 5.3 LLVM target binding

Replace `TargetMachine::get_default_triple()` in `codegen/llvm/mod.rs:42` with the user-provided triple. The `initialize_all()` call already registers every target built into the linked LLVM 18, so no feature-gate change is needed.

```rust
let target_triple = if let Some(t) = opts.target {
    TargetTriple::create(&t)
} else {
    TargetMachine::get_default_triple()
};
```

LLVM's triple strings are 1:1 with `target-lexicon`'s for the cases we care about. Mismatch spots (notes for the implementer):

- LLVM wants `wasm32-unknown-unknown` with the "unknown" vendor; `target-lexicon` parses that fine.
- LLVM accepts `wasm32-wasi` but canonicalizes to `wasm32-unknown-wasi` internally; passing either works.
- For `aarch64-apple-darwin` on LLVM < 11 (not our case — we require 18), the string is `arm64-apple-darwin`. We always emit the `aarch64-*` form; LLVM 18 accepts it.

### 5.4 `cfg(...)` evaluator

New `crates/riven-cli/src/cfg.rs` — also referenced by doc 01 §5.2 for feature gating. Evaluation inputs:

```rust
pub struct CfgContext {
    pub target_arch: &'static str,          // "x86_64", "aarch64", "wasm32", ...
    pub target_os: &'static str,            // "linux", "macos", "wasi", "none", ...
    pub target_env: &'static str,           // "gnu", "musl", "", ...
    pub target_family: &'static str,        // "unix", "wasm", "", ...
    pub target_vendor: &'static str,        // "unknown", "apple", ...
    pub target_pointer_width: &'static str, // "32", "64"
    pub target_endian: &'static str,        // "little", "big"
    pub features: HashSet<String>,
}
```

Derive from `target_lexicon::Triple`:

```rust
fn cfg_from_triple(triple: &Triple, features: HashSet<String>) -> CfgContext {
    CfgContext {
        target_arch: match triple.architecture {
            Architecture::X86_64 => "x86_64",
            Architecture::Aarch64(_) => "aarch64",
            Architecture::Wasm32 => "wasm32",
            // ...
        },
        target_os: match triple.operating_system {
            OperatingSystem::Linux => "linux",
            OperatingSystem::Darwin => "macos",
            OperatingSystem::Wasi => "wasi",
            OperatingSystem::Unknown => "none",
            // ...
        },
        // ...
    }
}
```

Grammar (subset of Rust's `cfg!`):

```
cfg_expr := 'all' '(' cfg_expr (',' cfg_expr)* ')'
          | 'any' '(' cfg_expr (',' cfg_expr)* ')'
          | 'not' '(' cfg_expr ')'
          | IDENT '=' STRING
          | IDENT
```

### 5.5 Linker selection

`codegen/object.rs:20,64` hard-codes `Command::new("cc")`. Replace with a lookup:

```rust
fn linker_for(triple: &Triple, manifest: &Manifest) -> Result<(String, Vec<String>), String> {
    // 1. Explicit [target.<triple>].linker override from manifest
    if let Some(cfg) = manifest.target.get(&triple.to_string()) {
        if let Some(linker) = &cfg.linker { return Ok((linker.clone(), cfg.link_args.clone())); }
    }

    // 2. Environment override: RIVEN_TARGET_<TRIPLE_UPPER_UNDERSCORED>_LINKER
    let env_key = format!("RIVEN_TARGET_{}_LINKER", triple.to_string().to_uppercase().replace('-', "_"));
    if let Ok(v) = std::env::var(&env_key) { return Ok((v, vec![])); }

    // 3. Built-in defaults
    Ok(match (triple.architecture, &triple.operating_system) {
        (_, OperatingSystem::Darwin) => ("cc".into(), vec![]),
        (_, OperatingSystem::Linux) if triple.architecture == host_arch() => ("cc".into(), vec![]),
        (Architecture::Aarch64(_), OperatingSystem::Linux) => ("aarch64-linux-gnu-gcc".into(), vec![]),
        (Architecture::X86_64, OperatingSystem::Linux) if host_arch() != Architecture::X86_64 =>
            ("x86_64-linux-gnu-gcc".into(), vec![]),
        (Architecture::Wasm32, _) => ("wasm-ld".into(), vec!["--no-entry".into(), "--export-all".into()]),
        _ => return Err(format!("no default linker known for target '{}'. Set [target.<triple>].linker in Riven.toml.", triple)),
    })
}
```

Also drop `-lc -lm` when the target is `wasm32-unknown-unknown` (there is no libc to link).

### 5.6 Per-target runtime

`codegen/mod.rs::find_runtime_c` (lines 27-65) currently locates `runtime.c` source. For cross-compilation we prefer a **precompiled** object to avoid requiring a cross C toolchain just to build the runtime.

New: `codegen/mod.rs::find_runtime_obj(triple: &Triple) -> Result<PathBuf, String>` resolution order:

1. `$RIVEN_RUNTIME_OBJ` (explicit override, path to `.o`).
2. `<exe>/../lib/runtime/<triple>/runtime.o` (installed sysroot).
3. `$RIVEN_HOME/lib/runtime/<triple>/runtime.o`.
4. Dev fallback — compile `runtime.c` locally with `cc --target=<triple> -c`, cache at `target/<triple>/runtime.o`. Only works if the user has a cross toolchain.

If the runtime is missing and the user hasn't run `riven target add <triple>`, emit:

```
error: runtime for target 'aarch64-unknown-linux-gnu' not installed.
       Run `riven target add aarch64-unknown-linux-gnu` to download it,
       or set RIVEN_RUNTIME_OBJ to a path you built yourself.
```

### 5.7 Output directory layout

Today: `target/debug/myapp`, `target/release/myapp`.

Tomorrow: `target/<triple>/debug/myapp`, `target/<triple>/release/myapp`. The host triple remains the default when `--target` is not given. `target/debug/` without a triple prefix is preserved for the host-target case (matches Cargo).

`build.rs:27-28` changes:

```rust
let target_dir = project_dir.join("target");
let target_dir = if let Some(triple) = &opts.target {
    target_dir.join(triple.to_string())
} else {
    target_dir
};
let target_dir = target_dir.join(profile);
```

### 5.8 Backend selection interacts with target

Not every backend supports every target:

| Target | Cranelift | LLVM |
|---|---|---|
| x86_64-unknown-linux-gnu | yes | yes |
| aarch64-unknown-linux-gnu | yes | yes |
| x86_64-apple-darwin | yes | yes |
| aarch64-apple-darwin | yes | yes |
| wasm32-unknown-unknown | **no** | yes |
| wasm32-wasi | **no** | yes |
| thumbv7em-none-eabihf | **no** | yes |
| riscv64gc-unknown-linux-gnu | yes | yes |

Rules:

- Default backend: Cranelift for debug, LLVM for `--release` (already the case in `build.rs:277-288`).
- If the user explicitly passes `--backend=cranelift` with an unsupported target, emit a specific error: `"target 'wasm32-unknown-unknown' requires the LLVM backend. Build with --release or pass --backend=llvm."`
- If `--backend` is not specified and the target requires LLVM, auto-switch (silent) and proceed.

### 5.9 `riven target add` implementation

A small HTTP client that fetches `https://releases.riven.land/<compiler-version>/runtime-<triple>.tar.gz`, verifies a sha256, extracts to `~/.riven/lib/runtime/<triple>/`.

The release workflow (`.github/workflows/release.yml`) grows a matrix job per tier-1 target that compiles `runtime.c` with the matching cross toolchain and uploads `runtime-<triple>.tar.gz` alongside the existing `riven-<tag>-<triple>.tar.gz` assets.

## 6. Implementation Plan — files to touch

### New files

- `crates/riven-cli/src/target.rs` — `riven target list/add/remove` subcommands.
- `crates/riven-cli/src/cfg.rs` — cfg parser + evaluator (shared with doc 01).
- `crates/riven-core/src/codegen/target.rs` — `Target` struct, triple parsing, linker-selection helper.

### Touched files

- `crates/riven-cli/src/cli.rs:24-114` — add `--target <triple>` to `Build`, `Run`, `Check`; add `Target { ... }` subcommand.
- `crates/riven-cli/src/manifest.rs:77-91` — add `[target.<triple>] { linker, link-args, runner, dependencies }` via `BTreeMap<String, TargetSpec>`.
- `crates/riven-cli/src/build.rs:21-118` — thread `Option<Triple>` through `build()`. Re-route `target_dir`.
- `crates/riven-core/src/codegen/mod.rs` — `Backend` variants gain target info; `compile_with_options` accepts `Triple`.
- `crates/riven-core/src/codegen/cranelift.rs:41-75` — `CodeGen::new(triple)` — swap `cranelift_native::builder` for `isa::lookup(triple)`.
- `crates/riven-core/src/codegen/llvm/mod.rs:37-83` — `CodeGen::new(opt_level, triple)` — swap `get_default_triple()` for the provided one.
- `crates/riven-core/src/codegen/object.rs:10-92` — accept `(Triple, &TargetSpec)`; pick linker; drop `-lc -lm` for wasm; thread link-args.
- `crates/riven-core/src/codegen/mod.rs:27-65` — add `find_runtime_obj(triple)` next to `find_runtime_c`.
- `.github/workflows/release.yml` — add a runtime-object job per tier-1 target; publish `runtime-<triple>.tar.gz` alongside the binaries.

### Tests

- `crates/riven-cli/tests/cross_compile.rs` — builds for `aarch64-unknown-linux-gnu` from an x86_64 host, verifies the ELF header is `EM_AARCH64`.
- `crates/riven-core/tests/triple_parsing.rs` — alias resolution, canonicalization, cfg derivation.
- `crates/riven-core/src/codegen/tests.rs` — round-trip a simple program through both backends with `Triple::host()` and compare to the current behavior (regression guard).

## 7. Interactions with Other Tiers

- **Tier 4.01 package manager.** `[target.<triple>.dependencies]` lives in the manifest and is driven by the resolved triple. `cfg(...)` evaluation in doc 01 §5.2 consumes the `CfgContext` built here.
- **Tier 4.03 WASM.** WASM is the first target that *requires* this work — `wasm32-unknown-unknown` has no `cc`, no `-lc`, no `-lm`. Doc 03 §6 lists the specific drops.
- **Tier 4.04 no_std.** `thumbv7em-none-eabihf` and kin imply `no_std`. The linker is the embedded toolchain's `arm-none-eabi-ld`. No runtime libc. Doc 04 §5.3 covers the runtime subset.
- **Tier 4.06 CI.** The CI matrix needs to smoke-test cross-compilation. Recommend `aarch64-unknown-linux-gnu` from Ubuntu runners using `qemu-user-static` for `riven run` to succeed.
- **Tier 1 stdlib.** Some stdlib items are Unix-only (`std::os::unix`), some are Windows-only (future `std::os::windows`). These are `@[cfg(unix)]` / `@[cfg(windows)]` gated and consume the cfg evaluator.
- **Tier 2 codegen.** The `Linkage::Import` FFI resolution (`cranelift.rs:98-103`) must still name-mangle the same way regardless of target — that's already correct, but worth guarding with a per-target test.

## 8. Phasing

### Phase 2a — Plumbing (1 week)

1. Parse `--target` on CLI; default to host.
2. Thread `Triple` through `CodeGen::new` for both backends.
3. Per-target `target/<triple>/` output directory.
4. Write `CfgContext` from the triple (used by doc 01).
5. Keep the linker invocation unchanged — still `cc` for every target.
6. **Exit:** `riven build --target x86_64-unknown-linux-gnu` on a Linux host produces a bit-identical binary to `riven build`.

### Phase 2b — Cross-compile to non-host Linux (1 week)

1. Linker-selection table (§5.5).
2. `riven target add <triple>` stub that expects the user to have a cross cc (v1: no auto-fetch; point at README).
3. `aarch64-unknown-linux-gnu` smoke test in CI with `qemu-user-static`.
4. **Exit:** `riven build --target aarch64-unknown-linux-gnu` on x86_64 Ubuntu produces an aarch64 ELF that runs under QEMU.

### Phase 2c — Runtime object fetching (1 week)

1. Release workflow produces `runtime-<triple>.tar.gz` artifacts per tier-1 target.
2. `riven target add` fetches from `https://releases.riven.land/<version>/runtime-<triple>.tar.gz`.
3. `find_runtime_obj` prefers precompiled over source.
4. **Exit:** `riven target add aarch64-unknown-linux-gnu` on a fresh machine (no cross cc installed) succeeds. Subsequent `riven build --target …` uses the fetched runtime.

### Phase 2d — Tier-3 and long-tail targets (ongoing)

Best-effort. Document what works. Don't gate releases on tier-3.

## 9. Open Questions & Risks

1. **Cranelift's limited target set.** Only x86_64, aarch64, s390x, riscv64. Wasm requires LLVM. Embedded (thumbv7em) requires LLVM. This restricts the debug-build target matrix. Mitigation: auto-switch to LLVM when Cranelift can't handle the target (§5.8).
2. **Cross-linker discovery.** The default `aarch64-linux-gnu-gcc` might not be installed. Error must be actionable: `"linker 'aarch64-linux-gnu-gcc' not found in PATH. On Debian/Ubuntu: apt install gcc-aarch64-linux-gnu"`.
3. **C library presence.** Even if the linker exists, the target's `libc.a` / `libm.a` must be available. On Debian: `libc6-dev-arm64-cross`. We can't auto-install system packages.
4. **macOS → Linux cross.** `cc -target aarch64-linux-gnu` works with a properly configured Clang, but the sysroot (headers, static libs) must be provided. Recommend documenting `zig cc` as a pre-built cross toolchain option.
5. **Rustflags via manifest.** `[target.<triple>].rustflags = [...]` echoes Cargo's dangerous-but-useful pattern. Recommend: for v1 accept them raw, pass through to LLVM as `-C <flag>`. Ignore for Cranelift.
6. **`runner` for non-native `riven run`.** If target != host and no runner is configured, we error. Should we auto-detect QEMU? Recommend: no, too magical. Require explicit config.
7. **Host-triple lock-in.** If a user publishes a piece with a binary runtime in `target/`, it's useless to anyone on a different triple. The `target/` directory is already gitignored (`scaffold.rs:155`), but we should make this explicit in CONTRIBUTING.md.
8. **LLVM 18 vs 19.** `riven-core/Cargo.toml:27` features `llvm18-0`. LLVM 19 is out. Defer upgrade; LLVM 18 covers every target we name.
9. **Reproducibility.** Cross-compiling the same source from two different hosts should produce the same bytes. `cc` may embed host timestamps. Recommend: pass `-ffile-prefix-map` and `-D__DATE__=…` to normalize; leave as phase 2d polish.
10. **Symbol naming differences.** macOS prepends `_` to C symbols; ELF does not. Cranelift and LLVM both handle this when given the right triple, but it's a place bugs hide. Regression test: same Riven source compiled for both Mach-O and ELF, disassembled, confirm `_riven_main` on Mach-O and `riven_main` on ELF.
11. **Environment variables leaking.** `CC`, `CFLAGS`, `LDFLAGS` — Cargo respects these with layered overrides. Recommend: Riven v1 ignores them (manifest + CLI only). Revisit if it causes friction.
12. **Riven-built `.rlib` portability.** A `.rlib` built for x86_64 is useless on aarch64. The lock file's checksum is *source*, not binary — so re-builds for a new target re-compile. `target/<triple>/deps/` isolates this.

## 10. Acceptance Criteria

- [ ] `rivenc --target aarch64-unknown-linux-gnu hello.rvn -o hello_arm` on x86_64 Linux produces an aarch64 ELF whose `readelf -h` shows `Machine: AArch64`.
- [ ] `riven build --target x86_64-unknown-linux-gnu` and `riven build` (no flag) on x86_64 Linux produce byte-identical outputs.
- [ ] `riven build --target wasm32-unknown-unknown` routes to the LLVM backend even without `--release` and produces a `.wasm` file (see doc 03 for validation).
- [ ] `riven target list` lists runtimes present in `~/.riven/lib/runtime/*/`.
- [ ] `riven target add aarch64-unknown-linux-gnu` downloads and extracts the runtime from the release URL, verifies the sha256.
- [ ] `target/<triple>/debug/myapp` exists after `riven build --target <triple>`; `target/debug/myapp` still exists after `riven build` with no flag.
- [ ] `@[cfg(target_os = "linux")] pub def foo` is visible only when the resolved triple's OS is Linux.
- [ ] `@[cfg(target_arch = "wasm32")]` is visible only when targeting wasm32-*.
- [ ] CI matrix includes an `aarch64-unknown-linux-gnu` job that builds the compiler's own test fixtures with `--target` and runs them via `qemu-aarch64-static`.
- [ ] Passing `--backend=cranelift --target=wasm32-unknown-unknown` errors with a specific message pointing at `--backend=llvm`.
- [ ] Passing `--target=invalid-triple` errors with a `target-lexicon` parse-diagnostic pointer at the failing segment.
- [ ] `Riven.toml`'s `[target.'cfg(unix)'.dependencies]` table resolves a dep when the triple's family is unix; skips it when it isn't.
