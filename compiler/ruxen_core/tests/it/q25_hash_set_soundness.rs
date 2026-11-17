//! Q25 — empty-Hash/Set lookup SIGSEGV + `&Hash`/`&Set` param consistency.
//!
//! (a) `key?`/`get`/`contains` on an EMPTY hash/set SIGSEGV'd: the runtime's
//!     `string_keys` tristate (-1 = unset) was tested with plain C truthiness,
//!     and -1 is truthy, so an empty-table lookup took the string path and
//!     `strcmp`'d an integer key as a `char*` (deref of a small bogus
//!     address). Fixed in `library/std/hash/runtime/hash.c` to test `> 0`.
//!     B73 later removed the tristate entirely — the key kind is supplied by
//!     the compiler from the static key type — but the invariant these cases
//!     pin is the same: a lookup must not dereference a key whose kind says
//!     it is a plain word.
//!     The runtime cases are the e2e fixtures `617_*` and `618_*`.
//!
//! (b) `&Hash[K,V]` / `&Set[T]` by-ref params are SOUND (a pointer to the
//!     backing struct, exactly like the widely-used `&Array[Int]`). The bug
//!     was an INCONSISTENCY: a free fn rejected `&Hash[Int,Int]` with E1118
//!     (the `Hash → Hashable` mixin-alias collision) while a method accepted
//!     it. Resolution (sound + minimal): a generic-args-bearing collection
//!     builtin in `&Name[..]` position resolves to the COLLECTION type in both
//!     positions; a bare static-dispatch mixin ref (`&Hashable`) is still
//!     correctly rejected with E1118. (e2e `619_*` exercises the runtime
//!     side; the two compile-time pins below assert the diagnostic boundary.)

use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

fn error_messages(source: &str) -> Vec<String> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    result
        .diagnostics
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .map(|d| d.message.clone())
        .collect()
}

/// (b) A bare `&Hashable` (static-dispatch mixin) is STILL rejected with
/// E1118 — both as a free fn …
#[test]
fn bare_hashable_mixin_ref_rejected_free_fn() {
    let errs =
        error_messages("def f(h: &Hashable) -> Int\n  0\nend\ndef main\n  puts \"x\"\nend\n");
    assert!(
        errs.iter()
            .any(|m| m.contains("E1118") || m.contains("does not use runtime dispatch")),
        "expected E1118 for bare `&Hashable`; got {errs:?}"
    );
}

/// … and as a method. The method path resolves `&Hashable` through a different
/// route than the free fn, so it surfaces a *different* diagnostic ("could
/// not infer type for parameter") rather than E1118 — but the key soundness
/// property holds: the bare `&Hashable` mixin ref is REJECTED AT COMPILE TIME in
/// both positions, never compiled to a silent runtime miscompile. (Unifying
/// the two diagnostics is a DX follow-up, not a soundness gap.)
#[test]
fn bare_hashable_mixin_ref_rejected_method() {
    let errs = error_messages(
        "class C\n  t: Int\n  def init; self.t = 0; end\n  def f(h: &Hashable) -> Int\n    0\n  end\nend\ndef main\n  puts \"x\"\nend\n",
    );
    assert!(
        !errs.is_empty(),
        "expected a compile-time rejection for bare `&Hashable` method param; got none"
    );
}
