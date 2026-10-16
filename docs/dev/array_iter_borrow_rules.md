# `Array[T]` borrow rules during iteration

This page documents how Ruxen v1 statically rejects mutating an
`Array[T]` while a borrow of it is live. The contract is enforced by the
borrow checker against the receiver mode of each method; it does not
require any runtime tag.

Iteration is Ruby-style: `for x in v` or `v.each do |x| … end`. The old
iterator layer (`iter`, `iter_var`, `into_iter`, `collect`,
`ArrayIter[T]`) was removed from the stdlib and is not part of the
language.

## Method receiver-mode summary

| Method            | Receiver mode | Rationale                                               |
|-------------------|---------------|---------------------------------------------------------|
| `each`, `for x in v` | borrow (`def`) | Reads elements. Concurrent shared borrows are fine.  |
| `push` / `pop`    | `def var`     | Mutator. Conflicts with any outstanding `&` or `&var`.   |
| `insert` / `remove` | `def var`   | Mutator.                                                 |
| `clear` / `truncate` / `swap` | `def var` | Mutator.                                         |
| `extend`          | `def var`     | Mutator.                                                 |
| `sort_by` / `sort!` | `def var`   | Mutator (in-place sort).                                 |
| `dedup`           | `def var`     | Mutator.                                                 |
| `==` / `!=`       | borrow (`def`) on both sides | Read-only.                                |
| `v[i]` (Index)    | borrow (`def`) | Read-only.                                              |

## Why this is enough

The borrow checker is the single enforcement point: any program that
holds a live borrow of an `Array` while invoking a mutator on the same
`Array` is rejected at compile time (E1002). The rule is stated
structurally rather than via runtime versioning, so the runtime does not
need a generation counter.

## Examples

```ruxen
var v: Array[Int] = Array.new
v.push(1)
v.push(2)

# OK: only a shared borrow is live across the loop.
for x in v
  puts "#{x}"
end

# Rejected: `v.push` needs `&var v`, but `r` holds `&v`.
let r = &v
v.push(3)        # E1002: cannot borrow `v` as mutable while `r` lives
puts "#{r.size}"
```

## Notes for implementers

- **Consume helpers**: the MIR-level drop analysis tags runtime
  helpers that take ownership of their argument (for example
  `ruxen_vec_from_iter`) as *consume helpers* in
  `compute_dealloc_safe_locals`, so the source local does not
  double-free at scope exit. See
  `compiler/ruxen_core/src/mir/lower/drops.rs` and the ownership table
  in `compiler/ruxen_core/src/mir/lower/runtime_abi.rs`.

- **`push` ownership transfer**: when the element type owns heap
  (`Array[String]`, `Array[Array[T]]`, …), the source temporary at the
  push site is tainted so the drop pass does not free it. The
  receiving slot inherits the responsibility through the
  element-aware drop glue (`ruxen_vec_drop` with the element
  descriptor).

- **Closure-takers** (`each`, `filter`, `find`, `position`, `map`,
  `partition`) are inlined at MIR lowering — they do not pass a
  function pointer to the runtime.
