//! Value-scope type-constructor variables.
//!
//! Phase D-5 of #06.95: collapse the previously hand-spelled
//! type_constructors table (~175 LOC, one tuple per type) into a
//! small data-druxen structure. The table registers each type
//! NAME in the value scope as a `DefKind::Variable` so call sites
//! like `Array.new(...)` / `String.new` / `Command.new(...)`
//! resolve the receiver to a class-id-like sentinel value. The
//! typeck path then promotes the Variable to the corresponding
//! `Ty::Class` / `Ty::Array` / … and the static-ctor fast path in
//! `mir/lower/expr/method_call.rs` handles dispatch.
//!
//! Three shape categories:
//!   - Container builtins (`Array`/`Vec`, `Map`/`HashMap`,
//!     `Set`/`HashSet`) carry a primitive Ty.
//!   - `String` carries `Ty::String`.
//!   - Every other class name carries `Ty::Class { name, ... }`
//!     with one or zero generic_args.
//!
//! Future cleanup (separate prompt): teach
//! `register_top_level_type_with_ffi`'s Class arm to insert into
//! the value scope alongside the type scope, eliminating the
//! need for the SIMPLE_CLASS_CTORS list entirely (the class .rx
//! declaration would self-register both bindings).

use crate::hir::types::Ty;
use crate::lexer::token::Span;
use crate::parser::ast::Visibility;

use super::super::symbols::*;
use super::super::Resolver;

// (name, generic_param_names) — classes with optional one-arg
// generics. `Arc` is an alias for `SharedSync`, so its
// value-scope Variable carries the SharedSync type identity.
const SIMPLE_CLASS_CTORS: &[(&str, &[&str])] = &[
    ("Thread", &[]),
    ("Duration", &[]),
    ("Instant", &[]),
    ("TcpListener", &[]),
    ("TcpStream", &[]),
    ("BufReader", &["R"]),
    ("BufWriter", &["W"]),
    ("Mutex", &["T"]),
    ("SharedSync", &["T"]),
];

fn class_ty(name: &str, gens: &[&str]) -> Ty {
    Ty::Class {
        name: name.to_string(),
        generic_args: gens
            .iter()
            .map(|g| Ty::TypeParam {
                name: g.to_string(),
                bounds: vec![],
            })
            .collect(),
    }
}

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

    let type_constructors: Vec<(&str, Ty)> = {
        let mut v: Vec<(&str, Ty)> = vec![
            ("Array", array_ty.clone()),
            ("Vec", array_ty),
            ("Hash", map_ty),
            ("Set", set_ty),
            ("String", Ty::String),
        ];
        for (name, gens) in SIMPLE_CLASS_CTORS {
            v.push((name, class_ty(name, gens)));
        }
        v
    };
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
