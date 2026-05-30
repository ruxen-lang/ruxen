# JSON

You need to read a config file, send some data to a web service, or store structured information on disk — and JSON is the lingua franca for all of those. The `std.json` module gives you a small focused API: parse a string into a value-tree, walk it to read fields, build new values programmatically, and stringify back out. This chapter walks through each step and finishes with a full round-trip.

Three pieces to know:

- **`Json`** — an opaque, heap-allocated value-tree node. Every JSON value — object, array, string, number, bool, null — is represented by one.
- **`JsonKind`** — an enum tag (`Object`, `Array`, `String`, `Number`, `Bool`, `Null`) used when you want to ask "what kind is this node?".
- **`JSON`** — a namespace class with all the parse / inspect / build / stringify operations as `def self.*` methods.

```ruxen
use std.json.{JSON, Json, JsonKind}
```

---

## 1. A complete round-trip

Save as `round_trip.rx`:

```ruxen
use std.json.{JSON, Json}

def main
  let source = String.from("{\"name\": \"ruxen\", \"version\": 1}")
  let root = JSON.parse(&source).expect!("parse")

  match JSON.object_get(&root, "name")
    Some(v) -> match JSON.as_string(&v)
      Some(s) -> puts "name = #{s}"
      None    -> puts "name not a string"
    end
    None -> puts "no name"
  end

  let back = JSON.stringify(&root).expect!("stringify")
  puts back
end
```

Run it:

```bash
ruxen run round_trip.rx
```

Output:

```
name = ruxen
{"name":"ruxen","version":1}
```

Two operations, both via `JSON.*` static methods: `parse` turned the string into a tree; `stringify` turned the tree back into a compact string. The middle bit walked into the object to fish out one field. The rest of the chapter is a tour of those operations.

## 2. Parsing

Two parser entry points:

```ruxen
def main
  let source = String.from("{\"name\": \"ruxen\", \"answer\": 42}")
  match JSON.parse(&source)
    Ok(root) -> puts "parsed"
    Err(_)   -> puts "fail"
  end
end
```

- **`JSON.parse(input: &String) -> Result[Json, JsonError]`** — relaxed parser. Accepts `// line` and `/* block */` comments, trailing commas, and a few config-file niceties. Good for reading hand-written config files.
- **`JSON.parse_strict(input: &String) -> Result[Json, JsonError]`** — RFC-style strict. No comments, no trailing commas, no NaN / Infinity. Good for wire data from untrusted sources.

`JsonError` is an enum with variants `Syntax(message)`, `DepthLimit`, `InvalidUtf8`, and `NumberOutOfRange`.

## 3. Inspecting a parsed value

Every accessor takes a borrowed `&Json`. To find out what kind a node is, use `JSON.kind(&v)` or one of the `is_*` predicates:

```ruxen
def main
  let source = String.from("{\"items\": [1, 2, 3]}")
  let root = JSON.parse(&source).expect!("parse")

  puts "is_object=#{JSON.is_object(&root)}"
  puts "has_items=#{JSON.object_has(&root, "items")}"

  match JSON.kind(&root)
    JsonKind.Object -> puts "kind is object"
    _               -> puts "other kind"
  end
end
```

### Object surface

- `JSON.object_has(v: &Json, key: &String) -> Bool`
- `JSON.object_len(v: &Json) -> Option[Int]`
- `JSON.object_get(v: &Json, key: &String) -> Option[Json]`

### Array surface

- `JSON.array_len(v: &Json) -> Option[Int]`

(Walking array elements by index is not in v1 — collect into an `Array[Json]` builder-side if you need to iterate.)

### Scalar extraction

Each returns `Option[T]` — `None` when the underlying kind doesn't match:

- `JSON.as_bool   (v: &Json) -> Option[Bool]`
- `JSON.as_int    (v: &Json) -> Option[Int]`
- `JSON.as_float  (v: &Json) -> Option[Float]`
- `JSON.as_string (v: &Json) -> Option[String]`

```ruxen
def show_int(v: Json, label: &String)
  match JSON.as_int(&v)
    Some(n) -> puts "#{label}=#{n}"
    None    -> puts "#{label}=missing"
  end
end
```

## 4. Building JSON

Scalar builders return a fresh `Json`:

```ruxen
let n = JSON.null_value
let b = JSON.bool(true)
let i = JSON.int(7)
let f = JSON.float(1.5)
let s = JSON.string("hi")
```

Composite builders take ownership of an `Array[Json]` or `Map[String, Json]`:

```ruxen
var items: Array[Json] = Array.new
items.push(JSON.int(1))
items.push(JSON.int(2))
let arr = JSON.array(items)

var fields: Map[String, Json] = Map.new
fields.insert(String.from("name"), JSON.string("ruxen"))
fields.insert(String.from("items"), arr)
let obj = JSON.object(fields)
```

Shortcuts for empty containers:

- `JSON.empty_array  -> Json`  (produces `[]`)
- `JSON.empty_object -> Json`  (produces `{}`)

## 5. Stringifying

Two modes:

```ruxen
def main
  let value = JSON.bool(true)

  match JSON.stringify(&value)
    Ok(s)  -> puts s
    Err(_) -> puts "fail"
  end

  match JSON.stringify_pretty(&value, 2)
    Ok(s)  -> puts s
    Err(_) -> puts "fail"
  end
end
```

- `JSON.stringify(v: &Json) -> Result[String, JsonError]` — compact, no whitespace.
- `JSON.stringify_pretty(v: &Json, indent: Int) -> Result[String, JsonError]` — `indent` is spaces per level (2 or 4 typical).

## 6. A full build-and-stringify

```ruxen
use std.json.{JSON, Json}

def main
  var items: Array[Json] = Array.new
  items.push(JSON.int(1))
  items.push(JSON.int(2))
  let arr = JSON.array(items)

  var fields: Map[String, Json] = Map.new
  fields.insert(String.from("title"), JSON.string("demo"))
  fields.insert(String.from("items"), arr)
  let obj = JSON.object(fields)

  let text = JSON.stringify(&obj).expect!("stringify")
  puts text

  let round = JSON.parse(&text).expect!("parse")

  match JSON.object_get(&round, "title")
    Some(title) -> match JSON.as_string(&title)
      Some(v) -> puts "title=#{v}"
      None    -> puts "title=missing"
    end
    None -> puts "title=missing"
  end
end
```

Output (the exact field order of the first line depends on map insertion order):

```
{"title":"demo","items":[1,2]}
title=demo
```

## 7. Common mistakes

- **Calling `as_int` on a node that was parsed as a float.** JSON numbers with a `.` parse as `Float`-tagged nodes; `as_int` returns `None`. Use `as_float` and convert if you want an integer view.
- **`unwrap!` on the parse `Result` for wire data.** Even strict-mode JSON from a trusted source can be truncated. Match and report a clean error instead.
- **Assuming key order in objects.** The stringifier emits keys in iteration order of the source `Map`, which is insertion order for `std.map.Map`. Don't rely on this for cross-language protocol compatibility — JSON itself says key order is unspecified.
- **Mutating an existing `Json`.** The API is build-then-stringify, not mutate-in-place. To change a field, build a new object containing the new field set.

> **Try it:** modify the round-trip example to also pull out the `items` array and print its length.

---

## Recap

- `JSON.parse(&s)` for relaxed input; `JSON.parse_strict(&s)` for wire data.
- `JSON.kind`, `JSON.is_object`, `JSON.object_get`, `JSON.array_len` to inspect.
- `JSON.as_int / as_float / as_string / as_bool` to extract scalars — each returns `Option`.
- `JSON.int / string / array / object` to build new `Json` values.
- `JSON.stringify` (compact) or `JSON.stringify_pretty(v, 2)` (indented) to emit.

**Next:** [Chapter 27 — Ruxen.toml and Dependencies](27-manifest-and-deps.md).
