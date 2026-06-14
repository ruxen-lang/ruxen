//! WASM codegen pin tests (tier 4.03).
//!
//! Gated on `--features llvm` (wasm32 requires the LLVM backend). Run with:
//!
//! ```bash
//! LLVM_SYS_180_PREFIX=/opt/homebrew/opt/llvm@18 \
//!   RUSTFLAGS="-L /opt/homebrew/opt/zstd/lib" \
//!   cargo test -p ruxen_core --features llvm --test wasm_codegen
//! ```
//!
//! Asserts the full pipeline: Ruxen source → MIR (no stdlib bootstrap, the
//! no_std reality) → LLVM wasm32 object with `export_name` attributes →
//! `wasm-ld` → a valid `.wasm` whose exports are callable from Node with the
//! correct results.

#![cfg(feature = "llvm")]

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use ruxen_core::codegen::target::ResolvedTarget;
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use ruxen_core::{borrow_check, typeck};

static N: AtomicU64 = AtomicU64::new(0);

/// Resolve `wasm-ld`: the LLVM-18 prefix the cross work assumes, else PATH.
/// Returns `None` so a host without lld SKIPs (does not fail) the link bars.
fn find_wasm_ld() -> Option<String> {
    if let Some(p) = std::env::var_os("RUXEN_WASM_LD") {
        let s = p.to_string_lossy().to_string();
        if std::path::Path::new(&s).is_file() {
            return Some(s);
        }
    }
    let prefixed = "/opt/homebrew/opt/llvm@18/bin/wasm-ld";
    if std::path::Path::new(prefixed).is_file() {
        return Some(prefixed.to_string());
    }
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|d| d.join("wasm-ld"))
        .find(|c| c.is_file())
        .map(|c| c.to_string_lossy().to_string())
}

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Compile `src` to a wasm32 object via the real Ruxen LLVM backend (no
/// stdlib bootstrap — wasm is a no_std reactor). Returns the object bytes.
fn compile_wasm_object(src: &str) -> Vec<u8> {
    let mut lexer = Lexer::new(src);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    // Empty bootstrap: the no_std reality. No `dispatch runtime` stdlib
    // classes, so `class_infos`/`vtables` stay empty and the LLVM backend
    // (which does not yet emit vtable globals) compiles cleanly.
    let tr = typeck::type_check_with_bootstrap(&program, &[]);
    let errs: Vec<_> = tr
        .diagnostics
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    assert!(errs.is_empty(), "type errors: {errs:?}");
    let be = borrow_check::borrow_check(&tr.program, &tr.symbols);
    assert!(be.is_empty(), "borrow errors: {be:?}");
    let mut lo = ruxen_core::mir::lower::Lowerer::new(&tr.symbols);
    let mir = lo.lower_program(&tr.program).expect("lower");
    assert!(
        mir.class_infos.is_empty() && mir.vtables.is_empty(),
        "a no-bootstrap program must not carry vtables/class_infos"
    );

    let mut cg = ruxen_core::codegen::llvm::CodeGen::new_for_target(
        2,
        Some("wasm32-unknown-unknown".to_string()),
    )
    .expect("llvm codegen new");
    cg.compile_program(&mir).expect("llvm compile wasm");
    cg.finish().expect("finish")
}

fn link_wasm(obj: &[u8]) -> Option<PathBuf> {
    let wasm_ld = find_wasm_ld()?;
    let id = N.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("rxwasm_{}_{}", std::process::id(), id));
    std::fs::create_dir_all(&dir).unwrap();
    let obj_path = dir.join("m.o");
    let wasm_path = dir.join("m.wasm");
    std::fs::write(&obj_path, obj).unwrap();
    let st = Command::new(&wasm_ld)
        .args(["--no-entry", "--export-dynamic", "--allow-undefined"])
        .arg(&obj_path)
        .arg("-o")
        .arg(&wasm_path)
        .status()
        .expect("spawn wasm-ld");
    assert!(st.success(), "wasm-ld failed");
    Some(wasm_path)
}

#[test]
fn lower_records_top_level_defs_as_wasm_exports() {
    // The export set is target-independent metadata, populated at lower time.
    let src = "def add(a: Int32, b: Int32) -> Int32\n  a + b\nend\n\
               def mul(a: Int32, b: Int32) -> Int32\n  a * b\nend\n";
    let mut lexer = Lexer::new(src);
    let tokens = lexer.tokenize().unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse().unwrap();
    let tr = typeck::type_check_with_bootstrap(&program, &[]);
    let mut lo = ruxen_core::mir::lower::Lowerer::new(&tr.symbols);
    let mir = lo.lower_program(&tr.program).unwrap();
    assert!(mir.wasm_exports.contains(&"add".to_string()));
    assert!(mir.wasm_exports.contains(&"mul".to_string()));
}

/// Regression: a 9-arg FFI/host-import call (4 Float + 4 Int after the
/// receiver) must pass ALL its arguments on wasm — and the integer→float `as`
/// cast that feeds the Float args must produce a real `sitofp`, not a
/// type-punned int store into a narrower (f32) struct slot. That punned store
/// was folded by the LLVM optimizer to `undef`, silently dropping the 4th
/// float arg (the canvas `ruxen_canvas_draw_rect` blank-frame bug on wasm).
#[test]
fn ffi_call_passes_all_nine_mixed_width_args() {
    // Mirror the canvas draw_rect shape: a class-body `lib` method with
    // `self` + 4 Float + 4 Int. The Float args originate as `Int` params cast
    // `as Float32`, stored into a struct, reloaded, and widened to Float —
    // exactly the path that lost the 4th float pre-fix.
    let src = "\
struct Box\n\
  x: Float32\n\
  y: Float32\n\
  w: Float32\n\
  h: Float32\n\
end\n\
\n\
class Sink\n\
  lib \"env\"\n\
    def draw9 as \"host_draw9\"(self, x: Float, y: Float, w: Float, h: Float, r: Int, g: Int, b: Int, a: Int) -> Int\n\
  end\n\
end\n\
\n\
def run(s: Sink, ix: Int, iy: Int, iw: Int, ih: Int) -> Int\n\
  let b = Box.new(ix as Float32, iy as Float32, iw as Float32, ih as Float32)\n\
  s.draw9(b.x as Float, b.y as Float, b.w as Float, b.h as Float, 200, 30, 30, 255)\n\
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

    // The JS host records every arg `host_draw9` receives. `run` passes a null
    // receiver (we never deref it) plus 11, 22, 33, 44 for the four floats.
    // Assert all four floats arrive (the 4th — h=44 — is the regression) and
    // the four trailing ints are intact.
    // The module exports its own linear memory and imports ruxen_alloc /
    // ruxen_dealloc (Box.new heap-allocates) plus host_draw9. A trivial bump
    // allocator hands out offsets into the module's own (exported) memory,
    // starting high enough to avoid the wasm data/stack region.
    let script = format!(
        "const b=require('fs').readFileSync({:?});\
         let got=null;\
         let bump=65536;\
         const env={{ \
           ruxen_alloc:(n)=>{{const p=bump; bump+=Number(n); return p;}},\
           ruxen_dealloc:(_p)=>{{}},\
           host_draw9:(...a)=>{{got=a.map(Number);return 0n;}} \
         }};\
         WebAssembly.instantiate(b,{{env}}).then(r=>{{\
           const e=r.instance.exports;\
           e.run(0,11n,22n,33n,44n);\
           const want=[0,11,22,33,44,200,30,30,255];\
           const ok = got && got.length===9 && want.every((v,i)=>got[i]===v);\
           if(!ok){{console.error('WRONG',JSON.stringify(got));process.exit(3)}}\
           console.log('OK');\
         }}).catch(err=>{{console.error(err);process.exit(4)}});",
        wasm_path.to_string_lossy()
    );
    let out = Command::new("node")
        .arg("-e")
        .arg(&script)
        .output()
        .expect("spawn node");
    let _ = std::fs::remove_dir_all(wasm_path.parent().unwrap());
    assert!(
        out.status.success(),
        "9-arg FFI call dropped/garbled an argument on wasm: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("OK"),
        "expected OK from node, got: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn wasm_target_resolves_to_llvm_backend() {
    let t = ResolvedTarget::resolve(Some("wasm32-unknown-unknown")).unwrap();
    assert!(t.requires_llvm_backend());
    assert!(t.is_wasm());
}

#[test]
fn emits_valid_wasm_with_expected_exports() {
    let obj = compile_wasm_object(
        "def add(a: Int32, b: Int32) -> Int32\n  a + b\nend\n\
         def square(n: Int32) -> Int32\n  n * n\nend\n",
    );
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

    // node: validate the module, then call the exports and assert results.
    let script = format!(
        "const b=require('fs').readFileSync({:?});\
         if(!WebAssembly.validate(b)){{console.error('INVALID');process.exit(2)}}\
         WebAssembly.instantiate(b).then(r=>{{\
           const e=r.instance.exports;\
           const ok = typeof e.add==='function' && typeof e.square==='function' \
                      && e.add(2,3)===5 && e.square(9)===81;\
           if(!ok){{console.error('WRONG',Object.keys(e));process.exit(3)}}\
           console.log('OK');\
         }}).catch(err=>{{console.error(err);process.exit(4)}});",
        wasm_path.to_string_lossy()
    );
    let out = Command::new("node")
        .arg("-e")
        .arg(&script)
        .output()
        .expect("spawn node");
    let _ = std::fs::remove_dir_all(wasm_path.parent().unwrap());
    assert!(
        out.status.success(),
        "node validation/call failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("OK"),
        "expected OK from node, got: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}
