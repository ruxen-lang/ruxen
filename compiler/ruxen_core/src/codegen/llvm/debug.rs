//! DWARF debug info generation for the LLVM backend (P0.14).
//!
//! Scope: minimal **line-number** info + per-function scopes. This is enough
//! for a debugger (lldb/gdb) to place breakpoints by function, map a program
//! counter back to a source line, and produce function+line backtraces.
//!
//! Explicitly OUT of scope (documented follow-ups):
//!   * Variable / parameter / local type debug info (`DILocalVariable`,
//!     `create_auto_variable`, `insert_declare_*`). A subroutine type with no
//!     parameter types is emitted, so a debugger sees the function but not its
//!     arguments' types.
//!   * Lexical blocks (`DILexicalBlock`): every location is scoped directly to
//!     the function's `DISubprogram`, so there is no nested block scoping.
//!   * Multi-file attribution. MIR carries no per-function source path, so all
//!     line info is attributed to a single placeholder `DIFile`. Correct
//!     per-file DWARF (user vs stdlib functions) needs a source-path threaded
//!     through MIR lowering into `MirFunction`.
//!
//! Gating lives in `super::CodeGen::compile_program`: debug info is emitted
//! only for unoptimized (`opt_level == 0`), non-wasm builds.

use inkwell::context::Context;
use inkwell::debug_info::{
    AsDIScope, DICompileUnit, DIFile, DIFlags, DIFlagsConstants, DILocation, DISubprogram,
    DWARFEmissionKind, DWARFSourceLanguage, DebugInfoBuilder,
};
use inkwell::module::{FlagBehavior, Module};

use crate::mir::nodes::MirFunction;

/// Per-module DWARF debug-info state for one LLVM compilation.
///
/// Holds the `DebugInfoBuilder` and its compile unit for the lifetime of a
/// single module lowering. [`Self::finalize`] MUST be called before
/// `module.verify()` / object emission so LLVM resolves the builder's
/// temporary metadata nodes.
pub(super) struct DebugCtx<'ctx> {
    builder: DebugInfoBuilder<'ctx>,
    compile_unit: DICompileUnit<'ctx>,
    file: DIFile<'ctx>,
}

impl<'ctx> DebugCtx<'ctx> {
    /// Initialize the debug-info builder + compile unit for `module`, and set
    /// the module flags LLVM requires to *keep* (not strip) debug metadata.
    pub(super) fn new(module: &Module<'ctx>, context: &'ctx Context) -> Self {
        // Without a "Debug Info Version" module flag equal to LLVM's current
        // DEBUG_METADATA_VERSION (3), the verifier silently strips ALL debug
        // info, so no DWARF would ever reach the object file. "Dwarf Version"
        // pins the emitted line-table standard (4 = broad lldb/gdb support).
        let i32t = context.i32_type();
        module.add_basic_value_flag(
            "Debug Info Version",
            FlagBehavior::Warning,
            i32t.const_int(3, false),
        );
        module.add_basic_value_flag(
            "Dwarf Version",
            FlagBehavior::Warning,
            i32t.const_int(4, false),
        );

        // Single-file model (see module docs): one placeholder DIFile carries
        // every function's line info.
        let (builder, compile_unit) = module.create_debug_info_builder(
            /* allow_unresolved         */ true,
            /* language                 */ DWARFSourceLanguage::C,
            /* filename                 */ "<ruxen>",
            /* directory                */ ".",
            /* producer                 */ "ruxen",
            /* is_optimized             */ false,
            /* flags                    */ "",
            /* runtime_ver              */ 0,
            /* split_name               */ "",
            /* kind                     */ DWARFEmissionKind::Full,
            /* dwo_id                   */ 0,
            /* split_debug_inlining     */ false,
            /* debug_info_for_profiling */ false,
            /* sysroot                  */ "",
            /* sdk                      */ "",
        );
        let file = compile_unit.get_file();
        DebugCtx {
            builder,
            compile_unit,
            file,
        }
    }

    /// Create the line-only `DISubprogram` scope for `func`, or `None` when the
    /// function has no source span (compiler-synthesized helpers). The caller
    /// is responsible for attaching it via `FunctionValue::set_subprogram`.
    ///
    /// `is_local_to_unit` should mirror the function's LLVM linkage (internal ⇒
    /// true), an accuracy hint that does not affect verification.
    pub(super) fn function_scope(
        &self,
        func: &MirFunction,
        is_local_to_unit: bool,
    ) -> Option<DISubprogram<'ctx>> {
        let line = func.def_span.as_ref()?.line;
        // Line-info only: a parameter-less subroutine type is enough to place
        // breakpoints and unwind by function+line. Argument/local type debug
        // info is out of scope (see module docs).
        let subroutine_ty =
            self.builder
                .create_subroutine_type(self.file, None, &[], DIFlags::ZERO);
        Some(self.builder.create_function(
            self.compile_unit.as_debug_info_scope(),
            &func.name,
            Some(&func.name),
            self.file,
            line,
            subroutine_ty,
            is_local_to_unit,
            /* is_definition */ true,
            /* scope_line    */ line,
            DIFlags::ZERO,
            /* is_optimized  */ false,
        ))
    }

    /// Build a `DILocation` at `(line, column)` scoped to `scope`.
    pub(super) fn location(
        &self,
        context: &'ctx Context,
        line: u32,
        column: u32,
        scope: DISubprogram<'ctx>,
    ) -> DILocation<'ctx> {
        self.builder
            .create_debug_location(context, line, column, scope.as_debug_info_scope(), None)
    }

    /// Resolve all outstanding debug metadata. MUST run before
    /// `module.verify()` / object emission.
    pub(super) fn finalize(&self) {
        self.builder.finalize();
    }
}
