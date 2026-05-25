# Collections

## Array (Growable Sequence)

The most common collection — a growable, heap-allocated sequence.

```ruxen
# Create
var v = Array.new              # empty
let v = [1, 2, 3]        # with initial values

# Add elements
v.push(4)

# Access
let first = v[0]               # by index
let maybe = v.get(10)          # returns Option[&T] (safe)

# Query
v.len                          # number of elements
v.is_empty                     # true if empty
```

### Iterating

```ruxen
let nums = [1, 2, 3, 4, 5]

# Iterate
nums.each { |n| puts n }

# Transform
let doubled = nums.map { |n| n * 2 }

# Filter
let evens = nums.filter { |n| n % 2 == 0 }

# Find
let first_big = nums.find { |n| n > 3 }    # Option[&Int]

# Partition
let (evens, odds) = nums.partition { |n| n % 2 == 0 }
```

## Map (Key-Value)

```ruxen
var h: Map[String, Int] = Map.new
let h = { "a" => 1, "b" => 2, "c" => 3 }

# Insert / update
h.insert("d", 4)

# Access
let val = h.get("a")           # Option[&Int]
let val = h["a"]               # panics if key missing

# Query
h.len
h.contains_key("a")
```

## Set (Unique Values)

```ruxen
var s: Set[Int] = Set.new

s.insert(1)
s.insert(2)
s.insert(1)                    # no effect, already present

s.contains(1)                  # true
s.len                          # 2
```

## String as a Collection

Strings support iteration:

```ruxen
let greeting = "Hello"

# Iterate over characters
for ch in greeting.chars
  puts ch
end

# Split into parts
for word in greeting.split(" ")
  puts word
end

# Length
greeting.len                   # byte length
greeting.char_count            # Unicode scalar count
```

## Owned vs Borrowed Access

Collections follow ownership rules:

```ruxen
let names = ["Alice", "Bob", "Charlie"]

# Borrowing elements
for name in &names             # iterate by reference
  puts name
end
puts names.len                 # names still valid

# Moving elements
for name in names              # iterate by value (moves)
  puts name
end
# names is no longer valid
```
