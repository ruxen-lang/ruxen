# Ownership and Borrowing

Ruxen has no garbage collector. Instead, every value has a single owner, and the compiler tracks ownership at compile time. When the owner goes out of scope, the value is freed.

## The Five Rules

1. **Every value has exactly one owner** at any time
2. **Assignment of non-Copy types is a move** — the source becomes invalid
3. **Borrowing**: a value can have EITHER many read-only borrows (`&T`) OR one writable borrow (`&var T`), never both
4. **A borrow must not outlive its owner**
5. **When the owner goes out of scope**, the value is dropped (destructor runs, memory freed)

## Move Semantics

When you assign a non-Copy value, ownership transfers:

```ruxen
let greeting = String.from("hello")
let moved = greeting              # ownership moves to `moved`
puts greeting                     # COMPILE ERROR: `greeting` was moved
```

This applies to function calls too:

```ruxen
def consume_string(s: String)
  puts s
end

let name = String.from("Ruxen")
consume_string(name)              # `name` moved into function
puts name                         # COMPILE ERROR: `name` was moved
```

## Copy Types

Primitive types are `Copy` — assignment duplicates the value:

```ruxen
let x = 42
let y = x       # copy, both valid
puts x           # OK
puts y           # OK
```

Copy types include: all integers, floats, `Bool`, `Char`, `()`, references (`&T`), and user structs whose fields are all `Copy` (implicit per §3.6).

## Read-only Borrowing (`&T`)

Borrow a value to read it without taking ownership:

```ruxen
def print_name(name: &String)     # borrows, doesn't own
  puts name
end

let name = String.from("Ruxen")
print_name(&name)                  # pass a borrow
puts name                          # still valid
```

You can have multiple read-only borrows at the same time:

```ruxen
let data = String.from("hello")
let r1 = &data
let r2 = &data                    # OK — multiple read-only borrows
puts r1
puts r2
```

## Writable Borrowing (`&var T`)

A writable borrow gives exclusive read-write access:

```ruxen
def append_bang(s: &var String)
  s.push('!')
end

var greeting = String.from("hello")
append_bang(&var greeting)
puts greeting                      # "hello!"
```

You cannot mix writable and read-only borrows:

```ruxen
var data = [1, 2, 3]
let view = &data                   # read-only borrow
data.push(4)                       # ERROR: writable borrow while `view` exists
puts view
```

## Non-Lexical Lifetimes (NLL)

Borrows end at their last use, not at the end of the scope:

```ruxen
var data = [1, 2, 3]
let view = &data
puts view                          # last use of `view`
data.push(4)                       # OK — `view` is no longer active
```

## Dangling References

The compiler prevents returning references to local values:

```ruxen
def dangling -> &String
  let local = String.from("hello")
  &local                           # ERROR: `local` dies when function returns
end
```

## Opting Into Copy

User-defined structs implicitly include `Copy` when every field is `Copy` (§3.6 — no explicit declaration needed):

```ruxen
struct Point
  x: Float
  y: Float
end

let a = Point.new(1.0, 2.0)
let b = a                          # copy, both valid
```

## Clone

For types that aren't Copy, use explicit `.clone` to duplicate:

```ruxen
let original = String.from("hello")
let copy = original.clone          # explicit deep copy
puts original                      # still valid
puts copy
```
