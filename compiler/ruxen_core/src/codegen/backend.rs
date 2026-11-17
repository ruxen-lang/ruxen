//! The one backend rule (F-29 P3): which code generator a build uses
//! ([`Backend::select`]) and the one place that runs it ([`Backend::emit_object`]).
//!
//! Every driver (`ruxen compile`, `ruxen build`/`run`/`test`, the e2e harness)
//! selects through here, so `--release`, `--backend=`, `--opt-level=` and a
//! target that needs LLVM mean the same thing everywhere.

use crate::codegen::cranelift;
#[cfg(feature = "llvm")]
use crate::codegen::llvm;
use crate::codegen::target::ResolvedTarget;
use crate::mir::nodes::MirProgram;

/// Which code-generation backend to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Cranelift,
    #[cfg(feature = "llvm")]
    Llvm {
        opt_level: u8,
    },
}

impl Backend {
    /// Choose the backend for a build.
    ///
    /// - `backend_override` (`--backend=cranelift|llvm`) wins; otherwise LLVM
    ///   for `release` or a `target` Cranelift cannot emit (wasm), Cranelift
    ///   for everything else.
    /// - `opt_level_override` (`--opt-level=0|1|2|3|s|z`, or a manifest
    ///   profile's `opt-level`) sets LLVM's level; without it a release build
    ///   is `2` and a debug build `0`. Cranelift has no levels.
    ///
    /// A request that needs LLVM from a toolchain built without the `llvm`
    /// feature is an error naming the feature, never a silent Cranelift build.
    /// So is an unknown value and `cranelift` for a target that needs LLVM.
    pub fn select(
        release: bool,
        backend_override: Option<&str>,
        opt_level_override: Option<&str>,
        target: &ResolvedTarget,
    ) -> Result<Backend, String> {
        let opt_level = match opt_level_override {
            None => {
                if release {
                    2
                } else {
                    0
                }
            }
            Some("0") => 0,
            Some("1") => 1,
            Some("2") => 2,
            Some("3") => 3,
            Some("s") => 4,
            Some("z") => 5,
            Some(other) => {
                return Err(format!(
                    "unknown opt level `{other}` (expected 0, 1, 2, 3, s or z)"
                ))
            }
        };
        let needs_llvm = match backend_override {
            Some("llvm") => Some("`--backend=llvm`".to_string()),
            Some("cranelift") if target.requires_llvm_backend() => {
                return Err(format!(
                    "target '{}' requires the LLVM backend; drop `--backend=cranelift`",
                    target.canonical()
                ))
            }
            Some("cranelift") => None,
            Some(other) => {
                return Err(format!(
                    "unknown backend `{other}` (expected cranelift or llvm)"
                ))
            }
            None if target.requires_llvm_backend() => {
                Some(format!("target '{}'", target.canonical()))
            }
            None if release => Some("`--release`".to_string()),
            None => None,
        };
        let Some(reason) = needs_llvm else {
            return Ok(Backend::Cranelift);
        };
        #[cfg(feature = "llvm")]
        {
            let _ = reason;
            Ok(Backend::Llvm { opt_level })
        }
        #[cfg(not(feature = "llvm"))]
        {
            let _ = opt_level;
            Err(format!(
                "{reason} needs the LLVM backend, but this ruxen was built without the \
                 `llvm` feature; rebuild it with `--features llvm` (LLVM 18)"
            ))
        }
    }

    /// Lower `program` to an object file for `target` (the host when
    /// `target.is_host()`).
    pub fn emit_object(
        &self,
        program: &MirProgram,
        target: &ResolvedTarget,
    ) -> Result<Vec<u8>, String> {
        match *self {
            Backend::Cranelift => {
                let mut codegen = cranelift::CodeGen::new_for_target(Some(target))?;
                codegen.compile_program(program)?;
                codegen.finish()
            }
            #[cfg(feature = "llvm")]
            Backend::Llvm { opt_level } => {
                let triple = (!target.is_host()).then(|| target.canonical().to_string());
                let mut codegen = llvm::CodeGen::new_for_target(opt_level, triple)?;
                codegen.compile_program(program)?;
                codegen.finish()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> ResolvedTarget {
        ResolvedTarget::host()
    }

    fn wasm() -> ResolvedTarget {
        ResolvedTarget::resolve(Some("wasm32")).unwrap()
    }

    #[test]
    fn debug_build_defaults_to_cranelift() {
        assert_eq!(
            Backend::select(false, None, None, &host()),
            Ok(Backend::Cranelift)
        );
    }

    #[test]
    fn explicit_cranelift_overrides_release() {
        assert_eq!(
            Backend::select(true, Some("cranelift"), None, &host()),
            Ok(Backend::Cranelift)
        );
    }

    #[test]
    fn unknown_values_are_errors() {
        let backend = Backend::select(false, Some("gcc"), None, &host()).unwrap_err();
        assert!(backend.contains("unknown backend `gcc`"), "{backend}");
        let opt = Backend::select(true, None, Some("4"), &host()).unwrap_err();
        assert!(opt.contains("unknown opt level `4`"), "{opt}");
    }

    #[test]
    fn cranelift_for_a_wasm_target_is_an_error() {
        let err = Backend::select(false, Some("cranelift"), None, &wasm()).unwrap_err();
        assert!(err.contains("requires the LLVM backend"), "{err}");
    }

    #[cfg(feature = "llvm")]
    #[test]
    fn release_selects_llvm_at_the_requested_opt_level() {
        assert_eq!(
            Backend::select(true, None, None, &host()),
            Ok(Backend::Llvm { opt_level: 2 })
        );
        assert_eq!(
            Backend::select(true, None, Some("3"), &host()),
            Ok(Backend::Llvm { opt_level: 3 })
        );
    }

    #[cfg(feature = "llvm")]
    #[test]
    fn wasm_target_selects_llvm_in_a_debug_build() {
        assert_eq!(
            Backend::select(false, None, None, &wasm()),
            Ok(Backend::Llvm { opt_level: 0 })
        );
    }

    #[cfg(not(feature = "llvm"))]
    #[test]
    fn release_without_the_llvm_feature_is_an_error() {
        let err = Backend::select(true, None, None, &host()).unwrap_err();
        assert!(
            err.contains("`--release` needs the LLVM backend") && err.contains("`llvm` feature"),
            "{err}"
        );
    }

    #[cfg(not(feature = "llvm"))]
    #[test]
    fn wasm_target_without_the_llvm_feature_is_an_error() {
        let err = Backend::select(false, None, None, &wasm()).unwrap_err();
        assert!(
            err.contains("target 'wasm32-unknown-unknown' needs the LLVM backend"),
            "{err}"
        );
    }
}
