# Formatting and Comments

This short chapter covers the small things that make a Ruxen codebase pleasant to read: automatic formatting, line and block comments, doc comments that show up in tooling, and the naming conventions everyone follows.

## Code formatting

Ruxen ships with a built-in formatter. There is one canonical style and zero configuration — every Ruxen file in every project looks the same.

```bash
ruxen fmt file.rx              # format a single file in place
ruxen fmt src/                 # format a directory recursively
ruxen fmt --check .            # exit non-zero if anything would change (CI-friendly)
ruxen fmt --diff file.rx       # show the diff without writing
echo 'let x=1+2' | ruxen fmt --stdin
```

Set `ruxen fmt --check .` in your CI and your style debates are over.

### Disabling the formatter

Sometimes a hand-laid block (an ASCII-art matrix, a column-aligned table of constants) reads better than what the formatter would produce. Wrap it in `fmt: off` / `fmt: on` comments:

```ruxen
# fmt: off
let identity = [
  [1, 0, 0],
  [0, 1, 0],
  [0, 0, 1],
]
# fmt: on
```

Everything between the two markers is left alone.

## Comments

### Line comments

```ruxen
# This is a line comment
let x = 42  # inline comment
```

### Block comments

Block comments use `#=` / `=#` and **nest** — useful when you want to comment out a region that already contains block comments:

```ruxen
#= This block is disabled.
   It contains its own block:
   #= inner block — nesting works =#
   That's still inside the outer block.
=#
```

### Documentation comments

A doc comment uses `##` and attaches to the item immediately following it. Doc comments support Markdown — links, code blocks, lists, emphasis — and show up in editor hovers and generated docs.

```ruxen
## Finds a user by their ID.
##
## Returns `nil` if no user with the given ID exists.
## The returned reference borrows from the user store.
def find_user(id: Int) -> Option[&User]
  # ...
end
```

Two rules: doc comments go *above* the item they document, and they should describe behaviour for the caller — not implementation details for the author.

## Naming conventions

Ruxen has the same conventions you'll recognize from most curly-brace languages plus a couple of additions:

| Convention | Used for | Example |
|------------|----------|---------|
| `snake_case` | Variables, functions, methods, file names | `user_name`, `find_by_id` |
| `UpperCamelCase` | Types, classes, mixins, enums, modules | `TaskList`, `Renderable` |
| `SCREAMING_SNAKE_CASE` | Module-level constants | `MAX_RETRIES`, `DEFAULT_PORT` |
| `_` prefix | Unused variables | `let _ = result`, `_unused` |
| lowercase identifier in `[...]` | Lifetime parameters | `a`, `input` |

The lowercase-identifier rule matters: in `[T, a]`, `T` is a type and `a` is a lifetime. Stick to UpperCamelCase for types and lowercase for lifetimes — that's how readers tell them apart at a glance.

## Line structure

A few small rules:

- **No semicolons.** Statements end at newlines.
- **No significant whitespace.** Blocks use `do ... end` or `{ ... }`.
- **Lines that end with an operator, comma, or opening delimiter continue on the next line.** That's how the formatter knows your call is one logical line, not several.

```ruxen
# Implicit continuation — comma at end of line says "more to come"
let result = long_function_name(
  argument_one,
  argument_two,
  argument_three,
)
```

## Common mistakes

**Manual alignment fighting the formatter.** Don't column-align assignment operators or arrows in normal code — `ruxen fmt` will undo it. Use `# fmt: off` if you really need to preserve a layout.

**Doc comments after the item.** A doc comment placed *below* its item attaches to nothing useful. Always put `##` lines above what they describe.

**Inconsistent constant naming.** A top-level `let max_retries = 3` works, but is jarring next to `MAX_RETRIES`. Pick the convention from the table above.

**Forgetting to run `ruxen fmt` before committing.** A pre-commit hook or CI step running `ruxen fmt --check .` will save you and your reviewers time.

## Try it

Take any file you've written so far and:

1. Run `ruxen fmt --diff` on it. Did the formatter change anything? Why?
2. Add a `##` doc comment to one public function. Hover the function in your editor — your hover provider should show the comment.
3. Add a `#= ... =#` block comment around a `def` body and confirm the file still compiles (the function body is now blank — you'll see a different error, but not a comment one).

## Recap

- `ruxen fmt` is the one source of truth for style. No config, no debate.
- Use `# fmt: off` / `# fmt: on` to opt out of formatting for hand-laid blocks.
- `#` is a line comment; `#= ... =#` is a (nesting) block comment; `##` is a doc comment that attaches to the next item.
- Naming: `snake_case` for values and functions, `UpperCamelCase` for types, `SCREAMING_SNAKE_CASE` for constants, lowercase for lifetime parameters.
- No semicolons. Lines that need to continue end with an operator, comma, or open delimiter.

**Next:** [String Formatting and Interpolation](17-string-formatting-and-interpolation.md) — formatting numbers, padding, precision, and the `Display`/`Debug` mixins in detail.
