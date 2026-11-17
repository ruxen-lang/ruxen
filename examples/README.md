## Examples

Each example is a stand-alone Ruxen package. Build and run with:

```sh
cd <example-dir>
ruxen run
```

| Example                | Showcases                                                     |
|------------------------|---------------------------------------------------------------|
| `01-cli-utility`       | command-line arguments, reading a file, exit status; written in the [deprecated] `use`, `Result` and `?` forms, which `ARGV`, `File.read` [planned](../TODO.md#t-stdlib-shapes) and exceptions [planned](../TODO.md#t-exceptions) replace |
| `02-fizzbuzz`          | `for` over a `1..20` range, `if`/`elsif`, string interpolation [implemented]; its explicit `def main` is [deprecated] (top-level statements replace it) |
| `03-tasklist`          | classes, `def var`, `Array[T]`, encapsulation [implemented]; written with the [deprecated] `def init`, `self.field`, `let`/`var` and brace-body forms, replaced by `initialize` [planned](../TODO.md#t-initialize), `@field`, plain assignment and endless `def` |
| `04-implicit-debug`    | a printable form synthesized for nested value types, shown with the [deprecated] `struct` and `#{x:?}` forms, replaced by `Data.define` [planned](../TODO.md#t-data-define) and `inspect` [planned](../TODO.md#t-d9) |
