/* REPL replay-suppression flag. Set by the REPL session when
 * re-running prior `let_bindings` + `session_var_mutations` so the
 * runtime can no-op every non-idempotent side effect (puts, print,
 * subprocess spawn, fs write, network connect). Cleared before the
 * user's new statement runs and after the wrapper returns.
 *
 * Idempotent reads (fs.read, canonicalize, getenv, …) ignore the
 * flag and always execute — they don't perturb state and the
 * resulting values are needed for correct let-RHS replay.
 *
 * `__thread` storage so we don't need locking. Today the REPL runs
 * single-threaded; multi-threaded REPL would still want per-thread
 * state because each thread's replay context is independent.
 *
 * Initially 0 (false). All non-REPL processes (AOT-compiled
 * binaries, `ruxen run`, `ruxen build` outputs) never set this, so
 * the flag is always 0 there and every runtime function executes
 * normally — zero runtime cost in non-REPL paths beyond a single
 * TLS load per call.
 */
#include "runtime.h"

__thread int ruxen_repl_is_replaying = 0;

int ruxen_repl_set_replaying(int v) {
    int prev = ruxen_repl_is_replaying;
    ruxen_repl_is_replaying = v ? 1 : 0;
    return prev;
}

int ruxen_repl_get_replaying(void) {
    return ruxen_repl_is_replaying;
}
