//! Pin tests for #06.8 Wave 1 Task 0c: in-body `layout tagged`
//! (enum) and `layout flat_heap_struct` (class) directives.
//!
//! The Wave 1 surface is parser + resolver only:
//!   - The parser captures the directive onto `EnumDef.layout` /
//!     `ClassDef.layout` (matches StructDef's existing `layout` field).
//!   - The resolver propagates the `flat_heap_struct` marker onto
//!     `ClassInfo.flat_heap_struct` (consumed link-time once a real
//!     stdlib class adopts it; E0724 is reserved).
//!   - The resolver tracks `layout tagged` enum names per scope at
//!     forward-declaration time and emits **E0723** on a duplicate.
//!
//! Riven source goes through `tests/fixtures/riven/layout_*.rvn` —
//! no inline `r#"..."#` in `.rs` pin tests (project rule, see
//! `feedback_no_inline_rvn_in_pin_tests`).

use riven_core::lexer::Lexer;
use riven_core::parser::ast::{Program, TopLevelItem};
use riven_core::parser::Parser;
use riven_core::resolve::symbols::DefKind;
use riven_core::resolve::Resolver;

fn rvn(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/riven")
        .join(format!("{name}.rvn"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn parse_fixture(name: &str) -> Program {
    let source = rvn(name);
    let mut lx = Lexer::new(&source);
    let toks = lx.tokenize().expect("lex");
    let mut p = Parser::new(toks);
    match p.parse() {
        Ok(prog) => prog,
        Err(diags) => panic!("parse failed for fixture {}: {:#?}", name, diags),
    }
}

fn resolve_fixture(name: &str) -> riven_core::resolve::ResolveResult {
    let program = parse_fixture(name);
    Resolver::new().resolve(&program)
}

#[test]
fn enum_with_layout_tagged_parses() {
    // The directive lands on `EnumDef.layout` as `"tagged"`. Variants
    // parse normally — the directive is purely opt-in metadata at
    // this layer, not a structural marker.
    let program = parse_fixture("layout_tagged_enum_basic");
    let enums: Vec<_> = program
        .items
        .iter()
        .filter_map(|i| match i {
            TopLevelItem::Enum(e) => Some(e),
            _ => None,
        })
        .collect();
    assert_eq!(enums.len(), 1, "expected exactly one enum, got {}", enums.len());
    let e = enums[0];
    assert_eq!(e.name, "E");
    assert!(
        e.layout.iter().any(|s| s == "tagged"),
        "expected `tagged` in EnumDef.layout; got {:?}",
        e.layout
    );
    assert_eq!(
        e.variants.len(),
        2,
        "expected 2 variants (A, B); got {}",
        e.variants.len()
    );
}

#[test]
fn class_with_layout_flat_heap_struct_parses() {
    // Parser captures the directive onto `ClassDef.layout` as
    // `"flat_heap_struct"`, and the resolver propagates it onto
    // `ClassInfo.flat_heap_struct = true`. Both layers are asserted
    // because the marker is load-bearing at resolve time (it gates
    // the future E0724 link-time check), not just at parse time.
    let program = parse_fixture("layout_flat_heap_struct_class");
    let classes: Vec<_> = program
        .items
        .iter()
        .filter_map(|i| match i {
            TopLevelItem::Class(c) => Some(c),
            _ => None,
        })
        .collect();
    assert_eq!(classes.len(), 1, "expected exactly one class, got {}", classes.len());
    let c = classes[0];
    // Class name intentionally does NOT collide with any stdlib
    // built-in (e.g. `File` / `Stdin`) so the `symbols.iter()` lookup
    // below picks the user-source def unambiguously.
    assert_eq!(c.name, "FlatHeapStructDemo");
    assert!(
        c.layout.iter().any(|s| s == "flat_heap_struct"),
        "expected `flat_heap_struct` in ClassDef.layout; got {:?}",
        c.layout
    );

    let result = Resolver::new().resolve(&program);
    // Resolver should accept the fixture cleanly — no E0723, no
    // unrelated diagnostics on a single-class file with one field.
    let unexpected: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| matches!(d.code.as_deref(), Some("E0723")))
        .collect();
    assert!(
        unexpected.is_empty(),
        "did not expect E0723 on flat_heap_struct class fixture; got {:?}",
        unexpected
    );

    let class_def = result
        .symbols
        .iter()
        .find(|d| d.name == "FlatHeapStructDemo" && matches!(d.kind, DefKind::Class { .. }))
        .expect("expected a `FlatHeapStructDemo` class def in the symbol table");
    match &class_def.kind {
        DefKind::Class { info } => assert!(
            info.flat_heap_struct,
            "ClassInfo.flat_heap_struct should be true on a `layout flat_heap_struct` class"
        ),
        other => panic!("expected DefKind::Class, got {:?}", other),
    }
}

#[test]
fn duplicate_layout_tagged_enum_in_scope_emits_e0723() {
    // Two `enum E` declarations both carrying `layout tagged` in the
    // same module scope: the SECOND one is the rejected duplicate.
    // The first wins because tag order is append-only — silently
    // accepting the second would let it renumber tags 0..N out from
    // under already-built objects.
    let result = resolve_fixture("layout_tagged_enum_duplicate");
    let e0723: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.code.as_deref() == Some("E0723"))
        .collect();
    assert_eq!(
        e0723.len(),
        1,
        "expected exactly one E0723 (on the duplicate); got {:?}",
        result.diagnostics
    );
    let msg = &e0723[0].message;
    assert!(
        msg.contains("duplicate") && msg.contains("layout tagged") && msg.contains("`E`"),
        "E0723 message should name the duplicate enum; got {:?}",
        msg
    );
}

#[test]
fn enum_without_layout_tagged_is_unaffected() {
    // Sanity check: the new attribute is opt-in. A vanilla enum
    // parses, resolves, and does not trip any tagged-related
    // diagnostic — even if a later refactor accidentally duplicates
    // the name, only `layout tagged` enums hit E0723.
    let program = parse_fixture("layout_plain_enum_unaffected");
    let enums: Vec<_> = program
        .items
        .iter()
        .filter_map(|i| match i {
            TopLevelItem::Enum(e) => Some(e),
            _ => None,
        })
        .collect();
    assert_eq!(enums.len(), 1);
    let e = enums[0];
    assert!(
        e.layout.is_empty(),
        "plain enum should have no `layout` entries; got {:?}",
        e.layout
    );

    let result = Resolver::new().resolve(&program);
    let any_e0723 = result
        .diagnostics
        .iter()
        .any(|d| d.code.as_deref() == Some("E0723"));
    assert!(
        !any_e0723,
        "plain (non-tagged) enum should never trip E0723; got {:?}",
        result.diagnostics
    );
}
