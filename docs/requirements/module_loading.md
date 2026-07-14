# Ruby-style module loading

Status: Implemented (2026-07-14)

Ruxen exposes Ruby's module-loading ergonomics while compiling ahead of time.
The directives are source-loading forms, not runtime function calls:

```ruby
require "lib/json"
require_relative "../shared/math"
autoload :HttpClient, "http/client"
```

`require` searches the project root first and dependency package roots in
manifest resolution order. An absolute request is loaded directly.
`require_relative` resolves only from the directory of the file containing the
directive. A `.rx` extension is optional in both forms. The embedded standard
library is always linked and is not represented by a filesystem load-path
entry.

Every resolved path is canonicalized. The loader includes a canonical file at
most once, so repeated requests, diamonds, and cycles have the same
loaded-feature behavior as Ruby. A missing request is a compile-time
`LoadError` analogue that lists every candidate searched.

`autoload :Name, "path"` records a namespace-qualified constant and a
load-path request. The loader performs an initial require traversal, runs name
resolution, loads files whose registrations match unresolved constants, and
repeats until no file is added. This is compile-time laziness: there is no
runtime file I/O, but a constant can be referenced without an explicit
`require`. Registrations are supported at top level, inside a `module`, and as
`Foo.autoload :Bar, "foo/bar"`.

The scanner removes directive lines before the normal lexer/parser pipeline,
so they never become executable expressions or enter an implicit `main`.
Required definitions remain ordinary declarations. When required files and
the entry use top-level executable statements, their statements join the one
synthesized `main` in load order: required-file statements run once before the
entry's statements. As elsewhere in Ruxen, an explicit `main` combined with
loose top-level statements remains the existing E0728 error.

Compatibility is opt-in. If the entry contains no module-loading directive,
project builds retain the prior behavior: dependencies and every `.rx` file
under the project's `src/` tree are concatenated as before. Direct single-file
compilation simply has a one-file graph when no directive is present.

The combined compilation source includes each loaded file's canonical path and
contents. That exact source feeds the low-level incremental cache, while rlib
creation hashes the same combined bytes. Editing, moving, adding, or removing
any transitive source therefore invalidates cached output.
