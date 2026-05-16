//! Integration test for Phase 3 `std::net` module.
//!
//! Verifies that `tcp_connect`, `tcp_listen`, `tcp_accept`, `tcp_read`,
//! `tcp_write`, and `tcp_close` resolve through the resolver, lower to
//! the right runtime calls, and produce correct values at runtime.
//!
//! Round-trip strategy:
//!  - The Rust test binds a `std::net::TcpListener` on `127.0.0.1:0`
//!    so the kernel picks an unused port.
//!  - We pass the chosen port to the compiled Riven program via the
//!    `RIVEN_NET_TEST_PORT` environment variable. The Riven binary
//!    uses `std.env.var` to read it, then calls `tcp_connect`,
//!    `tcp_write`, `tcp_close`.
//!  - The Rust side accepts, reads, and asserts on the bytes received.
//!
//! There is also a smoke test that verifies `tcp_connect` returns -1
//! for an obviously-unreachable address (port 1 with no listener).
//! That one doesn't depend on env vars or the network stack picking
//! a port, so it serves as a fast sanity check.

use riven_core::codegen;
use riven_core::lexer::Lexer;
use riven_core::mir::lower::Lowerer;
use riven_core::parser::Parser;
use riven_core::typeck;
use std::io::Read;
use std::net::TcpListener;
use std::process::Command;
use std::time::{Duration, Instant};

fn rvn(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/riven")
        .join(format!("{name}.rvn"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn workspace_root() -> std::path::PathBuf {
    let crate_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

fn compile(source: &str, basename: &str) -> std::path::PathBuf {
    let root = workspace_root();
    let tmp_dir = root.join("tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let bin_path = tmp_dir.join(format!("{}.bin", basename));

    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == riven_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "typecheck errors: {:?}", errors);

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .expect("MIR lowering");
    codegen::compile(&mir, bin_path.to_str().unwrap()).expect("codegen");
    bin_path
}

#[test]
fn tcp_connect_unreachable_returns_negative_one() {
    // Smoke test: connecting to a port with no listener should fail.
    // Port 1 is privileged + has no service in any normal config, so
    // any connection attempt either fails immediately (ECONNREFUSED)
    // or is rejected by privilege checks. Either way we expect -1.
    let source = rvn("tcp_connect_unreachable_returns_negative_one");
    let bin_path = compile(&source, "stdlib_net_unreachable");
    let output = Command::new(&bin_path).output().expect("run binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "binary exited non-zero");
    assert!(
        stdout.contains("fail"),
        "expected -1 from unreachable connect; stdout=[{}]",
        stdout
    );
}

#[test]
fn tcp_loopback_roundtrip() {
    // Bind on an ephemeral port and pass it to the Riven binary which
    // connects + writes "hello world".
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    let port = listener.local_addr().expect("local_addr").port();

    let source = rvn("tcp_loopback_roundtrip");
    let bin_path = compile(&source, "stdlib_net_roundtrip");

    // Spawn the Riven binary with the port, then accept on the listener.
    // The test must fail loudly within seconds if the child fails to
    // connect — so we accept with a hard deadline rather than letting
    // a blocking accept() hang indefinitely (which historically masked
    // child-side failures behind a CI watchdog timeout).
    let mut child = Command::new(&bin_path)
        .env("RIVEN_NET_TEST_PORT", port.to_string())
        .spawn()
        .expect("spawn riven binary");

    listener
        .set_nonblocking(true)
        .expect("set nonblocking listener");
    let deadline = Instant::now() + Duration::from_secs(5);
    let (mut stream, _peer) = loop {
        match listener.accept() {
            Ok(c) => break c,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let out = child.wait_with_output().ok();
                    let stderr = out
                        .as_ref()
                        .map(|o| String::from_utf8_lossy(&o.stderr).to_string())
                        .unwrap_or_default();
                    panic!("no inbound connection within 5s; child stderr=[{}]", stderr);
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => panic!("accept error: {}", e),
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set read timeout");

    let mut buf = [0u8; 64];
    let n = stream.read(&mut buf).expect("read from peer");
    let received = std::str::from_utf8(&buf[..n]).expect("utf8");

    let status = child.wait().expect("wait child");
    assert!(status.success(), "riven binary exited non-zero");

    assert_eq!(
        received, "hello world",
        "expected exactly 'hello world' over the wire; got [{}]",
        received
    );
}

/// End-to-end proof that Riven can host a blocking TCP server.
///
/// The Riven program plays the **server** role: it binds an ephemeral
/// port (chosen by Rust + handed off through the `RIVEN_NET_PORT`
/// env var), installs a SIGINT handler for graceful shutdown, and
/// loops `accept → read → write-echo → close`.  The test process
/// plays the client: connects, sends `"ping\n"`, reads the echo,
/// then signals SIGINT.  The server's blocking `accept()` returns
/// EINTR, the loop notices `signal_received_sigint() != 0`, the
/// listening fd is closed, the program prints `"bye"` and exits
/// cleanly.
///
/// What this proves end-to-end:
///   - `tcp_listen` + `tcp_accept` + `tcp_read` + `tcp_write` +
///     `tcp_close` work as a coherent surface
///   - `signal_install_sigint` + `signal_received_sigint` correctly
///     mediate cooperative shutdown
///   - A blocking server runs, handles a real connection, and
///     terminates without leaking on a real signal
///
/// User-level `TcpListener` / `TcpStream` class wrappers are
/// demonstrated inline in the Riven source — making them stdlib
/// auto-imports is a separate (resolver-level) commit.
#[test]
#[cfg_attr(windows, ignore = "POSIX signals + fork-style accept loop")]
fn blocking_tcp_echo_server_with_graceful_sigint_shutdown() {
    use std::io::Write;

    // Grab an ephemeral port by binding-and-dropping; the kernel will
    // not immediately re-issue it, so the Riven server's bind on the
    // same port wins in practice.  A theoretical race remains but is
    // negligible in test environments.
    let probe = TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let port = probe.local_addr().expect("local_addr").port();
    drop(probe);

    // The Riven program defines a small `TcpListener` / `TcpStream`
    // class wrapper at user level — illustrates the pattern users can
    // copy-paste today.  A future commit will register these as
    // stdlib auto-imports so users don't have to.
    let source = rvn("blocking_tcp_echo_server_with_graceful_sigint_shutdown");
    let bin_path = compile(&source, "stdlib_net_server_sigint");

    let mut child = Command::new(&bin_path)
        .env("RIVEN_NET_PORT", port.to_string())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn riven server");

    // Wait for the server to bind by polling with short connect
    // attempts.  Bounded by a 5s deadline so a stuck server fails
    // the test rather than hanging the suite.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut stream = loop {
        match std::net::TcpStream::connect(format!("127.0.0.1:{}", port)) {
            Ok(s) => break s,
            Err(_) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let out = child.wait_with_output().ok();
                    let stderr = out
                        .as_ref()
                        .map(|o| String::from_utf8_lossy(&o.stderr).to_string())
                        .unwrap_or_default();
                    panic!(
                        "server didn't accept connections within 5s; stderr=[{}]",
                        stderr
                    );
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set read timeout");

    // Send a ping, read the echo, drop the connection.
    stream.write_all(b"ping").expect("write ping");
    let mut buf = [0u8; 64];
    let n = stream.read(&mut buf).expect("read echo");
    drop(stream);
    let received = std::str::from_utf8(&buf[..n]).expect("utf8");
    assert_eq!(
        received, "ping",
        "expected exactly 'ping' echoed back; got [{}]",
        received
    );

    // Graceful shutdown: SIGINT lands while the server is back at
    // accept() waiting for the next client.  The server's loop must
    // notice the flag and exit with status 0.
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGINT);
    }

    // Wait for clean exit, bounded.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                assert!(
                    status.success(),
                    "server exited non-zero after SIGINT: {:?}",
                    status
                );
                break;
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    panic!("server did not exit within 5s of SIGINT");
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => panic!("try_wait: {}", e),
        }
    }

    // Confirm the server printed its shutdown marker.
    let out = child.wait_with_output().expect("wait_with_output");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.trim().ends_with("bye"),
        "expected server stdout to end with 'bye'; got [{}]",
        stdout
    );
}
