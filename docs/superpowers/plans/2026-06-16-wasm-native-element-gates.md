# WASM Native-Element Gates — De-Risk Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Settle the two L0 prerequisites for quiver's native-element web backend (ADR `quiver/docs/decisions/native-element-backend.md`) before any `ElementSurface` code is written: (1) can a DOM event be delivered to a Ruxen handler in **pure Ruxen** (Q42), and (2) does the **real quiver pipeline** (`paint_all[S: PaintSurface]` mixin-generic + a real `Mutex`/`SharedSync` reactive core) actually lower **and run** on LLVM/wasm?

**Architecture:** Three falsification spikes, each a self-contained `#[test]` added to `compiler/ruxen_core/tests/wasm_codegen.rs` (the in-process wasm e2e harness — release-e2e is native-only and **cannot** test wasm). Each spike compiles a `.rx` to a wasm32 object via the real LLVM backend, links with `wasm-ld`, runs in `node`, and asserts a result. **Every spike has two outcomes**: GREEN (capability confirmed → native-element work is a quiver feature) or RED (capability missing → a specific ruxen L0 task becomes the critical path). This is a spike plan: the deliverable is *knowledge + a pinned regression test*, and where a `.rx` shape is not yet certain, the spike result IS the finding.

**Tech Stack:** Rust (ruxen compiler, `--features llvm`), LLVM 18.1.8 (`/opt/homebrew/opt/llvm@18`), `wasm-ld`, `node`.

---

## RESULTS — EXECUTED 2026-06-16 · ALL THREE SPIKES GREEN

**Verdict: the native-element web path is a quiver feature, NOT gated on deep ruxen LLVM-backend work.** The feared Q41 vtable wall did not fire. The only ruxen-side additions are small/library-level.

| Spike | Outcome | Evidence |
|---|---|---|
| **A — pure-Ruxen cross-call dispatch** | ✅ GREEN | `boot()` returns a `Registry` heap handle; JS holds it; `dispatch_event(reg,0,1)` ×2 in **separate calls** → `read_count(reg)` = **2**. Zero imports. Closure stored in `Array[any Fn]`, called via `f.()`, mutation persists across calls. **Q42 is NOT L0 — pure-Ruxen pattern.** Repro: `tmp/spikeA/` (`spike.rx`, `run.mjs`). |
| **B — PaintSurface mixin-generic on wasm** | ✅ GREEN | `paint_all[S: PaintSurface]` with two implementors → `run_tally=7`, `run_doubler=62`. Q17 monomorphization survives LLVM/wasm; **no vtable/class_info `Err`**. The `ElementSurface` shape is identical → safe to build. Repro: `tmp/spikeB/`. |
| **C — reactive core (Mutex) on wasm** | ✅ GREEN (with a small runtime caveat) | `run()` = **41**. Mutex **compiles to wasm cleanly — no Q41 wall**. The pthread C runtime isn't bundled for wasm, so `ruxen_mutex_new/lock/guard_get/guard_set` become **host imports** (same pattern as canvas's `ruxen_canvas_*`); a thin single-threaded JS sync shim drives them. Repro: `tmp/spikeC/`. |

**Corrected harness reality (supersedes the per-task `wasm_codegen.rs` recipe below):**
- The in-process `compile_wasm_object` helper uses an **empty bootstrap** → no stdlib `Array`. Quiver-like code needs the **curated wasm bootstrap** (`typeck::type_check_wasm` → `run_wasm_bootstrap_with_package_names`) **plus the curated runtime-C**, which only the real cross-compile path assembles. So these spikes run via the **CLI**, not the in-process harness: `ruxen compile <f>.rx --target wasm32-unknown-unknown -o <f>.wasm` then `node run.mjs`. (Compiling a *new* `.rx` is not affected by the source-hash CLI cache.)
- **Toolchain gotcha (record for CI/dev):** the wasm runtime-C compile uses `RUXEN_WASM_CLANG` (default bare `clang`). On macOS that's Apple clang, which **lacks the WebAssembly target** → `No available targets are compatible with triple "wasm32-unknown-unknown"`. Set `RUXEN_WASM_CLANG=/opt/homebrew/opt/llvm@18/bin/clang` (and `RUXEN_WASM_LD=/opt/homebrew/opt/llvm@18/bin/wasm-ld`).
- The prebuilt `ruxen` binary (`target/release/ruxen`, `~/.ruxen/bin/ruxen`) was **stale** (built before the branch head). Rebuild from the branch: `cargo build -p ruxen_cli --features llvm` (debug, ~10s incremental).

**Ruxen-syntax facts learned (for the ElementSurface plan):** `init` is a **reserved word** (constructor) — don't name a `def` `init`. A class needs an **explicit `def init(...)` constructor** (no auto field-constructor; `Class.new` → `Class_init`). Methods **omit explicit `self`** (implicit; only `lib` externs list it). The empty-`Option` match arm is `nil ->` (not `None ->`). Mutating methods use `def var name`. `&var` of a `let` binding is rejected — use `var`.

**Decision-gate row reached: A=GREEN, B=GREEN, C=GREEN → write the `ElementSurface` mixin plan (ADR Phase 1).** Caveat folded into that plan: a wasm single-threaded sync shim (or `cfg(wasm)` sync runtime) for `Mutex`/`SharedSync` is needed — a stdlib/runtime task, not a compiler blocker.

---

## Why this plan exists / what the grounding changed

A principal-rust-engineer read of the ruxen tree (2026-06-16) established:

- **Q42 is almost certainly NOT compiler work.** Every top-level free `def` is auto-exported to wasm (`mir/lower/mod.rs:608-628` → `mir.wasm_exports`; `codegen/llvm/emit/mod.rs:78-96` sets the `wasm-export-name` attr). A Ruxen closure is a 16-byte `[fn_ptr, captures_ptr]` heap pair (`mir/lower/expr/closure.rs:13-24`); `f.()` already lowers on wasm to `build_indirect_call` (`codegen/llvm/emit/instructions.rs:566-619`). So an exported `def dispatch_event(...)` that looks up a stored closure in an `Array[any Fn]` and calls it needs **no** new export plumbing, no `__indirect_function_table`, no `call_indirect` trampoline.
- **The one real constraint:** Ruxen has **no top-level mutable global** (`const` is inlined at lowering). A handler registry must persist across two separate wasm export calls (register during `render`, fire on a later `click`) via a **heap handle passed back in by JS** — exactly canvas's opaque-handle inversion (a class lowers to `ptr`/i32, so an exported `def init() -> Registry` returns the heap pointer to JS as an i32, and `def dispatch_event(reg: Registry, ...)` accepts that i32 back). **This round-trip is the single unverified primitive** — and it is load-bearing for the *entire* web backend, not just events.
- **Phase 0.5's real risk is the reactive core, not PaintSurface.** `paint_all[S: PaintSurface]` is Q17-monomorphized → no vtable/class_info, so it satisfies the LLVM backend's hard guard (`codegen/llvm/mod.rs:98-105`, which `Err`s if `vtables`/`class_infos` are non-empty). `Mutex`/`SharedSync`/async carry `dispatch runtime` mixins + `__rx_classinfo_*` (the Q41 facet) and are where wasm lowering most likely breaks — either that compile-time `Err` or a runtime `unreachable` from a missing MIR terminator.

The order below front-loads the cheapest, most decisive experiment first.

---

## File Structure

- **Modify:** `compiler/ruxen_core/tests/wasm_codegen.rs` — add three `#[test]` fns (`pure_ruxen_dispatch_event_roundtrips_on_wasm`, `paint_surface_mixin_monomorphizes_on_wasm`, `reactive_core_mutex_roundtrips_on_wasm`). Reuse the existing `compile_wasm_object`, `link_wasm`, `find_wasm_ld`, `node_available`, `N` helpers verbatim (lines 27-109).
- **Cache:** `tmp/test-cache/wasm-gate-*.log` — one per spike run (global rule 41).
- **Update on completion:** `docs/dev/gui-stack-v1-issues.md` (Q42 status), `docs/TASKS.md` (Q42 line + outcomes), and — if a spike goes RED — a new `Q##` for the discovered L0 task. Cross-link `quiver/docs/decisions/native-element-backend.md`.

**No source edits to the compiler are expected by this plan.** If a spike's `.rx` cannot be made to compile, that compile failure IS the finding — file it as the L0 task; do not start editing `mir/`/`codegen/` inside this plan.

---

## Build / run reference (used by every task)

Narrow run (one test file — global rule 42), with the cache:

```bash
cd /Users/hassan/.projects/ruxen
mkdir -p tmp/test-cache
LLVM_SYS_180_PREFIX=/opt/homebrew/opt/llvm@18 \
  RUSTFLAGS="-L /opt/homebrew/opt/zstd/lib" \
  cargo test -p ruxen_core --features llvm --test wasm_codegen <TESTNAME> -- --nocapture \
  2>&1 | tee tmp/test-cache/wasm-gate-<tag>.log
```

`wasm-ld` is auto-found at `/opt/homebrew/opt/llvm@18/bin/wasm-ld` (or `RUXEN_WASM_LD`); absent toolchain/node → the test SKIPs (prints `SKIP:`), it does not fail. A real failure prints `WRONG`/`INVALID` or a Rust assert.

---

## Task 1: Spike A — pure-Ruxen cross-call closure dispatch on wasm (falsifies "Q42 is L0")

**Decisiveness:** This is the fork for the whole effort. GREEN ⇒ Q42 downgrades to a library pattern and the opaque-handle round-trip (the web backend's foundational primitive) is proven. RED ⇒ a specific L0 task is the critical path.

**Files:**
- Modify/Test: `compiler/ruxen_core/tests/wasm_codegen.rs` (add one `#[test]`)

- [ ] **Step 1: Write the spike test (failing — function/.rx not present yet)**

Add this test. The `.rx` is the best-effort starting shape; the closure-in-`Array` + capture + mutation surface is exactly what the spike probes (quiver already stores handlers as `Array[any Fn]`, the Q2-safe pattern, so this should be expressible — confirm empirically).

```rust
/// Q42 de-risk: prove a DOM-event-style callback works in PURE RUXEN on wasm —
/// register a handler closure during one exported call, then fire it from JS in
/// a SEPARATE later call, mutating Ruxen-side state. No compiler change, no
/// __indirect_function_table: just an exported `def`, an `Array[any Fn]`
/// registry, and `f.()` (which already lowers to a wasm `build_indirect_call`).
/// The registry persists across calls via a heap handle round-tripped through
/// JS as an i32 (a class lowers to ptr) — the opaque-handle inversion that the
/// whole web backend depends on.
#[test]
fn pure_ruxen_dispatch_event_roundtrips_on_wasm() {
    let src = "\
class Counter\n\
  n: Int\n\
end\n\
\n\
class Registry\n\
  count: Counter\n\
  handlers: Array[any Fn[]]\n\
end\n\
\n\
def init() -> Registry\n\
  let c = Counter.new(0)\n\
  let hs: Array[any Fn[]] = []\n\
  hs.push({ c.n = c.n + 1 })\n\
  Registry.new(c, hs)\n\
end\n\
\n\
def dispatch_event(reg: Registry, id: Int, kind: Int) -> Int\n\
  let h = reg.handlers.get(id)\n\
  h.()\n\
  0\n\
end\n\
\n\
def read_count(reg: Registry) -> Int\n\
  reg.count.n\n\
end\n";

    let obj = compile_wasm_object(src);
    assert!(!obj.is_empty(), "empty wasm object");

    let Some(wasm_path) = link_wasm(&obj) else {
        eprintln!("SKIP: wasm-ld not available");
        return;
    };
    if !node_available() {
        eprintln!("SKIP: node not available");
        let _ = std::fs::remove_dir_all(wasm_path.parent().unwrap());
        return;
    }

    // init() returns the Registry heap pointer as an i32 handle. JS holds it,
    // then calls dispatch_event(reg,0,1) in a SEPARATE call, then read_count.
    // A high-start bump allocator backs Counter/Array/Registry heap allocs.
    let script = format!(
        "const b=require('fs').readFileSync({:?});\
         let bump=65536;\
         const env={{ \
           ruxen_alloc:(n)=>{{const p=bump; bump+=Number(n); return p;}},\
           ruxen_dealloc:(_p)=>{{}} \
         }};\
         WebAssembly.instantiate(b,{{env}}).then(r=>{{\
           const e=r.instance.exports;\
           const reg=e.init();\
           e.dispatch_event(reg,0n,1n);\
           e.dispatch_event(reg,0n,1n);\
           const c=Number(e.read_count(reg));\
           if(c!==2){{console.error('WRONG count=',c);process.exit(3)}}\
           console.log('OK');\
         }}).catch(err=>{{console.error(err);process.exit(4)}});",
        wasm_path.to_string_lossy()
    );
    let out = Command::new("node").arg("-e").arg(&script).output().expect("spawn node");
    let _ = std::fs::remove_dir_all(wasm_path.parent().unwrap());
    assert!(
        out.status.success(),
        "pure-Ruxen cross-call dispatch failed on wasm: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("OK"),
        "expected OK, got: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}
```

- [ ] **Step 2: Run it**

```bash
cd /Users/hassan/.projects/ruxen
LLVM_SYS_180_PREFIX=/opt/homebrew/opt/llvm@18 RUSTFLAGS="-L /opt/homebrew/opt/zstd/lib" \
  cargo test -p ruxen_core --features llvm --test wasm_codegen \
  pure_ruxen_dispatch_event_roundtrips_on_wasm -- --nocapture \
  2>&1 | tee tmp/test-cache/wasm-gate-dispatch-v1.log
```

- [ ] **Step 3: Interpret the outcome (branch — this is the deliverable)**

- **GREEN (`OK`, count===2):** Q42 is a **pure-Ruxen library pattern**, the closure registry + `f.()` works on wasm, AND the heap-handle round-trip through JS works across calls. → Update `docs/dev/gui-stack-v1-issues.md` Q42 to `RESOLVED — not a compiler bug; pure-Ruxen dispatch pattern, pin in wasm_codegen.rs`; update `docs/TASKS.md`. The ADR's gate #1 is cleared. Proceed to Task 2.
- **RED — compile error** (`type errors`/`borrow errors`/`lower` panic from `compile_wasm_object`, or the `.rx` won't express closures-in-`Array`/capture/mutation): the exact error IS the finding. Common sub-cases and the L0 task each implies:
  - closures-in-`Array` payload corrupts (Q2-class bug on wasm) → file `Q43: any Fn in Array payload miscompiles on wasm`.
  - mutation `c.n = c.n + 1` through a captured class needs `&var`/different shape → adjust the `.rx` (try a `&var` capture or a `State`/cell) and re-run; if no Ruxen shape compiles, file the L0 task.
- **RED — runtime** (`WRONG`/trap/`exit 4`): the round-trip or indirect call fails at runtime. If the Registry handle does not survive the second call, the opaque-handle-as-i32 assumption is wrong → file `Q43: heap handle cannot round-trip through JS across wasm export calls` — this is the genuine L0 escalation, and it reshapes the web backend (the whole ADR depends on this primitive).

Record the outcome + chosen follow-up in the cache log header and `docs/TASKS.md`.

- [ ] **Step 4: Commit (the pinned spike + tracker update)**

```bash
cd /Users/hassan/.projects/ruxen
git add compiler/ruxen_core/tests/wasm_codegen.rs docs/dev/gui-stack-v1-issues.md docs/TASKS.md
git commit -m "test(wasm): pin pure-Ruxen cross-call dispatch_event de-risk (Q42)"
```

---

## Task 2: Spike B — `PaintSurface` mixin-generic monomorphizes and runs on wasm

**Decisiveness:** Proves the *render half* of the real quiver pipeline lowers to wasm. GREEN ⇒ the `ElementSurface` mixin (same Q17 shape) is safe to build. RED ⇒ Q17 monomorphization isn't firing on the wasm/LLVM path = L0.

**Files:**
- Modify/Test: `compiler/ruxen_core/tests/wasm_codegen.rs` (add one `#[test]`)

- [ ] **Step 1: Write the spike test**

Minimal mirror of quiver's `paint_all[S: PaintSurface]` shape: a `mixin` with one op, two implementors, and a generic free fn bounded by the mixin. The `compile_wasm_object` helper already **asserts `mir.vtables.is_empty() && mir.class_infos.is_empty()`** (lines 78-81) — so if Q17 mono fails to specialize and instead emits a runtime-dispatch vtable, this test fails *inside* `compile_wasm_object` with that assert. That assert firing IS a RED outcome.

```rust
/// Phase 0.5 (render half): the real quiver render path is `paint_all[S:
/// PaintSurface]` — a generic free fn over a mixin bound, Q17-monomorphized per
/// implementor at compile time (NO runtime vtable / class_info). Prove that
/// shape lowers AND runs on wasm. `compile_wasm_object` asserts vtables/
/// class_infos stay empty; if Q17 mono is not firing, that assert fires here.
#[test]
fn paint_surface_mixin_monomorphizes_on_wasm() {
    let src = "\
mixin PaintSurface\n\
  def emit(self, v: Int) -> Int\n\
end\n\
\n\
class Tally\n\
  include PaintSurface\n\
  total: Int\n\
  def emit(self, v: Int) -> Int\n\
    self.total = self.total + v\n\
    self.total\n\
  end\n\
end\n\
\n\
class Doubler\n\
  include PaintSurface\n\
  last: Int\n\
  def emit(self, v: Int) -> Int\n\
    self.last = v + v\n\
    self.last\n\
  end\n\
end\n\
\n\
def paint_all[S: PaintSurface](s: S, a: Int, b: Int) -> Int\n\
  s.emit(a)\n\
  s.emit(b)\n\
end\n\
\n\
def run_tally() -> Int\n\
  let t = Tally.new(0)\n\
  paint_all(t, 3, 4)\n\
end\n\
\n\
def run_doubler() -> Int\n\
  let d = Doubler.new(0)\n\
  paint_all(d, 10, 21)\n\
end\n";

    let obj = compile_wasm_object(src); // asserts vtables/class_infos empty
    assert!(!obj.is_empty(), "empty wasm object");

    let Some(wasm_path) = link_wasm(&obj) else {
        eprintln!("SKIP: wasm-ld not available");
        return;
    };
    if !node_available() {
        eprintln!("SKIP: node not available");
        let _ = std::fs::remove_dir_all(wasm_path.parent().unwrap());
        return;
    }

    let script = format!(
        "const b=require('fs').readFileSync({:?});\
         let bump=65536;\
         const env={{ ruxen_alloc:(n)=>{{const p=bump; bump+=Number(n); return p;}}, ruxen_dealloc:(_p)=>{{}} }};\
         WebAssembly.instantiate(b,{{env}}).then(r=>{{\
           const e=r.instance.exports;\
           const t=Number(e.run_tally());\
           const d=Number(e.run_doubler());\
           if(t!==7){{console.error('WRONG tally=',t);process.exit(3)}}\
           if(d!==42){{console.error('WRONG doubler=',d);process.exit(3)}}\
           console.log('OK');\
         }}).catch(err=>{{console.error(err);process.exit(4)}});",
        wasm_path.to_string_lossy()
    );
    let out = Command::new("node").arg("-e").arg(&script).output().expect("spawn node");
    let _ = std::fs::remove_dir_all(wasm_path.parent().unwrap());
    assert!(
        out.status.success(),
        "PaintSurface mixin-generic failed on wasm: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("OK"));
}
```

- [ ] **Step 2: Run it**

```bash
cd /Users/hassan/.projects/ruxen
LLVM_SYS_180_PREFIX=/opt/homebrew/opt/llvm@18 RUSTFLAGS="-L /opt/homebrew/opt/zstd/lib" \
  cargo test -p ruxen_core --features llvm --test wasm_codegen \
  paint_surface_mixin_monomorphizes_on_wasm -- --nocapture \
  2>&1 | tee tmp/test-cache/wasm-gate-paintsurface-v1.log
```

- [ ] **Step 3: Interpret**

- **GREEN (`OK`, 7 & 42):** the mixin-generic render path lowers + runs on wasm; `ElementSurface` (identical shape) is safe to build. Proceed to Task 3.
- **RED — `compile_wasm_object` panics on the `vtables/class_infos` assert:** Q17 mono is not specializing this shape on the wasm path; it fell back to runtime dispatch. File `Q43/Q44: generic-over-mixin free fn emits a runtime vtable instead of monomorphizing on the LLVM/wasm path` — this blocks BOTH PaintSurface and ElementSurface on wasm and is L0-critical.
- **RED — runtime `unreachable`/`WRONG`:** likely a missing MIR terminator or a mono call-site mis-redirect; capture the node `stderr` (a wasm trap prints an offset) and file the L0 bug with the minimal repro (this `.rx` already is minimal).

- [ ] **Step 4: Commit**

```bash
cd /Users/hassan/.projects/ruxen
git add compiler/ruxen_core/tests/wasm_codegen.rs
git commit -m "test(wasm): pin PaintSurface mixin-generic monomorphization de-risk (Phase 0.5 render)"
```

---

## Task 3: Spike C — real `Mutex`/`SharedSync` reactive core round-trips on wasm

**Decisiveness:** The deepest risk. quiver's `State[T]` is `SharedSync[Mutex[T]]`; if that hits the LLVM backend's vtable/class_info `Err` (`codegen/llvm/mod.rs:98-105`, the Q41 facet) the entire native-element effort is gated on ruxen LLVM-backend work, not quiver. **This spike needs the stdlib bootstrap** (`Mutex`/`SharedSync` are stdlib classes), so it does NOT use the no-bootstrap `compile_wasm_object` — it needs a bootstrapped compile path. Confirm whether the wasm pipeline can carry the curated bootstrap (the branch's `tier4.09` work added a curated stdlib bootstrap for wasm); if `Mutex` is excluded from that curated subset, that exclusion is itself the finding.

**Files:**
- Modify/Test: `compiler/ruxen_core/tests/wasm_codegen.rs` (add one `#[test]` + a bootstrapped compile variant)

- [ ] **Step 1: Determine the bootstrapped wasm compile path**

`compile_wasm_object` passes an **empty** bootstrap (`type_check_with_bootstrap(&program, &[])`) and asserts no vtables — which a real `Mutex` would violate. Read how the curated wasm bootstrap is assembled on this branch (the `tier4.09` "curated stdlib bootstrap for wasm" commit `f9ce6da`; search `compiler/`/`library/` for the curated-subset list and whether `Mutex`/`SharedSync` are in it):

```bash
cd /Users/hassan/.projects/ruxen
grep -rn "curated\|bootstrap_subset\|wasm.*bootstrap\|Mutex" compiler/ruxen_core/src/ library/std/ | grep -i wasm | head -40
```

If `Mutex`/`SharedSync` are **excluded** from the curated wasm subset → that is a RED finding by itself (the reactive core can't even be bootstrapped on wasm yet): file the L0 task and stop. If included, write a `compile_wasm_object_bootstrapped(src)` variant that passes the curated bootstrap instead of `&[]` and does NOT assert empty vtables.

- [ ] **Step 2: Write the spike test**

```rust
/// Phase 0.5 (reactive half): quiver's State[T] = SharedSync[Mutex[T]]. Prove a
/// real Mutex round-trips on wasm: construct, lock, mutate, read. This is where
/// the LLVM backend's vtable/class_info hard-Err (codegen/llvm/mod.rs:98, the
/// Q41 facet) most likely fires — if so, the reactive core requires L0 work.
#[test]
fn reactive_core_mutex_roundtrips_on_wasm() {
    // Smallest reactive-core shape. Exact Mutex API per library/std (confirm the
    // constructor/lock/get spelling against the stdlib source before running).
    let src = "\
def run() -> Int\n\
  let m = Mutex.new(0)\n\
  m.lock()\n\
  m.set(m.get() + 41)\n\
  m.unlock()\n\
  m.get()\n\
end\n";

    // Uses the bootstrapped variant (Step 1). If Step 1 found Mutex excluded
    // from the curated wasm subset, this test is `return;`-stubbed with the
    // finding recorded instead.
    let obj = compile_wasm_object_bootstrapped(src);
    assert!(!obj.is_empty(), "empty wasm object");

    let Some(wasm_path) = link_wasm(&obj) else { eprintln!("SKIP: wasm-ld"); return; };
    if !node_available() { eprintln!("SKIP: node"); let _ = std::fs::remove_dir_all(wasm_path.parent().unwrap()); return; }

    let script = format!(
        "const b=require('fs').readFileSync({:?});\
         let bump=65536;\
         const env={{ ruxen_alloc:(n)=>{{const p=bump; bump+=Number(n); return p;}}, ruxen_dealloc:(_p)=>{{}} }};\
         WebAssembly.instantiate(b,{{env}}).then(r=>{{\
           const e=r.instance.exports;\
           const v=Number(e.run());\
           if(v!==41){{console.error('WRONG v=',v);process.exit(3)}}\
           console.log('OK');\
         }}).catch(err=>{{console.error(err);process.exit(4)}});",
        wasm_path.to_string_lossy()
    );
    let out = Command::new("node").arg("-e").arg(&script).output().expect("spawn node");
    let _ = std::fs::remove_dir_all(wasm_path.parent().unwrap());
    assert!(out.status.success(),
        "Mutex round-trip failed on wasm: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("OK"));
}
```

- [ ] **Step 3: Run it**

```bash
cd /Users/hassan/.projects/ruxen
LLVM_SYS_180_PREFIX=/opt/homebrew/opt/llvm@18 RUSTFLAGS="-L /opt/homebrew/opt/zstd/lib" \
  cargo test -p ruxen_core --features llvm --test wasm_codegen \
  reactive_core_mutex_roundtrips_on_wasm -- --nocapture \
  2>&1 | tee tmp/test-cache/wasm-gate-mutex-v1.log
```

- [ ] **Step 4: Interpret (the critical-path decision)**

- **GREEN (`OK`, 41):** the reactive core runs on wasm. Combined with A+B green, **the native-element web path is a quiver feature, not blocked on L0** — proceed to the next plan (`ElementSurface` mixin).
- **RED — `mixin-vtables: LLVM backend does not yet emit vtable / class_info` `Err`** (from `compile_wasm_object_bootstrapped`): this is the predicted Q41 wall. **The critical path shifts to ruxen L0:** the LLVM backend must emit `__rx_classinfo_*` / vtable data sections for `dispatch runtime` mixin types on wasm. File/extend `Q41` with the wasm-reactive-core repro, mark it the blocker for Milestone 2.5, and write its own plan. Do NOT proceed to ElementSurface until it's resolved.
- **RED — runtime trap:** capture the trap offset; likely the same terminator/`unreachable` class as the ADR's `render()` note. File with this minimal repro.

- [ ] **Step 5: Commit**

```bash
cd /Users/hassan/.projects/ruxen
git add compiler/ruxen_core/tests/wasm_codegen.rs docs/dev/gui-stack-v1-issues.md docs/TASKS.md
git commit -m "test(wasm): pin reactive-core (Mutex) wasm de-risk + record Phase 0.5 outcome (Q41/Q42)"
```

---

## Decision gate (end of plan)

| A (dispatch) | B (PaintSurface) | C (reactive core) | Verdict |
|---|---|---|---|
| GREEN | GREEN | GREEN | Native-element web is a **quiver feature**. Write the next plan: `ElementSurface` mixin + `RecordingElementSurface` (ADR Phase 1). |
| GREEN | GREEN | RED | Critical path = **ruxen LLVM backend** (Q41: emit vtable/class_info on wasm). All quiver work blocked until resolved. |
| RED (any) | — | — | The discovered L0 task (Q43+) is the critical path; re-plan around it. |

Update `quiver/docs/ROADMAP.md` Milestone 2.5 checkboxes and `docs/TASKS.md` with the verdict. The ADR's risk-concentration note (the whole effort gated on L0) is resolved either way by this plan.

---

## Self-review notes

- **Spec coverage:** Covers ADR gates Phase 0 (Q42 → Spike A) and Phase 0.5 (render → Spike B; reactive core → Spike C). ElementSurface/web-backend/desktop phases are intentionally out of scope (separate plans, gated on this verdict).
- **No invented compiler APIs:** all Rust uses only the existing `wasm_codegen.rs` helpers (`compile_wasm_object`, `link_wasm`, `node_available`, `N`) read verbatim. The one new helper (`compile_wasm_object_bootstrapped`) is explicitly contingent on Step 1's bootstrap finding.
- **Honest uncertainty:** the `.rx` sources for A and C touch Ruxen surfaces (closures-in-`Array`+capture+mutation; the exact `Mutex` API) whose spelling must be confirmed against stdlib/quiver source before running — flagged inline. For a de-risk spike, a `.rx` that won't compile is a valid RED finding, not a plan defect.
- **Test discipline:** each spike is one narrow `--test wasm_codegen <name>` run, cached to `tmp/test-cache/` (rules 41/42); no `cargo test --workspace`.
