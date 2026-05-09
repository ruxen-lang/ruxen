# `Vec[T]` iterator borrow rules

This page documents how Riven v1 ensures iterator–mutator interleavings
on a `Vec[T]` are statically rejected. The contract is enforced by the
borrow checker against the receiver-mode of each method; it does not
require any runtime tag.

## Method receiver-mode summary

| Method            | Receiver mode | Rationale                                               |
|-------------------|---------------|---------------------------------------------------------|
| `iter`            | `&self`       | Hands out `&T` views. Concurrent immut borrows are fine. |
| `iter_mut`        | `&mut self`   | Hands out `&mut T`. Exclusive borrow blocks mutators.    |
| `into_iter`       | `self` (move) | Consumes the receiver entirely.                          |
| `as_slice`        | `&self`       | Same as `iter`.                                          |
| `push` / `pop`    | `&mut self`   | Mutator. Conflicts with any outstanding `&` or `&mut`.   |
| `insert` / `remove` | `&mut self` | Mutator.                                                 |
| `clear` / `truncate` / `swap` | `&mut self` | Mutator.                                       |
| `extend`          | `&mut self`   | Mutator.                                                 |
| `sort_by`         | `&mut self`   | Mutator (in-place sort).                                 |
| `dedup`           | `&mut self`   | Mutator.                                                 |
| `retain`          | `&mut self`   | Mutator.                                                 |
| `==` / `!=`       | `&self` x 2   | Read-only.                                               |
| `v[i]` (Index)    | `&self`       | Read-only.                                               |

## Why this is enough

The borrow checker is the single enforcement point: any program that
holds a live `iter` (or `iter_mut`) result while invoking a mutator on
the same `Vec` is rejected at compile time. This mirrors Rust's iterator
invalidation rule, but stated structurally rather than via runtime
versioning.

For the v1 runtime, the iterator types `VecIter[T]` /
`VecIntoIter[T]` / `VecMutIter[T]` are represented identically — each
is a `RivenVec*`. The compile-time receiver-mode contract is what
prevents conflicting access; the runtime does not need a generation
counter.

## Examples

```riven
let mut v: Vec[Int] = Vec.new
v.push(1)
v.push(2)

# OK: only an immut borrow is live across the loop.
for x in v.iter
  puts "#{x}"
end

# Rejected: `v.push` requires `&mut self`, but `it` holds `&self`.
let it = v.iter
v.push(3)        # ERROR: cannot borrow `v` as mutable while `it` lives
puts "#{it.next.unwrap_or(0)}"
```

```riven
# OK: into_iter consumes `v`. After the loop, `v` is gone — no
# mutator can be called.
let v: Vec[Int] = Vec.new
for x in v.into_iter
  puts "#{x}"
end
```

## Notes for implementers

- **`into_iter` taint**: the MIR-level analysis tags
  `riven_vec_from_iter` (and the planned `Vec_into_iter`) as a
  *consume-helper* in `compute_dealloc_safe_locals` so the source
  local does not double-free at scope exit. See
  `crates/riven-core/src/mir/lower.rs` (search
  `is_runtime_consume_helper`).

- **`push` ownership transfer**: when the element type owns heap
  (`Vec[String]`, `Vec[Vec[T]]`, …), the source temporary at the
  push site is tainted so the drop pass does not free it. The
  receiving slot inherits the responsibility through the
  per-element drop helper (`riven_vec_drop_string`,
  `riven_vec_drop_vec`).

- **Closure-takers** (`sort_by`, `retain`, `each`, `filter`,
  `find`, `position`, `map`, `partition`) are inlined at MIR
  lowering — they do not pass a function pointer to the runtime.
