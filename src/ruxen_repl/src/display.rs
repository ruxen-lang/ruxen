//! Result display formatting for the REPL.
//!
//! Formats values with their types: `=> value : Type`
//! Uses ANSI colors for terminal output.

use ruxen_core::hir::types::Ty;
use ruxen_core::parser::ast::{FuncDef, Program, TopLevelItem};
use ruxen_core::typeck;

// ANSI color codes
const GREEN: &str = "\x1b[32m";
const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";
const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";

/// Format and display an expression result with its type.
///
/// Returns the formatted string. Unit results return None (suppressed).
pub fn format_result(raw_value: i64, ty: &Ty) -> Option<String> {
    match ty {
        Ty::Unit => None,
        _ => {
            let value_str = format_value(raw_value, ty);
            let type_str = format_type(ty);
            Some(format!(
                "{GREEN}=>{RESET} {value_str} {DIM}: {type_str}{RESET}"
            ))
        }
    }
}

/// Format a raw value according to its type.
pub fn format_value(raw: i64, ty: &Ty) -> String {
    match ty {
        Ty::Int | Ty::Int64 | Ty::ISize => format!("{}", raw),
        Ty::Int8 => format!("{}", raw as i8),
        Ty::Int16 => format!("{}", raw as i16),
        Ty::Int32 => format!("{}", raw as i32),
        Ty::UInt | Ty::UInt64 | Ty::USize => format!("{}", raw as u64),
        Ty::UInt8 => format!("{}", raw as u8),
        Ty::UInt16 => format!("{}", raw as u16),
        Ty::UInt32 => format!("{}", raw as u32),
        Ty::Float | Ty::Float64 => {
            let f = f64::from_bits(raw as u64);
            format!("{}", f)
        }
        Ty::Float32 => {
            let f = f32::from_bits(raw as u32);
            format!("{}", f)
        }
        Ty::Bool => {
            if raw != 0 {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        Ty::Char => {
            if let Some(c) = char::from_u32(raw as u32) {
                format!("'{}'", c)
            } else {
                format!("'\\u{{{:x}}}'", raw)
            }
        }
        Ty::String | Ty::Str => {
            if raw == 0 {
                "\"\"".to_string()
            } else {
                // Safety: The C runtime returns null-terminated strings that
                // live in malloc'd memory. Copy immediately to a Rust String
                // to avoid lifetime concerns with C-managed memory.
                let ptr = raw as *const std::ffi::c_char;
                let s = unsafe { std::ffi::CStr::from_ptr(ptr) };
                match s.to_str() {
                    // Use debug formatting so embedded \n / \t / \" are
                    // rendered escaped on a single line. Keeps the REPL's
                    // `=>` echo one line and matches how users write string
                    // literals in source.
                    Ok(s) => format!("{:?}", s),
                    Err(_) => format!("<invalid string at 0x{:x}>", raw),
                }
            }
        }
        _ => {
            // For composite types, just show the raw pointer/value
            if raw == 0 {
                "nil".to_string()
            } else {
                format!("<{} at 0x{:x}>", format_type(ty), raw as u64)
            }
        }
    }
}

/// Format a Ty as a human-readable type string.
pub fn format_type(ty: &Ty) -> String {
    match ty {
        Ty::Int => "Int".to_string(),
        Ty::Int8 => "Int8".to_string(),
        Ty::Int16 => "Int16".to_string(),
        Ty::Int32 => "Int32".to_string(),
        Ty::Int64 => "Int64".to_string(),
        Ty::UInt => "UInt".to_string(),
        Ty::UInt8 => "UInt8".to_string(),
        Ty::UInt16 => "UInt16".to_string(),
        Ty::UInt32 => "UInt32".to_string(),
        Ty::UInt64 => "UInt64".to_string(),
        Ty::ISize => "ISize".to_string(),
        Ty::USize => "USize".to_string(),
        Ty::Float => "Float".to_string(),
        Ty::Float32 => "Float32".to_string(),
        Ty::Float64 => "Float64".to_string(),
        Ty::Bool => "Bool".to_string(),
        Ty::Char => "Char".to_string(),
        Ty::Unit => "Unit".to_string(),
        Ty::Never => "Never".to_string(),
        Ty::String => "String".to_string(),
        Ty::Str => "&str".to_string(),
        Ty::Array(inner) => format!("Array[{}]", format_type(inner)),
        Ty::Map(k, v) => format!("Hash[{}, {}]", format_type(k), format_type(v)),
        Ty::Set(inner) => format!("Set[{}]", format_type(inner)),
        Ty::Option(inner) => format!("Option[{}]", format_type(inner)),
        Ty::Result(ok, err) => format!("Result[{}, {}]", format_type(ok), format_type(err)),
        Ty::Ref(inner) => format!("&{}", format_type(inner)),
        Ty::RefMut(inner) => format!("&var {}", format_type(inner)),
        Ty::Tuple(elems) => {
            let parts: Vec<String> = elems.iter().map(format_type).collect();
            format!("({})", parts.join(", "))
        }
        Ty::FixedArray(inner, size) => format!("[{}; {}]", format_type(inner), size),
        Ty::Fn { params, ret } => {
            let parts: Vec<String> = params.iter().map(format_type).collect();
            format!("Fn({}) -> {}", parts.join(", "), format_type(ret))
        }
        Ty::Class { name, .. } => name.clone(),
        Ty::Struct { name, .. } => name.clone(),
        Ty::Enum { name, .. } => name.clone(),
        _ => format!("{:?}", ty),
    }
}

/// Format a function's type with parameter names — used by `:type foo`
/// when `foo` is a known user-defined function. `Ty::Fn` alone carries
/// only anonymous parameter types, so we fall back to type-checking the
/// def (together with any other known defs it may reference) and
/// stringifying the resolved parameter list.
pub fn format_fn_type_for_def(target: &FuncDef, all_defs: &[FuncDef]) -> String {
    // Build a program containing all accumulated defs so the target can
    // reference its siblings during type checking.
    let items: Vec<TopLevelItem> = all_defs
        .iter()
        .cloned()
        .map(TopLevelItem::Function)
        .collect();
    let program = Program {
        items,
        span: target.span.clone(),
    };
    let result = typeck::type_check(&program);

    let resolved = result
        .program
        .items
        .iter()
        .filter_map(|item| {
            if let ruxen_core::hir::nodes::HirItem::Function(f) = item {
                if f.name == target.name {
                    let params: Vec<(String, Ty)> = f
                        .params
                        .iter()
                        .map(|p| (p.name.clone(), p.ty.clone()))
                        .collect();
                    return Some((params, f.return_ty.clone()));
                }
            }
            None
        })
        .next();

    let (params, return_ty) = resolved.unwrap_or_else(|| (Vec::new(), Ty::Unit));

    let param_strs: Vec<String> = params
        .iter()
        .map(|(n, t)| format!("{}: {}", n, format_type(t)))
        .collect();
    format!(
        "Fn({}) -> {}",
        param_strs.join(", "),
        format_type(&return_ty)
    )
}

/// Format a function signature for display after definition.
pub fn format_fn_signature(name: &str, params: &[(String, Ty)], return_ty: &Ty) -> String {
    let param_strs: Vec<String> = params
        .iter()
        .map(|(n, t)| format!("{}: {}", n, format_type(t)))
        .collect();
    format!(
        "{GREEN}=>{RESET} {name} {DIM}: Fn({}) -> {}{RESET}",
        param_strs.join(", "),
        format_type(return_ty)
    )
}

/// Format a REPL error message (compact, 2-4 lines).
pub fn format_error(message: &str) -> String {
    format!("{RED}{BOLD}Error:{RESET} {RED}{message}{RESET}")
}
#[cfg(test)]
mod tests {
    use super::*;

    // ── format_type ────────────────────────────────────────────────

    #[test]
    fn format_type_primitives() {
        assert_eq!(format_type(&Ty::Int), "Int");
        assert_eq!(format_type(&Ty::Int8), "Int8");
        assert_eq!(format_type(&Ty::Int16), "Int16");
        assert_eq!(format_type(&Ty::Int32), "Int32");
        assert_eq!(format_type(&Ty::Int64), "Int64");
        assert_eq!(format_type(&Ty::UInt), "UInt");
        assert_eq!(format_type(&Ty::UInt8), "UInt8");
        assert_eq!(format_type(&Ty::UInt16), "UInt16");
        assert_eq!(format_type(&Ty::UInt32), "UInt32");
        assert_eq!(format_type(&Ty::UInt64), "UInt64");
        assert_eq!(format_type(&Ty::ISize), "ISize");
        assert_eq!(format_type(&Ty::USize), "USize");
        assert_eq!(format_type(&Ty::Float), "Float");
        assert_eq!(format_type(&Ty::Float32), "Float32");
        assert_eq!(format_type(&Ty::Float64), "Float64");
        assert_eq!(format_type(&Ty::Bool), "Bool");
        assert_eq!(format_type(&Ty::Char), "Char");
        assert_eq!(format_type(&Ty::Unit), "Unit");
        assert_eq!(format_type(&Ty::Never), "Never");
        assert_eq!(format_type(&Ty::String), "String");
        assert_eq!(format_type(&Ty::Str), "&str");
    }

    #[test]
    fn format_type_composite() {
        assert_eq!(format_type(&Ty::Array(Box::new(Ty::Int))), "Array[Int]");
        assert_eq!(
            format_type(&Ty::Map(Box::new(Ty::String), Box::new(Ty::Int))),
            "Hash[String, Int]",
        );
        assert_eq!(format_type(&Ty::Set(Box::new(Ty::Bool))), "Set[Bool]");
        assert_eq!(format_type(&Ty::Option(Box::new(Ty::Int))), "Option[Int]");
        assert_eq!(
            format_type(&Ty::Result(Box::new(Ty::Int), Box::new(Ty::String))),
            "Result[Int, String]",
        );
    }

    #[test]
    fn format_type_references() {
        assert_eq!(format_type(&Ty::Ref(Box::new(Ty::Int))), "&Int");
        assert_eq!(format_type(&Ty::RefMut(Box::new(Ty::Bool))), "&var Bool");
    }

    #[test]
    fn format_type_tuple_and_array() {
        assert_eq!(
            format_type(&Ty::Tuple(vec![Ty::Int, Ty::Bool, Ty::Char])),
            "(Int, Bool, Char)",
        );
        assert_eq!(
            format_type(&Ty::FixedArray(
                Box::new(Ty::Int),
                ruxen_core::hir::types::ConstExpr::Lit(4)
            )),
            "[Int; 4]"
        );
    }

    #[test]
    fn format_type_function() {
        let ty = Ty::Fn {
            params: vec![Ty::Int, Ty::Int],
            ret: Box::new(Ty::Int),
        };
        assert_eq!(format_type(&ty), "Fn(Int, Int) -> Int");
    }

    #[test]
    fn format_type_class_uses_name() {
        let ty = Ty::Class {
            name: "Point".to_string(),
            generic_args: vec![],
        };
        assert_eq!(format_type(&ty), "Point");
    }

    // ── format_value ───────────────────────────────────────────────

    #[test]
    fn format_value_int_positive() {
        assert_eq!(format_value(42, &Ty::Int), "42");
        assert_eq!(format_value(0, &Ty::Int), "0");
        assert_eq!(format_value(-5, &Ty::Int), "-5");
    }

    #[test]
    fn format_value_bool_true_and_false() {
        assert_eq!(format_value(1, &Ty::Bool), "true");
        assert_eq!(format_value(0, &Ty::Bool), "false");
        // Any non-zero value → true (defensive: JIT ABI for bool is i8
        // normalized to 0/1, but consumers should not depend on that).
        assert_eq!(format_value(42, &Ty::Bool), "true");
    }

    #[test]
    fn format_value_char_ascii() {
        // 'R' = U+0052 = 82.
        assert_eq!(format_value('R' as i64, &Ty::Char), "'R'");
        assert_eq!(format_value('a' as i64, &Ty::Char), "'a'");
    }

    #[test]
    fn format_value_char_unicode() {
        // 'π' = U+03C0 = 960.
        assert_eq!(format_value('π' as i64, &Ty::Char), "'π'");
    }

    #[test]
    fn format_value_char_invalid_codepoint_falls_back_to_hex() {
        // Surrogate range is not a valid Unicode scalar → fallback path.
        let invalid: i64 = 0xD800;
        let out = format_value(invalid, &Ty::Char);
        assert!(
            out.starts_with("'\\u{"),
            "expected \\u{{...}} fallback, got {:?}",
            out
        );
    }

    #[test]
    fn format_value_narrow_ints_widen_correctly() {
        // A raw i64 of 255 should read as -1 through Int8's sign-extension
        // and as 255 through UInt8.
        assert_eq!(format_value(255, &Ty::Int8), "-1");
        assert_eq!(format_value(255, &Ty::UInt8), "255");
        assert_eq!(format_value(65535, &Ty::Int16), "-1");
        assert_eq!(format_value(65535, &Ty::UInt16), "65535");
        assert_eq!(format_value(-1, &Ty::UInt32), format!("{}", u32::MAX));
    }

    #[test]
    fn format_value_float_round_trip() {
        // Round-trip through f64 bits.
        let f = 3.5_f64;
        let bits = f.to_bits() as i64;
        assert_eq!(format_value(bits, &Ty::Float), "3.5");
        assert_eq!(format_value(bits, &Ty::Float64), "3.5");
    }

    #[test]
    fn format_value_float32_round_trip() {
        let f = 2.5_f32;
        let bits = (f.to_bits() as u64) as i64;
        assert_eq!(format_value(bits, &Ty::Float32), "2.5");
    }

    #[test]
    fn format_value_string_null_pointer_is_empty_quotes() {
        assert_eq!(format_value(0, &Ty::String), "\"\"");
        assert_eq!(format_value(0, &Ty::Str), "\"\"");
    }

    #[test]
    fn format_value_string_reads_c_string() {
        // CStr::from_ptr reads until the nul terminator. A static C string
        // literal lives for the whole process, so the raw pointer is safe.
        let cstr = b"hello\0";
        let ptr = cstr.as_ptr() as usize as i64;
        assert_eq!(format_value(ptr, &Ty::String), "\"hello\"");
        assert_eq!(format_value(ptr, &Ty::Str), "\"hello\"");
    }

    #[test]
    fn format_value_composite_nil_when_zero() {
        let ty = Ty::Array(Box::new(Ty::Int));
        assert_eq!(format_value(0, &ty), "nil");
    }

    #[test]
    fn format_value_composite_nonzero_shows_pointer() {
        // ruby-naming.spec.md §10a: `Vec[T]` → `Array[T]`.
        let ty = Ty::Array(Box::new(Ty::Int));
        let out = format_value(0x1234, &ty);
        assert!(out.contains("Array[Int]"), "got {:?}", out);
        assert!(out.contains("0x1234"), "got {:?}", out);
    }

    // ── format_result ──────────────────────────────────────────────

    #[test]
    fn format_result_unit_is_suppressed() {
        assert!(format_result(0, &Ty::Unit).is_none());
    }

    #[test]
    fn format_result_includes_value_and_type() {
        let out = format_result(42, &Ty::Int).expect("non-unit result");
        // Strip ANSI for a robust assertion.
        let plain = strip_ansi(&out);
        assert!(plain.contains("=> 42"), "got {:?}", plain);
        assert!(plain.contains(": Int"), "got {:?}", plain);
    }

    #[test]
    fn format_result_bool_value() {
        let out = format_result(1, &Ty::Bool).expect("non-unit result");
        let plain = strip_ansi(&out);
        assert!(plain.contains("=> true"), "got {:?}", plain);
        assert!(plain.contains(": Bool"), "got {:?}", plain);
    }

    #[test]
    fn format_result_is_single_line() {
        // The display contract for `format_result` is that the top-level
        // output stays on one line (the REPL prints it with `println!`).
        let out = format_result(42, &Ty::Int).expect("non-unit result");
        assert!(
            !out.contains('\n'),
            "format_result must be single-line, got {:?}",
            out
        );
    }

    // ── format_fn_signature ─────────────────────────────────────────

    #[test]
    fn format_fn_signature_zero_args() {
        let out = format_fn_signature("noop", &[], &Ty::Unit);
        let plain = strip_ansi(&out);
        assert!(plain.contains("noop"));
        assert!(plain.contains("Fn() -> Unit"));
    }

    #[test]
    fn format_fn_signature_with_args() {
        let out = format_fn_signature(
            "add",
            &[("x".to_string(), Ty::Int), ("y".to_string(), Ty::Int)],
            &Ty::Int,
        );
        let plain = strip_ansi(&out);
        assert!(plain.contains("add"));
        assert!(plain.contains("x: Int"));
        assert!(plain.contains("y: Int"));
        assert!(plain.contains("-> Int"));
    }

    // ── format_error / format_warning ───────────────────────────────

    #[test]
    fn format_error_contains_message() {
        let out = format_error("oh no");
        let plain = strip_ansi(&out);
        assert!(plain.contains("Error:"), "got {:?}", plain);
        assert!(plain.contains("oh no"), "got {:?}", plain);
    }

    // ── helpers ─────────────────────────────────────────────────────

    /// Strip ANSI escape sequences so assertions don't depend on the
    /// exact color codes. Implemented manually (not a dependency) since
    /// the tests only need to ignore `\x1b[...m` runs.
    fn strip_ansi(input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let mut chars = input.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\x1b' && chars.peek() == Some(&'[') {
                chars.next(); // consume '['
                              // Consume until a letter terminates the sequence.
                for inner in chars.by_ref() {
                    if inner.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }
}
