# 06 — Ruxen no_std

Compiles Ruxen with `--no-std`: no standard library, no C runtime, no
allocator. Demonstrates tier 4.04 (no_std / embedded mode).

## What it shows

- `ruxen compile --no-std` skips the stdlib bootstrap and links **without** the
  Ruxen C runtime or `[system_libs]` — the user's object is the whole program
  (no `ruxen_*` symbol in the binary).
- Heap allocation (`String`/`Array`/`Map`/`Set` construction) is rejected at
  compile time with **E1400** — a no_std build has no allocator.
- A no_std unit can still compute and signal a result via a minimal libc FFI.

## Run it

```bash
# Build the toolchain (default features — no LLVM needed for no_std host):
cargo build -p ruxen_cli

# 1. A no_std program that computes 42 and exits with it:
target/debug/ruxen compile examples/06-no-std/exit42.rx --no-std -o /tmp/exit42
/tmp/exit42; echo "exit=$?"        # → exit=42

# 2. The E1400 negative case (heap allocation rejected):
target/debug/ruxen compile examples/06-no-std/heap_rejected.rx --no-std -o /tmp/x
# → error[E1400]: heap allocation (string literal) is not allowed in a no_std unit
```

`scripts/no_std_verify.sh` runs both bars and asserts them.

## Scope (v1) and platform note

On macOS, `cc` still implicitly links `libSystem` (the OS mandates it for any
dynamic executable — a truly libc-free binary is not possible there), but **no
Ruxen stdlib runtime** is linked. The strict `-nostdlib`, zero-libc-imports
binary is a Linux/embedded target (a `_start` shim + raw exit syscall, verified
in a container) and is the staged remainder, along with the `core`/`std`
re-export surface, the `alloc` tier (heap types with a user `global_allocator`),
and the `panic_handler`/`global_allocator`/`no_std` source directives. See
`docs/decisions/phase4-no-std-wasm.md` and
`docs/requirements/tier4_04_no_std_embedded.md`.
