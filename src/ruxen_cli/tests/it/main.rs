//! The ruxen_cli integration tests, one binary: each file in this directory
//! is a module. Run one with `cargo test -p ruxen_cli --test it -- <module>::`.
//!
//! Tests here share one process: never mutate process-wide state
//! (environment variables, the current directory). The one exception,
//! `publish_e2e`, changes the current directory under its own lock, so no
//! other test here may depend on the current directory.

#[path = "../common/mod.rs"]
mod common;

mod check_front_end;
mod cross_package_mono;
mod dep_visibility;
mod explain_long_form;
mod ffi_dep_link;
mod installed_pkg_manager;
mod publish_e2e;
mod workspace_e2e;
