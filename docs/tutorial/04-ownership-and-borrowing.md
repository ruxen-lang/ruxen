# Ownership and Borrowing

Ruxen tracks who owns what so nothing leaks and nothing is freed twice. Other languages do this at runtime with a **garbage collector** (a background process that periodically reclaims unused memory). Ruxen does it at compile time, which means there's no runtime pause and your binaries stay small. The price is that you'll see the compiler refuse some programs that look fine at first glance — this chapter is about why, and how to fix them.

Ownership is the one thing Ruxen takes from Rust, and it is spelled to read like Ruby: `&x` and `&var x` for borrows, `def var` and `def consume` for methods.

Take it slow. This is the chapter most new readers find scariest, but once it clicks, it stays clicked.

## A first runnable example

A normal-looking program. Nothing surprising — yet [implemented]:

```ruby
def print_name(name: &String)
  puts name
end

s = "Ruxen"
print_name(&s)
puts s
```

```bash
ruxen compile demo.rx
./demo
```

Output:

```text
Ruxen
Ruxen
```

We made a string, lent it to `print_name`, and then printed it again ourselves. The lending is the `&` symbol — we'll get there.

## Rule 1: every value has exactly one owner

When you write `s = "Ruxen"`, the variable `s` **owns** the string. When `s` goes out of scope (the method ends, the block closes), the string's memory is freed. That's it — no garbage collector running in the background, no double-free, no leak.

## Rule 2: assigning a non-Copy value moves it

What happens when you assign one owning variable to another? [implemented]:

```ruby
greeting = "hello"
moved = greeting              # ownership moves to `moved`
puts moved                    # OK
# puts greeting               # ERROR: `greeting` was moved (E1001)
```

This is **move semantics**: ownership transferred from `greeting` to `moved`, and `greeting` is no longer usable. There's only one owner at a time — that's Rule 1 in action.

Method calls move arguments by default too [implemented]:

```ruby
def take_string(s: String)
  puts s
end

name = "Ruxen"
take_string(name)             # `name` moved into the method
# puts name                   # ERROR: `name` was moved (E1001)
```

Why "non-Copy"? Because some types — the small, cheap ones — *copy* instead of moving. That's the next section.

## Copy types — the exception that proves the rule

**`Copy`** is a marker the compiler attaches to types that are cheap to duplicate bit-for-bit. Assignment of a Copy value duplicates it; the source remains usable [implemented]:

```ruby
x = 42
y = x            # copy, both valid
puts "#{x} #{y}" # 42 42
```

Copy types include: all integers, floats, `Bool`, `Char`, `nil`, references (`&T`), and value types whose fields are all Copy. `String` and `Array[T]` are *not* Copy — they own heap memory, and silently duplicating that would be expensive.

## Borrowing: reading without owning

You don't always want to take ownership. Often you just want to *read* a value, hand it back, and let the original owner carry on. That's a **borrow**, written `&T` [implemented]:

```ruby
def print_name(name: &String)         # borrows, doesn't own
  puts name
end

s = "Ruxen"
print_name(&s)                         # pass a borrow with &
puts s                                 # still valid — we never gave it away
```

You can hand out as many read-only borrows as you like, all at the same time [implemented]:

```ruby
data = "hello"
r1 = &data
r2 = &data                      # OK — multiple read-only borrows
puts "#{r1} / #{r2}"
```

Read-only borrows are like library cards — many readers, no editor.

## Writable borrows: editing through a loan

A **writable borrow**, written `&var T`, gives exclusive read-write access. Works today, with the [deprecated] `push_str` (replace with `<<` once `String#<<` appends, a [known issue](../../TODO.md#t-string-shovel)):

```ruby
def append_bang(s: &var String)
  s.push_str("!")
end

greeting = "hello"
append_bang(&var greeting)
puts greeting                   # hello!
```

While a writable borrow exists, no other borrows — read-only or writable — are allowed. **One editor, no readers.** Rejected today with E1002 [deprecated] (`push_str`, see above):

```ruby
data = "hi"
view = &data                    # read-only borrow
data.push_str("!")              # ERROR: tried to mutate while `view` is alive
puts view
```

Fix: let the read-only borrow finish first. The compiler is smart about lifetimes — a borrow ends at its last use, not at the end of the block [deprecated] (`push_str`, see above):

```ruby
data = "hi"
view = &data
puts view                       # last use of view — borrow ends here
data.push_str("!")              # OK
puts data
```

## Dangling references are impossible

The compiler refuses to let a borrow outlive the value it borrows from, so you cannot return a reference to a local variable. With an explicit `return`, rejected today with E1010 [implemented]:

```ruby
def dangling -> &String
  local = "hello"
  return &local           # ERROR: `local` dies when the method returns
end

puts dangling()
```

The same method with `&local` as its last expression (no `return`) is not caught today: it compiles and runs, a [known issue](../../TODO.md#t-dangling-tail-ref). Until it is fixed, return an owned value (`local`) instead of a borrow.

Other languages let this compile and then crash at runtime; closing that gap is the goal.

## Clone when you need a second copy

For non-Copy types, ask for an explicit duplicate with `.clone` [implemented]:

```ruby
original = "hello"
copy = original.clone           # explicit duplication
puts original                   # still valid
puts copy
```

`.clone` is loud on purpose — it's where you're paying for an allocation. If you find yourself cloning everywhere, that's a hint to use borrows instead. (Ruby's `dup` works on `String` today; on other types use `clone`.)

## Receiver modes on methods

Methods carry the same borrow rules for their receiver, spelled as a word after `def`:

| Declaration | What it borrows | When to use |
|-------------|-----------------|-------------|
| `def method` | the receiver, read-only | Most methods that just look at state |
| `def var method` | the receiver, writable | Methods that change a field |
| `def consume method` | the receiver, owned | Methods that retire the value (e.g. `into_*`) |
| `def self.method` | nothing | Class methods — no receiver at all |

You met these in Chapter 3; they're the same rules from this chapter applied to `self`.

## Common mistakes

**Using a value after passing it to a method.** Rejected today with E1001 [implemented]:

```ruby
def take(s: String)
  puts s
end

s = "x"
take(s)
puts s         # ERROR: moved
```

Fix: pass a borrow (`take(&s)`) and change `take`'s parameter to `&String`. If `take` really needs ownership, accept that `s` is gone.

**Trying to mutate while a borrow is alive.** Rejected today with E1002 [implemented], with `push_str` [deprecated] (see above):

```ruby
data = "hi"
view = &data
data.push_str("!")     # ERROR: writable use while `view` is borrowed
puts view
```

Fix: print `view` before the push, so the borrow ends first. The same mistake on an array (`first = &v[0]` or `all = &v`, then `v.push(4)`, then a use of the borrow) is not caught today: it compiles and runs, a [known issue](../../TODO.md#t-array-borrow-check). For an array of integers, read the value into a local (`first = v[0]`) — integers are Copy, so you don't even need a borrow.

**Lending an old `let` binding writably.** Locals created by `x = ...` are always reassignable, so you can lend them with `&var`. The [deprecated] `let` binding is immutable, so `&var` of it is rejected (E1007). Replace `let` with plain assignment.

**Cloning to silence the compiler.** When a `.clone` call makes an error go away, ask first whether a borrow would have worked. Cloning hides a perf cost; borrowing is free.

## Try it

Take the writable-borrow example and try to print `greeting` *before* calling `append_bang` [deprecated] (`push_str`, as above):

```ruby
def append_bang(s: &var String)
  s.push_str("!")
end

greeting = "hello"
puts greeting                # OK — no borrow active yet
append_bang(&var greeting)
puts greeting                # OK — borrow ended at method return
```

Now try sneaking in a read-only borrow that overlaps the writable one. Read the error and notice that the compiler points at both ends of the conflict.

## Recap

- Every value has one owner. When the owner goes out of scope, the value is freed.
- Assigning a non-Copy value **moves** it. The old name is no longer usable.
- Copy types (integers, floats, references, value types of Copy fields) duplicate instead.
- `&T` is a read-only borrow — many at a time. `&var T` is a writable borrow — exactly one, and no readers.
- The compiler refuses to let any borrow outlive its source, so dangling references never compile.
- Reach for `.clone` only when neither borrowing nor moving fits.

**Next:** [Control Flow](05-control-flow.md) — `if`, `case`, loops, and how they compose as expressions.
