//! Stdout capture infrastructure for the REPL.
//!
//! Task 3 (Path A — runtime replay-suppression flag) reshaped how
//! `puts` / `print` flow through the REPL. The capture shims registered
//! into the JIT (see `jit::register_runtime_symbols`) now do TWO
//! things on every call:
//!
//!   1. Write to **real stdout/stderr** immediately. This restores
//!      correct interleaving with subprocess output (`Command.status`
//!      writes the child's stdout directly to fd 1 via the kernel —
//!      the prior buffered-then-drained design always emitted `puts`
//!      AFTER subprocess stdout because the buffer flushed at wrapper
//!      exit, which broke fixtures like `508_command_status`).
//!   2. Append to a process-wide **`BUFFER`** so test harnesses (see
//!      `tests/state_persistence::run_session` and
//!      `tests/single_execution`) can snapshot what each input wrote
//!      without intercepting fd 1.
//!
//! Replay suppression: the C runtime's `ruxen_repl_is_replaying`
//! thread-local flag (set by the REPL around the replay portion of
//! each wrapper) gates both write paths. When the flag is non-zero,
//! every shim early-returns — matching the gate on the REAL
//! `ruxen_puts` / `ruxen_print` / … in
//! `library/std/io/runtime/stdio.c`. That mirroring is what lets a
//! replayed `puts` inside `let_bindings` / `session_var_mutations`
//! no-op cleanly even though the surrounding statement re-runs.

use std::sync::Mutex;

/// Process-wide capture buffer. The JIT-linked shim functions
/// append to this whenever the runtime replay-suppression flag is
/// clear (i.e. on the user's new-statement path). Test harnesses
/// drain via `take_all`.
static BUFFER: Mutex<String> = Mutex::new(String::new());

extern "C" {
    /// Cross-language linkage to the runtime replay-suppression flag
    /// (`library/std/core/runtime/repl_replay.c`). The capture shims
    /// below early-return when this is non-zero so the replayed
    /// portion of each input's wrapper doesn't duplicate stdout. The
    /// REAL `ruxen_puts` / `ruxen_print` family (in
    /// `library/std/io/runtime/stdio.c`) check the same flag — the
    /// shims override the JIT's `ruxen_puts` symbol so we have to
    /// re-check it here for the suppression to take effect on the
    /// REPL's stdout path.
    fn ruxen_repl_get_replaying() -> i32;

    /// Real C runtime print family. The shims delegate to these for
    /// stdout/stderr output, which routes through libc's stdio
    /// buffering — matching exactly how AOT-compiled binaries
    /// emit `puts` / `print`. Crucial for ordering with subprocess
    /// output that writes directly to fd 1 via the kernel (e.g.
    /// `Command.status` forking `/bin/echo`): when stdout is piped
    /// (as it is under the e2e harness), libc fully-buffers parent-
    /// process writes until exit, so `puts` lines after a child's
    /// output appear in the order the kernel saw them, not the
    /// order the parent issued them.
    fn ruxen_puts(s: *const std::ffi::c_char);
    fn ruxen_print(s: *const std::ffi::c_char);
    fn ruxen_eputs(s: *const std::ffi::c_char);
    fn ruxen_print_int(n: i64);
    fn ruxen_print_float(f: f64);
}

#[inline(always)]
fn is_replaying() -> bool {
    // SAFETY: pure TLS read with no aliasing concerns.
    unsafe { ruxen_repl_get_replaying() != 0 }
}

/// Append a raw string to the capture buffer (test-only sink).
fn buffer_append(s: &str) {
    if let Ok(mut buf) = BUFFER.lock() {
        buf.push_str(s);
    }
}

/// Read and clear the full capture buffer.
pub fn take_all() -> String {
    if let Ok(mut buf) = BUFFER.lock() {
        std::mem::take(&mut *buf)
    } else {
        String::new()
    }
}

/// Clear the capture buffer without reading it.
pub fn clear() {
    if let Ok(mut buf) = BUFFER.lock() {
        buf.clear();
    }
}

// ── Shim functions linked into the JIT module ──────────────────────
//
// Signatures mirror the C runtime functions in
// `library/std/io/runtime/stdio.c`; the JIT registers these under the
// same symbol names so compiled code calls us instead of the real
// stdout-emitting C helpers. Each shim:
//   1. Returns early when `ruxen_repl_is_replaying` is set.
//   2. Writes to real stdout/stderr immediately (correct ordering
//      with subprocess output).
//   3. Mirrors the same text into `BUFFER` for test snapshots.

/// Delegate to C `ruxen_puts` (which calls libc `puts`) and mirror
/// into BUFFER for test harnesses. The libc path provides the same
/// stdio buffering that AOT-compiled binaries use, so the e2e
/// fixture diffs see identical ordering between `puts` lines and
/// subprocess-stdout lines.
#[no_mangle]
pub extern "C" fn ruxen_repl_puts_shim(s: *const std::ffi::c_char) {
    if is_replaying() {
        return;
    }
    unsafe { ruxen_puts(s) };
    if s.is_null() {
        buffer_append("(nil)\n");
        return;
    }
    // SAFETY: the JIT passes a null-terminated C string here (same
    // contract as the C runtime). We read through the pointer without
    // assuming any particular lifetime since the string is interned
    // or heap-allocated by the runtime for the duration of the call.
    let c_str = unsafe { std::ffi::CStr::from_ptr(s) };
    if let Ok(rust) = c_str.to_str() {
        buffer_append(rust);
        buffer_append("\n");
    } else {
        buffer_append("(invalid-utf8)\n");
    }
}

/// Delegate to C `ruxen_print` (no trailing newline). Mirrors into
/// BUFFER for test harnesses.
#[no_mangle]
pub extern "C" fn ruxen_repl_print_shim(s: *const std::ffi::c_char) {
    if is_replaying() {
        return;
    }
    unsafe { ruxen_print(s) };
    if s.is_null() {
        return;
    }
    let c_str = unsafe { std::ffi::CStr::from_ptr(s) };
    if let Ok(rust) = c_str.to_str() {
        buffer_append(rust);
    }
}

/// Delegate to C `ruxen_eputs` (stderr). Mirrors into BUFFER so the
/// test harness can assert on diagnostic-emitting inputs, though
/// normal e2e harnesses diff stdout only.
#[no_mangle]
pub extern "C" fn ruxen_repl_eputs_shim(s: *const std::ffi::c_char) {
    if is_replaying() {
        return;
    }
    unsafe { ruxen_eputs(s) };
    if s.is_null() {
        return;
    }
    let c_str = unsafe { std::ffi::CStr::from_ptr(s) };
    if let Ok(rust) = c_str.to_str() {
        buffer_append(rust);
        buffer_append("\n");
    }
}

/// Delegate to C `ruxen_print_int` (prints int + newline via libc).
#[no_mangle]
pub extern "C" fn ruxen_repl_print_int_shim(n: i64) {
    if is_replaying() {
        return;
    }
    unsafe { ruxen_print_int(n) };
    buffer_append(&format!("{n}\n"));
}

/// Delegate to C `ruxen_print_float` (prints float + newline via
/// libc `%g`).
#[no_mangle]
pub extern "C" fn ruxen_repl_print_float_shim(f: f64) {
    if is_replaying() {
        return;
    }
    unsafe { ruxen_print_float(f) };
    buffer_append(&format!("{f}\n"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::sync::Mutex;

    // The BUFFER is global and shared across tests; serialize access.
    static LOCK: Mutex<()> = Mutex::new(());

    fn c(s: &str) -> CString {
        CString::new(s).unwrap()
    }

    #[test]
    fn take_all_after_clear_is_empty() {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        assert_eq!(take_all(), "");
    }

    #[test]
    fn puts_shim_appends_value_and_newline() {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        let cs = c("hello");
        ruxen_repl_puts_shim(cs.as_ptr());
        assert_eq!(take_all(), "hello\n");
    }

    #[test]
    fn print_shim_appends_without_newline() {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        let cs = c("no-nl");
        ruxen_repl_print_shim(cs.as_ptr());
        assert_eq!(take_all(), "no-nl");
    }

    #[test]
    fn print_int_shim_formats_correctly() {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        ruxen_repl_print_int_shim(42);
        assert_eq!(take_all(), "42\n");
    }

    #[test]
    fn print_int_shim_handles_negative() {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        ruxen_repl_print_int_shim(-7);
        assert_eq!(take_all(), "-7\n");
    }

    #[test]
    fn print_float_shim_formats_correctly() {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        ruxen_repl_print_float_shim(2.5);
        assert_eq!(take_all(), "2.5\n");
    }

    #[test]
    fn null_pointer_to_puts_produces_nil_marker() {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        ruxen_repl_puts_shim(std::ptr::null());
        assert_eq!(take_all(), "(nil)\n");
    }

    #[test]
    fn null_pointer_to_print_is_noop() {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        ruxen_repl_print_shim(std::ptr::null());
        assert_eq!(take_all(), "");
    }

    #[test]
    fn multi_call_accumulation_preserves_order() {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        let a = c("a");
        let b = c("b");
        ruxen_repl_puts_shim(a.as_ptr());
        ruxen_repl_print_shim(b.as_ptr());
        ruxen_repl_print_int_shim(1);
        assert_eq!(take_all(), "a\nb1\n");
    }

    #[test]
    fn clear_zeros_the_buffer_mid_accumulation() {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        let a = c("first");
        ruxen_repl_puts_shim(a.as_ptr());
        clear();
        let b = c("second");
        ruxen_repl_puts_shim(b.as_ptr());
        assert_eq!(take_all(), "second\n");
    }

    #[test]
    fn take_all_twice_returns_first_then_empty() {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        let cs = c("x");
        ruxen_repl_puts_shim(cs.as_ptr());
        assert_eq!(take_all(), "x\n");
        assert_eq!(take_all(), "");
    }
}
