# Spec — Zero-Rust for stdlib class additions

**Source docs:**
[docs/specs/codegen/ffi.spec.md](../codegen/ffi.spec.md),
[docs/specs/stdlib/async.spec.md](../stdlib/async.spec.md) + sub-phase 4A partial commit `42daaeb`.

**Status:** new — auto-connect cleanup absorbing the gaps surfaced
during the multithreading + async sub-phases. Five focused fixes
that together close the "trio leak": **adding a stdlib class or
method must require zero edits to `compiler/riven_core/src/` beyond a
single `BOOTSTRAP_FILES` entry.**

Three principles framing the spec:
- Language features (async/await, generators, parser additions) are
  compiler passes — they legitimately require Rust. NOT in scope.
- Stdlib class additions are not language features — they're data +
  FFI shells. They must NOT require Rust beyond the explicit
  bootstrap entry.
- Diagnostic codes are tolerated as one-line registry additions —
  not worth deriving.

---

## B1 — Bootstrap-merge resolves class bodies

`compiler/riven_core/src/resolve/bootstrap_merge.rs::resolve_with_bootstrap`
currently runs `resolve_item` only over the user program. Bootstrap
programs only get `register_top_level_type_with_ffi` (type + lib-decl
registration). User-body methods on bootstrap-loaded classes (`def
init`, `def var poll`, plain method bodies) are silently dropped.

**Fix:** run `resolve_item` over each bootstrap program in load
order, against the cumulative bootstrap symbol table (so a later
package can reference an earlier one). Then run it over the user
program last.

**Given** `library/std/time/src/lib.rvn` containing:
```rvn
class TimeSleepFuture
  remaining_nanos: Int
  handle: Int
  include Future
  type Output = ()

  def init(d: &Duration)
    self.remaining_nanos = d.as_nanos
    self.handle = 0
  end

  def var poll(cx: &var Context) -> Poll[()]
    # user-written Riven body that calls FFI helpers
    ...
  end

  def drop
    if self.handle != 0
      reactor_deregister(self.handle)
    end
  end
end
```
**Then** typeck reports the class as having all three user-body
methods. Running `TimeSleepFuture.new(d).poll(&var ctx)` actually
executes the user-written `def var poll` body, not a no-op.

Pre-fix: those bodies are silently dropped during bootstrap merge —
calls type-check but never execute the user-written code.

## B2 — Implicit transitive `Send` / `Sync` auto-derive

`compiler/riven_core/src/hir/types.rs::is_send_strict_with` currently
has hardcoded match arms:
```rust
match class_name {
    "Array" | "Option" | "Result" | "Box" | "HashMap" | "HashSet" => /* iff all generic params are Send */,
    "Mutex" | "SharedSync" => /* iff T: Send */,
    "AtomicI64" | "AtomicBool" | "AtomicUsize" => /* always Send + Sync */,
    "MutexGuard" | "ReadGuard" | "WriteGuard" => /* never Send */,
    "Sender" | "Receiver" | "JoinHandle" => /* always Send */,
    ...
}
```

Every new generic container requires editing this match.

**Fix:** replace the match with a generic walker.
1. If the class body has `include !Send` → not Send. (Escape hatch.)
2. If the class body has `include unsafe Send` → unconditionally Send. (Manual override.)
3. If the class body has `include Send` →
   - **Send iff** every generic type parameter `T` of the class
     satisfies `T: Send` AND every field type satisfies `Send`.
   - The recursion is the existing `is_send_strict_with` walker —
     it already traverses field sets; just generalise the trigger from
     hardcoded class names to "this class has `include Send`."
4. Same rules for `Sync`.

**Given** `class Foo[T]; include Send; include Sync; bar: T; baz: Int; end`
**Then** `Foo[Int]: Send + Sync` (because Int satisfies both, and
all fields satisfy both); `Foo[BadType]: !Send` if `BadType: !Send`.

**Given** the SAME definition with `include !Send` added
**Then** `Foo[anything]: !Send` (escape hatch wins).

## B3 — `[system_libs]` in `Riven.toml`

`compiler/riven_core/src/codegen/object.rs::linker_args` currently
hardcodes:
```rust
"-lc", "-lm", "-lpthread"
```

New stdlib package needing `-lssl`, `-lz`, `-lcurl` requires editing
this Rust function.

**Fix:** each package's `Riven.toml` may carry a `[system_libs]`
table:
```toml
[system_libs]
libs = ["pthread", "c", "m"]
```
The linker walks every loaded package's toml, collects the union of
`libs`, deduplicates, and emits `-l<name>` per entry. Order doesn't
matter (linker resolves transitive references).

`-lc` and `-lm` move from `linker_args` hardcoded into `std-core`'s
toml. `-lpthread` moves into `std-sync`'s toml. Sanitizer flags
stay in code (they're not package-specific).

**Given** a new package `library/std/crypto/Riven.toml` with
`[system_libs] libs = ["ssl", "crypto"]`
**Then** compiled programs linking against std-crypto get `-lssl`
`-lcrypto` automatically without touching `codegen/object.rs`.

## B4 — Sweep `def __drop` → `def drop` in stdlib

`compiler/riven_core/src/mir/lower.rs::collect_user_drop_classes`
matches the literal method name `drop` (no double-underscore).
`library/std/sync/src/lib.rvn` declares lib decls as `def __drop as
"..."`. The collector silently skips them — every Mutex / SharedSync /
MutexGuard / AtomicI64 / AtomicBool / AtomicUsize / Sender / Receiver /
JoinHandle in user code is leaking its C heap.

**Fix:** sweep across `library/std/sync/src/lib.rvn` (and any other
`.rvn` that uses the `__drop` form). Change `def __drop as "..."`
to `def drop as "..."`. Both forms map to the same C symbol; only
the Riven-side method name changes.

**Pin:** `grep -rn "def __drop" library/std/*/src/` returns empty
after the fix.

**Sanity:** a focused leak test (e.g. `Mutex.new(0)` in a loop with
RSS sampling, OR an explicit count of `riven_mutex_drop` invocations
via a counter) confirms drop fires post-fix.

## B5 — End-to-end pin test

The trio-leak detector. Add a fresh dummy package
`library/std/foobar/` containing:

- `Riven.toml` with deps + `[system_libs] libs = []`.
- `src/lib.rvn`:
  ```rvn
  class FooBar[T]
    payload: T
    include Send
    include Sync

    def init(value: T)
      self.payload = value
    end

    def get(self) -> T
      self.payload
    end

    lib "runtime/foobar.c"
      def drop as "riven_foobar_drop"(self) -> ()
    end
  end
  ```
- `runtime/foobar.c` with `riven_foobar_drop`.
- **One entry** in `BOOTSTRAP_FILES` (the explicit allowed exception).

Pin test (e2e or Rust-side):
- `FooBar.new(42).get()` returns 42 (dispatch works).
- `let _ = FooBar.new(42)` increments a drop counter (drop fires).
- `FooBar[Int]: Send + Sync` (auto-derive works transitively because
  Int satisfies both).
- `FooBar[NotSendType]: !Send` (negative transitivity).
- Verify the diff `git log -1 -- compiler/riven_core/src/` between
  HEAD and HEAD~1 (where HEAD adds FooBar) contains ONLY the
  BOOTSTRAP_FILES line addition. Nothing else in `compiler/`.

The last assertion is the test that closes the trio-leak structurally.

---

## Pin tests

| Behaviour | Test fn                                              | File                               |
|-----------|------------------------------------------------------|------------------------------------|
| B1        | `bootstrap_class_body_def_init_resolves`             | `tests/bootstrap_class_bodies.rs`  |
| B1        | `bootstrap_class_body_def_var_poll_resolves`         | `tests/bootstrap_class_bodies.rs`  |
| B2        | `transitive_send_iff_all_generic_params_send`        | `tests/auto_derive_send_sync.rs`   |
| B2        | `transitive_sync_iff_all_generic_params_sync`        | `tests/auto_derive_send_sync.rs`   |
| B2        | `include_negative_send_overrides_transitive`         | `tests/auto_derive_send_sync.rs`   |
| B2        | `include_unsafe_send_overrides_transitive`           | `tests/auto_derive_send_sync.rs`   |
| B3        | `system_libs_aggregate_from_riven_tomls`             | `tests/linker_system_libs.rs`      |
| B4        | `no_double_underscore_drop_remains_in_stdlib`        | `tests/drop_name_sweep.rs`         |
| B4        | `mutex_drop_fires_count_pin`                         | `tests/drop_name_sweep.rs`         |
| B5        | e2e `cases/750_foobar_zero_rust_pin.rvn`             | release-e2e                        |
| B5        | `foobar_addition_touches_only_bootstrap_files`       | `tests/trio_leak_pin.rs`           |

---

## Out of scope

- **`BOOTSTRAP_FILES` auto-discovery.** Per project decision, the
  hand-maintained ordered list IS the value. One Rust line per
  package is acceptable.
- **Error code declarations.** One line in `codes.rs` + an
  `Exxxx.md` per code is fine; deriving these from elsewhere is
  low-value.
- **Language features.** async/await/yield/traits/lifetimes are
  compiler passes. Out of scope for this auto-connect cleanup.
