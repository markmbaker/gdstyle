//! Full-language syntax validation using `tree-sitter-gdscript`.
//!
//! gdstyle's native parser deliberately extracts only the declarations and
//! block structure needed by lint rules. This module complements it with the
//! complete grammar used by editors and ast-grep, reporting recoverable
//! Tree-sitter `ERROR` and `MISSING` nodes as ordinary lint diagnostics.

use crate::diagnostic::Diagnostic;
use crate::token::Span;
use tree_sitter::{Node, Tree};

const RULE_NAME: &str = "syntax/parse-error";

fn parser() -> tree_sitter::Parser {
    let mut parser = tree_sitter::Parser::new();
    let language = tree_sitter_gdscript::LANGUAGE.into();
    parser
        .set_language(&language)
        .expect("tree-sitter-gdscript language version is incompatible");
    parser
}

/// A parsed GDScript syntax tree paired with the source bytes it describes.
///
/// Keeping these together gives declaration analysis, diagnostics, and future
/// syntax-aware fixes one authoritative parse instead of making each consumer
/// invoke Tree-sitter independently.
pub struct SyntaxDocument<'source> {
    source: &'source str,
    tree: Tree,
}

impl<'source> SyntaxDocument<'source> {
    /// Parse a source string. `None` is only possible if Tree-sitter parsing is
    /// cancelled; gdstyle does not currently configure a cancellation flag.
    pub fn parse(source: &'source str) -> Option<Self> {
        parser()
            .parse(source, None)
            .map(|tree| Self { source, tree })
    }

    /// Return the original source associated with the syntax tree.
    pub fn source(&self) -> &'source str {
        self.source
    }

    /// Return the root node of the parsed syntax tree.
    pub fn root_node(&self) -> Node<'_> {
        self.tree.root_node()
    }

    /// Return whether the document contains no recovered syntax errors or
    /// missing tokens.
    pub fn is_valid(&self) -> bool {
        !self.root_node().has_error()
    }

    /// Convert recovered Tree-sitter errors into gdstyle diagnostics.
    pub fn diagnostics(&self, file_path: &str) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        collect_errors(self.root_node(), self.source, file_path, &mut diagnostics);
        diagnostics
    }
}

/// Return whether `source` is accepted by the pinned Godot 4.7 grammar.
///
/// This is intended for safety checks where callers only need a yes/no result,
/// such as ensuring a formatter transformation preserves valid syntax.
pub fn is_valid(source: &str) -> bool {
    SyntaxDocument::parse(source).is_some_and(|document| document.is_valid())
}

/// Parse `source` with the pinned Godot 4.7 grammar and return all syntax
/// diagnostics. Tree-sitter recovers after malformed input, so callers can
/// still run the remaining lint rules on partially written files.
pub fn parse_diagnostics(source: &str, file_path: &str) -> Vec<Diagnostic> {
    let Some(document) = SyntaxDocument::parse(source) else {
        return vec![Diagnostic::error(
            RULE_NAME,
            "GDScript parsing was cancelled".to_string(),
            Span::new(1, 1, 0, 0),
            file_path,
        )];
    };

    document.diagnostics(file_path)
}

fn collect_errors(
    node: Node<'_>,
    source: &str,
    file_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if node.is_error() {
        diagnostics.push(diagnostic_for_node(
            node,
            source,
            "could not parse GDScript syntax".to_string(),
            file_path,
        ));
        // ERROR nodes can contain recovery artifacts. Reporting the outermost
        // error produces a stable, useful location without a cascade.
        return;
    }

    if node.is_missing() {
        diagnostics.push(diagnostic_for_node(
            node,
            source,
            format!("missing {}", node.kind()),
            file_path,
        ));
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_errors(child, source, file_path, diagnostics);
    }
}

fn diagnostic_for_node(
    node: Node<'_>,
    source: &str,
    message: String,
    file_path: &str,
) -> Diagnostic {
    let point = node.start_position();
    let character_column = character_column(source, node.start_byte());
    Diagnostic::error(
        RULE_NAME,
        message,
        Span::new(
            point.row + 1,
            character_column,
            node.start_byte(),
            node.end_byte().saturating_sub(node.start_byte()),
        ),
        file_path,
    )
}

fn character_column(source: &str, byte_offset: usize) -> usize {
    let line_start = source[..byte_offset]
        .rfind('\n')
        .map_or(0, |newline| newline + 1);
    source[line_start..byte_offset].chars().count() + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_modern_godot_4_syntax() {
        let source = r#"@export var values: Dictionary[String, Array] = {}

func collect(items: Array[String]) -> void:
	for item: String in items:
		values[item] = []
"#;

        let document = SyntaxDocument::parse(source).expect("parse should not be cancelled");
        assert_eq!(document.source(), source);
        assert_eq!(document.root_node().kind(), "source");
        assert!(document.is_valid());
        assert!(is_valid(source));
        assert!(parse_diagnostics(source, "modern.gd").is_empty());
    }

    #[test]
    fn reports_invalid_syntax_with_one_based_location() {
        let source = "func broken( -> void:\n\tpass\n";
        let diagnostics = parse_diagnostics(source, "broken.gd");

        assert!(!is_valid(source));
        assert!(!diagnostics.is_empty());
        assert!(diagnostics
            .iter()
            .all(|diagnostic| diagnostic.rule == RULE_NAME));
        assert!(diagnostics
            .iter()
            .all(|diagnostic| diagnostic.span.line >= 1 && diagnostic.span.column >= 1));
    }

    #[test]
    fn accepts_not_comparison_precedence_shape() {
        let source = "var comparison := not value == 0\n";
        assert!(parse_diagnostics(source, "comparison.gd").is_empty());
    }

    #[test]
    fn reports_character_column_after_unicode() {
        assert_eq!(character_column("é x", "é".len()), 2);
        assert_eq!(character_column("first\né x", "first\né".len()), 2);
    }
}
