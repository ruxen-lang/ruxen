//! TIER 2 — networking namespace resolvers.
//!
//! `TcpListener` / `TcpStream` surface, carved verbatim out of the legacy
//! match. Every fallible op returns `Result[_, IoError]`.

use crate::hir::types::Ty;

use super::resolver::MethodResolver;
use super::InferenceEngine;

const NAMES: &[&str] = &["TcpListener", "TcpStream"];

pub(super) fn resolvers() -> Vec<MethodResolver> {
    vec![MethodResolver {
        matches: |ty, _method| matches!(ty, Ty::Class { name, .. } if NAMES.contains(&name.as_str())),
        resolve: |_eng, ty, method, _args, _span| match (ty, method) {
            // Phase 2 stdlib (#06.5 T5): std::net::TcpListener surface.
            // Every fallible op returns `Result[_, IoError]`. Static
            // constructor `bind` (Ty::Class receiver promoted from the
            // class identifier by the resolver) returns
            // `Result[TcpListener, IoError]`.
            (Ty::Class { name, .. }, "bind") if name == "TcpListener" => Some(Ty::Result(
                Box::new(InferenceEngine::class_ty("TcpListener", vec![])),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            )),
            (Ty::Class { name, .. }, "accept") if name == "TcpListener" => Some(Ty::Result(
                Box::new(InferenceEngine::class_ty("TcpStream", vec![])),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            )),
            (Ty::Class { name, .. }, "local_addr") if name == "TcpListener" => Some(Ty::Result(
                Box::new(Ty::String),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            )),
            (Ty::Class { name, .. }, "set_nonblocking") if name == "TcpListener" => {
                Some(Ty::Result(
                    Box::new(Ty::Unit),
                    Box::new(Ty::Enum {
                        name: "IoError".to_string(),
                        generic_args: vec![],
                    }),
                ))
            }
            (Ty::Class { name, .. }, "close") if name == "TcpListener" => Some(Ty::Result(
                Box::new(Ty::Unit),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            )),
            // Phase 2 stdlib (#06.5 T5): std::net::TcpStream surface.
            (Ty::Class { name, .. }, "connect") if name == "TcpStream" => Some(Ty::Result(
                Box::new(InferenceEngine::class_ty("TcpStream", vec![])),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            )),
            (Ty::Class { name, .. }, "read") if name == "TcpStream" => Some(Ty::Result(
                Box::new(Ty::Int),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            )),
            (Ty::Class { name, .. }, "write") if name == "TcpStream" => Some(Ty::Result(
                Box::new(Ty::Int),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            )),
            (Ty::Class { name, .. }, "peer_addr") if name == "TcpStream" => Some(Ty::Result(
                Box::new(Ty::String),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            )),
            (Ty::Class { name, .. }, "shutdown") if name == "TcpStream" => Some(Ty::Result(
                Box::new(Ty::Unit),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            )),
            (Ty::Class { name, .. }, "close") if name == "TcpStream" => Some(Ty::Result(
                Box::new(Ty::Unit),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            )),
            // Phase 2 #06.5 T5 additions: socket read/write timeouts.
            // Both take a `&Duration` and return Result[(), IoError].
            (Ty::Class { name, .. }, "set_read_timeout") if name == "TcpStream" => {
                Some(Ty::Result(
                    Box::new(Ty::Unit),
                    Box::new(Ty::Enum {
                        name: "IoError".to_string(),
                        generic_args: vec![],
                    }),
                ))
            }
            (Ty::Class { name, .. }, "set_write_timeout") if name == "TcpStream" => {
                Some(Ty::Result(
                    Box::new(Ty::Unit),
                    Box::new(Ty::Enum {
                        name: "IoError".to_string(),
                        generic_args: vec![],
                    }),
                ))
            }
            // Within-namespace fallthrough (not a cross-cutting catch-all).
            _ => None,
        },
    }]
}
