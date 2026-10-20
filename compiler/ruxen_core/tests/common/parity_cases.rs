// The backend parity fixture list, read by both parity lanes:
// `ruxen_core`'s `tests/it/backend_parity.rs` (Cranelift vs LLVM) and
// `ruxen_repl`'s `tests/it/jit_parity.rs` (Cranelift vs the REPL JIT). Each
// lane defines its own `parity_cases!` and `include!`s this file, so a
// fixture added here runs on every backend.
//
// Entry: `test_name => "path under tests/fixtures"`. No lane ignores a
// fixture: every backend matches on all of them.
parity_cases! {
    hello => "hello.rx",
    arithmetic => "arithmetic.rx",
    control_flow => "control_flow.rx",
    functions => "functions.rx",
    enums => "enums.rx",
    enum_data => "enum_data.rx",
    classes => "classes.rx",
    simple_class => "simple_class.rx",
    class_methods => "class_methods.rx",
    string_interp => "string_interp.rx",
    mini_sample => "mini_sample.rx",
    tasklist => "tasklist.rx",
    sample_program => "sample_program.rx",
    drop_owned_struct => "drop_owned_struct.rx",
    // A negative Int32 argument widened to Int sign-extends (-5).
    neg_int32_arg_widened => "parity/neg_int32_arg_widened.rx",
    // Float to integer saturates (clamp, NaN to 0); guards x86_64, where
    // the hardware conversion does not.
    float_to_int_saturates => "parity/float_to_int_saturates.rx",
    // Every backend prints 2: MIR converts the Float argument to the FFI
    // param's declared Int class (`mir::coercion`, F-29 P1b).
    ffi_int_param_takes_float => "parity/ffi_int_param_takes_float.rx",
    // Every backend prints side/in-tap/after-tap: the zero-sized self has
    // no parameter slot, so the block binds the first one (the JIT
    // misbound it before it shared the batch driver, F-29 P2).
    unit_self_kernel_default => "parity/unit_self_kernel_default.rx",
}
