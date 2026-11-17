use super::*;

pub(super) fn is_option_type(ty: &Ty) -> bool {
    matches!(ty.builtin_head(), Some(("Option", _)))
}

/// Check if a type is an `Array`, the only receiver whose block methods
/// MIR inlines; any other receiver calls its own method body.
pub(super) fn is_vec_or_iterator_type(ty: &Ty) -> bool {
    // Either spelling of `Array`: the `self` receiver INSIDE an `Array[T]`
    // stdlib method body is typed `Ty::Class { name: "Array" }`, not
    // `Ty::Array`. The closure combinators migrated to `array/src/lib.rx`
    // invoke `self.each { … }`; that inline path must recognise this receiver
    // as a backing Vec (it shares the `RuxenVec*` repr).
    matches!(ty.builtin_head(), Some(("Array", _)))
}

/// Extract the element type from a collection type.
///
/// For `Vec[T]`, returns `T`. For references to collections, unwraps the
/// reference first. Falls back to `Ty::Int` for unrecognized types. (The
/// former `*Iter` wrapper branch was removed with the orphaned iterator
/// machinery — Phase B / Milestone 2.)
pub(super) fn element_type_of(ty: &Ty) -> Ty {
    match ty.peel_refs() {
        Ty::Array(inner) => *inner.clone(),
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
