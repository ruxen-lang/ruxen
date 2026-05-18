//! Pin tests for #06.8 follow-up: `lib "X" ... end` blocks INSIDE class
//! and mixin bodies (previously top-level only).
//!
//! Stdlib self-hosting needs the class-body form so e.g.
//! `library/std/src/io.rvn` can write:
//!
//! ```riven
//! class File
//!   layout flat_heap_struct
//!   fd: I32
//!
//!   lib "riven_runtime"
//!     def open as "riven_file_open"(path: &String) -> Result[File, IoError]
//!   end
//! end
//! ```
//!
//! Scope of THIS commit: parser + AST plumbing only. The class-scoped
//! `FfiFunction`s land on `ClassDef.lib_decls` (and `MixinDef.lib_decls`)
//! but are not yet registered as class methods by the resolver — that
//! is the Phase 2 follow-up. Pin tests here assert the AST shape so a
//! regression on the parser surface is caught immediately.
//!
//! Test discipline: no inline `r#"..."#` Riven source. All Riven goes
//! through `.rvn` fixtures under `tests/fixtures/riven/` (project rule
//! `feedback_no_inline_rvn_in_pin_tests`).

use riven_core::lexer::Lexer;
use riven_core::parser::ast::{ClassDef, MixinDef, TopLevelItem};
use riven_core::parser::Parser;

fn parse_fixture(path: &str) -> Result<riven_core::parser::ast::Program, String> {
    let source = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read fixture {}: {}", path, e));
    let mut lexer = Lexer::new(&source);
    let tokens = lexer
        .tokenize()
        .map_err(|d| format!("lexer errors: {:?}", d))?;
    let mut parser = Parser::new(tokens);
    parser
        .parse()
        .map_err(|d| format!("parser errors: {:?}", d))
}

fn only_class(program: &riven_core::parser::ast::Program) -> &ClassDef {
    let classes: Vec<&ClassDef> = program
        .items
        .iter()
        .filter_map(|i| match i {
            TopLevelItem::Class(c) => Some(c),
            _ => None,
        })
        .collect();
    assert_eq!(
        classes.len(),
        1,
        "expected exactly one class, found {}",
        classes.len()
    );
    classes[0]
}

fn only_mixin(program: &riven_core::parser::ast::Program) -> &MixinDef {
    let mixins: Vec<&MixinDef> = program
        .items
        .iter()
        .filter_map(|i| match i {
            TopLevelItem::Mixin(m) => Some(m),
            _ => None,
        })
        .collect();
    assert_eq!(
        mixins.len(),
        1,
        "expected exactly one mixin, found {}",
        mixins.len()
    );
    mixins[0]
}

#[test]
fn lib_block_in_class_body_parses() {
    let program = parse_fixture("tests/fixtures/riven/lib_in_class_body_single.rvn")
        .expect("fixture should parse");
    let class = only_class(&program);
    assert_eq!(class.name, "File");
    assert_eq!(
        class.lib_decls.len(),
        1,
        "class-body `lib` block should land on ClassDef.lib_decls"
    );
    let lib = &class.lib_decls[0];
    assert_eq!(lib.name, "rt");
    assert_eq!(lib.functions.len(), 1);
    let f = &lib.functions[0];
    assert_eq!(f.name, "open");
    assert_eq!(
        f.c_symbol.as_deref(),
        Some("riven_file_open"),
        "per-decl `as \"...\"` clause should round-trip through the class-body form"
    );
    // Sanity: class fields are still parsed alongside the lib block.
    assert_eq!(class.fields.len(), 1);
    assert_eq!(class.fields[0].name, "fd");
    assert_eq!(
        class.layout,
        vec!["flat_heap_struct".to_string()],
        "layout directive should coexist with class-body lib block"
    );
}

#[test]
fn multiple_lib_blocks_in_class_body_parse() {
    let program = parse_fixture("tests/fixtures/riven/lib_in_class_body_multiple.rvn")
        .expect("fixture should parse");
    let class = only_class(&program);
    assert_eq!(
        class.lib_decls.len(),
        2,
        "both back-to-back `lib` blocks should land on ClassDef.lib_decls in source order"
    );
    assert_eq!(class.lib_decls[0].name, "rt");
    assert_eq!(class.lib_decls[1].name, "rt2");

    let first = &class.lib_decls[0];
    assert_eq!(first.functions.len(), 2);
    assert_eq!(first.functions[0].name, "open");
    assert_eq!(
        first.functions[0].c_symbol.as_deref(),
        Some("riven_file_open")
    );
    assert_eq!(first.functions[1].name, "close");
    assert_eq!(
        first.functions[1].c_symbol.as_deref(),
        Some("riven_file_close")
    );

    let second = &class.lib_decls[1];
    assert_eq!(second.functions.len(), 1);
    assert_eq!(second.functions[0].name, "read");
    assert_eq!(
        second.functions[0].c_symbol.as_deref(),
        Some("riven_file_read")
    );
}

#[test]
fn lib_block_in_mixin_body_parses() {
    let program = parse_fixture("tests/fixtures/riven/lib_in_mixin_body.rvn")
        .expect("fixture should parse");
    let mixin = only_mixin(&program);
    assert_eq!(mixin.name, "Read");
    assert_eq!(
        mixin.lib_decls.len(),
        1,
        "mixin-body `lib` block should land on MixinDef.lib_decls"
    );
    let lib = &mixin.lib_decls[0];
    assert_eq!(lib.name, "rt");
    assert_eq!(lib.functions.len(), 1);
    let f = &lib.functions[0];
    assert_eq!(f.name, "__read_default");
    assert_eq!(
        f.c_symbol.as_deref(),
        Some("riven_default_read"),
        "per-decl `as \"...\"` clause should round-trip through the mixin-body form"
    );
    // Sanity: the mixin still parses its method signatures alongside the lib block.
    assert!(
        !mixin.items.is_empty(),
        "mixin should still carry its method signatures alongside the lib block"
    );
}
