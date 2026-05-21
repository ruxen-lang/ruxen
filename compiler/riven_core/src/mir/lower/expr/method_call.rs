//! Method call lowering — `expr.method(args)` and `Class.method(args)`.
//!
//! ## Dispatch structure
//!
//! Lowering walks three logical branches in order:
//!
//! 1. **Static-ctor fast path** (`is_collection_ctor`, ~lines 33-365).
//!    Fires when the call is `.new` / `.with_capacity` / specific
//!    builtin static names. The branch FIRST consults
//!    `lookup_ffi_alias` (the §B1 single-entry alias lookup) — if a
//!    `<Base>_<method>` alias is registered, it dispatches directly
//!    with the user's explicit args, no synthetic `self`. Falls back
//!    to a hardcoded class-name list with specialised remapping
//!    (Vec→Hash legacy names, BufReader suffix routing), then
//!    String/Struct special cases, and finally the
//!    `{ClassName}_init(self, args)` synthesis for any user class
//!    with a `def init` body.
//! 2. **`is_static` decision** (~line 667). For non-`.new` calls the
//!    receiver type drives "static vs instance" via four signals:
//!    builtin static methods, user-defined `def self.X`, the
//!    `default` constant for `T: Default`, and "receiver is a class
//!    identifier" (e.g. `JoinHandle.join_raw(handle)` written in
//!    static style). The decision controls whether `self` is
//!    prepended to `arg_values`.
//! 3. **Final-callee resolution** (~line 1179). Builds the mangled
//!    `ResolvedClass_method` name (with the bufio `_file`/`_tcp`
//!    suffix when applicable) and routes it through
//!    `resolve_ffi_alias_callee` — the wrapper that turns an alias
//!    hit into the C symbol and a miss into the unchanged mangled
//!    name (which the linker then resolves against a user-defined
//!    method).
//!
//! ## §B1 consolidation scope
//!
//! Spec `docs/specs/system/compiler_consolidation.spec.md` §B1
//! proposed collapsing the three branches into one
//! `lower_method_call_via_ffi_alias` entry. The §B1 stop condition
//! was triggered: the static-ctor fast path's `Class_init` synthesis
//! (the `format!("{}_init", type_name)` emit at the bottom of
//! Branch 1), the bufio kind-suffix routing, and the receiver-is-
//! class-identifier signal each have semantics that don't cleanly
//! fold into a generic helper without either preserving the existing
//! complexity (no consolidation win) or losing specialised behaviour
//! (regression).
//!
//! What §B1 DID consolidate:
//!
//! - The "is there an FFI alias for this name?" check now flows
//!   through `lookup_ffi_alias` (mir/lower/mod.rs) — the single
//!   entry. The `self.ffi_alias_map.contains_key(...)` direct probe
//!   that lived at Branch 1's top is gone; the symmetric direct
//!   probe in `fn_call.rs` is also gone.
//! - `resolve_ffi_alias_callee` is now a thin wrapper around
//!   `lookup_ffi_alias` (miss → unchanged-mangled), preserving the
//!   pre-§B1 caller surface.
//!
//! The pin test `compiler/riven_core/tests/ffi_alias_single_entry.rs`
//! locks in: no caller outside `mir/lower/mod.rs` accesses
//! `ffi_alias_map` directly.

use super::super::*;

impl<'a> Lowerer<'a> {
    pub(super) fn lower_method_call(&mut self, expr: &HirExpr) -> Result<Option<LocalId>, String> {
        match &expr.kind {
            // ── Method call ─────────────────────────────────────────
            HirExprKind::MethodCall {
                object,
                method_name,
                generic_args: _generic_args,
                args,
                block,
                ..
            } => {
                let type_name = self
                    .receiver_type_name(object)
                    .unwrap_or_else(|| type_name_from_ty(&object.ty));

                // ── Phase C: dynamic dispatch on `&Mixin` / `&var Mixin` ─
                // When the receiver is statically typed as a
                // runtime-dispatch mixin reference (e.g. `f: &Future`),
                // call the per-mixin-method helper instead of mangling
                // a static `<Class>_<method>`. The helper reads
                // class_info_ptr from slot 0 of self, looks up the
                // mixin's vtable, and indirect-calls the concrete
                // method. Spec docs/specs/types/mixin_vtables.spec.md §B5/§B6.
                if let Some(mixin_name) = self.dyn_mixin_receiver_name(&object.ty) {
                    let mut arg_vals = Vec::with_capacity(args.len() + 1);
                    let self_local = self.lower_expr(object)?;
                    if let Some(s) = self_local {
                        arg_vals.push(MirValue::Use(s));
                    }
                    for a in args {
                        let l = self.lower_expr(a)?;
                        arg_vals.push(local_to_value(l));
                    }
                    let dest = if expr.ty != Ty::Unit && expr.ty != Ty::Never {
                        Some(self.new_temp(expr.ty.clone()))
                    } else {
                        None
                    };
                    self.emit(MirInst::Call {
                        dest,
                        callee: format!("{}_dynamic_{}", mixin_name, method_name),
                        args: arg_vals,
                    });
                    return Ok(dest);
                }

                // Handle .new() / .with_capacity() constructor calls:
                // dispatch directly to the runtime symbol (no self arg).
                //
                // Phase 2 stdlib (#06.5 T2): `File.{open,create,append,
                // open_options}` are *static* constructor-style methods
                // (no `self` receiver). The receiver type-name resolves
                // to "File" (the class identifier promoted to its Ty by
                // resolve/mod.rs), so the regular dispatch path below
                // would mangle to `File_open(self, path)` and pass the
                // class-identifier value as a synthetic `self` — the
                // runtime `riven_file_open(path)` takes one arg, not
                // two, so the Cranelift verifier rejects the call. We
                // route these names through the same direct-dispatch
                // path as `Command.new` instead.
                let is_file_static_ctor = (type_name == "File" || type_name.starts_with("File["))
                    && matches!(
                        method_name.as_str(),
                        "open" | "create" | "append" | "open_options"
                    );
                // Phase 2 #06.5 T5: TcpListener.bind / TcpStream.connect
                // are static-style constructors — runtime entries
                // (`riven_tcp_listener_bind`, `riven_tcp_stream_connect`)
                // take a single `const char*` and return Result. Same
                // fast-path reasoning as File.open / File.create.
                let is_tcp_listener_static_ctor =
                    type_name == "TcpListener" && method_name == "bind";
                let is_tcp_stream_static_ctor =
                    type_name == "TcpStream" && method_name == "connect";
                // Phase 2 #06.5 T6: BufReader / BufWriter static ctors.
                // `.new(inner)` and `.with_capacity(cap, inner)` both
                // dispatch through the fast path. The runtime callee
                // is suffix-picked (`_new_file` vs `_new_tcp`) below
                // from the inner argument's type.
                let is_bufio_static_ctor = matches!(type_name.as_str(), "BufReader" | "BufWriter")
                    && matches!(method_name.as_str(), "new" | "with_capacity");
                // Phase 2 stdlib (#06.5 T4): Duration / Instant
                // static-style constructors join the same fast path.
                // `Duration.from_secs(5)` / `Duration.from_millis(ms)`
                // / `Duration.from_micros(us)` /
                // `Duration.from_nanos(ns)` and `Instant.now()`
                // dispatch directly to their runtime symbol with no
                // synthetic `self`. Same reason as File: the runtime
                // entry points take 0–1 args, not `self + args`.
                let is_duration_static_ctor = type_name == "Duration"
                    && matches!(
                        method_name.as_str(),
                        "from_secs" | "from_millis" | "from_micros" | "from_nanos"
                    );
                let is_instant_static_ctor = type_name == "Instant" && method_name == "now";
                let is_collection_ctor = method_name == "new"
                    || is_file_static_ctor
                    || is_duration_static_ctor
                    || is_instant_static_ctor
                    || is_tcp_listener_static_ctor
                    || is_tcp_stream_static_ctor
                    || is_bufio_static_ctor
                    || (method_name == "with_capacity" && {
                        let bt = if let Some(pos) = type_name.find('[') {
                            &type_name[..pos]
                        } else {
                            type_name.as_str()
                        };
                        matches!(
                            bt,
                            "Vec" | "Array" | "Hash" | "HashMap" | "Map" | "Set" | "HashSet"
                        )
                    });
                if is_collection_ctor {
                    // For built-in types (Vec, Hash, Set), call the runtime
                    // constructor directly instead of Alloc + init.
                    let base_type = if let Some(pos) = type_name.find('[') {
                        &type_name[..pos]
                    } else {
                        type_name.as_str()
                    };

                    // Bug-fix (v1-missing-features 2026-05-20):
                    // ANY class that has a `def self.{method_name} as "<c-symbol>"`
                    // lib decl registered — including generic classes like
                    // `Mutex[T]`, `SharedSync[T]`, `AtomicI64`, `Sender[T: Send]`,
                    // etc. — must route directly to the FFI symbol with NO
                    // synthetic `self` prepended and NO `Class_init` fallback.
                    // The hardcoded class lists below only cover legacy
                    // builtins; without this generic check, calls like
                    // `Mutex.new(7)` synthesize `Mutex_init(self, 7)` and the
                    // linker fails with "undefined symbol _Mutex_init".
                    // The dotted-name normalisation matches
                    // `register_class_lib_method_in`'s mangling shape.
                    //
                    // §B1 — every "is there an FFI alias?" check routes
                    // through `lookup_ffi_alias` (the single entry).
                    let alias_key_cs = base_type.replace('.', "_");
                    let alias_key = format!("{}_{}", alias_key_cs, method_name);
                    if let Some(callee) = self.lookup_ffi_alias(&alias_key) {
                        let obj = self.new_temp(expr.ty.clone());
                        let mut call_args = Vec::with_capacity(args.len());
                        for arg in args {
                            let local = self.lower_expr(arg)?;
                            call_args.push(local_to_value(local));
                        }
                        self.emit(MirInst::Call {
                            dest: Some(obj),
                            callee,
                            args: call_args,
                        });
                        return Ok(Some(obj));
                    }
                    // Phase 2 #06.D2.S0: `Formatter.new()` dispatches to
                    // the runtime constructor just like Vec/Hash.
                    // Phase 2 #06 (Command): `Command.new(prog)` joins
                    // the same fast path so it dispatches to
                    // `riven_command_new(prog)` instead of going through
                    // the `Class_init` path (Command has no user-defined
                    // init).
                    // Phase E.E of #06.95: any class with a dotted
                    // name (`BufReader.File`, `BufWriter.Tcp`, …) was
                    // declared inside a `module` block. Module-nested
                    // classes are necessarily new (they didn't exist
                    // before the module+mixin reshape), they have no
                    // user-defined `_init`, and their `def self.NAME`
                    // lib decls already register the right FFI alias.
                    // Treat them like the listed top-level classes —
                    // route through the static-ctor fast path, mangle
                    // with dot → underscore, and let
                    // `resolve_ffi_alias_callee` rewrite to the C
                    // symbol.
                    let is_module_nested_class = base_type.contains('.');
                    if is_module_nested_class
                        || matches!(
                        base_type,
                        "Vec"
                            | "Array"
                            | "Hash"
                            | "HashMap"
                            | "Map"
                            | "Set"
                            | "HashSet"
                            | "Formatter"
                            | "Command"
                            // Phase 2 stdlib (#06.5 T2): `File.open/create/
                            // append/open_options` and `OpenOptions.new`
                            // dispatch to the runtime constructor here so
                            // they bypass the `Class_init(self, args)`
                            // path (these classes have no user-defined
                            // init). The dispatch table in
                            // codegen/runtime.rs then maps the resulting
                            // `File_open` / `OpenOptions_new` mangled name
                            // to the `riven_file_*` / `riven_open_options_*`
                            // runtime fn.
                            | "File"
                            | "OpenOptions"
                            // Phase 2 stdlib (#06.5 T4): Duration /
                            // Instant static-style constructors
                            // dispatch directly to their runtime
                            // symbol — `Duration_from_secs(s)` →
                            // `riven_duration_from_secs(s)`,
                            // `Instant_now()` → `riven_instant_now()`.
                            // No `_init` path; these classes have no
                            // user-defined init.
                            | "Duration"
                            | "Instant"
                            // Phase 2 #06.5 T5: TcpListener.bind /
                            // TcpStream.connect — class-static
                            // constructors that take only `&String`.
                            // The dispatch table in codegen/
                            // runtime_table maps `TcpListener_bind` →
                            // `riven_tcp_listener_bind`, etc.
                            | "TcpListener"
                            | "TcpStream"
                            // Phase 2 #06.5 T6: BufReader[R] /
                            // BufWriter[W] LEGACY entries — superseded
                            // by the `is_module_nested_class` check
                            // above once callers migrate to
                            // BufReader.File / BufReader.Tcp. Remove
                            // after the BOOTSTRAP_FILES migration
                            // sweep deletes the last bare BufReader
                            // usage.
                            | "BufReader"
                            | "BufWriter"
                    ) {
                        let obj = self.new_temp(expr.ty.clone());
                        // ruby-naming.spec.md §3.11 renames stdlib types
                        // (`Vec` → `Array`, `HashMap` → `Map`, `HashSet` →
                        // `Set`). The runtime C functions keep their
                        // legacy names (`Vec_new`, `Hash_new`, …), so map
                        // the surface base-type back to the runtime
                        // before mangling.
                        let runtime_base = match base_type {
                            "Array" => "Vec",
                            "Map" => "Hash",
                            "HashMap" => "Hash",
                            "Set" => "HashSet",
                            other => other,
                        };
                        // The same fast path also handles `with_capacity`,
                        // which takes a single integer arg and lowers to
                        // e.g. `riven_hash_with_capacity(cap)`.
                        let mut call_args = Vec::with_capacity(args.len());
                        // Phase 2 #06.5 T6: BufReader[R] / BufWriter[W]
                        // pick `_new_file` vs `_new_tcp` (and similarly
                        // `_with_capacity_file` / `_with_capacity_tcp`)
                        // by peeking at the inner argument's type.
                        // For `new(inner)` inner is args[0]; for
                        // `with_capacity(cap, inner)` it's args[1]. The
                        // typeck E0714 check above already rejects any
                        // other inner type, so this match is exhaustive
                        // (the fallback to "file" is defensive — if we
                        // hit it the runtime dispatch table will fail
                        // to find a symbol and the link step errors
                        // out cleanly).
                        let bufio_suffix: Option<&'static str> =
                            if matches!(base_type, "BufReader" | "BufWriter") {
                                let inner_idx = if method_name == "with_capacity" { 1 } else { 0 };
                                let inner_name = args
                                    .get(inner_idx)
                                    .map(|a| type_name_from_ty(&a.ty))
                                    .unwrap_or_default();
                                // Peel leading reference if any (defensive —
                                // the spec passes inner by value).
                                let inner_name = inner_name
                                    .strip_prefix('&')
                                    .map(str::trim_start)
                                    .unwrap_or(&inner_name);
                                match inner_name {
                                    "TcpStream" => Some("tcp"),
                                    "File" => Some("file"),
                                    _ => Some("file"),
                                }
                            } else {
                                None
                            };
                        for arg in args {
                            let local = self.lower_expr(arg)?;
                            call_args.push(local_to_value(local));
                        }
                        // Phase E.E of #06.95: module-nested classes
                        // carry dotted names (`BufReader.File`). C
                        // symbols can't contain `.`, so normalise to
                        // `_` when building the mangled callee — same
                        // shape `register_class_lib_method_in` uses
                        // for the FFI alias map key.
                        let runtime_base_cs = runtime_base.replace('.', "_");
                        let raw_callee = if let Some(suffix) = bufio_suffix {
                            format!("{}_{}_{}", runtime_base_cs, method_name, suffix)
                        } else {
                            format!("{}_{}", runtime_base_cs, method_name)
                        };
                        // #06.8 T#14: the static-ctor fast path
                        // historically emitted `Vec_new` / `Hash_new`
                        // / `Set_new` directly and let runtime_table
                        // do the runtime-symbol rewrite. With the
                        // migration moving those constructors into
                        // .rvn class-body lib decls, route through
                        // the alias map first — try both the
                        // `runtime_base` (legacy `Vec_new`) and the
                        // surface `base_type` (canonical `Array_new`)
                        // shapes, since the bootstrap `class` shell
                        // is keyed on the canonical name. Falls
                        // through to the raw mangled callee when
                        // neither lookup hits, preserving
                        // backward-compat for any base/method whose
                        // entry still lives in runtime_table.
                        let callee = {
                            let aliased = self.resolve_ffi_alias_callee(raw_callee.clone());
                            if aliased != raw_callee {
                                aliased
                            } else if let Some(suffix) = bufio_suffix {
                                self.resolve_ffi_alias_callee(format!(
                                    "{}_{}_{}",
                                    base_type, method_name, suffix
                                ))
                            } else {
                                self.resolve_ffi_alias_callee(format!(
                                    "{}_{}",
                                    base_type, method_name
                                ))
                            }
                        };
                        self.emit(MirInst::Call {
                            dest: Some(obj),
                            callee,
                            args: call_args,
                        });
                        return Ok(Some(obj));
                    }
                    // String.new / String.with_capacity — dispatch to the
                    // C runtime directly. The dispatch table in
                    // codegen/runtime.rs maps `String_new` and
                    // `String_with_capacity` to their `riven_string_*`
                    // implementations.
                    if base_type == "String" {
                        let obj = self.new_temp(expr.ty.clone());
                        let mut call_args = Vec::with_capacity(args.len());
                        for arg in args {
                            let local = self.lower_expr(arg)?;
                            call_args.push(local_to_value(local));
                        }
                        self.emit(MirInst::Call {
                            dest: Some(obj),
                            callee: "String_new".to_string(),
                            args: call_args,
                        });
                        return Ok(Some(obj));
                    }

                    // Structs have no user-defined `init`. The positional
                    // arguments map directly onto the declared fields, so
                    // we allocate the backing storage and emit one
                    // SetField per argument — no synthetic init function.
                    if matches!(&object.ty, Ty::Struct { .. }) {
                        let obj = self.new_temp(expr.ty.clone());
                        self.emit(MirInst::Alloc {
                            dest: obj,
                            ty: expr.ty.clone(),
                            size: self.alloc_size(&expr.ty),
                        });
                        for (idx, arg) in args.iter().enumerate() {
                            let local = self.lower_expr(arg)?;
                            self.emit(MirInst::SetField {
                                base: obj,
                                field_index: idx,
                                value: local_to_value(local),
                            });
                        }
                        return Ok(Some(obj));
                    }

                    let layout = crate::codegen::layout::layout_of(&expr.ty, self.symbols);
                    let obj = self.new_temp(expr.ty.clone());
                    self.emit(MirInst::Alloc {
                        dest: obj,
                        ty: expr.ty.clone(),
                        size: self.alloc_size(&expr.ty),
                    });
                    // Phase B-5: write class_info_ptr at slot 0 before
                    // `ClassName_init` runs (init body may already
                    // try to dispatch a `dyn Mixin` method on
                    // `self`).
                    self.emit_class_info_init(&expr.ty, obj);

                    // Call ClassName_init(self, args...)
                    let mut arg_values = vec![MirValue::Use(obj)];
                    for arg in args {
                        let local = self.lower_expr(arg)?;
                        arg_values.push(local_to_value(local));
                    }
                    let _ = layout; // size used by Alloc internally via layout_of in codegen
                    self.emit(MirInst::Call {
                        dest: None,
                        callee: format!("{}_init", type_name),
                        args: arg_values,
                    });
                    return Ok(Some(obj));
                }

                // ── Phase 2 stdlib (#04): HashMap.entry chain ──────────
                // `m.entry(K).or_insert(V)` and `m.entry(K).or_insert_with { || V }`
                // are recognized as a single MIR unit and inlined to:
                //
                //   if !riven_hash_contains_key(map, k) {
                //       riven_hash_insert(map, k, v);   // discard prior value
                //   }
                //
                // Typeck has already verified the chain shape and the V
                // type — see `infer.rs` MethodCall handler. This emission
                // never materializes an `Entry[K,V]` value at runtime.
                if (method_name == "or_insert" || method_name == "or_insert_with")
                    && matches!(
                        &object.kind,
                        HirExprKind::MethodCall { method_name: m, .. } if m == "entry"
                    )
                {
                    let result = self.inline_entry_or_insert(object, method_name, args, block)?;
                    return Ok(result);
                }

                // ── Inline closure-taking methods ──────────────────────
                // When a method like .each, .filter, .find, .position,
                // .map, .partition, .where_matching takes a trailing block
                // (closure), inline the closure body as a loop instead of
                // passing a (null) function pointer.
                if let Some(block_expr) = block {
                    if let Some(result) =
                        self.try_inline_closure_method(expr, object, method_name, args, block_expr)?
                    {
                        return Ok(result);
                    }
                }

                // Phase 2 stdlib (#05 follow-up): built-in
                // `iter.collect[Target]` lowers directly to a runtime
                // constructor over the v1 eager-iterator representation
                // (`RivenVec*`). Typeck has already validated the target
                // and item compatibility, so lowering only picks the
                // concrete helper by the expression's result type.
                if method_name == "collect" {
                    let iter_local = self.lower_expr(object)?;
                    let iter_id = iter_local.unwrap_or_else(|| self.new_temp(Ty::Int));
                    let dest = self.new_temp(expr.ty.clone());
                    let callee = match &expr.ty {
                        Ty::Array(_) => "riven_vec_from_iter",
                        Ty::String | Ty::Str => "riven_string_from_iter",
                        Ty::Map(_, _) => "riven_hash_from_iter",
                        Ty::Set(_) => "riven_set_from_iter",
                        other => {
                            return Err(format!(
                                "unsupported collect target in MIR lowering: {other}"
                            ));
                        }
                    };
                    self.emit(MirInst::Call {
                        dest: Some(dest),
                        callee: callee.to_string(),
                        args: vec![MirValue::Use(iter_id)],
                    });
                    return Ok(Some(dest));
                }

                // ── Inline try_op (? operator) ──────────────────────────
                // The ? operator desugars to .try_op(). For Result types:
                // Ok(x) -> extract x and continue; Err(e) -> return Err(e).
                // For Option types: Some(x) -> x; None -> return Err(err)
                // (only when inside a Result-returning function via ok_or).
                if method_name == "try_op" {
                    let obj_local = self.lower_expr(object)?;
                    let scrut = obj_local.unwrap_or_else(|| self.new_temp(Ty::Int));

                    // Read the tag: 0 = Ok/Some, 1 = Err/None
                    let tag = self.new_temp(Ty::Int32);
                    self.emit(MirInst::GetTag {
                        dest: tag,
                        src: scrut,
                    });

                    let ok_block = self.new_block();
                    let err_block = self.new_block();
                    let merge_block = self.new_block();

                    // tag == 0 means Ok
                    let is_ok = self.new_temp(Ty::Bool);
                    self.emit(MirInst::Compare {
                        dest: is_ok,
                        op: CmpOp::Eq,
                        lhs: MirValue::Use(tag),
                        rhs: MirValue::Literal(Literal::Int(0)),
                    });
                    self.set_terminator(Terminator::Branch {
                        cond: MirValue::Use(is_ok),
                        then_block: ok_block,
                        else_block: err_block,
                    });

                    // Ok block: extract payload
                    let result_local = self.new_temp(expr.ty.clone());
                    self.current_block = ok_block;
                    let payload_ptr = self.new_temp(Ty::Int);
                    self.emit(MirInst::GetPayload {
                        dest: payload_ptr,
                        src: scrut,
                        ty: object.ty.clone(),
                    });
                    self.emit(MirInst::GetField {
                        dest: result_local,
                        base: payload_ptr,
                        field_index: 0,
                    });
                    self.set_terminator(Terminator::Goto(merge_block));

                    // Err block: early return with Err wrapping the error payload.
                    // Allocate a Result tagged union and return it.
                    self.current_block = err_block;
                    let err_result = self.new_temp(Ty::Int);
                    self.emit(MirInst::Alloc {
                        dest: err_result,
                        ty: Ty::Result(Box::new(Ty::Unit), Box::new(Ty::Int)),
                        size: 16,
                    });
                    // Tag 1 = Err
                    self.emit(MirInst::SetTag {
                        dest: err_result,
                        tag: 1,
                    });
                    // Copy error payload from source
                    let err_payload_ptr = self.new_temp(Ty::Int);
                    self.emit(MirInst::GetPayload {
                        dest: err_payload_ptr,
                        src: scrut,
                        ty: object.ty.clone(),
                    });
                    let err_payload = self.new_temp(Ty::Int);
                    self.emit(MirInst::GetField {
                        dest: err_payload,
                        base: err_payload_ptr,
                        field_index: 0,
                    });

                    // If the current function's declared Err type differs
                    // from the source's Err type and an `impl Into[Outer]
                    // for Inner` was registered, insert a call to
                    // `Inner_into(err_payload)` to coerce the error.
                    let final_payload = if let (Ty::Result(_, src_err), Ty::Result(_, dst_err)) =
                        (&object.ty, &self.fn_mut().return_ty.clone())
                    {
                        let src_name = type_name_from_ty(src_err);
                        let dst_name = type_name_from_ty(dst_err);
                        if !src_name.is_empty()
                            && !dst_name.is_empty()
                            && src_name != dst_name
                            && self
                                .into_impls
                                .contains(&(src_name.clone(), dst_name.clone()))
                        {
                            let converted = self.new_temp((**dst_err).clone());
                            self.emit(MirInst::Call {
                                dest: Some(converted),
                                callee: format!("{}_into", src_name),
                                args: vec![MirValue::Use(err_payload)],
                            });
                            MirValue::Use(converted)
                        } else {
                            MirValue::Use(err_payload)
                        }
                    } else {
                        MirValue::Use(err_payload)
                    };

                    self.emit(MirInst::SetField {
                        base: err_result,
                        field_index: 1,
                        value: final_payload,
                    });
                    self.set_terminator(Terminator::Return(Some(MirValue::Use(err_result))));

                    self.current_block = merge_block;
                    return Ok(Some(result_local));
                }

                // ── Inline ok_or (Option -> Result conversion) ───────────
                // option.ok_or(err_val) converts:
                //   Some(x) -> Result::Ok(x) (tag 0)
                //   None    -> Result::Err(err_val) (tag 1)
                if method_name == "ok_or" {
                    let obj_local = self.lower_expr(object)?;
                    let scrut = obj_local.unwrap_or_else(|| self.new_temp(Ty::Int));

                    // Evaluate the error value argument
                    let err_arg = args.first();
                    let err_val = if let Some(err_expr) = err_arg {
                        let local = self.lower_expr(err_expr)?;
                        local_to_value(local)
                    } else {
                        MirValue::Literal(Literal::Int(0))
                    };

                    // Allocate a Result tagged union
                    let result = self.new_temp(expr.ty.clone());
                    self.emit(MirInst::Alloc {
                        dest: result,
                        ty: expr.ty.clone(),
                        size: 16,
                    });

                    // Read the Option tag: 0 = None (in Option), 1 = Some
                    // Note: inline_position uses tag 0 = None, tag 1 = Some
                    let tag = self.new_temp(Ty::Int32);
                    self.emit(MirInst::GetTag {
                        dest: tag,
                        src: scrut,
                    });

                    let some_block = self.new_block();
                    let none_block = self.new_block();
                    let merge_block = self.new_block();

                    // tag == 1 means Some
                    let is_some = self.new_temp(Ty::Bool);
                    self.emit(MirInst::Compare {
                        dest: is_some,
                        op: CmpOp::Eq,
                        lhs: MirValue::Use(tag),
                        rhs: MirValue::Literal(Literal::Int(1)),
                    });
                    self.set_terminator(Terminator::Branch {
                        cond: MirValue::Use(is_some),
                        then_block: some_block,
                        else_block: none_block,
                    });

                    // Some block: Result::Ok(payload) — tag 0
                    self.current_block = some_block;
                    self.emit(MirInst::SetTag {
                        dest: result,
                        tag: 0,
                    }); // Ok
                    let payload_ptr = self.new_temp(Ty::Int);
                    self.emit(MirInst::GetPayload {
                        dest: payload_ptr,
                        src: scrut,
                        ty: object.ty.clone(),
                    });
                    let some_val = self.new_temp(Ty::Int);
                    self.emit(MirInst::GetField {
                        dest: some_val,
                        base: payload_ptr,
                        field_index: 0,
                    });
                    self.emit(MirInst::SetField {
                        base: result,
                        field_index: 1,
                        value: MirValue::Use(some_val),
                    });
                    self.set_terminator(Terminator::Goto(merge_block));

                    // None block: Result::Err(err_val) — tag 1
                    self.current_block = none_block;
                    self.emit(MirInst::SetTag {
                        dest: result,
                        tag: 1,
                    }); // Err
                    self.emit(MirInst::SetField {
                        base: result,
                        field_index: 1,
                        value: err_val,
                    });
                    self.set_terminator(Terminator::Goto(merge_block));

                    self.current_block = merge_block;
                    return Ok(Some(result));
                }

                // Check if this is a static/class method call (no `self`
                // argument needed). Covers built-in static methods as well
                // as user-defined `def self.method` forms on classes.
                //
                // Bug-fix (v1-missing-features 2026-05-20):
                // A `Type.method(arg)` call where `Type` is a class
                // identifier (HirExprKind::VarRef → DefKind::Class) MUST
                // be treated as static-style dispatch even if `method` is
                // declared as an instance method (`def method as "..."(self)`
                // in a class lib block). The instance-method FFI registration
                // already prepends the receiver type to `param_types` at
                // registration time (see `register_class_lib_method_in`),
                // so the C symbol's cranelift signature expects exactly
                // (self, user_args...). When the user writes the call in
                // static style — `JoinHandle.join_raw(handle)` — their
                // first explicit arg IS the self handle, and we must not
                // additionally prepend a phantom `Unit` (zero) at the
                // call site. Without this guard the verifier rejects
                // the resulting `riven_thread_join(0, handle)` as
                // "got 2, expected 1".
                let static_dispatch_ty = if matches!(&object.ty, Ty::Infer(_)) {
                    &expr.ty
                } else {
                    &object.ty
                };
                let receiver_is_class_identifier = self.is_class_identifier(object);
                let is_static = is_builtin_static_method(&type_name, method_name)
                    || self.is_user_static_method(&type_name, method_name)
                    || (method_name == "default"
                        && self.type_supports_trait(static_dispatch_ty, "Default"))
                    || receiver_is_class_identifier;

                // Regular method call: object becomes the first argument (self).
                let obj_local = self.lower_expr(object)?;

                let mut arg_values = if is_static {
                    // Static method: don't prepend self.
                    Vec::with_capacity(args.len())
                } else {
                    vec![local_to_value(obj_local)]
                };
                for arg in args {
                    let local = self.lower_expr(arg)?;
                    arg_values.push(local_to_value(local));
                }
                // Include trailing block argument if present (closures passed
                // as the last parameter of the method).
                if let Some(block_expr) = block {
                    let block_local = self.lower_expr(block_expr)?;
                    arg_values.push(local_to_value(block_local));
                }

                // Resolve through parent classes for inherited methods.
                // For a generic type parameter or impl/dyn Trait, dispatch
                // to the unique implementor of the trait bound when one
                // exists.
                let resolved_class = match &object.ty {
                    Ty::Class { name, .. } => self.resolve_method_class(name, method_name),
                    Ty::TypeParam { bounds, .. } | Ty::SomeMixin(bounds) | Ty::AnyMixin(bounds) => {
                        self.unique_bound_impl(bounds)
                            .unwrap_or_else(|| type_name.clone())
                    }
                    Ty::Ref(inner)
                    | Ty::RefMut(inner)
                    | Ty::RefLifetime(_, inner)
                    | Ty::RefMutLifetime(_, inner) => match inner.as_ref() {
                        Ty::TypeParam { bounds, .. }
                        | Ty::SomeMixin(bounds)
                        | Ty::AnyMixin(bounds) => self
                            .unique_bound_impl(bounds)
                            .unwrap_or_else(|| type_name.clone()),
                        _ => type_name.clone(),
                    },
                    _ => type_name.clone(),
                };
                // Phase 2 #06.5 T6: BufReader / BufWriter instance
                // methods that need kind-suffix routing (`into_inner`
                // returns the inner File or TcpStream — the runtime
                // exports `_into_inner_file` / `_into_inner_tcp` so
                // the LLVM/Cranelift return ABI is honest about the
                // concrete inner type). The closed-set typeck check
                // at construction time means generic_args[0] is one
                // of File / TcpStream here.
                let bufio_instance_suffix: Option<&'static str> =
                    if matches!(resolved_class.as_str(), "BufReader" | "BufWriter")
                        && method_name == "into_inner"
                    {
                        let inner_name = match &object.ty {
                            Ty::Class { generic_args, .. } => generic_args
                                .first()
                                .map(type_name_from_ty)
                                .unwrap_or_default(),
                            Ty::Ref(inner)
                            | Ty::RefMut(inner)
                            | Ty::RefLifetime(_, inner)
                            | Ty::RefMutLifetime(_, inner) => {
                                if let Ty::Class { generic_args, .. } = inner.as_ref() {
                                    generic_args
                                        .first()
                                        .map(type_name_from_ty)
                                        .unwrap_or_default()
                                } else {
                                    String::new()
                                }
                            }
                            _ => String::new(),
                        };
                        match inner_name.as_str() {
                            "TcpStream" => Some("tcp"),
                            _ => Some("file"),
                        }
                    } else {
                        None
                    };
                // #06.93 Phase 3: module-qualified class names carry a
                // dotted form (`Outer.Inner`). C symbol names can't
                // contain `.`, so normalise to `_` when building the
                // mangled callee — `Outer.Inner_make` becomes
                // `Outer_Inner_make`. The FFI alias map is keyed in
                // the same shape by `register_class_lib_method`.
                let resolved_class_cs = resolved_class.replace('.', "_");
                let mangled = if let Some(suffix) = bufio_instance_suffix {
                    format!("{}_{}_{}", resolved_class_cs, method_name, suffix)
                } else {
                    format!("{}_{}", resolved_class_cs, method_name)
                };

                // `&mut String` detection: when the receiver is a local
                // of type `&mut String` (i.e. the caller passed `&mut s`
                // into a parameter typed `&mut String`), the local holds
                // a pointer-to-`char*`. Mutating methods must read the
                // current buffer via `riven_deref_ptr`, call the string
                // helper, then write the new buffer back via
                // `riven_store_ptr` so the caller observes the update.
                let receiver_is_mut_string_ref = matches!(
                    &object.ty,
                    Ty::RefMut(inner) | Ty::RefMutLifetime(_, inner)
                        if matches!(inner.as_ref(), Ty::String | Ty::Str)
                );

                // Special handling for push_str on String variables:
                // riven_string_push_str returns a new char*, so we need to
                // capture the return value and reassign it to the object variable.
                if method_name == "push_str" {
                    if receiver_is_mut_string_ref {
                        // `self_arg` here is the pointer value (char**).
                        // We need the pointee to feed into push_str, and
                        // we must store the returned buffer back through
                        // the pointer.
                        let ptr_arg = arg_values[0].clone();
                        let tail_args: Vec<MirValue> = arg_values.iter().skip(1).cloned().collect();
                        let cur = self.new_temp(Ty::String);
                        self.emit(MirInst::Call {
                            dest: Some(cur),
                            callee: "riven_deref_ptr".to_string(),
                            args: vec![ptr_arg.clone()],
                        });
                        let new_buf = self.new_temp(Ty::String);
                        let mut call_args = vec![MirValue::Use(cur)];
                        call_args.extend(tail_args);
                        self.emit(MirInst::Call {
                            dest: Some(new_buf),
                            callee: "String_push_str".to_string(),
                            args: call_args,
                        });
                        self.emit(MirInst::Call {
                            dest: None,
                            callee: "riven_store_ptr".to_string(),
                            args: vec![ptr_arg, MirValue::Use(new_buf)],
                        });
                        return Ok(None);
                    }
                    if let HirExprKind::VarRef(def_id) = &object.kind {
                        if let Some(&obj_var) = self.def_to_local.get(def_id) {
                            let tmp = self.new_temp(Ty::String);
                            self.emit(MirInst::Call {
                                dest: Some(tmp),
                                callee: mangled,
                                args: arg_values,
                            });
                            self.emit(MirInst::Assign {
                                dest: obj_var,
                                value: MirValue::Use(tmp),
                            });
                            return Ok(None);
                        }
                    }
                }

                // Special handling for `String.push(char)`: the runtime
                // only exposes `riven_string_push_str`, so we first widen
                // the Char arg to a one-char heap string via
                // `riven_char_to_string`, then hand that to push_str.
                // Without this rewrite every program that calls
                // `s.push('!')` links against a missing `String_push`.
                //
                // When the receiver is `&mut String` (a parameter), we
                // lower to `*s = String_push_str(*s, one_char_str)` using
                // the deref/store runtime helpers so the caller's local
                // is updated in place.  For an owned local String binding
                // we just rebind the variable to the new buffer.
                if method_name == "push" && resolved_class == "String" && arg_values.len() == 2 {
                    // Phase 2 stdlib batch 2 (#02): route through the
                    // dedicated `riven_string_push(s, codepoint)` runtime
                    // fn rather than synthesising
                    // `riven_char_to_string` + `String_push_str` here.
                    // The dedicated fn allocates exactly one fresh
                    // buffer per call and frees its internal char-string
                    // temporary, so we don't leak the codepoint
                    // intermediate. The prior receiver buffer is freed
                    // here explicitly so the rebind doesn't leak it.
                    let char_arg = arg_values[1].clone();
                    let self_arg = arg_values[0].clone();
                    if receiver_is_mut_string_ref {
                        let cur = self.new_temp(Ty::String);
                        self.emit(MirInst::Call {
                            dest: Some(cur),
                            callee: "riven_deref_ptr".to_string(),
                            args: vec![self_arg.clone()],
                        });
                        let new_buf = self.new_temp(Ty::String);
                        self.emit(MirInst::Call {
                            dest: Some(new_buf),
                            callee: "String_push".to_string(),
                            args: vec![MirValue::Use(cur), char_arg],
                        });
                        // Free the prior buffer before overwriting the
                        // pointer slot, otherwise it leaks.
                        self.emit(MirInst::Call {
                            dest: None,
                            callee: "riven_string_free".to_string(),
                            args: vec![MirValue::Use(cur)],
                        });
                        self.emit(MirInst::Call {
                            dest: None,
                            callee: "riven_store_ptr".to_string(),
                            args: vec![self_arg, MirValue::Use(new_buf)],
                        });
                        return Ok(None);
                    }
                    let new_buf = self.new_temp(Ty::String);
                    self.emit(MirInst::Call {
                        dest: Some(new_buf),
                        callee: "String_push".to_string(),
                        args: vec![self_arg.clone(), char_arg],
                    });
                    if let HirExprKind::VarRef(def_id) = &object.kind {
                        if let Some(&obj_var) = self.def_to_local.get(def_id) {
                            // Free the prior buffer first; the local
                            // owns it (we just lowered it as the self
                            // arg above) and the assignment below is
                            // about to overwrite the slot.
                            self.emit(MirInst::Call {
                                dest: None,
                                callee: "riven_string_free".to_string(),
                                args: vec![MirValue::Use(obj_var)],
                            });
                            self.emit(MirInst::Assign {
                                dest: obj_var,
                                value: MirValue::Use(new_buf),
                            });
                        }
                    }
                    return Ok(None);
                }

                // Phase 2 stdlib: mutating String methods that allocate a
                // fresh buffer (insert, insert_str). Same dance as push_str.
                if matches!(method_name.as_str(), "insert" | "insert_str")
                    && resolved_class == "String"
                {
                    if receiver_is_mut_string_ref {
                        let ptr_arg = arg_values[0].clone();
                        let tail_args: Vec<MirValue> = arg_values.iter().skip(1).cloned().collect();
                        let cur = self.new_temp(Ty::String);
                        self.emit(MirInst::Call {
                            dest: Some(cur),
                            callee: "riven_deref_ptr".to_string(),
                            args: vec![ptr_arg.clone()],
                        });
                        let new_buf = self.new_temp(Ty::String);
                        let mut call_args = vec![MirValue::Use(cur)];
                        call_args.extend(tail_args);
                        self.emit(MirInst::Call {
                            dest: Some(new_buf),
                            callee: format!("String_{}", method_name),
                            args: call_args,
                        });
                        self.emit(MirInst::Call {
                            dest: None,
                            callee: "riven_store_ptr".to_string(),
                            args: vec![ptr_arg, MirValue::Use(new_buf)],
                        });
                        return Ok(None);
                    }
                    if let HirExprKind::VarRef(def_id) = &object.kind {
                        if let Some(&obj_var) = self.def_to_local.get(def_id) {
                            let tmp = self.new_temp(Ty::String);
                            self.emit(MirInst::Call {
                                dest: Some(tmp),
                                callee: format!("String_{}", method_name),
                                args: arg_values,
                            });
                            self.emit(MirInst::Assign {
                                dest: obj_var,
                                value: MirValue::Use(tmp),
                            });
                            return Ok(None);
                        }
                    }
                }

                // String.remove(i) — returns the removed Char and
                // simultaneously rewrites the buffer. The runtime returns
                // a 16-byte struct {removed: i64, new_buffer: ptr}; we read
                // .removed for the value and .new_buffer to update the
                // local / &mut String.
                if method_name == "remove" && resolved_class == "String" {
                    let self_arg = arg_values[0].clone();
                    // For &mut String, we must first deref to get the buf.
                    let buf_arg = if receiver_is_mut_string_ref {
                        let cur = self.new_temp(Ty::String);
                        self.emit(MirInst::Call {
                            dest: Some(cur),
                            callee: "riven_deref_ptr".to_string(),
                            args: vec![self_arg.clone()],
                        });
                        MirValue::Use(cur)
                    } else {
                        self_arg.clone()
                    };
                    let tail_args: Vec<MirValue> = arg_values.iter().skip(1).cloned().collect();
                    let result_struct = self.new_temp(Ty::Int);
                    let mut call_args = vec![buf_arg];
                    call_args.extend(tail_args);
                    self.emit(MirInst::Call {
                        dest: Some(result_struct),
                        callee: "String_remove".to_string(),
                        args: call_args,
                    });
                    // Read the removed Char (field 0 of the 16-byte struct).
                    let removed = self.new_temp(Ty::Char);
                    self.emit(MirInst::GetField {
                        dest: removed,
                        base: result_struct,
                        field_index: 0,
                    });
                    // Read the new buffer (field 1).
                    let new_buf = self.new_temp(Ty::String);
                    self.emit(MirInst::GetField {
                        dest: new_buf,
                        base: result_struct,
                        field_index: 1,
                    });
                    if receiver_is_mut_string_ref {
                        self.emit(MirInst::Call {
                            dest: None,
                            callee: "riven_store_ptr".to_string(),
                            args: vec![self_arg, MirValue::Use(new_buf)],
                        });
                    } else if let HirExprKind::VarRef(def_id) = &object.kind {
                        if let Some(&obj_var) = self.def_to_local.get(def_id) {
                            self.emit(MirInst::Assign {
                                dest: obj_var,
                                value: MirValue::Use(new_buf),
                            });
                        }
                    }
                    return Ok(Some(removed));
                }

                // String.clear / truncate — in-place mutation; for &mut
                // String we must deref to the buffer pointer first.
                if matches!(method_name.as_str(), "clear" | "truncate")
                    && resolved_class == "String"
                {
                    if receiver_is_mut_string_ref {
                        let ptr_arg = arg_values[0].clone();
                        let tail_args: Vec<MirValue> = arg_values.iter().skip(1).cloned().collect();
                        let cur = self.new_temp(Ty::String);
                        self.emit(MirInst::Call {
                            dest: Some(cur),
                            callee: "riven_deref_ptr".to_string(),
                            args: vec![ptr_arg],
                        });
                        let mut call_args = vec![MirValue::Use(cur)];
                        call_args.extend(tail_args);
                        self.emit(MirInst::Call {
                            dest: None,
                            callee: format!("String_{}", method_name),
                            args: call_args,
                        });
                        return Ok(None);
                    }
                    // Owned local: pass the buffer pointer directly.
                    self.emit(MirInst::Call {
                        dest: None,
                        callee: format!("String_{}", method_name),
                        args: arg_values,
                    });
                    return Ok(None);
                }

                let dest = if expr.ty != Ty::Unit && expr.ty != Ty::Never {
                    Some(self.new_temp(expr.ty.clone()))
                } else {
                    None
                };

                // For calls on Fn/FnMut/FnOnce types (closure invocation),
                // emit an indirect call through the function pointer instead
                // of a regular named call.
                let is_fn_type = matches!(
                    &object.ty,
                    Ty::Fn { .. } | Ty::FnMut { .. } | Ty::FnOnce { .. }
                );
                let is_ref_fn_type = matches!(&object.ty,
                    Ty::Ref(inner) | Ty::RefMut(inner)
                    if matches!(inner.as_ref(), Ty::Fn { .. } | Ty::FnMut { .. } | Ty::FnOnce { .. })
                );
                // Phase 2 #06.9: dyn-erased `any Fn(...)` receivers
                // dispatch through the same indirect-call path. The
                // physical representation is identical — a closure
                // value is a 16-byte `(fn_ptr, captures_ptr)` heap
                // pair (see `closure.rs`), and `Ty::AnyMixin` lays out
                // as a 16-byte primitive (see `codegen/layout.rs:445`),
                // so slot 0 / slot 1 line up without a vtable. The
                // typeck-side unification in `typeck/unify.rs` is what
                // gets the closure literal into the dyn slot in the
                // first place.
                fn bounds_contain_fn(bounds: &[crate::hir::types::MixinRef]) -> bool {
                    bounds
                        .iter()
                        .any(|b| matches!(b.name.as_str(), "Fn" | "FnMut" | "FnOnce"))
                }
                fn ty_is_fn_like(ty: &Ty) -> bool {
                    match ty {
                        Ty::Fn { .. } | Ty::FnMut { .. } | Ty::FnOnce { .. } => true,
                        Ty::AnyMixin(bounds) | Ty::SomeMixin(bounds) => bounds_contain_fn(bounds),
                        Ty::Ref(inner)
                        | Ty::RefMut(inner)
                        | Ty::RefLifetime(_, inner)
                        | Ty::RefMutLifetime(_, inner) => ty_is_fn_like(inner),
                        _ => false,
                    }
                }
                let is_any_fn_type = matches!(
                    &object.ty,
                    Ty::AnyMixin(bounds) if bounds_contain_fn(bounds)
                );
                let is_ref_any_fn_type = matches!(&object.ty,
                    Ty::Ref(inner) | Ty::RefMut(inner)
                    if matches!(inner.as_ref(), Ty::AnyMixin(bounds) if bounds_contain_fn(bounds))
                );
                // For-loop bindings (and any other receiver whose HIR
                // expression type was left as `Ty::Infer` by typeck —
                // see `typeck/infer.rs::HirExprKind::For` which never
                // unifies the binding with the iterable's element
                // type) carry their real shape in the MIR local's
                // declared `ty`. Peek there as a fallback so a
                // dyn-erased closure dispatched through `for h in
                // hs.iter` reaches the indirect-call path instead of
                // falling through to a named call against a
                // non-existent `?T*_call` symbol.
                let local_ty_is_fn = matches!(&object.ty, Ty::Infer(_))
                    && obj_local
                        .map(|id| {
                            let func = self.fn_mut();
                            func.locals
                                .get(id as usize)
                                .map(|l| ty_is_fn_like(&l.ty))
                                .unwrap_or(false)
                        })
                        .unwrap_or(false);
                let is_fn_call = is_fn_type
                    || is_ref_fn_type
                    || is_any_fn_type
                    || is_ref_any_fn_type
                    || local_ty_is_fn
                    || type_name.starts_with("Fn(")
                    || type_name.starts_with("Fn[")
                    || type_name.starts_with("&Fn(")
                    || type_name.starts_with("&Fn[")
                    || type_name.starts_with("any Fn(")
                    || type_name.starts_with("any Fn[")
                    || type_name.starts_with("&any Fn(")
                    || type_name.starts_with("&any Fn[");

                if is_fn_call {
                    // The closure value is a heap pair {fn_ptr, captures_ptr}.
                    // Load both, then call indirectly with captures_ptr
                    // prepended to the user-visible arg list.
                    let pair = obj_local.unwrap_or_else(|| self.new_temp(Ty::Int));
                    let fn_ptr = self.new_temp(Ty::Int);
                    self.emit(MirInst::GetField {
                        dest: fn_ptr,
                        base: pair,
                        field_index: 0,
                    });
                    let cap_ptr = self.new_temp(Ty::Int);
                    self.emit(MirInst::GetField {
                        dest: cap_ptr,
                        base: pair,
                        field_index: 1,
                    });
                    // Drop the self-as-first-arg that method-call lowering
                    // prepended; replace it with captures_ptr.
                    let user_args: Vec<MirValue> = if !is_static && !arg_values.is_empty() {
                        arg_values.into_iter().skip(1).collect()
                    } else {
                        arg_values
                    };
                    let mut indirect_args = Vec::with_capacity(user_args.len() + 1);
                    indirect_args.push(MirValue::Use(cap_ptr));
                    indirect_args.extend(user_args);
                    self.emit(MirInst::CallIndirect {
                        dest,
                        callee: fn_ptr,
                        args: indirect_args,
                    });
                } else {
                    // #06.8 Phase 3b: rewrite the mangled `ClassName_method`
                    // callee to the linked C symbol when this method came
                    // from a class-body `lib` block (the alias map was
                    // populated by `register_class_lib_method` keyed on
                    // the same mangled shape). Non-FFI class methods hit
                    // the unwrap_or branch and use the mangled name
                    // unchanged.
                    //
                    // #06.8 T#17: for generic builtin receivers (`Ty::Option(Inner)`,
                    // `Ty::Result(Ok,Err)`, ...) the call-site mangle carries the
                    // surface generic args — `Option[Int]_unwrap_or`. The
                    // alias-map entry from a `class Option do lib ... end end`
                    // bootstrap shell is keyed without args: `Option_unwrap_or`.
                    // After the exact-key lookup misses, peel the `[...]` segment
                    // from `mangled` and retry — that's the generic-stripped
                    // shape the alias map carries.
                    let callee = self.resolve_ffi_alias_callee(mangled);
                    self.emit(MirInst::Call {
                        dest,
                        callee,
                        args: arg_values,
                    });
                }
                Ok(dest)
            }

            // ── Assignment ──────────────────────────────────────────
            _ => unreachable!("lower_method_call: dispatched to wrong helper"),
        }
    }
}
