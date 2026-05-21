//! `Result[T, E]` builtin enum registration.
//!
//! Tag layout: `Ok = 0`, `Err = 1`. Registers qualified
//! (`Result.Ok`), bare (`Ok`), and dot-prefixed (`.Ok`) names so the
//! parser's empty-type_path variants resolve.

use crate::hir::types::Ty;
use crate::lexer::token::Span;
use crate::parser::ast::Visibility;

use super::super::symbols::*;
use super::super::Resolver;

pub(super) fn register_result(r: &mut Resolver) {
    let span = Span {
        start: 0,
        end: 0,
        line: 0,
        column: 0,
    };

    let result_id = r.symbols.define(
        "Result".to_string(),
        DefKind::Enum {
            info: EnumInfo {
                generic_params: vec![
                    GenericParamInfo::type_param("T".to_string(), vec![]),
                    GenericParamInfo::type_param("E".to_string(), vec![]),
                ],
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
    r.scopes.insert_type("Result".to_string(), result_id);
    r.type_registry.insert("Result".to_string(), result_id);

    let ok_id = r.symbols.define(
        "Ok".to_string(),
        DefKind::EnumVariant {
            parent: result_id,
            variant_idx: 0,
            kind: VariantDefKind::Tuple(vec![Ty::TypeParam {
                name: "T".to_string(),
                bounds: vec![],
            }]),
        },
        Visibility::Public,
        span.clone(),
    );
    let err_id = r.symbols.define(
        "Err".to_string(),
        DefKind::EnumVariant {
            parent: result_id,
            variant_idx: 1,
            kind: VariantDefKind::Tuple(vec![Ty::TypeParam {
                name: "E".to_string(),
                bounds: vec![],
            }]),
        },
        Visibility::Public,
        span.clone(),
    );
    // Register qualified and bare names
    r.scopes.insert("Result.Ok".to_string(), ok_id);
    r.scopes.insert("Result.Err".to_string(), err_id);
    r.scopes.insert("Ok".to_string(), ok_id);
    r.scopes.insert("Err".to_string(), err_id);
    // Also register bare names that the parser generates with empty type_path: ".Ok", ".Err"
    r.scopes.insert(".Ok".to_string(), ok_id);
    r.scopes.insert(".Err".to_string(), err_id);

    // Update Result enum with variant DefIds
    if let Some(res_def) = r.symbols.get_mut(result_id) {
        if let DefKind::Enum { ref mut info } = res_def.kind {
            info.variants = vec![ok_id, err_id];
        }
    }
}
