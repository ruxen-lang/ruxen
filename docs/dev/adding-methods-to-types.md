# Adding methods to types (prefer `.rx` extensions)

**Default rule: a new method on any type — including a primitive like
`Int`, `Float`, `Bool` — should be written in `.rx` stdlib source, not
hard-wired through the compiler and C runtime.**

Ruxen is compiled and statically dispatched (Rust-like), so it does *not*
box values into heap objects the way Ruby does. But it gives every type a
Ruby-like method surface: you can attach methods to primitives with an
`extension` block, and the compiler resolves and lowers them exactly like
methods on a user-defined class.

## The cheap path: `extension` in `.rx`

```rx
extension Int
  ## Double this integer.
  def double -> Int
    self + self
  end
end
```

This needs **no compiler change and no C function**. The pipeline already
handles it generically:

- **Resolve / typeck** — the `extension` block is an impl block; its
  methods register in the trait resolver's per-type method table keyed by
  the primitive's name (`typeck/mixins.rs::type_name` maps `Ty::Int` →
  `"Int"`). Method calls look this table up in
  `typeck/infer/collect.rs::resolve_method_call` (the
  `lookup_method_with_args` step) — *before* any "no method" error — so no
  `(Ty::Int, "name")` arm in `typeck/method_resolvers/mod.rs` is required.
- **MIR** — the `.rx` body lowers to a normal function named
  `Int_double` (`mir/lower/impl_block.rs`); the call site mangles to the
  same `Int_double` (`mir/lower/expr/method_call.rs`).
- **Codegen / link** — `codegen/lang_intrinsics.rs::runtime_name` passes
  `Int_double` through unchanged (`Ok(name)`); the linker binds it to the
  compiled `.rx` function. No `runtime_sigs.rs` ABI entry, no LLVM decl,
  no `library/std/**/runtime/*.c`.

## When the compiler/C layer *is* justified

Only for **irreducible primitive operations** that cannot be expressed in
`.rx` — the leaves the `.rx` methods bottom out on:

- A machine operation with no Ruxen spelling (e.g. an `Int`→`Float`
  bit/representation conversion, raw syscalls, allocation).
- Performance-critical kernels where a function-call-per-element in `.rx`
  is unacceptable.

These get a `ruxen_*` C helper **plus** matching entries in the C-ABI table
(`codegen/cranelift/runtime_sigs.rs`), the LLVM declarations
(`codegen/llvm/runtime_decl.rs`), and the symbol list
(`codegen/runtime/symbols/`). Keep that set **small and stable**; build the
ergonomic surface on top of it in `.rx`.

> Historical note: `Int.to_f` / `Float.to_i` / the universal `to_s` shipped
> via the C-helper path (`ruxen_int_to_f`, `ruxen_float_to_i`, the
> `ruxen_*_to_string` family) because that machinery already existed and a
> release was pending. They work and are pinned by tests; they are **not**
> the template for new methods. New conversions/helpers should be `.rx`
> `extension` methods. A future cleanup can make the numeric `as` cast a
> real codegen instruction (it is a no-op passthrough today at
> `mir/lower/expr/misc.rs`) and then re-express `to_f`/`to_i` in `.rx` as
> `self as Float` / `self as Int`, retiring the bespoke C helpers.
