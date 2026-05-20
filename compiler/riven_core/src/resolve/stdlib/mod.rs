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

use crate::hir::types::Ty;
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

    // Phase D-3 of #06.95: the 16 builtin mixins (Displayable / Error
    // / Comparable / Iterable / Copy / Clone / Send / Sync / PartialEq
    // / Eq / Hash / Default / Ord / PartialOrd / Drop / Future /
    // Into[T]) moved to `library/std/core/src/lib.rvn` as self-hosted
    // `mixin Foo` declarations. The bootstrap merge picks them up as
    // `DefKind::Trait` entries — same shape the Rust registrations
    // produced, just with `core.rvn` as the source of truth.
    //
    // The `Hash → Hashable` deprecation alias is re-established by
    // `fixup_bootstrapped_stdlib_modules` via its TYPE_ALIASES table
    // (Hash is also a TRAIT name; Hashable is the Ruby-naming
    // canonical mixin in library/std/hash/src/lib.rvn).

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
    // fns to library/std/io/src/lib.rvn where the return types are spelled
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
    // moved fs.metadata to library/std/fs/src/lib.rvn where the signature
    // is spelled directly; the alias is no longer needed.
    // `env_var_error_ty = io_error_ty.clone()` was here — used by the
    // pre-migration `("get", ..., Result[String, EnvVarError])` entry.
    // Wave 2 (#06.8) moved env.get to library/std/env/src/lib.rvn so the
    // alias is no longer needed; the .rvn signature spells the Result
    // out as `Result[String, IoError]` directly.

    // Phase D of #06.95 deleted the last three Rust-side builtin
    // free fns (`sleep`, `signal_install_sigint`,
    // `signal_received_sigint`). Their .rvn equivalents live in
    // `library/std/sync/src/lib.rvn` — `Thread.sleep` / `Signal.*`
    // class methods plus bare-fn transition shims for back-compat.
    // Every other historical builtin (io / env / fs / process /
    // time / path / rand entries) migrated during Wave 2 (#06.8).
    // The `builtin_fn_ids` map is gone; modules below start with
    // empty `items` and `fixup_bootstrapped_stdlib_modules`
    // populates them from the bootstrap-loaded prelude scope.

    // IoError tagged enum + IoErrorKind sibling enum were here.
    // Wave 2 (#06.8) followup moved BOTH to library/std/io/src/lib.rvn.
    // The variant-tag stability contract against
    // `RIVEN_IO_ERROR_*` in library/std/io/runtime/io_error.c is now
    // pinned by io_error_tag_stability scanning the .rvn enum body
    // (each variant's tag = its zero-based position).
    // Stdin / Stdout / Stderr class shells were here. Wave 2 (#06.8)
    // moved them to library/std/io/src/lib.rvn. The bootstrap merge
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
    // library/std/fs/src/lib.rvn as `class Metadata end`. Accessor
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
    // (#06.8) moved all three to library/std/process/src/lib.rvn as bare
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
    // moved both to library/std/io/src/lib.rvn. Methods still flow
    // through the static-ctor + runtime_table dispatch until T#20.

    // SeekFrom enum was here. Wave 2 (#06.8) migrated to
    // library/std/io/src/lib.rvn (`enum SeekFrom { Start(offset: Int),
    // End(offset: Int), Current(offset: Int) }`). The variant order
    // contract against RIVEN_SEEK_FROM_* in library/std/io/runtime/file.c
    // is now pinned by file_class_layout_stability scanning the
    // .rvn enum body.

    // Duration / Instant class shells were here. Wave 2 (#06.8) moved
    // both to library/std/time/src/lib.rvn as bare `class Foo end` bodies.
    // Methods (Duration.from_secs, Instant.now, …) still flow through
    // the static-ctor + runtime_table dispatch until T#20 lands.

    // TcpListener / TcpStream class shells were here. Wave 2 (#06.8)
    // moved both to library/std/net/src/lib.rvn as bare `class Foo end`
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
    // (#06.8) moved both to library/std/io/src/lib.rvn as
    // `class BufReader[R] end` / `class BufWriter[W] end`. The
    // static-ctor fast path's inner-type suffix-pick
    // (`_new_file` vs `_new_tcp`) still fires from
    // mir/lower/expr/method_call.rs — that part of the dispatch will
    // move when T#20 + T#21 land.

    // Shutdown enum was here (Read=0, Write=1, Both=2) — Wave 2
    // (#06.8) migrated to library/std/net/src/lib.rvn. The variant order
    // remains the load-bearing contract against
    // `RIVEN_SHUTDOWN_{READ,WRITE,BOTH}` in library/std/net/runtime/tcp.c;
    // the `shutdown_tag_stability` pin test scans the .rvn file now.

    // Phase 2 #06.A1/A3: `std::fmt::Formatter` is the buffer that
    // `Display::fmt` / `Debug::fmt` write into. v1 carries width
    // / alignment / precision metadata as opaque internal fields
    // (no public surface yet) plus a `Vec[Char]`-equivalent
    // backing buffer at the runtime layer (`riven_fmt_*` helpers
    // in `runtime.c`). Phase D wires the constructor + dispatch
    // into `lower_interpolation`.
    // Formatter / FmtError placeholder class registrations were here.
    // Wave 2 (#06.8) moved both to library/std/fmt/src/lib.rvn as bare
    // `class Foo end` bodies (no fields, no methods — same surface
    // the Rust registrations had). The bootstrap merge handles
    // `r.scopes.insert_type` and `r.type_registry.insert` for class
    // items symmetrically with user code, so no extra plumbing is
    // needed here beyond the FIXUPS entry that re-adds the four
    // bootstrap-loaded DefIds to the fmt module's items list.

    // 9 std.sync class shells (Context, Waker, ThreadId, Thread,
    // JoinHandle[T: Send], Mutex[T], MutexGuard[T], Arc[T],
    // PoisonError, ThreadPanic) were here. Wave 2 (#06.8) moved
    // all of them to library/std/sync/src/lib.rvn as bare
    // `class Foo end` / `class Foo[T] end` bodies preserving the
    // generic-parameter shapes (JoinHandle keeps the `T: Send`
    // bound). Methods still flow through the runtime_table
    // mangled-name dispatch until T#21 lands.
    //
    // Ruby-naming (TEC-13 / §10a): `SharedSync` is the canonical
    // name and lives in library/std/sync/src/lib.rvn. `Arc[T]` is
    // preserved here as a backward-compat alias class whose
    // type_constructors Variable below is typed
    // `Ty::Class { name: "SharedSync" }` so `Arc.new(5)` returns
    // a SharedSync-typed value and downstream `SharedSync[Int]`
    // annotations match.
    //
    // The `ThreadId` value-scope alias (a `DefKind::Variable` that
    // lets `ThreadId` itself appear as a sentinel value, not just
    // as a type) stays here as a one-line shim — its type field
    // resolves "ThreadId" by name, which the bootstrap-loaded
    // class registration satisfies.
    // Phase D-2 of #06.95: `Arc[T]` (backward-compat alias for
    // SharedSync[T]) and the `ThreadId` value-scope shim moved to
    // `library/std/sync/src/lib.rvn`. Arc is now a bootstrap-loaded
    // `class Arc[T] end` shell. ThreadId stays a `class ThreadId end`
    // bootstrap-loaded class; the value-scope `DefKind::Variable`
    // shim was deleted because fixtures only reference `ThreadId` as
    // a TYPE (`let x: ThreadId = ...`) — no sentinel-value usage
    // remains in tree.

    // Task #17 (Phase D-4 of #06.95 — auto-derived): the std-submodule
    // list is now derived from `bootstrap::BOOTSTRAP_FILES` rather than
    // hand-maintained here. Each package entry in BOOTSTRAP_FILES
    // (basename of `<pkg>/src/lib.rvn`) becomes one `std.<pkg>` empty
    // `DefKind::Module`; `auto_populate_std_submodules_from_packages`
    // then fills the items list from the matching package after the
    // bootstrap merge. Adding a new stdlib package = one line in
    // BOOTSTRAP_FILES; `use std.<new>.X` resolves automatically.
    //
    // SYNTHETIC_STD_SUBMODULES carries the namespaces that do NOT have
    // a same-named bootstrap package — `thread` and `signal` re-export
    // sync.rvn's bare-fn shims (`sleep`, `signal_install_sigint`,
    // `signal_received_sigint`) under the legacy import paths
    // `use std.thread.sleep` / `use std.signal.*`. The resolver's
    // auto-population skips them silently (no matching package in
    // `bootstrap_auto_packages`) and they stay empty modules — fine
    // for namespace tokenisation; callers go through the global-prelude
    // entries for the actual fn resolution.
    //
    // Ordering: bootstrap-package basenames first (in BOOTSTRAP_FILES
    // order), then synthetic. Duplicates between the two sets are
    // dropped (the synthetic list wins only if the name is unique).
    const SYNTHETIC_STD_SUBMODULES: &[&str] = &["thread", "signal"];
    let bootstrap_pkg_names = crate::resolve::bootstrap::bootstrap_package_names();
    let mut submodule_names: Vec<&str> = Vec::with_capacity(
        bootstrap_pkg_names.len() + SYNTHETIC_STD_SUBMODULES.len(),
    );
    for name in &bootstrap_pkg_names {
        if !submodule_names.contains(name) {
            submodule_names.push(*name);
        }
    }
    for name in SYNTHETIC_STD_SUBMODULES {
        if !submodule_names.contains(name) {
            submodule_names.push(*name);
        }
    }
    let std_items: Vec<_> = submodule_names
        .iter()
        .map(|name| {
            r.symbols.define(
                (*name).to_string(),
                DefKind::Module { items: vec![] },
                Visibility::Public,
                span.clone(),
            )
        })
        .collect();
    let std_id = r.symbols.define(
        "std".to_string(),
        DefKind::Module { items: std_items },
        Visibility::Public,
        span.clone(),
    );
    r.scopes.insert_type("std".to_string(), std_id);
    r.type_registry.insert("std".to_string(), std_id);

    // Phase D-5 of #06.95: collapse the previously hand-spelled
    // type_constructors table (~175 LOC, one tuple per type) into a
    // small data-driven structure. The table registers each type
    // NAME in the value scope as a `DefKind::Variable` so call sites
    // like `Array.new(...)` / `String.from(...)` / `Command.new(...)`
    // resolve the receiver to a class-id-like sentinel value. The
    // typeck path then promotes the Variable to the corresponding
    // `Ty::Class` / `Ty::Array` / … and the static-ctor fast path in
    // `mir/lower/expr/method_call.rs` handles dispatch.
    //
    // Three shape categories:
    //   - Container builtins (`Array`/`Vec`, `Map`/`HashMap`,
    //     `Set`/`HashSet`) carry a primitive Ty.
    //   - `String` carries `Ty::String`.
    //   - Every other class name carries `Ty::Class { name, ... }`
    //     with one or zero generic_args.
    //
    // Future cleanup (separate prompt): teach
    // `register_top_level_type_with_ffi`'s Class arm to insert into
    // the value scope alongside the type scope, eliminating the
    // need for the SIMPLE_CLASS_CTORS list entirely (the class .rvn
    // declaration would self-register both bindings).
    let array_ty = Ty::Array(Box::new(Ty::TypeParam {
        name: "T".to_string(),
        bounds: vec![],
    }));
    let map_ty = Ty::Map(
        Box::new(Ty::TypeParam {
            name: "K".to_string(),
            bounds: vec![],
        }),
        Box::new(Ty::TypeParam {
            name: "V".to_string(),
            bounds: vec![],
        }),
    );
    let set_ty = Ty::Set(Box::new(Ty::TypeParam {
        name: "T".to_string(),
        bounds: vec![],
    }));
    // (name, generic_param_names) — classes with optional one-arg
    // generics. `Arc` is an alias for `SharedSync`, so its
    // value-scope Variable carries the SharedSync type identity.
    const SIMPLE_CLASS_CTORS: &[(&str, &[&str])] = &[
        ("Thread", &[]),
        ("Duration", &[]),
        ("Instant", &[]),
        ("TcpListener", &[]),
        ("TcpStream", &[]),
        ("BufReader", &["R"]),
        ("BufWriter", &["W"]),
        ("Mutex", &["T"]),
        ("SharedSync", &["T"]),
    ];
    fn class_ty(name: &str, gens: &[&str]) -> Ty {
        Ty::Class {
            name: name.to_string(),
            generic_args: gens
                .iter()
                .map(|g| Ty::TypeParam {
                    name: g.to_string(),
                    bounds: vec![],
                })
                .collect(),
        }
    }
    let type_constructors: Vec<(&str, Ty)> = {
        let mut v: Vec<(&str, Ty)> = vec![
            ("Array", array_ty.clone()),
            ("Vec", array_ty),
            ("Map", map_ty.clone()),
            ("HashMap", map_ty),
            ("Set", set_ty.clone()),
            ("HashSet", set_ty),
            ("String", Ty::String),
        ];
        for (name, gens) in SIMPLE_CLASS_CTORS {
            v.push((name, class_ty(name, gens)));
        }
        v
    };
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

    // Poll[T] migrated to library/std/future/src/lib.rvn by the async
    // sub-phase 1 (docs/specs/stdlib/async.spec.md B2 + B11). The
    // bootstrap merge picks it up as a `DefKind::Enum` with the
    // same Ready / Pending variant order this Rust block historically
    // produced. Tag layout (Ready = 0, Pending = 1) is pinned by
    // `poll_tag_layout_stability` in tests/async_surface.rs.

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
