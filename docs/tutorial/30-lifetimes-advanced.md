# Lifetimes and Borrowing in Depth

Most of the time you don't write lifetimes — the compiler infers them. This chapter is for the small set of cases where you need to be explicit: functions that return a reference, classes that hold a borrow, generic types where the compiler can't guess the relationship for itself.

A **lifetime** is just a label the compiler uses to track "how long does this reference stay valid?" In `&Array[Int]`, the lifetime is invisible — the compiler picks one. In `&a Array[Int]`, the `a` is a name you've given to that lifetime so you can refer to it elsewhere in the signature.

If your reaction is "I have written quite a lot of Ruxen and never seen this", that's exactly the point — Chapter 4 covered everything you need 95% of the time. Come back here when the compiler asks you to spell something out.

---

## 1. A first example — and why you'd write one

Here's a function that returns the longer of two strings. It needs lifetime annotations:

```ruxen
def longest[a](x: &a String, y: &a String) -> &a String
  if x.len > y.len
    x
  else
    y
  end
end

def main
  let a = String.from("hello")
  let b = String.from("hi")
  let r = longest(&a, &b)
  puts "#{r}"
end
```

Run:

```bash
ruxen run longest.rx
```

Output:

```
hello
```

The `[a]` declares one lifetime parameter named `a`. Each `&a String` says "this reference lives for `a`". The return type `&a String` says "the returned reference also lives for `a`."

In plain English: "all three references share the same lifetime, so the returned reference is allowed to come from either input." Without the annotation, the compiler can't tell whether the return borrows from `x` or `y`, and refuses to compile.

> **Convention:** lowercase identifiers in `[...]` are lifetimes; uppercase are type parameters. See [Chapter 12](12-generics.md) for the type-parameter side.

## 2. Recap — the borrowing rules

For context, from [Chapter 4](04-ownership-and-borrowing.md):

- **Either** any number of `&T` borrows **or** exactly one `&var T` borrow — never both.
- A borrow ends at its **last use** (non-lexical lifetimes).
- A reference cannot outlive the value it points to.

```ruxen
def main
  var data = [1, 2, 3]
  let view = &data
  puts "#{view.len}"     # last use of view
  data.push(4)            # OK — view is dead
end
```

The compiler tracks each borrow's last use; once a borrow dies, the value is free again.

## 3. The elision rules in plain English

You almost never write lifetimes in function signatures because the compiler fills them in. The rules:

1. **Each input borrow gets its own lifetime parameter.** A function taking `&A` and `&B` is treated as `[a, b]` internally.
2. **If a function takes exactly one input borrow, the return borrow uses that lifetime.** So `def head(xs: &Array[Int]) -> &Int` needs no annotation.

When neither rule resolves the return borrow's lifetime, the compiler asks you to write one. That's your cue to add `[a]`.

## 4. Lifetimes mix freely with type parameters

```ruxen
def first[T, a](xs: &a Array[T]) -> &a T
  &xs[0]
end
```

Convention is "types first, lifetimes after" — the parser accepts either order. The signature reads: "given a borrow of an `Array[T]` named `a`, return a borrow of one `T` with the same lifetime `a`."

## 5. Lifetimes on classes

When a class holds a borrow, it has to carry the lifetime parameter in its type signature:

```ruxen
class Slice[T, a]
  data: &a Array[T]
  start: USize
  len: USize

  def init(@data: &a Array[T], @start: USize, @len: USize)
  end

  def first -> &a T
    &self.data[self.start]
  end
end

def main
  let xs = [10, 20, 30, 40, 50]
  let s = Slice.new(&xs, 1, 3)
  puts "#{s.first}"        # 20
end
```

The class cannot outlive the array it borrows from. Once `xs` would be dropped, the compiler stops any code that still references `s` — preventing a use-after-free at compile time rather than letting it crash at run time.

This is the central purpose of lifetimes on classes: **encode "I borrow from somewhere outside" in the type itself**, so the compiler can keep the borrowing rules consistent across function boundaries.

## 6. Multiple lifetimes when one isn't enough

Sometimes two inputs have genuinely independent lifetimes — only one of them needs to outlive the return:

```ruxen
def split_at[T, a, b](xs: &a Array[T], at: &b USize) -> &a T
  &xs[*at]
end
```

`xs` and the returned reference share lifetime `a`. `at` has its own lifetime `b`. The result has no relationship to `at` — once we've read the index, `at` is free to drop.

## 7. Borrowing across function boundaries

The rules apply transitively. When a function returns a borrow tied to one of its inputs, the borrow flows through unchanged:

```ruxen
def find(words: &Array[String], target: &String) -> Option[&String]
  for w in words
    if w == target
      return Some(w)
    end
  end
  nil
end

def main
  let dict = ["apple", "banana", "cherry"]
  let needle = String.from("banana")
  match find(&dict, &needle)
    Some(found) -> puts "got #{found}"
    None        -> puts "missing"
  end
end
```

The compiler infers that the returned `&String` borrows from `dict` (not from `needle`), so `needle` could drop first without invalidating the result.

## 8. Writable borrows and exclusivity

A `&var T` is **exclusive**: while one exists, no other borrow (read or write) can coexist. That's what makes mutation through a reference safe — no other reference can possibly see a half-changed value.

```ruxen
def push_one(buf: &var Array[Int])
  buf.push(1)
end

def main
  var data: Array[Int] = Array.new
  push_one(&var data)
  push_one(&var data)
  puts "#{data.len}"      # 2
end
```

The `&var data` borrow lives only for each `push_one` call. Between calls, `data` is fully owned by `main` again.

## 9. Closures and capture

Closures borrow from the enclosing scope by default. A read-only closure can be called any number of times:

```ruxen
def main
  let multiplier = 3
  let multiply = { |x: Int| x * multiplier }
  puts "#{multiply.(5)}"        # 15
  puts "#{multiply.(10)}"       # 30
end
```

A closure that mutates a captured variable needs the variable to be `var` and the closure binding itself to be `var`:

```ruxen
def main
  var count = 0
  var bump = { || count += 1 }
  bump.()
  bump.()
  bump.()
  puts "#{count}"               # 3
end
```

### When the closure must outlive its source — `move`

If the closure has to survive its source frame — typically because it's *returned* from a function — use a `move` closure. `move` transfers ownership of the captured values into the closure itself:

```ruxen
def make_adder(n: Int) -> some Fn(Int) -> Int
  move { |x| x + n }
end

def main
  let add_five = make_adder(5)
  puts "#{add_five.(10)}"       # 15
end
```

Without `move`, the closure would try to borrow `n` from `make_adder`'s stack frame — but that frame disappears the moment `make_adder` returns. The compiler catches this before the program ever runs.

## 10. When to write a lifetime by hand

A practical rubric:

- The compiler asks you to. (The error names the unresolved lifetime.)
- You're writing a class that stores a borrow — its type signature needs the parameter.
- You want to express that two inputs share a lifetime that the return value uses (the `longest` example).

Don't add lifetime parameters preemptively — they cost readability and the compiler will tell you when you actually need them.

## 11. Common mistakes

- **Adding `[a]` everywhere "just in case".** The elision rules cover the common cases. Extra lifetimes clutter the signature for no benefit.
- **Returning a reference to a local.** `def bad() -> &Int { let x = 1; &x }` — the local dies when the function returns. The compiler refuses; return by value instead.
- **Holding a `&var` while calling another method.** Any other method call on the same value would create a second borrow, which the exclusivity rule forbids. Pull the value out, or finish with the `&var` first.
- **Forgetting `move` on a closure that escapes.** If the closure is returned (or stored, or sent to another thread), it has to own its captures. `move` is how you say so.

> **Try it:** rewrite `longest` to take three strings instead of two. What changes about the signature?

---

## Recap

- A **lifetime parameter** (`[a]`) is a label the compiler uses to track how long a reference is valid.
- The compiler **elides** lifetimes when one input maps unambiguously to the return; spell them out only when it can't.
- Classes that hold borrows carry the lifetime in their type (`Slice[T, a]`).
- `&var T` is exclusive — no other borrow can coexist.
- Closures borrow by default; use `move` when the closure must outlive its source frame.

**Next:** [Chapter 31 — I/O and CLI Tools](31-io-and-cli.md).
