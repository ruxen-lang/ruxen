//! The `Hash` / `Set` key-kind protocol (B73).
//!
//! How a key hashes and compares is a property of the KEY TYPE. The
//! compiler knows that type statically at every construction site, so it
//! hands the runtime a small descriptor — a kind tag plus, for user
//! types, a hash function and an equality function — and the table keeps
//! it for its whole life.
//!
//! Before B73 the C runtime guessed instead: `ruxen_hash_looks_like_string`
//! dereferenced the first inserted key as a `char*` and scanned for a NUL.
//! That SIGSEGV'd on any `Hash[Int, V]` key >= 0x1000, made user types
//! unusable as keys (identity comparison only), and made a table's
//! behaviour depend on which key happened to be inserted first. The
//! heuristic is gone. Nothing here may reintroduce a memory probe.
//!
//! ## Shape of the mechanism
//!
//! This is deliberately structural, not nominal: the decision is a match
//! on `Ty` variants plus the generic `ty_has_derive_trait` predicate. No
//! stdlib type or method name is hardcoded, per
//! `docs/specs/system/generic-compiler.spec.md`.
//!
//! | key type | kind | hash / eq |
//! |---|---|---|
//! | `String` (and `&String`) | `CSTR` | FNV-1a + `strcmp`, in C |
//! | a struct that has both `Hashable` and `PartialEq` | `CUSTOM` | `<S>_hash_code` / `<S>_eq`, synthesized in `derive.rs` |
//! | everything else | `BITS` | the raw 64-bit word |
//!
//! `BITS` is the correct default rather than a fallback: an `Int`, a
//! `Char`, a `Bool` and a `Symbol` id (B29c) all ARE their word, and a
//! user type with no structural equality hashes by identity, which is
//! also what Ruby's default `Object#hash` does.
//!
//! The kind set is open. Adding one means adding a variant here and a
//! case in `library/std/hash/runtime/hash.c`; it never means widening a
//! range check on a key's value.

use crate::hir::types::Ty;
use crate::mir::nodes::{Literal, LocalId, MirFunction, MirInst, MirValue};
use crate::resolve::symbols::{ty_has_derive_trait, SymbolTable};

/// Wire values. Must stay in step with the `RUXEN_KEY_*` constants in
/// `library/std/core/runtime/runtime.h`.
const KEY_UNSET: i64 = -1;
const KEY_BITS: i64 = 0;
const KEY_CSTR: i64 = 1;
const KEY_CUSTOM: i64 = 2;

/// The runtime constructors that allocate a keyed table, paired with the
/// descriptor-taking form the lowerer rewrites them to.
///
/// Only entry points that allocate a table AND then populate it need to
/// be here. `clone`, `union`, `intersection` and `difference` inherit the
/// descriptor from their source table inside the C runtime, which is
/// strictly better than re-deriving it: it also covers the paths the
/// compiler does not see.
const KEYED_TABLE_CTORS: &[(&str, &str)] = &[
    ("ruxen_hash_new", "ruxen_hash_new_keyed"),
    ("ruxen_hash_with_capacity", "ruxen_hash_with_capacity_keyed"),
    ("ruxen_hash_from_iter", "ruxen_hash_from_iter_keyed"),
    ("ruxen_set_new", "ruxen_set_new_keyed"),
    ("ruxen_set_with_capacity", "ruxen_set_with_capacity_keyed"),
    ("ruxen_set_from_iter", "ruxen_set_from_iter_keyed"),
];

/// The key type a constructed collection hashes on: a `Hash[K, V]` keys
/// on `K`, a `Set[T]` keys on `T` (a set is a table whose values are
/// ignored). Anything else is not a keyed table.
fn key_ty_of(result_ty: &Ty) -> Option<&Ty> {
    match peel(result_ty) {
        Ty::Map(k, _) => Some(k),
        Ty::Set(t) => Some(t),
        _ => None,
    }
}

/// Peel the transparent layers that do not change a value's runtime
/// word: borrows, aliases, newtypes.
fn peel(ty: &Ty) -> &Ty {
    let mut cur = ty;
    loop {
        match cur {
            Ty::Ref(inner)
            | Ty::RefMut(inner)
            | Ty::RefLifetime(_, inner)
            | Ty::RefMutLifetime(_, inner)
            | Ty::Alias { target: inner, .. }
            | Ty::Newtype { inner, .. } => cur = inner,
            other => return other,
        }
    }
}

/// The descriptor for one key type.
enum KeyDesc {
    /// Kind tag only; the runtime needs no callbacks.
    Plain(i64),
    /// `CUSTOM`: the runtime calls these two synthesized MIR functions.
    Custom { hash_sym: String, eq_sym: String },
}

/// Classify a key type. Structural throughout: a match on `Ty` variants
/// plus the generic `ty_has_derive_trait` predicate. No stdlib type or
/// method name is matched against a string.
fn key_descriptor(key_ty: &Ty, symbols: &SymbolTable) -> KeyDesc {
    let peeled = peel(key_ty);
    if matches!(peeled, Ty::String) {
        return KeyDesc::Plain(KEY_CSTR);
    }
    // The key type is still a rigid generic parameter or an unresolved
    // inference variable, so we do not know how this table's keys hash.
    // Say so rather than guess: `UNSET` makes the runtime panic on the
    // first insert, naming this protocol. Guessing `BITS` here would
    // compare string keys by pointer identity and silently lose entries,
    // which is the class of failure B73 exists to remove.
    if matches!(peeled, Ty::TypeParam { .. } | Ty::Infer(_)) {
        return KeyDesc::Plain(KEY_UNSET);
    }
    // A user type keys on its own structural hash and equality only when
    // it has BOTH. `Hashable` without `PartialEq` would hash structurally
    // and then compare by identity, so equal keys collide in a bucket and
    // are still reported absent; `PartialEq` without `Hashable` would
    // compare structurally but scatter equal keys across buckets. Either
    // half alone is worse than plain identity, so require the pair —
    // identity is also what Ruby's default `Object#hash` gives you.
    if let Ty::Struct { name, .. } = peeled {
        let hashable = ty_has_derive_trait(peeled, symbols, "Hashable")
            || ty_has_derive_trait(peeled, symbols, "Hash");
        if hashable && ty_has_derive_trait(peeled, symbols, "PartialEq") {
            // The same mangling `derive.rs` builds its synthesized
            // `synthesize_struct_hash_code` / `synthesize_struct_eq`
            // function names from.
            return KeyDesc::Custom {
                hash_sym: format!("{}_hash_code", name),
                eq_sym: format!("{}_eq", name),
            };
        }
    }
    KeyDesc::Plain(KEY_BITS)
}

/// Install the key-kind descriptor on every table this function
/// constructs.
///
/// A post-pass rather than a per-call-site edit on purpose: `Hash` and
/// `Set` are built from more than two dozen `MirInst::Call` emission
/// sites across `expr/method_call.rs`, `expr/constructors.rs` and
/// `expr/field_access.rs` (paren-less `Hash.new`, `Hash.new()`, a map
/// literal, `collect`, `to_set`, `with_capacity`, …). Deciding once,
/// here, on the shape of the emitted MIR means a new construction site
/// cannot be added without the descriptor — there is nothing to
/// remember.
///
/// The key type comes from the constructed table's own local. Where that
/// local is still generic — a static constructor has no receiver to
/// substitute `-> Hash[K, V]` against — the descriptor is `UNSET` and the
/// runtime panics on the first insert instead of mis-hashing in silence.
/// Typeck's `refine_rigid_generics` is what keeps that from happening on
/// the ordinary binding and assignment paths.
pub(super) fn install_key_descriptors(func: &mut MirFunction, symbols: &SymbolTable) {
    // Descriptors are chosen from local types, which the loop below does
    // not modify, so collect them first and keep the borrows disjoint.
    let local_tys: Vec<Ty> = func.locals.iter().map(|l| l.ty.clone()).collect();
    let mut next_temp_id = func.locals.len();
    let mut new_locals: Vec<(LocalId, Ty)> = Vec::new();

    for block in &mut func.blocks {
        let mut rewritten = Vec::with_capacity(block.instructions.len());
        for inst in block.instructions.drain(..) {
            let MirInst::Call {
                dest: Some(dest),
                ref callee,
                ref args,
            } = inst
            else {
                rewritten.push(inst);
                continue;
            };
            let Some((_, keyed)) = KEYED_TABLE_CTORS.iter().find(|(base, _)| base == callee) else {
                rewritten.push(inst);
                continue;
            };
            let Some(key_ty) = local_tys.get(dest as usize).and_then(key_ty_of) else {
                rewritten.push(inst);
                continue;
            };

            let mut args = args.clone();
            match key_descriptor(key_ty, symbols) {
                KeyDesc::Plain(kind) => {
                    args.push(MirValue::Literal(Literal::Int(kind)));
                    args.push(MirValue::Literal(Literal::Int(0)));
                    args.push(MirValue::Literal(Literal::Int(0)));
                }
                KeyDesc::Custom { hash_sym, eq_sym } => {
                    // The two function slots travel as `Ty::Int` words
                    // holding a `FuncAddr`, the same convention the
                    // closure pointer uses in `ruxen_thread_spawn`.
                    let hash_ptr = next_temp_id as LocalId;
                    next_temp_id += 1;
                    let eq_ptr = next_temp_id as LocalId;
                    next_temp_id += 1;
                    new_locals.push((hash_ptr, Ty::Int));
                    new_locals.push((eq_ptr, Ty::Int));
                    rewritten.push(MirInst::FuncAddr {
                        dest: hash_ptr,
                        func_name: hash_sym,
                    });
                    rewritten.push(MirInst::FuncAddr {
                        dest: eq_ptr,
                        func_name: eq_sym,
                    });
                    args.push(MirValue::Literal(Literal::Int(KEY_CUSTOM)));
                    args.push(MirValue::Use(hash_ptr));
                    args.push(MirValue::Use(eq_ptr));
                }
            }
            rewritten.push(MirInst::Call {
                dest: Some(dest),
                callee: (*keyed).to_string(),
                args,
            });
        }
        block.instructions = rewritten;
    }

    for (id, ty) in new_locals {
        func.locals.push(crate::mir::nodes::MirLocal {
            id,
            name: format!("_keydesc{}", id),
            ty,
            mutable: false,
        });
    }
}
