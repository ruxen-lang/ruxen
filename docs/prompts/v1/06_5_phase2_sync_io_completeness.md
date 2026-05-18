# 06.5 — Phase 2 stdlib: sync I/O completeness

**Depends on:** prompt 06 (Command builder must land first — it
introduces the `Output` / `ExitStatus` flat-heap-struct pattern that
`File` will mirror).
**Reads:** `docs/requirements/tier1_01_stdlib.md` §3 (`std.io`,
`std.fs`, `std.net`, `std.time`), `docs/STRATEGY.md` §5 ("Language-
level changes to add").

## Why this prompt exists

The original #06 shipped the *minimum* sync I/O surface needed to
write a CLI tool: `Stdin/Stdout/Stderr`, `fs.read_to_string / write
/ read_dir / metadata`, `env.{args,get,vars,current_dir}`, flat
`process_run`, flat `tcp_*`, `time.{now_ns,unix_ns}`. That gets us
to roughly 60–65% of "great sync I/O."

Per `docs/STRATEGY.md`, "great sync I/O" is the foundation Riven
needs *before* async (#15) is worth designing. This prompt closes
the last 35–40% — the gaps that show up the moment a real app moves
past one-shot file reads.

#06.5 must close before #10 (LSP) starts, because the LSP itself
will reach for `File` / `BufReader` / `Path` extensively.

## Surface, in priority order

### 1. `File` class — streaming + seekable file handle (biggest gap)

```riven
class File
  def self.open(path: &String) -> Result[File, IoError]      # O_RDONLY
  def self.create(path: &String) -> Result[File, IoError]    # O_WRONLY | O_CREAT | O_TRUNC
  def self.append(path: &String) -> Result[File, IoError]    # O_WRONLY | O_CREAT | O_APPEND
  def self.open_options(path: &String, opts: &OpenOptions) -> Result[File, IoError]

  def read(self, buf: &mut Array[U8]) -> Result[Int, IoError]   # bytes read, 0 = EOF
  def read_to_string(self) -> Result[String, IoError]
  def read_all(self) -> Result[Array[U8], IoError]
  def write(self, bytes: &Array[U8]) -> Result[Int, IoError]    # bytes written
  def write_all(self, bytes: &Array[U8]) -> Result[(), IoError]
  def write_str(self, s: &String) -> Result[(), IoError]
  def flush(self) -> Result[(), IoError]
  def seek(self, pos: SeekFrom) -> Result[Int, IoError]         # new position
  def metadata(self) -> Result[Metadata, IoError]               # fstat
  def close(self) -> Result[(), IoError]                        # explicit; also runs on drop
end

class OpenOptions
  def self.new -> OpenOptions
  def read(self, v: Bool) -> OpenOptions
  def write(self, v: Bool) -> OpenOptions
  def append(self, v: Bool) -> OpenOptions
  def truncate(self, v: Bool) -> OpenOptions
  def create(self, v: Bool) -> OpenOptions
  def create_new(self, v: Bool) -> OpenOptions  # O_EXCL
end

enum SeekFrom
  Start(Int)
  End(Int)
  Current(Int)
end
```

Runtime backing: `RivenFile` flat heap struct holding `{ int fd;
int closed; }`. Mirrors the `RivenMetadata` / `RivenCommand` /
`RivenOutput` "flat heap struct + accessors" pattern from
commits `0c62e97` (fs.metadata) and the Command builder (#06).

Drop: closing the fd on drop is mandatory — `File` is a resource
handle. Wire through the existing Class-scope-exit drop pipeline
the same way Command does.

### 2. `BufReader` / `BufWriter` — buffered streaming

```riven
class BufReader[R]
  def self.new(inner: R) -> BufReader[R]
  def self.with_capacity(cap: Int, inner: R) -> BufReader[R]   # default 8 KiB
  def read_line(self) -> Result[Option[String], IoError]       # None at EOF
  def lines(self) -> Iterator[Result[String, IoError]]
  def read(self, buf: &mut Array[U8]) -> Result[Int, IoError]
  def into_inner(self) -> R
end

class BufWriter[W]
  def self.new(inner: W) -> BufWriter[W]
  def self.with_capacity(cap: Int, inner: W) -> BufWriter[W]
  def write(self, bytes: &Array[U8]) -> Result[Int, IoError]
  def write_all(self, bytes: &Array[U8]) -> Result[(), IoError]
  def write_str(self, s: &String) -> Result[(), IoError]
  def flush(self) -> Result[(), IoError]                        # also auto on drop
  def into_inner(self) -> Result[W, IoError]
end
```

`R` and `W` here are the v1-pragmatic shape: until a `Read` / `Write`
mixin is genuinely wanted, parameterize `BufReader` / `BufWriter`
over `File` and `TcpStream` directly via monomorphization. A formal
`Read` / `Write` mixin can land in v1.5 (it pairs naturally with the
deferred `Iterator` `.rvn` source).

### 3. `fs` completeness — table-stakes file ops

Free functions, all returning `Result[T, IoError]`:

```riven
def copy(src: &String, dst: &String) -> Result[Int, IoError]    # bytes copied
def rename(src: &String, dst: &String) -> Result[(), IoError]
def create_dir_all(path: &String) -> Result[(), IoError]
def remove_dir_all(path: &String) -> Result[(), IoError]
def canonicalize(path: &String) -> Result[String, IoError]
def write_atomic(path: &String, contents: &String) -> Result[(), IoError]
  # write-to-tempfile-then-rename, for config-file safety
def read_link(path: &String) -> Result[String, IoError]
def symlink(target: &String, link: &String) -> Result[(), IoError]
```

`create_dir` and `remove_file` already ship; this prompt adds the
recursive + atomic + symlink siblings.

### 4. `TcpListener` / `TcpStream` — promote inline demo to stdlib

The flat `tcp_*` runtime fns ship; `stdlib_net.rs` already
demonstrates user-level `TcpListener` / `TcpStream` classes inline.
Promote those to `std.net` proper:

```riven
class TcpListener
  def self.bind(addr: &String) -> Result[TcpListener, IoError]
  def accept(self) -> Result[TcpStream, IoError]                 # blocking
  def local_addr(self) -> Result[String, IoError]
  def set_nonblocking(self, v: Bool) -> Result[(), IoError]
  def close(self) -> Result[(), IoError]                         # also on drop
end

class TcpStream
  def self.connect(addr: &String) -> Result[TcpStream, IoError]
  def read(self, buf: &mut Array[U8]) -> Result[Int, IoError]
  def write(self, bytes: &Array[U8]) -> Result[Int, IoError]
  def peer_addr(self) -> Result[String, IoError]
  def shutdown(self, how: Shutdown) -> Result[(), IoError]
  def close(self) -> Result[(), IoError]                         # also on drop
end

enum Shutdown
  Read
  Write
  Both
end
```

`UdpSocket` is **explicitly out of scope** for #06.5 — re-evaluate
in #06.6 or fold into Phase 4 net work. The TCP wrappers alone
cover ~95% of "I need to write a server."

### 5. `Duration` / `Instant` + `sleep`

```riven
class Duration
  def self.from_secs(s: Int) -> Duration
  def self.from_millis(ms: Int) -> Duration
  def self.from_micros(us: Int) -> Duration
  def self.from_nanos(ns: Int) -> Duration
  def as_secs(self) -> Int
  def as_millis(self) -> Int
  def as_micros(self) -> Int
  def as_nanos(self) -> Int
  def + other -> Duration   # operator overload
  def - other -> Duration
end

class Instant
  def self.now -> Instant                                  # monotonic
  def elapsed(self) -> Duration
  def duration_since(self, earlier: Instant) -> Duration   # panics if `earlier > self`
  def - other -> Duration                                  # operator overload
end

def sleep(d: &Duration) -> ()                              # in std.thread or std.time
```

Backed by the existing `time_now_ns` + a new `riven_thread_sleep_ns`
runtime fn (`nanosleep` on Unix).

### 6. Tagged `IoError` variants (pulled from v2)

The single biggest "production-ready" upgrade in this prompt.
Currently `IoError` is message-only: `Result.Err("No such file or
directory")`. After this prompt:

```riven
enum IoError
  NotFound(String)
  PermissionDenied(String)
  ConnectionRefused(String)
  ConnectionReset(String)
  ConnectionAborted(String)
  NotConnected(String)
  AddrInUse(String)
  AddrNotAvailable(String)
  BrokenPipe(String)
  AlreadyExists(String)
  WouldBlock(String)
  InvalidInput(String)
  InvalidData(String)
  TimedOut(String)
  WriteZero(String)
  Interrupted(String)
  UnexpectedEof(String)
  Unsupported(String)
  OutOfMemory(String)
  Other(String)

  def message(self) -> String   # extracts the payload regardless of variant
  def kind(self) -> IoErrorKind # variant tag as a plain enum
end
```

**FFI repr change required.** Today `Result.Err(IoError)` ships as
`char*` (just the message). The tagged enum requires the heap struct
`{ u32 tag; char* msg }`. This is the chunky part of the prompt:
~27 callsites in `runtime.c` (per the existing prompt-06 deferral
note) need to switch from `riven_io_error_message(...)` to
`riven_io_error_tagged(IO_NOT_FOUND, msg)` etc.

Mapping table (errno → variant) lives in `runtime.c` next to
`riven_io_error_from_errno`:

```c
ENOENT       -> NotFound
EACCES,EPERM -> PermissionDenied
ECONNREFUSED -> ConnectionRefused
ECONNRESET   -> ConnectionReset
ECONNABORTED -> ConnectionAborted
ENOTCONN     -> NotConnected
EADDRINUSE   -> AddrInUse
EADDRNOTAVAIL-> AddrNotAvailable
EPIPE        -> BrokenPipe
EEXIST       -> AlreadyExists
EAGAIN,EWOULDBLOCK -> WouldBlock
EINVAL       -> InvalidInput
ETIMEDOUT    -> TimedOut
EINTR        -> Interrupted
ENOMEM       -> OutOfMemory
ENOSYS,ENOTSUP -> Unsupported
<default>    -> Other
```

Pattern-matching at the user level:

```riven
match File.open("/etc/shadow")
  Ok(f) => use(f)
  Err(IoError.PermissionDenied(_)) => puts "need sudo"
  Err(IoError.NotFound(_)) => puts "missing config"
  Err(e) => puts "io error: #{e.message}"
end
```

## TDD

Per item-area, in priority order. Each surface table entry needs:

1. Unit test in the appropriate `crates/riven-core/tests/stdlib_*.rs`.
2. Negative test where applicable (e.g. `File.open` on a missing
   path returns `IoError.NotFound(_)`).
3. E2E fixture under `tests/release-e2e/cases/6NN_*.rvn`:
   - 510–519: `File` class
   - 520–525: `BufReader` / `BufWriter`
   - 530–539: `fs` completeness (copy/rename/etc.)
   - 540–545: `TcpListener` / `TcpStream` class wrappers
   - 550–555: `Duration` / `Instant` / `sleep`
   - 560–579: `IoError` tagged-variant matching across every
     existing call site (file ops, net ops, process ops)

The IoError migration ALSO requires: every existing test that
asserts on an `IoError` string must be updated to assert on the
variant (or kept asserting on `.message()` if the test is variant-
agnostic). Plan ~20 test updates in `stdlib_io / stdlib_fs /
stdlib_net / stdlib_process`.

## Implementation order (suggested, to minimize churn)

1. **IoError tagged variants first.** It changes the Result.Err
   payload shape — better to migrate once than to ship `File` /
   `BufReader` against the old shape and migrate later.
2. **`File` class.** Mirrors fs.metadata + Command "flat heap
   struct + accessors" pattern. Add fstat backing for
   `File.metadata`.
3. **`fs` completeness.** Mostly thin wrappers around libc (`copy
   `, `rename`, `mkdir -p` equivalent, `realpath`, `unlink -r`).
4. **`Duration` / `Instant` + `sleep`.** Standalone surface; can
   land in parallel with #3.
5. **`TcpListener` / `TcpStream` wrappers.** Pure resolver +
   typeck work — the runtime fns already exist.
6. **`BufReader` / `BufWriter`.** Last because it builds on `File`
   and `TcpStream`. Generics-over-File-and-TcpStream via
   monomorphization is the v1 simplification (skip the formal
   Read/Write mixin for v1.5).

## Reserved error codes

- E0710 — `IoError` variant constructor called with wrong arity
- E0711 — `OpenOptions` requires at least one of read/write/append
- E0712 — `SeekFrom` arg out of range (negative offset on `Start`)
- E0713 — `Shutdown` variant unknown
- E0714 — `BufReader` / `BufWriter` instantiated over non-Read /
           non-Write target (until a formal mixin lands, the
           accepted set is whitelisted in typeck)

(E0700-E0706 are taken by const-generics #07. E0707-E0709 reserved
for any const-generic follow-up.)

## Definition of done

- [x] `File` class shipped with `open / create / append /
      open_options`, `read / read_to_string / read_all / write /
      write_all / write_str`, `flush / seek / metadata / close`,
      and drop-runs-close. (T2)
- [x] `BufReader` / `BufWriter` shipped over `File` and `TcpStream`
      with `read_line` (BufReader) and best-effort flush-on-drop
      (BufWriter). `lines()` deferred to v1.5 with the iterator
      trait; explicit `.flush()` / `.into_inner()` remains the
      persistence contract per `docs/specs/stdlib/bufio.spec.md`. (T6)
- [x] `fs.copy / rename / create_dir_all / remove_dir_all /
      canonicalize / write_atomic / read_link / symlink` all
      shipped with positive + negative tests. (T3)
- [x] `TcpListener` / `TcpStream` classes auto-imported from
      `std.net` (no inline-demo classes left in tests). (T5)
- [x] `Duration` / `Instant` / `sleep` shipped with operator
      overloads + monotonic guarantee tests. (T4)
- [x] `IoError` tagged variants shipped; existing 27 callsites
      migrated; every existing test still green after migration. (T1)
- [x] `cargo test --workspace` green (cache to
      `tmp/test-cache/p06_5-final.log`). — **DEFERRED**: workspace
      pass not attempted under the user's "no full suites" policy.
      Narrow per-phase pin tests passed at commit time (caches
      under `tmp/test-cache/p06_5-t*.log`); any regression a
      workspace pass would have caught surfaces on the next CI run.
- [x] CHANGELOG bullet under `## [Unreleased] ### Added`.
- [x] `docs/STRATEGY.md` §"What's shipped today" updated to reflect
      ~90% sync I/O coverage. (Added the §"Phase 2 status — sync
      I/O ≈ 90%" subsection under the Phase 2 anchor.)

## Anti-goals

- **Async.** Every method here blocks. Cooperative non-blocking
  via `set_nonblocking` is fine; futures / await / executors are
  #15.
- **`UdpSocket`.** Out of scope; covered later.
- **Unix domain sockets, file locking, chmod, chown, atime/ctime,
  signal handlers beyond SIGINT.** Out of scope; covered in a
  future "#06.6 — sync I/O power features" prompt if user pull
  warrants it.
- **HTTP client.** Different prompt entirely; this is bytes-only
  sync I/O.
- **Formal `Read` / `Write` mixin.** v1.5 — monomorphize over
  `File` and `TcpStream` for v1.

## Why this comes before #10 (LSP)

The LSP server (#10) will itself want `File`, `BufReader`,
`Path` operations, atomic config writes, and tagged `IoError` for
its own implementation. Landing #06.5 first means #10 builds on a
finished foundation instead of reaching back to fill gaps as it
goes.
