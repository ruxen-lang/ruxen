# Async lowering — `.await` inside `while` body (E1115 closure)

**Status:** v1 design (this spec). Closes E1115 for the canonical
single-await `while` shape. v2+ extends to multi-await bodies, `for`,
and `while let`.

**Source docs:**
- `docs/errors/E1115.md` (current rejection diagnostic)
- `docs/specs/syntax/async_lowering.spec.md` (Milestone 2A + 2B)
- `compiler/riven_core/src/async_lowering/mod.rs` (the implementation)

---

## 1. Problem

Today the async-lowering pass rejects every `.await` whose enclosing
expression is a `while` / `for` / `loop` body — see the dedicated
`collect_await_in_loop_diagnostics` pre-pass. The current segmenter
(`segment_body`) accepts only a strictly linear shape:

```
pre_await | [let x = expr.await]* | tail
```

Each `.await` becomes its own state in a linear chain
(`self.__state = i + 1` advances forward only). There is **no back
edge** anywhere in the lowering; the state field grows monotonically
from 0 to N and stops.

Rondo cannot use the recently-shipped `Task.spawn_raw` runtime
surface (riven `59b8de6`) because the natural accept-spawn pattern
requires `.await` inside a `while`:

```riven
async def serve(app: &Rondo, l: AsyncTcpListener) -> Int
  var keep_going: Bool = true
  while keep_going
    let pair = l.accept.await        # ← E1115
    match pair
      Ok(p) -> Task.spawn_raw(handle(app, p.0))
      Err(_) -> keep_going = false
    end
  end
  0
end
```

## 2. Design — "inner while" not "state-machine back-edge"

The naive approach is a CFG-style state machine with a back edge
from the await-state to the loop-head state. That requires
rewriting `segment_body` to emit a CFG instead of a linear segment
list, and rewriting `build_multi_state_poll_body` to emit
conditional state transitions including back edges. It is sound but
high-blast-radius — most of the existing lowering code assumes
monotonic state advancement.

**v1 ships a simpler shape**: the loop iterations happen inside an
**actual Riven `while` loop** in the synthesised `poll` body. The
state-machine field has only two logical values for the loop region
(running vs done). Each call to `poll` runs as many iterations as
the inner sub-future allows before either completing all iterations
or hitting `Poll.Pending`.

### 2.1 Shape recognised by the segmenter

A new `BodyShape` variant in addition to the existing linear shape:

```rust
enum BodyShape {
    Linear(Segments),                  // pre_await | await_lets | tail (existing)
    WhileLoopWithSingleAwait {
        pre_loop_stmts: Vec<Statement>,    // straight-line, no `.await`
        loop_cond: Expr,                   // sync, no `.await`
        body_pre_await: Vec<Statement>,    // body stmts before `let x = expr.await`; no `.await`
        loop_await_let: LetBinding,        // the one `let <name> = <expr>.await` in the body
        body_post_await: Vec<Statement>,   // body stmts after the await; no `.await`
        post_loop_stmts: Vec<Statement>,   // tail after the loop; no `.await`
    },
}
```

`segment_body` returns one of these. If neither shape fits, return
`None` and let the existing E1115 pre-pass surface its diagnostic.

### 2.2 Restrictions for v1

A `while` body qualifies for the new lowering only if **all** hold:
1. Exactly one `let <ident> = <expr>.await` statement in the body
   (no `.await` outside that `let`'s RHS).
2. The `<ident>` binding is a plain `Pattern::Identifier`.
3. The loop condition contains no `.await`.
4. Pre-loop and post-loop statements contain no `.await`. (v1
   doesn't combine the new shape with the existing linear-await
   shape in the same function — that combination is a follow-up.)
5. The async fn body contains exactly one `while` loop with an
   `.await` inside (no two such loops in the same fn).
6. Loop variant is bare `while cond { body }`. `loop { body }`,
   `while let`, and `for` are out of scope for v1.

Anything outside this matrix → fall through to `None` → existing
E1115 path fires.

### 2.3 State machine fields

For a `WhileLoopWithSingleAwait` shape:

```
class __FFuture
  __state: Int              # 0 = looping, 1 = done
  __sub_ready: Int          # 0 = sub-future not yet constructed this iteration, 1 = in-flight
  __sub: <AwaiteeFutureClass>
  <outer params>: ...       # promoted to self.* (existing crossing rule)
  <loop cond var>: <Ty>     # if it's a `var` declared pre-loop, it's already a field
  <loop binding>: <ResultTy>  # e.g. `pair`
  <crossing locals>: ...    # any pre-loop locals referenced after the loop (existing rule)
end
```

### 2.4 `init` body

The existing eager-init shape is preserved but adjusted:
- `__state = 0`
- `__sub_ready = 0` (sub is constructed inside `poll`, not in `init`)
- Outer args copied as today.
- Pre-loop straight-line statements run verbatim (with arg refs
  rewritten to `self.<arg>`).
- Crossing pre-loop locals: copy into `self.<name>` fields.
- Loop-cond var: if it was a pre-loop `var <name>: T = init`, run
  the initialiser and assign to the field.
- `__sub` is default-initialised (we never construct it eagerly —
  unlike the existing linear path which eagerly constructs `__sub_0`).

### 2.5 `poll` body

```riven
def var poll(cx: &var Context) -> Poll[<ReturnTy>]
  if self.__state != 0
    return Poll.Pending
  end

  var keep_iterating: Bool = true
  var pending_exit: Bool = false
  while keep_iterating
    if <cond_expr_with_self_refs>
      if self.__sub_ready == 0
        self.__sub = <awaitee_ctor_with_self_refs>
        self.__sub_ready = 1
      end
      let p = (&var self.__sub).poll(cx)
      match p
        Poll.Pending ->
          let _stop = 0
          pending_exit = true
          keep_iterating = false
        Poll.Ready(v) ->
          let _step = 0
          self.<loop_binding> = v
          self.__sub_ready = 0
          <body_post_await with self.* refs>
      end
    else
      self.__state = 1
      keep_iterating = false
    end
  end

  if pending_exit
    Poll.Pending
  else
    <post_loop_stmts with self.* refs>
    Poll.Ready(<tail_expr_with_self_refs>)
  end
end
```

#### Why the inner Riven `while` is sound

The poll method itself is **not** async — it's a plain Riven method
that returns `Poll[T]`. Loops inside non-async methods compile
today. The `.await` desugaring restriction (E1115) only applies to
`.await` expressions, and there are no `.await` expressions in the
synthesised poll body — the suspension is via the explicit
`.poll(cx)` call. So the inner `while` is plain control flow over a
sub-future poll, with no special handling needed in the lowering of
the poll method.

#### Why two control vars

`keep_iterating` controls the iteration loop. `pending_exit`
distinguishes "we hit `Poll.Pending`, return Pending" from "loop
cond went false, run tail and return Ready". Without
`pending_exit`, both exits use the same flag and we'd need a state-
check after the loop, which is uglier.

### 2.6 Per-iteration sub-future lifecycle

- `init` does NOT construct `__sub`. (Existing linear lowering DOES
  construct sub-futures in init — different rule for the loop
  shape.)
- On entering iteration: if `__sub_ready == 0`, construct `__sub`
  via the awaitee ctor (with arg refs rewritten to `self.*`), set
  `__sub_ready = 1`.
- On `Ready`: assign result to `self.<binding>`, reset
  `__sub_ready = 0` so the NEXT iteration re-constructs the sub-
  future fresh.
- On `Pending`: leave `__sub_ready = 1`; the next `poll` call
  re-polls the same sub-future.

### 2.7 Loop-body locals

For v1, loop-body `let` bindings are NOT supported beyond the one
`.await` let. Any other let inside the body must be inlined into
expressions OR moved outside the loop.

This avoids the complication of "field hoisting for re-init-each-
iteration locals". If a user hits this limitation, the E1115 doc
notes the workaround (extract the body into a non-async helper).

### 2.8 Var assignment in body_post_await

The body after the `.await` may contain assignments to pre-loop
`var`s (e.g. `keep_going = false`). These are already class fields
under the existing crossing-locals rule. The `rewrite_arg_refs_in_*`
pass promotes bare identifiers to `self.<name>` — so the assignment
becomes `self.keep_going = false`. No new work needed.

### 2.9 Tail expression

If the async fn body ends with the `while` loop followed by a tail
expression (e.g. `0`), the lowering emits that as
`Poll.Ready(<tail>)` after the loop exits cond-false.

If the async fn body ends with the `while` itself and no tail, the
return type must be `()` (or default-initialisable). Same as the
existing single-await terminal rule.

## 3. Implementation plan

### 3.1 Files touched

1. `compiler/riven_core/src/async_lowering/mod.rs`:
   - Add `BodyShape` enum.
   - Refactor `segment_body` to return `Option<BodyShape>` (today
     it returns `Option<Segments>`; the linear shape becomes
     `BodyShape::Linear(Segments)`).
   - Add `segment_while_loop_with_single_await(body)` recogniser.
     Returns `Option<WhileLoopWithSingleAwait>` data.
   - Add `build_loop_state_machine_poll_body(...)` — emits the
     poll body sketched in §2.5.
   - In `lower_one_async_fn_with_await`, dispatch on the shape:
     - `Linear(...)` → existing path (untouched).
     - `WhileLoopWithSingleAwait(...)` → new path.
   - Modify the E1115 pre-pass (`collect_await_in_loop_diagnostics`)
     to **not** fire for awaits that match the new shape. (Easiest
     check: skip awaits whose enclosing loop body matches all the
     v1 restrictions. If any restriction violates, the
     pre-pass still fires.)

2. `compiler/riven_core/tests/fixtures/riven/async_negative_await_in_while.rvn`:
   - Update the fixture to no longer reject the canonical shape.
     Add new negative fixtures for the unsupported subset
     (multi-await body, `for` loop, `while let`, etc.).

3. `tests/release-e2e/cases/728c_async_spawn_loop.rvn`:
   - Already drafted (by the prior agent attempt). Move from
     `.deferred` back to active once the implementation works.
   - Adjust if needed to match the v1 restriction surface.

### 3.2 Verification

Single-fixture filter, per the harness convention:

```bash
cd /Users/hassan/.projects/riven
RIVEN_E2E_CASES=728c_async_spawn_loop cargo test \
  -p riven_core --test release_e2e_smoke -- --ignored
```

**Do not run the full e2e suite during iteration** — it's ~3
minutes per run, kills the iteration loop. The single-case filter
runs in ~1s.

Final verification (once 728c passes), run the async-only subset:

```bash
RIVEN_E2E_CASES=723_async_block_on,724_async_block_on_chain,725_time_sleep_block_on,726_async_listener_bind,727_async_tcp_echo,727b_async_tcp_read_timeout,728_async_spawn_join_basic,728c_async_spawn_loop \
  cargo test -p riven_core --test release_e2e_smoke -- --ignored
```

### 3.3 Rondo bench

After the runtime fix, optionally wire Rondo's `async_serve_loop`
to use the new pattern:

```riven
async def accept_and_spawn(app: &Rondo, l: AsyncTcpListener) -> Int
  var keep_going: Bool = true
  while keep_going
    let pair = l.accept.await
    match pair
      Ok(p) -> 
        let _h = Task.spawn_raw(handle_connection_future(app, p.0))
        0
      Err(_) -> keep_going = false
    end
  end
  0
end
```

Then `block_on(accept_and_spawn(app, listener))` is the worker's
main. **The existing bench at 52 k RPS must remain green** — if
intra-worker concurrency adds overhead that dominates (wake-all
scheduling cost > single-task savings), keep the existing serial
accept loop and document the limitation.

## 4. Worked example

For the canonical Rondo accept loop:

```riven
async def serve(app: &Rondo, l: AsyncTcpListener) -> Int
  var keep_going: Bool = true
  while keep_going
    let pair = l.accept.await
    match pair
      Ok(p) -> Task.spawn_raw(handle(app, p.0))
      Err(_) -> keep_going = false
    end
  end
  0
end
```

Lowers to:

```riven
class __ServeFuture
  __state: Int
  __sub_ready: Int
  __sub: AsyncAcceptFuture
  app: &Rondo
  l: AsyncTcpListener
  keep_going: Bool
  pair: Result[(AsyncTcpStream, String), IoError]
  include Future
  type Output = Int

  def init(__state: Int, app: &Rondo, l: AsyncTcpListener)
    self.__state = 0
    self.__sub_ready = 0
    self.keep_going = true
    # self.__sub left default-uninit; constructed on first iteration
  end

  def var poll(cx: &var Context) -> Poll[Int]
    if self.__state != 0
      return Poll.Pending
    end
    var keep_iterating: Bool = true
    var pending_exit: Bool = false
    while keep_iterating
      if self.keep_going
        if self.__sub_ready == 0
          self.__sub = self.l.accept    # awaitee ctor; Riven instance method
          self.__sub_ready = 1
        end
        let p = (&var self.__sub).poll(cx)
        match p
          Poll.Pending ->
            let _stop = 0
            pending_exit = true
            keep_iterating = false
          Poll.Ready(v) ->
            let _step = 0
            self.pair = v
            self.__sub_ready = 0
            match self.pair
              Ok(p) -> Task.spawn_raw(handle(self.app, p.0))
              Err(_) -> self.keep_going = false
            end
        end
      else
        self.__state = 1
        keep_iterating = false
      end
    end
    if pending_exit
      Poll.Pending
    else
      Poll.Ready(0)
    end
  end
end

def serve(app: &Rondo, l: AsyncTcpListener) -> __ServeFuture
  __ServeFuture.new(0, app, l)
end
```

## 5. Out of scope (v2+)

- Multiple `.await`s in a single loop body.
- Nested loops with awaits.
- `for` and `while let` loops with awaits.
- `loop { ... }` (bare loop) — requires explicit `break` handling.
- Pre-loop or post-loop linear awaits combined with loop awaits in
  the same async fn.
- Loop-body `let` bindings outside the single `.await` let.

Each of these adds independent complexity. v1 ships the canonical
server accept-spawn pattern; the rest land as follow-up milestones.

## 6. Risks

1. **Drop elaboration on `__sub`**: the sub-future is constructed
   per-iteration. Each construction allocates a state struct (e.g.
   `riven_async_accept_state_new`). On Ready, the state's
   `take_result` consumes the buffer and `state_free` is called by
   the existing drop chain. Verify the per-iteration alloc is
   matched by per-iteration free — leak-check with the existing
   `runtime_no_leak_fixture` harness if time permits.

2. **Default-uninit field**: `self.__sub` is declared but not
   initialised in `init`. Riven's typeck may flag this. If so,
   either:
   - Initialise with a sentinel via a zero-arg ctor on the awaitee
     class (e.g. `AsyncAcceptFuture.new(0)` or similar), OR
   - Wrap in `Option[T]` and start as `None`.

   The `Option` shape is more correct but requires Option-aware
   poll dispatch. Try sentinel first.

3. **Bench regression**: the per-poll inner `while` is fine
   (Riven loops are cheap) but if the new field layout shifts
   anything in MIR, alignment changes could surprise hot-path
   allocations. Watch RPS after wiring Rondo; if it drops below
   50 k, revert Rondo and keep the runtime change as-is.

## 7. Definition of done

- [ ] 728c_async_spawn_loop fixture passes with the canonical
      while-cond + single-await body.
- [ ] All existing 720-series async fixtures still pass.
- [ ] The E1115 pre-pass still rejects the unsupported cases (for
      loop, while-let, multi-await body) with a clear message
      pointing at this spec for the supported subset.
- [ ] Rondo's release build is green against the new toolchain.
- [ ] Rondo's bench is within ±5 % of the pre-change number (52 k
      RPS).
- [ ] This spec moves from `(v1 design)` to `(v1 shipped)` with
      the commit SHA noted.
