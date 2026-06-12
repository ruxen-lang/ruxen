//! Tier 4.02 cross-compilation: triple resolution + Cranelift ISA lookup +
//! per-target cache isolation.
//!
//! These exercise the public `codegen::target` surface and prove that a cross
//! Cranelift `CodeGen` can be constructed for each first-class target (the ISA
//! is actually built, not just the triple parsed). The container/Docker bars
//! live in `scripts/cross_verify.sh` (they need Docker/Rosetta and run in the
//! integration phase, not here).

use ruxen_core::codegen::cranelift::CodeGen as CraneliftCodeGen;
use ruxen_core::codegen::target::{HostArch, HostOs, ResolvedTarget};

#[test]
fn host_resolves_and_builds() {
    let host = ResolvedTarget::resolve(None).unwrap();
    assert!(host.is_host());
    // The host CodeGen must construct (native builder path).
    CraneliftCodeGen::new_for_target(Some(&host)).expect("host CodeGen builds");
    CraneliftCodeGen::new().expect("legacy new() builds");
}

#[test]
fn cranelift_isa_lookup_succeeds_for_each_native_target() {
    // Every Cranelift-capable target must build an ISA via isa::lookup. This
    // is the proof that `all-native-arch` is enabled — without it these return
    // SupportDisabled.
    for triple in [
        "x86_64-apple-darwin",
        "aarch64-apple-darwin",
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
    ] {
        let t = ResolvedTarget::resolve(Some(triple)).unwrap();
        assert!(!t.requires_llvm_backend(), "{triple} is a Cranelift target");
        CraneliftCodeGen::new_for_target(Some(&t))
            .unwrap_or_else(|e| panic!("ISA lookup failed for {triple}: {e}"));
    }
}

#[test]
fn wasm_target_rejects_cranelift() {
    let t = ResolvedTarget::resolve(Some("wasm32-unknown-unknown")).unwrap();
    assert!(t.requires_llvm_backend());
    // Cranelift can't build a wasm ISA — constructing the cross CodeGen errors.
    // `CodeGen` isn't `Debug`, so match the result rather than `unwrap_err`.
    match CraneliftCodeGen::new_for_target(Some(&t)) {
        Ok(_) => panic!("Cranelift unexpectedly built a wasm ISA"),
        Err(err) => assert!(
            err.contains("wasm") || err.to_lowercase().contains("unsupported"),
            "expected an unsupported-target error, got: {err}"
        ),
    }
}

#[test]
fn linker_strategy_matches_host_capabilities() {
    // aarch64 Linux from this host with no cross gcc → container.
    let linux = ResolvedTarget::resolve(Some("aarch64-unknown-linux-gnu")).unwrap();
    let spec =
        ruxen_core::codegen::target::linker_for(&linux, HostOs::Darwin, HostArch::Aarch64, |_| {
            false
        })
        .unwrap();
    assert!(
        spec.needs_container,
        "linux target w/o cross gcc → container"
    );

    // x86_64 darwin from an arm64 darwin host → local `cc -arch x86_64`.
    let darwin = ResolvedTarget::resolve(Some("x86_64-apple-darwin")).unwrap();
    let spec =
        ruxen_core::codegen::target::linker_for(&darwin, HostOs::Darwin, HostArch::Aarch64, |_| {
            false
        })
        .unwrap();
    assert_eq!(spec.program, "cc");
    assert_eq!(spec.target_args, vec!["-arch", "x86_64"]);
    assert!(!spec.needs_container);
}

#[test]
fn android_is_config_ready() {
    // Android resolves and selects an NDK clang (untested — no NDK on host).
    let t = ResolvedTarget::resolve(Some("aarch64-linux-android")).unwrap();
    assert!(t.is_android());
    let spec =
        ruxen_core::codegen::target::linker_for(&t, HostOs::Darwin, HostArch::Aarch64, |_| false)
            .unwrap();
    assert!(
        spec.program.contains("android") && spec.program.contains("clang"),
        "android linker should be an NDK clang, got {}",
        spec.program
    );
}
