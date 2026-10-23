//! Instance symbol names: the one mangling every instance, call-site
//! redirect and glue key shares.

use crate::hir::types::Ty;

/// Marker separating a base name from its concrete generic args in an
/// instance symbol. Chosen so it cannot collide with a real source
/// identifier path and is C-symbol-safe (alphanumerics + `_`).
/// Example: `Box[String]` method `eq` → `Box__mono__String_eq`.
const MONO_SEP: &str = "__mono__";

/// First character of every abstract callee (see [`abstract_callee`]).
pub(in crate::mir::lower) const ABSTRACT_CALLEE_PREFIX: char = '?';

/// The callee MIR emits for a call that has no concrete target because a
/// type it depends on is still a generic parameter (`?T_to_display`). An
/// unresolved inference variable spells the same way through `Ty::Infer`'s
/// Display (`?T3200_to_display`). Only an opaque generic body emits one, and
/// `Lowerer::prune_unmonomorphised_bodies` guarantees none reaches codegen.
pub(in crate::mir::lower) fn abstract_callee(param: &str, method: &str) -> String {
    format!("{ABSTRACT_CALLEE_PREFIX}{param}_{method}")
}

/// The C-safe instance base for `(name, generic_args)`, e.g.
/// `Box__mono__String`, `Pair__mono__Int__String`. Method callees are then
/// `format!("{base}_{method}")`.
pub(in crate::mir::lower) fn mono_base(name: &str, args: &[Ty]) -> String {
    let mut s = String::with_capacity(name.len() + args.len() * 8);
    s.push_str(&name.replace('.', "_"));
    s.push_str(MONO_SEP);
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            s.push_str("__");
        }
        s.push_str(&symbol_token(&a.peel_refs().to_string()));
    }
    s
}

/// `text` as a C symbol fragment, injectively: ASCII letters and digits stay,
/// every other byte (`_` included) becomes `_` plus two hex digits. A token
/// therefore never contains `__`, the separator between generic args, and
/// two different texts never give the same token.
pub(in crate::mir::lower) fn symbol_token(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() {
            out.push(byte as char);
        } else {
            out.push_str(&format!("_{byte:02x}"));
        }
    }
    out
}

/// If `mangled` is an instance name (`Base__mono__Args_method`), return the
/// base name (`Base`, with `_` for the dotted form). Used by
/// `class_name_from_mangled` so self-type recovery in `lower_method`
/// resolves the receiver class for a specialized body.
pub(in crate::mir::lower) fn strip_mono_suffix(mangled: &str) -> Option<&str> {
    let idx = mangled.find(MONO_SEP)?;
    Some(&mangled[..idx])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nominal_and_structural_spellings_key_identically() {
        // `self` inside `class Array[T]` types nominally; a call site's
        // receiver types structurally. Both must produce the same mangled
        // base or a self-call inside a specialized body would miss it.
        let structural = Ty::Array(Box::new(Ty::String));
        let nominal = Ty::Class {
            name: "Array".to_string(),
            generic_args: vec![Ty::String],
        };
        let (sc, sa) = Ty::builtin_head(&structural).expect("structural head");
        let (nc, na) = Ty::builtin_head(&nominal).expect("nominal head");
        assert_eq!(mono_base(sc, &sa), mono_base(nc, &na));
    }

    #[test]
    fn nested_generic_args_give_distinct_bases() {
        let pair = |a: Ty, b: Ty| Ty::Class {
            name: "Pair".to_string(),
            generic_args: vec![a, b],
        };
        assert_ne!(
            mono_base("Box", &[pair(Ty::Int, Ty::String)]),
            mono_base("Box", &[pair(Ty::String, Ty::Int)])
        );
    }
}
