# Formatting and Comments

This short chapter covers the small things that make a Ruxen codebase pleasant to read: automatic formatting, line and block comments, doc comments that show up in tooling, and the naming conventions everyone follows.

## Code formatting

Ruxen ships with a built-in formatter. There is one canonical style and zero configuration — every Ruxen file in every project looks the same.

```bash
ruxen fmt file.rx              # format a single file in place
ruxen fmt src/                 # format a directory recursively
ruxen fmt --check .            # exit non-zero if anything would change (CI-friendly)
ruxen fmt --diff file.rx       # show the diff without writing
echo 'x=1+2' | ruxen fmt --stdin
```

Set `ruxen fmt --check .` in your CI and your style debates are over.

### Disabling the formatter

Sometimes a hand-laid block (an ASCII-art matrix, a column-aligned table of constants) reads better than what the formatter would produce. Wrap it in `fmt: off` / `fmt: on` comments [implemented]:

```ruby
# fmt: off
identity = [
  [1, 0, 0],
  [0, 1, 0],
  [0, 0, 1],
]
# fmt: on
puts identity.size
```

Everything between the two markers is left alone when they surround whole statements or items.

`ruxen fmt` never deletes a comment. Comments inside a method chain or an argument list, and a `# fmt: off` range that covers only part of a statement, cannot be kept yet: fmt leaves the file unchanged and reports an error naming the comment [implemented]. Keeping them is [planned](../../TODO.md#t-d21-impl).

## Comments

### Line comments

[implemented]:

```ruby
# This is a line comment
x = 42  # inline comment
```

### Block comments

Block comments use `#=` / `=#` and **nest** — useful when you want to comment out a region that already contains block comments [implemented]:

```ruby
#= This block is disabled.
   It contains its own block:
   #= inner block — nesting works =#
   That's still inside the outer block.
=#
```

Ruby's `=begin ... =end` block comment is [planned](../../TODO.md#t-begin-comment). Once it lands, `#= ... =#` stays alongside it for now; whether to keep both is an open question [planned](../../TODO.md#t-oq).

### Documentation comments

A doc comment uses `##` and attaches to the item immediately following it. Doc comments support Markdown — links, code blocks, lists, emphasis — and show up in editor hovers and generated docs [implemented]:

```ruby
## Finds a user name by its ID.
##
## Returns `nil` if no user with the given ID exists.
def find_user(id: Int) -> String?
  id == 1 ? "Ada" : nil
end
```

Two rules: doc comments go *above* the item they document, and they should describe behaviour for the caller — not implementation details for the author.

## Naming conventions

Ruxen follows Ruby's naming conventions, plus one addition for lifetimes:

| Convention | Used for | Example |
|------------|----------|---------|
| `snake_case` | Variables, methods, file names | `user_name`, `find_by_id` |
| `?` / `!` suffix | Predicates and "dangerous" variants | `empty?`, `save!` |
| `UpperCamelCase` | Types, classes, mixins, modules | `TaskList`, `Renderable` |
| `SCREAMING_SNAKE_CASE` | Constants | `MAX_RETRIES`, `DEFAULT_PORT` |
| `_` prefix | Unused variables | `_unused` |
| lowercase identifier in `[...]` | Lifetime parameters | `a`, `input` |

The lowercase-identifier rule matters: in `[T, a]`, `T` is a type and `a` is a lifetime. Stick to UpperCamelCase for types and lowercase for lifetimes — that's how readers tell them apart at a glance.

## Line structure

A few small rules:

- **No semicolons.** Statements end at newlines.
- **No significant whitespace.** Blocks use `do ... end` or `{ ... }`.
- **Lines that end with an operator, comma, or opening delimiter continue on the next line.** That's how the formatter knows your call is one logical line, not several.

[implemented]:

```ruby
def long_method_name(a: Int, b: Int, c: Int) -> Int = a + b + c

# Implicit continuation — comma at end of line says "more to come"
result = long_method_name(
  1,
  2,
  3,
)
puts result
```

## Common mistakes

**Manual alignment fighting the formatter.** Don't column-align assignment operators or arrows in normal code — `ruxen fmt` will undo it. Use `# fmt: off` if you really need to preserve a layout.

**Doc comments after the item.** A doc comment placed *below* its item attaches to nothing useful. Always put `##` lines above what they describe.

**Inconsistent constant naming.** A top-level `max_retries = 3` works, but is jarring next to `MAX_RETRIES`. Pick the convention from the table above. (The Ruby constant form `MAX_RETRIES = 3` is [planned](../../TODO.md#t-constants); see Chapter 2 for what works today.)

**Forgetting to run `ruxen fmt` before committing.** A pre-commit hook or CI step running `ruxen fmt --check .` will save you and your reviewers time.

## Try it

Take any file you've written so far and:

1. Run `ruxen fmt --diff` on it. Did the formatter change anything? Why?
2. Add a `##` doc comment to one public method. Hover the method in your editor — your hover provider should show the comment.
3. Add a `#= ... =#` block comment around a `def` body and confirm the file still parses (the method body is now blank — you'll see a different error, but not a comment one).

## Recap

- `ruxen fmt` is the one source of truth for style. No config, no debate.
- Use `# fmt: off` / `# fmt: on` to opt out of formatting for hand-laid blocks.
- `#` is a line comment; `#= ... =#` is a (nesting) block comment; `##` is a doc comment that attaches to the next item.
- Naming: `snake_case` for values and methods, `UpperCamelCase` for types, `SCREAMING_SNAKE_CASE` for constants, lowercase for lifetime parameters.
- No semicolons. Lines that need to continue end with an operator, comma, or open delimiter.
- fmt never deletes a comment; comments it cannot place yet make it stop with an error.

**Next:** [String Formatting and Interpolation](17-string-formatting-and-interpolation.md) — formatting numbers, padding, precision, and `to_s`/`inspect` in detail.
