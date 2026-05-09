# v2/01 — Actor model: library first, language later

**Depends on:** v1.0.0 shipped (prompt 25 complete).
**Reads:** `docs/requirements/tier1_02_concurrency.md` §future-work,
session log 2026-04-29 (decision to defer actors to v2).

## Context

v1 un-reserved `actor`/`spawn`/`send`/`receive` (prompt 01 part D)
to free user identifiers. Async runtime shipped (prompt 15). Actors
deferred per session ruling: ship as library on top of async, then
promote to language feature only if usage validates it.

## Phase A — Library actors (v2.0)

### Goal

`std::actor::Actor[Msg]` library type. Built entirely on top of v1
async + channels. No new keywords, no parser changes.

### Surface

```riven
use std.actor.{Actor, ActorRef}

class Counter
  count: Int = 0

  async def handle(self: &mut Self, msg: CounterMsg) -> ()
    match msg
      Inc => self.count = self.count + 1
      Get(reply) => reply.send(self.count).await
    end
  end
end

enum CounterMsg
  Inc
  Get(ActorRef[Int])
end

async def main
  let counter = Actor[CounterMsg].spawn(Counter.new).await
  counter.send(CounterMsg::Inc).await
  let reply: ActorRef[Int] = ActorRef.new
  counter.send(CounterMsg::Get(reply.clone)).await
  let value = reply.recv.await
  puts "#{value}"
end
```

### TDD

- Counter test: 1000 `Inc` messages → final `Get` returns 1000.
- Concurrent test: 4 actors echo to each other; pipeline completes.
- Crash isolation: handler panics; supervisor restarts actor; test
  asserts state reset and subsequent messages process.
- Selective receive: actor with two message types; assert only
  matching variants are dequeued in priority order.

### Implementation

- `Actor[Msg]` wraps `(receiver: Channel[Msg], state: Cell[State])`.
- `spawn(state)` creates the channel + `task::spawn` an async loop
  that pulls messages and dispatches `handle`.
- `ActorRef[Msg]` is `Sender[Msg]` wrapper; cheaply cloneable.
- Panics in `handle` caught via async-aware panic hook; supervisor
  policy `RestartOnPanic` (default) recreates the actor with the
  same channel.

### Definition of done — Phase A

- [ ] Library compiles on top of v1 stdlib without language changes.
- [ ] All 4 TDD scenarios pass.
- [ ] Documentation: `docs/cookbook/actor-patterns.md`.
- [ ] At least one in-tree example using actors
      (`examples/06-chat-server/`).

## Phase B — Language actors (v2.x or v3.0, ONLY if Phase A succeeds)

### Trigger

Promote to language feature only if **all three** are true:

1. ≥3 third-party crates in production use the Phase A library.
2. User feedback identifies ergonomic gaps (typed addresses,
   selective receive, supervisor trees) that library can't bridge.
3. The team has bandwidth for a 6-month language change.

### Scope (if triggered)

- Reserve `actor`/`spawn`/`send`/`receive` again on a new edition.
- `actor` syntax sugar for `class + Actor::spawn` boilerplate.
- Compiler-checked typed mailboxes.
- Supervisor tree syntax.
- Distribution story (cross-process / cross-machine).

### Definition of done — Phase B

- [ ] New edition `2030` (or whenever) gates the keywords.
- [ ] EditionLint flags pre-2030 source with new reserved idents.
- [ ] Library `Actor[Msg]` becomes deprecated in favor of language
      syntax; deprecation timeline documented.
- [ ] All Phase A examples ported to new syntax.

## What this does NOT include

- Erlang BEAM-style preemption — out of scope.
- Cross-machine actor distribution — explicitly v3+.
- Hot code reloading — out of scope.
