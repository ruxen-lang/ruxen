//! Translation environment — holds module-level state needed during
//! instruction translation, split from the `FunctionBuilder` borrow.
//!
//! Extracted from the original monolithic `cranelift.rs` for navigability —
//! the contents are otherwise unchanged.

use std::collections::HashMap;

use cranelift_codegen::ir::types::{self, Type};
use cranelift_codegen::ir::{AbiParam, Signature};
use cranelift_frontend::FunctionBuilder;
use cranelift_module::{DataDescription, FuncId, Linkage, Module};

use crate::codegen::runtime::extract_method_name;

use super::runtime_sigs::runtime_signature;

pub struct TranslationEnv<'a, M: Module> {
    pub module: &'a mut M,
    pub declared_fns: &'a mut HashMap<String, FuncId>,
    pub string_data: &'a mut HashMap<String, cranelift_module::DataId>,
    pub string_counter: &'a mut u32,
    /// Param types of every previously-declared user / FFI fn.  Read-only:
    /// instruction lowering never declares new entries here.  Used so that
    /// `coerce_call_args` can apply the *real* narrow-int signature of
    /// known user fns instead of the default widen-to-i64 fallback.
    pub user_fn_param_tys: &'a HashMap<String, Vec<Type>>,
    /// Mixin vtables Phase B-5: pre-declared data symbol IDs for
    /// `__rx_vtable_*` and `__rx_classinfo_*`. Read-only —
    /// instruction lowering never declares new entries here. Used by
    /// `MirInst::DataAddr` lowering to take the address of a vtable
    /// / class_info inside a function body.
    pub vtable_data: &'a HashMap<String, cranelift_module::DataId>,
}

impl<'a, M: Module> TranslationEnv<'a, M> {
    /// Create a data section for a null-terminated string literal.
    pub fn create_string_data(
        &mut self,
        value: &str,
    ) -> Result<cranelift_module::DataId, String> {
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
    pub fn get_or_declare_func(
        &mut self,
        name: &str,
        arg_vals: &[cranelift_codegen::ir::Value],
        has_return: bool,
        builder: &mut FunctionBuilder,
    ) -> Result<cranelift_codegen::ir::FuncRef, String> {
        if let Some(&func_id) = self.declared_fns.get(name) {
            let func_ref = self.module.declare_func_in_func(func_id, builder.func);
            return Ok(func_ref);
        }

        // TODO (quality review §1.3): the two suffix-fallback paths
        // below ("shortest-name wins") are unsound — any class whose
        // mangled name ends with the same suffix wins by chance for an
        // unresolved generic-typed callee. The proper fix is to
        // monomorphise generic methods at MIR time so the callee name
        // is concrete by codegen entry.
        //
        // Removing the fallback now breaks the sample fixture's
        // `Repository[T: Showable].display_all`'s `item.to_display`
        // call (MIR emits `?T625_to_display`), because mixin-bound
        // generics aren't monomorphised yet. Leaving the suffix
        // fallback in place but tracked.
        //
        // Blast radius (post-Cranelift-share): this `TranslationEnv` is the
        // SHARED codegen path, so the fallback now ships in BOTH backends —
        // the batch `CodeGen` (ObjectModule) and the REPL `JITCodeGen`
        // (JITModule). A fix here corrects both; a regression here breaks both.
        //
        // For inferred-type method calls (?T..._method), search for a
        // declared function whose name ends with _method.
        // Prefer the shortest match to avoid picking e.g.
        // TaskList_find_by_id when we want Task_id.
        if name.starts_with("?") {
            let method = extract_method_name(name);
            let suffix = format!("_{}", method);
            let match_name = self
                .declared_fns
                .keys()
                .filter(|k| k.ends_with(&suffix) && !k.starts_with("?"))
                .min_by_key(|k| k.len())
                .cloned();
            if let Some(resolved) = match_name {
                let func_id = self.declared_fns[&resolved];
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                return Ok(func_ref);
            }
        }

        // For unresolved generic type parameters (e.g., T_assign, E_message),
        // use suffix matching to find the concrete method.
        if !self.declared_fns.contains_key(name) {
            let method = extract_method_name(name);
            let type_prefix = if let Some(pos) = name.find('_') {
                &name[..pos]
            } else {
                ""
            };
            // Match single-letter type params or common generic names
            let is_generic_param =
                type_prefix.len() <= 2 && type_prefix.chars().all(|c| c.is_ascii_uppercase());
            if is_generic_param && !type_prefix.is_empty() {
                let suffix = format!("_{}", method);
                let match_name = self
                    .declared_fns
                    .keys()
                    .filter(|k| {
                        k.ends_with(&suffix) && !k.starts_with("?") && k.len() > suffix.len()
                    })
                    .min_by_key(|k| k.len())
                    .cloned();
                if let Some(resolved) = match_name {
                    let func_id = self.declared_fns[&resolved];
                    let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                    return Ok(func_ref);
                }
            }
        }

        // Try known runtime signatures first.
        if let Some((param_tys, ret_ty)) = runtime_signature(name) {
            return self.declare_runtime_func(name, &param_tys, ret_ty, builder);
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

        let func_id = self
            .module
            .declare_function(name, Linkage::Import, &sig)
            .map_err(|e| format!("Failed to declare imported function '{}': {}", name, e))?;

        self.declared_fns.insert(name.to_string(), func_id);
        let func_ref = self.module.declare_func_in_func(func_id, builder.func);
        Ok(func_ref)
    }

    /// Declare a runtime function with an explicit signature.
    pub fn declare_runtime_func(
        &mut self,
        name: &str,
        params: &[Type],
        ret: Option<Type>,
        builder: &mut FunctionBuilder,
    ) -> Result<cranelift_codegen::ir::FuncRef, String> {
        if let Some(&func_id) = self.declared_fns.get(name) {
            let func_ref = self.module.declare_func_in_func(func_id, builder.func);
            return Ok(func_ref);
        }

        let call_conv = self.module.isa().default_call_conv();
        let mut sig = Signature::new(call_conv);
        for &p in params {
            sig.params.push(AbiParam::new(p));
        }
        if let Some(r) = ret {
            sig.returns.push(AbiParam::new(r));
        }

        let func_id = self
            .module
            .declare_function(name, Linkage::Import, &sig)
            .map_err(|e| format!("Failed to declare runtime function '{}': {}", name, e))?;

        self.declared_fns.insert(name.to_string(), func_id);
        let func_ref = self.module.declare_func_in_func(func_id, builder.func);
        Ok(func_ref)
    }
}
