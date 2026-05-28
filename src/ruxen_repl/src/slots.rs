//! Persistent slot read/write helpers for the REPL session.
//!
//! Task 1.2 and 1.3 inject synthetic `let <name> = __slot_load(<addr>)`
//! and `__slot_store(<addr>, <name>)` calls into the wrapper body so
//! session-variable state survives across REPL inputs WITHOUT replaying
//! the cumulative statement history (`session.all_statements`). The
//! address argument is taken from `ReplSession::slot_addr(idx)` for the
//! variable's persistent slot in `ReplSession::slots`.
//!
//! Both helpers are 8-byte: every Ruxen heap-owned value is an i64
//! handle (pointer) and primitive `Int`/`Bool`/`Float`/`Char` all fit
//! in 8 bytes when zero-extended, so a single load_i64/store_i64 pair
//! covers every type we put in a session variable today. If a future
//! type widens past 8 bytes (e.g. an unboxed 128-bit int), it'll need
//! a sibling helper.

/// Read 8 bytes from `addr` and return them as an i64. The JIT calls
/// this for every session-variable READ in the wrapper prefix.
///
/// # Safety
///
/// `addr` MUST point at a live slot inside `ReplSession::slots`. The
/// REPL owns the `Box<[i64]>` for the session's lifetime and the
/// caller is the JIT'd wrapper, which only ever passes addresses
/// computed from `slot_addr(idx)`.
#[no_mangle]
pub extern "C" fn ruxen_repl_slot_load_i64(addr: i64) -> i64 {
    // SAFETY: see fn doc — addr is always a slot inside the session's
    // owned slot buffer.
    unsafe { *(addr as *const i64) }
}

/// Write `val` to the 8-byte slot at `addr`. The JIT calls this for
/// every session-variable WRITE in the wrapper suffix.
///
/// # Safety
///
/// Same contract as `ruxen_repl_slot_load_i64` — `addr` is a live
/// slot in the session's `Box<[i64]>`.
#[no_mangle]
pub extern "C" fn ruxen_repl_slot_store_i64(addr: i64, val: i64) {
    // SAFETY: see fn doc.
    unsafe { *(addr as *mut i64) = val };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_load_returns_what_store_wrote() {
        let mut cell: i64 = 0;
        let addr = &mut cell as *mut i64 as i64;
        ruxen_repl_slot_store_i64(addr, 0xDEAD_BEEF_CAFE);
        assert_eq!(ruxen_repl_slot_load_i64(addr), 0xDEAD_BEEF_CAFE);
    }

    #[test]
    fn slot_load_pointer_round_trip() {
        let owned: Box<i64> = Box::new(42);
        let owned_addr = Box::into_raw(owned) as i64;
        let mut cell: i64 = 0;
        let cell_addr = &mut cell as *mut i64 as i64;
        ruxen_repl_slot_store_i64(cell_addr, owned_addr);
        let read_back = ruxen_repl_slot_load_i64(cell_addr);
        assert_eq!(read_back, owned_addr);
        // SAFETY: reconstruct + drop the box so miri stays happy.
        unsafe { drop(Box::from_raw(read_back as *mut i64)) };
    }
}
