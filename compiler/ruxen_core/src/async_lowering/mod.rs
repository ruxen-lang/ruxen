//! AST-level async lowering — Milestone 2A/2B of async sub-phase 2
//! (`docs/specs/syntax/async_lowering.spec.md` B1–B10).
//!
//! This pass runs BEFORE the resolver. For each top-level `async def`
//! it synthesises a state-machine class that includes `Future` +
//! defines `def var poll(...) -> Poll[T]`, and rewrites the original
//! function so its body returns `__<FnName>Future.new(args...)`.
//!
//! By emitting the synthesised class as if it were user-written, the
//! resolver + typeck + MIR pipeline does its normal job — drop
//! elaboration, mixin satisfaction, codegen — without any
//! state-machine special-case downstream.
//!
//! Module layout (Phase 3 split):
//!   * [`cfg`] — the await-delimited control-flow graph (`segment_cfg`
//!     produces a `Cfg` of `Segment`/`Edge`s for the non-loop shapes;
//!     loop shapes carry an `Edge::Loop` back-edge).
//!   * [`lower`] — the lowering itself: the public entry points, the
//!     unified non-loop `Cfg`-driven path, the three loop builders, the
//!     awaitee classifier, and the `block_on` poll-loop rewriter.
//!   * [`diagnostics`] — the E1112 / E1115 / E1116 collectors over the
//!     `Visit` trait.
//!
//! This `mod.rs` is the facade: it wires the submodules and re-exports
//! the stable public surface (`lower_async_defs`,
//! `lower_async_defs_with_bootstrap`, and the diagnostics collectors).
//! `cfg` and `diagnostics` reach the shared await-scan + loop
//! recognizers through the `pub(crate)` re-exports below, keeping their
//! `super::` paths intact across the split.

mod cfg;
mod diagnostics;
mod lower;

pub use diagnostics::*;
pub use lower::{lower_async_defs, lower_async_defs_with_bootstrap};

// Shared helpers the sibling modules reach via `super::` / `super::super::`.
// `cfg` uses the await-scan; `cfg`'s acceptance cross-check and
// `diagnostics`' E1115 collector use the loop recognizers. Re-exported
// here (not moved) so those paths resolve after the lowering moved into
// `lower.rs`.
pub(crate) use lower::{
    block_contains_await, expr_contains_await, recognize_while_multi_await,
    recognize_while_single_await,
};
