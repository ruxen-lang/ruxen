//! MIR → LLVM IR translation.
//!
//! Declares the program's functions, imports and vtable data, then lowers
//! each MIR function through the one MIR walk (`codegen::lower`) with an
//! [`LlvmBuilder`].

use std::collections::HashMap;

use inkwell::context::Context;
use inkwell::module::{Linkage, Module};
use inkwell::values::{GlobalValue, PointerValue};
use inkwell::AddressSpace;

use crate::codegen::lower::lower_function;
use crate::mir::abi::CalleeIndex;
use crate::mir::nodes::*;

use super::debug::DebugCtx;
use super::types::llvm_fn_type;

mod builder;

use builder::LlvmBuilder;

/// Compile all functions in a MIR program into LLVM IR.
///
/// `is_wasm` is `true` when the target machine is `wasm32-*`/`wasm64-*`; in
/// that case every `program.wasm_exports` function gets the LLVM `export_name`
/// string attribute so `wasm-ld` emits it as a host-callable wasm export under
/// its source name, decoupled from any Ruxen name mangling (tier 4.03; ADR
/// `docs/decisions/phase4-no-std-wasm.md`, spec §9 Q6). On non-wasm targets the
/// flag is `false` and `wasm_exports` is ignored.
pub fn compile_program<'ctx>(
    program: &MirProgram,
    module: &Module<'ctx>,
    context: &'ctx Context,
    is_wasm: bool,
    debug: bool,
) -> Result<(), String> {
    // Every callee's signature, built by the last lowering step.
    let callees = &program.callees;

    // Declare every import once: each `.rx`-declared FFI symbol with the
    // signature derived from its declared types, then each compiler-emitted
    // runtime symbol (`residual_abi`) no `lib` block declares.
    //
    // Wasm host imports: we deliberately do NOT pin `wasm-import-module`/
    // `-name` attributes. An explicit import-module forces the symbol to be an
    // import even when a definition exists — which conflicts with FFI bindings
    // whose C runtime IS linked on wasm (e.g. the stdlib `ruxen_string_*` /
    // `ruxen_vec_*` in WASM_RUNTIME_CORE). Instead `wasm-ld --allow-undefined`
    // resolves defined symbols and routes the rest to the default `env`
    // import module, so genuinely host-supplied functions (e.g.
    // `ruxen_canvas_*`) arrive as `env.<symbol>` for the JS host to provide.
    for import in callees.imports() {
        if module.get_function(&import.link_name).is_none() {
            module.add_function(
                &import.link_name,
                llvm_fn_type(&import.abi, context),
                Some(inkwell::module::Linkage::External),
            );
        }
    }

    // Pass 1: declare all user functions
    for func in &program.functions {
        let abi = &callees
            .get(&func.name)
            .expect("the callee index holds every MIR function")
            .abi;
        let fn_type = llvm_fn_type(abi, context);
        // Lowering marks `main` and the `<Mixin>_dynamic_<method>` helpers
        // C runtime code calls by symbol (e.g. the scheduler's
        // `Future_dynamic_poll`) as `MirLinkage::Export`.
        // Tier 4.03 (WASM): a wasm-exported function needs External linkage
        // (Internal would be invisible to `wasm-ld`) plus the `export_name`
        // attribute set below.
        let is_wasm_export = is_wasm && program.wasm_exports.iter().any(|n| n == &func.name);
        let linkage = if func.linkage == MirLinkage::Export || is_wasm_export {
            Some(inkwell::module::Linkage::External)
        } else {
            Some(inkwell::module::Linkage::Internal)
        };
        let llvm_fn = module.add_function(&func.name, fn_type, linkage);
        if is_wasm_export {
            // `export_name` is the LLVM/clang mechanism that emits a wasm
            // export under an exact ASCII name, independent of the LLVM symbol
            // name — so the export survives `--gc-sections` and carries no
            // mangling. Equivalent to C's
            // `__attribute__((export_name("<name>")))`.
            let attr = context.create_string_attribute("wasm-export-name", &func.name);
            llvm_fn.add_attribute(inkwell::attributes::AttributeLoc::Function, attr);
        }
    }

    // Pass 1.5: define runtime-mixin vtable and class-info globals after all
    // method functions have been declared, but before function bodies can
    // lower `DataAddr` references to them. Each global is an array of opaque
    // pointers, so LLVM's target data layout controls pointer size and
    // alignment for both native and wasm targets.
    emit_mixin_vtable_data(program, module, context)?;

    // DWARF debug info (P0.14): create the builder + compile unit once, after
    // all functions are declared (so subprograms can attach to them) and
    // before any body is lowered (so instruction `DILocation`s resolve).
    // `None` when debug info is off (release / wasm), leaving the path inert.
    let debug_ctx = if debug {
        Some(DebugCtx::new(module, context))
    } else {
        None
    };

    // Pass 2: define all user functions
    let mut strings: HashMap<String, GlobalValue<'ctx>> = HashMap::new();

    for func in &program.functions {
        compile_function(
            func,
            callees,
            module,
            context,
            &mut strings,
            debug_ctx.as_ref(),
        )?;
    }

    // Resolve outstanding debug metadata before the caller verifies / emits.
    if let Some(dctx) = &debug_ctx {
        dctx.finalize();
    }

    Ok(())
}

fn emit_mixin_vtable_data<'ctx>(
    program: &MirProgram,
    module: &Module<'ctx>,
    context: &'ctx Context,
) -> Result<(), String> {
    let ptr_ty = context.ptr_type(AddressSpace::default());

    for vtable in &program.vtables {
        let symbol = vtable.symbol();
        if module.get_global(&symbol).is_some() {
            return Err(format!(
                "mixin-vtables: duplicate vtable data symbol '{}'",
                symbol
            ));
        }
        let global = module.add_global(
            ptr_ty.array_type(vtable.method_symbols.len() as u32),
            None,
            &symbol,
        );
        global.set_linkage(Linkage::Internal);
        global.set_constant(true);
    }

    for class_info in &program.class_infos {
        let symbol = class_info.symbol();
        if module.get_global(&symbol).is_some() {
            return Err(format!(
                "mixin-vtables: duplicate class_info data symbol '{}'",
                symbol
            ));
        }
        let global = module.add_global(
            ptr_ty.array_type(class_info.vtable_symbols.len() as u32),
            None,
            &symbol,
        );
        global.set_linkage(Linkage::Internal);
        global.set_constant(true);
    }

    for vtable in &program.vtables {
        let symbol = vtable.symbol();
        let method_ptrs: Result<Vec<PointerValue<'ctx>>, String> = vtable
            .method_symbols
            .iter()
            .map(|method_symbol| {
                module
                    .get_function(method_symbol)
                    .map(|function| function.as_global_value().as_pointer_value())
                    .ok_or_else(|| {
                        format!(
                            "mixin-vtables: method symbol '{}' for vtable '{}' not declared \
                             — required-method check (E1117) should have caught this",
                            method_symbol, symbol
                        )
                    })
            })
            .collect();
        let initializer = ptr_ty.const_array(&method_ptrs?);
        module
            .get_global(&symbol)
            .expect("vtable global was declared above")
            .set_initializer(&initializer);
    }

    for class_info in &program.class_infos {
        let symbol = class_info.symbol();
        let vtable_ptrs: Result<Vec<PointerValue<'ctx>>, String> = class_info
            .vtable_symbols
            .iter()
            .map(|vtable_symbol| {
                module
                    .get_global(vtable_symbol)
                    .map(|global| global.as_pointer_value())
                    .ok_or_else(|| {
                        format!(
                            "mixin-vtables: class_info '{}' references unknown vtable '{}'",
                            symbol, vtable_symbol
                        )
                    })
            })
            .collect();
        let initializer = ptr_ty.const_array(&vtable_ptrs?);
        module
            .get_global(&symbol)
            .expect("class_info global was declared above")
            .set_initializer(&initializer);
    }

    Ok(())
}

/// Lower one MIR function into its declared LLVM function, through the one
/// MIR walk (`codegen::lower`).
fn compile_function<'ctx>(
    func: &MirFunction,
    callees: &CalleeIndex,
    module: &Module<'ctx>,
    context: &'ctx Context,
    strings: &mut HashMap<String, GlobalValue<'ctx>>,
    debug: Option<&DebugCtx<'ctx>>,
) -> Result<(), String> {
    let mut builder = LlvmBuilder::new(func, module, context, callees, strings, debug)?;
    lower_function(func, &mut builder)
}
