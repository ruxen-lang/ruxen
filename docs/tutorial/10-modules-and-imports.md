# Modules and Files

As programs grow, you want to put related things together and keep unrelated things apart. A **module** is Ruxen's grouping mechanism — a named container for methods, classes, and other modules. To use code from another file, `require` it, as in Ruby.

## A first runnable example

Save this as `tools.rx` [implemented]:

```ruby
module Tools
  def pick(value: Int) -> String
    "module-int"
  end

  def pick(value: String) -> String
    "module-string"
  end
end

puts Tools.pick(7)
puts Tools.pick("seven")
```

```bash
ruxen compile tools.rx
./tools
```

Output:

```text
module-int
module-string
```

You defined a module `Tools`, called its methods with `Tools.pick(...)`, and saw that Ruxen routes the call to the right overload based on the argument type (overloading by type is a Ruxen addition; see Chapter 25).

## Loading other files: `require`

Split a program across files with `require_relative`, which loads a file relative to the current one, and `require`, which loads from the load path [implemented]. Given a file `greeting.rx`:

```ruby
def greeting(name: String) -> String
  "Hello, #{name}!"
end
```

the main file loads it and calls its method [implemented]:

```ruby
require_relative "greeting"

puts greeting("Ada")
```

Everything a required file defines is visible to the file that requires it.

## Defining a module

Works today, with the constructor spelled `def init` [deprecated] (replace with `def initialize` [planned](../../TODO.md#t-initialize)):

```ruby
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

req = Http::Request.new("https://example.com", "GET")
puts req.url
```

Refer to a member as `Http::Request` in expressions [implemented]; `Http.Request` is also accepted today. In a type position (`req: Http::Request`) the `::` form is [planned](../../TODO.md#t-type-colons); today write `req: Http.Request` there.

Items inside a module body are **public by default**. Use `private` and `protected` section markers to mark subsequent declarations. The markers are accepted [implemented]; the access rule is not enforced yet (see below):

```ruby
module Store
  def lookup(key: Int) -> Int
    key * 10
  end

  private

  def secret_table -> Int    # meant for Store's own use
    42
  end
end

puts Store.lookup(4)
```

Three limits today. A method in a module cannot call another method of the same module, either bare (`helper(x)`, E0735) or as `Store.helper(x)` (E0714), a [known issue](../../TODO.md#t-module-self-call); put helpers that call each other in a class. Calling a private module method from outside is not rejected yet (`Store.secret_table` type-checks), a [known issue](../../TODO.md#t-visibility-enforce). And a module method that takes no arguments type-checks but fails to build when called (`Store.secret_table` stops in MIR lowering: "it calls `..._secret_table` on a value whose type is still a generic parameter"), a [known issue](../../TODO.md#t-module-zero-arg); give it a parameter or put it in a class for now.

## Nested modules

Modules can hold modules — useful for layering an application. Works today, with `def init` [deprecated]: a class nested in a module, reached by its full path.

```ruby
module App
  module Models
    class User
      name: String
      def init(@name: String)
      end
    end
  end
end

u = App::Models::User.new("Ada")
puts u.name
```

A method defined directly in a nested module (`module App; module Services; def create_user ...`) type-checks, but calling it as `App::Services.create_user("Ada")` fails to link today (undefined `_Services_create_user`), a [known issue](../../TODO.md#t-nested-module-methods). Methods in a top-level module work; so do classes at any depth.

Nesting a class or a constant directly inside a class body (`class Http; class Request; end; end`) is [planned](../../TODO.md#t-nested-class); today nest inside a `module`.

## Shorter names

Long paths get tiresome. In Ruby you `require` the file and refer to names by their qualified path (`IO::Stdout`, `Http::Request`), or assign a short constant. Older Ruxen code imports names with `use`, which is [deprecated] (replace with `require` and qualified names):

```text
use Http.Request              # import one name
use Http.{ Request, Response } # import several
use Http.Client as HC          # import under an alias
```

The package-relative path root `use package.utils.format` is [deprecated] too: replace it with `require_relative "utils/format"`.

## Visibility recap

Public by default. Inside any module or class body:

| Section marker | Scope |
|----------------|-------|
| (default — none) | Public — accessible from anywhere |
| `private` | Only inside the current module or type |
| `protected` | Ruby's rule: callable with an explicit receiver of the same class family, from inside it |

A marker affects everything declared after it, up to the next marker or the end of the body. `private :a, :b` marks named methods. There is no per-item `pub` keyword; the section marker style keeps the surface visually grouped. The table is the intended rule. Today only the marking works: neither `private` nor `protected` is enforced on module or class members, a [known issue](../../TODO.md#t-visibility-enforce). Ruby's `protected` rule is [planned](../../TODO.md#t-protected); older drafts said "the current class plus subclasses".

## Common mistakes

**Trying to access something behind a `private` marker.** Once the rule is enforced this is an error (today it is not caught, see above). Move the marker, move the caller into the same class, or expose a wrapper that *is* public.

**Reaching for `use`.** `use` and `package.` paths are deprecated; `require` the file and write the qualified name.

**Confusing module nesting with directory layout.** Module structure is what you write in `module ... end` blocks; the file system layout is separate. A single file can declare deeply nested modules, and a project with many files can have flat ones. `require_relative` paths follow the file system, not the modules.

## Try it

Define a module `Geometry` with two methods: a public `square(n: Int) -> Int` and, after a `private` marker, `cube(n: Int) -> Int`. Call `Geometry.square(5)` at the top level — it should work. Then move `Geometry` into its own file `geometry.rx` and load it with `require_relative "geometry"`.

## Recap

- `module Name ... end` groups items under a name; refer to them as `Name::Item` (or `Name.method` for a module method).
- `require_relative "file"` and `require "lib"` load code from other files.
- Items are public by default; `private` and `protected` markers mark what follows (not enforced yet).
- `use` imports and `package.` paths are deprecated.
- Module nesting is structural, independent of file layout.

**Next:** [Error Handling](11-error-handling.md) — exceptions, and the `Result` form that works today.
