# Collections

Three standard collections cover most day-to-day storage:

- **`Array[T]`** — a growable ordered sequence (the workhorse).
- **`Hash[K, V]`** — a key-to-value dictionary.
- **`Set[T]`** — a collection of unique values.

They use Ruby's names and Ruby's methods. Some methods still carry Rust names (`get`, `insert`); those become [deprecated] aliases of the Ruby names and are never deleted. This chapter walks through each collection, then shows the borrowing rules that apply to all of them.

## A first runnable example

Save this as `collections.rx`. Works today, with `if let` [deprecated] (replace with `if (x = v.pop)` [planned](../../TODO.md#t-narrowing)):

```ruby
v = [1, 2, 3]
puts v.size                     # 3
if let Some(x) = v.pop
  puts "popped #{x}"
end
puts v.size                     # 2
```

```bash
ruxen compile collections.rx
./collections
```

Output:

```text
3
popped 3
2
```

## `Array[T]` — the growable sequence

### Creating and adding

[implemented]:

```ruby
w = [1, 2, 3]                   # with initial values
w.push(4)
w.push(5)
puts w.size                     # 5
```

Ruby's `w << 5` is meant to be the same as `push`, but today it compiles and silently does nothing (the array keeps its old size), a [known issue](../../TODO.md#t-string-shovel). Use `push` until it is fixed.

The literal `[1, 2, 3]` builds an `Array[T]`. An empty array needs its element type: the Ruby form is the typed local `v: Array[Int] = []` [planned](../../TODO.md#t-typed-local); today write `let v: Array[Int] = []` inside a `def` ([deprecated], see Chapter 2).

There's also a fixed-size form (`[Int; 3]` for stack arrays of exactly three `Int`s), but `Array[T]` is what you reach for by default.

### Reading

[implemented]:

```ruby
w = [1, 2, 3]
first = w[0]                    # by index — panics if out of range
maybe = w.get(10)               # safe — an Int?, nil when out of range
puts first
puts maybe.nil?                 # true
```

`w[0]` is convenient and very fast; `w.get(i)` returns a `T?`, so it can't crash. `get` is the Rust name; Ruby's lookups that return `nil` (`fetch` with a default, `dig`) are not in the stdlib yet [planned](../../TODO.md#t-collection-fetch). Indexing will become an ordinary `.rx` method, with `[]=` writing through a declared mutable place [planned](../../TODO.md#t-d1).

### Querying

[implemented]:

```ruby
w = [1, 2, 3]
puts w.size                     # number of elements
puts w.empty?                   # true if no elements
```

`size` returns a `USize` today; it will return an `Int` [planned](../../TODO.md#t-size-int).

### Iterating with blocks

Works today, with a tuple destructuring `let` [deprecated] (replace with `evens, odds = nums.partition { ... }` [planned](../../TODO.md#t-parallel-assign)):

```ruby
def tour
  nums = [1, 2, 3, 4, 5]

  nums.each { |n| puts n }                       # side effects
  doubled = nums.map { |n| n * 2 }               # transform
  evens = nums.select { |n| n % 2 == 0 }         # subset
  first_big = nums.find { |n| n > 3 }            # Int? — first match or nil
  let (even, odd) = nums.partition { |n| n % 2 == 0 }
  puts even.size
  puts nums.sum                                  # 15
end

tour()
```

`.each`, `.map`, `.select`, `.find`, `.partition` are part of the `Enumerable` vocabulary that works on every collection. Blocks power them — see Chapter 9. `sum` adds up a numeric array today; Ruby's `sum(init)` for any element type, plus the no-argument form for numeric elements, is [planned](../../TODO.md#t-d15).

## `Hash[K, V]` — key-to-value lookup

Works today, with `if let` [deprecated] (replace with `if (v = h.get("a"))` [planned](../../TODO.md#t-narrowing)):

```ruby
h = { "a" => 1, "b" => 2 }
if let Some(v) = h.get("a")
  puts "a=#{v}"
end
if h.get("z").nil?
  puts "z=missing"
end
```

Output:

```text
a=1
z=missing
```

### Building and inserting

The Ruby form starts from an empty `{}` [planned](../../TODO.md#t-empty-hash) (today `{}` parses as an empty block):

```ruby
h: Hash[String, Int] = {}
h["a"] = 1
h["b"] = 2
```

Works today: a typed `let` with `Hash.new` inside a `def` [deprecated] (replace with the typed local above), and `insert`, the Rust name of `[]=`:

```ruby
def build
  let h: Hash[String, Int] = Hash.new
  h["a"] = 1
  h.insert("b", 2)
  puts h.size
end

build()
```

Give `Hash.new` its type: an untyped `h = Hash.new` panics on its first insert, a [known issue](../../TODO.md#t-hash-new-key). The literal form `{ "a" => 1, "b" => 2 }` builds the same thing in one step. Ruby's symbol-key form `{ name: "Ada" }` needs symbols [planned](../../TODO.md#t-symbols).

### Reading

[implemented]:

```ruby
h = { "a" => 1 }
val = h.get("a")                # Int? — safe
also = h["a"]                   # Int — panics if key is missing
puts also
```

Prefer `.get` unless you can prove the key exists.

### Querying

[implemented]:

```ruby
h = { "a" => 1 }
puts h.size
puts h.key?("a")
```

### Iterating

Works today: a `for` loop with a tuple pattern [deprecated] (replace with `h.each { |k, v| ... }`, which takes one block parameter today, [planned](../../TODO.md#t-hash-each-pair)):

```ruby
h = { "a" => 1, "b" => 2 }
for (k, v) in &h
  puts "#{k} -> #{v}"
end
```

## `Set[T]` — unique values

Works today, with a typed `let` and `insert` [deprecated] (replace with a typed local [planned](../../TODO.md#t-typed-local) and `s << x`, which today compiles but does nothing, a [known issue](../../TODO.md#t-string-shovel)):

```ruby
def demo
  let s: Set[Int] = Set.new
  s.insert(1)
  s.insert(2)
  s.insert(1)                   # no effect, already present
  puts s.size                   # 2
  if s.include?(1)
    puts "has 1"
  end
  if s.include?(3)
    puts "has 3"
  else
    puts "no 3"
  end
end

demo()
```

Output:

```text
2
has 1
no 3
```

`s.insert(x)` is the Rust name of `s << x`; use it until `<<` works on a set. A set is a hash with no values — useful when you care about membership but not associated data.

## Strings as collections

Strings support iteration over characters [implemented]:

```ruby
greeting = "Hello"

for ch in greeting.chars
  puts ch
end

puts greeting.size          # byte length
```

Splitting [implemented]:

```ruby
for word in "one two three".split(" ")
  puts word
end
```

Chapter 29 covers strings, bytes, and numbers in detail.

## Borrowing rules apply

Collections are normal owned values. The same ownership and borrowing rules from Chapter 4 apply [implemented]:

```ruby
names = ["Alice", "Bob", "Charlie"]

# Iterate by reference — names still valid after
for name in &names
  puts name
end
puts names.size             # OK, names still owned
```

If you iterate by value, you move each element out — the collection is used up [implemented]:

```ruby
names = ["Alice", "Bob"]
for name in names           # iterates by value, MOVES
  puts name
end
# names is no longer usable
```

For a Copy element type (e.g. `Array[Int]`), iterating by value is fine — integers copy instead of moving. Block methods such as `names.each { |n| ... }` borrow the collection for you.

## Common mistakes

**Indexing with `[]` when the key might be missing.** `[]` panics on a missing key/index. Use `.get` to get a `T?` back.

**Iterating by value when you wanted a reference.** This is a classic — `for name in names` moves out of the array. Write `for name in &names`, or use `names.each { |name| ... }`.

**Mutating while iterating.** Adding to or removing from a collection while iterating over it is a borrow-rule violation — the loop borrows the collection, and `.push` needs a writable borrow. Build the changes in a separate list, then apply them after the loop.

**Using unsuitable types as `Hash`/`Set` keys.** Keys need hashing and equality. Value types of hashable fields qualify implicitly; for other types, the Ruby protocol is `def hash -> Int` plus `def eql?`, with `eql?` defaulting to `==` [planned](../../TODO.md#t-d8). Today that is `include Hashable` with a `hash_code` method, which is [deprecated] (`hash_code` becomes an alias of `hash`).

## Try it

1. Build an `Array[Int]` of the numbers 1 to 10. Use `.select` to get the evens, `.map` to double them, then `.each` to print them.
2. Build a `Hash[String, Int]` recording the count of each word in a string. Iterate the words with `.split`, and update counts with `h[word] = ...`.
3. Build a `Set[Int]` from the same input and print its `.size` — that's the count of distinct values.

## Recap

- `Array[T]` is the everyday growable sequence; `[1, 2, 3]` builds one; `Hash[K, V]` and `Set[T]` round out the trio.
- Use `.get` for safe lookup; `[]` panics on missing keys.
- The `Enumerable` vocabulary (`.each`, `.map`, `.select`, `.find`, `.partition`) takes blocks and chains cleanly.
- Borrowing rules apply: `for x in &v` borrows; `for x in v` moves.
- Hash and Set keys need `hash` and `eql?` (planned; `Hashable` today).

**Next:** [FFI](14-ffi.md) — calling C code and exposing Ruxen types to C.
