# Capping `rivenc` memory (8 GiB)

`rivenc` is pre-1.0 and still leaks memory on pathological inputs. Any
automation that runs the compiler — CI jobs, fuzzers, test loops, the
Paperclip heartbeat runner — must run it under a hard RSS cap so a
leak-bound process can't take down the host.

## Why not `ulimit -v`?

On macOS, `ulimit -v` does not enforce an address-space cap the way it
does on Linux. Setting `ulimit -v $((8 * 1024 * 1024))` before invoking
`rivenc` is silently ineffective: `mmap`/`malloc` continues to succeed
past the limit and the process keeps growing until the OS OOM-kills it
(or the host swaps itself to death).

The portable workaround is to poll `ps -o rss=` on the child PID and
send `SIGKILL` when the resident set size crosses the threshold.

## Wrapper

`scripts/rivenc-rss-cap.sh` is a POSIX-ish Bash wrapper that:

- launches `rivenc` with the given args in the background,
- polls the child's RSS via `ps -o rss= -p <pid>` every
  `RIVENC_POLL_SEC` seconds (default `1`),
- sends `SIGKILL` and exits `137` if RSS exceeds `RIVENC_RSS_KIB`
  (default `8388608`, i.e. 8 GiB),
- otherwise waits for the child and propagates its exit code,
- forwards `SIGINT` / `SIGTERM` from the parent to the child.

### Usage

```bash
# Normal use — pass every argument through to rivenc.
scripts/rivenc-rss-cap.sh path/to/program.rvn -o /tmp/out

# Override the cap (KiB). Example: 2 GiB for a constrained runner.
RIVENC_RSS_KIB=2097152 scripts/rivenc-rss-cap.sh path/to/program.rvn

# Point at a non-PATH binary.
RIVENC_BIN=./target/debug/rivenc scripts/rivenc-rss-cap.sh program.rvn
```

### Exit codes

| Code  | Meaning                                                 |
| ----- | ------------------------------------------------------- |
| 0–125 | Propagated directly from `rivenc`.                      |
| 130   | Parent received `SIGINT` / `SIGTERM`; child was killed. |
| 137   | RSS cap exceeded; child was `SIGKILL`-ed by the wrapper.|

### When to use it

- **CI.** Any job that invokes `rivenc` on the full test fixture set
  or on user-submitted programs should go through the wrapper. An
  uncapped `rivenc` on a fuzz-shaped input has taken down 32 GiB
  runners in under a minute.
- **Local dev.** If you are iterating on codegen, the borrow checker,
  or anything that allocates per-expression, run `cargo run` (or the
  installed `rivenc`) under the wrapper. A stuck recursion will
  terminate in ~a minute at 8 GiB instead of freezing the laptop.
- **Paperclip heartbeats.** Any agent that shells out to `rivenc` as
  part of its work — including this CTO agent executing B4-era code —
  should invoke the wrapper, not raw `rivenc`. The cap is per-invocation,
  so a leaky compile does not poison the run's overall budget.

### What it does not do

- Enforce virtual memory / VSZ caps. The wrapper only watches RSS
  because that is what ps reports reliably on macOS. A job that
  `mmap`s a large file will appear smaller to the cap than it really
  is; that's fine for the leak-guard use case.
- Rate-limit CPU or wall-clock. Combine with your runner's own
  per-step timeout (GitHub Actions `timeout-minutes`, cron
  `timeout 30m …`) for that.
- Gracefully terminate. It is `SIGKILL` only — `rivenc` is
  compute-bound and typically will not respond to `SIGTERM` during a
  typecheck. If you want partial progress, land it as crash-only state
  (disk-backed incremental cache) rather than expecting clean shutdown.
