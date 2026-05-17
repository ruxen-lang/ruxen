//! `riven explain ECODE` — look up a compiler error code in the
//! central registry and print its long-form explanation
//! (T5.04 phase 3).
//!
//! Phase 2 printed only the title from
//! `riven_core::diagnostics::codes::REGISTRY`. Phase 3 augments that
//! with multi-paragraph markdown content embedded at build time via
//! `include_str!`. The markdown source lives in `docs/errors/<code>.md`
//! at the repository root and follows a Why / Example / Fix template.
//!
//! The integration test `tests/explain_long_form.rs` and the unit test
//! `every_registered_code_has_embedded_markdown` enforce that every
//! REGISTRY entry has a matching markdown file — there is no graceful
//! "TODO" fallback for registered codes.

use riven_core::diagnostics::codes;

/// Embedded long-form explanations, keyed by error code. The list must
/// stay in sync with `riven_core::diagnostics::codes::REGISTRY`; the
/// unit test below ensures coverage.
static EXPLAINS: &[(&str, &str)] = &[
    // ── Lexer ────────────────────────────────────────────────────────
    ("E0001", include_str!("../../../docs/errors/E0001.md")),
    ("E0002", include_str!("../../../docs/errors/E0002.md")),
    ("E0003", include_str!("../../../docs/errors/E0003.md")),
    ("E0004", include_str!("../../../docs/errors/E0004.md")),
    ("E0005", include_str!("../../../docs/errors/E0005.md")),
    ("E0006", include_str!("../../../docs/errors/E0006.md")),
    ("E0007", include_str!("../../../docs/errors/E0007.md")),
    // ── Test fixture sentinel ───────────────────────────────────────
    ("E0042", include_str!("../../../docs/errors/E0042.md")),
    // ── Derive macros ───────────────────────────────────────────────
    ("E0601", include_str!("../../../docs/errors/E0601.md")),
    ("E0602", include_str!("../../../docs/errors/E0602.md")),
    ("E0603", include_str!("../../../docs/errors/E0603.md")),
    ("E0604", include_str!("../../../docs/errors/E0604.md")),
    ("E0605", include_str!("../../../docs/errors/E0605.md")),
    ("E0606", include_str!("../../../docs/errors/E0606.md")),
    ("E0608", include_str!("../../../docs/errors/E0608.md")),
    ("E0609", include_str!("../../../docs/errors/E0609.md")),
    ("E0610", include_str!("../../../docs/errors/E0610.md")),
    ("E0611", include_str!("../../../docs/errors/E0611.md")),
    ("E0613", include_str!("../../../docs/errors/E0613.md")),
    ("E0615", include_str!("../../../docs/errors/E0615.md")),
    ("E0616", include_str!("../../../docs/errors/E0616.md")),
    ("E0617", include_str!("../../../docs/errors/E0617.md")),
    ("E0618", include_str!("../../../docs/errors/E0618.md")),
    // ── Tier-2 type system ──────────────────────────────────────────
    ("E0700", include_str!("../../../docs/errors/E0700.md")),
    ("E0701", include_str!("../../../docs/errors/E0701.md")),
    ("E0702", include_str!("../../../docs/errors/E0702.md")),
    ("E0703", include_str!("../../../docs/errors/E0703.md")),
    ("E0704", include_str!("../../../docs/errors/E0704.md")),
    ("E0705", include_str!("../../../docs/errors/E0705.md")),
    ("E0706", include_str!("../../../docs/errors/E0706.md")),
    // Phase 2 #06.5 T1: IoError variant constructor arity.
    ("E0710", include_str!("../../../docs/errors/E0710.md")),
    // ── Borrow checker / trait-impl ─────────────────────────────────
    ("E1001", include_str!("../../../docs/errors/E1001.md")),
    ("E1002", include_str!("../../../docs/errors/E1002.md")),
    ("E1003", include_str!("../../../docs/errors/E1003.md")),
    ("E1004", include_str!("../../../docs/errors/E1004.md")),
    ("E1005", include_str!("../../../docs/errors/E1005.md")),
    ("E1006", include_str!("../../../docs/errors/E1006.md")),
    ("E1007", include_str!("../../../docs/errors/E1007.md")),
    ("E1008", include_str!("../../../docs/errors/E1008.md")),
    ("E1009", include_str!("../../../docs/errors/E1009.md")),
    ("E1010", include_str!("../../../docs/errors/E1010.md")),
    ("E1011", include_str!("../../../docs/errors/E1011.md")),
    ("E1012", include_str!("../../../docs/errors/E1012.md")),
    ("E1013", include_str!("../../../docs/errors/E1013.md")),
    ("E1014", include_str!("../../../docs/errors/E1014.md")),
];

/// Look up the embedded markdown explanation for `code`. Returns
/// `None` when no markdown is registered for the code.
pub fn load_explanation(code: &str) -> Option<&'static str> {
    EXPLAINS
        .iter()
        .find_map(|(c, body)| if *c == code { Some(*body) } else { None })
}

/// Print the long-form explanation for `code`, falling back to a
/// title-only message when no markdown is embedded. `code == "all"`
/// (or `"list"`) prints every registered code with its title.
pub fn explain(code: &str) -> Result<(), String> {
    let normalized = normalize_code(code);

    if normalized.eq_ignore_ascii_case("all") || normalized == "list" {
        list_all();
        return Ok(());
    }

    match codes::lookup(&normalized) {
        Some(info) => {
            match load_explanation(info.code) {
                Some(body) => {
                    // The first line of the markdown is `# <code>: <title>`,
                    // which already serves as the heading; just print the
                    // body verbatim.
                    print!("{body}");
                    if !body.ends_with('\n') {
                        println!();
                    }
                }
                None => {
                    println!("{}: {}", info.code, info.title);
                    println!();
                    println!(
                        "(no long-form explanation is embedded for this code yet — \
                         only the title above is available)"
                    );
                }
            }
            Ok(())
        }
        None => Err(format!(
            "unknown error code `{}`. Run `riven explain all` to list known codes.",
            code
        )),
    }
}

/// Accept user input forms `e0001`, `E0001`, `0001`, `1`, `E1` → `E0001`.
fn normalize_code(input: &str) -> String {
    let trimmed = input.trim();
    let body = trimmed
        .strip_prefix(|c: char| c == 'E' || c == 'e')
        .unwrap_or(trimmed);
    if body.chars().all(|c| c.is_ascii_digit()) && !body.is_empty() {
        format!("E{:0>4}", body)
    } else {
        trimmed.to_string()
    }
}

fn list_all() {
    println!("Registered error codes:");
    for info in codes::REGISTRY {
        println!("  {}  {}", info.code, info.title);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_code_pads_to_four_digits() {
        assert_eq!(normalize_code("E1"), "E0001");
        assert_eq!(normalize_code("e0001"), "E0001");
        assert_eq!(normalize_code("E0001"), "E0001");
        assert_eq!(normalize_code("1"), "E0001");
        assert_eq!(normalize_code("0042"), "E0042");
    }

    #[test]
    fn explain_unknown_code_is_err() {
        let result = explain("E9999");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown"));
    }

    #[test]
    fn explain_known_code_succeeds() {
        assert!(explain("E0001").is_ok());
        assert!(explain("E1011").is_ok());
    }

    #[test]
    fn explain_all_succeeds() {
        assert!(explain("all").is_ok());
    }

    #[test]
    fn load_explanation_returns_some_for_registered_code() {
        let body = load_explanation("E0001").expect("E0001 markdown should be embedded");
        assert!(body.contains("## Why"));
        assert!(body.contains("## Example"));
        assert!(body.contains("## Fix"));
    }

    #[test]
    fn load_explanation_returns_none_for_unregistered_code() {
        assert!(load_explanation("E9999").is_none());
    }

    #[test]
    fn every_registered_code_has_embedded_markdown() {
        // Mirror of the registry-coverage integration test, but at the
        // unit-test layer: ensures the EXPLAINS table stays in sync with
        // the central registry as new codes are added.
        for info in riven_core::diagnostics::codes::REGISTRY {
            assert!(
                load_explanation(info.code).is_some(),
                "no embedded markdown for registered code {}",
                info.code
            );
        }
    }
}
