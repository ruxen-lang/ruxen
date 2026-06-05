//! TIER 2 — I/O namespace resolvers (Problem-3 residual only).
//!
//! After the zero-Rust-stdlib migration (Phase B), the `Stdin` / `Stdout`
//! / `Stderr` typing arms and `IoError.message`/`.kind` resolve from
//! `library/std/io/src/{stdin,stdout,stderr,lib}.rx` via the general
//! `DefKind::Method` path (`lookup_method_with_args`) — those arms were
//! deleted.
//!
//! What REMAINS here is the irreducible residual: `BufReader` /
//! `BufWriter` `new` / `with_capacity` carry the **E0714** check that the
//! inner reader/writer is `File` or `TcpStream` — genuine compiler logic
//! computed from `args` + `eng`, not expressible as a static `.rx` return
//! type. (The `.rx` surface models these as module-nested monomorphic
//! variants `BufReader.File` / `BufReader.Tcp` with per-inner C symbols;
//! the resolver keeps the single generic `BufReader[inner]` representation
//! the corpus + downstream typing depend on, plus the E0714 gate.) The
//! BufReader/BufWriter instance methods (`read_line`/`read`/`write`/… /
//! `into_inner`) are ALSO kept here because they read `generic_args` off
//! that resolver-side generic representation.

use crate::diagnostics::Diagnostic;
use crate::hir::types::Ty;

use super::resolver::MethodResolver;
use super::{is_bufio_inner_supported, InferenceEngine};

const CLASS_NAMES: &[&str] = &["BufReader", "BufWriter"];

pub(super) fn resolvers() -> Vec<MethodResolver> {
    vec![MethodResolver {
        matches: |ty, _method| match ty {
            Ty::Class { name, .. } => CLASS_NAMES.contains(&name.as_str()),
            _ => false,
        },
        resolve,
    }]
}

fn resolve(
    eng: &mut InferenceEngine<'_>,
    ty: &Ty,
    method: &str,
    args: &[crate::hir::nodes::HirExpr],
    span: &crate::lexer::token::Span,
) -> Option<Ty> {
    match (ty, method) {
        // Phase 2 #06.5 T6: `std::io::BufReader[R]` / `BufWriter[W]`
        // surface. Static-style constructors (`new` / `with_capacity`)
        // are dispatched through the collection-ctor fast path in
        // mir/lower/expr/method_call.rs alongside File / TcpStream.
        // The inner type R / W is restricted to the closed set
        // {File, TcpStream} — anything else is E0714 here.
        //
        // For `with_capacity(cap: Int, inner: R)` the inner is args[1],
        // for `new(inner: R)` it's args[0]. We pick the right slot
        // below.
        (Ty::Class { name, .. }, "new") if name == "BufReader" => {
            let inner = args
                .first()
                .map(|arg| eng.ctx.resolve(&arg.ty))
                .unwrap_or_else(|| eng.ctx.fresh_type_var());
            if !is_bufio_inner_supported(&inner) {
                eng.diagnostics.push(Diagnostic::error_with_code(
                    format!(
                        "`BufReader.new` requires inner type to be `File` or `TcpStream`; got `{inner}`"
                    ),
                    span.clone(),
                    "E0714",
                ));
                return Some(Ty::Error);
            }
            Some(InferenceEngine::class_ty("BufReader", vec![inner]))
        }
        (Ty::Class { name, .. }, "with_capacity") if name == "BufReader" => {
            let inner = args
                .get(1)
                .map(|arg| eng.ctx.resolve(&arg.ty))
                .unwrap_or_else(|| eng.ctx.fresh_type_var());
            if !is_bufio_inner_supported(&inner) {
                eng.diagnostics.push(Diagnostic::error_with_code(
                    format!(
                        "`BufReader.with_capacity` requires inner type to be `File` or `TcpStream`; got `{inner}`"
                    ),
                    span.clone(),
                    "E0714",
                ));
                return Some(Ty::Error);
            }
            Some(InferenceEngine::class_ty("BufReader", vec![inner]))
        }
        (Ty::Class { name, generic_args }, "read_line") if name == "BufReader" => {
            let _ = generic_args; // shape-only; runtime ignores type param
            Some(Ty::Result(
                Box::new(InferenceEngine::option_ty(Ty::String)),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            ))
        }
        (Ty::Class { name, .. }, "read") if name == "BufReader" => Some(Ty::Result(
            Box::new(Ty::Int),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, generic_args }, "into_inner") if name == "BufReader" => {
            // Surrender the inner — return R directly (not wrapped).
            Some(generic_args.first().cloned().unwrap_or(Ty::Error))
        }
        (Ty::Class { name, .. }, "new") if name == "BufWriter" => {
            let inner = args
                .first()
                .map(|arg| eng.ctx.resolve(&arg.ty))
                .unwrap_or_else(|| eng.ctx.fresh_type_var());
            if !is_bufio_inner_supported(&inner) {
                eng.diagnostics.push(Diagnostic::error_with_code(
                    format!(
                        "`BufWriter.new` requires inner type to be `File` or `TcpStream`; got `{inner}`"
                    ),
                    span.clone(),
                    "E0714",
                ));
                return Some(Ty::Error);
            }
            Some(InferenceEngine::class_ty("BufWriter", vec![inner]))
        }
        (Ty::Class { name, .. }, "with_capacity") if name == "BufWriter" => {
            let inner = args
                .get(1)
                .map(|arg| eng.ctx.resolve(&arg.ty))
                .unwrap_or_else(|| eng.ctx.fresh_type_var());
            if !is_bufio_inner_supported(&inner) {
                eng.diagnostics.push(Diagnostic::error_with_code(
                    format!(
                        "`BufWriter.with_capacity` requires inner type to be `File` or `TcpStream`; got `{inner}`"
                    ),
                    span.clone(),
                    "E0714",
                ));
                return Some(Ty::Error);
            }
            Some(InferenceEngine::class_ty("BufWriter", vec![inner]))
        }
        (Ty::Class { name, .. }, "write") if name == "BufWriter" => Some(Ty::Result(
            Box::new(Ty::Int),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, .. }, "write_all") if name == "BufWriter" => Some(Ty::Result(
            Box::new(Ty::Unit),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, .. }, "write_str") if name == "BufWriter" => Some(Ty::Result(
            Box::new(Ty::Unit),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, .. }, "flush") if name == "BufWriter" => Some(Ty::Result(
            Box::new(Ty::Unit),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, generic_args }, "into_inner") if name == "BufWriter" => {
            // Result[W, IoError] — flush failure surfaces here.
            let inner = generic_args.first().cloned().unwrap_or(Ty::Error);
            Some(Ty::Result(
                Box::new(inner),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            ))
        }
        // Within-namespace fallthrough (not a cross-cutting catch-all).
        _ => None,
    }
}
