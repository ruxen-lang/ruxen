# FFI — Calling C and Creating Ruxen Objects from C

FFI ("foreign function interface") is how Ruxen and C code talk to
each other. There are two directions:

1. **Ruxen → C** — call existing C library functions from Ruxen.
2. **C → Ruxen** — write a C function that returns a Ruxen object
   (a class instance Ruxen can use as if you constructed it from
   `.rx` code).

This chapter walks both directions from zero, with complete
runnable examples at each step.

---

## Part 1 — Calling a C function from Ruxen

### Step 1. Call `sqrt` from libm

Save this as `hello_ffi.rx`:

```ruxen
lib "m"
  def sqrt(x: Float) -> Float
end

def main
  let r = unsafe { sqrt(16.0) }
  puts "sqrt(16) = #{r}"
end
```

Run it:

```bash
ruxen compile hello_ffi.rx
./hello_ffi
```

Output:

```
sqrt(16) = 4
```

What happened, line by line:

- `lib "m"` says "open the system library named `m`" (that's
  `libm.so` on Linux, `libm.dylib` on macOS — Ruxen adds the
  prefix and extension).
- `def sqrt(x: Float) -> Float` declares that the library exports
  a function called `sqrt` with that signature. Ruxen doesn't read
  the C header — *you* tell Ruxen the signature.
- `unsafe { sqrt(16.0) }` calls it. Every FFI call must be inside
  an `unsafe` block because Ruxen can't verify the C function
  won't, for example, crash if you pass it a bad pointer.

### Step 2. Wrap it in a safe function

Letting `unsafe` leak into every caller is unfriendly. The standard
pattern is to wrap the raw call in a regular Ruxen function:

```ruxen
lib "m"
  def sqrt(x: Float) -> Float
end

# Safe wrapper — shadow the raw FFI name.
def sqrt(x: Float) -> Result[Float, String]
  if x < 0.0
    Err("sqrt of negative number")
  else
    Ok(unsafe { sqrt(x) })
  end
end

def main
  match sqrt(16.0)
    Ok(r)  -> puts "sqrt(16) = #{r}"
    Err(e) -> puts "error: #{e}"
  end
end
```

The `def sqrt` defined inside the `lib` block is just the raw FFI
binding. The top-level `def sqrt(x) -> Result[...]` shadows it
with a checked version. Now callers see a `Result` and never
write `unsafe`.

---

## Part 2 — Creating a Ruxen object from a C function

This is the interesting direction. You write a C function that
*returns* a Ruxen class instance, and Ruxen code uses it as if it
had called a normal constructor.

The whole trick is one line: **a Ruxen class instance is just an
`int64_t` at the C ABI** — that integer is a pointer to a struct
your C code allocated. Ruxen never looks inside; it just hands the
integer back to your C functions when you call methods on it.

### Step 3. The smallest possible example

We're going to build a `Counter` class whose state lives in C.

**Project layout:**

```
counter-demo/
  Ruxen.toml
  src/
    main.rx
  runtime/
    counter.c
```

**`Ruxen.toml`:**

```toml
[package]
name    = "counter-demo"
version = "0.1.0"
edition = "2026"
```

**`src/main.rx`:**

```ruxen
class Counter
  lib "runtime/counter.c"
    def self.new as "rx_counter_new"(initial: Int) -> Counter
    def bump    as "rx_counter_bump"(self) -> Int
    def value   as "rx_counter_value"(self) -> Int
    def drop    as "rx_counter_drop"(self) -> nil
  end
end

def main
  let c = Counter.new(10)
  let _ = c.bump
  let _ = c.bump
  puts "count = #{c.value}"   # count = 12
end
```

**`runtime/counter.c`:**

```c
#include <stdint.h>
#include <stdlib.h>

/* The struct that holds the actual data.
 * Ruxen never sees this struct — it only ever sees the pointer
 * cast to int64_t. */
typedef struct {
    int64_t value;
} RxCounter;

/* Constructor: allocate, initialise, return the pointer as int64_t.
 * Ruxen's `Counter.new(initial)` will land here. */
int64_t rx_counter_new(int64_t initial) {
    RxCounter *c = (RxCounter *)malloc(sizeof(RxCounter));
    c->value = initial;
    return (int64_t)c;
}

/* Method: receives the same pointer back, casts it, mutates, returns
 * the new value. Ruxen's `c.bump` will land here. */
int64_t rx_counter_bump(int64_t self) {
    RxCounter *c = (RxCounter *)self;
    c->value += 1;
    return c->value;
}

/* Read-only method. */
int64_t rx_counter_value(int64_t self) {
    RxCounter *c = (RxCounter *)self;
    return c->value;
}

/* Destructor: free the heap allocation when the Ruxen value drops. */
void rx_counter_drop(int64_t self) {
    RxCounter *c = (RxCounter *)self;
    free(c);
}
```

Build and run:

```bash
ruxen run
```

Output:

```
count = 12
```

That's the whole pattern. Let's go through it slowly.

### Step 4. The four C functions Ruxen needs

Every class with a C-backed runtime follows the same shape:

| Ruxen side                             | C side                                          | What it does |
|----------------------------------------|-------------------------------------------------|--------------|
| `def self.new as "rx_x_new"(...)`      | `int64_t rx_x_new(...)`                         | malloc the struct, return its pointer as `int64_t` |
| `def method as "rx_x_method"(self)`    | `int64_t rx_x_method(int64_t self)`             | cast `self` back to your struct, do work, return result |
| `def method2 as "rx_x_method2"(self, v: Int)` | `int64_t rx_x_method2(int64_t self, int64_t v)` | same, plus arguments |
| `def drop as "rx_x_drop"(self) -> nil` | `void rx_x_drop(int64_t self)`                  | free the struct (called automatically when the Ruxen value dies) |

Three rules to remember:

1. **`self` is always the first parameter** on the C side, typed
   `int64_t`. On the Ruxen side it's spelled `self` (no type — the
   compiler knows it's the receiver).
2. **Every parameter and return on the C side is `int64_t`** for
   integers, pointers, and Ruxen objects. The Ruxen side declares
   the *real* type (`Int`, `Counter`, `Float`, …) and the
   compiler handles the mapping.
3. **Always implement `drop`** — without it, the malloc'd struct
   leaks every time a Ruxen `Counter` goes out of scope.

### Step 5. Where the C file lives

When you put a C file at `runtime/<name>.c` inside your project,
`ruxen build` finds it automatically. You do not edit any
`Ruxen.toml` field to register it.

The string in `lib "runtime/counter.c"` is just a label that says
"the symbols I'm declaring here live in `runtime/counter.c`". The
compiler uses it to compile that `.c` file and link the resulting
`.o` into your binary.

### Step 6. Returning Ruxen primitives from C

Some C-backed methods need to return other Ruxen types:

| Ruxen return type | C return type   | How to construct on the C side |
|-------------------|-----------------|--------------------------------|
| `Int` / `Int64`   | `int64_t`       | return the integer directly |
| `Bool`            | `int64_t`       | return `0` (false) or `1` (true) |
| `Float`           | `double`        | declare and return `double` (not `int64_t`) |
| `nil` (unit)      | `void`          | C function returns `void`; Ruxen sees `nil` |
| Another class     | `int64_t`       | allocate / return a pointer the same way `new` does |

`Float` is the one special case — C `double` and Ruxen `Float` are
the same 8-byte IEEE-754 value, but the calling convention puts
them in different registers from integers, so you must declare
the C return as `double` (not `int64_t`).

### Step 7. Sharing strings between Ruxen and C

Strings are passed as their C buffer pointer. Ruxen's `String` has
an `as_cstr` method that gives you a `*UInt8` valid for the
duration of the call:

```ruxen
class Greeter
  lib "runtime/greeter.c"
    def self.new as "rx_greeter_new"(name: &String) -> Greeter
    def greet    as "rx_greeter_greet"(self) -> nil
    def drop     as "rx_greeter_drop"(self) -> nil
  end
end
```

C side:

```c
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
    char *name;   /* owned copy */
} RxGreeter;

int64_t rx_greeter_new(int64_t name_ptr) {
    const char *name = (const char *)name_ptr;
    RxGreeter *g = (RxGreeter *)malloc(sizeof(RxGreeter));
    g->name = strdup(name);     /* copy — Ruxen will free the original */
    return (int64_t)g;
}

void rx_greeter_greet(int64_t self) {
    RxGreeter *g = (RxGreeter *)self;
    printf("Hello, %s!\n", g->name);
}

void rx_greeter_drop(int64_t self) {
    RxGreeter *g = (RxGreeter *)self;
    free(g->name);
    free(g);
}
```

The key rule: **`String` references passed to C are borrowed,
not owned**. If your C side needs to keep the string past the
call, copy it (`strdup`, or any other allocation). When `drop`
runs, free your copy.

---

## Raw Pointers and `nil`

For lower-level work — calling `malloc` or interacting with a C
API that exposes raw pointers — Ruxen has `*T` (read-only pointer)
and `*var T` (writable pointer). Pointer operations are `unsafe`
([Chapter 15](15-unsafe.md)).

```ruxen
lib "c"
  def malloc(n: USize) -> *var Void
  def free(p: *var Void) -> nil
end

def main
  unsafe
    let ptr = malloc(1024) as *var UInt8
    if ptr == nil
      puts "out of memory"
      return
    end
    free(ptr as *var Void)
  end
end
```

`nil` is the null-pointer literal — valid only at `*T` / `*var T`
types and only in `unsafe` / FFI contexts. Ruxen references
(`&T`, `&var T`) cannot be `nil` — they are always valid by
construction.

## Variadic C functions

```ruxen
lib "c"
  def printf(fmt: *UInt8, ...) -> Int32
end
```

The trailing `...` lets you pass any number of additional scalar
or pointer arguments. Aggregates can't go through `...` — wrap
them first.

## Library options

`lib` accepts keyword options for system libraries you don't
ship yourself:

```ruxen
lib "sqlite3", version: "3"
  def sqlite3_open(filename: *UInt8, db: *var *Void) -> Int32
end

lib "c", path: "/usr/lib"
  def malloc(n: USize) -> *var Void
end
```

---

## Quick recap

- **Calling C**: `lib "<system-lib>" ... end` declares the
  signatures. Calls go inside `unsafe`. Wrap with a regular `def`
  to keep `unsafe` out of caller code.
- **Constructing Ruxen objects from C**: put `lib "runtime/X.c"`
  inside the class body. Implement `rx_<class>_new` (returns a
  malloc'd pointer cast to `int64_t`), one C function per method,
  and `rx_<class>_drop` to free the allocation. The compiler
  finds `runtime/*.c` automatically.
- **All Ruxen objects at the C ABI are pointers**, passed and
  returned as `int64_t`. Your C code allocates the storage; Ruxen
  hands the pointer back to your code on every method call.
- **Always implement `drop`** — there is no garbage collector and
  the compiler will not synthesise it for C-backed classes.
- **Strings passed to C are borrowed.** Copy them if you need to
  outlive the call.
