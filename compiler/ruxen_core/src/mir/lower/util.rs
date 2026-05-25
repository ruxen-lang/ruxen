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
            matches!(
                base,
                "Vec" | "VecIter" | "VecIntoIter" | "SplitIter" | "HashIter" | "SetIter"
            )
        }
        // For inferred types, check if the type name suggests a collection.
        Ty::Infer(_) => false,
        _ => false,
    }
}

/// Check if a method on a built-in type is a static/class method
/// (no `self` argument). These are methods like `String.from(...)`,
/// `Vec.new()`, etc. that are called on the type itself.
pub(super) fn is_builtin_static_method(type_name: &str, method_name: &str) -> bool {
    // Handle both exact matches and generic type names (e.g., "Vec[T]").
    let base_type = if let Some(pos) = type_name.find('[') {
        &type_name[..pos]
    } else {
        type_name
    };
    match base_type {
        "String" => matches!(
            method_name,
            "from" | "new" | "with_capacity" | "from_iter" | "from_bytes"
        ),
        // `Vec.with_capacity(n)` is a stateless static constructor — like
        // `Vec.new` but takes one Int arg. Phase 2 stdlib batch 1 (#03).
        // `Vec.from_iter(iter)` (#03 batch 2) takes any iterator-producing
        // expression and treats it as a fresh allocation.
        // ruby-naming.spec.md §10a: `Vec[T]` → `Array[T]`. Both names
        // route here while the migration shim is in place.
        "Vec" | "Array" => matches!(method_name, "new" | "with_capacity" | "from_iter"),
        // Phase 2 stdlib (#04): full HashMap[K,V] / HashSet[T] surface.
        // The `Hash`, `HashMap`, and `Map` (ruby-naming.spec.md §10a)
        // aliases all reach here for the `Map.new` /
        // `Map.with_capacity(n)` / `Map.from_iter(iter)` constructors;
        // same for `Set` / `HashSet`. Without `Map` in the alias list
        // the static-dispatch detector at the method call site
        // (`is_builtin_static_method`) classifies the call as an
        // instance method and prepends a phantom `Unit` self arg,
        // producing a 2-arg call against the 1-arg `ruxen_hash_from_iter`
        // runtime symbol — the Cranelift verifier rejects with
        // "mismatched argument count".
        "Hash" | "HashMap" | "Map" => {
            matches!(method_name, "new" | "with_capacity" | "from_iter")
        }
        "Set" | "HashSet" => matches!(method_name, "new" | "with_capacity" | "from_iter"),
        "Thread" => matches!(method_name, "spawn" | "current" | "sleep" | "yield_now"),
        "Mutex" => matches!(method_name, "new"),
        "Arc" | "SharedSync" => matches!(method_name, "new"),
        // Phase 2 stdlib (#06.5 T4): Duration / Instant static-style
        // constructors. `Duration.from_secs(5)` / `Instant.now()` must
        // classify as static here so the method-call lowerer doesn't
        // synthesise a phantom `self` arg ahead of the runtime symbol
        // — `ruxen_duration_from_secs` takes one i64, `ruxen_instant_now`
        // takes none.
        "Duration" => matches!(
            method_name,
            "from_secs" | "from_millis" | "from_micros" | "from_nanos"
        ),
        "Instant" => matches!(method_name, "now"),
        // Phase 2 #06.5 T5: TcpListener / TcpStream class-static
        // constructors. `TcpListener.bind(&addr)` /
        // `TcpStream.connect(&addr)` dispatch directly to their
        // runtime symbol with no synthetic `self`. The runtime entries
        // (`ruxen_tcp_listener_bind`, `ruxen_tcp_stream_connect`) take
        // one `const char*`, not `self + char*`.
        "TcpListener" => matches!(method_name, "bind"),
        "TcpStream" => matches!(method_name, "connect"),
        _ => false,
    }
}

/// Extract the element type from a collection or iterator type.
///
/// For `Vec[T]`, returns `T`. For iterator wrappers like `VecIter[T]`,
/// `VecIntoIter[T]`, returns `T`. For references to collections, unwraps
/// the reference first. Falls back to `Ty::Int` for unrecognized types.
pub(super) fn element_type_of(ty: &Ty) -> Ty {
    match ty {
        Ty::Array(inner) => *inner.clone(),
        Ty::Ref(inner)
        | Ty::RefMut(inner)
        | Ty::RefLifetime(_, inner)
        | Ty::RefMutLifetime(_, inner) => element_type_of(inner),
        Ty::Class { name, generic_args } => {
            // Iterator wrapper types: VecIter[T], VecIntoIter[T], etc.
            if (name == "VecIter" || name == "VecIntoIter" || name == "SplitIter")
                && !generic_args.is_empty()
            {
                return generic_args[0].clone();
            }
            // Fall back to I64 (pointer-sized, covers most cases).
            Ty::Int
        }
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
