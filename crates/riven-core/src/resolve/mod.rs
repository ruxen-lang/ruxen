//! Name resolution pass for the Riven compiler.
//!
//! Walks the AST, resolves all identifiers to DefIds, registers definitions
//! in the symbol table, and produces a partially-lowered HIR. Type inference
//! variables are allocated for unresolved types; the type checker fills them in.

pub mod scope;
pub mod symbols;

use std::collections::HashMap;

use crate::diagnostics::Diagnostic;
use crate::hir::context::TypeContext;
use crate::hir::nodes::*;
use crate::hir::types::{MoveSemantics, TraitRef, Ty};
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
        }
    }

    /// Run name resolution on a parsed program.
    pub fn resolve(mut self, program: &ast::Program) -> ResolveResult {
        self.register_builtins();

        // Two-pass approach:
        // Pass 1: Register all top-level type names (classes, structs, enums, traits)
        //         so that forward references work.
        for item in &program.items {
            self.register_top_level_type(item);
        }

        // Scan for functions that contain `yield` — these receive a
        // synthetic `__block` parameter, and callers with a trailing block
        // forward it as the last argument.
        for item in &program.items {
            collect_yield_fns(item, &mut self.yield_fns);
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
        };

        ResolveResult {
            program: hir_program,
            symbols: self.symbols,
            type_context: self.type_context,
            diagnostics: self.diagnostics,
        }
    }

    // ─── Builtin Registration ───────────────────────────────────────

    fn register_builtins(&mut self) {
        // Register built-in types so they can be referenced by name.
        let builtins = [
            ("Int", Ty::Int),
            ("Int8", Ty::Int8),
            ("Int16", Ty::Int16),
            ("Int32", Ty::Int32),
            ("Int64", Ty::Int64),
            ("UInt", Ty::UInt),
            ("UInt8", Ty::UInt8),
            ("UInt16", Ty::UInt16),
            ("UInt32", Ty::UInt32),
            ("UInt64", Ty::UInt64),
            ("ISize", Ty::ISize),
            ("USize", Ty::USize),
            ("Float", Ty::Float),
            ("Float32", Ty::Float32),
            ("Float64", Ty::Float64),
            ("Bool", Ty::Bool),
            ("Char", Ty::Char),
            ("String", Ty::String),
        ];

        let span = Span {
            start: 0,
            end: 0,
            line: 0,
            column: 0,
        };

        for (name, ty) in builtins {
            let id = self.symbols.define(
                name.to_string(),
                DefKind::TypeAlias { target: ty },
                Visibility::Public,
                span.clone(),
            );
            self.scopes.insert_type(name.to_string(), id);
            self.type_registry.insert(name.to_string(), id);
        }

        // Register built-in traits: Displayable, Error, Serializable, etc.
        // Per TEC-13, the trait formerly known as `Hash` is `Hashable`.
        // `Hash` remains a deprecated alias for one transition release —
        // see the alias-registration block below.
        let builtin_traits = [
            ("Displayable", vec!["to_display"]),
            ("Error", vec!["message"]),
            ("Comparable", vec!["compare"]),
            ("Hashable", vec!["hash_code"]),
            ("Iterable", vec![]),
            ("Iterator", vec!["next"]),
            ("FromIterator", vec!["from_iter"]),
            ("Copy", vec![]),
            ("Clone", vec!["clone"]),
            ("Send", vec![]),
            ("Sync", vec![]),
            // Phase 2 #06.A1: `Display` and `Debug` formal traits.
            // Both share the `fmt(&self, &mut Formatter) -> Result[(), FmtError]`
            // signature. `Display` is the canonical interpolation
            // trait (Phase D will route `"#{x}"` through
            // `Display::fmt`); `Debug` is the `"#{x:?}"` (and existing
            // `derive Debug` synthesis) target. Required-methods list
            // is `["fmt"]` for both — typeck checks user `impl
            // Display for T` / `impl Debug for T` provides `fmt`.
            ("Display", vec!["fmt"]),
            ("Debug", vec!["fmt"]),
            ("PartialEq", vec!["eq"]),
            ("Eq", vec![]),
            ("Hash", vec!["hash"]),
            ("Default", vec!["default"]),
            ("Ord", vec!["cmp"]),
            ("PartialOrd", vec!["partial_cmp"]),
            ("Drop", vec!["drop"]),
        ];

        let mut hashable_id: Option<DefId> = None;
        for (name, methods) in builtin_traits {
            let id = self.symbols.define(
                name.to_string(),
                DefKind::Trait {
                    info: TraitInfo {
                        generic_params: vec![],
                        super_traits: vec![],
                        required_methods: methods.iter().map(|m| m.to_string()).collect(),
                        default_methods: vec![],
                        assoc_types: vec![],
                    },
                },
                Visibility::Public,
                span.clone(),
            );
            self.scopes.insert_type(name.to_string(), id);
            self.type_registry.insert(name.to_string(), id);
            if name == "Hashable" {
                hashable_id = Some(id);
            }
        }

        let future_trait_id = self.symbols.define(
            "Future".to_string(),
            DefKind::Trait {
                info: TraitInfo {
                    generic_params: vec![],
                    super_traits: vec![],
                    required_methods: vec!["poll".to_string()],
                    default_methods: vec![],
                    assoc_types: vec!["Output".to_string()],
                },
            },
            Visibility::Public,
            span.clone(),
        );
        self.scopes
            .insert_type("Future".to_string(), future_trait_id);
        self.type_registry
            .insert("Future".to_string(), future_trait_id);

        // Deprecated alias: `Hash` (the trait) → `Hashable`.
        // The collection type `Hash[K,V]` has its own resolution path
        // (see `resolve_type_path` / the `Hash`/`Vec`/`Set` match in
        // type-position) and is unaffected by this alias.
        if let Some(id) = hashable_id {
            self.scopes.insert_type("Hash".to_string(), id);
            self.type_registry.insert("Hash".to_string(), id);
        }

        // Register built-in functions. `IoError` is registered below
        // as `DefKind::Enum`, so the type reference here uses
        // `Ty::Enum` to keep the symbol-table kind and the type kind
        // in sync. Mismatching the two (Ty::Class vs DefKind::Enum)
        // breaks `enum_with_derive_trait` lookups and the codegen
        // enum-tag dispatch.
        let io_error_ty = Ty::Enum {
            name: "IoError".to_string(),
            generic_args: vec![],
        };
        let stdin_ty = Ty::Class {
            name: "Stdin".to_string(),
            generic_args: vec![],
        };
        let stdout_ty = Ty::Class {
            name: "Stdout".to_string(),
            generic_args: vec![],
        };
        let stderr_ty = Ty::Class {
            name: "Stderr".to_string(),
            generic_args: vec![],
        };
        let env_var_error_ty = io_error_ty.clone();

        let builtin_fns = [
            (
                "puts",
                vec![ParamInfo {
                    name: "value".into(),
                    ty: Ty::Ref(Box::new(Ty::String)),
                    auto_assign: false,
                }],
                Ty::Unit,
            ),
            (
                "eputs",
                vec![ParamInfo {
                    name: "value".into(),
                    ty: Ty::Ref(Box::new(Ty::String)),
                    auto_assign: false,
                }],
                Ty::Unit,
            ),
            (
                "print",
                vec![ParamInfo {
                    name: "value".into(),
                    ty: Ty::Ref(Box::new(Ty::String)),
                    auto_assign: false,
                }],
                Ty::Unit,
            ),
            (
                "println",
                vec![ParamInfo {
                    name: "value".into(),
                    ty: Ty::Ref(Box::new(Ty::String)),
                    auto_assign: false,
                }],
                Ty::Unit,
            ),
            (
                "eprintln",
                vec![ParamInfo {
                    name: "value".into(),
                    ty: Ty::Ref(Box::new(Ty::String)),
                    auto_assign: false,
                }],
                Ty::Unit,
            ),
            (
                "read_line",
                vec![],
                Ty::Result(Box::new(Ty::String), Box::new(io_error_ty.clone())),
            ),
            ("stdin", vec![], stdin_ty.clone()),
            ("stdout", vec![], stdout_ty.clone()),
            ("stderr", vec![], stderr_ty.clone()),
            ("args", vec![], Ty::Vec(Box::new(Ty::String))),
            (
                "var",
                vec![ParamInfo {
                    name: "name".into(),
                    ty: Ty::Ref(Box::new(Ty::String)),
                    auto_assign: false,
                }],
                Ty::Result(Box::new(Ty::String), Box::new(env_var_error_ty)),
            ),
            // Phase 2 stdlib (#06): env::vars / env::current_dir.
            // `vars` snapshots the process environment; mutations to
            // it after the call do not propagate to the returned map.
            (
                "vars",
                vec![],
                Ty::HashMap(Box::new(Ty::String), Box::new(Ty::String)),
            ),
            (
                "current_dir",
                vec![],
                Ty::Result(Box::new(Ty::String), Box::new(io_error_ty.clone())),
            ),
            (
                "read_to_string",
                vec![ParamInfo {
                    name: "path".into(),
                    ty: Ty::Ref(Box::new(Ty::String)),
                    auto_assign: false,
                }],
                Ty::Result(Box::new(Ty::String), Box::new(io_error_ty.clone())),
            ),
            (
                "write",
                vec![
                    ParamInfo {
                        name: "path".into(),
                        ty: Ty::Ref(Box::new(Ty::String)),
                        auto_assign: false,
                    },
                    ParamInfo {
                        name: "contents".into(),
                        ty: Ty::Ref(Box::new(Ty::String)),
                        auto_assign: false,
                    },
                ],
                Ty::Result(Box::new(Ty::Unit), Box::new(io_error_ty.clone())),
            ),
            (
                "exists",
                vec![ParamInfo {
                    name: "path".into(),
                    ty: Ty::Ref(Box::new(Ty::String)),
                    auto_assign: false,
                }],
                Ty::Bool,
            ),
            // Phase 2 stdlib (#06): fs::is_file / fs::is_dir / fs::read_dir.
            // is_file / is_dir mirror exists' Bool-on-error convention so
            // they slot into `if` predicates without `?`. read_dir wraps
            // the entry list in Result so the IO error is surfaced.
            (
                "is_file",
                vec![ParamInfo {
                    name: "path".into(),
                    ty: Ty::Ref(Box::new(Ty::String)),
                    auto_assign: false,
                }],
                Ty::Bool,
            ),
            (
                "is_dir",
                vec![ParamInfo {
                    name: "path".into(),
                    ty: Ty::Ref(Box::new(Ty::String)),
                    auto_assign: false,
                }],
                Ty::Bool,
            ),
            (
                "read_dir",
                vec![ParamInfo {
                    name: "path".into(),
                    ty: Ty::Ref(Box::new(Ty::String)),
                    auto_assign: false,
                }],
                Ty::Result(
                    Box::new(Ty::Vec(Box::new(Ty::String))),
                    Box::new(io_error_ty.clone()),
                ),
            ),
            (
                "remove_file",
                vec![ParamInfo {
                    name: "path".into(),
                    ty: Ty::Ref(Box::new(Ty::String)),
                    auto_assign: false,
                }],
                Ty::Result(Box::new(Ty::Unit), Box::new(io_error_ty.clone())),
            ),
            (
                "create_dir",
                vec![ParamInfo {
                    name: "path".into(),
                    ty: Ty::Ref(Box::new(Ty::String)),
                    auto_assign: false,
                }],
                Ty::Result(Box::new(Ty::Unit), Box::new(io_error_ty.clone())),
            ),
            (
                "create_dir_all",
                vec![ParamInfo {
                    name: "path".into(),
                    ty: Ty::Ref(Box::new(Ty::String)),
                    auto_assign: false,
                }],
                Ty::Result(Box::new(Ty::Unit), Box::new(io_error_ty.clone())),
            ),
            (
                "rename",
                vec![
                    ParamInfo {
                        name: "from".into(),
                        ty: Ty::Ref(Box::new(Ty::String)),
                        auto_assign: false,
                    },
                    ParamInfo {
                        name: "to".into(),
                        ty: Ty::Ref(Box::new(Ty::String)),
                        auto_assign: false,
                    },
                ],
                Ty::Result(Box::new(Ty::Unit), Box::new(io_error_ty.clone())),
            ),
            (
                "exit",
                vec![ParamInfo {
                    name: "code".into(),
                    ty: Ty::Int,
                    auto_assign: false,
                }],
                Ty::Never,
            ),
            // std::time — Phase 3. Both clocks return nanoseconds in Int.
            // `now_ns` is monotonic (only differences are meaningful);
            // `unix_ns` is wall-clock (nanoseconds since 1970-01-01 UTC).
            ("now_ns", vec![], Ty::Int),
            ("unix_ns", vec![], Ty::Int),
            // std::path — Phase 3. Unix-style separators only. The
            // path_ prefix avoids collision with the `join` method on
            // Vec[String]; Riven-side wrappers can rename later.
            // parent/file_name/extension return "" when the value is
            // absent (no Option[String] tagged-union runtime helper for
            // heap payloads yet — promote when one lands).
            (
                "path_join",
                vec![
                    ParamInfo {
                        name: "a".into(),
                        ty: Ty::Ref(Box::new(Ty::String)),
                        auto_assign: false,
                    },
                    ParamInfo {
                        name: "b".into(),
                        ty: Ty::Ref(Box::new(Ty::String)),
                        auto_assign: false,
                    },
                ],
                Ty::String,
            ),
            (
                "path_parent",
                vec![ParamInfo {
                    name: "path".into(),
                    ty: Ty::Ref(Box::new(Ty::String)),
                    auto_assign: false,
                }],
                Ty::String,
            ),
            (
                "path_file_name",
                vec![ParamInfo {
                    name: "path".into(),
                    ty: Ty::Ref(Box::new(Ty::String)),
                    auto_assign: false,
                }],
                Ty::String,
            ),
            (
                "path_extension",
                vec![ParamInfo {
                    name: "path".into(),
                    ty: Ty::Ref(Box::new(Ty::String)),
                    auto_assign: false,
                }],
                Ty::String,
            ),
            (
                "path_is_absolute",
                vec![ParamInfo {
                    name: "path".into(),
                    ty: Ty::Ref(Box::new(Ty::String)),
                    auto_assign: false,
                }],
                Ty::Bool,
            ),
            // std::process::run — Phase 3. Fork+execvp a child process,
            // inheriting stdin/stdout/stderr from the parent, and return
            // the exit code. Output capture is a follow-up; for v1 the
            // return type is just Int (128+signal on signal termination,
            // 127 on fork/exec failure — matches POSIX shell convention).
            (
                "process_run",
                vec![
                    ParamInfo {
                        name: "cmd".into(),
                        ty: Ty::Ref(Box::new(Ty::String)),
                        auto_assign: false,
                    },
                    ParamInfo {
                        name: "args".into(),
                        ty: Ty::Vec(Box::new(Ty::String)),
                        auto_assign: false,
                    },
                ],
                Ty::Int,
            ),
            // std::net — Phase 3. Minimum-viable TCP. fds surfaced as
            // Int with -1 on failure (POSIX-style). Class wrappers
            // (TcpStream/TcpListener) are a follow-up.
            (
                "tcp_connect",
                vec![ParamInfo {
                    name: "addr".into(),
                    ty: Ty::Ref(Box::new(Ty::String)),
                    auto_assign: false,
                }],
                Ty::Int,
            ),
            (
                "tcp_listen",
                vec![ParamInfo {
                    name: "addr".into(),
                    ty: Ty::Ref(Box::new(Ty::String)),
                    auto_assign: false,
                }],
                Ty::Int,
            ),
            (
                "tcp_accept",
                vec![ParamInfo {
                    name: "fd".into(),
                    ty: Ty::Int,
                    auto_assign: false,
                }],
                Ty::Int,
            ),
            (
                "tcp_read",
                vec![
                    ParamInfo {
                        name: "fd".into(),
                        ty: Ty::Int,
                        auto_assign: false,
                    },
                    ParamInfo {
                        name: "max".into(),
                        ty: Ty::Int,
                        auto_assign: false,
                    },
                ],
                Ty::String,
            ),
            (
                "tcp_write",
                vec![
                    ParamInfo {
                        name: "fd".into(),
                        ty: Ty::Int,
                        auto_assign: false,
                    },
                    ParamInfo {
                        name: "data".into(),
                        ty: Ty::Ref(Box::new(Ty::String)),
                        auto_assign: false,
                    },
                ],
                Ty::Int,
            ),
            (
                "tcp_close",
                vec![ParamInfo {
                    name: "fd".into(),
                    ty: Ty::Int,
                    auto_assign: false,
                }],
                Ty::Unit,
            ),
        ];

        let mut builtin_fn_ids = HashMap::new();
        for (name, params, ret_ty) in builtin_fns {
            let id = self.symbols.define(
                name.to_string(),
                DefKind::Function {
                    signature: FnSignature {
                        self_mode: None,
                        is_class_method: false,
                        is_async: false,
                        generic_params: vec![],
                        params,
                        return_ty: ret_ty,
                    },
                },
                Visibility::Public,
                span.clone(),
            );
            self.scopes.insert(name.to_string(), id);
            builtin_fn_ids.insert(name.to_string(), id);
        }

        // Phase 2 #06.5: `IoError` is a tagged-enum mirroring a small
        // subset of Rust's `std::io::ErrorKind`. Each errno class maps
        // to a unit variant; `Other(message: String)` is the fallback
        // for codes outside the curated subset. The runtime helpers
        // `riven_io_error_*` produce values matching this layout, and
        // the synthesized `IoError.message() -> String` method (wired
        // in codegen/runtime.rs) dispatches on tag.
        let io_error_id = self.symbols.define(
            "IoError".to_string(),
            DefKind::Enum {
                info: EnumInfo {
                    generic_params: vec![],
                    variants: vec![], // filled below
                    derive_traits: vec![],
                    opt_out_send: false,
                    opt_out_sync: false,
                    manual_send: false,
                    manual_sync: false,
                },
            },
            Visibility::Public,
            span.clone(),
        );
        self.scopes.insert_type("IoError".to_string(), io_error_id);
        self.type_registry
            .insert("IoError".to_string(), io_error_id);

        // Variant DefIds (tag indices match the runtime constants in
        // crates/riven-core/runtime/runtime.c — keep them in sync).
        let io_unit_variants: &[(&str, usize)] = &[
            ("NotFound", 0),
            ("PermissionDenied", 1),
            ("AlreadyExists", 2),
            ("Interrupted", 3),
            ("WouldBlock", 4),
            ("InvalidInput", 5),
            ("UnexpectedEof", 6),
            ("BrokenPipe", 7),
        ];
        let mut io_variant_ids: Vec<DefId> = Vec::with_capacity(io_unit_variants.len() + 1);
        for (vname, idx) in io_unit_variants {
            let vid = self.symbols.define(
                (*vname).to_string(),
                DefKind::EnumVariant {
                    parent: io_error_id,
                    variant_idx: *idx,
                    kind: VariantDefKind::Unit,
                },
                Visibility::Public,
                span.clone(),
            );
            self.scopes
                .insert(format!("IoError.{}", vname), vid);
            io_variant_ids.push(vid);
        }
        let io_other_id = self.symbols.define(
            "Other".to_string(),
            DefKind::EnumVariant {
                parent: io_error_id,
                variant_idx: 8,
                kind: VariantDefKind::Struct(vec![("message".to_string(), Ty::String)]),
            },
            Visibility::Public,
            span.clone(),
        );
        self.scopes.insert("IoError.Other".to_string(), io_other_id);
        io_variant_ids.push(io_other_id);

        if let Some(def) = self.symbols.get_mut(io_error_id) {
            if let DefKind::Enum { ref mut info } = def.kind {
                info.variants = io_variant_ids;
            }
        }
        let stdin_id = self.symbols.define(
            "Stdin".to_string(),
            DefKind::Class {
                info: ClassInfo {
                    generic_params: vec![],
                    parent: None,
                    fields: vec![],
                    methods: vec![],
                    derive_traits: vec![],
                    opt_out_send: false,
                    opt_out_sync: false,
                    manual_send: false,
                    manual_sync: false,
                },
            },
            Visibility::Public,
            span.clone(),
        );
        self.type_registry.insert("Stdin".to_string(), stdin_id);
        let stdout_id = self.symbols.define(
            "Stdout".to_string(),
            DefKind::Class {
                info: ClassInfo {
                    generic_params: vec![],
                    parent: None,
                    fields: vec![],
                    methods: vec![],
                    derive_traits: vec![],
                    opt_out_send: false,
                    opt_out_sync: false,
                    manual_send: false,
                    manual_sync: false,
                },
            },
            Visibility::Public,
            span.clone(),
        );
        self.type_registry.insert("Stdout".to_string(), stdout_id);
        let stderr_id = self.symbols.define(
            "Stderr".to_string(),
            DefKind::Class {
                info: ClassInfo {
                    generic_params: vec![],
                    parent: None,
                    fields: vec![],
                    methods: vec![],
                    derive_traits: vec![],
                    opt_out_send: false,
                    opt_out_sync: false,
                    manual_send: false,
                    manual_sync: false,
                },
            },
            Visibility::Public,
            span.clone(),
        );
        self.type_registry.insert("Stderr".to_string(), stderr_id);

        // Phase 2 #06.A1/A3: `std::fmt::Formatter` is the buffer that
        // `Display::fmt` / `Debug::fmt` write into. v1 carries width
        // / alignment / precision metadata as opaque internal fields
        // (no public surface yet) plus a `Vec[Char]`-equivalent
        // backing buffer at the runtime layer (`riven_fmt_*` helpers
        // in `runtime.c`). Phase D wires the constructor + dispatch
        // into `lower_interpolation`.
        let formatter_id = self.symbols.define(
            "Formatter".to_string(),
            DefKind::Class {
                info: ClassInfo {
                    generic_params: vec![],
                    parent: None,
                    fields: vec![],
                    methods: vec![],
                    derive_traits: vec![],
                    opt_out_send: false,
                    opt_out_sync: false,
                    manual_send: false,
                    manual_sync: false,
                },
            },
            Visibility::Public,
            span.clone(),
        );
        self.scopes
            .insert_type("Formatter".to_string(), formatter_id);
        self.type_registry
            .insert("Formatter".to_string(), formatter_id);

        // Phase 2 #06.A4: `std::fmt::FmtError` is a unit struct
        // returned by `Formatter::write_str/write_char`. v1 has no
        // variant payload (matches Rust's `std::fmt::Error`) — it's
        // just a sentinel type. Registered as a class so it can
        // appear in `Result[(), FmtError]` return annotations.
        let fmt_error_id = self.symbols.define(
            "FmtError".to_string(),
            DefKind::Class {
                info: ClassInfo {
                    generic_params: vec![],
                    parent: None,
                    fields: vec![],
                    methods: vec![],
                    derive_traits: vec![],
                    opt_out_send: false,
                    opt_out_sync: false,
                    manual_send: false,
                    manual_sync: false,
                },
            },
            Visibility::Public,
            span.clone(),
        );
        self.scopes
            .insert_type("FmtError".to_string(), fmt_error_id);
        self.type_registry
            .insert("FmtError".to_string(), fmt_error_id);

        let context_id = self.symbols.define(
            "Context".to_string(),
            DefKind::Class {
                info: ClassInfo {
                    generic_params: vec![],
                    parent: None,
                    fields: vec![],
                    methods: vec![],
                    derive_traits: vec![],
                    opt_out_send: false,
                    opt_out_sync: false,
                    manual_send: false,
                    manual_sync: false,
                },
            },
            Visibility::Public,
            span.clone(),
        );
        self.scopes.insert_type("Context".to_string(), context_id);
        self.type_registry.insert("Context".to_string(), context_id);
        let waker_id = self.symbols.define(
            "Waker".to_string(),
            DefKind::Class {
                info: ClassInfo {
                    generic_params: vec![],
                    parent: None,
                    fields: vec![],
                    methods: vec![],
                    derive_traits: vec![],
                    opt_out_send: false,
                    opt_out_sync: false,
                    manual_send: false,
                    manual_sync: false,
                },
            },
            Visibility::Public,
            span.clone(),
        );
        self.scopes.insert_type("Waker".to_string(), waker_id);
        self.type_registry.insert("Waker".to_string(), waker_id);
        let thread_id_id = self.symbols.define(
            "ThreadId".to_string(),
            DefKind::Class {
                info: ClassInfo {
                    generic_params: vec![],
                    parent: None,
                    fields: vec![],
                    methods: vec![],
                    derive_traits: vec![],
                    opt_out_send: false,
                    opt_out_sync: false,
                    manual_send: false,
                    manual_sync: false,
                },
            },
            Visibility::Public,
            span.clone(),
        );
        self.scopes
            .insert_type("ThreadId".to_string(), thread_id_id);
        self.type_registry
            .insert("ThreadId".to_string(), thread_id_id);
        let thread_id_ty = Ty::Class {
            name: "ThreadId".to_string(),
            generic_args: vec![],
        };
        let thread_id_value_id = self.symbols.define(
            "ThreadId".to_string(),
            DefKind::Variable {
                mutable: false,
                ty: thread_id_ty.clone(),
            },
            Visibility::Public,
            span.clone(),
        );
        self.scopes
            .insert("ThreadId".to_string(), thread_id_value_id);

        let thread_id = self.symbols.define(
            "Thread".to_string(),
            DefKind::Class {
                info: ClassInfo {
                    generic_params: vec![],
                    parent: None,
                    fields: vec![],
                    methods: vec![],
                    derive_traits: vec![],
                    opt_out_send: false,
                    opt_out_sync: false,
                    manual_send: false,
                    manual_sync: false,
                },
            },
            Visibility::Public,
            span.clone(),
        );
        self.scopes.insert_type("Thread".to_string(), thread_id);
        self.type_registry.insert("Thread".to_string(), thread_id);

        let join_handle_id = self.symbols.define(
            "JoinHandle".to_string(),
            DefKind::Class {
                info: ClassInfo {
                    generic_params: vec![GenericParamInfo {
                        name: "T".to_string(),
                        bounds: vec![TraitRef {
                            name: "Send".to_string(),
                            generic_args: vec![],
                        }],
                    }],
                    parent: None,
                    fields: vec![],
                    methods: vec![],
                    derive_traits: vec![],
                    opt_out_send: false,
                    opt_out_sync: false,
                    manual_send: false,
                    manual_sync: false,
                },
            },
            Visibility::Public,
            span.clone(),
        );
        self.scopes
            .insert_type("JoinHandle".to_string(), join_handle_id);
        self.type_registry
            .insert("JoinHandle".to_string(), join_handle_id);

        let mutex_id = self.symbols.define(
            "Mutex".to_string(),
            DefKind::Class {
                info: ClassInfo {
                    generic_params: vec![GenericParamInfo {
                        name: "T".to_string(),
                        bounds: vec![],
                    }],
                    parent: None,
                    fields: vec![],
                    methods: vec![],
                    derive_traits: vec![],
                    opt_out_send: false,
                    opt_out_sync: false,
                    manual_send: false,
                    manual_sync: false,
                },
            },
            Visibility::Public,
            span.clone(),
        );
        self.scopes.insert_type("Mutex".to_string(), mutex_id);
        self.type_registry.insert("Mutex".to_string(), mutex_id);

        let mutex_guard_id = self.symbols.define(
            "MutexGuard".to_string(),
            DefKind::Class {
                info: ClassInfo {
                    generic_params: vec![GenericParamInfo {
                        name: "T".to_string(),
                        bounds: vec![],
                    }],
                    parent: None,
                    fields: vec![],
                    methods: vec![],
                    derive_traits: vec![],
                    opt_out_send: false,
                    opt_out_sync: false,
                    manual_send: false,
                    manual_sync: false,
                },
            },
            Visibility::Public,
            span.clone(),
        );
        self.scopes
            .insert_type("MutexGuard".to_string(), mutex_guard_id);
        self.type_registry
            .insert("MutexGuard".to_string(), mutex_guard_id);

        let arc_id = self.symbols.define(
            "Arc".to_string(),
            DefKind::Class {
                info: ClassInfo {
                    generic_params: vec![GenericParamInfo {
                        name: "T".to_string(),
                        bounds: vec![],
                    }],
                    parent: None,
                    fields: vec![],
                    methods: vec![],
                    derive_traits: vec![],
                    opt_out_send: false,
                    opt_out_sync: false,
                    manual_send: false,
                    manual_sync: false,
                },
            },
            Visibility::Public,
            span.clone(),
        );
        self.scopes.insert_type("Arc".to_string(), arc_id);
        self.type_registry.insert("Arc".to_string(), arc_id);

        let poison_error_id = self.symbols.define(
            "PoisonError".to_string(),
            DefKind::Class {
                info: ClassInfo {
                    generic_params: vec![],
                    parent: None,
                    fields: vec![],
                    methods: vec![],
                    derive_traits: vec![],
                    opt_out_send: false,
                    opt_out_sync: false,
                    manual_send: false,
                    manual_sync: false,
                },
            },
            Visibility::Public,
            span.clone(),
        );
        self.scopes
            .insert_type("PoisonError".to_string(), poison_error_id);
        self.type_registry
            .insert("PoisonError".to_string(), poison_error_id);

        let thread_panic_id = self.symbols.define(
            "ThreadPanic".to_string(),
            DefKind::Class {
                info: ClassInfo {
                    generic_params: vec![],
                    parent: None,
                    fields: vec![],
                    methods: vec![],
                    derive_traits: vec![],
                    opt_out_send: false,
                    opt_out_sync: false,
                    manual_send: false,
                    manual_sync: false,
                },
            },
            Visibility::Public,
            span.clone(),
        );
        self.scopes
            .insert_type("ThreadPanic".to_string(), thread_panic_id);
        self.type_registry
            .insert("ThreadPanic".to_string(), thread_panic_id);

        // Register a minimal builtin std module tree so `use std.io`
        // resolves before the fuller Tier-1 stdlib lands.
        let io_id = self.symbols.define(
            "io".to_string(),
            DefKind::Module {
                items: vec![
                    builtin_fn_ids["puts"],
                    builtin_fn_ids["eputs"],
                    builtin_fn_ids["print"],
                    builtin_fn_ids["println"],
                    builtin_fn_ids["eprintln"],
                    builtin_fn_ids["read_line"],
                    builtin_fn_ids["stdin"],
                    builtin_fn_ids["stdout"],
                    builtin_fn_ids["stderr"],
                    io_error_id,
                    stdin_id,
                    stdout_id,
                    stderr_id,
                ],
            },
            Visibility::Public,
            span.clone(),
        );
        let env_id = self.symbols.define(
            "env".to_string(),
            DefKind::Module {
                items: vec![
                    builtin_fn_ids["args"],
                    builtin_fn_ids["var"],
                    // Phase 2 stdlib (#06).
                    builtin_fn_ids["vars"],
                    builtin_fn_ids["current_dir"],
                ],
            },
            Visibility::Public,
            span.clone(),
        );
        let fs_id = self.symbols.define(
            "fs".to_string(),
            DefKind::Module {
                items: vec![
                    builtin_fn_ids["read_to_string"],
                    builtin_fn_ids["write"],
                    builtin_fn_ids["exists"],
                    builtin_fn_ids["remove_file"],
                    builtin_fn_ids["create_dir"],
                    builtin_fn_ids["create_dir_all"],
                    builtin_fn_ids["rename"],
                    // Phase 2 stdlib (#06).
                    builtin_fn_ids["is_file"],
                    builtin_fn_ids["is_dir"],
                    builtin_fn_ids["read_dir"],
                ],
            },
            Visibility::Public,
            span.clone(),
        );
        let process_id = self.symbols.define(
            "process".to_string(),
            DefKind::Module {
                items: vec![builtin_fn_ids["exit"], builtin_fn_ids["process_run"]],
            },
            Visibility::Public,
            span.clone(),
        );
        let time_id = self.symbols.define(
            "time".to_string(),
            DefKind::Module {
                items: vec![builtin_fn_ids["now_ns"], builtin_fn_ids["unix_ns"]],
            },
            Visibility::Public,
            span.clone(),
        );
        let path_id = self.symbols.define(
            "path".to_string(),
            DefKind::Module {
                items: vec![
                    builtin_fn_ids["path_join"],
                    builtin_fn_ids["path_parent"],
                    builtin_fn_ids["path_file_name"],
                    builtin_fn_ids["path_extension"],
                    builtin_fn_ids["path_is_absolute"],
                ],
            },
            Visibility::Public,
            span.clone(),
        );
        let net_id = self.symbols.define(
            "net".to_string(),
            DefKind::Module {
                items: vec![
                    builtin_fn_ids["tcp_connect"],
                    builtin_fn_ids["tcp_listen"],
                    builtin_fn_ids["tcp_accept"],
                    builtin_fn_ids["tcp_read"],
                    builtin_fn_ids["tcp_write"],
                    builtin_fn_ids["tcp_close"],
                ],
            },
            Visibility::Public,
            span.clone(),
        );
        let sync_id = self.symbols.define(
            "sync".to_string(),
            DefKind::Module {
                items: vec![
                    thread_id,
                    thread_id_value_id,
                    join_handle_id,
                    thread_id_id,
                    mutex_id,
                    mutex_guard_id,
                    arc_id,
                    poison_error_id,
                    thread_panic_id,
                ],
            },
            Visibility::Public,
            span.clone(),
        );
        // Phase 2 #06.A1: `std::fmt` module — Display/Debug traits +
        // Formatter/FmtError types. Lookups via `use std.fmt.{...}`.
        // Display + Debug are registered as builtin traits above; we
        // re-export their DefIds here so module-path resolution
        // (`std.fmt.Display`) works.
        let display_trait_id = *self
            .type_registry
            .get("Display")
            .expect("Display trait registered above");
        let debug_trait_id = *self
            .type_registry
            .get("Debug")
            .expect("Debug trait registered above");
        let fmt_id = self.symbols.define(
            "fmt".to_string(),
            DefKind::Module {
                items: vec![display_trait_id, debug_trait_id, formatter_id, fmt_error_id],
            },
            Visibility::Public,
            span.clone(),
        );

        let std_id = self.symbols.define(
            "std".to_string(),
            DefKind::Module {
                items: vec![
                    io_id, env_id, fs_id, process_id, time_id, path_id, net_id, sync_id, fmt_id,
                ],
            },
            Visibility::Public,
            span.clone(),
        );
        self.scopes.insert_type("std".to_string(), std_id);
        self.type_registry.insert("std".to_string(), std_id);

        // Register type constructors in the value scope so Vec.new, String.from, etc. resolve
        let type_constructors = [
            (
                "Vec",
                Ty::Vec(Box::new(Ty::TypeParam {
                    name: "T".to_string(),
                    bounds: vec![],
                })),
            ),
            (
                "HashMap",
                Ty::HashMap(
                    Box::new(Ty::TypeParam {
                        name: "K".to_string(),
                        bounds: vec![],
                    }),
                    Box::new(Ty::TypeParam {
                        name: "V".to_string(),
                        bounds: vec![],
                    }),
                ),
            ),
            (
                "Set",
                Ty::Set(Box::new(Ty::TypeParam {
                    name: "T".to_string(),
                    bounds: vec![],
                })),
            ),
            // `HashSet[T]` alias — same Ty::Set representation. Lets
            // `HashSet.new` / `HashSet.with_capacity(_)` resolve in the
            // value scope alongside `Set.new`.
            (
                "HashSet",
                Ty::Set(Box::new(Ty::TypeParam {
                    name: "T".to_string(),
                    bounds: vec![],
                })),
            ),
            ("String", Ty::String),
            (
                "Thread",
                Ty::Class {
                    name: "Thread".to_string(),
                    generic_args: vec![],
                },
            ),
            (
                "Mutex",
                Ty::Class {
                    name: "Mutex".to_string(),
                    generic_args: vec![Ty::TypeParam {
                        name: "T".to_string(),
                        bounds: vec![],
                    }],
                },
            ),
            (
                "Arc",
                Ty::Class {
                    name: "Arc".to_string(),
                    generic_args: vec![Ty::TypeParam {
                        name: "T".to_string(),
                        bounds: vec![],
                    }],
                },
            ),
        ];
        for (name, ty) in type_constructors {
            let id = self.symbols.define(
                name.to_string(),
                DefKind::Variable { mutable: false, ty },
                Visibility::Public,
                span.clone(),
            );
            self.scopes.insert(name.to_string(), id);
        }

        // Register built-in enum types: Option and Result
        // These are needed so bare Ok/Err/Some/None resolve globally.

        // Option enum
        let option_id = self.symbols.define(
            "Option".to_string(),
            DefKind::Enum {
                info: EnumInfo {
                    generic_params: vec![GenericParamInfo {
                        name: "T".to_string(),
                        bounds: vec![],
                    }],
                    variants: vec![], // will be filled below
                    derive_traits: vec![],
                    opt_out_send: false,
                    opt_out_sync: false,
                    manual_send: false,
                    manual_sync: false,
                },
            },
            Visibility::Public,
            span.clone(),
        );
        self.scopes.insert_type("Option".to_string(), option_id);
        self.type_registry.insert("Option".to_string(), option_id);

        // None = tag 0, Some = tag 1 (matches runtime convention:
        // riven_vec_get_opt, riven_option_unwrap_or, inline_find, etc.)
        let none_id = self.symbols.define(
            "None".to_string(),
            DefKind::EnumVariant {
                parent: option_id,
                variant_idx: 0,
                kind: VariantDefKind::Unit,
            },
            Visibility::Public,
            span.clone(),
        );
        let some_id = self.symbols.define(
            "Some".to_string(),
            DefKind::EnumVariant {
                parent: option_id,
                variant_idx: 1,
                kind: VariantDefKind::Tuple(vec![Ty::TypeParam {
                    name: "T".to_string(),
                    bounds: vec![],
                }]),
            },
            Visibility::Public,
            span.clone(),
        );
        // Register qualified and bare names
        self.scopes.insert("Option.Some".to_string(), some_id);
        self.scopes.insert("Option.None".to_string(), none_id);
        self.scopes.insert("Some".to_string(), some_id);
        self.scopes.insert("None".to_string(), none_id);
        // Also register bare names that the parser generates with empty type_path: ".Some", ".None"
        self.scopes.insert(".Some".to_string(), some_id);
        self.scopes.insert(".None".to_string(), none_id);

        // Update Option enum with variant DefIds
        if let Some(opt_def) = self.symbols.get_mut(option_id) {
            if let DefKind::Enum { ref mut info } = opt_def.kind {
                info.variants = vec![none_id, some_id];
            }
        }

        // Result enum
        let result_id = self.symbols.define(
            "Result".to_string(),
            DefKind::Enum {
                info: EnumInfo {
                    generic_params: vec![
                        GenericParamInfo {
                            name: "T".to_string(),
                            bounds: vec![],
                        },
                        GenericParamInfo {
                            name: "E".to_string(),
                            bounds: vec![],
                        },
                    ],
                    variants: vec![], // will be filled below
                    derive_traits: vec![],
                    opt_out_send: false,
                    opt_out_sync: false,
                    manual_send: false,
                    manual_sync: false,
                },
            },
            Visibility::Public,
            span.clone(),
        );
        self.scopes.insert_type("Result".to_string(), result_id);
        self.type_registry.insert("Result".to_string(), result_id);

        let ok_id = self.symbols.define(
            "Ok".to_string(),
            DefKind::EnumVariant {
                parent: result_id,
                variant_idx: 0,
                kind: VariantDefKind::Tuple(vec![Ty::TypeParam {
                    name: "T".to_string(),
                    bounds: vec![],
                }]),
            },
            Visibility::Public,
            span.clone(),
        );
        let err_id = self.symbols.define(
            "Err".to_string(),
            DefKind::EnumVariant {
                parent: result_id,
                variant_idx: 1,
                kind: VariantDefKind::Tuple(vec![Ty::TypeParam {
                    name: "E".to_string(),
                    bounds: vec![],
                }]),
            },
            Visibility::Public,
            span.clone(),
        );
        // Register qualified and bare names
        self.scopes.insert("Result.Ok".to_string(), ok_id);
        self.scopes.insert("Result.Err".to_string(), err_id);
        self.scopes.insert("Ok".to_string(), ok_id);
        self.scopes.insert("Err".to_string(), err_id);
        // Also register bare names that the parser generates with empty type_path: ".Ok", ".Err"
        self.scopes.insert(".Ok".to_string(), ok_id);
        self.scopes.insert(".Err".to_string(), err_id);

        // Update Result enum with variant DefIds
        if let Some(res_def) = self.symbols.get_mut(result_id) {
            if let DefKind::Enum { ref mut info } = res_def.kind {
                info.variants = vec![ok_id, err_id];
            }
        }

        let poll_id = self.symbols.define(
            "Poll".to_string(),
            DefKind::Enum {
                info: EnumInfo {
                    generic_params: vec![GenericParamInfo {
                        name: "T".to_string(),
                        bounds: vec![],
                    }],
                    variants: vec![],
                    derive_traits: vec![],
                    opt_out_send: false,
                    opt_out_sync: false,
                    manual_send: false,
                    manual_sync: false,
                },
            },
            Visibility::Public,
            span.clone(),
        );
        self.scopes.insert_type("Poll".to_string(), poll_id);
        self.type_registry.insert("Poll".to_string(), poll_id);

        let ready_id = self.symbols.define(
            "Ready".to_string(),
            DefKind::EnumVariant {
                parent: poll_id,
                variant_idx: 0,
                kind: VariantDefKind::Tuple(vec![Ty::TypeParam {
                    name: "T".to_string(),
                    bounds: vec![],
                }]),
            },
            Visibility::Public,
            span.clone(),
        );
        let pending_id = self.symbols.define(
            "Pending".to_string(),
            DefKind::EnumVariant {
                parent: poll_id,
                variant_idx: 1,
                kind: VariantDefKind::Unit,
            },
            Visibility::Public,
            span.clone(),
        );
        self.scopes.insert("Poll.Ready".to_string(), ready_id);
        self.scopes.insert("Poll.Pending".to_string(), pending_id);
        self.scopes.insert("Ready".to_string(), ready_id);
        self.scopes.insert("Pending".to_string(), pending_id);

        if let Some(poll_def) = self.symbols.get_mut(poll_id) {
            if let DefKind::Enum { ref mut info } = poll_def.kind {
                info.variants = vec![ready_id, pending_id];
            }
        }

        // Register super as a built-in function (for parent class constructor calls)
        let super_id = self.symbols.define(
            "super".to_string(),
            DefKind::Function {
                signature: FnSignature {
                    self_mode: None,
                    is_class_method: false,
                    is_async: false,
                    generic_params: vec![],
                    params: vec![], // variadic-like; type checker handles it
                    return_ty: Ty::Unit,
                },
            },
            Visibility::Public,
            span.clone(),
        );
        self.scopes.insert("super".to_string(), super_id);
    }

    // ─── Pass 1: Forward Declaration of Types ───────────────────────

    fn register_top_level_type(&mut self, item: &ast::TopLevelItem) {
        let _span_zero = Span {
            start: 0,
            end: 0,
            line: 0,
            column: 0,
        };

        match item {
            ast::TopLevelItem::Class(class) => {
                let id = self.symbols.define(
                    class.name.clone(),
                    DefKind::Class {
                        info: ClassInfo {
                            generic_params: vec![],
                            parent: None,
                            fields: vec![],
                            methods: vec![],
                            derive_traits: class.derive_traits.clone(),
                            opt_out_send: false,
                            opt_out_sync: false,
                            manual_send: false,
                            manual_sync: false,
                        },
                    },
                    Visibility::Public,
                    class.span.clone(),
                );
                self.scopes.insert_type(class.name.clone(), id);
                self.type_registry.insert(class.name.clone(), id);
            }
            ast::TopLevelItem::Struct(s) => {
                let id = self.symbols.define(
                    s.name.clone(),
                    DefKind::Struct {
                        info: StructInfo {
                            generic_params: vec![],
                            fields: vec![],
                            derive_traits: s.derive_traits.clone(),
                            repr: s.repr.clone(),
                            opt_out_send: false,
                            opt_out_sync: false,
                            manual_send: false,
                            manual_sync: false,
                        },
                    },
                    Visibility::Public,
                    s.span.clone(),
                );
                self.scopes.insert_type(s.name.clone(), id);
                self.type_registry.insert(s.name.clone(), id);
            }
            ast::TopLevelItem::Enum(e) => {
                let id = self.symbols.define(
                    e.name.clone(),
                    DefKind::Enum {
                        info: EnumInfo {
                            generic_params: vec![],
                            variants: vec![],
                            derive_traits: e.derive_traits.clone(),
                            opt_out_send: false,
                            opt_out_sync: false,
                            manual_send: false,
                            manual_sync: false,
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
                let enum_generic_names: Vec<(String, Vec<TraitRef>, Span)> = e
                    .generic_params
                    .as_ref()
                    .map(|gps| {
                        gps.params
                            .iter()
                            .filter_map(|p| match p {
                                ast::GenericParam::Type { name, bounds, span } => {
                                    let trait_refs: Vec<TraitRef> = bounds
                                        .iter()
                                        .map(|b| TraitRef {
                                            name: b.path.segments.join("."),
                                            generic_args: vec![],
                                        })
                                        .collect();
                                    Some((name.clone(), trait_refs, span.clone()))
                                }
                                ast::GenericParam::Lifetime { .. } => None,
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
            ast::TopLevelItem::Trait(t) => {
                let mut required = vec![];
                let mut defaults = vec![];
                let mut assoc = vec![];
                for ti in &t.items {
                    match ti {
                        ast::TraitItem::MethodSig(sig) => required.push(sig.name.clone()),
                        ast::TraitItem::DefaultMethod(f) => defaults.push(f.name.clone()),
                        ast::TraitItem::AssocType { name, .. } => assoc.push(name.clone()),
                    }
                }

                let id = self.symbols.define(
                    t.name.clone(),
                    DefKind::Trait {
                        info: TraitInfo {
                            generic_params: vec![],
                            super_traits: t
                                .super_traits
                                .iter()
                                .map(|b| TraitRef {
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
                    self.register_top_level_type(sub_item);
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
                let id = self.symbols.define(
                    f.name.clone(),
                    DefKind::Function {
                        signature: FnSignature {
                            self_mode: None,
                            is_class_method: false,
                            is_async: f.is_async,
                            generic_params: generic_params
                                .iter()
                                .map(|gp| GenericParamInfo {
                                    name: gp.name.clone(),
                                    bounds: gp.bounds.clone(),
                                })
                                .collect(),
                            params,
                            return_ty,
                        },
                    },
                    Visibility::Public,
                    f.span.clone(),
                );
                self.scopes.insert(f.name.clone(), id);
            }
            ast::TopLevelItem::Lib(lib) => {
                for ffi_fn in &lib.functions {
                    let params: Vec<ParamInfo> = ffi_fn
                        .params
                        .iter()
                        .map(|p| ParamInfo {
                            name: p.name.clone(),
                            ty: self.resolve_type_expr(&p.type_expr),
                            auto_assign: false,
                        })
                        .collect();
                    let return_ty = ffi_fn
                        .return_type
                        .as_ref()
                        .map(|t| self.resolve_type_expr(t))
                        .unwrap_or(Ty::Unit);
                    let id = self.symbols.define(
                        ffi_fn.name.clone(),
                        DefKind::Function {
                            signature: FnSignature {
                                self_mode: None,
                                is_class_method: false,
                                is_async: false,
                                generic_params: vec![],
                                params,
                                return_ty,
                            },
                        },
                        Visibility::Public,
                        ffi_fn.span.clone(),
                    );
                    self.scopes.insert(ffi_fn.name.clone(), id);
                }
            }
            ast::TopLevelItem::Extern(ext) => {
                for ffi_fn in &ext.functions {
                    let params: Vec<ParamInfo> = ffi_fn
                        .params
                        .iter()
                        .map(|p| ParamInfo {
                            name: p.name.clone(),
                            ty: self.resolve_type_expr(&p.type_expr),
                            auto_assign: false,
                        })
                        .collect();
                    let return_ty = ffi_fn
                        .return_type
                        .as_ref()
                        .map(|t| self.resolve_type_expr(t))
                        .unwrap_or(Ty::Unit);
                    let id = self.symbols.define(
                        ffi_fn.name.clone(),
                        DefKind::Function {
                            signature: FnSignature {
                                self_mode: None,
                                is_class_method: false,
                                is_async: false,
                                generic_params: vec![],
                                params,
                                return_ty,
                            },
                        },
                        Visibility::Public,
                        ffi_fn.span.clone(),
                    );
                    self.scopes.insert(ffi_fn.name.clone(), id);
                }
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
            ast::TopLevelItem::Trait(t) => Some(HirItem::Trait(self.resolve_trait(t))),
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
            let trait_ref = TraitRef {
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

        // Update the symbol table with full class info
        if let Some(def) = self.symbols.get_mut(def_id) {
            def.kind = DefKind::Class {
                info: ClassInfo {
                    generic_params: generic_params
                        .iter()
                        .map(|gp| GenericParamInfo {
                            name: gp.name.clone(),
                            bounds: gp.bounds.clone(),
                        })
                        .collect(),
                    parent: parent_def,
                    fields: field_def_ids,
                    methods: method_def_ids,
                    derive_traits: class.derive_traits.clone(),
                    opt_out_send,
                    opt_out_sync,
                    manual_send,
                    manual_sync,
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

        // Update symbol table
        if let Some(def) = self.symbols.get_mut(def_id) {
            def.kind = DefKind::Struct {
                info: StructInfo {
                    generic_params: generic_params
                        .iter()
                        .map(|gp| GenericParamInfo {
                            name: gp.name.clone(),
                            bounds: gp.bounds.clone(),
                        })
                        .collect(),
                    fields: field_def_ids,
                    derive_traits: s.derive_traits.clone(),
                    repr: s.repr.clone(),
                    opt_out_send: false,
                    opt_out_sync: false,
                    manual_send: false,
                    manual_sync: false,
                },
            };
        }

        HirStructDef {
            def_id,
            name: s.name.clone(),
            generic_params,
            fields,
            derive_traits: s.derive_traits.clone(),
            repr: s.repr.clone(),
            doc_comments: s.doc_comments.clone(),
            span: s.span.clone(),
        }
    }

    // ─── Enum Resolution ────────────────────────────────────────────

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

        // Update symbol table
        if let Some(def) = self.symbols.get_mut(def_id) {
            def.kind = DefKind::Enum {
                info: EnumInfo {
                    generic_params: generic_params
                        .iter()
                        .map(|gp| GenericParamInfo {
                            name: gp.name.clone(),
                            bounds: gp.bounds.clone(),
                        })
                        .collect(),
                    variants: variant_def_ids,
                    derive_traits: e.derive_traits.clone(),
                    opt_out_send: false,
                    opt_out_sync: false,
                    manual_send: false,
                    manual_sync: false,
                },
            };
        }

        self.scopes.pop();

        HirEnumDef {
            def_id,
            name: e.name.clone(),
            generic_params,
            variants,
            derive_traits: e.derive_traits.clone(),
            doc_comments: e.doc_comments.clone(),
            span: e.span.clone(),
        }
    }

    // ─── Trait Resolution ───────────────────────────────────────────

    fn resolve_trait(&mut self, t: &ast::TraitDef) -> HirTraitDef {
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
            bounds: vec![TraitRef {
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

        let super_traits: Vec<TraitRef> = t
            .super_traits
            .iter()
            .map(|b| TraitRef {
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
                ast::TraitItem::AssocType { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect();
        let old_trait_ctx = self
            .current_trait_context
            .replace((t.name.clone(), assoc_names));

        let mut items = Vec::new();
        for ti in &t.items {
            match ti {
                ast::TraitItem::AssocType { name, span } => {
                    items.push(HirTraitItem::AssocType {
                        name: name.clone(),
                        span: span.clone(),
                    });
                }
                ast::TraitItem::MethodSig(sig) => {
                    let params = self.resolve_params(&sig.params);
                    let return_ty = sig
                        .return_type
                        .as_ref()
                        .map(|t| self.resolve_type_expr(t))
                        .unwrap_or(Ty::Unit);
                    let self_mode = sig.self_mode.map(|m| self.convert_self_mode(m));

                    items.push(HirTraitItem::MethodSig {
                        name: sig.name.clone(),
                        self_mode,
                        is_class_method: sig.is_class_method,
                        params,
                        return_ty,
                        span: sig.span.clone(),
                    });
                }
                ast::TraitItem::DefaultMethod(f) => {
                    items.push(HirTraitItem::DefaultMethod(self.resolve_func_def(f, None)));
                }
            }
        }

        self.current_trait_context = old_trait_ctx;
        self.current_self_ty = old_self_ty;
        self.scopes.pop();

        HirTraitDef {
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
        let trait_ref = imp.trait_name.as_ref().map(|tp| TraitRef {
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
                "`unsafe impl` is only meaningful for trait implementations",
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
                        "negative impls are only supported for Send and Sync",
                        span,
                        "E1014",
                    ));
                } else if is_unsafe {
                    self.diagnostics.push(Diagnostic::error_with_code(
                        "`unsafe impl` is only required for Send and Sync",
                        span,
                        "E1014",
                    ));
                }
                return;
            }
        };

        if !negative_trait && !is_unsafe {
            self.diagnostics.push(Diagnostic::error_with_code(
                "manual Send/Sync impls must be declared as `unsafe impl`",
                span,
                "E1014",
            ));
            return;
        }

        let Some(def) = nominal_type_definition_mut(target_ty, &mut self.symbols) else {
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
                                gp.bounds.push(TraitRef {
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
            generic_params: generic_params
                .iter()
                .map(|gp| GenericParamInfo {
                    name: gp.name.clone(),
                    bounds: gp.bounds.clone(),
                })
                .collect(),
            params: params
                .iter()
                .map(|p| ParamInfo {
                    name: p.name.clone(),
                    ty: p.ty.clone(),
                    auto_assign: p.auto_assign,
                })
                .collect(),
            return_ty: return_ty.clone(),
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
                    let ty = self
                        .symbols
                        .def_ty(def_id)
                        .unwrap_or_else(|| self.type_context.fresh_type_var());
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
                let ty = Ty::Vec(Box::new(elem_ty));
                HirExpr {
                    kind: HirExprKind::ArrayLiteral(elems_hir),
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
                    ty: Ty::Array(Box::new(elem_ty), count_val),
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
                    "vec" => {
                        let elem_ty = if args_hir.is_empty() {
                            self.type_context.fresh_type_var()
                        } else {
                            args_hir[0].ty.clone()
                        };
                        Ty::Vec(Box::new(elem_ty))
                    }
                    "hash" => {
                        let (k, v) = if args_hir.len() >= 2 {
                            (args_hir[0].ty.clone(), args_hir[1].ty.clone())
                        } else {
                            (
                                self.type_context.fresh_type_var(),
                                self.type_context.fresh_type_var(),
                            )
                        };
                        Ty::HashMap(Box::new(k), Box::new(v))
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
                    // Fixed-size array [T; N]
                    let n = match &size_expr.kind {
                        ast::ExprKind::IntLiteral(v, _) => *v as usize,
                        _ => 0,
                    };
                    Ty::Array(Box::new(elem_ty), n)
                } else {
                    // Slice [T] — treat as Vec for now
                    Ty::Vec(Box::new(elem_ty))
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
            ast::TypeExpr::ImplTrait { bounds, .. } => Ty::ImplTrait(
                bounds
                    .iter()
                    .map(|b| TraitRef {
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
            ast::TypeExpr::DynTrait { bounds, .. } => Ty::DynTrait(
                bounds
                    .iter()
                    .map(|b| TraitRef {
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
                        bounds: vec![TraitRef {
                            name: trait_name.clone(),
                            generic_args: vec![],
                        }],
                    };
                }
            }
            // Fall through to the default error path with the joined name.
        }

        let name = path.segments.join(".");
        let generic_args: Vec<Ty> = path
            .generic_args
            .as_ref()
            .map(|args| args.iter().map(|a| self.resolve_type_expr(a)).collect())
            .unwrap_or_default();

        // Check built-in generic types
        match name.as_str() {
            "Vec" => {
                let elem = generic_args
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| self.type_context.fresh_type_var());
                return Ty::Vec(Box::new(elem));
            }
            "HashMap" => {
                let mut iter = generic_args.into_iter();
                let k = iter
                    .next()
                    .unwrap_or_else(|| self.type_context.fresh_type_var());
                let v = iter
                    .next()
                    .unwrap_or_else(|| self.type_context.fresh_type_var());
                // K must be Hash + Eq. Reject compound containers
                // (Vec/Set/HashMap) and aggregates that don't derive Hash.
                if !ty_is_valid_hash_key(&k, &self.symbols) {
                    self.diagnostics.push(Diagnostic::error_with_code(
                        format!(
                            "HashMap key type `{}` is not hashable: K must implement Hash + Eq",
                            k
                        ),
                        path.span.clone(),
                        "E0615",
                    ));
                }
                return Ty::HashMap(Box::new(k), Box::new(v));
            }
            "Set" | "HashSet" => {
                // `HashSet[T]` is an alias for `Set[T]` per the v1 stdlib
                // surface (prompt #04). Both desugar to the same runtime
                // representation; method dispatch in
                // `codegen::runtime::runtime_name` accepts either prefix.
                let elem = generic_args
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| self.type_context.fresh_type_var());
                if !ty_is_valid_hash_key(&elem, &self.symbols) {
                    self.diagnostics.push(Diagnostic::error_with_code(
                        format!(
                            "HashSet element type `{}` is not hashable: T must implement Hash + Eq",
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

    fn resolve_generic_params(&mut self, gp: &Option<ast::GenericParams>) -> Vec<HirGenericParam> {
        gp.as_ref()
            .map(|gps| {
                gps.params
                    .iter()
                    .filter_map(|p| {
                        match p {
                            ast::GenericParam::Type { name, bounds, span } => {
                                let trait_refs: Vec<TraitRef> = bounds
                                    .iter()
                                    .map(|b| TraitRef {
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
fn ty_is_valid_hash_key(ty: &Ty, symbols: &crate::resolve::symbols::SymbolTable) -> bool {
    use crate::resolve::symbols::ty_has_derive_trait;
    if ty.is_integer() || ty.is_float() {
        return true;
    }
    if matches!(
        ty,
        Ty::Bool | Ty::Char | Ty::Unit | Ty::String | Ty::Str | Ty::Never
    ) {
        return true;
    }
    match ty {
        // Compound containers are NOT Hash even when their elements are.
        // Vec/HashMap/HashSet have interior heap pointers whose hash would
        // not be stable across allocations; v1 chooses not to derive a
        // structural hash for them.
        Ty::Vec(_) | Ty::Set(_) | Ty::HashMap(_, _) => false,
        // Tuples / arrays / Option / Result : recurse — Hash if every
        // component is Hash.
        Ty::Array(inner, _) | Ty::Option(inner) => ty_is_valid_hash_key(inner, symbols),
        Ty::Result(a, b) => ty_is_valid_hash_key(a, symbols) && ty_is_valid_hash_key(b, symbols),
        Ty::Tuple(elems) => elems.iter().all(|e| ty_is_valid_hash_key(e, symbols)),
        Ty::Ref(inner)
        | Ty::RefMut(inner)
        | Ty::RefLifetime(_, inner)
        | Ty::RefMutLifetime(_, inner) => ty_is_valid_hash_key(inner, symbols),
        Ty::Alias { target, .. } => ty_is_valid_hash_key(target, symbols),
        Ty::Newtype { inner, .. } => ty_is_valid_hash_key(inner, symbols),
        Ty::Struct { .. } | Ty::Class { .. } | Ty::Enum { .. } => {
            ty_has_derive_trait(ty, symbols, "Hash") || ty_has_derive_trait(ty, symbols, "Hashable")
        }
        // Type-vars / param types: assume satisfiable (typeck unifies later).
        // Returning true here keeps generic code (e.g. `def f[K]`) compiling;
        // the actual Hash bound check on call sites is enforced via trait
        // bounds elsewhere.
        Ty::TypeParam { .. } | Ty::Infer(_) | Ty::Error => true,
        _ => false,
    }
}

fn nominal_type_definition_mut<'a>(
    target_ty: &Ty,
    symbols: &'a mut SymbolTable,
) -> Option<&'a mut Definition> {
    let name = match target_ty {
        Ty::Class { name, .. } | Ty::Struct { name, .. } | Ty::Enum { name, .. } => name,
        _ => return None,
    };

    let def_id = symbols
        .iter()
        .find(|def| {
            def.name == *name
                && matches!(
                    def.kind,
                    DefKind::Class { .. } | DefKind::Struct { .. } | DefKind::Enum { .. }
                )
        })
        .map(|def| def.id)?;

    symbols.get_mut(def_id)
}

impl Default for Resolver {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Yield Pre-Scan ────────────────────────────────────────────────────
//
// `yield VALUE` inside a function body implicitly introduces a trailing
// `__block: Closure` parameter.  Before the main resolution pass, walk
// the AST and record every function whose body contains a `yield`, along
// with the arity of the first `yield` found.  The arity is used to
// pre-shape the synthetic block's `Ty::Fn` parameter list so that
// caller-side unification on the trailing closure produces a concrete type.

fn collect_yield_fns(item: &ast::TopLevelItem, out: &mut HashMap<String, usize>) {
    match item {
        ast::TopLevelItem::Function(f) => {
            if let Some(arity) = find_first_yield_arity_in_block(&f.body) {
                out.insert(f.name.clone(), arity);
            }
        }
        ast::TopLevelItem::Module(m) => {
            for sub in &m.items {
                collect_yield_fns(sub, out);
            }
        }
        ast::TopLevelItem::Class(c) => {
            for m in &c.methods {
                if let Some(arity) = find_first_yield_arity_in_block(&m.body) {
                    out.insert(m.name.clone(), arity);
                }
            }
        }
        ast::TopLevelItem::Impl(b) => {
            for it in &b.items {
                if let ast::ImplItem::Method(m) = it {
                    if let Some(arity) = find_first_yield_arity_in_block(&m.body) {
                        out.insert(m.name.clone(), arity);
                    }
                }
            }
        }
        _ => {}
    }
}

fn find_first_yield_arity_in_block(block: &ast::Block) -> Option<usize> {
    for stmt in &block.statements {
        if let Some(a) = find_first_yield_arity_in_stmt(stmt) {
            return Some(a);
        }
    }
    None
}

fn find_first_yield_arity_in_stmt(stmt: &ast::Statement) -> Option<usize> {
    match stmt {
        ast::Statement::Let(b) => b.value.as_deref().and_then(find_first_yield_arity_in_expr),
        ast::Statement::Expression(e) => find_first_yield_arity_in_expr(e),
    }
}

fn find_first_yield_arity_in_expr(expr: &ast::Expr) -> Option<usize> {
    use ast::ExprKind::*;
    match &expr.kind {
        Yield(args) => Some(args.len()),
        BinaryOp { left, right, .. } => {
            find_first_yield_arity_in_expr(left).or_else(|| find_first_yield_arity_in_expr(right))
        }
        UnaryOp { operand, .. } => find_first_yield_arity_in_expr(operand),
        Borrow(e) | BorrowMut(e) => find_first_yield_arity_in_expr(e),
        FieldAccess { object, .. } | SafeNav { object, .. } => {
            find_first_yield_arity_in_expr(object)
        }
        MethodCall {
            object,
            args,
            block,
            ..
        } => find_first_yield_arity_in_expr(object)
            .or_else(|| args.iter().find_map(find_first_yield_arity_in_expr))
            .or_else(|| block.as_deref().and_then(find_first_yield_arity_in_expr)),
        SafeNavCall { object, args, .. } => find_first_yield_arity_in_expr(object)
            .or_else(|| args.iter().find_map(find_first_yield_arity_in_expr)),
        Call {
            callee,
            args,
            block,
        } => find_first_yield_arity_in_expr(callee)
            .or_else(|| args.iter().find_map(find_first_yield_arity_in_expr))
            .or_else(|| block.as_deref().and_then(find_first_yield_arity_in_expr)),
        Index { object, index } => {
            find_first_yield_arity_in_expr(object).or_else(|| find_first_yield_arity_in_expr(index))
        }
        ClosureCall { callee, args } => find_first_yield_arity_in_expr(callee)
            .or_else(|| args.iter().find_map(find_first_yield_arity_in_expr)),
        Try(e) => find_first_yield_arity_in_expr(e),
        Assign { target, value } => {
            find_first_yield_arity_in_expr(target).or_else(|| find_first_yield_arity_in_expr(value))
        }
        CompoundAssign { target, value, .. } => {
            find_first_yield_arity_in_expr(target).or_else(|| find_first_yield_arity_in_expr(value))
        }
        If(ife) => find_first_yield_arity_in_expr(&ife.condition)
            .or_else(|| find_first_yield_arity_in_block(&ife.then_body))
            .or_else(|| {
                ife.elsif_clauses.iter().find_map(|c| {
                    find_first_yield_arity_in_expr(&c.condition)
                        .or_else(|| find_first_yield_arity_in_block(&c.body))
                })
            })
            .or_else(|| {
                ife.else_body
                    .as_ref()
                    .and_then(find_first_yield_arity_in_block)
            }),
        IfLet(ile) => find_first_yield_arity_in_expr(&ile.value)
            .or_else(|| find_first_yield_arity_in_block(&ile.then_body))
            .or_else(|| {
                ile.else_body
                    .as_ref()
                    .and_then(find_first_yield_arity_in_block)
            }),
        Match(me) => find_first_yield_arity_in_expr(&me.subject).or_else(|| {
            me.arms.iter().find_map(|a| match &a.body {
                ast::MatchArmBody::Expr(e) => find_first_yield_arity_in_expr(e),
                ast::MatchArmBody::Block(b) => find_first_yield_arity_in_block(b),
            })
        }),
        While(we) => find_first_yield_arity_in_expr(&we.condition)
            .or_else(|| find_first_yield_arity_in_block(&we.body)),
        WhileLet(wle) => find_first_yield_arity_in_expr(&wle.value)
            .or_else(|| find_first_yield_arity_in_block(&wle.body)),
        For(fe) => find_first_yield_arity_in_expr(&fe.iterable)
            .or_else(|| find_first_yield_arity_in_block(&fe.body)),
        Loop(le) => find_first_yield_arity_in_block(&le.body),
        Block(b) | UnsafeBlock(b) => find_first_yield_arity_in_block(b),
        // A `yield` inside a nested closure does not belong to the
        // enclosing function for our v1 implicit-block scheme; skipping
        // avoids double-counting cases like `.filter { |x| yield x }`
        // where the surrounding method already declares an explicit
        // block parameter.
        Closure(_) => None,
        Range { start, end, .. } => start
            .as_deref()
            .and_then(find_first_yield_arity_in_expr)
            .or_else(|| end.as_deref().and_then(find_first_yield_arity_in_expr)),
        ArrayLiteral(elems) => elems.iter().find_map(find_first_yield_arity_in_expr),
        ArrayFill { value, count } => {
            find_first_yield_arity_in_expr(value).or_else(|| find_first_yield_arity_in_expr(count))
        }
        TupleLiteral(elems) => elems.iter().find_map(find_first_yield_arity_in_expr),
        Return(e) | Break(e) => e.as_deref().and_then(find_first_yield_arity_in_expr),
        Continue => None,
        MacroCall { args, .. } => args.iter().find_map(find_first_yield_arity_in_expr),
        Cast { expr, .. } => find_first_yield_arity_in_expr(expr),
        EnumVariant { args, .. } => args
            .iter()
            .find_map(|fa| find_first_yield_arity_in_expr(&fa.value)),
        InterpolatedString(_) => None,
        _ => None,
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
