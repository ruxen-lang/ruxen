//! Ruby-style compile-time source loading for `require`, `require_relative`,
//! and `autoload`.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::lexer::token::{Token, TokenKind};
use crate::lexer::Lexer;
use crate::parser::Parser;
use crate::typeck;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedFile {
    pub path: PathBuf,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoloadRegistration {
    pub constant: String,
    pub request: String,
    pub declaring_file: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceBundle {
    pub source: String,
    pub files: Vec<LoadedFile>,
    pub autoloads: BTreeMap<String, AutoloadRegistration>,
    pub has_directives: bool,
}

impl SourceBundle {
    pub fn rebuild_source(&mut self) {
        self.source = combine_files(&self.files);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RequirementKind {
    LoadPath,
    Relative,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Requirement {
    kind: RequirementKind,
    request: String,
}

#[derive(Debug, Default)]
struct ScanResult {
    source: String,
    requirements: Vec<Requirement>,
    autoloads: Vec<(String, String)>,
    has_directives: bool,
}

pub struct ModuleLoader {
    load_paths: Vec<PathBuf>,
    loaded: HashSet<PathBuf>,
    files: Vec<LoadedFile>,
    autoloads: BTreeMap<String, AutoloadRegistration>,
    root: Option<PathBuf>,
    has_directives: bool,
}

impl ModuleLoader {
    pub fn new(load_paths: Vec<PathBuf>) -> Self {
        Self {
            load_paths,
            loaded: HashSet::new(),
            files: Vec::new(),
            autoloads: BTreeMap::new(),
            root: None,
            has_directives: false,
        }
    }

    pub fn load(mut self, entry: &Path) -> Result<SourceBundle, String> {
        let root = canonical_file(entry)?;
        self.root = Some(root.clone());
        self.load_file(&root)?;
        self.expand_autoloads()?;
        self.keep_root_last();
        Ok(SourceBundle {
            source: combine_files(&self.files),
            files: self.files,
            autoloads: self.autoloads,
            has_directives: self.has_directives,
        })
    }

    fn load_file(&mut self, path: &Path) -> Result<bool, String> {
        let path = canonical_file(path)?;
        if !self.loaded.insert(path.clone()) {
            return Ok(false);
        }

        let source = fs::read_to_string(&path)
            .map_err(|error| format!("failed to read {}: {}", path.display(), error))?;
        let scan = scan_directives(&source, &path)?;
        self.has_directives |= scan.has_directives;

        for (constant, request) in scan.autoloads {
            self.autoloads.insert(
                constant.clone(),
                AutoloadRegistration {
                    constant,
                    request,
                    declaring_file: path.clone(),
                },
            );
        }

        for requirement in scan.requirements {
            let resolved = match requirement.kind {
                RequirementKind::LoadPath => self.resolve_load_path(&requirement.request)?,
                RequirementKind::Relative => {
                    let directory = path.parent().unwrap_or_else(|| Path::new("."));
                    resolve_relative(directory, &requirement.request)?
                }
            };
            self.load_file(&resolved)?;
        }

        self.files.push(LoadedFile {
            path,
            source: scan.source,
        });
        Ok(true)
    }

    fn resolve_load_path(&self, request: &str) -> Result<PathBuf, String> {
        resolve_load_path(request, &self.load_paths)
    }

    fn expand_autoloads(&mut self) -> Result<(), String> {
        if self.autoloads.is_empty() {
            return Ok(());
        }
        loop {
            let source = combine_files(&self.files);
            let unresolved = compile_unresolved_names(&source);
            let pending: Vec<AutoloadRegistration> = self
                .autoloads
                .values()
                .filter(|registration| {
                    !autoload_already_loaded(registration, &self.loaded, &self.load_paths)
                        && unresolved_names_match(&unresolved, &registration.constant)
                })
                .cloned()
                .collect();

            if pending.is_empty() {
                break;
            }

            let mut added = false;
            for registration in pending {
                let resolved = self.resolve_load_path(&registration.request)?;
                added |= self.load_file(&resolved)?;
            }
            self.keep_root_last();
            if !added {
                break;
            }
        }
        Ok(())
    }

    fn keep_root_last(&mut self) {
        let Some(root) = &self.root else {
            return;
        };
        let Some(index) = self.files.iter().position(|file| &file.path == root) else {
            return;
        };
        if index + 1 != self.files.len() {
            let root_file = self.files.remove(index);
            self.files.push(root_file);
        }
    }
}

pub fn resolve_relative(directory: &Path, request: &str) -> Result<PathBuf, String> {
    let candidate = requested_path(&directory.join(request));
    canonical_candidate(&candidate).map_err(|_| {
        format!(
            "LoadError: cannot load such file -- {}\n  searched:\n    {}",
            request,
            candidate.display()
        )
    })
}

pub fn resolve_load_path(request: &str, load_paths: &[PathBuf]) -> Result<PathBuf, String> {
    let requested = Path::new(request);
    if requested.is_absolute() {
        let candidate = requested_path(requested);
        return canonical_candidate(&candidate).map_err(|_| {
            format!(
                "LoadError: cannot load such file -- {}\n  searched:\n    {}",
                request,
                candidate.display()
            )
        });
    }

    let candidates: Vec<PathBuf> = load_paths
        .iter()
        .map(|directory| requested_path(&directory.join(request)))
        .collect();
    for candidate in &candidates {
        if let Ok(path) = canonical_candidate(candidate) {
            return Ok(path);
        }
    }

    let searched = if candidates.is_empty() {
        "    (load path is empty)".to_string()
    } else {
        candidates
            .iter()
            .map(|path| format!("    {}", path.display()))
            .collect::<Vec<_>>()
            .join("\n")
    };
    Err(format!(
        "LoadError: cannot load such file -- {}\n  searched:\n{}",
        request, searched
    ))
}

fn requested_path(path: &Path) -> PathBuf {
    if path.extension().and_then(|extension| extension.to_str()) == Some("rx") {
        path.to_path_buf()
    } else {
        path.with_extension("rx")
    }
}

fn canonical_candidate(path: &Path) -> Result<PathBuf, std::io::Error> {
    if !path.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "not a file",
        ));
    }
    fs::canonicalize(path)
}

fn canonical_file(path: &Path) -> Result<PathBuf, String> {
    canonical_candidate(path)
        .map_err(|error| format!("failed to load {}: {}", path.display(), error))
}

fn combine_files(files: &[LoadedFile]) -> String {
    let mut combined = String::new();
    for file in files {
        combined.push_str("# module-loader: ");
        combined.extend(
            file.path
                .to_string_lossy()
                .chars()
                .flat_map(char::escape_default),
        );
        combined.push('\n');
        combined.push_str(&file.source);
        if !file.source.ends_with('\n') {
            combined.push('\n');
        }
    }
    combined
}

fn scan_directives(source: &str, path: &Path) -> Result<ScanResult, String> {
    let tokens = Lexer::new(source).tokenize().map_err(|diagnostics| {
        diagnostics
            .iter()
            .map(|diagnostic| format!("{}: {}", path.display(), diagnostic))
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    let mut result = ScanResult {
        source: source.to_string(),
        ..ScanResult::default()
    };
    let mut stack: Vec<Option<String>> = Vec::new();
    let mut lines_to_strip = HashSet::new();

    for line in token_lines(&tokens) {
        if line.is_empty() {
            continue;
        }
        let at_namespace_scope = stack.iter().all(Option::is_some);
        if at_namespace_scope {
            if let Some(requirement) = parse_requirement(&line, path)? {
                result.requirements.push(requirement);
                result.has_directives = true;
                lines_to_strip.insert(line[0].span.line);
            } else if let Some((constant, request)) = parse_autoload(&line, &stack, path)? {
                result.autoloads.push((constant, request));
                result.has_directives = true;
                lines_to_strip.insert(line[0].span.line);
            }
        }
        update_scope_stack(&line, &mut stack);
    }

    if !lines_to_strip.is_empty() {
        result.source = source
            .split_inclusive('\n')
            .enumerate()
            .map(|(index, line)| {
                if lines_to_strip.contains(&((index + 1) as u32)) {
                    if line.ends_with('\n') {
                        "\n"
                    } else {
                        ""
                    }
                } else {
                    line
                }
            })
            .collect();
    }
    Ok(result)
}

fn token_lines(tokens: &[Token]) -> Vec<Vec<Token>> {
    let mut lines = Vec::new();
    let mut current = Vec::new();
    for token in tokens {
        match token.kind {
            TokenKind::Newline | TokenKind::Eof => {
                if !current.is_empty() {
                    lines.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(token.clone()),
        }
    }
    lines
}

fn parse_requirement(tokens: &[Token], path: &Path) -> Result<Option<Requirement>, String> {
    let Some(Token {
        kind: TokenKind::Identifier(name),
        ..
    }) = tokens.first()
    else {
        return Ok(None);
    };
    let kind = match name.as_str() {
        "require" => RequirementKind::LoadPath,
        "require_relative" => RequirementKind::Relative,
        _ => return Ok(None),
    };
    if tokens.len() != 2 {
        return Err(directive_syntax_error(path, tokens, name));
    }
    let TokenKind::StringLiteral(request) = &tokens[1].kind else {
        return Err(directive_syntax_error(path, tokens, name));
    };
    Ok(Some(Requirement {
        kind,
        request: request.clone(),
    }))
}

fn parse_autoload(
    tokens: &[Token],
    stack: &[Option<String>],
    path: &Path,
) -> Result<Option<(String, String)>, String> {
    let mut namespace = stack.iter().filter_map(Clone::clone).collect::<Vec<_>>();

    let mut cursor = match tokens.first().map(|token| &token.kind) {
        Some(TokenKind::Identifier(name)) if name == "autoload" => 1,
        Some(TokenKind::TypeIdentifier(name)) => {
            namespace = vec![name.clone()];
            1
        }
        _ => return Ok(None),
    };
    if matches!(
        tokens.first().map(|token| &token.kind),
        Some(TokenKind::TypeIdentifier(_))
    ) {
        while cursor + 1 < tokens.len()
            && matches!(tokens[cursor].kind, TokenKind::Dot | TokenKind::ColonColon)
            && matches!(tokens[cursor + 1].kind, TokenKind::TypeIdentifier(_))
        {
            if let TokenKind::TypeIdentifier(segment) = &tokens[cursor + 1].kind {
                namespace.push(segment.clone());
            }
            cursor += 2;
        }
        if cursor + 1 >= tokens.len()
            || !matches!(tokens[cursor].kind, TokenKind::Dot)
            || !matches!(&tokens[cursor + 1].kind, TokenKind::Identifier(name) if name == "autoload")
        {
            return Ok(None);
        }
        cursor += 2;
    }

    if tokens.len() != cursor + 4
        || !matches!(tokens[cursor].kind, TokenKind::Colon)
        || !matches!(tokens[cursor + 1].kind, TokenKind::TypeIdentifier(_))
        || !matches!(tokens[cursor + 2].kind, TokenKind::Comma)
        || !matches!(tokens[cursor + 3].kind, TokenKind::StringLiteral(_))
    {
        return Err(directive_syntax_error(path, tokens, "autoload"));
    }
    let TokenKind::TypeIdentifier(constant) = &tokens[cursor + 1].kind else {
        unreachable!();
    };
    let TokenKind::StringLiteral(request) = &tokens[cursor + 3].kind else {
        unreachable!();
    };
    namespace.push(constant.clone());
    Ok(Some((namespace.join("."), request.clone())))
}

fn directive_syntax_error(path: &Path, tokens: &[Token], directive: &str) -> String {
    let line = tokens.first().map(|token| token.span.line).unwrap_or(1);
    format!(
        "{}:{}: invalid `{}` directive; expected Ruby-style syntax",
        path.display(),
        line,
        directive
    )
}

fn update_scope_stack(tokens: &[Token], stack: &mut Vec<Option<String>>) {
    let Some(first) = tokens.first() else {
        return;
    };
    if matches!(first.kind, TokenKind::End) {
        stack.pop();
        return;
    }
    match &first.kind {
        TokenKind::Module => {
            let name = tokens.get(1).and_then(|token| match &token.kind {
                TokenKind::TypeIdentifier(name) => Some(name.clone()),
                _ => None,
            });
            stack.push(name);
        }
        TokenKind::Class
        | TokenKind::Struct
        | TokenKind::Enum
        | TokenKind::Mixin
        | TokenKind::Extension
        | TokenKind::Def
        | TokenKind::If
        | TokenKind::Unless
        | TokenKind::While
        | TokenKind::For
        | TokenKind::Loop
        | TokenKind::Match
        | TokenKind::Do => stack.push(None),
        TokenKind::Async
            if matches!(tokens.get(1).map(|token| &token.kind), Some(TokenKind::Def)) =>
        {
            stack.push(None)
        }
        _ => {}
    }
}

/// Resolve `source` and return the raw identifiers name resolution could not
/// resolve. Drives the autoload fixpoint: a registration whose constant appears
/// here is loaded, and the loop repeats until the set is empty (or no file was
/// added). Parse/lex failures yield an empty list (a genuine syntax error fails
/// the real build downstream).
fn compile_unresolved_names(source: &str) -> Vec<String> {
    let Ok(tokens) = Lexer::new(source).tokenize() else {
        return Vec::new();
    };
    let Ok(program) = Parser::new(tokens).parse() else {
        return Vec::new();
    };
    typeck::type_check(&program).unresolved_names
}

/// True when `constant` is one of the raw identifiers name resolution reported
/// as unresolved. Tries the registration's fully-qualified name, its `::` form,
/// and its leaf, comparing by NAME EQUALITY against the structured `unresolved`
/// list rather than scraping `diagnostic.message` text (the old
/// `diagnostic_mentions_constant`). The structured signal removes the coupling
/// to diagnostic wording and cannot be fooled by an unrelated error that
/// happens to mention the name.
fn unresolved_names_match(unresolved: &[String], constant: &str) -> bool {
    let leaf = constant.rsplit('.').next().unwrap_or(constant);
    let alternatives = [
        constant.to_string(),
        constant.replace('.', "::"),
        leaf.to_string(),
    ];
    unresolved
        .iter()
        .any(|name| alternatives.iter().any(|alt| alt == name))
}

fn autoload_already_loaded(
    registration: &AutoloadRegistration,
    loaded: &HashSet<PathBuf>,
    load_paths: &[PathBuf],
) -> bool {
    resolve_load_path(&registration.request, load_paths)
        .map(|path| loaded.contains(&path))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(name: &str) -> PathBuf {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ruxen-module-loader-{}-{}-{}",
            name,
            std::process::id(),
            serial
        ));
        fs::create_dir_all(&path).expect("create temp dir");
        path
    }

    fn write(path: &Path, source: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, source).expect("write source");
    }

    #[test]
    fn require_relative_uses_calling_file_directory() {
        let root = temp_dir("relative");
        write(
            &root.join("app/main.rx"),
            "require_relative \"../lib/math\"\n",
        );
        write(&root.join("lib/math.rx"), "module MathUtil\nend\n");

        let bundle = ModuleLoader::new(vec![root.join("app")])
            .load(&root.join("app/main.rx"))
            .expect("load relative");

        assert_eq!(bundle.files.len(), 2);
        assert!(bundle.files[0].path.ends_with("lib/math.rx"));
        assert!(bundle.files[1].path.ends_with("app/main.rx"));
        fs::remove_dir_all(root).expect("remove temp dir");
    }

    #[test]
    fn require_searches_load_paths_in_order() {
        let root = temp_dir("load-path");
        write(&root.join("app/main.rx"), "require \"shared/value\"\n");
        write(
            &root.join("first/shared/value.rx"),
            "const VALUE: Int = 1\n",
        );
        write(
            &root.join("second/shared/value.rx"),
            "const VALUE: Int = 2\n",
        );

        let bundle = ModuleLoader::new(vec![root.join("first"), root.join("second")])
            .load(&root.join("app/main.rx"))
            .expect("load path");

        assert!(bundle.files[0].path.ends_with("first/shared/value.rx"));
        fs::remove_dir_all(root).expect("remove temp dir");
    }

    #[test]
    fn require_deduplicates_canonical_paths_across_diamond() {
        let root = temp_dir("diamond");
        write(
            &root.join("main.rx"),
            "require_relative \"left\"\nrequire_relative \"right\"\n",
        );
        write(&root.join("left.rx"), "require_relative \"shared\"\n");
        write(&root.join("right.rx"), "require_relative \"shared.rx\"\n");
        write(&root.join("shared.rx"), "module Shared\nend\n");

        let bundle = ModuleLoader::new(vec![root.clone()])
            .load(&root.join("main.rx"))
            .expect("load diamond");

        assert_eq!(bundle.files.len(), 4);
        assert_eq!(
            bundle
                .files
                .iter()
                .filter(|file| file.path.ends_with("shared.rx"))
                .count(),
            1
        );
        fs::remove_dir_all(root).expect("remove temp dir");
    }

    #[test]
    fn require_cycles_terminate_and_load_each_file_once() {
        let root = temp_dir("cycle");
        write(
            &root.join("a.rx"),
            "require_relative \"b\"\nmodule A\nend\n",
        );
        write(
            &root.join("b.rx"),
            "require_relative \"a\"\nmodule B\nend\n",
        );

        let bundle = ModuleLoader::new(vec![root.clone()])
            .load(&root.join("a.rx"))
            .expect("load cycle");

        assert_eq!(bundle.files.len(), 2);
        fs::remove_dir_all(root).expect("remove temp dir");
    }

    #[test]
    fn missing_require_lists_every_searched_path() {
        let root = temp_dir("missing");
        write(&root.join("main.rx"), "require \"missing/item\"\n");

        let error = ModuleLoader::new(vec![root.join("one"), root.join("two")])
            .load(&root.join("main.rx"))
            .expect_err("missing require must fail");

        assert!(error.contains("LoadError: cannot load such file -- missing/item"));
        assert!(error.contains("one/missing/item.rx"));
        assert!(error.contains("two/missing/item.rx"));
        fs::remove_dir_all(root).expect("remove temp dir");
    }

    #[test]
    fn autoload_expands_to_fixpoint_and_records_namespace() {
        let root = temp_dir("autoload");
        write(
            &root.join("main.rx"),
            "autoload :First, \"first\"\nputs First.value\n",
        );
        write(
            &root.join("first.rx"),
            "autoload :Second, \"second\"\nclass First\n  def self.value -> Int\n    Second.value\n  end\nend\n",
        );
        write(
            &root.join("second.rx"),
            "class Second\n  def self.value -> Int\n    42\n  end\nend\n",
        );

        let bundle = ModuleLoader::new(vec![root.clone()])
            .load(&root.join("main.rx"))
            .expect("autoload fixpoint");

        assert_eq!(bundle.files.len(), 3);
        assert!(bundle.autoloads.contains_key("First"));
        assert!(bundle.autoloads.contains_key("Second"));
        assert!(bundle.files.last().unwrap().path.ends_with("main.rx"));
        fs::remove_dir_all(root).expect("remove temp dir");
    }

    #[test]
    fn namespace_scoped_autoload_uses_qualified_key() {
        let root = temp_dir("namespace");
        write(
            &root.join("main.rx"),
            "module Foo\n  autoload :Bar, \"foo/bar\"\nend\n",
        );
        write(
            &root.join("foo/bar.rx"),
            "module Foo\n  class Bar\n  end\nend\n",
        );

        let bundle = ModuleLoader::new(vec![root.clone()])
            .load(&root.join("main.rx"))
            .expect("register namespace");

        assert!(bundle.autoloads.contains_key("Foo.Bar"));
        assert_eq!(bundle.files.len(), 1, "unreferenced autoload stays lazy");
        fs::remove_dir_all(root).expect("remove temp dir");
    }

    #[test]
    fn unresolved_names_match_uses_name_equality_not_substring() {
        // Bare and qualified constants match a structurally-reported unresolved
        // name (whole-name equality, tried against the `::` form and the leaf).
        assert!(unresolved_names_match(&["First".to_string()], "First"));
        assert!(unresolved_names_match(&["Foo::Bar".to_string()], "Foo.Bar"));
        assert!(unresolved_names_match(&["Bar".to_string()], "Foo.Bar"));
        // A constant absent from the unresolved set does not match.
        assert!(!unresolved_names_match(&["Other".to_string()], "First"));
        // The robustness win over the old diagnostic-text scrape: a longer,
        // unrelated name that merely CONTAINS the constant as a substring does
        // NOT match, because matching is by whole-name equality.
        assert!(!unresolved_names_match(
            &["FirstResponder".to_string()],
            "First"
        ));
    }
}
