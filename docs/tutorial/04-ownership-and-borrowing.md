# Ownership and Borrowing

Ruxen tracks who owns what so nothing leaks and nothing is freed twice. Other languages do this at runtime with a **garbage collector** (a background process that periodically reclaims unused memory). Ruxen does it at compile time, which means there's no runtime pause and your binaries stay small. The price is that you'll see the compiler refuse some programs that look fine at first glance — this chapter is about why, and how to fix them.

Take it slow. This is the chapter most new readers find scariest, but once it clicks, it stays clicked.

## A first runnable example

A normal-looking program. Nothing surprising — yet.

```ruxen
def print_name(name: &String)
  puts "#{name}"
end

def main
  let s = "Ruxen"
  print_name(&s)
  puts "#{s}"
end
```

```bash
ruxen compile demo.rx
./demo
```

Output:

```
Ruxen
Ruxen
```

We made a string, lent it to `print_name`, and then printed it again ourselves. The lending is the `&` symbol — we'll get there.

## Rule 1: every value has exactly one owner

When you write `let s = "Ruxen"`, the variable `s` **owns** the string. When `s` goes out of scope (the function ends, the block closes), the string's memory is freed. That's it — no garbage collector running in the background, no double-free, no leak.

## Rule 2: assigning a non-Copy value moves it

What happens when you assign one owning binding to another?

```ruxen
def main
  let greeting = "hello"
  let moved = greeting              # ownership moves to `moved`
  puts moved                        # OK
  # puts greeting                   # ERROR: `greeting` was moved
end
```

This is **move semantics**: ownership transferred from `greeting` to `moved`, and `greeting` is no longer usable. There's only one owner at a time — that's Rule 1 in action.

Function calls move arguments by default too:

```ruxen
def consume_string(s: String)
  puts s
end

def main
  let name = "Ruxen"
  consume_string(name)              # `name` moved into the function
  # puts name                       # ERROR: `name` was moved
end
```

Why "non-Copy"? Because some types — the small, cheap ones — *copy* instead of moving. That's the next section.

## Copy types — the exception that proves the rule

**`Copy`** is a tag the compiler attaches to types that are cheap to duplicate bit-for-bit. Assignment of a Copy value duplicates it; the source remains usable.

```ruxen
def main
  let x = 42
  let y = x            # copy, both valid
  puts "#{x} #{y}"     # 42 42
end
```

Copy types include: all integers, floats, `Bool`, `Char`, `nil`, references (`&T`), and structs whose fields are all Copy. `String` and `Array[T]` are *not* Copy — they own heap memory, and silently duplicating that would be expensive.

## Borrowing: reading without owning

You don't always want to take ownership. Often you just want to *read* a value, hand it back, and let the original owner carry on. That's a **borrow**, written `&T`.

```ruxen
def print_name(name: &String)         # borrows, doesn't own
  puts "#{name}"
end

def main
  let s = "Ruxen"
  print_name(&s)                       # pass a borrow with &
  puts "#{s}"                          # still valid — we never gave it away
end
```

You can hand out as many read-only borrows as you like, all at the same time:

```ruxen
def main
  let data = "hello"
  let r1 = &data
  let r2 = &data                      # OK — multiple read-only borrows
  puts "#{r1} / #{r2}"
end
```

Read-only borrows are like library cards — many readers, no editor.

## Writable borrows: editing through a loan

A **writable borrow**, written `&var T`, gives exclusive read-write access:

```ruxen
def append_bang(s: &var String)
  s.push('!')
end

def main
  var greeting = "hello"
  append_bang(&var greeting)
  puts "#{greeting}"                  # hello!
end
```

Two important details:

- The owner must be `var` (mutable) for you to lend it out writably.
- While a writable borrow exists, no other borrows — read-only or writable — are allowed. **One editor, no readers.**

```ruxen
# This won't compile:
var data = "hi"
let view = &data            # read-only borrow
data.push('!')              # ERROR: tried to mutate while `view` is alive
puts view
```

Fix: let the read-only borrow finish first. The compiler is smart about lifetimes — a borrow ends at its last use, not at the end of the block:

```ruxen
var data = "hi"
let view = &data
puts view                   # last use of view — borrow ends here
data.push('!')              # OK
```

## Dangling references are impossible

The compiler refuses to let a borrow outlive the value it borrows from. This makes it impossible to return a reference to a local variable:

```ruxen
def dangling -> &String
  let local = "hello"
  &local                  # ERROR: `local` dies when the function returns
end
```

Other languages let this compile and then crash at runtime. Ruxen catches it before it ships.

## Clone when you need a second copy

For non-Copy types, ask for an explicit duplicate with `.clone`:

```ruxen
def main
  let original = "hello"
  let copy = original.clone           # explicit duplication
  puts original                        # still valid
  puts copy
end
```

`.clone` is loud on purpose — it's where you're paying for an allocation. If you find yourself cloning everywhere, that's a hint to use borrows instead.

## Self-modes on methods

Methods carry the same borrow rules as functions, just spelled differently:

| Declaration | What it borrows | When to use |
|-------------|-----------------|-------------|
| `def method` | `&self` — read-only | Most methods that just look at state |
| `def var method` | `&var self` — writable | Methods that change a field |
| `def consume method` | `self` — owned | Methods that retire the value (e.g. `into_*`) |
| `def self.method` | nothing | Class methods — no receiver at all |

You met these in Chapter 3; they're the same rules from this chapter applied to `self`.

## Common mistakes

**Using a value after passing it to a function.**

```ruxen
let s = "x"
consume(s)
puts s         # ERROR: moved
```

Fix: pass a borrow (`consume(&s)`) and change `consume`'s parameter to `&String`. If consume really needs ownership, accept that `s` is gone.

**Trying to mutate while a borrow is alive.**

```ruxen
var v = [1, 2, 3]
let first = &v[0]
v.push(4)              # ERROR: writable use while `first` is borrowed
puts first
```

Fix: print `first` before the push, or read the value into a local (`let first = v[0]`) — primitives are Copy, so you don't even need a borrow.

**Forgetting `var` on the owner.** You can't take a writable borrow of an immutable binding:

```ruxen
let s = "x"
append_bang(&var s)    # ERROR: s is let, not var
```

Fix: change `let` to `var`.

**Cloning to silence the compiler.** When a `.clone` call makes an error go away, ask first whether a borrow would have worked. Cloning hides a perf cost; borrowing is free.

## Try it

Take the writable-borrow example and try to print `greeting` *before* calling `append_bang`:

```ruxen
var greeting = "hello"
puts greeting                # OK — no borrow active yet
append_bang(&var greeting)
puts greeting                # OK — borrow ended at function return
```

Now try sneaking in a read-only borrow that overlaps the writable one. Read the error and notice that the compiler points at both ends of the conflict.

## Recap

- Every value has one owner. When the owner goes out of scope, the value is freed.
- Assigning a non-Copy value **moves** it. The old name is no longer usable.
- Copy types (integers, floats, references, structs of Copy fields) duplicate instead.
- `&T` is a read-only borrow — many at a time. `&var T` is a writable borrow — exactly one, and no readers.
- The compiler refuses to let any borrow outlive its source, so dangling references never compile.
- Reach for `.clone` only when neither borrowing nor moving fits.

**Next:** [Control Flow](05-control-flow.md) — `if`, `match`, loops, and how they compose as expressions.
