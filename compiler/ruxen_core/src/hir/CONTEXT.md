# hir — typed, name-resolved IR shared by typeck, borrow_check and mir

## What lives here
- `nodes.rs`: HIR node definitions (`HirProgram`, `HirItem`, `HirExpr`/`HirExprKind`, statements, patterns, function/class/struct/enum/mixin/impl defs).
- `types.rs`: `Ty`, `MixinRef` (built from a source bound by `MixinRef::from_bound`, or `from_bound_name` in pass 1), `MoveSemantics`, the copy and thread-safety queries, plus `nominal_definition`, the one lookup from a nominal `Ty` to its class/struct/enum definition.
- `types/ty_helpers.rs`: `BUILTIN_HEAD_HOMES` and the canonical `Ty` helpers (`map_inner`, `any`, `subst_params`, `peel_refs`, `callable_signature`/`is_callable`, `builtin_head`/`from_builtin_head`, `method_home`, `is_string_form`, `is_scalar_primitive`), with their unit tests.
- `context.rs`: `TypeContext`, the inference-variable store typeck resolves through.
- `visit.rs`: the shared exhaustive traversal (`Visit`/`walk_item`/`walk_func`/`walk_expr`/`walk_stmt`/`walk_pattern`, and `VisitMut` with the `_mut` walks plus `visit_ty_mut`).
- `mod.rs`: module wiring and `IO_TEXT_BUILTINS`.
- `tests.rs`: unit tests.

## Public surface
- The node and type definitions, consumed by `typeck`, `borrow_check`, `mir`, `no_std` and the tools.
- `hir::visit`: the shared body traversal. Users today: `typeck`, `borrow_check`, `no_std`, MIR lowering (`function`, `captures`, `closure_inline`, `monomorphize`, `trait_default`) and the `ruxen_ide` body walkers in `completion`, `folding`, `inlay_hints`, `node_finder`, `references`, `semantic_tokens`, `signature_help` and `use_index`. Not yet: the IDE definition-resolution walkers (`highlight`, `rename`, `type_def`, `code_actions` and the resolution halves of `references`, `use_index`, `inlay_hints`, `signature_help`, `completion`) still hand-roll their matches; they move onto it in F-35 (docs/audits/2026-10-code-debt.md). New code that needs "all expressions in a body" uses it.

## Depends on
- `lexer` (spans, format specs), `parser::ast` (operators, visibility, parameter defaults), `resolve::symbols` (`Capture::current_ty` and the symbol-aware `Ty` queries in `types.rs`).

## Invariants & gotchas
- `walk_*` in `visit.rs` hold the only exhaustive matches over the HIR; they have no `_` arm, so a new variant is a compile error until it is walked. Do not add a `_` arm.
- `walk_item` reaches function bodies and const initializers only; item-level types (fields, variants, aliases) are left to a consumer's `visit_item`.
- `Capture::ty` is a resolve-time snapshot; the walks do not visit it. Use `Capture::current_ty` after typeck.
- Use the canonical `Ty` helpers instead of hand-written matches: `peel_refs` for reference peeling, `subst_params` for type-parameter substitution, `builtin_head` for the two spellings of a builtin head (`Ty::Array(T)` and the `Ty::Class { "Array", [T] }` that `self` has inside the shell's body), `method_home` for the class a type's methods live on. `map_inner` and `any` are exhaustive (no `_` arm) and reach mixin-bound generic args; a `TypeParam`'s own bounds are not children.
- Consumers stop recursion by not calling `walk_*` (nested loops for `break`, nested closures for `next`).
- Call shape: after typeck a `FieldAccess` names a real field (or a tuple / newtype index), or has a receiver typeck could not resolve; every other `x.m` is a `MethodCall` with `parens: false`. A parsed call's `parens` is copied from the AST (`x.m(a)` true; `x.m "a"` and `x.m { }` false). `safe_nav` marks `x&.m`: on a `T?` receiver typeck types the member against `T` and wraps the result, and MIR runs it only on a present value. Operators stay `BinaryOp`/`UnaryOp`/`Index`; MIR routes them to operator methods.
- Method selection: typeck records the method it selected on a `MethodCall` in `resolved` for every receiver head (user types, builtin heads, scalar primitives); read the callee's declared signature from it. `method` carries the same DefId only for a class, struct or enum receiver: MIR dispatches on `method`, and a builtin head mangles through the FFI alias map instead. An `extension` method is a free `Function`, not a `Method` of the head, so typeck selects none for it.
