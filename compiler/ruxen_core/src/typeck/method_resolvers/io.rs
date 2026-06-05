//! TIER 2 — I/O namespace resolvers.
//!
//! `Stdin` / `Stdout` / `Stderr` / `BufReader` / `BufWriter` (Class) and
//! `IoError` (Enum) method typing, carved verbatim out of the legacy
//! match. Includes the `is_bufio_inner_supported` E0714 guard +
//! `return Some(Ty::Error)` early-returns for `BufReader`/`BufWriter`
//! `new`/`with_capacity`.

use crate::diagnostics::Diagnostic;
use crate::hir::types::Ty;

use super::resolver::MethodResolver;
use super::{is_bufio_inner_supported, InferenceEngine};

const CLASS_NAMES: &[&str] = &["Stdin", "Stdout", "Stderr", "BufReader", "BufWriter"];

pub(super) fn resolvers() -> Vec<MethodResolver> {
    vec![MethodResolver {
        matches: |ty, _method| match ty {
            Ty::Class { name, .. } => CLASS_NAMES.contains(&name.as_str()),
            Ty::Enum { name, .. } => name == "IoError",
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
        (Ty::Class { name, .. }, "read_line") if name == "Stdin" => Some(Ty::Result(
            Box::new(Ty::String),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, .. }, "read_to_string") if name == "Stdin" => Some(Ty::Result(
            Box::new(Ty::String),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        // Phase 2 stdlib (#06.2): `Stdin.lines()` returns
        // `Vec[Result[String, IoError]]`. v1 simplification of
        // Rust's `BufRead::lines` iterator — every line is read
        // up front (see `ruxen_stdin_lines` in runtime.c). On
        // read failure the vec holds a single Err element.
        (Ty::Class { name, .. }, "lines") if name == "Stdin" => {
            Some(Ty::Array(Box::new(Ty::Result(
                Box::new(Ty::String),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            ))))
        }
        (Ty::Class { name, .. }, "write_str") if name == "Stdout" || name == "Stderr" => {
            Some(Ty::Result(
                Box::new(Ty::Unit),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            ))
        }
        (Ty::Class { name, .. }, "flush") if name == "Stdout" || name == "Stderr" => {
            Some(Ty::Result(
                Box::new(Ty::Unit),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            ))
        }
        // Phase 2 stdlib (#06.1): Stdout / Stderr convenience methods
        // that swallow errors and return `Unit`. Mirror Rust's
        // `print!` / `println!` / `eprint!` / `eprintln!` macros at
        // method-shape level. Use `write_str` + `match` if you need
        // the IoError back.
        (Ty::Class { name, .. }, "print") if name == "Stdout" => Some(Ty::Unit),
        (Ty::Class { name, .. }, "println") if name == "Stdout" => Some(Ty::Unit),
        (Ty::Class { name, .. }, "eprint") if name == "Stderr" => Some(Ty::Unit),
        (Ty::Class { name, .. }, "eprintln") if name == "Stderr" => Some(Ty::Unit),
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
        // Phase 2 #06.5: `IoError` is a tagged enum, not a class.
        // `.message() -> String` dispatches on tag in the runtime
        // (see `ruxen_io_error_get_message` in runtime.c).
        (Ty::Enum { name, .. }, "message") if name == "IoError" => Some(Ty::String),
        // Phase 2 #06.5 T1: `.kind() -> IoErrorKind` returns the
        // discriminant as a sibling 20-unit-variant enum. Lets
        // user code branch on the variant tag without binding the
        // payload. Wired through `ruxen_io_error_kind`.
        (Ty::Enum { name, .. }, "kind") if name == "IoError" => Some(Ty::Enum {
            name: "IoErrorKind".to_string(),
            generic_args: vec![],
        }),
        // Within-namespace fallthrough (not a cross-cutting catch-all).
        _ => None,
    }
}
