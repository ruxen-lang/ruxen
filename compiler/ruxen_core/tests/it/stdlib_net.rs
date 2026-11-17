//! `std::net` blocking echo server with a SIGINT shutdown. The Ruxen
//! binary is the server; the host process connects as the client and
//! sends the signal, which a `.rx` test cannot do to itself. The rest of
//! the TcpListener / TcpStream tests live in `library/std/net/tests`.

use ruxen_core::codegen;
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;
use std::io::Read;
use std::net::TcpListener;
use std::process::Command;
use std::time::{Duration, Instant};

fn rx(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruxen")
        .join(format!("{name}.rx"));
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
    let bin_path = tmp_dir.join(format!(
        "{}-{}-{}.bin",
        basename,
        std::process::id(),
        ruxen_unique_id()
    ));

    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "typecheck errors: {:?}", errors);

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .expect("MIR lowering");
    codegen::compile(&mir, bin_path.to_str().unwrap()).expect("codegen");
    bin_path
}

fn loopback_bind_available() -> bool {
    match TcpListener::bind("127.0.0.1:0") {
        Ok(listener) => {
            drop(listener);
            true
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => false,
        Err(e) => panic!("probe bind: {e}"),
    }
}

/// End-to-end proof that Ruxen can host a blocking TCP server using
/// the new class surface.
///
/// The Ruxen program plays the **server** role using TcpListener +
/// TcpStream classes. It binds an ephemeral port (chosen by Rust +
/// handed off through `RUXEN_NET_PORT`), installs a SIGINT handler,
/// and loops `accept → read → write-echo → close`. The test process
/// plays the client, connects, sends `"ping"`, reads the echo, then
/// signals SIGINT. The server's blocking `accept()` returns EINTR
/// (mapped to Err(IoError.Interrupted)), the loop notices
/// `signal_received_sigint() != 0`, the listener is closed, the
/// program prints `"bye"` and exits cleanly.
///
/// What this proves end-to-end:
///   - TcpListener.bind / accept / close work as a coherent surface
///   - TcpStream.read / write / close work as a coherent surface
///   - `signal_install_sigint` + `signal_received_sigint` correctly
///     mediate cooperative shutdown — EINTR rises into the class
///     result as Err(IoError.Interrupted)
///   - A blocking server runs, handles a real connection, and
///     terminates without leaking on a real signal
#[test]
#[cfg_attr(windows, ignore = "POSIX signals + fork-style accept loop")]
fn blocking_tcp_echo_server_with_graceful_sigint_shutdown() {
    use std::io::Write;

    if !loopback_bind_available() {
        return;
    }
    let probe = TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let port = probe.local_addr().expect("local_addr").port();
    drop(probe);

    let source = rx("blocking_tcp_echo_server_with_graceful_sigint_shutdown");
    let bin_path = compile(&source, "stdlib_net_server_sigint");

    let mut child = Command::new(&bin_path)
        .env("RUXEN_NET_PORT", port.to_string())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn ruxen server");

    // Generous window: under `cargo test`'s default parallelism the sibling
    // tests in this file each shell out to a full `compile()`, saturating CPU
    // and starving this freshly-spawned server of scheduling time before it
    // reaches `accept()`. The old 5s window flaked (solo and in-suite); 30s
    // never legitimately elapses for a localhost connect once the server is
    // up. We also fail fast if the child has already exited (e.g. a bind
    // error) instead of burning the whole window and reporting a misleading
    // "didn't accept" with empty stderr.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut stream = loop {
        if let Ok(Some(status)) = child.try_wait() {
            let out = child.wait_with_output().ok();
            let stderr = out
                .as_ref()
                .map(|o| String::from_utf8_lossy(&o.stderr).to_string())
                .unwrap_or_default();
            panic!(
                "server exited before accepting (status={:?}); stderr=[{}]",
                status, stderr
            );
        }
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
                        "server didn't accept connections within 30s; stderr=[{}]",
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

    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGINT);
    }

    let deadline = Instant::now() + Duration::from_secs(30);
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
                    panic!("server did not exit within 30s of SIGINT");
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => panic!("try_wait: {}", e),
        }
    }

    let out = child.wait_with_output().expect("wait_with_output");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.trim().ends_with("bye"),
        "expected server stdout to end with 'bye'; got [{}]",
        stdout
    );
}

fn ruxen_unique_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}
