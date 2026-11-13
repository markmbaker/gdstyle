use gdstyle::{config::Config, diagnostic::Diagnostic, linter, syntax};

fn private_diagnostics(source: &str) -> Vec<Diagnostic> {
    assert!(syntax::is_valid(source), "invalid test input: {source}");
    linter::lint_source(source, "private_members.gd", &Config::default())
        .into_iter()
        .filter(|diagnostic| diagnostic.rule == "naming/private-underscore-prefix")
        .collect()
}

#[test]
fn private_export_checks_every_class_depth_and_preserves_source_spans() {
    let source = "@export\nvar _top: bool\nclass Outer:\n\t@export\n\tvar _outer: bool\n\tclass Nested:\n\t\t@export_range(0, 10)\n\t\tvar _inner: int\n";
    let diagnostics = private_diagnostics(source);
    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| (diagnostic.span.line, diagnostic.span.column))
            .collect::<Vec<_>>(),
        vec![(2, 1), (5, 2), (8, 3)]
    );
    for (diagnostic, name) in diagnostics.iter().zip(["_top", "_outer", "_inner"]) {
        assert!(diagnostic.message.contains(name));
        assert_eq!(diagnostic.file, "private_members.gd");
        assert!(diagnostic.fix.is_none());
    }
}

#[test]
fn nested_private_exports_are_checked_for_each_export_variant() {
    for (annotation, type_hint) in [
        ("export", "bool"),
        ("export_range(0, 10)", "int"),
        ("export_enum(\"A\", \"B\")", "int"),
        ("export_file(\"*.txt\")", "String"),
    ] {
        let source = format!("class Inner:\n\t@{annotation}\n\tvar _value: {type_hint}\n");
        let diagnostics = private_diagnostics(&source);
        assert_eq!(diagnostics.len(), 1, "missing @{annotation} diagnostic");
        assert_eq!(diagnostics[0].span.line, 3);
    }
}

#[test]
fn nested_private_storage_and_section_markers_do_not_expose_variables() {
    for annotation in [
        "export_storage",
        "export_group(\"\")",
        "export_subgroup(\"Private\")",
        "export_category(\"Private\")",
    ] {
        let source =
            format!("class Outer:\n\tclass Inner:\n\t\t@{annotation}\n\t\tvar _value: bool\n");
        assert!(
            private_diagnostics(&source).is_empty(),
            "@{annotation} must not expose private state"
        );
    }
    let source = "@export var public_value: bool\nvar _private_value: bool\nclass Inner:\n\t@export var public_value: bool\n\tvar _private_value: bool\n";
    assert!(private_diagnostics(source).is_empty());
}
