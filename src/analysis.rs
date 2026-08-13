use crate::model::{CodeUnit, Diagnostic, Language};
use anyhow::{Context, Result, anyhow};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use rayon::prelude::*;
use std::collections::HashSet;
use std::path::Path;
use tree_sitter::{Node, Parser};

#[derive(Debug, Default)]
pub struct Analysis {
    pub units: Vec<CodeUnit>,
    pub candidate_files: usize,
    pub parsed_files: usize,
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

pub fn analyze_tree(root: &Path, languages: &[Language], excludes: &[String]) -> Result<Analysis> {
    if !root.exists() {
        return Err(anyhow!("analysis path does not exist: {}", root.display()));
    }
    if !root.is_dir() {
        return Err(anyhow!(
            "analysis path is not a directory: {}",
            root.display()
        ));
    }

    let exclude_set = build_globs(excludes)?;
    let enabled: HashSet<_> = languages.iter().copied().collect();
    let paths: Vec<_> = ignore::WalkBuilder::new(root)
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
        .collect();

    let results: Vec<_> = paths
        .par_iter()
        .map(|(path, language)| analyze_file(path, *language))
        .collect();

    let mut analysis = Analysis {
        candidate_files: paths.len(),
        ..Analysis::default()
    };
    for result in results {
        match result {
            Ok(file) => {
                if let Some(diagnostic) = file.diagnostic {
                    analysis.diagnostics.push(diagnostic);
                } else {
                    analysis.parsed_files += 1;
                    analysis.units.extend(file.units);
                }
            }
            Err(error) => analysis.diagnostics.push(Diagnostic {
                kind: "parse".into(),
                message: error.to_string(),
                file: None,
            }),
        }
    }
    analysis.units.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(a.start_line.cmp(&b.start_line))
            .then(a.symbol.cmp(&b.symbol))
    });
    analysis
        .diagnostics
        .sort_by(|a, b| a.file.cmp(&b.file).then(a.message.cmp(&b.message)));
    Ok(analysis)
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

fn grammar(language: Language, path: &Path) -> tree_sitter::Language {
    match language {
        Language::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
        Language::TypeScript if path.extension().is_some_and(|ext| ext == "tsx") => {
            tree_sitter_typescript::LANGUAGE_TSX.into()
        }
        Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        Language::Python => tree_sitter_python::LANGUAGE.into(),
        Language::Go => tree_sitter_go::LANGUAGE.into(),
        Language::Rust => tree_sitter_rust::LANGUAGE.into(),
        Language::Java => tree_sitter_java::LANGUAGE.into(),
        Language::Terraform => tree_sitter_hcl::LANGUAGE.into(),
    }
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
            complexity: complexity(spec.node, language, bytes) as f64,
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
    if language == Language::Terraform {
        if node.kind() == "block" && !has_ancestor_kind(node, "block") {
            if let Some(name) = terraform_block_name(node, source) {
                output.push(UnitSpec { node, name });
            }
        }
    } else if let Some((unit_node, name)) = named_unit(node, language, source) {
        output.push(UnitSpec {
            node: unit_node,
            name,
        });
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_units(child, language, source, output);
    }
}

fn named_unit<'tree>(
    node: Node<'tree>,
    language: Language,
    source: &[u8],
) -> Option<(Node<'tree>, String)> {
    let declaration = match language {
        Language::JavaScript | Language::TypeScript => matches!(
            node.kind(),
            "function_declaration" | "generator_function_declaration" | "method_definition"
        ),
        Language::Python => node.kind() == "function_definition",
        Language::Go => matches!(node.kind(), "function_declaration" | "method_declaration"),
        Language::Rust => node.kind() == "function_item",
        Language::Java => matches!(
            node.kind(),
            "method_declaration" | "constructor_declaration"
        ),
        Language::Terraform => false,
    };
    if declaration {
        if language == Language::Java && node.child_by_field_name("body").is_none() {
            return None;
        }
        let name = node
            .child_by_field_name("name")
            .and_then(|name| text(name, source))?
            .to_string();
        return Some((node, java_signature(node, language, name, source)));
    }

    if let Some((value, name)) = assigned_callable(node, language, source) {
        return Some((value, name));
    }
    None
}

fn assigned_callable<'tree>(
    node: Node<'tree>,
    language: Language,
    source: &[u8],
) -> Option<(Node<'tree>, String)> {
    let (name_field, value_field, callable_kinds): (&str, &str, &[&str]) = match language {
        Language::JavaScript | Language::TypeScript => match node.kind() {
            "variable_declarator" | "public_field_definition" => (
                "name",
                "value",
                &[
                    "arrow_function",
                    "function_expression",
                    "generator_function",
                ],
            ),
            "pair" => (
                "key",
                "value",
                &[
                    "arrow_function",
                    "function_expression",
                    "generator_function",
                ],
            ),
            "assignment_expression" => (
                "left",
                "right",
                &[
                    "arrow_function",
                    "function_expression",
                    "generator_function",
                ],
            ),
            _ => return None,
        },
        Language::Python if node.kind() == "assignment" => ("left", "right", &["lambda"]),
        Language::Go if node.kind() == "short_var_declaration" => {
            ("left", "right", &["func_literal"])
        }
        Language::Go if node.kind() == "var_spec" => ("name", "value", &["func_literal"]),
        Language::Rust if node.kind() == "let_declaration" => {
            ("pattern", "value", &["closure_expression"])
        }
        Language::Java if node.kind() == "variable_declarator" => {
            ("name", "value", &["lambda_expression"])
        }
        _ => return None,
    };
    let name_node = unwrap_singleton(node.child_by_field_name(name_field)?);
    let value = unwrap_singleton(node.child_by_field_name(value_field)?);
    callable_kinds.contains(&value.kind()).then(|| {
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
    let mut types = Vec::new();
    let mut cursor = parameters.walk();
    for parameter in parameters.named_children(&mut cursor) {
        if let Some(kind) = parameter.child_by_field_name("type") {
            types.push(normalize(text(kind, source).unwrap_or_default()));
        }
    }
    format!("{name}({})", types.join(","))
}

fn qualify_symbol(node: Node<'_>, language: Language, name: &str, source: &[u8]) -> String {
    let mut parts = Vec::new();
    let mut parent = node.parent();
    while let Some(ancestor) = parent {
        let kind = ancestor.kind();
        let is_container = matches!(
            kind,
            "class_declaration"
                | "class_definition"
                | "interface_declaration"
                | "enum_declaration"
                | "record_declaration"
                | "trait_item"
                | "impl_item"
                | "function_declaration"
                | "function_definition"
                | "function_item"
                | "method_definition"
                | "method_declaration"
                | "constructor_declaration"
        );
        if is_container {
            if let Some(value) = container_name(ancestor, language, source) {
                parts.push(value);
            }
        }
        parent = ancestor.parent();
    }
    parts.reverse();

    if language == Language::Go && node.kind() == "method_declaration" {
        if let Some(receiver) = node.child_by_field_name("receiver") {
            let parameter = receiver.named_child(0).unwrap_or(receiver);
            if let Some(receiver_type) = parameter.child_by_field_name("type") {
                parts.push(normalize(text(receiver_type, source).unwrap_or_default()));
            }
        }
    }
    parts.push(name.to_string());
    let separator = if language == Language::Rust {
        "::"
    } else {
        "."
    };
    parts.join(separator)
}

fn container_name(node: Node<'_>, language: Language, source: &[u8]) -> Option<String> {
    if node.kind() == "impl_item" {
        let header = text(node, source)?.split('{').next()?.trim();
        return header
            .strip_prefix("impl")
            .map(str::trim)
            .and_then(|value| value.split_whitespace().last())
            .map(str::to_string);
    }
    let name = node.child_by_field_name("name")?;
    let value = text(name, source)?.to_string();
    if language == Language::Java
        && matches!(
            node.kind(),
            "method_declaration" | "constructor_declaration"
        )
    {
        return Some(java_signature(node, language, value, source));
    }
    (language != Language::Java || !value.is_empty()).then_some(value)
}

fn complexity(root: Node<'_>, language: Language, source: &[u8]) -> usize {
    let mut count = 1;
    count_decisions(root, root, language, source, &mut count);
    count
}

fn count_decisions(
    node: Node<'_>,
    root: Node<'_>,
    language: Language,
    source: &[u8],
    count: &mut usize,
) {
    if node != root && is_callable(node.kind()) {
        return;
    }
    if is_decision(node, language, source) {
        *count += 1;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        count_decisions(child, root, language, source, count);
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

fn is_decision(node: Node<'_>, language: Language, source: &[u8]) -> bool {
    let kind = node.kind();
    if language == Language::Terraform {
        let body = text(node, source).unwrap_or_default();
        return matches!(kind, "conditional" | "for_expr" | "for_cond")
            || (kind == "binary_operation" && has_boolean_operator(node, source))
            || (kind == "attribute"
                && first_named_text(node, source)
                    .is_some_and(|name| matches!(name, "count" | "for_each")))
            || (kind == "block"
                && [
                    "dynamic",
                    "validation",
                    "precondition",
                    "postcondition",
                    "assert",
                ]
                .iter()
                .any(|prefix| body.trim_start().starts_with(prefix)));
    }

    let common = matches!(
        kind,
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
            | "switch_case"
            | "expression_case"
            | "type_case"
            | "communication_case"
            | "case_clause"
            | "match_arm"
            | "switch_rule"
    );
    if common {
        if matches!(
            kind,
            "switch_case"
                | "expression_case"
                | "type_case"
                | "communication_case"
                | "case_clause"
                | "match_arm"
                | "switch_rule"
        ) {
            let value = text(node, source).unwrap_or_default().trim_start();
            return !value.starts_with("default") && !value.starts_with("_ =>");
        }
        return true;
    }
    matches!(kind, "binary_expression" | "boolean_operator") && has_boolean_operator(node, source)
}

fn has_boolean_operator(node: Node<'_>, source: &[u8]) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor).any(|child| {
        matches!(child.kind(), "&&" | "||" | "and" | "or")
            || text(child, source).is_some_and(|value| matches!(value, "&&" | "||" | "and" | "or"))
    })
}

fn terraform_block_name(node: Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let mut parts = node
        .named_children(&mut cursor)
        .filter_map(|child| text(child, source))
        .take_while(|value| !value.trim_start().starts_with('{'))
        .map(|value| value.trim().trim_matches('"').to_string());
    let kind = parts.next()?;
    if ![
        "resource",
        "data",
        "module",
        "variable",
        "output",
        "locals",
        "provider",
        "terraform",
        "check",
        "import",
        "moved",
    ]
    .contains(&kind.as_str())
    {
        return None;
    }
    Some(
        std::iter::once(kind)
            .chain(parts)
            .collect::<Vec<_>>()
            .join("."),
    )
}

fn first_named_text<'a>(node: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .next()
        .and_then(|child| text(child, source))
}

fn has_ancestor_kind(node: Node<'_>, kind: &str) -> bool {
    let mut parent = node.parent();
    while let Some(ancestor) = parent {
        if ancestor.kind() == kind {
            return true;
        }
        parent = ancestor.parent();
    }
    false
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
    fn terraform_reports_top_level_blocks() {
        let units = analyze(
            "tf",
            "resource \"aws_x\" \"main\" { count = var.on ? 1 : 0 }\nmodule \"child\" { source = \"./child\" }",
        );
        assert_eq!(units.len(), 2);
        assert!(units[0].symbol.starts_with("resource"));
        assert_eq!(units[0].complexity, 3.0);
    }
}
