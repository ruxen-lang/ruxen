//! The one rule deriving a declaration's [`ParamMode`]s from its declared
//! signature. Lowering records the result on every `MirFunction` and
//! `FfiFuncDecl` it builds from HIR; passes over MIR read it from there.

use crate::hir::nodes::HirSelfMode;
use crate::hir::types::Ty;
use crate::mir::nodes::ParamMode;
use crate::resolve::symbols::{ty_is_effectively_copy, SymbolTable};

/// How a method receiver declared with `mode` is passed.
pub fn receiver_mode(mode: &HirSelfMode) -> ParamMode {
    match mode {
        HirSelfMode::Ref => ParamMode::Read,
        HirSelfMode::RefMut => ParamMode::Mut,
        HirSelfMode::Consuming => ParamMode::Move,
    }
}

/// How a parameter declared as `ty` receives its argument.
pub fn param_mode(ty: &Ty, symbols: &SymbolTable) -> ParamMode {
    if ty.is_mut_ref() {
        ParamMode::Mut
    } else if ty.is_ref() || ty.is_borrowed_block() || ty_is_effectively_copy(ty, symbols) {
        ParamMode::Read
    } else {
        ParamMode::Move
    }
}

/// How a callee whose signature is unknown (an `Fn` bound with no spelled
/// signature) is taken to receive an argument of type `ty`: a shared
/// reference or a Copy value is only read, anything else (a handle, a
/// `&var`) may be written through. Never `Read` for a handle, so an unknown
/// callee cannot hide a mutation from the loan check.
pub fn unknown_param_mode(ty: &Ty, symbols: &SymbolTable) -> ParamMode {
    let read_only = !ty.is_mut_ref() && (ty.is_ref() || ty_is_effectively_copy(ty, symbols));
    if read_only {
        ParamMode::Read
    } else {
        ParamMode::Mut
    }
}
