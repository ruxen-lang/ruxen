# Workspaces and Multi-Package Projects

Your project has grown to the point where everything-in-one-package is starting to hurt. You want a library that holds the domain logic, a CLI binary that wraps it, and maybe a separate server binary that consumes the same library — each evolvable on its own. That's the **multi-package** shape. In Ruxen, you build it by putting each package in its own subdirectory with its own `Ruxen.toml`, and wiring them together with **path dependencies** (`{ path = "../sibling" }`).

This is exactly how Ruxen's own standard library is structured: `library/std/core/`, `library/std/io/`, `library/std/json/`, and so on are each independent packages that depend on each other via paths.

> Ruxen v1 does not yet ship a top-level `[workspace]` table — that's planned for a future release. The path-deps pattern below covers the common case.

---

## 1. A working two-package example

```
my_project/
  packages/
    core/
      Ruxen.toml
      src/
        lib.rx
    cli/
      Ruxen.toml
      src/
        main.rx
```

`packages/core/Ruxen.toml`:

```toml
[package]
name    = "core"
version = "0.1.0"
description = "Domain types and shared helpers."
```

`packages/core/src/lib.rx`:

```ruxen
module types
  class UserId
    inner: Int

    def init(@inner: Int)
    end

    def value -> Int
      self.inner
    end
  end
end
```

`packages/cli/Ruxen.toml`:

```toml
[package]
name    = "cli"
version = "0.1.0"

[dependencies]
core = { path = "../core" }
```

`packages/cli/src/main.rx`:

```ruxen
use core.types.UserId

def main
  let id = UserId.new(42)
  puts "user id = #{id.value}"
end
```

Build and run the CLI:

```bash
cd packages/cli
ruxen run
```

Output:

```
user id = 42
```

That's the whole pattern. The `cli` package declares a path dependency on `core`; it can `use core.types.*` as if `core` were a registry dependency.

## 2. Recommended layout

For anything beyond two packages, settle on a flat shape:

```
my_project/
  README.md
  packages/
    core/
      Ruxen.toml
      src/lib.rx
    server/
      Ruxen.toml
      src/main.rx
    cli/
      Ruxen.toml
      src/main.rx
```

Each subdirectory under `packages/` is a complete, normal Ruxen package. There's no project-level manifest — the root directory is just a folder you happen to keep packages in.

## 3. Building everything

Inside any single package directory, `ruxen build` and `ruxen run` work as usual. To build them all in one go, wrap a shell loop:

```bash
#!/usr/bin/env bash
set -e
for pkg in core server cli; do
  (cd packages/"$pkg" && ruxen build)
done
```

`set -e` aborts on the first failure so your CI catches it.

Each package keeps its own build artefacts under `packages/<name>/target/` — there's no shared `target/` directory in v1.

## 4. Library + CLI shape

The most common shape: one library package that owns the domain logic, one binary package that wraps it as a CLI:

```
my_app/
  packages/
    my_app/             # the library
      Ruxen.toml        # name = "my_app"
      src/lib.rx
    my-app-cli/         # the binary
      Ruxen.toml        # name = "my-app-cli"
      src/main.rx
```

The CLI's manifest depends on the library by path:

```toml
[package]
name    = "my-app-cli"
version = "0.1.0"

[dependencies]
my_app = { path = "../my_app" }
```

Once you're ready to publish, the library can be released to a registry (or pulled in via `git = "..."` by another project) entirely independently of the CLI — the CLI is just a thin user-facing shell.

## 5. Sharing dependencies across packages

Each package has its own `Ruxen.lock`, and each pulls in its own copy of any registry / git dependencies. There's no shared lockfile yet.

For sanity, **declare common dependencies at the same version across packages**. A small project can manage this by convention (grep the manifests before bumping); a larger one might keep a `versions.md` checklist alongside the package folder.

## 6. When to split a package

A few rules of thumb:

- **One binary, one focus per package.** A CLI package that also bundles a daemon is harder to test than two smaller packages.
- **Library code that exists to be reused → its own package.** Even within a single project, separating domain types into a library makes them easy to import from multiple binaries.
- **Stop splitting when you're inventing names.** If the only reason a package exists is "it needs a place to live", fold it back into its consumer.

## 7. What's not here yet

These are tracked for a future release:

- A top-level `[workspace]` table.
- `ruxen workspaces`-style commands that operate on all packages at once.
- A shared `target/` directory.

For now, the flat-package-folder pattern plus a small shell wrapper covers the common case.

## 8. Common mistakes

- **Relative paths drifting.** If you move a package, every other package that depends on it via `{ path = "..." }` breaks. Search-and-replace the `path = "../core"` lines.
- **Cyclic dependencies.** `core` depends on `helpers`, `helpers` depends on `core` — the resolver will refuse. The fix is to extract the shared bit into a third package both depend on.
- **Inconsistent versions of a shared registry dep.** Two packages depending on `http = "1.2"` and `http = "2.0"` will each pull their own copy. Keep versions aligned across the workspace by convention.

> **Try it:** add a third package called `tools` to the layout in section 2 that also depends on `core`. Make `cli` depend on both `core` and `tools` and check it builds.

---

## Recap

- Multi-package projects live under `packages/<name>/`, each with its own `Ruxen.toml`.
- Path dependencies (`{ path = "../sibling" }`) wire packages together.
- Build each package individually; wrap them in a shell loop for "build all".
- The standard library itself is organised this way — read `library/std/` for a real-world example.
- Top-level `[workspace]` and shared `target/` are planned for a future release.

**Next:** [Chapter 29 — Strings, Bytes, and Numbers](29-strings-bytes-numbers.md).
