//! Pin tests for `@[derive(...)]` attribute form vs in-body `derive`
//! clause parity.
//!
//! `tests/release-e2e/cases/131_attr_derive_copy.rvn` covers Copy + Clone
//! end-to-end.  This file pins **AST shape parity** — both forms must
//! populate `StructDef.derive_traits` with the same `Vec<String>` so
//! every downstream pass (typeck, MIR derive expansion) treats them
//! identically.
//!
//! Confirms the audit-noted thin coverage of the attribute form for
//! derives beyond Copy.

use riven_core::lexer::Lexer;
use riven_core::parser::ast::{Program, TopLevelItem};
use riven_core::parser::Parser;

fn parse(src: &str) -> Program {
    let mut lx = Lexer::new(src);
    let toks = lx.tokenize().expect("lex");
    let mut p = Parser::new(toks);
    p.parse().expect("parse")
}

fn struct_derives<'a>(prog: &'a Program, name: &str) -> &'a Vec<String> {
    for item in &prog.items {
        if let TopLevelItem::Struct(s) = item {
            if s.name == name {
                return &s.derive_traits;
            }
        }
    }
    panic!("no struct `{}` in program", name);
}

#[test]
fn attr_derive_debug_parity_with_in_body_form() {
    let src = r#"
@[derive(Debug)]
struct AttrPoint
  x: Int
end

struct BodyPoint
  x: Int
  derive Debug
end
"#;
    let prog = parse(src);
    assert_eq!(struct_derives(&prog, "AttrPoint"), struct_derives(&prog, "BodyPoint"));
    assert!(struct_derives(&prog, "AttrPoint").contains(&"Debug".to_string()));
}

#[test]
fn attr_derive_multi_parity_with_in_body_form() {
    // Five distinct derives in one attribute and in the body.
    let src = r#"
@[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct AttrThing
  x: Int
end

struct BodyThing
  x: Int
  derive Debug, Clone, PartialEq, Eq, Hash
end
"#;
    let prog = parse(src);
    let mut attr: Vec<String> = struct_derives(&prog, "AttrThing").clone();
    let mut body: Vec<String> = struct_derives(&prog, "BodyThing").clone();
    attr.sort();
    body.sort();
    assert_eq!(attr, body);
    for expected in ["Debug", "Clone", "PartialEq", "Eq", "Hash"] {
        assert!(
            attr.iter().any(|s| s == expected),
            "missing `{}` in attribute-form derives: {:?}",
            expected,
            attr
        );
    }
}

#[test]
fn attr_derive_default_ord_partial_ord_parity() {
    let src = r#"
@[derive(Default, Ord, PartialOrd)]
struct AttrOrd
  x: Int
end

struct BodyOrd
  x: Int
  derive Default, Ord, PartialOrd
end
"#;
    let prog = parse(src);
    let mut attr: Vec<String> = struct_derives(&prog, "AttrOrd").clone();
    let mut body: Vec<String> = struct_derives(&prog, "BodyOrd").clone();
    attr.sort();
    body.sort();
    assert_eq!(attr, body);
    for expected in ["Default", "Ord", "PartialOrd"] {
        assert!(attr.contains(&expected.to_string()), "missing `{}`: {:?}", expected, attr);
    }
}
