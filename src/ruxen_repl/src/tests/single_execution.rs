use super::state_persistence::run_session;

/// Once the refactor lands, `puts` MUST fire exactly once per input.
/// Currently the REPL line-count-diffs cumulative output to mask the
/// replay, which works for puts of static strings but NOT for
/// subprocess output (508) or for prior statements that fail on
/// replay (534/536/727/727b).
///
/// Marked `#[ignore]` until Phase 3 lands so CI stays green.
#[test]
#[ignore = "flips green when Phase 3 lands"]
fn puts_fires_exactly_once_per_input() {
    let outs = run_session(&[
        r#"puts "hello""#,
        r#"puts "world""#,
    ]);
    // input 0 emits "hello\n", input 1 emits "world\n". Neither
    // should contain a duplicate "hello".
    assert_eq!(outs[0].matches("hello").count(), 1, "got: {:?}", outs);
    assert_eq!(outs[1].matches("hello").count(), 0, "got: {:?}", outs[1]);
    assert_eq!(outs[1].matches("world").count(), 1, "got: {:?}", outs[1]);
}

#[test]
#[ignore = "flips green when Phase 3 lands"]
fn subprocess_stdout_appears_once() {
    let outs = run_session(&[
        r#"use std.process.Command"#,
        r#"let _ = Command.new("echo", "hello").status"#,
        r#"puts "after""#,
    ]);
    let joined: String = outs.concat();
    assert_eq!(joined.matches("hello").count(), 1, "got: {:?}", outs);
    assert_eq!(joined.matches("after").count(), 1, "got: {:?}", outs);
}
