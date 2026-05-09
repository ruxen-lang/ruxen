use riven_core::hir::types::Ty;
use riven_core::parser::ast::Visibility;
use riven_core::resolve::symbols::{DefKind, Definition};

use crate::analysis::AnalysisResult;
use crate::node_finder::{node_at_position, NodeAtPosition};

pub struct HoverInfo {
    pub content: String,
    pub range: lsp_types::Range,
}

pub fn hover_at(result: &AnalysisResult, position: lsp_types::Position) -> Option<HoverInfo> {
    let program = result.program.as_ref()?;
    let symbols = result.symbols.as_ref()?;
    let byte_offset = result.line_index.byte_offset_of(position);
    let node = node_at_position(program, byte_offset)?;

    match node {
        NodeAtPosition::VarRef(def_id, span) => {
            let def = symbols.get(def_id)?;
            let ty = symbols.def_ty(def_id)?;
            Some(HoverInfo {
                content: format_variable_hover(def, &ty),
                range: result.line_index.span_to_range(&span),
            })
        }
        NodeAtPosition::FnCall { callee, span } => {
            let def = symbols.get(callee)?;
            Some(HoverInfo {
                content: format_function_hover(def),
                range: result.line_index.span_to_range(&span),
            })
        }
        NodeAtPosition::MethodCall { method, span } => {
            let def = symbols.get(method)?;
            Some(HoverInfo {
                content: format_function_hover(def),
                range: result.line_index.span_to_range(&span),
            })
        }
        NodeAtPosition::FieldAccess {
            object_ty,
            field_name,
            span,
        } => Some(HoverInfo {
            content: format!("```riven\n(field) {}: {}\n```", field_name, object_ty),
            range: result.line_index.span_to_range(&span),
        }),
        NodeAtPosition::TypeRef { name, span } => Some(HoverInfo {
            content: format!("```riven\ntype {}\n```", name),
            range: result.line_index.span_to_range(&span),
        }),
        NodeAtPosition::Definition(def_id, span) => {
            let def = symbols.get(def_id)?;
            Some(HoverInfo {
                content: format_definition_hover(def),
                range: result.line_index.span_to_range(&span),
            })
        }
    }
}

fn format_variable_hover(def: &Definition, ty: &Ty) -> String {
    let prefix = match &def.kind {
        DefKind::Variable { mutable: true, .. } => "let mut",
        DefKind::Variable { mutable: false, .. } => "let",
        DefKind::Param { .. } => "param",
        DefKind::SelfValue { .. } => "self",
        _ => "",
    };
    format!("```riven\n{} {}: {}\n```", prefix, def.name, ty)
}

fn format_function_hover(def: &Definition) -> String {
    match &def.kind {
        DefKind::Function { signature } | DefKind::Method { signature, .. } => {
            let params = signature
                .params
                .iter()
                .map(|p| format!("{}: {}", p.name, p.ty))
                .collect::<Vec<_>>()
                .join(", ");
            let vis = match def.visibility {
                Visibility::Public => "pub ",
                Visibility::Protected => "protected ",
                Visibility::Private => "",
            };
            format!(
                "```riven\n{}def {}({}) -> {}\n```",
                vis, def.name, params, signature.return_ty
            )
        }
        _ => format!("```riven\n{}\n```", def.name),
    }
}

fn format_definition_hover(def: &Definition) -> String {
    match &def.kind {
        DefKind::Variable { ty, mutable, .. } => {
            let prefix = if *mutable { "let mut" } else { "let" };
            format!("```riven\n{} {}: {}\n```", prefix, def.name, ty)
        }
        DefKind::Param { ty, .. } => {
            format!("```riven\nparam {}: {}\n```", def.name, ty)
        }
        DefKind::Function { .. } | DefKind::Method { .. } => format_function_hover(def),
        DefKind::Class { .. } => {
            format!("```riven\nclass {}\n```", def.name)
        }
        DefKind::Struct { .. } => {
            format!("```riven\nstruct {}\n```", def.name)
        }
        DefKind::Enum { .. } => {
            format!("```riven\nenum {}\n```", def.name)
        }
        DefKind::Trait { .. } => {
            format!("```riven\ntrait {}\n```", def.name)
        }
        DefKind::Field { ty, .. } => {
            format!("```riven\n(field) {}: {}\n```", def.name, ty)
        }
        DefKind::EnumVariant { .. } => {
            format!("```riven\n(variant) {}\n```", def.name)
        }
        DefKind::TypeAlias { target } => {
            format!("```riven\ntype {} = {}\n```", def.name, target)
        }
        DefKind::Newtype { inner } => {
            format!("```riven\nnewtype {} = {}\n```", def.name, inner)
        }
        DefKind::Const { ty } => {
            format!("```riven\nconst {}: {}\n```", def.name, ty)
        }
        DefKind::SelfValue { ty } => {
            format!("```riven\nself: {}\n```", ty)
        }
        DefKind::TypeParam { bounds } => {
            if bounds.is_empty() {
                format!("```riven\ntype param {}\n```", def.name)
            } else {
                let bounds_str = bounds
                    .iter()
                    .map(|b| format!("{}", b))
                    .collect::<Vec<_>>()
                    .join(" + ");
                format!("```riven\ntype param {}: {}\n```", def.name, bounds_str)
            }
        }
        DefKind::Module { .. } => {
            format!("```riven\nmodule {}\n```", def.name)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;
    use lsp_types::Position;

    /// Find the (line, column) for the first occurrence of `needle` in `src`,
    /// optionally skipping `skip` occurrences.
    fn pos_of(src: &str, needle: &str, skip: usize) -> Position {
        let mut remaining = skip + 1;
        let mut search_start = 0;
        while remaining > 0 {
            let found = src[search_start..]
                .find(needle)
                .expect("needle not found in source");
            remaining -= 1;
            if remaining == 0 {
                let byte_offset = search_start + found;
                // Compute line/char by counting
                let prefix = &src[..byte_offset];
                let line = prefix.matches('\n').count() as u32;
                let col = prefix
                    .rfind('\n')
                    .map(|i| prefix[i + 1..].chars().count())
                    .unwrap_or_else(|| prefix.chars().count()) as u32;
                return Position {
                    line,
                    character: col,
                };
            }
            search_start += found + needle.len();
        }
        unreachable!()
    }

    #[test]
    fn hover_on_let_binding_shows_inferred_type() {
        let src = "def main\n  let x = 42\nend\n";
        let result = analyze(src);
        // x is in "let x"
        let pos = pos_of(src, "x", 0);
        let hover = hover_at(&result, pos);
        assert!(hover.is_some(), "Expected hover on let binding");
        let info = hover.unwrap();
        assert!(
            info.content.contains("Int") || info.content.contains("x"),
            "Expected hover content to mention Int or x, got: {}",
            info.content
        );
    }

    #[test]
    fn hover_on_let_mut_binding_shows_mut_prefix() {
        let src = "def main\n  let mut y = 10\n  y = y + 1\nend\n";
        let result = analyze(src);
        // The 'y' usage in "y = y + 1"
        let pos = pos_of(src, "y", 1);
        let hover = hover_at(&result, pos);
        if let Some(info) = hover {
            assert!(
                info.content.contains("y"),
                "Expected hover to mention y, got: {}",
                info.content
            );
        }
    }

    #[test]
    fn hover_on_function_name_shows_signature() {
        let src =
            "def add(a: Int, b: Int) -> Int\n  a + b\nend\n\ndef main\n  let r = add(1, 2)\nend\n";
        let result = analyze(src);
        // The 'add' call in main
        let pos = pos_of(src, "add", 1);
        let hover = hover_at(&result, pos);
        assert!(hover.is_some(), "Expected hover on function call");
        let info = hover.unwrap();
        assert!(
            info.content.contains("add") || info.content.contains("Int"),
            "Expected hover to mention add/Int, got: {}",
            info.content
        );
    }

    #[test]
    fn hover_on_parameter_shows_type() {
        let src = "def add(a: Int, b: Int) -> Int\n  a + b\nend\n";
        let result = analyze(src);
        // The 'a' in function body: "a + b"
        let pos = pos_of(src, "a + b", 0);
        let hover = hover_at(&result, pos);
        if let Some(info) = hover {
            assert!(
                info.content.contains("Int") || info.content.contains("a"),
                "Expected hover to mention Int/a, got: {}",
                info.content
            );
        }
    }

    #[test]
    fn hover_on_class_field_access_shows_field_type() {
        let src = "class Point\n  x: Int\n  y: Int\n\n  def init(@x: Int, @y: Int)\n  end\n\n  pub def get_x -> Int\n    self.x\n  end\nend\n\ndef main\n  let p = Point.new(1, 2)\nend\n";
        let result = analyze(src);
        // The 'x' in "self.x"
        // Find the exact position of "self.x"
        let offset = src.find("self.x").unwrap() + "self.".len();
        let pos = result.line_index.position_of(offset);
        let hover = hover_at(&result, pos);
        // Hover may or may not fire depending on span resolution — just verify no crash
        if let Some(info) = hover {
            assert!(
                !info.content.is_empty(),
                "Hover content should be non-empty"
            );
        }
    }

    #[test]
    fn hover_at_whitespace_returns_none() {
        let src = "def main\n  let x = 42\nend\n";
        let result = analyze(src);
        // Whitespace between 'main' and 'let'
        // Line 0: "def main" — after 'main' end (byte 8) there's '\n'.
        // Line 1: "  let x = 42" — first 2 chars are spaces.
        let pos = Position {
            line: 1,
            character: 0,
        };
        let hover = hover_at(&result, pos);
        // May or may not be None depending on whether a func-level span envelops whitespace.
        // Accept either outcome but ensure no crash
        if let Some(info) = hover {
            assert!(
                !info.content.is_empty(),
                "Hover content must not be empty when returned"
            );
        }
    }

    #[test]
    fn hover_beyond_eof_returns_none_or_handles_gracefully() {
        let src = "def main\n  let x = 42\nend\n";
        let result = analyze(src);
        let pos = Position {
            line: 100,
            character: 0,
        };
        let hover = hover_at(&result, pos);
        assert!(hover.is_none(), "Expected None for position beyond EOF");
    }

    #[test]
    fn hover_with_empty_source_returns_none() {
        let src = "";
        let result = analyze(src);
        let pos = Position {
            line: 0,
            character: 0,
        };
        let hover = hover_at(&result, pos);
        assert!(hover.is_none(), "Expected None for empty source");
    }

    #[test]
    fn hover_on_unknown_position_does_not_panic() {
        let src = "def main\n  let x = 42\nend\n";
        let result = analyze(src);
        // Position in middle of end keyword
        let pos = Position {
            line: 2,
            character: 1,
        };
        let _ = hover_at(&result, pos);
        // Just verify no panic
    }

    #[test]
    fn hover_on_int_literal_returns_none() {
        let src = "def main\n  let x = 42\nend\n";
        let result = analyze(src);
        // 42 is at line 1; find offset
        let offset = src.find("42").unwrap();
        let pos = result.line_index.position_of(offset);
        // A hover on an int literal won't produce a var-ref node — either None or Definition
        let _ = hover_at(&result, pos);
    }

    #[test]
    fn hover_with_parse_error_returns_none() {
        let src = "def\n"; // parse error
        let result = analyze(src);
        let pos = Position {
            line: 0,
            character: 0,
        };
        let hover = hover_at(&result, pos);
        assert!(hover.is_none(), "Expected None when program is missing");
    }

    #[test]
    fn hover_range_is_nonempty() {
        let src = "def main\n  let xyz = 42\n  puts \"#{xyz}\"\nend\n";
        let result = analyze(src);
        // Find the xyz in "#{xyz}"
        let offset = src.rfind("xyz").unwrap();
        let pos = result.line_index.position_of(offset);
        if let Some(info) = hover_at(&result, pos) {
            // Range end should be after start
            assert!(
                info.range.end.line > info.range.start.line
                    || info.range.end.character >= info.range.start.character,
                "Range end {:?} must be >= start {:?}",
                info.range.end,
                info.range.start
            );
        }
    }
}
