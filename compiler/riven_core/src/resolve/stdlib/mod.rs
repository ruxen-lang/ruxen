//! Built-in stdlib registrations.
//!
//! Phase C of #06.75 carved this out of `resolve/mod.rs` (where it lived
//! as the 2156-LOC `Resolver::register_builtins` method).  Every primitive
//! type, builtin trait, free function, stdlib class (IoError / File /
//! Command / Metadata / Duration / Instant / Formatter / …), the
//! `std::{io,fs,net,time,process,env,fmt,…}` module tree, and the
//! Option/Result enum types is wired here.
//!
//! Prompt 06.8 follow-up: the original single-file `register_all`
//! was split into per-namespace files (`primitives.rs`, `modules.rs`,
//! `type_constructors.rs`, `option.rs`, `result.rs`). This module
//! now only orchestrates the calls in the order the resolver expects.
//!
//! Historical context worth preserving here (NOT duplicated in the
//! sibling files):
//!
//! - Phase D-3 of #06.95: the 16 builtin mixins (Displayable / Error /
//!   Comparable / Iterable / Copy / Clone / Send / Sync / PartialEq /
//!   Eq / Hash / Default / Ord / PartialOrd / Drop / Future / Into[T])
//!   moved to `library/std/core/src/lib.rvn` as self-hosted `mixin Foo`
//!   declarations. The bootstrap merge picks them up as
//!   `DefKind::Trait` entries. The `Hash → Hashable` deprecation alias
//!   is re-established by `fixup_bootstrapped_stdlib_modules` via its
//!   TYPE_ALIASES table.
//!
//! - Wave 2 (#06.8): every historical stdlib class/enum shell (IoError,
//!   IoErrorKind, Stdin/Stdout/Stderr, Metadata, Command/ExitStatus/
//!   Output, File/OpenOptions, SeekFrom, Duration/Instant, TcpListener/
//!   TcpStream, BufReader[R]/BufWriter[W], Shutdown, Formatter/FmtError,
//!   the 9 std.sync class shells, Arc[T] alias, ThreadId shim) migrated
//!   to its respective `library/std/<pkg>/src/lib.rvn` file. Tag-stability
//!   for the runtime-pinned enums (IoError, SeekFrom, Shutdown, Poll)
//!   is now enforced by tests scanning the .rvn enum bodies.
//!
//! - Phase D of #06.95 deleted the last three Rust-side builtin free
//!   fns (`sleep`, `signal_install_sigint`, `signal_received_sigint`).
//!   Their .rvn equivalents live in `library/std/sync/src/lib.rvn`.

use crate::hir::types::Ty;
use crate::lexer::token::Span;
use crate::parser::ast::Visibility;

use super::symbols::*;
use super::Resolver;

mod modules;
mod option;
mod primitives;
mod result;
mod type_constructors;

/// Register every built-in into the resolver.
///
/// Called once from `Resolver::register_builtins` at the start of a
/// resolution run, before any user code is walked.
pub(super) fn register_all(r: &mut Resolver) {
    primitives::register_primitives(r);
    modules::register_modules(r);
    type_constructors::register_type_constructors(r);
    option::register_option(r);
    result::register_result(r);
    register_super(r);
}

/// Register `super` as a built-in function (for parent class constructor calls).
fn register_super(r: &mut Resolver) {
    let span = Span {
        start: 0,
        end: 0,
        line: 0,
        column: 0,
    };
    let super_id = r.symbols.define(
        "super".to_string(),
        DefKind::Function {
            signature: FnSignature {
                self_mode: None,
                is_class_method: false,
                is_async: false,
                generic_params: vec![],
                params: vec![], // variadic-like; type checker handles it
                return_ty: Ty::Unit,
                c_symbol: None,
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes.insert("super".to_string(), super_id);
}
