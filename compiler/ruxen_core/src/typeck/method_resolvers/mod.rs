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

mod collections;
mod concurrency;
mod fmt;
mod fs;
mod io;
mod iter;
mod net;
mod numeric;
mod process;
mod resolver;
mod strings;
mod time;

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
    // The single, deliberate "nothing claimed it" — was the legacy
    // match's trailing `_ => None`.
    None
}

/// The ONE precedence decision. During the migration this delegates to a
/// single legacy-wrapping resolver; each migration task carves a
/// namespace out of the legacy match into its own slot here, at the
/// correct precedence position.
fn resolvers() -> Vec<MethodResolver> {
    let mut v = Vec::new();
    v.extend(resolver::declared_method_resolvers()); // TIER 1 — fixes A2
    v.extend(concurrency::resolvers()); // TIER 2 — named stdlib
    v.extend(fmt::resolvers());
    v.extend(io::resolvers());
    v.extend(fs::resolvers());
    v.extend(process::resolvers());
    v.extend(net::resolvers());
    v.extend(time::resolvers());
    v.extend(strings::resolvers()); // TIER 3 — structural
    v.extend(collections::resolvers());
    v.extend(numeric::resolvers());
    v.extend(iter::resolvers());
    v.extend(resolver::structural_fallback_resolvers()); // TIER 3 tail
    v
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

        // ── Ty::String structural ──────────────────────────────────
        for m in [
            "clone", "size", "empty?", "push_str", "trim", "to_lower", "to_upper", "chars",
            "split", "push", "as_str", "from", "include?", "starts_with", "ends_with", "repeat",
            "lines", "replace", "new", "with_capacity", "to_string", "bytes", "trim_start",
            "trim_end", "find", "splitn", "clear", "truncate", "insert", "insert_str", "remove",
            "parse_int", "parse_float", "into_bytes", "from_iter", "to_s",
        ] {
            v.push(c(Ty::String, m));
        }

        // ── Ty::Str structural ─────────────────────────────────────
        for m in [
            "size", "empty?", "trim", "to_lower", "to_upper", "chars", "split", "parse_uint",
            "as_str", "include?", "starts_with", "ends_with", "lines", "replace", "to_string",
            "bytes", "trim_start", "trim_end", "find", "splitn", "parse_int", "parse_float",
            "to_s",
        ] {
            v.push(c(Ty::Str, m));
        }

        // ── ParseIntError / ParseFloatError ────────────────────────
        v.push(c(class("ParseIntError", vec![]), "message"));
        v.push(c(class("ParseFloatError", vec![]), "message"));

        // ── Ty::Array structural ───────────────────────────────────
        let arr = || Ty::Array(Box::new(Ty::Int));
        for m in [
            "size", "empty?", "push", "pop", "get", "get_mut", "iter", "into_iter", "each", "map",
            "filter", "find", "position", "to_vec", "new", "sum", "count", "reverse", "first",
            "last", "clone", "include?", "sort", "join", "with_capacity", "capacity", "clear",
            "truncate", "swap", "insert", "remove", "extend", "iter_mut", "as_slice", "from_iter",
            "dedup", "sort_by", "retain",
        ] {
            v.push(c(arr(), m));
        }

        // ── Ty::Map structural ─────────────────────────────────────
        let map = || Ty::Map(Box::new(Ty::String), Box::new(Ty::Int));
        for m in [
            "new", "from_iter", "insert", "get", "key?", "size", "empty?", "with_capacity",
            "remove", "clear", "keys", "values", "iter",
        ] {
            v.push(c(map(), m));
        }

        // ── Ty::Set structural ─────────────────────────────────────
        let set = || Ty::Set(Box::new(Ty::Int));
        for m in [
            "new", "from_iter", "insert", "include?", "size", "empty?", "with_capacity", "remove",
            "clear", "iter", "union", "intersection", "difference",
        ] {
            v.push(c(set(), m));
        }

        // ── Ty::Option structural ──────────────────────────────────
        let opt = || Ty::Option(Box::new(Ty::Int));
        for m in [
            "try_op", "unwrap", "expect", "unwrap_or", "unwrap_or_else", "map", "ok_or", "is_some",
            "is_none",
        ] {
            v.push(c(opt(), m));
        }

        // ── Ty::Result structural ──────────────────────────────────
        let res = || Ty::Result(Box::new(Ty::Int), Box::new(Ty::String));
        for m in [
            "try_op", "unwrap", "expect", "unwrap_or", "unwrap_or_else", "map", "map_err", "is_ok",
            "is_err",
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
        v.push(c_args(class("SharedSync", vec![]), "new", vec![arg(Ty::Int)]));
        v.push(c(class("Arc", vec![Ty::Int]), "deref"));
        v.push(c(class("Arc", vec![Ty::Int]), "clone"));
        v.push(c(class("Arc", vec![Ty::Int]), "strong_count"));
        v.push(c(class("Arc", vec![Ty::Int]), "weak_count"));

        // ── fmt (TIER 2) ───────────────────────────────────────────
        for m in [
            "write_str", "write_char", "size", "width", "precision", "align", "fill",
        ] {
            v.push(c(class("Formatter", vec![]), m));
        }

        // ── *Iter combinators (TIER 2-ish, name.ends_with("Iter")) ──
        let veciter = || class("VecIter", vec![Ty::Int]);
        for m in [
            "filter", "map", "find", "position", "sum", "count", "fold", "all", "any", "take",
            "skip", "chain", "zip", "collect_vec", "to_vec", "enumerate", "partition",
        ] {
            v.push(c(veciter(), m));
        }
        // SplitIter.to_vec yields &str segments (distinct branch).
        v.push(c(class("SplitIter", vec![]), "to_vec"));

        // ── io: Stdin / Stdout / Stderr / IoError ──────────────────
        for m in ["read_line", "read_to_string", "lines"] {
            v.push(c(class("Stdin", vec![]), m));
        }
        for m in ["write_str", "flush", "print", "println"] {
            v.push(c(class("Stdout", vec![]), m));
        }
        for m in ["write_str", "flush", "eprint", "eprintln"] {
            v.push(c(class("Stderr", vec![]), m));
        }
        v.push(c(enum_ty("IoError"), "message"));
        v.push(c(enum_ty("IoError"), "kind"));

        // ── fs: Metadata / File / OpenOptions ──────────────────────
        for m in ["size", "modified", "is_file", "is_dir", "is_symlink"] {
            v.push(c(class("Metadata", vec![]), m));
        }
        for m in [
            "open", "create", "append", "open_options", "read", "read_to_string", "read_all",
            "write", "write_all", "write_str", "flush", "seek", "metadata", "close",
        ] {
            v.push(c(class("File", vec![]), m));
        }
        for m in [
            "read", "write", "append", "truncate", "create", "create_new",
        ] {
            v.push(c(class("OpenOptions", vec![]), m));
        }

        // ── process: Command / ExitStatus / Output ─────────────────
        for m in ["arg", "args", "env", "current_dir", "status", "output"] {
            v.push(c(class("Command", vec![]), m));
        }
        for m in ["code", "success"] {
            v.push(c(class("ExitStatus", vec![]), m));
        }
        for m in ["status", "stdout", "stderr"] {
            v.push(c(class("Output", vec![]), m));
        }

        // ── time: Duration / Instant ───────────────────────────────
        for m in [
            "from_secs", "from_millis", "from_micros", "from_nanos", "as_secs", "as_millis",
            "as_micros", "as_nanos", "add", "sub",
        ] {
            v.push(c(class("Duration", vec![]), m));
        }
        for m in ["now", "elapsed", "duration_since", "sub"] {
            v.push(c(class("Instant", vec![]), m));
        }

        // ── net: TcpListener / TcpStream ───────────────────────────
        for m in [
            "bind", "accept", "local_addr", "set_nonblocking", "close",
        ] {
            v.push(c(class("TcpListener", vec![]), m));
        }
        for m in [
            "connect", "read", "write", "peer_addr", "shutdown", "close", "set_read_timeout",
            "set_write_timeout",
        ] {
            v.push(c(class("TcpStream", vec![]), m));
        }

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
        v.push(c(class("BufReader", vec![class("File", vec![])]), "read_line"));
        v.push(c(class("BufReader", vec![class("File", vec![])]), "read"));
        v.push(c(class("BufReader", vec![class("File", vec![])]), "into_inner"));
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

        // ── Enum / numeric / scalar structural (TIER 3) ────────────
        v.push(c(enum_ty("Priority"), "weight"));
        v.push(c(Ty::Bool, "to_string"));
        v.push(c(Ty::Int, "to_string"));
        v.push(c(Ty::USize, "to_string"));
        v.push(c(Ty::Float, "to_string"));
        v.push(c(Ty::Int, "to_f"));
        v.push(c(Ty::Float, "to_i"));
        v.push(c(Ty::Int, "to_s"));
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
        v.push(c_args(class("Arc", vec![]), "new", vec![arg(class("NotSend", vec![]))]));
        // BufReader / BufWriter with an unsupported inner (Int) → E0714.
        v.push(c_args(class("BufReader", vec![]), "new", vec![arg(Ty::Int)]));
        v.push(c_args(
            class("BufReader", vec![]),
            "with_capacity",
            vec![arg(Ty::Int), arg(Ty::Int)],
        ));
        v.push(c_args(class("BufWriter", vec![]), "new", vec![arg(Ty::Int)]));
        v.push(c_args(
            class("BufWriter", vec![]),
            "with_capacity",
            vec![arg(Ty::Int), arg(Ty::Int)],
        ));
        // *Iter.sum on a non-numeric element type → E0700.
        v.push(c(class("VecIter", vec![Ty::String]), "sum"));

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
