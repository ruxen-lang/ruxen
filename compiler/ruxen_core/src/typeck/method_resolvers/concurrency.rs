//! TIER 2 — concurrency namespace resolvers.
//!
//! `Future` / `Thread` / `JoinHandle` / `ThreadPanic` / `Mutex` /
//! `MutexGuard` / `Arc` / `SharedSync` method typing, carved verbatim out
//! of the legacy match (`mod.rs:272–477`). Includes the Send-bound
//! construction checks: `Thread.spawn` (E1100 capture check),
//! `Mutex.new` (E1101), `Arc`/`SharedSync.new` (E1102).

use crate::diagnostics::Diagnostic;
use crate::hir::nodes::HirExprKind;
use crate::hir::types::Ty;

use super::resolver::MethodResolver;
use super::InferenceEngine;

/// Receiver class names owned by this namespace.
const NAMES: &[&str] = &[
    "Future",
    "Thread",
    "JoinHandle",
    "ThreadPanic",
    "Mutex",
    "MutexGuard",
    "Arc",
    "SharedSync",
];

pub(super) fn resolvers() -> Vec<MethodResolver> {
    vec![MethodResolver {
        matches: |ty, _method| matches!(ty, Ty::Class { name, .. } if NAMES.contains(&name.as_str())),
        resolve,
    }]
}

fn resolve(
    eng: &mut InferenceEngine<'_>,
    ty: &Ty,
    method: &str,
    args: &[crate::hir::nodes::HirExpr],
    _span: &crate::lexer::token::Span,
) -> Option<Ty> {
    match (ty, method) {
        (Ty::Class { name, generic_args }, "await") if name == "Future" => {
            generic_args.first().cloned()
        }
        (Ty::Class { name, .. }, "spawn") if name == "Thread" => {
            // Spec B6 (send_sync_enforcement.spec.md) — `Thread.spawn`
            // rejects closures whose captures don't satisfy Send (or
            // Sync, for by-ref captures per B7). The check fires only
            // at this construction site; closures used in
            // `Array.each` / `Array.map` / … are unaffected.
            if let Some(arg) = args.first() {
                if let HirExprKind::Closure {
                    captures, is_move, ..
                } = &arg.kind
                {
                    for cap in captures {
                        // The capture's stored `ty` is recorded at
                        // resolve time (control_flow.rs:323) — for
                        // un-annotated `let` bindings that's
                        // `Ty::Infer(_)`. Re-fetch the variable's
                        // current type from the symbol table; typeck
                        // updates it in-place when inferring `let`
                        // bindings (see `update_ty` in
                        // resolve/symbols.rs). Fall back to the
                        // capture's stored ty if no def is registered.
                        let cap_ty = eng
                            .symbols
                            .def_ty(cap.def_id)
                            .map(|t| eng.ctx.resolve(&t))
                            .unwrap_or_else(|| eng.ctx.resolve(&cap.ty));
                        // Class-typed values are moved by default at
                        // the Ruxen level (no `&` was written), even
                        // if the recorded `by_move` flag is false (the
                        // resolver only sets it when an explicit `move`
                        // keyword precedes the closure body). Treat
                        // a non-Copy class capture as by-move for the
                        // Send check; primitives are Copy and Send so
                        // the branch doesn't matter for them.
                        let by_move = cap.by_move
                            || *is_move
                            || matches!(
                                cap_ty,
                                Ty::Class { .. } | Ty::Struct { .. } | Ty::Enum { .. }
                            );
                        let satisfied = if by_move {
                            cap_ty.is_send_with(eng.symbols)
                        } else {
                            // B7 — by-ref capture requires `&T: Send`,
                            // which means `T: Sync`. The Sync auto-derive
                            // walks fields, so a user class without
                            // `include Sync` is still rejected when its
                            // field set isn't all Sync. We use the
                            // existing `is_sync_with` here (the strict
                            // rule on Sync is left as v2 polish).
                            cap_ty.is_sync_with(eng.symbols)
                        };
                        if !satisfied {
                            let note = if by_move {
                                format!(
                                    "captured value `{}` of type `{}` is not `Send`. \
                                     Add `include Send` to the type if it is safe to share across threads.",
                                    cap.name, cap_ty
                                )
                            } else {
                                format!(
                                    "captured value `{}` is held by reference; the closure \
                                     requires `&{}: Send`, which means `{}` must implement `Sync`.",
                                    cap.name, cap_ty, cap_ty
                                )
                            };
                            eng.diagnostics.push(Diagnostic::error_with_code(
                                note,
                                arg.span.clone(),
                                "E1100",
                            ));
                        }
                    }
                }
            }
            let output = args
                .first()
                .and_then(|arg| InferenceEngine::callable_return_ty(&arg.ty))
                .unwrap_or_else(|| eng.ctx.fresh_type_var());
            Some(InferenceEngine::class_ty("JoinHandle", vec![output]))
        }
        (Ty::Class { name, .. }, "current") if name == "Thread" => {
            Some(InferenceEngine::class_ty("Thread", vec![]))
        }
        (Ty::Class { name, .. }, "sleep") if name == "Thread" => Some(Ty::Unit),
        (Ty::Class { name, .. }, "yield_now") if name == "Thread" => Some(Ty::Unit),
        (Ty::Class { name, generic_args }, "join") if name == "JoinHandle" => {
            let output = generic_args.first().cloned().unwrap_or(Ty::Error);
            Some(InferenceEngine::result_ty(output, "ThreadPanic"))
        }
        (Ty::Class { name, generic_args }, "join!") if name == "JoinHandle" => {
            generic_args.first().cloned()
        }
        (Ty::Class { name, .. }, "thread_id") if name == "JoinHandle" => {
            Some(InferenceEngine::class_ty("ThreadId", vec![]))
        }
        (Ty::Class { name, .. }, "id") if name == "Thread" => {
            Some(InferenceEngine::class_ty("ThreadId", vec![]))
        }
        (Ty::Class { name, .. }, "name") if name == "Thread" => {
            Some(InferenceEngine::option_ty(Ty::String))
        }
        (Ty::Class { name, .. }, "message") if name == "ThreadPanic" => Some(Ty::String),
        (Ty::Class { name, .. }, "new") if name == "Mutex" => {
            // The payload-Send check (E1101) MIGRATED to the `.rx` bound
            // `class Mutex[T: Send]` (sync/src/mutex.rx) — Feature B's
            // construction-seam enforcement (`check_constructor_generic_
            // bounds` in typeck/infer/expr.rs) reads that bound and emits
            // E1101 via the preserved-code bridge. This arm only types the
            // constructor now (the payload type into `Mutex[inner]`).
            let inner = args
                .first()
                .map(|arg| arg.ty.clone())
                .unwrap_or_else(|| eng.ctx.fresh_type_var());
            Some(InferenceEngine::class_ty("Mutex", vec![inner]))
        }
        (Ty::Class { name, generic_args }, "lock") if name == "Mutex" => {
            let inner = generic_args.first().cloned().unwrap_or(Ty::Error);
            Some(InferenceEngine::result_ty(
                InferenceEngine::class_ty("MutexGuard", vec![inner]),
                "PoisonError",
            ))
        }
        (Ty::Class { name, generic_args }, "lock!") if name == "Mutex" => {
            let inner = generic_args.first().cloned().unwrap_or(Ty::Error);
            Some(InferenceEngine::class_ty("MutexGuard", vec![inner]))
        }
        (Ty::Class { name, generic_args }, "try_lock") if name == "Mutex" => {
            let inner = generic_args.first().cloned().unwrap_or(Ty::Error);
            Some(InferenceEngine::option_ty(InferenceEngine::class_ty(
                "MutexGuard",
                vec![inner],
            )))
        }
        (Ty::Class { name, generic_args }, "into_inner") if name == "Mutex" => {
            let inner = generic_args.first().cloned().unwrap_or(Ty::Error);
            Some(InferenceEngine::result_ty(inner, "PoisonError"))
        }
        (Ty::Class { name, generic_args }, "deref")
            if name == "MutexGuard" || name == "Arc" || name == "SharedSync" =>
        {
            let inner = generic_args.first().cloned().unwrap_or(Ty::Error);
            Some(Ty::Ref(Box::new(inner)))
        }
        (Ty::Class { name, generic_args }, "deref_mut") if name == "MutexGuard" => {
            let inner = generic_args.first().cloned().unwrap_or(Ty::Error);
            Some(Ty::RefMut(Box::new(inner)))
        }
        // ruby-naming.spec.md §10a: Arc → SharedSync. Internal name
        // kept as alias; new code uses SharedSync.
        (Ty::Class { name, generic_args }, "deref_var") if name == "MutexGuard" => {
            let inner = generic_args.first().cloned().unwrap_or(Ty::Error);
            Some(Ty::RefMut(Box::new(inner)))
        }
        (Ty::Class { name, .. }, "new") if name == "Arc" || name == "SharedSync" => {
            // The payload-Send check (E1102) MIGRATED to the `.rx` bound
            // `class SharedSync[T: Send]` (sync/src/shared_sync.rx) —
            // Feature B's construction-seam enforcement reads that bound
            // and emits E1102 via the preserved-code bridge. `Arc` is the
            // internal back-compat alias for the same class; the owner the
            // bridge sees is whichever name the receiver carries. This arm
            // only types the constructor now.
            let inner = args
                .first()
                .map(|arg| arg.ty.clone())
                .unwrap_or_else(|| eng.ctx.fresh_type_var());
            Some(InferenceEngine::class_ty(name, vec![inner]))
        }
        (Ty::Class { name, .. }, "clone") if name == "Arc" || name == "SharedSync" => {
            Some(ty.clone())
        }
        (Ty::Class { name, .. }, "strong_count") if name == "Arc" || name == "SharedSync" => {
            Some(Ty::USize)
        }
        (Ty::Class { name, .. }, "weak_count") if name == "Arc" || name == "SharedSync" => {
            Some(Ty::USize)
        }
        // Within-namespace fallthrough: a concurrency-named receiver
        // calling a method this namespace doesn't define falls through to
        // the next resolver (NOT a cross-cutting catch-all).
        _ => None,
    }
}
