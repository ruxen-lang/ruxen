//! Stdout capture infrastructure for the REPL.
//!
//! The REPL replays the full cumulative statement history into each
//! synthetic wrapper so that mutations (`x = x + 1`, `v.push(...)`,
//! `s.insert(...)`, `c.inc`, etc.) actually persist across inputs.
//! As a side effect, every run re-executes every prior `puts` and the
//! captured stdout accumulates. To show the user only the *new* output
//! produced by the latest input, we redirect the runtime's print
//! family (`riven_puts` / `riven_print` / `riven_print_int` /
//! `riven_print_float`) into a process-wide buffer; the REPL then
//! diffs it against the previous run's capture and emits just the
//! delta to real stdout.

use std::sync::Mutex;

/// Process-wide capture buffer. The JIT-linked shim functions append
/// to this; `take_all` returns and clears the accumulated output.
static BUFFER: Mutex<String> = Mutex::new(String::new());

/// Append a raw string to the capture buffer.
fn append(s: &str) {
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
// Signatures mirror the C runtime functions in `runtime/runtime.c`;
// the JIT registers these under the same symbol names so compiled
// code calls us instead of the real stdout-emitting C helpers.

/// Append a C string and a trailing newline. Mirrors C `riven_puts`.
#[no_mangle]
pub extern "C" fn riven_repl_puts_shim(s: *const std::ffi::c_char) {
    if s.is_null() {
        append("(nil)\n");
        return;
    }
    // SAFETY: the JIT passes a null-terminated C string here (same
    // contract as the C runtime). We read through the pointer without
    // assuming any particular lifetime since the string is interned or
    // heap-allocated by the runtime for the duration of the call.
    let c_str = unsafe { std::ffi::CStr::from_ptr(s) };
    match c_str.to_str() {
        Ok(rust) => {
            append(rust);
            append("\n");
        }
        Err(_) => append("(invalid-utf8)\n"),
    }
}

/// Append a C string (no trailing newline). Mirrors C `riven_print`.
#[no_mangle]
pub extern "C" fn riven_repl_print_shim(s: *const std::ffi::c_char) {
    if s.is_null() {
        return;
    }
    let c_str = unsafe { std::ffi::CStr::from_ptr(s) };
    if let Ok(rust) = c_str.to_str() {
        append(rust);
    }
}

/// Append a C string to stderr. We still mix stderr into the capture
/// buffer so the harness diff sees panics / `eputs` messages emitted
/// during a re-run only show up once.
#[no_mangle]
pub extern "C" fn riven_repl_eputs_shim(s: *const std::ffi::c_char) {
    if s.is_null() {
        return;
    }
    let c_str = unsafe { std::ffi::CStr::from_ptr(s) };
    if let Ok(rust) = c_str.to_str() {
        // Route to real stderr — it's not part of stdout diffing.
        eprintln!("{rust}");
    }
}

/// Mirrors C `riven_print_int`: prints an integer followed by newline.
#[no_mangle]
pub extern "C" fn riven_repl_print_int_shim(n: i64) {
    append(&format!("{n}\n"));
}

/// Mirrors C `riven_print_float`: prints a float followed by newline,
/// using `%g`-style formatting.
#[no_mangle]
pub extern "C" fn riven_repl_print_float_shim(f: f64) {
    // C's `%g` picks between `%e` and `%f`, dropping trailing zeros.
    // Rust's default `{}` for f64 is close enough for REPL display.
    append(&format!("{f}\n"));
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
        riven_repl_puts_shim(cs.as_ptr());
        assert_eq!(take_all(), "hello\n");
    }

    #[test]
    fn print_shim_appends_without_newline() {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        let cs = c("no-nl");
        riven_repl_print_shim(cs.as_ptr());
        assert_eq!(take_all(), "no-nl");
    }

    #[test]
    fn print_int_shim_formats_correctly() {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        riven_repl_print_int_shim(42);
        assert_eq!(take_all(), "42\n");
    }

    #[test]
    fn print_int_shim_handles_negative() {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        riven_repl_print_int_shim(-7);
        assert_eq!(take_all(), "-7\n");
    }

    #[test]
    fn print_float_shim_formats_correctly() {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        riven_repl_print_float_shim(2.5);
        assert_eq!(take_all(), "2.5\n");
    }

    #[test]
    fn null_pointer_to_puts_produces_nil_marker() {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        riven_repl_puts_shim(std::ptr::null());
        assert_eq!(take_all(), "(nil)\n");
    }

    #[test]
    fn null_pointer_to_print_is_noop() {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        riven_repl_print_shim(std::ptr::null());
        assert_eq!(take_all(), "");
    }

    #[test]
    fn multi_call_accumulation_preserves_order() {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        let a = c("a");
        let b = c("b");
        riven_repl_puts_shim(a.as_ptr());
        riven_repl_print_shim(b.as_ptr());
        riven_repl_print_int_shim(1);
        assert_eq!(take_all(), "a\nb1\n");
    }

    #[test]
    fn clear_zeros_the_buffer_mid_accumulation() {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        let a = c("first");
        riven_repl_puts_shim(a.as_ptr());
        clear();
        let b = c("second");
        riven_repl_puts_shim(b.as_ptr());
        assert_eq!(take_all(), "second\n");
    }

    #[test]
    fn take_all_twice_returns_first_then_empty() {
        let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        let cs = c("x");
        riven_repl_puts_shim(cs.as_ptr());
        assert_eq!(take_all(), "x\n");
        assert_eq!(take_all(), "");
    }
}
