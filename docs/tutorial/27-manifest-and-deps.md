# Ruxen.toml and Dependencies

Your project grew past one file. Maybe you want to pull in someone else's library, or split your code into modules, or have multiple executables in one place. That's where the **package manifest** comes in: a small `Ruxen.toml` file at your project root that tells the `ruxen` toolchain what to build, what to depend on, and how to organise the output. This chapter walks through the minimal manifest, then progressively adds dependencies, dev-only dependencies, binaries, and libraries.

If you've used Bundler's `Gemfile`, npm, or pip with `pyproject.toml`, the layout will look familiar.

---

## 1. Your first project

The easiest way to get a working project is to ask `ruxen` to make one [implemented]:

```sh
ruxen new hello
cd hello
ruxen run
```

That prints:

```text
Hello, Ruxen!
```

The generated layout:

```text
hello/
  Ruxen.toml      # the manifest
  .gitignore
  src/
    main.rx       # default entry point
  tests/
    example.rx    # a passing starter test (Chapter 19)
```

The manifest itself:

```toml
[package]
name    = "hello"
version = "0.1.0"
edition = "2026"
```

- `name` — lowercase letters, digits, `_`, and `-`. Required [implemented].
- `version` — SemVer (`major.minor.patch`). Required [implemented].
- `edition` — optional; `ruxen new` writes `"2026"` [implemented]. It does not change how your code is read: Ruxen has no language editions, and versions follow semver.

That's enough for a single-binary, no-dependency project.

## 2. Adding a dependency

Suppose you need an HTTP client. Use the `ruxen add` command [implemented]:

```sh
ruxen add http --version 1.2.0
```

That edits `Ruxen.toml` for you:

```toml
[package]
name    = "hello"
version = "0.1.0"
edition = "2026"

[dependencies]
http = "1.2.0"
```

Now `ruxen build` / `ruxen run` will fetch and compile the dependency, and its classes and methods are visible in your code: a binary build merges the whole dependency closure, so there is nothing to import [implemented]. The old `use http.*` import is [deprecated].

Three dependency styles you can mix freely, all [implemented]:

```toml
[dependencies]
# Registry — version constraint
http = "1.2.0"

# Git checkout — branch / tag / rev all optional
crypto = { git = "https://github.com/example/crypto.git", tag = "v1.0.0" }

# Path dependency — points at a sibling package on disk
shared = { path = "../shared" }
```

CLI shortcuts for each [implemented]:

```sh
ruxen add http --version 1.2.0
ruxen add crypto --git https://github.com/example/crypto.git --tag v1.0.0
ruxen add shared --path ../shared
ruxen add fixtures --dev               # writes to [dev-dependencies]
ruxen remove http
```

`--branch <name>`, `--tag <name>`, and `--rev <sha>` pin git checkouts [implemented].

## 3. Dev-only dependencies

Test helpers, benchmark utilities, and example-only libraries belong under `[dev-dependencies]` so they don't ship with the released library. The table is parsed and kept by `ruxen add --dev` and `ruxen remove` [implemented], but nothing builds it yet: `ruxen build` prints a warning and ignores it, and `ruxen test` does not use it. Compiling dev-dependencies for tests and benches is [planned](../../TODO.md#t-dev-deps).

```toml
[dev-dependencies]
test-helpers = { path = "../test-helpers" }
```

`ruxen add ... --dev` writes here automatically [implemented].

## 4. Version requirements

The string after `=` is a SemVer requirement:

| Form        | Means                                       |
|-------------|---------------------------------------------|
| `"1.2.3"`   | Compatible with 1.2.3 (same major version) |
| `"= 1.2.3"` | Exactly 1.2.3                                |
| `">= 1.2"`  | At least 1.2                                |
| `"~1.2"`    | At least 1.2, less than 2.0                  |
| `"^1.2"`    | Equivalent to the bare `"1.2"` form          |

When in doubt, write the bare version — Ruxen treats it as "compatible with this version".

## 5. The lockfile

`ruxen build`, `ruxen run`, `ruxen update`, and `ruxen add` all maintain a companion file: `Ruxen.lock`. It records the exact resolved versions of every dependency (transitive ones too) plus their integrity checksums [implemented].

```text
hello/
  Ruxen.toml      # what you ask for ("http >= 1.2")
  Ruxen.lock      # what you actually got ("http = 1.2.7 with hash sha256:...")
  src/
    main.rx
```

- **Binaries:** commit `Ruxen.lock`. That way every machine — your CI, your laptop, your teammate's — builds with the exact same dependency set.
- **Libraries:** committing `Ruxen.lock` is optional. It's nice for reproducible tests in CI; downstream consumers ignore it.

Useful commands, all [implemented]:

```sh
ruxen update              # refresh every dep to its newest matching version
ruxen update http         # refresh just one
ruxen verify              # check checksums match the cache
ruxen build --locked      # fail if Ruxen.lock would change (use in CI)
```

## 6. Inspecting the dependency graph

```sh
ruxen tree
```

prints the full resolved tree including transitive dependencies [implemented]. Useful for spotting duplicates or unexpected pulls before a release.

## 7. Multiple binaries from one package

Building several executables from one package, with a `[[bin]]` table per binary holding its `name` and `entry` file, is [planned](../../TODO.md#t-multi-bin). The manifest reader accepts these tables today, but the build does not read them:

```toml
[package]
name    = "my_app"
version = "0.1.0"

[[bin]]
name  = "my-cli"
entry = "src/cli.rx"

[[bin]]
name  = "my-server"
entry = "src/server.rx"
```

What works today [implemented]: `ruxen build` always compiles the package's one entry point (`[build] entry`, or `src/main.rx` when that key is absent), and `ruxen build --bin <name>` only names the output file:

```sh
ruxen build --bin my-cli      # compiles src/main.rx, writes target/debug/my-cli
./target/debug/my-cli
```

`ruxen run` runs the default binary and has no `--bin` switch. Until `[[bin]]` lands, put each extra executable in its own package (see [Chapter 28](28-workspaces.md)).

## 8. Building a library

A library has no `src/main.rx`. Create one with [implemented]:

```sh
ruxen new my_lib --lib
```

```text
my_lib/
  Ruxen.toml
  src/
    lib.rx
```

`ruxen new --lib` writes `type = "library"` under `[build]` in the manifest [implemented]. Other packages depend on it the usual way (path, git, or registry).

## 9. A fully populated manifest

For reference, here's a manifest that uses every common section:

```toml
[package]
name        = "demo"
version     = "0.1.0"
edition     = "2026"
authors     = ["Alaric <a@example.com>"]
description = "A demo Ruxen project."
license     = "MIT"
repository  = "https://github.com/example/demo"

[dependencies]
http   = "1.2"
crypto = { git = "https://github.com/example/crypto.git", branch = "main" }
shared = { path = "../shared" }

[dev-dependencies]
test-helpers = { path = "../test-helpers" }

[build]
# The file to compile (default: src/main.rx)
entry = "src/main.rx"
# Extra libraries to link (-l flags)
link = ["ssl"]
# Additional library search paths (-L flags)
link-search = ["/opt/local/lib"]
```

Most projects need only `[package]` and (sometimes) `[dependencies]`. `[dev-dependencies]` is recorded but not built yet [planned](../../TODO.md#t-dev-deps).

The `[build]` keys above (`type`, `entry`, `link`, `link-search`, `target`) are [implemented]. A `[runtime]` table for a package's C runtime sources, kept beside a `[native] include` table for C headers and read by one shared manifest reader for every driver, is [planned](../../TODO.md#t-d20).

## 10. Common mistakes

- **Forgetting to commit `Ruxen.lock` for a binary.** Means every machine resolves dependencies independently and you'll occasionally get version drift. Commit it.
- **Pinning every dep with `"= x.y.z"`.** Caret requirements (`"1.2"`) let you pick up bug fixes within the same major version. Use `"="` only when you actually need a specific build.
- **Expecting `[[bin]]` tables to build.** They are accepted but ignored today [planned](../../TODO.md#t-multi-bin); `--bin` only renames the output of the one entry point.
- **Naming a binary the same as the package.** Allowed but produces collisions in shell scripts. Pick distinct names for binaries within a multi-binary package.

> **Try it:** add a second executable `tool` next to your `hello` package as its own package (`ruxen new tool`), depend on `hello` with a path dependency, and build it with `ruxen build`.

---

## Recap

- `Ruxen.toml` is the manifest; required keys are `name` and `version`.
- `[dependencies]` lists registry / git / path deps [implemented]; `[dev-dependencies]` records test / bench helpers, which are not built yet [planned](../../TODO.md#t-dev-deps).
- `ruxen add` / `ruxen remove` / `ruxen update` edit the manifest for you.
- `Ruxen.lock` records exact versions — commit it for binaries.
- `[[bin]]` tables for several executables per package are [planned](../../TODO.md#t-multi-bin); today one package builds one entry point.

**Next:** [Chapter 28 — Workspaces and Multi-Package Projects](28-workspaces.md).
