# Tier 1.01 — Standard Library (v1)

## 1. Summary & Motivation

Today, Riven programs can only call a handful of built-in functions (`puts`, `print`, `eputs`) plus method-level stubs on `String`, `Vec`, `Option`, and `Result` that are hard-coded in the typechecker and routed to `runtime.c` by a name-mangling table (`crates/riven-core/src/codegen/runtime.rs`). There is no `io`, no `fs`, no `env`, no `process`, no `time`, no `net`, no `path`, no `fmt`, no iterator trait; `Vec.map` / `Vec.filter` are compiled to `riven_noop_passthrough`. This document specifies the v1 standard library: the set of modules, types, traits, and method surfaces a Riven program can rely on; how those surfaces are exposed through the module system; and how the compiler, runtime, and toolchain must change to deliver them. The goal is an honest-to-goodness "batteries-included" core that lets a user build a CLI tool, read a config file, make an HTTP-style byte-stream request, and format structured data without reaching for FFI — while still composing cleanly with Riven's ownership model (P4) and single-obvious-path philosophy (P3).

## 2. Current State

This section inventories what exists *now* in the repo, so that the implementation plan in §7 can point at concrete deltas.

### 2.1 C runtime (`crates/riven-core/runtime/runtime.c`, 426 lines)

All extant stdlib surface resolves to one of ~25 C functions in a single file:

| Area | Functions | Lines |
|---|---|---|
| Printing | `riven_puts`, `riven_print`, `riven_eputs`, `riven_print_int`, `riven_print_float` | 29-55 |
| To-string conversions | `riven_int_to_string`, `riven_float_to_string`, `riven_bool_to_string` | 59-92 |
| String ops | `riven_string_eq`, `riven_string_cmp`, `riven_string_concat`, `riven_string_from`, `riven_string_len`, `riven_string_is_empty`, `riven_string_push_str`, `riven_string_trim`, `riven_string_to_lower` | 98-216 |
| Memory | `riven_alloc`, `riven_dealloc`, `riven_realloc` | 144-163 |
| Vec | `riven_vec_new`, `riven_vec_push`, `riven_vec_len`, `riven_vec_get`, `riven_vec_get_mut`, `riven_vec_get_opt`, `riven_vec_get_mut_opt`, `riven_vec_is_empty`, `riven_vec_each` | 221-322 |
| `&str` | `riven_str_split`, `riven_str_parse_uint` | 326-372 |
| Option/Result | `riven_option_unwrap_or`, `riven_result_unwrap_or_else`, `riven_result_try_op` | 377-405 |
| Fallbacks | `riven_noop_passthrough`, `riven_noop_return_null`, `riven_noop` | 410-419 |
| Panic | `riven_panic` | 423-426 |

Critical limitations:

- `Vec` is hard-coded to hold `int64_t` elements (line 220-225). No element-type genericity in the runtime — the generated code always passes a 64-bit slot, so collections of `String`, tuples, or user classes are *already* working by accident because the box pointer fits in 64 bits. Collections of structs larger than 64 bits will misbehave silently.
- `HashMap` and `Set` are declared in the type system (`Ty::HashMap`, `Ty::Set` in `crates/riven-core/src/hir/types.rs:77-79`) but have **zero** runtime functions and **zero** typechecker method entries.
- `Vec.map`, `Vec.filter`, `Vec.find`, `Vec.position`, `Vec.partition`, `Option.map`, `Result.map_err`, `Result.ok_or`, all iterator chain methods → compile to `riven_noop_passthrough` or `riven_noop_return_null`. See `crates/riven-core/src/codegen/runtime.rs:86-167`. They typecheck but do nothing at runtime.
- `String.to_upper` is declared in `builtin_method_type` (`crates/riven-core/src/typeck/infer.rs:822`) but has no C implementation — it would link against `String_to_upper` which does not exist.
- No `FILE*`, no `fopen`, no sockets, no clock, no env, no `argv`.
- No `panic!`, `println!`, `eprintln!`, `format!`, `dbg!` macros. `vec!` exists (`crates/riven-core/src/mir/lower.rs:1502-1535`), `hash!` is documented (tutorial 13) but unimplemented.

### 2.2 Type system (`crates/riven-core/src/hir/types.rs`)

The `Ty` enum already knows about: `String` (66), `Str` (68), `Vec(Box<Ty>)` (75), `Hash(Box<Ty>, Box<Ty>)` (77), `Set(Box<Ty>)` (79), `Option(Box<Ty>)` (83), `Result(Box<Ty>, Box<Ty>)` (85), `Array(Box<Ty>, usize)` (73), tuples, references with lifetimes, function types, raw pointers, newtypes, aliases. No dedicated `Ty::Path`, `Ty::PathBuf`, `Ty::Instant`, `Ty::Duration`, `Ty::SystemTime`, `Ty::File` — those must be modeled as `Ty::Class { name, .. }` (the same mechanism that holds `SplitIter`, `VecIter`, `VecIntoIter` today at `infer.rs:823, 842, 843`).

### 2.3 Resolver / builtin registration (`crates/riven-core/src/resolve/mod.rs`)

`Resolver::register_builtins` (lines 97-343) registers:

- 18 primitive type aliases (`Int`, `Int8`, ..., `Float`, `Bool`, `Char`, `String`) at 99-118.
- 10 built-in traits (`Displayable`, `Error`, `Comparable`, `Hashable`, `Iterable`, `Iterator`, `FromIterator`, `Copy`, `Clone`, `Debug`, `Drop`) at 139-151. All of them have zero generic params in their `TraitInfo` and only list required method names as strings — the method signatures are not registered, which is why the typechecker falls back to structural matching in `traits.rs:106-125`.
- 3 built-in top-level functions (`puts`, `eputs`, `print`) at 173-195.
- 4 built-in type constructors (`Vec`, `Hash`, `Set`, `String`) at 198-215, registered as `DefKind::Variable` so that `Vec.new` resolves.
- `Option`/`Result` enums with `Some`/`None`/`Ok`/`Err` variants (221-325).
- A `super` shim (328-342).

The registry uses string-keyed lookups in a `HashMap<String, DefId>` (`type_registry`, line 37). This is the point at which a new module like `io` or `fs` would be injected.

Use-decls are resolved by `resolve_use_decl` (line 1180-1238) and already support `use Foo.Bar` (Simple), `use Foo.Bar as B` (Alias), `use Foo.Bar.{X, Y}` (Group). The walker handles `Module`, `Enum`, `Class` as namespaces (1263-1319). **There is no `crate::` or `std::` root** — the first segment is looked up in the current scope or type registry.

### 2.4 Typechecker method surface (`crates/riven-core/src/typeck/infer.rs`)

`builtin_method_type` (lines 813-928) is a giant `match` that hard-codes the return type of every built-in method. Additions to the stdlib must land here.

### 2.5 Codegen name mangling (`crates/riven-core/src/codegen/runtime.rs`)

`runtime_name()` maps mangled Riven method names (`Vec[T]_push`, `String_trim`, `Option[T]_unwrap_or`, …) to C symbols. It has a large fallback block (lines 131-167) that maps unknown `?T…` (unresolved inference-variable) methods to best-effort runtime calls — this is how generic methods limp through today. The list `RUNTIME_FUNCTIONS` (lines 11-26) is out of date: it only names 14 of the ~25 functions actually in `runtime.c`.

### 2.6 Module discovery & linking (`crates/riven-cli/src/`)

- `module_discovery.rs` walks `src/**.rvn` and turns file paths into `UpperCamelCase` dotted module paths (e.g. `src/http/client.rvn` → `Http.Client`). This is how user modules get loaded.
- `build.rs` `gather_sources` (line 356-388) concatenates the entry file and all modules into one big source string. The compiler does not yet link separately-compiled modules in the main project — *.rlib loading exists for dependencies but not intra-project.
- `codegen/mod.rs` `find_runtime_c` (line 27-65) searches for `runtime.c` at `$RIVEN_RUNTIME`, `<exe>/../lib/runtime.c`, `<exe>/../share/riven/runtime.c`, or the workspace dev path. This is the hook for shipping additional runtime source.
- `install.sh` already copies `lib/`, `share/`, `include/` from the release tarball into `~/.riven/` (lines 161-168).

### 2.7 Documentation surface

The tutorial (`docs/tutorial/`) *already* writes code that calls methods not yet implemented: `File.read_string(path)?` (tutorial 11, 7), `input.trim.parse_int` (tutorial 11), `read_line()` (tutorial 5), `hash!{...}` (tutorial 13), `h.insert`, `h.contains_key`, `s.insert`, `s.contains`, `greeting.chars`, `greeting.char_count` (tutorial 13). These are *aspirational* and frame what v1 must deliver.

## 3. Goals & Non-Goals

### Goals

1. A discoverable, documented stdlib surface so the tutorial examples compile and run.
2. `Hash` and `Set` work end-to-end (type-check, borrow-check, codegen, run).
3. `Vec.map`, `Vec.filter`, `Vec.each`, `Vec.find`, `Option.map`, `Result.map` stop being no-ops.
4. A module system (`use std::io`) that scales to 10-15 modules without ad-hoc additions to `resolve/mod.rs::register_builtins`.
5. Formatting: `println!`, `eprintln!`, `format!`, `dbg!`, `panic!` as first-class compiler macros; `Debug`/`Display` traits enforceable.
6. Build-time and link-time story: stdlib ships in the release tarball, is located the same way `runtime.c` is, and does not require the user's build to download anything.
7. FFI paths for `fs`, `env`, `process`, `time` use the existing `extern "C"` mechanism (see tutorial 14) through vetted wrappers — so `std::fs::read_to_string` is a Riven function that calls `fopen`/`fread`/`fclose` under the hood.
8. The stdlib is organized so `no_std`-style subsets are possible later (P3: one path, but the path need not be all-or-nothing).

### Non-Goals

- **Concurrency primitives** (`thread`, `sync`, channels, mutex, atomics). These live in a sibling doc — `tier1_02_concurrency.md` will cover them.
- **Randomness.** `rand` is a separate concurrency-adjacent concern; out of scope here.
- **Unicode beyond ASCII-correct byte semantics.** `String.chars` enumerates `u8` today; proper UTF-8 decoding is explicitly deferred.
- **Async I/O.** All `io`/`fs`/`net` calls in v1 are blocking.
- **A registry for third-party packages.** The existing `riven add` plumbing (git / path / version) is sufficient.
- **A `core` crate separation.** We leave room for it (§6.3) but v1 ships a single `std` prelude.

## 4. Scope — Modules, Types, Functions

Every module below is a Riven module under `std`. Unless stated otherwise, items are public. Names follow the conventions in `docs/tutorial/16`: `snake_case` for functions, `UpperCamelCase` for types, `SCREAMING_SNAKE_CASE` for constants.

### 4.1 `std::prelude` (auto-imported)

Everything listed here is in scope without a `use`. Matches Ruby's "core" feel (P3, P5 for boundaries).

- Types: `String`, `Vec[T]`, `Hash[K,V]`, `Set[T]`, `Option[T]`, `Result[T,E]`, `Box[T]` (deferred — see §9), `Range`, `RangeInclusive`.
- Traits: `Display`, `Debug`, `Clone`, `Copy`, `Drop`, `Eq`, `Ord` (new), `PartialEq`/`PartialOrd` (new, explicit), `Hash` (trait, distinct from `Hash[K,V]` — rename one of these; see §9 Open Questions), `Iterator`, `IntoIterator`, `FromIterator`, `Error`, `Default`, `From[T]`, `Into[T]`, `TryFrom[T]`, `TryInto[T]`.
- Macros: `println!`, `eprintln!`, `print!`, `eprint!`, `format!`, `panic!`, `dbg!`, `todo!`, `unimplemented!`, `assert!`, `assert_eq!`, `vec!`, `hash!`, `set!`.
- Functions: `puts` (legacy; kept for one release, delegates to `println!`), `eputs`, `print`.

### 4.2 `std::io`

```riven
use std::io

pub def read_line -> Result[String, IoError]
pub def stdin -> Stdin
pub def stdout -> Stdout
pub def stderr -> Stderr

pub class Stdin
  pub def read_line(self) -> Result[String, IoError]
  pub def read_to_string(self) -> Result[String, IoError]
  pub def lines(self) -> Lines        # Iterator[Result[String, IoError]]
end

pub class Stdout
  pub def write(&mut self, bytes: &[UInt8]) -> Result[USize, IoError]
  pub def write_str(&mut self, s: &str)   -> Result[(), IoError]
  pub def flush(&mut self)                 -> Result[(), IoError]
end

pub class Stderr  # same surface as Stdout

pub enum IoError
  NotFound(path: String)
  PermissionDenied(path: String)
  AlreadyExists(path: String)
  InvalidInput(message: String)
  UnexpectedEof
  Interrupted
  Other(code: Int32, message: String)
end
```

Backing C functions: `fgets`, `fread`, `fwrite`, `fflush`, `ferror`, `errno`. `IoError::Other.code` is a raw `errno` for round-tripping.

### 4.3 `std::fs`

```riven
use std::fs
use std::path::{Path, PathBuf}

pub def read_to_string(path: &impl AsRef[Path]) -> Result[String, IoError]
pub def read(path: &impl AsRef[Path]) -> Result[Vec[UInt8], IoError]
pub def write(path: &impl AsRef[Path], contents: &[UInt8]) -> Result[(), IoError]
pub def write_str(path: &impl AsRef[Path], contents: &str) -> Result[(), IoError]
pub def exists(path: &impl AsRef[Path]) -> Bool
pub def is_file(path: &impl AsRef[Path]) -> Bool
pub def is_dir(path: &impl AsRef[Path])  -> Bool
pub def remove_file(path: &impl AsRef[Path]) -> Result[(), IoError]
pub def remove_dir(path: &impl AsRef[Path])  -> Result[(), IoError]
pub def create_dir(path: &impl AsRef[Path])  -> Result[(), IoError]
pub def create_dir_all(path: &impl AsRef[Path]) -> Result[(), IoError]
pub def rename(from: &impl AsRef[Path], to: &impl AsRef[Path]) -> Result[(), IoError]
pub def metadata(path: &impl AsRef[Path]) -> Result[Metadata, IoError]
pub def read_dir(path: &impl AsRef[Path]) -> Result[ReadDir, IoError]   # Iterator[DirEntry]

pub class File
  pub def self.open(path: &impl AsRef[Path])   -> Result[File, IoError]
  pub def self.create(path: &impl AsRef[Path]) -> Result[File, IoError]
  pub def read(&mut self, buf: &mut [UInt8])   -> Result[USize, IoError]
  pub def read_to_string(&mut self, buf: &mut String) -> Result[USize, IoError]
  pub def read_to_end(&mut self, buf: &mut Vec[UInt8]) -> Result[USize, IoError]
  pub def write(&mut self, bytes: &[UInt8])    -> Result[USize, IoError]
  pub def flush(&mut self)                      -> Result[(), IoError]
  pub def sync(&self)                           -> Result[(), IoError]
  pub def metadata(&self)                       -> Result[Metadata, IoError]
end

pub struct Metadata
  pub len: UInt64
  pub is_file: Bool
  pub is_dir: Bool
  pub modified: Result[SystemTime, IoError]
end
```

### 4.4 `std::net` (minimal)

Deferred to phase 1c. Surface:

```riven
use std::net

pub class TcpStream
  pub def self.connect(addr: &str) -> Result[TcpStream, IoError]
  pub def read(&mut self, buf: &mut [UInt8]) -> Result[USize, IoError]
  pub def write(&mut self, buf: &[UInt8])    -> Result[USize, IoError]
  pub def shutdown(&self)                     -> Result[(), IoError]
  pub def peer_addr(&self)                    -> Result[String, IoError]
end

pub class TcpListener
  pub def self.bind(addr: &str)               -> Result[TcpListener, IoError]
  pub def accept(&mut self)                    -> Result[TcpStream, IoError]
  pub def incoming(&mut self)                  -> Incoming     # Iterator[Result[TcpStream, IoError]]
end
```

No DNS resolver beyond what `getaddrinfo` gives us; no UDP; no TLS. These are future work.

### 4.5 `std::time`

```riven
use std::time

pub struct Duration
  # Opaque; constructors:
  pub def self.from_secs(s: UInt64)    -> Duration
  pub def self.from_millis(ms: UInt64) -> Duration
  pub def self.from_micros(us: UInt64) -> Duration
  pub def self.from_nanos(ns: UInt64)  -> Duration
  pub def self.zero                     -> Duration

  pub def as_secs(&self)        -> UInt64
  pub def as_millis(&self)      -> UInt64
  pub def as_micros(&self)      -> UInt64
  pub def as_nanos(&self)       -> UInt128  # see §9
  pub def subsec_nanos(&self)   -> UInt32

  # Arithmetic (via operator overload trait impls)
  impl Add[Duration] for Duration   # a + b
  impl Sub[Duration] for Duration   # a - b (saturating)
  impl Ord for Duration
end

pub struct Instant
  pub def self.now             -> Instant
  pub def elapsed(&self)       -> Duration
  pub def duration_since(&self, earlier: Instant) -> Duration
end

pub struct SystemTime
  pub def self.now                     -> SystemTime
  pub def self.UNIX_EPOCH              -> SystemTime
  pub def duration_since(&self, earlier: SystemTime)
       -> Result[Duration, SystemTimeError]
end
```

Backing: `clock_gettime(CLOCK_MONOTONIC)` for `Instant`, `CLOCK_REALTIME` for `SystemTime`.

### 4.6 `std::env`

```riven
use std::env

pub def args -> Vec[String]                            # argv, owned strings
pub def var(name: &str) -> Result[String, VarError]
pub def vars -> Vec[(String, String)]
pub def set_var(name: &str, value: &str)
pub def remove_var(name: &str)
pub def current_dir -> Result[PathBuf, IoError]
pub def set_current_dir(path: &impl AsRef[Path]) -> Result[(), IoError]
pub def home_dir -> Option[PathBuf]                    # $HOME on unix

pub enum VarError
  NotPresent
  NotUnicode(String)
end

pub const ARCH:   &str    # "x86_64" | "aarch64"
pub const OS:     &str    # "linux" | "macos"
pub const FAMILY: &str    # "unix"
```

`env::args()` must be populated from `argc`/`argv` at program start; this requires emitting a `main` shim in codegen (see §7.6).

### 4.7 `std::process`

```riven
use std::process

pub def exit(code: Int32) -> !            # uses Ty::Never
pub def abort() -> !
pub def id() -> UInt32

pub class Command
  pub def self.new(program: &str) -> Command
  pub def arg(self, a: &str) -> Command              # consume self, return self
  pub def args[I: IntoIterator[Item=String]](self, xs: I) -> Command
  pub def env(self, key: &str, val: &str) -> Command
  pub def current_dir(self, path: &impl AsRef[Path]) -> Command
  pub def spawn(self) -> Result[Child, IoError]
  pub def status(self) -> Result[ExitStatus, IoError]
  pub def output(self) -> Result[Output, IoError]
end

pub struct Output
  pub status: ExitStatus
  pub stdout: Vec[UInt8]
  pub stderr: Vec[UInt8]
end

pub struct ExitStatus
  pub def success(&self)   -> Bool
  pub def code(&self)      -> Option[Int32]
end

pub class Child
  pub def wait(self) -> Result[ExitStatus, IoError]
  pub def kill(&mut self) -> Result[(), IoError]
  pub def id(&self) -> UInt32
end
```

Backing: `execvp`, `fork`, `waitpid`, `pipe` on Unix.

### 4.8 `std::fmt`

This module is *definitional* — it publishes the traits and types that format macros rely on. Implementation of the macros themselves lives in the compiler (see §7.4).

```riven
module std::fmt
  pub trait Display
    def fmt(&self, f: &mut Formatter) -> Result[(), FmtError]
  end

  pub trait Debug
    def fmt(&self, f: &mut Formatter) -> Result[(), FmtError]
  end

  pub class Formatter
    pub def write_str(&mut self, s: &str) -> Result[(), FmtError]
    pub def write_char(&mut self, c: Char) -> Result[(), FmtError]
    # width/precision/fill knobs:
    pub def width(&self) -> Option[USize]
    pub def precision(&self) -> Option[USize]
    pub def fill(&self) -> Char
    pub def alignment(&self) -> Alignment
  end

  pub enum Alignment
    Left
    Right
    Center
  end

  pub enum FmtError
    WriteFailed
  end
end
```

Every primitive (`Int`, `Float`, `Bool`, `Char`, `String`, `&str`), `Vec`, `Hash`, `Set`, `Option`, `Result`, `Tuple`, `Array` ships with blanket `Display` and/or `Debug` impls. User types get `Display` via explicit `impl Display for T`; `Debug` is auto-derivable (`derive Debug` on struct/class bodies, already parsed — see `StructDef::derive_traits` at `parser/ast.rs:597`).

Format strings handled by the compiler-side macro expander: `{}` (Display), `{:?}` (Debug), `{:width.precision$}`, `{:>10}`, `{:<}`, `{:^}`, `{:0>4}`, `{:x}`, `{:X}`, `{:b}`, `{:o}`, `{:e}`, `{:.3}`.

### 4.9 `std::path`

```riven
use std::path

pub struct Path          # an unsized borrowed slice over a &[UInt8]; modeled as a newtype around &str for v1
pub struct PathBuf       # owned, heap-allocated

impl Path
  pub def self.new(s: &str) -> &Path
  pub def file_name(&self) -> Option[&str]
  pub def extension(&self) -> Option[&str]
  pub def parent(&self)    -> Option[&Path]
  pub def is_absolute(&self) -> Bool
  pub def is_relative(&self) -> Bool
  pub def join(&self, other: &impl AsRef[Path]) -> PathBuf
  pub def to_path_buf(&self) -> PathBuf
  pub def to_string_lossy(&self) -> &str
  pub def components(&self) -> Components          # Iterator[&str]
end

impl PathBuf
  pub def self.new                 -> PathBuf
  pub def self.from(s: String)     -> PathBuf
  pub def push(&mut self, p: &impl AsRef[Path])
  pub def pop(&mut self) -> Bool
  pub def set_extension(&mut self, ext: &str) -> Bool
  pub def as_path(&self) -> &Path
end

pub trait AsRef[T]
  def as_ref(&self) -> &T
end
impl AsRef[Path] for &str
impl AsRef[Path] for String
impl AsRef[Path] for Path
impl AsRef[Path] for PathBuf
```

For v1 `Path` is a thin newtype wrapper on `&str`; on Windows (future work) it will switch to `[UInt8]`. The abstraction insulates callers now so that change is non-breaking.

### 4.10 `std::hash`

Defines the `Hash` *trait* (for hashable keys) separately from the `Hash[K, V]` *type* (see §9 on the name collision).

```riven
module std::hash
  pub trait Hasher
    def write(&mut self, bytes: &[UInt8])
    def write_u64(&mut self, n: UInt64)
    def finish(&self) -> UInt64
  end

  pub trait Hashable
    def hash[H: Hasher](&self, state: &mut H)
  end

  pub class DefaultHasher       # SipHash-1-3 or similar; seeded per-process
    pub def self.new -> DefaultHasher
    impl Hasher for DefaultHasher
  end

  pub class BuildHasher
    pub def self.new -> BuildHasher
    pub def build(&self) -> DefaultHasher
  end
end
```

This replaces the skeleton `Hashable` trait registered at `resolve/mod.rs:143`. Keys for `Hash[K, V]` and `Set[T]` must implement `Hashable + Eq`.

### 4.11 Method surfaces for built-in types (see §5)

`std::collections` re-exports `Vec`, `Hash`, `Set`, and also exposes:

```riven
pub class VecDeque[T]          # ring buffer — phase 1c
pub class BTreeMap[K, V]       # sorted map — phase 2
pub class BTreeSet[T]
```

For v1, only `Vec`, `Hash`, `Set` are required.

## 5. Method Surface for Built-in Types

This is the "one blend per method" table the project lead asked for. The "Ruby/Rust" columns show what each language calls a close analog; the "Riven" column is the canonical choice. Justifications are keyed to the core principles (P1–P5).

### 5.1 `Vec[T]`

| Method | Signature (Riven) | Rust analog | Ruby analog | Justification |
|---|---|---|---|---|
| `self.new` | `-> Vec[T]` | `Vec::new` | `Array.new` | P3 one obvious constructor |
| `self.with_capacity` | `(cap: USize) -> Vec[T]` | `Vec::with_capacity` | — | perf escape hatch |
| `push` | `(&mut self, item: T)` | `push` | `push` | identical name both sides |
| `pop` | `(&mut self) -> Option[T]` | `pop` | `pop` (returns `nil`) | Option is the Riven convention |
| `len` | `(&self) -> USize` | `len` | `length`/`size` | Rust wins — shorter; P3 |
| `is_empty` | `(&self) -> Bool` | `is_empty` | `empty?` | Rust name; `?` is reserved for try (tension 4) |
| `clear` | `(&mut self)` | `clear` | `clear` | same |
| `get` | `(&self, i: USize) -> Option[&T]` | `get` | `[]` (panics) | `.get` always safe; `[i]` panics (P1 loud danger) |
| `get_mut` | `(&mut self, i: USize) -> Option[&mut T]` | `get_mut` | — | mutability explicit |
| `first` / `last` | `(&self) -> Option[&T]` | `first`/`last` | `first`/`last` | same |
| `contains` | `(&self, x: &T) -> Bool` where `T: Eq` | `contains` | `include?` | Rust name |
| `iter` | `(&self) -> Iter[T]` | `iter` | `each` | `each` is the block form; `iter` returns a value |
| `iter_mut` | `(&mut self) -> IterMut[T]` | `iter_mut` | — | needed for ownership correctness |
| `into_iter` | `(self) -> IntoIter[T]` | `into_iter` | — | moves |
| `each` | `(&self, f: Fn(&T))` *or* `(&self) do \|x\| … end` | — | `each` | Ruby block form coexists with iter |
| `map` | `[U](&self, f: Fn(&T) -> U) -> Vec[U]` | `iter().map().collect()` | `map` | single call, no `.collect` (P3) |
| `filter` | `(&self, f: Fn(&T) -> Bool) -> Vec[T]` where `T: Clone` | `iter().filter().collect()` | `select` | Rust name; `filter` is clearer |
| `filter_map` | `[U](&self, f: Fn(&T) -> Option[U]) -> Vec[U]` | `filter_map` | — | ergonomic combinator |
| `find` | `(&self, f: Fn(&T) -> Bool) -> Option[&T]` | `find` | `find`/`detect` | Ruby convergence |
| `position` | `(&self, f: Fn(&T) -> Bool) -> Option[USize]` | `position` | `index` | Rust name |
| `any` | `(&self, f: Fn(&T) -> Bool) -> Bool` | `any` | `any?` | drop the `?` |
| `all` | `(&self, f: Fn(&T) -> Bool) -> Bool` | `all` | `all?` | same |
| `count` | `(&self) -> USize` | `count` | `count` | both agree |
| `fold` | `[B](&self, init: B, f: Fn(B, &T) -> B) -> B` | `fold` | `inject`/`reduce` | Rust name (tension 4: explicit over implicit) |
| `sum` | `(&self) -> T` where `T: Add` | `sum` | `sum` | same |
| `min` / `max` | `(&self) -> Option[&T]` where `T: Ord` | `min`/`max` | `min`/`max` | same |
| `sort` | `(&mut self)` where `T: Ord` | `sort` | `sort!` | we always mutate Vec in place, no copying variant |
| `sort_by` | `(&mut self, f: Fn(&T, &T) -> Ordering)` | `sort_by` | `sort` with block | same |
| `reverse` | `(&mut self)` | `reverse` | `reverse!` | in place |
| `join` | `(&self, sep: &str) -> String` where `T: Display` | `join` (on `&[&str]` only) | `join` | Ruby wins — works on any `Display` |
| `partition` | `(&self, f: Fn(&T) -> Bool) -> (Vec[T], Vec[T])` | `partition` | `partition` | same |
| `enumerate` | `(&self) -> Enumerate[Iter[T]]` | `enumerate` | `each_with_index` | Rust wins — shorter |
| `zip` | `[U](&self, other: &Vec[U]) -> Zip` | `zip` | `zip` | same |
| `chunks` | `(&self, n: USize) -> Chunks[T]` | `chunks` | `each_slice` | Rust wins |
| `extend` | `[I: IntoIterator[Item=T]](&mut self, xs: I)` | `extend` | `concat` | Rust name |
| `drain` | `(&mut self) -> Drain[T]` | `drain` | — | ownership-transfer iteration |
| `clone` | `(&self) -> Vec[T]` where `T: Clone` | `clone` | `dup` | Rust name |
| `to_vec` | `(&self) -> Vec[T]` where `T: Clone` | `to_vec` | — | needed for iterator pipelines (already in codegen) |

Indexing `v[i]` panics on OOB (the safe form is `.get`), matching Rust and tutorial 13. This is P1 — danger is loud and explicit via `[]`.

### 5.2 `Hash[K, V]`

Requires `K: Hashable + Eq`.

| Method | Signature | Justification |
|---|---|---|
| `self.new` | `-> Hash[K, V]` | ctor |
| `self.with_capacity` | `(cap: USize) -> Hash[K, V]` | perf |
| `insert` | `(&mut self, k: K, v: V) -> Option[V]` | returns displaced value |
| `get` | `(&self, k: &K) -> Option[&V]` | safe lookup |
| `get_mut` | `(&mut self, k: &K) -> Option[&mut V]` | mutable lookup |
| `remove` | `(&mut self, k: &K) -> Option[V]` | returns removed |
| `contains_key` | `(&self, k: &K) -> Bool` | tutorial 13 uses this spelling |
| `len` / `is_empty` | as `Vec` | |
| `clear` | `(&mut self)` | |
| `keys` | `(&self) -> Keys[K, V]` | iterator over &K |
| `values` | `(&self) -> Values[K, V]` | iterator over &V |
| `values_mut` | `(&mut self) -> ValuesMut[K, V]` | |
| `iter` | `(&self) -> Iter[K, V]` yielding `(&K, &V)` | |
| `each` | `(&self) do \|k, v\| … end` | Ruby block form |
| `entry` | `(&mut self, k: K) -> Entry[K, V]` | `or_insert` / `or_insert_with` API |
| `h[k]` | indexing panics if missing (tutorial 13) | P1 |

### 5.3 `Set[T]`

| Method | Signature |
|---|---|
| `self.new` | `-> Set[T]` |
| `insert` | `(&mut self, x: T) -> Bool` (true iff newly inserted) |
| `contains` | `(&self, x: &T) -> Bool` |
| `remove` | `(&mut self, x: &T) -> Bool` |
| `len` / `is_empty` / `clear` | |
| `iter` | `(&self) -> Iter[T]` |
| `each` | `(&self) do \|x\| … end` |
| `union` / `intersection` / `difference` / `symmetric_difference` | `(&self, other: &Set[T]) -> Iter[T]` |

### 5.4 `Option[T]`

| Method | Signature | Notes |
|---|---|---|
| `is_some` / `is_none` | `(&self) -> Bool` | |
| `unwrap!` | `(self) -> T` | `!` suffix signals danger (P1, tutorial 11) |
| `expect!` | `(self, msg: &str) -> T` | |
| `unwrap_or` | `(self, default: T) -> T` | |
| `unwrap_or_else` | `(self, f: Fn() -> T) -> T` | |
| `unwrap_or_default` | `(self) -> T` where `T: Default` | |
| `map` | `[U](self, f: Fn(T) -> U) -> Option[U]` | |
| `and_then` | `[U](self, f: Fn(T) -> Option[U]) -> Option[U]` | |
| `or` | `(self, other: Option[T]) -> Option[T]` | |
| `or_else` | `(self, f: Fn() -> Option[T]) -> Option[T]` | |
| `ok_or` | `[E](self, err: E) -> Result[T, E]` | |
| `ok_or_else` | `[E](self, f: Fn() -> E) -> Result[T, E]` | |
| `as_ref` | `(&self) -> Option[&T]` | |
| `as_mut` | `(&mut self) -> Option[&mut T]` | |
| `take` | `(&mut self) -> Option[T]` | leaves `None` behind |
| `replace` | `(&mut self, v: T) -> Option[T]` | |
| `filter` | `(self, f: Fn(&T) -> Bool) -> Option[T]` | |
| `try_op` | desugar target for `?` | already wired |

### 5.5 `Result[T, E]`

| Method | Signature |
|---|---|
| `is_ok` / `is_err` | `(&self) -> Bool` |
| `ok` / `err` | `(self) -> Option[T]` / `-> Option[E]` |
| `unwrap!` / `expect!` | as Option |
| `unwrap_err!` / `expect_err!` | |
| `unwrap_or` / `unwrap_or_else` / `unwrap_or_default` | |
| `map` | `[U](self, f: Fn(T) -> U) -> Result[U, E]` |
| `map_err` | `[F](self, f: Fn(E) -> F) -> Result[T, F]` |
| `and_then` | `[U](self, f: Fn(T) -> Result[U, E]) -> Result[U, E]` |
| `or_else` | `[F](self, f: Fn(E) -> Result[T, F]) -> Result[T, F]` |
| `as_ref` / `as_mut` | |
| `try_op` | `?` operator |

### 5.6 `String` / `&str`

`String` is owned, growable; `&str` is a borrowed slice. The asymmetry between `self` types mirrors Rust.

| Method | `String` | `&str` | Notes |
|---|---|---|---|
| `self.new` | `-> String` | — | empty |
| `self.with_capacity(cap)` | `-> String` | — | |
| `self.from(s: &str)` | `-> String` | — | |
| `len` | `(&self) -> USize` | same | byte length |
| `is_empty` | `(&self) -> Bool` | same | |
| `clear` | `(&mut self)` | — | |
| `push` | `(&mut self, c: Char)` | — | |
| `push_str` | `(&mut self, s: &str)` | — | |
| `pop` | `(&mut self) -> Option[Char]` | — | |
| `insert` | `(&mut self, i: USize, c: Char)` | — | byte index; panics if not on char boundary |
| `insert_str` | `(&mut self, i: USize, s: &str)` | — | |
| `remove` | `(&mut self, i: USize) -> Char` | — | |
| `truncate` | `(&mut self, n: USize)` | — | |
| `capacity` | `(&self) -> USize` | — | |
| `chars` | `(&self) -> Chars` | same | iterator over `Char` — UTF-8 decode (v1 may fall back to byte iteration; see §9) |
| `bytes` | `(&self) -> Bytes` | same | iterator over `UInt8` |
| `as_bytes` | `(&self) -> &[UInt8]` | same | |
| `as_str` | `(&self) -> &str` | same | |
| `to_string` | `(&self) -> String` | `(&self) -> String` | trait `Display` |
| `to_lower` / `to_upper` | `(&self) -> String` | same | |
| `trim` / `trim_start` / `trim_end` | `(&self) -> &str` | same | returns slice |
| `starts_with` / `ends_with` | `(&self, p: &str) -> Bool` | same | |
| `contains` | `(&self, p: &str) -> Bool` | same | |
| `find` / `rfind` | `(&self, p: &str) -> Option[USize]` | same | byte offset |
| `replace` | `(&self, from: &str, to: &str) -> String` | same | |
| `split` | `(&self, sep: &str) -> Split` | same | already wired; returns iterator of `&str` |
| `split_whitespace` | `(&self) -> SplitWhitespace` | same | |
| `splitn` | `(&self, n: USize, sep: &str) -> SplitN` | same | |
| `lines` | `(&self) -> Lines` | same | |
| `repeat` | `(&self, n: USize) -> String` | same | |
| `parse[T]` | `(&self) -> Result[T, ParseError]` where `T: FromStr` | same | replaces current `parse_uint`/`parse_int` |
| `char_count` | `(&self) -> USize` | same | tutorial 13 |
| `clone` | `(&self) -> String` | — | |

Operators: `s1 + s2` (consumes `s1`, borrows `s2`), `s1 == s2`, `<`, `>`, etc.

### 5.7 Primitive numeric methods

Minimum surface for v1:

```riven
impl Int
  pub def self.MIN / MAX                         # constants
  pub def abs / pow / saturating_add / checked_add / wrapping_add
  pub def to_string -> String
  pub def to_string_radix(r: UInt32) -> String
end

impl Float
  pub def self.INFINITY / NAN / EPSILON
  pub def abs / sqrt / sin / cos / tan / ln / log2 / log10 / exp
  pub def floor / ceil / round / trunc / fract
  pub def is_nan / is_infinite / is_finite
  pub def to_string -> String
end
```

Backed by `libm` (already linked via `-lm` in `codegen/object.rs:70`).

### 5.8 Iterator trait (`std::iter`)

```riven
pub trait Iterator
  type Item
  def mut next(&mut self) -> Option[Self.Item]

  # Default methods — all of §5.1's iterator combinators live here
  def map[B](self, f: Fn(Self.Item) -> B) -> Map[Self, B]   where Self: Sized
  def filter(self, f: Fn(&Self.Item) -> Bool) -> Filter[Self] where Self: Sized
  def take(self, n: USize) -> Take[Self]
  def skip(self, n: USize) -> Skip[Self]
  def chain[I: Iterator[Item=Self.Item]](self, other: I) -> Chain[Self, I]
  def enumerate(self) -> Enumerate[Self]
  def zip[I: Iterator](self, other: I) -> Zip[Self, I]
  def collect[B: FromIterator[Self.Item]](self) -> B
  def fold[B](self, init: B, f: Fn(B, Self.Item) -> B) -> B
  def count(self) -> USize
  def sum(self) -> Self.Item where Self.Item: Add
  def min / max (self) -> Option[Self.Item] where Self.Item: Ord
  def find(self, f: Fn(&Self.Item) -> Bool) -> Option[Self.Item]
  def position(self, f: Fn(&Self.Item) -> Bool) -> Option[USize]
  def any / all (self, f: Fn(Self.Item) -> Bool) -> Bool
  def for_each(self, f: Fn(Self.Item))       # Rust-style non-block
  def each(self) do |x| … end                 # Ruby-style block form
  def to_vec(self) -> Vec[Self.Item]
end

pub trait IntoIterator
  type Item
  type IntoIter: Iterator[Item=Self.Item]
  def consume into_iter(self) -> Self.IntoIter
end

pub trait FromIterator[A]
  def self.from_iter[I: IntoIterator[Item=A]](iter: I) -> Self
end
```

With this trait in place, `Vec`'s map/filter/find stop being runtime-stubbed — they return iterator adapters that only materialize when `.to_vec`/`.collect` is called. This is the single largest change from the current state.

## 6. Module System Design

### 6.1 Syntax

Riven already parses `use Foo.Bar`, `use Foo.Bar as B`, `use Foo.Bar.{X, Y}` (see §2.3 and `parser/ast.rs:692-704`). Stdlib reuses this machinery with a distinguished root name `std`:

```riven
use std::io::{read_line, stdin}
use std::fs
use std::collections::Hash as HashMap         # rename (see §9)
use std::time::{Instant, Duration}
```

Riven uses `.` as the path separator today (`Http.Client`). Rust and the stdlib design naturally want `::` (e.g. `std::io::stdin`). **Decision**: accept **both** `.` and `::` in `use` paths; the lexer already supports `.` and we add `::` as an alternative path separator token. `.` remains the canonical form in diagnostics and documentation. This resolves the tension between tutorial 10 (`use Http.Request`) and the standard form `std::io`.

### 6.2 How stdlib is exposed to the resolver

Three layers, from cheapest to most general:

1. **Prelude (compiler-blessed).** `Resolver::register_builtins` (at `crates/riven-core/src/resolve/mod.rs:97`) grows to register the prelude types/traits/functions/macros. These are in every scope without a `use`.
2. **`std` root module (compiler-blessed).** We add a synthetic `DefKind::Module { items: [..] }` registered under the name `std` with children `io`, `fs`, `net`, `time`, `env`, `process`, `fmt`, `path`, `hash`, `collections`, `iter`, `mem`. Each child is itself a `DefKind::Module`. Users write `use std::io` or `use std::io::read_line` and the existing `resolve_use_decl` walker handles the rest.
3. **User-visible source.** Items that must have source bodies (e.g. `std::fs::read_to_string`) are *preferred* to live in `.rvn` files shipped in the release tarball, discovered via `find_runtime_c`-style search. Items that are zero-Riven-wrapping-FFI-only (e.g. the raw `fopen` binding) live in a hidden `std::ffi` module, *not* in the user surface.

### 6.3 `core` vs `std` split (deferred)

The Rust split `core`/`alloc`/`std` is attractive but premature for v1. We pre-adapt by:

- Organizing source so that `Vec`, `Hash`, `Set`, heap-allocating string ops, and anything touching `malloc` live under `std::`. Everything else (primitive methods, `Option`, `Result`, `fmt` traits, iterators as pure traits, `mem::size_of`) lives under a hidden sub-module `std::core::*` that gets re-exported. A future `core` crate is a `mv` + cargo feature flag away.
- Enforcing that `std::core` files include no FFI declarations and no `malloc`-dependent calls.

### 6.4 File layout of the stdlib source tree

```
crates/riven-std/
  Cargo.toml                       # bookkeeping only — not compiled by cargo
  src/
    lib.rvn                        # re-exports
    prelude.rvn
    io.rvn
    io/
      stdin.rvn
      stdout.rvn
    fs.rvn
    fs/file.rvn
    net.rvn
    net/tcp.rvn
    time.rvn
    env.rvn
    process.rvn
    fmt.rvn
    fmt/formatter.rvn
    path.rvn
    hash.rvn
    collections.rvn
    collections/vec.rvn
    collections/hash_map.rvn
    collections/hash_set.rvn
    iter.rvn
    mem.rvn
    ffi/                           # hidden, not in prelude
      posix.rvn
      clock.rvn
```

Rationale: keeping the stdlib in its own crate directory (but *not* compiled by cargo) lets us ship it as a discrete deliverable in the release tarball at `~/.riven/share/riven/std/` and lets the compiler find it the same way it finds `runtime.c` today.

### 6.5 Name resolution for `std::*`

At the top of `Resolver::register_builtins` we call a new `register_std()` that parses the stdlib sources (read from the search path below), runs them through the full resolve pass, and merges the resulting `SymbolTable` into the compiler's registry under a root `DefId` named `std`. The search path mirrors `find_runtime_c` (`codegen/mod.rs:27-65`):

1. `$RIVEN_STD` env var (overrides everything).
2. `<exe>/../lib/riven/std/` (installed layout).
3. `<exe>/../share/riven/std/` (alternate).
4. `$CARGO_MANIFEST_DIR/../riven-std/src/` (dev fallback).

Failure to find the stdlib is a hard compile error unless the user passes `rivenc --no-std` (see §7.7).

## 7. Implementation Strategy

### 7.1 Decision: compiler-blessed vs written-in-Riven vs FFI-wrappers

One blend per module (P3). The table below pins the call for each v1 module.

| Module | Written in | Rationale |
|---|---|---|
| `std::prelude` (re-export list) | Compiler-registered in `register_builtins` | it's just a name table |
| `std::io` surface | Riven; bodies call `extern "C"` libc | already the FFI pattern (tutorial 14) |
| `std::fs` | Riven + `extern "C"` (`fopen`, `fread`, `fclose`, `unlink`, `mkdir`, `rename`, `stat`) | same |
| `std::net` | Riven + `extern "C"` (`socket`, `bind`, `listen`, `accept`, `connect`, `send`, `recv`, `close`) | same |
| `std::time` | Riven + `extern "C"` (`clock_gettime`) | same |
| `std::env` | Riven + `extern "C"` (`getenv`, `setenv`, `unsetenv`, `environ`) + compiler-emitted `argv` | argv needs main-shim support |
| `std::process` | Riven + `extern "C"` (`fork`, `execvp`, `waitpid`, `_exit`, `pipe`) | |
| `std::fmt` traits | Riven | plain trait defs |
| `std::fmt` format macros | Compiler | hygienic expansion (tension 5) — must know about types at call site |
| `std::path` | Riven (thin wrapper over `&str`) | no C needed |
| `std::hash` trait | Riven | plain trait |
| `std::hash::DefaultHasher` | Riven (SipHash impl) or `extern "C"` wrapping a bundled C SipHash | perf; either works |
| `Vec[T]`, `Hash[K,V]`, `Set[T]` runtime | C — new functions in `runtime.c` | keeps the element-type-erased convention |
| Iterator combinators (`Map`, `Filter`, `Zip`, …) | Riven | they are zero-cost when monomorphized (tension 6) |

**Why we keep collections in C**: generic monomorphization in Riven is not yet load-bearing (the typechecker erases to `i64`-slot, see §2.1), so we continue the pattern runtime.c uses: callers pass 64-bit slots. A real monomorphized path is follow-up work and is sketched in §10.

### 7.2 Runtime growth (`runtime.c`)

Phase 1a adds ~18 functions:

```
riven_vec_pop, riven_vec_clear, riven_vec_first, riven_vec_last
riven_vec_remove, riven_vec_insert, riven_vec_extend_from_ptr
riven_vec_sort_i64 (initial, scalar only), riven_vec_reverse
riven_hash_new, riven_hash_insert, riven_hash_get, riven_hash_remove
riven_hash_contains, riven_hash_len, riven_hash_clear
riven_set_new, riven_set_insert, riven_set_contains, riven_set_remove
riven_string_push, riven_string_push_char
riven_string_to_upper, riven_string_replace, riven_string_starts_with
riven_string_ends_with, riven_string_contains, riven_string_find
riven_str_parse_int (signed, replaces parse_uint)
riven_panic_with_location (file, line, col)
```

Phase 1b adds ~12 I/O functions:

```
riven_file_open, riven_file_close, riven_file_read, riven_file_write
riven_fs_read_to_string, riven_fs_write, riven_fs_exists
riven_fs_remove_file, riven_fs_create_dir, riven_fs_rename
riven_env_var, riven_env_args_count, riven_env_args_at
riven_process_exit, riven_clock_gettime_monotonic
```

All new functions follow the existing convention:

- Return `void*` for heap values, `int64_t` for integers/booleans.
- Errors are encoded as a tagged union (same layout as `Option` / `Result` — see `runtime.c:283-309`): `[tag: i32][pad: i32][payload: i64]`.
- No direct memory transfer of struct-by-value across the boundary — always a heap pointer.

Alternative considered: replacing `runtime.c` with a `libriven_std.a` Rust crate. **Rejected** for v1: it doubles the release artifacts, requires the user to have a Rust toolchain to build from source, and the existing pattern (a single C TU compiled at link time) already works. The codegen's `object::compile_runtime` (`codegen/object.rs:10`) would need to be generalized to list-of-translation-units either way. Revisit in v2.

### 7.3 Typechecker deltas (`typeck/infer.rs`, `resolve/mod.rs`)

- `builtin_method_type` (lines 813-928) must grow to cover the full §5 tables. This will roughly triple its size; consider extracting to a declarative table (`src/typeck/stdlib_methods.rs`) keyed on `(ty_pattern, method_name)` → `fn(&mut Ctx) -> Ty`.
- `register_builtins` must register `std` as a module root, register traits with full method signatures (not just names), and register prelude macros (`println!`, `format!`, …) — the macros need a new `DefKind::Macro` variant plus support in `parser/expr.rs`'s macro call path (currently at line 294-302).
- Iterator default methods blow up the constraint solver unless we're careful: each `.map().filter().to_vec()` is a chain of monomorphized generic calls. Recommendation: implement the trait's default methods as Riven source (so they get lowered to MIR normally) rather than hard-coding their return types in `builtin_method_type`. This means we do *not* expand `builtin_method_type` to 300+ lines; instead, it keeps the existing ~100 lines for the things that must be compiler-known (`String.split`, `Vec[T].iter` returning a specific opaque iterator type) and the rest is resolved via the generic method lookup already in place at `infer.rs:800`.

### 7.4 Format macros (compiler-side)

`println!`, `format!`, `eprintln!`, `print!`, `eprint!`, `dbg!`, `panic!` are hygienic compile-time macros (tension 5, see `decision_tensions.md`). Expansion lives in `crates/riven-core/src/parser/macros.rs` (new file) and runs at *parse* time (not desugar-in-resolve), so that the expanded `HirExpr` flows through type checking normally. Each format call expands to:

```riven
# println!("hello {}, age {}", name, age)
# becomes:
{
  let __buf = String.new
  Display.fmt(&name, &mut Formatter.for(&mut __buf)).unwrap!
  __buf.push_str(", age ")
  Display.fmt(&age, &mut Formatter.for(&mut __buf)).unwrap!
  __buf.push('\n')
  std::io::stdout().write_str(&__buf).unwrap!
}
```

This allows the existing borrow checker and type checker to work unchanged. Alternative: build a single `vformat` runtime call with a compile-time type-tag array — rejected because it hides errors from the borrow checker and multiplies codegen complexity.

Each macro call site becomes a `HirExprKind::Block` after expansion, with `HirExprKind::MacroCall` kept only as a fallback for `vec!`/`hash!`/`set!` literal macros which already have a lowering path in `mir/lower.rs:1503`.

### 7.5 Threading through the pipeline

Walk order (lexer is unchanged):

1. **Parser** (`parser/expr.rs`): recognize `{:x}`, `{:?}`, `{:>10.3}` inside format strings. Recognize `::` as path separator in `use` decls.
2. **Macro expander** (new): run after parse, before resolve. Expands format macros to HIR-style `Block` exprs (still in AST).
3. **Resolver** (`resolve/mod.rs`): `register_std()` loads stdlib `.rvn` sources from the search path, runs a mini resolve pass, and merges their `SymbolTable` into the main one. Also registers all new DefIds for stdlib types/traits/fns.
4. **Typechecker** (`typeck/infer.rs`): new `builtin_method_type` entries + trait default methods now resolve via normal nominal impl lookup (`traits.rs:136-180`), not hard-coded returns.
5. **Borrow checker** (unchanged): new types follow the existing Move/Copy rules (`hir/types.rs:184-235`). `File`, `Stdin`, `TcpStream` are Move; `Path` is borrow; `Duration`, `Instant` are Copy.
6. **MIR lowerer** (`mir/lower.rs`): add `hash!` / `set!` macro lowering next to the existing `vec!` case at line 1502. Format-macro expansion is already handled at step 2 so no lowerer change.
7. **Codegen** (`codegen/runtime.rs`): extend `runtime_name()` with the new mangled names. The `?T...` fallback block (lines 131-167) can shrink once real trait dispatch lands.

### 7.6 Program entry shim

For `std::env::args` to work, the compiler's emitted `main` must capture `argc`/`argv`. Today `codegen/cranelift.rs` emits a plain `main`. Change: emit

```c
int main(int argc, char **argv) {
    riven_env_init(argc, argv);
    riven_user_main();
    return 0;
}
```

where `riven_user_main` is the user's `def main`. `riven_env_init` stashes argv in a static and exposes it via `riven_env_args_count` / `riven_env_args_at` used by `std::env`. This is ~20 lines of C in `runtime.c` and a 3-line tweak in `cranelift.rs` and `llvm/emit.rs`.

### 7.7 `--no-std` and the REPL

Add `rivenc --no-std` and a `[package] no-std = true` manifest key. When set:

- `register_std()` is skipped.
- Prelude is reduced to: primitive types, `Option`, `Result`, `Vec`, traits `Copy`/`Clone`/`Drop`/`Sized`, macros `panic!`/`assert!`. No `println!`, no `io`, no `fmt`.
- This pre-plans the `core` vs `std` split in §6.3 without committing to it.

For the REPL (`crates/riven-repl`, phase 12 per memory), stdlib loads lazily on first use.

### 7.8 Distribution

Update `install.sh` (already at lines 161-168) to install `share/riven/std/` from the tarball. Update the release workflow (`project_release_setup.md`) to bundle `crates/riven-std/src/` into the tarball as `share/riven/std/`. Update `codegen/mod.rs::find_runtime_c` pattern with a `find_std_root` sibling.

## 8. Phasing

### Phase 1a — "make what exists real" (2–3 weeks)

- Real bodies for `Vec.map`, `.filter`, `.find`, `.position`, `.each`, `.partition`, `.enumerate` via iterator trait.
- `Hash[K,V]` and `Set[T]` end-to-end (runtime.c + typechecker + mangled names).
- `Iterator` trait in `std::iter` with default methods; `IntoIterator`, `FromIterator`.
- `String` full surface (§5.6).
- `hash!{…}` and `set!{…}` macros in `mir/lower.rs`.
- `println!`, `eprintln!`, `print!`, `eprint!`, `format!`, `dbg!`, `panic!`, `assert!`, `assert_eq!` format macros.
- `Display` / `Debug` traits with blanket impls for primitives; `derive Debug` on structs/classes.
- Prelude registration.
- Delete `riven_noop_passthrough`, `riven_noop_return_null` (they become unreferenced once real dispatch lands).

Exit: `tutorial/13-collections.md` examples compile and run. `sample_program.rvn` (which uses `.partition`, `.iter`, `.map`, `.unwrap_or_else`, `{"closure"}`, `.filter`) runs with observable output.

### Phase 1b — "I/O that matters" (2 weeks)

- `std::io` (stdin/stdout/stderr + IoError).
- `std::fs` (file read/write/open + metadata).
- `std::env` (args, var, vars, current_dir) with argv shim in main.
- `std::process` (exit, Command, spawn, status, output).
- FFI hidden module `std::ffi::posix` with raw `extern "C"` bindings to libc.

Exit: `File.read_string(path)?` (tutorial 7/11), `read_line()` (tutorial 5), `env::args()` all work.

### Phase 1c — "time, paths, hashing, network" (2 weeks)

- `std::time` (Instant, Duration, SystemTime) + `clock_gettime` binding.
- `std::path` (Path, PathBuf) + integration with `fs` APIs.
- `std::hash` (Hasher trait, SipHash impl, BuildHasher) — replaces the placeholder `Hashable` trait registered at `resolve/mod.rs:143`.
- `std::net` (TcpStream, TcpListener) + `socket`/`connect`/`listen`/`accept` bindings.
- `std::fmt` polish: width/precision/alignment/radix specifiers.

Exit: a minimal HTTP client demo compiles and runs (`TcpStream.connect` + `write` + `read_to_string`).

### Phase 2 (out of v1 scope, enumerated for context)

- `VecDeque`, `BTreeMap`, `BTreeSet`, `LinkedList`.
- Proper UTF-8 `char` handling in `String.chars`.
- `std::sync`, `std::thread` (sibling doc).
- `std::rand`.
- `core` vs `std` split.
- Windows `Path` (WTF-8).
- A `libriven_std.a` Rust-side option to replace the C runtime growth strategy.

## 9. Open Questions

1. **`Hash` name collision.** The type is `Hash[K,V]`; the hash-trait is also conventionally called `Hash`. Options:
   - a) Rename the collection type to `HashMap[K,V]` (Rust/Java convention). Keep `Hash` as the trait name. Breaks tutorial 13 and every existing example. **Recommended.**
   - b) Keep `Hash[K,V]` as the collection; name the trait `Hashable` (already the placeholder name in `resolve/mod.rs:143`).
   - c) Scope them: collection is `Hash[K,V]` (a type); trait is `std::hash::Hash` (a name at a path). Plausible but will confuse users.
2. **`Char` width.** `hir/types.rs:57` has `Ty::Char` but the lexer/string code treats strings as byte arrays. For `String.chars`: emit `Char` as `u32` and UTF-8 decode in `riven_string_chars_next`? Or keep `Char` as ASCII byte and defer? **Recommend**: `Char` stays 32-bit Unicode scalar; `String.chars` decodes; non-ASCII strings that come via FFI get validated lazily.
3. **`UInt128`.** `Duration.as_nanos` wants a 128-bit result. Riven does not model `Int128`/`UInt128` yet (`hir/types.rs:40-56`). Options: add it; return `Result[UInt64, OverflowError]`; return `UInt64` with saturating semantics. **Recommend**: saturating `UInt64` in v1, add `UInt128` in phase 2.
4. **Operator overloading for arithmetic traits (`Add`, `Sub`, `Mul`, …).** Needed for `Duration + Duration`. Riven parses `a + b` via `BinOp::Add` in `parser/ast.rs:367`. The typechecker today only accepts numeric operands. Do we (a) hard-code Duration arithmetic in the typechecker (ugly) or (b) add operator-overloading via trait lookup (principled but larger scope)? **Recommend (b)**, track as a dependency of `std::time`.
5. **`read_line` in prelude vs `std::io::read_line`.** Tutorial 5 uses bare `read_line()`. Either the prelude exports it (breaking the namespace discipline of P5) or we update the tutorial. **Recommend**: put it in `std::io` and update tutorial 5; it's one-line addition of `use std::io::read_line` in that example.
6. **Default string type for literals.** `"foo"` is `&str` today (tutorial 2). `String.new("foo")` and `String.from("foo")` both exist in the sample. Pick one? **Recommend**: `String.from` is canonical; `String.new` with no args is empty string; `String.new(&str)` is removed (it's redundant with `from`).
7. **Stdlib compiled or interpreted?** Does the compiler re-parse `share/riven/std/*.rvn` every invocation, or do we cache the resolved `SymbolTable` to a `.rlib`-style blob? Phase 13 already has content-addressed caching in `rivenc`. Reuse that. **Recommend**: cache on first use, invalidate on compiler version bump.
8. **`panic!` macro location info.** Needs file/line/col at the call site. Riven already plumbs `Span` through to HIR (`lexer/token.rs`). We need `runtime.c::riven_panic_with_location` — trivial to add.

## 10. Risks

1. **Iterator trait + monomorphization is a load-bearing change.** The compiler today sidesteps generics by falling through to `riven_noop_passthrough` for `?T...` methods (`codegen/runtime.rs:131-167`). Implementing real iterator combinators *forces* us to commit to a concrete monomorphization strategy. If monomorphization slips, phase 1a slips. Mitigation: ship phase 1a with a hybrid — `Vec.map`/`.filter` work as direct methods that allocate; `.iter().map().filter().collect()` is deferred to phase 1a.5. This keeps the tutorial examples honest without blocking on generics.
2. **`Hash[K,V]` generic keys in a non-generic runtime.** The runtime is element-type-erased (`int64_t` slots). Hash keys need *equality* and *hashing* of user-defined structs; 64-bit slot erasure makes that hard. Mitigation: in v1, restrict `Hash[K, V]` keys to `{Int, UInt, USize, String, &str}`. Generalize in phase 2 when we have real monomorphization.
3. **FFI calling conventions on aggregates.** `stat`, `addrinfo`, `timespec` pass structs by value/pointer and vary across libc / musl / glibc / macOS. Mitigation: in v1, do not expose `stat` struct directly; `fs::metadata` calls `stat` inside C and returns a flat `Metadata` struct with ABI-stable types.
4. **`argv` ownership.** Turning `char **argv` into `Vec[String]` means copying — argv strings live until process exit, but our `Vec[String]` owns its elements and frees them on drop, which would `free()` memory we don't own. Mitigation: copy argv into heap strings at `riven_env_init` time.
5. **The `?T...` codegen fallback masks real bugs.** `runtime.c`'s `riven_noop_passthrough` makes miscompiled code *run without error*. Once real dispatch lands, some currently-passing tests may start failing because the noop hid a type-resolution bug. Mitigation: in phase 1a, add a `rivenc --strict-dispatch` flag that turns all `?T...` → `riven_noop_passthrough` lookups into hard errors, and run the full test suite with it on.
6. **Scope creep.** The obvious temptation is to ship `BTreeMap`, async, UTF-8, rand, thread, and sync all at once. Mitigation: this doc is explicit about v1 vs phase 2, and the sibling concurrency doc owns thread/sync.
7. **Tutorial drift.** The tutorial already promises `Hash.new` (not `HashMap.new`) and `hash!{}`. If we rename to `HashMap`, the tutorial needs a sweep. Mitigation: if decision #1 above is (a), track the tutorial update as a blocking subtask of phase 1a.
8. **Macro hygiene / identifier capture.** `println!("{}", x)` expanding to `let __buf = ...` risks name collision with a user variable named `__buf`. Mitigation: use gensym'd names with a reserved prefix (`__rvn_fmt_N`) that the lexer rejects at the user surface.
9. **Sanitizer builds.** `object.rs:10-42` compiles `runtime.c` with `-fsanitize=address,undefined` under `--sanitize`. New stdlib C code must be ASan-clean; expect several rounds of fixing leaks and UB in the first implementation pass.
