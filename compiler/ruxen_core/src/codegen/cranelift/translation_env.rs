//! Translation environment — holds module-level state needed during
//! instruction translation, split from the `FunctionBuilder` borrow.
//!
//! Extracted from the original monolithic `cranelift.rs` for navigability —
//! the contents are otherwise unchanged.

use std::collections::HashMap;

use cranelift_codegen::ir::types::{self, Type};
use cranelift_codegen::ir::{AbiParam, FuncRef, Signature};
use cranelift_frontend::FunctionBuilder;
use cranelift_module::{DataDescription, FuncId, Linkage, Module};

use super::helpers::clif_signature;
use crate::mir::abi::{CalleeIndex, CalleeKind};

pub struct TranslationEnv<'a, M: Module> {
    pub module: &'a mut M,
    pub declared_fns: &'a mut HashMap<String, FuncId>,
    pub string_data: &'a mut HashMap<String, cranelift_module::DataId>,
    pub string_counter: &'a mut u32,
    /// Every callee's signature by each name a call spells
    /// (`MirProgram::callees`, a `mir::abi::CalleeIndex`). Read-only:
    /// `get_or_declare_func` resolves an FFI alias to its C symbol and
    /// declares an import from it.
    pub callees: &'a CalleeIndex,
    /// Mixin vtables Phase B-5: pre-declared data symbol IDs for
    /// `__rx_vtable_*` and `__rx_classinfo_*`. Read-only —
    /// instruction lowering never declares new entries here. Used by
    /// `MirInst::DataAddr` lowering to take the address of a vtable
    /// / class_info inside a function body.
    pub vtable_data: &'a HashMap<String, cranelift_module::DataId>,
}

impl<'a, M: Module> TranslationEnv<'a, M> {
    /// Create a data section for a null-terminated string literal.
    pub fn create_string_data(&mut self, value: &str) -> Result<cranelift_module::DataId, String> {
        if let Some(&data_id) = self.string_data.get(value) {
            return Ok(data_id);
        }

        let name = format!(".str.{}", *self.string_counter);
        *self.string_counter += 1;

        let data_id = self
            .module
            .declare_data(&name, Linkage::Local, false, false)
            .map_err(|e| format!("Failed to declare string data '{}': {}", name, e))?;

        let mut desc = DataDescription::new();
        let mut bytes = value.as_bytes().to_vec();
        bytes.push(0);
        desc.define(bytes.into_boxed_slice());

        self.module
            .define_data(data_id, &desc)
            .map_err(|e| format!("Failed to define string data '{}': {}", name, e))?;

        self.string_data.insert(value.to_string(), data_id);
        Ok(data_id)
    }

    /// Get or declare a function by name, returning a `FuncRef` usable inside
    /// the current Cranelift function being built.
    ///
    /// A name the callee index knows resolves to its link symbol (an FFI's
    /// Ruxen name or lib-qualified alias reaches its C symbol) and, for an
    /// import not declared yet, is declared from its derived signature. Only
    /// a callee no declaration describes takes its signature from the call.
    pub fn get_or_declare_func(
        &mut self,
        name: &str,
        arg_vals: &[cranelift_codegen::ir::Value],
        has_return: bool,
        builder: &mut FunctionBuilder,
    ) -> Result<FuncRef, String> {
        if let Some(&func_id) = self.declared_fns.get(name) {
            return Ok(self.module.declare_func_in_func(func_id, builder.func));
        }
        if let Some(callee) = self.callees.get(name) {
            if let Some(&func_id) = self.declared_fns.get(&callee.link_name) {
                return Ok(self.module.declare_func_in_func(func_id, builder.func));
            }
            if callee.kind == CalleeKind::Import {
                let sig = clif_signature(self.module.isa().default_call_conv(), &callee.abi);
                let link_name = callee.link_name.clone();
                return self.declare_import(&link_name, &sig, builder);
            }
        }

        // An abstract callee (`?T_method`) has no concrete body; MIR never
        // lets one reach codegen (`prune_unmonomorphised_bodies`).
        if name.starts_with('?') {
            return Err(format!(
                "internal error: abstract callee `{name}` reached codegen (its receiver is \
                 still a generic parameter)"
            ));
        }

        // Fall back: infer signature from call-site.
        let call_conv = self.module.isa().default_call_conv();
        let mut sig = Signature::new(call_conv);
        for val in arg_vals {
            let ty = builder.func.dfg.value_type(*val);
            sig.params.push(AbiParam::new(ty));
        }
        if has_return {
            sig.returns.push(AbiParam::new(types::I64));
        }
        self.declare_import(name, &sig, builder)
    }

    /// A `FuncRef` for a runtime function the compiler emits itself, from
    /// its `residual_abi` signature (or its `.rx` declaration).
    pub fn runtime_func(
        &mut self,
        name: &str,
        builder: &mut FunctionBuilder,
    ) -> Result<FuncRef, String> {
        if self.callees.get(name).is_none() {
            return Err(format!(
                "internal error: runtime function `{name}` has no declared or residual ABI"
            ));
        }
        self.get_or_declare_func(name, &[], false, builder)
    }

    fn declare_import(
        &mut self,
        name: &str,
        sig: &Signature,
        builder: &mut FunctionBuilder,
    ) -> Result<FuncRef, String> {
        let func_id = self
            .module
            .declare_function(name, Linkage::Import, sig)
            .map_err(|e| format!("Failed to declare imported function '{}': {}", name, e))?;
        self.declared_fns.insert(name.to_string(), func_id);
        Ok(self.module.declare_func_in_func(func_id, builder.func))
    }

    /// Declare a C library function with an explicit signature (libm
    /// `fmod`, which no `.rx` block declares).
    pub fn declare_runtime_func(
        &mut self,
        name: &str,
        params: &[Type],
        ret: Option<Type>,
        builder: &mut FunctionBuilder,
    ) -> Result<FuncRef, String> {
        if let Some(&func_id) = self.declared_fns.get(name) {
            return Ok(self.module.declare_func_in_func(func_id, builder.func));
        }
        let mut sig = Signature::new(self.module.isa().default_call_conv());
        sig.params.extend(params.iter().map(|&p| AbiParam::new(p)));
        sig.returns.extend(ret.map(AbiParam::new));
        self.declare_import(name, &sig, builder)
    }
}
