//! Pins the universal-`new` reconciliation claim that
//! `runtime_abi::is_static_constructor` makes (runtime_abi.rs:127-144):
//! `method_name == "new"` is a static constructor for ANY type, yet a user
//! class with `def init` and NO `def self.new` constructed via `.new` must
//! still route through `{Class}_init` with `self` prepended as arg0.
//!
//! Without this guarantee the universal-`new` rule would be a UAF / wrong-ABI
//! hazard: a user `.new` could be misclassified as a runtime collection ctor
//! and skip the init body entirely.

use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::mir::nodes::{MirInst, MirValue};
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

fn lower_fixture(name: &str) -> ruxen_core::mir::nodes::MirProgram {
    let source = std::fs::read_to_string(format!("tests/fixtures/{}", name))
        .unwrap_or_else(|e| panic!("failed to read {}: {}", name, e));
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lexer failed");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parser failed");
    let result = typeck::type_check(&program);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "type errors: {:?}", errors);
    let mut lowerer = Lowerer::new(&result.symbols);
    lowerer
        .lower_program(&result.program)
        .expect("MIR lowering failed")
}

/// A user class with `def init` and NO `def self.new`, constructed via `.new`,
/// must lower to an `Alloc` of the object followed by a
/// `Call { callee: "Widget_init", args: [Use(obj), <ctor args>...] }` — i.e.
/// `self` is prepended as arg0 and the call routes through `{Class}_init`.
#[test]
fn user_class_new_routes_through_init_with_self_prepended() {
    let mir = lower_fixture("static_constructor_init_route.rx");
    let main = mir
        .functions
        .iter()
        .find(|f| f.name == "main")
        .expect("main function present");

    // Find the Widget_init call.
    let init_call = main
        .blocks
        .iter()
        .flat_map(|b| b.instructions.iter())
        .find_map(|inst| match inst {
            MirInst::Call { dest, callee, args } if callee == "Widget_init" => {
                Some((dest, args))
            }
            _ => None,
        })
        .expect("main must emit a Widget_init call for `Widget.new(...)`");

    let (dest, args) = init_call;

    // The init call discards its result (it mutates `self` in place).
    assert!(
        dest.is_none(),
        "Widget_init is a void init, not a value-returning ctor; got dest={dest:?}"
    );

    // arg0 is `self`: the allocated object local. There must be an `Alloc`
    // (Class) whose dest is exactly args[0].
    let self_local = match args.first() {
        Some(MirValue::Use(l)) => *l,
        other => panic!("Widget_init arg0 must be Use(self); got {other:?}"),
    };
    let allocs_self = main.blocks.iter().flat_map(|b| b.instructions.iter()).any(
        |inst| matches!(inst, MirInst::Alloc { dest, .. } if *dest == self_local),
    );
    assert!(
        allocs_self,
        "Widget_init arg0 (local {self_local:?}) must be the freshly Alloc'd object (self)"
    );

    // The ctor argument (`7`) follows `self`, so init receives 2 args.
    assert_eq!(
        args.len(),
        2,
        "Widget.new(7) → Widget_init(self, 7): expected 2 args, got {args:?}"
    );

    // The synthesized `Widget_init` function exists and takes `self` first.
    let init_fn = mir
        .functions
        .iter()
        .find(|f| f.name == "Widget_init")
        .expect("Widget_init function must be lowered");
    assert!(
        !init_fn.params.is_empty(),
        "Widget_init must take self as its first parameter"
    );
}
