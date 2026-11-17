//! The compiler's built-in macro names — the single source of truth for
//! which `name!(...)` spellings are a MACRO rather than an ordinary call.
//!
//! # Why this registry exists (B87)
//!
//! Ruby names methods with a trailing `!` all the time (`save!`, `sort!`,
//! `exit!`, `update!`). Ruxen keeps that convention: the lexer glues a
//! trailing `!` (and `?`) onto the preceding identifier, so `shout!` arrives
//! as a single `Identifier("shout!")` token.
//!
//! The parser used to treat **every** identifier ending in `!` as a macro
//! call, strip the `!`, and emit `ExprKind::MacroCall`. Nothing downstream
//! recognised the resulting name, and MIR's macro lowering ended in a
//! `_ => Ok(None)` catch-all, so the whole call evaporated: `shout!(5)`
//! compiled clean and did nothing (and `let r = twice!(3)` silently produced
//! `0`). That is why `exit!` could not ship.
//!
//! The fix is to make "is this a macro?" a **closed** question answered in one
//! place. A `name!` is a macro if and only if `name` appears below. Every
//! other trailing `!` is just part of a Ruby bang name and the identifier is
//! carried through verbatim, so the call resolves — or fails to resolve with a
//! hard `undefined function` diagnostic — like any other call.
//!
//! Adding a name here is therefore a language-level decision: it *reserves*
//! that spelling, because a user function named `foo!` can no longer be called
//! once `foo` is a macro. Keep the list minimal, and give every entry a
//! lowering arm in `mir::lower::expr::misc` (the catch-all there is an error,
//! not a no-op, so a registered-but-unlowered macro fails loudly).

/// Names the compiler recognises as built-in macros, **without** the `!`.
///
/// * `panic` — lowers to `ruxen_panic(msg)` + `Terminator::Unreachable`.
/// * `array` / `vec` / `map` / `hash` / `set` — retired by
///   `ruby-naming.spec.md` §10a in favour of the literal forms. They stay
///   registered so the old spellings reach resolve, which rejects them with
///   E1126 and a migration message (see [`RETIRED_MACRO_NAMES`]).
pub const BUILTIN_MACRO_NAMES: &[&str] = &["panic", "array", "vec", "map", "hash", "set"];

/// The registered macro names that are retired: resolve rejects each with
/// E1126 instead of lowering it.
pub const RETIRED_MACRO_NAMES: &[&str] = &["array", "vec", "map", "hash", "set"];

/// True when `name` (the identifier with any trailing `!` already stripped)
/// is one of the compiler's built-in macros.
pub fn is_builtin_macro(name: &str) -> bool {
    BUILTIN_MACRO_NAMES.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_the_registered_macros() {
        for name in BUILTIN_MACRO_NAMES {
            assert!(is_builtin_macro(name), "`{name}!` should be a macro");
        }
    }

    #[test]
    fn ruby_bang_names_are_not_macros() {
        // B87: these are ordinary function/method names, not macros.
        for name in ["shout", "save", "exit", "sort", "update", "twice"] {
            assert!(!is_builtin_macro(name), "`{name}!` must not be a macro");
        }
    }

    #[test]
    fn the_bang_is_not_part_of_the_registered_name() {
        // Callers must strip the `!` before asking.
        assert!(!is_builtin_macro("panic!"));
    }
}
