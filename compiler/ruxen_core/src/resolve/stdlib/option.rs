//! `Option[T]` builtin enum registration.
//!
//! Tag layout: `None = 0`, `Some = 1` (matches runtime convention:
//! `ruxen_vec_get_opt`, `ruxen_option_unwrap_or`, inline_find, etc.).
//! Registers qualified (`Option.Some`), bare (`Some`), and dot-prefixed
//! (`.Some`) names so the parser's empty-type_path variants resolve.

use crate::hir::types::Ty;
use crate::lexer::token::Span;
use crate::parser::ast::Visibility;

use super::super::symbols::*;
use super::super::Resolver;

pub(super) fn register_option(r: &mut Resolver) {
    let span = Span {
        start: 0,
        end: 0,
        line: 0,
        column: 0,
    };

    let option_id = r.symbols.define(
        "Option".to_string(),
        DefKind::Enum {
            info: EnumInfo {
                generic_params: vec![GenericParamInfo::type_param("T".to_string(), vec![])],
                variants: vec![], // will be filled below
                derive_traits: vec![],
                opt_out_send: false,
                opt_out_sync: false,
                manual_send: false,
                manual_sync: false,
                const_predicates: vec![],
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes.insert_type("Option".to_string(), option_id);
    r.type_registry.insert("Option".to_string(), option_id);

    let none_id = r.symbols.define(
        "None".to_string(),
        DefKind::EnumVariant {
            parent: option_id,
            variant_idx: 0,
            kind: VariantDefKind::Unit,
        },
        Visibility::Public,
        span.clone(),
    );
    let some_id = r.symbols.define(
        "Some".to_string(),
        DefKind::EnumVariant {
            parent: option_id,
            variant_idx: 1,
            kind: VariantDefKind::Tuple(vec![Ty::TypeParam {
                name: "T".to_string(),
                bounds: vec![],
            }]),
        },
        Visibility::Public,
        span.clone(),
    );
    // Register qualified and bare names
    r.scopes.insert("Option.Some".to_string(), some_id);
    r.scopes.insert("Option.None".to_string(), none_id);
    r.scopes.insert("Some".to_string(), some_id);
    r.scopes.insert("None".to_string(), none_id);
    // Also register bare names that the parser generates with empty type_path: ".Some", ".None"
    r.scopes.insert(".Some".to_string(), some_id);
    r.scopes.insert(".None".to_string(), none_id);

    // Update Option enum with variant DefIds
    if let Some(opt_def) = r.symbols.get_mut(option_id) {
        if let DefKind::Enum { ref mut info } = opt_def.kind {
            info.variants = vec![none_id, some_id];
        }
    }
}
