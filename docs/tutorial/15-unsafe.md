# Unsafe Code

Ruxen's safety guarantees are enforced by default (P1 — Implicit Safety, Explicit Danger). The `unsafe` keyword opts into operations that the compiler cannot verify.

## What Requires Unsafe

- Dereferencing raw pointers (`*T`, `*var T`)
- Calling FFI functions
- Accessing writable global state
- Performing unchecked type casts

## Unsafe Blocks

```ruxen
let ptr: *Int = get_raw_pointer()

# Must wrap pointer operations in unsafe
unsafe
  let value = *ptr              # dereference
end
```

## The `!` Convention

Methods that can panic use `!` suffix — they're safe but signal danger:

```ruxen
let value = option.unwrap!           # panics on nil
let value = result.expect!("oops")   # panics on Err
```

This is a naming convention, not a language-level unsafe mechanism. `unwrap!` is valid safe code — it just might crash at runtime.

## Keeping Unsafe Minimal

The idiomatic approach is to create safe abstractions over unsafe code:

```ruxen
# Unsafe implementation detail
class SafeBuffer
  ptr: *var UInt8
  len: Int

  def init(size: Int)
    unsafe
      self.ptr = malloc(size) as *var UInt8
      self.len = size
    end
  end

  # Safe public API
  def get(index: Int) -> Option[UInt8]
    if index < 0 || index >= self.len
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

Users of `SafeBuffer` never need to write `unsafe` — the type's API is fully safe. `include Drop` declares the type a `Drop` participant; the writing `drop` method runs when an instance goes out of scope.
