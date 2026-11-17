//! The one rule deriving a declaration's [`ParamMode`]s from its declared
//! signature. Lowering records the result on every `MirFunction` and
//! `FfiFuncDecl` it builds from HIR; passes over MIR read it from there,
//! by callee through [`CalleeModes`], and learn what a call may store into
//! its other arguments from [`call_stores`].

use std::collections::HashMap;

use crate::hir::nodes::HirSelfMode;
use crate::hir::types::Ty;
use crate::mir::lang_intrinsics::runtime_name;
use crate::mir::nodes::{LocalId, MirFunction, MirProgram, MirValue, ParamMode};
use crate::mir::residual_abi::RESIDUAL_RUNTIME_FNS;
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

/// Every callee's declared [`ParamMode`]s, by the names a `Call` spells:
/// the runtime symbols the compiler emits itself (`residual_abi`), then the
/// `.rx` declarations (which win), then the program's functions. Built once
/// per program and read by the loan check and drop elaboration alike.
#[derive(Debug, Clone, Default)]
pub struct CalleeModes {
    by_callee: HashMap<String, Vec<ParamMode>>,
    /// The declared slots that may keep a reference argument
    /// (`DeclaredResult::keeps`), for callees with a declaration.
    keeps: HashMap<String, Vec<(usize, usize)>>,
}

impl CalleeModes {
    pub fn new(mir: &MirProgram) -> Self {
        let mut by_callee: HashMap<String, Vec<ParamMode>> = RESIDUAL_RUNTIME_FNS
            .iter()
            .map(|residual| (residual.name.to_string(), residual.modes.to_vec()))
            .collect();
        let mut keeps = HashMap::new();
        for decl in mir.ffi_libs.iter().flat_map(|lib| &lib.functions) {
            for name in [&decl.name, &decl.ruxen_name] {
                by_callee.insert(name.clone(), decl.param_modes.clone());
                keeps.insert(name.clone(), decl.declared_result.keeps.clone());
            }
        }
        for func in &mir.functions {
            by_callee.insert(func.name.clone(), func.param_modes.clone());
            keeps.insert(func.name.clone(), func.declared_result.keeps.clone());
        }
        CalleeModes { by_callee, keeps }
    }

    /// The slots `callee`'s declaration says may keep a reference
    /// argument; `None` when it has no declaration.
    pub fn keeps(&self, callee: &str) -> Option<&[(usize, usize)]> {
        let linked = runtime_name(callee).unwrap_or(callee);
        [callee, linked]
            .iter()
            .find_map(|name| self.keeps.get(*name))
            .map(Vec::as_slice)
    }

    /// The modes `callee` declares, under its own name or the runtime
    /// symbol it is linked as (`lang_intrinsics::runtime_name`); empty when
    /// it declares none.
    pub fn of(&self, callee: &str) -> &[ParamMode] {
        let linked = runtime_name(callee).unwrap_or(callee);
        [callee, linked]
            .iter()
            .find_map(|name| self.by_callee.get(*name))
            .map_or(&[], Vec::as_slice)
    }

    /// How the call `callee(args)` in `func` passes each argument: as
    /// declared, else as an unknown callee would ([`unknown_param_mode`]).
    /// Field drop glue (recorded by its one emitter) consumes the object.
    pub fn of_call(
        &self,
        func: &MirFunction,
        callee: &str,
        args: &[MirValue],
        symbols: &SymbolTable,
    ) -> Vec<ParamMode> {
        let declared = if func.field_glue_calls.contains_key(callee) {
            &[ParamMode::Move][..]
        } else {
            self.of(callee)
        };
        resolve_modes(declared, func, args, symbols)
    }
}

/// Each argument's mode: `declared`, else [`unknown_param_mode`] of its type.
pub fn resolve_modes(
    declared: &[ParamMode],
    func: &MirFunction,
    args: &[MirValue],
    symbols: &SymbolTable,
) -> Vec<ParamMode> {
    args.iter()
        .enumerate()
        .map(
            |(position, arg)| match (declared.get(position), arg.local()) {
                (Some(&mode), _) => mode,
                (None, Some(local)) => unknown_param_mode(&func.locals[local as usize].ty, symbols),
                (None, None) => ParamMode::Read,
            },
        )
        .collect()
}

/// What a call may store into its other arguments, as `(from, into)`
/// positions: a callee can keep what it takes by value in any argument it
/// takes by reference (`push(x: T)` into an `Array[&String]`, a closure
/// into a field), and an argument it may write (`Mut`) keeps a reference
/// argument it is passed when the slot's declared type may hold a
/// reference to what that argument points at (`keeps`,
/// `DeclaredResult::keeps`: `def init(@r: &String)` stores `r` in a class
/// with a `&String` field; `Hash#remove(key: &K)` keeps nothing). With no
/// declaration (`None`), the argument's own type decides. `scalar` locals
/// keep nothing and give nothing. The one rule the loan check and drop
/// elaboration both follow.
pub fn call_stores(
    func: &MirFunction,
    args: &[MirValue],
    modes: &[ParamMode],
    keeps: Option<&[(usize, usize)]>,
    scalar: impl Fn(LocalId) -> bool,
) -> Vec<(usize, usize)> {
    let locals: Vec<(usize, LocalId)> = args
        .iter()
        .enumerate()
        .filter_map(|(position, arg)| arg.local().map(|local| (position, local)))
        .filter(|&(_, local)| !scalar(local))
        .collect();
    let may_hold = |local: LocalId| {
        func.locals[local as usize]
            .ty
            .may_hold_reference(&mut |_| true)
    };
    // Undeclared: the declared rule over the argument types
    // (`declared_keeps`), every user class counted as able to keep: below
    // the container's own top reference, a reference to what the value
    // points at (a `&var String` keeps none, nor does a bare type
    // parameter, whose instances are checked with their own callees).
    let may_keep = |(from, value): (usize, LocalId), into: usize, container: LocalId| match keeps {
        Some(pairs) => pairs.contains(&(from, into)),
        None => {
            let ty = &func.locals[container as usize].ty;
            let pointees = func.locals[value as usize].ty.pointees();
            ty.deref_ty()
                .unwrap_or(ty)
                .may_hold_reference_to(&pointees, &mut |_| true)
        }
    };
    let mut stores = Vec::new();
    for &(from, value) in &locals {
        for &(into, container) in &locals {
            let kept = match (modes[from], modes[into]) {
                (_, ParamMode::Move) => false,
                (ParamMode::Move, _) => true,
                (ParamMode::Read, ParamMode::Mut) => {
                    may_hold(value) && may_keep((from, value), into, container)
                }
                _ => false,
            };
            if from != into && kept {
                stores.push((from, into));
            }
        }
    }
    stores
}
