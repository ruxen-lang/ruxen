# Modules and Imports

As programs grow, you want to put related things together and keep unrelated things apart. A **module** is Ruxen's grouping mechanism — a named container for functions, classes, and other modules. Importing a name with `use` brings it into your file's vocabulary.

## A first runnable example

```ruxen
module Tools
  def pick(value: Int) -> String
    String.from("module-int")
  end

  def pick(value: String) -> String
    String.from("module-string")
  end
end

def main
  puts Tools.pick(7)
  puts Tools.pick(String.from("seven"))
end
```

```bash
ruxen compile tools.rx
./tools
```

Output:

```
module-int
module-string
```

You defined a module `Tools`, called its functions with `Tools.pick(...)`, and saw that Ruxen routes the call to the right overload based on the argument type.

## Defining a module

```ruxen
module Http
  class Request
    url: String
    method: String
    def init(@url: String, @method: String)
    end
  end

  class Response
    status: Int
    body: String
    def init(@status: Int, @body: String)
    end
  end
end
```

Items inside a module body are **public by default**. Use `private` and `protected` section markers to gate subsequent declarations:

```ruxen
module Database
  def query(sql: &str) -> Result[Rows, DbError]
    let conn = connect_internal()
    # ...
  end

  private

  def connect_internal -> Connection
    # only accessible inside Database
  end
end
```

## Nested modules

Modules can hold modules — useful for layering an application:

```ruxen
module App
  module Models
    class User
      name: String
      def init(@name: String)
      end
    end
  end

  module Services
    def create_user(name: String) -> App.Models.User
      App.Models.User.new(name)
    end
  end
end
```

## `use` — bringing names into scope

Long paths get tiresome. `use` imports a name so you can refer to it directly:

```ruxen
use Http.Request
use Http.Response

def main
  let req = Request.new(String.from("https://example.com"), String.from("GET"))
end
```

### Grouped imports

Pull several names from one module in one line:

```ruxen
use Http.{ Request, Response }
```

### Aliased imports

Give an imported name a local alias to avoid a clash:

```ruxen
use Http.Client as HC

let c = HC.new
```

### Package-relative imports

The unit of compilation is a **package** (one `Ruxen.toml` and the source tree under it). The keyword `package` means "this package":

```ruxen
use package.utils.format
use package.models.User
```

Use these when you want to be explicit that you're importing from your own project, not a dependency.

## Visibility recap

Public by default. Inside any module, class, or struct body:

| Section marker | Scope |
|----------------|-------|
| (default — none) | Public — accessible from anywhere |
| `private` | Only inside the current module or type |
| `protected` | Inside the current class, plus subclasses |

A marker affects everything declared after it, up to the next marker or the end of the body. There is no per-item `pub` keyword; the section marker style keeps the surface visually grouped.

## Common mistakes

**Trying to access something behind a `private` marker.** The compiler tells you the item is private. Either move the marker, move the caller into the same module, or expose a wrapper that *is* public.

**Forgetting `use` and using the full path everywhere.** Both are valid — `Http.Request` and `use Http.Request` then `Request` — but the imported form keeps your file readable.

**Naming collisions on import.** If two `use` lines bring in the same name, give one an `as` alias.

**Confusing module nesting with directory layout.** Module structure is what you write in `module ... end` blocks; the file system layout is separate. A single file can declare deeply nested modules, and a project with many files can have flat ones.

## Try it

Define a module `Math` with two functions: a public `square(n: Int) -> Int` and a private helper `cube(n: Int) -> Int`. Call `Math.square(5)` from `main` — it should work. Then try `Math.cube(5)` — the compiler should refuse, and the error message should point at the `private` marker.

## Recap

- `module Name ... end` groups items under a name.
- Items are public by default; `private` and `protected` markers gate what follows.
- `use Path.Name` brings a name into scope. `use Path.{ A, B }` groups; `use Path.X as Y` aliases.
- `package` is the keyword for "this project" in import paths.
- Module nesting is structural, independent of file layout.

**Next:** [Error Handling](11-error-handling.md) — `Result`, `Option`, and the `?` operator in depth.
