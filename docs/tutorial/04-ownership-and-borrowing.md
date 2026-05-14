# Ownership and Borrowing

Riven has no garbage collector. Instead, every value has a single owner, and the compiler tracks ownership at compile time. When the owner goes out of scope, the value is freed.

## The Five Rules

1. **Every value has exactly one owner** at any time
2. **Assignment of non-Copy types is a move** — the source becomes invalid
3. **Borrowing**: a value can have EITHER many immutable borrows (`&T`) OR one mutable borrow (`&mut T`), never both
4. **A borrow must not outlive its owner**
5. **When the owner goes out of scope**, the value is dropped (destructor runs, memory freed)

## Move Semantics

When you assign a non-Copy value, ownership transfers:

```riven
let greeting = String.new("hello")
let moved = greeting              # ownership moves to `moved`
puts greeting                     # COMPILE ERROR: `greeting` was moved
```

This applies to function calls too:

```riven
def consume_string(s: String)
  puts s
end

let name = String.new("Riven")
consume_string(name)              # `name` moved into function
puts name                         # COMPILE ERROR: `name` was moved
```

## Copy Types

Primitive types are `Copy` — assignment duplicates the value:

```riven
let x = 42
let y = x       # copy, both valid
puts x           # OK
puts y           # OK
```

Copy types include: all integers, floats, `Bool`, `Char`, `()`, references (`&T`), and user structs that `derive Copy`.

## Immutable Borrowing (`&T`)

Borrow a value to read it without taking ownership:

```riven
def print_name(name: &String)     # borrows, doesn't own
  puts name
end

let name = String.new("Riven")
print_name(&name)                  # pass a borrow
puts name                          # still valid
```

You can have multiple immutable borrows at the same time:

```riven
let data = String.new("hello")
let r1 = &data
let r2 = &data                    # OK — multiple immutable borrows
puts r1
puts r2
```

## Mutable Borrowing (`&mut T`)

A mutable borrow gives exclusive read-write access:

```riven
def append_bang(s: &mut String)
  s.push('!')
end

var greeting = String.new("hello")
append_bang(&mut greeting)
puts greeting                      # "hello!"
```

You cannot mix mutable and immutable borrows:

```riven
var data = array![1, 2, 3]
let view = &data                   # immutable borrow
data.push(4)                       # ERROR: mutable borrow while `view` exists
puts view
```

## Non-Lexical Lifetimes (NLL)

Borrows end at their last use, not at the end of the scope:

```riven
var data = array![1, 2, 3]
let view = &data
puts view                          # last use of `view`
data.push(4)                       # OK — `view` is no longer active
```

## Dangling References

The compiler prevents returning references to local values:

```riven
def dangling -> &String
  let local = String.new("hello")
  &local                           # ERROR: `local` dies when function returns
end
```

## Opting Into Copy

User-defined structs can derive `Copy` if all fields are Copy:

```riven
struct Point
  x: Float
  y: Float
end

let a = Point.new(1.0, 2.0)
let b = a                          # copy, both valid
```

## Clone

For types that aren't Copy, use explicit `.clone` to duplicate:

```riven
let original = String.new("hello")
let copy = original.clone          # explicit deep copy
puts original                      # still valid
puts copy
```
