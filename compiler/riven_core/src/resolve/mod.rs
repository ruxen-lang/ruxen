//! Name resolution pass for the Riven compiler.
//!
//! Walks the AST, resolves all identifiers to DefIds, registers definitions
//! in the symbol table, and produces a partially-lowered HIR. Type inference
//! variables are allocated for unresolved types; the type checker fills them in.

pub mod bootstrap;
mod const_helpers;
pub mod scope;
mod stdlib;
pub mod symbols;
mod yield_scan;

mod bootstrap_merge;
mod control_flow;
mod exprs;
mod ffi_registration;
mod funcs;
mod helpers;
mod items;
mod patterns;
mod types;

use std::collections::HashMap;

use crate::diagnostics::Diagnostic;
use crate::hir::context::TypeContext;
use crate::hir::nodes::*;
use crate::hir::types::Ty;
use crate::lexer::token::Span;
use crate::parser::ast;
use scope::{ScopeId, ScopeStack};
use symbols::*;

/// The result of name resolution: a partially-typed HIR plus symbol table.
pub struct ResolveResult {
    pub program: HirProgram,
    pub symbols: SymbolTable,
    pub type_context: TypeContext,
    pub diagnostics: Vec<Diagnostic>,
    /// Phase E.E of #06.95: surfaced to typeck so its `collect_impls`
    /// pass can register module-nested class methods under their
    /// QUALIFIED names (e.g. `BufReader.File` rather than the
    /// unqualified `File`). Without this, instance-method lookups on
    /// receivers typed `BufReader.File` fall through to the fresh-
    /// inference-var fallback (the `?T37_read_to_string` symptom).
    pub type_registry: HashMap<String, DefId>,
}

/// The name resolver walks the AST and produces HIR with resolved names.
pub struct Resolver {
    pub symbols: SymbolTable,
    pub scopes: ScopeStack,
    pub type_context: TypeContext,
    pub diagnostics: Vec<Diagnostic>,

    /// Maps type names to their DefIds for quick lookup during type resolution.
    pub(super) type_registry: HashMap<String, DefId>,

    /// The current `self` type (inside class/impl bodies).
    pub(super) current_self_ty: Option<Ty>,

    /// The current class DefId (for field/method resolution).
    pub(super) current_class_def: Option<DefId>,

    /// The current function's return type (for return statement checking).
    pub(super) current_return_ty: Option<Ty>,

    /// Associated-type bindings from the currently-resolving `impl` block:
    /// `Self.Item` → concrete Ty declared by `type Item = …`.
    pub(super) current_impl_assoc_types: HashMap<String, Ty>,

    /// The trait whose body we are currently resolving (if any). Used to
    /// recognise `Self.AssocName` inside trait method signatures and map it
    /// to a placeholder TypeParam bound by that trait.
    pub(super) current_trait_context: Option<(String, Vec<String>)>,

    /// Functions whose body contains `yield` — these take a synthetic
    /// `__block: Closure` trailing parameter.  Maps function name to the
    /// arity of the first observed `yield` (used to pre-shape the block's
    /// `Ty::Fn` parameter list so inference can unify with caller blocks).
    pub(super) yield_fns: HashMap<String, usize>,

    /// Nesting depth of async functions/closures currently being resolved.
    pub(super) async_scope_depth: usize,

    /// Active closure stack used to record free-variable captures.
    pub(super) closure_stack: Vec<ClosureCaptureContext>,

    /// #06.8 T0c: tracks enums declared with an in-body `layout tagged`
    /// directive during pass 1, keyed by name. On a second insertion
    /// with the same name (i.e. two `layout tagged` enums with the
    /// same identifier in the same module scope) the resolver emits
    /// **E0723** at the duplicate's span. Wave 1 implementation only
    /// tracks the flat top-level module scope; nested-scope semantics
    /// arrive with the broader module-system pass.
    pub(super) tagged_enums_in_scope: HashMap<String, Span>,

    /// #06.8 Phase 2: tracks every C-symbol declared by an FFI def.
    /// Maps `c_symbol → (signature, declaration_span)`. When a second
    /// FFI def declares the same C symbol with a non-matching
    /// signature (param types, return type, or arity differ), the
    /// resolver emits **E0722** at the duplicate's span. Two decls
    /// with matching signatures are silently allowed — they're a
    /// no-op redundancy, not a conflict. The Riven-side name is
    /// independent of this table; only the C-symbol is the key.
    pub(super) extern_symbol_table: HashMap<String, (FnSignature, Span)>,

    /// #06.8 Phase 3b: DefIds of class-body `lib` FFI methods registered
    /// in pass-1, keyed by the parent class's `DefId`. Pass-2's
    /// `resolve_class` reads this map and appends these DefIds to the
    /// final `ClassInfo.methods` list, so `File.open(...)` resolves to
    /// the lib-declared method alongside any in-body `def`s.
    pub(super) pass1_class_lib_methods: HashMap<DefId, Vec<DefId>>,

    /// #06.95 Phase A pre-flight: snapshot of every mixin's
    /// `lib_decls` keyed by mixin name. Populated by
    /// [`collect_mixin_lib_decls`](Self::collect_mixin_lib_decls)
    /// BEFORE Pass 1's per-item registration loop runs.
    ///
    /// Used by the `Class` arm of `register_top_level_type_with_ffi`
    /// to re-register a mixin's FFI lib functions under the
    /// INCLUDING CLASS's name. Without this, `class Thing; include
    /// Adder; end` resolves `Thing.add_one(...)` at typeck (the
    /// mixin's method surface is visible through include
    /// propagation) but MIR's `ffi_alias_map` only carries
    /// `Adder_add_one → <c-symbol>`. The call site builds
    /// `Thing_add_one` from the receiver's class, the lookup
    /// misses, and the linker fails with "undefined symbol
    /// _Thing_add_one". Re-registering under `class.name` adds
    /// the parallel `Thing_add_one → <c-symbol>` entry.
    pub(super) mixin_lib_decls: HashMap<String, Vec<ast::LibDecl>>,

    /// Phase D of #06.95: when a package-aware bootstrap (via
    /// [`resolve_with_bootstrap_packages`](Self::resolve_with_bootstrap_packages))
    /// is driving the resolution, this carries `(pkg_name,
    /// item_names_declared)` pairs. The post-merge fixup walks this
    /// list and appends each item's DefId to the matching
    /// `std.<pkg>` submodule's `items` list, replacing the
    /// hand-maintained `FIXUPS` table for bulk per-package
    /// population.
    pub(super) bootstrap_auto_packages: Vec<(String, Vec<String>)>,

    /// Per-package snapshot of each declared item's resolved DefId,
    /// captured at the moment that package's bootstrap program
    /// finished its first-walk registration. Keyed by package name
    /// (e.g. `"time"`, `"sync"`); inner map is `item_name → DefId`.
    ///
    /// The fallback path is `scopes.lookup(name) or
    /// scopes.lookup_type(name)`, but that path is last-wins across
    /// the entire bootstrap merge — when two packages declare the
    /// same name (e.g. `def sleep` in both `library/std/time/src/lib.rvn`
    /// and `library/std/sync/src/lib.rvn`'s back-compat shim), the
    /// last-loaded package wins the lookup. The `std.<pkg>` submodule
    /// items list MUST contain the DefId of the item declared in
    /// `library/std/<pkg>/src/lib.rvn`, not whoever-came-last — otherwise
    /// `use std.time.sleep` resolves to a different function than the
    /// one declared in `time/lib.rvn`, and the signature mismatch
    /// silently propagates to typeck (manifests as `time.sleep`
    /// returning `()` instead of `TimeSleepFuture`).
    ///
    /// Snapshot is taken in [`merge_bootstrap_programs`](Self::merge_bootstrap_programs)
    /// after each program's first walk and consumed in
    /// [`auto_populate_std_submodules_from_packages`](Self::auto_populate_std_submodules_from_packages).
    pub(super) bootstrap_package_item_ids: HashMap<String, HashMap<String, DefId>>,
}

#[derive(Debug)]
pub(super) struct ClosureCaptureContext {
    pub(super) scope_id: ScopeId,
    pub(super) is_move: bool,
    pub(super) captures: Vec<Capture>,
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
            mixin_lib_decls: HashMap::new(),
            bootstrap_auto_packages: Vec::new(),
            bootstrap_package_item_ids: HashMap::new(),
        }
    }

    /// #06.95 Phase A pre-flight: walk every top-level item in
    /// `programs` and snapshot each mixin's `lib_decls` into
    /// `self.mixin_lib_decls`. Walks `Module` items recursively so
    /// mixins declared inside a `module Foo ... end` are picked up.
    ///
    /// MUST run BEFORE Pass 1's registration loop so the `Class` arm
    /// can re-register a mixin's lib decls under any class that
    /// `include`s it, regardless of source-order between class and
    /// mixin.
    pub(super) fn collect_mixin_lib_decls<'a, I>(&mut self, programs: I)
    where
        I: IntoIterator<Item = &'a ast::Program>,
    {
        for program in programs {
            Self::walk_items_for_mixins(&program.items, &[], &mut self.mixin_lib_decls);
        }
    }

    /// #06.93 Phase 5 update: mixins declared inside a `module`
    /// register under their QUALIFIED name (e.g. `BufReader.Reader`)
    /// so an `include` directive can disambiguate between two
    /// mixins of the same un-qualified name in different modules.
    /// Top-level mixins keep their un-qualified key.
    fn walk_items_for_mixins(
        items: &[ast::TopLevelItem],
        module_path: &[String],
        out: &mut HashMap<String, Vec<ast::LibDecl>>,
    ) {
        for item in items {
            match item {
                ast::TopLevelItem::Mixin(m) => {
                    if !m.lib_decls.is_empty() {
                        let key = if module_path.is_empty() {
                            m.name.clone()
                        } else {
                            format!("{}.{}", module_path.join("."), m.name)
                        };
                        out.entry(key)
                            .or_default()
                            .extend(m.lib_decls.iter().cloned());
                    }
                }
                ast::TopLevelItem::Module(md) => {
                    let mut nested = module_path.to_vec();
                    nested.push(md.name.clone());
                    Self::walk_items_for_mixins(&md.items, &nested, out);
                }
                _ => {}
            }
        }
    }
}

impl Default for Resolver {
    fn default() -> Self {
        Self::new()
    }
}

// Helper trait for Pattern to get span
pub(super) trait PatternSpan {
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

    /// #06.8 Wave 2 helper: build a `Resolver` whose symbol table is
    /// populated by BOTH `register_builtins` (Rust-side) AND the
    /// bootstrap merge (self-hosted `library/std/<pkg>/src/lib.rvn`). Tests
    /// that inspect the registered class / enum / trait surface
    /// need both halves because the migration moved several
    /// previously-Rust-registered names (Context, Waker, Thread,
    /// JoinHandle, Mutex, MutexGuard, Arc, PoisonError, ThreadPanic,
    /// ...) into .rvn class shells.
    fn resolver_with_bootstrap_for_tests() -> Resolver {
        let mut resolver = Resolver::new();
        resolver.register_builtins();
        let mut diags: Vec<crate::diagnostics::Diagnostic> = Vec::new();
        let programs = crate::resolve::bootstrap::run_bootstrap(&mut diags);
        assert!(diags.is_empty(), "bootstrap parse errors: {:?}", diags);
        let mut ffi_libs = Vec::new();
        resolver.merge_bootstrap_programs(&programs, &mut ffi_libs);
        // `fixup_bootstrapped_stdlib_modules` is what re-populates
        // each stdlib submodule's `items` list with the
        // bootstrap-loaded DefIds. Without it, the `std.sync` /
        // `std.io` / etc. `DefKind::Module` items vectors are empty.
        resolver.fixup_bootstrapped_stdlib_modules();
        resolver
    }

    #[test]
    fn register_builtins_includes_async_core_symbols() {
        let resolver = resolver_with_bootstrap_for_tests();

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

        // Context / Waker migrated to library/std/sync/src/lib.rvn in
        // Wave 2; the bootstrap merge above loads them.
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
        let resolver = resolver_with_bootstrap_for_tests();

        // Thread / JoinHandle / Mutex / MutexGuard / PoisonError /
        // ThreadPanic migrated to library/std/sync/src/lib.rvn in Wave 2.
        // `Arc` remains a Rust-side `DefKind::Class` shim — see
        // resolve/stdlib/mod.rs `arc_alias_id` (it's the backward-compat
        // alias that types as `Ty::Class { name: "SharedSync" }` so
        // `Arc.new(...)` returns a SharedSync-typed value). `ThreadId`
        // ALSO exists in sync.rvn as a class; the Rust shim is a
        // `DefKind::Variable` for the value-scope sentinel — the
        // symbol table thus carries TWO `ThreadId` entries (one Class
        // from sync.rvn, one Variable from the Rust shim), so this
        // test's `any(name == "ThreadId" && Class)` predicate still
        // matches via the .rvn-loaded one.
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
                .any(|diag| diag.code.as_deref() == Some("E1110")),
            "expected E1110, got {:?}",
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
                .all(|diag| diag.code.as_deref() != Some("E1110")),
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
                .all(|diag| diag.code.as_deref() != Some("E1110")),
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
