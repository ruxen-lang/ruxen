//! The descriptors the compiler supplies to C (B73).
//!
//! A slot in a collection is a bare 64-bit word, so the C runtime cannot
//! tell an `Int` from an owned `char*` or a user object by looking at it,
//! and must not try. A `lib` declaration that needs to know asks for it
//! with a trailing parameter of a descriptor class (std/core):
//!
//! | parameter | filled with | used for |
//! |---|---|---|
//! | `ElemDesc[T]` | `drops::elem_desc(T)` | copying a `T` as deep as its drop frees it (`Array#to_h`, `drop`, `chain`) |
//! | `KeyDesc[K]` | `drops::key_desc(K)` | a table key: copied, freed, hashed and compared by it (`Hash.new`, `insert`, `to_set`) |
//!
//! A caller never writes these parameters (resolve leaves them out of the
//! signature typeck checks, `ffi_registration.rs::resolve_ffi_sig`); every call to such a declaration
//! gets them appended here, with the declaration's type parameters bound
//! from the call's arguments (the receiver's `Array[(String, Int)]` binds
//! `T`) and its result (`Hash.new`'s `Hash[String, Int]` binds `K`). No
//! callee or type is named in Rust: the declarations say what they need.

use std::collections::HashMap;

use crate::hir::types::Ty;
use crate::mir::nodes::{MirFunction, MirInst, MirValue};
use crate::resolve::symbols::{Supplied, SymbolTable};

/// The key descriptor of a table whose key type is not known yet
/// (`RUXEN_ELEM_UNSET`): the table takes the descriptor of its first
/// insert, and an insert with none panics.
const KEY_DESC_UNSET: i64 = -1;

/// One `lib` declaration with compiler-supplied parameters: its declared
/// parameter types and result.
#[derive(Debug, Clone)]
struct SuppliedDecl {
    params: Vec<Ty>,
    result: Option<Ty>,
}

/// The `lib` declarations with a compiler-supplied parameter
/// (`SymbolTable::supplied_arg`), by C symbol and Ruxen name.
#[derive(Debug, Clone, Default)]
pub(super) struct SuppliedParams(HashMap<String, SuppliedDecl>);

impl SuppliedParams {
    /// Index the declarations of `libs`; a compiler-supplied parameter
    /// followed by one a caller writes is error E0747.
    pub(super) fn from_libs(
        libs: &[crate::mir::nodes::FfiLib],
        symbols: &SymbolTable,
    ) -> Result<Self, String> {
        let mut decls = HashMap::new();
        for decl in libs.iter().flat_map(|lib| &lib.functions) {
            let supplied: Vec<bool> = decl
                .param_types
                .iter()
                .map(|ty| symbols.supplied_arg(ty).is_some())
                .collect();
            let Some(first) = supplied.iter().position(|s| *s) else {
                continue;
            };
            if supplied[first..].iter().any(|s| !s) {
                return Err(format!(
                    "error[E0747]: `{}` declares a compiler-supplied parameter (`ElemDesc` or \
                     `KeyDesc`) before a parameter its callers pass; compiler-supplied \
                     parameters must come last",
                    decl.ruxen_name
                ));
            }
            let entry = SuppliedDecl {
                params: decl.param_types.clone(),
                result: decl.return_type.clone(),
            };
            for key in [&decl.name, &decl.ruxen_name] {
                decls.insert(key.clone(), entry.clone());
            }
        }
        Ok(SuppliedParams(decls))
    }
}

/// Append, at every call to a declaration in `decls`, the descriptors of
/// its trailing compiler-supplied parameters, with the declaration's type
/// parameters bound from the arguments it was given and from the local its
/// result is stored in.
///
/// An `ElemDesc` of a type still naming a type parameter would be a silent
/// POD, so the call becomes the abstract placeholder that
/// `prune_unmonomorphised_bodies` removes with its (opaque) body or
/// reports. An unbound `KeyDesc` is `KEY_DESC_UNSET`: the table takes its
/// first insert's descriptor (`f(Hash.new)` is keyed by the insert in `f`).
pub(super) fn append_supplied_descriptors(
    func: &mut MirFunction,
    decls: &SuppliedParams,
    symbols: &SymbolTable,
) {
    if decls.0.is_empty() {
        return;
    }
    let head = |ty: &Ty| {
        ty.builtin_head()
            .and_then(|(home, args)| Ty::from_builtin_head(home, &args))
            .unwrap_or_else(|| ty.peel_refs().clone())
    };
    let mut glue_descs: Vec<super::drops::ElemDesc> = Vec::new();
    for block in 0..func.blocks.len() {
        let mut index = 0;
        while index < func.blocks[block].instructions.len() {
            let MirInst::Call { dest, callee, args } = &func.blocks[block].instructions[index]
            else {
                index += 1;
                continue;
            };
            let Some(decl) = decls.0.get(callee) else {
                index += 1;
                continue;
            };
            let mut subst = HashMap::new();
            for (position, (param, arg)) in decl.params.iter().zip(args.iter()).enumerate() {
                let MirValue::Use(local) = arg else { continue };
                let actual = &func.locals[*local as usize].ty;
                // A class `lib` method's receiver is declared as the bare
                // class: bind the class's own generic params to the
                // receiver's arguments.
                if let (0, Ty::Class { name, generic_args }) = (position, param) {
                    if generic_args.is_empty() {
                        receiver_bindings(name, actual, symbols, &mut subst);
                        continue;
                    }
                }
                super::instances::unify_collect_type_params(
                    &head(param),
                    &head(actual),
                    &mut subst,
                );
            }
            if let (Some(result), Some(dest)) = (&decl.result, dest) {
                let actual = &func.locals[*dest as usize].ty;
                super::instances::unify_collect_type_params(
                    &head(result),
                    &head(actual),
                    &mut subst,
                );
            }
            let missing: Vec<(Supplied, Ty)> = decl.params[args.len().min(decl.params.len())..]
                .iter()
                .filter_map(|param| symbols.supplied_arg(param))
                .map(|(supplied, ty)| (supplied, ty.subst_params(&subst)))
                .collect();
            let open = |ty: &Ty| ty.any(&mut |t| matches!(t, Ty::TypeParam { .. } | Ty::Infer(_)));
            if missing
                .iter()
                .any(|(supplied, ty)| *supplied == Supplied::ElemDesc && open(ty))
            {
                let callee = callee.clone();
                if let MirInst::Call { callee: target, .. } =
                    &mut func.blocks[block].instructions[index]
                {
                    *target = super::instances::abstract_callee("ElemDesc", &callee);
                }
                index += 1;
                continue;
            }
            let mut values = Vec::new();
            let mut insts = Vec::new();
            for (supplied, ty) in &missing {
                let desc = match supplied {
                    Supplied::KeyDesc if open(ty) => super::drops::ElemDesc::Word(KEY_DESC_UNSET),
                    Supplied::KeyDesc => super::drops::key_desc(ty, symbols),
                    Supplied::ElemDesc => super::drops::elem_desc(ty, symbols),
                };
                glue_descs.push(desc.clone());
                let mut new_temp = || func.new_temp(Ty::Int);
                values.push(super::drops::elem_desc_value(
                    desc,
                    &mut new_temp,
                    &mut insts,
                ));
            }
            let added = insts.len();
            if let MirInst::Call { args, .. } = &mut func.blocks[block].instructions[index] {
                args.extend(values);
            }
            for (offset, inst) in insts.into_iter().enumerate() {
                func.blocks[block].insert_inst(index + offset, inst, None);
            }
            index += added + 1;
        }
    }
    for desc in &glue_descs {
        super::drops::record_elem_glue(func, desc);
    }
}

/// Bind the generic params `class` declares to the generic args of
/// `receiver` (a builtin head such as `Array[(String, Int)]` or a generic
/// class instance).
fn receiver_bindings(
    class: &str,
    receiver: &Ty,
    symbols: &SymbolTable,
    out: &mut HashMap<String, Ty>,
) {
    let args = match receiver.builtin_head() {
        Some((_, args)) => args,
        None => match receiver.peel_refs() {
            Ty::Class { generic_args, .. } => generic_args.clone(),
            _ => return,
        },
    };
    let Some(params) = symbols
        .type_def(class)
        .and_then(|def| def.kind.generic_params())
    else {
        return;
    };
    for (param, arg) in params.iter().zip(args) {
        out.entry(param.name.clone()).or_insert(arg);
    }
}
