# 06.93 — module-qualified class resolution: nested namespaces for the stdlib

**Depends on:** #06.8 (mixin lib_decls + bootstrap merge),
#06.95-preflight (mixin-include propagation — already landed).
**Blocks:** #06.95 (stdlib packagization) Phase C — the module + mixin
reshape for `BufReader`, `BufWriter`, `Shutdown`, `SeekFrom`,
`IoError` rests entirely on this prompt.
**Reads:**
`compiler/riven_core/src/resolve/ffi_registration.rs` (Pass 1's
`register_top_level_type_with_ffi` Module arm at lines 630-642),
`compiler/riven_core/src/resolve/types.rs:166-200` (`resolve_type_path`),
`compiler/riven_core/src/resolve/funcs.rs:298+` (`resolve_child_in_def`),
`compiler/riven_core/src/parser/types.rs:305-340` (`parse_type_path`),
`compiler/riven_core/src/parser/ast.rs:48-53` (`TypePath` struct),
`compiler/riven_core/src/resolve/scope.rs` (`ScopeStack`,
`ScopeKind`),
`compiler/riven_core/tests/module_class_qualified_type.rs` (the
ignored success-criterion test).

**Status:** plan only — no code changes. Phasing spelled out so each
phase lands as one commit with narrow pin tests. The ignored test
in `tests/module_class_qualified_type.rs` is the success criterion;
when this prompt's Phase 4 lands, that test gets its `#[ignore]`
attribute removed.

---

## Why this prompt exists

The #06.95 plan adopts a module + mixin shape for tagged-union stdlib
classes:

```riven
module BufReader
  mixin Reader
    lib "riven_runtime"
      def read_line as "riven_bufreader_read_line"(self) -> ...
    end
  end
  class File
    include Reader
    lib "riven_runtime"
      def self.new(inner: ::File) -> BufReader.File
    end
  end
end

# Usage:
let br = BufReader.File.new(f)
```

The 06.95 pre-flight discovered that the resolver does NOT support
this today. Probing with a minimal fixture (`tests/fixtures/riven/
module_class_qualified_type.rvn`):

```riven
module Outer
  class Inner
    lib "riven_runtime"
      def self.make as "riven_test_extern_add_one"(x: Int) -> Int
    end
  end
end

def main() -> Int
  let r = Outer.Inner.make(41)
  ...
end
```

Fails with `typecheck errors: undefined enum variant `Outer.Inner``.
The resolver:

1. Parses `module Outer { class Inner }` fine.
2. Registers `Inner` into the **un-qualified** type scope via
   `r.scopes.insert_type("Inner", id)` (line 638 of
   `ffi_registration.rs`).
3. Does NOT register `"Outer.Inner"` into `type_registry`.
4. At the call site `Outer.Inner.make(41)`, expression-resolution
   takes the dotted path as an enum-variant pattern (`Outer` enum,
   `Inner` variant) — the closest grammar match — and emits the
   "undefined enum variant" diagnostic.

Net: nested namespaces are absent from Riven's resolver model. The
existing stdlib's `std.io.File` works only because `File` is
top-level and `std.io`'s `Module.items` list re-exports it for
`use`-path resolution; nothing actually lives "inside" the module
in the scoping sense.

The stdlib packagization (#06.95) cannot ship its module + mixin
shape without this feature. Either we build it here, or we revert
06.95 Decision #3 and ship BufReader as top-level sibling classes
(`BufReaderFile` / `BufReaderTcp`). The user picked "build the
feature" at the 06.95 pre-flight checkpoint.

The feature is also independently valuable beyond the stdlib —
user programs with multi-context domain models (e.g.
`module Auth { class User; class Session }` +
`module Billing { class User; class Subscription }`) hit the same
need.

---

## End-state surface

```riven
# 1. Class inside module, qualified access from outside.
module Outer
  class Inner
    def self.make(x: Int) -> Int ...
  end
end

let v: Outer.Inner = Outer.Inner.make(41)

# 2. Inner-first scope walk for shadowing.
class File ... end           # top-level

module BufReader
  class File                  # BufReader.File — shadows outer
    def self.new(inner: ::File) -> BufReader.File ...
    #              ^^^^^^ root anchor: the OUTER File
  end
end

# 3. Nested modules.
module A
  module B
    class C ... end
  end
end

let c: A.B.C = A.B.C.make(...)

# 4. Cross-module references.
module Foo
  class A ... end
end

module Bar
  class B
    def self.thing(a: Foo.A) -> Foo.A ...
    #             ^^^^^^^^ resolves via type_registry["Foo.A"]
  end
end
```

---

## Touch list

| File                                                       | Change                                                                                  | LOC est. |
| ---------------------------------------------------------- | --------------------------------------------------------------------------------------- | -------- |
| `compiler/riven_core/src/parser/ast.rs`                    | `TypePath` gains `pub rooted: bool` field                                               | ~5       |
| `compiler/riven_core/src/parser/types.rs`                  | `parse_type_path` consumes optional leading `::`; sets `rooted = true`                  | ~25      |
| `compiler/riven_core/src/resolve/scope.rs`                 | New `ScopeKind::ModuleNamed(String)` variant; helpers to walk + resolve qualified names | ~50      |
| `compiler/riven_core/src/resolve/ffi_registration.rs`      | Module arm pushes/pops named module scope; registers each inner type under qualified `Outer.Inner` key in `type_registry`; nested modules concatenate (`A.B.C`) | ~80      |
| `compiler/riven_core/src/resolve/types.rs`                 | `resolve_type_path` honours `rooted`; tries qualified-key lookup before un-qualified fallback; inner-first scope walk for un-qualified names | ~60      |
| `compiler/riven_core/src/resolve/funcs.rs` + expr resolver | Distinguish `Module.Class.method(args)` from `Enum.Variant(args)` at call-resolution sites — check if first segment resolves to `DefKind::Module` and walk `items` | ~70      |
| `compiler/riven_core/src/mir/lower/expr/method_call.rs`    | Build callee name from full qualified path (`Outer_Inner_make` not `Inner_make`)        | ~30      |
| `compiler/riven_core/src/resolve/ffi_registration.rs`      | `register_class_lib_method` accepts an optional module-path prefix and mangles into the FFI alias map under the qualified name | ~30      |
| `compiler/riven_core/tests/`                               | 6 new pin tests (see "Pin tests" section)                                               | ~250     |
| **Total**                                                  |                                                                                         | **~600** |

---

## Phasing

### Phase 1 — Qualified-name registration (pass 1)

**Goal:** every class/struct/enum/mixin/typealias declared inside a
module gets a *second* `type_registry` entry under its
qualified path (`Outer.Inner`), in addition to the existing
un-qualified entry.

Tasks:
1. In `register_top_level_type_with_ffi`, thread a
   `module_path: &[String]` parameter through. Empty at top level;
   each Module-arm recursion appends its own name.
2. When registering a type (Class, Struct, Enum, Mixin, TypeAlias,
   Newtype), if `module_path` is non-empty, ALSO insert the
   qualified key (`module_path.join(".") + "." + name`) into
   `type_registry` pointing at the same DefId.
3. For nested modules (`module A { module B { ... } }`), the
   qualified key concatenates all path segments
   (`A.B.SomeType`).

Pin test: `qualified_type_registry_insertion.rs` asserts that
after resolving `module M { class C end end`, both `"C"` and
`"M.C"` resolve to the same DefId in `type_registry`.

**No surface behaviour change yet** — un-qualified `class C end` at
top level still works; module-qualified `M.C` at a type position
(`let x: M.C`) is the new capability. Method calls via qualified
path still don't work (Phase 3 fixes that).

### Phase 2 — `::Name` root anchor

**Goal:** `::Name` at a type position resolves to the global
namespace, skipping enclosing module scopes.

Tasks:
1. `TypePath` gains `pub rooted: bool` field. Default `false`.
   Update the 4 construction sites in `parser/types.rs`.
2. `parse_type_path` peeks at the first token: if `ColonColon`,
   consume it and set `rooted = true`. Otherwise unchanged.
3. `resolve_type_path` honours `rooted`: skip
   `scopes.lookup_type` (which walks enclosing scopes), look up
   directly in `type_registry` by the un-qualified key (the path
   inside `::X` is canonically un-qualified — by definition
   resolving from the root).

Pin tests:
- `root_anchor_parses.rs` — `let x: ::Foo` parses, AST carries
  `rooted: true`.
- `root_anchor_resolves_globally.rs` — inside `module M { class
  Foo end ... let x: ::Foo = ... }` where a top-level
  `class Foo` exists, `::Foo` resolves to the top-level one, not
  `M.Foo`.

### Phase 3 — Module-qualified method dispatch

**Goal:** `Outer.Inner.method(args)` resolves correctly at the call
site and lowers to the right C symbol.

Tasks:
1. Expression-resolution: before treating `A.B.method(args)` as an
   enum-variant pattern, check whether `A` resolves to a Module
   DefKind. If yes AND `B` is in its `items` list as a Class,
   route to method-on-class resolution with the class identified
   by `A.B`. If no, fall through to the existing enum-variant
   path.
2. MIR method-call lowering: when the receiver path has 2+ class
   segments (i.e. `Outer.Inner` not just `Inner`), build the
   mangled callee using `_` joining the full path
   (`Outer_Inner_method`). The FFI alias map keys these too.
3. `register_class_lib_method` accepts an optional `module_path:
   &[String]` and builds the mangled riven_name accordingly:
   `format!("{}_{}_{}", module_path.join("_"), class_name,
   method_name)`. The C-symbol alias still points at whatever
   `lib "X" def foo as "real_c_name"` declared — the mangling
   only affects the Riven-side key.

Pin tests:
- `module_class_qualified_type.rs` — un-ignore the existing
  fixture; it now passes.
- `qualified_method_dispatches_to_aliased_c_symbol.rs` — declare
  `module M { class C { lib ... def self.f as "rt_thing"(...) }
  end end`; call `M.C.f(...)`; assert the binary links against
  `rt_thing`.

### Phase 4 — Inner-first scope shadowing

**Goal:** inside `module Foo`, the un-qualified name `Bar`
resolves to `Foo.Bar` if such a type exists in this module,
otherwise walks outward.

Tasks:
1. New `ScopeKind::ModuleNamed(String)` scope variant. Pushed by
   the Module arm of `register_top_level_type_with_ffi` before
   recursing into sub-items; popped after.
2. `scopes.insert_type` for items inside a `ModuleNamed` scope
   adds the binding to that scope frame (not to global).
3. `scopes.lookup_type` walks inner-to-outer as today — but
   `ModuleNamed` frames are now in the stack. Inside `module Foo`,
   a lookup for `Bar` checks the Foo-named module's bindings
   first, then the global module scope.
4. The Phase 1 qualified-name registration into `type_registry`
   stays — that's the external view. The new scope frame is for
   in-module lookup.

Pin tests:
- `inner_class_shadows_outer.rs` — top-level `class File end`;
  `module M { class File end ... let x: File = ...}`; the `File`
  inside M resolves to `M.File`, not the top-level.
- `root_anchor_disambiguates.rs` — same setup; inside M, `::File`
  resolves to the top-level.
- `nested_module_lookup_walks_outward.rs` — `module M { module N
  { let x: SomeTopLevelType = ... } }` resolves correctly when
  `SomeTopLevelType` is declared neither in `M` nor in `N`.

### Phase 5 — Mixin include + qualified class interplay

**Goal:** `module M { mixin Mx ... end; class C; include Mx end end`
works — the mixin's lib decls propagate to `M.C` with the FFI
alias map keyed under `M_C_method`.

Tasks:
1. Extend the #06.95 pre-flight pre-pass
   (`collect_mixin_lib_decls`) to use qualified mixin names
   (`M.Mx` not just `Mx`) so `include` directives that reference
   a module-qualified mixin (`include Foo.Bar`) resolve.
2. The Class arm's include-walk re-registers under the *qualified*
   class name including the module path.

Pin test:
- `qualified_class_includes_qualified_mixin.rs` — full end-to-end:
  `module M { mixin Mx; lib; def foo as "rt"; end end; class C;
  include Mx end end`; user code calls `M.C.foo(...)`; binary
  links to `rt`.

---

## Open decisions

1. **Mangled name separator.** `Outer.Inner.method` → `Outer_Inner_method`
   or `OuterDotInnerDotMethod` or `Outer__Inner__method`?
   Recommendation: single underscore matches existing `ClassName_method`
   shape. Risk: collision with a top-level class literally named
   `Outer_Inner`. Mitigation: error E07XX on declaration if any
   top-level name in the global scope conflicts with any
   qualified-name-mangled string in scope.
2. **Symbol-table representation of nested modules.** Add `module_path`
   to `DefKind::Class { info, module_path }` etc. so downstream consumers
   can ask the symbol table "what module does this class live in"?
   Recommendation: yes — many use sites (LSP, doc generator, error
   messages) want this. Carry it through Phase 1.
3. **Re-exports.** Should `module Foo` be able to re-export an outer
   class with `use ::Bar as Baz` or similar? Out of scope for 06.93.
   Tracked separately.

---

## Risks

- **R1 — Expression-resolution disambiguation false positives.**
  Phase 3 has to distinguish `Module.Class.method(args)` from
  `Enum.Variant(args)`. The first segment determines the path; if a
  module and an enum share a name, the resolver picks the module
  (modules win because they're a namespace). **Mitigation:** parser
  rejects duplicate top-level names already (`class Foo` and `enum
  Foo` can't coexist).
- **R2 — Symbol-table churn.** Adding `module_path` to every type
  DefKind variant is a wide AST/HIR change. **Mitigation:** isolate
  in Phase 1; downstream consumers ignore the new field until they
  need it.
- **R3 — Existing un-qualified stdlib lookups break.** `let x: File`
  must still resolve to the top-level `File`. **Mitigation:** Phase 1
  keeps un-qualified registration as today; the qualified entry is
  *additive*. Phase 4's inner-first walk only affects code inside
  module bodies — top-level code (and existing tests) unaffected.

---

## Effort estimate

| Phase | Description                          | LOC | New pin tests |
| ----- | ------------------------------------ | --- | ------------- |
| 1     | Qualified-name registration          | 100 | 1             |
| 2     | `::Name` root anchor                 | 90  | 2             |
| 3     | Module-qualified method dispatch     | 130 | 2             |
| 4     | Inner-first scope shadowing          | 200 | 3             |
| 5     | Mixin/include interplay              | 80  | 1             |
| **Total** |                                  | 600 | 9             |

1–2 weeks of focused work. Each phase is independently testable.
Phase 4 is the highest-risk because it changes scope-walk semantics
that touch every name lookup; budget extra iteration there.

---

## Success criterion

`compiler/riven_core/tests/module_class_qualified_type.rs` has its
`#[ignore]` attribute removed and the test passes. That implies:
- Class inside module is declared and resolves at qualified path.
- Class method via qualified path dispatches correctly.
- FFI alias map carries the right key.
- The binary links and exits 0.

Once this is true, #06.95 Phase A can start.
