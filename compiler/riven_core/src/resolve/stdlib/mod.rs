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
        // ("Hashable", vec!["hash_code"]) — migrated to library/std/src/hash.rvn (#06.8 Wave 2).
        // The `Hash → Hashable` deprecation alias is re-established
        // by Resolver::fixup_bootstrapped_stdlib_modules after the
        // bootstrap merge.
        ("Iterable", vec![]),
        // ("Iterator", vec!["next"]) — migrated to library/std/src/iter.rvn (#06.8 Wave 2).
        // ("FromIterator", vec!["from_iter"]) — migrated to library/std/src/iter.rvn (#06.8 Wave 2).
        ("Copy", vec![]),
        ("Clone", vec!["clone"]),
        ("Send", vec![]),
        ("Sync", vec![]),
        // Display / Debug were here. Wave 2 (#06.8) moved both to
        // library/std/src/fmt.rvn. The fmt_id Module's items list
        // (formerly [display_trait_id, debug_trait_id, ...]) is
        // populated by fixup_bootstrapped_stdlib_modules instead.
        ("PartialEq", vec!["eq"]),
        ("Eq", vec![]),
        ("Hash", vec!["hash"]),
        ("Default", vec!["default"]),
        ("Ord", vec!["cmp"]),
        ("PartialOrd", vec!["partial_cmp"]),
        ("Drop", vec!["drop"]),
    ];

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

    // Deprecated alias `Hash → Hashable` was here. Wave 2 (#06.8)
    // moved Hashable to library/std/src/hash.rvn, so the alias is
    // re-established by Resolver::fixup_bootstrapped_stdlib_modules
    // once the bootstrap merge has inserted Hashable into the prelude
    // type scope.

    // Register built-in functions. `IoError` is registered below
    // as `DefKind::Enum`, so the type reference here uses
    // `Ty::Enum` to keep the symbol-table kind and the type kind
    // in sync. Mismatching the two (Ty::Class vs DefKind::Enum)
    // breaks `enum_with_derive_trait` lookups and the codegen
    // enum-tag dispatch.
    // `io_error_ty` alias was here — used by the pre-migration io /
    // env / fs / process / rand / time builtin_fn entries that
    // returned `Result[_, IoError]`. Wave 2 (#06.8) moved all those
    // entries into their respective .rvn files where the type is
    // spelled directly. The alias is gone; IoError stays a Ty::Enum
    // through the type registry like every other enum.
    // stdin_ty / stdout_ty / stderr_ty aliases were here — used by
    // the pre-migration `("stdin", ..., Stdin)` etc. builtin_fn
    // entries. Wave 2 (#06.8) moved the stdin / stdout / stderr free
    // fns to library/std/src/io.rvn where the return types are spelled
    // directly; the aliases are no longer needed.
    // Phase 2 stdlib (#06): `fs::metadata` returns
    // `Result[Metadata, IoError]`. `Metadata` is a flat heap struct
    // exposing `len`/`modified`/`is_file`/`is_dir`/`is_symlink`
    // accessors; see `riven_fs_metadata` in `runtime.c` for the
    // on-wire layout. Registered as a Class below so it can appear
    // in Result return annotations and dispatch via the standard
    // `{Type}_{method}` mangled-name pipeline.
    // `metadata_ty` was here — used by the pre-migration `("metadata",
    // ..., Result[Metadata, IoError])` builtin_fn entry. Wave 2 (#06.8)
    // moved fs.metadata to library/std/src/fs.rvn where the signature
    // is spelled directly; the alias is no longer needed.
    // `env_var_error_ty = io_error_ty.clone()` was here — used by the
    // pre-migration `("get", ..., Result[String, EnvVarError])` entry.
    // Wave 2 (#06.8) moved env.get to library/std/src/env.rvn so the
    // alias is no longer needed; the .rvn signature spells the Result
    // out as `Result[String, IoError]` directly.

    let builtin_fns = [
        // std::io free fns (puts, eputs, print, println, eprintln,
        // read_line, stdin, stdout, stderr) were here. Wave 2 (#06.8)
        // migrated all nine to library/std/src/io.rvn as a
        // `lib "riven_runtime"` block. The println / eprintln aliases
        // (sharing the riven_puts / riven_eputs C symbols with
        // puts / eputs) are preserved verbatim in the .rvn lib block.
        // std::env entries (args, get, vars, current_dir) were here.
        //
        // Wave 2 (#06.8): migrated to `library/std/src/env.rvn`.
        // The `std.env` module namespace is still assembled below
        // (with empty items) and populated by
        // `fixup_bootstrapped_stdlib_modules` after the bootstrap
        // merge so `use std.env.{...}` keeps tokenising.
        // std::fs free fns (read_to_string, write, exists, is_file,
        // is_dir, read_dir, metadata, remove_file, create_dir,
        // create_dir_all, rename, copy, remove_dir_all, canonicalize,
        // write_atomic, read_link, symlink) were here. Wave 2 (#06.8)
        // migrated all seventeen to library/std/src/fs.rvn as a
        // `lib "riven_runtime"` block. The C symbols (`riven_fs_*`)
        // are unchanged.
        // `exit(code) -> Never` was here. Wave 2 (#06.8) migrated to
        // library/std/src/process.rvn (`lib "riven_runtime" def exit
        // as "riven_process_exit"(code: Int) -> Never`). The FFI alias
        // map populated during MIR lowering rewrites the callee to the
        // C symbol BEFORE codegen consults runtime_table.
        // std::time — Phase 3 / #06.5. `unix_ns` is wall-clock
        // (nanoseconds since 1970-01-01 UTC) and stays exposed as a
        // bare Int-returning free-fn until a `SystemTime` class lands.
        // The previously-exposed monotonic `now_ns()` free-fn was
        // removed in #06.5 T5.5 once `Instant.now` + `Instant.elapsed`
        // covered every use case. The C symbol `riven_time_now_ns` is
        // still linked from the runtime (it is the implementation
        // behind `riven_instant_now`); it just is not reachable from
        // Riven user code.
        // `unix_ns` was here — Wave 2 (#06.8) migrated to
        // library/std/src/time.rvn (lib block with c_symbol alias
        // to riven_time_unix_ns).
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
        // std::path — Phase 3 was here.
        //
        // Wave 2 (#06.8): migrated to `library/std/src/path.rvn`.
        // The five `path_*` free fns + their `riven_path_*` aliases
        // now live in the .rvn file as a `lib "riven_runtime"` block.
        // The `std.path` module namespace is still assembled below
        // (with empty items) and populated by
        // `fixup_bootstrapped_stdlib_modules` after the bootstrap
        // merge so `use std.path.{...}` keeps tokenising.
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

    // IoError tagged enum + IoErrorKind sibling enum were here.
    // Wave 2 (#06.8) followup moved BOTH to library/std/src/io.rvn.
    // The variant-tag stability contract against
    // `RIVEN_IO_ERROR_*` in library/runtime/io/io_error.c is now
    // pinned by io_error_tag_stability scanning the .rvn enum body
    // (each variant's tag = its zero-based position).
    // Stdin / Stdout / Stderr class shells were here. Wave 2 (#06.8)
    // moved them to library/std/src/io.rvn. The bootstrap merge
    // inserts each into both the type scope and type registry, which
    // is permissively MORE than the pre-migration registrations did
    // (those only inserted into type_registry).

    // Phase 2 stdlib (#06): `std::fs::Metadata` is a flat heap
    // struct returned by `fs::metadata(path)`. Accessor methods
    // (`len` / `modified` / `is_file` / `is_dir` / `is_symlink`)
    // are wired in typeck (`infer.rs`) and dispatch through the
    // standard `Metadata_{method}` mangled-name pipeline; the
    // runtime helpers live in `runtime.c`. The Class has no
    // public fields — the wire layout is an opaque
    // implementation detail of the runtime.
    // Metadata class shell was here. Wave 2 (#06.8) moved it to
    // library/std/src/fs.rvn as `class Metadata end`. Accessor
    // methods still flow through runtime_table mangled-name dispatch
    // until T#21 lands.

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
    // Command / ExitStatus / Output class shells were here. Wave 2
    // (#06.8) moved all three to library/std/src/process.rvn as bare
    // `class Foo end` bodies. Methods still flow through the
    // static-ctor + runtime_table dispatch until T#20 lands. The
    // bootstrap merge handles `insert_type` / `type_registry.insert`
    // symmetrically with user code.

    // Phase 2 #06.5 T2: `std::io::File` — owning wrapper over a
    // POSIX fd. Constructed via `File.open / .create / .append /
    // .open_options`; consumed by the standard scope-exit drop
    // pipeline which emits `File_drop(f) + riven_dealloc(f)` —
    // see mir/lower/collect.rs::collect_user_drop_classes for the
    // user_drop_classes registration. Wire layout (8-byte
    // {fd:i32, closed:i32}) documented in runtime.c at `RivenFile`.
    // File and OpenOptions class shells were here. Wave 2 (#06.8)
    // moved both to library/std/src/io.rvn. Methods still flow
    // through the static-ctor + runtime_table dispatch until T#20.

    // SeekFrom enum was here. Wave 2 (#06.8) migrated to
    // library/std/src/io.rvn (`enum SeekFrom { Start(offset: Int),
    // End(offset: Int), Current(offset: Int) }`). The variant order
    // contract against RIVEN_SEEK_FROM_* in library/runtime/io/file.c
    // is now pinned by file_class_layout_stability scanning the
    // .rvn enum body.

    // Duration / Instant class shells were here. Wave 2 (#06.8) moved
    // both to library/std/src/time.rvn as bare `class Foo end` bodies.
    // Methods (Duration.from_secs, Instant.now, …) still flow through
    // the static-ctor + runtime_table dispatch until T#20 lands.

    // TcpListener / TcpStream class shells were here. Wave 2 (#06.8)
    // moved both to library/std/src/net.rvn as bare `class Foo end`
    // bodies — the bootstrap merge handles `insert_type` and
    // `type_registry.insert` symmetrically with user code, so no
    // explicit re-registration is needed on the Rust side. Methods
    // still flow through the static-ctor + runtime_table dispatch
    // (T#20 will move them through the FFI alias path).

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
    // BufReader[R] / BufWriter[W] class shells were here. Wave 2
    // (#06.8) moved both to library/std/src/io.rvn as
    // `class BufReader[R] end` / `class BufWriter[W] end`. The
    // static-ctor fast path's inner-type suffix-pick
    // (`_new_file` vs `_new_tcp`) still fires from
    // mir/lower/expr/method_call.rs — that part of the dispatch will
    // move when T#20 + T#21 land.

    // Shutdown enum was here (Read=0, Write=1, Both=2) — Wave 2
    // (#06.8) migrated to library/std/src/net.rvn. The variant order
    // remains the load-bearing contract against
    // `RIVEN_SHUTDOWN_{READ,WRITE,BOTH}` in library/runtime/net/tcp.c;
    // the `shutdown_tag_stability` pin test scans the .rvn file now.

    // Phase 2 #06.A1/A3: `std::fmt::Formatter` is the buffer that
    // `Display::fmt` / `Debug::fmt` write into. v1 carries width
    // / alignment / precision metadata as opaque internal fields
    // (no public surface yet) plus a `Vec[Char]`-equivalent
    // backing buffer at the runtime layer (`riven_fmt_*` helpers
    // in `runtime.c`). Phase D wires the constructor + dispatch
    // into `lower_interpolation`.
    // Formatter / FmtError placeholder class registrations were here.
    // Wave 2 (#06.8) moved both to library/std/src/fmt.rvn as bare
    // `class Foo end` bodies (no fields, no methods — same surface
    // the Rust registrations had). The bootstrap merge handles
    // `r.scopes.insert_type` and `r.type_registry.insert` for class
    // items symmetrically with user code, so no extra plumbing is
    // needed here beyond the FIXUPS entry that re-adds the four
    // bootstrap-loaded DefIds to the fmt module's items list.

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
    // Wave 2 (#06.8): 9 io free fns + 7 io class shells (Stdin,
    // Stdout, Stderr, File, OpenOptions, BufReader, BufWriter) moved
    // to library/std/src/io.rvn. IoError / IoErrorKind / SeekFrom
    // stay in Rust for now (each is a tagged enum with a pinned
    // tag-stability contract — separate migration commit).
    // io_id starts with empty items; fixup_bootstrapped_stdlib_modules
    // populates them.
    let io_id = r.symbols.define(
        "io".to_string(),
        DefKind::Module { items: vec![] },
        Visibility::Public,
        span.clone(),
    );
    // Wave 2 (#06.8): see rand_id / path_id above for the empty-items
    // pattern. The four env free fns are populated by
    // [`Resolver::fixup_bootstrapped_stdlib_modules`] after the
    // bootstrap merge loads `library/std/src/env.rvn`.
    let env_id = r.symbols.define(
        "env".to_string(),
        DefKind::Module { items: vec![] },
        Visibility::Public,
        span.clone(),
    );
    // Wave 2 (#06.8): all seventeen fs free fns + Metadata moved to
    // library/std/src/fs.rvn. fs_id starts with empty items;
    // fixup_bootstrapped_stdlib_modules populates them. The `File`
    // re-export entry (for `use std.fs.File`) is preserved via the
    // FIXUPS row — "File" looks up in the type scope at fixup time
    // (Rust-registered today via the unmigrated io section; .rvn
    // once io migrates).
    let fs_id = r.symbols.define(
        "fs".to_string(),
        DefKind::Module { items: vec![] },
        Visibility::Public,
        span.clone(),
    );
    // Wave 2 (#06.8): exit + Command + Output + ExitStatus all moved
    // to library/std/src/process.rvn. process_id starts with empty
    // items; fixup_bootstrapped_stdlib_modules populates them.
    let process_id = r.symbols.define(
        "process".to_string(),
        DefKind::Module { items: vec![] },
        Visibility::Public,
        span.clone(),
    );
    // Wave 2 (#06.8): unix_ns + Duration + Instant all moved to
    // library/std/src/time.rvn. time_id starts with empty items;
    // fixup_bootstrapped_stdlib_modules populates them.
    let time_id = r.symbols.define(
        "time".to_string(),
        DefKind::Module { items: vec![] },
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
    // Wave 2 (#06.8): see rand_id above for the empty-items pattern.
    // The five `path_*` fn DefIds are populated by
    // [`Resolver::fixup_bootstrapped_stdlib_modules`] after the
    // bootstrap merge loads `library/std/src/path.rvn`.
    let path_id = r.symbols.define(
        "path".to_string(),
        DefKind::Module { items: vec![] },
        Visibility::Public,
        span.clone(),
    );
    // Phase 2 #06.5 T5: std.net is class-only — TcpListener /
    // TcpStream / Shutdown. The flat tcp_* free fns are gone; the C
    // runtime symbols remain linked and back the class methods.
    // Wave 2 (#06.8): TcpListener / TcpStream / Shutdown moved to
    // library/std/src/net.rvn. net_id starts with empty items;
    // fixup_bootstrapped_stdlib_modules populates them via type-scope
    // lookup after the bootstrap merge.
    let net_id = r.symbols.define(
        "net".to_string(),
        DefKind::Module { items: vec![] },
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
    // Wave 2 (#06.8): Display, Debug, Formatter, FmtError all moved
    // to library/std/src/fmt.rvn. fmt_id Module starts with empty
    // items; fixup_bootstrapped_stdlib_modules populates the four
    // DefIds via type-scope lookup after the bootstrap merge.
    let fmt_id = r.symbols.define(
        "fmt".to_string(),
        DefKind::Module { items: vec![] },
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
