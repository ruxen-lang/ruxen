//! Builtin primitive type registrations.
//!
//! Registers the closed set of primitive `Ty`s (`Int`, `Int8`…`UInt64`,
//! `Float`, `Bool`, `Char`, `String`, etc.) as `DefKind::TypeAlias`
//! entries so they can be referenced by name in user code. Each entry
//! lands in both the type scope and the `type_registry`.

use crate::hir::types::Ty;
use crate::lexer::token::Span;
use crate::parser::ast::Visibility;

use super::super::symbols::*;
use super::super::Resolver;

pub(super) fn register_primitives(r: &mut Resolver) {
    let builtins = [
        ("Int", Ty::Int),
        ("Int8", Ty::Int8),
        ("Int16", Ty::Int16),
        ("Int32", Ty::Int32),
        ("Int64", Ty::Int64),
        ("UInt", Ty::UInt),
        ("UInt8", Ty::UInt8),
        ("UInt16", Ty::UInt16),
        ("UInt32", Ty::UInt32),
        ("UInt64", Ty::UInt64),
        ("ISize", Ty::ISize),
        ("USize", Ty::USize),
        ("Float", Ty::Float),
        ("Float32", Ty::Float32),
        ("Float64", Ty::Float64),
        ("Bool", Ty::Bool),
        ("Char", Ty::Char),
        ("String", Ty::String),
    ];

    let span = Span {
        start: 0,
        end: 0,
        line: 0,
        column: 0,
    };

    for (name, ty) in builtins {
        let id = r.symbols.define(
            name.to_string(),
            DefKind::TypeAlias { target: ty },
            Visibility::Public,
            span.clone(),
        );
        r.scopes.insert_type(name.to_string(), id);
        r.type_registry.insert(name.to_string(), id);
    }
}
