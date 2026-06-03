//! TIER 2 — `std::fmt::Formatter` namespace resolver.
//!
//! The `Formatter` write surface + read-only spec accessors, carved
//! verbatim out of the legacy match (`mod.rs:278–312`). Pure arms (no
//! `eng`/`args`/`span`).

use crate::hir::types::Ty;

use super::resolver::MethodResolver;

pub(super) fn resolvers() -> Vec<MethodResolver> {
    vec![MethodResolver {
        matches: |ty, _method| matches!(ty, Ty::Class { name, .. } if name == "Formatter"),
        resolve: |_eng, ty, method, _args, _span| match (ty, method) {
            // Phase 2 #06.A3: `std::fmt::Formatter` write surface.
            // `write_str(&str)` and `write_char(Char)` both return
            // `Result[(), FmtError]` — caller chooses to propagate
            // via `?` or match. Phase D wires the runtime semantics;
            // here we only register the typeck contract so user
            // `impl Display` bodies can call `f.write_str("x")` etc.
            // without typeck rejecting the unknown method.
            (Ty::Class { name, .. }, "write_str") if name == "Formatter" => Some(Ty::Result(
                Box::new(Ty::Unit),
                Box::new(Ty::Class {
                    name: "FmtError".to_string(),
                    generic_args: vec![],
                }),
            )),
            (Ty::Class { name, .. }, "write_char") if name == "Formatter" => Some(Ty::Result(
                Box::new(Ty::Unit),
                Box::new(Ty::Class {
                    name: "FmtError".to_string(),
                    generic_args: vec![],
                }),
            )),
            // `len()` returns the current byte count of the accumulated
            // buffer — mirrors `ruxen_fmt_formatter_len` (returns int64_t).
            (Ty::Class { name, .. }, "size") if name == "Formatter" => Some(Ty::Int),
            // Read-only spec accessors that Phase D will use when
            // formatting widths / precision / fill. Optional types
            // because `"#{x}"` (no spec) leaves them all None.
            (Ty::Class { name, .. }, "width") if name == "Formatter" => {
                Some(Ty::Option(Box::new(Ty::USize)))
            }
            (Ty::Class { name, .. }, "precision") if name == "Formatter" => {
                Some(Ty::Option(Box::new(Ty::USize)))
            }
            (Ty::Class { name, .. }, "align") if name == "Formatter" => Some(Ty::Char),
            (Ty::Class { name, .. }, "fill") if name == "Formatter" => Some(Ty::Char),
            _ => None,
        },
    }]
}
