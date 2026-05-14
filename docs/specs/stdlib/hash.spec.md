# Spec — `Hashable` (a.k.a. `std.hash`)

**Source docs:**
[docs/requirements/tier1_01_stdlib.md §2.3](../../requirements/tier1_01_stdlib.md),
[docs/requirements/tier1_05_derive_macros.md](../../requirements/tier1_05_derive_macros.md).

**Status:** shipped Phase 2 #04-#05 (trait + derive + HashMap key
wiring).  Spec backfilled here so the v1 hash surface has a single
source of truth.

Riven's hashing surface is intentionally thin.  There is one trait —
`Hashable` (CEO ruling TEC-13: renamed from `Hash` to keep the noun
slot free for the type) — exposing one method that returns a single
`Int`.  No `Hasher` builder, no `BuildHasher`, no DOS-resistant
random seed.  Custom hashers and incremental builders are explicit
v2 work; v1 ships exactly enough for `HashMap[K, V]` and
`HashSet[T]` to key on user types via `derive Hashable`.

---

## B1 — The `Hashable` trait

```riven
trait Hashable
  def hash_code(self: &Self) -> Int
end
```

**Given** any value `x: T` where `T: Hashable`
**When** the program calls `x.hash_code`
**Then** the result is a deterministic `Int` that depends only on
the bytes of `x` reachable through `&T`.

The trait is registered as a built-in by `Resolver::register_builtins`
(see `crates/riven-core/src/resolve/mod.rs:179-186`); user code
needs no `import` to refer to it.

## B2 — Primitive keys hash via the runtime, not a user-callable method

`Int`, `Int8`/`Int16`/`Int32`/`Int64`, `UInt8`/`UInt16`/`UInt32`/
`UInt64`, `USize`/`ISize`, `Bool`, `Char`, `String`, and `&str` are
all valid `HashMap` keys / `HashSet` elements: the runtime hashes
them with a stable 64-bit FNV-style mix
(`runtime/runtime.c::riven_hash_bits` for ints, `riven_hash_str` for
strings; bool / char widen into `riven_hash_bits`).

In the v1 surface this is a runtime-internal capability — there is
no user-callable `42.hash_code` method on primitives, and a generic
function with a `T: Hashable` bound does not yet monomorphise for
primitive `T`.  Generic dispatch works against user types that
opt in via `derive Hashable` (covered by B5); the primitive case
is a v2 follow-up that needs primitive-Hashable impls and method
mangling support (`Int_hash_code`, `String_hash_code`) wired
through `runtime_name`.

`Float` / `Float32` / `Float64` are **not** valid HashMap keys —
NaN ≠ NaN breaks the equality contract `Hashable` shares with `Eq`.
`ty_is_valid_hash_key` rejects them.

`Vec[T]`, `HashMap[K, V]`, and `HashSet[T]` are also explicitly
non-keys; aggregating container hashes belongs to v2 once a
`Hasher` builder exists.

## B3 — `derive Hashable` on user aggregates

`@[derive(Hashable)]` on a `struct` / `class` / `enum` synthesises
`hash_code` by hashing each field in declaration order and folding
the partial hashes through a `combine` mix.  Every field's type
must itself be `Hashable`; the per-field validator at
`crates/riven-core/src/derive/validator.rs::validate_per_field_traits`
emits **E0615** when any field violates the bound (e.g. a struct
field of type `Vec[Int]` rejects `derive Hashable` because `Vec`
isn't `Hashable`).

```riven
struct Score
  player: String
  v: Int
  derive Hashable
end

let s = Score.new("alice", 42)
puts "#{s.hash_code}"   # prints a stable Int
```

## B4 — `HashMap[K, V]` and `HashSet[T]` require `K: Hashable`

The container constructors propagate the bound: `HashMap[K, V]::new`
where `K` is non-`Hashable` is rejected at type-check time.  This
check is rooted at the type-construction site
(`resolve/mod.rs::ty_is_valid_hash_key`) so a bad `K` is caught
before any insertion call.

The key path inside the runtime
(`runtime.c::riven_hash_key_hash`) routes string keys through
`riven_hash_str` and every other key through `riven_hash_bits`,
mirroring the trait dispatch the front-end performs.

## B5 — `T: Hashable` bound dispatches `hash_code`

Any generic function with the bound resolves `.hash_code` against
the trait method, even when the receiver type is a user struct
that gained the impl through `derive`:

```riven
def hash_it[T: Hashable](a: &T) -> Int
  a.hash_code
end
```

This is the exact pattern pin-tested by
`derive_hashable_dispatches_through_trait_bounds` in
`tests/derive_trait_dispatch.rs`.

## B6 — Hash code semantics

- Deterministic across runs of the same compiled binary on the same
  architecture.
- 64-bit `Int` width on every supported target.
- **Not** DOS-resistant (no random seed).  An attacker who controls
  the keys inserted into a `HashMap` can force pathological bucket
  distributions.  v2 will gain a randomly seeded default hasher;
  for v1, server code that hashes attacker-controlled input should
  layer its own keyed cryptographic hash on top.
- Stable across minor compiler versions within v1.0.x.  The
  underlying mix function is allowed to change in 1.1.x; users
  must not persist hash codes across compiler upgrades.

---

## Pin tests

| Behaviour                                           | Test fn                                               | File                                         |
|-----------------------------------------------------|-------------------------------------------------------|----------------------------------------------|
| B1 / B5 — bound dispatches `hash_code` on derived user types | `derive_hashable_dispatches_through_trait_bounds`     | `tests/derive_trait_dispatch.rs`             |
| B3 — derive rejects non-Hashable field (E0615)      | `derive_hash_on_struct_with_non_hash_field_emits_e0615` | `tests/derive_negatives.rs` (`NotHashable` fixture) |
| B2 — primitive `String` / `Int` hash via runtime    | covered indirectly via `HashMap[String, V]` / `HashMap[Int, V]` insert + lookup pin tests | `tests/hashmap_*.rs`                         |
| B2 v2 follow-up — `T: Hashable` dispatch for primitive `T` | `primitive_int_and_string_dispatch_through_hashable_bound` (`#[ignore]` pending v2) | `tests/derive_trait_dispatch.rs`             |
| B4 — `HashMap[K, V]` rejects non-Hashable K         | covered by `ty_is_valid_hash_key` rejection diags     | `tests/hashmap_*.rs`                         |

The B6 stability claims are attested by the v1 release checklist
rather than runtime tests; persisting hash codes is not a supported
use case so a regression test would be a costly noise generator.

---

## Out of scope (v2)

- User-callable `.hash_code` on primitives (`42.hash_code`,
  `"x".hash_code`) and the corresponding monomorphisation of a
  `T: Hashable` bound for primitive `T`.  The runtime already
  hashes primitives for `HashMap` keys; v2 will expose the same
  bit-mix as a method via `Int_hash_code` / `String_hash_code`
  shims wired through `runtime_name`.
- `Hasher` trait — incremental hash builder (`update(&[u8])`,
  `finish() -> Int`).  v1's `hash_code` is one-shot.
- `BuildHasher` / pluggable hash strategies on `HashMap`.
- DOS-resistant default hasher with random seed.
- `Hashable` impl for `Vec[T]` / `HashMap[K, V]` / `HashSet[T]`
  (needs `Hasher` to fold per-element hashes deterministically).
- `Hashable` impl for `Float*` (deferred until the NaN-equality
  question has a v2 ruling).
- `&[u8]` / `&[T]` slice hashing — needs slice types as a
  first-class surface, also v2.
- `hash!{...}` macro form — tutorial 13 mentions it; formal macros
  land later.

---

## Cross-references

- `HashMap[K, V]` surface: [hashmap.spec.md](hashmap.spec.md).
- `HashSet[T]` surface: [hashset.spec.md](hashset.spec.md).
- Derive validator that rejects non-Hashable fields: see the
  E0615 entry in `docs/errors/`.
