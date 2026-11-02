# Unsafe Code

The compiler enforces Ruxen's safety guarantees by default. Sometimes you know something the compiler can't prove — "this pointer is valid here," "this C function won't keep the buffer." The **`unsafe ... end`** block is how you mark that code: "trust me, I've checked." It doesn't turn off the type system; it marks a small, specific set of operations that the compiler can't verify on its own.

`unsafe` should be rare. The standard pattern is: write a small unsafe block, wrap it in a safe public API, and never let `unsafe` leak into callers. Whether `unsafe` blocks stay, or raw pointers are limited to `lib` blocks, is still an open question [planned](../../TODO.md#t-oq).

## A first runnable example

Save this as `sqrt_demo.rx` [implemented]:

```ruby
lib "m"
  def sqrt(x: Float) -> Float
end

r = unsafe
  sqrt(16.0)
end
puts "sqrt(16) = #{r}"
```

```bash
ruxen compile sqrt_demo.rx
./sqrt_demo
```

Output:

```text
sqrt(16) = 4
```

Calling a C function is unsafe because the compiler can't verify what the C side does with its arguments. Wrapping the call in `unsafe ... end` is your acknowledgement. The block is an expression: its last value is the block's value. The brace form `unsafe { ... }` does not parse.

## What belongs in `unsafe`

A short list — these are the operations where the compiler can't promise safety:

- Dereferencing a raw pointer (`*T`, `*var T`).
- Calling an FFI function (anything declared in a `lib "..."` block).
- Accessing writable global state.
- Performing an unchecked type cast.

Everything else — regular method calls, normal references, normal arithmetic — is safe. You don't need `unsafe` for the day-to-day.

The compiler does not check today that these operations sit inside an `unsafe` block, so the block is a marker for readers and auditors. Dereferencing a raw pointer with `*p` does not type-check yet either (the result keeps the pointer type, a [known issue](../../TODO.md#t-raw-deref)), so read and write C memory through C functions for now.

## `unsafe` blocks

[implemented]:

```ruby
lib "c"
  def strlen(s: &String) -> Int    # C receives a pointer to the bytes
end

def c_length(s: &String) -> Int
  unsafe
    strlen(s)             # a C call the compiler can't check
  end
end

puts c_length("hello")
```

Inside the block, you do the unsafe operations. Keep the block small — the smaller the unsafe region, the easier it is to audit.

## The `!` convention — *not* unsafe, just loud

Methods ending in `!` may do something surprising: in Ruby, `save!` raises where `save` returns `false`. They're not `unsafe`: they don't break memory safety. `unwrap!` and `expect!` panic when the value is missing; they work today and are [deprecated] (replace with `x || default` and `x or raise ...`, both [planned](../../TODO.md#t-truthiness)):

```ruby
def demo
  let option: Int? = 5
  value = option.unwrap!               # safe code; panics on nil
  other = option.expect!("oops")       # safe code; panics with this message on nil
  puts value + other
end

demo()
```

Use sparingly outside tests.

## The idiomatic pattern: safe wrappers around unsafe internals

The right way to use `unsafe` is to confine it inside a type's implementation and expose a safe API. The caller never writes `unsafe`. Works today, with the constructor spelled `def init` [deprecated] (replace with `def initialize` [planned](../../TODO.md#t-initialize)):

```ruby
lib "c"
  def malloc(n: Int) -> *var UInt8
  def free(p: *var UInt8) -> nil
  def memset(p: *var UInt8, c: Int32, n: Int) -> *var UInt8
end

class SafeBuffer
  ptr: *var UInt8
  size: Int

  def init(size: Int)
    unsafe
      @ptr = malloc(size)
    end
    @size = size
  end

  # Safe public API — checks the pointer before touching it
  def var fill(byte: Int32) -> Bool
    return false if @ptr == nil
    unsafe
      memset(@ptr, byte, @size)
    end
    true
  end

  def drop
    unsafe
      free(@ptr)
    end
  end
end

buf = SafeBuffer.new(64)
puts buf.fill(0i32)      # true
puts buf.size            # 64
```

Two things to notice:

- `init` and `fill` both have `unsafe` blocks scoped to exactly the C calls — nothing more.
- `def drop` is how you attach cleanup logic that runs when the value goes out of scope. The compiler calls it automatically — that's how `SafeBuffer` avoids leaking memory. Older code also writes `include Drop` before it; the `def drop` alone is enough.

A caller uses `SafeBuffer` like any other class: `SafeBuffer.new(64)`, then `buf.fill(0i32)`. No `unsafe` in sight at the call site.

## Common mistakes

**Wrapping too much code in `unsafe`.** Keep `unsafe` blocks as small as possible — ideally one expression. A big `unsafe` block is hard to audit.

**Using `unsafe` to "make the compiler stop complaining."** If you're using it to silence an error you don't fully understand, stop and read the error. Almost always there's a safe fix.

**Forgetting `drop` on a class that allocates.** Without it, every instance leaks. Define `def drop` to release whatever your unsafe code acquired.

**Returning a raw pointer from a safe API.** If your method returns `*T`, callers will need `unsafe` to use it — that defeats the wrapper. Return a safe abstraction instead.

## Try it

Take the `SafeBuffer` example and add a `zero?` method that returns `true` when the pointer is `nil`. Then add a `copy_from(src: &String)` method that bounds-checks `src.size` against `@size` and, on success, copies the bytes with the C function `memcpy` inside an `unsafe` block.

## Recap

- `unsafe ... end` marks a small set of operations the compiler can't verify: raw pointer dereference, FFI calls, writable globals, unchecked casts.
- Keep `unsafe` blocks small. Wrap them in a safe public API.
- `!` methods are loud, not `unsafe`; `unwrap!` and `expect!` are deprecated.
- Classes that allocate should define `def drop` for cleanup.
- Calling C functions through `lib "..."` is the everyday source of `unsafe` — see [Chapter 14](14-ffi.md).

**Next:** [Formatting and Comments](16-formatting-and-comments.md) — `ruxen fmt`, doc comments, and naming conventions.
