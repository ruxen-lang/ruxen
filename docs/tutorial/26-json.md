# JSON

You need to read a config file, send some data to a web service, or store structured information on disk — and JSON is the lingua franca for all of those. The `std.json` module gives you a small focused API: parse a string into a value-tree, walk it to read fields, build new values programmatically, and stringify back out. This chapter walks through each step and finishes with a full round-trip.

Three pieces to know:

- **`Json`** — an opaque, heap-allocated value-tree node. Every JSON value — object, array, string, number, bool, null — is represented by one.
- **`JsonKind`** — a tag (`Object`, `Array`, `String`, `Number`, `Bool`, `Null`) used when you want to ask "what kind is this node?".
- **`JSON`** — a namespace class with all the parse / inspect / build / stringify operations as `def self.*` methods.

All three are available without a `require` [implemented]. The old `use std.json.{JSON, Json, JsonKind}` import is [deprecated], replace it with the bare names.

Parsing and stringifying return a `Result`, and the accessors return an optional value. The examples take them apart with `case/in` and the `Ok(...)`, `Err(...)` and `Some(...)` patterns, which are [deprecated]: errors become Ruby exceptions [planned](../../TODO.md#t-exceptions), and an optional value is matched with `in nil` and a plain binding (today a binding arm after `in nil` still binds the optional value [known issue](../../TODO.md#t-case-in-nil), so the examples keep `Some(...)`).

---

## 1. A complete round-trip

Save as `round_trip.rx`. Works today [deprecated] (`Result` and `Some` patterns, replaced as described above):

```ruby
def show_name(root: &Json)
  case JSON.object_get(root, "name")
  in Some(v) then
    case JSON.as_string(v)
    in Some(s) then puts "name = #{s}"
    in nil then puts "name not a string"
    end
  in nil then puts "no name"
  end
end

source = "{\"name\": \"ruxen\", \"version\": 1}"
case JSON.parse(source)
in Ok(root) then
  case JSON.stringify(root)
  in Ok(back) then puts back
  in Err(_) then puts "stringify failed"
  end
  show_name(root)
in Err(_) then puts "parse failed"
end
```

Run it:

```sh
ruxen run round_trip.rx
```

Output:

```text
{"name":"ruxen","version":1}
name = ruxen
```

Two operations, both via `JSON.*` static methods: `parse` turned the string into a tree; `stringify` turned the tree back into a compact string. The last bit walked into the object to fish out one field. It comes last on purpose: today the value `JSON.object_get` returns is freed when it goes out of scope even though it still belongs to the tree, so any later use of the tree reads freed memory [known issue](../../TODO.md#t-json-get-borrow). Until that is fixed, finish with a tree before you read fields from it, and read each field once. The rest of the chapter is a tour of those operations.

## 2. Parsing

Two parser entry points. Works today [deprecated] (`Result` with `Ok` / `Err` patterns, replaced by exceptions [planned](../../TODO.md#t-exceptions)):

```ruby
source = "{\"name\": \"ruxen\", \"answer\": 42}"
case JSON.parse(source)
in Ok(root) then puts "parsed"
in Err(_) then puts "fail"
end
```

- **`JSON.parse(input)`** [implemented] — relaxed parser, returning `Result[Json, JsonError]`. Accepts `// line` and `/* block */` comments, trailing commas, and a few config-file niceties. Good for reading hand-written config files.
- **`JSON.parse_strict(input)`** [implemented] — RFC-style strict, same return type. No comments, no trailing commas, no NaN / Infinity. Good for wire data from untrusted sources.

`JsonError` has cases `Syntax(message)`, `DepthLimit`, `InvalidUtf8`, and `NumberOutOfRange`.

## 3. Inspecting a parsed value

Every accessor takes a borrowed `&Json` (the call site passes the value; the borrow is automatic). To find out what kind a node is, use `JSON.kind(v)` or one of the `is_*` predicates. Works today [deprecated] (`Result` with `Ok` / `Err` patterns, replaced by exceptions [planned](../../TODO.md#t-exceptions)):

```ruby
source = "{\"items\": [1, 2, 3]}"
case JSON.parse(source)
in Ok(root) then
  puts "is_object=#{JSON.is_object(root)}"
  puts "has_items=#{JSON.object_has(root, "items")}"
  case JSON.kind(root)
  in JsonKind.Object then puts "kind is object"
  in _ then puts "other kind"
  end
in Err(_) then puts "parse failed"
end
```

### Object surface

- `JSON.object_has(v, key)` [implemented] — a `Bool`.
- `JSON.object_len(v)` [implemented] — the number of keys, or `nil` if `v` is not an object.
- `JSON.object_get(v, key)` [implemented] — the value under `key`, or `nil`.

### Array surface

- `JSON.array_len(v)` [implemented] — the number of elements, or `nil` if `v` is not an array.
- `JSON.array_get(v, index)` [implemented] — the element at `index`, or `nil`.

Both `object_get` and `array_get` hand back a child that the tree still owns, and today that child is freed when your copy goes out of scope, which corrupts the tree [known issue](../../TODO.md#t-json-get-borrow). Use them as the last reads of a tree, once per field, as in section 1.

### Scalar extraction

Each returns an optional value (`T?`) — `nil` when the underlying kind doesn't match. All [implemented]:

- `JSON.as_bool(v)` — `Bool?`
- `JSON.as_int(v)` — `Int?`
- `JSON.as_float(v)` — `Float?`
- `JSON.as_string(v)` — `String?`

Works today [deprecated] (the `Some` pattern, replaced by a plain binding after `in nil` once the [known issue](../../TODO.md#t-case-in-nil) is fixed):

```ruby
def show_int(v: &Json, label: &String)
  case JSON.as_int(v)
  in Some(n) then puts "#{label}=#{n}"
  in nil then puts "#{label}=missing"
  end
end
```

## 4. Building JSON

Scalar builders return a fresh `Json`. Example [implemented]:

```ruby
n = JSON.null_value
b = JSON.bool(true)
i = JSON.int(7)
f = JSON.float(1.5)
s = JSON.string("hi")
```

Composite builders take ownership of an `Array[Json]` or `Hash[String, Json]`. Example [implemented]:

```ruby
items = [JSON.int(1), JSON.int(2)]
arr = JSON.array(items)

fields = { "name" => JSON.string("ruxen"), "items" => arr }
obj = JSON.object(fields)
```

Shortcuts for empty containers, both [implemented]:

- `JSON.empty_array` — produces `[]`
- `JSON.empty_object` — produces `{}`

## 5. Stringifying

Two modes. Works today [deprecated] (`Result` with `Ok` / `Err` patterns, replaced by exceptions [planned](../../TODO.md#t-exceptions)):

```ruby
value = JSON.bool(true)

case JSON.stringify(value)
in Ok(s) then puts s
in Err(_) then puts "fail"
end

case JSON.stringify_pretty(value, 2)
in Ok(s) then puts s
in Err(_) then puts "fail"
end
```

- `JSON.stringify(v)` [implemented] — compact, no whitespace; returns `Result[String, JsonError]`.
- `JSON.stringify_pretty(v, indent)` [implemented] — `indent` is spaces per level (2 or 4 typical).

## 6. A full build-and-stringify

Works today [deprecated] (`Result` and `Some` patterns, replaced as described at the top of the chapter):

```ruby
def show_title(round: &Json)
  case JSON.object_get(round, "title")
  in Some(title) then
    case JSON.as_string(title)
    in Some(v) then puts "title=#{v}"
    in nil then puts "title=missing"
    end
  in nil then puts "title=missing"
  end
end

items = [JSON.int(1), JSON.int(2)]
arr = JSON.array(items)
fields = { "title" => JSON.string("demo"), "items" => arr }
obj = JSON.object(fields)

case JSON.stringify(obj)
in Ok(text) then
  puts text
  case JSON.parse(text)
  in Ok(round) then show_title(round)
  in Err(_) then puts "parse failed"
  end
in Err(_) then puts "stringify failed"
end
```

Output (the exact field order of the first line depends on `Hash` insertion order):

```text
{"title":"demo","items":[1,2]}
title=demo
```

## 7. Common mistakes

- **Calling `as_int` on a node that was parsed as a float.** JSON numbers with a `.` parse as `Float`-tagged nodes; `as_int` returns `nil`. Use `as_float` and convert if you want an integer view.
- **`unwrap!` ([deprecated]) on the parse `Result` for wire data.** Even strict-mode JSON from a trusted source can be truncated. Match and report a clean error instead.
- **Assuming key order in objects.** The stringifier emits keys in iteration order of the source `Hash`, which is insertion order. Don't rely on this for cross-language protocol compatibility — JSON itself says key order is unspecified.
- **Mutating an existing `Json`.** The API is build-then-stringify, not mutate-in-place. To change a field, build a new object containing the new field set.

> **Try it:** modify the round-trip example to also pull out the `items` array and print its length with `JSON.array_len`.

---

## Recap

- `JSON.parse(s)` for relaxed input; `JSON.parse_strict(s)` for wire data [implemented].
- `JSON.kind`, `JSON.is_object`, `JSON.object_get`, `JSON.array_len`, `JSON.array_get` to inspect [implemented].
- `JSON.as_int / as_float / as_string / as_bool` to extract scalars — each returns an optional value [implemented].
- `JSON.int / string / array / object` to build new `Json` values [implemented].
- `JSON.stringify` (compact) or `JSON.stringify_pretty(v, 2)` (indented) to emit [implemented].

**Next:** [Chapter 27 — Ruxen.toml and Dependencies](27-manifest-and-deps.md).
