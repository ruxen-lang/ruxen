# formatter — `ruxen fmt`: AST to canonical source, comments kept

## What lives here
- `mod.rs`: `format` (lex, classify comments, parse, attach, render, refuse when a comment would be lost) and `render_fn_signature` for IDE display.
- `comments.rs`: `classify_comments` (from the lexer's `comment_spans`), `collect_node_spans` (all nodes plus the members written on their own lines), `CommentAttacher`, `CommentMap`.
- `format_items.rs`: items and bodies; `with_comments` and `dangling_comments_doc` place comments, `signature_doc` is the one `def` header.
- `format_expr.rs`, `format_pattern.rs`, `format_type.rs`: expressions, patterns, types.
- `prec.rs`: when an operand needs parentheses to re-parse identically.
- `doc.rs`: the Wadler-style document IR and printer.

## Public surface
- `format(source) -> FormatResult`, `render_fn_signature` (IDE hover, signature help, completion), `doc` and `comments` for tests.

## Depends on
- `lexer` (comment trivia), `parser` (AST, `BinOp` table). Nothing later.

## Invariants & gotchas
- Comments come from the lexer, never a second scanner. Every comment is leading, trailing, after the last member of a scope, or dangling in an empty scope; the `CommentMap` accessors mark what they return as written.
- `format` returns the source unchanged with an error naming the comment if any comment was not written (`CommentMap::dropped`): fmt never deletes a comment. A new member kind must go through `with_comments` and be recorded with `member` in `collect_node_spans`.
- Output must re-parse to the same AST and be idempotent (`tests/it/syntax_parity.rs` over the whole corpus; `test_comments_everywhere_idempotent`).
- Class, struct and enum bodies print every member in source order (`format_type_body` sorts by span start); a new body member kind needs a `BodyMember` arm, never a separate section printed after the others.
- Operator spellings and precedence come from `BinOp`; do not add local tables.
- fmt keeps the author's spelling and layout, never normalizing what the AST records:
  - call parens (`Call`/`MethodCall.parens`: `puts "x"`, `v.map { }` stay paren-less, `f()` keeps `()`), `@field` (`FieldAccess.ivar`), `T?` (`TypePath.nilable`), a block's form and header (`ClosureExpr.do_end` / `pipes`), and a `where` clause's line;
  - a `def` body prints as `{ expr }` only when written on the `def` line (`is_simple_inline_body`);
  - one blank line between statements, type-body members, mixin/extension/`lib` members and comment groups survives, and none is added (`CommentMap::blank_line_before` / `blank_line_directly_above`, over `CommentMap::lines`, byte offsets).
- Still normalized (not recorded in the AST): grouping parens (re-added only where precedence needs them), match-arm and trailing-comment alignment padding, map-literal brace spacing, heredoc strings, long lines (re-wrapped at the width).
