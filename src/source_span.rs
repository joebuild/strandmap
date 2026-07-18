use std::path::Path;

use tree_sitter::{Language, Node, Parser, Tree};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start_line: u32,
    pub end_line: u32,
}

pub fn supports_dynamic(path: &str) -> bool {
    matches!(
        Path::new(path).extension().and_then(|value| value.to_str()),
        Some("rs" | "js" | "jsx" | "mjs" | "cjs" | "py" | "sh" | "bash" | "zsh" | "lean" | "tla")
    )
}

enum Engine {
    Tree(Tree, fn(&str) -> bool),
    Declaration,
    Structural,
}

pub struct Resolver<'a> {
    text: &'a str,
    engine: Engine,
}

impl<'a> Resolver<'a> {
    pub fn new(path: &str, text: &'a str) -> Result<Self, String> {
        let extension = Path::new(path)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        let engine = match extension {
            "rs" => tree_engine(text, tree_sitter_rust::LANGUAGE.into(), rust_node)?,
            "js" | "jsx" | "mjs" | "cjs" => tree_engine(
                text,
                tree_sitter_javascript::LANGUAGE.into(),
                javascript_node,
            )?,
            "py" => tree_engine(text, tree_sitter_python::LANGUAGE.into(), python_node)?,
            "sh" | "bash" | "zsh" => {
                tree_engine(text, tree_sitter_bash::LANGUAGE.into(), bash_node)?
            }
            "lean" | "tla" => Engine::Declaration,
            _ => Engine::Structural,
        };
        Ok(Self { text, engine })
    }

    pub fn resolve(&self, marker_line: usize) -> Result<Span, String> {
        match &self.engine {
            Engine::Tree(tree, accepted) => resolve_tree(self.text, marker_line, tree, *accepted)
                .or_else(|tree_error| {
                    resolve_structural(self.text, marker_line).map_err(|structural_error| {
                        format!("{tree_error}; structural fallback failed: {structural_error}")
                    })
                }),
            Engine::Declaration => resolve_declaration(self.text, marker_line),
            Engine::Structural => resolve_structural(self.text, marker_line),
        }
    }
}

pub fn resolve(path: &str, text: &str, marker_line: usize) -> Result<Span, String> {
    Resolver::new(path, text)?.resolve(marker_line)
}

/// Return the smallest coherent source unit containing `line`.
///
/// Search uses this independently of anchor watch modes: `watch=file` controls
/// change detection, while context should still prefer a local declaration when
/// the search evidence is local.
pub fn enclosing(path: &str, text: &str, line: usize) -> Span {
    let line_count = text.lines().count().max(1);
    let line = line.clamp(1, line_count);
    enclosing_tree(path, text, line)
        .or_else(|| enclosing_section(text, line))
        .unwrap_or(Span {
            start_line: 1,
            end_line: u32::try_from(line_count).unwrap_or(u32::MAX),
        })
}

/// Resolve Rust test-only declarations from their attributes.
pub fn rust_test_ranges(path: &str, text: &str) -> Vec<Span> {
    if Path::new(path).extension().and_then(|value| value.to_str()) != Some("rs")
        || !text.contains("#[")
        || !text.contains("test")
    {
        return Vec::new();
    }
    let Ok(Engine::Tree(tree, _)) = tree_engine(text, tree_sitter_rust::LANGUAGE.into(), rust_node)
    else {
        return Vec::new();
    };
    let mut ranges = Vec::new();
    collect_rust_test_ranges(tree.root_node(), text, &mut ranges);
    ranges.sort_by_key(|span| (span.start_line, std::cmp::Reverse(span.end_line)));
    let mut merged: Vec<Span> = Vec::new();
    for span in ranges {
        if let Some(previous) = merged.last_mut() {
            if span.start_line >= previous.start_line && span.end_line <= previous.end_line {
                continue;
            }
            if span.start_line <= previous.end_line.saturating_add(1) {
                previous.end_line = previous.end_line.max(span.end_line);
                continue;
            }
        }
        merged.push(span);
    }
    merged
}

fn collect_rust_test_ranges(node: Node<'_>, text: &str, ranges: &mut Vec<Span>) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "attribute_item"
            && text
                .get(child.byte_range())
                .is_some_and(is_rust_test_attribute)
        {
            if let Some(item) = next_rust_item(child) {
                let mut start = child.start_position().row + 1;
                let mut previous = child.prev_named_sibling();
                while let Some(attribute) = previous.filter(|node| node.kind() == "attribute_item")
                {
                    start = attribute.start_position().row + 1;
                    previous = attribute.prev_named_sibling();
                }
                if let (Ok(start_line), Ok(end_line)) = (to_u32(start), node_end_line(item)) {
                    ranges.push(Span {
                        start_line,
                        end_line,
                    });
                }
            }
        }
        collect_rust_test_ranges(child, text, ranges);
    }
}

fn next_rust_item(attribute: Node<'_>) -> Option<Node<'_>> {
    let mut node = attribute.next_named_sibling()?;
    while node.kind() == "attribute_item" || is_tree_comment(node.kind()) {
        node = node.next_named_sibling()?;
    }
    rust_test_item(node.kind()).then_some(node)
}

fn is_tree_comment(kind: &str) -> bool {
    matches!(kind, "line_comment" | "block_comment")
}

fn rust_test_item(kind: &str) -> bool {
    rust_context_node(kind)
        || matches!(
            kind,
            "impl_item" | "use_declaration" | "extern_crate_declaration"
        )
}

fn is_rust_test_attribute(line: &str) -> bool {
    let compact: String = line
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    let Some(attribute) = compact
        .strip_prefix("#[")
        .and_then(|value| value.strip_suffix(']'))
    else {
        return false;
    };
    if let Some(condition) = attribute
        .strip_prefix("cfg(")
        .and_then(|value| value.strip_suffix(')'))
    {
        return normalize_attribute_tokens(condition)
            .iter()
            .any(|token| token == "test");
    }
    let name = attribute.split('(').next().unwrap_or(attribute);
    name == "test"
        || name.ends_with("::test")
        || matches!(name, "rstest" | "proptest" | "test_case")
}

fn normalize_attribute_tokens(value: &str) -> Vec<String> {
    value
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter(|token| !token.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn enclosing_tree(path: &str, text: &str, line: usize) -> Option<Span> {
    let extension = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let (language, accepted): (Language, fn(&str) -> bool) = match extension {
        "rs" => (tree_sitter_rust::LANGUAGE.into(), rust_context_node),
        "js" | "jsx" | "mjs" | "cjs" => (
            tree_sitter_javascript::LANGUAGE.into(),
            javascript_context_node,
        ),
        "py" => (tree_sitter_python::LANGUAGE.into(), python_context_node),
        "sh" | "bash" | "zsh" => (tree_sitter_bash::LANGUAGE.into(), bash_node),
        _ => return None,
    };
    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;
    let tree = parser.parse(text, None)?;
    let start = line_start_offset(text, line)?;
    let end = line_end_offset(text, line)?.saturating_sub(1).max(start);
    let mut node = tree.root_node().descendant_for_byte_range(start, end)?;
    loop {
        if accepted(node.kind()) {
            return Some(Span {
                start_line: u32::try_from(node.start_position().row + 1).ok()?,
                end_line: node_end_line(node).ok()?,
            });
        }
        node = node.parent()?;
    }
}

fn rust_context_node(kind: &str) -> bool {
    matches!(
        kind,
        "function_item"
            | "struct_item"
            | "enum_item"
            | "union_item"
            | "type_item"
            | "const_item"
            | "static_item"
            | "trait_item"
            | "macro_definition"
            | "macro_invocation"
            | "mod_item"
    )
}

fn javascript_context_node(kind: &str) -> bool {
    matches!(
        kind,
        "export_statement"
            | "function_declaration"
            | "generator_function_declaration"
            | "method_definition"
            | "variable_declaration"
            | "lexical_declaration"
            | "expression_statement"
            | "arrow_function"
            | "function_expression"
            | "generator_function"
            | "class_declaration"
    )
}

fn python_context_node(kind: &str) -> bool {
    matches!(
        kind,
        "decorated_definition"
            | "function_definition"
            | "class_definition"
            | "expression_statement"
            | "assignment"
    )
}

fn enclosing_section(text: &str, line: usize) -> Option<Span> {
    let lines: Vec<_> = text.lines().collect();
    if lines.is_empty() {
        return None;
    }
    const MAX_SECTION_LINES: usize = 80;
    let selected = line - 1;
    let mut start = selected;
    while start > 0
        && selected - start < MAX_SECTION_LINES / 2
        && !lines[start - 1].trim().is_empty()
        && !is_heading(lines[start].trim())
    {
        start -= 1;
    }
    let mut end = selected;
    while end + 1 < lines.len()
        && end + 1 - start < MAX_SECTION_LINES
        && !lines[end + 1].trim().is_empty()
        && !is_heading(lines[end + 1].trim())
    {
        end += 1;
    }
    Some(Span {
        start_line: u32::try_from(start + 1).ok()?,
        end_line: u32::try_from(end + 1).ok()?,
    })
}

fn is_heading(line: &str) -> bool {
    line.starts_with('#')
        || line.starts_with("section ")
        || line.starts_with("namespace ")
        || line.starts_with("module ")
}

fn tree_engine(
    text: &str,
    language: Language,
    accepted: fn(&str) -> bool,
) -> Result<Engine, String> {
    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .map_err(|error| format!("failed to load source parser: {error}"))?;
    let tree = parser
        .parse(text, None)
        .ok_or_else(|| "source parser returned no syntax tree".to_string())?;
    Ok(Engine::Tree(tree, accepted))
}

fn resolve_tree(
    text: &str,
    marker_line: usize,
    tree: &Tree,
    accepted: fn(&str) -> bool,
) -> Result<Span, String> {
    let marker_start = line_start_offset(text, marker_line)
        .ok_or_else(|| format!("annotation line {marker_line} is outside the source"))?;
    let marker_end = line_end_offset(text, marker_line)
        .ok_or_else(|| format!("annotation line {marker_line} is outside the source"))?;
    if tree
        .root_node()
        .descendant_for_byte_range(marker_start, marker_end.saturating_sub(1))
        .is_some_and(|node| node.kind().contains("string") || node.kind().contains("template"))
    {
        return Err("annotation is embedded in a string literal".into());
    }
    let mut candidates = Vec::new();
    collect_candidates(tree.root_node(), marker_end, accepted, &mut candidates);
    candidates.sort_by_key(|node| (node.start_byte(), usize::MAX - node.end_byte()));
    let node = candidates
        .into_iter()
        .next()
        .ok_or_else(|| "no attachable syntax node follows the annotation".to_string())?;
    let node_start = node.start_position().row + 1;
    if node_start.saturating_sub(marker_line) > 32 {
        return Err(format!(
            "nearest syntax node starts {} lines after the annotation",
            node_start - marker_line
        ));
    }
    Ok(Span {
        start_line: to_u32(node_start)?,
        end_line: node_end_line(node)?,
    })
}

fn collect_candidates<'tree>(
    node: Node<'tree>,
    marker_end: usize,
    accepted: fn(&str) -> bool,
    output: &mut Vec<Node<'tree>>,
) {
    if node.start_byte() >= marker_end && accepted(node.kind()) {
        output.push(node);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.end_byte() >= marker_end {
            collect_candidates(child, marker_end, accepted, output);
        }
    }
}

fn rust_node(kind: &str) -> bool {
    matches!(
        kind,
        "function_item" | "function_signature_item" | "closure_expression" | "macro_invocation"
    )
}

fn javascript_node(kind: &str) -> bool {
    matches!(
        kind,
        "export_statement"
            | "function_declaration"
            | "generator_function_declaration"
            | "method_definition"
            | "variable_declaration"
            | "lexical_declaration"
            | "expression_statement"
            | "call_expression"
            | "arrow_function"
            | "function_expression"
            | "generator_function"
    )
}

fn python_node(kind: &str) -> bool {
    matches!(
        kind,
        "decorated_definition"
            | "function_definition"
            | "class_definition"
            | "expression_statement"
            | "assignment"
    )
}

fn bash_node(kind: &str) -> bool {
    matches!(
        kind,
        "function_definition"
            | "command"
            | "variable_assignment"
            | "if_statement"
            | "for_statement"
            | "while_statement"
            | "case_statement"
    )
}

fn resolve_declaration(text: &str, marker_line: usize) -> Result<Span, String> {
    let lines: Vec<_> = text.lines().collect();
    if marker_line == 0 || marker_line > lines.len() {
        return Err(format!(
            "annotation line {marker_line} is outside the source"
        ));
    }
    let marker_indent = indentation(lines[marker_line - 1]);
    let start = (marker_line + 1..=lines.len())
        .find(|line| {
            let trimmed = lines[*line - 1].trim();
            !trimmed.is_empty() && !is_comment_only(trimmed)
        })
        .ok_or_else(|| "no declaration follows the annotation".to_string())?;
    let next_anchor = (start + 1..=lines.len()).find(|line| {
        let value = lines[*line - 1];
        indentation(value) <= marker_indent
            && is_comment_only(value.trim())
            && value.contains("@anchor")
    });
    let end = next_anchor.map_or(lines.len(), |line| line.saturating_sub(1));
    let end = (start..=end)
        .rev()
        .find(|line| !lines[*line - 1].trim().is_empty())
        .unwrap_or(start);
    Ok(Span {
        start_line: to_u32(start)?,
        end_line: to_u32(end)?,
    })
}

fn resolve_structural(text: &str, marker_line: usize) -> Result<Span, String> {
    let lines: Vec<_> = text.lines().collect();
    if marker_line == 0 || marker_line > lines.len() {
        return Err(format!(
            "annotation line {marker_line} is outside the source"
        ));
    }
    let start_line = (marker_line + 1..=lines.len())
        .find(|line| {
            let trimmed = lines[*line - 1].trim();
            !trimmed.is_empty() && !is_comment_only(trimmed)
        })
        .ok_or_else(|| "no source construct follows the annotation".to_string())?;
    let start_offset = line_start_offset(text, start_line)
        .ok_or_else(|| "failed to locate source construct".to_string())?;
    let bytes = text.as_bytes();
    let mut stack = Vec::new();
    let mut quote = None;
    let mut escaped = false;
    let mut line_comment = false;
    let mut block_comment = false;
    let mut saw_brace = false;
    let mut index = start_offset;
    while index < bytes.len() {
        let byte = bytes[index];
        let next = bytes.get(index + 1).copied();
        if line_comment {
            if byte == b'\n' {
                line_comment = false;
            }
            index += 1;
            continue;
        }
        if block_comment {
            if byte == b'*' && next == Some(b'/') {
                block_comment = false;
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == active_quote {
                quote = None;
            }
            index += 1;
            continue;
        }
        if byte == b'/' && next == Some(b'/') {
            line_comment = true;
            index += 2;
            continue;
        }
        if byte == b'/' && next == Some(b'*') {
            block_comment = true;
            index += 2;
            continue;
        }
        if matches!(byte, b'\'' | b'"' | b'`') {
            quote = Some(byte);
            index += 1;
            continue;
        }
        match byte {
            b'(' | b'[' | b'{' => {
                stack.push(byte);
                saw_brace |= byte == b'{';
            }
            b')' | b']' | b'}' => {
                let expected = match byte {
                    b')' => b'(',
                    b']' => b'[',
                    _ => b'{',
                };
                if stack.last().copied() == Some(expected) {
                    stack.pop();
                    if byte == b'}' && saw_brace && stack.is_empty() {
                        return Ok(Span {
                            start_line: to_u32(start_line)?,
                            end_line: to_u32(line_for_offset(text, index))?,
                        });
                    }
                }
            }
            b';' if stack.is_empty() => {
                return Ok(Span {
                    start_line: to_u32(start_line)?,
                    end_line: to_u32(line_for_offset(text, index))?,
                });
            }
            _ => {}
        }
        index += 1;
    }
    Err("could not find the end of the following source construct".into())
}

fn line_start_offset(text: &str, line: usize) -> Option<usize> {
    if line == 0 {
        return None;
    }
    if line == 1 {
        return Some(0);
    }
    text.match_indices('\n')
        .nth(line - 2)
        .map(|(index, _)| index + 1)
}

fn line_end_offset(text: &str, line: usize) -> Option<usize> {
    let start = line_start_offset(text, line)?;
    Some(
        text[start..]
            .find('\n')
            .map_or(text.len(), |relative| start + relative + 1),
    )
}

fn line_for_offset(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1
}

fn node_end_line(node: Node<'_>) -> Result<u32, String> {
    let position = node.end_position();
    let line = if position.column == 0 {
        position.row.max(1)
    } else {
        position.row + 1
    };
    to_u32(line)
}

fn indentation(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn is_comment_only(trimmed: &str) -> bool {
    trimmed.starts_with("//")
        || trimmed.starts_with('#')
        || trimmed.starts_with("--")
        || trimmed.starts_with("\\*")
        || trimmed.starts_with("/*")
        || trimmed.starts_with("(*")
}

fn to_u32(value: usize) -> Result<u32, String> {
    u32::try_from(value).map_err(|_| "resolved source line exceeds u32".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_function_span_moves_with_its_annotation() {
        let source = "// unrelated\n// @anchor service.run\npub fn run() {\n    work();\n}\n";
        assert_eq!(
            resolve("src/lib.rs", source, 2).unwrap(),
            Span {
                start_line: 3,
                end_line: 5
            }
        );
    }

    #[test]
    fn javascript_nested_arrow_uses_the_following_callback() {
        let source = "const values = input\n  // @anchor values.keep\n  .filter((value) => {\n    return value.keep;\n  });\n";
        assert_eq!(
            resolve("src/index.js", source, 2).unwrap(),
            Span {
                start_line: 3,
                end_line: 5
            }
        );
    }

    #[test]
    fn declarations_end_at_the_next_anchor() {
        let source = "-- @anchor proof.one\ntheorem one : True := by\n  trivial\n\n-- @anchor proof.two\ntheorem two : True := by\n  trivial\n";
        assert_eq!(
            resolve("Proof.lean", source, 1).unwrap(),
            Span {
                start_line: 2,
                end_line: 3
            }
        );
    }

    #[test]
    fn rust_test_ranges_cover_modules_and_standalone_tests() {
        let source = r#"pub fn production() {}

#[cfg(test)]
mod tests {
    #[test]
    fn unit_test() {}
}

#[tokio::test]
async fn async_test() {}
"#;
        assert_eq!(
            rust_test_ranges("src/lib.rs", source),
            [
                Span {
                    start_line: 3,
                    end_line: 7,
                },
                Span {
                    start_line: 9,
                    end_line: 10,
                },
            ]
        );
    }

    #[test]
    fn rust_test_ranges_include_the_complete_attribute_block() {
        let source = r#"#[allow(clippy::unwrap_used)]
#[tokio::test(
    flavor = "multi_thread"
)]
async fn attributed_test() {}
"#;
        assert_eq!(
            rust_test_ranges("src/lib.rs", source),
            [Span {
                start_line: 1,
                end_line: 5,
            }]
        );
    }
}
