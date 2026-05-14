# Formatting and Comments

## Code Formatting

Riven ships with a built-in formatter. Zero configuration — one canonical style.

```bash
# Format files
rivenc fmt file.rvn
rivenc fmt src/

# Check without modifying
rivenc fmt --check .

# Show diff
rivenc fmt --diff file.rvn

# Read from stdin
echo 'let x=1+2' | rivenc fmt --stdin
```

### Disabling Formatting

Use `fmt: off` / `fmt: on` comments to preserve manual formatting:

```riven
# fmt: off
let matrix = [
  [1, 0, 0],
  [0, 1, 0],
  [0, 0, 1],
]
# fmt: on
```

## Comments

### Line Comments

```riven
# This is a line comment
let x = 42  # inline comment
```

### Block Comments

Block comments can be nested:

```riven
#= This is a block comment
   spanning multiple lines
   #= nested block comments work =#
=#
```

### Documentation Comments

Attach to the following item. Support Markdown formatting:

```riven
## Finds a user by their ID.
##
## Returns `nil` if no user with the given ID exists.
## The returned reference borrows from the user store.
def find_user(id: Int) -> Option[&User]
  # ...
end
```

## Naming Conventions

| Convention | Used for | Example |
|------------|----------|---------|
| `snake_case` | Variables, functions, methods, file names | `user_name`, `find_by_id` |
| `UpperCamelCase` | Types, classes, mixins, enums, modules | `TaskList`, `Serializable` |
| `SCREAMING_SNAKE_CASE` | Constants | `MAX_RETRIES`, `DEFAULT_PORT` |
| `_` prefix | Unused variables | `_unused`, `_` |
| lowercase identifier | Lifetime parameters (in `[...]` slot) | `a`, `input` |

## Line Structure

- **No semicolons** — statements end at newlines
- **No significant whitespace** — blocks use `do...end` or `{ }`
- Lines ending with an operator, comma, or opening delimiter continue on the next line

```riven
# Implicit continuation
let result = long_function_name(
  argument_one,
  argument_two,
  argument_three
)
```
