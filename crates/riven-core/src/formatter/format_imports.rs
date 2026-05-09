/// Import sorting and formatting.
///
/// Imports are sorted into three groups:
/// 1. Standard library (`Std.*`)
/// 2. External packages
/// 3. Local/project modules
///
/// Within each group, imports are alphabetized. Duplicate imports from the
/// same module path are merged into grouped imports.
use crate::parser::ast::{UseDecl, UseKind};

use super::doc::*;
use super::format_items::format_use;

/// Sort and format a slice of use declarations, returning a single Doc.
pub fn format_sorted_imports(imports: &[UseDecl]) -> Doc {
    if imports.is_empty() {
        return nil();
    }

    // Merge duplicate paths
    let merged = merge_imports(imports);

    // Classify into groups
    let mut std_imports: Vec<UseDecl> = Vec::new();
    let mut external_imports: Vec<UseDecl> = Vec::new();
    let mut local_imports: Vec<UseDecl> = Vec::new();

    for imp in merged {
        match classify_import(&imp) {
            ImportGroup::Std => std_imports.push(imp),
            ImportGroup::External => external_imports.push(imp),
            ImportGroup::Local => local_imports.push(imp),
        }
    }

    // Sort each group alphabetically
    std_imports.sort_by_key(import_sort_key);
    external_imports.sort_by_key(import_sort_key);
    local_imports.sort_by_key(import_sort_key);

    // Build doc
    let mut groups: Vec<Vec<Doc>> = Vec::new();

    if !std_imports.is_empty() {
        groups.push(std_imports.iter().map(format_use).collect());
    }
    if !external_imports.is_empty() {
        groups.push(external_imports.iter().map(format_use).collect());
    }
    if !local_imports.is_empty() {
        groups.push(local_imports.iter().map(format_use).collect());
    }

    let mut parts: Vec<Doc> = Vec::new();
    for (i, group) in groups.into_iter().enumerate() {
        if i > 0 {
            // Blank line between groups
            parts.push(hardline());
        }
        for (j, doc) in group.into_iter().enumerate() {
            if j > 0 {
                parts.push(hardline());
            }
            parts.push(doc);
        }
    }

    concat(parts)
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ImportGroup {
    Std,
    External,
    Local,
}

fn classify_import(u: &UseDecl) -> ImportGroup {
    if let Some(first) = u.path.first() {
        if first == "Std" {
            return ImportGroup::Std;
        }
        // Heuristic: uppercase first segment = external package
        // lowercase first segment = local module
        if first.chars().next().is_some_and(|c| c.is_uppercase()) {
            return ImportGroup::External;
        }
    }
    ImportGroup::Local
}

fn import_sort_key(u: &UseDecl) -> String {
    let mut key = u.path.join(".");
    match &u.kind {
        UseKind::Simple => {}
        UseKind::Alias(alias) => {
            key.push('.');
            key.push_str(alias);
        }
        UseKind::Group(names) => {
            let mut sorted = names.clone();
            sorted.sort();
            key.push_str(".{");
            key.push_str(&sorted.join(","));
            key.push('}');
        }
    }
    key
}

/// Merge imports with the same path into grouped imports.
fn merge_imports(imports: &[UseDecl]) -> Vec<UseDecl> {
    use std::collections::HashMap;

    // Group by path
    let mut by_path: HashMap<Vec<String>, Vec<&UseDecl>> = HashMap::new();
    let mut order: Vec<Vec<String>> = Vec::new();

    for imp in imports {
        let key = imp.path.clone();
        if !by_path.contains_key(&key) {
            order.push(key.clone());
        }
        by_path.entry(key).or_default().push(imp);
    }

    let mut result: Vec<UseDecl> = Vec::new();

    for path in order {
        let group = by_path.remove(&path).unwrap();

        if group.len() == 1 {
            result.push(group[0].clone());
            continue;
        }

        // Multiple imports with same path — try to merge
        let mut simple_names: Vec<String> = Vec::new();
        let mut aliases: Vec<UseDecl> = Vec::new();
        let mut has_simple = false;

        for imp in &group {
            match &imp.kind {
                UseKind::Simple => {
                    has_simple = true;
                    // A simple import of a path is equivalent to importing
                    // the last segment
                    if let Some(last) = imp.path.last() {
                        simple_names.push(last.clone());
                    }
                }
                UseKind::Alias(_) => {
                    aliases.push((*imp).clone());
                }
                UseKind::Group(names) => {
                    simple_names.extend(names.iter().cloned());
                }
            }
        }

        // If we have aliases, those can't be merged
        for alias in aliases {
            result.push(alias);
        }

        if !simple_names.is_empty() {
            simple_names.sort();
            simple_names.dedup();

            if simple_names.len() == 1 && has_simple {
                result.push(group[0].clone());
            } else {
                result.push(UseDecl {
                    path,
                    kind: UseKind::Group(simple_names),
                    span: group[0].span.clone(),
                });
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::token::Span;

    fn make_use(path: &[&str], kind: UseKind) -> UseDecl {
        UseDecl {
            path: path.iter().map(|s| s.to_string()).collect(),
            kind,
            span: Span::new(0, 0, 1, 1),
        }
    }

    #[test]
    fn test_classify_std() {
        let u = make_use(&["Std", "IO", "File"], UseKind::Simple);
        assert_eq!(classify_import(&u), ImportGroup::Std);
    }

    #[test]
    fn test_classify_external() {
        let u = make_use(&["Http", "Client"], UseKind::Simple);
        assert_eq!(classify_import(&u), ImportGroup::External);
    }

    #[test]
    fn test_classify_local() {
        let u = make_use(&["app", "models", "User"], UseKind::Simple);
        assert_eq!(classify_import(&u), ImportGroup::Local);
    }

    #[test]
    fn test_sort_within_group() {
        let imports = vec![
            make_use(&["Std", "IO", "File"], UseKind::Simple),
            make_use(&["Std", "Collections", "Vec"], UseKind::Simple),
        ];
        let doc = format_sorted_imports(&imports);
        let rendered = render(&doc);
        let lines: Vec<&str> = rendered.lines().collect();
        assert!(lines[0].contains("Collections"));
        assert!(lines[1].contains("IO"));
    }

    #[test]
    fn test_group_names_sorted() {
        let u = make_use(
            &["Std", "IO"],
            UseKind::Group(vec!["File".into(), "BufReader".into()]),
        );
        let doc = format_use(&u);
        let rendered = render(&doc);
        assert!(rendered.contains("{BufReader, File}"));
    }
}
