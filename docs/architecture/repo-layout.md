# Repo layout (rust-lang/rust style)

Ruxen follows rust-lang/rust's top-level partition between compiler,
library (runtime + std), tooling drivers, and tests.

```
ruxen/
├── compiler/                       # one crate per compiler phase
│   ├── ruxen_lexer/
│   ├── ruxen_parser/
│   ├── ruxen_ast/
│   ├── ruxen_hir/
│   ├── ruxen_resolve/              # scope + import resolution + stdlib registrations
│   ├── ruxen_typeck/               # core inference + per-namespace method resolvers
│   ├── ruxen_mir/
│   ├── ruxen_borrowck/
│   ├── ruxen_codegen_shared/       # "Class_method" → "ruxen_*" tables shared between backends
│   ├── ruxen_codegen_cranelift/
│   ├── ruxen_codegen_llvm/
│   ├── ruxen_diagnostics/
│   ├── ruxen_formatter/
│   └── ruxen_driver/               # orchestrator that re-exports the phase crates
│
├── library/                        # everything user code sees at runtime
│   ├── runtime/                    # C runtime, unity-built from per-module files
│   │   ├── runtime.h               # public C surface
│   │   ├── runtime.c               # top-level aggregator (#includes per-module files)
│   │   ├── core/                   # alloc, vec, string, hash
│   │   ├── io/                     # stdio, file, bufio, io_error
│   │   ├── fs.c
│   │   ├── net/                    # tcp, shutdown
│   │   ├── time.c
│   │   ├── process.c
│   │   ├── fmt.c
│   │   ├── env.c
│   │   └── signal.c
│   └── std/                        # .rx-source side of the stdlib
│       └── src/
│           ├── iter.rx
│           └── …
│
├── src/                            # tooling & drivers
│   ├── ruxenc/                     # CLI compiler driver
│   ├── ruxen_lsp/
│   ├── ruxen_ide/
│   └── ruxen_repl/
│
├── tests/                          # workspace-level integration + e2e
│   ├── release-e2e/
│   ├── stdlib/                     # per-stdlib-module integration tests
│   └── ui/                         # diagnostic snapshot tests
│
├── docs/
├── Cargo.toml                      # workspace root; members = compiler/* src/* tests/*
└── README.md
```

## Crate dependency invariants

- `compiler/*` may depend on each other in phase order:
  `lexer → parser → ast → hir → resolve → typeck → mir → borrowck → codegen_*`.
  No upward edges (codegen does not import resolve). The `ruxen_driver`
  crate sits on top and orchestrates each phase.
- `compiler/*` may **not** depend on `library/*`. The compiler is pure
  Rust; the runtime is C and is linked into the user binary, not into
  the compiler.
- `library/runtime/*` is built by `library/runtime/build.rs` into a
  single static lib (`libruxen_runtime.a`) that the codegen crates emit
  a link directive for. Splitting the C source files does **not** mean
  splitting the link product — every `.c` file is concatenated via
  `#include` into one translation unit, archived once, and consumed by
  every Ruxen binary.
- `library/std/*.rx` is read at compile-time by `ruxen_resolve` via
  the implicit-includes pipeline.
- `src/ruxenc` depends on `compiler/ruxen_driver` plus the codegen
  backend it chooses at runtime.
- `tests/*` lives at workspace root and depends on
  `compiler/ruxen_driver` + `src/ruxenc` only.

## Why this layout

The previous layout was a single `crates/ruxen-core` crate that owned
~26 KLOC across lexer, parser, hir, resolve, typeck, mir, borrowck,
codegen, the formatter, the runtime C, and every stdlib registration.
Three concrete problems it caused:

1. **Rebuild blast radius.** Every typeck-only edit recompiled all of
   codegen + mir + borrowck. Per-phase crates let cargo skip 60–80% of
   that.
2. **Merge-conflict surface.** Every Phase 2 stdlib prompt touched
   `runtime.c`, `resolve/mod.rs`, and `typeck/infer.rs` simultaneously.
   Per-namespace files turn that contention into independent edits.
3. **Code locality.** Reading "how is `std.io.File` wired?" used to
   require reading 4 files in 4 different parts of the tree. After
   the split it's three files in three predictable places:
   `library/runtime/io/file.c`,
   `compiler/ruxen_resolve/src/stdlib/io.rs`,
   `compiler/ruxen_typeck/src/method_resolvers/io.rs`.

The layout follows rust-lang/rust deliberately: anyone fluent in that
tree can navigate Ruxen's tree without a guided tour.
