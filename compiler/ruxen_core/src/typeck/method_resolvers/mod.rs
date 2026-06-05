//! Per-namespace method-resolver dispatch.
//!
//! Phase D of #06.75 carved this out of `typeck/infer.rs` (where it
//! lived as the 774-LOC `InferenceEngine::builtin_method_type` method,
//! a single `match (recv_ty, method_name) { … }` table).
//!
//! Phase 5 (thermonuke) completed the split this header originally
//! envisioned: the ~290-arm match is now an ORDERED resolver pipeline.
//! `builtin_method_type` walks `resolvers()`; the FIRST resolver whose
//! `resolve` returns `Some` wins, so precedence is ONE decision (the Vec
//! order) instead of being smeared across arm positions. Three tiers:
//!
//!   1. declared-method (`resolver::declared_method_resolvers`) — a user
//!      class's own `new` beats a builtin (bug A2 fix), scoped away from
//!      the stdlib types that own a payload-inferring named `new` arm.
//!   2. named-stdlib — `concurrency`/`fmt`/`io`/`fs`/`process`/`net`/
//!      `time`, each a `resolvers()` table keyed on the receiver class
//!      name (incl. the E1100/E1101/E1102/E0714 construction checks).
//!   3. structural — `strings`/`collections`/`numeric`/`iter` (keyed on
//!      `Ty` shape) + `resolver::structural_fallback_resolvers` (the
//!      generic Class/Struct/Enum `to_s`/`clone`/`new`/`default` tail).
//!
//! No resolver may match all receivers and no `_ => Some(...)` may claim
//! an arbitrary type; the only `_ => None` arms are within-namespace
//! "method not in this type" fallthroughs and the dispatcher's tail.

use crate::hir::nodes::*;
use crate::hir::types::Ty;
use crate::lexer::token::Span;

use super::infer::{is_bufio_inner_supported, is_iter_sum_compatible, InferenceEngine};

mod builtin_bridge;
mod collections;
mod concurrency;
mod io;
mod numeric;
mod resolver;
mod strings;

use resolver::MethodResolver;

/// The method-resolution entry point. Signature UNCHANGED — callers in
/// `infer/collect.rs:333` and `infer/expr.rs` stay byte-identical. Walks
/// the ordered resolver pipeline; the first resolver to return `Some`
/// wins. The pipeline (one ordered `Vec`) is the single precedence
/// decision, replacing the smear of ~290 arm positions.
pub(super) fn builtin_method_type(
    eng: &mut InferenceEngine<'_>,
    ty: &Ty,
    method: &str,
    args: &[HirExpr],
    span: &Span,
) -> Option<Ty> {
    for r in resolvers() {
        if (r.matches)(ty, method) {
            if let Some(ret) = (r.resolve)(eng, ty, method, args, span) {
                return Some(ret);
            }
        }
    }
    // (`resolvers()` returns a build-once `&'static [MethodResolver]`, so
    // this loop iterates a cached slice — no per-call Vec rebuild.)
    // The single, deliberate "nothing claimed it" — was the legacy
    // match's trailing `_ => None`.
    None
}

/// The ONE precedence decision. The assembled pipeline is immutable for
/// the life of the process (every stage is a pair of `fn` pointers with no
/// captured environment), so it is built exactly once into a `OnceLock`
/// and every `builtin_method_type` call iterates the cached slice. This
/// replaces the previous per-method-call rebuild (a `Vec` plus 13 `extend`
/// calls, each allocating a per-namespace `Vec`) — that work happened once
/// per method-call inference. Behaviour is identical: the slice order is
/// exactly the former `extend` order, which the golden parity test pins.
fn resolvers() -> &'static [MethodResolver] {
    static PIPELINE: std::sync::OnceLock<Vec<MethodResolver>> = std::sync::OnceLock::new();
    PIPELINE.get_or_init(|| {
        let mut v = Vec::new();
        v.extend(resolver::declared_method_resolvers()); // TIER 1 — fixes A2
                                                         // TIER 2 — named-stdlib residual. After the zero-Rust-stdlib
                                                         // migration (Phase B), only the resolvers carrying genuine
                                                         // compiler logic remain:
                                                         //   * `concurrency` — Mutex/Arc/JoinHandle generic-payload
                                                         //     substitution + the E1100/E1101/E1102 Send construction
                                                         //     checks (Problem-3 residual, not a static `.rx` return).
                                                         //   * `io` — BufReader/BufWriter `new`/`with_capacity` E0714
                                                         //     inner-type check + their generic-representation methods.
        v.extend(concurrency::resolvers());
        v.extend(io::resolvers());
        // TIER 3 — structural RESIDUAL resolvers. After the
        // zero-Rust-stdlib migration these carry ONLY the arms that
        // CANNOT be a static `.rx` return, and they MUST precede
        // `builtin_bridge` so those residuals win over the `.rx`
        // delegation for their shared heads:
        //   * `strings` — `String` ABI-divergent `remove`, E0722-blocked
        //     `clone`, structural-head `to_s`, mutation `push`/…, and all
        //     `&str` methods (no `class str`).
        //   * `collections` — `Array`/`Set`/`Map` CLOSURE combinators
        //     (`map`/`select`/`reduce`/…, MIR-inlined), the arg-dependent
        //     `zip`/`to_h`, the E0700 `sum` check, and `get_mut`/`get_var`
        //     (E0722 alias cluster); PLUS `Option`/`Result` (enum heads
        //     the bridge does not cover at all).
        //   * `numeric` — the `Enum.weight` accessor (scalar `to_s`/
        //     conversions migrate in Phase 3 but the enum arm stays).
        v.extend(strings::resolvers());
        v.extend(collections::resolvers());
        v.extend(numeric::resolvers());
        // The delegator: builtin heads (`String`/`Array`/`Set` so far)
        // whose non-residual methods were migrated to their `.rx`
        // method-home resolve here via `bridge_builtin_method` — zero
        // hardcoded method knowledge. Placed AFTER the residual
        // resolvers (so they shadow it) but still at the
        // inference-order-tolerant pipeline site (line 77), preserving
        // the fixpoint ordering the old arms relied on.
        v.extend(builtin_bridge::resolvers());
        // MIGRATED to `.rx` (resolve via `lookup_method_with_args` from the
        // general `DefKind::Method` path), resolver tables deleted:
        //   * `time`    — Duration/Instant       (library/std/time/src/lib.rx)
        //   * `fs`      — File/Metadata/OpenOptions
        //                 (library/std/io/src/{file,metadata,open_options}.rx)
        //   * `net`     — TcpListener/TcpStream   (library/std/net/src/lib.rx)
        //   * `process` — Command/ExitStatus/Output
        //                 (library/std/process/src/lib.rx)
        //   * `fmt`     — Formatter               (library/std/fmt/src/lib.rx)
        //   * `io` Stdin/Stdout/Stderr/IoError
        //                 (library/std/io/src/{stdin,stdout,stderr,lib}.rx)
        //
        // The `*Iter` combinator resolver was deleted with the rest of the
        // orphaned iterator machinery — `split`/`chars`/`lines`/`bytes`
        // return `Array`, nothing produces `VecIter`/`SplitIter`, and no
        // `.rx`/fixture calls `.iter`/`.into_iter`.
        v.extend(resolver::structural_fallback_resolvers()); // TIER 3 tail
        v
    })
}

#[cfg(test)]
mod golden {
    //! Golden parity corpus for `builtin_method_type` (Phase 5).
    //!
    //! This module is the migration oracle: it captures the CURRENT
    //! `match (ty, method)` answer (the `Option<Ty>` return AND any
    //! diagnostic codes pushed) for one representative triple per arm,
    //! freezes it in a committed snapshot, and asserts every later
    //! migration task reproduces it byte-for-byte. The corpus uses
    //! stdlib-named receivers with an EMPTY symbol table (no user-declared
    //! methods), so it pins stdlib/structural behaviour only — bug A2
    //! (user class shadowing a stdlib name) is deliberately NOT exercised
    //! here and is pinned separately.
    //!
    //! Record mode (regenerate the snapshot from the current code):
    //!   `RECORD_GOLDEN=1 cargo test -p ruxen_core --lib \
    //!        method_resolvers::golden::golden_parity -- --nocapture`
    //! Assert mode (default): the test compares against the committed
    //! snapshot and fails on any divergence.

    use super::*;
    use crate::hir::context::TypeContext;
    use crate::hir::nodes::{HirExpr, HirExprKind};
    use crate::hir::types::Ty;
    use crate::lexer::token::Span;
    use crate::resolve::symbols::SymbolTable;
    use crate::typeck::infer::InferenceEngine;
    use crate::typeck::mixins::MixinResolver;

    fn span() -> Span {
        Span {
            start: 0,
            end: 0,
            line: 1,
            column: 1,
        }
    }

    /// An argument expression whose `.ty` is what the effectful arms read
    /// (e.g. `Mutex.new` reads `args[0].ty`). The `kind` is irrelevant to
    /// the resolvers except `Thread.spawn`'s closure-capture scan, which
    /// is exercised with a dedicated closure-free arg below (a non-closure
    /// arg simply skips the E1100 capture loop).
    fn arg(ty: Ty) -> HirExpr {
        HirExpr {
            kind: HirExprKind::UnitLiteral,
            ty,
            span: span(),
        }
    }

    fn class(name: &str, generic_args: Vec<Ty>) -> Ty {
        Ty::Class {
            name: name.to_string(),
            generic_args,
        }
    }

    fn enum_ty(name: &str) -> Ty {
        Ty::Enum {
            name: name.to_string(),
            generic_args: vec![],
        }
    }

    fn struct_ty(name: &str) -> Ty {
        Ty::Struct {
            name: name.to_string(),
            generic_args: vec![],
        }
    }

    /// One corpus triple: receiver, method, and the args the (effectful)
    /// arm inspects. Most arms ignore args, so the common case is `&[]`.
    struct Case {
        recv: Ty,
        method: &'static str,
        args: Vec<HirExpr>,
    }

    fn c(recv: Ty, method: &'static str) -> Case {
        Case {
            recv,
            method,
            args: vec![],
        }
    }

    fn c_args(recv: Ty, method: &'static str, args: Vec<HirExpr>) -> Case {
        Case { recv, method, args }
    }

    /// The full corpus — one entry per distinct `(Ty-head, guard, method)`
    /// arm in `mod.rs`. Effectful arms carry the args they read. Built
    /// programmatically per receiver group; transcribed from the live
    /// arms, then cross-checked for completeness against the source's
    /// method-literal set by `golden_covers_every_method`.
    fn corpus() -> Vec<Case> {
        let mut v: Vec<Case> = Vec::new();

        // ── Ty::String delegated to .rx (string.rx via builtin_bridge) ─
        // Most `String` methods resolve from string.rx through the
        // delegator; the golden runs with an EMPTY symbol table (no `.rx`
        // loaded) so those would return None and are not pinned here. The
        // residual arms that stay Rust-side (strings.rs) ARE pinned:
        // `remove` (ABI divergence), `clone` (E0722 alias), `to_s`
        // (structural-head), and the mutation methods `push`/`push_str`/
        // `insert`/`insert_str` (surface `Unit` vs C `char*`). End-to-end
        // `.rx` resolution is covered by the builtin_receiver_bridge pins
        // + the e2e suite.
        for m in [
            "remove",
            "clone",
            "to_s",
            "push",
            "push_str",
            "insert",
            "insert_str",
        ] {
            v.push(c(Ty::String, m));
        }

        // ── Ty::Str structural ─────────────────────────────────────
        for m in [
            "size",
            "empty?",
            "trim",
            "to_lower",
            "to_upper",
            "chars",
            "split",
            "parse_uint",
            "as_str",
            "include?",
            "starts_with",
            "ends_with",
            "lines",
            "replace",
            "to_string",
            "bytes",
            "trim_start",
            "trim_end",
            "find",
            "splitn",
            "parse_int",
            "parse_float",
            "to_s",
        ] {
            v.push(c(Ty::Str, m));
        }

        // ── ParseIntError / ParseFloatError migrated to .rx ────────
        // `.message` resolves from string/src/parse_{int,float}_error.rx;
        // the resolver arms were deleted.

        // ── Ty::Array structural RESIDUAL ──────────────────────────
        // The migrated Array methods (size/empty?/push/pop/get/first/
        // last/include?/clone/to_a/reverse/sort/join/clear/truncate/
        // swap/insert/remove/extend/dedup/take/drop/chain/to_set/new/
        // with_capacity/capacity/count) resolve from array.rx via the
        // bridge; the golden runs with an EMPTY symbol table (no `.rx`),
        // so those return None and are NOT pinned here. Only the residual
        // arms in collections.rs (closure combinators, arg-dependent
        // `zip`/`to_h`, the E0700 `sum`, and the E0722 `get_mut` alias)
        // resolve against the empty table and ARE pinned. End-to-end
        // `.rx` resolution is covered by the builtin_receiver_bridge pins
        // + the e2e suite.
        let arr = || Ty::Array(Box::new(Ty::Int));
        for m in [
            "get_mut",
            "each",
            "each_with_index",
            "map",
            "select",
            "reject",
            "reduce",
            "all?",
            "any?",
            "find",
            "index",
            "partition",
            "zip",
            "to_h",
            "sum",
            "sort_by",
            "select!",
        ] {
            v.push(c(arr(), m));
        }

        // ── Ty::Map structural — fully migrated to map.rx (class Hash) ─
        // Every Map method (new/with_capacity/size/empty?/get/key?/keys/
        // values/to_a/insert/remove/clear) resolves from map.rx via the
        // bridge; with an EMPTY symbol table they return None, so there
        // is NOTHING to pin here. End-to-end resolution is covered by
        // `hash_resolves_to_ty_map` + the e2e suite.

        // ── Ty::Set structural — fully migrated to set.rx ──────────
        // Every Set method (new/with_capacity/size/empty?/include?/to_a/
        // union/intersection/difference/insert/remove/clear) resolves
        // from set.rx via the bridge; with an EMPTY symbol table they
        // return None, so there is NOTHING to pin here. End-to-end
        // resolution is covered by `set_methods_resolve_via_general_path`
        // + the e2e suite.

        // ── Ty::Option structural ──────────────────────────────────
        let opt = || Ty::Option(Box::new(Ty::Int));
        for m in [
            "try_op",
            "unwrap",
            "expect",
            "unwrap_or",
            "unwrap_or_else",
            "map",
            "ok_or",
            "nil?",
            "present?",
        ] {
            v.push(c(opt(), m));
        }

        // ── Ty::Result structural ──────────────────────────────────
        let res = || Ty::Result(Box::new(Ty::Int), Box::new(Ty::String));
        for m in [
            "try_op",
            "unwrap",
            "expect",
            "unwrap_or",
            "unwrap_or_else",
            "map",
            "map_err",
            "ok?",
            "err?",
        ] {
            v.push(c(res(), m));
        }

        // ── concurrency (TIER 2) ───────────────────────────────────
        v.push(c(class("Future", vec![Ty::Int]), "await"));
        // Thread.spawn with a non-closure arg (skips the E1100 loop, hits
        // the fresh-var output fallback).
        v.push(c_args(class("Thread", vec![]), "spawn", vec![arg(Ty::Int)]));
        v.push(c(class("Thread", vec![]), "current"));
        v.push(c(class("Thread", vec![]), "sleep"));
        v.push(c(class("Thread", vec![]), "yield_now"));
        v.push(c(class("Thread", vec![]), "id"));
        v.push(c(class("Thread", vec![]), "name"));
        v.push(c(class("JoinHandle", vec![Ty::Int]), "join"));
        v.push(c(class("JoinHandle", vec![Ty::Int]), "join!"));
        v.push(c(class("JoinHandle", vec![Ty::Int]), "thread_id"));
        v.push(c(class("ThreadPanic", vec![]), "message"));
        // Mutex.new with a Send payload (Int) — no E1101.
        v.push(c_args(class("Mutex", vec![]), "new", vec![arg(Ty::Int)]));
        v.push(c(class("Mutex", vec![Ty::Int]), "lock"));
        v.push(c(class("Mutex", vec![Ty::Int]), "lock!"));
        v.push(c(class("Mutex", vec![Ty::Int]), "try_lock"));
        v.push(c(class("Mutex", vec![Ty::Int]), "into_inner"));
        v.push(c(class("MutexGuard", vec![Ty::Int]), "deref"));
        v.push(c(class("MutexGuard", vec![Ty::Int]), "deref_mut"));
        v.push(c(class("MutexGuard", vec![Ty::Int]), "deref_var"));
        // Arc / SharedSync.new with a Send payload (Int) — no E1102.
        v.push(c_args(class("Arc", vec![]), "new", vec![arg(Ty::Int)]));
        v.push(c_args(
            class("SharedSync", vec![]),
            "new",
            vec![arg(Ty::Int)],
        ));
        v.push(c(class("Arc", vec![Ty::Int]), "deref"));
        v.push(c(class("Arc", vec![Ty::Int]), "clone"));
        v.push(c(class("Arc", vec![Ty::Int]), "strong_count"));
        v.push(c(class("Arc", vec![Ty::Int]), "weak_count"));

        // ── fmt (Formatter) migrated to .rx ────────────────────────
        // Resolve from `library/std/fmt/src/lib.rx` via
        // `lookup_method_with_args`; the fmt resolver was deleted.

        // ── *Iter combinators removed (orphaned iterator machinery) ─
        // The `iter` resolver was deleted; nothing produces VecIter/
        // SplitIter and no surface calls `.iter`/`.into_iter`.

        // ── io: Stdin / Stdout / Stderr / IoError migrated to .rx ──
        // Resolve from `library/std/io/src/{stdin,stdout,stderr,lib}.rx`
        // via `lookup_method_with_args`; those io resolver arms were
        // deleted. BufReader / BufWriter remain Rust residual (E0714 +
        // generic representation) — exercised below.

        // ── fs (Metadata / File / OpenOptions) migrated to .rx ─────
        // Those methods resolve from
        // `library/std/io/src/{file,metadata,open_options}.rx` via
        // `lookup_method_with_args`; the fs resolver was deleted, so the
        // golden corpus (empty symbol table) no longer pins them.

        // ── process (Command/ExitStatus/Output) migrated to .rx ────
        // Resolve from `library/std/process/src/lib.rx`.

        // ── net (TcpListener/TcpStream) migrated to .rx ────────────
        // Resolve from `library/std/net/src/lib.rx`.

        // ── BufReader / BufWriter (effectful E0714) ────────────────
        // `new` with a supported inner (File) — no diagnostic.
        v.push(c_args(
            class("BufReader", vec![]),
            "new",
            vec![arg(class("File", vec![]))],
        ));
        // `with_capacity(cap, inner)` reads args[1].
        v.push(c_args(
            class("BufReader", vec![]),
            "with_capacity",
            vec![arg(Ty::Int), arg(class("File", vec![]))],
        ));
        v.push(c(
            class("BufReader", vec![class("File", vec![])]),
            "read_line",
        ));
        v.push(c(class("BufReader", vec![class("File", vec![])]), "read"));
        v.push(c(
            class("BufReader", vec![class("File", vec![])]),
            "into_inner",
        ));
        v.push(c_args(
            class("BufWriter", vec![]),
            "new",
            vec![arg(class("File", vec![]))],
        ));
        v.push(c_args(
            class("BufWriter", vec![]),
            "with_capacity",
            vec![arg(Ty::Int), arg(class("File", vec![]))],
        ));
        for m in ["write", "write_all", "write_str", "flush", "into_inner"] {
            v.push(c(class("BufWriter", vec![class("File", vec![])]), m));
        }

        // ── Enum / numeric / scalar structural RESIDUAL (TIER 3) ───
        // Int `to_s` / `to_string` / `to_f` MIGRATED to scalar.rx
        // (`class Int`) and resolve via the bridge; with an EMPTY symbol
        // table they return None, so they are NOT pinned here. The
        // ABI-divergent Float/Bool/Char and the class-less USize
        // residuals stay in numeric.rs and ARE pinned.
        v.push(c(enum_ty("Priority"), "weight"));
        v.push(c(Ty::Bool, "to_string"));
        v.push(c(Ty::USize, "to_string"));
        v.push(c(Ty::Float, "to_string"));
        v.push(c(Ty::Float, "to_i"));
        v.push(c(Ty::USize, "to_s"));
        v.push(c(Ty::Float, "to_s"));
        v.push(c(Ty::Bool, "to_s"));
        v.push(c(Ty::Char, "to_s"));
        // generic to_s / clone / new / default fallbacks
        v.push(c(class("Widget", vec![]), "to_s"));
        v.push(c(struct_ty("Point"), "to_s"));
        v.push(c(enum_ty("Color"), "to_s"));
        v.push(c(class("Widget", vec![]), "new"));
        v.push(c(class("Widget", vec![]), "clone"));
        v.push(c(struct_ty("Point"), "new"));
        v.push(c(struct_ty("Point"), "clone"));
        v.push(c(enum_ty("Color"), "clone"));
        v.push(c(struct_ty("Point"), "default"));
        v.push(c(class("Widget", vec![]), "default"));

        // ── Effectful-arm DIAGNOSTIC pins ──────────────────────────
        // These exercise the early-return + pushed-diagnostic branches
        // of the effectful arms, so the oracle captures the error code
        // AND the `Some(Ty::Error)` return. With an empty symbol table an
        // unknown class is non-Send, so `Mutex.new(NotSend)` fires E1101.
        v.push(c_args(
            class("Mutex", vec![]),
            "new",
            vec![arg(class("NotSend", vec![]))],
        ));
        v.push(c_args(
            class("SharedSync", vec![]),
            "new",
            vec![arg(class("NotSend", vec![]))],
        ));
        v.push(c_args(
            class("Arc", vec![]),
            "new",
            vec![arg(class("NotSend", vec![]))],
        ));
        // BufReader / BufWriter with an unsupported inner (Int) → E0714.
        v.push(c_args(
            class("BufReader", vec![]),
            "new",
            vec![arg(Ty::Int)],
        ));
        v.push(c_args(
            class("BufReader", vec![]),
            "with_capacity",
            vec![arg(Ty::Int), arg(Ty::Int)],
        ));
        v.push(c_args(
            class("BufWriter", vec![]),
            "new",
            vec![arg(Ty::Int)],
        ));
        v.push(c_args(
            class("BufWriter", vec![]),
            "with_capacity",
            vec![arg(Ty::Int), arg(Ty::Int)],
        ));
        v
    }

    /// Build a minimal real engine with an EMPTY symbol table. With no
    /// user-declared methods, `lookup_class_method_return` returns `None`,
    /// so the declared-`new` arm (mod.rs:1173) falls through to the
    /// stdlib/structural answer — exactly the oracle we want to pin.
    fn run_case(case: &Case) -> (Option<Ty>, Vec<String>) {
        let mut ctx = TypeContext::new();
        let mut symbols = SymbolTable::new();
        let traits = MixinResolver::new();
        let mut eng = InferenceEngine::new(&mut ctx, &mut symbols, &traits);
        let ret = builtin_method_type(&mut eng, &case.recv, case.method, &case.args, &span());
        let codes: Vec<String> = eng
            .diagnostics
            .iter()
            .filter_map(|d| d.code.clone())
            .collect();
        (ret, codes)
    }

    fn render(case: &Case, ret: &Option<Ty>, codes: &[String]) -> String {
        format!(
            "{:?} :: {} => {:?} | diag={:?}",
            case.recv, case.method, ret, codes
        )
    }

    fn snapshot_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/method_resolver_golden.snapshot")
    }

    /// The parity oracle. In record mode (`RECORD_GOLDEN=1`) it writes the
    /// snapshot; otherwise it asserts each case matches the committed line.
    #[test]
    fn golden_parity() {
        let cases = corpus();
        let lines: Vec<String> = cases
            .iter()
            .map(|case| {
                let (ret, codes) = run_case(case);
                render(case, &ret, &codes)
            })
            .collect();

        if std::env::var("RECORD_GOLDEN").is_ok() {
            std::fs::write(snapshot_path(), format!("{}\n", lines.join("\n")))
                .expect("write golden snapshot");
            eprintln!("recorded {} golden lines", lines.len());
            return;
        }

        let expected = std::fs::read_to_string(snapshot_path())
            .expect("golden snapshot missing — run with RECORD_GOLDEN=1 first");
        let expected_lines: Vec<&str> = expected.lines().collect();
        assert_eq!(
            lines.len(),
            expected_lines.len(),
            "corpus size ({}) != snapshot size ({}); re-record if you added arms",
            lines.len(),
            expected_lines.len()
        );
        for (i, (got, want)) in lines.iter().zip(expected_lines.iter()).enumerate() {
            assert_eq!(
                got, want,
                "golden parity divergence at corpus index {i}:\n  got:  {got}\n  want: {want}"
            );
        }
    }

    /// Task 2: the dispatcher (`resolvers()` walked by `builtin_method_type`)
    /// must reproduce the legacy match's answer for the whole corpus. Since
    /// `builtin_method_type` IS the dispatcher after Task 2, this asserts the
    /// dispatcher is behaviour-identical to the legacy match it wraps, and
    /// that the resolver pipeline is actually assembled (non-empty).
    #[test]
    fn dispatcher_matches_legacy_for_whole_corpus() {
        assert!(
            !super::resolvers().is_empty(),
            "dispatcher must assemble at least one resolver"
        );
        // Re-run the corpus through the public dispatcher entry and compare
        // to the frozen oracle — identical assertion to `golden_parity`,
        // named to document that the dispatcher path is what's exercised.
        let cases = corpus();
        let expected = std::fs::read_to_string(snapshot_path())
            .expect("golden snapshot missing — run with RECORD_GOLDEN=1 first");
        let expected_lines: Vec<&str> = expected.lines().collect();
        for (i, case) in cases.iter().enumerate() {
            let (ret, codes) = run_case(case);
            assert_eq!(
                render(case, &ret, &codes),
                expected_lines[i],
                "dispatcher diverged from legacy at corpus index {i}"
            );
        }
    }

    /// Task 2 — bug-A2 precedence DECISION (decided empirically, per plan).
    ///
    /// The plan posed: is the real stdlib `Mutex.new` a builtin arm only
    /// (no symbol-table `DefKind::Method`)? If so, tier-1-first would be
    /// safe. This test records the empirical answer: it is NOT — the
    /// stdlib `class Mutex[T]` in `library/std/sync/src/mutex.rx` declares
    /// `def self.new as "ruxen_mutex_new"(initial: T) -> Mutex[T]`, so
    /// `lookup_class_method_return("Mutex", "new")` returns `Some(Mutex[T])`.
    ///
    /// CONSEQUENCE (load-bearing for Task 3): tier-1-first is UNSAFE — a
    /// declared-method resolver running before the named-stdlib arms would
    /// return the unsubstituted `Mutex[T]` instead of the named arm's
    /// payload-inferred `Mutex[<args[0].ty>]` + E1101 Send check, changing
    /// stdlib behaviour and breaking the golden corpus. Therefore tier 1
    /// (declared-method) MUST be scoped to USER-DEFINED receivers only: it
    /// skips any receiver whose name is in the stdlib type-name set
    /// (`resolver::STDLIB_TYPE_NAMES`). This test pins that precondition so
    /// a future change to the stdlib surface that removed the declared
    /// `new` would re-open the tier-1-first option deliberately.
    #[test]
    fn stdlib_mutex_new_is_a_declared_method_so_tier1_is_user_scoped() {
        let mut bootstrap_diagnostics = Vec::new();
        let bootstrap_packages =
            crate::resolve::bootstrap::run_bootstrap_with_package_names(&mut bootstrap_diagnostics);
        // A trivial user program; we only need the stdlib symbols loaded.
        let src = "def main\nend\n";
        let mut lx = crate::lexer::Lexer::new(src);
        let toks = lx.tokenize().expect("lex");
        let mut p = crate::parser::Parser::new(toks);
        let prog = p.parse().expect("parse");
        let resolver = crate::resolve::Resolver::new();
        let result = resolver.resolve_with_bootstrap_packages(&prog, &bootstrap_packages);

        let mut ctx = result.type_context;
        let mut symbols = result.symbols;
        let traits = MixinResolver::new();
        let eng = InferenceEngine::new(&mut ctx, &mut symbols, &traits);
        let mutex_new = eng.lookup_class_method_return("Mutex", "new");
        assert!(
            matches!(mutex_new, Some(Ty::Class { ref name, .. }) if name == "Mutex"),
            "stdlib Mutex declares its own `new` (FFI) returning Mutex[T]; \
             tier-1 must therefore be scoped to user-defined receivers only. \
             got {mutex_new:?}"
        );
    }

    /// Resolve `builtin_method_type(receiver, method, [])` against a real
    /// engine built from the given user `src` (no stdlib prelude — the
    /// program is self-contained), so a user-declared `new` is present in
    /// the symbol table for the declared-method tier to find.
    fn resolve_method_in_program(src: &str, recv: Ty, method: &str) -> Option<Ty> {
        let mut lx = crate::lexer::Lexer::new(src);
        let toks = lx.tokenize().expect("lex");
        let mut p = crate::parser::Parser::new(toks);
        let prog = p.parse().expect("parse");
        let resolver = crate::resolve::Resolver::new();
        let result = resolver.resolve(&prog);
        let mut ctx = result.type_context;
        let mut symbols = result.symbols;
        let traits = MixinResolver::new();
        let mut eng = InferenceEngine::new(&mut ctx, &mut symbols, &traits);
        builtin_method_type(&mut eng, &recv, method, &[], &span())
    }

    /// Bug A2 (the ONE intended behaviour change in Phase 5): a user class
    /// that declares its own `new` returning a `Result` must honour that
    /// DECLARED return type, not be overridden by the builtin's structural
    /// `Some(Self)` constructor fallback.
    ///
    /// Collision choice (per the Task 11 implementer obligation): verified
    /// against the real fs.rs arms — the stdlib `File` arms claim
    /// `open`/`create`/`read`/`metadata`/… but NOT `new` (there is no
    /// named `File.new` arm; `File.new` flowed to the generic
    /// `(Ty::Class, "new")` declared-lookup at legacy `mod.rs:1214`). So
    /// the real, fixable A2 collision IS `new` via the structural
    /// fallback, exactly the case the plan flagged as "correct as written".
    /// We exercise it with BOTH a stdlib-shaped name (`File`, the plan's
    /// example) and a plain user name to show the fix is general.
    ///
    /// Before Phase 5 the declared-`new` lookup sat positionally AFTER the
    /// generic structural arm only in spirit; the tier split now guarantees
    /// declared (tier 1) precedes the structural fallback (tier 3 tail).
    #[test]
    fn user_class_named_like_stdlib_honours_declared_new_return_a2() {
        // A user class literally named `File` (a stdlib type name, but NOT
        // one with a payload-inferring named `new` arm) declaring its own
        // `self.new -> Result[File, String]`.
        let src = "\
class File
  def self.new -> Result[File, String]
    Err(\"nope\")
  end
end
def main
  let _f = File.new
end
";
        let ret = resolve_method_in_program(src, class("File", vec![]), "new");
        assert!(
            matches!(ret, Some(Ty::Result(..))),
            "user-declared `File.new -> Result` must win over the builtin \
             structural `Some(Self)` fallback; got {ret:?}"
        );

        // And the same fix for an ordinary user name (no stdlib shadow at
        // all) — declared `new` is honoured.
        let src2 = "\
class Config
  def self.new -> Result[Config, String]
    Err(\"nope\")
  end
end
def main
  let _c = Config.new
end
";
        let ret2 = resolve_method_in_program(src2, class("Config", vec![]), "new");
        assert!(
            matches!(ret2, Some(Ty::Result(..))),
            "user-declared `Config.new -> Result` must be honoured; got {ret2:?}"
        );
    }

    /// Counterpart to A2: a stdlib type that owns a payload-inferring named
    /// `new` arm (`Mutex`) must keep that arm's behaviour — the named arm
    /// wins over the declared-method tier, so `Mutex.new(7)` is still
    /// `Mutex[Int]` (not the unsubstituted declared `Mutex[T]`). This is
    /// the non-contradiction the corpus and A2 must both satisfy.
    #[test]
    fn stdlib_named_new_arm_still_wins_over_declared() {
        let mut ctx = TypeContext::new();
        let mut symbols = SymbolTable::new();
        let traits = MixinResolver::new();
        let mut eng = InferenceEngine::new(&mut ctx, &mut symbols, &traits);
        let ret = builtin_method_type(
            &mut eng,
            &class("Mutex", vec![]),
            "new",
            &[arg(Ty::Int)],
            &span(),
        );
        assert!(
            matches!(ret, Some(Ty::Class { ref name, ref generic_args, .. })
                if name == "Mutex" && generic_args == &vec![Ty::Int]),
            "Mutex.new(Int) must stay the named arm's Mutex[Int]; got {ret:?}"
        );
    }

    /// Completeness backstop: every method-name literal that appears in a
    /// `mod.rs` arm pattern must be exercised by at least one corpus case.
    /// This turns "I forgot to transcribe an arm" from a silent gap into a
    /// test failure.
    #[test]
    fn golden_covers_every_method() {
        let src = include_str!("mod.rs");
        let mut source_methods: std::collections::BTreeSet<String> = Default::default();
        for line in src.lines() {
            let s = line.trim_start();
            if s.starts_with("(Ty::") || s.starts_with("| (Ty::") {
                // method literals: `, "name")` inside the tuple pattern.
                let mut rest = line;
                while let Some(pos) = rest.find(", \"") {
                    let after = &rest[pos + 3..];
                    if let Some(end) = after.find("\")") {
                        source_methods.insert(after[..end].to_string());
                        rest = &after[end + 2..];
                    } else {
                        break;
                    }
                }
            }
        }
        let covered: std::collections::BTreeSet<String> =
            corpus().iter().map(|c| c.method.to_string()).collect();
        let missing: Vec<&String> = source_methods.difference(&covered).collect();
        assert!(
            missing.is_empty(),
            "corpus is missing source-arm methods: {missing:?}"
        );
    }
}
