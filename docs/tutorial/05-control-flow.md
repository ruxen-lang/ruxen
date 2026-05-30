# Control Flow

This chapter is about how programs make decisions and how they repeat work. The pieces are `if`, `match`, `while`, `for`, and `loop`. A small but important detail: in Ruxen, **almost every control structure is an expression** — it produces a value you can assign or return.

## A first runnable example

```ruxen
def label(n: Int) -> String
  match n
    1 -> String.from("one")
    2 -> String.from("two")
    _ -> String.from("many")
  end
end

def main
  puts label(1)
  puts label(2)
  puts label(99)
end
```

```bash
ruxen compile labels.rx
./labels
```

Output:

```
one
two
many
```

You used `match` (more powerful than `if`) and a `_` wildcard ("anything else"). The whole `match ... end` *was* the return value of `label` — no `return` keyword in sight.

## `if` / `elsif` / `else`

`if` is an expression — it returns a value:

```ruxen
def main
  let x = 5
  let kind = if x > 0
    "positive"
  elsif x < 0
    "negative"
  else
    "zero"
  end
  puts kind
end
```

A few small notes:

- Ruxen uses `elsif`, not `else if` or `elif`.
- The parentheses around the condition are not required.
- Branch bodies are blocks — their last expression is the branch's value, and all branches must agree on a single type.

## `match` — exhaustive pattern matching

**Pattern matching** is a generalized `switch`: each arm tries a pattern, and the first matching arm runs. The compiler checks that every possible input is handled, so you can't accidentally forget a case.

```ruxen
def describe(n: Int) -> &str
  match n
    0 -> "zero"
    1 -> "one"
    _ -> "many"
  end
end
```

`_` is the wildcard — it matches anything.

### Matching enums

`match` shines on enums (Chapter 7 covers enums properly; here's a teaser):

```ruxen
enum Color
  Red
  Green
  Blue
end

def describe(c: Color) -> String
  match c
    Color.Red   -> String.from("red")
    Color.Green -> String.from("green")
    Color.Blue  -> String.from("blue")
  end
end

def main
  puts describe(Color.Red)
  puts describe(Color.Green)
  puts describe(Color.Blue)
end
```

If you forget to handle `Color.Blue`, the compiler refuses to build until you do.

### Match guards

A **guard** is an extra `if` condition on a match arm:

```ruxen
def grade(score: Int) -> &str
  match score
    n if n >= 90 -> "A"
    n if n >= 80 -> "B"
    n if n >= 70 -> "C"
    _            -> "F"
  end
end
```

### Or-patterns

Pipe alternatives into one arm:

```ruxen
def kind(day: &str) -> &str
  match day
    "Saturday" | "Sunday" -> "weekend"
    _                     -> "weekday"
  end
end
```

### Destructuring tuples

You can take apart structured values right in the pattern:

```ruxen
def main
  let point = (3, 4)
  let label = match point
    (0, 0) -> String.from("origin")
    (x, 0) -> "on x-axis at #{x}"
    (0, y) -> "on y-axis at #{y}"
    (x, y) -> "at (#{x}, #{y})"
  end
  puts label
end
```

## `if let` and `while let`

Patterns can ride alongside `if` and `while` when you want to combine "is this the case I care about?" with "and bind the inside":

```ruxen
def find(n: Int) -> Int?
  if n > 0
    n
  else
    nil
  end
end

def main
  if let Some(v) = find(7)
    puts "got #{v}"
  else
    puts "nothing"
  end
end
```

`Int?` is shorthand for `Option[Int]` — a value that might or might not be there. We'll meet `Option` in Chapter 7.

`while let` is the same trick for loops — keep going as long as the pattern matches:

```ruxen
while let Some(item) = queue.pop
  process(item)
end
```

## `while` loops

The everyday loop. Runs while the condition is true:

```ruxen
def main
  var i = 0
  while i < 5
    puts "#{i}"
    i += 1
  end
end
```

Output:

```
0
1
2
3
4
```

## `for` loops

Iterate over anything iterable — an array, a range, the characters of a string:

```ruxen
def main
  for i in 0..5
    puts "#{i}"           # 0 through 4
  end

  for i in 0..=5
    puts "#{i}"           # 0 through 5 (inclusive)
  end
end
```

`0..5` is a half-open range (excludes the end). `0..=5` is inclusive.

```ruxen
def main
  let nums = [10, 20, 30]
  for n in nums
    puts "#{n}"
  end
end
```

## `loop` — the infinite loop you exit explicitly

When the condition doesn't fit in `while`, use `loop` and `break`:

```ruxen
def main
  var i = 0
  loop
    if i >= 3
      break
    end
    puts "#{i}"
    i += 1
  end
end
```

`break` can also carry a value out of the loop — `loop` is an expression too:

```ruxen
let answer = loop
  let n = compute_candidate()
  if n.is_valid
    break n
  end
end
```

## `break` and `continue`

```ruxen
def main
  for n in 0..10
    if n == 3
      continue        # skip to the next iteration
    end
    if n == 7
      break           # exit the loop entirely
    end
    puts "#{n}"
  end
end
```

## Blocks as expressions

A `do ... end` block lets you stash a few statements and produce a value. The last expression is the result:

```ruxen
def main
  let v = do
    let a = 1
    let b = 2
    a + b
  end
  puts "#{v}"      # 3
end
```

This is handy inside `match` arms when you need more than one expression:

```ruxen
match shape
  Shape.Triangle(a, b, c) -> do
    let s = (a + b + c) / 2.0
    s * (s - a)
  end
  _ -> 0.0
end
```

## Common mistakes

**Forgetting `end`.** Every `if`, `match`, `while`, `for`, `loop`, `def`, `do` needs an `end`. The compiler points at where it expected one.

**Non-exhaustive `match`.** Leaving out a case the compiler can construct (e.g. one enum variant) is an error. Add the missing arm or a `_` catch-all.

**Mixed types across `if` branches.** All branches of an `if` expression must agree on a type. This:

```ruxen
let x = if cond
  42
else
  "no"        # ERROR: Int and &str don't match
end
```

…needs both branches to return the same type, or you need to use the `if` for its side-effects only (and not bind the result).

**Confusing `..` and `..=`.** `0..5` does not include 5. If you want 5, write `0..=5`.

## Try it

Change the `grade` function so an input of 100 prints `"A+"`. (Hint: add a new arm above the existing `n >= 90` arm — the first matching arm wins.) Then add a guard that rejects negative scores with `"invalid"`.

## Recap

- `if` is an expression — branches must agree on a type.
- `match` is exhaustive — the compiler enforces handling every case.
- `if let` and `while let` combine pattern matching with control flow.
- `while`, `for`, and `loop` cover normal, range/collection, and infinite loops.
- `break` exits a loop (and can carry a value out of `loop`). `continue` skips to the next iteration.
- `do ... end` is a block expression — useful inside other expressions.

**Next:** [Classes and Structs](06-classes-and-structs.md) — defining your own types.
