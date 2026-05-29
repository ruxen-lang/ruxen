//! Pins the canonical signature shape produced by the shared renderer that
//! `ruxen fmt`, hover, and signature help all rely on.

use ruxen_core::formatter::{render_fn_signature, SigSelf};

#[test]
fn no_params_omits_parens() {
    let s = render_fn_signature("get_items", SigSelf::None, false, &[], &[], Some("Array[Int]"));
    assert_eq!(s, "def get_items -> Array[Int]");
}

#[test]
fn params_rendered_with_types() {
    let params = [("a".to_string(), "Int".to_string()), ("b".to_string(), "String".to_string())];
    let s = render_fn_signature("add", SigSelf::None, false, &[], &params, Some("Int"));
    assert_eq!(s, "def add(a: Int, b: String) -> Int");
}

#[test]
fn generics_and_self_mode_and_class_method() {
    let generics = ["T".to_string()];
    let params = [("x".to_string(), "T".to_string())];
    let s = render_fn_signature("make", SigSelf::None, true, &generics, &params, Some("T"));
    assert_eq!(s, "def self.make[T](x: T) -> T");

    let s = render_fn_signature("push", SigSelf::RefMut, false, &[], &[("v".into(), "Int".into())], None);
    assert_eq!(s, "def var push(v: Int)");

    let s = render_fn_signature("into", SigSelf::Consuming, false, &[], &[], Some("Int"));
    assert_eq!(s, "def consume into -> Int");
}

#[test]
fn ref_self_has_no_prefix() {
    let s = render_fn_signature("len", SigSelf::Ref, false, &[], &[], Some("Int"));
    assert_eq!(s, "def len -> Int");
}
