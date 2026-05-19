//! Built-in stdlib registrations.
//!
//! Phase C of #06.75 carved this out of `resolve/mod.rs` (where it lived
//! as the 2156-LOC `Resolver::register_builtins` method).  Every primitive
//! type, builtin trait, free function, stdlib class (IoError / File /
//! Command / Metadata / Duration / Instant / Formatter / …), the
//! `std::{io,fs,net,time,process,env,fmt,…}` module tree, and the
//! Option/Result enum types is wired here.
//!
//! Future follow-up (tracked separately): split this single `register_all`
//! function into per-namespace files (`primitives.rs`, `io.rs`, `fs.rs`,
//! `process.rs`, `time.rs`, `fmt.rs`, `collections.rs`, `option.rs`,
//! `result.rs`).  The internal section banners below already mark the cut
//! lines.

use std::collections::HashMap;

use crate::hir::nodes::*;
use crate::hir::types::{MixinRef, Ty};
use crate::lexer::token::Span;
use crate::parser::ast::Visibility;

use super::symbols::*;
use super::Resolver;

/// Register every built-in into the resolver.
///
/// Called once from `Resolver::register_builtins` at the start of a
/// resolution run, before any user code is walked.
pub(super) fn register_all(r: &mut Resolver) {
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
        let id = r.symbols.define(
            name.to_string(),
            DefKind::TypeAlias { target: ty },
            Visibility::Public,
            span.clone(),
        );
        r.scopes.insert_type(name.to_string(), id);
        r.type_registry.insert(name.to_string(), id);
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
        let id = r.symbols.define(
            name.to_string(),
            DefKind::Trait {
                info: MixinInfo {
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
        r.scopes.insert_type(name.to_string(), id);
        r.type_registry.insert(name.to_string(), id);
        if name == "Hashable" {
            hashable_id = Some(id);
        }
    }

    let future_trait_id = r.symbols.define(
        "Future".to_string(),
        DefKind::Trait {
            info: MixinInfo {
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
    r.scopes.insert_type("Future".to_string(), future_trait_id);
    r.type_registry
        .insert("Future".to_string(), future_trait_id);

    // `Into[T]` — generic conversion trait used by `?` to coerce
    // source errors into the outer function's declared error type.
    // The MIR `try_op` lowering looks up
    // `into_impls[(Src, Dst)]` (populated from `include Into[Dst]`
    // blocks in collect.rs) and emits a call to `{Src}_into`. See
    // docs/tutorial/11-error-handling.md §"Error Conversion".
    let into_trait_id = r.symbols.define(
        "Into".to_string(),
        DefKind::Trait {
            info: MixinInfo {
                generic_params: vec![GenericParamInfo::type_param("T".to_string(), vec![])],
                super_traits: vec![],
                required_methods: vec!["into".to_string()],
                default_methods: vec![],
                assoc_types: vec![],
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes.insert_type("Into".to_string(), into_trait_id);
    r.type_registry.insert("Into".to_string(), into_trait_id);

    // Deprecated alias: `Hash` (the trait) → `Hashable`.
    // The collection type `Hash[K,V]` has its own resolution path
    // (see `resolve_type_path` / the `Hash`/`Vec`/`Set` match in
    // type-position) and is unaffected by this alias.
    if let Some(id) = hashable_id {
        r.scopes.insert_type("Hash".to_string(), id);
        r.type_registry.insert("Hash".to_string(), id);
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
    // Phase 2 stdlib (#06): `fs::metadata` returns
    // `Result[Metadata, IoError]`. `Metadata` is a flat heap struct
    // exposing `len`/`modified`/`is_file`/`is_dir`/`is_symlink`
    // accessors; see `riven_fs_metadata` in `runtime.c` for the
    // on-wire layout. Registered as a Class below so it can appear
    // in Result return annotations and dispatch via the standard
    // `{Type}_{method}` mangled-name pipeline.
    let metadata_ty = Ty::Class {
        name: "Metadata".to_string(),
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
        ("args", vec![], Ty::Array(Box::new(Ty::String))),
        // ruby-naming.spec.md §3.14: `var` is a reserved keyword
        // (binding form / writable-reference / writable-pointer /
        // writing-method marker). Renamed from `env.var` to `env.get`
        // — `get` is also the canonical collection-lookup verb in
        // the rest of the stdlib (`Map.get`, `Array.get`).
        (
            "get",
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
            Ty::Map(Box::new(Ty::String), Box::new(Ty::String)),
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
                Box::new(Ty::Array(Box::new(Ty::String))),
                Box::new(io_error_ty.clone()),
            ),
        ),
        // Phase 2 stdlib (#06): fs::metadata. Backed by `lstat(2)`
        // (symlinks are reported as Symlink, not followed). The
        // returned Metadata is heap-allocated and freed via the
        // standard Class scope-exit drop pass.
        (
            "metadata",
            vec![ParamInfo {
                name: "path".into(),
                ty: Ty::Ref(Box::new(Ty::String)),
                auto_assign: false,
            }],
            Ty::Result(Box::new(metadata_ty.clone()), Box::new(io_error_ty.clone())),
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
        // Phase 2 stdlib (#06.5 T3): fs completeness — copy / recursive
        // remove / canonicalize / atomic write / symlink helpers. Each
        // is a thin wrapper over its libc equivalent in `runtime.c`;
        // null inputs surface IoError.InvalidInput rather than Other.
        (
            "copy",
            vec![
                ParamInfo {
                    name: "src".into(),
                    ty: Ty::Ref(Box::new(Ty::String)),
                    auto_assign: false,
                },
                ParamInfo {
                    name: "dst".into(),
                    ty: Ty::Ref(Box::new(Ty::String)),
                    auto_assign: false,
                },
            ],
            Ty::Result(Box::new(Ty::Int), Box::new(io_error_ty.clone())),
        ),
        (
            "remove_dir_all",
            vec![ParamInfo {
                name: "path".into(),
                ty: Ty::Ref(Box::new(Ty::String)),
                auto_assign: false,
            }],
            Ty::Result(Box::new(Ty::Unit), Box::new(io_error_ty.clone())),
        ),
        (
            "canonicalize",
            vec![ParamInfo {
                name: "path".into(),
                ty: Ty::Ref(Box::new(Ty::String)),
                auto_assign: false,
            }],
            Ty::Result(Box::new(Ty::String), Box::new(io_error_ty.clone())),
        ),
        (
            "write_atomic",
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
            "read_link",
            vec![ParamInfo {
                name: "path".into(),
                ty: Ty::Ref(Box::new(Ty::String)),
                auto_assign: false,
            }],
            Ty::Result(Box::new(Ty::String), Box::new(io_error_ty.clone())),
        ),
        (
            "symlink",
            vec![
                ParamInfo {
                    name: "target".into(),
                    ty: Ty::Ref(Box::new(Ty::String)),
                    auto_assign: false,
                },
                ParamInfo {
                    name: "link".into(),
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
        // std::time — Phase 3 / #06.5. `unix_ns` is wall-clock
        // (nanoseconds since 1970-01-01 UTC) and stays exposed as a
        // bare Int-returning free-fn until a `SystemTime` class lands.
        // The previously-exposed monotonic `now_ns()` free-fn was
        // removed in #06.5 T5.5 once `Instant.now` + `Instant.elapsed`
        // covered every use case. The C symbol `riven_time_now_ns` is
        // still linked from the runtime (it is the implementation
        // behind `riven_instant_now`); it just is not reachable from
        // Riven user code.
        ("unix_ns", vec![], Ty::Int),
        // std::thread — Phase 2 stdlib (#06.5 T4). Free fn
        // `sleep(&Duration)` is the Duration-typed wrapper around
        // the existing `Thread.sleep(int)` static method. Both
        // surfaces coexist intentionally: `Thread.sleep` is a
        // bare-int convenience; `std.thread.sleep(Duration.from_*)`
        // is the typed surface that integrates with `Instant`.
        (
            "sleep",
            vec![ParamInfo {
                name: "d".into(),
                ty: Ty::Ref(Box::new(Ty::Class {
                    name: "Duration".to_string(),
                    generic_args: vec![],
                })),
                auto_assign: false,
            }],
            Ty::Unit,
        ),
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
        // std::process — the flat `process_run(cmd, args) -> Int`
        // free-fn previously exposed here was removed in #06.5 T5.5
        // once `Command.new(cmd).args(args).status` covered every use
        // case. The C symbol `riven_process_run` is still linked from
        // the runtime (it is the implementation behind
        // `riven_command_status`); it just is not reachable from
        // Riven user code. See docs/specs/stdlib/process.spec.md.
        // std::net — Phase 2 #06.5 T5: class-only surface. The flat
        // tcp_* free fns previously exposed here (tcp_connect/listen/
        // accept/read/write/close) were removed when the typed
        // TcpListener / TcpStream wrappers shipped — see
        // docs/specs/stdlib/net.spec.md C16 for the rationale and the
        // `flat_tcp_free_fns_removed_from_resolver` pin test that
        // guards this surface against a regression. The C runtime
        // symbols (riven_tcp_connect/...) are still linked and reused
        // internally by the class methods; they just aren't reachable
        // from Riven user code.
        // std::signal — cooperative graceful-shutdown surface for
        // long-running programs (servers, daemons, REPLs).
        // `install_sigint` registers a handler that sets an atomic
        // flag; `received_sigint` polls it.  Blocking syscalls
        // (`tcp_accept`, `tcp_read`, …) return EINTR / `-1` when
        // the signal lands mid-call so cooperative loops can
        // notice and break.  No SIGTERM / SIGHUP coverage in v1.
        ("signal_install_sigint", vec![], Ty::Unit),
        (
            // Returns Int (0 or 1) for safe codegen — the
            // underlying C helper returns `int64_t`.  Callers
            // pattern this as `if signal_received_sigint != 0`.
            "signal_received_sigint",
            vec![],
            Ty::Int,
        ),
        // std::rand — Phase 2 #06.5 T8 was here.
        //
        // Wave 2 (#06.8): migrated to `library/std/src/rand.rvn`. The
        // `random_bytes` / `random_u64` / `random_fill` signatures and
        // their `c_symbol` aliases (`riven_rand_random_bytes`, …) now
        // live in the .rvn file as `lib "riven_runtime" def NAME as
        // "C-SYMBOL"(…) -> …` decls, processed by the bootstrap loader
        // before the user's program is resolved. The `std.rand` module
        // namespace is still assembled below (with empty items) and
        // populated by [`Resolver::fixup_bootstrapped_stdlib_modules`]
        // after the bootstrap merge so `use std.rand.{random_bytes, …}`
        // keeps working without flag-day coordination.
    ];

    let mut builtin_fn_ids = HashMap::new();
    for (name, params, ret_ty) in builtin_fns {
        let id = r.symbols.define(
            name.to_string(),
            DefKind::Function {
                signature: FnSignature {
                    self_mode: None,
                    is_class_method: false,
                    is_async: false,
                    generic_params: vec![],
                    params,
                    return_ty: ret_ty,
                    c_symbol: None,
                },
            },
            Visibility::Public,
            span.clone(),
        );
        r.scopes.insert(name.to_string(), id);
        builtin_fn_ids.insert(name.to_string(), id);
    }

    // Phase 2 #06.5: `IoError` is a tagged-enum mirroring a small
    // subset of Rust's `std::io::ErrorKind`. Each errno class maps
    // to a unit variant; `Other(message: String)` is the fallback
    // for codes outside the curated subset. The runtime helpers
    // `riven_io_error_*` produce values matching this layout, and
    // the synthesized `IoError.message() -> String` method (wired
    // in codegen/runtime.rs) dispatches on tag.
    let io_error_id = r.symbols.define(
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
                const_predicates: vec![],
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes.insert_type("IoError".to_string(), io_error_id);
    r.type_registry.insert("IoError".to_string(), io_error_id);

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
        let vid = r.symbols.define(
            (*vname).to_string(),
            DefKind::EnumVariant {
                parent: io_error_id,
                variant_idx: *idx,
                kind: VariantDefKind::Unit,
            },
            Visibility::Public,
            span.clone(),
        );
        r.scopes.insert(format!("IoError.{}", vname), vid);
        io_variant_ids.push(vid);
    }
    let io_other_id = r.symbols.define(
        "Other".to_string(),
        DefKind::EnumVariant {
            parent: io_error_id,
            variant_idx: 8,
            kind: VariantDefKind::Struct(vec![("message".to_string(), Ty::String)]),
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes.insert("IoError.Other".to_string(), io_other_id);
    io_variant_ids.push(io_other_id);

    // Phase 2 #06.5 T1: 11 additional message-carrying variants
    // (idx 9..19). Same shape as `Other` — each carries a single
    // `message: String` field. Tag values must stay in sync with
    // the `RIVEN_IO_ERROR_*` constants in
    // crates/riven-core/runtime/runtime.c.
    let io_struct_variants: &[(&str, usize)] = &[
        ("ConnectionRefused", 9),
        ("ConnectionReset", 10),
        ("ConnectionAborted", 11),
        ("NotConnected", 12),
        ("AddrInUse", 13),
        ("AddrNotAvailable", 14),
        ("InvalidData", 15),
        ("TimedOut", 16),
        ("WriteZero", 17),
        ("Unsupported", 18),
        ("OutOfMemory", 19),
    ];
    for (vname, idx) in io_struct_variants {
        let vid = r.symbols.define(
            (*vname).to_string(),
            DefKind::EnumVariant {
                parent: io_error_id,
                variant_idx: *idx,
                kind: VariantDefKind::Struct(vec![("message".to_string(), Ty::String)]),
            },
            Visibility::Public,
            span.clone(),
        );
        r.scopes.insert(format!("IoError.{}", vname), vid);
        io_variant_ids.push(vid);
    }

    if let Some(def) = r.symbols.get_mut(io_error_id) {
        if let DefKind::Enum { ref mut info } = def.kind {
            info.variants = io_variant_ids;
        }
    }

    // Phase 2 #06.5 T1: `IoErrorKind` is a sibling enum of 20 unit
    // variants whose tag values match `IoError` 1:1. Returned by
    // `IoError.kind()`; lets user code branch on the discriminant
    // without inspecting the payload. The runtime helper
    // `riven_io_error_kind` (codegen/runtime.rs) allocates a
    // 16-byte tagged-union value matching this enum's layout.
    let io_error_kind_id = r.symbols.define(
        "IoErrorKind".to_string(),
        DefKind::Enum {
            info: EnumInfo {
                generic_params: vec![],
                variants: vec![], // filled below
                derive_traits: vec![],
                opt_out_send: false,
                opt_out_sync: false,
                manual_send: false,
                manual_sync: false,
                const_predicates: vec![],
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes
        .insert_type("IoErrorKind".to_string(), io_error_kind_id);
    r.type_registry
        .insert("IoErrorKind".to_string(), io_error_kind_id);

    let io_error_kind_variants: &[(&str, usize)] = &[
        ("NotFound", 0),
        ("PermissionDenied", 1),
        ("AlreadyExists", 2),
        ("Interrupted", 3),
        ("WouldBlock", 4),
        ("InvalidInput", 5),
        ("UnexpectedEof", 6),
        ("BrokenPipe", 7),
        ("Other", 8),
        ("ConnectionRefused", 9),
        ("ConnectionReset", 10),
        ("ConnectionAborted", 11),
        ("NotConnected", 12),
        ("AddrInUse", 13),
        ("AddrNotAvailable", 14),
        ("InvalidData", 15),
        ("TimedOut", 16),
        ("WriteZero", 17),
        ("Unsupported", 18),
        ("OutOfMemory", 19),
    ];
    let mut io_error_kind_variant_ids: Vec<DefId> =
        Vec::with_capacity(io_error_kind_variants.len());
    for (vname, idx) in io_error_kind_variants {
        let vid = r.symbols.define(
            (*vname).to_string(),
            DefKind::EnumVariant {
                parent: io_error_kind_id,
                variant_idx: *idx,
                kind: VariantDefKind::Unit,
            },
            Visibility::Public,
            span.clone(),
        );
        r.scopes.insert(format!("IoErrorKind.{}", vname), vid);
        io_error_kind_variant_ids.push(vid);
    }
    if let Some(def) = r.symbols.get_mut(io_error_kind_id) {
        if let DefKind::Enum { ref mut info } = def.kind {
            info.variants = io_error_kind_variant_ids;
        }
    }
    let stdin_id = r.symbols.define(
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
                const_predicates: vec![],
                flat_heap_struct: false,
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.type_registry.insert("Stdin".to_string(), stdin_id);
    let stdout_id = r.symbols.define(
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
                const_predicates: vec![],
                flat_heap_struct: false,
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.type_registry.insert("Stdout".to_string(), stdout_id);
    let stderr_id = r.symbols.define(
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
                const_predicates: vec![],
                flat_heap_struct: false,
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.type_registry.insert("Stderr".to_string(), stderr_id);

    // Phase 2 stdlib (#06): `std::fs::Metadata` is a flat heap
    // struct returned by `fs::metadata(path)`. Accessor methods
    // (`len` / `modified` / `is_file` / `is_dir` / `is_symlink`)
    // are wired in typeck (`infer.rs`) and dispatch through the
    // standard `Metadata_{method}` mangled-name pipeline; the
    // runtime helpers live in `runtime.c`. The Class has no
    // public fields — the wire layout is an opaque
    // implementation detail of the runtime.
    let metadata_id = r.symbols.define(
        "Metadata".to_string(),
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
                const_predicates: vec![],
                flat_heap_struct: false,
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes.insert_type("Metadata".to_string(), metadata_id);
    r.type_registry.insert("Metadata".to_string(), metadata_id);

    // Phase 2 stdlib (#06): `std::process::Command` builder class.
    // Constructed via `Command.new(program)`, chained through
    // `.arg/.args/.env/.current_dir` (mutate-in-place, return self),
    // terminated by `.status -> Result[ExitStatus, IoError]` or
    // `.output -> Result[Output, IoError]`. The terminal methods
    // consume the Command — the runtime frees the inner allocations
    // and the spine. A bare un-consumed Command also gets cleaned
    // up via `Command_drop` (registered in `user_drop_classes` —
    // see mir/lower.rs::collect_user_drop_classes for the special-
    // cased built-in entry). Wire layout documented at
    // `riven_command_new` in runtime.c.
    //
    // `Command.spawn -> Child` (async-style handle with
    // `.wait/.kill/.try_wait`) is explicitly DEFERRED to v2 per
    // `docs/prompts/v1/06_phase2_stdlib_io_fmt.md` — v1 ships the
    // blocking terminals only.
    let command_id = r.symbols.define(
        "Command".to_string(),
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
                const_predicates: vec![],
                flat_heap_struct: false,
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes.insert_type("Command".to_string(), command_id);
    r.type_registry.insert("Command".to_string(), command_id);

    // Phase 2 stdlib (#06): `std::process::ExitStatus` wraps the
    // child's exit code as a single int64 (POSIX-shell convention:
    // 0..=255 normal exit; 128+signal on signal termination). The
    // accessor methods are `code -> Int` and `success -> Bool`.
    // Constructed only by the runtime (callers receive it via
    // `Result[ExitStatus, IoError]` from `Command.status` or via
    // `Output.status`); has no user-facing constructor.
    let exit_status_id = r.symbols.define(
        "ExitStatus".to_string(),
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
                const_predicates: vec![],
                flat_heap_struct: false,
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes
        .insert_type("ExitStatus".to_string(), exit_status_id);
    r.type_registry
        .insert("ExitStatus".to_string(), exit_status_id);

    // Phase 2 stdlib (#06): `std::process::Output` carries the
    // captured stdout/stderr of a finished child plus its exit
    // status. Accessors:
    //   `.status -> ExitStatus`  (fresh clone — Output keeps its own)
    //   `.stdout -> String`      (UTF-8 only in v1; raw bytes v2)
    //   `.stderr -> String`
    // Constructed only by the runtime via `Command.output`.
    let output_id = r.symbols.define(
        "Output".to_string(),
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
                const_predicates: vec![],
                flat_heap_struct: false,
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes.insert_type("Output".to_string(), output_id);
    r.type_registry.insert("Output".to_string(), output_id);

    // Phase 2 #06.5 T2: `std::io::File` — owning wrapper over a
    // POSIX fd. Constructed via `File.open / .create / .append /
    // .open_options`; consumed by the standard scope-exit drop
    // pipeline which emits `File_drop(f) + riven_dealloc(f)` —
    // see mir/lower/collect.rs::collect_user_drop_classes for the
    // user_drop_classes registration. Wire layout (8-byte
    // {fd:i32, closed:i32}) documented in runtime.c at `RivenFile`.
    let file_id = r.symbols.define(
        "File".to_string(),
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
                const_predicates: vec![],
                flat_heap_struct: false,
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes.insert_type("File".to_string(), file_id);
    r.type_registry.insert("File".to_string(), file_id);

    // Phase 2 #06.5 T2: `std::io::OpenOptions` — builder for
    // `File.open_options(path, opts)`. POD 8-byte struct (no
    // inner heap), so the standard `riven_dealloc` at scope exit
    // is the entire drop story. Builder methods mutate-in-place
    // and return the same pointer (mirrors Command.arg/...).
    let open_options_id = r.symbols.define(
        "OpenOptions".to_string(),
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
                const_predicates: vec![],
                flat_heap_struct: false,
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes
        .insert_type("OpenOptions".to_string(), open_options_id);
    r.type_registry
        .insert("OpenOptions".to_string(), open_options_id);

    // Phase 2 #06.5 T2: `SeekFrom` — three single-field struct
    // variants used as the second argument of `File.seek`. Each
    // carries a single `offset: Int` field; the codegen lays the
    // value out as a 16-byte tagged enum (`{i32 tag; i32 pad; i64
    // offset}`) which `riven_file_seek` reads directly. Tag
    // values are pinned to match `RIVEN_SEEK_FROM_*` in
    // runtime.c — the `file_class_layout_stability` pin test
    // cross-checks them.
    let seek_from_id = r.symbols.define(
        "SeekFrom".to_string(),
        DefKind::Enum {
            info: EnumInfo {
                generic_params: vec![],
                variants: vec![], // filled below
                derive_traits: vec![],
                opt_out_send: false,
                opt_out_sync: false,
                manual_send: false,
                manual_sync: false,
                const_predicates: vec![],
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes.insert_type("SeekFrom".to_string(), seek_from_id);
    r.type_registry.insert("SeekFrom".to_string(), seek_from_id);
    let seek_from_variants: &[(&str, usize)] = &[("Start", 0), ("End", 1), ("Current", 2)];
    let mut seek_from_variant_ids: Vec<DefId> = Vec::with_capacity(seek_from_variants.len());
    for (vname, idx) in seek_from_variants {
        let vid = r.symbols.define(
            (*vname).to_string(),
            DefKind::EnumVariant {
                parent: seek_from_id,
                variant_idx: *idx,
                kind: VariantDefKind::Struct(vec![("offset".to_string(), Ty::Int)]),
            },
            Visibility::Public,
            span.clone(),
        );
        r.scopes.insert(format!("SeekFrom.{}", vname), vid);
        seek_from_variant_ids.push(vid);
    }
    if let Some(def) = r.symbols.get_mut(seek_from_id) {
        if let DefKind::Enum { ref mut info } = def.kind {
            info.variants = seek_from_variant_ids;
        }
    }

    // Phase 2 #06.5 T4: `std::time::Duration` — scalar-wrapper class
    // over `int64_t nanos`. Pure POD with no inner heap, no resource
    // — the default scope-exit `riven_dealloc` is the entire drop
    // story (NOT added to `user_drop_classes`). 8-byte wire layout
    // documented in runtime.c at `RivenDuration`.
    let duration_id = r.symbols.define(
        "Duration".to_string(),
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
                const_predicates: vec![],
                flat_heap_struct: false,
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes.insert_type("Duration".to_string(), duration_id);
    r.type_registry.insert("Duration".to_string(), duration_id);

    // Phase 2 #06.5 T4: `std::time::Instant` — scalar-wrapper class
    // over `int64_t monotonic_nanos`. Same drop story as Duration.
    let instant_id = r.symbols.define(
        "Instant".to_string(),
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
                const_predicates: vec![],
                flat_heap_struct: false,
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes.insert_type("Instant".to_string(), instant_id);
    r.type_registry.insert("Instant".to_string(), instant_id);

    // Phase 2 #06.5 T5: `std::net::TcpListener` — owning wrapper over
    // a POSIX listening socket fd. Constructed via `TcpListener.bind`;
    // consumed by the standard scope-exit drop pipeline which emits
    // `TcpListener_drop(l) + riven_dealloc(l)` — see
    // mir/lower/collect.rs::collect_user_drop_classes. Wire layout
    // (8-byte {fd:i32, closed:i32}) documented at `RivenTcpListener`
    // in library/runtime/net/tcp.c.
    let tcp_listener_id = r.symbols.define(
        "TcpListener".to_string(),
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
                const_predicates: vec![],
                flat_heap_struct: false,
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes
        .insert_type("TcpListener".to_string(), tcp_listener_id);
    r.type_registry
        .insert("TcpListener".to_string(), tcp_listener_id);

    // Phase 2 #06.5 T5: `std::net::TcpStream` — owning wrapper over a
    // connected POSIX socket fd. Same drop story as TcpListener.
    let tcp_stream_id = r.symbols.define(
        "TcpStream".to_string(),
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
                const_predicates: vec![],
                flat_heap_struct: false,
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes.insert_type("TcpStream".to_string(), tcp_stream_id);
    r.type_registry
        .insert("TcpStream".to_string(), tcp_stream_id);

    // Phase 2 #06.5 T6: `BufReader[R]` / `BufWriter[W]` — generic
    // buffered wrappers parameterized over the closed set {File,
    // TcpStream}. The class registration carries a single type
    // parameter; the static-ctor fast path in
    // `mir/lower/expr/method_call.rs` peeks at the inner argument's
    // type at call-site to pick `_new_file` vs `_new_tcp` runtime
    // symbol. typeck rejects any other R via E0714. Drop pipeline
    // (collect.rs::collect_user_drop_classes) emits `BufReader_drop +
    // riven_dealloc` at scope exit, freeing the 32-byte spine and
    // (for BufWriter) auto-flushing before close.
    let buf_reader_id = r.symbols.define(
        "BufReader".to_string(),
        DefKind::Class {
            info: ClassInfo {
                generic_params: vec![GenericParamInfo::type_param("R".to_string(), vec![])],
                parent: None,
                fields: vec![],
                methods: vec![],
                derive_traits: vec![],
                opt_out_send: false,
                opt_out_sync: false,
                manual_send: false,
                manual_sync: false,
                const_predicates: vec![],
                flat_heap_struct: false,
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes.insert_type("BufReader".to_string(), buf_reader_id);
    r.type_registry
        .insert("BufReader".to_string(), buf_reader_id);

    let buf_writer_id = r.symbols.define(
        "BufWriter".to_string(),
        DefKind::Class {
            info: ClassInfo {
                generic_params: vec![GenericParamInfo::type_param("W".to_string(), vec![])],
                parent: None,
                fields: vec![],
                methods: vec![],
                derive_traits: vec![],
                opt_out_send: false,
                opt_out_sync: false,
                manual_send: false,
                manual_sync: false,
                const_predicates: vec![],
                flat_heap_struct: false,
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes.insert_type("BufWriter".to_string(), buf_writer_id);
    r.type_registry
        .insert("BufWriter".to_string(), buf_writer_id);

    // Phase 2 #06.5 T5: `Shutdown` — three unit variants used as the
    // argument of `TcpStream.shutdown`. Tag values are pinned to match
    // `RIVEN_SHUTDOWN_*` in library/runtime/net/tcp.c — the
    // `shutdown_tag_stability` pin test cross-checks them.
    let shutdown_id = r.symbols.define(
        "Shutdown".to_string(),
        DefKind::Enum {
            info: EnumInfo {
                generic_params: vec![],
                variants: vec![], // filled below
                derive_traits: vec![],
                opt_out_send: false,
                opt_out_sync: false,
                manual_send: false,
                manual_sync: false,
                const_predicates: vec![],
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes.insert_type("Shutdown".to_string(), shutdown_id);
    r.type_registry
        .insert("Shutdown".to_string(), shutdown_id);
    let shutdown_variants: &[(&str, usize)] = &[("Read", 0), ("Write", 1), ("Both", 2)];
    let mut shutdown_variant_ids: Vec<DefId> = Vec::with_capacity(shutdown_variants.len());
    for (vname, idx) in shutdown_variants {
        let vid = r.symbols.define(
            (*vname).to_string(),
            DefKind::EnumVariant {
                parent: shutdown_id,
                variant_idx: *idx,
                kind: VariantDefKind::Unit,
            },
            Visibility::Public,
            span.clone(),
        );
        r.scopes.insert(format!("Shutdown.{}", vname), vid);
        shutdown_variant_ids.push(vid);
    }
    if let Some(def) = r.symbols.get_mut(shutdown_id) {
        if let DefKind::Enum { ref mut info } = def.kind {
            info.variants = shutdown_variant_ids;
        }
    }

    // Phase 2 #06.A1/A3: `std::fmt::Formatter` is the buffer that
    // `Display::fmt` / `Debug::fmt` write into. v1 carries width
    // / alignment / precision metadata as opaque internal fields
    // (no public surface yet) plus a `Vec[Char]`-equivalent
    // backing buffer at the runtime layer (`riven_fmt_*` helpers
    // in `runtime.c`). Phase D wires the constructor + dispatch
    // into `lower_interpolation`.
    let formatter_id = r.symbols.define(
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
                const_predicates: vec![],
                flat_heap_struct: false,
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes.insert_type("Formatter".to_string(), formatter_id);
    r.type_registry
        .insert("Formatter".to_string(), formatter_id);

    // Phase 2 #06.A4: `std::fmt::FmtError` is a unit struct
    // returned by `Formatter::write_str/write_char`. v1 has no
    // variant payload (matches Rust's `std::fmt::Error`) — it's
    // just a sentinel type. Registered as a class so it can
    // appear in `Result[(), FmtError]` return annotations.
    let fmt_error_id = r.symbols.define(
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
                const_predicates: vec![],
                flat_heap_struct: false,
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes.insert_type("FmtError".to_string(), fmt_error_id);
    r.type_registry.insert("FmtError".to_string(), fmt_error_id);

    let context_id = r.symbols.define(
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
                const_predicates: vec![],
                flat_heap_struct: false,
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes.insert_type("Context".to_string(), context_id);
    r.type_registry.insert("Context".to_string(), context_id);
    let waker_id = r.symbols.define(
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
                const_predicates: vec![],
                flat_heap_struct: false,
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes.insert_type("Waker".to_string(), waker_id);
    r.type_registry.insert("Waker".to_string(), waker_id);
    let thread_id_id = r.symbols.define(
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
                const_predicates: vec![],
                flat_heap_struct: false,
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes.insert_type("ThreadId".to_string(), thread_id_id);
    r.type_registry.insert("ThreadId".to_string(), thread_id_id);
    let thread_id_ty = Ty::Class {
        name: "ThreadId".to_string(),
        generic_args: vec![],
    };
    let thread_id_value_id = r.symbols.define(
        "ThreadId".to_string(),
        DefKind::Variable {
            mutable: false,
            ty: thread_id_ty.clone(),
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes.insert("ThreadId".to_string(), thread_id_value_id);

    let thread_id = r.symbols.define(
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
                const_predicates: vec![],
                flat_heap_struct: false,
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes.insert_type("Thread".to_string(), thread_id);
    r.type_registry.insert("Thread".to_string(), thread_id);

    let join_handle_id = r.symbols.define(
        "JoinHandle".to_string(),
        DefKind::Class {
            info: ClassInfo {
                generic_params: vec![GenericParamInfo::type_param(
                    "T".to_string(),
                    vec![MixinRef {
                        name: "Send".to_string(),
                        generic_args: vec![],
                    }],
                )],
                parent: None,
                fields: vec![],
                methods: vec![],
                derive_traits: vec![],
                opt_out_send: false,
                opt_out_sync: false,
                manual_send: false,
                manual_sync: false,
                const_predicates: vec![],
                flat_heap_struct: false,
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes
        .insert_type("JoinHandle".to_string(), join_handle_id);
    r.type_registry
        .insert("JoinHandle".to_string(), join_handle_id);

    let mutex_id = r.symbols.define(
        "Mutex".to_string(),
        DefKind::Class {
            info: ClassInfo {
                generic_params: vec![GenericParamInfo::type_param("T".to_string(), vec![])],
                parent: None,
                fields: vec![],
                methods: vec![],
                derive_traits: vec![],
                opt_out_send: false,
                opt_out_sync: false,
                manual_send: false,
                manual_sync: false,
                const_predicates: vec![],
                flat_heap_struct: false,
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes.insert_type("Mutex".to_string(), mutex_id);
    r.type_registry.insert("Mutex".to_string(), mutex_id);

    let mutex_guard_id = r.symbols.define(
        "MutexGuard".to_string(),
        DefKind::Class {
            info: ClassInfo {
                generic_params: vec![GenericParamInfo::type_param("T".to_string(), vec![])],
                parent: None,
                fields: vec![],
                methods: vec![],
                derive_traits: vec![],
                opt_out_send: false,
                opt_out_sync: false,
                manual_send: false,
                manual_sync: false,
                const_predicates: vec![],
                flat_heap_struct: false,
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes
        .insert_type("MutexGuard".to_string(), mutex_guard_id);
    r.type_registry
        .insert("MutexGuard".to_string(), mutex_guard_id);

    // `SharedSync[T]` (was `Arc[T]` pre-Ruby-naming). The Rust-side
    // class name is preserved as `Arc` for now; the resolve layer
    // accepts both `SharedSync` and the legacy `Arc` spelling as the
    // user-facing name.
    let arc_id = r.symbols.define(
        "Arc".to_string(),
        DefKind::Class {
            info: ClassInfo {
                generic_params: vec![GenericParamInfo::type_param("T".to_string(), vec![])],
                parent: None,
                fields: vec![],
                methods: vec![],
                derive_traits: vec![],
                opt_out_send: false,
                opt_out_sync: false,
                manual_send: false,
                manual_sync: false,
                const_predicates: vec![],
                flat_heap_struct: false,
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes.insert_type("Arc".to_string(), arc_id);
    r.type_registry.insert("Arc".to_string(), arc_id);
    // Ruby-naming alias: `SharedSync` is the canonical name (Arc is
    // retained for backward-compat in scope/registry lookups). The
    // module-path resolver matches by `def.name`, so we register a
    // separate symbol named `SharedSync` with the same `ClassInfo`
    // so `use std.sync.SharedSync` resolves cleanly.
    let shared_sync_id = r.symbols.define(
        "SharedSync".to_string(),
        DefKind::Class {
            info: ClassInfo {
                generic_params: vec![GenericParamInfo::type_param("T".to_string(), vec![])],
                parent: None,
                fields: vec![],
                methods: vec![],
                derive_traits: vec![],
                opt_out_send: false,
                opt_out_sync: false,
                manual_send: false,
                manual_sync: false,
                const_predicates: vec![],
                flat_heap_struct: false,
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes
        .insert_type("SharedSync".to_string(), shared_sync_id);
    r.type_registry
        .insert("SharedSync".to_string(), shared_sync_id);

    let poison_error_id = r.symbols.define(
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
                const_predicates: vec![],
                flat_heap_struct: false,
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes
        .insert_type("PoisonError".to_string(), poison_error_id);
    r.type_registry
        .insert("PoisonError".to_string(), poison_error_id);

    let thread_panic_id = r.symbols.define(
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
                const_predicates: vec![],
                flat_heap_struct: false,
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes
        .insert_type("ThreadPanic".to_string(), thread_panic_id);
    r.type_registry
        .insert("ThreadPanic".to_string(), thread_panic_id);

    // Register a minimal builtin std module tree so `use std.io`
    // resolves before the fuller Tier-1 stdlib lands.
    let io_id = r.symbols.define(
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
                // T1 follow-up: `IoErrorKind` was registered as a
                // type but never added to the `std.io` module's
                // items, so `use std.io.IoErrorKind` failed at
                // resolve. Wired here as part of T2 because the
                // File diagnostic tests are the first consumer of
                // the import. Safe additive change — IoErrorKind
                // already existed as a `type_registry` entry, this
                // just makes module-path resolution find it.
                io_error_kind_id,
                stdin_id,
                stdout_id,
                stderr_id,
                // Phase 2 #06.5 T2: `File` / `OpenOptions` /
                // `SeekFrom` are importable via `use std.io.{...}`.
                // `File` is intentionally re-exported from
                // `std.fs` below too (Rust ships it as
                // `std::fs::File`; we keep both paths so prior
                // examples don't break). `OpenOptions` and
                // `SeekFrom` live only under `std.io`.
                file_id,
                open_options_id,
                seek_from_id,
                // Phase 2 #06.5 T6: BufReader[R] / BufWriter[W] —
                // generic buffered wrappers over File + TcpStream.
                // Importable via `use std.io.BufReader` /
                // `use std.io.BufWriter`.
                buf_reader_id,
                buf_writer_id,
            ],
        },
        Visibility::Public,
        span.clone(),
    );
    let env_id = r.symbols.define(
        "env".to_string(),
        DefKind::Module {
            items: vec![
                builtin_fn_ids["args"],
                builtin_fn_ids["get"],
                // Phase 2 stdlib (#06).
                builtin_fn_ids["vars"],
                builtin_fn_ids["current_dir"],
            ],
        },
        Visibility::Public,
        span.clone(),
    );
    let fs_id = r.symbols.define(
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
                builtin_fn_ids["metadata"],
                // Phase 2 stdlib (#06.5 T3): fs completeness.
                builtin_fn_ids["copy"],
                builtin_fn_ids["remove_dir_all"],
                builtin_fn_ids["canonicalize"],
                builtin_fn_ids["write_atomic"],
                builtin_fn_ids["read_link"],
                builtin_fn_ids["symlink"],
                // Phase 2 #06.5 T2: re-export of `File` for Rust-
                // style `use std.fs.File` paths. The canonical
                // definition is in `std.io` above; this entry just
                // makes both import paths work.
                file_id,
            ],
        },
        Visibility::Public,
        span.clone(),
    );
    let process_id = r.symbols.define(
        "process".to_string(),
        DefKind::Module {
            items: vec![
                builtin_fn_ids["exit"],
                // Phase 2 stdlib (#06): Command builder + its
                // terminal return types.  Importable via
                // `use std.process.{Command, Output, ExitStatus}`.
                // The flat `process_run` free-fn was removed in
                // #06.5 T5.5 — see process.spec.md "Removed".
                command_id,
                output_id,
                exit_status_id,
            ],
        },
        Visibility::Public,
        span.clone(),
    );
    let time_id = r.symbols.define(
        "time".to_string(),
        DefKind::Module {
            items: vec![
                builtin_fn_ids["unix_ns"],
                // Phase 2 stdlib (#06.5 T4): Duration / Instant
                // imported via `use std.time.{Duration, Instant}`.
                // The flat `now_ns` free-fn was removed in
                // #06.5 T5.5 — see time.spec.md "Removed".
                duration_id,
                instant_id,
            ],
        },
        Visibility::Public,
        span.clone(),
    );
    // Phase 2 stdlib (#06.5 T4): the `std.thread` module hosts the
    // typed free-fn `sleep(&Duration) -> ()`. Intentionally kept
    // distinct from `std.sync.Thread`: the latter is the spawn /
    // join surface (class methods), the former is the typed
    // free-function counterpart to `Thread.sleep(int)`. A future
    // refactor may consolidate the two namespaces.
    let thread_module_id = r.symbols.define(
        "thread".to_string(),
        DefKind::Module {
            items: vec![builtin_fn_ids["sleep"]],
        },
        Visibility::Public,
        span.clone(),
    );
    let path_id = r.symbols.define(
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
    // Phase 2 #06.5 T5: std.net is class-only — TcpListener /
    // TcpStream / Shutdown. The flat tcp_* free fns are gone; the C
    // runtime symbols remain linked and back the class methods.
    let net_id = r.symbols.define(
        "net".to_string(),
        DefKind::Module {
            items: vec![tcp_listener_id, tcp_stream_id, shutdown_id],
        },
        Visibility::Public,
        span.clone(),
    );
    let signal_id = r.symbols.define(
        "signal".to_string(),
        DefKind::Module {
            items: vec![
                builtin_fn_ids["signal_install_sigint"],
                builtin_fn_ids["signal_received_sigint"],
            ],
        },
        Visibility::Public,
        span.clone(),
    );
    // Phase 2 #06.5 T8: std::rand — three free fns over the kernel
    // CSPRNG. Importable via `use std.rand.{random_bytes, random_u64,
    // random_fill}`. Class wrapping is intentionally absent — see
    // docs/specs/stdlib/rand.spec.md "Out of scope".
    //
    // Wave 2 (#06.8): the three fns moved to library/std/src/rand.rvn.
    // We still register the `rand` Module namespace here (with empty
    // items) so `std_id` can include it at construction time and
    // `use std.rand.<fn>` keeps tokenising. The items vector is
    // populated by [`Resolver::fixup_bootstrapped_stdlib_modules`]
    // AFTER the bootstrap merge has inserted the FFI fn DefIds into
    // the resolver scope.
    let rand_id = r.symbols.define(
        "rand".to_string(),
        DefKind::Module { items: vec![] },
        Visibility::Public,
        span.clone(),
    );
    let sync_id = r.symbols.define(
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
                shared_sync_id,
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
    let display_trait_id = *r
        .type_registry
        .get("Display")
        .expect("Display trait registered above");
    let debug_trait_id = *r
        .type_registry
        .get("Debug")
        .expect("Debug trait registered above");
    let fmt_id = r.symbols.define(
        "fmt".to_string(),
        DefKind::Module {
            items: vec![display_trait_id, debug_trait_id, formatter_id, fmt_error_id],
        },
        Visibility::Public,
        span.clone(),
    );

    let std_id = r.symbols.define(
        "std".to_string(),
        DefKind::Module {
            items: vec![
                io_id,
                env_id,
                fs_id,
                process_id,
                time_id,
                path_id,
                net_id,
                sync_id,
                fmt_id,
                signal_id,
                // Phase 2 stdlib (#06.5 T4): `std.thread` — typed
                // `sleep(&Duration)` free fn. Sibling of std.sync.
                thread_module_id,
                // Phase 2 stdlib (#06.5 T8): `std.rand` — CSPRNG-
                // backed random_bytes / random_u64 / random_fill.
                rand_id,
            ],
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes.insert_type("std".to_string(), std_id);
    r.type_registry.insert("std".to_string(), std_id);

    // Register type constructors in the value scope so Array.new, String.from, etc. resolve.
    // Per docs/specs/syntax/ruby-naming.spec.md the user-facing names are
    // Array / Map / Set / Shared / SharedSync; the legacy names
    // (Vec, HashMap, HashSet, Rc, Arc) are retained as aliases.
    let type_constructors = [
        (
            "Array",
            Ty::Array(Box::new(Ty::TypeParam {
                name: "T".to_string(),
                bounds: vec![],
            })),
        ),
        // Legacy `Vec[T]` constructor alias — same `Ty::Array` repr.
        (
            "Vec",
            Ty::Array(Box::new(Ty::TypeParam {
                name: "T".to_string(),
                bounds: vec![],
            })),
        ),
        (
            "Map",
            Ty::Map(
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
        // Legacy `HashMap[K, V]` constructor alias — same `Ty::Map` repr.
        (
            "HashMap",
            Ty::Map(
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
        // Legacy `HashSet[T]` alias — same `Ty::Set` representation.
        // Lets `HashSet.new` / `HashSet.with_capacity(_)` resolve in
        // the value scope alongside `Set.new`.
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
        // Phase 2 stdlib (#06.5 T4): Duration / Instant value-scope
        // type constructors. Registering them here lets
        // `Duration.from_secs(5)` / `Instant.now()` resolve the
        // receiver as a class identifier (typeck::infer promotes it
        // to the corresponding class Ty, then the static-ctor fast
        // path in mir/lower/expr/method_call.rs handles dispatch).
        (
            "Duration",
            Ty::Class {
                name: "Duration".to_string(),
                generic_args: vec![],
            },
        ),
        (
            "Instant",
            Ty::Class {
                name: "Instant".to_string(),
                generic_args: vec![],
            },
        ),
        // Phase 2 #06.5 T5: TcpListener / TcpStream value-scope type
        // constructors. Registering them here lets `TcpListener.bind(
        // &addr)` / `TcpStream.connect(&addr)` resolve the receiver as
        // a class identifier (typeck::infer promotes it to the
        // corresponding class Ty, then the static-ctor fast path in
        // mir/lower/expr/method_call.rs dispatches to
        // `TcpListener_bind` / `TcpStream_connect`).
        (
            "TcpListener",
            Ty::Class {
                name: "TcpListener".to_string(),
                generic_args: vec![],
            },
        ),
        (
            "TcpStream",
            Ty::Class {
                name: "TcpStream".to_string(),
                generic_args: vec![],
            },
        ),
        // Phase 2 #06.5 T6: BufReader[R] / BufWriter[W] value-scope
        // type constructors. Registering them here lets
        // `BufReader.new(f)` resolve the receiver to the class Ty
        // (with a fresh inference variable for R, pinned by the inner
        // arg's type at typeck). The static-ctor fast path in
        // mir/lower/expr/method_call.rs picks `_new_file` vs
        // `_new_tcp` from args[0].ty.
        (
            "BufReader",
            Ty::Class {
                name: "BufReader".to_string(),
                generic_args: vec![Ty::TypeParam {
                    name: "R".to_string(),
                    bounds: vec![],
                }],
            },
        ),
        (
            "BufWriter",
            Ty::Class {
                name: "BufWriter".to_string(),
                generic_args: vec![Ty::TypeParam {
                    name: "W".to_string(),
                    bounds: vec![],
                }],
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
        // Ruby-naming spelling for `Arc[T]` — same underlying class.
        (
            "SharedSync",
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
        let id = r.symbols.define(
            name.to_string(),
            DefKind::Variable { mutable: false, ty },
            Visibility::Public,
            span.clone(),
        );
        r.scopes.insert(name.to_string(), id);
    }

    // Register built-in enum types: Option and Result
    // These are needed so bare Ok/Err/Some/None resolve globally.

    // Option enum
    let option_id = r.symbols.define(
        "Option".to_string(),
        DefKind::Enum {
            info: EnumInfo {
                generic_params: vec![GenericParamInfo::type_param("T".to_string(), vec![])],
                variants: vec![], // will be filled below
                derive_traits: vec![],
                opt_out_send: false,
                opt_out_sync: false,
                manual_send: false,
                manual_sync: false,
                const_predicates: vec![],
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes.insert_type("Option".to_string(), option_id);
    r.type_registry.insert("Option".to_string(), option_id);

    // None = tag 0, Some = tag 1 (matches runtime convention:
    // riven_vec_get_opt, riven_option_unwrap_or, inline_find, etc.)
    let none_id = r.symbols.define(
        "None".to_string(),
        DefKind::EnumVariant {
            parent: option_id,
            variant_idx: 0,
            kind: VariantDefKind::Unit,
        },
        Visibility::Public,
        span.clone(),
    );
    let some_id = r.symbols.define(
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
    r.scopes.insert("Option.Some".to_string(), some_id);
    r.scopes.insert("Option.None".to_string(), none_id);
    r.scopes.insert("Some".to_string(), some_id);
    r.scopes.insert("None".to_string(), none_id);
    // Also register bare names that the parser generates with empty type_path: ".Some", ".None"
    r.scopes.insert(".Some".to_string(), some_id);
    r.scopes.insert(".None".to_string(), none_id);

    // Update Option enum with variant DefIds
    if let Some(opt_def) = r.symbols.get_mut(option_id) {
        if let DefKind::Enum { ref mut info } = opt_def.kind {
            info.variants = vec![none_id, some_id];
        }
    }

    // Result enum
    let result_id = r.symbols.define(
        "Result".to_string(),
        DefKind::Enum {
            info: EnumInfo {
                generic_params: vec![
                    GenericParamInfo::type_param("T".to_string(), vec![]),
                    GenericParamInfo::type_param("E".to_string(), vec![]),
                ],
                variants: vec![], // will be filled below
                derive_traits: vec![],
                opt_out_send: false,
                opt_out_sync: false,
                manual_send: false,
                manual_sync: false,
                const_predicates: vec![],
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes.insert_type("Result".to_string(), result_id);
    r.type_registry.insert("Result".to_string(), result_id);

    let ok_id = r.symbols.define(
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
    let err_id = r.symbols.define(
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
    r.scopes.insert("Result.Ok".to_string(), ok_id);
    r.scopes.insert("Result.Err".to_string(), err_id);
    r.scopes.insert("Ok".to_string(), ok_id);
    r.scopes.insert("Err".to_string(), err_id);
    // Also register bare names that the parser generates with empty type_path: ".Ok", ".Err"
    r.scopes.insert(".Ok".to_string(), ok_id);
    r.scopes.insert(".Err".to_string(), err_id);

    // Update Result enum with variant DefIds
    if let Some(res_def) = r.symbols.get_mut(result_id) {
        if let DefKind::Enum { ref mut info } = res_def.kind {
            info.variants = vec![ok_id, err_id];
        }
    }

    let poll_id = r.symbols.define(
        "Poll".to_string(),
        DefKind::Enum {
            info: EnumInfo {
                generic_params: vec![GenericParamInfo::type_param("T".to_string(), vec![])],
                variants: vec![],
                derive_traits: vec![],
                opt_out_send: false,
                opt_out_sync: false,
                manual_send: false,
                manual_sync: false,
                const_predicates: vec![],
            },
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes.insert_type("Poll".to_string(), poll_id);
    r.type_registry.insert("Poll".to_string(), poll_id);

    let ready_id = r.symbols.define(
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
    let pending_id = r.symbols.define(
        "Pending".to_string(),
        DefKind::EnumVariant {
            parent: poll_id,
            variant_idx: 1,
            kind: VariantDefKind::Unit,
        },
        Visibility::Public,
        span.clone(),
    );
    r.scopes.insert("Poll.Ready".to_string(), ready_id);
    r.scopes.insert("Poll.Pending".to_string(), pending_id);
    r.scopes.insert("Ready".to_string(), ready_id);
    r.scopes.insert("Pending".to_string(), pending_id);

    if let Some(poll_def) = r.symbols.get_mut(poll_id) {
        if let DefKind::Enum { ref mut info } = poll_def.kind {
            info.variants = vec![ready_id, pending_id];
        }
    }

    // Register super as a built-in function (for parent class constructor calls)
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
