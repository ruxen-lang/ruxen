# Repo layout (rust-lang/rust style)

Riven follows rust-lang/rust's top-level partition between compiler,
library (runtime + std), tooling drivers, and tests.

```
riven/
├── compiler/                       # one crate per compiler phase
│   ├── riven_lexer/
│   ├── riven_parser/
│   ├── riven_ast/
│   ├── riven_hir/
│   ├── riven_resolve/              # scope + import resolution + stdlib registrations
│   ├── riven_typeck/               # core inference + per-namespace method resolvers
│   ├── riven_mir/
│   ├── riven_borrowck/
│   ├── riven_codegen_shared/       # "Class_method" → "riven_*" tables shared between backends
│   ├── riven_codegen_cranelift/
│   ├── riven_codegen_llvm/
│   ├── riven_diagnostics/
│   ├── riven_formatter/
│   └── riven_driver/               # orchestrator that re-exports the phase crates
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
│   └── std/                        # .rvn-source side of the stdlib
│       └── src/
│           ├── iter.rvn
│           └── …
│
├── src/                            # tooling & drivers
│   ├── rivenc/                     # CLI compiler driver
│   ├── riven_lsp/
│   ├── riven_ide/
│   └── riven_repl/
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
  No upward edges (codegen does not import resolve). The `riven_driver`
  crate sits on top and orchestrates each phase.
- `compiler/*` may **not** depend on `library/*`. The compiler is pure
  Rust; the runtime is C and is linked into the user binary, not into
  the compiler.
- `library/runtime/*` is built by `library/runtime/build.rs` into a
  single static lib (`libriven_runtime.a`) that the codegen crates emit
  a link directive for. Splitting the C source files does **not** mean
  splitting the link product — every `.c` file is concatenated via
  `#include` into one translation unit, archived once, and consumed by
  every Riven binary.
- `library/std/*.rvn` is read at compile-time by `riven_resolve` via
  the implicit-includes pipeline.
- `src/rivenc` depends on `compiler/riven_driver` plus the codegen
  backend it chooses at runtime.
- `tests/*` lives at workspace root and depends on
  `compiler/riven_driver` + `src/rivenc` only.

## Why this layout

The previous layout was a single `crates/riven-core` crate that owned
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
   `compiler/riven_resolve/src/stdlib/io.rs`,
   `compiler/riven_typeck/src/method_resolvers/io.rs`.

The layout follows rust-lang/rust deliberately: anyone fluent in that
tree can navigate Riven's tree without a guided tour.
