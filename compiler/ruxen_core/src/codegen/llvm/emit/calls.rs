//! Function call dispatch helpers for LLVM emission.
//!
//! Resolves runtime, user-defined, and FFI function references by name,
//! lazily declaring them on the module when necessary.

use super::*;

/// Get or declare a runtime function by name.
pub(super) fn get_or_declare_runtime<'ctx>(
    module: &Module<'ctx>,
    context: &'ctx Context,
    name: &str,
) -> FunctionValue<'ctx> {
    if let Some(f) = module.get_function(name) {
        return f;
    }
    // If not found, declare runtime functions and try again
    runtime_decl::declare_runtime_functions(module, context);
    module.get_function(name).unwrap_or_else(|| {
        // Fallback: declare as ptr -> i64
        let ptr_ty = context.ptr_type(AddressSpace::default());
        let fn_ty = context.i64_type().fn_type(&[ptr_ty.into()], false);
        module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External))
    })
}

/// Get or declare a function (runtime, user-defined, or FFI) by name.
#[allow(clippy::too_many_arguments)]
pub(super) fn get_or_declare_func<'ctx>(
    name: &str,
    arg_vals: &[BasicMetadataValueEnum<'ctx>],
    has_return: bool,
    _func: &MirFunction,
    program: &MirProgram,
    module: &Module<'ctx>,
    context: &'ctx Context,
) -> Result<FunctionValue<'ctx>, String> {
    // Check if already declared
    if let Some(f) = module.get_function(name) {
        return Ok(f);
    }

    // Pass 0 declares every FFI function under its linked C symbol. Mirror
    // Cranelift's two-key `declared_fns` lookup by resolving either the
    // Ruxen-side alias (`String_new`) or a lib-qualified spelling to that
    // existing C-symbol declaration. Synthesized MIR calls do not all pass
    // through the lowerer's alias map, so this backend boundary must not let
    // a residual Ruxen name become a new undefined external symbol.
    if let Some(link_name) = ffi_link_name(program, name) {
        return module.get_function(link_name).ok_or_else(|| {
            format!(
                "FFI function '{}' resolved to C symbol '{}', but the symbol was not declared",
                name, link_name
            )
        });
    }

    // An abstract callee (`?T_method`) has no concrete body; MIR never lets
    // one reach codegen (`prune_unmonomorphised_bodies`).
    if name.starts_with('?') {
        return Err(format!(
            "internal error: abstract callee `{name}` reached codegen (its receiver is \
             still a generic parameter)"
        ));
    }

    // Try runtime functions
    if let Some(f) = module.get_function(name) {
        return Ok(f);
    }

    // Declare runtime function if it's a known one
    runtime_decl::declare_runtime_functions(module, context);
    if let Some(f) = module.get_function(name) {
        return Ok(f);
    }

    // Fallback: infer signature from call-site
    let param_types: Vec<BasicMetadataTypeEnum> = arg_vals
        .iter()
        .map(|a| {
            let bv: BasicValueEnum = (*a)
                .try_into()
                .unwrap_or(context.i64_type().const_int(0, false).into());
            bv.get_type().into()
        })
        .collect();

    let fn_type = if has_return {
        context.i64_type().fn_type(&param_types, false)
    } else {
        context.void_type().fn_type(&param_types, false)
    };

    Ok(module.add_function(name, fn_type, Some(inkwell::module::Linkage::External)))
}

fn ffi_link_name<'a>(program: &'a MirProgram, name: &str) -> Option<&'a str> {
    for lib in &program.ffi_libs {
        for ffi_fn in &lib.functions {
            let qualified_match = !lib.name.is_empty()
                && name
                    .strip_prefix(&lib.name)
                    .and_then(|rest| rest.strip_prefix('_'))
                    == Some(ffi_fn.name.as_str());
            if ffi_fn.ruxen_name == name || qualified_match {
                return Some(&ffi_fn.name);
            }
        }
    }
    None
}

/// B01: per-parameter "declared slot is an erased uniform word" flags for a
/// callee, resolved from the program's declared Ruxen `Ty`s.
///
/// The LLVM twin of `CodeGen::user_fn_param_word_slots`. Argument coercion
/// needs it because an LLVM `i64` param could be either a declared `Int` (a
/// float argument there is Ruxen's implicit numeric narrowing, keep `fptosi`)
/// or an erased `T` (a float argument there is an erasure, bit-reinterpret).
///
/// `None` means "no declaration found" — the compiler-internal residual
/// (`ruxen_vec_get`, `ruxen_hash_index`, …), whose `i64` slots are uniform
/// words by construction, so callers treat a missing entry as "word".
///
/// The linear scan matches this backend's existing `ffi_link_name` lookup;
/// the Cranelift backend precomputes the same information into a map at
/// Pass 0/1, and the two MUST agree on the answer.
pub(super) fn callee_param_word_slots(program: &MirProgram, name: &str) -> Option<Vec<bool>> {
    for lib in &program.ffi_libs {
        for ffi_fn in &lib.functions {
            let qualified_match = !lib.name.is_empty()
                && name
                    .strip_prefix(&lib.name)
                    .and_then(|rest| rest.strip_prefix('_'))
                    == Some(ffi_fn.name.as_str());
            if ffi_fn.name == name || ffi_fn.ruxen_name == name || qualified_match {
                return Some(
                    ffi_fn
                        .param_types
                        .iter()
                        .filter(|t| occupies_abi_slot(t))
                        .map(crate::codegen::is_erased_word_ty)
                        .collect(),
                );
            }
        }
    }
    for f in &program.functions {
        if f.name == name {
            return Some(
                f.params
                    .iter()
                    .filter_map(|&id| f.locals.get(id as usize))
                    .filter(|l| occupies_abi_slot(&l.ty))
                    .map(|l| crate::codegen::is_erased_word_ty(&l.ty))
                    .collect(),
            );
        }
    }
    None
}

/// Whether a `Ty` occupies an ABI parameter slot at all. `Unit` / `Never`
/// are zero-sized and are skipped when a signature is built, so the word
/// flags must skip them identically or every later index would be off by one.
fn occupies_abi_slot(ty: &Ty) -> bool {
    !matches!(ty, Ty::Unit | Ty::Never | Ty::Error | Ty::ConstArg(_))
}
