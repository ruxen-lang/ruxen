# parser — tokens to AST, the one parser every tool shares

## What lives here
- `mod.rs`: `Parser`, token navigation, error reporting, `parse` (a whole file), `parse_repl_input` (one REPL entry) and `has_unclosed_delimiters`.
- `items.rs`: the top-level item dispatcher, `is_item_start`, `parse_script_units` (a test file split into items and statements), module, `use`, `alias`, type alias, newtype and `const` items.
- `classes.rs`: class, struct, enum, mixin and `extension` bodies.
- `methods.rs`: `def` headers (`parse_method_signature`, which `parse_func_def` calls), parameters, bodies, statements, `let`; the body helpers `BodyState`, `TypeMembers`, `at_body_member`, `parse_section_marker`, `parse_type_body_member`.
- `types.rs`: type expressions, generics, bounds, `where`; `parse_fn_signature_tail` is the one function-type signature parser.
- `expr/`: the Pratt expression parser and its atoms; `patterns.rs`, `ffi.rs` (`lib` blocks; `def var NAME` marks a receiver-mutating instance method, `FfiFunction.mutates_self`), `macros.rs`.
- `ast.rs`: the AST, including the operator table (`BinOp::from_token`, `symbol`, `binding_power`).
- `visit.rs`: the shared exhaustive AST walk. `implicit_main.rs`: top-level statements into a synthesized `main`.

## Public surface
- `Parser::{new, parse, parse_repl_input, parse_script_units, is_item_start, has_unclosed_delimiters}` and the `ast` types, used by the compiler, formatter, REPL, IDE/LSP and test runner. `ruxenc --emit ast` prints the AST with `{:#?}`.

## Depends on
- `lexer` (tokens, spans, `KEYWORD_TABLE`) and `diagnostics`. Nothing later.

## Invariants & gotchas
- Every tool parses through here; a grammar gap is fixed once, here. Tools ask the parser (`is_item_start`, `parse_repl_input`, `parse_script_units`) instead of re-scanning text or tokens.
- Binary operators' token, spelling and precedence live only in `BinOp::spec`; assignment, range and cast powers are in `expr::infix_binding_power`.
- REPL incompleteness is structured: an error raised at end of input, or a missing `end`, bumps `maybe_incomplete_errors`; never match diagnostic text.
- Body loops call `ensure_loop_progress` with the position taken before `at_body_member`, so a member parser that consumes nothing cannot spin.
- `##` docs collected before a member are pending until the member takes them; `alias` drops them so they cannot land on the next `def`.
- A paren-less call takes a non-string argument only for the names in `is_bare_call_arg_start` (`puts`, `print`, `eputs`, `include`, `raise`, `p`, `pp`); any other `foo bar` is two expressions. `p`/`pp` take value tokens only, so `p & 3` stays an operator on a local `p`.
- The AST records surface spellings that sugar would otherwise erase, for `ruxen fmt`: `parens` on `Call`/`MethodCall`, `ivar` on `FieldAccess` (`@x`), `nilable` on `TypePath` (`T?`), `pipes`/`do_end` on `ClosureExpr`. They are semantically inert; a node the compiler synthesizes sets them to the plain spelling.
