# Tier 1.01 — Standard Library (v1)

## 1. Summary & Motivation

Today, Ruxen programs can only call a handful of built-in functions (`puts`, `print`, `eputs`) plus method-level stubs on `String`, `Array`, `Option`, and `Result` that are hard-coded in the typechecker and routed to `runtime.c` by a name-mangling table (`crates/ruxen-core/src/codegen/runtime.rs`). There is no `io`, no `fs`, no `env`, no `process`, no `time`, no `net`, no `path`, no `fmt`, no Iterator mixin; `Array.map` / `Array.filter` are compiled to `ruxen_noop_passthrough`. This document specifies the v1 standard library: the set of modules, types, mixins, and method surfaces a Ruxen program can rely on; how those surfaces are exposed through the module system; and how the compiler, runtime, and toolchain must change to deliver them. The goal is an honest-to-goodness "batteries-included" core that lets a user build a CLI tool, read a config file, make an HTTP-style byte-stream request, and format structured data without reaching for FFI — while still composing cleanly with Ruxen's ownership model (P4) and single-obvious-path philosophy (P3).

## 2. Current State

This section inventories what exists *now* in the repo, so that the implementation plan in §7 can point at concrete deltas.

### 2.1 C runtime (`crates/ruxen-core/runtime/runtime.c`, 426 lines)

All extant stdlib surface resolves to one of ~25 C functions in a single file:

| Area | Functions | Lines |
|---|---|---|
| Printing | `ruxen_puts`, `ruxen_print`, `ruxen_eputs`, `ruxen_print_int`, `ruxen_print_float` | 29-55 |
| To-string conversions | `ruxen_int_to_string`, `ruxen_float_to_string`, `ruxen_bool_to_string` | 59-92 |
| String ops | `ruxen_string_eq`, `ruxen_string_cmp`, `ruxen_string_concat`, `ruxen_string_from`, `ruxen_string_len`, `ruxen_string_is_empty`, `ruxen_string_push_str`, `ruxen_string_trim`, `ruxen_string_to_lower` | 98-216 |
| Memory | `ruxen_alloc`, `ruxen_dealloc`, `ruxen_realloc` | 144-163 |
| Array | `ruxen_vec_new`, `ruxen_vec_push`, `ruxen_vec_len`, `ruxen_vec_get`, `ruxen_vec_get_mut`, `ruxen_vec_get_opt`, `ruxen_vec_get_mut_opt`, `ruxen_vec_is_empty`, `ruxen_vec_each` | 221-322 |
| `&str` | `ruxen_str_split`, `ruxen_str_parse_uint` | 326-372 |
| Option/Result | `ruxen_option_unwrap_or`, `ruxen_result_unwrap_or_else`, `ruxen_result_try_op` | 377-405 |
| Fallbacks | `ruxen_noop_passthrough`, `ruxen_noop_return_null`, `ruxen_noop` | 410-419 |
| Panic | `ruxen_panic` | 423-426 |

Critical limitations:

- `Array` is hard-coded to hold `int64_t` elements (line 220-225). No element-type genericity in the runtime — the generated code always passes a 64-bit slot, so collections of `String`, tuples, or user classes are *already* working by accident because the box pointer fits in 64 bits. Collections of structs larger than 64 bits will misbehave silently.
- `Map` and `Set` are declared in the type system (`Ty::Map`, `Ty::Set` in `crates/ruxen-core/src/hir/types.rs:77-79`) but have **zero** runtime functions and **zero** typechecker method entries.
- `Array.map`, `Array.filter`, `Array.find`, `Array.position`, `Array.partition`, `Option.map`, `Result.map_err`, `Result.ok_or`, all iterator chain methods → compile to `ruxen_noop_passthrough` or `ruxen_noop_return_null`. See `crates/ruxen-core/src/codegen/runtime.rs:86-167`. They typecheck but do nothing at runtime.
- `String.to_upper` is declared in `builtin_method_type` (`crates/ruxen-core/src/typeck/infer.rs:822`) but has no C implementation — it would link against `String_to_upper` which does not exist.
- No `FILE*`, no `fopen`, no sockets, no clock, no env, no `argv`.
- No `panic!`, `println!`, `eprintln!`, `format!`, `dbg!` macros. Bare `[…]` array literals lower to MIR via the existing path (`crates/ruxen-core/src/mir/lower.rs:1502-1535`); bare `{ k => v, … }` map literals are documented (tutorial 13) but unimplemented.

### 2.2 Type system (`crates/ruxen-core/src/hir/types.rs`)

The internal `Ty` enum already knows about all the prelude type forms. Today its variants for sequence and key-value collections still use Rust-era names (`Vec`, `Hash`, `HashMap` in the comments at lines 75-79); those internal names should be renamed to match the user-facing `Array`/`Map`/`Set` rename — flagged separately. New types like `Path`, `PathBuf`, `Instant`, `Duration`, `SystemTime`, `File` are modeled as `Ty::Class { name, .. }` (the same mechanism that holds `SplitIter`, `ArrayIter`, `ArrayIntoIter` today at `infer.rs:823, 842, 843`).

### 2.3 Resolver / builtin registration (`crates/ruxen-core/src/resolve/mod.rs`)

`Resolver::register_builtins` (lines 97-343) registers:

- 18 primitive type aliases (`Int`, `Int8`, ..., `Float`, `Bool`, `Char`, `String`) at 99-118.
- 10 built-in mixins (`Display`, `Error`, `Ord`, `Hashable`, `Iterator`, `Iterator`, `FromIterator`, `Copy`, `Clone`, `Debug`, `Drop`) at 139-151. All of them have zero generic params in their internal info record and only list required method names as strings — the method signatures are not registered, which is why the typechecker falls back to structural matching in `traits.rs:106-125`.
- 3 built-in top-level functions (`puts`, `eputs`, `print`) at 173-195.
- 4 built-in type constructors (`Array`, `Map`, `Set`, `String`) at 198-215, registered as `DefKind::Variable` so that `Array.new` resolves.
- `Option`/`Result` enums with `Some`/`nil`/`Ok`/`Err` variants (221-325).
- A `super` shim (328-342).

The registry uses string-keyed lookups in a string-to-`DefId` table (`type_registry`, line 37). This is the point at which a new module like `io` or `fs` would be injected.

Use-decls are resolved by `resolve_use_decl` (line 1180-1238) and already support `use Foo.Bar` (Simple), `use Foo.Bar as B` (Alias), `use Foo.Bar.{X, Y}` (Group). The walker handles `Module`, `Enum`, `Class` as namespaces (1263-1319). **There is no `package` or `std` root yet** — the first segment is looked up in the current scope or type registry.

### 2.4 Typechecker method surface (`crates/ruxen-core/src/typeck/infer.rs`)

`builtin_method_type` (lines 813-928) is a giant `match` that hard-codes the return type of every built-in method. Additions to the stdlib must land here.

### 2.5 Codegen name mangling (`crates/ruxen-core/src/codegen/runtime.rs`)

`runtime_name()` maps mangled Ruxen method names (`Array[T]_push`, `String_trim`, `Option[T]_unwrap_or`, …) to C symbols. It has a large fallback block (lines 131-167) that maps unknown `?T…` (unresolved inference-variable) methods to best-effort runtime calls — this is how generic methods limp through today. The list `RUNTIME_FUNCTIONS` (lines 11-26) is out of date: it only names 14 of the ~25 functions actually in `runtime.c`.

### 2.6 Module discovery & linking (`crates/ruxen-cli/src/`)

- `module_discovery.rs` walks `src/**.rx` and turns file paths into `UpperCamelCase` dotted module paths (e.g. `src/http/client.rx` → `Http.Client`). This is how user modules get loaded.
- `build.rs` `gather_sources` (line 356-388) concatenates the entry file and all modules into one big source string. The compiler does not yet link separately-compiled modules in the main project — *.rlib loading exists for dependencies but not intra-project.
- `codegen/mod.rs` `find_runtime_c` (line 27-65) searches for `runtime.c` at `$RUXEN_RUNTIME`, `<exe>/../lib/runtime.c`, `<exe>/../share/ruxen/runtime.c`, or the workspace dev path. This is the hook for shipping additional runtime source.
- `install.sh` already copies `lib/`, `share/`, `include/` from the release tarball into `~/.ruxen/` (lines 161-168).

### 2.7 Documentation surface

The tutorial (`docs/tutorial/`) *already* writes code that calls methods not yet implemented: `File.read_string(path)?` (tutorial 11, 7), `input.trim.parse_int` (tutorial 11), `read_line()` (tutorial 5), `{ k => v, … }` map literals (tutorial 13), `h.insert`, `h.contains_key`, `s.insert`, `s.contains`, `greeting.chars`, `greeting.char_count` (tutorial 13). These are *aspirational* and frame what v1 must deliver.

## 3. Goals & Non-Goals

### Goals

1. A discoverable, documented stdlib surface so the tutorial examples compile and run.
2. `Map` and `Set` work end-to-end (type-check, borrow-check, codegen, run).
3. `Array.map`, `Array.filter`, `Array.each`, `Array.find`, `Option.map`, `Result.map` stop being no-ops.
4. A module system (`use std.io`) that scales to 10-15 modules without ad-hoc additions to `resolve/mod.rs::register_builtins`.
5. Formatting: `println!`, `eprintln!`, `format!`, `dbg!`, `panic!` as first-class compiler macros; `Debug`/`Display` mixins enforceable.
6. Build-time and link-time story: stdlib ships in the release tarball, is located the same way `runtime.c` is, and does not require the user's build to download anything.
7. FFI paths for `fs`, `env`, `process`, `time` use the `lib "<libname>" ... end` mechanism (see tutorial 14) through vetted wrappers — so `std.fs.read_to_string` is a Ruxen function that calls `fopen`/`fread`/`fclose` under the hood.
8. The stdlib is organized so `no_std`-style subsets are possible later (P3: one path, but the path need not be all-or-nothing).

### Non-Goals

- **Concurrency primitives** (`thread`, `sync`, channels, mutex, atomics). These live in a sibling doc — `tier1_02_concurrency.md` will cover them.
- **Randomness.** `rand` is a separate concurrency-adjacent concern; out of scope here.
- **Unicode beyond ASCII-correct byte semantics.** `String.chars` enumerates `u8` today; proper UTF-8 decoding is explicitly deferred.
- **Async I/O.** All `io`/`fs`/`net` calls in v1 are blocking.
- **A registry for third-party packages.** The existing `ruxen add` plumbing (git / path / version) is sufficient.
- **A `core` package separation.** We leave room for it (§6.3) but v1 ships a single `std` prelude.

## 4. Scope — Modules, Types, Functions

Every module below is a Ruxen module under `std`. Unless stated otherwise, items are public (sectionless top-level items are public by default). Names follow the conventions in `docs/tutorial/16`: `snake_case` for functions, `UpperCamelCase` for types, `SCREAMING_SNAKE_CASE` for constants.

### 4.1 `std.prelude` (auto-imported)

Everything listed here is in scope without a `use`. Matches Ruby's "core" feel (P3, P5 for boundaries).

- Types: `String`, `Array[T]`, `Map[K,V]`, `Set[T]`, `Option[T]`, `Result[T,E]`, `Box[T]` (deferred — see §9), `Range`, `RangeInclusive`.
- Mixins: `Display`, `Debug`, `Clone`, `Copy`, `Drop`, `Eq`, `Ord` (new), `PartialEq`/`PartialOrd` (new, explicit), `Hashable` (distinct from `Map[K,V]`), `Iterator`, `IntoIterator`, `FromIterator`, `Error`, `Default`, `From[T]`, `Into[T]`, `TryFrom[T]`, `TryInto[T]`.
- Macros: `println!`, `eprintln!`, `print!`, `eprint!`, `format!`, `panic!`, `dbg!`, `todo!`, `unimplemented!`, `assert!`, `assert_eq!`. (Collections use the native literal forms: `[…]` for `Array[T]`, `{ k => v, … }` for `Map[K,V]`, and `Set.from_iter([…])` for `Set[T]`.)
- Functions: `puts` (legacy; kept for one release, delegates to `println!`), `eputs`, `print`.

### 4.2 `std.io`

```ruxen
use std.io

def read_line -> Result[String, IoError]
def stdin -> Stdin
def stdout -> Stdout
def stderr -> Stderr

class Stdin
  def read_line(self) -> Result[String, IoError]
  def read_to_string(self) -> Result[String, IoError]
  def lines(self) -> Lines        # Iterator[Result[String, IoError]]
end

class Stdout
  def write(&var self, bytes: &[UInt8]) -> Result[USize, IoError]
  def write_str(&var self, s: &str)   -> Result[(), IoError]
  def flush(&var self)                 -> Result[(), IoError]
end

class Stderr  # same surface as Stdout

enum IoError
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

### 4.3 `std.fs`

```ruxen
use std.fs
use std.path.{Path, PathBuf}

def read_to_string(path: &some AsRef[Path]) -> Result[String, IoError]
def read(path: &some AsRef[Path]) -> Result[Array[UInt8], IoError]
def write(path: &some AsRef[Path], contents: &[UInt8]) -> Result[(), IoError]
def write_str(path: &some AsRef[Path], contents: &str) -> Result[(), IoError]
def exists(path: &some AsRef[Path]) -> Bool
def is_file(path: &some AsRef[Path]) -> Bool
def is_dir(path: &some AsRef[Path])  -> Bool
def remove_file(path: &some AsRef[Path]) -> Result[(), IoError]
def remove_dir(path: &some AsRef[Path])  -> Result[(), IoError]
def create_dir(path: &some AsRef[Path])  -> Result[(), IoError]
def create_dir_all(path: &some AsRef[Path]) -> Result[(), IoError]
def rename(from: &some AsRef[Path], to: &some AsRef[Path]) -> Result[(), IoError]
def metadata(path: &some AsRef[Path]) -> Result[Metadata, IoError]
def read_dir(path: &some AsRef[Path]) -> Result[ReadDir, IoError]   # Iterator[DirEntry]

class File
  def self.open(path: &some AsRef[Path])   -> Result[File, IoError]
  def self.create(path: &some AsRef[Path]) -> Result[File, IoError]
  def read(&var self, buf: &var [UInt8])   -> Result[USize, IoError]
  def read_to_string(&var self, buf: &var String) -> Result[USize, IoError]
  def read_to_end(&var self, buf: &var Array[UInt8]) -> Result[USize, IoError]
  def write(&var self, bytes: &[UInt8])    -> Result[USize, IoError]
  def flush(&var self)                      -> Result[(), IoError]
  def sync(&self)                           -> Result[(), IoError]
  def metadata(&self)                       -> Result[Metadata, IoError]
end

struct Metadata
  len: UInt64
  is_file: Bool
  is_dir: Bool
  modified: Result[SystemTime, IoError]
end
```

### 4.4 `std.net` (minimal)

Deferred to phase 1c. Surface:

```ruxen
use std.net

class TcpStream
  def self.connect(addr: &str) -> Result[TcpStream, IoError]
  def read(&var self, buf: &var [UInt8]) -> Result[USize, IoError]
  def write(&var self, buf: &[UInt8])    -> Result[USize, IoError]
  def shutdown(&self)                     -> Result[(), IoError]
  def peer_addr(&self)                    -> Result[String, IoError]
end

class TcpListener
  def self.bind(addr: &str)               -> Result[TcpListener, IoError]
  def accept(&var self)                    -> Result[TcpStream, IoError]
  def incoming(&var self)                  -> Incoming     # Iterator[Result[TcpStream, IoError]]
end
```

No DNS resolver beyond what `getaddrinfo` gives us; no UDP; no TLS. These are future work.

### 4.5 `std.time`

```ruxen
use std.time

struct Duration
  # Opaque; constructors:
  def self.from_secs(s: UInt64)    -> Duration
  def self.from_millis(ms: UInt64) -> Duration
  def self.from_micros(us: UInt64) -> Duration
  def self.from_nanos(ns: UInt64)  -> Duration
  def self.zero                     -> Duration

  def as_secs(&self)        -> UInt64
  def as_millis(&self)      -> UInt64
  def as_micros(&self)      -> UInt64
  def as_nanos(&self)       -> UInt128  # see §9
  def subsec_nanos(&self)   -> UInt32

  # Arithmetic (via operator-overload mixin inclusion)
  include Add[Duration]   # a + b
  include Sub[Duration]   # a - b (saturating)
  include Ord
end

struct Instant
  def self.now             -> Instant
  def elapsed(&self)       -> Duration
  def duration_since(&self, earlier: Instant) -> Duration
end

struct SystemTime
  def self.now                     -> SystemTime
  def self.UNIX_EPOCH              -> SystemTime
  def duration_since(&self, earlier: SystemTime)
       -> Result[Duration, SystemTimeError]
end
```

Backing: `clock_gettime(CLOCK_MONOTONIC)` for `Instant`, `CLOCK_REALTIME` for `SystemTime`.

### 4.6 `std.env`

```ruxen
use std.env

def args -> Array[String]                          # argv, owned strings
def var(name: &str) -> Result[String, VarError]
def vars -> Array[(String, String)]
def set_var(name: &str, value: &str)
def remove_var(name: &str)
def current_dir -> Result[PathBuf, IoError]
def set_current_dir(path: &some AsRef[Path]) -> Result[(), IoError]
def home_dir -> Option[PathBuf]                    # $HOME on unix

enum VarError
  NotPresent
  NotUnicode(String)
end

const ARCH:   &str    # "x86_64" | "aarch64"
const OS:     &str    # "linux" | "macos"
const FAMILY: &str    # "unix"
```

`env.args` must be populated from `argc`/`argv` at program start; this requires emitting a `main` shim in codegen (see §7.6).

### 4.7 `std.process`

```ruxen
use std.process

def exit(code: Int32) -> !            # uses Ty::Never
def abort() -> !
def id() -> UInt32

class Command
  def self.new(program: &str) -> Command
  def arg(self, a: &str) -> Command              # consume self, return self
  def args[I: IntoIterator[Item=String]](self, xs: I) -> Command
  def env(self, key: &str, val: &str) -> Command
  def current_dir(self, path: &some AsRef[Path]) -> Command
  def spawn(self) -> Result[Child, IoError]
  def status(self) -> Result[ExitStatus, IoError]
  def output(self) -> Result[Output, IoError]
end

struct Output
  status: ExitStatus
  stdout: Array[UInt8]
  stderr: Array[UInt8]
end

struct ExitStatus
  def success(&self)   -> Bool
  def code(&self)      -> Option[Int32]
end

class Child
  def wait(self) -> Result[ExitStatus, IoError]
  def kill(&var self) -> Result[(), IoError]
  def id(&self) -> UInt32
end
```

Backing: `execvp`, `fork`, `waitpid`, `pipe` on Unix.

### 4.8 `std.fmt`

This module is *definitional* — it publishes the traits and types that format macros rely on. Implementation of the macros themselves lives in the compiler (see §7.4).

```ruxen
module std.fmt
  mixin Display
    def fmt(&self, f: &var Formatter) -> Result[(), FmtError]
  end

  mixin Debug
    def fmt(&self, f: &var Formatter) -> Result[(), FmtError]
  end

  class Formatter
    def write_str(&var self, s: &str) -> Result[(), FmtError]
    def write_char(&var self, c: Char) -> Result[(), FmtError]
    # width/precision/fill knobs:
    def width(&self) -> Option[USize]
    def precision(&self) -> Option[USize]
    def fill(&self) -> Char
    def alignment(&self) -> Alignment
  end

  enum Alignment
    Left
    Right
    Center
  end

  enum FmtError
    WriteFailed
  end
end
```

Every primitive (`Int`, `Float`, `Bool`, `Char`, `String`, `&str`), `Array`, `Map`, `Set`, `Option`, `Result`, `Tuple`, and fixed-size array types ship with blanket `Display` and/or `Debug` provisions. User types adopt `Display` via an `include Display` directive plus the required `fmt` method; `Debug` is implicitly included on struct/class bodies per §3.6 of the syntax spec (auto-implicit-include).

Format strings handled by the compiler-side macro expander: `{}` (Display), `{:?}` (Debug), `{:width.precision$}`, `{:>10}`, `{:<}`, `{:^}`, `{:0>4}`, `{:x}`, `{:X}`, `{:b}`, `{:o}`, `{:e}`, `{:.3}`.

### 4.9 `std.path`

```ruxen
use std.path

struct Path          # an unsized borrowed slice over a &[UInt8]; modeled as a newtype around &str for v1
  def self.new(s: &str) -> &Path
  def file_name(&self) -> Option[&str]
  def extension(&self) -> Option[&str]
  def parent(&self)    -> Option[&Path]
  def is_absolute(&self) -> Bool
  def is_relative(&self) -> Bool
  def join(&self, other: &some AsRef[Path]) -> PathBuf
  def to_path_buf(&self) -> PathBuf
  def to_string_lossy(&self) -> &str
  def components(&self) -> Components          # Iterator[&str]

  include AsRef[Path]
end

struct PathBuf       # owned, heap-allocated
  def self.new                 -> PathBuf
  def self.from(s: String)     -> PathBuf
  def var push(p: &some AsRef[Path])
  def var pop -> Bool
  def var set_extension(ext: &str) -> Bool
  def as_path(&self) -> &Path

  include AsRef[Path]
end

mixin AsRef[T]
  def as_ref(&self) -> &T
end

# &str and String adopt AsRef[Path] via in-body `include AsRef[Path]`
# directives next to their other method definitions.
```

For v1 `Path` is a thin newtype wrapper on `&str`; on Windows (future work) it will switch to `[UInt8]`. The abstraction insulates callers now so that change is non-breaking.

### 4.10 `std.hash`

Defines the `Hashable` *mixin* (for hashable keys) separately from the `Map[K, V]` *type* (see §9 on the name collision).

```ruxen
module std.hash
  mixin Hasher
    def var write(bytes: &[UInt8])
    def var write_u64(n: UInt64)
    def finish(&self) -> UInt64
  end

  mixin Hashable
    def hash[H: Hasher](&self, state: &var H)
  end

  class DefaultHasher       # SipHash-1-3 or similar; seeded per-process
    def self.new -> DefaultHasher

    include Hasher
    def var write(bytes: &[UInt8])   # ...body...
    def var write_u64(n: UInt64)     # ...body...
    def finish(&self) -> UInt64      # ...body...
  end

  class BuildHasher
    def self.new -> BuildHasher
    def build(&self) -> DefaultHasher
  end
end
```

This replaces the skeleton `Hashable` mixin registered at `resolve/mod.rs:143`. Keys for `Map[K, V]` and `Set[T]` must satisfy `Hashable + Eq`.

### 4.11 Method surfaces for built-in types (see §5)

`std.collections` re-exports `Array`, `Map`, `Set`, and also exposes:

```ruxen
class ArrayDeque[T]        # ring buffer — phase 1c
class BTreeMap[K, V]       # sorted map — phase 2
class BTreeSet[T]
```

For v1, only `Array`, `Map`, `Set` are required.

## 5. Method Surface for Built-in Types

This is the "one blend per method" table the project lead asked for. The "Ruby/Rust" columns show what each language calls a close analog; the "Ruxen" column is the canonical choice. Justifications are keyed to the core principles (P1–P5).

### 5.1 `Array[T]`

| Method | Signature (Ruxen) | Rust analog | Ruby analog | Justification |
|---|---|---|---|---|
| `self.new` | `-> Array[T]` | `Vec::new` | `Array.new` | P3 one obvious constructor |
| `self.with_capacity` | `(cap: USize) -> Array[T]` | `Vec::with_capacity` | — | perf escape hatch |
| `push` | `(&var self, item: T)` | `push` | `push` | identical name both sides |
| `pop` | `(&var self) -> Option[T]` | `pop` | `pop` (returns `nil`) | Option is the Ruxen convention |
| `len` | `(&self) -> USize` | `len` | `length`/`size` | Rust wins — shorter; P3 |
| `is_empty` | `(&self) -> Bool` | `is_empty` | `empty?` | Rust name; `?` is reserved for try (tension 4) |
| `clear` | `(&var self)` | `clear` | `clear` | same |
| `get` | `(&self, i: USize) -> Option[&T]` | `get` | `[]` (panics) | `.get` always safe; `[i]` panics (P1 loud danger) |
| `get_mut` | `(&var self, i: USize) -> Option[&var T]` | `get_mut` | — | mutability explicit |
| `first` / `last` | `(&self) -> Option[&T]` | `first`/`last` | `first`/`last` | same |
| `contains` | `(&self, x: &T) -> Bool` where `T: Eq` | `contains` | `include?` | Rust name |
| `iter` | `(&self) -> Iter[T]` | `iter` | `each` | `each` is the block form; `iter` returns a value |
| `iter_var` | `(&var self) -> IterMut[T]` | `iter_var` | — | needed for ownership correctness |
| `into_iter` | `(self) -> IntoIter[T]` | `into_iter` | — | moves |
| `each` | `(&self, f: Fn(&T))` *or* `(&self) do \|x\| … end` | — | `each` | Ruby block form coexists with iter |
| `map` | `[U](&self, f: Fn(&T) -> U) -> Array[U]` | `iter().map().collect()` | `map` | single call, no `.collect` (P3) |
| `filter` | `(&self, f: Fn(&T) -> Bool) -> Array[T]` where `T: Clone` | `iter().filter().collect()` | `select` | Rust name; `filter` is clearer |
| `filter_map` | `[U](&self, f: Fn(&T) -> Option[U]) -> Array[U]` | `filter_map` | — | ergonomic combinator |
| `find` | `(&self, f: Fn(&T) -> Bool) -> Option[&T]` | `find` | `find`/`detect` | Ruby convergence |
| `position` | `(&self, f: Fn(&T) -> Bool) -> Option[USize]` | `position` | `index` | Rust name |
| `any` | `(&self, f: Fn(&T) -> Bool) -> Bool` | `any` | `any?` | drop the `?` |
| `all` | `(&self, f: Fn(&T) -> Bool) -> Bool` | `all` | `all?` | same |
| `count` | `(&self) -> USize` | `count` | `count` | both agree |
| `fold` | `[B](&self, init: B, f: Fn(B, &T) -> B) -> B` | `fold` | `inject`/`reduce` | Rust name (tension 4: explicit over implicit) |
| `sum` | `(&self) -> T` where `T: Add` | `sum` | `sum` | same |
| `min` / `max` | `(&self) -> Option[&T]` where `T: Ord` | `min`/`max` | `min`/`max` | same |
| `sort` | `(&var self)` where `T: Ord` | `sort` | `sort!` | we always mutate Array in place, no copying variant |
| `sort_by` | `(&var self, f: Fn(&T, &T) -> Ordering)` | `sort_by` | `sort` with block | same |
| `reverse` | `(&var self)` | `reverse` | `reverse!` | in place |
| `join` | `(&self, sep: &str) -> String` where `T: Display` | `join` (on `&[&str]` only) | `join` | Ruby wins — works on any `Display` |
| `partition` | `(&self, f: Fn(&T) -> Bool) -> (Array[T], Array[T])` | `partition` | `partition` | same |
| `enumerate` | `(&self) -> Enumerate[Iter[T]]` | `enumerate` | `each_with_index` | Rust wins — shorter |
| `zip` | `[U](&self, other: &Array[U]) -> Zip` | `zip` | `zip` | same |
| `chunks` | `(&self, n: USize) -> Chunks[T]` | `chunks` | `each_slice` | Rust wins |
| `extend` | `[I: IntoIterator[Item=T]](&var self, xs: I)` | `extend` | `concat` | Rust name |
| `drain` | `(&var self) -> Drain[T]` | `drain` | — | ownership-transfer iteration |
| `clone` | `(&self) -> Array[T]` where `T: Clone` | `clone` | `dup` | Rust name |
| `to_vec` | `(&self) -> Array[T]` where `T: Clone` | `to_vec` | — | needed for iterator pipelines (already in codegen) |

Indexing `v[i]` panics on OOB (the safe form is `.get`), matching Rust and tutorial 13. This is P1 — danger is loud and explicit via `[]`.

### 5.2 `Map[K, V]`

Requires `K: Hashable + Eq`.

| Method | Signature | Justification |
|---|---|---|
| `self.new` | `-> Map[K, V]` | ctor |
| `self.with_capacity` | `(cap: USize) -> Map[K, V]` | perf |
| `insert` | `(&var self, k: K, v: V) -> Option[V]` | returns displaced value |
| `get` | `(&self, k: &K) -> Option[&V]` | safe lookup |
| `get_mut` | `(&var self, k: &K) -> Option[&var V]` | mutable lookup |
| `remove` | `(&var self, k: &K) -> Option[V]` | returns removed |
| `contains_key` | `(&self, k: &K) -> Bool` | tutorial 13 uses this spelling |
| `len` / `is_empty` | as `Array` | |
| `clear` | `(&var self)` | |
| `keys` | `(&self) -> Keys[K, V]` | iterator over &K |
| `values` | `(&self) -> Values[K, V]` | iterator over &V |
| `values_mut` | `(&var self) -> ValuesMut[K, V]` | |
| `iter` | `(&self) -> Iter[K, V]` yielding `(&K, &V)` | |
| `each` | `(&self) do \|k, v\| … end` | Ruby block form |
| `entry` | `(&var self, k: K) -> Entry[K, V]` | `or_insert` / `or_insert_with` API |
| `h[k]` | indexing panics if missing (tutorial 13) | P1 |

### 5.3 `Set[T]`

| Method | Signature |
|---|---|
| `self.new` | `-> Set[T]` |
| `insert` | `(&var self, x: T) -> Bool` (true iff newly inserted) |
| `contains` | `(&self, x: &T) -> Bool` |
| `remove` | `(&var self, x: &T) -> Bool` |
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
| `as_mut` | `(&var self) -> Option[&var T]` | |
| `take` | `(&var self) -> Option[T]` | leaves `nil` behind |
| `replace` | `(&var self, v: T) -> Option[T]` | |
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
| `clear` | `(&var self)` | — | |
| `push` | `(&var self, c: Char)` | — | |
| `push_str` | `(&var self, s: &str)` | — | |
| `pop` | `(&var self) -> Option[Char]` | — | |
| `insert` | `(&var self, i: USize, c: Char)` | — | byte index; panics if not on char boundary |
| `insert_str` | `(&var self, i: USize, s: &str)` | — | |
| `remove` | `(&var self, i: USize) -> Char` | — | |
| `truncate` | `(&var self, n: USize)` | — | |
| `capacity` | `(&self) -> USize` | — | |
| `chars` | `(&self) -> Chars` | same | iterator over `Char` — UTF-8 decode (v1 may fall back to byte iteration; see §9) |
| `bytes` | `(&self) -> Bytes` | same | iterator over `UInt8` |
| `as_bytes` | `(&self) -> &[UInt8]` | same | |
| `as_str` | `(&self) -> &str` | same | |
| `to_string` | `(&self) -> String` | `(&self) -> String` | mixin `Display` |
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

```ruxen
extension Int
  def self.MIN / MAX                         # constants
  def abs / pow / saturating_add / checked_add / wrapping_add
  def to_string -> String
  def to_string_radix(r: UInt32) -> String
end

extension Float
  def self.INFINITY / NAN / EPSILON
  def abs / sqrt / sin / cos / tan / ln / log2 / log10 / exp
  def floor / ceil / round / trunc / fract
  def is_nan / is_infinite / is_finite
  def to_string -> String
end
```

Backed by `libm` (already linked via `-lm` in `codegen/object.rs:70`).

### 5.8 Iterator mixin (`std.iter`)

```ruxen
mixin Iterator
  type Item
  def var next -> Option[Self.Item]

  # Default methods — all of §5.1's iterator combinators live here
  def consume map[B](f: Fn(Self.Item) -> B) -> Map[Self, B]   where Self: Sized
  def consume filter(f: Fn(&Self.Item) -> Bool) -> Filter[Self] where Self: Sized
  def consume take(n: USize) -> Take[Self]
  def consume skip(n: USize) -> Skip[Self]
  def consume chain[I: Iterator[Item=Self.Item]](other: I) -> Chain[Self, I]
  def consume enumerate -> Enumerate[Self]
  def consume zip[I: Iterator](other: I) -> Zip[Self, I]
  def consume collect[B: FromIterator[Self.Item]] -> B
  def consume fold[B](init: B, f: Fn(B, Self.Item) -> B) -> B
  def consume count -> USize
  def consume sum -> Self.Item where Self.Item: Add
  def consume min -> Option[Self.Item] where Self.Item: Ord
  def consume max -> Option[Self.Item] where Self.Item: Ord
  def consume find(f: Fn(&Self.Item) -> Bool) -> Option[Self.Item]
  def consume position(f: Fn(&Self.Item) -> Bool) -> Option[USize]
  def consume any(f: Fn(Self.Item) -> Bool) -> Bool
  def consume all(f: Fn(Self.Item) -> Bool) -> Bool
  def consume for_each(f: Fn(Self.Item))     # Rust-style non-block
  def consume each do |x| … end               # Ruby-style block form
  def consume to_array -> Array[Self.Item]
end

mixin IntoIterator
  type Item
  type IntoIter: Iterator[Item=Self.Item]
  def consume into_iter -> Self.IntoIter
end

mixin FromIterator[A]
  def self.from_iter[I: IntoIterator[Item=A]](iter: I) -> Self
end
```

With this mixin in place, `Array`'s map/filter/find stop being runtime-stubbed — they return iterator adapters that only materialize when `.to_array`/`.collect` is called. This is the single largest change from the current state.

## 6. Module System Design

### 6.1 Syntax

Ruxen already parses `use Foo.Bar`, `use Foo.Bar as B`, `use Foo.Bar.{X, Y}` (see §2.3 and `parser/ast.rs:692-704`). Stdlib reuses this machinery with a distinguished root name `std`:

```ruxen
use std.io.{read_line, stdin}
use std.fs
use std.collections.Map                       # canonical collection name
use std.time.{Instant, Duration}
```

Ruxen uses `.` as the path separator (`Http.Client`, `std.io.stdin`). The `package` keyword refers to the current compilation unit's root.

### 6.2 How stdlib is exposed to the resolver

Three layers, from cheapest to most general:

1. **Prelude (compiler-blessed).** `Resolver::register_builtins` (at `crates/ruxen-core/src/resolve/mod.rs:97`) grows to register the prelude types/traits/functions/macros. These are in every scope without a `use`.
2. **`std` root module (compiler-blessed).** We add a synthetic `DefKind::Module { items: [..] }` registered under the name `std` with children `io`, `fs`, `net`, `time`, `env`, `process`, `fmt`, `path`, `hash`, `collections`, `iter`, `mem`. Each child is itself a `DefKind::Module`. Users write `use std.io` or `use std.io.read_line` and the existing `resolve_use_decl` walker handles the rest.
3. **User-visible source.** Items that must have source bodies (e.g. `std.fs.read_to_string`) are *preferred* to live in `.rx` files shipped in the release tarball, discovered via `find_runtime_c`-style search. Items that are zero-Ruxen-wrapping-FFI-only (e.g. the raw `fopen` binding) live in a hidden `std.ffi` module, *not* in the user surface.

### 6.3 `core` vs `std` split (deferred)

The Rust split `core`/`alloc`/`std` is attractive but premature for v1. We pre-adapt by:

- Organizing source so that `Array`, `Map`, `Set`, heap-allocating string ops, and anything touching `malloc` live under `std.*`. Everything else (primitive methods, `Option`, `Result`, `fmt` mixins, iterators as pure mixins, `mem.size_of`) lives under a hidden sub-module `std.core.*` that gets re-exported. A future `core` package is a `mv` + cargo feature flag away.
- Enforcing that `std.core` files include no FFI declarations and no `malloc`-dependent calls.

### 6.4 File layout of the stdlib source tree

```
crates/ruxen-std/
  Cargo.toml                       # bookkeeping only — not compiled by cargo
  src/
    lib.rx                        # re-exports
    prelude.rx
    io.rx
    io/
      stdin.rx
      stdout.rx
    fs.rx
    fs/file.rx
    net.rx
    net/tcp.rx
    time.rx
    env.rx
    process.rx
    fmt.rx
    fmt/formatter.rx
    path.rx
    hash.rx
    collections.rx
    collections/array.rx
    collections/map.rx
    collections/set.rx
    iter.rx
    mem.rx
    ffi/                           # hidden, not in prelude
      posix.rx
      clock.rx
```

Rationale: keeping the stdlib in its own crate directory (but *not* compiled by cargo) lets us ship it as a discrete deliverable in the release tarball at `~/.ruxen/share/ruxen/std/` and lets the compiler find it the same way it finds `runtime.c` today.

### 6.5 Name resolution for `std.*`

At the top of `Resolver::register_builtins` we call a new `register_std()` that parses the stdlib sources (read from the search path below), runs them through the full resolve pass, and merges the resulting `SymbolTable` into the compiler's registry under a root `DefId` named `std`. The search path mirrors `find_runtime_c` (`codegen/mod.rs:27-65`):

1. `$RUXEN_STD` env var (overrides everything).
2. `<exe>/../lib/ruxen/std/` (installed layout).
3. `<exe>/../share/ruxen/std/` (alternate).
4. `$CARGO_MANIFEST_DIR/../ruxen-std/src/` (dev fallback).

Failure to find the stdlib is a hard compile error unless the user passes `ruxenc --no-std` (see §7.7).

## 7. Implementation Strategy

### 7.1 Decision: compiler-blessed vs written-in-Ruxen vs FFI-wrappers

One blend per module (P3). The table below pins the call for each v1 module.

| Module | Written in | Rationale |
|---|---|---|
| `std.prelude` (re-export list) | Compiler-registered in `register_builtins` | it's just a name table |
| `std.io` surface | Ruxen; bodies call into `lib "c"` libc bindings | already the FFI pattern (tutorial 14) |
| `std.fs` | Ruxen + `lib "c"` (`fopen`, `fread`, `fclose`, `unlink`, `mkdir`, `rename`, `stat`) | same |
| `std.net` | Ruxen + `lib "c"` (`socket`, `bind`, `listen`, `accept`, `connect`, `send`, `recv`, `close`) | same |
| `std.time` | Ruxen + `lib "c"` (`clock_gettime`) | same |
| `std.env` | Ruxen + `lib "c"` (`getenv`, `setenv`, `unsetenv`, `environ`) + compiler-emitted `argv` | argv needs main-shim support |
| `std.process` | Ruxen + `lib "c"` (`fork`, `execvp`, `waitpid`, `_exit`, `pipe`) | |
| `std.fmt` mixins | Ruxen | plain mixin defs |
| `std.fmt` format macros | Compiler | hygienic expansion (tension 5) — must know about types at call site |
| `std.path` | Ruxen (thin wrapper over `&str`) | no C needed |
| `std.hash` mixin | Ruxen | plain mixin |
| `std.hash.DefaultHasher` | Ruxen (SipHash provision) or `lib "c"` wrapping a bundled C SipHash | perf; either works |
| `Array[T]`, `Map[K,V]`, `Set[T]` runtime | C — new functions in `runtime.c` | keeps the element-type-erased convention |
| Iterator combinators (`Map`, `Filter`, `Zip`, …) | Ruxen | they are zero-cost when monomorphized (tension 6) |

**Why we keep collections in C**: generic monomorphization in Ruxen is not yet load-bearing (the typechecker erases to `i64`-slot, see §2.1), so we continue the pattern runtime.c uses: callers pass 64-bit slots. A real monomorphized path is follow-up work and is sketched in §10.

### 7.2 Runtime growth (`runtime.c`)

Phase 1a adds ~18 functions:

```
ruxen_vec_pop, ruxen_vec_clear, ruxen_vec_first, ruxen_vec_last
ruxen_vec_remove, ruxen_vec_insert, ruxen_vec_extend_from_ptr
ruxen_vec_sort_i64 (initial, scalar only), ruxen_vec_reverse
ruxen_hash_new, ruxen_hash_insert, ruxen_hash_get, ruxen_hash_remove
ruxen_hash_contains, ruxen_hash_len, ruxen_hash_clear
ruxen_set_new, ruxen_set_insert, ruxen_set_contains, ruxen_set_remove
ruxen_string_push, ruxen_string_push_char
ruxen_string_to_upper, ruxen_string_replace, ruxen_string_starts_with
ruxen_string_ends_with, ruxen_string_contains, ruxen_string_find
ruxen_str_parse_int (signed, replaces parse_uint)
ruxen_panic_with_location (file, line, col)
```

Phase 1b adds ~12 I/O functions:

```
ruxen_file_open, ruxen_file_close, ruxen_file_read, ruxen_file_write
ruxen_fs_read_to_string, ruxen_fs_write, ruxen_fs_exists
ruxen_fs_remove_file, ruxen_fs_create_dir, ruxen_fs_rename
ruxen_env_var, ruxen_env_args_count, ruxen_env_args_at
ruxen_process_exit, ruxen_clock_gettime_monotonic
```

All new functions follow the existing convention:

- Return `void*` for heap values, `int64_t` for integers/booleans.
- Errors are encoded as a tagged union (same layout as `Option` / `Result` — see `runtime.c:283-309`): `[tag: i32][pad: i32][payload: i64]`.
- No direct memory transfer of struct-by-value across the boundary — always a heap pointer.

Alternative considered: replacing `runtime.c` with a `libruxen_std.a` Rust crate. **Rejected** for v1: it doubles the release artifacts, requires the user to have a Rust toolchain to build from source, and the existing pattern (a single C TU compiled at link time) already works. The codegen's `object::compile_runtime` (`codegen/object.rs:10`) would need to be generalized to list-of-translation-units either way. Revisit in v2.

### 7.3 Typechecker deltas (`typeck/infer.rs`, `resolve/mod.rs`)

- `builtin_method_type` (lines 813-928) must grow to cover the full §5 tables. This will roughly triple its size; consider extracting to a declarative table (`src/typeck/stdlib_methods.rs`) keyed on `(ty_pattern, method_name)` → `fn(&var Ctx) -> Ty`.
- `register_builtins` must register `std` as a module root, register mixins with full method signatures (not just names), and register prelude macros (`println!`, `format!`, …) — the macros need a new `DefKind::Macro` variant plus support in `parser/expr.rs`'s macro call path (currently at line 294-302).
- Iterator default methods blow up the constraint solver unless we're careful: each `.map().filter().to_array` is a chain of monomorphized generic calls. Recommendation: implement the mixin's default methods as Ruxen source (so they get lowered to MIR normally) rather than hard-coding their return types in `builtin_method_type`. This means we do *not* expand `builtin_method_type` to 300+ lines; instead, it keeps the existing ~100 lines for the things that must be compiler-known (`String.split`, `Array[T].iter` returning a specific opaque iterator type) and the rest is resolved via the generic method lookup already in place at `infer.rs:800`.

### 7.4 Format macros (compiler-side)

`println!`, `format!`, `eprintln!`, `print!`, `eprint!`, `dbg!`, `panic!` are hygienic compile-time macros (tension 5, see `decision_tensions.md`). Expansion lives in `crates/ruxen-core/src/parser/macros.rs` (new file) and runs at *parse* time (not desugar-in-resolve), so that the expanded `HirExpr` flows through type checking normally. Each format call expands to:

```ruxen
# println!("hello {}, age {}", name, age)
# becomes:
do
  var __buf = String.new
  Display.fmt(&name, &var Formatter.for(&var __buf)).unwrap!
  __buf.push_str(", age ")
  Display.fmt(&age, &var Formatter.for(&var __buf)).unwrap!
  __buf.push('\n')
  std.io.stdout.write_str(&__buf).unwrap!
end
```

This allows the existing borrow checker and type checker to work unchanged. Alternative: build a single `vformat` runtime call with a compile-time type-tag array — rejected because it hides errors from the borrow checker and multiplies codegen complexity.

Each macro call site becomes a `HirExprKind::Block` after expansion, with `HirExprKind::MacroCall` kept only as a fallback for the native collection literals (`[…]` array, `{ k => v, … }` map, and `Set.from_iter([…])` construction) which already have a lowering path in `mir/lower.rs:1503`.

### 7.5 Threading through the pipeline

Walk order (lexer is unchanged):

1. **Parser** (`parser/expr.rs`): recognize `{:x}`, `{:?}`, `{:>10.3}` inside format strings.
2. **Macro expander** (new): run after parse, before resolve. Expands format macros to HIR-style `Block` exprs (still in AST).
3. **Resolver** (`resolve/mod.rs`): `register_std()` loads stdlib `.rx` sources from the search path, runs a mini resolve pass, and merges their `SymbolTable` into the main one. Also registers all new DefIds for stdlib types/mixins/fns.
4. **Typechecker** (`typeck/infer.rs`): new `builtin_method_type` entries + mixin default methods now resolve via normal nominal `include` lookup (`traits.rs:136-180`), not hard-coded returns.
5. **Borrow checker** (unchanged): new types follow the existing Move/Copy rules (`hir/types.rs:184-235`). `File`, `Stdin`, `TcpStream` are Move; `Path` is borrow; `Duration`, `Instant` are Copy.
6. **MIR lowerer** (`mir/lower.rs`): add `{ k => v, … }` map-literal and `Set.from_iter([…])` lowering next to the existing array-literal case at line 1502. Format-macro expansion is already handled at step 2 so no lowerer change.
7. **Codegen** (`codegen/runtime.rs`): extend `runtime_name()` with the new mangled names. The `?T...` fallback block (lines 131-167) can shrink once real mixin dispatch lands.

### 7.6 Program entry shim

For `std.env.args` to work, the compiler's emitted `main` must capture `argc`/`argv`. Today `codegen/cranelift.rs` emits a plain `main`. Change: emit

```c
int main(int argc, char **argv) {
    ruxen_env_init(argc, argv);
    ruxen_user_main();
    return 0;
}
```

where `ruxen_user_main` is the user's `def main`. `ruxen_env_init` stashes argv in a static and exposes it via `ruxen_env_args_count` / `ruxen_env_args_at` used by `std.env`. This is ~20 lines of C in `runtime.c` and a 3-line tweak in `cranelift.rs` and `llvm/emit.rs`.

### 7.7 `--no-std` and the REPL

Add `ruxenc --no-std` and a `[package] no-std = true` manifest key. When set:

- `register_std()` is skipped.
- Prelude is reduced to: primitive types, `Option`, `Result`, `Array`, mixins `Copy`/`Clone`/`Drop`/`Sized`, macros `panic!`/`assert!`. No `println!`, no `io`, no `fmt`.
- This pre-plans the `core` vs `std` split in §6.3 without committing to it.

For the REPL (`crates/ruxen-repl`, phase 12 per memory), stdlib loads lazily on first use.

### 7.8 Distribution

Update `install.sh` (already at lines 161-168) to install `share/ruxen/std/` from the tarball. Update the release workflow (`project_release_setup.md`) to bundle `crates/ruxen-std/src/` into the tarball as `share/ruxen/std/`. Update `codegen/mod.rs::find_runtime_c` pattern with a `find_std_root` sibling.

## 8. Phasing

### Phase 1a — "make what exists real" (2–3 weeks)

- Real bodies for `Array.map`, `.filter`, `.find`, `.position`, `.each`, `.partition`, `.enumerate` via Iterator mixin.
- `Map[K,V]` and `Set[T]` end-to-end (runtime.c + typechecker + mangled names).
- `Iterator` mixin in `std.iter` with default methods; `IntoIterator`, `FromIterator`.
- `String` full surface (§5.6).
- `{ k => v, … }` map-literal and `Set.from_iter([…])` lowering in `mir/lower.rs`.
- `println!`, `eprintln!`, `print!`, `eprint!`, `format!`, `dbg!`, `panic!`, `assert!`, `assert_eq!` format macros.
- `Display` / `Debug` mixins with blanket provisions for primitives; implicit `Debug` include on structs/classes per §3.6.
- Prelude registration.
- Delete `ruxen_noop_passthrough`, `ruxen_noop_return_null` (they become unreferenced once real dispatch lands).

Exit: `tutorial/13-collections.md` examples compile and run. `sample_program.rx` (which uses `.partition`, `.iter`, `.map`, `.unwrap_or_else`, `{"closure"}`, `.filter`) runs with observable output.

### Phase 1b — "I/O that matters" (2 weeks)

- `std.io` (stdin/stdout/stderr + IoError).
- `std.fs` (file read/write/open + metadata).
- `std.env` (args, var, vars, current_dir) with argv shim in main.
- `std.process` (exit, Command, spawn, status, output).
- FFI hidden module `std.ffi.posix` with raw `lib "c"` bindings to libc.

Exit: `File.read_string(path)?` (tutorial 7/11), `read_line()` (tutorial 5), `env.args` all work.

### Phase 1c — "time, paths, hashing, network" (2 weeks)

- `std.time` (Instant, Duration, SystemTime) + `clock_gettime` binding.
- `std.path` (Path, PathBuf) + integration with `fs` APIs.
- `std.hash` (Hasher mixin, SipHash provision, BuildHasher) — replaces the placeholder `Hashable` mixin registered at `resolve/mod.rs:143`.
- `std.net` (TcpStream, TcpListener) + `socket`/`connect`/`listen`/`accept` bindings.
- `std.fmt` polish: width/precision/alignment/radix specifiers.

Exit: a minimal HTTP client demo compiles and runs (`TcpStream.connect` + `write` + `read_to_string`).

### Phase 2 (out of v1 scope, enumerated for context)

- `ArrayDeque`, `BTreeMap`, `BTreeSet`, `LinkedList`.
- Proper UTF-8 `char` handling in `String.chars`.
- `std.sync`, `std.thread` (sibling doc).
- `std.rand`.
- `core` vs `std` split.
- Windows `Path` (WTF-8).
- A `libruxen_std.a` Rust-side option to replace the C runtime growth strategy.

## 9. Open Questions

1. **`Hashable` vs hash-keyed map.** The canonical resolution chosen by spec §3.11 is:
   - Collection type is `Map[K,V]` (Ruby word, no conflict). **Final.**
   - Mixin describing "value that can be hashed" is `Hashable`. **Final.**
   - `Hashable` is included via the usual `include Hashable` directive (or implicitly per §3.6 when every field is `Hashable`).
2. **`Char` width.** `hir/types.rs:57` has `Ty::Char` but the lexer/string code treats strings as byte arrays. For `String.chars`: emit `Char` as `u32` and UTF-8 decode in `ruxen_string_chars_next`? Or keep `Char` as ASCII byte and defer? **Recommend**: `Char` stays 32-bit Unicode scalar; `String.chars` decodes; non-ASCII strings that come via FFI get validated lazily.
3. **`UInt128`.** `Duration.as_nanos` wants a 128-bit result. Ruxen does not model `Int128`/`UInt128` yet (`hir/types.rs:40-56`). Options: add it; return `Result[UInt64, OverflowError]`; return `UInt64` with saturating semantics. **Recommend**: saturating `UInt64` in v1, add `UInt128` in phase 2.
4. **Operator overloading for arithmetic mixins (`Add`, `Sub`, `Mul`, …).** Needed for `Duration + Duration`. Ruxen parses `a + b` via `BinOp::Add` in `parser/ast.rs:367`. The typechecker today only accepts numeric operands. Do we (a) hard-code Duration arithmetic in the typechecker (ugly) or (b) add operator overloading via mixin lookup (principled but larger scope)? **Recommend (b)**, track as a dependency of `std.time`.
5. **`read_line` in prelude vs `std.io.read_line`.** Tutorial 5 uses bare `read_line()`. Either the prelude exports it (breaking the namespace discipline of P5) or we update the tutorial. **Recommend**: put it in `std.io` and update tutorial 5; it's one-line addition of `use std.io.read_line` in that example.
6. **Default string type for literals.** `"foo"` is `&str` today (tutorial 2). `String.new("foo")` and `"foo"` both exist in the sample. Pick one? **Recommend**: `String.from` is canonical; `String.new` with no args is empty string; `String.new(&str)` is removed (it's redundant with `from`).
7. **Stdlib compiled or interpreted?** Does the compiler re-parse `share/ruxen/std/*.rx` every invocation, or do we cache the resolved `SymbolTable` to a `.rlib`-style blob? Phase 13 already has content-addressed caching in `ruxenc`. Reuse that. **Recommend**: cache on first use, invalidate on compiler version bump.
8. **`panic!` macro location info.** Needs file/line/col at the call site. Ruxen already plumbs `Span` through to HIR (`lexer/token.rs`). We need `runtime.c::ruxen_panic_with_location` — trivial to add.

## 10. Risks

1. **Iterator mixin + monomorphization is a load-bearing change.** The compiler today sidesteps generics by falling through to `ruxen_noop_passthrough` for `?T...` methods (`codegen/runtime.rs:131-167`). Implementing real iterator combinators *forces* us to commit to a concrete monomorphization strategy. If monomorphization slips, phase 1a slips. Mitigation: ship phase 1a with a hybrid — `Array.map`/`.filter` work as direct methods that allocate; `.iter().map().filter().collect()` is deferred to phase 1a.5. This keeps the tutorial examples honest without blocking on generics.
2. **`Map[K,V]` generic keys in a non-generic runtime.** The runtime is element-type-erased (`int64_t` slots). Hash keys need *equality* and *hashing* of user-defined structs; 64-bit slot erasure makes that hard. Mitigation: in v1, restrict `Map[K, V]` keys to `{Int, UInt, USize, String, &str}`. Generalize in phase 2 when we have real monomorphization.
3. **FFI calling conventions on aggregates.** `stat`, `addrinfo`, `timespec` pass structs by value/pointer and vary across libc / musl / glibc / macOS. Mitigation: in v1, do not expose `stat` struct directly; `fs.metadata` calls `stat` inside C and returns a flat `Metadata` struct with ABI-stable types.
4. **`argv` ownership.** Turning `char **argv` into `Array[String]` means copying — argv strings live until process exit, but our `Array[String]` owns its elements and frees them on drop, which would `free()` memory we don't own. Mitigation: copy argv into heap strings at `ruxen_env_init` time.
5. **The `?T...` codegen fallback masks real bugs.** `runtime.c`'s `ruxen_noop_passthrough` makes miscompiled code *run without error*. Once real dispatch lands, some currently-passing tests may start failing because the noop hid a type-resolution bug. Mitigation: in phase 1a, add a `ruxenc --strict-dispatch` flag that turns all `?T...` → `ruxen_noop_passthrough` lookups into hard errors, and run the full test suite with it on.
6. **Scope creep.** The obvious temptation is to ship `BTreeMap`, async, UTF-8, rand, thread, and sync all at once. Mitigation: this doc is explicit about v1 vs phase 2, and the sibling concurrency doc owns thread/sync.
7. **Tutorial drift.** The tutorial already promises `Map.new` and `{ k => v, … }` map literals. The rename to `Map` requires the tutorial sweep tracked as a blocking subtask of phase 1a.
8. **Macro hygiene / identifier capture.** `println!("{}", x)` expanding to `var __buf = ...` risks name collision with a user variable named `__buf`. Mitigation: use gensym'd names with a reserved prefix (`__rx_fmt_N`) that the lexer rejects at the user surface.
9. **Sanitizer builds.** `object.rs:10-42` compiles `runtime.c` with `-fsanitize=address,undefined` under `--sanitize`. New stdlib C code must be ASan-clean; expect several rounds of fixing leaks and UB in the first implementation pass.
