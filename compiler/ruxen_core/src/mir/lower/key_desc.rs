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
use crate::implicit_includes::HASHABLE;
use crate::mir::lower::type_helpers::derived_nominal;
use crate::mir::nodes::{Literal, LocalId, MirFunction, MirInst, MirValue};
use crate::resolve::symbols::SymbolTable;

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

/// Key-addressed operations, paired with the `ensure` entry point for the
/// family they belong to. Every one of these hashes or compares a key, so
/// the table it runs against must already know how. Their RECEIVER is
/// argument 0, and unlike a constructor's result temp it is a real,
/// concretely-typed local at nearly every call site.
///
/// This is the second half of the protocol and it is what makes the first
/// half's failure recoverable rather than fatal. A construction site
/// cannot always be typed — `f(Hash.new)` types the temp from the
/// declared `-> Hash[K, V]`, since a static constructor has no receiver
/// to substitute against — but the USE site inside `f` knows `K`
/// perfectly well. Installing there is still the static type deciding;
/// nothing inspects a key.
///
/// The list covers every key-addressed operation, not just the two
/// `insert`s, and that redundancy is deliberate. You can argue the reads
/// down: only an insert populates a table, an insert installs the kind,
/// so a table with entries always has one, and a lookup on an empty table
/// finds nothing under any kind. That argument is correct today and it is
/// a WHOLE-PROGRAM argument — it breaks the moment some other path learns
/// to add entries. Paying one no-op call per lookup keeps the invariant
/// local and checkable at each site. If a profile ever shows it matters,
/// narrowing to the inserts is the safe cut, but make that a measured
/// decision and leave this reasoning with it.
const KEYED_TABLE_OPS: &[(&str, &str)] = &[
    ("ruxen_hash_insert", HASH_ENSURE),
    ("ruxen_hash_get", HASH_ENSURE),
    ("ruxen_hash_contains_key", HASH_ENSURE),
    ("ruxen_hash_remove", HASH_ENSURE),
    ("ruxen_hash_index", HASH_ENSURE),
    ("ruxen_set_insert", SET_ENSURE),
    ("ruxen_set_contains", SET_ENSURE),
    ("ruxen_set_remove", SET_ENSURE),
];

const HASH_ENSURE: &str = "ruxen_hash_ensure_key_kind";
const SET_ENSURE: &str = "ruxen_set_ensure_key_kind";

/// `Vec` helpers that build a fresh OWNED array by copying slots out of a
/// borrowed source, paired with the element-descriptor-taking form the
/// lowerer rewrites them to.
///
/// A `Hash`/`Set` derive is absent from this table and that is not an
/// oversight: those carry `key_kind` on the table itself, so `union`,
/// `intersection`, `difference` and `from_iter` read the descriptor from
/// their own source inside the C runtime. A `RuxenVec` has no such field,
/// so its element kind has to be injected here, at the one place that
/// still knows the static element type.
///
/// Membership is decided by the ownership obligation, not by which
/// helpers happened to crash: any helper whose result is an owned
/// `Array[T]` populated from a source it does not consume belongs here.
/// `ruxen_vec_from_iter` does not, because it CONSUMES its source (it is
/// an identity passthrough listed in `CONSUME_HELPERS`), and
/// `ruxen_vec_take`/`ruxen_vec_clone` do not because they are not Fresh
/// — see the note on `FRESH_ALLOC_CALLEES` about why that is a leak
/// rather than a safe default.
const ELEM_DERIVE_HELPERS: &[(&str, &str)] = &[
    ("ruxen_vec_skip", "ruxen_vec_skip_elem"),
    ("ruxen_vec_chain", "ruxen_vec_chain_elem"),
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

/// The key type of a table built by collecting an iterable, read off the
/// SOURCE rather than the result.
///
/// A `from_iter` constructor populates its table inside the C runtime,
/// where no `ensure` at a later operation site can rescue it, so its
/// descriptor has to be right at construction. The result type is the
/// weaker witness of the two: `Array#to_h` is declared `-> Int` (a
/// laundered residual decl — its true key type is not expressible on that
/// `lib` line today), which says nothing at all. The source iterable is a
/// concrete `Array[T]`, and collecting it is exactly what fixes the key
/// type: a `Set` collects its elements, so `T` IS the key; a `Hash`
/// collects `(K, V)` pairs, so the key is the pair's first field.
fn key_ty_from_iterable(iter_ty: &Ty, collects_pairs: bool) -> Option<&Ty> {
    let Ty::Array(elem) = peel(iter_ty) else {
        return None;
    };
    if !collects_pairs {
        return Some(elem);
    }
    match peel(elem) {
        Ty::Tuple(fields) if fields.len() == 2 => Some(&fields[0]),
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
    //
    // A dotted name means a module-nested type, whose synthesized
    // functions `derive.rs` mangles from the BARE `s.name` while a use
    // site's `Ty` may carry the qualified one. Rather than emit a
    // `FuncAddr` to a symbol that might not exist and fail at link, fall
    // through to identity — the same semantics such a key already had.
    // For the same reason `mir/lower/mod.rs` only synthesises a class's
    // derives at module_path root, so a nested class also lands here.
    //
    // B74: classes join structs here. `{Class}_hash_code` /
    // `{Class}_eq` are synthesised from the identical mangling, so a
    // class carrying Hashable + PartialEq keys a Hash/Set exactly as a
    // struct does; before, it fell through to identity and two equal
    // keys were distinct entries.
    //
    // The predicate is `derived_nominal`, not the raw
    // `ty_has_derive_trait`, so this asks whether the two functions were
    // actually EMITTED rather than whether the type nominally qualifies.
    // A fieldless class satisfies the structural rule vacuously but has
    // no synthesised `_hash_code` / `_eq`, so pointing the runtime at
    // those symbols would fail at link.
    if let Ty::Struct { name, .. } | Ty::Class { name, .. } = peeled {
        let hashable = !name.contains('.') && derived_nominal(peeled, symbols, HASHABLE).is_some();
        if hashable && derived_nominal(peeled, symbols, "PartialEq").is_some() {
            // The same mangling `derive.rs` builds its synthesized
            // `build_hash_code` / `build_eq` function names from.
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
/// Two routes, because one is not enough on its own:
///
/// * **Construction sites** get the descriptor baked into the call. This
///   is the only route that can serve a table the C runtime populates
///   internally (`from_iter`), and the only one that reaches a table
///   nothing ever looks up.
/// * **Operation sites** get an idempotent `ensure` in front, chosen from
///   the RECEIVER's key type. This catches every table whose construction
///   site could not be typed — `f(Hash.new)` types its temp from the
///   declared `-> Hash[K, V]`, since a static constructor has no receiver
///   to substitute against — and it generalizes over syntactic position
///   (argument, field init, return, element) instead of needing one
///   typeck refinement per position.
///
/// A table that neither route can type keeps `RUXEN_KEY_UNSET`, and the
/// runtime panics on the first insert naming the protocol rather than
/// guessing a kind and mis-hashing in silence.
pub(super) fn install_key_descriptors(func: &mut MirFunction, symbols: &SymbolTable) {
    // Descriptors are chosen from local types, which the loop below does
    // not modify, so collect them first and keep the borrows disjoint.
    let local_tys: Vec<Ty> = func.locals.iter().map(|l| l.ty.clone()).collect();
    let mut next_temp_id = func.locals.len();
    let mut new_locals: Vec<(LocalId, Ty)> = Vec::new();
    let mut glue_descs: Vec<super::drops::ElemDesc> = Vec::new();

    for block in &mut func.blocks {
        let mut rewritten = Vec::with_capacity(block.instructions.len());
        for inst in block.instructions.drain(..) {
            let MirInst::Call {
                ref dest,
                ref callee,
                ref args,
            } = inst
            else {
                rewritten.push(inst);
                continue;
            };

            // ── Route 1: a construction site becomes its `_keyed` form.
            if let Some(keyed) = keyed_ctor_for(callee) {
                let Some(desc) = ctor_descriptor(callee, dest, args, &local_tys, symbols) else {
                    rewritten.push(inst);
                    continue;
                };
                let mut args = args.clone();
                args.extend(materialize(
                    desc,
                    &mut rewritten,
                    &mut next_temp_id,
                    &mut new_locals,
                ));
                if let Some(value_desc) =
                    pair_value_desc(callee, dest, args.first(), &local_tys, symbols)
                {
                    glue_descs.push(value_desc.clone());
                    let mut new_temp = || {
                        let id = next_temp_id as LocalId;
                        next_temp_id += 1;
                        new_locals.push((id, Ty::Int));
                        id
                    };
                    args.push(super::drops::elem_desc_value(
                        value_desc,
                        &mut new_temp,
                        &mut rewritten,
                    ));
                }
                rewritten.push(MirInst::Call {
                    dest: *dest,
                    callee: keyed.to_string(),
                    args,
                });
                continue;
            }

            // ── Route 3: a derived-array helper gets its element kind
            //    appended, so the copy it makes is as deep as the drop
            //    the result will receive.
            //
            //    The witness is the RESULT's type, and ONLY the result's.
            //    The result is the local that will be handed to
            //    `heap_free_call`, so its element type is the one that
            //    decides what must be owned.
            //
            //    Deliberately NOT falling back to argument 0 when the
            //    result cannot be typed. The source's element type is the
            //    wrong witness for this question, and using it inverts the
            //    bug instead of fixing it: an untypeable result gets no
            //    deep drop from `heap_free_call` either, so cloning for
            //    it would allocate elements nothing ever frees. POD (alias)
            //    is the correct answer for an unknown result — it is what
            //    keeps the copy exactly as deep as the drop.
            if let Some(with_desc) = elem_derive_for(callee) {
                let desc = dest
                    .and_then(|d| local_tys.get(d as usize))
                    .map_or(super::drops::ElemDesc::Word(0), |ty| {
                        elem_kind_of(ty, symbols)
                    });
                glue_descs.push(desc.clone());
                let mut args = args.clone();
                let mut new_temp = || {
                    let id = next_temp_id as LocalId;
                    next_temp_id += 1;
                    new_locals.push((id, Ty::Int));
                    id
                };
                args.push(super::drops::elem_desc_value(
                    desc,
                    &mut new_temp,
                    &mut rewritten,
                ));
                rewritten.push(MirInst::Call {
                    dest: *dest,
                    callee: with_desc.to_string(),
                    args,
                });
                continue;
            }

            // ── Route 2: a key-addressed operation gets an `ensure` in
            //    front, unless we cannot type its receiver either — an
            //    `ensure` carrying UNSET would be a no-op call.
            if let Some(ensure) = keyed_op_ensure(callee) {
                let recv_key_ty = args
                    .first()
                    .and_then(as_local)
                    .and_then(|l| local_tys.get(l as usize))
                    .and_then(key_ty_of);
                if let Some(key_ty) = recv_key_ty {
                    let desc = key_descriptor(key_ty, symbols);
                    if !matches!(desc, KeyDesc::Plain(KEY_UNSET)) {
                        let mut ensure_args = vec![args[0].clone()];
                        ensure_args.extend(materialize(
                            desc,
                            &mut rewritten,
                            &mut next_temp_id,
                            &mut new_locals,
                        ));
                        rewritten.push(MirInst::Call {
                            dest: None,
                            callee: ensure.to_string(),
                            args: ensure_args,
                        });
                    }
                }
            }
            rewritten.push(inst);
        }
        block.instructions = rewritten;
    }
    for desc in &glue_descs {
        super::drops::record_elem_glue(func, desc);
    }

    for (id, ty) in new_locals {
        func.locals.push(crate::mir::nodes::MirLocal {
            id,
            name: format!("_keydesc{}", id),
            ty,
            mutable: false,
            kind: crate::mir::nodes::LocalKind::Temp,
        });
    }
}

/// The runtime symbol a descriptor-taking form was rewritten FROM
/// (`ruxen_hash_new_keyed` -> `ruxen_hash_new`), or `callee` itself. The
/// rewritten form allocates and transfers exactly what its base does, so
/// `runtime_abi` classifies it under the base's one row instead of
/// listing every spelling twice.
pub(super) fn base_symbol(callee: &str) -> &str {
    KEYED_TABLE_CTORS
        .iter()
        .chain(ELEM_DERIVE_HELPERS)
        .find(|(_, rewritten)| *rewritten == callee)
        .map_or(callee, |(base, _)| base)
}

fn elem_derive_for(callee: &str) -> Option<&'static str> {
    ELEM_DERIVE_HELPERS
        .iter()
        .find(|(base, _)| *base == callee)
        .map(|(_, with_desc)| *with_desc)
}

/// How deep a copy the elements of `array_ty` need, expressed as a
/// `RUXEN_ELEM_*` descriptor; POD (0) when `array_ty` is not an `Array`.
///
/// Built by the drop pass's own `elem_desc`, on purpose. The invariant
/// that matters is copy-depth == drop-depth: whatever the result's drop
/// glue will free out of its slots is exactly what the derive has to have
/// allocated, no more and no less. Both read the one descriptor builder,
/// so they cannot drift apart.
fn elem_kind_of(array_ty: &Ty, symbols: &SymbolTable) -> super::drops::ElemDesc {
    match peel(array_ty) {
        Ty::Array(elem) => super::drops::elem_desc(elem, symbols),
        _ => super::drops::ElemDesc::Word(0),
    }
}

fn keyed_ctor_for(callee: &str) -> Option<&'static str> {
    KEYED_TABLE_CTORS
        .iter()
        .find(|(base, _)| *base == callee)
        .map(|(_, keyed)| *keyed)
}

fn keyed_op_ensure(callee: &str) -> Option<&'static str> {
    KEYED_TABLE_OPS
        .iter()
        .find(|(op, _)| *op == callee)
        .map(|(_, ensure)| *ensure)
}

/// The local behind an argument, when there is one. Argument 0 of these
/// calls is a table receiver or a source iterable, and both arrive as a
/// plain local use — including a borrowed receiver (`&Hash[K, V]` param),
/// since a borrow does not change the runtime word.
fn as_local(arg: &MirValue) -> Option<LocalId> {
    match arg {
        MirValue::Use(l) => Some(*l),
        _ => None,
    }
}

/// The descriptor for a construction site, chosen from the best-informed
/// witness available.
///
/// `None` means this is not a keyed table at all (no witness names a
/// `Hash` or `Set`), and the caller leaves the call alone. `Some(UNSET)`
/// means it IS one and we could not type it — that is a descriptor, and
/// the runtime turns it into a panic on the first insert.
///
/// Two witnesses, in priority of INFORMATION rather than fixed order:
///
/// * The result local. Right for `new` / `with_capacity`.
/// * The source iterable, for the two `from_iter` collectors, whose
///   result type is the laundered one (`Array#to_h` is declared `-> Int`,
///   which names no key) while their input is always a concrete `Array`.
///
/// Whichever yields a real kind wins. Ordering them by information rather
/// than by position matters in both directions: inside a generic body the
/// SOURCE is the abstract one (`Array[T]`) while a concrete annotation may
/// have pinned the result, and for `to_h` it is the other way round.
/// Taking a fixed side would throw away the witness that actually knew.
fn ctor_descriptor(
    callee: &str,
    dest: &Option<LocalId>,
    args: &[MirValue],
    local_tys: &[Ty],
    symbols: &SymbolTable,
) -> Option<KeyDesc> {
    let from_result = dest
        .and_then(|d| local_tys.get(d as usize))
        .and_then(key_ty_of);
    let from_source = match callee {
        "ruxen_hash_from_iter" => Some(true),
        "ruxen_set_from_iter" => Some(false),
        _ => None,
    }
    .and_then(|collects_pairs| {
        args.first()
            .and_then(as_local)
            .and_then(|l| local_tys.get(l as usize))
            .and_then(|t| key_ty_from_iterable(t, collects_pairs))
    });

    let mut fallback = None;
    for key_ty in [from_source, from_result].into_iter().flatten() {
        match key_descriptor(key_ty, symbols) {
            KeyDesc::Plain(KEY_UNSET) => fallback = Some(KeyDesc::Plain(KEY_UNSET)),
            desc => return Some(desc),
        }
    }
    fallback
}

/// The value descriptor a pair collector (`ruxen_hash_from_iter`) copies
/// each value with: the element descriptor of the result's value type, the
/// one its drop frees values with (`drops::heap_free_call`), else of the
/// source pairs' second element (the result is laundered, `Array#to_h` is
/// declared `-> Int`). POD when neither is typed, which then also drops
/// nothing. `None` for every other constructor.
fn pair_value_desc(
    callee: &str,
    dest: &Option<LocalId>,
    source: Option<&MirValue>,
    local_tys: &[Ty],
    symbols: &SymbolTable,
) -> Option<super::drops::ElemDesc> {
    if callee != "ruxen_hash_from_iter" {
        return None;
    }
    let from_result = dest.and_then(|d| match local_tys.get(d as usize).map(peel) {
        Some(Ty::Map(_, value)) => Some((**value).clone()),
        _ => None,
    });
    let from_source =
        source
            .and_then(as_local)
            .and_then(|l| match local_tys.get(l as usize).map(peel) {
                Some(Ty::Array(elem)) => match peel(elem) {
                    Ty::Tuple(pair) if pair.len() == 2 => Some(pair[1].clone()),
                    _ => None,
                },
                _ => None,
            });
    let value = from_result.or(from_source);
    Some(value.map_or(super::drops::ElemDesc::Word(0), |v| {
        super::drops::elem_desc(&v, symbols)
    }))
}

/// Turn a descriptor into the three trailing `(kind, hash_fn, eq_fn)`
/// argument words, emitting the `FuncAddr` instructions a CUSTOM kind
/// needs into `out` first.
fn materialize(
    desc: KeyDesc,
    out: &mut Vec<MirInst>,
    next_temp_id: &mut usize,
    new_locals: &mut Vec<(LocalId, Ty)>,
) -> Vec<MirValue> {
    match desc {
        KeyDesc::Plain(kind) => vec![
            MirValue::Literal(Literal::Int(kind)),
            MirValue::Literal(Literal::Int(0)),
            MirValue::Literal(Literal::Int(0)),
        ],
        KeyDesc::Custom { hash_sym, eq_sym } => {
            // The two function slots travel as `Ty::Int` words holding a
            // `FuncAddr`, the same convention the closure pointer uses in
            // `ruxen_thread_spawn`.
            let hash_ptr = *next_temp_id as LocalId;
            *next_temp_id += 1;
            let eq_ptr = *next_temp_id as LocalId;
            *next_temp_id += 1;
            new_locals.push((hash_ptr, Ty::Int));
            new_locals.push((eq_ptr, Ty::Int));
            out.push(MirInst::FuncAddr {
                dest: hash_ptr,
                func_name: hash_sym,
            });
            out.push(MirInst::FuncAddr {
                dest: eq_ptr,
                func_name: eq_sym,
            });
            vec![
                MirValue::Literal(Literal::Int(KEY_CUSTOM)),
                MirValue::Use(hash_ptr),
                MirValue::Use(eq_ptr),
            ]
        }
    }
}
