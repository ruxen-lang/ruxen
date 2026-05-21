//! Phase A + Phase B-1/B-2/B-3 pin tests for
//! `docs/specs/types/mixin_vtables.spec.md`.
//!
//! Phase A scope: parser surface + typeck validation. Phase B-1 scope:
//! `ClassInfo.runtime_dispatch_includes` tracks each runtime-dispatch
//! mixin a class includes (DefId list). Phase B-2/B-3: MIR carries
//! `MirVtable`/`MirClassInfo` metadata that codegen emits as static
//! data sections. Phases B-4..B-6 (header alloc, init-time write,
//! pin tests for B-4/B-5) and Phase C/D follow.
//!
//! | Behaviour | Test                                                       | Spec |
//! |-----------|------------------------------------------------------------|------|
//! | B1        | `mixin_dispatch_runtime_modifier_parses`                   | §B1  |
//! | B7 syntax | `dyn_mixin_param_type_parses_and_typechecks`               | §B7  |
//! | E1118     | `amp_mixin_to_static_mixin_emits_e1118`                    | §B7  |
//! | E1117     | `runtime_dispatch_mixin_missing_method_emits_e1117`        | §B1  |
//! | B1        | `class_includes_runtime_mixin_is_tracked_on_classinfo`     | §B-1 |
//! | B-2       | `vtable_struct_emitted_per_implementor`                    | §B3  |
//! | B-3       | `class_info_struct_emitted_per_runtime_dispatch_class`     | §B8  |
//! | B-2/B-3   | `static_mixin_class_produces_no_vtable_or_classinfo`       | §B11 |
//! | B-4       | `runtime_dispatch_class_field_index_shifted_by_one`        | §B2  |
//! | B-4       | `static_mixin_class_field_index_unshifted`                 | §B11 |
//! | B-5       | `class_init_writes_class_info_ptr_at_slot_zero`            | §B4/§B5 |
//! | C         | `dyn_mixin_call_lowers_to_dynamic_helper`                  | §B5/§B6 |
//! | C         | `concrete_class_call_stays_static_dispatch`                | §B6  |
//! | C         | `dynamic_dispatch_helper_synthesized_per_mixin_method`      | §B5  |
//!
//! Discipline: all Riven source goes through `.rvn` fixtures
//! (`feedback_no_inline_rvn_in_pin_tests`).

use riven_core::diagnostics::{Diagnostic, DiagnosticLevel};
use riven_core::lexer::Lexer;
use riven_core::parser::ast::{DispatchMode, MixinDef, TopLevelItem};
use riven_core::parser::Parser;
use riven_core::typeck;

fn rvn(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/riven")
        .join(format!("{name}.rvn"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn parse(source: &str) -> riven_core::parser::ast::Program {
    let mut lx = Lexer::new(source);
    let toks = lx.tokenize().expect("lex");
    let mut p = Parser::new(toks);
    p.parse().expect("parse")
}

fn errors(source: &str) -> Vec<Diagnostic> {
    let prog = parse(source);
    let result = typeck::type_check(&prog);
    result
        .diagnostics
        .into_iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect()
}

fn only_mixin(program: &riven_core::parser::ast::Program) -> &MixinDef {
    let mixins: Vec<&MixinDef> = program
        .items
        .iter()
        .filter_map(|i| match i {
            TopLevelItem::Mixin(m) => Some(m),
            _ => None,
        })
        .collect();
    assert_eq!(
        mixins.len(),
        1,
        "expected exactly one mixin, found {}",
        mixins.len()
    );
    mixins[0]
}

// ─── B1: parser captures `dispatch runtime` modifier ────────────────

#[test]
fn mixin_dispatch_runtime_modifier_parses() {
    let source = rvn("mixin_dispatch_runtime_modifier_parses");
    let program = parse(&source);
    let mixin = only_mixin(&program);
    assert_eq!(mixin.name, "Sized");
    assert_eq!(
        mixin.dispatch_mode,
        DispatchMode::Runtime,
        "expected `dispatch runtime` modifier to set dispatch_mode = Runtime",
    );

    // And: a class that includes it and implements the required method
    // typechecks clean (no E1117, no E1118).
    let errs = errors(&source);
    assert!(
        errs.is_empty(),
        "expected no diagnostics, got: {:?}",
        errs.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
    );
}

#[test]
fn mixin_without_dispatch_runtime_defaults_to_static() {
    // Sanity / regression: ordinary mixins (no modifier) keep the
    // default `DispatchMode::Static`. The fixture is the existing
    // `mixin_and_include.rvn` which predates this feature.
    let source = rvn("mixin_and_include");
    let program = parse(&source);
    let mixin = only_mixin(&program);
    assert_eq!(
        mixin.dispatch_mode,
        DispatchMode::Static,
        "mixins without `dispatch runtime` must stay statically dispatched",
    );
}

// ─── B7 syntax: `&Mixin` / `&var Mixin` parameter types ─────────────

#[test]
fn dyn_mixin_param_type_parses_and_typechecks() {
    let source = rvn("dyn_mixin_param_type_parses_and_typechecks");
    // Parse succeeds (already guaranteed by `parse()` panicking on err).
    let _ = parse(&source);
    // And typeck reports no errors — the dyn-shape reference is valid
    // because `Drawable` is `dispatch runtime`.
    let errs = errors(&source);
    assert!(
        errs.is_empty(),
        "expected typecheck to accept `&Drawable` / `&var Drawable`, got: {:?}",
        errs.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
    );
}

// ─── E1118: `&Mixin` to a non-runtime-dispatch mixin ────────────────

#[test]
fn amp_mixin_to_static_mixin_emits_e1118() {
    let source = rvn("dyn_mixin_ref_to_static_mixin_emits_e1118");
    let errs = errors(&source);
    let count = errs
        .iter()
        .filter(|d| d.code.as_deref() == Some("E1118"))
        .count();
    assert_eq!(
        count, 1,
        "expected exactly one E1118 for `&Drawable` against a statically-dispatched mixin, \
         got: {:?}",
        errs.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
    );
}

// ─── E1117: runtime-dispatch mixin missing required method ──────────

#[test]
fn runtime_dispatch_mixin_missing_method_emits_e1117() {
    let source = rvn("runtime_mixin_missing_method_emits_e1117");
    let errs = errors(&source);
    let count = errs
        .iter()
        .filter(|d| d.code.as_deref() == Some("E1117"))
        .count();
    assert_eq!(
        count, 1,
        "expected exactly one E1117 for missing `draw` on Triangle, got: {:?}",
        errs.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
    );
    // And the message should name the missing method so the user can
    // act on it.
    let msg = errs
        .iter()
        .find(|d| d.code.as_deref() == Some("E1117"))
        .map(|d| d.message.clone())
        .unwrap_or_default();
    assert!(
        msg.contains("size"),
        "E1117 message should name the missing method `size`, got: {}",
        msg
    );
}

// ─── Phase B-1 — runtime_dispatch_includes bookkeeping ────────────

/// A class that `include`s a `dispatch runtime` mixin must carry
/// the mixin's DefId on `ClassInfo.runtime_dispatch_includes`.
/// Phases B-2..B-5 consume this list to emit per-mixin vtables,
/// per-class class_info structs, and to inject a class_info_ptr
/// header at allocation time. Without this bookkeeping, codegen
/// can't tell which classes need the header.
#[test]
fn class_includes_runtime_mixin_is_tracked_on_classinfo() {
    let source = rvn("mixin_dispatch_runtime_modifier_parses");
    let mut lx = Lexer::new(&source);
    let toks = lx.tokenize().expect("lex");
    let mut p = Parser::new(toks);
    let prog = p.parse().expect("parse");
    let result = typeck::type_check(&prog);

    // Fixture has `mixin Sized dispatch runtime` + `class Circle;
    // include Sized; ...`. The runtime-dispatch include should land
    // on Circle's ClassInfo.runtime_dispatch_includes.
    let sized_def_id = result
        .symbols
        .iter()
        .find(|d| {
            d.name == "Sized"
                && matches!(
                    &d.kind,
                    riven_core::resolve::symbols::DefKind::Trait { info }
                        if matches!(info.dispatch_mode, DispatchMode::Runtime)
                )
        })
        .map(|d| d.id)
        .expect("mixin `Sized dispatch runtime` should be registered");

    let circle_info = result
        .symbols
        .iter()
        .find(|d| d.name == "Circle")
        .map(|d| match &d.kind {
            riven_core::resolve::symbols::DefKind::Class { info } => info.clone(),
            _ => panic!("Circle is not a class"),
        })
        .expect("class `Circle` should be registered");

    assert!(
        circle_info.runtime_dispatch_includes.contains(&sized_def_id),
        "Circle.runtime_dispatch_includes should contain Sized's DefId; got {:?}",
        circle_info.runtime_dispatch_includes
    );
    // No other includes in this fixture, so list length is 1.
    assert_eq!(
        circle_info.runtime_dispatch_includes.len(),
        1,
        "expected exactly one runtime-dispatch include on Circle, got {:?}",
        circle_info.runtime_dispatch_includes
    );
}

/// Negative: a class that includes only STATIC mixins (`include Foo`
/// where Foo has no `dispatch runtime`) gets an empty list — the
/// existing static-dispatch path is untouched. No vtable header,
/// no class_info struct emission.
#[test]
fn class_with_only_static_mixin_has_empty_runtime_dispatch_list() {
    let source = "
        mixin Plain
          def speak -> Int
        end

        class Bob
          include Plain

          def speak -> Int
            1
          end
        end
    ";
    let mut lx = Lexer::new(source);
    let toks = lx.tokenize().expect("lex");
    let mut p = Parser::new(toks);
    let prog = p.parse().expect("parse");
    let result = typeck::type_check(&prog);

    let bob_info = result
        .symbols
        .iter()
        .find(|d| d.name == "Bob")
        .map(|d| match &d.kind {
            riven_core::resolve::symbols::DefKind::Class { info } => info.clone(),
            _ => panic!(),
        })
        .expect("class `Bob` should be registered");

    assert!(
        bob_info.runtime_dispatch_includes.is_empty(),
        "static-only mixin includes should produce empty runtime_dispatch_includes, got {:?}",
        bob_info.runtime_dispatch_includes
    );
}

// ─── Phase B-2/B-3 — MIR vtable + class_info metadata ─────────────

/// Helper: parse, typecheck, and MIR-lower a fixture, asserting clean
/// typecheck. Returns the lowered `MirProgram`.
fn lower_fixture(name: &str) -> riven_core::mir::nodes::MirProgram {
    let source = rvn(name);
    let prog = parse(&source);
    let result = typeck::type_check(&prog);
    let errs: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(errs.is_empty(), "fixture {} typecheck errors: {:?}", name, errs);
    let mut lowerer = riven_core::mir::lower::Lowerer::new(&result.symbols);
    lowerer
        .lower_program(&result.program)
        .expect("MIR lowering failed")
}

/// Phase B-2: every class that `include`s a `dispatch runtime` mixin
/// gets exactly one `MirVtable` per `(class, mixin)` pair. The
/// vtable's `method_symbols` are the mangled `<Class>_<method>`
/// callees, ordered by the mixin's `required_methods` declaration.
#[test]
fn vtable_struct_emitted_per_implementor() {
    let mir = lower_fixture("mixin_dispatch_runtime_modifier_parses");

    // Fixture: `mixin Sized dispatch runtime do def size -> Int end`
    // + `class Circle do include Sized; def size -> Int ... end end`.
    // Expect exactly one vtable for (Circle, Sized) holding [Circle_size].
    let circle_sized: Vec<_> = mir
        .vtables
        .iter()
        .filter(|v| v.class_name == "Circle" && v.mixin_name == "Sized")
        .collect();
    assert_eq!(
        circle_sized.len(),
        1,
        "expected exactly one vtable for (Circle, Sized), got: {:?}",
        mir.vtables
            .iter()
            .map(|v| (&v.class_name, &v.mixin_name))
            .collect::<Vec<_>>()
    );
    let vt = circle_sized[0];
    assert_eq!(
        vt.method_symbols,
        vec!["Circle_size".to_string()],
        "vtable method_symbols should be the mangled <Class>_<method> list",
    );
    assert_eq!(
        vt.symbol(),
        "__rvn_vtable_Sized_for_Circle",
        "vtable symbol name follows spec §B3 convention",
    );
}

/// Phase B-3: every class with non-empty `runtime_dispatch_includes`
/// produces exactly one `MirClassInfo` whose `vtable_symbols` are
/// listed in mixin-inclusion order. For the single-mixin fixture, the
/// info has one slot pointing at the one vtable.
#[test]
fn class_info_struct_emitted_per_runtime_dispatch_class() {
    let mir = lower_fixture("mixin_dispatch_runtime_modifier_parses");

    let circle_ci: Vec<_> = mir
        .class_infos
        .iter()
        .filter(|c| c.class_name == "Circle")
        .collect();
    assert_eq!(
        circle_ci.len(),
        1,
        "expected exactly one class_info for Circle, got {}",
        circle_ci.len()
    );
    let ci = circle_ci[0];
    assert_eq!(
        ci.vtable_symbols,
        vec!["__rvn_vtable_Sized_for_Circle".to_string()],
        "class_info should point at the (Circle, Sized) vtable",
    );
    assert_eq!(
        ci.symbol(),
        "__rvn_classinfo_Circle",
        "class_info symbol name follows spec §B8 convention",
    );
}

/// Negative: a fixture-defined class that includes only
/// statically-dispatched mixins (no `dispatch runtime`) gets ZERO
/// vtable and ZERO class_info entries FOR THAT CLASS — its layout
/// stays flat, no header is added. Spec §B11.
///
/// Note: the bootstrap stdlib prelude declares `mixin Future
/// dispatch runtime` (Phase D) and several Future implementors
/// (`TimeSleepFuture`, `AsyncReadFuture`, etc.), so the program-
/// level vtable lists are non-empty for any fixture that loads the
/// prelude. This test scopes to the fixture's `Bob` class — Bob
/// must NOT appear in any vtable or class_info symbol.
#[test]
fn static_mixin_class_produces_no_vtable_or_classinfo() {
    // Single static-mixin class — no `dispatch runtime` anywhere.
    let mir = lower_fixture("static_mixin_no_vtable_emission");
    let bob_vtables: Vec<_> = mir
        .vtables
        .iter()
        .filter(|v| v.class_name == "Bob")
        .collect();
    assert!(
        bob_vtables.is_empty(),
        "static-only-mixin class Bob should emit no vtable, got: {:?}",
        bob_vtables.iter().map(|v| v.symbol()).collect::<Vec<_>>()
    );
    let bob_class_infos: Vec<_> = mir
        .class_infos
        .iter()
        .filter(|c| c.class_name == "Bob")
        .collect();
    assert!(
        bob_class_infos.is_empty(),
        "static-only-mixin class Bob should emit no class_info, got: {:?}",
        bob_class_infos.iter().map(|c| c.symbol()).collect::<Vec<_>>()
    );
}

// ─── Phase B-4 — class_info_ptr layout header shift ──────────────

/// Phase B-4: a runtime-dispatch class's user-declared field `radius`
/// (declared index 0) lowers to a `SetField { field_index: 1 }` /
/// `GetField { field_index: 1 }` because slot 0 is reserved for the
/// class_info_ptr header. Without the +1 shift, init writes would
/// overwrite the header (slot 0) and any subsequent dynamic dispatch
/// would chase a corrupted pointer.
#[test]
fn runtime_dispatch_class_field_index_shifted_by_one() {
    let mir = lower_fixture("mixin_dispatch_runtime_modifier_parses");

    // Fixture defines `Circle.init(@radius: Int)` and a method
    // `Circle.size` that reads `self.radius`. The Circle_init MIR
    // body should `SetField` radius at slot 1 (after class_info_ptr);
    // Circle_size should `GetField` radius at slot 1 too.

    let init_fn = mir
        .functions
        .iter()
        .find(|f| f.name == "Circle_init")
        .expect("Circle_init MIR fn must exist");

    let radius_set_slots: Vec<usize> = init_fn
        .blocks
        .iter()
        .flat_map(|b| b.instructions.iter())
        .filter_map(|i| match i {
            riven_core::mir::nodes::MirInst::SetField { field_index, .. } => Some(*field_index),
            _ => None,
        })
        .collect();
    // The init body auto-assigns the @radius param to self.radius;
    // with B-4's +1 header shift it must land at slot 1 (declared
    // idx 0 + 1 header slot for class_info_ptr). The slot-0 write
    // (class_info_ptr) happens in the caller at the alloc site
    // (`Circle.new(...)`), not in init.
    assert_eq!(
        radius_set_slots,
        vec![1],
        "Circle_init must SetField the declared field `radius` at slot 1 \
         (declared idx 0 + class_info_ptr header shift = 1), got slots: {:?}",
        radius_set_slots
    );

    let size_fn = mir
        .functions
        .iter()
        .find(|f| f.name == "Circle_size")
        .expect("Circle_size MIR fn must exist");

    let radius_get_slots: Vec<usize> = size_fn
        .blocks
        .iter()
        .flat_map(|b| b.instructions.iter())
        .filter_map(|i| match i {
            riven_core::mir::nodes::MirInst::GetField { field_index, .. } => Some(*field_index),
            _ => None,
        })
        .collect();
    assert!(
        radius_get_slots.iter().any(|&s| s == 1),
        "Circle_size must GetField `radius` at slot 1, got slots: {:?}",
        radius_get_slots
    );
    assert!(
        !radius_get_slots.iter().any(|&s| s == 0),
        "Circle_size must NOT GetField at slot 0 (that's the class_info_ptr header), \
         got slots: {:?}",
        radius_get_slots
    );
}

/// Phase B-4 (negative): a class that only includes a static-dispatch
/// mixin (or none at all) does NOT get a header shift — declared
/// field index 0 stays at MIR slot 0. Without this guarantee, every
/// existing class in the stdlib would silently shift +1 and the
/// codegen-side `field_index * 8` stride would corrupt every existing
/// field access.
#[test]
fn static_mixin_class_field_index_unshifted() {
    let mir = lower_fixture("static_mixin_no_vtable_emission");

    // Fixture: `class Bob do include Plain (static); def speak ... 1 end end`.
    // Bob has no declared fields, but Bob_speak doesn't GetField at all;
    // the test that matters is that NO class_info_ptr SetField
    // is emitted at slot 0 in any Bob method (no `__rvn_classinfo_Bob`
    // exists, so emitting one would be a hard codegen error).
    let bob_methods: Vec<&riven_core::mir::nodes::MirFunction> = mir
        .functions
        .iter()
        .filter(|f| f.name.starts_with("Bob_"))
        .collect();
    assert!(
        !bob_methods.is_empty(),
        "expected at least one Bob_* method in MIR",
    );
    for f in &bob_methods {
        let dataaddrs: Vec<&str> = f
            .blocks
            .iter()
            .flat_map(|b| b.instructions.iter())
            .filter_map(|i| match i {
                riven_core::mir::nodes::MirInst::DataAddr { data_sym, .. } => {
                    Some(data_sym.as_str())
                }
                _ => None,
            })
            .collect();
        assert!(
            dataaddrs.is_empty(),
            "{}: static-mixin class must NOT emit any DataAddr (class_info_ptr) \
             instructions, found: {:?}",
            f.name,
            dataaddrs
        );
    }
}

/// Phase B-4 follow-up: synthesised async state-machine classes
/// (`__<FnName>Future`) `include Future` which is `dispatch runtime`,
/// so the same +1 header shift must apply to their auto-assigned
/// init params. The MIR-side bug fixed alongside this test was that
/// `name.split('_').next()` (the old strategy for recovering the
/// owning class name from `__HandlerFuture_init`) returned `""` for
/// any `__`-prefixed name, leaving the auto-assigns at slot 0/1 even
/// when the class layout reserved slot 0 for class_info_ptr. Result:
/// state-machine init clobbered the header AND wrote outer args one
/// slot too low, while body field reads correctly used slot 1/2+ —
/// so any pre-await `self.<arg>` read in the init body returned the
/// wrong value (or a zero-init garbage cell), and any dynamic
/// dispatch on the future via the Future mixin chased a NULL
/// class_info_ptr.
#[test]
fn async_state_machine_init_auto_assigns_use_header_shift() {
    let mir = lower_fixture("segmenter_awaitee_pre_await_local");

    let init_fn = mir
        .functions
        .iter()
        .find(|f| f.name == "__HandlerFuture_init")
        .expect("__HandlerFuture_init MIR fn must exist");

    // Collect SetField slots for the AUTO-ASSIGN prologue only. The
    // init body also emits SetFields (for `self.__sub_0` and the
    // award-binding default), so we restrict to the leading run that
    // assigns from a param-source `Use(_)` value. Since the auto-
    // assign block is emitted FIRST in `lower_method`, the auto-
    // assign SetFields appear at the top of the entry block in order.
    let entry = init_fn
        .blocks
        .first()
        .expect("__HandlerFuture_init must have an entry block");
    let mut auto_assign_slots: Vec<usize> = Vec::new();
    for inst in &entry.instructions {
        match inst {
            riven_core::mir::nodes::MirInst::SetField { field_index, .. } => {
                auto_assign_slots.push(*field_index);
                // Stop once we leave the leading SetField prologue (a
                // GetField or non-SetField follows the auto-assigns).
                if auto_assign_slots.len() >= 2 {
                    break;
                }
            }
            _ => break,
        }
    }

    // Two outer params (@__state, @req) → declared idx 0, 1 →
    // post-shift slot 1, 2. Slot 0 stays the class_info_ptr header
    // written at the caller's `Alloc + DataAddr + SetField{0}` site.
    assert_eq!(
        auto_assign_slots,
        vec![1, 2],
        "__HandlerFuture_init auto-assigns must land at slot 1 (state) and \
         slot 2 (req) after class_info_ptr header shift, not at slot 0/1 \
         (which would clobber the header). Got: {:?}",
        auto_assign_slots
    );

    // And: the init body's `let parsed = self.req * 2` reads
    // `self.req` from slot 2 (declared idx 1 + shift 1). The earlier
    // bug left auto-assigns unshifted while body reads stayed
    // shifted, so `self.req` returned the zero-init slot 2 value
    // instead of the @req param value.
    let req_reads: Vec<usize> = init_fn
        .blocks
        .iter()
        .flat_map(|b| b.instructions.iter())
        .filter_map(|i| match i {
            riven_core::mir::nodes::MirInst::GetField { field_index, .. } => Some(*field_index),
            _ => None,
        })
        .collect();
    assert!(
        req_reads.contains(&2),
        "__HandlerFuture_init body must GetField `self.req` at slot 2 (declared \
         idx 1 + class_info_ptr header shift = 2), got slots: {:?}",
        req_reads
    );
}

// ─── Phase B-5 — class_info_ptr init-time write ────────────────────

/// Phase B-5: every alloc site for a runtime-dispatch class emits a
/// `DataAddr { data_sym: "__rvn_classinfo_<Class>" }` followed by a
/// `SetField { field_index: 0 }` writing the address into the
/// header. The exact pairing is enforced: every DataAddr for a
/// classinfo symbol must be followed by a SetField at slot 0 with
/// that local as its value.
///
/// The alloc site lives in the CALLER (the user code that writes
/// `Circle.new(5)`), not in `Circle_init` — `Circle_init` mutates an
/// already-allocated object passed as `self`. The fixture `make_one`
/// allocates a Circle and reads its size, so the pair lives in
/// `make_one`'s MIR body.
#[test]
fn class_init_writes_class_info_ptr_at_slot_zero() {
    let mir = lower_fixture("mixin_vtables_alloc_site");
    let make_one = mir
        .functions
        .iter()
        .find(|f| f.name == "make_one")
        .expect("make_one MIR fn must exist");

    // Scan instructions in order, looking for a DataAddr →
    // SetField(slot=0) pair where the SetField's value is the
    // DataAddr's dest.
    let mut found = false;
    for block in &make_one.blocks {
        let mut last_data_addr: Option<(riven_core::mir::nodes::LocalId, String)> = None;
        for inst in &block.instructions {
            match inst {
                riven_core::mir::nodes::MirInst::DataAddr { dest, data_sym } => {
                    last_data_addr = Some((*dest, data_sym.clone()));
                }
                riven_core::mir::nodes::MirInst::SetField {
                    field_index: 0,
                    value: riven_core::mir::nodes::MirValue::Use(v_local),
                    ..
                } => {
                    if let Some((da_dest, da_sym)) = &last_data_addr {
                        if *v_local == *da_dest && da_sym == "__rvn_classinfo_Circle" {
                            found = true;
                        }
                    }
                }
                _ => {}
            }
        }
    }
    assert!(
        found,
        "make_one must emit `DataAddr __rvn_classinfo_Circle` → `SetField slot 0` pair \
         to install the class_info_ptr header at the Circle.new alloc site (Phase B-5)",
    );
}

// ─── Phase C — dynamic dispatch helper + call-site lowering ───────

/// Helper for Phase C tests: extract every `Call { callee }` from a
/// MIR function's blocks.
fn callees_in(f: &riven_core::mir::nodes::MirFunction) -> Vec<String> {
    f.blocks
        .iter()
        .flat_map(|b| b.instructions.iter())
        .filter_map(|i| match i {
            riven_core::mir::nodes::MirInst::Call { callee, .. } => Some(callee.clone()),
            _ => None,
        })
        .collect()
}

/// Phase C: when the receiver is typed as `&Sized` (a single-bound
/// reference to a runtime-dispatch mixin), the method call lowers to
/// `Sized_dynamic_size(c)`, NOT `Circle_size(c)` or any other
/// concrete-class mangling.
#[test]
fn dyn_mixin_call_lowers_to_dynamic_helper() {
    let mir = lower_fixture("mixin_vtables_dyn_dispatch");
    let report = mir
        .functions
        .iter()
        .find(|f| f.name == "report")
        .expect("report MIR fn must exist");
    let calls = callees_in(report);
    assert!(
        calls.iter().any(|c| c == "Sized_dynamic_size"),
        "report's body must call the dispatch helper `Sized_dynamic_size`, got calls: {:?}",
        calls
    );
    assert!(
        !calls.iter().any(|c| c == "Circle_size" || c == "Square_size"),
        "report must NOT statically dispatch on a concrete class — the receiver \
         is `&Sized`, calls: {:?}",
        calls
    );
}

/// Phase C (regression): when the receiver is typed as the concrete
/// class `Circle` (NOT `&Sized`), the static-dispatch path stays
/// intact and emits `Circle_size(c)`. Without this, every method
/// call on a class type would accidentally route through the
/// dynamic helper.
#[test]
fn concrete_class_call_stays_static_dispatch() {
    let mir = lower_fixture("mixin_vtables_dyn_dispatch");
    let report_circle = mir
        .functions
        .iter()
        .find(|f| f.name == "report_circle")
        .expect("report_circle MIR fn must exist");
    let calls = callees_in(report_circle);
    assert!(
        calls.iter().any(|c| c == "Circle_size"),
        "report_circle's body must statically dispatch `c.size` to `Circle_size`, \
         got calls: {:?}",
        calls
    );
    assert!(
        !calls.iter().any(|c| c == "Sized_dynamic_size"),
        "report_circle must NOT call the dynamic helper — the receiver is `Circle`, \
         calls: {:?}",
        calls
    );
}

/// Phase C: for every (mixin, method) pair where the mixin is
/// `dispatch runtime` AND has at least one implementor in the
/// program, MIR carries a synthesized `<Mixin>_dynamic_<method>`
/// function. The helper's body is the three-load indirect call.
#[test]
fn dynamic_dispatch_helper_synthesized_per_mixin_method() {
    let mir = lower_fixture("mixin_vtables_dyn_dispatch");
    let helper = mir
        .functions
        .iter()
        .find(|f| f.name == "Sized_dynamic_size")
        .expect("Sized_dynamic_size helper must be synthesized");

    // The body must contain (in order in some block): a GetField
    // at slot 0 (class_info), a GetField at slot 0 (vtable), a
    // GetField at slot 0 (method_ptr — `size` is method index 0),
    // and a CallIndirect.
    let mut get_field_slots: Vec<usize> = vec![];
    let mut saw_call_indirect = false;
    for block in &helper.blocks {
        for inst in &block.instructions {
            match inst {
                riven_core::mir::nodes::MirInst::GetField { field_index, .. } => {
                    get_field_slots.push(*field_index);
                }
                riven_core::mir::nodes::MirInst::CallIndirect { .. } => {
                    saw_call_indirect = true;
                }
                _ => {}
            }
        }
    }
    assert_eq!(
        get_field_slots,
        vec![0, 0, 0],
        "helper body must have three GetField loads at slots [0,0,0] \
         (class_info, vtable, method[0]=size), got: {:?}",
        get_field_slots
    );
    assert!(
        saw_call_indirect,
        "helper must emit a CallIndirect on the method pointer",
    );
}
