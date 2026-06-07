//! `ruxen explain ECODE` — look up a compiler error code in the
//! central registry and print its long-form explanation
//! (T5.04 phase 3).
//!
//! Phase 2 printed only the title from
//! `ruxen_core::diagnostics::codes::REGISTRY`. Phase 3 augments that
//! with multi-paragraph markdown content embedded at build time via
//! `include_str!`. The markdown source lives in `docs/errors/<code>.md`
//! at the repository root and follows a Why / Example / Fix template.
//!
//! The integration test `tests/explain_long_form.rs` and the unit test
//! `every_registered_code_has_embedded_markdown` enforce that every
//! REGISTRY entry has a matching markdown file — there is no graceful
//! "TODO" fallback for registered codes.

use ruxen_core::diagnostics::codes;

// Embedded long-form explanations, keyed by error code (`static EXPLAINS`).
//
// AUTO-GENERATED at build time by `build.rs`, which scans every
// `docs/errors/*.md` and emits the `EXPLAINS` table (sorted by code) into
// `$OUT_DIR/error_docs_table.rs`. Adding a new error explanation is just
// dropping a `docs/errors/E####.md` file — it is embedded automatically, no
// edit here (the old hand-maintained table drifted from the folder and broke
// `every_registered_code_has_embedded_markdown`, which is why it's generated
// now). That test still guards that every REGISTERED code has a `.md` file.
include!(concat!(env!("OUT_DIR"), "/error_docs_table.rs"));

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
            "unknown error code `{}`. Run `ruxen explain all` to list known codes.",
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
        for info in ruxen_core::diagnostics::codes::REGISTRY {
            assert!(
                load_explanation(info.code).is_some(),
                "no embedded markdown for registered code {}",
                info.code
            );
        }
    }
}
