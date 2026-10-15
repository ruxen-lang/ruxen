//! How a `&var T` crosses the C ABI: the single owner of that rule.
//!
//! A `&var T` is the ADDRESS of the slot holding `T` only when `T` is a
//! String (a `char*` the callee may reallocate, so the caller must see the
//! new pointer). For every other `T` (Int, Float, Bool, arrays, hashes,
//! classes) a `&var T` has the same word as the value itself: scalars are
//! copied and heap types are already pointers the callee mutates through.
//!
//! Both backends' `RefMut` lowering ask [`refmut_is_slot_address`], and
//! [`load_generic_refmut_returns`] applies the rule to FFI functions whose
//! declared return is `&var T` for a type parameter `T`. Such a C function
//! cannot know the concrete `T`, so its contract is to return the slot
//! address; this pass loads the value through it at every call site whose
//! concrete `T` is not slot-represented. A C function declared with a
//! concrete `&var T` return returns the representation of that `T` itself.

use std::collections::HashSet;

use crate::hir::types::Ty;
use crate::mir::nodes::{MirInst, MirProgram};

/// Whether a `&var` of a value of type `ty` is the address of its slot
/// (String) rather than a copy of its word (every other type). References
/// are looked through, so `&var &String` answers like `&var String`.
pub fn refmut_is_slot_address(ty: &Ty) -> bool {
    ty.is_string_form()
}

/// The pointee of a `&var T` type, or `None` for any other type.
fn refmut_pointee(ty: &Ty) -> Option<&Ty> {
    match ty {
        Ty::RefMut(inner) | Ty::RefMutLifetime(_, inner) => Some(inner),
        _ => None,
    }
}

/// After every call to an FFI function declared `-> &var T` for a type
/// parameter `T`, load the value through the returned slot address when the
/// call's concrete `T` is not slot-represented. `MutexGuard[T]#deref_mut`
/// returns `&payload`; for `MutexGuard[Array[Int]]` the caller needs the
/// array pointer stored there, not the address of the mutex's field.
pub fn load_generic_refmut_returns(mir: &mut MirProgram) {
    let slot_returning: HashSet<String> = mir
        .ffi_libs
        .iter()
        .flat_map(|lib| &lib.functions)
        .filter(|f| {
            f.return_type
                .as_ref()
                .and_then(refmut_pointee)
                .is_some_and(|t| matches!(t, Ty::TypeParam { .. }))
        })
        .flat_map(|f| [f.name.clone(), f.ruxen_name.clone()])
        .collect();
    if slot_returning.is_empty() {
        return;
    }
    for func in &mut mir.functions {
        for b in 0..func.blocks.len() {
            let mut i = 0;
            while i < func.blocks[b].instructions.len() {
                let dest = match &func.blocks[b].instructions[i] {
                    MirInst::Call {
                        dest: Some(dest),
                        callee,
                        ..
                    } if slot_returning.contains(callee) => *dest,
                    _ => {
                        i += 1;
                        continue;
                    }
                };
                let needs_load = refmut_pointee(&func.locals[dest as usize].ty)
                    .is_some_and(|t| !refmut_is_slot_address(t));
                if !needs_load {
                    i += 1;
                    continue;
                }
                let slot = func.new_temp(Ty::Int);
                let block = &mut func.blocks[b];
                if let MirInst::Call { dest: d, .. } = &mut block.instructions[i] {
                    *d = Some(slot);
                }
                let span = block.span_at(i);
                block.insert_inst(
                    i + 1,
                    MirInst::GetField {
                        dest,
                        base: slot,
                        field_index: 0,
                    },
                    span,
                );
                i += 2;
            }
        }
    }
}
