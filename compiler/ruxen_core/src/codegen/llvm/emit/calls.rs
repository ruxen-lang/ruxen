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
    _program: &MirProgram,
    module: &Module<'ctx>,
    context: &'ctx Context,
) -> Result<FunctionValue<'ctx>, String> {
    // Check if already declared
    if let Some(f) = module.get_function(name) {
        return Ok(f);
    }

    // For inferred-type method calls (?T..._method), search declared functions
    if name.starts_with('?') {
        let method = extract_method_name(name);
        let suffix = format!("_{}", method);
        if let Some(resolved) = find_function_by_suffix(module, &suffix) {
            return Ok(resolved);
        }
    }

    // For generic type parameter methods (T_assign, E_message)
    if let Some(pos) = name.find('_') {
        let prefix = &name[..pos];
        if prefix.len() <= 2 && !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_uppercase())
        {
            let method = &name[pos..]; // includes the _
            if let Some(resolved) = find_function_by_suffix(module, method) {
                return Ok(resolved);
            }
        }
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

/// Find a function whose name ends with the given suffix.
/// Prefers the shortest match (most specific).
pub(super) fn find_function_by_suffix<'ctx>(
    module: &Module<'ctx>,
    suffix: &str,
) -> Option<FunctionValue<'ctx>> {
    let mut best: Option<FunctionValue<'ctx>> = None;
    let mut best_len = usize::MAX;

    let mut func = module.get_first_function();
    while let Some(f) = func {
        let fname = f.get_name().to_str().unwrap_or("");
        if fname.ends_with(suffix) && !fname.starts_with('?') && fname.len() < best_len {
            best = Some(f);
            best_len = fname.len();
        }
        func = f.get_next_function();
    }

    best
}
