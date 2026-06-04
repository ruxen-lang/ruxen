//! TIER 2 — process namespace resolvers.
//!
//! `Command` builder / `ExitStatus` / `Output` accessors, carved verbatim
//! out of the legacy match.

use crate::hir::types::Ty;

use super::resolver::MethodResolver;
use super::InferenceEngine;

const NAMES: &[&str] = &["Command", "ExitStatus", "Output"];

pub(super) fn resolvers() -> Vec<MethodResolver> {
    vec![MethodResolver {
        matches: |ty, _method| matches!(ty, Ty::Class { name, .. } if NAMES.contains(&name.as_str())),
        resolve: |_eng, ty, method, _args, _span| match (ty, method) {
            // Phase 2 stdlib (#06): std::process::Command builder.
            // `.arg/.args/.env/.current_dir` return Self (same handle,
            // mutate-in-place — the source local is tainted by the
            // method-call default in `compute_dealloc_safe_locals` so
            // double-free is avoided in chained-let bindings).
            // `.status` / `.output` consume self and return Result.
            (Ty::Class { name, .. }, "arg") if name == "Command" => Some(ty.clone()),
            (Ty::Class { name, .. }, "args") if name == "Command" => Some(ty.clone()),
            (Ty::Class { name, .. }, "env") if name == "Command" => Some(ty.clone()),
            (Ty::Class { name, .. }, "current_dir") if name == "Command" => Some(ty.clone()),
            (Ty::Class { name, .. }, "status") if name == "Command" => Some(Ty::Result(
                Box::new(InferenceEngine::class_ty("ExitStatus", vec![])),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            )),
            (Ty::Class { name, .. }, "output") if name == "Command" => Some(Ty::Result(
                Box::new(InferenceEngine::class_ty("Output", vec![])),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            )),
            // ExitStatus accessors.
            (Ty::Class { name, .. }, "code") if name == "ExitStatus" => Some(Ty::Int),
            (Ty::Class { name, .. }, "success") if name == "ExitStatus" => Some(Ty::Bool),
            // Output accessors. `.status` returns a fresh ExitStatus
            // (cloned in the runtime so the Output can be dropped
            // independently).
            (Ty::Class { name, .. }, "status") if name == "Output" => {
                Some(InferenceEngine::class_ty("ExitStatus", vec![]))
            }
            (Ty::Class { name, .. }, "stdout") if name == "Output" => Some(Ty::String),
            (Ty::Class { name, .. }, "stderr") if name == "Output" => Some(Ty::String),
            // Within-namespace fallthrough (not a cross-cutting catch-all).
            _ => None,
        },
    }]
}
