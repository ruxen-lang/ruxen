//! Single source of truth for rendering a function/method signature in IDE
//! features. Both `hover` and `signature_help` call [`render`] so they can
//! never drift from each other — or from `ruxen fmt`, since the actual
//! assembly lives in `ruxen_core::formatter::render_fn_signature`.

use ruxen_core::formatter::{render_fn_signature, SigSelf};
use ruxen_core::hir::nodes::HirSelfMode;
use ruxen_core::hir::types::Ty;
use ruxen_core::resolve::symbols::FnSignature;

/// Render the canonical `def …` signature line for `name` with `sig`,
/// matching the formatter. No visibility/`async` prefix — neither is part of
/// the canonical `ruxen fmt` signature shape.
pub fn render(name: &str, sig: &FnSignature) -> String {
    let self_mode = match sig.self_mode {
        Some(HirSelfMode::Ref) => SigSelf::Ref,
        Some(HirSelfMode::RefMut) => SigSelf::RefMut,
        Some(HirSelfMode::Consuming) => SigSelf::Consuming,
        None => SigSelf::None,
    };
    let generics: Vec<String> = sig.generic_params.iter().map(|g| g.name.clone()).collect();
    let params: Vec<(String, String)> = sig
        .params
        .iter()
        .map(|p| (p.name.clone(), p.ty.to_string()))
        .collect();
    // Omit `-> ()` for a Unit return — matches the IDE convention and avoids
    // noise; any concrete return type is shown.
    let ret = sig.return_ty.to_string();
    let return_ty = if matches!(sig.return_ty, Ty::Unit) {
        None
    } else {
        Some(ret.as_str())
    };
    render_fn_signature(
        name,
        self_mode,
        sig.is_class_method,
        &generics,
        &params,
        return_ty,
    )
}
