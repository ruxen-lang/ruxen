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
    /// Package-aware variant of [`resolve_with_bootstrap`]. Pairs each
    /// bootstrap program with its package name (e.g. `"io"`,
    /// `"fs"`), so the resolver can auto-populate each `std.<pkg>`
    /// submodule's `items` list from the program's top-level
    /// declarations. Production callers (`typeck::type_check`) use
    /// this path; tests with synthetic programs keep the legacy
    /// `resolve_with_bootstrap` and rely on the static FIXUPS table.
    pub fn resolve_with_bootstrap_packages(
        self,
        program: &ast::Program,
        bootstrap_packages: &[(String, ast::Program)],
    ) -> ResolveResult {
        // Extract just the programs for the existing merge path…
        let bootstrap_programs: Vec<ast::Program> =
            bootstrap_packages.iter().map(|(_, p)| p.clone()).collect();
        // …and remember the (pkg_name, item_names) pairs so the
        // post-merge fixup can auto-populate each std.<pkg> submodule.
        let auto_pkgs: Vec<(String, Vec<String>)> = bootstrap_packages
            .iter()
            .map(|(name, prog)| {
                let items = Self::top_level_item_names(prog);
                (name.clone(), items)
            })
            .collect();
        let mut resolver = self;
        resolver.bootstrap_auto_packages = auto_pkgs;
        resolver.resolve_with_bootstrap(program, &bootstrap_programs)
    }

    /// Collect the names of every top-level item the program declares
    /// (free fns, classes, enums, traits, mixins, modules). Used by
    /// the package-aware fixup to find DefIds that should populate the
    /// matching `std.<pkg>` submodule.
    fn top_level_item_names(program: &ast::Program) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for item in &program.items {
            match item {
                ast::TopLevelItem::Function(f) => names.push(f.name.clone()),
                ast::TopLevelItem::Class(c) => names.push(c.name.clone()),
                ast::TopLevelItem::Struct(s) => names.push(s.name.clone()),
                ast::TopLevelItem::Enum(e) => names.push(e.name.clone()),
                ast::TopLevelItem::Mixin(m) => names.push(m.name.clone()),
                ast::TopLevelItem::Module(m) => names.push(m.name.clone()),
                ast::TopLevelItem::TypeAlias(a) => names.push(a.name.clone()),
                ast::TopLevelItem::Newtype(n) => names.push(n.name.clone()),
                ast::TopLevelItem::Const(c) => names.push(c.name.clone()),
                ast::TopLevelItem::Lib(lib) => {
                    for f in &lib.functions {
                        names.push(f.name.clone());
                    }
                }
                _ => {}
            }
        }
        names
    }

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

        // #06.95 Phase A pre-flight: snapshot every mixin's lib_decls
        // BEFORE the bootstrap merge so the Class arm can re-register
        // them under any class that `include`s the mixin, regardless
        // of source order between class and mixin. Walks both
        // bootstrap and user programs so a user class can include a
        // stdlib mixin (and vice versa).
        //
        // Phase E.E of #06.95: this MUST run BEFORE
        // `merge_bootstrap_programs` — the merge processes nested
        // module bodies (e.g. `module BufReader { class File ...
        // include Reader }`), and the include-propagation lookup
        // depends on `mixin_lib_decls` already containing the
        // qualified-name key `"BufReader.Reader"`. The previous
        // ordering ran the merge first, finding an empty map, so
        // every `include Mixin` inside a module silently failed to
        // propagate lib decls — culminating in linker errors like
        // `_BufReader_File_read_line undefined`.
        self.collect_mixin_lib_decls(bootstrap_programs.iter().chain(std::iter::once(program)));

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

        // Two-pass approach:
        // Pass 1: Register all top-level type names (classes, structs, enums, traits)
        //         so that forward references work.
        let user_ctx = super::ffi_registration::RegistrationCtx::user_program();
        for item in &program.items {
            self.register_top_level_type_with_ffi(item, &mut ffi_libs, user_ctx);
        }

        // Scan for functions that contain `yield` — these receive a
        // synthetic `__block` parameter, and callers with a trailing block
        // forward it as the last argument. (Bootstrap programs were
        // already scanned by `merge_bootstrap_programs` above.)
        for item in &program.items {
            super::yield_scan::collect_yield_fns(item, &mut self.yield_fns);
        }

        // B1 of `docs/specs/system/zero_rust_stdlib_classes.spec.md`:
        // resolve bootstrap class bodies / free-fn bodies so user-side
        // methods (`def init`, `def poll`, `def drop`) declared on a
        // stdlib `.rvn` class actually execute. Pre-B1 the merge only
        // registered class TYPES + lib-decl FFI methods; method bodies
        // were silently dropped because `resolve_item` ran exclusively
        // over the user program.
        //
        // Ordering: walk in `bootstrap_programs` load order. Each
        // bootstrap program's bodies resolve against the cumulative
        // bootstrap symbol table (already populated above by
        // `merge_bootstrap_programs`) PLUS the user's pass-1 forward
        // decls — bootstrap bodies must not reference user types, but
        // we run after user pass-1 to keep the symbol table monotonic.
        //
        // The resulting HirItems are prepended to the user's items so
        // MIR-lowerer walks them through the same `program.items`
        // pathway as user code — `collect_user_drop_classes` discovers
        // stdlib class drop methods, and codegen lowers any user-body
        // method (e.g. `def var poll` on a hand-written future).
        let mut bootstrap_items: Vec<HirItem> = Vec::new();
        for bs_program in bootstrap_programs {
            for item in &bs_program.items {
                if !Self::is_bootstrap_supported_item(item) {
                    continue;
                }
                if let Some(hir_item) = self.resolve_item(item) {
                    bootstrap_items.push(hir_item);
                }
            }
        }

        // Pass 2: Fully resolve all items.
        let mut items = bootstrap_items;
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
            type_registry: self.type_registry,
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
        // First walk: namespace-anchor mode on, class-body lib decls
        // deferred. The deferral handles cross-class typed returns
        // (docs/specs/types/typed_ffi_returns.spec.md) — a class's
        // lib-decl return types may name OTHER classes declared later
        // in the same file (e.g. `class Mutex[T]; def lock_raw ->
        // MutexGuard[T]` ahead of `class MutexGuard[T]`). Suppressing
        // lib-decl processing on the first walk lets the second walk
        // resolve those once every class name is registered.
        let first_walk_ctx = super::ffi_registration::RegistrationCtx::bootstrap_first_walk();
        for (idx, program) in programs.iter().enumerate() {
            for item in &program.items {
                if Self::is_bootstrap_supported_item(item) {
                    self.register_top_level_type_with_ffi(item, ffi_libs, first_walk_ctx);
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

            // Snapshot per-package DefIds while THIS program's
            // registrations are still the most recent in scope.
            // Without this, a later bootstrap program that re-declares
            // an item with the same name (e.g. `def sleep` in
            // `library/std/sync/src/lib.rvn`'s back-compat shim
            // overwriting the typed `def sleep(d: &Duration) ->
            // TimeSleepFuture` from `library/std/time/src/lib.rvn`)
            // wins the `scopes.lookup` at fixup time and
            // `std.time.items` ends up pointing at the wrong DefId —
            // `use std.time.sleep` then resolves to the shim with
            // `return_ty: Ty::Unit`, and `block_on(sleep(d))` builds
            // a `(&var ()).poll(...)` call that mangles to `()_poll`
            // at link time. See the field doc on
            // `bootstrap_package_item_ids`.
            //
            // `bootstrap_auto_packages` is populated in
            // `resolve_with_bootstrap_packages` and is index-aligned
            // with `programs`. Empty when the legacy
            // `resolve_with_bootstrap` path is in use (test harness
            // with synthetic programs); in that case the snapshot is
            // a no-op and the fixup falls back to `scopes.lookup`.
            if let Some((pkg_name, item_names)) = self.bootstrap_auto_packages.get(idx) {
                let pkg_name = pkg_name.clone();
                let item_names = item_names.clone();
                let mut name_to_id: HashMap<String, DefId> = HashMap::new();
                for name in &item_names {
                    if let Some(id) = self
                        .scopes
                        .lookup(name)
                        .or_else(|| self.scopes.lookup_type(name))
                    {
                        name_to_id.insert(name.clone(), id);
                    }
                }
                if !name_to_id.is_empty() {
                    self.bootstrap_package_item_ids
                        .entry(pkg_name)
                        .or_default()
                        .extend(name_to_id);
                }
            }
        }
        // Second walk: every class name in every bootstrap program is
        // now forward-declared in `type_registry`, so the deferred
        // class-body lib decls can resolve cross-class return types.
        for program in programs {
            for item in &program.items {
                if Self::is_bootstrap_supported_item(item) {
                    self.process_deferred_class_lib_decls(item, ffi_libs, &[]);
                }
            }
        }
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
    /// Phase D of #06.95: walk `bootstrap_auto_packages` and, for each
    /// `(pkg_name, item_names)` pair, append every item's DefId to
    /// the matching `std.<pkg>` submodule's `items` list. Replaces
    /// the bulk per-package entries in the legacy FIXUPS table —
    /// adding a new stdlib package no longer requires touching the
    /// fixup code.
    fn auto_populate_std_submodules_from_packages(&mut self) {
        if self.bootstrap_auto_packages.is_empty() {
            return;
        }
        let Some(std_id) = self.scopes.lookup_type("std") else {
            return;
        };
        let std_items: Vec<DefId> = match self.symbols.get(std_id).map(|d| &d.kind) {
            Some(DefKind::Module { items }) => items.clone(),
            _ => return,
        };

        // Snapshot the package list to release the immutable borrow on
        // `self.bootstrap_auto_packages` before we take mutable refs
        // through `self.symbols.get_mut` below.
        let pkgs = std::mem::take(&mut self.bootstrap_auto_packages);

        for (pkg_name, item_names) in pkgs.iter() {
            let Some(&module_id) = std_items.iter().find(|&&id| {
                self.symbols
                    .get(id)
                    .map(|d| &d.name == pkg_name && matches!(d.kind, DefKind::Module { .. }))
                    .unwrap_or(false)
            }) else {
                continue;
            };
            // Prefer the per-package snapshot captured at merge time:
            // when two bootstrap packages declare items with the same
            // name (`def sleep` in both time.rvn and sync.rvn), the
            // snapshot has THIS package's DefId, while `scopes.lookup`
            // would return whoever last overwrote the global binding.
            // See `bootstrap_package_item_ids` doc on `Resolver`.
            let pkg_snapshot = self.bootstrap_package_item_ids.get(pkg_name);
            let mut item_ids: Vec<DefId> = Vec::with_capacity(item_names.len());
            for name in item_names {
                let id = pkg_snapshot
                    .and_then(|m| m.get(name).copied())
                    .or_else(|| self.scopes.lookup(name))
                    .or_else(|| self.scopes.lookup_type(name));
                if let Some(id) = id {
                    if !item_ids.contains(&id) {
                        item_ids.push(id);
                    }
                }
            }
            if let Some(def) = self.symbols.get_mut(module_id) {
                if let DefKind::Module { items } = &mut def.kind {
                    for id in item_ids {
                        if !items.contains(&id) {
                            items.push(id);
                        }
                    }
                }
            }
        }
    }

    pub(super) fn fixup_bootstrapped_stdlib_modules(&mut self) {
        // Generic per-package auto-population first; cross-package
        // FIXUPS (below) only handle re-exports that span packages.
        self.auto_populate_std_submodules_from_packages();

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
        // Phase D of #06.95: the bulk per-package FIXUPS table is
        // gone. `auto_populate_std_submodules_from_packages` above
        // derives each `std.<pkg>` submodule's `items` from the
        // matching `library/std/<pkg>/src/lib.rvn`. The only
        // remaining responsibility here is the small TYPE_ALIASES
        // table below.

        // Wave 2 (#06.8): re-establish stdlib type aliases whose target
        // moved from a Rust registration to a bootstrap-loaded `.rvn`
        // file. Today this is just `Hash → Hashable` (the TEC-13
        // deprecation alias).
        //
        // The `Hash[K, V]` collection type has its OWN type-position
        // resolver path (see `resolve_type_path`'s explicit `Hash` /
        // `Vec` / `Set` arms) and is unaffected by this alias.
        const TYPE_ALIASES: &[(&str, &str)] = &[("Hash", "Hashable")];
        for (alias, target) in TYPE_ALIASES {
            if let Some(target_id) = self.scopes.lookup_type(target) {
                // Only insert when missing — never overwrite a real
                // scope entry.
                if self.scopes.lookup_type(alias).is_none() {
                    self.scopes.insert_type(alias.to_string(), target_id);
                    self.type_registry.insert(alias.to_string(), target_id);
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
                // Phase E.E of #06.95: `module Foo { class A; class B }`
                // shape (used by BufReader / BufWriter to express the
                // closed-set inner-type dispatch) needs Module bodies
                // visited at bootstrap-merge time so the nested classes
                // get registered under their qualified names. Without
                // this gate, the merger would silently skip every
                // module item in the stdlib.
                | ast::TopLevelItem::Module(_)
        )
    }

    // ─── Builtin Registration ───────────────────────────────────────
}
