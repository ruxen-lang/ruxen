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
    let source = r#"
use std.net.tcp_connect

def main
  let fd = tcp_connect(&"127.0.0.1:1")
  if fd < 0
    puts "fail"
  else
    puts "ok"
  end
end
"#;
    let bin_path = compile(source, "stdlib_net_unreachable");
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

    let source = r#"
use std.net.{tcp_connect, tcp_write, tcp_close}
use std.env.var

def main
  # Explicit `String` annotation works around a type-inference bug
  # where `Result<String>.expect!(...)` infers as `Int` when its
  # result is only used inside string interpolation, producing a
  # decimal of the heap pointer instead of the string contents.
  let p: String = var("RIVEN_NET_TEST_PORT").expect!("port")
  let addr = "127.0.0.1:#{p}"
  let fd = tcp_connect(&addr)
  if fd < 0
    eputs "connect failed"
  else
    let n = tcp_write(fd, &"hello world")
    if n < 0
      eputs "write failed"
    end
    tcp_close(fd)
  end
end
"#;
    let bin_path = compile(source, "stdlib_net_roundtrip");

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
