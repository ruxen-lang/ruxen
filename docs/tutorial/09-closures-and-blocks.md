# Closures and Blocks

Riven has **one closure type** (design principle P3 — One Obvious Path). No proc vs lambda distinction.

## Syntax

Two equivalent forms:

```riven
# Brace block — single line
numbers.each { |n| puts n }

# do...end block — multi-line
numbers.each do |n|
  let doubled = n * 2
  puts doubled
end
```

## Closures as Values

Store closures in variables and call them with `.(...)`:

```riven
let double = { |x: Int| x * 2 }
let result = double.(10)           # 20

let add = { |a: Int, b: Int| a + b }
puts add.(3, 4)                    # 7
```

## Type Inference in Closures

Closure parameter types are usually inferred from context:

```riven
let nums = [1, 2, 3]

# `n` is inferred as &Int from Array[Int]
nums.each { |n| puts n }

# Explicit types when needed
let parse = { |s: &str| s.parse_int }
```

## Iterator Methods

Closures power Riven's iterator chain:

```riven
let nums = [1, 2, 3, 4, 5, 6]

let result = nums
  .filter { |n| n % 2 == 0 }
  .map { |n| n * 10 }

# find, partition, each, etc.
let found = nums.find { |n| n > 3 }

let (evens, odds) = nums.partition { |n| n % 2 == 0 }
```

## Capture Semantics

Closures capture variables from their enclosing scope:

```riven
let multiplier = 3
let multiply = { |x: Int| x * multiplier }  # captures `multiplier`
```

### Capture Modes

| Mode | Syntax | When |
|------|--------|------|
| Immutable borrow | `{ ... }` (inferred) | Closure reads the variable |
| Mutable borrow | `{ ... }` (inferred) | Closure mutates the variable |
| Move | `move { ... }` | Closure must outlive captured values |

### Move Closures

When a closure needs to own its captures (e.g., returned from a function):

```riven
def make_adder(n: Int) -> some Fn(Int) -> Int
  move { |x| x + n }
end

let add_five = make_adder(5)
puts add_five.(10)                 # 15
```

### Explicit Capture List

For fine-grained control:

```riven
let name = String.new("Riven")
let age = 30
let closure = [&name, move age] { puts "#{name}, #{age}" }
```

## Yield

Functions can receive an implicit block via `yield`:

```riven
def with_timing
  let start = Time.now
  yield
  puts "Took #{Time.now - start}ms"
end

with_timing do
  heavy_computation()
end
```

## Closure Types

| Type | Meaning |
|------|---------|
| `Fn(Args) -> Ret` | Can be called multiple times, captures by reference |
| `FnMut(Args) -> Ret` | Can be called multiple times, captures mutably |
| `FnOnce(Args) -> Ret` | Can be called once, may consume captures |
