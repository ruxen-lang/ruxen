# Unsafe Code

The compiler enforces Ruxen's safety guarantees by default. Sometimes you know something the compiler can't prove — "this pointer is valid here," "this cast is fine because I just checked the tag." The **`unsafe`** keyword is how you tell the compiler "trust me, I've checked." It doesn't turn off the type system; it unlocks a small, specific set of operations that the compiler can't verify on its own.

`unsafe` should be rare. The standard pattern is: write a small unsafe block, wrap it in a safe public API, and never let `unsafe` leak into callers.

## A first runnable example

```ruxen
lib "m"
  def sqrt(x: Float) -> Float
end

def main
  let r = unsafe { sqrt(16.0) }
  puts "sqrt(16) = #{r}"
end
```

```bash
ruxen compile sqrt_demo.rx
./sqrt_demo
```

Output:

```
sqrt(16) = 4
```

Calling a C function is unsafe because the compiler can't verify what the C side does with its arguments. Wrapping the call in `unsafe { ... }` is your acknowledgement.

## What requires `unsafe`

A short list — these are the operations where the compiler can't promise safety:

- Dereferencing a raw pointer (`*T`, `*var T`).
- Calling an FFI function (anything declared in a `lib "..."` block).
- Accessing writable global state.
- Performing an unchecked type cast.

Everything else — regular method calls, normal references, normal arithmetic — is safe. You don't need `unsafe` for the day-to-day.

## `unsafe` blocks

```ruxen
let ptr: *Int = get_raw_pointer()
unsafe
  let value = *ptr      # dereferencing a raw pointer
end
```

Inside the block, you can do the unsafe operations. Outside, you can't. Keep the block small — the smaller the unsafe region, the easier it is to audit.

## The `!` convention — *not* unsafe, just loud

Methods ending in `!` — `unwrap!`, `expect!`, `panic!` — can **panic** at runtime. They're not `unsafe`: they don't break memory safety. The `!` is a naming convention: "this is safe code that might crash if its precondition fails."

```ruxen
let value = option.unwrap!           # safe code; panics on nil
let value = result.expect!("oops")   # safe code; panics on Err
```

Use sparingly outside tests.

## The idiomatic pattern: safe wrappers around unsafe internals

The right way to use `unsafe` is to confine it inside a type's implementation and expose a safe API. The caller never writes `unsafe`:

```ruxen
class SafeBuffer
  ptr: *var UInt8
  len: Int

  def init(size: Int)
    unsafe
      self.ptr = malloc(size) as *var UInt8
      self.size = size
    end
  end

  # Safe public API — bounds-checked, returns Option
  def get(index: Int) -> Option[UInt8]
    if index < 0 || index >= self.size
      nil
    else
      unsafe
        Some(*(self.ptr + index))
      end
    end
  end

  include Drop

  def var drop
    unsafe
      free(self.ptr as *var Void)
    end
  end
end
```

Two things to notice:

- The `def init` and `def get` both have `unsafe` blocks scoped to exactly the pointer operations — nothing more.
- `include Drop` plus `def var drop` is how you attach cleanup logic that runs when the value goes out of scope. The compiler calls it automatically — that's how `SafeBuffer` avoids leaking memory.

A caller uses `SafeBuffer` like any other class:

```ruxen
let buf = SafeBuffer.new(64)
match buf.get(0)
  Some(b) -> puts "first byte = #{b}"
  nil     -> puts "out of range"
end
```

No `unsafe` in sight at the call site.

## Common mistakes

**Wrapping too much code in `unsafe`.** Keep `unsafe` blocks as small as possible — ideally one expression. A big `unsafe` block is hard to audit.

**Using `unsafe` to "make the compiler stop complaining."** If you're using it to silence an error you don't fully understand, stop and read the error. Almost always there's a safe fix.

**Forgetting `Drop` on a class that allocates.** Without it, every instance leaks. Include `Drop` and define `def var drop` to release whatever your unsafe code acquired.

**Returning a raw pointer from a safe API.** If your method returns `*T`, callers will need `unsafe` to use it — that defeats the wrapper. Return a safe abstraction instead.

## Try it

Take the `SafeBuffer` example and add a `set(index: Int, value: UInt8) -> Result[nil, &str]` method. Bounds-check; on success, write through the pointer in an `unsafe` block. Use it from `main` and try writing to an out-of-range index — the `Result` should report the error without crashing.

## Recap

- `unsafe` unlocks a small set of operations the compiler can't verify: raw pointer dereference, FFI calls, writable globals, unchecked casts.
- Keep `unsafe` blocks small. Wrap them in a safe public API.
- `!` (`unwrap!`, `expect!`) is a panic warning — not `unsafe`.
- Classes that allocate should `include Drop` and define `def var drop` for cleanup.
- Calling C functions through `lib "..."` is the everyday source of `unsafe` — see [Chapter 14](14-ffi.md).

**Next:** [Formatting and Comments](16-formatting-and-comments.md) — `ruxen fmt`, doc comments, and naming conventions.
