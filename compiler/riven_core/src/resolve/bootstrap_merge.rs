#![allow(unused_imports)]

use std::collections::HashMap;

use crate::diagnostics::Diagnostic;
use crate::hir::context::TypeContext;
use crate::hir::nodes::*;
use crate::hir::types::{MixinRef, MoveSemantics, Ty};
use crate::lexer::token::Span;
use crate::parser::ast::{self, Visibility};

use super::const_helpers;
use super::scope::{ScopeId, ScopeKind, ScopeStack};
use super::symbols::*;
use super::{ClosureCaptureContext, ResolveResult, Resolver};

impl Resolver {
    pub fn resolve(self, program: &ast::Program) -> ResolveResult {
        self.resolve_with_bootstrap(program, &[])
    }

    /// Run name resolution with stdlib bootstrap programs merged into
    /// the prelude. `bootstrap_programs` are typically the output of
    /// [`crate::resolve::bootstrap::run_bootstrap`] — each one is a
    /// parsed `.rvn` file from `library/std/<pkg>/src/lib.rvn`.
    ///
    /// Bootstrap programs are dispatched through the same Pass-1
    /// forward-declaration logic as user code (see
    /// [`merge_bootstrap_programs`](Self::merge_bootstrap_programs))
    /// AFTER `register_builtins` and BEFORE the user's program. They
    /// share the user's `ffi_libs` vector, so any `lib`/`extern` blocks
    /// inside a stdlib file land on `HirProgram.ffi_libs` exactly like
    /// user-side decls do — the MIR lowerer and linker do not need to
    /// know whether a given C-symbol alias came from user code or from
    /// the bootstrap prelude.
    ///
    /// Pass 2 (full resolution / body lowering) only runs on the user
    /// program. Bootstrap items are forward-declared into the resolver
    /// scope but their bodies are not lowered here — Wave 1.5 stdlib
    /// files are signature-only (FFI aliases), so there are no bodies
    /// to lower. Wave 2 will revisit this when the first stdlib file
    /// with actual Riven function bodies arrives.
    pub fn resolve_with_bootstrap(
        mut self,
        program: &ast::Program,
        bootstrap_programs: &[ast::Program],
    ) -> ResolveResult {
        self.register_builtins();

        // Collected FFI library declarations (filled by the
        // `Lib`/`Extern` arms in `register_top_level_type`). Surfaced
        // on `HirProgram::ffi_libs` so MIR lowering can populate
        // `MirProgram::ffi_libs` and rewrite call-site callees to the
        // declared C symbol. The bootstrap merge below contributes to
        // the SAME vector as user code — there is one ffi_libs list
        // per compilation, period.
        let mut ffi_libs: Vec<HirFfiLib> = Vec::new();

        // Merge stdlib bootstrap programs BEFORE the user's pass 1 so
        // forward references from user code into stdlib symbols (e.g.
        // a user `def main()` calling a bootstrap-loaded `bootstrap_smoke_add_one(...)`)
        // resolve cleanly.
        self.merge_bootstrap_programs(bootstrap_programs, &mut ffi_libs);

        // Wave 2 (#06.8): re-bind stdlib module items to bootstrap-loaded
        // FFI fn DefIds. `register_builtins` ran BEFORE the bootstrap
        // merge and pre-registered modules like `std.rand` with empty
        // `items` because the fn DefIds didn't exist yet; we patch them
        // up now so `use std.rand.{random_bytes, …}` resolution finds
        // the bootstrap-loaded functions through the Module item walk
        // in `resolve_child_in_def`.
        self.fixup_bootstrapped_stdlib_modules();

        // #06.95 Phase A pre-flight: snapshot every mixin's lib_decls
        // BEFORE Pass 1 so the Class arm can re-register them under
        // any class that `include`s the mixin, regardless of source
        // order between class and mixin. Walks both bootstrap and
        // user programs so a user class can include a stdlib mixin
        // (and vice versa).
        self.collect_mixin_lib_decls(bootstrap_programs.iter().chain(std::iter::once(program)));

        // Two-pass approach:
        // Pass 1: Register all top-level type names (classes, structs, enums, traits)
        //         so that forward references work.
        for item in &program.items {
            self.register_top_level_type_with_ffi(item, &mut ffi_libs);
        }

        // Scan for functions that contain `yield` — these receive a
        // synthetic `__block` parameter, and callers with a trailing block
        // forward it as the last argument.
        for item in &program.items {
            super::yield_scan::collect_yield_fns(item, &mut self.yield_fns);
        }

        // Pass 2: Fully resolve all items.
        let mut items = Vec::new();
        for item in &program.items {
            if let Some(hir_item) = self.resolve_item(item) {
                items.push(hir_item);
            }
        }

        let hir_program = HirProgram {
            items,
            span: program.span.clone(),
            ffi_libs,
        };

        ResolveResult {
            program: hir_program,
            symbols: self.symbols,
            type_context: self.type_context,
            diagnostics: self.diagnostics,
        }
    }

    /// Merge parsed stdlib `Program`s into the resolver scope.
    ///
    /// Called by [`resolve_with_bootstrap`](Self::resolve_with_bootstrap)
    /// between `register_builtins()` and the user's pass-1 forward-decl
    /// loop. Each stdlib program's top-level items are dispatched
    /// through the same registration path that user code uses
    /// (`register_top_level_type_with_ffi`) — there is no separate
    /// stdlib grammar, no separate resolver path, no special-casing.
    ///
    /// Wave 1.5 only supports the variants needed by today's bootstrap
    /// content (`Lib`, `Extern`, `Function`, `Class`, `Struct`, `Enum`,
    /// `Const`). `Mixin`/`Impl`/`Module`/`Use`/`TypeAlias`/`Newtype`
    /// inside a stdlib file are silently skipped for now — they will
    /// land in Wave 2 when the first stdlib module that uses them
    /// migrates. The conservative skip keeps the Phase 3 surface
    /// minimal; adding them later is purely additive.
    /// # Double-registration policy
    ///
    /// `register_builtins` (the Rust-side `resolve/stdlib/mod.rs`)
    /// runs BEFORE this method. If `register_builtins` already
    /// defined a name (e.g. `class File`) that a bootstrap `.rvn`
    /// also defines, the bootstrap version takes precedence —
    /// scope insertions use `HashMap::insert`, which is last-wins.
    /// The old Rust-side `DefId` remains in the symbol table but
    /// becomes unreachable through name resolution.
    ///
    /// This is intentional: it's the migration runway. As stdlib
    /// classes move from Rust registrations to `.rvn` files, the
    /// `.rvn` version naturally wins without flag-day coordination.
    /// Once a class's full surface is migrated, the corresponding
    /// `register_builtins` block can be deleted; the `.rvn` version
    /// has been canonical since it landed.
    pub fn merge_bootstrap_programs(
        &mut self,
        programs: &[ast::Program],
        ffi_libs: &mut Vec<HirFfiLib>,
    ) {
        // #06.8 T#21: while this flag is set the Class arm of
        // `register_top_level_type_with_ffi` switches to namespace-anchor
        // mode for already-known builtin type names (see field doc).
        self.merging_bootstrap = true;
        for program in programs {
            for item in &program.items {
                if Self::is_bootstrap_supported_item(item) {
                    self.register_top_level_type_with_ffi(item, ffi_libs);
                }
            }
            // Bootstrap files are part of the prelude, so yield scanning
            // applies to them too — a stdlib helper that uses `yield`
            // needs its `__block` parameter wired up the same way a
            // user function does.
            for item in &program.items {
                if Self::is_bootstrap_supported_item(item) {
                    super::yield_scan::collect_yield_fns(item, &mut self.yield_fns);
                }
            }
        }
        self.merging_bootstrap = false;
    }

    /// Wave 2 (#06.8): re-bind stdlib module item lists to the FFI fn
    /// DefIds that the bootstrap loader just inserted into the prelude
    /// scope. This is the bridge between two facts of life:
    ///
    /// 1. `register_builtins` (`resolve/stdlib/mod.rs`) runs FIRST and
    ///    assembles the `std.{io,fs,net,time,rand,…}` module tree.
    ///    When it constructs `std.rand`, the random_* fn DefIds don't
    ///    exist yet because the bootstrap merge hasn't run — so the
    ///    pre-registered Module's `items` vector starts empty.
    /// 2. `use std.rand.random_bytes` resolution
    ///    (`resolve_child_in_def`) walks the Module's `items` list.
    ///    Empty list ⇒ "'random_bytes' not found in module 'rand'".
    ///
    /// This function closes the gap by walking a small mapping of
    /// `(module-name, &[fn-name…])` tuples; for each entry it looks
    /// up the module DefId in `scopes.lookup_type` and each fn DefId
    /// in `scopes.lookup`, then mutates the Module's items via
    /// `symbols.get_mut`.
    ///
    /// The mapping is intentionally a static array rather than scanned
    /// from the .rvn files themselves: every migration commit
    /// (`stdlib(<module>): migrate from Rust registrations to Riven
    /// source`) appends one row and deletes the corresponding builtin_fn
    /// entries. When the migration epic completes the array AND this
    /// function vanish along with `resolve/stdlib/mod.rs`'s module
    /// assembly — a `.rvn`-defined `module rand … end` (or the
    /// bootstrap-merge-auto-wraps-each-file behaviour landing later in
    /// the epic) becomes the only registration site.
    pub(super) fn fixup_bootstrapped_stdlib_modules(&mut self) {
        // (stdlib-module-name, &[item-names-the-.rvn-defines])
        //
        // `register_builtins` inserts the outer `std` module into the
        // type scope (so `std.X` resolution works) but does NOT
        // insert submodules (`std.rand`, `std.io`, …) into any scope —
        // they are only reachable by walking `std`'s `items` list.
        // The fixup therefore looks each `module_name` up by walking
        // `std`'s items rather than calling `scopes.lookup_type`
        // directly.
        //
        // For each item name in a fixup row we try the value scope
        // first (free fns from a `lib` block — rand/path/env case)
        // and fall back to the type scope (mixins / classes / enums —
        // fmt case). This keeps a single table covering both shapes
        // without doubling the data structure.
        const FIXUPS: &[(&str, &[&str])] = &[
            // Wave 2 — library/std/rand/src/lib.rvn
            ("rand", &["random_bytes", "random_u64", "random_fill"]),
            // Wave 2 — library/std/path/src/lib.rvn
            (
                "path",
                &[
                    "path_join",
                    "path_parent",
                    "path_file_name",
                    "path_extension",
                    "path_is_absolute",
                ],
            ),
            // Wave 2 — library/std/env/src/lib.rvn
            ("env", &["args", "get", "vars", "current_dir"]),
            // Wave 2 — library/std/fmt/src/lib.rvn (mixed mixins + classes —
            // each name resolves through the type-scope fallback).
            ("fmt", &["Display", "Debug", "Formatter", "FmtError"]),
            // Wave 2 — library/std/net/src/lib.rvn (class shells +
            // Shutdown enum; methods still flow through the static-ctor
            // + runtime_table dispatch until T#20 lands).
            ("net", &["TcpListener", "TcpStream", "Shutdown"]),
            // Wave 2 — library/std/process/src/lib.rvn (`exit` free fn +
            // Command / Output / ExitStatus class shells; class methods
            // still go through static-ctor + runtime_table until T#20).
            ("process", &["exit", "Command", "Output", "ExitStatus"]),
            // Wave 2 — library/std/time/src/lib.rvn (`unix_ns` free fn +
            // Duration / Instant class shells; class methods still go
            // through static-ctor + runtime_table until T#20).
            ("time", &["unix_ns", "Duration", "Instant"]),
            // Wave 2 — library/std/fs/src/lib.rvn (17 fs free fns +
            // Metadata class shell; also re-exports File for the
            // `use std.fs.File` import alias).
            (
                "fs",
                &[
                    "read_to_string",
                    "write",
                    "exists",
                    "is_file",
                    "is_dir",
                    "read_dir",
                    "metadata",
                    "remove_file",
                    "create_dir",
                    "create_dir_all",
                    "rename",
                    "copy",
                    "remove_dir_all",
                    "canonicalize",
                    "write_atomic",
                    "read_link",
                    "symlink",
                    "Metadata",
                    "File",
                ],
            ),
            // Wave 2 — library/std/io/src/lib.rvn (9 free fns + 7 class
            // shells + IoError / IoErrorKind / SeekFrom enums).
            (
                "io",
                &[
                    "puts",
                    "eputs",
                    "print",
                    "println",
                    "eprintln",
                    "read_line",
                    "stdin",
                    "stdout",
                    "stderr",
                    "IoError",
                    "IoErrorKind",
                    "Stdin",
                    "Stdout",
                    "Stderr",
                    "File",
                    "OpenOptions",
                    "SeekFrom",
                    "BufReader",
                    "BufWriter",
                ],
            ),
            // Wave 2 — library/std/sync/src/lib.rvn (9 class shells:
            // Thread, ThreadId, JoinHandle, Mutex, MutexGuard,
            // SharedSync, PoisonError, ThreadPanic + Context /
            // Waker from std::task). `SharedSync` is the Ruby-
            // naming canonical name (TEC-13 / §10a); `Arc` stays
            // Rust-registered as a backward-compat alias class
            // whose type_constructors Variable produces
            // `Ty::Class { name: "SharedSync" }`. Methods still
            // flow through runtime_table mangled-name dispatch
            // until T#21.
            (
                "sync",
                &[
                    "Thread",
                    "ThreadId",
                    "JoinHandle",
                    "Mutex",
                    "MutexGuard",
                    "SharedSync",
                    "PoisonError",
                    "ThreadPanic",
                    "Context",
                    "Waker",
                ],
            ),
        ];

        let Some(std_id) = self.scopes.lookup_type("std") else {
            return;
        };
        // Snapshot std's items so we don't hold an immutable borrow of
        // `self.symbols` across the mutable `get_mut` calls below.
        let std_items: Vec<DefId> = match self.symbols.get(std_id).map(|d| &d.kind) {
            Some(DefKind::Module { items }) => items.clone(),
            _ => return,
        };

        // Wave 2 (#06.8): re-establish stdlib type aliases whose target
        // moved from a Rust registration to a bootstrap-loaded `.rvn`
        // file. Today this is just `Hash → Hashable` (the TEC-13
        // deprecation alias), but the table is the obvious place to
        // add new aliases as other migrations land.
        //
        // The `Hash[K, V]` collection type has its OWN type-position
        // resolver path (see `resolve_type_path`'s explicit `Hash` /
        // `Vec` / `Set` arms) and is unaffected by this alias.
        const TYPE_ALIASES: &[(&str, &str)] = &[("Hash", "Hashable")];
        for (alias, target) in TYPE_ALIASES {
            if let Some(target_id) = self.scopes.lookup_type(target) {
                // Only insert when missing — never overwrite a real
                // scope entry. (Today `Hash` is unbound after the
                // Hashable migration; tomorrow it might be a real
                // mixin in its own right.)
                if self.scopes.lookup_type(alias).is_none() {
                    self.scopes.insert_type(alias.to_string(), target_id);
                    self.type_registry.insert(alias.to_string(), target_id);
                }
            }
        }

        for (module_name, fn_names) in FIXUPS {
            // Find the submodule DefId by name inside std's items.
            let Some(&module_id) = std_items.iter().find(|&&id| {
                self.symbols
                    .get(id)
                    .map(|d| d.name == *module_name && matches!(d.kind, DefKind::Module { .. }))
                    .unwrap_or(false)
            }) else {
                continue;
            };
            let mut fn_ids: Vec<DefId> = Vec::with_capacity(fn_names.len());
            for fn_name in *fn_names {
                // Value scope first (free fns), type scope as fallback
                // (mixins / classes / enums).
                let item_id = self
                    .scopes
                    .lookup(fn_name)
                    .or_else(|| self.scopes.lookup_type(fn_name));
                if let Some(id) = item_id {
                    fn_ids.push(id);
                }
            }
            if let Some(def) = self.symbols.get_mut(module_id) {
                if let DefKind::Module { items } = &mut def.kind {
                    // APPEND the looked-up DefIds rather than
                    // overwriting. Most stdlib modules start with
                    // empty items so append == replace for them, but
                    // a few (e.g. std.sync) keep one-off Rust shims
                    // like the ThreadId value-scope Variable that
                    // need to coexist with bootstrap-loaded class
                    // DefIds. Append keeps both working without a
                    // second fixup mechanism.
                    items.extend(fn_ids);
                }
            }
        }
    }

    /// Whitelist of `TopLevelItem` variants the bootstrap merge path
    /// handles. See [`merge_bootstrap_programs`] for the rationale on
    /// what is deferred.
    ///
    /// Wave 2 (#06.8): `Mixin` added to unblock `iter.rvn` (Iterator /
    /// FromIterator), `hash.rvn` (Hashable), and `fmt.rvn`
    /// (Display / Debug). The same Mixin arm in
    /// `register_top_level_type_with_ffi` that handles user code
    /// already does the right thing — it registers the mixin as a
    /// `DefKind::Trait` and inserts into the type scope — so no new
    /// resolver logic is needed, only the whitelist gate.
    ///
    /// `Impl` remains deferred: top-level `impl X for Y` blocks in
    /// stdlib files would require Pass-2 resolution (associated-type
    /// bindings, method bodies) at bootstrap time, and no stdlib
    /// module needs that surface in v1.
    pub(super) fn is_bootstrap_supported_item(item: &ast::TopLevelItem) -> bool {
        matches!(
            item,
            ast::TopLevelItem::Lib(_)
                | ast::TopLevelItem::Extern(_)
                | ast::TopLevelItem::Function(_)
                | ast::TopLevelItem::Class(_)
                | ast::TopLevelItem::Struct(_)
                | ast::TopLevelItem::Enum(_)
                | ast::TopLevelItem::Const(_)
                | ast::TopLevelItem::Mixin(_)
        )
    }

    // ─── Builtin Registration ───────────────────────────────────────

}
