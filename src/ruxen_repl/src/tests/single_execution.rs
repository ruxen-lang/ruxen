use super::state_persistence::run_session;

extern "C" {
    fn ruxen_repl_set_replaying(v: i32) -> i32;
    fn ruxen_repl_get_replaying() -> i32;
}

/// Smoke test: the runtime replay-suppression flag is actually
/// linked into the test binary and round-trips via the C
/// accessors. If this fails, the suppression flag isn't wired
/// at the runtime layer and no downstream test can succeed.
#[test]
fn replay_flag_round_trips() {
    let prev = unsafe { ruxen_repl_get_replaying() };
    assert_eq!(prev, 0, "flag must default to 0");
    let old = unsafe { ruxen_repl_set_replaying(1) };
    assert_eq!(old, 0);
    assert_eq!(unsafe { ruxen_repl_get_replaying() }, 1);
    unsafe { ruxen_repl_set_replaying(0) };
    assert_eq!(unsafe { ruxen_repl_get_replaying() }, 0);
}

/// Phase 3 contract — `puts` MUST fire exactly once per input. The
/// runtime replay-suppression flag (set around the replayed
/// `let_bindings + session_var_mutations` portion of each wrapper)
/// makes every replayed `ruxen_puts` no-op at the C-runtime layer,
/// so the second input's stdout only contains its own output.
#[test]
fn puts_fires_exactly_once_per_input() {
    let outs = run_session(&[r#"puts "hello""#, r#"puts "world""#]);
    // input 0 emits "hello\n", input 1 emits "world\n". Neither
    // should contain a duplicate "hello".
    assert_eq!(outs[0].matches("hello").count(), 1, "got: {:?}", outs);
    assert_eq!(outs[1].matches("hello").count(), 0, "got: {:?}", outs[1]);
    assert_eq!(outs[1].matches("world").count(), 1, "got: {:?}", outs[1]);
}

/// `Command.new("echo", "hello").status` would re-fork the subprocess
/// every input under the old cumulative-replay model. With the runtime
/// suppression flag wrapping the replay portion of every wrapper,
/// the subprocess fires exactly once on the input it appears in.
/// Subprocess output bypasses the REPL's capture buffer (echo's
/// stdout writes directly to fd 1, not via `ruxen_puts`), so this
/// test only asserts the *REPL-side* puts contract: once the
/// subprocess has been bound on input 1, a subsequent input's puts
/// fires exactly once and the prior puts (none here) does not
/// re-emit. The end-to-end "subprocess stdout appears once"
/// assertion lives in `tests/release-e2e/cases/508_command_status.rx`
/// where the harness diffs real stdout.
#[test]
fn subprocess_stdout_appears_once() {
    let outs = run_session(&[
        r#"use std.process.Command"#,
        r#"let _ = Command.new("/usr/bin/true").status"#,
        r#"puts "after""#,
    ]);
    let joined: String = outs.concat();
    assert_eq!(joined.matches("after").count(), 1, "got: {:?}", outs);
}
