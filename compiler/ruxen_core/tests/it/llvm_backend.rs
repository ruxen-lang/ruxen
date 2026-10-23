//! LLVM backend checks that need more than a stdout comparison: MIR shape
//! pins, LLVM IR content, opt levels, no_std linking and IR verification.
//! The Cranelift-vs-LLVM run comparison of each fixture lives in
//! `backend_parity`.

use ruxen_core::codegen::{self, Backend};
use ruxen_core::driver::{Options, Prelude};

use crate::common::{compile_with_backend, lower, lower_with, run_binary};

#[test]
fn q49_branch_scoped_string_drops_stay_in_their_arms() {
    // Each `if`/`else` arm binds its OWN owned `String` (via a `ruxen_string_from`
    // wrapper), so the drop elaborator must free + `Drop` it inside that arm's
    // block and NOT hoist the free to the shared `Return`. (A bare `puts
    // "positive"` is a zero-copy `&String` borrow since the bare-literal
    // optimization, so it owns nothing to drop; the fixture binds a local per
    // arm to exercise Q49's branch-scoped drop sinking.)
    let source = std::fs::read_to_string("tests/fixtures/branch_scoped_string_drops.rx").unwrap();
    let mir = lower(&source);
    let main = mir
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main MIR function");

    let mut arm_freed_locals = Vec::new();
    for expected in ["positive", "non-positive"] {
        let (block, literal_local) = main
            .blocks
            .iter()
            .find_map(|block| {
                block.instructions.iter().find_map(|inst| match inst {
                    ruxen_core::mir::nodes::MirInst::StringLiteral { dest, value }
                        if value == expected =>
                    {
                        Some((block, *dest))
                    }
                    _ => None,
                })
            })
            .unwrap_or_else(|| panic!("missing {expected:?} StringLiteral arm"));
        // The arm must OWN the string: a `ruxen_string_from` wrapping this
        // literal (proves it is not a zero-copy borrow).
        assert!(
            block.instructions.iter().any(|inst| matches!(
                inst,
                ruxen_core::mir::nodes::MirInst::Call { dest: Some(_), callee, args }
                    if callee == "ruxen_string_from"
                        && matches!(args.as_slice(), [ruxen_core::mir::nodes::MirValue::Use(local)] if *local == literal_local)
            )),
            "missing owned wrapper for {expected:?}"
        );
        // The owned local bound in this arm must be freed INSIDE the arm block.
        let freed_local = block
            .instructions
            .iter()
            .find_map(|inst| match inst {
                ruxen_core::mir::nodes::MirInst::Call {
                    dest: None,
                    callee,
                    args,
                } if callee == "ruxen_string_free" => match args.as_slice() {
                    [ruxen_core::mir::nodes::MirValue::Use(local)] => Some(*local),
                    _ => None,
                },
                _ => None,
            })
            .unwrap_or_else(|| {
                panic!(
                    "Q49 regression: {expected:?} arm's owned String is not freed inside its arm"
                )
            });
        assert!(
            block.instructions.iter().any(|inst| matches!(
                inst,
                ruxen_core::mir::nodes::MirInst::Drop { local } if *local == freed_local
            )),
            "Q49 regression: local {freed_local} lacks its arm-local Drop marker"
        );
        assert!(matches!(
            block.terminator,
            ruxen_core::mir::nodes::Terminator::Goto(_)
        ));
        arm_freed_locals.push(freed_local);
    }

    for block in &main.blocks {
        if !matches!(
            block.terminator,
            ruxen_core::mir::nodes::Terminator::Return(_)
        ) {
            continue;
        }
        assert!(
            !block.instructions.iter().any(|inst| matches!(
                inst,
                ruxen_core::mir::nodes::MirInst::Call { callee, args, .. }
                    if callee == "ruxen_string_free"
                        && matches!(args.as_slice(), [ruxen_core::mir::nodes::MirValue::Use(local)] if arm_freed_locals.contains(local))
            )),
            "Q49 regression: a branch-only String free was hoisted to Return"
        );
    }
}

#[test]
fn q48_llvm_ffi_alias_calls_reference_only_c_symbols() {
    let source = r#"
def main
  var s = String.new
  s.push_str(&"hello ")
  s.push_str(&"world")
  puts "len=#{s.size} val=#{s}"
end
"#;
    let mir = lower(source);
    let context = inkwell::context::Context::create();
    let module = context.create_module("q48_ffi_alias_pin");

    ruxen_core::codegen::llvm::emit::compile_program(&mir, &module, &context, false, false)
        .expect("LLVM IR emission failed");

    let ir = module.print_to_string().to_string();
    for symbol in [
        "ruxen_string_new",
        "ruxen_string_push_str",
        "ruxen_string_truncate",
        "ruxen_fmt_formatter_new",
        "ruxen_fmt_formatter_write_str",
        "ruxen_fmt_formatter_buffer",
    ] {
        assert!(
            ir.contains(&format!("@{symbol}")),
            "LLVM IR does not reference FFI C symbol @{symbol}"
        );
    }
    for called_symbol in [
        "ruxen_string_new",
        "ruxen_string_push_str",
        "ruxen_fmt_formatter_new",
        "ruxen_fmt_formatter_write_str",
        "ruxen_fmt_formatter_buffer",
    ] {
        assert!(
            ir.matches(&format!("@{called_symbol}")).count() > 1,
            "LLVM IR declares @{called_symbol} but does not call it"
        );
    }
    for mangled in [
        "String_new",
        "String_push_str",
        "String_truncate",
        "Formatter_new",
        "Formatter_write_str",
        "Formatter_buffer",
    ] {
        assert!(
            !ir.contains(&format!("@{mangled}")),
            "LLVM IR leaked mangled FFI alias @{mangled} instead of its C symbol"
        );
    }
}

#[test]
fn q41_runtime_mixin_class_info_lowers_links_and_runs() {
    let source = std::fs::read_to_string("tests/fixtures/q41_llvm_runtime_mixin.rx").unwrap();

    let llvm_exe = compile_with_backend(&source, "q41", Backend::Llvm { opt_level: 2 });
    let (stdout, _, exit) = run_binary(&llvm_exe);

    assert_eq!(stdout, "Q\n");
    assert_eq!(exit, Some(0));
}

/// A no_std `main` has no `std.env`, so the LLVM build must not call
/// `ruxen_env_init`: the object links with no stdlib runtime and runs.
#[test]
fn no_std_main_links_without_env_init() {
    let source =
        "def add(a: Int, b: Int) -> Int\n  a + b\nend\n\ndef main\n  let x = add(1, 2)\nend\n";
    let no_std = Options {
        prelude: Prelude::None,
        no_std: true,
        ..Options::default()
    };
    let mir_program = lower_with(source, &no_std);
    let output_path = format!(
        "{}/ruxen_test_{}_no_std.exe",
        std::env::temp_dir().display(),
        std::process::id()
    );

    codegen::compile_no_std(&mir_program, &output_path, Backend::Llvm { opt_level: 0 })
        .expect("no_std LLVM build should link without ruxen_env_init");
    let (_, _, exit) = run_binary(output_path.as_ref());

    assert_eq!(exit, Some(0));
}

/// Test that all optimization levels produce correct output.
#[test]
fn all_opt_levels_correct() {
    let source = std::fs::read_to_string("tests/fixtures/arithmetic.rx").unwrap();

    let cl_exe = compile_with_backend(&source, "opt_levels", Backend::Cranelift);
    let (baseline, _, _) = run_binary(&cl_exe);

    for opt in [0u8, 1, 2, 3] {
        let llvm_exe =
            compile_with_backend(&source, "opt_levels", Backend::Llvm { opt_level: opt });
        let (result, _, exit) = run_binary(&llvm_exe);
        assert_eq!(baseline, result, "Output mismatch at -O{}", opt);
        assert_eq!(exit, Some(0), "Non-zero exit at -O{}", opt);
    }
}

/// Test that LLVM IR verification passes for all fixtures.
#[test]
fn llvm_ir_verifies_all_fixtures() {
    for fixture in std::fs::read_dir("tests/fixtures").unwrap() {
        let path = fixture.unwrap().path();
        if path.extension().is_some_and(|e| e == "rx") {
            let source = std::fs::read_to_string(&path).unwrap();
            // If compile_with_backend succeeds, verification passed (it's called inside)
            let exe = compile_with_backend(&source, "verify", Backend::Llvm { opt_level: 0 });
            let _ = std::fs::remove_file(&exe);
        }
    }
}
