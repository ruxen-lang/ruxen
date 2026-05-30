//! Canonical list of C runtime functions, grouped by namespace.
//!
//! This module is data-only. The per-namespace slices document which C
//! runtime symbols the link step must resolve, grouped to match the
//! header comments in `runtime/runtime.c`. They are intentionally
//! `pub(super)`-only: the codegen pipeline never iterates the catalogue
//! (callee names reach the linker through MIR's `ffi_alias_map` and
//! `lang_intrinsics::runtime_name`), but keeping the list in tree-form
//! makes it easy to audit what's wired up.
//!
//! If you ever need a flat list for tooling, chain the slices below
//! with `io_fmt::IO.iter().chain(io_fmt::FMT.iter()).chain(...).copied()`.
#![allow(dead_code)]

pub(super) mod collections;
pub(super) mod concurrency;
pub(super) mod fs_string;
pub(super) mod io_fmt;
