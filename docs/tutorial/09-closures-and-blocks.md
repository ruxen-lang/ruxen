# Closures and Blocks

A **closure** is a small anonymous function you can pass around as a value. Sort a list "by this rule," transform every item "with this function," delay an action until later — closures are how you say all of those without giving each helper a name.

Ruxen has Ruby's two shapes:

- A **block** is written `{ |x| ... }` or `do |x| ... end` and attached to a method call. It is borrowed for the call: the method can run it, but cannot keep it.
- A **lambda** is a closure stored in a variable, written `->(x: Int) { ... }`. It owns its captures and can be passed around, returned and called later. `lambda { |x| ... }`, `proc { |x| ... }` and `Proc.new { ... }` make the same kind of value [implemented].

## A first runnable example

Save this as `closures.rx` [implemented]:

```ruby
double = ->(x: Int) { x * 2 }
puts double.(5)
puts double.call(10)
```

```bash
ruxen compile closures.rx
./closures
```

Output:

```text
10
20
```

You created a lambda, stored it in `double`, and called it twice: once with the `.( ... )` syntax and once with `.call`. Ruby's third spelling, `double[5]`, is [planned](../../TODO.md#t-lambda-call).

## Two equivalent block syntaxes

[implemented]:

```ruby
numbers = [1, 2, 3]

# Brace form — good for one-liners
numbers.each { |n| puts n }

# do...end form — good for multi-line
numbers.each do |n|
  doubled = n * 2
  puts doubled
end
```

The pipes (`| ... |`) hold the parameter list. Inside, write the body — last expression is the return value. A block with no parameters has no pipes: write `{ work }`, not `{ || work }` (the empty pipes are [deprecated]).

## Type inference in closures

Block parameter types are usually inferred from context. When the block is passed to a method that expects `Fn[(Int) -> nil]`, the compiler knows the parameter is `Int` [implemented]:

```ruby
nums = [1, 2, 3]
nums.each { |n| puts n }   # n inferred from nums
```

You can spell them out when you want to [implemented]:

```ruby
parse = ->(s: &String) { s.to_i }
puts parse.("42")
```

## Chaining block methods

Blocks shine on collections [implemented]:

```ruby
nums = [1, 2, 3, 4, 5, 6]

doubled = nums.map { |n| n * 2 }
evens = nums.select { |n| n % 2 == 0 }
big = nums.find { |n| n > 3 }       # Int? — the first match, or nil

for n in doubled
  puts n
end
puts evens.size
```

`.map`, `.select`, `.find`, `.each`, `.partition`, and friends all take blocks and form the `Enumerable` vocabulary you'll use most days. Ruby's shorthand `names.map(&:upcase)` works too [implemented].

## Capturing variables

A closure can refer to variables defined in the surrounding scope. This is the "closure" in the name — it *closes over* the environment [implemented]:

```ruby
multiplier = 3
multiply = ->(x: Int) { x * multiplier }    # captures `multiplier`
puts multiply.(5)      # 15
puts multiply.(10)     # 30
```

## Capture modes

How a closure captures a variable matches the rules from Chapter 4:

| Mode | Syntax | When it happens |
|------|--------|-----------------|
| Borrow (read) | a block body | The block only reads the captured value |
| Borrow (write) | a block body | The block writes through the captured variable |
| Move (own) | a lambda, or `move { ... }` / `move do ... end` | The closure must outlive its enclosing scope (e.g. returned, sent to a thread) |

A block borrows its captures for the call. A lambda always owns its captures: capturing a non-Copy variable moves it into the lambda. `move` makes a block own its captures too.

### Returning a closure

Use a lambda or a `move` block when a closure must own its captures — typically when you return it from a method. The return type of a stored closure is `Proc[(Args) -> Ret]` [implemented]:

```ruby
def make_adder(n: Int) -> Proc[(Int) -> Int]
  move { |x: Int| x + n }
end

add_five = make_adder(5)
puts add_five.(10)    # 15
```

Without `move`, the block would try to borrow `n`, but `n` lives in `make_adder`'s stack frame — by the time the caller uses the closure, `n` is gone. `move` says "take `n` with you." Writing `->(x: Int) { x + n }` instead works the same way, since a lambda owns its captures.

## Passing closures to your own methods

A method can take a closure parameter just like any other value. A borrowed block-like parameter has the type `Fn[(ArgTypes) -> ReturnType]` [implemented]:

```ruby
def apply(f: Fn[(Int) -> Int], x: Int) -> Int
  f.(x)
end

square = ->(n: Int) { n * n }
puts apply(square, 4)     # 16
```

The paren spelling `Fn(Int) -> Int` is [deprecated]: replace it with `Fn[(Int) -> Int]`.

### Closure types

| Type | Meaning |
|------|---------|
| `Fn[(Args) -> Ret]` | A block: borrowed for the call, called any number of times. It cannot be stored (E1018). |
| `Proc[(Args) -> Ret]` | A stored proc or lambda: owns its captures, can be kept and returned. |
| `FnMut[(Args) -> Ret]`, `FnOnce[(Args) -> Ret]` | Capability bounds on a type parameter: may mutate its captures, or may be called only once. |

Most parameters take a block. Reach for `FnMut` or `FnOnce` only when you actually need the stronger capability. They work today as bounds on a type parameter [implemented]; whether the compiler can infer the capability for you is still open [planned](../../TODO.md#t-oq). The RBS bound `[F < FnOnce[...]]` is [planned](../../TODO.md#t-d7-respell).

Works today: the colon bound `[F: FnOnce[...]]` [deprecated] (replace with `[F < FnOnce[...]]`):

```ruby
def once[F: FnOnce[() -> Int]](f: F) -> Int
  f.()
end

puts once(-> { 3 })
```

The Rust name `FnVar` is [deprecated]: use `FnMut`.

## `yield` and block-receiving methods

A method can accept an implicit block via `yield`. The block is the trailing `do ... end` at the call site [implemented]:

```ruby
def with_x
  yield 42
end

with_x do |n|
  puts n
end
```

Output:

```text
42
```

`yield N` calls the block with `N` as its argument. This pattern is great for "wrap-around" operations: time something, set up and tear down a resource, run something inside a transaction [implemented]:

```ruby
def with_banner
  puts "start"
  yield
  puts "done"
end

with_banner do
  puts "working"
end
```

A method defined inside a class whose body ends in `yield` or a block call needs an explicit `-> nil` return type today.

## Declaring the block explicitly: `&block`

The examples above let `yield` discover the block implicitly. You can also
declare the block as an explicit, named parameter using the `&` sigil. The
block parameter is always **last**, and its type is written with the canonical
square-bracket signature `Fn[(Args…) -> Return]` [implemented]:

```ruby
def each_twice(x: Int, &block: Fn[(Int) -> nil])
  yield x
  yield x
end

each_twice(7) do |n|
  puts n     # prints 7 twice
end
```

Inside the body the block is also an ordinary callable value — you can invoke
it directly with `block.(args)` or `block.call(args)` instead of `yield` [implemented]:

```ruby
def run_it(&block: Fn[() -> nil])
  block.()
end

run_it do
  puts "ran"
end
```

Pass a block on to another method with `&block` in argument position (`@items.each(&block)`) [implemented].

A `&block` parameter **must be the last** parameter (anything after it is error
`E1119`). Calling a block with `.call` is recognised by its name today; it will become its own compiler node, so a user method that happens to be named `call` stays an ordinary method [planned](../../TODO.md#t-d4).

## `yield`'s value

When you declare the block's return type, `yield` is an expression whose value
**is** the block's return value. So you can use it directly [implemented]:

```ruby
def doubled(&block: Fn[(Int) -> Int]) -> Int
  r = yield(4)
  r + 1
end

v = doubled do |n|
  n * 10
end
puts v          # 41
```

When the block's declared result is `nil`, its last value is discarded. Today the compiler decides that by a rule of its own; it will discard a block result only where a block boundary declares a `nil` result [planned](../../TODO.md#t-x2).

## Optional blocks and `block_given?`

Every `&block` is **optional** — calling the method without a block is legal.
Test whether the caller passed one with `block_given?` (the Ruxen name
`block_defined?` works too), and pick a fallback [implemented]:

```ruby
def render(&block: Fn[() -> nil])
  if block_given?
    yield
  else
    puts "default"
  end
end

render do
  puts "custom"
end
render()                 # no block → prints "default"
```

Write `render()` with parentheses for now: a bare zero-argument call is [planned](../../TODO.md#t-f45).

If you `yield` without a block having been passed, the program stops with a
clear runtime error naming the method — it never silently misbehaves:

```text
ruxen panic: yield called without a block in `render`
```

## `do…end` vs `{ }`

Both forms attach a trailing block as the implicit last argument, identically
for top-level methods and methods on objects [implemented]:

```ruby
def each_twice(x: Int, &block: Fn[(Int) -> nil])
  yield x
  yield x
end

each_twice(7) do |n| puts n end
each_twice(7) { |n| puts n }
```

Use `do…end` for multi-line blocks and `{ }` for short one-liners — they mean
the same thing. A brace block on a bare call with no arguments and no block parameters (`run { ... }`) is [planned](../../TODO.md#t-f45); write `run() { ... }` or use `do ... end`.

## Common mistakes

**Storing a block.** A block (`Fn[...]`) is borrowed for the call; keeping it in a field or a variable that outlives the call is rejected (E1018). Take a lambda (`Proc[...]`) instead when you need to keep it.

**Capturing a variable and confusion about who owns it.** A block borrowing a variable makes that variable un-touchable in its enclosing scope until the block's last use passes. If you get a borrow conflict around a closure, the fix is often `move`.

**Returning a closure that captures borrows.** A returned closure outlives the method it was made in. If it borrows local data, the borrow becomes dangling — the compiler refuses. Use `move` or a lambda.

**Type annotation in pipes when not needed.** If context already tells the compiler the type, you don't need `|x: Int|` — just `|x|` is fine. Add annotations when the compiler asks or for readability.

## Try it

Start from this [implemented]:

```ruby
nums = [1, 2, 3, 4, 5]
doubled = nums.map { |n| n * 2 }
```

1. Add a `.select` step that keeps only values greater than 4.
2. Print each result with `.each`.
3. Write `make_multiplier(k: Int) -> Proc[(Int) -> Int]` modelled on `make_adder` above, and use it to build `times_seven`.

## Recap

- A block is written `{ |params| body }` or `do ... end` and attached to a call; a lambda `->(x: Int) { ... }` is a closure you store.
- Call a stored closure with `value.(args)` or `value.call(args)`.
- Closures capture variables from their enclosing scope, following the same borrow rules as the rest of the language.
- Use a lambda or `move { ... }` when the closure needs to own its captures (typically when returned or sent away).
- Block parameters are `Fn[(...) -> R]`; stored closures are `Proc[(...) -> R]`; `FnMut` and `FnOnce` are capability bounds. The paren form `Fn(...)` is deprecated.
- `yield` lets a method run an implicit block from its caller.
- Declare a block explicitly as the last parameter with `&block: Fn[(Args…) -> R]`; invoke it with `yield` or `block.(args)`.
- `yield`'s value is the block's return value (`r = yield(4)`).
- Every `&block` is optional — test for one with `block_given?` (alias `block_defined?`); `yield` without a block panics cleanly, naming the method.
- `do…end` and `{ }` blocks attach identically as the trailing argument.

**Next:** [Modules and Files](10-modules-and-imports.md) — organizing code across files.
