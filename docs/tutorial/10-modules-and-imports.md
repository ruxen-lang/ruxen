# Modules and Imports

## Defining Modules

Group related code with `module`. Items inside a module body are **public by default**; use a `private` section marker to gate subsequent declarations.

```ruxen
module Http
  class Request
    url: String
    method: String
    def init(@url: String, @method: String) end
  end

  class Response
    status: Int
    body: String
    def init(@status: Int, @body: String) end
  end

  def get(url: &str) -> Result[Response, HttpError]
    # ...
  end
end
```

## Nested Modules

```ruxen
module App
  module Models
    class User
      name: String
      def init(@name: String) end
    end
  end

  module Services
    def create_user(name: String) -> User
      User.new(name)
    end
  end
end
```

## Importing

### Simple Import

```ruxen
use Http.Request
use Http.Response
```

### Grouped Import

```ruxen
use Http.{ Request, Response }
```

### Aliased Import

```ruxen
use Http.Client as HC
```

### Package-Relative Imports

The compilation unit is a **package**. Use `package` to refer to "this package" in an import path:

```ruxen
use package.utils.format
use package.models.User
```

### Using Imported Names

```ruxen
use Http.{ Request, Response }

let req = Request.new("https://example.com", "GET")
```

## Visibility Rules

Items inside a module are **public by default**. A `private` section marker inside a module body makes subsequent declarations module-local; a `protected` section marker scopes them to subclass-visible.

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
