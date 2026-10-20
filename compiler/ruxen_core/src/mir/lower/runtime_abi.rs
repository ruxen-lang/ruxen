//! The single source of truth for runtime-callee ownership in MIR lowering.
//!
//! Every runtime symbol the drop-elaboration pass (`drops/`) or the
//! static-constructor fast path (`expr/method_call.rs`) special-cases is
//! classified HERE, exactly once, by `callee_ownership`. This replaces the six
//! overlapping, partly-contradicting predicates that previously lived inline in
//! the `drops/` `Call` arm (FRESH_ALLOC_CALLEES, is_runtime_consume_helper,
//! is_runtime_borrow_helper ×3 rebinds, is_move_by_ffi_callee,
//! is_pointer_store_helper, borrows_first_arg) plus the duplicated static-ctor
//! lists in method_call.rs / util.rs.
//!
//! ABI contract: the C side's ownership semantics are documented per-symbol in
//! the stdlib specs cited inline (task_spawn.spec.md §B10, the Command builder
//! notes, mutex.c, scheduler.c:150-152). A WRONG entry here is a double-free or
//! use-after-free — every change must move a row, with a visible diff to the
//! parity oracle (tests/runtime_abi_parity.rs).

/// What the callee does with the value it RETURNS into `dest`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultOwnership {
    /// Returns a fresh heap allocation owned exclusively by `dest`
    /// (drop-elaborate `dest` at scope exit). Old: FRESH_ALLOC_CALLEES.
    Fresh,
    /// No owning result to track (Unit/Int return, or a free helper).
    /// `dest` is conservatively tainted (dropped from alloc_rooted).
    None,
}

/// The full ownership verdict for one callee. Mirrors the decisions the
/// `drops/` arg loop makes, in the SAME precedence order it applied them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CalleeOwnership {
    /// Result-into-`dest` ownership.
    pub result: ResultOwnership,
    /// arg0 is borrowed even though the callee otherwise transfers (the
    /// `borrows_first_arg` rule: `_init` ctors + the free/drop family). The
    /// arg loop `continue`s on arg0 when this is set. HIGHEST arg precedence.
    pub borrows_first_arg: bool,
    /// All remaining pointer args are BORROWED (not transferred) — the runtime
    /// helper reads/mutates in place. Old: effective is_runtime_borrow_helper.
    /// When false (user callees, move-by-FFI), every remaining Use(arg) is
    /// tainted. LOWEST arg precedence.
    pub args_are_borrowed: bool,
}

impl Default for CalleeOwnership {
    /// The conservative default for an unknown / user-defined callee: no owning
    /// result to track, every arg transferred (tainted). This is the ONE
    /// fallthrough and is intentionally pessimistic (over-tainting a primitive
    /// temp is a no-op; under-tainting is a UAF).
    fn default() -> Self {
        CalleeOwnership {
            result: ResultOwnership::None,
            borrows_first_arg: false,
            args_are_borrowed: false,
        }
    }
}

/// The C functions whose `.rx` declaration marks the result owned
/// (`def clone as "ruxen_closure_share" -> owned Self`,
/// `ast::FfiFunction::returns_owned`), by C symbol and by Ruxen-side name.
/// Built once per program from `MirProgram::ffi_libs`; read by
/// [`result_ownership`] before the hand-listed table, so a helper that
/// declares its result needs no row (F-25).
#[derive(Debug, Clone, Default)]
pub struct DeclaredResults(std::collections::HashSet<String>);

impl DeclaredResults {
    /// The owned-result declarations of every `lib` block in `libs`.
    pub fn from_libs(libs: &[crate::mir::nodes::FfiLib]) -> Self {
        let mut owned = std::collections::HashSet::new();
        for decl in libs.iter().flat_map(|lib| &lib.functions) {
            if decl.returns_owned {
                owned.insert(decl.name.clone());
                owned.insert(decl.ruxen_name.clone());
            }
        }
        DeclaredResults(owned)
    }
}

/// What `callee` does with its result: a declaration's `-> owned T` marker
/// first, then the hand-listed table ([`callee_ownership`]). The one reader
/// of result ownership; a descriptor-taking rewrite answers as its base
/// symbol.
pub fn result_ownership(callee: &str, declared: &DeclaredResults) -> ResultOwnership {
    let base = super::key_desc::base_symbol(callee);
    if declared.0.contains(callee) || declared.0.contains(base) {
        return ResultOwnership::Fresh;
    }
    callee_ownership(callee).result
}

/// THE single ownership lookup. Ordered exactly like the old `Call` arm so the
/// parity oracle (tests/runtime_abi_parity.rs) reproduces 1:1.
pub fn callee_ownership(callee: &str) -> CalleeOwnership {
    // A descriptor-taking rewrite (`ruxen_hash_new_keyed`, `ruxen_vec_skip_elem`)
    // owns and transfers exactly what its base symbol does.
    let callee = super::key_desc::base_symbol(callee);

    // --- result ownership (old FRESH_ALLOC_CALLEES) ---
    let result = if listed(FRESH_ALLOC_CALLEES, callee) {
        ResultOwnership::Fresh
    } else {
        ResultOwnership::None
    };

    // --- arg0 borrow (old borrows_first_arg) ---
    let borrows_first_arg = callee.ends_with("_init") || FREE_FAMILY.contains(&callee);

    // --- remaining-arg borrow (old effective is_runtime_borrow_helper) ---
    let consumes = FREE_FAMILY.contains(&callee) || listed(CONSUME_HELPERS, callee);
    let move_by_ffi = listed(MOVE_BY_FFI, callee);
    let builder = listed(COMMAND_BUILDER, callee);
    let prefix_borrow = has_runtime_prefix(callee);
    let terminal = listed(COMMAND_TERMINAL_OR_ACCESSOR, callee);
    let args_are_borrowed = ((!consumes && !move_by_ffi && prefix_borrow) || terminal) && !builder;

    CalleeOwnership {
        result,
        borrows_first_arg,
        args_are_borrowed,
    }
}

/// For a USER callee (a function/method NOT classified by the runtime-ABI
/// tables above), resolve which of its first `arg_count` *value-carrying*
/// parameters are declared as a reference (`&T` / `&var T`) or are the
/// synthetic `__block`/`&block` call-duration borrow.
///
/// A value passed to a reference parameter is AUTO-BORROWED, not consumed: the
/// callee never owns it (drop elaboration unconditionally skips a function's
/// own params — the caller owns the argument allocation for the call's
/// duration). So the drop pass must NOT taint such an argument's source local;
/// the source stays drop-eligible and is freed exactly once at the CALLER's
/// scope exit. Without this, a `String` passed to `def f(s: &String)` was
/// tainted by the conservative user-callee default and LEAKED (TASKS.md
/// "Discovered drop-elaboration gap").
///
/// Resolution is by callee NAME (not DefId), mirroring the borrow_check
/// precedent (`checks.rs::lookup_param_is_ref`, fix `fbe65da`): the HIR call's
/// method DefId can be `UNRESOLVED_DEF` for `lib`-declared / builtin methods,
/// so a by-DefId lookup would miss. The mangled MIR callee is reduced to its
/// resolver name:
///   * free fn  → the whole callee string (overload/mono suffixes stripped),
///   * method   → the bare method name via `mir::lang_intrinsics::extract_method_name`.
///
/// SAFETY / no-double-free guard: this is consulted ONLY for callees the
/// runtime-ABI tables did not classify, and ONLY to UNTAINT a borrow param.
/// A by-value (consuming) param keeps its taint (returned `false`), except for
/// the explicit block slot whose pair remains caller-owned. When the
/// name resolves to MULTIPLE candidate signatures (same method name across
/// classes / an overload set) that DISAGREE on a slot's ref-ness, we
/// conservatively report `false` for that slot — never untaint on ambiguity.
/// An unresolved name reports all-`false` (unchanged, fully conservative).
///
/// `self` offset: an instance method's MIR call passes the receiver as arg0,
/// but the resolver signature's `params` do NOT include `self` (it lives in
/// `self_mode`). We therefore align the signature against args[1..] when the
/// signature is a `self`-taking method, and against args[0..] for a free fn /
/// static method.
///
/// Receiver (arg0 of a method): reported a borrow ONLY when EVERY candidate's
/// `self_mode` is the immutable `Ref` (`&self`). A read-only method borrows its
/// receiver — the caller owns the instance and must free it at scope exit;
/// tainting it (the old default) leaked the receiver. `RefMut` (`var self`) and
/// `Consuming` (`consume self`) keep the receiver TAINTED: a `var self` method
/// can be an in-place builder whose returns-self rebind is governed by
/// `elide_returns_self_realloc`, and a consuming receiver is genuinely moved —
/// untainting either risks a double-free. This is the provably-safe subset.
///
/// Block slots (`Fn[...]`, `Ty::is_borrowed_block`) are always borrows. They
/// come from `lowered_blocks`, the callee's own lowered parameters, when the
/// caller has its body (`drops::CallSummaries::block_params`: this also
/// covers callees name resolution cannot find, such as a mixin default); a
/// caller that runs before the callee is lowered passes `None` and the
/// declared signature decides. This is the one place either source is read.
pub fn user_callee_param_is_ref(
    symbols: &crate::resolve::symbols::SymbolTable,
    mangled_callee: &str,
    arg_count: usize,
    lowered_blocks: Option<&[bool]>,
) -> Vec<bool> {
    let mut result = vec![false; arg_count];
    if arg_count == 0 {
        return result;
    }
    let lowered_block = |slot: usize| {
        lowered_blocks
            .and_then(|blocks| blocks.get(slot))
            .copied()
            .unwrap_or(false)
    };

    let candidates = user_callee_candidates(symbols, mangled_callee);
    if candidates.is_empty() {
        for (slot, slot_is_ref) in result.iter_mut().enumerate() {
            *slot_is_ref = lowered_block(slot);
        }
        return result;
    }

    // ESCAPE GATE (soundness). Untainting a borrow arg is only safe when the
    // borrowed value CANNOT escape the call into a location that outlives it.
    // A callee with a `&var T` OUT-PARAM (or a `&var self` / `consume self`
    // receiver) is a mutable out-channel: it can store a value DERIVED FROM —
    // or aliasing — a borrowed arg into a collection / field the caller still
    // owns after the call returns (rondo's `split_path_query_into(p: &String,
    // request: &var Request)` writes `request.path = …(p)`; `Request.parse`
    // then RETURNS `request`). If we untaint the borrow args of such a callee,
    // the caller frees the borrowed source at scope exit while the out-channel
    // still references it → use-after-free (rondo's `<none>` capture reads).
    //
    // Full escape analysis is out of scope at this layer. The cheap, sound,
    // conservative rule: if ANY candidate signature carries a mutable
    // out-channel — a `&var T` param, or a `RefMut`/`Consuming` self — keep the
    // CONSERVATIVE taint for EVERY arg of this callee (report all-`false`). A
    // genuinely-borrowed source then leaks rather than dangles; soundness
    // strictly beats leak-fixing. Pure readers (no `&var` out-param, `&self`
    // or no self) still get their borrow args untainted — the common
    // `include?`/`find`/measure/`to_eq` shapes that motivated the leak fix.
    //
    // A `Fn[...]` parameter is exempt from the gate: borrow check rejects
    // every use of one other than calling or forwarding it (E1018), so its
    // closure pair cannot escape through any out-channel.
    let has_mut_out_channel = candidates.iter().any(|(sig, _)| {
        matches!(
            sig.self_mode,
            Some(crate::hir::nodes::HirSelfMode::RefMut)
                | Some(crate::hir::nodes::HirSelfMode::Consuming)
        ) || sig.params.iter().any(|p| ty_is_mut_reference(&p.ty))
    });

    // For each value-carrying arg slot, decide ref-ness. A slot is BORROWED
    // only when EVERY candidate that maps a parameter to that slot agrees it is
    // a reference. Any disagreement (or a candidate with no param at that slot)
    // makes the slot conservatively non-borrowed.
    for (slot, slot_is_ref) in result.iter_mut().enumerate() {
        let mut all_ref = true;
        let mut saw_any = false;
        for (sig, takes_self) in &candidates {
            // Map the MIR arg slot to the signature param index. A self-taking
            // method's arg0 is the receiver (no signature param); its declared
            // params line up with args[1..].
            let param_idx = if *takes_self {
                if slot == 0 {
                    // Receiver slot: a `var self` receiver is always a
                    // borrow of the caller's object (the out-channel itself,
                    // see `&var` below); `&self` is one unless the callee has
                    // an out-channel; `consume self` keeps the taint.
                    saw_any = true;
                    let receiver_borrows = match sig.self_mode {
                        Some(crate::hir::nodes::HirSelfMode::RefMut) => true,
                        Some(crate::hir::nodes::HirSelfMode::Ref) => !has_mut_out_channel,
                        _ => false,
                    };
                    if !receiver_borrows {
                        all_ref = false;
                        break;
                    }
                    continue;
                }
                slot - 1
            } else {
                slot
            };
            match sig.params.get(param_idx) {
                Some(p) => {
                    saw_any = true;
                    // A `Fn[...]` parameter (incl. the synthetic `__block`)
                    // is a borrow for the duration of the call. The callee
                    // never owns or drops the closure pair; the caller
                    // reclaims it after the call. Treating this slot as
                    // consuming would leak every block and lambda passed to
                    // it, with its captures.
                    //
                    // A `&var T` parameter is the out-channel itself, never
                    // what escapes through it: the callee cannot store it
                    // (a borrow into an owned slot is E0744) or rebind it
                    // (E1020 for a handle; a String slot write frees the old
                    // buffer), so the caller still owns and frees the value.
                    let declared_block = lowered_blocks.is_none() && p.ty.is_borrowed_block();
                    let borrows = declared_block
                        || lowered_block(slot)
                        || ty_is_mut_reference(&p.ty)
                        || (!has_mut_out_channel && ty_is_reference(&p.ty));
                    if !borrows {
                        all_ref = false;
                        break;
                    }
                }
                None => {
                    // Variadic / default-filled / arity mismatch — be
                    // conservative for this candidate's view of the slot.
                    saw_any = true;
                    all_ref = false;
                    break;
                }
            }
        }
        *slot_is_ref = (saw_any && all_ref) || lowered_block(slot);
    }

    result
}

/// Every resolver signature a mangled USER callee could name, each paired
/// with whether it takes `self` (so the caller knows the arg/param offset).
///
/// Strips the generic-class mono and overload suffixes so the name matches
/// the resolver's stored def name, then resolves:
///
/// 1. a CLASS-QUALIFIED method: a method callee mangles `Class_method`, so the
///    base splits into the longest prefix that NAMES a Class/Struct/Enum and
///    the remaining method name. Resolving on the SPECIFIC parent class (by
///    DefId) is load-bearing: a bare method name like `read` / `init`
///    collides across dozens of stdlib classes whose `self_mode` and param
///    shapes differ. A generic-class instantiation mangles
///    `Class__mono__<Args>_method`; it resolves on the generic class itself,
///    whose declared params are the specialised copy's before substitution.
/// 2. a FREE FUNCTION: no class prefix, so the whole base name. Free-fn
///    overloads (`name__overload…`) collapse onto the same base.
///
/// An unresolved name yields no candidates.
fn user_callee_candidates<'s>(
    symbols: &'s crate::resolve::symbols::SymbolTable,
    mangled_callee: &'s str,
) -> Vec<(&'s crate::resolve::symbols::FnSignature, bool)> {
    use crate::resolve::symbols::DefKind;

    let base = strip_call_suffixes(mangled_callee);
    let mut candidates = Vec::new();
    let qualified = split_mono_class_method(symbols, mangled_callee)
        .or_else(|| split_class_qualified_method(symbols, base));
    if let Some((parent_id, method_name)) = qualified {
        for def in symbols.methods_of(parent_id, method_name) {
            if let DefKind::Method { signature, .. } = &def.kind {
                candidates.push((signature, signature.self_mode.is_some()));
            }
        }
    }
    if candidates.is_empty() {
        for def in symbols.overloads_named(base) {
            if let DefKind::Function { signature } = &def.kind {
                candidates.push((signature, signature.self_mode.is_some()));
            }
        }
    }
    candidates
}

/// For a USER callee, which of its `arg_count` MIR argument slots bind a
/// slot-addressed `&var T` parameter (`n: &var Int`, `s: &var String`): the
/// caller must pass the ADDRESS of the argument's storage there, because the
/// callee loads and stores through it (`mir::refmut_abi`). The receiver slot
/// of a `self`-taking method is never one.
///
/// The signature comes from the callee's mangled name (the same resolution
/// `user_callee_param_is_ref` uses), falling back to `preferred` (the def
/// typeck selected) when the name names none, as for a mixin default copied
/// into an implementor. Every candidate must agree on a slot; overloads that
/// disagree are disambiguated by `preferred` when it is one of them.
///
/// # Errors
///
/// When the candidates disagree on a slot and `preferred` does not settle
/// it. Passing a value where the callee expects an address (or the reverse)
/// would corrupt memory, so the call is refused rather than guessed.
pub fn user_callee_refmut_slot_params(
    symbols: &crate::resolve::symbols::SymbolTable,
    mangled_callee: &str,
    preferred: Option<crate::hir::nodes::DefId>,
    arg_count: usize,
) -> Result<Vec<bool>, String> {
    use crate::resolve::symbols::DefKind;

    let preferred_sig = preferred.and_then(|id| match &symbols.get(id)?.kind {
        DefKind::Method { signature, .. } | DefKind::Function { signature } => Some(signature),
        _ => None,
    });
    let mut candidates = user_callee_candidates(symbols, mangled_callee);
    if candidates.is_empty() {
        if let Some(sig) = preferred_sig {
            candidates.push((sig, sig.self_mode.is_some()));
        }
    }
    let slot_mask = |sig: &crate::resolve::symbols::FnSignature, takes_self: bool| {
        (0..arg_count)
            .map(|slot| {
                let param_idx = if takes_self {
                    match slot.checked_sub(1) {
                        Some(idx) => idx,
                        None => return false,
                    }
                } else {
                    slot
                };
                sig.params
                    .get(param_idx)
                    .is_some_and(|p| crate::mir::refmut_abi::is_slot_refmut(&p.ty))
            })
            .collect::<Vec<bool>>()
    };
    let masks: Vec<Vec<bool>> = candidates
        .iter()
        .map(|(sig, takes_self)| slot_mask(sig, *takes_self))
        .collect();
    let Some(first) = masks.first() else {
        return Ok(vec![false; arg_count]);
    };
    if masks.iter().all(|m| m == first) {
        return Ok(first.clone());
    }
    if let Some(sig) = preferred_sig {
        if candidates.iter().any(|(c, _)| std::ptr::eq(*c, sig)) {
            return Ok(slot_mask(sig, sig.self_mode.is_some()));
        }
    }
    Err(format!(
        "cannot tell which arguments of `{mangled_callee}` are `&var` parameters: its \
         overloads disagree, and a `&var` of a scalar or String must be passed as the \
         address of the caller's variable"
    ))
}

/// A declared parameter type that auto-borrows its argument: `&T` / `&var T`
/// (with or without an explicit lifetime). Mirrors
/// `borrow_check::checks::arg_is_reborrowed_reference` — kept in lockstep.
fn ty_is_reference(ty: &crate::hir::types::Ty) -> bool {
    use crate::hir::types::Ty;
    matches!(
        ty,
        Ty::Ref(_) | Ty::RefMut(_) | Ty::RefLifetime(_, _) | Ty::RefMutLifetime(_, _)
    )
}

/// A declared parameter type that is a MUTABLE borrow: `&var T` (with or
/// without an explicit lifetime). A `&var T` param is an OUT-CHANNEL — the
/// callee can store into the location the caller still owns after the call —
/// so its presence in a signature gates off the borrow-arg untaint (see the
/// escape gate in `user_callee_param_is_ref`). The immutable `&T` is excluded:
/// a read-only borrow cannot escape a value the way a `&var` write can.
fn ty_is_mut_reference(ty: &crate::hir::types::Ty) -> bool {
    use crate::hir::types::Ty;
    matches!(ty, Ty::RefMut(_) | Ty::RefMutLifetime(_, _))
}

/// Split a mangled method callee `Class_method` into `(parent_class_def_id,
/// method_name)` by finding the LONGEST prefix that names a Class/Struct/Enum
/// in `symbols`. Returns `None` for a name with no such prefix (a free fn).
///
/// Mirrors `Lowerer::class_name_from_mangled`'s right-to-left longest-prefix
/// walk — class names contain underscores (`__HandlerFuture`) and method names
/// contain underscores (`read_to_string`), so a naive `split('_')` is wrong.
/// We additionally return the parent's `DefId` so the caller can scope the
/// method lookup to exactly that class's methods.
fn split_class_qualified_method<'s>(
    symbols: &'s crate::resolve::symbols::SymbolTable,
    base: &'s str,
) -> Option<(crate::hir::nodes::DefId, &'s str)> {
    let mut end = base.len();
    while let Some(pos) = base[..end].rfind('_') {
        let candidate = &base[..pos];
        if !candidate.is_empty() {
            if let Some(def) = symbols.type_def(candidate) {
                return Some((def.id, &base[pos + 1..]));
            }
        }
        end = pos;
    }
    None
}

/// Split a generic-class method callee `Class__mono__<Args>_method` into the
/// generic class and the method name. `<Args>` is one or more `mono_arg_token`s
/// joined by `__`, and a token may itself contain `_`, so the method is the
/// first `_`-suffix of the remainder that names a method of `Class`. `None`
/// when the callee is not a mono method of a known type (a mono free fn
/// `f__mono__Int` has no method suffix and is handled by the free-fn path).
fn split_mono_class_method<'s>(
    symbols: &crate::resolve::symbols::SymbolTable,
    callee: &'s str,
) -> Option<(crate::hir::nodes::DefId, &'s str)> {
    const MONO_SEP: &str = "__mono__";
    let pos = callee.find(MONO_SEP)?;
    let class_id = symbols.type_def(&callee[..pos])?.id;
    let rest = &callee[pos + MONO_SEP.len()..];
    rest.match_indices('_').find_map(|(i, _)| {
        let method = &rest[i + 1..];
        let method = method
            .find("__overload")
            .map_or(method, |end| &method[..end]);
        (!method.is_empty() && symbols.methods_of(class_id, method).next().is_some())
            .then_some((class_id, method))
    })
}

/// Strip the synthetic `__mono__…` and `__overload…` suffixes a mangled MIR
/// callee may carry, leaving the textual base the resolver stored as a def
/// name. Order: trim `__mono__` first (generic-class instantiation), then
/// `__overload` (arity/type overload disambiguation).
fn strip_call_suffixes(callee: &str) -> &str {
    let base = match callee.find("__mono__") {
        Some(pos) => &callee[..pos],
        None => callee,
    };
    match base.find("__overload") {
        Some(pos) => &base[..pos],
        None => base,
    }
}

/// The three runtime collection families, keyed off the mangled-callee
/// type prefix. The `Vec` family folds in `Array`, `Hash` folds in `HashMap`
/// and `Map`, `Set` folds in `HashSet` — the same aliasing the runtime ABI
/// uses for these container types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CollectionFamily {
    Vec,
    Hash,
    Set,
}

/// Classify `callee` by its collection-type prefix (e.g. `Vec_push`,
/// `HashMap[K, V]_insert`), for `has_runtime_prefix`.
fn collection_family(callee: &str) -> Option<CollectionFamily> {
    const VEC: &[&str] = &["Vec_", "Vec[", "Array_", "Array["];
    const HASH: &[&str] = &["Hash_", "Hash[", "HashMap_", "HashMap[", "Map_", "Map["];
    const SET: &[&str] = &["Set_", "Set[", "HashSet_", "HashSet["];
    if VEC.iter().any(|p| callee.starts_with(p)) {
        Some(CollectionFamily::Vec)
    } else if HASH.iter().any(|p| callee.starts_with(p)) {
        Some(CollectionFamily::Hash)
    } else if SET.iter().any(|p| callee.starts_with(p)) {
        Some(CollectionFamily::Set)
    } else {
        None
    }
}

/// Whether a `.rx` FFI declaration's parameter of type `ty` takes an owned
/// value that moves into C: a parameter that is not the receiver, a
/// reference, a raw pointer, a block or a scalar. The one rule for "an owned
/// value passed to C moves" (`drops::FfiByValue`); a C function that only
/// reads its argument declares it `&T`.
pub fn ffi_param_takes_value(ty: &crate::hir::types::Ty, is_receiver: bool) -> bool {
    use crate::hir::types::Ty;
    let borrowed = matches!(
        ty,
        Ty::Ref(_)
            | Ty::RefMut(_)
            | Ty::RefLifetime(..)
            | Ty::RefMutLifetime(..)
            | Ty::RawPtr(_)
            | Ty::RawPtrMut(_)
            | Ty::RawPtrVoid
    ) || ty.is_borrowed_block();
    !(is_receiver || borrowed || ty.is_scalar_primitive())
}

/// Old prefix list inside `is_runtime_borrow_helper` (the pre-split `drops.rs`).
/// The collection-prefix ladder is `collection_family`; this adds the
/// non-collection runtime prefixes.
fn has_runtime_prefix(c: &str) -> bool {
    collection_family(c).is_some()
        || c.starts_with("ruxen_")
        || c.starts_with("String_")
        || c.starts_with("&str_")
}

// ---------------------------------------------------------------------------
// The declarative tables started as a 1:1 move of the literals formerly inline
// in drops/ (line ranges noted). Intentional additions after extraction carry
// focused tests and are recorded as deltas in runtime_abi_parity.rs.
//
// Each table is keyed once by C symbol. A row's second field lists the
// mangled `Class_method` spellings a call can still carry when no FFI alias
// rewrote it, so a symbol and its spellings can never be classified apart.
// The descriptor-taking forms `key_desc.rs` rewrites calls to are not rows:
// `callee_ownership` maps them back to their base symbol first.
// ---------------------------------------------------------------------------

/// One runtime symbol and the mangled spellings that name it.
type SymbolRow = (&'static str, &'static [&'static str]);

/// Is `callee` a row's C symbol or one of its mangled spellings?
fn listed(table: &[SymbolRow], callee: &str) -> bool {
    table
        .iter()
        .any(|(symbol, spellings)| *symbol == callee || spellings.contains(&callee))
}

/// Callees returning a fresh heap allocation owned by `dest`.
/// Old: `FRESH_ALLOC_CALLEES` in the pre-split `drops.rs`. A C function
/// whose `.rx` declaration says `-> owned T` needs no row here
/// ([`DeclaredResults`], read first by [`result_ownership`]).
///
/// # The obligation you take on by adding a name here
///
/// A helper listed here must return a value the destination owns
/// **completely**: for a collection that means the spine, the backing
/// array, AND every heap element reachable from a slot. Drop elaboration
/// will hand that destination the element-aware deep drop built by
/// [`heap_free_call`](super::drops::heap_free_call), so a slot the
/// helper did not itself allocate is a **double free by construction**.
/// Copying a borrowed source's element pointer into a slot the result
/// will deep-free is exactly that mistake.
///
/// This is not hypothetical. `ruxen_vec_skip` (`Array#drop`),
/// `ruxen_vec_chain`, `ruxen_set_union` / `_intersection` / `_difference`
/// and `ruxen_set_from_iter` were all listed here while only aliasing
/// their source's elements, and every one of them aborted on a `String`
/// element type the moment both owners went out of scope. They now
/// deep-copy per the `RUXEN_ELEM_*` descriptor (see `runtime.h`), which
/// is what makes the membership below honest.
///
/// # Leaving a helper OUT is not a safe default either
///
/// The tempting fix for the above is to drop the name from this list:
/// with no Fresh classification the destination is tainted, no drop is
/// emitted, and nothing double-frees. That trades a crash for a silent
/// leak of the spine and backing array, and it is why `ruxen_vec_take`
/// and `ruxen_vec_clone` leak today — they are the same aliasing shape as
/// `skip`, and the only reason they never crashed is that they are absent
/// here. Neither membership nor absence is automatically correct. The
/// question to answer is "does this helper allocate everything its result
/// will be asked to free", and if the answer is no, fix the helper rather
/// than adjusting its classification to route around the symptom.
///
/// # Helpers returning a borrowed VIEW are fine
///
/// `ruxen_hash_keys` / `_values` / `ruxen_set_iter` alias their source's
/// elements deliberately and are still correctly listed here. They are
/// typed `Array[&T]`, and a `Ty::Ref(_)` element is described as POD, so
/// `ruxen_vec_drop` frees the result's spine (which this list owns) and
/// none of its elements (which it borrows). The element type carrying the
/// borrow is what keeps them honest; a view helper retyped to return owned
/// elements would become a violator.
const FRESH_ALLOC_CALLEES: &[SymbolRow] = &[
    ("ruxen_string_from", &["String_from"]),
    ("ruxen_string_concat", &[]),
    // The scalar-to-text helpers each `malloc` a new buffer (string.c), and
    // `Formatter#buffer` hands over the formatter's buffer and frees the
    // formatter (fmt.c). Interpolation and the synthesised `{Prim}_fmt`
    // bodies call them for every formatted part; unclassified, every one of
    // those strings leaked.
    ("ruxen_int_to_string", &["Int_to_s", "USize_to_s"]),
    ("ruxen_float_to_string", &["Float_to_s"]),
    ("ruxen_bool_to_string", &["Bool_to_s"]),
    ("ruxen_char_to_string", &["Char_to_s"]),
    ("ruxen_float_to_string_prec", &["Float_to_string_prec"]),
    ("ruxen_string_truncate_chars", &["String_truncate_chars"]),
    ("ruxen_fmt_formatter_buffer", &["Formatter_buffer"]),
    ("ruxen_vec_new", &["Vec_new"]),
    // `ruxen_vec_skip` materializes the splatted tail into a new Array. MIR
    // emits it as `ruxen_vec_skip_elem` (key_desc.rs Route 3 appends the
    // element descriptor), which classifies through this row.
    ("ruxen_vec_skip", &[]),
    ("ruxen_hash_new", &["Hash_new", "HashMap_new"]),
    // `ruxen_string_into_bytes` is in BOTH this table and CONSUME_HELPERS,
    // and this is NOT a contradiction — the two are load-bearing in two
    // INDEPENDENT axes the drop pass tracks separately:
    //   * RESULT (dest): into_bytes produces a fresh Vec[U8] spine the caller
    //     owns. Without Fresh here, the dest is tainted, scope-exit drop emits
    //     no `ruxen_vec_drop`, and the Vec struct + data buffer LEAK (verified:
    //     drop_fixtures::string_into_bytes_transfers_ownership goes to
    //     raw_outstanding=2, vec_frees=0 the moment this membership is removed).
    //   * ARG (arg0): the runtime frees the source `char*` internally, so the
    //     source String must be tainted (consume) to avoid a double-free.
    // The original "fresh-vs-consume contradiction" hypothesis (Phase 2 plan
    // Task 3b) was disproven by the leak backstop: the result and arg axes are
    // orthogonal, and BOTH classifications are required. Kept as-is.
    ("ruxen_string_into_bytes", &["String_into_bytes"]),
    ("ruxen_string_push", &["String_push"]),
    ("ruxen_vec_from_iter", &["Vec_from_iter"]),
    ("ruxen_string_from_iter", &["String_from_iter"]),
    // B73: the six table constructors below are emitted as their
    // descriptor-taking `_keyed` forms, which classify through these rows.
    (
        "ruxen_hash_from_iter",
        &["Hash_from_iter", "HashMap_from_iter"],
    ),
    (
        "ruxen_set_from_iter",
        &["Set_from_iter", "HashSet_from_iter"],
    ),
    ("ruxen_set_new", &["Set_new", "HashSet_new"]),
    (
        "ruxen_hash_with_capacity",
        &["Hash_with_capacity", "HashMap_with_capacity"],
    ),
    ("ruxen_hash_keys", &["Hash_keys", "HashMap_keys"]),
    ("ruxen_hash_values", &["Hash_values", "HashMap_values"]),
    ("ruxen_hash_iter", &["Hash_iter", "HashMap_iter"]),
    ("ruxen_hash_entries", &[]),
    ("ruxen_hash_remove", &["Hash_remove", "HashMap_remove"]),
    (
        "ruxen_set_with_capacity",
        &["Set_with_capacity", "HashSet_with_capacity"],
    ),
    ("ruxen_set_iter", &["Set_iter", "HashSet_iter"]),
    ("ruxen_set_union", &["Set_union", "HashSet_union"]),
    (
        "ruxen_set_intersection",
        &["Set_intersection", "HashSet_intersection"],
    ),
    (
        "ruxen_set_difference",
        &["Set_difference", "HashSet_difference"],
    ),
    // Emitted as `ruxen_vec_chain_elem` (key_desc.rs Route 3).
    ("ruxen_vec_chain", &[]),
    ("ruxen_vec_zip", &[]),
    ("ruxen_command_new", &["Command_new"]),
    ("ruxen_command_arg", &["Command_arg"]),
    ("ruxen_command_args", &["Command_args"]),
    ("ruxen_command_env", &["Command_env"]),
    ("ruxen_command_current_dir", &["Command_current_dir"]),
    ("ruxen_command_status", &["Command_status"]),
    ("ruxen_command_output", &["Command_output"]),
    ("ruxen_output_status", &["Output_status"]),
    ("ruxen_output_stdout", &["Output_stdout"]),
    ("ruxen_output_stderr", &["Output_stderr"]),
    ("ruxen_file_open", &["File_open"]),
    ("ruxen_file_create", &["File_create"]),
    ("ruxen_file_append", &["File_append"]),
    ("ruxen_file_open_options", &["File_open_options"]),
    ("ruxen_file_metadata", &["File_metadata"]),
    ("ruxen_file_read", &["File_read"]),
    ("ruxen_file_read_to_string", &["File_read_to_string"]),
    ("ruxen_file_read_all", &["File_read_all"]),
    ("ruxen_file_write", &["File_write"]),
    ("ruxen_file_write_all", &["File_write_all"]),
    ("ruxen_file_write_str", &["File_write_str"]),
    ("ruxen_file_flush", &["File_flush"]),
    ("ruxen_file_seek", &["File_seek"]),
    ("ruxen_file_close", &["File_close"]),
    ("ruxen_open_options_new", &["OpenOptions_new"]),
    ("ruxen_mutex_lock", &["Mutex_lock_raw"]),
    ("ruxen_mutex_try_lock", &["Mutex_try_lock_raw"]),
];

/// The free and drop glue the drop pass emits (`drops::heap_free_call`) and
/// `ruxen_dealloc`. Each borrows arg0 (the `borrows_first_arg` rule, whose
/// `_init` half stays an `ends_with` check in `callee_ownership`) and is a
/// consume helper; the descriptor args after it are literals.
const FREE_FAMILY: &[&str] = &[
    "ruxen_dealloc",
    "ruxen_string_free",
    "ruxen_vec_drop",
    "ruxen_hash_drop",
    "ruxen_set_drop",
];

/// Consume helpers beyond [`FREE_FAMILY`]. Old `is_runtime_consume_helper`
/// (the pre-split `drops.rs`).
const CONSUME_HELPERS: &[SymbolRow] = &[
    ("ruxen_string_into_bytes", &["String_into_bytes"]),
    ("ruxen_vec_from_iter", &["Vec_from_iter"]),
];

/// Does this callee MOVE the ownership of its pointer args into an FFI-owned
/// runtime structure (the task scheduler / a spawned OS thread)? The single
/// source of truth, backed by [`MOVE_BY_FFI`]. `drops/`'s `moved_to_ffi`
/// collection consults this instead of re-listing the symbols inline, so the
/// drop-pass exclusion set and the `callee_ownership` borrow verdict can never
/// disagree again (they previously did: the inline list carried a dead dotted
/// `"Task.spawn_raw"` spelling the table never had).
pub fn is_move_by_ffi(callee: &str) -> bool {
    listed(MOVE_BY_FFI, callee)
}

/// Old `is_move_by_ffi_callee` (the pre-split `drops.rs`).
const MOVE_BY_FFI: &[SymbolRow] = &[
    ("ruxen_executor_spawn", &["Task_spawn_raw"]),
    ("ruxen_thread_spawn", &["Thread_spawn", "Thread_spawn_raw"]),
];

/// Old `is_command_builder_method` (the pre-split `drops.rs`).
const COMMAND_BUILDER: &[SymbolRow] = &[
    ("ruxen_command_arg", &["Command_arg"]),
    ("ruxen_command_args", &["Command_args"]),
    ("ruxen_command_env", &["Command_env"]),
    ("ruxen_command_current_dir", &["Command_current_dir"]),
];

/// Old `is_command_terminal_or_accessor` (the pre-split `drops.rs`).
const COMMAND_TERMINAL_OR_ACCESSOR: &[SymbolRow] = &[
    ("ruxen_command_status", &["Command_status"]),
    ("ruxen_command_output", &["Command_output"]),
    ("ruxen_output_status", &["Output_status"]),
    ("ruxen_output_stdout", &["Output_stdout"]),
    ("ruxen_output_stderr", &["Output_stderr"]),
    ("ruxen_exit_status_code", &["ExitStatus_code"]),
    ("ruxen_exit_status_success", &["ExitStatus_success"]),
];

#[cfg(test)]
#[path = "runtime_abi_tests.rs"]
mod tests;
