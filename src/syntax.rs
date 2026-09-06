//! Full-language syntax validation using `tree-sitter-gdscript`.
//!
//! gdstyle's native parser deliberately extracts only the declarations and
//! block structure needed by lint rules. This module complements it with the
//! complete grammar used by editors and ast-grep, reporting recoverable
//! Tree-sitter `ERROR` and `MISSING` nodes as ordinary lint diagnostics.

use crate::ast::{AnnotationInfo, ClassMember, EnumMember, Parameter};
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

    /// Project the concrete syntax tree into gdstyle's declaration model.
    ///
    /// The projection intentionally contains declarations, annotations, and
    /// comments rather than expression trees. Token-based formatting and rules
    /// continue to use the native lexer while declaration consumers migrate to
    /// this grammar-backed representation.
    pub fn class_members(&self) -> Vec<ClassMember> {
        lower_members(self.root_node(), self.source)
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
    let line_start = line_start_offset(source, byte_offset);
    source[line_start..byte_offset].chars().count() + 1
}

fn line_start_offset(source: &str, byte_offset: usize) -> usize {
    source[..byte_offset]
        .rfind('\n')
        .map_or(0, |newline| newline + 1)
}

fn lower_members(container: Node<'_>, source: &str) -> Vec<ClassMember> {
    let mut members = Vec::new();
    let mut pending_annotations = Vec::new();
    let mut cursor = container.walk();
    for child in container.named_children(&mut cursor) {
        if child.kind() == "annotation" {
            let Some(annotation) = annotation_info(child, source) else {
                continue;
            };
            match annotation.name.as_str() {
                "tool" | "icon" | "static_unload" => {
                    lower_class_annotation_from_info(annotation, &mut members);
                }
                "warning_ignore" | "warning_ignore_start" | "warning_ignore_restore" => {}
                _ => pending_annotations.push(annotation),
            }
            continue;
        }

        let accepts_annotations = matches!(
            child.kind(),
            "variable_statement"
                | "export_variable_statement"
                | "onready_variable_statement"
                | "function_definition"
                | "constructor_definition"
        );
        if !accepts_annotations {
            for annotation in std::mem::take(&mut pending_annotations) {
                lower_class_annotation_from_info(annotation, &mut members);
            }
        }
        let annotations = if accepts_annotations {
            std::mem::take(&mut pending_annotations)
        } else {
            Vec::new()
        };
        lower_member(child, source, &mut members, annotations);
    }
    for annotation in pending_annotations {
        lower_class_annotation_from_info(annotation, &mut members);
    }
    members
}

fn lower_member(
    node: Node<'_>,
    source: &str,
    members: &mut Vec<ClassMember>,
    pending_annotations: Vec<AnnotationInfo>,
) {
    match node.kind() {
        "class_name_statement" => {
            lower_attached_class_annotations(node, source, members);
            let Some(name_node) = node.child_by_field_name("name") else {
                return;
            };
            members.push(ClassMember::ClassNameDecl {
                name: node_text(name_node, source),
                name_span: span_for_node(name_node, source),
                span: keyword_span(node, "class_name", source),
            });
            if let Some(extends) = node.child_by_field_name("extends") {
                lower_extends(extends, source, members);
            }
        }
        "extends_statement" => lower_extends(node, source, members),
        "signal_statement" => {
            lower_attached_class_annotations(node, source, members);
            let Some(name_node) = node.child_by_field_name("name") else {
                return;
            };
            members.push(ClassMember::Signal {
                name: node_text(name_node, source),
                name_span: span_for_node(name_node, source),
                parameters: node
                    .child_by_field_name("parameters")
                    .map_or_else(Vec::new, |parameters| lower_parameters(parameters, source)),
                span: keyword_span(node, "signal", source),
            });
        }
        "enum_definition" => {
            lower_attached_class_annotations(node, source, members);
            let name_node = node.child_by_field_name("name");
            let enum_members = node
                .child_by_field_name("body")
                .map_or_else(Vec::new, |body| lower_enumerators(body, source));
            members.push(ClassMember::Enum {
                name: name_node.map(|name| node_text(name, source)),
                name_span: name_node.map(|name| span_for_node(name, source)),
                members: enum_members,
                span: keyword_span(node, "enum", source),
            });
        }
        "const_statement" => {
            lower_attached_class_annotations(node, source, members);
            let Some(name_node) = node.child_by_field_name("name") else {
                return;
            };
            members.push(ClassMember::Constant {
                name: node_text(name_node, source),
                name_span: span_for_node(name_node, source),
                type_hint: lower_type_hint(node, source),
                span: keyword_span(node, "const", source),
            });
        }
        "variable_statement" | "export_variable_statement" | "onready_variable_statement" => {
            let Some(name_node) = node.child_by_field_name("name") else {
                return;
            };
            let mut annotations = pending_annotations;
            annotations.extend(lower_annotations(node, source));
            if node.kind() == "export_variable_statement" {
                push_inferred_annotation(&mut annotations, node, "export", source);
            } else if node.kind() == "onready_variable_statement" {
                push_inferred_annotation(&mut annotations, node, "onready", source);
            }
            let name = node_text(name_node, source);
            let name_span = span_for_node(name_node, source);
            let type_hint = lower_type_hint(node, source);
            if node.child_by_field_name("static").is_some() {
                members.push(ClassMember::StaticVariable {
                    name,
                    name_span,
                    type_hint,
                    annotations,
                    span: keyword_span(node, "static", source),
                });
            } else {
                members.push(ClassMember::Variable {
                    name,
                    name_span,
                    type_hint,
                    annotations,
                    span: keyword_span(node, "var", source),
                });
            }
        }
        "function_definition" | "constructor_definition" => {
            let mut annotations = pending_annotations;
            annotations.extend(lower_annotations(node, source));
            let (name, name_span) = if node.kind() == "constructor_definition" {
                let span = substring_span(node, "_init", source)
                    .unwrap_or_else(|| keyword_span(node, "func", source));
                ("_init".to_string(), span)
            } else if let Some(name_node) = node.child_by_field_name("name") {
                (
                    node_text(name_node, source),
                    span_for_node(name_node, source),
                )
            } else {
                return;
            };
            let span = keyword_span(node, "func", source);
            members.push(ClassMember::Function {
                name,
                name_span,
                parameters: node
                    .child_by_field_name("parameters")
                    .map_or_else(Vec::new, |parameters| lower_parameters(parameters, source)),
                return_type: node
                    .child_by_field_name("return_type")
                    .map(|return_type| node_text(return_type, source)),
                is_static: has_named_child(node, "static_keyword"),
                annotations,
                body_line_count: node.end_position().row + 1 - (span.line),
                span,
            });
        }
        "class_definition" => {
            lower_attached_class_annotations(node, source, members);
            let Some(name_node) = node.child_by_field_name("name") else {
                return;
            };
            let inner_members = node
                .child_by_field_name("body")
                .map_or_else(Vec::new, |body| lower_members(body, source));
            members.push(ClassMember::InnerClass {
                name: node_text(name_node, source),
                name_span: span_for_node(name_node, source),
                members: inner_members,
                span: keyword_span(node, "class", source),
            });
        }
        "comment" => {
            if !source[line_start_offset(source, node.start_byte())..node.start_byte()]
                .trim()
                .is_empty()
            {
                return;
            }
            let text = node_text(node, source);
            let span = span_for_node(node, source);
            if let Some(content) = text.strip_prefix("##") {
                members.push(ClassMember::DocComment {
                    text: content.to_string(),
                    span,
                });
            } else {
                members.push(ClassMember::Comment {
                    text: text.strip_prefix('#').unwrap_or(&text).to_string(),
                    is_doc: false,
                    span,
                });
            }
        }
        _ => {}
    }
}

fn lower_extends(node: Node<'_>, source: &str, members: &mut Vec<ClassMember>) {
    lower_attached_class_annotations(node, source, members);
    let mut cursor = node.walk();
    let base = node
        .named_children(&mut cursor)
        .find(|child| matches!(child.kind(), "type" | "string"))
        .map_or_else(String::new, |base| node_text(base, source));
    members.push(ClassMember::ExtendsDecl {
        base,
        span: keyword_span(node, "extends", source),
    });
}

fn lower_attached_class_annotations(node: Node<'_>, source: &str, members: &mut Vec<ClassMember>) {
    for annotation in lower_annotations(node, source) {
        match annotation.name.as_str() {
            "warning_ignore" | "warning_ignore_start" | "warning_ignore_restore" => {}
            _ => lower_class_annotation_from_info(annotation, members),
        }
    }
}

fn lower_class_annotation_from_info(annotation: AnnotationInfo, members: &mut Vec<ClassMember>) {
    match annotation.name.as_str() {
        "tool" => members.push(ClassMember::ToolAnnotation {
            span: annotation.span,
        }),
        "icon" => members.push(ClassMember::IconAnnotation {
            span: annotation.span,
        }),
        "static_unload" => members.push(ClassMember::StaticUnloadAnnotation {
            span: annotation.span,
        }),
        _ => members.push(ClassMember::ClassAnnotation {
            name: annotation.name,
            span: annotation.span,
        }),
    }
}

fn lower_annotations(node: Node<'_>, source: &str) -> Vec<AnnotationInfo> {
    let mut annotations = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() != "annotations" {
            continue;
        }
        let mut annotations_cursor = child.walk();
        annotations.extend(
            child
                .named_children(&mut annotations_cursor)
                .filter_map(|annotation| annotation_info(annotation, source)),
        );
    }
    annotations
}

fn annotation_info(node: Node<'_>, source: &str) -> Option<AnnotationInfo> {
    if node.kind() != "annotation" {
        return None;
    }
    let mut cursor = node.walk();
    let name_node = node
        .named_children(&mut cursor)
        .find(|child| child.kind() == "identifier")?;
    let mut span = span_for_node(node, source);
    span.length = name_node.end_byte().saturating_sub(node.start_byte());
    Some(AnnotationInfo {
        name: node_text(name_node, source),
        span,
    })
}

fn push_inferred_annotation(
    annotations: &mut Vec<AnnotationInfo>,
    node: Node<'_>,
    name: &str,
    source: &str,
) {
    if annotations.iter().any(|annotation| annotation.name == name) {
        return;
    }
    annotations.push(AnnotationInfo {
        name: name.to_string(),
        span: keyword_span(node, name, source),
    });
}

fn lower_enumerators(body: Node<'_>, source: &str) -> Vec<EnumMember> {
    let mut members = Vec::new();
    let mut cursor = body.walk();
    for enumerator in body.named_children(&mut cursor) {
        if enumerator.kind() != "enumerator" {
            continue;
        }
        if let Some(name_node) = enumerator.child_by_field_name("left") {
            members.push(EnumMember {
                name: node_text(name_node, source),
                span: span_for_node(name_node, source),
            });
        }
    }
    members
}

fn lower_parameters(parameters: Node<'_>, source: &str) -> Vec<Parameter> {
    let mut lowered = Vec::new();
    let mut cursor = parameters.walk();
    for parameter in parameters.named_children(&mut cursor) {
        let name_node = if parameter.kind() == "identifier" {
            Some(parameter)
        } else {
            first_named_descendant(parameter, "identifier")
        };
        let Some(name_node) = name_node else {
            continue;
        };
        lowered.push(Parameter {
            name: node_text(name_node, source),
            type_hint: parameter
                .child_by_field_name("type")
                .map(|type_node| node_text(type_node, source)),
            span: span_for_node(name_node, source),
        });
    }
    lowered
}

fn lower_type_hint(node: Node<'_>, source: &str) -> Option<String> {
    node.child_by_field_name("type").map(|type_node| {
        if type_node.kind() == "inferred_type" {
            ":=".to_string()
        } else {
            node_text(type_node, source)
        }
    })
}

fn first_named_descendant<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == kind {
            return Some(child);
        }
        if let Some(found) = first_named_descendant(child, kind) {
            return Some(found);
        }
    }
    None
}

fn has_named_child(node: Node<'_>, kind: &str) -> bool {
    let mut cursor = node.walk();
    let found = node
        .named_children(&mut cursor)
        .any(|child| child.kind() == kind);
    found
}

fn keyword_span(node: Node<'_>, keyword: &str, source: &str) -> Span {
    let mut cursor = node.walk();
    let span = node
        .children(&mut cursor)
        .find(|child| child.kind() == keyword)
        .map_or_else(
            || substring_span(node, keyword, source).unwrap_or_else(|| span_for_node(node, source)),
            |keyword_node| span_for_node(keyword_node, source),
        );
    span
}

fn substring_span(node: Node<'_>, needle: &str, source: &str) -> Option<Span> {
    let text = source.get(node.byte_range())?;
    let relative_offset = text.find(needle)?;
    let offset = node.start_byte() + relative_offset;
    Some(Span::new(
        source[..offset]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count()
            + 1,
        character_column(source, offset),
        offset,
        needle.len(),
    ))
}

fn span_for_node(node: Node<'_>, source: &str) -> Span {
    Span::new(
        node.start_position().row + 1,
        character_column(source, node.start_byte()),
        node.start_byte(),
        node.end_byte().saturating_sub(node.start_byte()),
    )
}

fn node_text(node: Node<'_>, source: &str) -> String {
    source
        .get(node.byte_range())
        .unwrap_or_default()
        .to_string()
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
    fn projects_class_declarations_from_the_concrete_tree() {
        let source = r#"@tool
@icon("res://icon.svg")
class_name Player
extends CharacterBody2D

## Emitted when state changes.
signal state_changed(old_value: int, reason = "")

enum State { IDLE, RUNNING = 2 }
const MAX_SPEED: float = 200.0
@export_range(0.0, 200.0) var speed: float = 100.0
static var total_count: int = 0
@onready var label: Label = $Label

func _init(value: int = 0) -> void:
	speed = value
@rpc("any_peer")
static func create(values: Dictionary[String, Array] = {}) -> Player:
	return Player.new()
class Inner extends RefCounted:
	signal done
	var value := 1
	func work(argument):
		pass
"#;
        let document = SyntaxDocument::parse(source).expect("parse should not be cancelled");
        assert!(document.is_valid());
        let members = document.class_members();

        assert!(matches!(members[0], ClassMember::ToolAnnotation { .. }));
        assert!(matches!(members[1], ClassMember::IconAnnotation { .. }));
        assert!(matches!(
            &members[2],
            ClassMember::ClassNameDecl { name, .. } if name == "Player"
        ));
        assert!(matches!(
            &members[3],
            ClassMember::ExtendsDecl { base, .. } if base == "CharacterBody2D"
        ));
        assert!(matches!(members[4], ClassMember::DocComment { .. }));
        assert!(matches!(
            &members[5],
            ClassMember::Signal { name, parameters, .. }
                if name == "state_changed"
                    && parameters.len() == 2
                    && parameters[0].type_hint.as_deref() == Some("int")
                    && parameters[1].type_hint.is_none()
        ));
        assert!(matches!(
            &members[6],
            ClassMember::Enum { name, members, .. }
                if name.as_deref() == Some("State")
                    && members.iter().map(|member| member.name.as_str()).collect::<Vec<_>>()
                        == ["IDLE", "RUNNING"]
        ));
        assert!(matches!(
            &members[7],
            ClassMember::Constant { name, type_hint, .. }
                if name == "MAX_SPEED" && type_hint.as_deref() == Some("float")
        ));
        assert!(matches!(
            &members[8],
            ClassMember::Variable { name, annotations, .. }
                if name == "speed" && annotations[0].name == "export_range"
        ));
        assert!(matches!(
            &members[9],
            ClassMember::StaticVariable { name, .. } if name == "total_count"
        ));
        assert!(matches!(
            &members[10],
            ClassMember::Variable { name, annotations, .. }
                if name == "label" && annotations[0].name == "onready"
        ));
        assert!(matches!(
            &members[11],
            ClassMember::Function { name, parameters, return_type, body_line_count, .. }
                if name == "_init"
                    && parameters.len() == 1
                    && return_type.as_deref() == Some("void")
                    && *body_line_count == 1
        ));
        assert!(
            matches!(
                &members[12],
                ClassMember::Function { name, is_static, annotations, parameters, .. }
                    if name == "create"
                        && *is_static
                        && annotations[0].name == "rpc"
                        && parameters[0].type_hint.as_deref() == Some("Dictionary[String, Array]")
            ),
            "{:#?}",
            members[12]
        );
        assert!(matches!(
            &members[13],
            ClassMember::InnerClass { name, members, .. }
                if name == "Inner"
                    && matches!(&members[0], ClassMember::Signal { name, .. } if name == "done")
                    && matches!(&members[1], ClassMember::Variable { name, type_hint, .. }
                        if name == "value" && type_hint.as_deref() == Some(":="))
                    && matches!(&members[2], ClassMember::Function { name, .. } if name == "work")
        ));

        let mut lexer = crate::lexer::Lexer::new(source);
        let tokens = lexer.tokenize();
        let legacy_members = crate::parser::Parser::new(&tokens).parse();
        assert_eq!(members, legacy_members);
    }

    #[test]
    fn projected_spans_use_character_columns_and_byte_offsets() {
        let source = "var café: int = 1\n";
        let document = SyntaxDocument::parse(source).expect("parse should not be cancelled");
        let members = document.class_members();

        let ClassMember::Variable { name_span, .. } = members[0] else {
            panic!("expected a variable");
        };
        assert_eq!(name_span.column, 5);
        assert_eq!(name_span.offset, 4);
        assert_eq!(name_span.length, "café".len());
    }

    #[test]
    fn projection_matches_legacy_declarations_on_valid_fixtures() {
        let fixture_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures");
        for entry in std::fs::read_dir(fixture_dir).expect("fixture directory should exist") {
            let path = entry.expect("fixture entry should be readable").path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("gd") {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("fixture should be readable");
            let document = SyntaxDocument::parse(&source).expect("parse should not be cancelled");
            if !document.is_valid() {
                continue;
            }

            let mut projected = document.class_members();
            let mut lexer = crate::lexer::Lexer::new(&source);
            let tokens = lexer.tokenize();
            let mut legacy = crate::parser::Parser::new(&tokens).parse();
            clear_body_line_counts(&mut projected);
            clear_body_line_counts(&mut legacy);
            assert_eq!(
                projected,
                legacy,
                "projection mismatch in {}",
                path.display()
            );
        }
    }

    fn clear_body_line_counts(members: &mut [ClassMember]) {
        for member in members {
            match member {
                ClassMember::Function {
                    body_line_count, ..
                } => *body_line_count = 0,
                ClassMember::InnerClass { members, .. } => clear_body_line_counts(members),
                _ => {}
            }
        }
    }

    #[test]
    fn reports_character_column_after_unicode() {
        assert_eq!(character_column("é x", "é".len()), 2);
        assert_eq!(character_column("first\né x", "first\né".len()), 2);
    }
}
