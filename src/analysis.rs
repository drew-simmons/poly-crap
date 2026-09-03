use crate::model::{CodeUnit, Diagnostic, Language};
use anyhow::{Context, Result, anyhow};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use rayon::prelude::*;
use std::collections::HashSet;
use std::path::Path;
use tree_sitter::{Node, Parser};

/// Every function found under a root, with counts of the files behind them.
#[derive(Debug, Default)]
pub struct Analysis {
    /// One per named function, sorted by file, then start line, then symbol.
    pub units: Vec<CodeUnit>,
    /// Files of an enabled language that survived the excludes.
    pub candidate_files: usize,
    /// Candidates whose syntax tree had no errors.
    pub parsed_files: usize,
    /// One entry per file that failed to read or parse.
    pub diagnostics: Vec<Diagnostic>,
}

struct FileAnalysis {
    units: Vec<CodeUnit>,
    diagnostic: Option<Diagnostic>,
}

struct UnitSpec<'tree> {
    node: Node<'tree>,
    name: String,
}

/// Parse every file of the given languages under `root`, minus the exclude
/// globs, which match paths relative to `root`.
///
/// `.gitignore` and hidden directories are honored. No excludes are applied
/// beyond the ones passed in; see [`crate::config::DEFAULT_EXCLUDES`].
pub fn analyze_tree(root: &Path, languages: &[Language], excludes: &[String]) -> Result<Analysis> {
    validate_root(root)?;
    let exclude_set = build_globs(excludes)?;
    let enabled: HashSet<_> = languages.iter().copied().collect();
    let paths = discover_paths(root, &enabled, &exclude_set);
    analyze_discovered(paths)
}

/// [`analyze_tree`] limited to the `selected` paths, given relative to `root`.
/// A selected path still has to pass the language and exclude filters.
pub fn analyze_paths(
    root: &Path,
    languages: &[Language],
    excludes: &[String],
    selected: &[std::path::PathBuf],
) -> Result<Analysis> {
    validate_root(root)?;
    let exclude_set = build_globs(excludes)?;
    let enabled: HashSet<_> = languages.iter().copied().collect();
    let selected: HashSet<_> = selected.iter().cloned().collect();
    let paths = discover_paths(root, &enabled, &exclude_set)
        .into_iter()
        .filter(|(path, _)| {
            let relative = path.strip_prefix(root).unwrap_or(path);
            selected.contains(relative)
        })
        .collect();
    analyze_discovered(paths)
}

fn analyze_discovered(paths: Vec<(std::path::PathBuf, Language)>) -> Result<Analysis> {
    let results: Vec<_> = paths
        .par_iter()
        .map(|(path, language)| analyze_file(path, *language))
        .collect();
    let mut analysis = Analysis {
        candidate_files: paths.len(),
        ..Analysis::default()
    };
    for result in results {
        add_file_result(&mut analysis, result);
    }
    sort_analysis(&mut analysis);
    Ok(analysis)
}

fn validate_root(root: &Path) -> Result<()> {
    if !root.exists() {
        return Err(anyhow!("analysis path does not exist: {}", root.display()));
    }
    if !root.is_dir() {
        return Err(anyhow!(
            "analysis path is not a directory: {}",
            root.display()
        ));
    }
    Ok(())
}

fn discover_paths(
    root: &Path,
    enabled: &HashSet<Language>,
    exclude_set: &GlobSet,
) -> Vec<(std::path::PathBuf, Language)> {
    ignore::WalkBuilder::new(root)
        .standard_filters(true)
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .filter_map(|entry| {
            let path = entry.into_path();
            let language = Language::from_path(&path)?;
            if !enabled.contains(&language) {
                return None;
            }
            let relative = path.strip_prefix(root).unwrap_or(&path);
            (!exclude_set.is_match(relative)).then_some((path, language))
        })
        .collect()
}

fn add_file_result(analysis: &mut Analysis, result: Result<FileAnalysis>) {
    match result {
        Ok(file) => add_file_analysis(analysis, file),
        Err(error) => analysis.diagnostics.push(Diagnostic {
            kind: "parse".into(),
            message: error.to_string(),
            file: None,
        }),
    }
}

fn add_file_analysis(analysis: &mut Analysis, file: FileAnalysis) {
    if let Some(diagnostic) = file.diagnostic {
        analysis.diagnostics.push(diagnostic);
        return;
    }
    analysis.parsed_files += 1;
    analysis.units.extend(file.units);
}

fn sort_analysis(analysis: &mut Analysis) {
    analysis.units.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(a.start_line.cmp(&b.start_line))
            .then(a.symbol.cmp(&b.symbol))
    });
    analysis
        .diagnostics
        .sort_by(|a, b| a.file.cmp(&b.file).then(a.message.cmp(&b.message)));
}

fn build_globs(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(
            GlobBuilder::new(pattern)
                .literal_separator(true)
                .build()
                .with_context(|| format!("invalid exclude pattern: {pattern}"))?,
        );
    }
    builder.build().context("building exclude patterns")
}

type GrammarLoader = fn(&Path) -> tree_sitter::Language;

const GRAMMAR_LOADERS: [GrammarLoader; 6] = [
    javascript_grammar,
    typescript_grammar,
    python_grammar,
    go_grammar,
    rust_grammar,
    java_grammar,
];

fn grammar(language: Language, path: &Path) -> tree_sitter::Language {
    GRAMMAR_LOADERS[language.index()](path)
}

fn javascript_grammar(_: &Path) -> tree_sitter::Language {
    tree_sitter_javascript::LANGUAGE.into()
}

fn typescript_grammar(path: &Path) -> tree_sitter::Language {
    if path.extension().is_some_and(|extension| extension == "tsx") {
        tree_sitter_typescript::LANGUAGE_TSX.into()
    } else {
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
    }
}

fn python_grammar(_: &Path) -> tree_sitter::Language {
    tree_sitter_python::LANGUAGE.into()
}

fn go_grammar(_: &Path) -> tree_sitter::Language {
    tree_sitter_go::LANGUAGE.into()
}

fn rust_grammar(_: &Path) -> tree_sitter::Language {
    tree_sitter_rust::LANGUAGE.into()
}

fn java_grammar(_: &Path) -> tree_sitter::Language {
    tree_sitter_java::LANGUAGE.into()
}

fn analyze_file(path: &Path, language: Language) -> Result<FileAnalysis> {
    let source = std::fs::read_to_string(path)
        .with_context(|| format!("reading source file {}", path.display()))?;
    let mut parser = Parser::new();
    parser
        .set_language(&grammar(language, path))
        .with_context(|| format!("loading {language} grammar"))?;
    let tree = parser
        .parse(&source, None)
        .ok_or_else(|| anyhow!("parser returned no tree for {}", path.display()))?;
    if tree.root_node().has_error() {
        return Ok(FileAnalysis {
            units: Vec::new(),
            diagnostic: Some(Diagnostic {
                kind: "parse".into(),
                message: format!(
                    "skipped {} because its syntax tree has errors",
                    path.display()
                ),
                file: Some(path.to_path_buf()),
            }),
        });
    }

    let bytes = source.as_bytes();
    let mut specs = Vec::new();
    collect_units(tree.root_node(), language, bytes, &mut specs);
    let units = specs
        .into_iter()
        .map(|spec| CodeUnit {
            language,
            file: path.to_path_buf(),
            symbol: qualify_symbol(spec.node, language, &spec.name, bytes),
            start_line: spec.node.start_position().row + 1,
            end_line: spec.node.end_position().row + 1,
            complexity: complexity(spec.node, bytes) as f64,
        })
        .collect();
    Ok(FileAnalysis {
        units,
        diagnostic: None,
    })
}

fn collect_units<'tree>(
    node: Node<'tree>,
    language: Language,
    source: &[u8],
    output: &mut Vec<UnitSpec<'tree>>,
) {
    if let Some((unit_node, name)) = named_unit(node, language, source) {
        output.push(UnitSpec {
            node: unit_node,
            name,
        });
    }

    let mut cursor = node.walk();
    let mut attributes = TestAttributes::new(language);
    for child in node.named_children(&mut cursor) {
        if attributes.skips(child, source) {
            continue;
        }
        collect_units(child, language, source, output);
    }
}

/// Tracks Rust test attributes while walking a list of sibling nodes.
///
/// Rust puts an attribute in its own node ahead of the item it annotates, so a
/// `#[cfg(test)] mod tests` block and its `#[test]` functions look like plain
/// units. Scoring them measures the test suite against itself, which says
/// nothing about production risk, and it buries the real entries. Other
/// languages keep their tests in separate files, which the default excludes
/// already drop.
struct TestAttributes {
    rust: bool,
    pending: bool,
}

impl TestAttributes {
    fn new(language: Language) -> Self {
        Self {
            rust: language == Language::Rust,
            pending: false,
        }
    }

    /// Report whether this sibling should be skipped along with its subtree.
    ///
    /// Attributes are always skipped themselves; they hold no units. A test
    /// attribute stays pending across any attributes that follow it, so
    /// `#[cfg(test)]` still suppresses the item under `#[allow(..)]`.
    fn skips(&mut self, node: Node<'_>, source: &[u8]) -> bool {
        if !self.rust {
            return false;
        }
        if node.kind() == "attribute_item" {
            self.pending |= is_test_attribute(node, source);
            return true;
        }
        std::mem::take(&mut self.pending)
    }
}

/// Match `#[cfg(test)]` and `#[test]`-style attributes, including `#[tokio::test]`.
///
/// Compound forms such as `cfg(all(test, feature = "x"))` are not matched; they
/// are rare, and treating them as production code errs toward reporting more.
fn is_test_attribute(node: Node<'_>, source: &[u8]) -> bool {
    let Some(raw) = text(node, source) else {
        return false;
    };
    let stripped: String = raw.chars().filter(|value| !value.is_whitespace()).collect();
    let inner = stripped.trim_start_matches("#[").trim_end_matches(']');
    inner == "cfg(test)" || is_test_marker(inner)
}

fn is_test_marker(inner: &str) -> bool {
    inner == "test" || inner.ends_with("::test")
}

const DECLARATION_KINDS: [&[&str]; 6] = [
    &[
        "function_declaration",
        "generator_function_declaration",
        "method_definition",
    ],
    &[
        "function_declaration",
        "generator_function_declaration",
        "method_definition",
    ],
    &["function_definition"],
    &["function_declaration", "method_declaration"],
    &["function_item"],
    &["method_declaration", "constructor_declaration"],
];

fn named_unit<'tree>(
    node: Node<'tree>,
    language: Language,
    source: &[u8],
) -> Option<(Node<'tree>, String)> {
    if DECLARATION_KINDS[language.index()].contains(&node.kind()) {
        return declared_unit(node, language, source);
    }
    assigned_callable(node, language, source)
}

fn declared_unit<'tree>(
    node: Node<'tree>,
    language: Language,
    source: &[u8],
) -> Option<(Node<'tree>, String)> {
    if is_bodyless_java_declaration(node, language) {
        return None;
    }
    let name = declared_name(node, source)?;
    Some((node, java_signature(node, language, name, source)))
}

fn is_bodyless_java_declaration(node: Node<'_>, language: Language) -> bool {
    language == Language::Java && node.child_by_field_name("body").is_none()
}

fn declared_name(node: Node<'_>, source: &[u8]) -> Option<String> {
    node.child_by_field_name("name")
        .and_then(|name| text(name, source))
        .map(str::to_string)
}

struct AssignmentSpec {
    language: Language,
    node_kind: &'static str,
    name_field: &'static str,
    value_field: &'static str,
    callable_kinds: &'static [&'static str],
}

const JS_CALLABLES: &[&str] = &[
    "arrow_function",
    "function_expression",
    "generator_function",
];
const ASSIGNMENT_SPECS: &[AssignmentSpec] = &[
    AssignmentSpec {
        language: Language::JavaScript,
        node_kind: "variable_declarator",
        name_field: "name",
        value_field: "value",
        callable_kinds: JS_CALLABLES,
    },
    AssignmentSpec {
        language: Language::JavaScript,
        node_kind: "public_field_definition",
        name_field: "name",
        value_field: "value",
        callable_kinds: JS_CALLABLES,
    },
    AssignmentSpec {
        language: Language::JavaScript,
        node_kind: "pair",
        name_field: "key",
        value_field: "value",
        callable_kinds: JS_CALLABLES,
    },
    AssignmentSpec {
        language: Language::JavaScript,
        node_kind: "assignment_expression",
        name_field: "left",
        value_field: "right",
        callable_kinds: JS_CALLABLES,
    },
    AssignmentSpec {
        language: Language::TypeScript,
        node_kind: "variable_declarator",
        name_field: "name",
        value_field: "value",
        callable_kinds: JS_CALLABLES,
    },
    AssignmentSpec {
        language: Language::TypeScript,
        node_kind: "public_field_definition",
        name_field: "name",
        value_field: "value",
        callable_kinds: JS_CALLABLES,
    },
    AssignmentSpec {
        language: Language::TypeScript,
        node_kind: "pair",
        name_field: "key",
        value_field: "value",
        callable_kinds: JS_CALLABLES,
    },
    AssignmentSpec {
        language: Language::TypeScript,
        node_kind: "assignment_expression",
        name_field: "left",
        value_field: "right",
        callable_kinds: JS_CALLABLES,
    },
    AssignmentSpec {
        language: Language::Python,
        node_kind: "assignment",
        name_field: "left",
        value_field: "right",
        callable_kinds: &["lambda"],
    },
    AssignmentSpec {
        language: Language::Go,
        node_kind: "short_var_declaration",
        name_field: "left",
        value_field: "right",
        callable_kinds: &["func_literal"],
    },
    AssignmentSpec {
        language: Language::Go,
        node_kind: "var_spec",
        name_field: "name",
        value_field: "value",
        callable_kinds: &["func_literal"],
    },
    AssignmentSpec {
        language: Language::Rust,
        node_kind: "let_declaration",
        name_field: "pattern",
        value_field: "value",
        callable_kinds: &["closure_expression"],
    },
    AssignmentSpec {
        language: Language::Java,
        node_kind: "variable_declarator",
        name_field: "name",
        value_field: "value",
        callable_kinds: &["lambda_expression"],
    },
];

fn assigned_callable<'tree>(
    node: Node<'tree>,
    language: Language,
    source: &[u8],
) -> Option<(Node<'tree>, String)> {
    let spec = ASSIGNMENT_SPECS
        .iter()
        .find(|spec| spec.language == language && spec.node_kind == node.kind())?;
    let name_node = unwrap_singleton(node.child_by_field_name(spec.name_field)?);
    let value = unwrap_singleton(node.child_by_field_name(spec.value_field)?);
    spec.callable_kinds.contains(&value.kind()).then(|| {
        (
            value,
            normalize(text(name_node, source).unwrap_or("anonymous")),
        )
    })
}

fn unwrap_singleton(mut node: Node<'_>) -> Node<'_> {
    while matches!(node.kind(), "expression_list" | "pattern_list") && node.named_child_count() == 1
    {
        node = node.named_child(0).expect("singleton node has one child");
    }
    node
}

fn java_signature(node: Node<'_>, language: Language, name: String, source: &[u8]) -> String {
    if language != Language::Java {
        return name;
    }
    let Some(parameters) = node.child_by_field_name("parameters") else {
        return name;
    };
    format!(
        "{name}({})",
        java_parameter_types(parameters, source).join(",")
    )
}

fn java_parameter_types(parameters: Node<'_>, source: &[u8]) -> Vec<String> {
    let mut cursor = parameters.walk();
    parameters
        .named_children(&mut cursor)
        .filter_map(|parameter| parameter.child_by_field_name("type"))
        .map(|kind| normalize(text(kind, source).unwrap_or_default()))
        .collect()
}

fn qualify_symbol(node: Node<'_>, language: Language, name: &str, source: &[u8]) -> String {
    let mut parts = container_parts(node, language, source);
    if let Some(receiver) = go_receiver_name(node, language, source) {
        parts.push(receiver);
    }
    parts.push(name.to_string());
    parts.join(SYMBOL_SEPARATORS[language.index()])
}

const SYMBOL_SEPARATORS: [&str; 6] = [".", ".", ".", ".", "::", "."];

fn container_parts(node: Node<'_>, language: Language, source: &[u8]) -> Vec<String> {
    let mut parts = Vec::new();
    let mut parent = node.parent();
    while let Some(ancestor) = parent {
        if is_container(ancestor.kind()) {
            if let Some(value) = container_name(ancestor, language, source) {
                parts.push(value);
            }
        }
        parent = ancestor.parent();
    }
    parts.reverse();
    parts
}

fn is_container(kind: &str) -> bool {
    matches!(
        kind,
        "class_declaration"
            | "abstract_class_declaration"
            | "class"
            | "class_definition"
            | "interface_declaration"
            | "enum_declaration"
            | "record_declaration"
            | "trait_item"
            | "impl_item"
            | "mod_item"
            | "function_declaration"
            | "function_definition"
            | "function_item"
            | "method_definition"
            | "method_declaration"
            | "constructor_declaration"
    )
}

fn go_receiver_name(node: Node<'_>, language: Language, source: &[u8]) -> Option<String> {
    if language != Language::Go {
        return None;
    }
    if node.kind() != "method_declaration" {
        return None;
    }
    let receiver = node.child_by_field_name("receiver")?;
    let parameter = receiver.named_child(0).unwrap_or(receiver);
    let receiver_type = parameter.child_by_field_name("type")?;
    Some(normalize(text(receiver_type, source).unwrap_or_default()))
}

fn container_name(node: Node<'_>, language: Language, source: &[u8]) -> Option<String> {
    if node.kind() == "impl_item" {
        return impl_name(node, source);
    }
    let name = node
        .child_by_field_name("name")
        .or_else(|| assigned_class_name(node, language))?;
    let value = text(name, source)?.to_string();
    if is_java_callable(node, language) {
        return Some(java_signature(node, language, value, source));
    }
    (!value.is_empty()).then_some(value)
}

/// The variable a class expression is assigned to, so the methods of
/// `const Widget = class { ... }` read as `Widget.method` rather than bare.
fn assigned_class_name<'tree>(node: Node<'tree>, language: Language) -> Option<Node<'tree>> {
    if node.kind() != "class" {
        return None;
    }
    let parent = node.parent()?;
    let spec = ASSIGNMENT_SPECS
        .iter()
        .find(|spec| spec.language == language && spec.node_kind == parent.kind())?;
    parent.child_by_field_name(spec.name_field)
}

fn impl_name(node: Node<'_>, source: &[u8]) -> Option<String> {
    let header = text(node, source)?.split('{').next()?.trim();
    header
        .strip_prefix("impl")
        .map(str::trim)
        .and_then(|value| value.split_whitespace().last())
        .map(str::to_string)
}

fn is_java_callable(node: Node<'_>, language: Language) -> bool {
    language == Language::Java
        && matches!(
            node.kind(),
            "method_declaration" | "constructor_declaration"
        )
}

fn complexity(root: Node<'_>, source: &[u8]) -> usize {
    let mut count = 1;
    count_decisions(root, root, source, &mut count);
    count
}

fn count_decisions(node: Node<'_>, root: Node<'_>, source: &[u8], count: &mut usize) {
    if node != root && is_callable(node.kind()) {
        return;
    }
    if is_decision(node, source) {
        *count += 1;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        count_decisions(child, root, source, count);
    }
}

fn is_callable(kind: &str) -> bool {
    matches!(
        kind,
        "function_declaration"
            | "generator_function_declaration"
            | "function_expression"
            | "generator_function"
            | "arrow_function"
            | "method_definition"
            | "function_definition"
            | "lambda"
            | "method_declaration"
            | "func_literal"
            | "function_item"
            | "closure_expression"
            | "constructor_declaration"
            | "lambda_expression"
    )
}

fn is_decision(node: Node<'_>, source: &[u8]) -> bool {
    if is_simple_decision(node) {
        return true;
    }
    if is_arm_decision(node.kind()) {
        return is_non_default_arm(node, source);
    }
    is_boolean_decision(node, source)
}

fn is_simple_decision(node: Node<'_>) -> bool {
    is_let_else(node)
        || matches!(
            node.kind(),
            "if_statement"
                | "elif_clause"
                | "if_expression"
                | "for_statement"
                | "for_in_statement"
                | "while_statement"
                | "while_expression"
                | "do_statement"
                | "enhanced_for_statement"
                | "loop_expression"
                | "for_expression"
                | "catch_clause"
                | "except_clause"
                | "ternary_expression"
                | "conditional_expression"
                | "for_in_clause"
                | "if_clause"
        )
}

/// Rust's `let … else` diverges on its else branch, so it is a decision like
/// `if let`, even though the grammar files it under `let_declaration`.
fn is_let_else(node: Node<'_>) -> bool {
    node.kind() == "let_declaration" && node.child_by_field_name("alternative").is_some()
}

fn is_arm_decision(kind: &str) -> bool {
    matches!(
        kind,
        "switch_case"
            | "expression_case"
            | "type_case"
            | "communication_case"
            | "case_clause"
            | "match_arm"
            // Java: one per label in both `case x:` and `case x ->` forms.
            // Counting the enclosing `switch_rule` instead would miss the
            // colon form and double-count the arrow form.
            | "switch_label"
    )
}

fn is_non_default_arm(node: Node<'_>, source: &[u8]) -> bool {
    let value = text(node, source).unwrap_or_default().trim_start();
    !value.starts_with("default") && !value.starts_with("_ =>") && !is_wildcard_case(value)
}

/// Python's `case _:` is the default arm of a `match` and adds nothing, like
/// `default:` and `_ =>`. A guarded `case _ if x:` still branches.
fn is_wildcard_case(value: &str) -> bool {
    value
        .strip_prefix("case")
        .map(str::trim_start)
        .and_then(|rest| rest.strip_prefix('_'))
        .is_some_and(|after| after.trim_start().starts_with(':'))
}

/// `let_chain` is Rust's `if let … && …`: each node holds one `&&`, so it
/// counts like the `&&` of a plain condition.
fn is_boolean_decision(node: Node<'_>, source: &[u8]) -> bool {
    matches!(
        node.kind(),
        "binary_expression" | "boolean_operator" | "let_chain"
    ) && has_boolean_operator(node, source)
}

fn has_boolean_operator(node: Node<'_>, source: &[u8]) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor).any(|child| {
        matches!(child.kind(), "&&" | "||" | "and" | "or")
            || text(child, source).is_some_and(|value| matches!(value, "&&" | "||" | "and" | "or"))
    })
}

fn text<'a>(node: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    node.utf8_text(source).ok()
}

fn normalize(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn analyze(extension: &str, source: &str) -> Vec<CodeUnit> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("sample.{extension}"));
        fs::write(&path, source).unwrap();
        let language = Language::from_path(&path).unwrap();
        analyze_file(&path, language).unwrap().units
    }

    #[test]
    fn javascript_named_arrow_and_function() {
        let units = analyze(
            "js",
            "function f(a) { if (a && a.ok) return 1; return 0; }\nconst g = (x) => x ? 1 : 0;",
        );
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].symbol, "f");
        assert_eq!(units[0].complexity, 3.0);
        assert_eq!(units[1].symbol, "g");
        assert_eq!(units[1].complexity, 2.0);
    }

    #[test]
    fn typescript_tsx_and_async_functions_parse() {
        let units = analyze(
            "tsx",
            "async function load<T>(x: T) { return x ? <div /> : null; }\nconst View = (p: {ok: boolean}) => p.ok && <span />;",
        );
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].symbol, "load");
        assert_eq!(units[0].complexity, 2.0);
        assert_eq!(units[1].complexity, 2.0);
    }

    #[test]
    fn assigned_callable_values_get_names() {
        let python = analyze("py", "handler = lambda x: 1 if x else 0\n");
        assert_eq!(python[0].symbol, "handler");
        assert_eq!(python[0].complexity, 2.0);

        let rust = analyze(
            "rs",
            "fn main() { let handler = |x| if x { 1 } else { 0 }; }",
        );
        assert!(rust.iter().any(|unit| unit.symbol == "main::handler"));

        let java = analyze(
            "java",
            "class A { void run() { Runnable handler = () -> { if (true) {} }; } }",
        );
        assert!(java.iter().any(|unit| unit.symbol == "A.run().handler"));

        let go = analyze(
            "go",
            "package p\nfunc main() { handler := func(x bool) { if x {} }; handler(true) }",
        );
        assert!(go.iter().any(|unit| unit.symbol == "main.handler"));
    }

    #[test]
    fn python_nested_units_do_not_inflate_parent() {
        let units = analyze(
            "py",
            "async def outer(x):\n    def inner(y):\n        return 1 if y else 0\n    if x:\n        return inner(x)\n",
        );
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].complexity, 2.0);
        assert_eq!(units[1].symbol, "outer.inner");
        assert_eq!(units[1].complexity, 2.0);
    }

    #[test]
    fn rust_impl_is_qualified() {
        let units = analyze(
            "rs",
            "impl Thing { fn run(&self, x: bool) { if x { loop {} } } }",
        );
        assert_eq!(units[0].symbol, "Thing::run");
        assert_eq!(units[0].complexity, 3.0);
    }

    #[test]
    fn rust_match_ignores_default_arm() {
        let units = analyze(
            "rs",
            "fn choose(x: u8) -> u8 { match x { 0 => 1, _ => 2 } }",
        );
        assert_eq!(units[0].complexity, 2.0);
    }

    #[test]
    fn rust_test_code_is_not_scored() {
        let units = analyze(
            "rs",
            concat!(
                "fn keep(x: bool) { if x { loop {} } }\n",
                "#[cfg(test)]\n",
                "mod tests {\n",
                "    fn helper(x: bool) { if x { loop {} } }\n",
                "    #[test]\n",
                "    fn case() {}\n",
                "}\n",
                "#[cfg(test)]\n",
                "fn gated() {}\n",
                "#[tokio::test]\n",
                "async fn async_case() {}\n",
                "#[allow(dead_code)]\n",
                "fn attributed(x: bool) { if x { loop {} } }\n",
            ),
        );
        let symbols: Vec<_> = units.iter().map(|unit| unit.symbol.as_str()).collect();
        // A non-test attribute must not suppress the item it annotates.
        assert_eq!(symbols, ["keep", "attributed"]);
    }

    #[test]
    fn go_receiver_is_qualified() {
        let units = analyze(
            "go",
            "package p\ntype T struct{}\nfunc (t T) Run(x bool) { if x { return } }",
        );
        assert!(units[0].symbol.ends_with(".Run"));
        assert_eq!(units[0].complexity, 2.0);
    }

    #[test]
    fn java_overload_has_parameter_types() {
        let units = analyze(
            "java",
            "class A { A() {} int f(String x) { return x == null ? 0 : 1; } }",
        );
        assert_eq!(units.len(), 2);
        assert!(units.iter().any(|unit| unit.symbol == "A.f(String)"));
    }

    #[test]
    fn java_switch_forms_score_the_same() {
        let colon = analyze(
            "java",
            "class A { int f(int x) { switch (x) { case 1: return 1; case 2: return 2; case 3: return 3; default: return 0; } } }",
        );
        let arrow = analyze(
            "java",
            "class A { int f(int x) { return switch (x) { case 1 -> 1; case 2 -> 2; case 3 -> 3; default -> 0; }; } }",
        );
        assert_eq!(
            colon[0].complexity, 4.0,
            "colon-form switch was not counted"
        );
        assert_eq!(
            arrow[0].complexity, 4.0,
            "arrow-form switch was double counted"
        );
    }

    #[test]
    fn terraform_files_are_not_candidates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.tf");
        fs::write(&path, "resource \"aws_x\" \"main\" { count = 1 }\n").unwrap();
        assert!(Language::from_path(&path).is_none());
    }

    fn symbols(units: &[CodeUnit]) -> Vec<&str> {
        units.iter().map(|unit| unit.symbol.as_str()).collect()
    }

    #[test]
    fn typescript_abstract_and_assigned_classes_qualify_methods() {
        let units = analyze(
            "ts",
            concat!(
                "export abstract class Base { run(x: boolean) { return x ? 1 : 0; } }\n",
                "const Widget = class { go(y: number) { return y > 1; } };\n",
                "const Alias = class Inner { stop() { return 0; } };\n",
            ),
        );
        // A class expression takes the variable's name unless it names itself.
        assert_eq!(symbols(&units), ["Base.run", "Widget.go", "Inner.stop"]);
    }

    #[test]
    fn rust_inline_modules_qualify_functions() {
        let units = analyze(
            "rs",
            "mod alpha { pub fn run() {} }\nmod beta { pub fn run() {} }\nfn top() {}",
        );
        assert_eq!(symbols(&units), ["alpha::run", "beta::run", "top"]);
    }

    #[test]
    fn python_match_ignores_the_wildcard_arm() {
        let units = analyze(
            "py",
            concat!(
                "def pick(x):\n",
                "    match x:\n",
                "        case 1:\n",
                "            return 1\n",
                "        case 2:\n",
                "            return 2\n",
                "        case _ if x > 9:\n",
                "            return 9\n",
                "        case _:\n",
                "            return 0\n",
            ),
        );
        // Two literal arms, the guarded wildcard arm, and its guard count.
        // The bare `case _:` is the default and adds nothing.
        assert_eq!(units[0].complexity, 5.0);
    }

    #[test]
    fn rust_let_else_and_let_chains_count() {
        let units = analyze(
            "rs",
            concat!(
                "fn first(x: Option<u8>) -> u8 { let Some(v) = x else { return 0 }; v }\n",
                "fn second(x: Option<u8>) -> u8 { if let Some(v) = x && v > 1 { v } else { 0 } }\n",
                "fn third(x: Option<u8>) -> u8 { let v = x.unwrap_or(0); v }\n",
            ),
        );
        assert_eq!(units[0].complexity, 2.0, "let-else diverges like if-let");
        assert_eq!(
            units[1].complexity, 3.0,
            "the && of a let chain is a decision"
        );
        assert_eq!(units[2].complexity, 1.0, "a plain let is not");
    }
}
