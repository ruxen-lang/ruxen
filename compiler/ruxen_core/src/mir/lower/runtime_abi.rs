//! The single source of truth for runtime-callee ownership in MIR lowering.
//!
//! Every runtime symbol the drop-elaboration pass (`drops.rs`) or the
//! static-constructor fast path (`expr/method_call.rs`) special-cases is
//! classified HERE, exactly once, by `callee_ownership`. This replaces the six
//! overlapping, partly-contradicting predicates that previously lived inline in
//! the `drops.rs` `Call` arm (FRESH_ALLOC_CALLEES, is_runtime_consume_helper,
//! is_runtime_borrow_helper ×3 rebinds, is_move_by_ffi_callee,
//! is_pointer_store_helper, borrows_first_arg) plus the duplicated static-ctor
//! lists in method_call.rs / util.rs.
//!
//! ABI contract: the C side's ownership semantics are documented per-symbol in
//! the stdlib specs cited inline (task_spawn.spec.md §B10, the Command builder
//! notes, mutex.c, scheduler.c:150-152). A WRONG entry here is a double-free or
//! use-after-free — every change must move a row, with a visible diff to the
//! parity oracle (tests/runtime_abi_parity.rs).

use crate::codegen::runtime::extract_method_name;

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

/// Which positional arguments have their ownership TRANSFERRED to the callee
/// (the arg's local must be tainted / removed from `alloc_rooted`). Small,
/// bounded set — at most arg0..arg2 ever transfer in v1, so a u8 bitset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ArgMask(u8);

impl ArgMask {
    pub const fn none() -> Self {
        ArgMask(0)
    }
    pub const fn single(i: usize) -> Self {
        ArgMask(1 << i)
    }
    pub const fn pair(i: usize, j: usize) -> Self {
        ArgMask((1 << i) | (1 << j))
    }
    pub fn contains(self, i: usize) -> bool {
        // The mask is a u8 (only arg0..arg7 can ever be flagged as transferred).
        // `contains` is queried for EVERY arg index in the drops.rs arg loop,
        // including args >= 8 (e.g. a 9-arg FFI like `ruxen_canvas_draw_rect`),
        // so guard the shift — a bit beyond the u8 is unset by definition.
        i < 8 && self.0 & (1 << i) != 0
    }
}

/// The full ownership verdict for one callee. Mirrors the four decisions the
/// `drops.rs` arg loop makes, in the SAME precedence order it applied them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CalleeOwnership {
    /// Result-into-`dest` ownership.
    pub result: ResultOwnership,
    /// arg0 is borrowed even though the callee otherwise transfers (the
    /// `borrows_first_arg` rule: `_init` ctors + the free/drop family). The
    /// arg loop `continue`s on arg0 when this is set. HIGHEST arg precedence.
    pub borrows_first_arg: bool,
    /// Args that are explicitly transferred regardless of `args_are_borrowed`
    /// (Vec/Hash/Set push/insert value+key slots). These
    /// are tainted before the borrow check. Old: transfer_indices +
    /// is_pointer_store_helper.
    pub arg_transfer: ArgMask,
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
            arg_transfer: ArgMask::none(),
            args_are_borrowed: false,
        }
    }
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

    // --- explicit per-arg transfers (old transfer_indices + pointer-store) ---
    let arg_transfer = arg_transfer_mask(callee);

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
        arg_transfer,
        args_are_borrowed,
    }
}

/// True when a user-drop FFI callee releases the allocation that represents
/// `self`, rather than only cleaning up resources stored inside it.
///
/// Most Ruxen drop hooks follow the compiler-owned-storage contract: run the
/// hook, then emit the type-directed heap free. These runtime handles are the
/// exception because their destructor is also their ownership primitive
/// (atomic/refcount decrement or handle release). Appending `ruxen_dealloc`
/// after one of them double-frees a final handle and incorrectly frees a
/// non-final shared handle.
pub fn drop_frees_storage(callee: &str) -> bool {
    DROP_FREES_STORAGE.contains(&callee)
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
///   * method   → the bare method name via [`extract_method_name`].
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
pub fn user_callee_param_is_ref(
    symbols: &crate::resolve::symbols::SymbolTable,
    mangled_callee: &str,
    arg_count: usize,
) -> Vec<bool> {
    use crate::resolve::symbols::DefKind;

    let mut result = vec![false; arg_count];
    if arg_count == 0 {
        return result;
    }

    // Strip generic-class mono + overload suffixes so the name matches the
    // resolver's stored def name. `__mono__` and `__overload` are the two
    // synthetic separators MIR appends; `mono_base` / the overload mangler are
    // their producers. We only need the textual base for the name compare.
    let base = strip_call_suffixes(mangled_callee);

    // Collect every candidate signature this name could resolve to, paired with
    // whether it is a `self`-taking method (so we know the arg/param offset).
    let mut candidates: Vec<(&crate::resolve::symbols::FnSignature, bool)> = Vec::new();

    // (1) CLASS-QUALIFIED method: a method callee mangles `Class_method`, so the
    // base splits into the longest prefix that NAMES a Class/Struct/Enum and the
    // remaining method name. Resolving on the SPECIFIC parent class (by DefId)
    // is load-bearing: a bare method name like `read` / `init` collides across
    // dozens of stdlib classes (File.read, BufReader.read, async readers, …)
    // whose `self_mode` and param shapes differ, so an unqualified by-name match
    // would see a mass of contradictory candidates and conservatively untaint
    // NOTHING. Class-qualification narrows to exactly the parent class's
    // overload set.
    // A generic-class instantiation mangles `Class__mono__<Args>_method`, so
    // `strip_call_suffixes` cuts the method name off together with the args
    // and leaves a bare `Class` that names no callee. Resolve the method on
    // the generic class itself first: its declared `self_mode` and params are
    // the specialised copy's, only with `T` substituted.
    let qualified = split_mono_class_method(symbols, mangled_callee)
        .or_else(|| split_class_qualified_method(symbols, base));
    if let Some((parent_id, method_name)) = qualified {
        for def in symbols.methods_of(parent_id, method_name) {
            if let DefKind::Method { signature, .. } = &def.kind {
                let takes_self = signature.self_mode.is_some();
                candidates.push((signature, takes_self));
            }
        }
    }

    // (2) FREE FUNCTION: no class prefix — match the whole base name. Free-fn
    // overloads (`name__overload…`) collapse onto the same base; the
    // all-candidates-agree rule below keeps a disagreeing overload set
    // conservative.
    if candidates.is_empty() {
        for def in symbols.overloads_named(base) {
            if let DefKind::Function { signature } = &def.kind {
                let takes_self = signature.self_mode.is_some();
                candidates.push((signature, takes_self));
            }
        }
    }

    if candidates.is_empty() {
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
    let has_mut_out_channel = candidates.iter().any(|(sig, _)| {
        matches!(
            sig.self_mode,
            Some(crate::hir::nodes::HirSelfMode::RefMut)
                | Some(crate::hir::nodes::HirSelfMode::Consuming)
        ) || sig.params.iter().any(|p| ty_is_mut_reference(&p.ty))
    });
    if has_mut_out_channel {
        return result;
    }

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
                    // Receiver slot: a borrow only when this candidate's
                    // `self_mode` is the immutable `Ref` (`&self`). `RefMut`
                    // and `Consuming` keep the taint (see doc above).
                    saw_any = true;
                    if !matches!(sig.self_mode, Some(crate::hir::nodes::HirSelfMode::Ref)) {
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
                    // The synthetic `__block` parameter is also a borrow for
                    // the duration of the yielding call. The callee never owns
                    // or drops the closure pair; the caller reclaims it after
                    // the call. Treating this Fn slot as consuming would leak
                    // every trailing block and its captures.
                    if !matches!(p.name.as_str(), "__block" | "&block") && !ty_is_reference(&p.ty) {
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
        *slot_is_ref = saw_any && all_ref;
    }

    result
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
/// `HashMap[K, V]_insert`). Single source for the prefix ladder that both
/// `arg_transfer_mask` and `has_runtime_prefix` consumed in duplicate.
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

/// Old `transfer_indices` + `is_pointer_store_helper`, folded into one mask.
fn arg_transfer_mask(callee: &str) -> ArgMask {
    let m = extract_method_name(callee);
    let family = collection_family(callee);
    match (family, m) {
        (Some(CollectionFamily::Vec), "push") => ArgMask::single(1),
        (Some(CollectionFamily::Vec), "insert") => ArgMask::single(2),
        (Some(CollectionFamily::Hash), "insert") => ArgMask::pair(1, 2),
        (Some(CollectionFamily::Set), "insert") => ArgMask::single(1),
        _ if callee == "ruxen_vec_push" => ArgMask::single(1),
        _ if callee == "ruxen_vec_insert" => ArgMask::single(2),
        _ if callee == "ruxen_hash_insert" => ArgMask::pair(1, 2),
        _ if callee == "ruxen_set_insert" => ArgMask::single(1),
        // A `Mutex[T]` owns its payload: `Mutex.new(v)` moves `v` in and
        // `MutexGuard#set(v)` moves the new value in. Borrowing them left the
        // caller freeing a heap payload the mutex still held, so a write
        // through `MutexGuard#deref_mut` (which frees the old value) then
        // double-freed it at the caller's scope exit.
        _ if callee == "ruxen_mutex_new" => ArgMask::single(0),
        _ if callee == "ruxen_mutex_guard_set" => ArgMask::single(1),
        _ => ArgMask::none(),
    }
}

/// Old prefix list inside `is_runtime_borrow_helper` (drops.rs:1091-1107).
/// The collection-prefix ladder is shared with `arg_transfer_mask` via
/// `collection_family`; this adds the non-collection runtime prefixes.
fn has_runtime_prefix(c: &str) -> bool {
    collection_family(c).is_some()
        || c.starts_with("ruxen_")
        || c.starts_with("String_")
        || c.starts_with("&str_")
}

// ---------------------------------------------------------------------------
// The declarative tables started as a 1:1 move of the literals formerly inline
// in drops.rs (line ranges noted). Intentional additions after extraction carry
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
/// Old: drops.rs:694-934 `FRESH_ALLOC_CALLEES`.
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
    ("ruxen_open_options_read", &["OpenOptions_read"]),
    ("ruxen_open_options_write", &["OpenOptions_write"]),
    ("ruxen_open_options_append", &["OpenOptions_append"]),
    ("ruxen_open_options_truncate", &["OpenOptions_truncate"]),
    ("ruxen_open_options_create", &["OpenOptions_create"]),
    ("ruxen_open_options_create_new", &["OpenOptions_create_new"]),
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

/// FFI drop hooks that consume the storage carrying `self`.
const DROP_FREES_STORAGE: &[&str] = &[
    "ruxen_atomic_i64_drop",
    "ruxen_atomic_bool_drop",
    "ruxen_channel_sender_drop",
    "ruxen_channel_receiver_drop",
    "ruxen_mutex_drop",
    "ruxen_sharedsync_drop",
];

/// Consume helpers beyond [`FREE_FAMILY`]. Old `is_runtime_consume_helper`
/// (drops.rs:1003-1043).
const CONSUME_HELPERS: &[SymbolRow] = &[
    ("ruxen_string_into_bytes", &["String_into_bytes"]),
    ("ruxen_vec_from_iter", &["Vec_from_iter"]),
];

/// Does this callee MOVE the ownership of its pointer args into an FFI-owned
/// runtime structure (the task scheduler / a spawned OS thread)? The single
/// source of truth, backed by [`MOVE_BY_FFI`]. `drops.rs`'s `moved_to_ffi`
/// collection consults this instead of re-listing the symbols inline, so the
/// drop-pass exclusion set and the `callee_ownership` borrow verdict can never
/// disagree again (they previously did: the inline list carried a dead dotted
/// `"Task.spawn_raw"` spelling the table never had).
pub fn is_move_by_ffi(callee: &str) -> bool {
    listed(MOVE_BY_FFI, callee)
}

/// Old `is_move_by_ffi_callee` (drops.rs:1068-1088).
const MOVE_BY_FFI: &[SymbolRow] = &[
    ("ruxen_executor_spawn", &["Task_spawn_raw"]),
    ("ruxen_thread_spawn", &["Thread_spawn", "Thread_spawn_raw"]),
];

/// Old `is_command_builder_method` (drops.rs:1158-1168).
const COMMAND_BUILDER: &[SymbolRow] = &[
    ("ruxen_command_arg", &["Command_arg"]),
    ("ruxen_command_args", &["Command_args"]),
    ("ruxen_command_env", &["Command_env"]),
    ("ruxen_command_current_dir", &["Command_current_dir"]),
];

/// Old `is_command_terminal_or_accessor` (drops.rs:1123-1139).
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
