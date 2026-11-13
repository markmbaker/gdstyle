use gdstyle::{
    config::{Config, RuleSeverityConfig},
    fixer, formatter,
    lexer::Lexer,
    linter, syntax,
    token::TokenKind,
};

fn strings(source: &str) -> Vec<String> {
    let mut literals: Vec<_> = Lexer::new(source)
        .tokenize()
        .into_iter()
        .filter_map(|token| matches!(token.kind, TokenKind::String(_)).then_some(token.text))
        .collect();
    literals.sort();
    literals
}

fn format(source: &str, config: &Config) -> String {
    assert!(syntax::is_valid(source), "invalid test input: {source}");
    let result = formatter::format_source(source, config);
    assert!(syntax::is_valid(&result), "invalid output: {result}");
    assert_eq!(
        formatter::format_source(&result, config),
        result,
        "not idempotent"
    );
    result
}

#[test]
fn multiline_literals_keep_their_bytes_in_fmt_and_safe_fix() {
    let fixture = include_str!("fixtures/multiline_string.gd");
    for source in [
        fixture.to_string(),
        fixture.replace('\n', "\r\n"),
        "extends Node\r\n\r\nvar text = \"\"\"a\nb   \n\n\n\n    c\"\"\"\r\n".into(),
        "class Inner:\n\tvar text = '''\n  class Fake:\n\n\n\n    # not a comment\n  '''\n".into(),
        "var text = r\"\"\"\n    raw data   \n\n\n\n\"\"\"\n".into(),
        "var text = \"first\n    second   \n\n\n\nthird\"\n".into(),
    ] {
        let config = Config::default();
        let result = format(&source, &config);
        assert_eq!(strings(&result), strings(&source));
        let diagnostics = linter::lint_source(&source, "example.gd", &config);
        let fixed = fixer::apply_fixes(&source, &diagnostics, true);
        assert_eq!(strings(&fixed), strings(&source));
    }
}

#[test]
fn unicode_long_conditions_wrap_without_a_panic_or_data_loss() {
    let source = format!(
        "func f():\n\tif \"éé\" == {} or {}:\n\t\tpass\n",
        "a".repeat(65),
        "b".repeat(65)
    );
    let result = format(&source, &Config::default());
    assert_ne!(result, source);
    assert!(result.contains(&"b".repeat(65)));
    assert_eq!(strings(&result), strings(&source));
}

#[test]
fn disabled_ordering_preserves_outer_and_inner_member_order() {
    let source = "class Inner:\n\tfunc a():\n\t\tpass\n\tconst X = 1\n\nfunc f():\n\tpass\n";
    let mut config = Config::default();
    config
        .rules
        .insert("order/class-member-order".into(), RuleSeverityConfig::Off);
    let result = format(source, &config);
    assert!(result.find("class Inner").unwrap() < result.find("func f").unwrap());
    assert!(result.find("func a").unwrap() < result.find("const X").unwrap());
    let enabled = format(source, &Config::default());
    assert!(enabled.find("func f").unwrap() < enabled.find("class Inner").unwrap());
}

#[test]
fn region_folds_keep_the_same_members() {
    let source = "extends Node\n\n\n#region helpers\nfunc helper() -> int:\n\treturn 1\n\n\nclass Inner:\n\tvar value: int\n\n\n#endregion\n\n\n#region tests\nfunc test_a() -> void:\n\tpass\n#endregion\n";
    let result = format(source, &Config::default());
    assert!(result.find("class Inner").unwrap() < result.find("#endregion").unwrap());
    assert!(result.find("#endregion").unwrap() < result.find("#region tests").unwrap());
}

#[test]
fn decorative_hash_banners_stay_together() {
    let source = "extends Node\n\n\n################################################################################\n# signal handlers\n################################################################################\nfunc _on_event() -> void:\n\tpass\n";
    assert_eq!(format(source, &Config::default()), source);
    let docs = "extends Node\n## Real docs\n##\n## ###\n\n\nfunc f():\n\tpass\n";
    assert_eq!(format(docs, &Config::default()), docs);
}

fn diagnostics(source: &str, rule: &str) -> Vec<usize> {
    assert!(syntax::is_valid(source), "invalid test input: {source}");
    linter::lint_source(source, "test.gd", &Config::default())
        .iter()
        .filter(|d| d.rule == rule)
        .map(|d| d.span.line)
        .collect()
}

#[test]
fn inspector_sections_do_not_export_the_following_private_variable() {
    for marker in ["export_group", "export_subgroup", "export_category"] {
        let source = format!("extends Node\n@{marker}(\"\")\nvar _private: bool\n@export var _shown: bool\n@export_storage var _stored: bool\n");
        assert_eq!(
            diagnostics(&source, "naming/private-underscore-prefix"),
            vec![4]
        );
        let output = format(&source, &Config::default());
        assert!(output.find("var _private").unwrap() < output.find("var _shown").unwrap());
    }
    assert_eq!(
        diagnostics(
            "@export_range(0, 1) var _shown: int\n",
            "naming/private-underscore-prefix"
        ),
        vec![1]
    );
}

#[test]
fn else_return_respects_scope_chains_and_multiline_returns() {
    let nested = "func f(a, b):\n\tif a:\n\t\tif b:\n\t\t\tpass\n\t\telse:\n\t\t\treturn\n\telse:\n\t\tpass\n";
    assert!(diagnostics(nested, "quality/no-else-return").is_empty());
    let chain = "func f(a, b):\n\tif a:\n\t\tpass\n\telif b:\n\t\treturn\n\telse:\n\t\tpass\n";
    assert!(diagnostics(chain, "quality/no-else-return").is_empty());
    let positive = "func f(a, b):\n\tif a:\n\t\treturn (\n\t\t\t1\n\t\t)\n\telif b: return -1\n\telse:\n\t\tpass\n";
    assert_eq!(diagnostics(positive, "quality/no-else-return"), vec![6, 7]);
    let name = "func f(a):\n\tif a:\n\t\tvar returned = true\n\telse:\n\t\tpass\n";
    assert!(diagnostics(name, "quality/no-else-return").is_empty());
}

#[test]
fn process_lookups_ignore_strings_comments_and_modulo() {
    let source = "func _physics_process(delta):\n\tprint(\"%s\" % [\"x\"])\n\tprint(\"$Node get_node(\")\n\tvar text = \"\"\"\n%Node\n$Node\nget_node(\n\"\"\"\n\tvar value = 5 % delta\n\tmy_get_node() # get_node(\n\tvar a = %Player\n\tvar b = $Player\n\tget_node(\"Player\")\n\tself.get_node_or_null(\"Player\")\n";
    assert_eq!(
        diagnostics(source, "quality/process-get-node"),
        vec![11, 12, 13, 14]
    );
}

#[test]
fn unified_diff_can_be_applied_as_a_patch() {
    use std::{fs, process::Command};
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("gdstyle.toml"), "").unwrap();
    let source = "extends Node\n\n\nclass Inner:\n\tvar value: int\n\n\nfunc helper():\n\tpass\n";
    fs::write(dir.path().join("example.gd"), source).unwrap();
    assert!(Command::new("git")
        .arg("init")
        .current_dir(dir.path())
        .output()
        .unwrap()
        .status
        .success());
    let output = Command::new(env!("CARGO_BIN_EXE_gdstyle"))
        .args(["fmt", "--diff", "--no-color", "example.gd"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let diff = String::from_utf8(output.stdout).unwrap();
    assert!(diff.contains("@@ "));
    fs::write(dir.path().join("changes.patch"), &diff).unwrap();
    let applied = Command::new("git")
        .args(["apply", "changes.patch"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        applied.status.success(),
        "{}",
        String::from_utf8_lossy(&applied.stderr)
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("example.gd")).unwrap(),
        formatter::format_source(source, &Config::default())
    );
}
