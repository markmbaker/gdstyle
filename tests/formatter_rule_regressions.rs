use gdstyle::{
    config::{Config, RuleSeverityConfig},
    formatter::format_source,
    syntax,
};

fn disabled(rule: &str) -> Config {
    let mut config = Config::default();
    config.rules.insert(rule.into(), RuleSeverityConfig::Off);
    config
}

#[test]
fn disabled_formatter_token_rules_preserve_their_spelling() {
    let cases = [
        ("format/double-quotes", "var text = 'hello'\n"),
        (
            "format/boolean-operators",
            "func f(a, b):\n\treturn a && b\n",
        ),
        ("format/comment-spacing", "extends Node\n##Documentation\n"),
        ("format/float-literal-zeros", "var value = 1.\n"),
        ("format/number-literals", "var value = 0xAB\n"),
    ];
    for (rule, source) in cases {
        assert!(syntax::is_valid(source), "invalid test for {rule}");
        let result = format_source(source, &disabled(rule));
        assert_eq!(result, source, "disabled {rule} changed source");
        assert_eq!(format_source(&result, &disabled(rule)), result);
        assert_ne!(
            format_source(source, &Config::default()),
            source,
            "enabled {rule} did nothing"
        );
    }
}

#[test]
fn disabled_layout_rules_preserve_layout_while_other_rules_still_fix() {
    let cases = [
        (
            "format/no-tabs-as-spaces",
            "func f():\n    var text = 'hello'\n    return text\n",
            "    var text = \"hello\"",
        ),
        (
            "format/trailing-whitespace",
            "var text = 'hello'   \n",
            "var text = \"hello\"   \n",
        ),
        (
            "format/blank-lines",
            "var text = 'hello'\n\n\n\nvar other = 1\n",
            "\n\n\n\n",
        ),
        (
            "format/trailing-newline",
            "var text = 'hello'",
            "var text = \"hello\"",
        ),
    ];
    for (rule, source, kept) in cases {
        assert!(syntax::is_valid(source));
        let result = format_source(source, &disabled(rule));
        assert!(
            result.contains(kept),
            "disabled {rule} changed layout: {result:?}"
        );
        if rule == "format/trailing-newline" {
            assert!(!result.ends_with('\n'));
        }
        assert!(
            !result.contains("'hello'"),
            "unrelated quote fix was skipped"
        );
        assert_eq!(format_source(&result, &disabled(rule)), result);
    }
}

#[test]
fn disabled_line_length_preserves_long_comments_and_code() {
    let source = "func f():\n\tprint(aaaaaaaa, bbbbbbbb, cccccccc)\n\t# Some long prose that exceeds the configured width by several words.\n";
    let mut config = disabled("format/max-line-length");
    config.max_line_length = 30;
    assert_eq!(format_source(source, &config), source);
    config.rules.clear();
    assert_ne!(format_source(source, &config), source);
}

#[test]
fn disabling_all_formatter_rules_is_a_noop() {
    let source = "extends Node\n\n\n\nfunc f():\n    var text = 'hello'   \n    return text";
    let mut config = Config::default();
    for rule in gdstyle::rules::all_rules() {
        config.rules.insert(rule.0.into(), RuleSeverityConfig::Off);
    }
    assert_eq!(format_source(source, &config), source);
}

#[test]
fn inner_class_reordering_preserves_indentation_when_its_rule_is_off() {
    let source = "class Inner:\n    func f():\n        return 1\n    const VALUE = 2\n";
    let result = format_source(source, &disabled("format/no-tabs-as-spaces"));
    assert!(
        !result.contains('\t'),
        "reordering converted spaces to tabs: {result:?}"
    );
    assert!(result.find("const VALUE").unwrap() < result.find("func f").unwrap());
    assert!(result.contains("    const VALUE"));
    assert!(result.contains("        return 1"));
}

#[test]
fn reordering_preserves_disabled_blank_line_layout_at_each_class_depth() {
    let config = disabled("format/blank-lines");
    for source in [
        "func f():\n\tpass\n\n\n\nvar value = 1\n",
        "class Inner:\n\tfunc f():\n\t\tpass\n\n\n\n\tvar value = 1\n",
    ] {
        let result = format_source(source, &config);
        assert!(
            result.contains("\n\n\n\n"),
            "reordering collapsed the gap: {result:?}"
        );
        assert!(result.find("var value").unwrap() < result.find("func f").unwrap());
        assert_eq!(format_source(&result, &config), result);
    }
}

#[test]
fn overindented_leading_comments_do_not_change_inner_function_scope() {
    let source = "class Inner:\n        # deliberately overindented comment\n    func f():\n        return 1\n\nvar text = 'hello'\n";
    let result = format_source(source, &Config::default());
    assert!(
        result.contains("\tfunc f():\n\t\treturn 1"),
        "function/body indentation changed: {result:?}"
    );
    assert!(result.contains("\"hello\""), "unrelated format was skipped");
    assert_eq!(format_source(&result, &Config::default()), result);
}
