# Spec — `std.io` BufReader / BufWriter

**Status:** shipped Phase 2 #06.5 T6 — generic buffered wrappers over
the closed set `{File, TcpStream}`.

v1 simplification: no formal `Read` / `Write` mixin. The runtime carves
a 1-byte `kind` tag into the wrapper spine and branches on it for every
fill / flush. `BufReader` / `BufWriter` are modules with one class per
inner type; calling a method on the module itself (`BufReader.new`)
raises **E0714**, which names the classes. The formal mixin story is
deferred to v1.5.

## Surface

The shipped surface (`library/std/bufio/src/lib.rx`) is one module per
direction with a class per inner type, sharing the methods through a
mixin:

```ruxen
module BufReader
  mixin Reader
    def read_line -> Result[String?, IoError]               # nil at EOF
    def read(buf: &var Array[Int]) -> Result[Int, IoError]
  end

  class File        # and the parallel `class Tcp` over TcpStream
    include Reader
    def self.new(inner: File) -> BufReader.File
    def self.with_capacity(cap: Int, inner: File) -> BufReader.File   # 8 KiB default
    def into_inner -> File                                  # surrenders the inner, frees buf
  end
end

module BufWriter
  mixin Writer
    def write(buf: &Array[Int]) -> Result[Int, IoError]
    def write_all(buf: &Array[Int]) -> Result[nil, IoError]
    def write_str(s: &String) -> Result[nil, IoError]
    def flush -> Result[nil, IoError]                       # required for guaranteed persistence
  end

  class File        # and the parallel `class Tcp` over TcpStream
    include Writer
    def self.new(inner: File) -> BufWriter.File
    def self.with_capacity(cap: Int, inner: File) -> BufWriter.File
    def into_inner -> File                                  # flush then surrender
  end
end
```

- The inner is restricted to `File` or `TcpStream` by the `File` and
  `Tcp` classes' declared constructor parameters. `BufReader.new(..)`
  on the module raises **E0714**.
- `BufReader#each_line` is deferred to v1.5.

## Semantics

- Default capacity: 8 KiB.
- `with_capacity(0, inner)` rounds up to 1 to keep `fill_buf` from
  spinning. Negative caps are also rounded to 1.
- `read_line` returns `Ok(Some(line))` where `line` includes the
  trailing `?\n` if one was seen, or just the remaining bytes at EOF.
  Returns `Ok(nil)` at true EOF (no bytes read). `Err` on real I/O
  failure.
- `read(buf)` pulls the next chunk from the inner (refilling if empty),
  then drains the buffered window into `buf` as one byte per slot —
  same wire shape as `File.read` / `TcpStream.read`. Returns `Ok(0)`
  at EOF.
- `into_inner` (reader) surrenders the inner pointer and frees the
  byte buffer immediately. `closed` flag is set so the scope-exit
  drop is a no-op.
- `into_inner` (writer) flushes first; on flush failure returns
  `Err(IoError.*)` and does NOT surrender (the wrapper's drop will
  re-attempt the flush — best-effort). On success surrenders the
  inner pointer wrapped in `Ok`.
- `flush` and `write_*` errors return `Err(IoError.*)`. EPIPE on TCP
  side surfaces as `IoError.BrokenPipe` (the per-socket SO_NOSIGPIPE /
  MSG_NOSIGNAL machinery in `net/tcp.c` keeps a write to a dead peer
  from killing the process).

## Drop semantics

Both classes participate in the user_drop_classes pipeline (see
`mir/lower/collect.rs::collect_user_drop_classes`). Scope exit emits
`<Type>_drop(p) + ruxen_dealloc(p)`:

- `ruxen_bufreader_drop` frees the 8 KiB byte buffer only. The inner
  `File` / `TcpStream` has its OWN drop helper that runs in the same
  scope-exit pass — bufio's drop intentionally does NOT close the
  inner.
- `ruxen_bufwriter_drop` does a best-effort flush of pending bytes,
  then frees the byte buffer. Flush errors are swallowed (drop has
  nowhere to surface them). **The drop-time flush is a safety net,
  not a guarantee**: depending on scope-exit drop ordering, the inner
  File / TcpStream's own drop may close the fd before the BufWriter's
  drop runs — in that case the flush silently fails. Callers who
  require persistence MUST call `.flush()` or `.into_inner()`
  explicitly before the BufWriter goes out of scope.

## Wire layout (32 bytes — same shape for reader and writer)

```
+0   uint8  kind     0 = File, 1 = TcpStream
+1   uint8  closed   1 once into_inner / drop has emptied the buffer
+2   uint16 _pad     reserved (zeroed)
+4   uint32 cap      capacity in bytes of buf
+8   uint32 pos      reader: next byte to return; writer: unused
+12  uint32 filled   reader: bytes valid in buf; writer: bytes pending
+16  uint8* buf      malloc(cap)
+24  void*  inner    borrowed RuxenFile* / RuxenTcpStream*; not owned
```

Wire layout pinned by `_Static_assert(sizeof(RuxenBufReader) == 32, …)`
in `library/std/io/runtime/bufio.c`. If the assertion drifts the runtime
won't link — that's the design-level pin for this spine.

## Runtime symbol routing

The constructors carry a `_file` / `_tcp` suffix picked at MIR
lowering from the inner argument's type:

| Mangled name                          | Runtime symbol                            |
|---------------------------------------|-------------------------------------------|
| `BufReader_new_file`                  | `ruxen_bufreader_new_file`                |
| `BufReader_new_tcp`                   | `ruxen_bufreader_new_tcp`                 |
| `BufReader_with_capacity_file`        | `ruxen_bufreader_with_capacity_file`      |
| `BufReader_with_capacity_tcp`         | `ruxen_bufreader_with_capacity_tcp`       |
| `BufReader_read_line`                 | `ruxen_bufreader_read_line`               |
| `BufReader_read`                      | `ruxen_bufreader_read`                    |
| `BufReader_into_inner_file`           | `ruxen_bufreader_into_inner_file`         |
| `BufReader_into_inner_tcp`            | `ruxen_bufreader_into_inner_tcp`          |
| `BufReader_drop`                      | `ruxen_bufreader_drop`                    |
| (and the parallel `BufWriter_*` set)  | (and `ruxen_bufwriter_*`)                 |

The instance method `into_inner` is suffix-picked from the receiver's
`generic_args[0]` in `mir/lower/expr/method_call.rs`. Other instance
methods (`read_line`, `read`, `write*`, `flush`) carry no suffix —
they branch on the 1-byte `kind` tag inside the runtime spine.

## Out of scope (v1)

- No generic `Read` / `Write` mixin over arbitrary inners — deferred
  to v1.5.
- No `BufReader#each_line` — deferred to v1.5.
- No `into_inner` error-recovery type carrying both the error and the
  BufWriter so the caller can retry. The BufWriter's own drop
  re-attempts the flush on a best-effort basis.
- No dynamic `any Reader` dispatch — the closed-set monomorphisation
  is the only dispatch path.
