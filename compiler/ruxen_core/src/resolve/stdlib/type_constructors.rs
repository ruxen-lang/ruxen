//! Value-scope bindings for the builtin-head type names.
//!
//! `Array`, `Hash`, `Set` and `String` have no type-scope class entry
//! (`resolve_type_expr` builds their primitive `Ty` directly), so each
//! name is registered in the value scope as a `DefKind::Variable`
//! carrying that `Ty`. That lets `Array.new(...)` / `String.new` resolve
//! the receiver. Every other class used as a receiver (`Duration`,
//! `Mutex`, `File`, ...) resolves through its own type-scope entry.

use crate::hir::types::Ty;
use crate::lexer::token::Span;
use crate::parser::ast::Visibility;

use super::super::symbols::*;
use super::super::Resolver;

pub(super) fn register_type_constructors(r: &mut Resolver) {
    let span = Span {
        start: 0,
        end: 0,
        line: 0,
        column: 0,
    };

    let array_ty = Ty::Array(Box::new(Ty::TypeParam {
        name: "T".to_string(),
        bounds: vec![],
    }));
    let map_ty = Ty::Map(
        Box::new(Ty::TypeParam {
            name: "K".to_string(),
            bounds: vec![],
        }),
        Box::new(Ty::TypeParam {
            name: "V".to_string(),
            bounds: vec![],
        }),
    );
    let set_ty = Ty::Set(Box::new(Ty::TypeParam {
        name: "T".to_string(),
        bounds: vec![],
    }));

    let type_constructors: Vec<(&str, Ty)> = vec![
        ("Array", array_ty),
        ("Hash", map_ty),
        ("Set", set_ty),
        ("String", Ty::String),
    ];
    for (name, ty) in type_constructors {
        let id = r.symbols.define(
            name.to_string(),
            DefKind::Variable { mutable: false, ty },
            Visibility::Public,
            span.clone(),
        );
        r.scopes.insert(name.to_string(), id);
    }
}
