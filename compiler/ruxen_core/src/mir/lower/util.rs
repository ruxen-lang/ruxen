use super::*;

pub(super) fn is_option_type(ty: &Ty) -> bool {
    match ty {
        Ty::Option(_) => true,
        Ty::Ref(inner)
        | Ty::RefMut(inner)
        | Ty::RefLifetime(_, inner)
        | Ty::RefMutLifetime(_, inner) => is_option_type(inner),
        Ty::Class { name, .. } => name.starts_with("Option"),
        _ => false,
    }
}

pub(super) fn is_result_type(ty: &Ty) -> bool {
    match ty {
        Ty::Result(_, _) => true,
        Ty::Ref(inner)
        | Ty::RefMut(inner)
        | Ty::RefLifetime(_, inner)
        | Ty::RefMutLifetime(_, inner) => is_result_type(inner),
        Ty::Class { name, .. } => name.starts_with("Result"),
        _ => false,
    }
}

/// Check if a method name is a known collection operation that takes a closure
/// and can be inlined by accessing the class's underlying Vec (first field).
pub(super) fn is_collection_method(method_name: &str) -> bool {
    matches!(
        method_name,
        "each"
            | "each_with_index"
            | "filter"
            | "where_matching"
            | "find"
            | "position"
            | "map"
            | "partition"
            | "into_filtered"
            | "display_all"
    )
}

/// Check if a type is a Vec, iterator, or similar collection type
/// that supports closure inlining (as opposed to a user-defined class
/// like Repository or TaskList).
pub(super) fn is_vec_or_iterator_type(ty: &Ty) -> bool {
    match ty {
        Ty::Array(_) => true,
        Ty::Ref(inner)
        | Ty::RefMut(inner)
        | Ty::RefLifetime(_, inner)
        | Ty::RefMutLifetime(_, inner) => is_vec_or_iterator_type(inner),
        Ty::Class { name, .. } => {
            let base = if let Some(pos) = name.find('[') {
                &name[..pos]
            } else {
                name.as_str()
            };
            // `*Iter` wrapper names removed with the orphaned iterator
            // machinery (Phase B / Milestone 2) — nothing produces them.
            //
            // `Array` appears here (alongside the legacy `Vec`) because the
            // `self` receiver INSIDE an `Array[T]` stdlib method body is
            // typed `Ty::Class { name: "Array" }`, not `Ty::Array`. The
            // closure combinators migrated to `array/src/lib.rx` invoke
            // `self.each { … }`; that inline path must recognise this
            // receiver as a backing Vec (it shares the `RuxenVec*` repr),
            // not fall through to the `is_collection_method` wrapper-class
            // branch that dereferences a non-existent field 0.
            matches!(base, "Vec" | "Array")
        }
        // For inferred types, check if the type name suggests a collection.
        Ty::Infer(_) => false,
        _ => false,
    }
}

/// Check if a method on a built-in type is a static/class method
/// (no `self` argument). These are methods like `String.with_capacity(...)`,
/// `Vec.new()`, etc. that are called on the type itself.
///
/// Single source: delegates to `runtime_abi::is_static_constructor`, the
/// reconciled union of this list and the formerly-diverged method_call.rs
/// `is_*_static_ctor` cascade. See that function for the reconciliation notes.
pub(super) fn is_builtin_static_method(type_name: &str, method_name: &str) -> bool {
    crate::mir::lower::runtime_abi::is_static_constructor(type_name, method_name)
}

/// Extract the element type from a collection type.
///
/// For `Vec[T]`, returns `T`. For references to collections, unwraps the
/// reference first. Falls back to `Ty::Int` for unrecognized types. (The
/// former `*Iter` wrapper branch was removed with the orphaned iterator
/// machinery — Phase B / Milestone 2.)
/// True when `ty` is a callable shape — a bare `Ty::Fn`/`FnMut`/`FnOnce` or
/// the surface `any Fn[…]` / `some Fn[…]` mixin spelling (peeling one
/// reference layer). Used by MIR overload-symbol selection so a closure
/// argument mangles to the closure overload, not a `&str` one (Q1).
pub(super) fn ty_is_callable(ty: &Ty) -> bool {
    let peeled = match ty {
        Ty::Ref(inner)
        | Ty::RefMut(inner)
        | Ty::RefLifetime(_, inner)
        | Ty::RefMutLifetime(_, inner) => inner.as_ref(),
        other => other,
    };
    match peeled {
        Ty::Fn { .. } | Ty::FnMut { .. } | Ty::FnOnce { .. } => true,
        Ty::SomeMixin(bounds) | Ty::AnyMixin(bounds) => bounds
            .iter()
            .any(|b| matches!(b.name.as_str(), "Fn" | "FnMut" | "FnOnce")),
        _ => false,
    }
}

pub(super) fn element_type_of(ty: &Ty) -> Ty {
    match ty {
        Ty::Array(inner) => *inner.clone(),
        Ty::Ref(inner)
        | Ty::RefMut(inner)
        | Ty::RefLifetime(_, inner)
        | Ty::RefMutLifetime(_, inner) => element_type_of(inner),
        // Fall back to I64 (pointer-sized, covers most cases).
        _ => Ty::Int,
    }
}

/// Phase 2 #06.D4: encode a `FormatSpec` into the four i64 arguments
/// accepted by `ruxen_fmt_formatter_new_with_spec`.
///
/// * `width`:     0  = unset
/// * `precision`: -1 = unset
/// * `align`:     0  = default, 1 = left ('<'), 2 = center ('^'), 3 = right ('>')
/// * `fill`:      -1 = unset (runtime treats as ' ')
pub(super) fn encode_format_spec(spec: &crate::lexer::token::FormatSpec) -> (i64, i64, i64, i64) {
    let width = spec.width.map(|w| w as i64).unwrap_or(0);
    let precision = spec.precision.map(|p| p as i64).unwrap_or(-1);
    let align = match spec.align {
        Some('<') => 1,
        Some('^') => 2,
        Some('>') => 3,
        _ => 0,
    };
    let fill = spec.fill.map(|c| c as i64).unwrap_or(-1);
    (width, precision, align, fill)
}

/// Convert an `Option<LocalId>` to a `MirValue`. If None, returns `MirValue::Unit`.
pub(super) fn local_to_value(local: Option<LocalId>) -> MirValue {
    match local {
        Some(id) => MirValue::Use(id),
        None => MirValue::Unit,
    }
}

/// Check if a BinOp is a comparison operator.
pub(super) fn is_comparison(op: BinOp) -> bool {
    matches!(
        op,
        BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::Gt | BinOp::LtEq | BinOp::GtEq
    )
}

/// Convert a comparison BinOp to the corresponding CmpOp.
pub(super) fn binop_to_cmpop(op: BinOp) -> CmpOp {
    match op {
        BinOp::Eq => CmpOp::Eq,
        BinOp::NotEq => CmpOp::NotEq,
        BinOp::Lt => CmpOp::Lt,
        BinOp::Gt => CmpOp::Gt,
        BinOp::LtEq => CmpOp::LtEq,
        BinOp::GtEq => CmpOp::GtEq,
        _ => unreachable!("not a comparison op: {:?}", op),
    }
}
