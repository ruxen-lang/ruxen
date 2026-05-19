//! Name resolution pass for the Riven compiler.
//!
//! Walks the AST, resolves all identifiers to DefIds, registers definitions
//! in the symbol table, and produces a partially-lowered HIR. Type inference
//! variables are allocated for unresolved types; the type checker fills them in.

pub mod bootstrap;
pub mod scope;
mod stdlib;
pub mod symbols;
mod const_helpers;
mod yield_scan;

use std::collections::HashMap;

use crate::diagnostics::Diagnostic;
use crate::hir::context::TypeContext;
use crate::hir::nodes::*;
use crate::hir::types::{MixinRef, MoveSemantics, Ty};
use crate::lexer::token::Span;
use crate::parser::ast::{self, Visibility};
use scope::{ScopeId, ScopeKind, ScopeStack};
use symbols::*;

/// The result of name resolution: a partially-typed HIR plus symbol table.
pub struct ResolveResult {
    pub program: HirProgram,
    pub symbols: SymbolTable,
    pub type_context: TypeContext,
    pub diagnostics: Vec<Diagnostic>,
}

/// The name resolver walks the AST and produces HIR with resolved names.
pub struct Resolver {
    pub symbols: SymbolTable,
    pub scopes: ScopeStack,
    pub type_context: TypeContext,
    pub diagnostics: Vec<Diagnostic>,

    /// Maps type names to their DefIds for quick lookup during type resolution.
    type_registry: HashMap<String, DefId>,

    /// The current `self` type (inside class/impl bodies).
    current_self_ty: Option<Ty>,

    /// The current class DefId (for field/method resolution).
    current_class_def: Option<DefId>,

    /// The current function's return type (for return statement checking).
    current_return_ty: Option<Ty>,

    /// Associated-type bindings from the currently-resolving `impl` block:
    /// `Self.Item` → concrete Ty declared by `type Item = …`.
    current_impl_assoc_types: HashMap<String, Ty>,

    /// The trait whose body we are currently resolving (if any). Used to
    /// recognise `Self.AssocName` inside trait method signatures and map it
    /// to a placeholder TypeParam bound by that trait.
    current_trait_context: Option<(String, Vec<String>)>,

    /// Functions whose body contains `yield` — these take a synthetic
    /// `__block: Closure` trailing parameter.  Maps function name to the
    /// arity of the first observed `yield` (used to pre-shape the block's
    /// `Ty::Fn` parameter list so inference can unify with caller blocks).
    yield_fns: HashMap<String, usize>,

    /// Nesting depth of async functions/closures currently being resolved.
    async_scope_depth: usize,

    /// Active closure stack used to record free-variable captures.
    closure_stack: Vec<ClosureCaptureContext>,

    /// #06.8 T0c: tracks enums declared with an in-body `layout tagged`
    /// directive during pass 1, keyed by name. On a second insertion
    /// with the same name (i.e. two `layout tagged` enums with the
    /// same identifier in the same module scope) the resolver emits
    /// **E0723** at the duplicate's span. Wave 1 implementation only
    /// tracks the flat top-level module scope; nested-scope semantics
    /// arrive with the broader module-system pass.
    tagged_enums_in_scope: HashMap<String, Span>,

    /// #06.8 Phase 2: tracks every C-symbol declared by an FFI def.
    /// Maps `c_symbol → (signature, declaration_span)`. When a second
    /// FFI def declares the same C symbol with a non-matching
    /// signature (param types, return type, or arity differ), the
    /// resolver emits **E0722** at the duplicate's span. Two decls
    /// with matching signatures are silently allowed — they're a
    /// no-op redundancy, not a conflict. The Riven-side name is
    /// independent of this table; only the C-symbol is the key.
    extern_symbol_table: HashMap<String, (FnSignature, Span)>,

    /// #06.8 Phase 3b: DefIds of class-body `lib` FFI methods registered
    /// in pass-1, keyed by the parent class's `DefId`. Pass-2's
    /// `resolve_class` reads this map and appends these DefIds to the
    /// final `ClassInfo.methods` list, so `File.open(...)` resolves to
    /// the lib-declared method alongside any in-body `def`s.
    pass1_class_lib_methods: HashMap<DefId, Vec<DefId>>,

    /// #06.8 T#21: set to `true` while `merge_bootstrap_programs` is
    /// processing a stdlib `.rvn` file. When set, the `Class` arm of
    /// `register_top_level_type_with_ffi` treats a `class Foo do lib ...
    /// end end` whose name already exists in the type scope as a
    /// **namespace anchor** rather than a redefinition: it reuses the
    /// existing DefId (TypeAlias for `String`, Enum for `Option` /
    /// `Result`, etc.) as the parent for class-body lib FFI decls, so
    /// the methods land on the canonical type without overwriting its
    /// type-scope binding (which would change `Ty::String` to
    /// `Ty::Class { name: "String", … }` for the whole compilation —
    /// catastrophic for codegen). User-side `class Foo` blocks keep
    /// last-wins redefinition semantics; only bootstrap is treated as
    /// anchoring.
    merging_bootstrap: bool,
}

#[derive(Debug)]
struct ClosureCaptureContext {
    scope_id: ScopeId,
    is_move: bool,
    captures: Vec<Capture>,
}

impl Resolver {
    pub fn new() -> Self {
        Self {
            symbols: SymbolTable::new(),
            scopes: ScopeStack::new(),
            type_context: TypeContext::new(),
            diagnostics: Vec::new(),
            type_registry: HashMap::new(),
            current_self_ty: None,
            current_class_def: None,
            current_return_ty: None,
            current_impl_assoc_types: HashMap::new(),
            current_trait_context: None,
            yield_fns: HashMap::new(),
            async_scope_depth: 0,
            closure_stack: Vec::new(),
            tagged_enums_in_scope: HashMap::new(),
            extern_symbol_table: HashMap::new(),
            pass1_class_lib_methods: HashMap::new(),
            merging_bootstrap: false,
        }
    }

    /// Run name resolution on a parsed program.
    ///
    /// Equivalent to [`resolve_with_bootstrap`](Self::resolve_with_bootstrap)
    /// with an empty bootstrap list — kept as the legacy entry point so
    /// existing callers (tests, REPL, etc.) compile unchanged.
    pub fn resolve(self, program: &ast::Program) -> ResolveResult {
        self.resolve_with_bootstrap(program, &[])
    }

    /// Run name resolution with stdlib bootstrap programs merged into
    /// the prelude. `bootstrap_programs` are typically the output of
    /// [`crate::resolve::bootstrap::run_bootstrap`] — each one is a
    /// parsed `.rvn` file from `library/std/src/`.
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
            yield_scan::collect_yield_fns(item, &mut self.yield_fns);
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
                    yield_scan::collect_yield_fns(item, &mut self.yield_fns);
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
    fn fixup_bootstrapped_stdlib_modules(&mut self) {
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
            // Wave 2 — library/std/src/rand.rvn
            ("rand", &["random_bytes", "random_u64", "random_fill"]),
            // Wave 2 — library/std/src/path.rvn
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
            // Wave 2 — library/std/src/env.rvn
            ("env", &["args", "get", "vars", "current_dir"]),
            // Wave 2 — library/std/src/fmt.rvn (mixed mixins + classes —
            // each name resolves through the type-scope fallback).
            ("fmt", &["Display", "Debug", "Formatter", "FmtError"]),
            // Wave 2 — library/std/src/net.rvn (class shells +
            // Shutdown enum; methods still flow through the static-ctor
            // + runtime_table dispatch until T#20 lands).
            ("net", &["TcpListener", "TcpStream", "Shutdown"]),
            // Wave 2 — library/std/src/process.rvn (`exit` free fn +
            // Command / Output / ExitStatus class shells; class methods
            // still go through static-ctor + runtime_table until T#20).
            ("process", &["exit", "Command", "Output", "ExitStatus"]),
            // Wave 2 — library/std/src/time.rvn (`unix_ns` free fn +
            // Duration / Instant class shells; class methods still go
            // through static-ctor + runtime_table until T#20).
            ("time", &["unix_ns", "Duration", "Instant"]),
            // Wave 2 — library/std/src/fs.rvn (17 fs free fns +
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
            // Wave 2 — library/std/src/io.rvn (9 free fns + 7 class
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
            // Wave 2 — library/std/src/sync.rvn (9 class shells:
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
    fn is_bootstrap_supported_item(item: &ast::TopLevelItem) -> bool {
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

    fn register_builtins(&mut self) {
        stdlib::register_all(self);
    }

    /// #06.8 Phase 3b: register a single FFI decl from inside a class
    /// (or mixin) body's `lib "X" ... end` block as a CLASS METHOD on
    /// that parent.
    ///
    /// The lib-block syntax is identical inside or outside a class — a
    /// plain `def NAME(params) -> Type` (no `self.` prefix, no body).
    /// What makes it a class method is the parent context: there is no
    /// implicit `self` on FFI calls (they bind to a verbatim C symbol),
    /// so `is_class_method` is always true and `self_mode` always None.
    /// Call sites spell `ClassName.method(...)` — the same surface as
    /// `def self.method(...)`.
    ///
    /// Pushes the `HirFfiFunc` onto `ffi_libs` keyed by the MANGLED
    /// `ClassName_method` so the MIR `ffi_alias_map` (which is keyed
    /// the same way `lower_method_call` builds the callee) can rewrite
    /// the call to the C symbol at lowering time.
    fn register_class_lib_method(
        &mut self,
        parent: DefId,
        parent_name: &str,
        ffi_fn: &ast::FfiFunction,
        hir_fns: &mut Vec<HirFfiFunc>,
    ) {
        let param_tys: Vec<Ty> = ffi_fn
            .params
            .iter()
            .map(|p| self.resolve_type_expr(&p.type_expr))
            .collect();
        let params: Vec<ParamInfo> = ffi_fn
            .params
            .iter()
            .zip(param_tys.iter().cloned())
            .map(|(p, ty)| ParamInfo {
                name: p.name.clone(),
                ty,
                auto_assign: false,
            })
            .collect();
        let return_ty = ffi_fn
            .return_type
            .as_ref()
            .map(|t| self.resolve_type_expr(t))
            .unwrap_or(Ty::Unit);
        let return_ty_for_hir = if ffi_fn.return_type.is_some() {
            Some(return_ty.clone())
        } else {
            None
        };
        // The class-method vs instance-method distinction is carried on
        // the AST FfiFunction (set by the parser based on whether the
        // decl was `def self.NAME` or plain `def NAME`). Riven's
        // ruby-naming.spec.md §3.4a uses the same convention everywhere
        // — FFI decls are no exception. Instance-method FFI decls take
        // an implicit `self` receiver as their first arg to the C
        // symbol; class methods do not.
        let signature = FnSignature {
            self_mode: if ffi_fn.is_class_method {
                None
            } else {
                Some(crate::hir::nodes::HirSelfMode::Ref)
            },
            is_class_method: ffi_fn.is_class_method,
            is_async: false,
            generic_params: vec![],
            params,
            return_ty,
            c_symbol: ffi_fn.c_symbol.clone(),
        };
        let link_symbol = ffi_fn
            .c_symbol
            .clone()
            .unwrap_or_else(|| ffi_fn.name.clone());
        self.check_ffi_signature_conflict(&link_symbol, &signature, &ffi_fn.span);
        let mangled = format!("{}_{}", parent_name, ffi_fn.name);
        // Register under the PLAIN method name so MIR's
        // `is_user_static_method(class, method)` lookup — which scans
        // for a `DefKind::Method` whose `def.name == method_name` and
        // whose parent matches the class — finds this method.
        // Registering under the mangled name would hide it from that
        // scan and make method-call lowering prepend `self` (which is
        // wrong for class methods).
        let method_def_id = self.symbols.define(
            ffi_fn.name.clone(),
            DefKind::Method {
                parent,
                signature: signature.clone(),
            },
            Visibility::Public,
            ffi_fn.span.clone(),
        );
        self.extern_symbol_table
            .entry(link_symbol)
            .or_insert_with(|| (signature, ffi_fn.span.clone()));
        self.pass1_class_lib_methods
            .entry(parent)
            .or_default()
            .push(method_def_id);
        // Push onto ffi_libs with the MANGLED riven_name so the MIR
        // lowering's `ffi_alias_map` is keyed in the same shape that
        // `lower_method_call` builds the `MirInst::Call::callee` —
        // `format!("{}_{}", class_name, method_name)`. With the alias
        // map populated under that key, the existing Phase 2 rewrite
        // path picks up class-body FFI calls for free.
        // Instance methods receive `self` as the first arg at the C
        // ABI level — the MIR method-call lowering prepends the
        // receiver to arg_values for any non-static method, so the
        // FfiFuncDecl's `param_types` (which drives cranelift's
        // `Linkage::Import` signature) must include the class type
        // at index 0. Without this prepend the linker-side signature
        // would be off-by-one and cranelift would refuse the call
        // with "mismatched argument count".
        let final_param_tys = if ffi_fn.is_class_method {
            param_tys
        } else {
            let receiver_ty = Ty::Class {
                name: parent_name.to_string(),
                generic_args: vec![],
            };
            let mut tys = Vec::with_capacity(param_tys.len() + 1);
            tys.push(receiver_ty);
            tys.extend(param_tys);
            tys
        };
        hir_fns.push(HirFfiFunc {
            riven_name: mangled,
            c_symbol: ffi_fn.c_symbol.clone(),
            param_types: final_param_tys,
            return_type: return_ty_for_hir,
            is_variadic: ffi_fn.is_variadic,
            // The class/mixin name is encoded in the mangled riven_name
            // (`ClassName_method`) so any downstream consumer that wants
            // the parent type can split there. Setting it explicitly
            // here makes that intent visible.
            parent_type: Some(parent_name.to_string()),
        });
    }

    /// #06.8 Phase 2: emit **E0722** when a Riven `lib`/`extern` block
    /// declares the same C symbol that an earlier block already
    /// declared with an incompatible signature (arity, param types, or
    /// return type differ). The first decl wins; subsequent matching
    /// decls are silently allowed (a redundant restatement is a no-op,
    /// not an error). The check is keyed on the LINKED symbol so two
    /// Riven names that alias the same C symbol must agree on its
    /// type — otherwise codegen would produce a mis-typed call.
    fn check_ffi_signature_conflict(
        &mut self,
        link_symbol: &str,
        new_sig: &FnSignature,
        new_span: &Span,
    ) {
        if let Some((existing_sig, _existing_span)) =
            self.extern_symbol_table.get(link_symbol)
        {
            let arity_ok = existing_sig.params.len() == new_sig.params.len();
            let params_ok = arity_ok
                && existing_sig
                    .params
                    .iter()
                    .zip(new_sig.params.iter())
                    .all(|(a, b)| a.ty == b.ty);
            let return_ok = existing_sig.return_ty == new_sig.return_ty;
            if !(params_ok && return_ok) {
                self.diagnostics.push(Diagnostic::error_with_code(
                    format!(
                        "conflicting FFI declarations for the same C symbol `{}` — \
                         the earlier declaration's signature does not match this one",
                        link_symbol
                    ),
                    new_span.clone(),
                    "E0722",
                ));
            }
        }
    }

    // ─── Pass 1: Forward Declaration of Types ───────────────────────

    fn register_top_level_type_with_ffi(
        &mut self,
        item: &ast::TopLevelItem,
        ffi_libs: &mut Vec<HirFfiLib>,
    ) {
        let _span_zero = Span {
            start: 0,
            end: 0,
            line: 0,
            column: 0,
        };

        match item {
            ast::TopLevelItem::Class(class) => {
                // T2.02 S5: pre-populate generic_params with kinds so
                // use-site E0700 checks during pass 1 (e.g. inside a
                // forward-declared fn signature that references this
                // class) see the right param kinds.  Without this,
                // const params would still register as `Type` kind.
                let class_gp = self.collect_generic_param_infos(&class.generic_params);
                // #06.8 T0c: capture the `layout flat_heap_struct`
                // marker at forward-declaration time so any pre-pass
                // user (e.g. forward-referenced fn sigs) sees it.
                let flat_heap_struct = class
                    .layout
                    .iter()
                    .any(|s| s == "flat_heap_struct");

                // #06.8 T#21: namespace-anchor mode. When the bootstrap
                // is merging a `class Foo` whose name already has a
                // type-scope DefId (e.g. `String` → TypeAlias to
                // Ty::String, `Option`/`Result` → Enum), DO NOT
                // replace the binding. Instead reuse the existing
                // DefId as the parent for class-body `lib` decls.
                // This is the only way to attach FFI methods to a
                // builtin type without changing its `Ty` representation
                // for the whole compilation — see the field doc for
                // `merging_bootstrap` for the catastrophic-failure
                // mode this guards against.
                let anchor_id: Option<DefId> = if self.merging_bootstrap {
                    self.scopes.lookup_type(&class.name)
                } else {
                    None
                };

                let id = if let Some(existing) = anchor_id {
                    existing
                } else {
                    let new_id = self.symbols.define(
                        class.name.clone(),
                        DefKind::Class {
                            info: ClassInfo {
                                generic_params: class_gp,
                                parent: None,
                                fields: vec![],
                                methods: vec![],
                                derive_traits: class.derive_traits.clone(),
                                opt_out_send: false,
                                opt_out_sync: false,
                                manual_send: false,
                                manual_sync: false,
                                const_predicates: vec![],
                                flat_heap_struct,
                            },
                        },
                        Visibility::Public,
                        class.span.clone(),
                    );
                    self.scopes.insert_type(class.name.clone(), new_id);
                    self.type_registry.insert(class.name.clone(), new_id);
                    new_id
                };

                // #06.8 Phase 3b: register class-body `lib` FFI decls as
                // class methods on this class. The lib-block syntax is
                // identical inside or outside a class; the parent
                // context is what flips `is_class_method` to true and
                // routes calls through `ClassName.method(...)`.
                if !class.lib_decls.is_empty() {
                    let mut hir_fns: Vec<HirFfiFunc> = Vec::new();
                    let mut link_flags: Vec<String> = Vec::new();
                    for lib in &class.lib_decls {
                        for flag in lib.link_attrs.iter().map(|a| format!("-l{}", a.name)) {
                            if !link_flags.contains(&flag) {
                                link_flags.push(flag);
                            }
                        }
                        for ffi_fn in &lib.functions {
                            self.register_class_lib_method(
                                id,
                                &class.name,
                                ffi_fn,
                                &mut hir_fns,
                            );
                        }
                    }
                    if !hir_fns.is_empty() {
                        ffi_libs.push(HirFfiLib {
                            name: class.name.clone(),
                            link_flags,
                            functions: hir_fns,
                        });
                    }
                }
            }
            ast::TopLevelItem::Struct(s) => {
                let struct_gp = self.collect_generic_param_infos(&s.generic_params);
                let id = self.symbols.define(
                    s.name.clone(),
                    DefKind::Struct {
                        info: StructInfo {
                            generic_params: struct_gp,
                            fields: vec![],
                            derive_traits: s.derive_traits.clone(),
                            layout: s.layout.clone(),
                            opt_out_send: false,
                            opt_out_sync: false,
                            manual_send: false,
                            manual_sync: false,
                            const_predicates: vec![],
                        },
                    },
                    Visibility::Public,
                    s.span.clone(),
                );
                self.scopes.insert_type(s.name.clone(), id);
                self.type_registry.insert(s.name.clone(), id);
            }
            ast::TopLevelItem::Enum(e) => {
                // #06.8 T0c: duplicate `layout tagged` enum names in
                // the same scope are E0723. Detection happens here at
                // forward-declaration time so the diagnostic lands on
                // the second declaration's span (the first remains the
                // accepted one — matching the "tags are append-only"
                // invariant). The tracker is a flat HashMap keyed by
                // name, which matches the current top-level-only
                // scoping; nested-module semantics are deferred.
                if e.layout.iter().any(|s| s == "tagged") {
                    if let Some(_first_span) = self.tagged_enums_in_scope.get(&e.name).cloned() {
                        self.diagnostics.push(Diagnostic::error_with_code(
                            format!(
                                "duplicate `layout tagged` enum `{}` in scope",
                                e.name
                            ),
                            e.span.clone(),
                            "E0723",
                        ));
                    } else {
                        self.tagged_enums_in_scope
                            .insert(e.name.clone(), e.span.clone());
                    }
                }
                let enum_gp = self.collect_generic_param_infos(&e.generic_params);
                let id = self.symbols.define(
                    e.name.clone(),
                    DefKind::Enum {
                        info: EnumInfo {
                            generic_params: enum_gp,
                            variants: vec![],
                            derive_traits: e.derive_traits.clone(),
                            opt_out_send: false,
                            opt_out_sync: false,
                            manual_send: false,
                            manual_sync: false,
                            const_predicates: vec![],
                        },
                    },
                    Visibility::Public,
                    e.span.clone(),
                );
                self.scopes.insert_type(e.name.clone(), id);
                self.type_registry.insert(e.name.clone(), id);

                // Push a scope for the enum's own generic params so that
                // variant field types (e.g. `Some(T)` in
                // `enum MyOpt[T] { Some(T), None }`) can resolve `T` to
                // a `TypeParam` rather than `Error` during this pre-pass.
                let enum_generic_names: Vec<(String, Vec<MixinRef>, Span)> = e
                    .generic_params
                    .as_ref()
                    .map(|gps| {
                        gps.params
                            .iter()
                            .filter_map(|p| match p {
                                ast::GenericParam::Type { name, bounds, span } => {
                                    let trait_refs: Vec<MixinRef> = bounds
                                        .iter()
                                        .map(|b| MixinRef {
                                            name: b.path.segments.join("."),
                                            generic_args: vec![],
                                        })
                                        .collect();
                                    Some((name.clone(), trait_refs, span.clone()))
                                }
                                ast::GenericParam::Lifetime { .. } => None,
                                // Stage 3 of const generics: const
                                // params are registered separately
                                // below as `DefKind::ConstParam`
                                // (not type params), so this filter
                                // (which collects type-generic names
                                // for the enum's HIR) skips them.
                                ast::GenericParam::Const { .. } => None,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let has_generics = !enum_generic_names.is_empty();
                if has_generics {
                    self.scopes.push(ScopeKind::Class);
                    for (name, bounds, span) in &enum_generic_names {
                        let gp_def = self.symbols.define(
                            name.clone(),
                            DefKind::TypeParam {
                                bounds: bounds.clone(),
                            },
                            Visibility::Private,
                            span.clone(),
                        );
                        self.scopes.insert_type(name.clone(), gp_def);
                    }
                }

                // Also register each variant for resolution. Collect the
                // resolved info while the generic-param scope is active
                // (so `T` resolves), then register the composite
                // `Type.Variant` lookup entries after popping the scope
                // so they live on the outer top-level scope where callers
                // look them up.
                let mut pending_registrations: Vec<(String, DefId)> = Vec::new();
                for (idx, variant) in e.variants.iter().enumerate() {
                    let vkind = match &variant.fields {
                        ast::VariantKind::Unit => VariantDefKind::Unit,
                        ast::VariantKind::Tuple(fields) => VariantDefKind::Tuple(
                            fields
                                .iter()
                                .map(|f| self.resolve_type_expr(&f.type_expr))
                                .collect(),
                        ),
                        ast::VariantKind::Struct(fields) => VariantDefKind::Struct(
                            fields
                                .iter()
                                .map(|f| {
                                    (
                                        f.name.clone().unwrap_or_default(),
                                        self.resolve_type_expr(&f.type_expr),
                                    )
                                })
                                .collect(),
                        ),
                    };
                    let vid = self.symbols.define(
                        variant.name.clone(),
                        DefKind::EnumVariant {
                            parent: id,
                            variant_idx: idx,
                            kind: vkind,
                        },
                        Visibility::Public,
                        variant.span.clone(),
                    );
                    pending_registrations.push((format!("{}.{}", e.name, variant.name), vid));
                }

                if has_generics {
                    self.scopes.pop();
                }

                // Register Type.Variant lookup entries on the outer scope.
                for (key, vid) in pending_registrations {
                    self.scopes.insert(key, vid);
                }
            }
            ast::TopLevelItem::Mixin(t) => {
                let mut required = vec![];
                let mut defaults = vec![];
                let mut assoc = vec![];
                for ti in &t.items {
                    match ti {
                        ast::MixinItem::MethodSig(sig) => required.push(sig.name.clone()),
                        ast::MixinItem::DefaultMethod(f) => defaults.push(f.name.clone()),
                        ast::MixinItem::AssocType { name, .. } => assoc.push(name.clone()),
                    }
                }

                let id = self.symbols.define(
                    t.name.clone(),
                    DefKind::Trait {
                        info: MixinInfo {
                            generic_params: vec![],
                            super_traits: t
                                .super_traits
                                .iter()
                                .map(|b| MixinRef {
                                    name: b.path.segments.join("."),
                                    generic_args: vec![],
                                })
                                .collect(),
                            required_methods: required,
                            default_methods: defaults,
                            assoc_types: assoc,
                        },
                    },
                    Visibility::Public,
                    t.span.clone(),
                );
                self.scopes.insert_type(t.name.clone(), id);
                self.type_registry.insert(t.name.clone(), id);

                // #06.8 Phase 3b: register mixin-body `lib` FFI decls as
                // class methods on the mixin (parallel to class-body lib
                // handling above). Same semantics: no implicit `self`,
                // `ClassName.method(...)` call surface.
                if !t.lib_decls.is_empty() {
                    let mut hir_fns: Vec<HirFfiFunc> = Vec::new();
                    let mut link_flags: Vec<String> = Vec::new();
                    for lib in &t.lib_decls {
                        for flag in lib.link_attrs.iter().map(|a| format!("-l{}", a.name)) {
                            if !link_flags.contains(&flag) {
                                link_flags.push(flag);
                            }
                        }
                        for ffi_fn in &lib.functions {
                            self.register_class_lib_method(
                                id,
                                &t.name,
                                ffi_fn,
                                &mut hir_fns,
                            );
                        }
                    }
                    if !hir_fns.is_empty() {
                        ffi_libs.push(HirFfiLib {
                            name: t.name.clone(),
                            link_flags,
                            functions: hir_fns,
                        });
                    }
                }
            }
            ast::TopLevelItem::TypeAlias(ta) => {
                let target = self.resolve_type_expr(&ta.type_expr);
                let id = self.symbols.define(
                    ta.name.clone(),
                    DefKind::TypeAlias { target },
                    Visibility::Public,
                    ta.span.clone(),
                );
                self.scopes.insert_type(ta.name.clone(), id);
                self.type_registry.insert(ta.name.clone(), id);
            }
            ast::TopLevelItem::Newtype(nt) => {
                let inner = self.resolve_type_expr(&nt.inner_type);
                let id = self.symbols.define(
                    nt.name.clone(),
                    DefKind::Newtype { inner },
                    Visibility::Public,
                    nt.span.clone(),
                );
                self.scopes.insert_type(nt.name.clone(), id);
                self.type_registry.insert(nt.name.clone(), id);
            }
            ast::TopLevelItem::Module(m) => {
                // Register module type name, then recurse
                let id = self.symbols.define(
                    m.name.clone(),
                    DefKind::Module { items: vec![] },
                    Visibility::Public,
                    m.span.clone(),
                );
                self.scopes.insert_type(m.name.clone(), id);
                self.type_registry.insert(m.name.clone(), id);
                for sub_item in &m.items {
                    self.register_top_level_type_with_ffi(sub_item, ffi_libs);
                }
            }
            ast::TopLevelItem::Function(f) => {
                // Forward-declare top-level functions so they can be referenced
                // before their definition (e.g. parse_priority called from impl body).
                // Push a temporary scope for generic params
                self.scopes.push(ScopeKind::Function);
                let generic_params = self.resolve_generic_params(&f.generic_params);
                for gp in &generic_params {
                    let gp_def = self.symbols.define(
                        gp.name.clone(),
                        DefKind::TypeParam {
                            bounds: gp.bounds.clone(),
                        },
                        Visibility::Private,
                        gp.span.clone(),
                    );
                    self.scopes.insert_type(gp.name.clone(), gp_def);
                }
                let return_ty = f
                    .return_type
                    .as_ref()
                    .map(|t| self.resolve_type_expr(t))
                    .unwrap_or_else(|| {
                        if f.name == "main" {
                            Ty::Unit
                        } else {
                            self.type_context.fresh_type_var()
                        }
                    });
                let params: Vec<ParamInfo> = f
                    .params
                    .iter()
                    .map(|p| {
                        let ty = self.resolve_type_expr(&p.type_expr);
                        ParamInfo {
                            name: p.name.clone(),
                            ty,
                            auto_assign: p.auto_assign,
                        }
                    })
                    .collect();
                self.scopes.pop();
                let fn_generic_param_infos = self.collect_generic_param_infos(&f.generic_params);
                let id = self.symbols.define(
                    f.name.clone(),
                    DefKind::Function {
                        signature: FnSignature {
                            self_mode: None,
                            is_class_method: false,
                            is_async: f.is_async,
                            generic_params: fn_generic_param_infos,
                            params,
                            return_ty,
                            c_symbol: None,
                        },
                    },
                    Visibility::Public,
                    f.span.clone(),
                );
                self.scopes.insert(f.name.clone(), id);
            }
            ast::TopLevelItem::Lib(lib) => {
                let mut hir_fns: Vec<HirFfiFunc> = Vec::with_capacity(lib.functions.len());
                for ffi_fn in &lib.functions {
                    let param_tys: Vec<Ty> = ffi_fn
                        .params
                        .iter()
                        .map(|p| self.resolve_type_expr(&p.type_expr))
                        .collect();
                    let params: Vec<ParamInfo> = ffi_fn
                        .params
                        .iter()
                        .zip(param_tys.iter().cloned())
                        .map(|(p, ty)| ParamInfo {
                            name: p.name.clone(),
                            ty,
                            auto_assign: false,
                        })
                        .collect();
                    let return_ty = ffi_fn
                        .return_type
                        .as_ref()
                        .map(|t| self.resolve_type_expr(t))
                        .unwrap_or(Ty::Unit);
                    let return_ty_for_hir = if ffi_fn.return_type.is_some() {
                        Some(return_ty.clone())
                    } else {
                        None
                    };
                    let signature = FnSignature {
                        self_mode: None,
                        is_class_method: false,
                        is_async: false,
                        generic_params: vec![],
                        params,
                        return_ty,
                        c_symbol: ffi_fn.c_symbol.clone(),
                    };
                    // #06.8 Phase 2: E0722 cross-decl conflict check. Keyed
                    // on the LINKED C symbol (alias if present, Riven name
                    // otherwise) so two decls that route to the same linker
                    // symbol with incompatible signatures are caught before
                    // codegen produces a mis-typed call.
                    let link_symbol = ffi_fn
                        .c_symbol
                        .clone()
                        .unwrap_or_else(|| ffi_fn.name.clone());
                    self.check_ffi_signature_conflict(
                        &link_symbol,
                        &signature,
                        &ffi_fn.span,
                    );
                    let id = self.symbols.define(
                        ffi_fn.name.clone(),
                        DefKind::Function {
                            signature: signature.clone(),
                        },
                        Visibility::Public,
                        ffi_fn.span.clone(),
                    );
                    self.extern_symbol_table
                        .entry(link_symbol)
                        .or_insert_with(|| (signature, ffi_fn.span.clone()));
                    self.scopes.insert(ffi_fn.name.clone(), id);
                    hir_fns.push(HirFfiFunc {
                        riven_name: ffi_fn.name.clone(),
                        c_symbol: ffi_fn.c_symbol.clone(),
                        parent_type: None,
                        param_types: param_tys,
                        return_type: return_ty_for_hir,
                        is_variadic: ffi_fn.is_variadic,
                    });
                }
                ffi_libs.push(HirFfiLib {
                    name: lib.name.clone(),
                    link_flags: lib
                        .link_attrs
                        .iter()
                        .map(|a| format!("-l{}", a.name))
                        .collect(),
                    functions: hir_fns,
                });
            }
            ast::TopLevelItem::Extern(ext) => {
                let mut hir_fns: Vec<HirFfiFunc> = Vec::with_capacity(ext.functions.len());
                for ffi_fn in &ext.functions {
                    let param_tys: Vec<Ty> = ffi_fn
                        .params
                        .iter()
                        .map(|p| self.resolve_type_expr(&p.type_expr))
                        .collect();
                    let params: Vec<ParamInfo> = ffi_fn
                        .params
                        .iter()
                        .zip(param_tys.iter().cloned())
                        .map(|(p, ty)| ParamInfo {
                            name: p.name.clone(),
                            ty,
                            auto_assign: false,
                        })
                        .collect();
                    let return_ty = ffi_fn
                        .return_type
                        .as_ref()
                        .map(|t| self.resolve_type_expr(t))
                        .unwrap_or(Ty::Unit);
                    let return_ty_for_hir = if ffi_fn.return_type.is_some() {
                        Some(return_ty.clone())
                    } else {
                        None
                    };
                    let signature = FnSignature {
                        self_mode: None,
                        is_class_method: false,
                        is_async: false,
                        generic_params: vec![],
                        params,
                        return_ty,
                        c_symbol: ffi_fn.c_symbol.clone(),
                    };
                    let link_symbol = ffi_fn
                        .c_symbol
                        .clone()
                        .unwrap_or_else(|| ffi_fn.name.clone());
                    self.check_ffi_signature_conflict(
                        &link_symbol,
                        &signature,
                        &ffi_fn.span,
                    );
                    let id = self.symbols.define(
                        ffi_fn.name.clone(),
                        DefKind::Function {
                            signature: signature.clone(),
                        },
                        Visibility::Public,
                        ffi_fn.span.clone(),
                    );
                    self.extern_symbol_table
                        .entry(link_symbol)
                        .or_insert_with(|| (signature, ffi_fn.span.clone()));
                    self.scopes.insert(ffi_fn.name.clone(), id);
                    hir_fns.push(HirFfiFunc {
                        riven_name: ffi_fn.name.clone(),
                        c_symbol: ffi_fn.c_symbol.clone(),
                        parent_type: None,
                        param_types: param_tys,
                        return_type: return_ty_for_hir,
                        is_variadic: ffi_fn.is_variadic,
                    });
                }
                ffi_libs.push(HirFfiLib {
                    name: ext.abi.clone(),
                    link_flags: vec![],
                    functions: hir_fns,
                });
            }
            _ => {
                // Use, Const — resolved in pass 2
            }
        }
    }

    // ─── Pass 2: Full Resolution ────────────────────────────────────

    fn resolve_item(&mut self, item: &ast::TopLevelItem) -> Option<HirItem> {
        match item {
            ast::TopLevelItem::Class(class) => Some(HirItem::Class(self.resolve_class(class))),
            ast::TopLevelItem::Struct(s) => Some(HirItem::Struct(self.resolve_struct(s))),
            ast::TopLevelItem::Enum(e) => Some(HirItem::Enum(self.resolve_enum(e))),
            ast::TopLevelItem::Mixin(t) => Some(HirItem::Mixin(self.resolve_trait(t))),
            ast::TopLevelItem::Impl(imp) => Some(HirItem::Impl(self.resolve_impl(imp))),
            ast::TopLevelItem::Function(f) => {
                Some(HirItem::Function(self.resolve_func_def(f, None)))
            }
            ast::TopLevelItem::TypeAlias(ta) => {
                let def_id = self
                    .type_registry
                    .get(&ta.name)
                    .copied()
                    .unwrap_or(UNRESOLVED_DEF);
                let ty = self.resolve_type_expr(&ta.type_expr);
                Some(HirItem::TypeAlias(HirTypeAlias {
                    def_id,
                    name: ta.name.clone(),
                    ty,
                    span: ta.span.clone(),
                }))
            }
            ast::TopLevelItem::Newtype(nt) => {
                let def_id = self
                    .type_registry
                    .get(&nt.name)
                    .copied()
                    .unwrap_or(UNRESOLVED_DEF);
                let inner_ty = self.resolve_type_expr(&nt.inner_type);
                Some(HirItem::Newtype(HirNewtype {
                    def_id,
                    name: nt.name.clone(),
                    inner_ty,
                    span: nt.span.clone(),
                }))
            }
            ast::TopLevelItem::Module(m) => Some(HirItem::Module(self.resolve_module(m))),
            ast::TopLevelItem::Const(c) => {
                let ty = self.resolve_type_expr(&c.type_expr);
                let value = self.resolve_expr(&c.value);
                let def_id = self.symbols.define(
                    c.name.clone(),
                    DefKind::Const { ty: ty.clone() },
                    Visibility::Public,
                    c.span.clone(),
                );
                self.scopes.insert(c.name.clone(), def_id);
                Some(HirItem::Const(HirConst {
                    def_id,
                    name: c.name.clone(),
                    ty,
                    value,
                    doc_comments: c.doc_comments.clone(),
                    span: c.span.clone(),
                }))
            }
            ast::TopLevelItem::Use(use_decl) => {
                self.resolve_use_decl(use_decl);
                None
            }
            ast::TopLevelItem::Lib(_) | ast::TopLevelItem::Extern(_) => {
                // FFI declarations are handled during codegen — they don't produce
                // HIR items. The functions they declare are resolved by name at
                // call sites during codegen (via runtime_name / get_or_declare_func).
                None
            }
        }
    }

    // ─── Class Resolution ───────────────────────────────────────────

    fn resolve_class(&mut self, class: &ast::ClassDef) -> HirClassDef {
        let def_id = self
            .type_registry
            .get(&class.name)
            .copied()
            .unwrap_or(UNRESOLVED_DEF);

        let generic_params = self.resolve_generic_params(&class.generic_params);

        let parent_def = class.parent.as_ref().and_then(|p| {
            let name = p.segments.join(".");
            self.type_registry.get(&name).copied()
        });

        // Build the self type
        let self_ty = Ty::Class {
            name: class.name.clone(),
            generic_args: generic_params
                .iter()
                .map(|gp| Ty::TypeParam {
                    name: gp.name.clone(),
                    bounds: gp.bounds.clone(),
                })
                .collect(),
        };

        let old_self_ty = self.current_self_ty.replace(self_ty.clone());
        let old_class_def = self.current_class_def.replace(def_id);

        self.scopes.push(ScopeKind::Class);

        // Register generic type parameters in scope
        for gp in &generic_params {
            let gp_def = self.symbols.define(
                gp.name.clone(),
                DefKind::TypeParam {
                    bounds: gp.bounds.clone(),
                },
                Visibility::Private,
                gp.span.clone(),
            );
            self.scopes.insert_type(gp.name.clone(), gp_def);
        }

        // Register `Self` type
        let self_def_id = self.symbols.define(
            "Self".to_string(),
            DefKind::TypeAlias {
                target: self_ty.clone(),
            },
            Visibility::Private,
            class.span.clone(),
        );
        self.scopes.insert_type("Self".to_string(), self_def_id);

        // Resolve fields
        let mut fields = Vec::new();
        let mut field_def_ids = Vec::new();
        let mut opt_out_send = false;
        let mut opt_out_sync = false;
        let mut manual_send = false;
        let mut manual_sync = false;
        for (idx, field) in class.fields.iter().enumerate() {
            let ty = self.resolve_type_expr(&field.type_expr);
            let fid = self.symbols.define(
                field.name.clone(),
                DefKind::Field {
                    parent: def_id,
                    ty: ty.clone(),
                    index: idx,
                },
                field.visibility,
                field.span.clone(),
            );
            self.scopes.insert(field.name.clone(), fid);
            field_def_ids.push(fid);
            fields.push(HirFieldDef {
                def_id: fid,
                name: field.name.clone(),
                ty,
                visibility: field.visibility,
                index: idx,
                span: field.span.clone(),
            });
        }

        // Resolve methods
        let mut methods = Vec::new();
        let mut method_def_ids = Vec::new();
        for method in &class.methods {
            let hir_method = self.resolve_func_def(method, Some(def_id));
            method_def_ids.push(hir_method.def_id);
            methods.push(hir_method);
        }

        // Resolve inner impl blocks
        let mut impl_blocks = Vec::new();
        for inner in &class.inner_impls {
            let trait_ref = MixinRef {
                name: inner.trait_name.segments.join("."),
                generic_args: inner
                    .trait_name
                    .generic_args
                    .as_ref()
                    .map(|args| args.iter().map(|a| self.resolve_type_expr(a)).collect())
                    .unwrap_or_default(),
            };

            // Collect `type Foo = X` bindings from the inner impl block so
            // that `Self.Foo` in method signatures resolves concretely.
            let old_assoc = std::mem::take(&mut self.current_impl_assoc_types);
            for ii in &inner.items {
                if let ast::ImplItem::AssocType {
                    name, type_expr, ..
                } = ii
                {
                    let ty = self.resolve_type_expr(type_expr);
                    self.current_impl_assoc_types.insert(name.clone(), ty);
                }
            }

            let mut items = Vec::new();
            for ii in &inner.items {
                match ii {
                    ast::ImplItem::Method(f) => {
                        items.push(HirImplItem::Method(self.resolve_func_def(f, Some(def_id))));
                    }
                    ast::ImplItem::AssocType {
                        name,
                        type_expr,
                        span,
                    } => {
                        items.push(HirImplItem::AssocType {
                            name: name.clone(),
                            ty: self.resolve_type_expr(type_expr),
                            span: span.clone(),
                        });
                    }
                    ast::ImplItem::Include {
                        is_unsafe,
                        negative_trait,
                        trait_name,
                        span,
                    } => {
                        items.push(HirImplItem::Include {
                            is_unsafe: *is_unsafe,
                            negative_trait: *negative_trait,
                            trait_name: trait_name.segments.join("."),
                            span: span.clone(),
                        });
                    }
                }
            }
            self.current_impl_assoc_types = old_assoc;
            self.record_auto_trait_flags(
                &self_ty,
                Some(&trait_ref.name),
                inner.negative_trait,
                inner.is_unsafe,
                inner.span.clone(),
            );
            match trait_ref.name.as_str() {
                "Send" if inner.negative_trait => opt_out_send = true,
                "Sync" if inner.negative_trait => opt_out_sync = true,
                "Send" if inner.is_unsafe => manual_send = true,
                "Sync" if inner.is_unsafe => manual_sync = true,
                _ => {}
            }

            impl_blocks.push(HirImplBlock {
                generic_params: vec![],
                is_unsafe: inner.is_unsafe,
                negative_trait: inner.negative_trait,
                trait_ref: Some(trait_ref),
                target_ty: self_ty.clone(),
                items,
                span: inner.span.clone(),
            });
        }

        self.scopes.pop();
        self.current_self_ty = old_self_ty;
        self.current_class_def = old_class_def;

        // Update the symbol table with full class info.  Pre-compute
        // generic_params (which needs `&mut self` for type-expr
        // resolution) before grabbing the mutable borrow of the symbol.
        let class_generic_param_infos = self.collect_generic_param_infos(&class.generic_params);
        // T2.02 S9: lower where-clause const predicates so the
        // instantiation site can evaluate them against the binding map.
        let const_predicates: Vec<_> = class
            .where_clause
            .as_ref()
            .map(|wc| {
                wc.const_predicates
                    .iter()
                    .map(const_helpers::lower_const_predicate)
                    .collect()
            })
            .unwrap_or_default();
        // #06.8 T0c: preserve the `layout flat_heap_struct` marker
        // captured in pass 1 across the pass-2 rewrite.
        let flat_heap_struct = class
            .layout
            .iter()
            .any(|s| s == "flat_heap_struct");
        // #06.8 Phase 3b: append the class-body `lib` FFI methods that
        // pass-1 registered onto the side-map. They were registered as
        // `DefKind::Method` with `parent = def_id` and need to appear
        // in `ClassInfo.methods` so name lookups (`Foo.bar`) find them
        // alongside the in-body `def`s above.
        if let Some(lib_method_ids) = self.pass1_class_lib_methods.get(&def_id) {
            method_def_ids.extend(lib_method_ids.iter().copied());
        }
        if let Some(def) = self.symbols.get_mut(def_id) {
            def.kind = DefKind::Class {
                info: ClassInfo {
                    generic_params: class_generic_param_infos,
                    parent: parent_def,
                    fields: field_def_ids,
                    methods: method_def_ids,
                    derive_traits: class.derive_traits.clone(),
                    opt_out_send,
                    opt_out_sync,
                    manual_send,
                    manual_sync,
                    const_predicates,
                    flat_heap_struct,
                },
            };
        }

        HirClassDef {
            def_id,
            name: class.name.clone(),
            generic_params,
            parent: parent_def,
            fields,
            methods,
            impl_blocks,
            derive_traits: class.derive_traits.clone(),
            doc_comments: class.doc_comments.clone(),
            span: class.span.clone(),
        }
    }

    // ─── Struct Resolution ──────────────────────────────────────────

    fn resolve_struct(&mut self, s: &ast::StructDef) -> HirStructDef {
        let def_id = self
            .type_registry
            .get(&s.name)
            .copied()
            .unwrap_or(UNRESOLVED_DEF);
        let generic_params = self.resolve_generic_params(&s.generic_params);

        let mut fields = Vec::new();
        let mut field_def_ids = Vec::new();
        for (idx, field) in s.fields.iter().enumerate() {
            let ty = self.resolve_type_expr(&field.type_expr);
            let fid = self.symbols.define(
                field.name.clone(),
                DefKind::Field {
                    parent: def_id,
                    ty: ty.clone(),
                    index: idx,
                },
                field.visibility,
                field.span.clone(),
            );
            field_def_ids.push(fid);
            fields.push(HirFieldDef {
                def_id: fid,
                name: field.name.clone(),
                ty,
                visibility: field.visibility,
                index: idx,
                span: field.span.clone(),
            });
        }

        // Update symbol table.  Pre-compute generic_params before
        // grabbing the mutable symbol borrow.
        let struct_generic_param_infos = self.collect_generic_param_infos(&s.generic_params);
        // T2.02 S9: lower where-clause const predicates.
        let const_predicates: Vec<_> = s
            .where_clause
            .as_ref()
            .map(|wc| {
                wc.const_predicates
                    .iter()
                    .map(const_helpers::lower_const_predicate)
                    .collect()
            })
            .unwrap_or_default();
        if let Some(def) = self.symbols.get_mut(def_id) {
            def.kind = DefKind::Struct {
                info: StructInfo {
                    generic_params: struct_generic_param_infos,
                    fields: field_def_ids,
                    derive_traits: s.derive_traits.clone(),
                    layout: s.layout.clone(),
                    opt_out_send: false,
                    opt_out_sync: false,
                    manual_send: false,
                    manual_sync: false,
                    const_predicates,
                },
            };
        }

        // ruby-naming.spec.md §3.4a: structs may carry inline methods
        // and `include Mixin` directives.
        let old_self_ty = self.current_self_ty.take();
        let self_ty = Ty::Struct {
            name: s.name.clone(),
            generic_args: vec![],
        };
        self.current_self_ty = Some(self_ty.clone());
        let methods = s
            .methods
            .iter()
            .map(|m| self.resolve_func_def(m, Some(def_id)))
            .collect::<Vec<_>>();
        let impl_blocks = self.lower_inner_impls(&s.inner_impls, &self_ty, Some(def_id));
        self.current_self_ty = old_self_ty;

        HirStructDef {
            def_id,
            name: s.name.clone(),
            generic_params,
            fields,
            methods,
            impl_blocks,
            derive_traits: s.derive_traits.clone(),
            layout: s.layout.clone(),
            doc_comments: s.doc_comments.clone(),
            span: s.span.clone(),
        }
    }

    // ─── Enum Resolution ────────────────────────────────────────────

    /// Lower a list of AST `InnerImpl` directives (collected from a
    /// struct or enum body under ruby-naming.spec.md §3.4a) into HIR
    /// `HirImplBlock` records. The same routine the class path uses,
    /// minus the class-specific `opt_out_*` tracking — struct/enum
    /// auto-trait flags live on their own info structs.
    fn lower_inner_impls(
        &mut self,
        inner_impls: &[ast::InnerImpl],
        self_ty: &Ty,
        parent_def: Option<DefId>,
    ) -> Vec<HirImplBlock> {
        let mut impl_blocks = Vec::new();
        for inner in inner_impls {
            let trait_ref = MixinRef {
                name: inner.trait_name.segments.join("."),
                generic_args: inner
                    .trait_name
                    .generic_args
                    .as_ref()
                    .map(|args| args.iter().map(|a| self.resolve_type_expr(a)).collect())
                    .unwrap_or_default(),
            };

            let old_assoc = std::mem::take(&mut self.current_impl_assoc_types);
            for ii in &inner.items {
                if let ast::ImplItem::AssocType {
                    name, type_expr, ..
                } = ii
                {
                    let ty = self.resolve_type_expr(type_expr);
                    self.current_impl_assoc_types.insert(name.clone(), ty);
                }
            }

            let mut items = Vec::new();
            for ii in &inner.items {
                match ii {
                    ast::ImplItem::Method(f) => {
                        items.push(HirImplItem::Method(self.resolve_func_def(f, parent_def)));
                    }
                    ast::ImplItem::AssocType {
                        name,
                        type_expr,
                        span,
                    } => {
                        items.push(HirImplItem::AssocType {
                            name: name.clone(),
                            ty: self.resolve_type_expr(type_expr),
                            span: span.clone(),
                        });
                    }
                    ast::ImplItem::Include {
                        is_unsafe,
                        negative_trait,
                        trait_name,
                        span,
                    } => {
                        items.push(HirImplItem::Include {
                            is_unsafe: *is_unsafe,
                            negative_trait: *negative_trait,
                            trait_name: trait_name.segments.join("."),
                            span: span.clone(),
                        });
                    }
                }
            }
            self.current_impl_assoc_types = old_assoc;

            impl_blocks.push(HirImplBlock {
                generic_params: vec![],
                is_unsafe: inner.is_unsafe,
                negative_trait: inner.negative_trait,
                trait_ref: Some(trait_ref),
                target_ty: self_ty.clone(),
                items,
                span: inner.span.clone(),
            });
        }
        impl_blocks
    }

    fn resolve_enum(&mut self, e: &ast::EnumDef) -> HirEnumDef {
        let def_id = self
            .type_registry
            .get(&e.name)
            .copied()
            .unwrap_or(UNRESOLVED_DEF);
        let generic_params = self.resolve_generic_params(&e.generic_params);

        // Push a scope so enum generic params are visible while resolving
        // variant field types (e.g. `Some(T)` in `enum MyOpt[T]`). Without
        // this, `T` resolved to `undefined type`, which propagated as an
        // `Error` payload type and kept the match/codegen paths from
        // producing a valid lowering.
        self.scopes.push(ScopeKind::Class);
        for gp in &generic_params {
            let gp_def = self.symbols.define(
                gp.name.clone(),
                DefKind::TypeParam {
                    bounds: gp.bounds.clone(),
                },
                Visibility::Private,
                gp.span.clone(),
            );
            self.scopes.insert_type(gp.name.clone(), gp_def);
        }

        let mut variants = Vec::new();
        let mut variant_def_ids = Vec::new();

        for (idx, variant) in e.variants.iter().enumerate() {
            let kind = match &variant.fields {
                ast::VariantKind::Unit => HirVariantKind::Unit,
                ast::VariantKind::Tuple(fields) => HirVariantKind::Tuple(
                    fields
                        .iter()
                        .map(|f| HirVariantField {
                            name: f.name.clone(),
                            ty: self.resolve_type_expr(&f.type_expr),
                            span: f.span.clone(),
                        })
                        .collect(),
                ),
                ast::VariantKind::Struct(fields) => HirVariantKind::Struct(
                    fields
                        .iter()
                        .map(|f| HirVariantField {
                            name: f.name.clone(),
                            ty: self.resolve_type_expr(&f.type_expr),
                            span: f.span.clone(),
                        })
                        .collect(),
                ),
            };

            // Look up the variant DefId registered in pass 1
            let composite_name = format!("{}.{}", e.name, variant.name);
            let vid = self.scopes.lookup(&composite_name).unwrap_or_else(|| {
                // Shouldn't happen if pass 1 ran correctly, but be defensive
                self.symbols.define(
                    variant.name.clone(),
                    DefKind::EnumVariant {
                        parent: def_id,
                        variant_idx: idx,
                        kind: VariantDefKind::Unit,
                    },
                    Visibility::Public,
                    variant.span.clone(),
                )
            });
            variant_def_ids.push(vid);

            variants.push(HirVariant {
                def_id: vid,
                name: variant.name.clone(),
                kind,
                index: idx,
                span: variant.span.clone(),
            });
        }

        // Update symbol table.  Pre-compute generic_params before
        // grabbing the mutable symbol borrow.
        let enum_generic_param_infos = self.collect_generic_param_infos(&e.generic_params);
        if let Some(def) = self.symbols.get_mut(def_id) {
            def.kind = DefKind::Enum {
                info: EnumInfo {
                    generic_params: enum_generic_param_infos,
                    variants: variant_def_ids,
                    derive_traits: e.derive_traits.clone(),
                    opt_out_send: false,
                    opt_out_sync: false,
                    manual_send: false,
                    manual_sync: false,
                    const_predicates: vec![],
                },
            };
        }

        // ruby-naming.spec.md §3.4a: enums may carry inline methods
        // and `include Mixin` directives.
        let old_self_ty = self.current_self_ty.take();
        let self_ty = Ty::Enum {
            name: e.name.clone(),
            generic_args: vec![],
        };
        self.current_self_ty = Some(self_ty.clone());
        let methods = e
            .methods
            .iter()
            .map(|m| self.resolve_func_def(m, Some(def_id)))
            .collect::<Vec<_>>();
        let impl_blocks = self.lower_inner_impls(&e.inner_impls, &self_ty, Some(def_id));
        self.current_self_ty = old_self_ty;

        self.scopes.pop();

        HirEnumDef {
            def_id,
            name: e.name.clone(),
            generic_params,
            variants,
            methods,
            impl_blocks,
            derive_traits: e.derive_traits.clone(),
            doc_comments: e.doc_comments.clone(),
            span: e.span.clone(),
        }
    }

    // ─── Trait Resolution ───────────────────────────────────────────

    fn resolve_trait(&mut self, t: &ast::MixinDef) -> HirMixinDef {
        let def_id = self
            .type_registry
            .get(&t.name)
            .copied()
            .unwrap_or(UNRESOLVED_DEF);
        let generic_params = self.resolve_generic_params(&t.generic_params);

        self.scopes.push(ScopeKind::Trait);

        // Register Self as a type alias pointing to a TypeParam with this trait as bound
        let self_ty = Ty::TypeParam {
            name: "Self".to_string(),
            bounds: vec![MixinRef {
                name: t.name.clone(),
                generic_args: vec![],
            }],
        };
        let self_type_id = self.symbols.define(
            "Self".to_string(),
            DefKind::TypeAlias {
                target: self_ty.clone(),
            },
            Visibility::Private,
            t.span.clone(),
        );
        self.scopes.insert_type("Self".to_string(), self_type_id);

        // Make `self` (the value) available inside default method bodies so
        // expressions like `self.name` resolve to the abstract trait method.
        // The concrete `self` type is supplied when each impl monomorphises
        // the default body; here we only need a placeholder so the resolver
        // and typechecker treat it as a valid method-context value.
        let old_self_ty = self.current_self_ty.replace(self_ty);

        let super_traits: Vec<MixinRef> = t
            .super_traits
            .iter()
            .map(|b| MixinRef {
                name: b.path.segments.join("."),
                generic_args: b
                    .path
                    .generic_args
                    .as_ref()
                    .map(|args| args.iter().map(|a| self.resolve_type_expr(a)).collect())
                    .unwrap_or_default(),
            })
            .collect();

        // Make the trait's declared associated-type names visible so
        // `Self.Name` inside method signatures resolves to a placeholder
        // `Ty::TypeParam` (which behaves opaquely during trait resolution).
        let assoc_names: Vec<String> = t
            .items
            .iter()
            .filter_map(|ti| match ti {
                ast::MixinItem::AssocType { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect();
        let old_trait_ctx = self
            .current_trait_context
            .replace((t.name.clone(), assoc_names));

        let mut items = Vec::new();
        for ti in &t.items {
            match ti {
                ast::MixinItem::AssocType { name, span } => {
                    items.push(HirMixinItem::AssocType {
                        name: name.clone(),
                        span: span.clone(),
                    });
                }
                ast::MixinItem::MethodSig(sig) => {
                    let params = self.resolve_params(&sig.params);
                    let return_ty = sig
                        .return_type
                        .as_ref()
                        .map(|t| self.resolve_type_expr(t))
                        .unwrap_or(Ty::Unit);
                    let self_mode = sig.self_mode.map(|m| self.convert_self_mode(m));

                    items.push(HirMixinItem::MethodSig {
                        name: sig.name.clone(),
                        self_mode,
                        is_class_method: sig.is_class_method,
                        params,
                        return_ty,
                        span: sig.span.clone(),
                    });
                }
                ast::MixinItem::DefaultMethod(f) => {
                    items.push(HirMixinItem::DefaultMethod(self.resolve_func_def(f, None)));
                }
            }
        }

        self.current_trait_context = old_trait_ctx;
        self.current_self_ty = old_self_ty;
        self.scopes.pop();

        HirMixinDef {
            def_id,
            name: t.name.clone(),
            generic_params,
            super_traits,
            items,
            doc_comments: t.doc_comments.clone(),
            span: t.span.clone(),
        }
    }

    // ─── Impl Block Resolution ──────────────────────────────────────

    fn resolve_impl(&mut self, imp: &ast::ImplBlock) -> HirImplBlock {
        let generic_params = self.resolve_generic_params(&imp.generic_params);
        let target_ty = self.resolve_type_expr(&imp.target_type);
        let trait_ref = imp.trait_name.as_ref().map(|tp| MixinRef {
            name: tp.segments.join("."),
            generic_args: tp
                .generic_args
                .as_ref()
                .map(|args| args.iter().map(|a| self.resolve_type_expr(a)).collect())
                .unwrap_or_default(),
        });
        if let Some(ref trait_ref) = trait_ref {
            self.record_auto_trait_flags(
                &target_ty,
                Some(&trait_ref.name),
                imp.negative_trait,
                imp.is_unsafe,
                imp.span.clone(),
            );
        } else if imp.is_unsafe {
            self.diagnostics.push(Diagnostic::error_with_code(
                "`unsafe include` is only meaningful for mixin inclusions",
                imp.span.clone(),
                "E1014",
            ));
        }

        // Determine the class def for self resolution
        let class_def = match &target_ty {
            Ty::Class { name, .. } | Ty::Enum { name, .. } | Ty::Struct { name, .. } => {
                self.type_registry.get(name).copied()
            }
            _ => None,
        };

        let old_self_ty = self.current_self_ty.replace(target_ty.clone());
        let old_class_def = std::mem::replace(&mut self.current_class_def, class_def);

        self.scopes.push(ScopeKind::Impl);

        // Register Self type
        let self_type_id = self.symbols.define(
            "Self".to_string(),
            DefKind::TypeAlias {
                target: target_ty.clone(),
            },
            Visibility::Private,
            imp.span.clone(),
        );
        self.scopes.insert_type("Self".to_string(), self_type_id);

        // First pass: collect `type Foo = X` bindings so that `Self.Foo`
        // references inside method signatures/bodies resolve to the
        // concrete type declared here.
        let old_assoc = std::mem::take(&mut self.current_impl_assoc_types);
        for ii in &imp.items {
            if let ast::ImplItem::AssocType {
                name, type_expr, ..
            } = ii
            {
                let ty = self.resolve_type_expr(type_expr);
                self.current_impl_assoc_types.insert(name.clone(), ty);
            }
        }

        let mut items = Vec::new();
        for ii in &imp.items {
            match ii {
                ast::ImplItem::Method(f) => {
                    items.push(HirImplItem::Method(self.resolve_func_def(f, class_def)));
                }
                ast::ImplItem::AssocType {
                    name,
                    type_expr,
                    span,
                } => {
                    items.push(HirImplItem::AssocType {
                        name: name.clone(),
                        ty: self.resolve_type_expr(type_expr),
                        span: span.clone(),
                    });
                }
                ast::ImplItem::Include {
                    is_unsafe,
                    negative_trait,
                    trait_name,
                    span,
                } => {
                    items.push(HirImplItem::Include {
                        is_unsafe: *is_unsafe,
                        negative_trait: *negative_trait,
                        trait_name: trait_name.segments.join("."),
                        span: span.clone(),
                    });
                }
            }
        }

        self.current_impl_assoc_types = old_assoc;
        self.scopes.pop();
        self.current_self_ty = old_self_ty;
        self.current_class_def = old_class_def;

        HirImplBlock {
            generic_params,
            is_unsafe: imp.is_unsafe,
            negative_trait: imp.negative_trait,
            trait_ref,
            target_ty,
            items,
            span: imp.span.clone(),
        }
    }

    fn record_auto_trait_flags(
        &mut self,
        target_ty: &Ty,
        trait_name: Option<&str>,
        negative_trait: bool,
        is_unsafe: bool,
        span: Span,
    ) {
        let Some(trait_name) = trait_name else {
            return;
        };

        let (mark_send, mark_sync) = match trait_name {
            "Send" => (true, false),
            "Sync" => (false, true),
            _ => {
                if negative_trait {
                    self.diagnostics.push(Diagnostic::error_with_code(
                        "negative include (`exclude`) is only supported for Send and Sync",
                        span,
                        "E1014",
                    ));
                } else if is_unsafe {
                    self.diagnostics.push(Diagnostic::error_with_code(
                        "`unsafe include` is only required for Send and Sync",
                        span,
                        "E1014",
                    ));
                }
                return;
            }
        };

        if !negative_trait && !is_unsafe {
            self.diagnostics.push(Diagnostic::error_with_code(
                "manual Send/Sync includes must be declared as `unsafe include`",
                span,
                "E1014",
            ));
            return;
        }

        let Some(def) = const_helpers::nominal_type_definition_mut(target_ty, &mut self.symbols) else {
            return;
        };

        match &mut def.kind {
            DefKind::Class { info } => {
                if mark_send {
                    info.opt_out_send = negative_trait;
                    info.manual_send = !negative_trait;
                }
                if mark_sync {
                    info.opt_out_sync = negative_trait;
                    info.manual_sync = !negative_trait;
                }
            }
            DefKind::Struct { info } => {
                if mark_send {
                    info.opt_out_send = negative_trait;
                    info.manual_send = !negative_trait;
                }
                if mark_sync {
                    info.opt_out_sync = negative_trait;
                    info.manual_sync = !negative_trait;
                }
            }
            DefKind::Enum { info } => {
                if mark_send {
                    info.opt_out_send = negative_trait;
                    info.manual_send = !negative_trait;
                }
                if mark_sync {
                    info.opt_out_sync = negative_trait;
                    info.manual_sync = !negative_trait;
                }
            }
            _ => {}
        }
    }

    // ─── Function Resolution ────────────────────────────────────────

    fn resolve_func_def(&mut self, f: &ast::FuncDef, parent: Option<DefId>) -> HirFuncDef {
        let mut generic_params = self.resolve_generic_params(&f.generic_params);
        // Merge `where T: Bound, ...` predicates into the matching generic
        // parameter's bounds. Predicates whose left-hand side is not a
        // declared type parameter (e.g., associated-type constraints like
        // `Iterable[Item = Int]`) are parsed and dropped for now — they
        // require associated-type infrastructure not yet present.
        if let Some(ref wc) = f.where_clause {
            for pred in &wc.predicates {
                if let ast::TypeExpr::Named(path) = &pred.type_expr {
                    if path.segments.len() == 1 && path.generic_args.is_none() {
                        let name = &path.segments[0];
                        if let Some(gp) = generic_params.iter_mut().find(|g| &g.name == name) {
                            for bound in &pred.bounds {
                                gp.bounds.push(MixinRef {
                                    name: bound.path.segments.join("."),
                                    generic_args: bound
                                        .path
                                        .generic_args
                                        .as_ref()
                                        .map(|args| {
                                            args.iter().map(|a| self.resolve_type_expr(a)).collect()
                                        })
                                        .unwrap_or_default(),
                                });
                            }
                        }
                    }
                }
                // TODO: associated-type bounds (e.g. `A: Iterable[Item = Int]`)
                // are parsed but ignored until the type system models them.
            }
        }

        self.scopes.push(ScopeKind::Function);

        // Register generic type params in scope
        for gp in &generic_params {
            let gp_def = self.symbols.define(
                gp.name.clone(),
                DefKind::TypeParam {
                    bounds: gp.bounds.clone(),
                },
                Visibility::Private,
                gp.span.clone(),
            );
            self.scopes.insert_type(gp.name.clone(), gp_def);
        }

        let self_mode = f.self_mode.map(|m| self.convert_self_mode(m));

        // Register self if this is a method.
        // If we're inside a class/impl body (current_self_ty is set) and
        // the function has no explicit self_mode, default to:
        //   - &mut self for init (needs to assign fields)
        //   - &self for all other instance methods
        // Class methods (self.method_name) don't get implicit self.
        let self_mode =
            if self_mode.is_none() && self.current_self_ty.is_some() && !f.is_class_method {
                if f.name == "init" {
                    Some(HirSelfMode::RefMut)
                } else {
                    Some(HirSelfMode::Ref)
                }
            } else {
                self_mode
            };

        if let Some(ref self_ty) = self.current_self_ty {
            if self_mode.is_some() {
                let self_def = self.symbols.define(
                    "self".to_string(),
                    DefKind::SelfValue {
                        ty: self_ty.clone(),
                    },
                    Visibility::Private,
                    f.span.clone(),
                );
                self.scopes.insert("self".to_string(), self_def);
            }
        }

        // Resolve parameters
        let mut params = self.resolve_and_register_params(&f.params);

        // If this function's body contains `yield`, append a synthetic
        // `__block: Fn(…) -> ()` parameter so `yield VALUE` can desugar
        // to `__block.(VALUE)` and callers can forward a trailing block.
        if let Some(&arity) = self.yield_fns.get(&f.name) {
            let block_ty = Ty::Fn {
                params: (0..arity)
                    .map(|_| self.type_context.fresh_type_var())
                    .collect(),
                ret: Box::new(self.type_context.fresh_type_var()),
            };
            let block_def_id = self.symbols.define(
                "__block".to_string(),
                DefKind::Param {
                    ty: block_ty.clone(),
                    auto_assign: false,
                },
                Visibility::Private,
                f.span.clone(),
            );
            self.scopes.insert("__block".to_string(), block_def_id);
            params.push(HirParam {
                def_id: block_def_id,
                name: "__block".to_string(),
                ty: block_ty,
                auto_assign: false,
                span: f.span.clone(),
            });
        }

        let return_ty = f
            .return_type
            .as_ref()
            .map(|t| self.resolve_type_expr(t))
            .unwrap_or_else(|| {
                // Default to Unit for:
                // - init methods (constructors)
                // - mut methods (typically mutate in place, return nothing)
                // - main function
                // - display/display_all methods (void-like)
                // Otherwise use a fresh type var for inference
                let is_mut = matches!(f.self_mode, Some(ast::SelfMode::Mutable));
                let is_init = f.name == "init";
                let is_main = f.name == "main" && self.current_self_ty.is_none();
                let is_display_like = f.name == "display" || f.name == "display_all";
                if is_init || is_mut || is_main || is_display_like {
                    Ty::Unit
                } else {
                    self.type_context.fresh_type_var()
                }
            });

        let old_return_ty = self.current_return_ty.replace(return_ty.clone());
        let old_async_scope_depth = self.async_scope_depth;
        if f.is_async {
            self.async_scope_depth += 1;
        }

        let body = self.resolve_block_as_expr(&f.body);

        self.async_scope_depth = old_async_scope_depth;
        self.current_return_ty = old_return_ty;
        self.scopes.pop();

        let sig = FnSignature {
            self_mode,
            is_class_method: f.is_class_method,
            is_async: f.is_async,
            generic_params: self.collect_generic_param_infos(&f.generic_params),
            params: params
                .iter()
                .map(|p| ParamInfo {
                    name: p.name.clone(),
                    ty: p.ty.clone(),
                    auto_assign: p.auto_assign,
                })
                .collect(),
            return_ty: return_ty.clone(),
            c_symbol: None,
        };

        let def_kind = if let Some(parent) = parent {
            DefKind::Method {
                parent,
                signature: sig,
            }
        } else {
            DefKind::Function { signature: sig }
        };

        let def_id = self
            .symbols
            .define(f.name.clone(), def_kind, f.visibility, f.span.clone());

        // Register the function name in the enclosing scope (not the function scope we just popped)
        self.scopes.insert(f.name.clone(), def_id);

        HirFuncDef {
            def_id,
            name: f.name.clone(),
            visibility: f.visibility,
            is_async: f.is_async,
            self_mode,
            is_class_method: f.is_class_method,
            generic_params,
            params,
            return_ty,
            body: Box::new(body),
            doc_comments: f.doc_comments.clone(),
            span: f.span.clone(),
        }
    }

    // ─── Module Resolution ──────────────────────────────────────────

    // ─── Use Declaration Resolution ────────────────────────────────

    fn resolve_use_decl(&mut self, use_decl: &ast::UseDecl) {
        let path = &use_decl.path;
        if path.is_empty() {
            self.diagnostics.push(Diagnostic::error(
                "empty use path".to_string(),
                use_decl.span.clone(),
            ));
            return;
        }

        // Try to resolve the first segment as a known type or module
        let first = &path[0];
        let root_def_id = self
            .scopes
            .lookup_type(first)
            .or_else(|| self.scopes.lookup(first));

        match root_def_id {
            Some(def_id) => {
                // Walk the remaining path segments to resolve nested names
                let target_def_id = self.resolve_use_path_from(def_id, &path[1..], use_decl);

                if let Some(final_id) = target_def_id {
                    // Import the name(s) into the current scope based on UseKind
                    match &use_decl.kind {
                        ast::UseKind::Simple => {
                            // `use Foo.Bar.Baz` — import the last segment name
                            let import_name = path.last().unwrap().clone();
                            self.scopes.insert(import_name.clone(), final_id);
                            self.scopes.insert_type(import_name, final_id);
                        }
                        ast::UseKind::Alias(alias) => {
                            // `use Foo.Bar as B` — import under the alias
                            self.scopes.insert(alias.clone(), final_id);
                            self.scopes.insert_type(alias.clone(), final_id);
                        }
                        ast::UseKind::Group(names) => {
                            // `use Foo.Bar.{ X, Y }` — import each named item
                            // final_id should be a module; resolve each name within it
                            for name in names {
                                let child_id = self.resolve_child_in_def(final_id, name, use_decl);
                                if let Some(cid) = child_id {
                                    self.scopes.insert(name.clone(), cid);
                                    self.scopes.insert_type(name.clone(), cid);
                                }
                            }
                        }
                    }
                }
            }
            None => {
                self.diagnostics.push(Diagnostic::error(
                    format!(
                        "unknown module '{}'. Did you forget to add it to [dependencies]?",
                        first
                    ),
                    use_decl.span.clone(),
                ));
            }
        }
    }

    /// Walk a use path from a starting DefId through remaining segments.
    fn resolve_use_path_from(
        &mut self,
        mut current: DefId,
        segments: &[String],
        use_decl: &ast::UseDecl,
    ) -> Option<DefId> {
        for seg in segments {
            match self.resolve_child_in_def(current, seg, use_decl) {
                Some(child) => current = child,
                None => return None,
            }
        }
        Some(current)
    }

    /// Resolve a child name within a definition (module, class, enum, etc.).
    fn resolve_child_in_def(
        &mut self,
        parent: DefId,
        name: &str,
        use_decl: &ast::UseDecl,
    ) -> Option<DefId> {
        let parent_def = self.symbols.get(parent).cloned();
        match parent_def {
            Some(def) => {
                match &def.kind {
                    DefKind::Module { items } => {
                        // Search module items for the name
                        for &item_id in items {
                            if let Some(item_def) = self.symbols.get(item_id) {
                                if item_def.name == name {
                                    return Some(item_id);
                                }
                            }
                        }
                        self.diagnostics.push(Diagnostic::error(
                            format!("'{}' not found in module '{}'", name, def.name),
                            use_decl.span.clone(),
                        ));
                        None
                    }
                    DefKind::Enum { info } => {
                        // Allow `use MyEnum.Variant`
                        for &variant_id in &info.variants {
                            if let Some(variant_def) = self.symbols.get(variant_id) {
                                if variant_def.name == name {
                                    return Some(variant_id);
                                }
                            }
                        }
                        self.diagnostics.push(Diagnostic::error(
                            format!("'{}' is not a variant of enum '{}'", name, def.name),
                            use_decl.span.clone(),
                        ));
                        None
                    }
                    DefKind::Class { info } => {
                        // Allow `use MyClass.method` for class methods
                        for &method_id in &info.methods {
                            if let Some(method_def) = self.symbols.get(method_id) {
                                if method_def.name == name {
                                    return Some(method_id);
                                }
                            }
                        }
                        self.diagnostics.push(Diagnostic::error(
                            format!("'{}' not found in class '{}'", name, def.name),
                            use_decl.span.clone(),
                        ));
                        None
                    }
                    _ => {
                        self.diagnostics.push(Diagnostic::error(
                            format!("'{}' is not a module or namespace", def.name),
                            use_decl.span.clone(),
                        ));
                        None
                    }
                }
            }
            None => {
                self.diagnostics.push(Diagnostic::error(
                    "unresolved name in use path".to_string(),
                    use_decl.span.clone(),
                ));
                None
            }
        }
    }

    fn resolve_module(&mut self, m: &ast::ModuleDef) -> HirModule {
        let def_id = self
            .type_registry
            .get(&m.name)
            .copied()
            .unwrap_or(UNRESOLVED_DEF);

        self.scopes.push(ScopeKind::Module);

        let mut items = Vec::new();
        for item in &m.items {
            if let Some(hir_item) = self.resolve_item(item) {
                items.push(hir_item);
            }
        }

        self.scopes.pop();

        HirModule {
            def_id,
            name: m.name.clone(),
            items,
            span: m.span.clone(),
        }
    }

    // ─── Expression Resolution ──────────────────────────────────────

    fn resolve_expr(&mut self, expr: &ast::Expr) -> HirExpr {
        let span = expr.span.clone();
        match &expr.kind {
            ast::ExprKind::IntLiteral(val, suffix) => {
                let ty = self.int_literal_type(*suffix);
                HirExpr {
                    kind: HirExprKind::IntLiteral(*val),
                    ty,
                    span,
                }
            }
            ast::ExprKind::FloatLiteral(val, suffix) => {
                let ty = self.float_literal_type(*suffix);
                HirExpr {
                    kind: HirExprKind::FloatLiteral(*val),
                    ty,
                    span,
                }
            }
            ast::ExprKind::StringLiteral(s) => HirExpr {
                kind: HirExprKind::StringLiteral(s.clone()),
                ty: Ty::Str,
                span,
            },
            ast::ExprKind::InterpolatedString(parts) => {
                let hir_parts: Vec<HirInterpolationPart> = parts
                    .iter()
                    .map(|p| {
                        match p {
                            crate::lexer::token::StringPart::Literal(s) => {
                                HirInterpolationPart::Literal(s.clone())
                            }
                            crate::lexer::token::StringPart::Expr { tokens, spec } => {
                                // Parse the interpolation tokens as an expression.
                                // The format spec (Phase 2 #06.B) is threaded through
                                // unchanged to MIR — Phase C/D consume it.
                                let inner_expr = self.resolve_interpolation_tokens(tokens, &span);
                                HirInterpolationPart::Expr {
                                    expr: inner_expr,
                                    spec: spec.clone(),
                                }
                            }
                        }
                    })
                    .collect();
                HirExpr {
                    kind: HirExprKind::Interpolation { parts: hir_parts },
                    ty: Ty::String, // interpolated strings produce owned Strings
                    span,
                }
            }
            ast::ExprKind::CharLiteral(c) => HirExpr {
                kind: HirExprKind::CharLiteral(*c),
                ty: Ty::Char,
                span,
            },
            ast::ExprKind::BoolLiteral(b) => HirExpr {
                kind: HirExprKind::BoolLiteral(*b),
                ty: Ty::Bool,
                span,
            },
            ast::ExprKind::UnitLiteral => HirExpr {
                kind: HirExprKind::UnitLiteral,
                ty: Ty::Unit,
                span,
            },
            ast::ExprKind::Identifier(name) => {
                if let Some((def_id, def_scope_id)) = self.scopes.lookup_with_scope(name) {
                    // If the identifier resolves to an enum variant (e.g.
                    // bare `None`, `Color.Red`), lower it as an
                    // EnumVariant construction rather than a VarRef so
                    // codegen allocates and tags it correctly.
                    if let Some(def) = self.symbols.get(def_id) {
                        if let DefKind::EnumVariant {
                            parent,
                            variant_idx,
                            ..
                        } = def.kind
                        {
                            let parent_name = self
                                .symbols
                                .get(parent)
                                .map(|p| p.name.clone())
                                .unwrap_or_default();
                            return HirExpr {
                                kind: HirExprKind::EnumVariant {
                                    type_def: parent,
                                    type_name: parent_name.clone(),
                                    variant_name: name.clone(),
                                    variant_idx,
                                    fields: vec![],
                                },
                                ty: Ty::Enum {
                                    name: parent_name,
                                    generic_args: vec![],
                                },
                                span,
                            };
                        }
                    }
                    self.record_capture_if_needed(def_id, def_scope_id);
                    // Phase 2 stdlib (#06): Class/Struct identifiers
                    // imported via `use std.process.Command` (or
                    // otherwise reached through the value-scope
                    // `lookup` rather than `lookup_type`) must surface
                    // as their own `Ty::Class { name }` /
                    // `Ty::Struct { name }` so a subsequent
                    // `.new(...)` MethodCall sees a concrete receiver
                    // and dispatches through the collection-ctor fast
                    // path in mir/lower.rs. Without this branch,
                    // `def_ty` returns None for Class kinds
                    // (intentionally — see
                    // `def_ty_returns_none_for_class` in symbols.rs)
                    // and the receiver collapses to a fresh inference
                    // variable, which means `.arg/.status/etc.` would
                    // never resolve to the right `builtin_method_type`
                    // arm.
                    //
                    // Enum is intentionally NOT promoted here: enum
                    // identifiers reach this path only as the receiver
                    // for `EnumName.Variant(...)` constructor calls,
                    // which are parsed as their own AST shape
                    // (`ExprKind::EnumVariant`) and reach
                    // `resolve_expr` through a different arm. Limiting
                    // the promotion to Class/Struct also keeps the
                    // 226-fixture e2e baseline stable (only the pre-
                    // existing `95_error_into_conversion` typecheck
                    // failure remains; everything else passes).
                    let ty = match self.symbols.get(def_id).map(|d| &d.kind) {
                        Some(DefKind::Class { .. }) => Ty::Class {
                            name: name.clone(),
                            generic_args: vec![],
                        },
                        Some(DefKind::Struct { .. }) => Ty::Struct {
                            name: name.clone(),
                            generic_args: vec![],
                        },
                        _ => self
                            .symbols
                            .def_ty(def_id)
                            .unwrap_or_else(|| self.type_context.fresh_type_var()),
                    };
                    HirExpr {
                        kind: HirExprKind::VarRef(def_id),
                        ty,
                        span,
                    }
                } else if let Some(def_id) = self.scopes.lookup_type(name) {
                    // Type name used as a value — needed for constructor calls
                    // like Point.new(...), Color.Red, etc.
                    let ty = match self.symbols.get(def_id).map(|d| &d.kind) {
                        Some(DefKind::Class { .. }) => Ty::Class {
                            name: name.clone(),
                            generic_args: vec![],
                        },
                        Some(DefKind::Struct { .. }) => Ty::Struct {
                            name: name.clone(),
                            generic_args: vec![],
                        },
                        Some(DefKind::Enum { .. }) => Ty::Enum {
                            name: name.clone(),
                            generic_args: vec![],
                        },
                        _ => self.type_context.fresh_type_var(),
                    };
                    HirExpr {
                        kind: HirExprKind::VarRef(def_id),
                        ty,
                        span,
                    }
                } else {
                    self.error(format!("undefined variable `{}`", name), &span);
                    HirExpr {
                        kind: HirExprKind::Error,
                        ty: Ty::Error,
                        span,
                    }
                }
            }
            ast::ExprKind::SelfRef => {
                if let Some(def_id) = self.scopes.lookup("self") {
                    let ty = self.current_self_ty.clone().unwrap_or(Ty::Error);
                    HirExpr {
                        kind: HirExprKind::VarRef(def_id),
                        ty,
                        span,
                    }
                } else {
                    self.error("`self` used outside of method context".to_string(), &span);
                    HirExpr {
                        kind: HirExprKind::Error,
                        ty: Ty::Error,
                        span,
                    }
                }
            }
            ast::ExprKind::SelfType => {
                if let Some(ref ty) = self.current_self_ty {
                    let def_id = self.scopes.lookup_type("Self").unwrap_or(UNRESOLVED_DEF);
                    HirExpr {
                        kind: HirExprKind::VarRef(def_id),
                        ty: ty.clone(),
                        span,
                    }
                } else {
                    self.error("`Self` used outside of type context".to_string(), &span);
                    HirExpr {
                        kind: HirExprKind::Error,
                        ty: Ty::Error,
                        span,
                    }
                }
            }
            ast::ExprKind::BinaryOp { left, op, right } => {
                let left_hir = self.resolve_expr(left);
                let right_hir = self.resolve_expr(right);
                let result_ty = self.type_context.fresh_type_var();
                HirExpr {
                    kind: HirExprKind::BinaryOp {
                        op: *op,
                        left: Box::new(left_hir),
                        right: Box::new(right_hir),
                    },
                    ty: result_ty,
                    span,
                }
            }
            ast::ExprKind::UnaryOp { op, operand } => {
                let operand_hir = self.resolve_expr(operand);
                let result_ty = self.type_context.fresh_type_var();
                HirExpr {
                    kind: HirExprKind::UnaryOp {
                        op: *op,
                        operand: Box::new(operand_hir),
                    },
                    ty: result_ty,
                    span,
                }
            }
            ast::ExprKind::Borrow(inner) => {
                let inner_hir = self.resolve_expr(inner);
                let ty = Ty::Ref(Box::new(inner_hir.ty.clone()));
                HirExpr {
                    kind: HirExprKind::Borrow {
                        mutable: false,
                        expr: Box::new(inner_hir),
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::BorrowMut(inner) => {
                let inner_hir = self.resolve_expr(inner);
                let ty = Ty::RefMut(Box::new(inner_hir.ty.clone()));
                HirExpr {
                    kind: HirExprKind::Borrow {
                        mutable: true,
                        expr: Box::new(inner_hir),
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::FieldAccess { object, field } => {
                let obj_hir = self.resolve_expr(object);
                let ty = self.type_context.fresh_type_var();
                HirExpr {
                    kind: HirExprKind::FieldAccess {
                        object: Box::new(obj_hir),
                        field_name: field.clone(),
                        field_idx: 0, // resolved during type checking
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::MethodCall {
                object,
                method,
                generic_args,
                args,
                block,
            } => {
                let obj_hir = self.resolve_expr(object);
                let args_hir: Vec<HirExpr> = args.iter().map(|a| self.resolve_expr(a)).collect();
                let block_hir = block.as_ref().map(|b| Box::new(self.resolve_expr(b)));
                let generic_args_hir = generic_args
                    .iter()
                    .map(|a| self.resolve_type_expr(a))
                    .collect();
                let ty = self.type_context.fresh_type_var();
                HirExpr {
                    kind: HirExprKind::MethodCall {
                        object: Box::new(obj_hir),
                        method: UNRESOLVED_DEF, // resolved during type checking
                        method_name: method.clone(),
                        generic_args: generic_args_hir,
                        args: args_hir,
                        block: block_hir,
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::Call {
                callee,
                args,
                block,
            } => {
                let mut args_hir: Vec<HirExpr> =
                    args.iter().map(|a| self.resolve_expr(a)).collect();
                let mut block_hir = block.as_ref().map(|b| Box::new(self.resolve_expr(b)));

                // Try to resolve the callee
                match &callee.kind {
                    ast::ExprKind::Identifier(name) => {
                        // If `name` names a function that takes an implicit
                        // block (i.e. its body contains `yield`), forward
                        // the trailing block as the last argument and emit
                        // a plain `FnCall`.  The callee's signature was
                        // given an extra trailing `__block` parameter.
                        let takes_implicit_block = self.yield_fns.contains_key(name);
                        if takes_implicit_block {
                            if let Some(blk) = block_hir.take() {
                                args_hir.push(*blk);
                            }
                            if let Some(def_id) = self.scopes.lookup(name) {
                                let ty = self.type_context.fresh_type_var();
                                return HirExpr {
                                    kind: HirExprKind::FnCall {
                                        callee: def_id,
                                        callee_name: name.clone(),
                                        args: args_hir,
                                    },
                                    ty,
                                    span,
                                };
                            }
                        }
                        if let Some(def_id) = self.scopes.lookup(name) {
                            let ty = self.type_context.fresh_type_var();
                            // Check if this is a function or a closure call
                            let kind = match block_hir {
                                Some(blk) => HirExprKind::MethodCall {
                                    object: Box::new(HirExpr {
                                        kind: HirExprKind::VarRef(def_id),
                                        ty: self.symbols.def_ty(def_id).unwrap_or(Ty::Error),
                                        span: callee.span.clone(),
                                    }),
                                    method: UNRESOLVED_DEF,
                                    method_name: "call".to_string(),
                                    generic_args: vec![],
                                    args: args_hir,
                                    block: Some(blk),
                                },
                                None => HirExprKind::FnCall {
                                    callee: def_id,
                                    callee_name: name.clone(),
                                    args: args_hir,
                                },
                            };
                            HirExpr { kind, ty, span }
                        } else if let Some(type_def_id) = self.scopes.lookup_type(name) {
                            // `Name(arg)` where `Name` is the name of a type.
                            // For a zero-cost `newtype Meters(Float)` wrapper
                            // this desugars to a single-field Construct that
                            // can later be read back via `.0`.
                            if let Some(def) = self.symbols.get(type_def_id) {
                                if let DefKind::Newtype { inner } = &def.kind {
                                    let inner_ty = inner.clone();
                                    if args_hir.len() != 1 {
                                        self.error(
                                            format!(
                                                "newtype `{}` expects exactly 1 argument, got {}",
                                                name,
                                                args_hir.len(),
                                            ),
                                            &span,
                                        );
                                        return HirExpr {
                                            kind: HirExprKind::Error,
                                            ty: Ty::Error,
                                            span,
                                        };
                                    }
                                    let arg = args_hir.into_iter().next().unwrap();
                                    let ty = Ty::Newtype {
                                        name: name.clone(),
                                        inner: Box::new(inner_ty),
                                    };
                                    return HirExpr {
                                        kind: HirExprKind::Construct {
                                            type_def: type_def_id,
                                            type_name: name.clone(),
                                            fields: vec![("0".to_string(), arg)],
                                        },
                                        ty,
                                        span,
                                    };
                                }
                            }
                            self.error(format!("undefined function `{}`", name), &span);
                            HirExpr {
                                kind: HirExprKind::Error,
                                ty: Ty::Error,
                                span,
                            }
                        } else {
                            // Could be a type constructor: Type.new(...)
                            self.error(format!("undefined function `{}`", name), &span);
                            HirExpr {
                                kind: HirExprKind::Error,
                                ty: Ty::Error,
                                span,
                            }
                        }
                    }
                    // FieldAccess could be a static method call: Type.method(...)
                    ast::ExprKind::FieldAccess { object, field } => {
                        let obj_hir = self.resolve_expr(object);
                        let ty = self.type_context.fresh_type_var();
                        HirExpr {
                            kind: HirExprKind::MethodCall {
                                object: Box::new(obj_hir),
                                method: UNRESOLVED_DEF,
                                method_name: field.clone(),
                                generic_args: vec![],
                                args: args_hir,
                                block: block_hir,
                            },
                            ty,
                            span,
                        }
                    }
                    _ => {
                        let callee_hir = self.resolve_expr(callee);
                        let ty = self.type_context.fresh_type_var();
                        HirExpr {
                            kind: HirExprKind::MethodCall {
                                object: Box::new(callee_hir),
                                method: UNRESOLVED_DEF,
                                method_name: "call".to_string(),
                                generic_args: vec![],
                                args: args_hir,
                                block: block_hir,
                            },
                            ty,
                            span,
                        }
                    }
                }
            }
            ast::ExprKind::Index { object, index } => {
                let obj_hir = self.resolve_expr(object);
                let idx_hir = self.resolve_expr(index);
                let ty = self.type_context.fresh_type_var();
                HirExpr {
                    kind: HirExprKind::Index {
                        object: Box::new(obj_hir),
                        index: Box::new(idx_hir),
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::Assign { target, value } => {
                let target_hir = self.resolve_expr(target);
                let value_hir = self.resolve_expr(value);
                HirExpr {
                    kind: HirExprKind::Assign {
                        target: Box::new(target_hir),
                        value: Box::new(value_hir),
                        semantics: MoveSemantics::Move, // determined during type checking
                    },
                    ty: Ty::Unit,
                    span,
                }
            }
            ast::ExprKind::CompoundAssign { target, op, value } => {
                let target_hir = self.resolve_expr(target);
                let value_hir = self.resolve_expr(value);
                HirExpr {
                    kind: HirExprKind::CompoundAssign {
                        target: Box::new(target_hir),
                        op: *op,
                        value: Box::new(value_hir),
                    },
                    ty: Ty::Unit,
                    span,
                }
            }
            ast::ExprKind::If(if_expr) => self.resolve_if(if_expr),
            ast::ExprKind::IfLet(if_let) => self.resolve_if_let(if_let),
            ast::ExprKind::Match(match_expr) => self.resolve_match(match_expr),
            ast::ExprKind::While(while_expr) => {
                let cond = self.resolve_expr(&while_expr.condition);
                self.scopes.push(ScopeKind::Loop);
                let body = self.resolve_block_as_expr(&while_expr.body);
                self.scopes.pop();
                HirExpr {
                    kind: HirExprKind::While {
                        condition: Box::new(cond),
                        body: Box::new(body),
                    },
                    ty: Ty::Unit,
                    span,
                }
            }
            ast::ExprKind::WhileLet(wl) => {
                // Desugar while-let to loop + match
                let value = self.resolve_expr(&wl.value);
                self.scopes.push(ScopeKind::Loop);
                let pattern = self.resolve_pattern(&wl.pattern);
                let body = self.resolve_block_as_expr(&wl.body);
                self.scopes.pop();
                let break_expr = HirExpr {
                    kind: HirExprKind::Break(None),
                    ty: Ty::Never,
                    span: span.clone(),
                };
                HirExpr {
                    kind: HirExprKind::Loop {
                        body: Box::new(HirExpr {
                            kind: HirExprKind::Match {
                                scrutinee: Box::new(value),
                                arms: vec![
                                    HirMatchArm {
                                        pattern,
                                        guard: None,
                                        body: Box::new(body),
                                        span: span.clone(),
                                    },
                                    HirMatchArm {
                                        pattern: HirPattern::Wildcard { span: span.clone() },
                                        guard: None,
                                        body: Box::new(break_expr),
                                        span: span.clone(),
                                    },
                                ],
                            },
                            ty: Ty::Unit,
                            span: span.clone(),
                        }),
                    },
                    ty: Ty::Unit,
                    span,
                }
            }
            ast::ExprKind::For(for_expr) => {
                let iterable = self.resolve_expr(&for_expr.iterable);
                self.scopes.push(ScopeKind::Loop);
                let binding_name = self.pattern_binding_name(&for_expr.pattern);
                let binding_ty = self.type_context.fresh_type_var();
                let binding_def = self.symbols.define(
                    binding_name.clone(),
                    DefKind::Variable {
                        mutable: false,
                        ty: binding_ty.clone(),
                    },
                    Visibility::Private,
                    for_expr.pattern.span().clone(),
                );
                self.scopes.insert(binding_name.clone(), binding_def);
                // For tuple patterns like (i, result), also register each sub-binding
                // and collect their DefIds so the MIR lowerer can destructure.
                let mut tuple_bindings = Vec::new();
                if let ast::Pattern::Tuple { elements, .. } = &for_expr.pattern {
                    self.register_pattern_bindings(
                        &for_expr.pattern,
                        false,
                        for_expr.pattern.span(),
                    );
                    for elem in elements {
                        if let ast::Pattern::Identifier { name, .. } = elem {
                            if let Some(def_id) = self.scopes.lookup(name) {
                                tuple_bindings.push((def_id, name.clone()));
                            }
                        }
                    }
                }
                let body = self.resolve_block_as_expr(&for_expr.body);
                self.scopes.pop();
                HirExpr {
                    kind: HirExprKind::For {
                        binding: binding_def,
                        binding_name,
                        iterable: Box::new(iterable),
                        body: Box::new(body),
                        tuple_bindings,
                    },
                    ty: Ty::Unit,
                    span,
                }
            }
            ast::ExprKind::Loop(loop_expr) => {
                self.scopes.push(ScopeKind::Loop);
                let body = self.resolve_block_as_expr(&loop_expr.body);
                self.scopes.pop();
                HirExpr {
                    kind: HirExprKind::Loop {
                        body: Box::new(body),
                    },
                    ty: self.type_context.fresh_type_var(),
                    span,
                }
            }
            ast::ExprKind::Block(block) => self.resolve_block_as_expr(block),
            ast::ExprKind::Closure(closure) => self.resolve_closure(closure, &span),
            ast::ExprKind::Return(value) => {
                let value_hir = value.as_ref().map(|v| Box::new(self.resolve_expr(v)));
                HirExpr {
                    kind: HirExprKind::Return(value_hir),
                    ty: Ty::Never,
                    span,
                }
            }
            ast::ExprKind::Break(value) => {
                if !self.scopes.in_loop() {
                    self.error("`break` used outside of loop".to_string(), &span);
                }
                let value_hir = value.as_ref().map(|v| Box::new(self.resolve_expr(v)));
                HirExpr {
                    kind: HirExprKind::Break(value_hir),
                    ty: Ty::Never,
                    span,
                }
            }
            ast::ExprKind::Continue => {
                if !self.scopes.in_loop() {
                    self.error("`continue` used outside of loop".to_string(), &span);
                }
                HirExpr {
                    kind: HirExprKind::Continue,
                    ty: Ty::Never,
                    span,
                }
            }
            ast::ExprKind::Range {
                start,
                end,
                inclusive,
            } => {
                let start_hir = start.as_ref().map(|s| Box::new(self.resolve_expr(s)));
                let end_hir = end.as_ref().map(|e| Box::new(self.resolve_expr(e)));
                let ty = self.type_context.fresh_type_var();
                HirExpr {
                    kind: HirExprKind::Range {
                        start: start_hir,
                        end: end_hir,
                        inclusive: *inclusive,
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::ArrayLiteral(elems) => {
                let elems_hir: Vec<HirExpr> = elems.iter().map(|e| self.resolve_expr(e)).collect();
                let elem_ty = if elems_hir.is_empty() {
                    self.type_context.fresh_type_var()
                } else {
                    elems_hir[0].ty.clone()
                };
                let ty = Ty::Array(Box::new(elem_ty));
                HirExpr {
                    kind: HirExprKind::ArrayLiteral(elems_hir),
                    ty,
                    span,
                }
            }
            ast::ExprKind::MapLiteral(entries) => {
                let entries_hir: Vec<(HirExpr, HirExpr)> = entries
                    .iter()
                    .map(|(k, v)| (self.resolve_expr(k), self.resolve_expr(v)))
                    .collect();
                let (k_ty, v_ty) = if let Some((k, v)) = entries_hir.first() {
                    (k.ty.clone(), v.ty.clone())
                } else {
                    (
                        self.type_context.fresh_type_var(),
                        self.type_context.fresh_type_var(),
                    )
                };
                let ty = Ty::Map(Box::new(k_ty), Box::new(v_ty));
                HirExpr {
                    kind: HirExprKind::MapLiteral(entries_hir),
                    ty,
                    span,
                }
            }
            ast::ExprKind::ArrayFill { value, count } => {
                let value_hir = self.resolve_expr(value);
                let count_hir = self.resolve_expr(count);
                let elem_ty = value_hir.ty.clone();
                // Try to extract count as a usize
                let count_val = match &count_hir.kind {
                    HirExprKind::IntLiteral(n) => *n as usize,
                    _ => 0, // will be validated during type checking
                };
                HirExpr {
                    kind: HirExprKind::ArrayFill {
                        value: Box::new(value_hir),
                        count: count_val,
                    },
                    ty: Ty::FixedArray(
                        Box::new(elem_ty),
                        crate::hir::types::ConstExpr::Lit(count_val as u64),
                    ),
                    span,
                }
            }
            ast::ExprKind::TupleLiteral(elems) => {
                let elems_hir: Vec<HirExpr> = elems.iter().map(|e| self.resolve_expr(e)).collect();
                let tys: Vec<Ty> = elems_hir.iter().map(|e| e.ty.clone()).collect();
                HirExpr {
                    kind: HirExprKind::Tuple(elems_hir),
                    ty: Ty::Tuple(tys),
                    span,
                }
            }
            ast::ExprKind::Cast {
                expr: inner,
                target_type,
            } => {
                let inner_hir = self.resolve_expr(inner);
                let target = self.resolve_type_expr(target_type);
                HirExpr {
                    kind: HirExprKind::Cast {
                        expr: Box::new(inner_hir),
                        target: target.clone(),
                    },
                    ty: target,
                    span,
                }
            }
            ast::ExprKind::Await(inner) => {
                if self.async_scope_depth == 0 {
                    self.diagnostics.push(Diagnostic::error_with_code(
                        "cannot `await` outside an `async` function or closure",
                        span.clone(),
                        "E_await_outside_async",
                    ));
                }
                let inner_hir = self.resolve_expr(inner);
                let ty = self.type_context.fresh_type_var();
                HirExpr {
                    kind: HirExprKind::MethodCall {
                        object: Box::new(inner_hir),
                        method: UNRESOLVED_DEF,
                        method_name: "await".to_string(),
                        generic_args: vec![],
                        args: vec![],
                        block: None,
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::Try(inner) => {
                // Desugar `expr?` to match + early return
                let inner_hir = self.resolve_expr(inner);
                let result_ty = self.type_context.fresh_type_var();
                // For now, represent as a method call to a special `try_unwrap` operation
                // The type checker will handle the actual desugaring
                HirExpr {
                    kind: HirExprKind::MethodCall {
                        object: Box::new(inner_hir),
                        method: UNRESOLVED_DEF,
                        method_name: "try_op".to_string(),
                        generic_args: vec![],
                        args: vec![],
                        block: None,
                    },
                    ty: result_ty,
                    span,
                }
            }
            ast::ExprKind::SafeNav { object, field } => {
                let obj_hir = self.resolve_expr(object);
                let ty = self.type_context.fresh_type_var();
                // Desugar `x?.field` to match on Option
                HirExpr {
                    kind: HirExprKind::FieldAccess {
                        object: Box::new(obj_hir),
                        field_name: field.clone(),
                        field_idx: 0,
                    },
                    ty: Ty::Option(Box::new(ty)),
                    span,
                }
            }
            ast::ExprKind::SafeNavCall {
                object,
                method,
                args,
            } => {
                let obj_hir = self.resolve_expr(object);
                let args_hir: Vec<HirExpr> = args.iter().map(|a| self.resolve_expr(a)).collect();
                let ty = self.type_context.fresh_type_var();
                HirExpr {
                    kind: HirExprKind::MethodCall {
                        object: Box::new(obj_hir),
                        method: UNRESOLVED_DEF,
                        method_name: method.clone(),
                        generic_args: vec![],
                        args: args_hir,
                        block: None,
                    },
                    ty: Ty::Option(Box::new(ty)),
                    span,
                }
            }
            ast::ExprKind::MacroCall { name, args, .. } => {
                let args_hir: Vec<HirExpr> = args.iter().map(|a| self.resolve_expr(a)).collect();
                let ty = match name.as_str() {
                    // ruby-naming.spec.md §10a:
                    //   `vec![...]` → `array![...]`
                    //   `hash!{...}` → `map!{...}`
                    //   `set!{...}` (unchanged)
                    // Both old and new macro names produce identical HIR
                    // while sources transition.
                    "vec" | "array" => {
                        let elem_ty = if args_hir.is_empty() {
                            self.type_context.fresh_type_var()
                        } else {
                            args_hir[0].ty.clone()
                        };
                        Ty::Array(Box::new(elem_ty))
                    }
                    "hash" | "map" => {
                        let (k, v) = if args_hir.len() >= 2 {
                            (args_hir[0].ty.clone(), args_hir[1].ty.clone())
                        } else {
                            (
                                self.type_context.fresh_type_var(),
                                self.type_context.fresh_type_var(),
                            )
                        };
                        Ty::Map(Box::new(k), Box::new(v))
                    }
                    "set" => {
                        let elem = if args_hir.is_empty() {
                            self.type_context.fresh_type_var()
                        } else {
                            args_hir[0].ty.clone()
                        };
                        Ty::Set(Box::new(elem))
                    }
                    "panic" => Ty::Never,
                    _ => self.type_context.fresh_type_var(),
                };
                HirExpr {
                    kind: HirExprKind::MacroCall {
                        name: name.clone(),
                        args: args_hir,
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::EnumVariant {
                type_path,
                variant,
                args,
            } => {
                let type_name = type_path.join(".");
                let composite = format!("{}.{}", type_name, variant);
                let variant_def = self.scopes.lookup(&composite).unwrap_or(UNRESOLVED_DEF);
                let mut type_def = self
                    .type_registry
                    .get(&type_name)
                    .copied()
                    .unwrap_or(UNRESOLVED_DEF);

                // For bare variants (Ok, Err, Some, None) where type_path is empty,
                // look up the parent enum from the variant definition
                let mut resolved_type_name = type_name.clone();
                if type_def == UNRESOLVED_DEF && variant_def != UNRESOLVED_DEF {
                    if let Some(def) = self.symbols.get(variant_def) {
                        if let DefKind::EnumVariant { parent, .. } = &def.kind {
                            type_def = *parent;
                            if let Some(parent_def) = self.symbols.get(*parent) {
                                resolved_type_name = parent_def.name.clone();
                            }
                        }
                    }
                }

                // Extract variant_idx first to avoid borrow conflicts
                let variant_idx = if variant_def != UNRESOLVED_DEF {
                    self.symbols
                        .get(variant_def)
                        .and_then(|def| {
                            if let DefKind::EnumVariant { variant_idx, .. } = &def.kind {
                                Some(*variant_idx)
                            } else {
                                None
                            }
                        })
                        .unwrap_or(0)
                } else {
                    self.error(
                        format!("undefined enum variant `{}.{}`", type_name, variant),
                        &span,
                    );
                    0
                };

                let fields_hir: Vec<(String, HirExpr)> = args
                    .iter()
                    .map(|fa| {
                        (
                            fa.name.clone().unwrap_or_default(),
                            self.resolve_expr(&fa.value),
                        )
                    })
                    .collect();

                let ty = if type_def != UNRESOLVED_DEF {
                    Ty::Enum {
                        name: resolved_type_name.clone(),
                        generic_args: vec![],
                    }
                } else {
                    Ty::Error
                };

                HirExpr {
                    kind: HirExprKind::EnumVariant {
                        type_def,
                        type_name: resolved_type_name,
                        variant_name: variant.clone(),
                        variant_idx,
                        fields: fields_hir,
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::ClosureCall { callee, args } => {
                let callee_hir = self.resolve_expr(callee);
                let args_hir: Vec<HirExpr> = args.iter().map(|a| self.resolve_expr(a)).collect();
                let ty = self.type_context.fresh_type_var();
                HirExpr {
                    kind: HirExprKind::MethodCall {
                        object: Box::new(callee_hir),
                        method: UNRESOLVED_DEF,
                        method_name: "call".to_string(),
                        generic_args: vec![],
                        args: args_hir,
                        block: None,
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::Yield(args) => {
                let args_hir: Vec<HirExpr> = args.iter().map(|a| self.resolve_expr(a)).collect();
                // `yield VALUE …` desugars to `BLOCK.(VALUE …)`, encoded as
                // a MethodCall with method_name == "call" on the enclosing
                // function's block parameter.  Prefer the synthetic
                // `__block` inserted for implicit-block functions; fall
                // back to the explicit `&block` parameter name that the
                // older `Block(…)` syntax produces.  If neither is in
                // scope (e.g. a `yield` sitting inside a nested closure
                // whose enclosing method has no block), we keep the old
                // unresolved-FnCall shape so downstream passes can report
                // a clearer error.
                let block_def = self
                    .scopes
                    .lookup("__block")
                    .or_else(|| self.scopes.lookup("&block"));
                if let Some(block_def) = block_def {
                    let block_ty = self.symbols.def_ty(block_def).unwrap_or(Ty::Error);
                    let callee = HirExpr {
                        kind: HirExprKind::VarRef(block_def),
                        ty: block_ty,
                        span: span.clone(),
                    };
                    let ty = self.type_context.fresh_type_var();
                    HirExpr {
                        kind: HirExprKind::MethodCall {
                            object: Box::new(callee),
                            method: UNRESOLVED_DEF,
                            method_name: "call".to_string(),
                            generic_args: vec![],
                            args: args_hir,
                            block: None,
                        },
                        ty,
                        span,
                    }
                } else {
                    let ty = self.type_context.fresh_type_var();
                    HirExpr {
                        kind: HirExprKind::FnCall {
                            callee: UNRESOLVED_DEF,
                            callee_name: "yield".to_string(),
                            args: args_hir,
                        },
                        ty,
                        span,
                    }
                }
            }
            ast::ExprKind::UnsafeBlock(block) => {
                // Resolve the unsafe block body just like a regular block.
                self.scopes.push(ScopeKind::Block);
                let mut stmts = Vec::new();
                let mut tail_expr = None;
                for (i, stmt) in block.statements.iter().enumerate() {
                    let is_last = i == block.statements.len() - 1;
                    match stmt {
                        ast::Statement::Let(binding) => {
                            stmts.push(self.resolve_let(binding));
                        }
                        ast::Statement::Expression(expr) => {
                            let hir_expr = self.resolve_expr(expr);
                            if is_last {
                                tail_expr = Some(Box::new(hir_expr));
                            } else {
                                stmts.push(HirStatement::Expr(hir_expr));
                            }
                        }
                    }
                }
                self.scopes.pop();
                let ty = tail_expr.as_ref().map(|e| e.ty.clone()).unwrap_or(Ty::Unit);
                HirExpr {
                    kind: HirExprKind::UnsafeBlock(stmts, tail_expr),
                    ty,
                    span,
                }
            }
            ast::ExprKind::NullLiteral => {
                HirExpr {
                    kind: HirExprKind::NullLiteral,
                    ty: Ty::UInt64, // null is a zero-valued pointer; for now UInt64
                    span,
                }
            }
        }
    }

    // ─── Block Resolution ───────────────────────────────────────────

    fn resolve_block_as_expr(&mut self, block: &ast::Block) -> HirExpr {
        self.scopes.push(ScopeKind::Block);

        let mut stmts = Vec::new();
        let mut tail_expr = None;

        for (i, stmt) in block.statements.iter().enumerate() {
            let is_last = i == block.statements.len() - 1;
            match stmt {
                ast::Statement::Let(binding) => {
                    stmts.push(self.resolve_let(binding));
                }
                ast::Statement::Expression(expr) => {
                    let hir_expr = self.resolve_expr(expr);
                    if is_last {
                        // Last expression in block is the tail (implicit return)
                        tail_expr = Some(Box::new(hir_expr));
                    } else {
                        stmts.push(HirStatement::Expr(hir_expr));
                    }
                }
            }
        }

        self.scopes.pop();

        let ty = tail_expr.as_ref().map(|e| e.ty.clone()).unwrap_or(Ty::Unit);

        HirExpr {
            kind: HirExprKind::Block(stmts, tail_expr),
            ty,
            span: block.span.clone(),
        }
    }

    fn resolve_let(&mut self, binding: &ast::LetBinding) -> HirStatement {
        let ty = binding
            .type_annotation
            .as_ref()
            .map(|t| self.resolve_type_expr(t))
            .unwrap_or_else(|| self.type_context.fresh_type_var());

        let value = binding.value.as_ref().map(|v| self.resolve_expr(v));

        let pattern = self.resolve_pattern_with_type(&binding.pattern, &ty);

        // Register the binding
        let name = self.pattern_binding_name(&binding.pattern);
        let def_id = self.symbols.define(
            name,
            DefKind::Variable {
                mutable: binding.mutable,
                ty: ty.clone(),
            },
            Visibility::Private,
            binding.span.clone(),
        );

        // Insert into current scope
        if let ast::Pattern::Identifier { name, .. } = &binding.pattern {
            self.scopes.insert(name.clone(), def_id);
        } else if let ast::Pattern::Tuple { .. } = &binding.pattern {
            // For tuple destructuring, register each element
            self.register_pattern_bindings(&binding.pattern, binding.mutable, &binding.span);
        } else {
            self.register_pattern_bindings(&binding.pattern, binding.mutable, &binding.span);
        }

        HirStatement::Let {
            def_id,
            pattern,
            ty,
            value,
            mutable: binding.mutable,
            span: binding.span.clone(),
        }
    }

    // ─── If Expression Resolution ───────────────────────────────────

    fn resolve_if(&mut self, if_expr: &ast::IfExpr) -> HirExpr {
        let cond = self.resolve_expr(&if_expr.condition);
        let then_branch = self.resolve_block_as_expr(&if_expr.then_body);

        // Handle elsif + else chain by nesting
        let else_branch = if !if_expr.elsif_clauses.is_empty() {
            // Build nested if-else from elsif chain
            let mut else_expr = if_expr
                .else_body
                .as_ref()
                .map(|b| self.resolve_block_as_expr(b));

            for elsif in if_expr.elsif_clauses.iter().rev() {
                let elsif_cond = self.resolve_expr(&elsif.condition);
                let elsif_body = self.resolve_block_as_expr(&elsif.body);
                let ty = self.type_context.fresh_type_var();
                else_expr = Some(HirExpr {
                    kind: HirExprKind::If {
                        cond: Box::new(elsif_cond),
                        then_branch: Box::new(elsif_body),
                        else_branch: else_expr.map(Box::new),
                    },
                    ty,
                    span: elsif.span.clone(),
                });
            }
            else_expr
        } else {
            if_expr
                .else_body
                .as_ref()
                .map(|b| self.resolve_block_as_expr(b))
        };

        let ty = self.type_context.fresh_type_var();
        HirExpr {
            kind: HirExprKind::If {
                cond: Box::new(cond),
                then_branch: Box::new(then_branch),
                else_branch: else_branch.map(Box::new),
            },
            ty,
            span: if_expr.span.clone(),
        }
    }

    fn resolve_if_let(&mut self, if_let: &ast::IfLetExpr) -> HirExpr {
        let value = self.resolve_expr(&if_let.value);

        self.scopes.push(ScopeKind::Block);
        let pattern = self.resolve_pattern(&if_let.pattern);
        self.register_pattern_bindings(&if_let.pattern, false, &if_let.span);
        let then_body = self.resolve_block_as_expr(&if_let.then_body);
        self.scopes.pop();

        let else_body = if_let
            .else_body
            .as_ref()
            .map(|b| self.resolve_block_as_expr(b));

        // Desugar to match
        let wildcard_arm = HirMatchArm {
            pattern: HirPattern::Wildcard {
                span: if_let.span.clone(),
            },
            guard: None,
            body: Box::new(else_body.unwrap_or(HirExpr {
                kind: HirExprKind::UnitLiteral,
                ty: Ty::Unit,
                span: if_let.span.clone(),
            })),
            span: if_let.span.clone(),
        };

        let ty = self.type_context.fresh_type_var();
        HirExpr {
            kind: HirExprKind::Match {
                scrutinee: Box::new(value),
                arms: vec![
                    HirMatchArm {
                        pattern,
                        guard: None,
                        body: Box::new(then_body),
                        span: if_let.span.clone(),
                    },
                    wildcard_arm,
                ],
            },
            ty,
            span: if_let.span.clone(),
        }
    }

    // ─── Match Expression Resolution ────────────────────────────────

    fn resolve_match(&mut self, match_expr: &ast::MatchExpr) -> HirExpr {
        let scrutinee = self.resolve_expr(&match_expr.subject);

        let mut arms = Vec::new();
        for arm in &match_expr.arms {
            self.scopes.push(ScopeKind::Match);
            let pattern = self.resolve_pattern(&arm.pattern);
            self.register_pattern_bindings(&arm.pattern, false, &arm.span);
            let guard = arm.guard.as_ref().map(|g| Box::new(self.resolve_expr(g)));
            let body = match &arm.body {
                ast::MatchArmBody::Expr(e) => self.resolve_expr(e),
                ast::MatchArmBody::Block(b) => self.resolve_block_as_expr(b),
            };
            self.scopes.pop();
            arms.push(HirMatchArm {
                pattern,
                guard,
                body: Box::new(body),
                span: arm.span.clone(),
            });
        }

        let ty = self.type_context.fresh_type_var();
        HirExpr {
            kind: HirExprKind::Match {
                scrutinee: Box::new(scrutinee),
                arms,
            },
            ty,
            span: match_expr.span.clone(),
        }
    }

    // ─── Closure Resolution ─────────────────────────────────────────

    fn resolve_closure(&mut self, closure: &ast::ClosureExpr, span: &Span) -> HirExpr {
        let closure_scope_id = self.scopes.push(ScopeKind::Closure);
        self.closure_stack.push(ClosureCaptureContext {
            scope_id: closure_scope_id,
            is_move: closure.is_move,
            captures: Vec::new(),
        });

        let mut params = Vec::new();
        for p in &closure.params {
            let ty = p
                .type_expr
                .as_ref()
                .map(|t| self.resolve_type_expr(t))
                .unwrap_or_else(|| self.type_context.fresh_type_var());
            let def_id = self.symbols.define(
                p.name.clone(),
                DefKind::Param {
                    ty: ty.clone(),
                    auto_assign: false,
                },
                Visibility::Private,
                p.span.clone(),
            );
            self.scopes.insert(p.name.clone(), def_id);
            params.push(HirClosureParam {
                def_id,
                name: p.name.clone(),
                ty,
                span: p.span.clone(),
            });
        }

        let old_async_scope_depth = self.async_scope_depth;
        if closure.is_async {
            self.async_scope_depth += 1;
        }

        let body = match &closure.body {
            ast::ClosureBody::Expr(e) => self.resolve_expr(e),
            ast::ClosureBody::Block(b) => self.resolve_block_as_expr(b),
        };

        self.async_scope_depth = old_async_scope_depth;
        let captures = self
            .closure_stack
            .pop()
            .map(|ctx| ctx.captures)
            .unwrap_or_default();
        self.scopes.pop();

        let param_tys: Vec<Ty> = params.iter().map(|p| p.ty.clone()).collect();
        let ret_ty = if closure.is_async {
            Ty::Class {
                name: "Future".to_string(),
                generic_args: vec![body.ty.clone()],
            }
        } else {
            body.ty.clone()
        };
        let fn_ty = Ty::Fn {
            params: param_tys,
            ret: Box::new(ret_ty),
        };

        HirExpr {
            kind: HirExprKind::Closure {
                params,
                body: Box::new(body),
                captures,
                is_async: closure.is_async,
                is_move: closure.is_move,
            },
            ty: fn_ty,
            span: span.clone(),
        }
    }

    fn record_capture_if_needed(&mut self, def_id: DefId, def_scope_id: ScopeId) {
        let Some(closure) = self.closure_stack.last_mut() else {
            return;
        };
        if self.scopes.is_within_scope(def_scope_id, closure.scope_id) {
            return;
        }
        let Some(def) = self.symbols.get(def_id) else {
            return;
        };
        let should_capture = matches!(
            def.kind,
            DefKind::Variable { .. } | DefKind::Param { .. } | DefKind::SelfValue { .. }
        );
        if !should_capture || closure.captures.iter().any(|cap| cap.def_id == def_id) {
            return;
        }
        closure.captures.push(Capture {
            def_id,
            name: def.name.clone(),
            by_move: closure.is_move,
            ty: self.symbols.def_ty(def_id).unwrap_or(Ty::Error),
        });
    }

    // ─── Pattern Resolution ─────────────────────────────────────────

    fn resolve_pattern(&mut self, pattern: &ast::Pattern) -> HirPattern {
        self.resolve_pattern_with_type(pattern, &Ty::Error)
    }

    fn resolve_pattern_with_type(
        &mut self,
        pattern: &ast::Pattern,
        _expected_ty: &Ty,
    ) -> HirPattern {
        match pattern {
            ast::Pattern::Wildcard { span } => HirPattern::Wildcard { span: span.clone() },
            ast::Pattern::Identifier {
                mutable,
                name,
                span,
            } => {
                let ty = self.type_context.fresh_type_var();
                let def_id = self.symbols.define(
                    name.clone(),
                    DefKind::Variable {
                        mutable: *mutable,
                        ty,
                    },
                    Visibility::Private,
                    span.clone(),
                );
                // Register the binding in the current scope so that body
                // expressions (e.g. match arm bodies) resolve to the same
                // def_id.  `register_pattern_bindings` guards against
                // duplicates with an `is_none()` check.
                self.scopes.insert(name.clone(), def_id);
                HirPattern::Binding {
                    def_id,
                    name: name.clone(),
                    mutable: *mutable,
                    span: span.clone(),
                }
            }
            ast::Pattern::Literal { expr, span } => {
                let hir_expr = self.resolve_expr(expr);
                HirPattern::Literal {
                    expr: Box::new(hir_expr),
                    span: span.clone(),
                }
            }
            ast::Pattern::Tuple { elements, span } => {
                let elems: Vec<HirPattern> =
                    elements.iter().map(|e| self.resolve_pattern(e)).collect();
                HirPattern::Tuple {
                    elements: elems,
                    span: span.clone(),
                }
            }
            ast::Pattern::Enum {
                path,
                variant,
                fields,
                span,
            } => {
                let type_name = path.join(".");
                let composite = format!("{}.{}", type_name, variant);
                let variant_def = self.scopes.lookup(&composite).unwrap_or_else(|| {
                    self.error(format!("undefined enum variant `{}`", composite), span);
                    UNRESOLVED_DEF
                });

                let variant_idx = if variant_def != UNRESOLVED_DEF {
                    if let Some(def) = self.symbols.get(variant_def) {
                        if let DefKind::EnumVariant { variant_idx, .. } = &def.kind {
                            *variant_idx
                        } else {
                            0
                        }
                    } else {
                        0
                    }
                } else {
                    0
                };

                let type_def = self
                    .type_registry
                    .get(&type_name)
                    .copied()
                    .unwrap_or(UNRESOLVED_DEF);
                let fields_hir: Vec<HirPattern> =
                    fields.iter().map(|f| self.resolve_pattern(f)).collect();

                HirPattern::Enum {
                    type_def,
                    variant_idx,
                    variant_name: variant.clone(),
                    fields: fields_hir,
                    span: span.clone(),
                }
            }
            ast::Pattern::Struct {
                path,
                fields,
                rest,
                span,
            } => {
                let type_name = path.join(".");
                let type_def = self
                    .type_registry
                    .get(&type_name)
                    .copied()
                    .unwrap_or(UNRESOLVED_DEF);
                let fields_hir: Vec<(String, HirPattern)> = fields
                    .iter()
                    .map(|f| {
                        let name = f.name.clone().unwrap_or_default();
                        let pat = self.resolve_pattern(&f.pattern);
                        (name, pat)
                    })
                    .collect();
                HirPattern::Struct {
                    type_def,
                    fields: fields_hir,
                    rest: *rest,
                    span: span.clone(),
                }
            }
            ast::Pattern::Or { patterns, span } => {
                let pats: Vec<HirPattern> =
                    patterns.iter().map(|p| self.resolve_pattern(p)).collect();
                HirPattern::Or {
                    patterns: pats,
                    span: span.clone(),
                }
            }
            ast::Pattern::Ref {
                mutable,
                name,
                span,
            } => {
                let ty = self.type_context.fresh_type_var();
                let def_id = self.symbols.define(
                    name.clone(),
                    DefKind::Variable {
                        mutable: *mutable,
                        ty,
                    },
                    Visibility::Private,
                    span.clone(),
                );
                // Insert into scope so that VarRef lookups in the arm
                // body resolve to the same def_id as the pattern binding.
                self.scopes.insert(name.clone(), def_id);
                HirPattern::Ref {
                    mutable: *mutable,
                    name: name.clone(),
                    def_id,
                    span: span.clone(),
                }
            }
            ast::Pattern::Rest { span } => HirPattern::Rest { span: span.clone() },
        }
    }

    fn register_pattern_bindings(&mut self, pattern: &ast::Pattern, mutable: bool, span: &Span) {
        match pattern {
            ast::Pattern::Identifier { name, .. } => {
                // Already handled in resolve_pattern_with_type for let-bindings,
                // but for match/for patterns we need to register too
                if self.scopes.lookup(name).is_none() {
                    let ty = self.type_context.fresh_type_var();
                    let def_id = self.symbols.define(
                        name.clone(),
                        DefKind::Variable { mutable, ty },
                        Visibility::Private,
                        span.clone(),
                    );
                    self.scopes.insert(name.clone(), def_id);
                }
            }
            ast::Pattern::Tuple { elements, .. } => {
                for elem in elements {
                    self.register_pattern_bindings(elem, mutable, span);
                }
            }
            ast::Pattern::Enum { fields, .. } => {
                for field in fields {
                    self.register_pattern_bindings(field, mutable, span);
                }
            }
            ast::Pattern::Struct { fields, .. } => {
                for field in fields {
                    self.register_pattern_bindings(&field.pattern, mutable, span);
                }
            }
            ast::Pattern::Or { patterns, .. } => {
                // All alternatives must bind the same names
                if let Some(first) = patterns.first() {
                    self.register_pattern_bindings(first, mutable, span);
                }
            }
            ast::Pattern::Ref {
                name, mutable: m, ..
            } => {
                if self.scopes.lookup(name).is_none() {
                    let ty = self.type_context.fresh_type_var();
                    let def_id = self.symbols.define(
                        name.clone(),
                        DefKind::Variable { mutable: *m, ty },
                        Visibility::Private,
                        span.clone(),
                    );
                    self.scopes.insert(name.clone(), def_id);
                }
            }
            _ => {}
        }
    }

    // ─── Type Expression Resolution ─────────────────────────────────

    pub fn resolve_type_expr(&mut self, type_expr: &ast::TypeExpr) -> Ty {
        match type_expr {
            ast::TypeExpr::Named(path) => self.resolve_type_path(path),
            ast::TypeExpr::Reference {
                lifetime,
                mutable,
                inner,
                ..
            } => {
                let inner_ty = self.resolve_type_expr(inner);
                match (lifetime, mutable) {
                    (Some(lt), true) => Ty::RefMutLifetime(lt.clone(), Box::new(inner_ty)),
                    (Some(lt), false) => Ty::RefLifetime(lt.clone(), Box::new(inner_ty)),
                    (None, true) => Ty::RefMut(Box::new(inner_ty)),
                    (None, false) => Ty::Ref(Box::new(inner_ty)),
                }
            }
            ast::TypeExpr::Tuple { elements, .. } => {
                if elements.is_empty() {
                    Ty::Unit
                } else {
                    Ty::Tuple(elements.iter().map(|e| self.resolve_type_expr(e)).collect())
                }
            }
            ast::TypeExpr::Array { element, size, .. } => {
                let elem_ty = self.resolve_type_expr(element);
                if let Some(size_expr) = size {
                    // Fixed-size array [T; N].  T2.02 stage 4: the
                    // size is captured as a `ConstExpr` rather than
                    // a bare `usize`.  T2.02 stage 8: `+ - * /` and
                    // parens fold into `ConstExpr::Op` trees; S8.S4
                    // normalises identities (`N + 0 = N`, …) so
                    // `[T; N + 0]` and `[T; N]` produce the same
                    // `Ty`.  S8.S4 follow-up: pure-literal subtrees
                    // that overflow or divide by zero surface as
                    // E0703 here — they're invariant across
                    // instantiations.
                    let n = const_helpers::lower_const_expr_from_expr(size_expr).normal_form();
                    self.check_const_expr_for_non_const(&n, &size_expr.span);
                    self.check_const_expr_eval_errors(&n, &size_expr.span);
                    Ty::FixedArray(Box::new(elem_ty), n)
                } else {
                    // Slice [T] — treat as Vec for now
                    Ty::Array(Box::new(elem_ty))
                }
            }
            ast::TypeExpr::Function {
                params,
                return_type,
                ..
            } => Ty::Fn {
                params: params.iter().map(|p| self.resolve_type_expr(p)).collect(),
                ret: Box::new(self.resolve_type_expr(return_type)),
            },
            ast::TypeExpr::SomeMixin { bounds, .. } => Ty::SomeMixin(
                bounds
                    .iter()
                    .map(|b| MixinRef {
                        name: b.path.segments.join("."),
                        generic_args: b
                            .path
                            .generic_args
                            .as_ref()
                            .map(|args| args.iter().map(|a| self.resolve_type_expr(a)).collect())
                            .unwrap_or_default(),
                    })
                    .collect(),
            ),
            ast::TypeExpr::AnyMixin { bounds, .. } => Ty::AnyMixin(
                bounds
                    .iter()
                    .map(|b| MixinRef {
                        name: b.path.segments.join("."),
                        generic_args: b
                            .path
                            .generic_args
                            .as_ref()
                            .map(|args| args.iter().map(|a| self.resolve_type_expr(a)).collect())
                            .unwrap_or_default(),
                    })
                    .collect(),
            ),
            ast::TypeExpr::Never { .. } => Ty::Never,
            ast::TypeExpr::Inferred { .. } => self.type_context.fresh_type_var(),
            ast::TypeExpr::RawPointer { mutable, inner, .. } => {
                let inner_ty = self.resolve_type_expr(inner);
                // Check for *Void and *mut Void
                if matches!(&inner_ty, Ty::Struct { name, .. } | Ty::Class { name, .. } if name == "Void")
                    || matches!(&inner_ty, Ty::Error)
                        && matches!(inner.as_ref(), ast::TypeExpr::Named(p) if p.segments == ["Void"])
                {
                    if *mutable {
                        Ty::RawPtrMutVoid
                    } else {
                        Ty::RawPtrVoid
                    }
                } else if let ast::TypeExpr::Named(p) = inner.as_ref() {
                    if p.segments == ["Void"] {
                        if *mutable {
                            Ty::RawPtrMutVoid
                        } else {
                            Ty::RawPtrVoid
                        }
                    } else if *mutable {
                        Ty::RawPtrMut(Box::new(inner_ty))
                    } else {
                        Ty::RawPtr(Box::new(inner_ty))
                    }
                } else if *mutable {
                    Ty::RawPtrMut(Box::new(inner_ty))
                } else {
                    Ty::RawPtr(Box::new(inner_ty))
                }
            }
            // Stage 2 of const generics — parser only.  A ConstLit in
            // generic-arg position has no type-level meaning yet;
            // resolve currently treats it as `Ty::Error` so that any
            // accidental use against a type parameter degrades safely.
            // S3 will introduce DefKind::ConstParam and promote ConstLit
            // to a real `ConstExpr::Lit` against const params, then
            // emit E0704 against type params.
            // T2.02 S6: a `ConstLit` in a generic-arg position
            // becomes a `Ty::ConstArg(ConstExpr::Lit(v))` so distinct
            // const instantiations of a generic type produce
            // distinct Ty values.  The S5 kind-check (above the call
            // site) emits E0704 when this lands against a Type slot;
            // here we only build the value.
            ast::TypeExpr::ConstLit { value, .. } => {
                Ty::ConstArg(crate::hir::types::ConstExpr::Lit(*value as u64))
            }
            // T2.02 S8.S3: an arithmetic const expression in a
            // generic-arg position folds through the same
            // `lower_const_expr_from_expr` helper that S8.S2 uses
            // for `[T; expr]` array sizes.  The kind-check (above
            // the call site) also treats this as a const-arg slot.
            // S8.S4: rewrite to normal form so `Vector[T, N + 0]`
            // and `Vector[T, N]` produce the same `Ty::ConstArg`.
            // S8.S4 follow-up: surface pure-literal overflow /
            // div-zero as E0703 against the source span.
            ast::TypeExpr::ConstExprArg { expr, span } => {
                let folded = const_helpers::lower_const_expr_from_expr(expr).normal_form();
                self.check_const_expr_for_non_const(&folded, span);
                self.check_const_expr_eval_errors(&folded, span);
                Ty::ConstArg(folded)
            }
        }
    }

    fn resolve_type_path(&mut self, path: &ast::TypePath) -> Ty {
        // Handle `Self.AssocName` — an associated-type reference.
        // Inside an impl block where `type AssocName = X` is declared,
        // map to `X` directly; inside a trait body, map to an opaque
        // `TypeParam` placeholder bound by the enclosing trait.
        if path.segments.len() == 2 && path.segments[0] == "Self" {
            let assoc = &path.segments[1];
            if let Some(ty) = self.current_impl_assoc_types.get(assoc) {
                return ty.clone();
            }
            if let Some((trait_name, names)) = &self.current_trait_context {
                if names.iter().any(|n| n == assoc) {
                    return Ty::TypeParam {
                        name: format!("Self::{}", assoc),
                        bounds: vec![MixinRef {
                            name: trait_name.clone(),
                            generic_args: vec![],
                        }],
                    };
                }
            }
            // Fall through to the default error path with the joined name.
        }

        let name = path.segments.join(".");

        // Tier-2 const generics S5: kind-check each generic-arg slot
        // against the declared param kind on the target type before
        // running the generic-arg resolution loop.  A `ConstLit` at
        // a slot whose declared param is `Type` is E0700 (kind
        // mismatch).  We look the target up by name in the type
        // registry; built-in containers (Vec/HashMap/Set/etc.) have
        // no const-param slots, so any ConstLit against them is E0700.
        if let Some(ast_args) = path.generic_args.as_ref() {
            // Snapshot the declared param kinds + the declared const
            // type for each Const slot (None for Type slots).  Type slots
            // → kind-check (E0704); Const slots → value-type-fit check
            // (E0701).
            // Snapshot the declared `GenericParamKind` per slot.  The
            // `Const { ty }` variant carries the declared const-param
            // type, used downstream for the E0701 type-fit check.
            let declared_kinds: Option<Vec<GenericParamKind>> = self
                .type_registry
                .get(&name)
                .copied()
                .and_then(|id| self.symbols.get(id))
                .and_then(|def| match &def.kind {
                    DefKind::Class { info } => Some(
                        info.generic_params
                            .iter()
                            .map(|gp| gp.kind.clone())
                            .collect(),
                    ),
                    DefKind::Struct { info } => Some(
                        info.generic_params
                            .iter()
                            .map(|gp| gp.kind.clone())
                            .collect(),
                    ),
                    DefKind::Enum { info } => Some(
                        info.generic_params
                            .iter()
                            .map(|gp| gp.kind.clone())
                            .collect(),
                    ),
                    _ => None,
                });
            for (idx, arg) in ast_args.iter().enumerate() {
                let is_const_arg = matches!(
                    arg,
                    ast::TypeExpr::ConstLit { .. } | ast::TypeExpr::ConstExprArg { .. }
                );
                if !is_const_arg {
                    continue;
                }
                let declared_kind = declared_kinds
                    .as_ref()
                    .and_then(|ks| ks.get(idx).cloned())
                    .unwrap_or(GenericParamKind::Type);
                if matches!(declared_kind, GenericParamKind::Type) {
                    let arg_span = match arg {
                        ast::TypeExpr::ConstLit { span, .. } => span.clone(),
                        ast::TypeExpr::ConstExprArg { span, .. } => span.clone(),
                        _ => path.span.clone(),
                    };
                    let what = match arg {
                        ast::TypeExpr::ConstLit { .. } => "const literal",
                        ast::TypeExpr::ConstExprArg { .. } => "const expression",
                        _ => "const argument",
                    };
                    self.diagnostics.push(Diagnostic::error_with_code(
                        format!(
                            "expected a type at generic argument position {}, found {}",
                            idx + 1,
                            what
                        ),
                        arg_span,
                        // E0704 — kind mismatch on const-generic arg.  Previously
                        // shared E0700 with the iterator-`sum` validator; spec
                        // §"Error code reservations" was amended to fork them
                        // (iterator-sum keeps E0700; this is E0704).
                        "E0704",
                    ));
                } else if let GenericParamKind::Const { ty: declared_ty } = declared_kind {
                    // E0701 — wrong const-arg type.  Kind matches
                    // (const → const slot), but the literal value
                    // doesn't fit the declared type.  Today reachable
                    // when a Bool const-param is given an int literal
                    // other than 0 / 1; future overflows on tight
                    // unsigned widths would land here too once parser
                    // accepts negative literals or wider arithmetic.
                    let arg_value: Option<i64> = match arg {
                        ast::TypeExpr::ConstLit { value, .. } => Some(*value),
                        ast::TypeExpr::ConstExprArg { expr, .. } => {
                            // Fold via the same path used for
                            // resolution, then read the literal value
                            // if we have one after normalization.
                            let folded = const_helpers::lower_const_expr_from_expr(expr).normal_form();
                            folded.as_lit().map(|v| v as i64)
                        }
                        _ => None,
                    };
                    let arg_span = match arg {
                        ast::TypeExpr::ConstLit { span, .. } => span.clone(),
                        ast::TypeExpr::ConstExprArg { span, .. } => span.clone(),
                        _ => path.span.clone(),
                    };
                    if let Some(v) = arg_value {
                        let fits = match &declared_ty {
                            Ty::Bool => v == 0 || v == 1,
                            // Unsigned families: parser produces non-
                            // negative literals today, but be defensive.
                            Ty::USize
                            | Ty::UInt
                            | Ty::UInt8
                            | Ty::UInt16
                            | Ty::UInt32
                            | Ty::UInt64 => v >= 0,
                            // Signed / other integer families accept
                            // every i64 value the parser can produce.
                            _ => true,
                        };
                        if !fits {
                            self.diagnostics.push(Diagnostic::error_with_code(
                                format!(
                                    "const-generic argument `{}` does not fit declared type `{}`",
                                    v, declared_ty
                                ),
                                arg_span,
                                "E0701",
                            ));
                        }
                    }
                }
            }
        }

        let generic_args: Vec<Ty> = path
            .generic_args
            .as_ref()
            .map(|args| args.iter().map(|a| self.resolve_type_expr(a)).collect())
            .unwrap_or_default();

        // T2.02 S9 enforcement: at every instantiation site with const
        // args, walk the target type's where-clause const predicates
        // against the binding map.  Any predicate that evaluates to
        // false produces E0706.  Predicates that still reference
        // unresolved params (e.g. instantiating with a parent's const
        // param that hasn't been substituted yet) are skipped — they
        // re-evaluate at the outer instantiation.
        if let Some(class_def_id) = self.type_registry.get(&name).copied() {
            // Build the binding map for this instantiation.
            let predicates: Vec<crate::resolve::symbols::HirConstPredicate> = self
                .symbols
                .get(class_def_id)
                .map(|def| match &def.kind {
                    DefKind::Class { info } => info.const_predicates.clone(),
                    DefKind::Struct { info } => info.const_predicates.clone(),
                    DefKind::Enum { info } => info.const_predicates.clone(),
                    _ => vec![],
                })
                .unwrap_or_default();
            if !predicates.is_empty() {
                let declared_params: Vec<crate::resolve::symbols::GenericParamInfo> = self
                    .symbols
                    .get(class_def_id)
                    .map(|def| match &def.kind {
                        DefKind::Class { info } => info.generic_params.clone(),
                        DefKind::Struct { info } => info.generic_params.clone(),
                        DefKind::Enum { info } => info.generic_params.clone(),
                        _ => vec![],
                    })
                    .unwrap_or_default();
                let mut bindings = std::collections::HashMap::new();
                let empty_inner = std::collections::HashMap::new();
                for (param, arg) in declared_params.iter().zip(generic_args.iter()) {
                    if matches!(param.kind, GenericParamKind::Const { .. }) {
                        if let Ty::ConstArg(ce) = arg {
                            if let Ok(v) = ce.eval(&empty_inner) {
                                bindings.insert(param.name.clone(), v);
                            }
                        }
                    }
                }
                for pred in &predicates {
                    if let Some(false) = const_helpers::eval_const_predicate(pred, &bindings) {
                        self.diagnostics.push(Diagnostic::error_with_code(
                            format!(
                                "where-clause predicate is not satisfied at this instantiation of `{}`",
                                name
                            ),
                            pred.span.clone(),
                            "E0706",
                        ));
                    }
                }
            }
        }

        // Check built-in generic types
        match name.as_str() {
            // `Array[T]` was `Vec[T]` pre-Ruby-naming. The legacy spelling
            // is kept as an alias so older sources still resolve while
            // the new vocabulary settles.
            "Array" | "Vec" => {
                let elem = generic_args
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| self.type_context.fresh_type_var());
                return Ty::Array(Box::new(elem));
            }
            // `Map[K, V]` was `HashMap[K, V]` pre-Ruby-naming.
            "Map" | "HashMap" => {
                let mut iter = generic_args.into_iter();
                let k = iter
                    .next()
                    .unwrap_or_else(|| self.type_context.fresh_type_var());
                let v = iter
                    .next()
                    .unwrap_or_else(|| self.type_context.fresh_type_var());
                // K must be Hash + Eq. Reject compound containers
                // (Array/Set/Map) and aggregates that don't derive Hash.
                if !const_helpers::ty_is_valid_hash_key(&k, &self.symbols) {
                    self.diagnostics.push(Diagnostic::error_with_code(
                        format!(
                            "Map key type `{}` is not hashable: K must include Hashable + Eq",
                            k
                        ),
                        path.span.clone(),
                        "E0615",
                    ));
                }
                return Ty::Map(Box::new(k), Box::new(v));
            }
            "Set" | "HashSet" => {
                // `HashSet[T]` is the legacy spelling for `Set[T]`. Both
                // desugar to the same runtime representation; method
                // dispatch in `codegen::runtime::runtime_name` accepts
                // either prefix.
                let elem = generic_args
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| self.type_context.fresh_type_var());
                if !const_helpers::ty_is_valid_hash_key(&elem, &self.symbols) {
                    self.diagnostics.push(Diagnostic::error_with_code(
                        format!(
                            "Set element type `{}` is not hashable: T must include Hashable + Eq",
                            elem
                        ),
                        path.span.clone(),
                        "E0615",
                    ));
                }
                return Ty::Set(Box::new(elem));
            }
            "Option" => {
                let inner = generic_args
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| self.type_context.fresh_type_var());
                return Ty::Option(Box::new(inner));
            }
            "Result" => {
                let mut iter = generic_args.into_iter();
                let ok = iter
                    .next()
                    .unwrap_or_else(|| self.type_context.fresh_type_var());
                let err = iter
                    .next()
                    .unwrap_or_else(|| self.type_context.fresh_type_var());
                return Ty::Result(Box::new(ok), Box::new(err));
            }
            "Box" => {
                let inner = generic_args
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| self.type_context.fresh_type_var());
                return Ty::Class {
                    name: "Box".to_string(),
                    generic_args: vec![inner],
                };
            }
            "Fn" => {
                if let Some((ret, params)) = generic_args.split_last() {
                    return Ty::Fn {
                        params: params.to_vec(),
                        ret: Box::new(ret.clone()),
                    };
                }
            }
            "FnMut" => {
                if let Some((ret, params)) = generic_args.split_last() {
                    return Ty::FnMut {
                        params: params.to_vec(),
                        ret: Box::new(ret.clone()),
                    };
                }
            }
            "Block" => {
                // Block(&T) -> Bool is like Fn
                if let Some((ret, params)) = generic_args.split_last() {
                    return Ty::Fn {
                        params: params.to_vec(),
                        ret: Box::new(ret.clone()),
                    };
                }
            }
            _ => {}
        }

        // Look up in type registry
        if let Some(&def_id) = self.type_registry.get(&name) {
            if let Some(def) = self.symbols.get(def_id) {
                match &def.kind {
                    DefKind::TypeAlias { target } => return target.clone(),
                    DefKind::Class { .. } => {
                        return Ty::Class { name, generic_args };
                    }
                    DefKind::Struct { .. } => {
                        return Ty::Struct { name, generic_args };
                    }
                    DefKind::Enum { .. } => {
                        return Ty::Enum { name, generic_args };
                    }
                    DefKind::Trait { .. } => {
                        // A trait used as a type — impl Trait or type param
                        return Ty::TypeParam {
                            name,
                            bounds: vec![],
                        };
                    }
                    DefKind::TypeParam { bounds } => {
                        return Ty::TypeParam {
                            name,
                            bounds: bounds.clone(),
                        };
                    }
                    DefKind::Newtype { inner } => {
                        return Ty::Newtype {
                            name,
                            inner: Box::new(inner.clone()),
                        };
                    }
                    _ => {}
                }
            }
        }

        // Check if it's a generic type parameter or type alias in scope
        if let Some(def_id) = self.scopes.lookup_type(&name) {
            if let Some(def) = self.symbols.get(def_id) {
                match &def.kind {
                    DefKind::TypeParam { bounds } => {
                        return Ty::TypeParam {
                            name,
                            bounds: bounds.clone(),
                        };
                    }
                    DefKind::TypeAlias { target } => {
                        return target.clone();
                    }
                    _ => {}
                }
            }
        }

        // Special case: &str
        if name == "str" {
            return Ty::Str;
        }

        self.error(format!("undefined type `{}`", name), &path.span);
        Ty::Error
    }

    // ─── Helper Methods ─────────────────────────────────────────────

    /// Tier-2 const generics S5: walk the AST `GenericParams` and
    /// produce a kind-aware `Vec<GenericParamInfo>` suitable for
    /// storage on `ClassInfo` / `StructInfo` / `EnumInfo` / `FnSignature`.
    /// Each entry preserves the source-order position (so use-site
    /// generic-arg validation can pair the i'th arg with the i'th
    /// declared param).  Lifetimes are skipped (not stored on info).
    pub(crate) fn collect_generic_param_infos(
        &mut self,
        gp: &Option<ast::GenericParams>,
    ) -> Vec<GenericParamInfo> {
        let Some(gps) = gp.as_ref() else {
            return vec![];
        };
        gps.params
            .iter()
            .filter_map(|p| match p {
                ast::GenericParam::Type { name, bounds, .. } => {
                    let trait_refs: Vec<MixinRef> = bounds
                        .iter()
                        .map(|b| MixinRef {
                            name: b.path.segments.join("."),
                            generic_args: vec![],
                        })
                        .collect();
                    Some(GenericParamInfo::type_param(name.clone(), trait_refs))
                }
                ast::GenericParam::Const { name, ty, .. } => {
                    let resolved_ty = self.resolve_type_expr(ty);
                    Some(GenericParamInfo::const_param(name.clone(), resolved_ty))
                }
                ast::GenericParam::Lifetime { .. } => None,
            })
            .collect()
    }

    fn resolve_generic_params(&mut self, gp: &Option<ast::GenericParams>) -> Vec<HirGenericParam> {
        // Stage 3 of const generics: first pass registers every
        // `GenericParam::Const` as a `DefKind::ConstParam` in the
        // symbol table so future passes (S4 HIR ConstExpr, S5
        // typeck unification) can look the name up.  We do this in
        // a separate pre-pass because `filter_map` captures `&mut
        // self` and Rust's borrow checker dislikes the symbol-table
        // mutation happening inside the type-param iteration.
        if let Some(gps) = gp.as_ref() {
            for p in &gps.params {
                if let ast::GenericParam::Const { name, ty, span } = p {
                    let resolved_ty = self.resolve_type_expr(ty);
                    // T2.02 spec §B8 (E-CONST-BAD-TYPE → E0705):
                    // a const-generic parameter's declared type must be
                    // an integer family or `Bool`.  Float* is non-goal
                    // NG2 (NaN ≠ NaN breaks the Eq contract const
                    // generics share); String / class / Vec / tuple
                    // const generics are also non-goals (NG3).
                    if !const_helpers::is_valid_const_param_ty(&resolved_ty) {
                        self.diagnostics.push(Diagnostic::error_with_code(
                            format!(
                                "const-generic parameter `{}` must be an integer or `Bool`, found `{}`",
                                name, resolved_ty
                            ),
                            span.clone(),
                            "E0705",
                        ));
                    }
                    let _ = self.symbols.define(
                        name.clone(),
                        DefKind::ConstParam { ty: resolved_ty },
                        Visibility::Public,
                        span.clone(),
                    );
                }
            }
        }

        gp.as_ref()
            .map(|gps| {
                gps.params
                    .iter()
                    .filter_map(|p| {
                        match p {
                            ast::GenericParam::Type { name, bounds, span } => {
                                let trait_refs: Vec<MixinRef> = bounds
                                    .iter()
                                    .map(|b| MixinRef {
                                        name: b.path.segments.join("."),
                                        generic_args: b
                                            .path
                                            .generic_args
                                            .as_ref()
                                            .map(|args| {
                                                args.iter()
                                                    .map(|a| self.resolve_type_expr(a))
                                                    .collect()
                                            })
                                            .unwrap_or_default(),
                                    })
                                    .collect();
                                Some(HirGenericParam {
                                    name: name.clone(),
                                    bounds: trait_refs,
                                    span: span.clone(),
                                })
                            }
                            ast::GenericParam::Lifetime { .. } => {
                                // Lifetimes are tracked but not yet used in Phase 3
                                None
                            }
                            // Const params were registered in the
                            // pre-pass above and don't appear in the
                            // HirGenericParam list (which is for
                            // type params only).
                            ast::GenericParam::Const { .. } => None,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn resolve_params(&mut self, params: &[ast::Param]) -> Vec<HirParam> {
        params
            .iter()
            .map(|p| {
                let ty = self.resolve_type_expr(&p.type_expr);
                let def_id = self.symbols.define(
                    p.name.clone(),
                    DefKind::Param {
                        ty: ty.clone(),
                        auto_assign: p.auto_assign,
                    },
                    Visibility::Private,
                    p.span.clone(),
                );
                HirParam {
                    def_id,
                    name: p.name.clone(),
                    ty,
                    auto_assign: p.auto_assign,
                    span: p.span.clone(),
                }
            })
            .collect()
    }

    fn resolve_and_register_params(&mut self, params: &[ast::Param]) -> Vec<HirParam> {
        params
            .iter()
            .map(|p| {
                let ty = self.resolve_type_expr(&p.type_expr);
                let def_id = self.symbols.define(
                    p.name.clone(),
                    DefKind::Param {
                        ty: ty.clone(),
                        auto_assign: p.auto_assign,
                    },
                    Visibility::Private,
                    p.span.clone(),
                );
                self.scopes.insert(p.name.clone(), def_id);
                HirParam {
                    def_id,
                    name: p.name.clone(),
                    ty,
                    auto_assign: p.auto_assign,
                    span: p.span.clone(),
                }
            })
            .collect()
    }

    fn convert_self_mode(&self, mode: ast::SelfMode) -> HirSelfMode {
        match mode {
            ast::SelfMode::Immutable => HirSelfMode::Ref,
            ast::SelfMode::Mutable => HirSelfMode::RefMut,
            ast::SelfMode::Consuming => HirSelfMode::Consuming,
        }
    }

    fn int_literal_type(&self, suffix: Option<crate::lexer::token::NumericSuffix>) -> Ty {
        use crate::lexer::token::NumericSuffix;
        match suffix {
            None => Ty::Int,
            Some(NumericSuffix::I8) => Ty::Int8,
            Some(NumericSuffix::I16) => Ty::Int16,
            Some(NumericSuffix::I32) => Ty::Int32,
            Some(NumericSuffix::I64) => Ty::Int64,
            Some(NumericSuffix::U) => Ty::UInt,
            Some(NumericSuffix::U8) => Ty::UInt8,
            Some(NumericSuffix::U16) => Ty::UInt16,
            Some(NumericSuffix::U32) => Ty::UInt32,
            Some(NumericSuffix::U64) => Ty::UInt64,
            Some(NumericSuffix::ISize) => Ty::ISize,
            Some(NumericSuffix::USize) => Ty::USize,
            Some(NumericSuffix::F32) => Ty::Float32,
            Some(NumericSuffix::F64) => Ty::Float64,
        }
    }

    fn float_literal_type(&self, suffix: Option<crate::lexer::token::NumericSuffix>) -> Ty {
        use crate::lexer::token::NumericSuffix;
        match suffix {
            None => Ty::Float,
            Some(NumericSuffix::F32) => Ty::Float32,
            Some(NumericSuffix::F64) => Ty::Float64,
            _ => Ty::Float,
        }
    }

    fn pattern_binding_name(&self, pattern: &ast::Pattern) -> String {
        match pattern {
            ast::Pattern::Identifier { name, .. } => name.clone(),
            ast::Pattern::Tuple { .. } => "_tuple".to_string(),
            ast::Pattern::Ref { name, .. } => name.clone(),
            _ => "_".to_string(),
        }
    }

    fn resolve_interpolation_tokens(
        &mut self,
        tokens: &[crate::lexer::token::Token],
        span: &Span,
    ) -> HirExpr {
        // The lexer gives us pre-tokenized expression tokens from #{...}
        // We need to parse them as an expression.
        // Wrap in a function body so the parser can handle them.
        if tokens.is_empty() {
            return HirExpr {
                kind: HirExprKind::StringLiteral(String::new()),
                ty: Ty::String,
                span: span.clone(),
            };
        }

        // Build a synthetic token stream: def _interp_ \n <tokens> \n end
        use crate::lexer::token::{Token, TokenKind};
        let dummy_span = Span {
            start: 0,
            end: 0,
            line: 0,
            column: 0,
        };
        let mut wrapped_tokens = vec![
            Token {
                kind: TokenKind::Def,
                span: dummy_span.clone(),
            },
            Token {
                kind: TokenKind::Identifier("_interp_".to_string()),
                span: dummy_span.clone(),
            },
            Token {
                kind: TokenKind::Newline,
                span: dummy_span.clone(),
            },
        ];
        wrapped_tokens.extend(tokens.iter().cloned());
        wrapped_tokens.push(Token {
            kind: TokenKind::Newline,
            span: dummy_span.clone(),
        });
        wrapped_tokens.push(Token {
            kind: TokenKind::End,
            span: dummy_span.clone(),
        });
        wrapped_tokens.push(Token {
            kind: TokenKind::Newline,
            span: dummy_span.clone(),
        });
        wrapped_tokens.push(Token {
            kind: TokenKind::Eof,
            span: dummy_span.clone(),
        });

        let mut parser = crate::parser::Parser::new(wrapped_tokens);
        if let Ok(program) = parser.parse() {
            if let Some(ast::TopLevelItem::Function(f)) = program.items.first() {
                if let Some(ast::Statement::Expression(expr)) = f.body.statements.first() {
                    return self.resolve_expr(expr);
                }
            }
        }

        // Fallback: if we can't parse, try a simple identifier lookup
        // (handles the common `#{variable}` case)
        if tokens.len() == 1 {
            if let TokenKind::Identifier(ref name) = tokens[0].kind {
                if let Some(def_id) = self.scopes.lookup(name) {
                    let ty = self
                        .symbols
                        .def_ty(def_id)
                        .unwrap_or_else(|| self.type_context.fresh_type_var());
                    return HirExpr {
                        kind: HirExprKind::VarRef(def_id),
                        ty,
                        span: span.clone(),
                    };
                }
            }
        }

        HirExpr {
            kind: HirExprKind::Error,
            ty: Ty::String,
            span: span.clone(),
        }
    }

    fn error(&mut self, message: String, span: &Span) {
        self.diagnostics
            .push(Diagnostic::error(message, span.clone()));
    }

    /// T2.02 S8.S4 follow-up: surface pure-literal overflow /
    /// div-zero in a `ConstExpr` as **E0703**.  Called immediately
    /// after the S8.S4 normal-form pass at every const-arg /
    /// array-size resolve site; the normalisation collapses
    /// successful pure-literal `Op` nodes to `Lit`, so any `Op`
    /// that survives with literal children is by definition an
    /// eval failure and is invariant across instantiations.
    ///
    /// Param-bearing trees (`N + 1`, `M * 2`) surface
    /// `Err(Unresolved(name))` from `eval` — those are *deferred*
    /// to the monomorphization-side check (the spec's per-
    /// instantiation eval surfacing pass that's still pending).
    /// `Err(Malformed)` (parser recovery) is also skipped — the
    /// parser already emitted its own diagnostic upstream.
    fn check_const_expr_eval_errors(&mut self, expr: &crate::hir::types::ConstExpr, span: &Span) {
        use crate::hir::types::ConstEvalError;
        let bindings = std::collections::HashMap::new();
        match expr.eval(&bindings) {
            Ok(_) => {}
            Err(ConstEvalError::Unresolved(_)) | Err(ConstEvalError::Malformed) => {}
            Err(ConstEvalError::NotImplemented) => {}
            Err(ConstEvalError::Overflow) => {
                self.diagnostics.push(Diagnostic::error_with_code(
                    "const expression overflows during evaluation".to_string(),
                    span.clone(),
                    "E0703",
                ));
            }
            Err(ConstEvalError::DivisionByZero) => {
                self.diagnostics.push(Diagnostic::error_with_code(
                    "const expression divides by zero".to_string(),
                    span.clone(),
                    "E0703",
                ));
            }
        }
    }

    /// T2.02 §B8 (E-CONST-NONCONST → E0702): surface
    /// `ConstExpr::Error` nodes — the marker
    /// `lower_const_expr_from_expr` produces for AST shapes that
    /// aren't valid v1 const expressions (unsupported binary ops
    /// like `%` / `<` / `<<`, function calls, method calls, field
    /// access, runtime variable references, …).
    ///
    /// Walks the tree once.  At most one E0702 is emitted per
    /// resolve-site span — the first reachable `Error` triggers
    /// it; nested noise stays quiet so the user sees the source
    /// location, not a diagnostic for every leaf.
    fn check_const_expr_for_non_const(&mut self, expr: &crate::hir::types::ConstExpr, span: &Span) {
        if const_helpers::contains_const_expr_error(expr) {
            self.diagnostics.push(Diagnostic::error_with_code(
                "expression is not a valid const expression \
                 (v1 supports integer literals, in-scope const-param references, \
                 and `+ - * /` arithmetic over those)"
                    .to_string(),
                span.clone(),
                "E0702",
            ));
        }
    }
}

/// Returns true iff `ty` is a valid HashMap key / HashSet element type
/// (i.e. implements Hash + Eq). Phase 2 stdlib (#04 batch 3): reject
/// compound containers (`Vec`, `Set`/`HashSet`, `HashMap`) — they are
/// explicitly NOT Hash in v1, even when their element type is. Aggregate
/// types (struct/class/enum) must opt in via `#[derive(Hash)]`.
///
/// This mirrors the per-field validator in `derive::validate_per_field_traits`
/// for E0615 but is rooted at the *type-construction* site so that any
/// `HashMap[K, V]` / `HashSet[T]` whose K/T is non-Hash is caught at
/// resolve time, not just when a user tries to derive Hash on a field.
/// T2.02 S8: fold a parser `Expr` appearing in a `[T; <expr>]`
/// array-size slot into a HIR `ConstExpr`.
///
/// Accepted forms (per `docs/specs/types/const-generics.spec.md` §B8):
/// - `IntLiteral(v, _)` → `ConstExpr::Lit(v)`.
/// - `Identifier(name)` → `ConstExpr::Param(name)`.
/// - `BinaryOp { left, op: + | - | * | /, right }` → recurse on
///   both sides and build a `ConstExpr::Op`.  Non-arithmetic ops
///   (`%`, comparisons, `&&`, bit ops, shifts) fall through to
///   `ConstExpr::Error`; S9 may admit a wider set for where-clause
///   predicates.
/// - Anything else (calls, field access, ...) → `ConstExpr::Error`.
///
/// The returned `Error` propagates through `eval` as
/// `ConstEvalError::Malformed`; the call site decides whether to
/// downgrade to a layout-zero placeholder or emit a hard diagnostic.
/// T2.02 S9: lower a parser-level where-clause const predicate
/// (e.g. `N > 0`, `N + M == 8`) into a HIR `HirConstPredicate` —
/// `(lhs: ConstExpr, op: ConstPredOp, rhs: ConstExpr)`.
///
/// Recognised top-level shape: `BinaryOp { left, op: cmp, right }`
/// where `cmp` ∈ `{Eq, NotEq, Lt, Gt, LtEq, GtEq}`.  Both sides
/// lower via `lower_const_expr_from_expr` — so they can be literals,
/// in-scope const-param references, or `+ - * /` arithmetic.
///
/// Anything else (top-level non-comparison, etc.) lowers to a
/// sentinel: `0 == 1` which evaluates to false at every
/// instantiation, with the original span — so users see a clear
impl Default for Resolver {
    fn default() -> Self {
        Self::new()
    }
}

// Helper trait for Pattern to get span
trait PatternSpan {
    fn span(&self) -> &Span;
}

impl PatternSpan for ast::Pattern {
    fn span(&self) -> &Span {
        match self {
            ast::Pattern::Literal { span, .. }
            | ast::Pattern::Identifier { span, .. }
            | ast::Pattern::Wildcard { span }
            | ast::Pattern::Tuple { span, .. }
            | ast::Pattern::Enum { span, .. }
            | ast::Pattern::Struct { span, .. }
            | ast::Pattern::Or { span, .. }
            | ast::Pattern::Ref { span, .. }
            | ast::Pattern::Rest { span } => span,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DefKind, Resolver};
    use crate::hir::nodes::{HirExprKind, HirItem};
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    fn resolve_source(input: &str) -> super::ResolveResult {
        let mut lexer = Lexer::new(input);
        let tokens = lexer.tokenize().expect("lexer failed");
        let mut parser = Parser::new(tokens);
        let program = parser.parse().expect("parser failed");
        Resolver::new().resolve(&program)
    }

    #[test]
    fn register_builtins_includes_send_and_sync_traits() {
        let mut resolver = Resolver::new();
        resolver.register_builtins();

        let has_send = resolver
            .symbols
            .iter()
            .any(|def| def.name == "Send" && matches!(def.kind, DefKind::Trait { .. }));
        let has_sync = resolver
            .symbols
            .iter()
            .any(|def| def.name == "Sync" && matches!(def.kind, DefKind::Trait { .. }));

        assert!(has_send, "expected builtins to include Send");
        assert!(has_sync, "expected builtins to include Sync");
    }

    #[test]
    fn register_builtins_includes_async_core_symbols() {
        let mut resolver = Resolver::new();
        resolver.register_builtins();

        let future_trait = resolver
            .symbols
            .iter()
            .find(|def| def.name == "Future")
            .expect("expected Future trait");
        let DefKind::Trait { info } = &future_trait.kind else {
            panic!("expected Future to be a trait");
        };
        assert_eq!(info.required_methods, vec!["poll".to_string()]);
        assert_eq!(info.assoc_types, vec!["Output".to_string()]);

        let poll_enum = resolver
            .symbols
            .iter()
            .find(|def| def.name == "Poll")
            .expect("expected Poll enum");
        let DefKind::Enum { info } = &poll_enum.kind else {
            panic!("expected Poll to be an enum");
        };
        assert_eq!(info.variants.len(), 2, "expected Ready/Pending variants");

        assert!(
            resolver
                .symbols
                .iter()
                .any(|def| def.name == "Context" && matches!(def.kind, DefKind::Class { .. })),
            "expected Context builtin class"
        );
        assert!(
            resolver
                .symbols
                .iter()
                .any(|def| def.name == "Waker" && matches!(def.kind, DefKind::Class { .. })),
            "expected Waker builtin class"
        );
    }

    #[test]
    fn register_builtins_includes_concurrency_core_symbols() {
        let mut resolver = Resolver::new();
        resolver.register_builtins();

        for name in [
            "Thread",
            "JoinHandle",
            "ThreadId",
            "Mutex",
            "MutexGuard",
            "Arc",
            "PoisonError",
            "ThreadPanic",
        ] {
            assert!(
                resolver
                    .symbols
                    .iter()
                    .any(|def| def.name == name && matches!(def.kind, DefKind::Class { .. })),
                "expected {name} builtin class"
            );
        }

        let std_sync = resolver
            .symbols
            .iter()
            .find(|def| def.name == "sync")
            .expect("expected sync module");
        let DefKind::Module { items } = &std_sync.kind else {
            panic!("expected sync to be a module");
        };
        assert!(
            items.iter().any(|id| resolver
                .symbols
                .get(*id)
                .is_some_and(|def| def.name == "Thread")),
            "expected std.sync to expose Thread"
        );
        assert!(
            items.iter().any(|id| resolver
                .symbols
                .get(*id)
                .is_some_and(|def| def.name == "Mutex")),
            "expected std.sync to expose Mutex"
        );
        assert!(
            items.iter().any(|id| resolver
                .symbols
                .get(*id)
                .is_some_and(|def| def.name == "Arc")),
            "expected std.sync to expose Arc"
        );
    }

    #[test]
    fn await_outside_async_reports_resolver_error() {
        let result = resolve_source(
            "def main\n  fetch_user(42).await\nend\n\ndef fetch_user(id: Int) -> Int\n  id\nend",
        );

        assert!(
            result
                .diagnostics
                .iter()
                .any(|diag| diag.code.as_deref() == Some("E_await_outside_async")),
            "expected E_await_outside_async, got {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn await_inside_async_function_resolves_without_async_scope_error() {
        let result = resolve_source(
            "async def main\n  fetch_user(42).await\nend\n\ndef fetch_user(id: Int) -> Int\n  id\nend",
        );

        assert!(
            result
                .diagnostics
                .iter()
                .all(|diag| diag.code.as_deref() != Some("E_await_outside_async")),
            "unexpected async-scope diagnostic: {:?}",
            result.diagnostics
        );

        let HirItem::Function(func) = &result.program.items[0] else {
            panic!("expected top-level function");
        };
        assert!(func.is_async, "expected async flag on HIR function");
        match &func.body.kind {
            HirExprKind::Block(stmts, tail) => {
                let expr = match (stmts.first(), tail.as_deref()) {
                    (Some(crate::hir::nodes::HirStatement::Expr(expr)), _) => expr,
                    (_, Some(expr)) => expr,
                    other => panic!("expected await expression in block, got {:?}", other),
                };
                match &expr.kind {
                    HirExprKind::MethodCall { method_name, .. } => {
                        assert_eq!(method_name, "await");
                    }
                    other => panic!("expected await desugaring, got {:?}", other),
                }
            }
            other => panic!("expected block body, got {:?}", other),
        }
    }

    #[test]
    fn await_inside_async_closure_resolves_without_async_scope_error() {
        let result = resolve_source(
            "def main\n  let f = async do\n    fetch_user(42).await\n  end\nend\n\ndef fetch_user(id: Int) -> Int\n  id\nend",
        );

        assert!(
            result
                .diagnostics
                .iter()
                .all(|diag| diag.code.as_deref() != Some("E_await_outside_async")),
            "unexpected async-scope diagnostic: {:?}",
            result.diagnostics
        );

        let HirItem::Function(func) = &result.program.items[0] else {
            panic!("expected top-level function");
        };
        let HirExprKind::Block(stmts, _) = &func.body.kind else {
            panic!("expected block body");
        };
        let crate::hir::nodes::HirStatement::Let {
            value: Some(value), ..
        } = &stmts[0]
        else {
            panic!("expected let binding with closure value");
        };
        let HirExprKind::Closure { is_async, .. } = &value.kind else {
            panic!("expected closure expression");
        };
        assert!(*is_async, "expected async flag on HIR closure");
    }

    #[test]
    fn closure_records_capture_from_outer_scope() {
        let result = resolve_source("def main\n  let x = 42\n  let f = do |y|\n    x\n  end\nend");

        let HirItem::Function(func) = &result.program.items[0] else {
            panic!("expected top-level function");
        };
        let HirExprKind::Block(stmts, _) = &func.body.kind else {
            panic!("expected block body");
        };
        let crate::hir::nodes::HirStatement::Let {
            value: Some(value), ..
        } = &stmts[1]
        else {
            panic!("expected closure binding");
        };
        let HirExprKind::Closure {
            captures, is_move, ..
        } = &value.kind
        else {
            panic!("expected closure expression");
        };

        assert!(!*is_move, "expected plain closure");
        assert_eq!(captures.len(), 1, "expected one outer capture");
        assert_eq!(captures[0].name, "x");
        assert!(!captures[0].by_move, "expected non-move capture");
    }

    #[test]
    fn move_closure_marks_capture_as_by_move() {
        let result = resolve_source("def main\n  let x = 42\n  let f = move do\n    x\n  end\nend");

        let HirItem::Function(func) = &result.program.items[0] else {
            panic!("expected top-level function");
        };
        let HirExprKind::Block(stmts, _) = &func.body.kind else {
            panic!("expected block body");
        };
        let crate::hir::nodes::HirStatement::Let {
            value: Some(value), ..
        } = &stmts[1]
        else {
            panic!("expected closure binding");
        };
        let HirExprKind::Closure {
            captures, is_move, ..
        } = &value.kind
        else {
            panic!("expected closure expression");
        };

        assert!(*is_move, "expected move closure");
        assert_eq!(captures.len(), 1, "expected one outer capture");
        assert!(captures[0].by_move, "expected move capture");
    }
}
