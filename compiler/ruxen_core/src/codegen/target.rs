//! Target-triple resolution for cross-compilation (tier 4.02).
//!
//! A single funnel: a user-supplied `--target <triple>` string (or `None`
//! for the host) becomes a parsed, canonicalized [`ResolvedTarget`] that the
//! Cranelift/LLVM backends, the linker picker, and the per-target runtime
//! resolver all consume. The host path (`None`) stays byte-identical to the
//! pre-4.02 behaviour — `cranelift_native::builder()`, plain `cc`,
//! `target/<profile>/` — so passing no `--target` is a no-op.
//!
//! ## Why an explicit alias table (spec §4.1 delta)
//!
//! The spec claims `target-lexicon` canonicalizes short aliases
//! (`x86_64-macos` → `x86_64-apple-darwin`). It does **not** in 0.13: parsing
//! `x86_64-macos` yields `os=Unknown, binary_format=Elf` (a *wrong*, silently
//! lossy result — macOS is Mach-O), and `aarch64-linux` drops the `gnu`
//! environment. Round-tripping `.to_string()` does not re-expand them either.
//! So we normalize aliases to full canonical triples *before* handing the
//! string to `target-lexicon`. This is deterministic and avoids the lossy
//! parse. Recorded in `docs/decisions/cross-compilation-linker-matrix.md`.

use std::fmt;

use target_lexicon::{Architecture, Environment, OperatingSystem, Triple};

/// A canonicalized cross-compilation target.
///
/// Constructed via [`ResolvedTarget::resolve`]. Carries both the parsed
/// `target_lexicon::Triple` (for ISA/linker decisions) and the canonical
/// triple string (for cache keys, runtime-dir lookup, and diagnostics).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTarget {
    triple: Triple,
    canonical: String,
    /// `true` when the user passed no `--target` (host build). The host path
    /// must remain byte-identical to pre-4.02 behaviour, so callers branch on
    /// this to choose `cranelift_native::builder()` over `isa::lookup`.
    is_host: bool,
}

/// The canonical triple strings we accept as first-class in this pass.
/// Android is config-ready (NDK-gated, untested on this host); wasm is the
/// NEXT phase (LLVM backend) and is accepted only so the §5.8 backend-compat
/// error can fire with a useful message rather than a parse error.
const KNOWN_CANONICAL: &[&str] = &[
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "x86_64-unknown-linux-gnu",
    "aarch64-linux-android",
    "wasm32-unknown-unknown",
];

/// Map a user-facing alias to a canonical triple. Returns the input
/// unchanged when it isn't a recognized alias (it may already be canonical,
/// or an arbitrary triple the user declared at their own risk).
fn canonicalize_alias(input: &str) -> &str {
    match input {
        "x86_64-linux" => "x86_64-unknown-linux-gnu",
        "aarch64-linux" | "arm64-linux" => "aarch64-unknown-linux-gnu",
        "x86_64-macos" | "x86_64-darwin" => "x86_64-apple-darwin",
        "aarch64-macos" | "aarch64-darwin" | "arm64-macos" | "arm64-darwin" => {
            "aarch64-apple-darwin"
        }
        "android" | "aarch64-android" => "aarch64-linux-android",
        "wasm32" | "wasm" => "wasm32-unknown-unknown",
        other => other,
    }
}

impl ResolvedTarget {
    /// The host triple, byte-identical-path build (no `--target` given).
    pub fn host() -> Self {
        // `Triple::host()` is resolved at *this crate's* build time, which is
        // exactly the host we generate code for on the default path.
        let triple = Triple::host();
        let canonical = triple.to_string();
        ResolvedTarget {
            triple,
            canonical,
            is_host: true,
        }
    }

    /// Resolve an optional user `--target`. `None` → [`ResolvedTarget::host`].
    /// `Some(s)` → alias-normalize, parse, canonicalize. Errors with a clear,
    /// actionable message on an unparseable triple.
    pub fn resolve(target: Option<&str>) -> Result<Self, String> {
        let Some(raw) = target else {
            return Ok(Self::host());
        };
        let raw = raw.trim();
        if raw.is_empty() {
            return Ok(Self::host());
        }
        let canonical_input = canonicalize_alias(raw);
        let triple: Triple = canonical_input.parse().map_err(|e| {
            format!(
                "invalid target triple '{}': {}. Known targets: {}",
                raw,
                e,
                KNOWN_CANONICAL.join(", ")
            )
        })?;
        // Re-derive the canonical string from the (possibly alias-expanded)
        // input so the cache key and runtime-dir lookup agree regardless of
        // which alias the user typed.
        let canonical = canonical_input.to_string();
        Ok(ResolvedTarget {
            triple,
            canonical,
            is_host: false,
        })
    }

    /// `true` when this is the implicit host target (no `--target`).
    pub fn is_host(&self) -> bool {
        self.is_host
    }

    /// The canonical triple string (cache key, runtime dir, diagnostics).
    pub fn canonical(&self) -> &str {
        &self.canonical
    }

    /// The parsed triple.
    pub fn triple(&self) -> &Triple {
        &self.triple
    }

    /// `true` when codegen for this target must route through the LLVM
    /// backend because Cranelift cannot emit it. Cranelift handles only
    /// x86_64 / aarch64 / s390x / riscv64 (wasm/embedded need LLVM). See
    /// `cranelift_codegen::isa::lookup`. The §5.8 backend-compatibility
    /// check uses this to reject `--backend=cranelift` with a wasm target.
    pub fn requires_llvm_backend(&self) -> bool {
        matches!(
            self.triple.architecture,
            Architecture::Wasm32 | Architecture::Wasm64
        )
    }

    /// `true` when the target OS is Linux (drives the two-stage Docker link
    /// flow on a macOS host and the per-target runtime compile).
    pub fn is_linux(&self) -> bool {
        matches!(self.triple.operating_system, OperatingSystem::Linux)
    }

    /// `true` when the target OS is Apple/Darwin (Mach-O, local `cc -arch`
    /// cross on a macOS host).
    pub fn is_darwin(&self) -> bool {
        matches!(self.triple.operating_system, OperatingSystem::Darwin(_))
    }

    /// `true` when the target environment is Android (bionic/NDK). Config-ready
    /// only in this pass — the NDK is not present on the build host, so the
    /// linker selection is wired but untested (`docs/CROSS_COMPILE.md`).
    pub fn is_android(&self) -> bool {
        matches!(self.triple.environment, Environment::Android)
            || matches!(self.triple.environment, Environment::Androideabi)
    }

    /// Derive the `cfg(...)` evaluation context from the resolved triple.
    /// Tier 4.01 (package manager) owns the cfg-expr *parser* and the
    /// `[target.<triple>.dependencies]` gating; this method provides only the
    /// fact table those consume, which the spec (§5.4) sites here.
    pub fn cfg_context(&self) -> CfgContext {
        CfgContext::from_triple(&self.triple)
    }
}

impl fmt::Display for ResolvedTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.canonical)
    }
}

/// The fact table a `cfg(...)` evaluator consumes (spec §5.4). String values
/// mirror Rust's `cfg!` so user intuition transfers. Tier 4.01 builds the
/// expression parser/evaluator on top of this; we only derive the facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CfgContext {
    pub target_arch: &'static str,
    pub target_os: &'static str,
    pub target_env: &'static str,
    pub target_family: &'static str,
    pub target_vendor: &'static str,
    pub target_pointer_width: &'static str,
    pub target_endian: &'static str,
}

impl CfgContext {
    fn from_triple(triple: &Triple) -> Self {
        let target_arch = match triple.architecture {
            Architecture::X86_64 | Architecture::X86_64h => "x86_64",
            Architecture::X86_32(_) => "x86",
            Architecture::Aarch64(_) => "aarch64",
            Architecture::Arm(_) => "arm",
            Architecture::Riscv64(_) => "riscv64",
            Architecture::Wasm32 => "wasm32",
            Architecture::Wasm64 => "wasm64",
            Architecture::S390x => "s390x",
            _ => "unknown",
        };
        let target_os = match triple.operating_system {
            OperatingSystem::Linux => "linux",
            OperatingSystem::Darwin(_) | OperatingSystem::MacOSX(_) => "macos",
            OperatingSystem::IOS(_) => "ios",
            OperatingSystem::Windows => "windows",
            OperatingSystem::Wasi | OperatingSystem::WasiP1 | OperatingSystem::WasiP2 => "wasi",
            OperatingSystem::None_ | OperatingSystem::Unknown => "none",
            _ => "unknown",
        };
        let target_env = match triple.environment {
            Environment::Gnu
            | Environment::Gnuabi64
            | Environment::Gnueabi
            | Environment::Gnueabihf
            | Environment::Gnux32 => "gnu",
            Environment::Musl
            | Environment::Musleabi
            | Environment::Musleabihf
            | Environment::Muslabi64 => "musl",
            Environment::Msvc => "msvc",
            Environment::Android | Environment::Androideabi => "android",
            Environment::Unknown | Environment::None => "",
            _ => "",
        };
        // `target_family`: unix for linux/macos/ios/android, wasm for wasm*,
        // empty otherwise. Mirrors Rust's definition.
        let target_family = match (target_os, target_arch) {
            (_, "wasm32") | (_, "wasm64") => "wasm",
            ("linux", _) | ("macos", _) | ("ios", _) => "unix",
            _ => "",
        };
        let target_vendor = match triple.operating_system {
            OperatingSystem::Darwin(_) | OperatingSystem::MacOSX(_) | OperatingSystem::IOS(_) => {
                "apple"
            }
            _ => "unknown",
        };
        let target_pointer_width = match triple.pointer_width() {
            Ok(target_lexicon::PointerWidth::U16) => "16",
            Ok(target_lexicon::PointerWidth::U32) => "32",
            Ok(target_lexicon::PointerWidth::U64) => "64",
            Err(_) => "64",
        };
        let target_endian = match triple.endianness() {
            Ok(target_lexicon::Endianness::Big) => "big",
            Ok(target_lexicon::Endianness::Little) => "little",
            Err(_) => "little",
        };
        CfgContext {
            target_arch,
            target_os,
            target_env,
            target_family,
            target_vendor,
            target_pointer_width,
            target_endian,
        }
    }
}

/// A resolved linker invocation: the program to exec plus the leading args
/// that select the target architecture/format. The build driver appends the
/// object files, runtime objects, `-o <out>`, and `[system_libs]` flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkerSpec {
    /// The linker driver program (`cc`, a cross gcc, etc.).
    pub program: String,
    /// Leading flags that select the target (e.g. `-arch x86_64` for a
    /// Darwin cross, or `--target=...` for clang).
    pub target_args: Vec<String>,
    /// `true` when the link must happen inside a target container because no
    /// local cross linker is available (Linux target from a macOS host with
    /// no `zig`/gnu-cross installed). The build driver routes these through
    /// the two-stage Docker flow rather than spawning `program` directly.
    pub needs_container: bool,
}

/// Decide how to link for a target on the current host.
///
/// Resolution order (subset of spec §5.5 — manifest/env overrides are applied
/// by the CLI layer which owns the manifest; this is the built-in default):
///
/// 1. Host target → plain `cc` (byte-identical path).
/// 2. Darwin → Darwin cross (any arch) → `cc -arch <arch>` locally (Mach-O,
///    same OS; runs under Rosetta for the opposite arch).
/// 3. Linux target whose arch == host arch on a Linux host → plain `cc`.
/// 4. Linux target with a conventional cross gcc on PATH → that gcc.
/// 5. Linux target with no local cross linker → `needs_container = true`
///    (two-stage Docker flow).
/// 6. Android → `<arch>-linux-android<api>-clang` (NDK; untested here).
///
/// `host_os` / `host_arch` are passed explicitly so this is a pure function
/// testable without reading process-global `cfg!`.
pub fn linker_for(
    target: &ResolvedTarget,
    host_os: HostOs,
    host_arch: HostArch,
    cross_gcc_on_path: impl Fn(&str) -> bool,
) -> Result<LinkerSpec, String> {
    if target.is_host() {
        return Ok(LinkerSpec {
            program: "cc".to_string(),
            target_args: vec![],
            needs_container: false,
        });
    }

    let arch = arch_tag(target);

    // Darwin target.
    if target.is_darwin() {
        // A Darwin host links any Darwin target locally with `-arch`.
        if host_os == HostOs::Darwin {
            return Ok(LinkerSpec {
                program: "cc".to_string(),
                target_args: vec!["-arch".to_string(), darwin_arch_flag(arch).to_string()],
                needs_container: false,
            });
        }
        return Err(format!(
            "cross-compiling to '{}' (macOS) requires a macOS host; \
             building Darwin targets from {:?} is not supported in this pass.",
            target.canonical(),
            host_os
        ));
    }

    // Android target — NDK clang. Wired but untested (no NDK on the build
    // host); the NDK provides `<triple><api>-clang` wrappers.
    if target.is_android() {
        let prog = format!("{}-linux-android21-clang", arch);
        return Ok(LinkerSpec {
            program: prog,
            target_args: vec![],
            // The NDK clang links locally when present. Untested here.
            needs_container: false,
        });
    }

    // Linux target.
    if target.is_linux() {
        let host_arch_matches = matches!(
            (host_arch, arch),
            (HostArch::X86_64, "x86_64") | (HostArch::Aarch64, "aarch64")
        );
        // Native: a Linux host targeting its own arch is just `cc`.
        if host_os == HostOs::Linux && host_arch_matches {
            return Ok(LinkerSpec {
                program: "cc".to_string(),
                target_args: vec![],
                needs_container: false,
            });
        }
        // A conventional cross gcc on PATH (e.g. `aarch64-linux-gnu-gcc`).
        let cross = format!("{}-linux-gnu-gcc", arch);
        if cross_gcc_on_path(&cross) {
            return Ok(LinkerSpec {
                program: cross,
                target_args: vec![],
                needs_container: false,
            });
        }
        // No local cross linker → two-stage container flow. The object is
        // emitted locally by the cross Cranelift/LLVM backend; the link
        // happens inside a target container where `cc` + libc are native.
        return Ok(LinkerSpec {
            program: "cc".to_string(),
            target_args: vec![],
            needs_container: true,
        });
    }

    Err(format!(
        "no linker strategy known for target '{}'. Set `[target.{}].linker` \
         in Ruxen.toml or the RUXEN_TARGET_<TRIPLE>_LINKER env var.",
        target.canonical(),
        target.canonical()
    ))
}

/// The Cranelift/cc architecture tag for a target ("x86_64" / "aarch64").
fn arch_tag(target: &ResolvedTarget) -> &'static str {
    match target.triple().architecture {
        Architecture::X86_64 | Architecture::X86_64h => "x86_64",
        Architecture::Aarch64(_) => "aarch64",
        Architecture::Riscv64(_) => "riscv64",
        Architecture::S390x => "s390x",
        _ => "unknown",
    }
}

/// The `cc -arch <flag>` value for a Darwin cross. Apple's `cc` spells
/// aarch64 as `arm64`.
fn darwin_arch_flag(arch: &str) -> &str {
    match arch {
        "aarch64" => "arm64",
        other => other,
    }
}

/// The Docker platform string for a Linux target's two-stage container link.
pub fn docker_platform(target: &ResolvedTarget) -> &'static str {
    match arch_tag(target) {
        "aarch64" => "linux/arm64",
        "x86_64" => "linux/amd64",
        _ => "linux/amd64",
    }
}

/// Host OS, passed explicitly to keep [`linker_for`] pure/testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostOs {
    Darwin,
    Linux,
    Other,
}

/// Host arch, passed explicitly to keep [`linker_for`] pure/testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostArch {
    X86_64,
    Aarch64,
    Other,
}

impl HostOs {
    /// The current build host's OS (from `cfg!`).
    pub fn current() -> Self {
        #[cfg(target_os = "macos")]
        {
            HostOs::Darwin
        }
        #[cfg(target_os = "linux")]
        {
            HostOs::Linux
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            HostOs::Other
        }
    }
}

impl HostArch {
    /// The current build host's arch (from `cfg!`).
    pub fn current() -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            HostArch::X86_64
        }
        #[cfg(target_arch = "aarch64")]
        {
            HostArch::Aarch64
        }
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            HostArch::Other
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_target_is_host() {
        let t = ResolvedTarget::resolve(None).unwrap();
        assert!(t.is_host());
        let empty = ResolvedTarget::resolve(Some("  ")).unwrap();
        assert!(empty.is_host(), "blank --target falls back to host");
    }

    #[test]
    fn canonical_triples_parse() {
        for s in [
            "aarch64-apple-darwin",
            "x86_64-apple-darwin",
            "aarch64-unknown-linux-gnu",
            "x86_64-unknown-linux-gnu",
            "aarch64-linux-android",
            "wasm32-unknown-unknown",
        ] {
            let t = ResolvedTarget::resolve(Some(s)).unwrap();
            assert_eq!(t.canonical(), s, "canonical round-trip for {s}");
            assert!(!t.is_host());
        }
    }

    #[test]
    fn aliases_expand_to_canonical() {
        // The spec claims target-lexicon does this; it does NOT in 0.13
        // (it parses x86_64-macos to os=Unknown/Elf). Our table fixes it.
        let cases = [
            ("aarch64-linux", "aarch64-unknown-linux-gnu"),
            ("x86_64-linux", "x86_64-unknown-linux-gnu"),
            ("x86_64-macos", "x86_64-apple-darwin"),
            ("aarch64-macos", "aarch64-apple-darwin"),
            ("aarch64-darwin", "aarch64-apple-darwin"),
            ("wasm32", "wasm32-unknown-unknown"),
            ("android", "aarch64-linux-android"),
        ];
        for (alias, canonical) in cases {
            let t = ResolvedTarget::resolve(Some(alias)).unwrap();
            assert_eq!(t.canonical(), canonical, "alias {alias} → {canonical}");
        }
    }

    #[test]
    fn alias_expansion_fixes_lossy_parse() {
        // Direct proof the alias table is load-bearing: x86_64-macos must
        // resolve to a Darwin OS, not the lossy Unknown/Elf target-lexicon
        // produces for the bare alias.
        let t = ResolvedTarget::resolve(Some("x86_64-macos")).unwrap();
        assert!(t.is_darwin(), "x86_64-macos must be a Darwin target");
        assert_eq!(t.cfg_context().target_os, "macos");
    }

    #[test]
    fn invalid_triple_errors() {
        let err = ResolvedTarget::resolve(Some("not-a-real-@triple")).unwrap_err();
        assert!(err.contains("invalid target triple"), "got: {err}");
    }

    #[test]
    fn wasm_requires_llvm() {
        let t = ResolvedTarget::resolve(Some("wasm32-unknown-unknown")).unwrap();
        assert!(t.requires_llvm_backend());
        let linux = ResolvedTarget::resolve(Some("aarch64-unknown-linux-gnu")).unwrap();
        assert!(!linux.requires_llvm_backend());
    }

    #[test]
    fn cfg_context_linux_gnu() {
        let t = ResolvedTarget::resolve(Some("aarch64-unknown-linux-gnu")).unwrap();
        let c = t.cfg_context();
        assert_eq!(c.target_arch, "aarch64");
        assert_eq!(c.target_os, "linux");
        assert_eq!(c.target_env, "gnu");
        assert_eq!(c.target_family, "unix");
        assert_eq!(c.target_vendor, "unknown");
        assert_eq!(c.target_pointer_width, "64");
        assert_eq!(c.target_endian, "little");
    }

    #[test]
    fn cfg_context_darwin() {
        let t = ResolvedTarget::resolve(Some("x86_64-apple-darwin")).unwrap();
        let c = t.cfg_context();
        assert_eq!(c.target_arch, "x86_64");
        assert_eq!(c.target_os, "macos");
        assert_eq!(c.target_family, "unix");
        assert_eq!(c.target_vendor, "apple");
    }

    #[test]
    fn cfg_context_wasm() {
        let t = ResolvedTarget::resolve(Some("wasm32-unknown-unknown")).unwrap();
        let c = t.cfg_context();
        assert_eq!(c.target_arch, "wasm32");
        assert_eq!(c.target_family, "wasm");
        assert_eq!(c.target_pointer_width, "32");
    }

    #[test]
    fn linker_darwin_to_darwin_cross_local() {
        // x86_64-apple-darwin from an aarch64 macOS host: local `cc -arch x86_64`.
        let t = ResolvedTarget::resolve(Some("x86_64-apple-darwin")).unwrap();
        let spec = linker_for(&t, HostOs::Darwin, HostArch::Aarch64, |_| false).unwrap();
        assert_eq!(spec.program, "cc");
        assert_eq!(spec.target_args, vec!["-arch", "x86_64"]);
        assert!(!spec.needs_container);
    }

    #[test]
    fn linker_aarch64_darwin_arch_flag_is_arm64() {
        let t = ResolvedTarget::resolve(Some("aarch64-apple-darwin")).unwrap();
        let spec = linker_for(&t, HostOs::Darwin, HostArch::X86_64, |_| false).unwrap();
        assert_eq!(spec.target_args, vec!["-arch", "arm64"]);
    }

    #[test]
    fn linker_linux_no_cross_gcc_needs_container() {
        // aarch64-linux from a macOS host with no cross gcc → container.
        let t = ResolvedTarget::resolve(Some("aarch64-unknown-linux-gnu")).unwrap();
        let spec = linker_for(&t, HostOs::Darwin, HostArch::Aarch64, |_| false).unwrap();
        assert!(spec.needs_container);
        assert_eq!(docker_platform(&t), "linux/arm64");
    }

    #[test]
    fn linker_linux_with_cross_gcc_uses_it() {
        let t = ResolvedTarget::resolve(Some("aarch64-unknown-linux-gnu")).unwrap();
        let spec = linker_for(&t, HostOs::Linux, HostArch::X86_64, |name| {
            name == "aarch64-linux-gnu-gcc"
        })
        .unwrap();
        assert_eq!(spec.program, "aarch64-linux-gnu-gcc");
        assert!(!spec.needs_container);
    }

    #[test]
    fn linker_linux_native_is_plain_cc() {
        let t = ResolvedTarget::resolve(Some("x86_64-unknown-linux-gnu")).unwrap();
        let spec = linker_for(&t, HostOs::Linux, HostArch::X86_64, |_| false).unwrap();
        assert_eq!(spec.program, "cc");
        assert!(!spec.needs_container);
    }

    #[test]
    fn linker_host_is_plain_cc() {
        let t = ResolvedTarget::host();
        let spec = linker_for(&t, HostOs::current(), HostArch::current(), |_| false).unwrap();
        assert_eq!(spec.program, "cc");
        assert!(spec.target_args.is_empty());
        assert!(!spec.needs_container);
    }

    #[test]
    fn docker_platform_per_arch() {
        let arm = ResolvedTarget::resolve(Some("aarch64-unknown-linux-gnu")).unwrap();
        let x64 = ResolvedTarget::resolve(Some("x86_64-unknown-linux-gnu")).unwrap();
        assert_eq!(docker_platform(&arm), "linux/arm64");
        assert_eq!(docker_platform(&x64), "linux/amd64");
    }
}
