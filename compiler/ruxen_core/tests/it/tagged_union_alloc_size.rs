//! Pins `Lowerer::alloc_size` for `Option`/`Result` heap cells.
//!
//! Codegen addresses every tagged union with 8-byte slots: the tag sits in
//! slot 0 and `GetPayload` points at base + 8, where the payload word is
//! stored. A class payload is that one pointer word, whatever the class's own
//! field count. Sizing the cell from the packed layout counted a fieldless
//! class (`MutexGuard`, `PoisonError`) as 0 bytes, so `Ok(guard)` wrote 8
//! bytes past an 8-byte allocation on every `Mutex#lock`.

use ruxen_core::hir::types::Ty;
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::mir::nodes::{MirInst, MirProgram};
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

fn lower(source: &str) -> MirProgram {
    let tokens = Lexer::new(source).tokenize().expect("lexer failed");
    let program = Parser::new(tokens).parse().expect("parser failed");
    let result = typeck::type_check(&program);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "type errors: {errors:?}");
    Lowerer::new(&result.symbols)
        .lower_program(&result.program)
        .expect("MIR lowering failed")
}

/// Every `Alloc` of an `Option`/`Result` in `function` must hold the tag slot
/// plus the payload slot codegen writes.
fn assert_tagged_union_allocs_fit_payload(mir: &MirProgram, function: &str) {
    let func = mir
        .functions
        .iter()
        .find(|f| f.name == function)
        .unwrap_or_else(|| panic!("{function} not lowered"));
    let sizes: Vec<usize> = func
        .blocks
        .iter()
        .flat_map(|b| b.instructions.iter())
        .filter_map(|inst| match inst {
            MirInst::Alloc {
                ty: Ty::Option(_) | Ty::Result(..),
                size,
                ..
            } => Some(*size),
            _ => None,
        })
        .collect();
    assert!(!sizes.is_empty(), "{function} allocates no Option/Result");
    assert!(
        sizes.iter().all(|size| *size >= 16),
        "{function}: tag slot + payload slot need 16 bytes, got {sizes:?}"
    );
}

#[test]
fn result_and_option_of_fieldless_classes_hold_their_payload_slot() {
    let mir = lower(
        r#"
class Guard
  def init
  end
end

class Failure
  def init
  end
end

def acquire(ok: Bool) -> Result[Guard, Failure]
  if ok
    Ok(Guard.new)
  else
    Err(Failure.new)
  end
end

def maybe -> Option[Guard]
  Some(Guard.new)
end

_r = acquire(true)
_o = maybe
"#,
    );
    assert_tagged_union_allocs_fit_payload(&mir, "acquire");
    assert_tagged_union_allocs_fit_payload(&mir, "maybe");
}
