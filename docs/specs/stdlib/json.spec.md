# Spec — `std.json`

**Status:** first stdlib package slice. The package source lives under
`library/std/json/` and is included in the embedded stdlib bootstrap list.

`std.json` provides an opaque `Json` node handle, parser, stringifier,
explicit builders, and shallow inspection helpers. The runtime is native C and
stores JSON nodes as heap values owned by the JSON runtime.

## Values

- `class Json` is an opaque JSON node handle.
- Node shape is reported by `JSON.kind(value: &Json) -> JsonKind`.
- Primitive payloads are read with `as_bool`, `as_int`, `as_float`, and
  `as_string`.
- Array and object payloads are inspected with `array_len`, `object_len`,
  `object_has`, and `object_get`.

## Parsing

- `JSON.parse(input: &String) -> Result[Json, JsonError]` accepts:
  - RFC-style JSON values
  - `// line comments`
  - `/* block comments */`
  - trailing commas in arrays and objects
- `JSON.parse_strict(input: &String) -> Result[Json, JsonError]` rejects
  comments and trailing commas.
- Both parsers reject `NaN`, `Infinity`, unterminated strings/comments,
  invalid escapes, invalid surrogate pairs, and nesting beyond the runtime
  depth limit.

## Stringifying

- `JSON.stringify(value: &Json) -> Result[String, JsonError]` emits compact
  JSON.
- `JSON.stringify_pretty(value: &Json, indent: Int) -> Result[String, JsonError]`
  emits newline-indented JSON. Negative indent is treated as zero.
- Object key order follows the current `Map` bucket traversal order and is
  not stable API.

## Explicit Marshalling

The first marshalling surface is explicit builder-based conversion into JSON
nodes:

- `JSON.null_value() -> Json`
- `JSON.bool(value: Bool) -> Json`
- `JSON.int(value: Int) -> Json`
- `JSON.float(value: Float) -> Json`
- `JSON.string(value: &String) -> Json`
- `JSON.array(items: Array[Json]) -> Json`
- `JSON.object(fields: Map[String, Json]) -> Json`
- `JSON.empty_array() -> Json`
- `JSON.empty_object() -> Json`

Reflection-based class/object marshalling is intentionally not specified in
this slice; it needs compiler/runtime support outside `library/std/json`.

## Inspection Helpers

- `JSON.kind(value: &Json) -> JsonKind`
- `JSON.is_null`, `is_bool`, `is_int`, `is_float`, `is_string`, `is_array`,
  `is_object`
- `JSON.as_bool`, `as_int`, `as_float`, `as_string`
- `JSON.array_len`, `object_len`, `object_has`, `object_get`

`as_string` returns a fresh string copy. `object_get` is shallow and returns
the existing parsed/built `Json` node.

## Tests

Focused runtime tests live in `compiler/ruxen_core/tests/stdlib_json.rs`. They
compile only the runtime files used by `std.json` and drive the C ABI directly.
Release e2e coverage lives in `tests/release-e2e/cases/800_json_parse_relaxed_strict.rx`
and `tests/release-e2e/cases/801_json_builders_stringify.rx`.
