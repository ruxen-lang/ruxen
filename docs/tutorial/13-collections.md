# Collections

Three standard collections cover most day-to-day storage:

- **`Array[T]`** — a growable ordered sequence (the workhorse).
- **`Hash[K, V]`** — a key-to-value dictionary.
- **`Set[T]`** — a collection of unique values.

This chapter walks through each, then shows the borrowing rules that apply to all of them.

## A first runnable example

```ruxen
def main
  var v: Array[Int] = Array.new
  v.push(1)
  v.push(2)
  v.push(3)

  puts "#{v.size}"               # 3
  match v.pop
    Some(x) -> puts "popped #{x}"
    nil     -> puts "empty"
  end
  puts "#{v.size}"               # 2
end
```

```bash
ruxen compile collections.rx
./collections
```

Output:

```
3
popped 3
2
```

## `Array[T]` — the growable sequence

### Creating and adding

```ruxen
var v: Array[Int] = Array.new        # empty
let w = [1, 2, 3]                    # with initial values
v.push(4)
```

The literal `[1, 2, 3]` builds an `Array[T]`. There's also a fixed-size form (`[Int; 3]` for stack arrays of exactly three `Int`s), but `Array[T]` is what you reach for by default.

### Reading

```ruxen
let first = w[0]                     # by index — panics if out of range
let maybe = w.get(10)                # safe — Option[&T]
```

`w[0]` is convenient and very fast; `w.get(i)` returns an `Option`, so it can't crash.

### Querying

```ruxen
w.size                                # number of elements
w.empty?                           # true if no elements
```

### Iterating with closures

```ruxen
def main
  let nums = [1, 2, 3, 4, 5]

  nums.each { |n| puts "#{n}" }                  # side effects
  let doubled = nums.map { |n| n * 2 }           # transform
  let evens = nums.select { |n| n % 2 == 0 }     # subset
  let first_big = nums.find { |n| *n > 3 }       # Option[&Int]
  let (evens, odds) = nums.partition { |n| n % 2 == 0 }
end
```

`.each`, `.map`, `.select`, `.find`, `.partition` are part of the Enumerable vocabulary that works on every iterable type. Closures power them — see Chapter 9.

## `Hash[K, V]` — key-to-value lookup

```ruxen
def main
  let h = { "a" => 1, "b" => 2 }
  match h.get("a")
    Some(v) -> puts "a=#{v}"
    nil     -> puts "a=missing"
  end
  match h.get("z")
    Some(v) -> puts "z=#{v}"
    nil     -> puts "z=missing"
  end
end
```

Output:

```
a=1
z=missing
```

### Building and inserting

```ruxen
var h: Hash[String, Int] = Hash.new
h.insert(String.from("a"), 1)
h.insert(String.from("b"), 2)
```

The literal form `{ "a" => 1, "b" => 2 }` is shorthand for the same thing.

### Reading

```ruxen
let val = h.get("a")            # Option[&V] — safe
let val = h["a"]                # &V — panics if key is missing
```

Prefer `.get` unless you can prove the key exists.

### Querying

```ruxen
h.size
h.key?("a")
```

### Iterating

```ruxen
for (k, v) in &h
  puts "#{k} -> #{v}"
end
```

## `Set[T]` — unique values

```ruxen
def main
  var s: Set[Int] = Set.new
  s.insert(1)
  s.insert(2)
  s.insert(1)                   # no effect, already present
  puts "#{s.size}"               # 2
  if s.include?(1)
    puts "has 1"
  end
  if s.include?(3)
    puts "has 3"
  else
    puts "no 3"
  end
end
```

Output:

```
2
has 1
no 3
```

A set is a map with no values — useful when you care about membership but not associated data.

## Strings as collections

Strings support iteration over characters:

```ruxen
def main
  let greeting = "Hello"

  for ch in greeting.chars
    puts "#{ch}"
  end

  puts "#{greeting.size}"          # byte length
end
```

Splitting:

```ruxen
for word in "one two three".split(" ")
  puts word
end
```

Chapter 29 covers strings, bytes, and numbers in detail.

## Borrowing rules apply

Collections are normal owned values. The same ownership and borrowing rules from Chapter 4 apply:

```ruxen
def main
  let names = [String.from("Alice"), String.from("Bob"), String.from("Charlie")]

  # Iterate by reference — names still valid after
  for name in &names
    puts name
  end
  puts "#{names.size}"          # OK, names still owned
end
```

If you iterate by value, you move each element out — the collection becomes empty:

```ruxen
for name in names              # iterates by value, MOVES
  puts name
end
# names is no longer usable
```

For a Copy element type (e.g. `Array[Int]`), iterating by value is fine — primitives copy instead of moving.

## Common mistakes

**Indexing with `[]` when the key might be missing.** `[]` panics on a missing key/index. Use `.get` to get an `Option` back:

```ruxen
let x = h["a"]                  # panic if absent
let x = h.get("a")              # Option[&V]
```

**Iterating by value when you wanted a reference.** This is a classic — `for name in names` moves out of the array. Write `for name in &names` to iterate by reference.

**Mutating while iterating.** Adding to or removing from a collection while iterating over it is a borrow-rule violation — the iterator borrows the collection, and `.push` needs a writable borrow. Build the changes in a separate list, then apply them after the loop.

**Using non-Hashable types as `Hash`/`Set` keys.** Keys must include the `Hashable` mixin. Structs of hashable fields qualify implicitly; classes and custom types might need an explicit `include Hashable` plus a `def hash` method.

## Try it

1. Build an `Array[Int]` of the numbers 1 to 10. Use `.select` to get the evens, `.map` to double them, then `.each` to print them.
2. Build a `Hash[String, Int]` recording the count of each word in a string. Iterate the words with `.split`, use `.get` + `.insert` (or a get-or-default helper).
3. Build a `Set[Int]` from the same input and print its `.size` — that's the count of distinct values.

## Recap

- `Array[T]` is the everyday growable sequence; `[1, 2, 3]` builds one; `Hash[K, V]` and `Set[T]` round out the trio.
- Use `.get` for safe lookup; `[]` panics on missing keys.
- The Enumerable vocabulary (`.each`, `.map`, `.select`, `.find`, `.partition`) takes closures and chains cleanly.
- Borrowing rules apply: `for x in &v` borrows; `for x in v` moves.
- Hash/Set keys must be `Hashable`.

**Next:** [FFI](14-ffi.md) — calling C code and exposing Ruxen types to C.
