# FFI (Foreign Function Interface)

Ruxen calls C libraries through `lib "..." ... end` blocks. The string after `lib` is the link name; options follow as keyword arguments.

## Declaring External Libraries

```ruxen
lib "m"
  def sin(x: Float) -> Float
  def cos(x: Float) -> Float
  def sqrt(x: Float) -> Float
end

unsafe
  let result = sin(3.14159 / 2.0)
  puts result    # ~1.0
end
```

The bare string is the library name *without* the `lib` prefix or platform extension (`.so`/`.dylib`) — the linker adds those.

### Library Options

`lib` accepts keyword arguments for version pinning, custom search paths, and similar linker hints:

```ruxen
lib "sqlite3", version: "3"
  def sqlite3_open(filename: *UInt8, db: *var *Void) -> Int32
  def sqlite3_close(db: *Void) -> Int32
end

lib "c", path: "/usr/lib"
  def malloc(n: USize) -> *var Void
  def free(p: *var Void)
end
```

## Raw Pointers and `nil`

FFI uses raw pointers (`*T`, `*var T`). Pointer operations — including dereference and comparison against `nil` — are `unsafe` (see [Chapter 15](15-unsafe.md)).

```ruxen
lib "c"
  def malloc(n: USize) -> *var Void
  def free(p: *var Void)
end

unsafe
  let ptr = malloc(1024) as *var UInt8
  if ptr == nil
    return Err("out of memory")
  end
  # ... use ptr ...
  free(ptr as *var Void)
end
```

`nil` is the raw-pointer literal for an invalid/zero pointer. It is valid only at `*T` / `*var T` types and only in `unsafe` / FFI contexts; Ruxen references (`&T`, `&var T`) cannot be `nil` — they are always valid by construction.

## Variadic Functions

C functions with `...` in the parameter list:

```ruxen
lib "c"
  def printf(fmt: *UInt8, ...) -> Int32
end
```

## Safety

All FFI calls are inherently `unsafe` — the compiler cannot verify memory safety across the language boundary. The idiomatic pattern is to wrap FFI calls in safe Ruxen APIs:

```ruxen
lib "m"
  def sqrt(x: Float) -> Float
end

# Safe wrapper
def sqrt(x: Float) -> Result[Float, String]
  if x < 0.0
    Err(String.from("cannot take sqrt of negative number"))
  else
    Ok(unsafe { sqrt(x) })
  end
end
```

The `sqrt` definition shadows the linked-in C function with a checked Ruxen version; callers of the Ruxen `sqrt` never see `unsafe`.
