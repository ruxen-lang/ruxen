# Ruxen.toml and Dependencies

Your project grew past one file. Maybe you want to pull in someone else's library, or split your code into modules, or have multiple executables in one place. That's where the **package manifest** comes in: a small `Ruxen.toml` file at your project root that tells the `ruxen` toolchain what to build, what to depend on, and how to organise the output. This chapter walks through the minimal manifest, then progressively adds dependencies, dev-only dependencies, multiple binaries, and libraries.

If you've used Cargo (Rust), npm, or pip with `pyproject.toml`, the layout will look familiar.

---

## 1. Your first project

The easiest way to get a working project is to ask `ruxen` to make one:

```bash
ruxen new hello
cd hello
ruxen run
```

That prints:

```
Hello, world!
```

The generated layout:

```
hello/
  Ruxen.toml      # the manifest
  src/
    main.rx       # default entry point
```

The manifest itself:

```toml
[package]
name    = "hello"
version = "0.1.0"
edition = "2026"
```

- `name` — lowercase letters, digits, `_`, and `-`. Required.
- `version` — SemVer (`major.minor.patch`). Required.
- `edition` — the language edition. New projects use `"2026"`.

That's enough for a single-binary, no-dependency project.

## 2. Adding a dependency

Suppose you need an HTTP client. Use the `ruxen add` command:

```bash
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

Now you can `use http.*` from your code, and `ruxen build` / `ruxen run` will fetch and compile the dependency.

Three dependency styles you can mix freely:

```toml
[dependencies]
# Registry — version constraint
http = "1.2.0"

# Git checkout — branch / tag / rev all optional
crypto = { git = "https://github.com/example/crypto.git", tag = "v1.0.0" }

# Path dependency — points at a sibling package on disk
shared = { path = "../shared" }
```

CLI shortcuts for each:

```bash
ruxen add http --version 1.2.0
ruxen add crypto --git https://github.com/example/crypto.git --tag v1.0.0
ruxen add shared --path ../shared
ruxen add fixtures --dev               # writes to [dev-dependencies]
ruxen remove http
```

`--branch <name>`, `--tag <name>`, and `--rev <sha>` pin git checkouts.

## 3. Dev-only dependencies

Test helpers, benchmark utilities, and example-only libraries belong under `[dev-dependencies]` so they don't ship with the released library:

```toml
[dev-dependencies]
test-helpers = { path = "../test-helpers" }
```

`ruxen add ... --dev` writes here automatically.

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

`ruxen build`, `ruxen run`, `ruxen update`, and `ruxen add` all maintain a companion file: `Ruxen.lock`. It records the exact resolved versions of every dependency (transitive ones too) plus their integrity checksums.

```
hello/
  Ruxen.toml      # what you ask for ("http >= 1.2")
  Ruxen.lock      # what you actually got ("http = 1.2.7 with hash sha256:...")
  src/
    main.rx
```

- **Binaries:** commit `Ruxen.lock`. That way every machine — your CI, your laptop, your teammate's — builds with the exact same dependency set.
- **Libraries:** committing `Ruxen.lock` is optional. It's nice for reproducible tests in CI; downstream consumers ignore it.

Useful commands:

```bash
ruxen update              # refresh every dep to its newest matching version
ruxen update http         # refresh just one
ruxen verify              # check checksums match the cache
ruxen build --locked      # fail if Ruxen.lock would change (use in CI)
```

## 6. Inspecting the dependency graph

```bash
ruxen tree
```

prints the full resolved tree including transitive dependencies. Useful for spotting duplicates or unexpected pulls before a release.

## 7. Multiple binaries from one package

A package may build more than one executable. Add a `[[bin]]` table for each extra:

```toml
[package]
name    = "my_app"
version = "0.1.0"

[[bin]]
name = "my-cli"
path = "src/cli.rx"

[[bin]]
name = "my-server"
path = "src/server.rx"
```

Build and run individually:

```bash
ruxen build --bin my-cli
ruxen run --bin my-server
```

The default binary at `src/main.rx` is implicit — you only need `[[bin]]` entries for the extras.

## 8. Building a library

A library has no `src/main.rx`. Create one with:

```bash
ruxen new my_lib --lib
```

```
my_lib/
  Ruxen.toml
  src/
    lib.rx
```

The manifest needs no special flag — `ruxen build` notices the layout automatically. Other packages depend on it the usual way (path, git, or registry).

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

[[bin]]
name = "demo"
path = "src/main.rx"

[[bin]]
name = "demo-tool"
path = "src/tool.rx"

[build]
# Extra C source files compiled and linked with the runtime
sources = ["c/extra.c"]
# Additional library search paths (-L flags)
lib-paths = ["/opt/local/lib"]
```

Most projects need only `[package]` and (sometimes) `[dependencies]`.

## 10. Common mistakes

- **Forgetting to commit `Ruxen.lock` for a binary.** Means every machine resolves dependencies independently and you'll occasionally get version drift. Commit it.
- **Pinning every dep with `"= x.y.z"`.** Caret requirements (`"1.2"`) let you pick up bug fixes within the same major version. Use `"="` only when you actually need a specific build.
- **Mixing `[[bin]]` and `src/main.rx`.** That works (main.rx is the default binary), but it's confusing. Either keep `src/main.rx` and add `[[bin]]` for extras, or drop `main.rx` and declare every binary explicitly.
- **Naming a binary the same as the package.** Allowed but produces collisions in shell scripts. Pick distinct names for binaries within a multi-binary package.

> **Try it:** add a second binary `tool` to your `hello` package (create `src/tool.rx` with a `def main` of its own), then run `ruxen run --bin tool`.

---

## Recap

- `Ruxen.toml` is the manifest; required keys are `name`, `version`, `edition`.
- `[dependencies]` lists registry / git / path deps; `[dev-dependencies]` is for test / bench helpers.
- `ruxen add` / `ruxen remove` / `ruxen update` edit the manifest for you.
- `Ruxen.lock` records exact versions — commit it for binaries.
- `[[bin]]` tables let one package produce multiple executables.

**Next:** [Chapter 28 — Workspaces and Multi-Package Projects](28-workspaces.md).
