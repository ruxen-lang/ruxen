# Lifetimes and Borrowing in Depth

Most of the time you don't write lifetimes — the compiler infers them. This chapter is for the small set of cases where you need to be explicit: methods that return a reference, classes that hold a borrow, generic types where the compiler can't guess the relationship for itself.

A **lifetime** is just a label the compiler uses to track "how long does this reference stay valid?" In `&Array[Int]`, the lifetime is invisible — the compiler picks one. In `&a Array[Int]`, the `a` is a name you've given to that lifetime so you can refer to it elsewhere in the signature. Lifetimes are part of the ownership surface Ruxen keeps from Rust, spelled to read like Ruby: lowercase names in the generic list, no `'a` sigil (that spelling is [retired]).

If your reaction is "I have written quite a lot of Ruxen and never seen this", that's exactly the point — Chapter 4 covered everything you need 95% of the time. Come back here when the compiler asks you to spell something out.

---

## 1. A first example — and why you'd write one

Here's a method that returns the longer of two strings. It needs lifetime annotations. Example [implemented]:

```ruby
def longest[a](x: &a String, y: &a String) -> &a String
  x.size > y.size ? x : y
end

a = "hello"
b = "hi"
r = longest(a, b)
puts "#{r}"
```

Run:

```sh
ruxen run longest.rx
```

Output:

```text
hello
```

The `[a]` declares one lifetime parameter named `a`. Each `&a String` says "this reference lives for `a`". The return type `&a String` says "the returned reference also lives for `a`." The call passes `a` and `b` without a `&`: the call site borrows them for the `&String` parameters automatically [implemented].

In plain English: "all three references share the same lifetime, so the returned reference is allowed to come from either input." Without the annotation, the compiler can't tell whether the return borrows from `x` or `y`, and refuses to compile.

> **Convention:** lowercase identifiers in `[...]` are lifetimes; uppercase are type parameters. See [Chapter 12](12-generics.md) for the type-parameter side.

## 2. Recap — the borrowing rules

For context, from [Chapter 4](04-ownership-and-borrowing.md):

- **Either** any number of `&T` borrows **or** exactly one `&var T` borrow — never both [implemented].
- A borrow ends at its **last use** (non-lexical lifetimes) [implemented].
- A reference cannot outlive the value it points to [implemented].

Example [implemented]:

```ruby
data = [1, 2, 3]
view = &data
puts "#{view.size}"     # last use of view
data.push(4)            # OK — view is dead
```

The compiler tracks each borrow's last use; once a borrow dies, the value is free again.

## 3. The elision rules in plain English

You almost never write lifetimes in method signatures because the compiler fills them in. The rules [implemented]:

1. **Each input borrow gets its own lifetime parameter.** A method taking `&A` and `&B` is treated as `[a, b]` internally.
2. **If a method takes exactly one input borrow, the return borrow uses that lifetime.** So `def head(xs: &Array[Int]) -> &Int` needs no annotation.

When neither rule resolves the return borrow's lifetime, the compiler asks you to write one. That's your cue to add `[a]`.

## 4. Lifetimes mix freely with type parameters

Example [implemented]:

```ruby
def first[T, a](xs: &a Array[T]) -> &a T
  xs[0]
end
```

Convention is "types first, lifetimes after" — the parser accepts either order. The signature reads: "given a borrow of an `Array[T]` named `a`, return a borrow of one `T` with the same lifetime `a`." Indexing a borrowed array already gives a borrow of the element, so the body is just `xs[0]`; writing `&xs[0]` there loses the lifetime and is a type mismatch today.

## 5. Lifetimes on classes

When a class holds a borrow, it has to carry the lifetime parameter in its type signature. Example [implemented], with the `def init` constructor [deprecated] (replace with `def initialize` [planned](../../TODO.md#t-initialize)):

```ruby
class Slice[T, a]
  data: &a Array[T]
  start: Int
  len: Int

  def init(@data: &a Array[T], @start: Int, @len: Int)
  end

  def first -> &a T
    @data[@start]
  end
end

xs = [10, 20, 30, 40, 50]
s = Slice.new(&xs, 1, 3)
puts "#{s.first}"        # 20
```

The class cannot outlive the array it borrows from. Once `xs` would be dropped, the compiler stops any code that still references `s` — preventing a use-after-free at compile time rather than letting it crash at run time.

This is the central purpose of lifetimes on classes: **encode "I borrow from somewhere outside" in the type itself**, so the compiler can keep the borrowing rules consistent across method boundaries.

## 6. Multiple lifetimes when one isn't enough

Sometimes two inputs have genuinely independent lifetimes — only one of them needs to outlive the return. Example [implemented]:

```ruby
def split_at[T, a, b](xs: &a Array[T], at: &b Int) -> &a T
  xs[*at]
end
```

`xs` and the returned reference share lifetime `a`. `at` has its own lifetime `b`. The result has no relationship to `at` — once we've read the index, `at` is free to drop.

## 7. Borrowing across method boundaries

The rules apply transitively. When a method returns a borrow tied to one of its inputs, the borrow flows through unchanged. Example [implemented]:

```ruby
def pick(words: &Array[String], i: Int) -> &String
  words[i]
end

dict = ["apple", "banana", "cherry"]
puts "got #{pick(dict, 1)}"
```

`words` is the only input borrow, so the compiler infers that the returned `&String` borrows from `dict`; `dict` must stay alive while the result is used.

Returning an *optional* borrow (a `&String` or `nil`) from a loop over the array does not type-check yet [planned](../../TODO.md#t-optional-borrow-return); return the index, or a `clone` of the element, instead.

## 8. Writable borrows and exclusivity

A `&var T` is **exclusive**: while one exists, no other borrow (read or write) can coexist. That's what makes mutation through a reference safe — no other reference can possibly see a half-changed value. Example [implemented]:

```ruby
def push_one(buf: &var Array[Int])
  buf.push(1)
end

nums = [7]
push_one(&var nums)
push_one(&var nums)
puts "#{nums.size}"      # 3
```

The `&var nums` borrow lives only for each `push_one` call. Between calls, `nums` is fully owned by the top-level code again.

## 9. Blocks, lambdas and capture

A block passed to a method borrows from the enclosing scope for the length of the call, so it can read and change local variables. Example [implemented]:

```ruby
def three_times -> nil
  yield
  yield
  yield
end

count = 0
three_times do
  count += 1
end
puts "#{count}"               # 3
```

A lambda is different: it can be stored and called later, so it **owns** its captures (a Copy value such as an `Int` is copied in; a non-Copy value is moved). A lambda that only reads its captures can be called any number of times. Example [implemented]:

```ruby
multiplier = 3
multiply = ->(x: Int) { x * multiplier }
puts "#{multiply.(5)}"        # 15
puts "#{multiply.(10)}"       # 30
```

### When the closure must outlive its source

If the closure has to survive its source frame — typically because it's *returned* from a method — return a lambda, typed as a stored proc `Proc[(Int) -> Int]`. The lambda owns `n`, so it stays valid after `make_adder` returns. Example [implemented]:

```ruby
def make_adder(n: Int) -> Proc[(Int) -> Int]
  ->(x: Int) { x + n }
end

add_five = make_adder(5)
puts "#{add_five.(10)}"       # 15
```

A plain block can't be returned or stored: its type `Fn[(Int) -> Int]` is borrowed for one call (E1018). To make a block own its captures, write `move { |x| ... }` or `move do ... end` [implemented]. The compiler catches a borrowed capture that would outlive its frame before the program ever runs. The older closure types `some Fn(Int) -> Int` and `Fn(Int) -> Int` are [deprecated], replaced by `Proc[(Int) -> Int]` and `Fn[(Int) -> Int]`; and the empty-pipes block `{ || ... }` is [deprecated], replaced by `{ ... }`.

## 10. When to write a lifetime by hand

A practical rubric:

- The compiler asks you to. (The error names the unresolved lifetime.)
- You're writing a class that stores a borrow — its type signature needs the parameter.
- You want to express that two inputs share a lifetime that the return value uses (the `longest` example).

Don't add lifetime parameters preemptively — they cost readability and the compiler will tell you when you actually need them.

## 11. Common mistakes

- **Adding `[a]` everywhere "just in case".** The elision rules cover the common cases. Extra lifetimes clutter the signature for no benefit.
- **Returning a reference to a local.** A method `def bad -> &Int` whose body is `x = 1` then `&x` returns a borrow of a local that dies when the method returns. The compiler refuses; return by value instead.
- **Holding a `&var` while calling another method.** Any other method call on the same value would create a second borrow, which the exclusivity rule forbids. Pull the value out, or finish with the `&var` first.
- **Returning a block instead of a lambda.** If the closure is returned (or stored, or sent to another thread), it has to own its captures. Return a lambda, or a `move` block.

> **Try it:** rewrite `longest` to take three strings instead of two. What changes about the signature?

---

## Recap

- A **lifetime parameter** (`[a]`) is a label the compiler uses to track how long a reference is valid [implemented].
- The compiler **elides** lifetimes when one input maps unambiguously to the return; spell them out only when it can't.
- Classes that hold borrows carry the lifetime in their type (`Slice[T, a]`).
- `&var T` is exclusive — no other borrow can coexist.
- Blocks borrow for the call; lambdas and `move` blocks own their captures and can outlive their source frame.

**Next:** [Chapter 31 — I/O and CLI Tools](31-io-and-cli.md).
