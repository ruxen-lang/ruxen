//! How a `&var T` crosses the C ABI: the single owner of that rule.
//!
//! A `&var T` of a VALUE type (a String, or a scalar: every integer and float
//! width, `Bool`, `Char`) is the ADDRESS of the slot holding the `T`, so a
//! write in the callee (`n += 1`, `s.push_str`, `s = "x"`) lands in the
//! caller's variable. Every read in the callee loads through the address.
//! For a HANDLE type (classes, structs, enums, arrays, hashes, closures, a
//! type parameter) a `&var T` is the handle word itself: the callee mutates
//! the object through it. A handle `&var` stays a handle because C entry
//! points (`cx: &var Context`, `buf: &var Array[Int]`) and the scheduler's
//! calls into `poll` exchange the bare handle.
//!
//! MIR lowering applies the rule (see `mir::lower` `refmut_slots`): it emits
//! `MirInst::RefMut` only for a slot-addressed `T`, binds a `&var` parameter
//! of such a type as an address whose reads load and whose writes store, and
//! loads a slot-addressed `&var T` call result where its value is read. Both
//! backends and the REPL JIT therefore lower every `RefMut` the same way: the
//! address of the source local's stack slot.
//!
//! [`load_generic_refmut_returns`] applies the rule to FFI functions whose
//! declared return is `&var T` for a type parameter `T`. Such a C function
//! cannot know the concrete `T`, so its contract is to return the slot
//! address; this pass loads the handle through it at every call site whose
//! concrete `T` is a handle type.

use std::collections::HashSet;

use crate::hir::types::Ty;
use crate::mir::nodes::{MirInst, MirProgram};

/// Whether a `&var` of a value of type `ty` is the address of its slot (a
/// String or a scalar) rather than a copy of its handle word (every other
/// type). References are looked through, so `&var &String` answers like
/// `&var String`.
pub fn refmut_is_slot_address(ty: &Ty) -> bool {
    ty.refmut_is_slot_address()
}

/// Whether a declared parameter or return type is a `&var T` whose `T` is
/// slot-addressed: such a parameter receives the address of the caller's
/// storage. Callers and callees both decide on the DECLARED (unsubstituted)
/// type, so a generic `&var T` stays a handle in every monomorphised copy.
pub fn is_slot_refmut(declared: &Ty) -> bool {
    refmut_pointee(declared).is_some_and(refmut_is_slot_address)
}

/// The pointee `T` of a slot-addressed `&var T` type, or `None` when `ty` is
/// not one (see [`is_slot_refmut`]).
pub fn slot_refmut_pointee(ty: &Ty) -> Option<&Ty> {
    refmut_pointee(ty).filter(|t| refmut_is_slot_address(t))
}

/// The pointee of a `&var T` type, or `None` for any other type.
fn refmut_pointee(ty: &Ty) -> Option<&Ty> {
    match ty {
        Ty::RefMut(inner) | Ty::RefMutLifetime(_, inner) => Some(inner),
        _ => None,
    }
}

/// After every call to an FFI function declared `-> &var T` for a type
/// parameter `T`, load the handle through the returned slot address when the
/// call's concrete `T` is a handle type. `MutexGuard[T]#deref_mut` returns
/// `&payload`; for `MutexGuard[Array[Int]]` the caller needs the array
/// pointer stored there, not the address of the mutex's field. For a
/// slot-addressed `T` (`MutexGuard[Int]`) the address is the `&var Int`
/// itself, and MIR lowering loads through it where the value is read.
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
