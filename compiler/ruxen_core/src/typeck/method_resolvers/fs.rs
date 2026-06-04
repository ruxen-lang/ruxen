//! TIER 2 — filesystem namespace resolvers.
//!
//! `Metadata` / `File` / `OpenOptions` method typing, carved verbatim out
//! of the legacy match. Pure-ish arms (use `eng` only via the static
//! `InferenceEngine::class_ty` helper; no diagnostics).

use crate::hir::types::Ty;

use super::resolver::MethodResolver;
use super::InferenceEngine;

const NAMES: &[&str] = &["Metadata", "File", "OpenOptions"];

pub(super) fn resolvers() -> Vec<MethodResolver> {
    vec![MethodResolver {
        matches: |ty, _method| matches!(ty, Ty::Class { name, .. } if NAMES.contains(&name.as_str())),
        resolve: |_eng, ty, method, _args, _span| match (ty, method) {
            // Phase 2 stdlib (#06): std::fs::Metadata accessors.
            (Ty::Class { name, .. }, "size") if name == "Metadata" => Some(Ty::Int),
            (Ty::Class { name, .. }, "modified") if name == "Metadata" => Some(Ty::Int),
            (Ty::Class { name, .. }, "is_file") if name == "Metadata" => Some(Ty::Bool),
            (Ty::Class { name, .. }, "is_dir") if name == "Metadata" => Some(Ty::Bool),
            (Ty::Class { name, .. }, "is_symlink") if name == "Metadata" => Some(Ty::Bool),
            // Phase 2 stdlib (#06.5 T2): std::io::File static-style
            // constructors. Receiver type is `File` (the class identifier
            // promoted to a type via resolve::IdentifierKind promotion).
            // All return `Result[File, IoError]`.
            (Ty::Class { name, .. }, "open") if name == "File" => Some(Ty::Result(
                Box::new(InferenceEngine::class_ty("File", vec![])),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            )),
            (Ty::Class { name, .. }, "create") if name == "File" => Some(Ty::Result(
                Box::new(InferenceEngine::class_ty("File", vec![])),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            )),
            (Ty::Class { name, .. }, "append") if name == "File" => Some(Ty::Result(
                Box::new(InferenceEngine::class_ty("File", vec![])),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            )),
            (Ty::Class { name, .. }, "open_options") if name == "File" => Some(Ty::Result(
                Box::new(InferenceEngine::class_ty("File", vec![])),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            )),
            // Phase 2 stdlib (#06.5 T2): std::io::File instance methods.
            // Every path that can fail returns `Result[_, IoError]`. The
            // io_error_ty helper would be cleaner but inferring it here
            // matches the existing Command-arm style above.
            (Ty::Class { name, .. }, "read") if name == "File" => Some(Ty::Result(
                Box::new(Ty::Int),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            )),
            (Ty::Class { name, .. }, "read_to_string") if name == "File" => Some(Ty::Result(
                Box::new(Ty::String),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            )),
            (Ty::Class { name, .. }, "read_all") if name == "File" => Some(Ty::Result(
                Box::new(Ty::Array(Box::new(Ty::Int))),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            )),
            (Ty::Class { name, .. }, "write") if name == "File" => Some(Ty::Result(
                Box::new(Ty::Int),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            )),
            (Ty::Class { name, .. }, "write_all") if name == "File" => Some(Ty::Result(
                Box::new(Ty::Unit),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            )),
            (Ty::Class { name, .. }, "write_str") if name == "File" => Some(Ty::Result(
                Box::new(Ty::Unit),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            )),
            (Ty::Class { name, .. }, "flush") if name == "File" => Some(Ty::Result(
                Box::new(Ty::Unit),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            )),
            (Ty::Class { name, .. }, "seek") if name == "File" => Some(Ty::Result(
                Box::new(Ty::Int),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            )),
            (Ty::Class { name, .. }, "metadata") if name == "File" => Some(Ty::Result(
                Box::new(InferenceEngine::class_ty("Metadata", vec![])),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            )),
            (Ty::Class { name, .. }, "close") if name == "File" => Some(Ty::Result(
                Box::new(Ty::Unit),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            )),
            // OpenOptions builder methods — each returns Self.
            (Ty::Class { name, .. }, "read") if name == "OpenOptions" => Some(ty.clone()),
            (Ty::Class { name, .. }, "write") if name == "OpenOptions" => Some(ty.clone()),
            (Ty::Class { name, .. }, "append") if name == "OpenOptions" => Some(ty.clone()),
            (Ty::Class { name, .. }, "truncate") if name == "OpenOptions" => Some(ty.clone()),
            (Ty::Class { name, .. }, "create") if name == "OpenOptions" => Some(ty.clone()),
            (Ty::Class { name, .. }, "create_new") if name == "OpenOptions" => Some(ty.clone()),
            // Within-namespace fallthrough (not a cross-cutting catch-all).
            _ => None,
        },
    }]
}
