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
        self.0 & (1 << i) != 0
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
    /// (Vec/Hash/Set push/insert value+key slots; ruxen_store_ptr arg1). These
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
    // --- result ownership (old FRESH_ALLOC_CALLEES) ---
    let result = if FRESH_ALLOC_CALLEES.contains(&callee) {
        ResultOwnership::Fresh
    } else {
        ResultOwnership::None
    };

    // --- arg0 borrow (old borrows_first_arg) ---
    let borrows_first_arg = callee.ends_with("_init") || FREE_FAMILY.contains(&callee);

    // --- explicit per-arg transfers (old transfer_indices + pointer-store) ---
    let arg_transfer = arg_transfer_mask(callee);

    // --- remaining-arg borrow (old effective is_runtime_borrow_helper) ---
    let consumes = CONSUME_HELPERS.contains(&callee);
    let move_by_ffi = MOVE_BY_FFI.contains(&callee);
    let builder = COMMAND_BUILDER.contains(&callee);
    let prefix_borrow = has_runtime_prefix(callee);
    let terminal = COMMAND_TERMINAL_OR_ACCESSOR.contains(&callee);
    let args_are_borrowed = ((!consumes && !move_by_ffi && prefix_borrow) || terminal) && !builder;

    CalleeOwnership {
        result,
        borrows_first_arg,
        arg_transfer,
        args_are_borrowed,
    }
}

/// For a USER callee (a function/method NOT classified by the runtime-ABI
/// tables above), resolve which of its first `arg_count` *value-carrying*
/// parameters are declared as a reference (`&T` / `&var T`).
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
/// A by-value (consuming) param keeps its taint (returned `false`). When the
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
    if let Some((parent_id, method_name)) = split_class_qualified_method(symbols, base) {
        for def in symbols.iter() {
            if let DefKind::Method { parent, signature } = &def.kind {
                if *parent != parent_id {
                    continue;
                }
                let name_matches = def.name == method_name
                    || def.name.starts_with(&format!("{}__overload", method_name));
                if name_matches {
                    let takes_self = signature.self_mode.is_some();
                    candidates.push((signature, takes_self));
                }
            }
        }
    }

    // (2) FREE FUNCTION: no class prefix — match the whole base name. Free-fn
    // overloads (`name__overload…`) collapse onto the same base; the
    // all-candidates-agree rule below keeps a disagreeing overload set
    // conservative.
    if candidates.is_empty() {
        for def in symbols.iter() {
            let name_matches =
                def.name == base || def.name.starts_with(&format!("{}__overload", base));
            if name_matches {
                if let DefKind::Function { signature } = &def.kind {
                    let takes_self = signature.self_mode.is_some();
                    candidates.push((signature, takes_self));
                }
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
                    if !ty_is_reference(&p.ty) {
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
    use crate::resolve::symbols::DefKind;
    let is_type_def = |d: &crate::resolve::symbols::Definition, cand: &str| {
        matches!(
            &d.kind,
            DefKind::Class { .. } | DefKind::Struct { .. } | DefKind::Enum { .. }
        ) && (d.name == cand || d.name.replace('.', "_") == cand)
    };
    let mut end = base.len();
    while let Some(pos) = base[..end].rfind('_') {
        let candidate = &base[..pos];
        if !candidate.is_empty() {
            if let Some(def) = symbols.iter().find(|d| is_type_def(d, candidate)) {
                return Some((def.id, &base[pos + 1..]));
            }
        }
        end = pos;
    }
    None
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

/// Is `type_name::method_name` a static (no-`self`) constructor that dispatches
/// directly to a runtime symbol? Single source for the static-vs-instance
/// decision shared by `util.rs::is_builtin_static_method` and the
/// `method_call.rs` fast-path gate. Reconciles the two formerly-diverged lists
/// (util.rs:74-132 + method_call.rs:162-214) into one declarative table.
///
/// RECONCILIATION DECISIONS (recorded per the plan's obligation):
///  1. The universal `.new` rule: `method_name == "new"` is a static ctor for
///     ANY type. This is the behaviour `method_call.rs::is_collection_ctor`
///     actually ships (line 197); `util.rs` did NOT have it. The union keeps
///     it — verified against ffi_alias_single_entry + the broad suite that it
///     does not reroute a user class with a real `init` (the method_call.rs
///     fast path still gates on `lookup_ffi_alias` / the base-type allowlist
///     downstream, and the field_access/method_call `is_static` checks OR in
///     `is_user_static_method`, so a user `.new` with an init still resolves
///     correctly).
///  2. `String.from_bytes/from_iter`, `Thread.*`, `Mutex/Arc/SharedSync.new`
///     came from util.rs; `File.*`, `Duration.*`, `Instant.now`, `Tcp*`,
///     `BufReader/BufWriter` came from method_call.rs. The union is the set both
///     sites now read. Widening each site to the union is behaviour-preserving
///     for the observable compilation outcome (proven by the integration suite
///     in Task 5): the extra (type, method) pairs a given site gains were
///     already classified static at the OTHER site, and both sites funnel a
///     positive answer through the same downstream FFI-alias / `_init` gates.
pub fn is_static_constructor(type_name: &str, method_name: &str) -> bool {
    // Any `.new` is a static constructor (method_call.rs is_collection_ctor rule).
    if method_name == "new" {
        return true;
    }
    let base = match type_name.find('[') {
        Some(pos) => &type_name[..pos],
        None => type_name,
    };
    STATIC_CTORS
        .iter()
        .any(|(b, methods)| *b == base && methods.contains(&method_name))
}

/// (base_type, static-constructor method names). UNION of the two formerly
/// duplicated lists (util.rs:74-132 + method_call.rs:162-214). One source.
/// `new` is handled by the universal rule in `is_static_constructor`, so it is
/// omitted here.
const STATIC_CTORS: &[(&str, &[&str])] = &[
    // `from` REMOVED — the surface `String.from` static method was deleted
    // (the borrow→owned spelling is `x.clone`). `with_capacity`/`from_iter`/
    // `from_bytes` remain real static constructors. (The C symbol
    // `ruxen_string_from` is unaffected — it still backs `clone` and the
    // string-literal heap-copy machinery.)
    ("String", &["with_capacity", "from_iter", "from_bytes"]),
    ("Vec", &["with_capacity", "from_iter"]),
    ("Array", &["with_capacity", "from_iter"]),
    ("Hash", &["with_capacity", "from_iter"]),
    ("HashMap", &["with_capacity", "from_iter"]),
    ("Map", &["with_capacity", "from_iter"]),
    ("Set", &["with_capacity", "from_iter"]),
    ("HashSet", &["with_capacity", "from_iter"]),
    ("Thread", &["spawn", "current", "sleep", "yield_now"]),
    // Mutex / Arc / SharedSync: only `.new`, handled by the universal rule.
    (
        "Duration",
        &["from_secs", "from_millis", "from_micros", "from_nanos"],
    ),
    ("Instant", &["now"]),
    ("TcpListener", &["bind"]),
    ("TcpStream", &["connect"]),
    ("File", &["open", "create", "append", "open_options"]),
    ("BufReader", &["with_capacity"]),
    ("BufWriter", &["with_capacity"]),
];

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
    if callee == "ruxen_store_ptr" {
        return ArgMask::single(1);
    }
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
// The declarative tables. 1:1 move of the literals formerly inline in
// drops.rs (line ranges noted). Not invented — transcribed verbatim.
// ---------------------------------------------------------------------------

/// Callees returning a fresh heap allocation owned by `dest`.
/// Old: drops.rs:694-934 `FRESH_ALLOC_CALLEES`.
const FRESH_ALLOC_CALLEES: &[&str] = &[
    "String_from",
    "Vec_new",
    "Hash_new",
    "HashMap_new",
    "ruxen_string_from",
    "ruxen_string_concat",
    "ruxen_vec_new",
    "ruxen_hash_new",
    // `String_into_bytes` / `ruxen_string_into_bytes` are in BOTH this list and
    // CONSUME_HELPERS, and this is NOT a contradiction — they are load-bearing
    // in two INDEPENDENT axes the drop pass tracks separately:
    //   * RESULT (dest): into_bytes produces a fresh Vec[U8] spine the caller
    //     owns. Without Fresh here, the dest is tainted, scope-exit drop emits
    //     no `ruxen_vec_free`, and the Vec struct + data buffer LEAK (verified:
    //     drop_fixtures::string_into_bytes_transfers_ownership goes to
    //     raw_outstanding=2, vec_frees=0 the moment this membership is removed).
    //   * ARG (arg0): the runtime frees the source `char*` internally, so the
    //     source String must be tainted (consume) to avoid a double-free.
    // The original "fresh-vs-consume contradiction" hypothesis (Phase 2 plan
    // Task 3b) was disproven by the leak backstop: the result and arg axes are
    // orthogonal, and BOTH classifications are required. Kept as-is.
    "String_into_bytes",
    "ruxen_string_into_bytes",
    "String_push",
    "ruxen_string_push",
    "Vec_from_iter",
    "ruxen_vec_from_iter",
    "String_from_iter",
    "HashMap_from_iter",
    "Hash_from_iter",
    "Set_from_iter",
    "HashSet_from_iter",
    "ruxen_string_from_iter",
    "ruxen_hash_from_iter",
    "ruxen_set_from_iter",
    "Set_new",
    "HashSet_new",
    "ruxen_set_new",
    "HashMap_with_capacity",
    "Hash_with_capacity",
    "ruxen_hash_with_capacity",
    "HashMap_keys",
    "HashMap_values",
    "HashMap_iter",
    "Hash_keys",
    "Hash_values",
    "Hash_iter",
    "ruxen_hash_keys",
    "ruxen_hash_values",
    "ruxen_hash_iter",
    "ruxen_hash_entries",
    "HashMap_remove",
    "Hash_remove",
    "ruxen_hash_remove",
    "HashSet_with_capacity",
    "Set_with_capacity",
    "ruxen_set_with_capacity",
    "HashSet_iter",
    "Set_iter",
    "ruxen_set_iter",
    "HashSet_union",
    "HashSet_intersection",
    "HashSet_difference",
    "Set_union",
    "Set_intersection",
    "Set_difference",
    "ruxen_set_union",
    "ruxen_set_intersection",
    "ruxen_set_difference",
    "ruxen_vec_chain",
    "ruxen_vec_zip",
    "Command_new",
    "ruxen_command_new",
    "Command_arg",
    "ruxen_command_arg",
    "Command_args",
    "ruxen_command_args",
    "Command_env",
    "ruxen_command_env",
    "Command_current_dir",
    "ruxen_command_current_dir",
    "Command_status",
    "ruxen_command_status",
    "Command_output",
    "ruxen_command_output",
    "Output_status",
    "ruxen_output_status",
    "Output_stdout",
    "ruxen_output_stdout",
    "Output_stderr",
    "ruxen_output_stderr",
    "File_open",
    "ruxen_file_open",
    "File_create",
    "ruxen_file_create",
    "File_append",
    "ruxen_file_append",
    "File_open_options",
    "ruxen_file_open_options",
    "File_metadata",
    "ruxen_file_metadata",
    "File_read",
    "ruxen_file_read",
    "File_read_to_string",
    "ruxen_file_read_to_string",
    "File_read_all",
    "ruxen_file_read_all",
    "File_write",
    "ruxen_file_write",
    "File_write_all",
    "ruxen_file_write_all",
    "File_write_str",
    "ruxen_file_write_str",
    "File_flush",
    "ruxen_file_flush",
    "File_seek",
    "ruxen_file_seek",
    "File_close",
    "ruxen_file_close",
    "OpenOptions_new",
    "ruxen_open_options_new",
    "OpenOptions_read",
    "ruxen_open_options_read",
    "OpenOptions_write",
    "ruxen_open_options_write",
    "OpenOptions_append",
    "ruxen_open_options_append",
    "OpenOptions_truncate",
    "ruxen_open_options_truncate",
    "OpenOptions_create",
    "ruxen_open_options_create",
    "OpenOptions_create_new",
    "ruxen_open_options_create_new",
    "Mutex_lock_raw",
    "Mutex_try_lock_raw",
    "ruxen_mutex_lock",
    "ruxen_mutex_try_lock",
];

/// The non-`_init` members of the old `borrows_first_arg` rule
/// (drops.rs:968-984). The `_init` suffix stays as an `ends_with` check in
/// `callee_ownership`.
const FREE_FAMILY: &[&str] = &[
    "ruxen_dealloc",
    "ruxen_string_free",
    "ruxen_vec_free",
    "ruxen_vec_drop_string",
    "ruxen_vec_drop_vec",
    "ruxen_hash_free",
    "ruxen_set_free",
    "ruxen_hash_drop_string_v",
    "ruxen_hash_drop_v_string",
    "ruxen_hash_drop_string_string",
    "ruxen_hash_drop_v_vec",
    "ruxen_set_drop_string",
];

/// Old `is_runtime_consume_helper` (drops.rs:1003-1043).
const CONSUME_HELPERS: &[&str] = &[
    "ruxen_dealloc",
    "ruxen_string_free",
    "ruxen_vec_free",
    "ruxen_vec_drop_string",
    "ruxen_vec_drop_vec",
    "ruxen_hash_free",
    "ruxen_set_free",
    "ruxen_hash_drop_string_v",
    "ruxen_hash_drop_v_string",
    "ruxen_hash_drop_string_string",
    "ruxen_hash_drop_v_vec",
    "ruxen_set_drop_string",
    "ruxen_string_into_bytes",
    "String_into_bytes",
    "ruxen_vec_from_iter",
    "Vec_from_iter",
];

/// Does this callee MOVE the ownership of its pointer args into an FFI-owned
/// runtime structure (the task scheduler / a spawned OS thread)? The single
/// source of truth, backed by [`MOVE_BY_FFI`]. `drops.rs`'s `moved_to_ffi`
/// collection consults this instead of re-listing the symbols inline, so the
/// drop-pass exclusion set and the `callee_ownership` borrow verdict can never
/// disagree again (they previously did: the inline list carried a dead dotted
/// `"Task.spawn_raw"` spelling the table never had).
pub fn is_move_by_ffi(callee: &str) -> bool {
    MOVE_BY_FFI.contains(&callee)
}

/// Old `is_move_by_ffi_callee` (drops.rs:1068-1088).
const MOVE_BY_FFI: &[&str] = &[
    "ruxen_executor_spawn",
    "Task_spawn_raw",
    "ruxen_thread_spawn",
    "Thread_spawn",
    "Thread_spawn_raw",
];

/// Old `is_command_builder_method` (drops.rs:1158-1168).
const COMMAND_BUILDER: &[&str] = &[
    "Command_arg",
    "ruxen_command_arg",
    "Command_args",
    "ruxen_command_args",
    "Command_env",
    "ruxen_command_env",
    "Command_current_dir",
    "ruxen_command_current_dir",
];

/// Old `is_command_terminal_or_accessor` (drops.rs:1123-1139).
const COMMAND_TERMINAL_OR_ACCESSOR: &[&str] = &[
    "Command_status",
    "ruxen_command_status",
    "Command_output",
    "ruxen_command_output",
    "Output_status",
    "ruxen_output_status",
    "Output_stdout",
    "ruxen_output_stdout",
    "Output_stderr",
    "ruxen_output_stderr",
    "ExitStatus_code",
    "ruxen_exit_status_code",
    "ExitStatus_success",
    "ruxen_exit_status_success",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_alloc_constructors_classify_as_fresh() {
        assert_eq!(callee_ownership("Vec_new").result, ResultOwnership::Fresh);
        assert_eq!(
            callee_ownership("ruxen_hash_new").result,
            ResultOwnership::Fresh
        );
        assert_eq!(
            callee_ownership("Command_new").result,
            ResultOwnership::Fresh
        );
        assert_eq!(callee_ownership("File_open").result, ResultOwnership::Fresh);
        assert_eq!(
            callee_ownership("Mutex_lock_raw").result,
            ResultOwnership::Fresh
        );
    }

    #[test]
    fn free_family_consumes_first_arg_and_returns_nothing() {
        let o = callee_ownership("ruxen_string_free");
        assert_eq!(o.result, ResultOwnership::None);
        // free helpers BORROW arg0 in the drop-pass sense (the arg loop
        // `continue`s on arg0 so it is NOT tainted) — encoded as the
        // borrows_first_arg flag.
        assert!(o.borrows_first_arg);
    }

    #[test]
    fn vec_push_transfers_arg1() {
        assert_eq!(
            callee_ownership("ruxen_vec_push").arg_transfer,
            ArgMask::single(1)
        );
        assert_eq!(
            callee_ownership("Vec_push").arg_transfer,
            ArgMask::single(1)
        );
    }

    #[test]
    fn hashmap_insert_transfers_arg1_and_arg2() {
        assert_eq!(
            callee_ownership("ruxen_hash_insert").arg_transfer,
            ArgMask::pair(1, 2)
        );
        assert_eq!(
            callee_ownership("HashMap_insert").arg_transfer,
            ArgMask::pair(1, 2)
        );
    }

    #[test]
    fn move_by_ffi_does_not_borrow_args() {
        let o = callee_ownership("ruxen_executor_spawn");
        assert!(!o.args_are_borrowed); // default-taint path runs → spawned future moved
    }

    #[test]
    fn is_move_by_ffi_matches_table_and_rejects_dotted_and_user() {
        // Every member of MOVE_BY_FFI is recognized — this is the single
        // predicate drops.rs's `moved_to_ffi` collection routes through, so
        // the two can no longer diverge.
        for &c in MOVE_BY_FFI {
            assert!(is_move_by_ffi(c), "MOVE_BY_FFI member {c:?} not recognized");
        }
        // The dotted `Task.spawn_raw` form is DEAD: MIR call callees are
        // always underscore-mangled (`Type_method`), so the dotted spelling
        // never reaches a `MirInst::Call.callee`. It is intentionally NOT in
        // the table (and absent from the parity SYMBOLS list).
        assert!(!is_move_by_ffi("Task.spawn_raw"));
        // User callees never move by FFI.
        assert!(!is_move_by_ffi("MyClass_method"));
    }

    #[test]
    fn pointer_store_transfers_arg1() {
        assert_eq!(
            callee_ownership("ruxen_store_ptr").arg_transfer,
            ArgMask::single(1)
        );
    }

    #[test]
    fn user_callee_is_fully_conservative() {
        let o = callee_ownership("MyClass_method");
        assert_eq!(o.result, ResultOwnership::None); // dest tainted
        assert!(!o.args_are_borrowed); // every Use(arg) tainted
        assert!(!o.borrows_first_arg);
    }

    #[test]
    fn static_constructor_union_is_reconciled() {
        // From the method_call.rs fast-path cascade:
        assert!(is_static_constructor("File", "open"));
        assert!(is_static_constructor("Duration", "from_secs"));
        assert!(is_static_constructor("Instant", "now"));
        assert!(is_static_constructor("TcpListener", "bind"));
        assert!(is_static_constructor("TcpStream", "connect"));
        assert!(is_static_constructor("BufReader", "new"));
        assert!(is_static_constructor("BufWriter", "with_capacity"));
        // From util.rs::is_builtin_static_method (the diverged sibling):
        // `String.from` was DELETED from the language — it is no longer a
        // static constructor (borrow→owned is `x.clone`); the negative below
        // pins that.
        assert!(!is_static_constructor("String", "from"));
        assert!(is_static_constructor("String", "from_bytes"));
        assert!(is_static_constructor("Thread", "spawn"));
        assert!(is_static_constructor("Mutex", "new"));
        assert!(is_static_constructor("Arc", "new"));
        assert!(is_static_constructor("SharedSync", "new"));
        // Generic-base handling (Vec[T] → Vec):
        assert!(is_static_constructor("Vec[String]", "with_capacity"));
        assert!(is_static_constructor("HashMap[K, V]", "from_iter"));
        // The universal `new` rule that method_call.rs's is_collection_ctor
        // applies to ANY type:
        assert!(is_static_constructor("AnyUserClass", "new"));
        // Negatives:
        assert!(!is_static_constructor("Vec", "len"));
        assert!(!is_static_constructor("File", "read"));
    }
}
