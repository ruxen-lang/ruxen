//! Phase 7 integration tests: unsafe blocks, raw pointer types, FFI lib blocks,
//! null literal, and @[repr(C)] attributes.

use riven_core::lexer::Lexer;
use riven_core::parser::ast::*;
use riven_core::parser::Parser;

fn parse(source: &str) -> Program {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lexer failed");
    let mut parser = Parser::new(tokens);
    parser.parse().expect("parser failed")
}

// ── Unsafe Block Parsing ─────────────────────────────────────────────

#[test]
fn parse_unsafe_block() {
    let source = r#"
def main
  let x = unsafe
    42
  end
end
"#;
    let program = parse(source);
    assert!(!program.items.is_empty());
}

#[test]
fn parse_unsafe_block_with_statements() {
    let source = r#"
def main
  unsafe
    let x = 1
    let y = 2
  end
end
"#;
    let program = parse(source);
    assert!(!program.items.is_empty());
}

// ── Null Literal ─────────────────────────────────────────────────────

#[test]
fn parse_null_literal() {
    let source = r#"
def main
  let p = nil
end
"#;
    let program = parse(source);
    assert!(!program.items.is_empty());

    if let TopLevelItem::Function(f) = &program.items[0] {
        assert_eq!(f.name, "main");
        if let Statement::Let(binding) = &f.body.statements[0] {
            if let Some(val) = &binding.value {
                assert!(matches!(val.kind, ExprKind::NullLiteral));
            }
        }
    }
}

// ── Raw Pointer Type Parsing ─────────────────────────────────────────

#[test]
fn parse_raw_pointer_type() {
    let source = r#"
def foo(p: *Int64) -> *Int64
  p
end
"#;
    let program = parse(source);
    assert!(!program.items.is_empty());

    if let TopLevelItem::Function(f) = &program.items[0] {
        assert_eq!(f.name, "foo");
        assert_eq!(f.params.len(), 1);
        // Verify the parameter type is a raw pointer
        if let TypeExpr::RawPointer { mutable, .. } = &f.params[0].type_expr {
            assert!(!mutable);
        } else {
            panic!("expected raw pointer type for parameter");
        }
        // Verify the return type is a raw pointer
        if let Some(TypeExpr::RawPointer { mutable, .. }) = &f.return_type {
            assert!(!mutable);
        } else {
            panic!("expected raw pointer return type");
        }
    }
}

#[test]
fn parse_raw_mut_pointer_type() {
    let source = r#"
def bar(p: *mut Int64) -> *mut Int64
  p
end
"#;
    let program = parse(source);
    assert!(!program.items.is_empty());

    if let TopLevelItem::Function(f) = &program.items[0] {
        if let TypeExpr::RawPointer { mutable, .. } = &f.params[0].type_expr {
            assert!(mutable);
        } else {
            panic!("expected raw mut pointer type");
        }
    }
}

// ── Lib Block Parsing ────────────────────────────────────────────────

#[test]
fn parse_lib_block() {
    let source = r#"
lib LibM
  def sin(x: Float64) -> Float64
  def cos(x: Float64) -> Float64
end
"#;
    let program = parse(source);
    assert!(!program.items.is_empty());

    if let TopLevelItem::Lib(lib) = &program.items[0] {
        assert_eq!(lib.name, "LibM");
        assert_eq!(lib.functions.len(), 2);
        assert_eq!(lib.functions[0].name, "sin");
        assert_eq!(lib.functions[1].name, "cos");

        // Verify parameter types
        assert_eq!(lib.functions[0].params.len(), 1);
        assert_eq!(lib.functions[0].params[0].name, "x");
        assert!(!lib.functions[0].is_variadic);
    } else {
        panic!("expected Lib item, got {:?}", program.items[0]);
    }
}

#[test]
fn parse_lib_block_with_link_attr() {
    let source = r#"
@[link("m")]
lib LibM
  def sqrt(x: Float64) -> Float64
end
"#;
    let program = parse(source);
    assert!(!program.items.is_empty());

    if let TopLevelItem::Lib(lib) = &program.items[0] {
        assert_eq!(lib.name, "LibM");
        assert_eq!(lib.link_attrs.len(), 1);
        assert_eq!(lib.link_attrs[0].name, "m");
        assert_eq!(lib.functions.len(), 1);
        assert_eq!(lib.functions[0].name, "sqrt");
    } else {
        panic!("expected Lib item");
    }
}

// ruby-naming.spec.md §3.7 / §10a: `extern "X" ... end` blocks are
// retired; FFI declarations live inside a `lib "X" ... end` block. The
// AST node these tests previously asserted (`TopLevelItem::Extern`) is
// no longer produced; the equivalent `TopLevelItem::Lib` carries the
// same function signatures.

#[test]
fn parse_lib_block_replaces_extern() {
    let source = r#"
lib "c"
  def getenv(name: *Int8) -> *Int8
end
"#;
    let program = parse(source);
    assert!(!program.items.is_empty());

    if let TopLevelItem::Lib(lib) = &program.items[0] {
        assert_eq!(lib.name, "c");
        assert_eq!(lib.functions.len(), 1);
        assert_eq!(lib.functions[0].name, "getenv");
    } else {
        panic!("expected Lib item");
    }
}

#[test]
fn parse_ffi_void_return() {
    let source = r#"
lib "c"
  def free(ptr: *mut Int8)
end
"#;
    let program = parse(source);

    if let TopLevelItem::Lib(lib) = &program.items[0] {
        assert_eq!(lib.functions[0].name, "free");
        assert!(lib.functions[0].return_type.is_none());
    } else {
        panic!("expected Lib item");
    }
}

#[test]
fn parse_ffi_multiple_params() {
    let source = r#"
lib "c"
  def memcpy(dest: *mut Int8, src: *Int8, n: UInt64) -> *mut Int8
end
"#;
    let program = parse(source);

    if let TopLevelItem::Lib(lib) = &program.items[0] {
        let f = &lib.functions[0];
        assert_eq!(f.name, "memcpy");
        assert_eq!(f.params.len(), 3);
        assert_eq!(f.params[0].name, "dest");
        assert_eq!(f.params[1].name, "src");
        assert_eq!(f.params[2].name, "n");
        assert!(f.return_type.is_some());
    } else {
        panic!("expected Lib item");
    }
}

// ── @[repr(C)] Struct Parsing ────────────────────────────────────────

#[test]
fn parse_repr_c_struct() {
    let source = r#"
@[repr(C)]
struct Point
  x: Float64
  y: Float64
end
"#;
    let program = parse(source);
    assert!(!program.items.is_empty());

    if let TopLevelItem::Struct(s) = &program.items[0] {
        assert_eq!(s.name, "Point");
        assert_eq!(s.fields.len(), 2);
        // `@[repr(C)]` is stored on its own `repr` field — not stuffed
        // into `derive_traits`. The dispatched `@[derive(...)]` form
        // populates `derive_traits` instead.
        assert!(s.repr.iter().any(|r| r == "C"));
        assert!(
            s.derive_traits.is_empty(),
            "repr must not leak into derive_traits: {:?}",
            s.derive_traits
        );
    } else {
        panic!("expected Struct item");
    }
}

// ── @[derive(...)] parse surface ───────────────────────────────────

#[test]
fn parse_derive_attr_on_struct() {
    let source = r#"
@[derive(Copy, Clone)]
struct Pair
  x: Int
  y: Int
end
"#;
    let program = parse(source);
    if let TopLevelItem::Struct(s) = &program.items[0] {
        assert_eq!(s.name, "Pair");
        assert_eq!(
            s.derive_traits,
            vec!["Copy".to_string(), "Clone".to_string()]
        );
        assert!(s.repr.is_empty());
    } else {
        panic!("expected Struct item");
    }
}

#[test]
fn parse_derive_attr_on_class() {
    let source = r#"
@[derive(Debug)]
class Widget
  value: Int
end
"#;
    let program = parse(source);
    if let TopLevelItem::Class(c) = &program.items[0] {
        assert_eq!(c.name, "Widget");
        assert_eq!(c.derive_traits, vec!["Debug".to_string()]);
    } else {
        panic!("expected Class item");
    }
}

#[test]
fn parse_derive_attr_on_enum() {
    let source = r#"
@[derive(Debug, Clone)]
enum Color
  Red
  Green
  Blue
end
"#;
    let program = parse(source);
    if let TopLevelItem::Enum(e) = &program.items[0] {
        assert_eq!(e.name, "Color");
        assert_eq!(
            e.derive_traits,
            vec!["Debug".to_string(), "Clone".to_string()]
        );
    } else {
        panic!("expected Enum item");
    }
}

#[test]
fn parse_inbody_include_on_enum() {
    // ruby-naming.spec.md §3.6 / §10a: the in-body `derive D1, D2`
    // form is replaced by `include D1, D2` (the loud, explicit form
    // — structural mixins are also implicitly included).
    let source = r#"
enum Shape
  Circle
  Square

  include Debug
end
"#;
    let program = parse(source);
    if let TopLevelItem::Enum(e) = &program.items[0] {
        assert_eq!(e.inner_impls.len(), 1);
        assert_eq!(e.inner_impls[0].trait_name.segments, vec!["Debug"]);
        let _legacy_derive_traits_still_empty: &Vec<String> = &e.derive_traits;
        assert_eq!(e.derive_traits, vec![] as Vec<String>);
        assert_eq!(e.variants.len(), 2);
    } else {
        panic!("expected Enum item");
    }
}

#[test]
fn parse_inbody_include_on_class() {
    // ruby-naming.spec.md §3.6 / §10a: in-body `derive D1, D2` →
    // `include D1, D2`. Each becomes its own `InnerImpl` entry.
    let source = r#"
class Counter
  value: Int

  include Debug
  include Clone
end
"#;
    let program = parse(source);
    if let TopLevelItem::Class(c) = &program.items[0] {
        let traits: Vec<String> = c
            .inner_impls
            .iter()
            .map(|i| i.trait_name.segments.join("."))
            .collect();
        assert_eq!(traits, vec!["Debug".to_string(), "Clone".to_string()]);
    } else {
        panic!("expected Class item");
    }
}

// ── Layout Tests ─────────────────────────────────────────────────────

#[test]
fn raw_pointer_layout() {
    use riven_core::codegen::layout::layout_of;
    use riven_core::hir::types::Ty;
    use riven_core::resolve::symbols::SymbolTable;

    let symbols = SymbolTable::new();

    let ptr_layout = layout_of(&Ty::RawPtr(Box::new(Ty::Int64)), &symbols);
    assert_eq!(ptr_layout.size, 8);
    assert_eq!(ptr_layout.alignment, 8);

    let mut_ptr_layout = layout_of(&Ty::RawPtrMut(Box::new(Ty::Int32)), &symbols);
    assert_eq!(mut_ptr_layout.size, 8);
    assert_eq!(mut_ptr_layout.alignment, 8);

    let void_ptr_layout = layout_of(&Ty::RawPtrVoid, &symbols);
    assert_eq!(void_ptr_layout.size, 8);
    assert_eq!(void_ptr_layout.alignment, 8);

    let mut_void_ptr_layout = layout_of(&Ty::RawPtrMutVoid, &symbols);
    assert_eq!(mut_void_ptr_layout.size, 8);
    assert_eq!(mut_void_ptr_layout.alignment, 8);
}

#[test]
fn packed_struct_layout() {
    use riven_core::codegen::layout::{packed_struct_layout, TypeLayout};

    // A struct with UInt8 (1 byte) and Int64 (8 bytes).
    // Packed: no padding, total = 9 bytes, align = 1.
    let fields = vec![
        TypeLayout {
            size: 1,
            alignment: 1,
            field_offsets: vec![],
        },
        TypeLayout {
            size: 8,
            alignment: 8,
            field_offsets: vec![],
        },
    ];
    let layout = packed_struct_layout(&fields);
    assert_eq!(layout.size, 9);
    assert_eq!(layout.alignment, 1);
    assert_eq!(layout.field_offsets, vec![0, 1]);
}

#[test]
fn transparent_struct_layout() {
    use riven_core::codegen::layout::{transparent_struct_layout, TypeLayout};

    let fields = vec![TypeLayout {
        size: 8,
        alignment: 8,
        field_offsets: vec![],
    }];
    let layout = transparent_struct_layout(&fields).unwrap();
    assert_eq!(layout.size, 8);
    assert_eq!(layout.alignment, 8);
}

#[test]
fn transparent_struct_rejects_multiple_fields() {
    use riven_core::codegen::layout::{transparent_struct_layout, TypeLayout};

    let fields = vec![
        TypeLayout {
            size: 8,
            alignment: 8,
            field_offsets: vec![],
        },
        TypeLayout {
            size: 4,
            alignment: 4,
            field_offsets: vec![],
        },
    ];
    assert!(transparent_struct_layout(&fields).is_err());
}

#[test]
fn c_struct_layout_matches_regular() {
    use riven_core::codegen::layout::{c_struct_layout, layout_struct_fields, TypeLayout};

    // C layout should match the regular struct layout
    // (since Riven already uses C-compatible layout rules).
    let fields = vec![
        TypeLayout {
            size: 1,
            alignment: 1,
            field_offsets: vec![],
        },
        TypeLayout {
            size: 8,
            alignment: 8,
            field_offsets: vec![],
        },
        TypeLayout {
            size: 4,
            alignment: 4,
            field_offsets: vec![],
        },
    ];
    let c_layout = c_struct_layout(&fields);
    let regular_layout = layout_struct_fields(&fields);
    assert_eq!(c_layout.size, regular_layout.size);
    assert_eq!(c_layout.alignment, regular_layout.alignment);
    assert_eq!(c_layout.field_offsets, regular_layout.field_offsets);
}
