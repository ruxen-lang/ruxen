# Control Flow

This chapter is about how programs make decisions and how they repeat work. The pieces are `if`, `case`, `while`, `until` and `for`. A small but important detail: in Ruxen, as in Ruby, **almost every control structure is an expression** — it produces a value you can assign or return.

## A first runnable example

Save this as `labels.rx` [implemented]:

```ruby
def label(n: Int) -> String
  case n
  when 1 then "one"
  when 2 then "two"
  else "many"
  end
end

puts label(1)
puts label(2)
puts label(99)
```

```bash
ruxen compile labels.rx
./labels
```

Output:

```text
one
two
many
```

You used `case/when` and an `else` branch ("anything else"). The whole `case ... end` *was* the return value of `label` — no `return` keyword in sight.

## `if` / `elsif` / `else`

`if` is an expression — it returns a value [implemented]:

```ruby
x = 5
kind = if x > 0
  "positive"
elsif x < 0
  "negative"
else
  "zero"
end
puts kind
```

A few small notes:

- Ruxen uses `elsif`, not `else if` or `elif`.
- The parentheses around the condition are not required.
- `unless cond ... end` is the negated `if`; the ternary `ok ? "yes" : "no"` and the words `and`, `or`, `not` work as in Ruby.
- Branch bodies are blocks — their last expression is the branch's value, and all branches should agree on a single type.
- A condition must be a `Bool` today: `if user` on a `T?` value (Ruby truthiness) is [planned](../../TODO.md#t-truthiness).

Inside a method body, statement modifiers work as in Ruby: `return if done`, `x += 1 while x < 10`. At the top level of a file they are [planned](../../TODO.md#t-toplevel-stmts).

## `case/when` — matching values

`case/when` compares a value against each `when` clause in turn; the first match runs. A clause can list several values or a range [implemented]:

```ruby
def describe(n: Int) -> String
  case n
  when 0 then "zero"
  when 1, 2 then "a few"
  when 3..9 then "several"
  else "many"
  end
end

puts describe(5)
```

## `case/in` — pattern matching

**Pattern matching** goes further: each `in` arm tries a pattern, binds names from it, and the first matching arm runs. `then` or a newline separates the pattern from the arm's body. `_` is the wildcard — it matches anything.

### Guards

A **guard** is an extra `if` condition on an arm [implemented]:

```ruby
def grade(score: Int) -> String
  case score
  in n if n >= 90 then "A"
  in n if n >= 80 then "B"
  in n if n >= 70 then "C"
  in _ then "F"
  end
end

puts grade(85)
```

### Alternatives

Pipe alternatives into one arm [implemented]:

```ruby
def kind(day: &String) -> String
  case day
  in "Saturday" | "Sunday" then "weekend"
  in _ then "weekday"
  end
end

puts kind("Sunday")
```

The compiler does not check today that the arms cover every input, so end a
`case` with an `in _` or `else` arm.

### Matching tags

The Ruby form for plain tags is symbols matched with `case/in` [planned](../../TODO.md#t-symbols) (Chapter 7 covers this properly; here's a teaser):

```ruby
def describe(c: Symbol) -> String
  case c
  in :red then "red"
  in :green then "green"
  in :blue then "blue"
  end
end

puts describe(:red)
```

Works today: an `enum` matched with `case/in` [deprecated] (replace with symbols or `Data` variants, Chapter 7):

```ruby
enum Color
  Red
  Green
  Blue
end

def describe(c: Color) -> String
  case c
  in Color.Red then "red"
  in Color.Green then "green"
  in Color.Blue then "blue"
  end
end

puts describe(Color.Red)
puts describe(Color.Green)
puts describe(Color.Blue)
```

### Destructuring

You can take structured values apart right in the pattern. The Ruby form is an array pattern [planned](../../TODO.md#t-patterns):

```ruby
def where_is(x: Int, y: Int) -> String
  case [x, y]
  in [0, 0] then "origin"
  in [a, 0] then "on x-axis at #{a}"
  in [0, b] then "on y-axis at #{b}"
  in [a, b] then "at (#{a}, #{b})"
  end
end

puts where_is(3, 4)
```

Works today: a tuple pattern [deprecated] (replace with the array pattern above):

```ruby
def where_is(x: Int, y: Int) -> String
  case (x, y)
  in (0, 0) then "origin"
  in (a, 0) then "on x-axis at #{a}"
  in (0, b) then "on y-axis at #{b}"
  in (a, b) then "at (#{a}, #{b})"
  end
end

puts where_is(3, 4)
```

### Multi-statement arms

An arm whose body needs more than one statement puts it on the lines after the pattern; the **last expression** is the arm's value. Works today, with an `enum` [deprecated] (replace with `Data` variants, Chapter 7):

```ruby
enum Shape
  Triangle(a: Float, b: Float, c: Float)
  Dot
end

def area(shape: Shape) -> Float
  case shape
  in Shape.Triangle(a, b, c)
    s = (a + b + c) / 2.0
    s * (s - a)
  in _ then 0.0
  end
end

puts area(Shape.Dot)
```

The older `match v ... pat -> expr ... end` expression is [deprecated]: replace it with `case/in`, which also frees the name `match` for `String#match`.

## Values that may be missing

A method that may find nothing returns `T?` (Chapter 7). The Ruby form narrows it in the condition itself [planned](../../TODO.md#t-narrowing):

```ruby
def find(n: Int) -> Int?
  n > 0 ? n : nil
end

if (v = find(7))
  puts "got #{v}"
else
  puts "nothing"
end
```

Works today: `if let` [deprecated] (replace with `if (v = ...)` above):

```ruby
def find(n: Int) -> Int?
  if n > 0
    n
  else
    nil
  end
end

if let Some(v) = find(7)
  puts "got #{v}"
else
  puts "nothing"
end
```

The same goes for loops: keep going while a value is present. The Ruby form [planned](../../TODO.md#t-narrowing):

```ruby
queue = [1, 2, 3]
while (item = queue.pop)
  puts item
end
```

Works today: `while let` [deprecated] (replace with `while (item = ...)`):

```ruby
queue = [1, 2, 3]
while let Some(item) = queue.pop
  puts item
end
```

## `while` loops

The everyday loop. Runs while the condition is true; `until` runs while it is false [implemented]:

```ruby
i = 0
while i < 5
  puts i
  i += 1
end
```

Output:

```text
0
1
2
3
4
```

## `for` loops

Iterate over an array or a range [implemented]:

```ruby
for i in 0...5
  puts i           # 0 through 4 (exclusive)
end

for i in 0..5
  puts i           # 0 through 5 (inclusive)
end
```

Ranges are Ruby-shaped: `0..5` is **inclusive** (0 through 5), and
`0...5` is **exclusive** (0 through 4). There is no `..=`. Ranges work in
`for` and `case/when` today; calling `Range` methods such as `each`, `size`
or `to_a` fails to compile, a [known issue](../../TODO.md#t-range-methods).

[implemented]:

```ruby
nums = [10, 20, 30]
for n in nums
  puts n
end
```

Most Ruby code iterates with blocks instead (`nums.each { |n| puts n }`); see Chapter 9.

## The infinite loop you exit explicitly

When the condition doesn't fit in `while`, loop forever and `break` out. The Ruby form is the `Kernel` method `loop` with a block [planned](../../TODO.md#t-loop-do):

```ruby
i = 0
loop do
  break if i >= 3
  puts i
  i += 1
end
```

Works today: the `loop ... end` keyword form [deprecated] (replace with `loop do ... end`):

```ruby
i = 0
loop
  if i >= 3
    break
  end
  puts i
  i += 1
end
```

`break` can also carry a value out of the loop — the loop is an expression too [deprecated] (`loop ... end`, as above):

```ruby
n = 0
answer = loop
  n += 1
  if n * n > 50
    break n
  end
end
puts answer      # 8
```

## `break` and `next`

`break` exits a loop entirely. `next` skips to the next iteration; in Ruby it works in every loop, and in Ruxen today it works only inside blocks (`xs.each { |x| ... }`, Chapter 9). The Ruby form in a `for` loop [planned](../../TODO.md#t-next-loops):

```ruby
for n in 0..10
  next if n == 3     # skip to the next iteration
  break if n == 7    # exit the loop entirely
  puts n
end
```

Works today: `continue` [deprecated] (replace with `next`):

```ruby
for n in 0..10
  if n == 3
    continue        # skip to the next iteration
  end
  if n == 7
    break           # exit the loop entirely
  end
  puts n
end
```

## Several statements as one value

`do ... end` and `{ ... }` are blocks: they belong to a method call (`xs.each do |x| ... end`, Chapter 9). To compute a value from several statements, use a helper method, or an `if` or `case` expression, all of which *are* values [implemented]:

```ruby
def sum_two -> Int
  a = 1
  b = 2
  a + b
end

v = sum_two()   # 3
puts v
```

Ruby's `begin ... end` expression block is not available yet; it arrives with exceptions [planned](../../TODO.md#t-exceptions).

## Common mistakes

**Forgetting `end`.** Every `if`, `unless`, `case`, `while`, `until`, `for`, `def`, `do` needs an `end`. The compiler points at where it expected one.

**Leaving out a case.** The compiler does not check today that a `case` covers every input; the check is [planned](../../TODO.md#t-case-exhaustive). End it with an `in _` or `else` arm.

**Mixed types across `if` branches.** All branches of an `if` expression should agree on a type; today mismatched branches are not reported [known issue](../../TODO.md#t-if-branch-types). Keep both branches the same type, or use the `if` for its side-effects only (and not bind the result).

**Confusing `..` and `...`.** Ruby-style: `0..5` includes 5 (inclusive); `0...5` stops at 4 (exclusive). There is no `..=`.

## Try it

Change the `grade` method so an input of 100 prints `"A+"`. (Hint: add a new arm above the existing `n >= 90` arm — the first matching arm wins.) Then add a guard that rejects negative scores with `"invalid"`.

## Recap

- `if` is an expression — branches should agree on a type.
- `case/when` compares values; `case/in` matches patterns with guards and alternatives. End both with a catch-all arm.
- `match`, `if let`, `while let`, `loop ... end` and `continue` are deprecated; their Ruby replacements are `case/in`, `if (x = ...)`, `while (x = ...)`, `loop do ... end` and `next`.
- `while`, `until` and `for` cover condition, range and collection loops.
- `break` exits a loop (and can carry a value out of it).

**Next:** [Classes and Value Types](06-classes-and-structs.md) — defining your own types.
