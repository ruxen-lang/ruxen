# Closures and Blocks

A **closure** is a small anonymous function you can pass around as a value. Sort a list "by this rule," transform every item "with this function," delay an action until later — closures are how you say all of those without giving each helper a name.

A **block** is the same idea written in a Ruby-style `do ... end` or `{ ... }` form, attached to a method call. Ruxen has one closure type (no proc vs. lambda confusion) and the syntax is uniform — block form and standalone form mean the same thing.

## A first runnable example

```ruxen
def main
  let double = { |x: Int| x * 2 }
  puts "#{double.(5)}"
  puts "#{double.(10)}"
end
```

```bash
ruxen compile closures.rx
./closures
```

Output:

```
10
20
```

You created a closure, stored it in `double`, and called it twice with the `.( ... )` syntax. The `.(` (dot-paren) is how Ruxen says "call this value as a function."

## Two equivalent syntaxes

```ruxen
# Brace form — good for one-liners
numbers.each { |n| puts n }

# do...end form — good for multi-line
numbers.each do |n|
  let doubled = n * 2
  puts doubled
end
```

The pipes (`| ... |`) hold the parameter list. Inside, write the body — last expression is the return value.

## Type inference in closures

Closure parameter types are usually inferred from context. When the closure is passed to a method that expects `Fn(Int) -> Int`, the compiler knows the parameter is `Int`:

```ruxen
def main
  let nums = [1, 2, 3]
  nums.each { |n| puts "#{n}" }   # n inferred from nums
end
```

You can spell them out when you want to:

```ruxen
let parse = { |s: &str| s.parse_int }
```

## Iterator chains

Closures shine on collections:

```ruxen
def main
  let nums = [1, 2, 3, 4, 5, 6]

  let doubled = nums.map { |n| n * 2 }
  let evens = nums.select { |n| n % 2 == 0 }
  let big = nums.find { |n| *n > 3 }       # Option[&Int]

  for n in doubled
    puts "#{n}"
  end
end
```

`.map`, `.select`, `.find`, `.each`, `.partition`, and friends all take closures and form the Enumerable vocabulary you'll use most days.

## Capturing variables

A closure can refer to variables defined in the surrounding scope. This is the "closure" in the name — it *closes over* the environment:

```ruxen
def main
  let multiplier = 3
  let multiply = { |x: Int| x * multiplier }    # captures `multiplier`
  puts "#{multiply.(5)}"     # 15
  puts "#{multiply.(10)}"    # 30
end
```

## Capture modes

How the closure captures a variable matches the rules from Chapter 4:

| Mode | Syntax | When it happens |
|------|--------|-----------------|
| Borrow (read) | normal closure body | Closure only reads the captured value |
| Borrow (write) | normal closure body | Closure writes through the captured variable |
| Move (own) | `move { ... }` | Closure must outlive its enclosing scope (e.g. returned, sent to a thread) |

The compiler picks the lightest mode that works. `move` is when you need to take it further.

### Move closures

Use `move` when a closure must own its captures — typically when you return it from a function:

```ruxen
def make_adder(n: Int) -> some Fn(Int) -> Int
  move { |x| x + n }
end

def main
  let add_five = make_adder(5)
  puts "#{add_five.(10)}"    # 15
end
```

Without `move`, the closure would try to borrow `n`, but `n` lives in `make_adder`'s stack frame — by the time the caller uses the closure, `n` is gone. `move` says "take `n` with you."

## Passing closures to your own functions

A function can take a closure parameter just like any other value. The type is `Fn(ArgTypes) -> ReturnType`:

```ruxen
def apply(f: Fn(Int) -> Int, x: Int) -> Int
  f.(x)
end

def main
  let square = { |n: Int| n * n }
  puts "#{apply(square, 4)}"     # 16
end
```

### Three closure types

| Type | Meaning |
|------|---------|
| `Fn(Args) -> Ret` | Can be called many times, captures read-only |
| `FnVar(Args) -> Ret` | Can be called many times, may mutate captures |
| `FnOnce(Args) -> Ret` | Can be called once, may consume captures |

Most parameters accept `Fn`; reach for `FnVar` or `FnOnce` only when you actually need the stronger capability.

## `yield` and block-receiving functions

A method can accept an implicit block via `yield`. The block is the trailing `do ... end` at the call site:

```ruxen
def with_x
  yield 42
end

def main
  with_x do |n|
    puts "#{n}"
  end
end
```

Output:

```
42
```

`yield N` calls the block with `N` as its argument. This pattern is great for "wrap-around" operations: time something, set up and tear down a resource, run something inside a transaction.

```ruxen
def with_timing
  let start = clock_now()
  yield
  puts "took #{clock_now() - start} ms"
end

with_timing do
  do_work()
end
```

## Common mistakes

**Forgetting the dot in the call.** Closures are called with `.(args)`, not `(args)`:

```ruxen
let f = { |x| x + 1 }
f(5)         # ERROR: looking for a function called `f`
f.(5)        # OK
```

**Capturing a `var` and confusion about who owns it.** A closure borrowing a `var` makes that `var` un-touchable in its enclosing scope until the closure is dropped or its last use passes. If you get a borrow conflict around a closure, the fix is often `move`.

**Returning a closure that captures borrows.** A returned closure outlives the function it was made in. If it borrows local data, the borrow becomes dangling — the compiler refuses. Use `move`.

**Type annotation in pipes when not needed.** If context already tells the compiler the type, you don't need `|x: Int|` — just `|x|` is fine. Add annotations when the compiler asks or for readability.

## Try it

Start from this:

```ruxen
let nums = [1, 2, 3, 4, 5]
let doubled = nums.map { |n| n * 2 }
```

1. Add a `.select` step that keeps only values greater than 4.
2. Print each result with `.each`.
3. Write `make_multiplier(k: Int) -> some Fn(Int) -> Int` modelled on `make_adder` above, and use it to build `times_seven`.

## Recap

- A closure is an anonymous function written `{ |params| body }` or in `do ... end` form.
- Call a closure with `value.(args)`.
- Closures capture variables from their enclosing scope, following the same borrow rules as the rest of the language.
- Use `move { ... }` when the closure needs to own its captures (typically when returned or sent away).
- Function-typed parameters are `Fn(...)`, `FnVar(...)`, or `FnOnce(...)`.
- `yield` lets a method run an implicit `do ... end` block from its caller.

**Next:** [Modules and Imports](10-modules-and-imports.md) — organizing code across files.
