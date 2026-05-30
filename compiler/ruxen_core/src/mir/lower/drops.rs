use super::*;

/// Insert `MirInst::Drop` instructions for all locals that have Move semantics
/// before every `Terminator::Return` in the function.
///
/// Drops are inserted in **reverse declaration order** (LIFO: last declared,
/// first dropped). We skip:
/// - Copy types (primitives, references, bools, etc.)
/// - Parameters (owned by the caller)
/// - Any local that appears as the value of a `Return` terminator
///   (returning the value, not dropping it).
///
/// Post-lowering fixup that elides use-after-free deallocs caused by
/// the `lower_assign` "pre-reassign drop" path emitting
/// `ruxen_dealloc(R)` before `Assign { dest: R, value: Use(X) }` —
/// when the call that produced `X` was an instance method that
/// returns `self` (i.e. `X` aliases `R`), the dealloc frees the very
/// heap object the assign then re-reads.
///
/// Reproducer:
///
/// ```text
///   def var with_status(c: Int) -> Reply
///     self.status = c
///     self
///   end
///   …
///   var r = Reply.new
///   r = r.with_status(201)   // ← UAF: dealloc(r) before re-assign of r
/// ```
///
/// MIR pattern (consecutive instructions in one block):
///
/// ```text
///   Call { dest: Some(X), callee: F, args: [Use(R), …] }
///   Call { dest: None,    callee: "ruxen_dealloc", args: [Use(R)] }
///   Assign { dest: R, value: Use(X) }
/// ```
///
/// where `F` is a function whose final terminator is
/// `Return(Some(Use(params[0])))` — i.e. it returns its self
/// argument. Action: remove the middle `ruxen_dealloc` instruction.
///
/// We can't fix this in `lower_assign` itself because the callee's
/// "returns self" property isn't known until that callee's body has
/// been lowered, and lowering order is per-function. Running this
/// as a post-pass over the whole `MirProgram` sidesteps the ordering.
pub fn elide_returns_self_realloc(mir: &mut crate::mir::nodes::MirProgram) {
    use crate::mir::nodes::{MirInst, MirValue, Terminator};

    // Step 1: collect the names of functions that return their first
    // parameter (i.e. instance methods that yield `self`).
    let mut returns_self: std::collections::HashSet<String> = std::collections::HashSet::new();
    for func in &mir.functions {
        let Some(&self_id) = func.params.first() else {
            continue;
        };
        // Any block ending in `Return(Some(Use(self_id)))` counts. If
        // there's at least one such block AND every non-trivial return
        // returns self, the function unconditionally returns self —
        // but for the dealloc-elide we only need to know the return
        // ALIASES self on at least one tail. To be conservative we
        // require ALL returning blocks to yield self.
        let mut saw_return = false;
        let mut all_return_self = true;
        for block in &func.blocks {
            if let Terminator::Return(Some(MirValue::Use(local))) = &block.terminator {
                saw_return = true;
                if *local != self_id {
                    all_return_self = false;
                    break;
                }
            }
        }
        if saw_return && all_return_self {
            returns_self.insert(func.name.clone());
        }
    }

    if returns_self.is_empty() {
        return;
    }

    // Step 2: walk every function's blocks, locate the bad triple,
    // and delete the middle (ruxen_dealloc) instruction.
    for func in &mut mir.functions {
        for block in &mut func.blocks {
            // Track each local's provenance: the name of the last
            // callee that wrote it (or `<indirect>` for closure /
            // fn-pointer dispatch), alongside the set of LocalIds
            // that could alias the return value (i.e. any arg the
            // callee might have returned). For named calls we use
            // `returns_self` to decide; for indirect calls we
            // conservatively assume the callee might return any of
            // its user-visible args.
            const INDIRECT_SENTINEL: &str = "<indirect>";
            // Map dest_local → (callee_name, candidate_alias_args).
            let mut last_call: std::collections::HashMap<
                crate::mir::nodes::LocalId,
                (String, Vec<crate::mir::nodes::LocalId>),
            > = std::collections::HashMap::new();

            let mut to_remove: Vec<usize> = Vec::new();
            for (i, inst) in block.instructions.iter().enumerate() {
                match inst {
                    MirInst::Call { dest, callee, args } => {
                        if let Some(d) = dest {
                            let candidates: Vec<crate::mir::nodes::LocalId> = args
                                .iter()
                                .filter_map(|a| match a {
                                    MirValue::Use(l) => Some(*l),
                                    _ => None,
                                })
                                .collect();
                            last_call.insert(*d, (callee.clone(), candidates));
                        }
                        // Detect: ruxen_dealloc(R) followed by
                        // Assign R = X where X came from a callee
                        // that aliases R via one of its args.
                        if callee == "ruxen_dealloc" && args.len() == 1 {
                            if let MirValue::Use(r) = &args[0] {
                                if let Some(MirInst::Assign {
                                    dest: a_dest,
                                    value: MirValue::Use(x),
                                }) = block.instructions.get(i + 1)
                                {
                                    if a_dest == r {
                                        if let Some((callee_name, candidates)) = last_call.get(x) {
                                            let aliases = candidates.contains(r);
                                            let could_return_self = returns_self
                                                .contains(callee_name)
                                                || callee_name == INDIRECT_SENTINEL;
                                            if aliases && could_return_self {
                                                to_remove.push(i);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    MirInst::CallIndirect {
                        dest: Some(d),
                        args,
                        ..
                    } => {
                        // Indirect call through a closure / fn pointer.
                        // ABI: args[0] is the captures_ptr the closure
                        // lowering prepends; args[1..] are the user's
                        // visible arguments. We can't know which (if
                        // any) the closure returns, so mark every
                        // user arg as a potential alias and tag with
                        // INDIRECT_SENTINEL — worst case is a bounded
                        // one-time leak, not a use-after-free.
                        let candidates: Vec<crate::mir::nodes::LocalId> = args
                            .iter()
                            .skip(1) // skip captures_ptr
                            .filter_map(|a| match a {
                                MirValue::Use(l) => Some(*l),
                                _ => None,
                            })
                            .collect();
                        last_call.insert(*d, (INDIRECT_SENTINEL.to_string(), candidates));
                    }
                    MirInst::Assign {
                        dest,
                        value: MirValue::Use(src),
                    } => {
                        if let Some(entry) = last_call.get(src).cloned() {
                            last_call.insert(*dest, entry);
                        }
                    }
                    _ => {}
                }
            }
            // Remove in reverse so earlier indices stay valid.
            for &i in to_remove.iter().rev() {
                block.instructions.remove(i);
            }
        }
    }
}

pub(super) fn insert_drops(
    func: &mut MirFunction,
    return_locals: &HashSet<LocalId>,
    symbols: &SymbolTable,
    user_drop_classes: &HashSet<String>,
    resolve_ffi_alias_callee: &dyn Fn(String) -> String,
) {
    // Avoid recursing into a class's own `drop` method: if `Holder_drop`
    // takes `self: Holder`, drop-elaboration on `self` would emit a call
    // back to `Holder_drop`, recursing forever at runtime.
    let in_user_drop_method = func
        .name
        .strip_suffix("_drop")
        .map(|prefix| user_drop_classes.contains(prefix))
        .unwrap_or(false);

    // Build a set of parameter locals to skip.
    let param_set: HashSet<LocalId> = func.params.iter().copied().collect();

    // Determine which locals' value provenance is a fresh allocation that
    // this frame owns exclusively. We need this set BEFORE building
    // `drop_locals` because String / Vec / HashMap intermediates that are
    // technically compiler temporaries (`_t…` names) can still own heap
    // (e.g. the implicit `ruxen_string_from` wrap inserted around a string
    // literal whose owner is an outer `String.from(_)` call). Without
    // dropping such intermediates, every interpolated literal leaks the
    // owned-string copy of itself.
    let dealloc_safe = compute_dealloc_safe_locals(func);

    // Locals handed to FFI calls that take ownership must not be
    // dropped at scope exit even when they are constructor temps rather
    // than user `let` bindings. Rondo's accept loop exercises this:
    // `Task.spawn_raw(RondoConnectionTask.new(...))` lowers to a temp
    // allocation passed to `ruxen_executor_spawn`; the scheduler owns
    // that Future after the call.
    let mut moved_to_ffi: HashSet<LocalId> = HashSet::new();
    for block in &func.blocks {
        for inst in &block.instructions {
            if let MirInst::Call { callee, args, .. } = inst {
                let moves_args = matches!(
                    callee.as_str(),
                    "ruxen_executor_spawn"
                        | "Task_spawn_raw"
                        | "Task.spawn_raw"
                        | "ruxen_thread_spawn"
                        | "Thread_spawn"
                        | "Thread_spawn_raw"
                );
                if moves_args {
                    for arg in args {
                        if let MirValue::Use(local) = arg {
                            moved_to_ffi.insert(*local);
                        }
                    }
                }
            }
        }
    }

    // Locals whose pointer transitively flows into a `Return` value via
    // `Assign`/`Copy`/`Move`. The dealloc-safety analysis processes blocks
    // in linear order, so a forward edge such as `block 4: Assign 2 = 5`
    // (where `5` is initialized later in `block 5`) leaves `2` tainted
    // and `5` alloc-rooted. Without this fixpoint, dropping `5` at the
    // common return block would free the value the caller is about to
    // read through `2`. We compute the set iteratively from the seed
    // `return_locals` and exclude every member from `drop_locals` below.
    let return_alias_chain = compute_return_alias_chain(func, return_locals);

    // Collect locals that need dropping: Move types, not params, not the
    // return value. Collect in declaration order.
    //
    // We always drop user-declared locals (`let` bindings) of an owning
    // type. Compiler-generated temporaries (`_t0`, `_t1`, …) are dropped
    // only when:
    //   * the temp's type is a built-in heap-owning type
    //     (`String`, `Vec`, `HashMap`), AND
    //   * the dealloc-safety analysis classified the temp as alloc-rooted
    //     (its value chain traces to a fresh-alloc callee like
    //     `ruxen_string_from` / `ruxen_vec_new` / `ruxen_hash_new`, with
    //     no aliasing or transfer).
    //
    // Class / Struct / Enum temps are still skipped: they almost always
    // arise from `MirInst::Alloc` followed by an `Assign` to a named
    // local, and the dealloc-safety pass already moves ownership to the
    // named local — dropping the temp would double-free.
    let drop_locals: Vec<LocalId> = func
        .locals
        .iter()
        .filter(|local| {
            // Must be a Move type. Primitives, references, tuples-of-Copy
            // etc. never reach MirInst::Alloc, so excluding them here is
            // safe. User-defined structs/classes/enums are heap-allocated
            // by codegen even when their fields qualify them for the
            // implicit `Copy` include (ruby-naming.spec.md §3.6); the
            // Copy include affects copy-on-assign semantics, not the
            // heap-allocation strategy. They still need scope-exit drop.
            let is_user_aggregate = matches!(
                local.ty,
                Ty::Class { .. } | Ty::Struct { .. } | Ty::Enum { .. }
            );
            if !is_user_aggregate && ty_is_effectively_copy(&local.ty, symbols) {
                return false;
            }
            // Must not be a parameter.
            if param_set.contains(&local.id) {
                return false;
            }
            // Must not be the return value (any block) or alias to one.
            if return_locals.contains(&local.id) || return_alias_chain.contains(&local.id) {
                return false;
            }
            // Must not be an allocation whose ownership moved into an
            // FFI-owned runtime structure, such as the task scheduler.
            if moved_to_ffi.contains(&local.id) {
                return false;
            }
            // Drop types that own heap memory:
            //   * Class/Struct/Enum — always heap-allocated via `ruxen_alloc`.
            //   * String/Vec/HashMap — heap-allocated via the built-in
            //     constructors (`ruxen_string_from`, `ruxen_vec_new`,
            //     `ruxen_hash_new`). Each gets a dedicated free helper at
            //     drop time (see drop callee dispatch below).
            //
            // The dealloc-safety analysis (`compute_dealloc_safe_locals`)
            // is what guards against double-free for locals whose value
            // came from a non-fresh source (e.g. a function-call return
            // pointing into the caller's heap).
            if !matches!(
                local.ty,
                Ty::Class { .. }
                    | Ty::Struct { .. }
                    | Ty::Enum { .. }
                    | Ty::String
                    | Ty::Array(_)
                    | Ty::Map(_, _)
                    | Ty::Set(_)
            ) {
                return false;
            }
            // Compiler temporaries: only drop heap-owning built-ins
            // whose value is a fresh allocation (see comment above).
            // Class temps with a registered `def drop` (user_drop_classes
            // includes the built-in std::process::Command + Output
            // entries for inner-heap cleanup) also need scope-exit drop
            // when alloc-rooted — without this, the chained
            // `Command.new(p).arg(a).status()` pattern would leak the
            // intermediate Command's args/env spines.
            if local.name.starts_with("_t") {
                let is_builtin_heap = matches!(
                    local.ty,
                    Ty::String | Ty::Array(_) | Ty::Map(_, _) | Ty::Set(_)
                );
                if is_builtin_heap && dealloc_safe.contains(&local.id) {
                    return true;
                }
                if let Ty::Class { name, .. } = &local.ty {
                    // Match bare name OR any module-qualified entry
                    // ending in ".<name>" — user_drop_classes uses
                    // qualified names for stdlib classes (see W15 fix
                    // in drop_callees builder below).
                    let suffix = format!(".{}", name);
                    let user_drop_match = user_drop_classes.contains(name)
                        || user_drop_classes.iter().any(|q| q.ends_with(&suffix));
                    if user_drop_match && dealloc_safe.contains(&local.id) {
                        return true;
                    }
                }
                return false;
            }
            // User-named aggregate locals only need scope-exit drop when
            // ownership analysis says this frame still owns the heap
            // allocation. This is not just a synthetic-HIR concern:
            // move-by-FFI calls such as `Task.spawn_raw(fut)` remove
            // `fut` from dealloc_safe because the executor now owns the
            // allocation. Dropping solely because the local has a class
            // type frees the queued Future before the scheduler polls it.
            if is_user_aggregate {
                return dealloc_safe.contains(&local.id);
            }
            true
        })
        .map(|local| local.id)
        .collect();

    if drop_locals.is_empty() {
        return;
    }

    // Pre-compute, for each drop-eligible local, the user-drop class name
    // so we can emit `{ClassName}_drop(self)` immediately before the
    // dealloc + no-op cleanup. Indexed for cheap lookup inside the loop.
    // Build the drop-callee map. For each drop-eligible local, look
    // up its class name in user_drop_classes (bare or qualified) and
    // record the synthesized `{ClassName}_drop` callee. Resolve that
    // callee through `ffi_alias_map` so lib-decl `def drop as
    // "ruxen_..."` aliases reach the right C symbol — without this
    // last step, emitted MirInst::Call would point at an undefined
    // `JoinHandle_drop`-style symbol and the user destructor would
    // silently fail to run while `ruxen_dealloc` still freed memory.
    // That was the failure mode behind rondo's
    // docs/ruxen-issues.md §W15: Thread.spawn JoinHandle was getting
    // bare-dealloc'd, freeing the struct out from under the spawned
    // thread.
    let drop_callees: HashMap<LocalId, String> = drop_locals
        .iter()
        .filter_map(|&id| {
            let local = func.locals.iter().find(|l| l.id == id)?;
            if let Ty::Class { name, .. } = &local.ty {
                let bare_match = user_drop_classes.contains(name);
                let suffix = format!(".{}", name);
                let suffix_match = user_drop_classes.iter().any(|q| q.ends_with(&suffix));
                if bare_match || suffix_match {
                    let mangled = format!("{}_drop", name);
                    let callee = resolve_ffi_alias_callee(mangled);
                    return Some((id, callee));
                }
            }
            None
        })
        .collect();

    // For each block that ends with a Return terminator, insert Drop
    // instructions (in reverse declaration order) before the return.
    for block in &mut func.blocks {
        if matches!(block.terminator, Terminator::Return(_)) {
            // Insert drops in reverse declaration order (LIFO).
            for &local_id in drop_locals.iter().rev() {
                // 1) User-defined `def drop` runs first (if any), so the
                //    body still sees the live allocation. Skip when we're
                //    already lowering that class's own drop method to
                //    avoid infinite self-recursion.
                //
                //    Gate the drop call on `dealloc_safe` so we don't
                //    run the destructor on a value whose ownership has
                //    been transferred (otherwise builder patterns like
                //    `let cmd = Command.new(p); cmd.arg(a)` would
                //    double-`Command_drop` — once on the `arg` return
                //    temp, once on the now-tainted `cmd`). This is also
                //    semantically correct: dropping a moved-from value
                //    should be a no-op everywhere.
                if !in_user_drop_method && dealloc_safe.contains(&local_id) {
                    if let Some(callee) = drop_callees.get(&local_id) {
                        block.instructions.push(MirInst::Call {
                            dest: None,
                            callee: callee.clone(),
                            args: vec![MirValue::Use(local_id)],
                        });
                    }
                }
                // 2) Heap-allocated values need their backing memory
                // freed at scope exit. `MirInst::Drop` itself remains a
                // no-op in both backends — the free call we emit just
                // below is what releases memory.
                //
                // Only emit the free when we can prove the local owns a
                // fresh allocation (see `compute_dealloc_safe_locals`).
                // The callee depends on the local's type:
                //   * Class/Struct/Enum     → `ruxen_dealloc`
                //   * String                → `ruxen_string_free`
                //   * Vec[_]                → `ruxen_vec_free` (spine only)
                //   * HashMap[_, _]         → `ruxen_hash_free` (spine only)
                if dealloc_safe.contains(&local_id) {
                    let local = func
                        .locals
                        .iter()
                        .find(|l| l.id == local_id)
                        .expect("drop_locals references a missing local");
                    let drop_callee = match &local.ty {
                        Ty::String => "ruxen_string_free",
                        // Phase 2 stdlib batch 2 (#03): pick the
                        // element-aware drop helper for Vec types whose
                        // element owns heap. `ruxen_vec_drop_string`
                        // walks slots as `char*` and frees each before
                        // releasing the spine; `ruxen_vec_drop_vec`
                        // walks slots as `RuxenVec*` and recurses one
                        // level. Anything else (primitive elements,
                        // HashMap-of-Vec, deeper nesting) falls back to
                        // the spine-only `ruxen_vec_free`. The deeper
                        // shapes will land alongside the trait-druxen
                        // drop dispatch in #05.
                        Ty::Array(elem) => match elem.as_ref() {
                            Ty::String => "ruxen_vec_drop_string",
                            Ty::Array(_) => "ruxen_vec_drop_vec",
                            _ => "ruxen_vec_free",
                        },
                        // Phase 2 stdlib batch 2 (#04): pick the
                        // element-aware drop helper for HashMap types
                        // whose key and/or value owns heap. The selector
                        // mirrors the Vec one above: a four-way table
                        // over `(K is heap, V is heap)`. Heap-owned in
                        // v1 means String or Vec[_]; the deeper Trie of
                        // nested heap (HashMap-in-HashMap, Set-in-V) is
                        // a follow-up alongside the trait-druxen drop
                        // dispatch in #05 and is documented in
                        // CHANGELOG known limitations.
                        Ty::Map(k, v) => {
                            let k_string = matches!(k.as_ref(), Ty::String);
                            let v_string = matches!(v.as_ref(), Ty::String);
                            let v_vec = matches!(v.as_ref(), Ty::Array(_));
                            match (k_string, v_string, v_vec) {
                                (true, true, _) => "ruxen_hash_drop_string_string",
                                (true, false, _) => "ruxen_hash_drop_string_v",
                                (false, true, _) => "ruxen_hash_drop_v_string",
                                (false, false, true) => "ruxen_hash_drop_v_vec",
                                _ => "ruxen_hash_free",
                            }
                        }
                        // Phase 2 stdlib batch 2 (#04): HashSet[T] —
                        // spine free is `ruxen_set_free`; if T is a
                        // String the per-element drop selector walks
                        // slots before delegating.
                        Ty::Set(elem) => match elem.as_ref() {
                            Ty::String => "ruxen_set_drop_string",
                            _ => "ruxen_set_free",
                        },
                        Ty::Class { .. } | Ty::Struct { .. } | Ty::Enum { .. } => "ruxen_dealloc",
                        // Unreachable: the drop_locals filter above
                        // restricts to exactly the variants matched here.
                        // We use unimplemented! rather than a silent
                        // fallback so a future widening of the filter
                        // surfaces immediately.
                        other => {
                            unimplemented!("insert_drops: no drop callee for type {:?}", other)
                        }
                    };
                    block.instructions.push(MirInst::Call {
                        dest: None,
                        callee: drop_callee.to_string(),
                        args: vec![MirValue::Use(local_id)],
                    });
                }
                block.instructions.push(MirInst::Drop { local: local_id });
            }
        }
    }
}

/// Compute the set of locals whose value provenance is a fresh
/// `MirInst::Alloc` (or a chain of `Assign`/`Copy`/`Move` from one) AND
/// whose ownership of that allocation has not been aliased away to
/// another local or transferred to a callee/aggregate.
///
/// These are the locals safe to pass to `ruxen_dealloc` at scope exit:
/// freeing them releases an allocation that this frame owns exclusively.
///
/// A local is excluded (NOT dealloc-safe) when:
///
///   * Its provenance is not a fresh `Alloc` — for example `GetField` /
///     `GetPayload` (pointer into a parent struct), a function-call
///     return value (caller-owned semantics), `Ref`/`RefMut`, or numeric
///     /string literal output.
///   * Ownership escapes the current frame: the local is passed as a
///     `Use(local)` argument to any `Call`/`CallIndirect`, or stored
///     into another aggregate via `SetField`. After such a transfer the
///     callee/aggregate may have copied or freed the pointer, so a
///     scope-exit dealloc could double-free or use-after-free.
///   * Its value flows (via `Assign`/`Copy`/`Move`) into another local.
///     The MIR doesn't deep-copy structs/classes/enums on assignment —
///     the destination receives the same pointer — so dealloc'ing both
///     would double-free the shared allocation. We move the dealloc
///     responsibility to the last local in each propagation chain.
fn compute_dealloc_safe_locals(func: &MirFunction) -> std::collections::HashSet<LocalId> {
    use std::collections::HashSet;

    // `alloc_rooted`: locals whose value currently traces back to a fresh
    // `MirInst::Alloc` and that have not yet been aliased away or
    // transferred. We process instructions in a single forward pass over
    // a flattened block order; an instruction may both *grant* a local
    // alloc-rooted status (e.g. `Alloc dest=L`) and *revoke* it (e.g.
    // `Use(L)` as a Call argument later in the same block).
    let mut alloc_rooted: HashSet<LocalId> = HashSet::new();
    // `tainted_perm`: once a local was passed to a callee/aggregate or
    // its value was propagated to another local, it can never become
    // dealloc-safe. Re-allocations into the same id (rare) would still
    // honor this — we conservatively keep the local out of the safe set.
    let mut tainted_perm: HashSet<LocalId> = HashSet::new();

    // `match_payload_ptrs`: locals produced by `GetPayload` whose source
    // is a `Result`/`Option` value. A `GetField` reading out of such a
    // pointer extracts an OWNED payload (the sum type owned it; the match
    // moves ownership to the binding). We only seed this for Result /
    // Option — NOT user enums — because Result/Option are never themselves
    // drop-elaborated (not in the drop-eligible `Ty` set), so dropping the
    // extracted payload cannot double-free with a container drop; user
    // enums ARE drop-elaborated and their drop recurses into the payload.
    let mut match_payload_ptrs: HashSet<LocalId> = HashSet::new();
    let local_ty = |id: LocalId| func.locals.iter().find(|l| l.id == id).map(|l| &l.ty);

    // Single forward pass — block order is the lowering order, which
    // matches program execution order for the cases we care about.
    // Loops/back-edges are not modeled; back-edges only matter when an
    // alloc-rooted local is mutated mid-loop, which the lowerer
    // currently never produces for user `let` bindings of Class/Struct
    // /Enum types.
    for block in &func.blocks {
        for inst in &block.instructions {
            match inst {
                MirInst::Alloc { dest, .. } | MirInst::StackAlloc { dest, .. } => {
                    if !tainted_perm.contains(dest) {
                        alloc_rooted.insert(*dest);
                    }
                }
                MirInst::Assign { dest, value } => {
                    if let MirValue::Use(src) = value {
                        if alloc_rooted.contains(src) && !tainted_perm.contains(dest) {
                            // Pointer-copy: dest now aliases src's
                            // allocation. Hand dealloc responsibility to
                            // dest by tainting src permanently.
                            tainted_perm.insert(*src);
                            alloc_rooted.remove(src);
                            alloc_rooted.insert(*dest);
                            continue;
                        }
                    }
                    // Literal or non-alloc-rooted source → permanently
                    // exclude dest. (We also drop any prior alloc-root
                    // status, since the local is being overwritten.)
                    tainted_perm.insert(*dest);
                    alloc_rooted.remove(dest);
                }
                MirInst::Copy { dest, src } | MirInst::Move { dest, src } => {
                    if alloc_rooted.contains(src) && !tainted_perm.contains(dest) {
                        tainted_perm.insert(*src);
                        alloc_rooted.remove(src);
                        alloc_rooted.insert(*dest);
                    } else {
                        tainted_perm.insert(*dest);
                        alloc_rooted.remove(dest);
                    }
                }
                // Every other instruction that defines a local produces
                // a value that is NOT a fresh allocation owned by this
                // frame — taint the destination.
                // Payload pointer of a `match` / `?` extraction. The
                // pointer itself is an intermediate (taint it), but if it
                // reads out of a Result/Option we remember it so the
                // GetField that pulls the payload field out knows that
                // field is OWNED (the sum type owned it; the match transfers
                // ownership to the binding). Scoped to Result/Option — see
                // `match_payload_ptrs` doc above.
                MirInst::GetPayload { dest, src, .. } => {
                    if matches!(local_ty(*src), Some(Ty::Result(_, _)) | Some(Ty::Option(_))) {
                        match_payload_ptrs.insert(*dest);
                    }
                    tainted_perm.insert(*dest);
                    alloc_rooted.remove(dest);
                }
                // Field read. Out of a match payload pointer it is an OWNED
                // extraction of a heap-owning value → alloc-root it so the
                // drop-eligibility filter (plus the return-alias / move-out
                // guards) can give it a scope-exit drop; without this a
                // `File` matched out of `File.open(p)` leaks its fd. A field
                // read of anything else is a borrowed view into a parent the
                // frame already owns — tainting keeps us from double-freeing.
                MirInst::GetField { dest, base, .. } => {
                    let owns_heap = matches!(
                        local_ty(*dest),
                        Some(Ty::Class { .. })
                            | Some(Ty::Struct { .. })
                            | Some(Ty::String)
                            | Some(Ty::Array(_))
                            | Some(Ty::Map(_, _))
                            | Some(Ty::Set(_))
                    );
                    if match_payload_ptrs.contains(base)
                        && owns_heap
                        && !tainted_perm.contains(dest)
                    {
                        alloc_rooted.insert(*dest);
                    } else {
                        tainted_perm.insert(*dest);
                        alloc_rooted.remove(dest);
                    }
                }
                MirInst::BinOp { dest, .. }
                | MirInst::Negate { dest, .. }
                | MirInst::Not { dest, .. }
                | MirInst::Compare { dest, .. }
                | MirInst::GetTag { dest, .. }
                | MirInst::Ref { dest, .. }
                | MirInst::RefMut { dest, .. }
                | MirInst::StringLiteral { dest, .. }
                | MirInst::FuncAddr { dest, .. }
                | MirInst::DataAddr { dest, .. } => {
                    // Phase B-5: a static data pointer (vtable /
                    // class_info) is not a heap allocation — taint
                    // the dest so dealloc-tracking treats it as a
                    // non-owning pointer.
                    tainted_perm.insert(*dest);
                    alloc_rooted.remove(dest);
                }
                MirInst::Call { dest, callee, args } => {
                    // Whitelist of callees that produce a fresh, heap
                    // allocation owned exclusively by `dest`. The result
                    // behaves like `MirInst::Alloc { dest }` — the local
                    // can be freed at scope exit by the matching helper.
                    //
                    // Rule: the callee must allocate new heap that is
                    // returned to the caller, with no pre-existing alias
                    // on entry. `ruxen_string_from` / `ruxen_string_concat`
                    // both return a fresh `malloc`'d buffer; `ruxen_vec_new`
                    // and `ruxen_hash_new` return fresh struct heap.
                    // The whitelist covers both the MIR-level mangled
                    // names emitted by the lowerer (e.g. `Vec_new`,
                    // `HashMap_new`, `String_from`) and the runtime-
                    // level names used at internal emit sites (e.g.
                    // `ruxen_string_from` in literal materialisation).
                    // Both forms reach the codegen as `Call { callee }`
                    // and both produce fresh heap.
                    const FRESH_ALLOC_CALLEES: &[&str] = &[
                        "String_from",
                        "Vec_new",
                        "Hash_new",
                        "HashMap_new",
                        "ruxen_string_from",
                        "ruxen_string_concat",
                        "ruxen_vec_new",
                        "ruxen_hash_new",
                        // Phase 2 stdlib batch 2 (#02): `into_bytes`
                        // returns a freshly-allocated Vec[U8] whose
                        // ownership is exclusive to the dest local —
                        // drop-elaborate it like any other Vec.
                        "String_into_bytes",
                        "ruxen_string_into_bytes",
                        // `push` allocates a fresh char buffer and the
                        // surrounding mir lowering rebinds the receiver
                        // local to it; the local must remain alloc-rooted
                        // so scope-exit drop emits `ruxen_string_free`.
                        "String_push",
                        "ruxen_string_push",
                        // Phase 2 stdlib batch 2 (#03): `Vec.from_iter`
                        // takes ownership of the source iter (which IS a
                        // RuxenVec* in v1) and re-emerges as a fresh
                        // owning Vec. Drop-elaborate as a fresh alloc; the
                        // source iter local is tainted via the consume
                        // helper match below.
                        "Vec_from_iter",
                        "ruxen_vec_from_iter",
                        "String_from_iter",
                        "HashMap_from_iter",
                        "Hash_from_iter",
                        "Set_from_iter",
                        "HashSet_from_iter",
                        // Phase 2 stdlib (#05 follow-up): built-in
                        // `collect[Target]` helpers materialise fresh
                        // heap-owned collections / strings from the
                        // eager v1 iterator representation.
                        "ruxen_string_from_iter",
                        "ruxen_hash_from_iter",
                        "ruxen_set_from_iter",
                        // Phase 2 stdlib (#04): HashMap[K,V] / HashSet[T]
                        // surface. Each of these returns a freshly-
                        // allocated heap object whose ownership is the
                        // caller's drop frame. Without this whitelist
                        // the dest local would be tainted on the very
                        // first instruction (default Call rule) and the
                        // scope-exit drop pass would skip the spine
                        // free, leaking the allocation.
                        // The bare constructors are emitted via the
                        // mangled `Hash_new` / `HashMap_new` / `Set_new`
                        // / `HashSet_new` callee names; without them
                        // here the dest local would be tainted by the
                        // default Call rule and the spine would leak.
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
                        // `ruxen_hash_remove` returns a fresh
                        // 16-byte tagged Option allocated via
                        // `ruxen_alloc` — drop-elaborate as a fresh
                        // allocation so the temp is freed on scope exit.
                        "HashMap_remove",
                        "Hash_remove",
                        "ruxen_hash_remove",
                        "HashSet_with_capacity",
                        "Set_with_capacity",
                        "ruxen_set_with_capacity",
                        "HashSet_iter",
                        "Set_iter",
                        "ruxen_set_iter",
                        // Set ops (`union` / `intersection` /
                        // `difference`) each materialise a brand-new
                        // RuxenSet. The two source sets are borrowed.
                        "HashSet_union",
                        "HashSet_intersection",
                        "HashSet_difference",
                        "Set_union",
                        "Set_intersection",
                        "Set_difference",
                        "ruxen_set_union",
                        "ruxen_set_intersection",
                        "ruxen_set_difference",
                        // Phase 2 stdlib (#05 batch 3): `chain(other)` and
                        // `zip(other)` materialise into a fresh
                        // `RuxenVec*` (and, for `zip`, fresh per-pair
                        // tuple cells). The destination local owns the
                        // spine so the scope-exit drop pass must emit
                        // a `ruxen_vec_free` on it. Both source iters
                        // are *borrowed* (not consumed) — they remain
                        // owned by their caller frames, mirroring
                        // `clone` / `take` / `skip`.
                        "ruxen_vec_chain",
                        "ruxen_vec_zip",
                        // Phase 2 stdlib (#06): std::process::Command
                        // builder. `Command_new` allocates a fresh
                        // RuxenCommand. The chained builder methods
                        // (`arg/args/env/current_dir`) mutate-in-place
                        // and return the same pointer — but the source
                        // local is tainted on each call (because
                        // `Command_*` does not match the runtime-
                        // borrow-helper prefix list below), so a
                        // chained `let cmd = Command.new("ls").arg("a")`
                        // ends with `cmd` owning the only live root
                        // and the prior intermediates already tainted.
                        // Without these whitelist entries the dest
                        // local would be default-tainted by the
                        // generic `Call` rule and the scope-exit drop
                        // pass would skip the `Command_drop` we need.
                        //
                        // `Command_status` / `Command_output` are NOT
                        // listed here — they consume the Command (free
                        // it internally) and the dest is a freshly-
                        // allocated `Result[ExitStatus|Output,
                        // IoError]` whose Ok payload owns its own
                        // heap. The Result wrapper is laid out the
                        // same as every other Result return in the
                        // runtime, so the standard drop path freeing
                        // the Result spine + payload applies.
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
                        // Terminals: each returns a fresh Result wrapper
                        // (Ok payload is a freshly-allocated ExitStatus
                        // or Output that owns its own inner heap). The
                        // dest Local must stay alloc-rooted so the
                        // scope-exit drop frees the Result spine
                        // through the standard Class drop pipeline.
                        "Command_status",
                        "ruxen_command_status",
                        "Command_output",
                        "ruxen_command_output",
                        // Output accessors that return fresh heap.
                        // `Output_status` clones the inner ExitStatus
                        // (so the Output can be dropped independently);
                        // `Output_stdout` / `Output_stderr` clone the
                        // captured Strings for the same reason.
                        "Output_status",
                        "ruxen_output_status",
                        "Output_stdout",
                        "ruxen_output_stdout",
                        "Output_stderr",
                        "ruxen_output_stderr",
                        // Phase 2 stdlib (#06.5 T2): File ctors return
                        // a freshly-allocated Result wrapper whose Ok
                        // payload is a fresh RuxenFile owning the fd.
                        // The dest local must stay alloc-rooted so the
                        // scope-exit drop frees the Result spine
                        // through the standard Class drop pipeline.
                        // (File itself is in `user_drop_classes`, so
                        // pattern-bound `f` from `Ok(f) -> ...` is
                        // separately drop-elaborated by the destructure
                        // path.)
                        "File_open",
                        "ruxen_file_open",
                        "File_create",
                        "ruxen_file_create",
                        "File_append",
                        "ruxen_file_append",
                        "File_open_options",
                        "ruxen_file_open_options",
                        // File.metadata returns a fresh Result wrapping
                        // a freshly-allocated 24-byte RuxenMetadata.
                        "File_metadata",
                        "ruxen_file_metadata",
                        // File.read_all returns Result wrapping a fresh
                        // RuxenVec. read_to_string returns Result
                        // wrapping a fresh String. write/read return
                        // Result wrapping an Int (no inner heap on Ok
                        // — still needs alloc-rooted for the Result
                        // spine).
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
                        // OpenOptions builder. `new` allocates a fresh
                        // RuxenOpenOptions; the chained setters mutate
                        // in place and return the same pointer (same
                        // shape as Command builder methods).
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
                        // std::sync — guard handles allocated by lock /
                        // try_lock. The C side malloc's a fresh
                        // `RuxenMutexGuard { parent }` (see
                        // `library/std/sync/runtime/mutex.c::ruxen_mutex_lock`)
                        // — without this whitelist entry the guard local
                        // would be tainted on the call, scope-exit drop
                        // would skip `MutexGuard_drop`, and the pthread
                        // mutex would stay locked forever (second lock
                        // attempt deadlocks). The Mutex's own pointer
                        // (the receiver) is unaffected — the guard is a
                        // separate heap object that wraps a back-pointer.
                        "Mutex_lock_raw",
                        "Mutex_try_lock_raw",
                        "ruxen_mutex_lock",
                        "ruxen_mutex_try_lock",
                    ];
                    let returns_fresh_alloc = FRESH_ALLOC_CALLEES.contains(&callee.as_str());
                    if let Some(d) = dest {
                        if returns_fresh_alloc && !tainted_perm.contains(d) {
                            alloc_rooted.insert(*d);
                        } else {
                            tainted_perm.insert(*d);
                            alloc_rooted.remove(d);
                        }
                    }
                    // Args passed as `Use(L)` may have their ownership
                    // transferred to the callee — taint L by default.
                    // Structural exceptions (callee borrows rather than
                    // consumes its first arg, so the local stays
                    // dealloc-safe afterwards):
                    //
                    //  * `{Type}_init`: constructor pseudo-call that only
                    //    initializes fields of `self` (first arg).
                    //  * `ruxen_dealloc` / `ruxen_string_free` /
                    //    `ruxen_vec_free` / `ruxen_hash_free`: emitted by
                    //    drop elaboration itself; treating their arg as a
                    //    transfer would cancel the free we are about to
                    //    insert.
                    //  * `ruxen_*` runtime helpers (other than the four
                    //    above): in v1 every runtime built-in borrows its
                    //    pointer arguments. `ruxen_vec_push(v, item)`,
                    //    `ruxen_hash_insert(h, k, v)`, `ruxen_string_len(s)`
                    //    and friends mutate or read in place — they never
                    //    free the pointer, so passing a heap-owned local
                    //    must not invalidate its dealloc-safety.
                    //
                    // User-defined callees fall through to the
                    // conservative default and taint every `Use(L)` arg.
                    let borrows_first_arg = callee.ends_with("_init")
                        || callee == "ruxen_dealloc"
                        || callee == "ruxen_string_free"
                        || callee == "ruxen_vec_free"
                        || callee == "ruxen_vec_drop_string"
                        || callee == "ruxen_vec_drop_vec"
                        || callee == "ruxen_hash_free"
                        // Phase 2 stdlib batch 2 (#04): set spine +
                        // hash/set per-element drop selectors are
                        // emitted by the drop pass itself; treating
                        // their first arg as a transfer would cancel
                        // the free we are about to insert.
                        || callee == "ruxen_set_free"
                        || callee == "ruxen_hash_drop_string_v"
                        || callee == "ruxen_hash_drop_v_string"
                        || callee == "ruxen_hash_drop_string_string"
                        || callee == "ruxen_hash_drop_v_vec"
                        || callee == "ruxen_set_drop_string";
                    // A callee borrows its pointer args (does not transfer
                    // ownership) when it is a runtime helper. Two forms
                    // appear in MIR:
                    //
                    //  1. The literal `ruxen_*` runtime symbol (emitted at
                    //     internal lowering sites that bypass the method
                    //     dispatch path).
                    //  2. The MIR-level mangled name (`Vec_push`,
                    //     `HashMap_insert`, `String_len`, …) which is
                    //     translated to a `ruxen_*` symbol by
                    //     `codegen::runtime::runtime_name`. The mangled
                    //     name uses one of the known built-in type prefixes,
                    //     so we recognise it by prefix.
                    //
                    // The four free helpers are excluded — they are the
                    // ONLY runtime helpers that consume their pointer
                    // arg, but the drop pass inserts those itself, so the
                    // analysis must stop before they appear.
                    let is_runtime_consume_helper = matches!(
                        callee.as_str(),
                        "ruxen_dealloc"
                            | "ruxen_string_free"
                            | "ruxen_vec_free"
                            | "ruxen_vec_drop_string"
                            | "ruxen_vec_drop_vec"
                            | "ruxen_hash_free"
                            // Phase 2 stdlib batch 2 (#04): set spine +
                            // hash/set per-element drop helpers consume
                            // their first arg the same way the rest of
                            // the free family does. Excluded from the
                            // borrow-helper analysis below so the drop
                            // pass that emits the call doesn't get the
                            // arg double-tainted.
                            | "ruxen_set_free"
                            | "ruxen_hash_drop_string_v"
                            | "ruxen_hash_drop_v_string"
                            | "ruxen_hash_drop_string_string"
                            | "ruxen_hash_drop_v_vec"
                            | "ruxen_set_drop_string"
                            // Phase 2 stdlib batch 2 (#02): `into_bytes`
                            // is the consuming variant of `bytes` — its
                            // runtime fn frees the source `char*`
                            // internally. Treat it as a transfer so the
                            // drop pass does NOT also emit a
                            // `ruxen_string_free` on the receiver
                            // (which would double-free).
                            | "ruxen_string_into_bytes"
                            | "String_into_bytes"
                            // Phase 2 stdlib batch 2 (#03): `Vec.from_iter`
                            // and `Vec.into_iter` both consume their
                            // source iterator/Vec. The runtime
                            // representation is identity so the destination
                            // owns the same backing array as the source
                            // — taint the source local so scope-exit
                            // drop does not emit a second
                            // `ruxen_vec_free` (double-free).
                            | "ruxen_vec_from_iter"
                            | "Vec_from_iter"
                    );
                    // Move-by-FFI callees: runtime symbols that take
                    // ownership of one or more pointer arguments and
                    // either transfer them to a long-lived structure
                    // (e.g. the executor's task queue) or otherwise
                    // free them internally. They start with the
                    // `ruxen_` prefix and would therefore be matched
                    // by `is_runtime_borrow_helper` below — we must
                    // exclude them so the default-taint path on the
                    // per-arg loop runs and the caller's local is
                    // dropped from `alloc_rooted`. Without this, the
                    // scope-exit drop pass would emit a free on a
                    // pointer the executor still references and the
                    // next pump would deref freed memory.
                    //
                    // Spec: `docs/specs/stdlib/task_spawn.spec.md`
                    // §B10 documents the move semantics. The C-side
                    // contract lives in
                    // `library/std/future/runtime/scheduler.c:150-152`.
                    //
                    // Additions to this list MUST update the matching
                    // `Task.spawn` ownership-gap pin test
                    // (`compiler/ruxen_core/tests/task_spawn_ownership.rs`)
                    // and any analogous fixtures so the move semantics
                    // are exercised end-to-end.
                    let is_move_by_ffi_callee = matches!(
                        callee.as_str(),
                        "ruxen_executor_spawn"
                            | "Task_spawn_raw"
                            // `Thread.spawn(closure)` and `Thread.spawn_raw(closure)`
                            // hand the closure-capture struct to a new
                            // pthread; the captured locals (including
                            // any moved-in TcpStream / handle) outlive
                            // the spawning scope by definition. Without
                            // marking these as move-by-FFI the
                            // accept-loop's scope-exit drop emits a
                            // free on the closure capture (and its
                            // owned fd) while the worker thread is
                            // still reading from it — SIGSEGV on the
                            // first per-connection-thread test.
                            // Mirror of the Task.spawn fix
                            // (`project_ruxen_task_spawn_ownership_gap.md`).
                            | "ruxen_thread_spawn"
                            | "Thread_spawn"
                            | "Thread_spawn_raw"
                    );
                    let is_runtime_borrow_helper = !is_runtime_consume_helper
                        && !is_move_by_ffi_callee
                        && (callee.starts_with("ruxen_")
                            || callee.starts_with("Vec_")
                            || callee.starts_with("Vec[")
                            || callee.starts_with("Array_")
                            || callee.starts_with("Array[")
                            || callee.starts_with("Hash_")
                            || callee.starts_with("Hash[")
                            || callee.starts_with("HashMap_")
                            || callee.starts_with("HashMap[")
                            || callee.starts_with("Map_")
                            || callee.starts_with("Map[")
                            || callee.starts_with("Set_")
                            || callee.starts_with("Set[")
                            || callee.starts_with("HashSet_")
                            || callee.starts_with("HashSet[")
                            || callee.starts_with("String_")
                            || callee.starts_with("&str_"));
                    // Phase 2 stdlib (#06): std::process::Command terminal
                    // methods + Output / ExitStatus accessors borrow
                    // their receiver (the runtime keeps the Command
                    // alive across `.status` / `.output`; the Output
                    // accessors return fresh clones). The builder
                    // methods (`arg/args/env/current_dir`) are NOT in
                    // this list — they mutate-in-place and return the
                    // SAME pointer as the receiver, so we need ownership
                    // to transfer to the dest local to avoid double-
                    // free in the chained pattern
                    // `Command.new(p).arg(a).status()`. Tainting the
                    // receiver on every builder call leaves exactly one
                    // alloc-rooted Command pointer at any point in the
                    // chain — that's the one the scope-exit drop will
                    // free.
                    let is_command_terminal_or_accessor = matches!(
                        callee.as_str(),
                        "Command_status"
                            | "ruxen_command_status"
                            | "Command_output"
                            | "ruxen_command_output"
                            | "Output_status"
                            | "ruxen_output_status"
                            | "Output_stdout"
                            | "ruxen_output_stdout"
                            | "Output_stderr"
                            | "ruxen_output_stderr"
                            | "ExitStatus_code"
                            | "ruxen_exit_status_code"
                            | "ExitStatus_success"
                            | "ruxen_exit_status_success"
                    );
                    let is_runtime_borrow_helper =
                        is_runtime_borrow_helper || is_command_terminal_or_accessor;
                    // Phase 2 stdlib (#06): `Command` builder methods
                    // (`arg/args/env/current_dir`) return the SAME pointer
                    // as the receiver — they mutate `c` in place. The
                    // prefix-based `is_runtime_borrow_helper` rule above
                    // wrongly bundles them in (they start with `ruxen_`),
                    // which leaves the receiver local alloc-rooted
                    // alongside the dest. Both then attract a full
                    // `Command_drop + ruxen_dealloc` at scope exit and
                    // we double-free the same RuxenCommand allocation
                    // (manifests as SIGABRT after `puts` prints
                    // successfully — pin tests
                    // `stdlib_process::command_arg_passes_through`,
                    // `…args_bulk`, `…current_dir_changes_cwd`,
                    // `…output_captures_stdout`, `…stderr`,
                    // `…env_visible_to_child`). Force-taint the
                    // receiver so the dest local is the sole alloc root.
                    let is_command_builder_method = matches!(
                        callee.as_str(),
                        "Command_arg"
                            | "ruxen_command_arg"
                            | "Command_args"
                            | "ruxen_command_args"
                            | "Command_env"
                            | "ruxen_command_env"
                            | "Command_current_dir"
                            | "ruxen_command_current_dir"
                    );
                    let is_runtime_borrow_helper =
                        is_runtime_borrow_helper && !is_command_builder_method;
                    // `ruxen_store_ptr(p, v)` writes `v` into `*p`,
                    // transferring `v`'s allocation through the pointer
                    // to whatever owns `*p`. The store is the moment
                    // ownership leaves this frame, so `v` must be
                    // tainted even though the helper itself starts
                    // with `ruxen_`. Without this special-case the
                    // mir-emitted free at scope-exit on the temp would
                    // race the caller's free on the same pointer
                    // (P0.7 double-free).
                    let is_pointer_store_helper = callee == "ruxen_store_ptr";
                    // Phase 2 stdlib batch 2 (#03): `ruxen_vec_push(v, item)`
                    // is normally a borrow helper (mutates `v` in place,
                    // returns nothing). But for `Vec[String]` /
                    // `Vec[Vec[_]]` / `Vec[HashMap[_,_]]`, the item slot
                    // takes ownership of the heap allocation behind
                    // `item`. Now that `ruxen_vec_drop_string` /
                    // `ruxen_vec_drop_vec` walk those slots and free the
                    // contents at scope exit (#03 batch 2 drop selector
                    // wiring), the pushed temp must be tainted so the
                    // drop pass does NOT also emit a `ruxen_string_free`
                    // / `ruxen_vec_free` on it (double-free).
                    //
                    // Same logic for `ruxen_vec_insert(v, idx, item)` and
                    // `ruxen_hash_insert(h, k, v)` whose value slots also
                    // take ownership.
                    // Match both the un-mangled runtime name and the
                    // MIR-level mangled `Vec[T]_push` form. The mangled
                    // form is what `mangled = format!("{}_{}", type_name,
                    // method_name)` produces (line ~1545); type_name for
                    // a `Vec[String]` receiver is the full `"Vec[String]"`
                    // string. Use `extract_method_name` to pull the
                    // method off the mangled form so we don't have to
                    // enumerate every `Vec[T]` instantiation.
                    // Returns the indices of arguments whose ownership
                    // is transferred by this callee (set of arg
                    // positions). For Vec.push / Vec.insert the slot
                    // is single (1 or 2); for HashMap.insert and
                    // HashSet.insert BOTH the key and the value can
                    // own heap, so we taint each independently. The
                    // dynamic check on element type was considered
                    // and rejected: at this point in the analysis we
                    // don't carry per-arg static types, but tainting
                    // a primitive temp is a no-op (it has no heap to
                    // double-free), so over-tainting is safe.
                    let transfer_indices: &[usize] = {
                        use crate::codegen::runtime::extract_method_name;
                        let m = extract_method_name(callee.as_str());
                        let is_vec_method = callee.starts_with("Vec_")
                            || callee.starts_with("Vec[")
                            || callee.starts_with("Array_")
                            || callee.starts_with("Array[");
                        let is_hash_method = callee.starts_with("Hash_")
                            || callee.starts_with("Hash[")
                            || callee.starts_with("HashMap_")
                            || callee.starts_with("HashMap[")
                            || callee.starts_with("Map_")
                            || callee.starts_with("Map[");
                        let is_set_method = callee.starts_with("Set_")
                            || callee.starts_with("Set[")
                            || callee.starts_with("HashSet_")
                            || callee.starts_with("HashSet[");
                        match (is_vec_method, is_hash_method, is_set_method, m) {
                            (true, _, _, "push") => &[1],
                            (true, _, _, "insert") => &[2],
                            // HashMap.insert(self, K, V) — both K (1)
                            // and V (2) can own heap.
                            (_, true, _, "insert") => &[1, 2],
                            // HashSet.insert(self, T) — T (1) can own
                            // heap.
                            (_, _, true, "insert") => &[1],
                            _ if callee == "ruxen_vec_push" => &[1],
                            _ if callee == "ruxen_vec_insert" => &[2],
                            _ if callee == "ruxen_hash_insert" => &[1, 2],
                            _ if callee == "ruxen_set_insert" => &[1],
                            _ => &[],
                        }
                    };
                    for (idx, arg) in args.iter().enumerate() {
                        if let MirValue::Use(l) = arg {
                            if borrows_first_arg && idx == 0 {
                                continue;
                            }
                            if is_pointer_store_helper && idx == 1 {
                                tainted_perm.insert(*l);
                                alloc_rooted.remove(l);
                                continue;
                            }
                            if transfer_indices.contains(&idx) {
                                tainted_perm.insert(*l);
                                alloc_rooted.remove(l);
                                continue;
                            }
                            if is_runtime_borrow_helper {
                                continue;
                            }
                            tainted_perm.insert(*l);
                            alloc_rooted.remove(l);
                        }
                    }
                }
                MirInst::CallIndirect { dest, args, .. } => {
                    if let Some(d) = dest {
                        tainted_perm.insert(*d);
                        alloc_rooted.remove(d);
                    }
                    for arg in args {
                        if let MirValue::Use(l) = arg {
                            tainted_perm.insert(*l);
                            alloc_rooted.remove(l);
                        }
                    }
                }
                MirInst::SetField { value, .. } => {
                    // Storing a local into another aggregate transfers
                    // ownership (the aggregate now references it).
                    if let MirValue::Use(l) = value {
                        tainted_perm.insert(*l);
                        alloc_rooted.remove(l);
                    }
                }
                // No-op for instructions that don't define or move a
                // local.
                MirInst::SetTag { .. } | MirInst::Drop { .. } | MirInst::Nop => {}
            }
        }
    }

    alloc_rooted
}

/// Compute the transitive set of locals whose value flows into a `Return`
/// terminator via `Assign` / `Copy` / `Move`.
///
/// Seed the set with the locals that appear directly in any
/// `Return(Some(Use(L)))` terminator, then iterate to a fixpoint: every
/// `Assign { dest: D, value: Use(S) }` (and `Copy`/`Move`) where `D` is
/// already in the set adds `S` as well — `S` aliases the same heap as
/// `D`, so freeing it would corrupt the returned value.
///
/// We need this because `compute_dealloc_safe_locals` walks blocks in
/// linear order. A function whose return block precedes the producer
/// block (typical for `match`-arm-as-tail-expression) leaves the
/// intermediate alloc-rooted, which would otherwise be picked up as a
/// drop candidate. Excluding the alias chain keeps the dealloc safe.
fn compute_return_alias_chain(
    func: &MirFunction,
    seed_locals: &HashSet<LocalId>,
) -> HashSet<LocalId> {
    use std::collections::HashSet;
    let mut chain: HashSet<LocalId> = seed_locals.clone();
    loop {
        let before = chain.len();
        for block in &func.blocks {
            for inst in &block.instructions {
                match inst {
                    MirInst::Assign { dest, value } => {
                        if chain.contains(dest) {
                            if let MirValue::Use(src) = value {
                                chain.insert(*src);
                            }
                        }
                    }
                    MirInst::Copy { dest, src } | MirInst::Move { dest, src } => {
                        if chain.contains(dest) {
                            chain.insert(*src);
                        }
                    }
                    _ => {}
                }
            }
        }
        if chain.len() == before {
            break;
        }
    }
    chain
}
