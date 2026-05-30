//! LLVM optimization pass pipeline configuration.

use inkwell::module::Module;
use inkwell::passes::PassBuilderOptions;
use inkwell::targets::TargetMachine;

/// Run the LLVM optimization pipeline on the module.
pub fn run_optimization(
    module: &Module,
    target_machine: &TargetMachine,
    opt_level: u8,
) -> Result<(), String> {
    if opt_level == 0 {
        return Ok(());
    }

    let pass_options = PassBuilderOptions::create();
    pass_options.set_verify_each(cfg!(debug_assertions));
    pass_options.set_loop_interleaving(true);
    pass_options.set_loop_vectorization(true);
    pass_options.set_loop_slp_vectorization(true);
    pass_options.set_merge_functions(true);

    let passes = match opt_level {
        1 => "default<O1>",
        2 => "default<O2>",
        3 => "default<O3>",
        4 => "default<Os>",
        5 => "default<Oz>",
        _ => "default<O2>",
    };

    module
        .run_passes(passes, target_machine, pass_options)
        .map_err(|e| format!("Optimization failed: {}", e.to_string()))
}
