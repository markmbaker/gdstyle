use gdstyle::{
    config::{Config, RuleSeverityConfig},
    fixer, linter, syntax,
};

fn fix(source: &str, config: &Config) -> String {
    assert!(syntax::is_valid(source), "invalid input: {source:?}");
    let diagnostics = linter::lint_source(source, "test.gd", config);
    let fixed = fixer::apply_fixes(source, &diagnostics, true);
    assert!(syntax::is_valid(&fixed), "invalid output: {fixed:?}");
    fixed
}

#[test]
fn unrelated_fixes_preserve_blank_runs_when_rule_is_off() {
    let mut config = Config::default();
    config
        .rules
        .insert("format/blank-lines".into(), RuleSeverityConfig::Off);
    for source in [
        "var text = 'hello'\n\n\n\n\nvar other = 1\n",
        "var text = 'hello'\n\n\n\n\n",
    ] {
        assert_eq!(fix(source, &config), source.replace("'hello'", "\"hello\""));
        let crlf = source.replace('\n', "\r\n");
        assert_eq!(fix(&crlf, &config), crlf.replace("'hello'", "\"hello\""));
    }
}

#[test]
fn unrelated_fixes_preserve_suppressed_blank_runs() {
    for directive in ["# gdstyle:ignore-file=format/blank-lines\n", ""] {
        let source = if directive.is_empty() {
            "var text = 'hello'\n# gdstyle:ignore=format/blank-lines\n\n\n\n\nvar other = 1\n"
                .to_string()
        } else {
            format!("{directive}var text = 'hello'\n\n\n\n\nvar other = 1\n")
        };
        assert_eq!(
            fix(&source, &Config::default()),
            source.replace("'hello'", "\"hello\"")
        );
    }
}

#[test]
fn enabled_blank_run_fix_wins_over_whitespace_fixes_inside_deleted_lines() {
    let source = "var text = 'hello'\n \n\t\n \t\n\t \nvar other = 1\n";
    let config = Config::default();
    let fixed = fix(source, &config);
    assert_eq!(fixed, "var text = \"hello\"\n\n\nvar other = 1\n");
    assert_eq!(fix(&fixed, &config), fixed);
}

#[test]
fn unrelated_fixes_preserve_blank_lines_inside_literals() {
    let source = "var text = 'hello'\nvar block = \"\"\"a\n\n\n\n\n  b   \n\"\"\"\n";
    assert_eq!(
        fix(source, &Config::default()),
        source.replace("'hello'", "\"hello\"")
    );
}

#[test]
fn trailing_semicolon_fix_does_not_create_a_blank_or_indented_empty_line() {
    for tail in [";", " ;  ", "; # inline comment", ";# inline comment"] {
        let source = format!("func f():\n\tvar x = 1{tail}\n\tprint(x)\n");
        let config = Config::default();
        let fixed = fix(&source, &config);
        let comment = if tail.contains('#') {
            " # inline comment"
        } else {
            ""
        };
        assert_eq!(
            fixed,
            format!("func f():\n\tvar x = 1{comment}\n\tprint(x)\n")
        );
        assert_eq!(fix(&fixed, &config), fixed);
    }
    let peers = "func f():\n\tvar x = 1; print(x)\n";
    assert_eq!(
        fix(peers, &Config::default()),
        "func f():\n\tvar x = 1\n\tprint(x)\n"
    );
}

#[test]
fn float_zero_fix_belongs_only_to_the_float_zero_rule() {
    let source = "var value = 1.\n";
    let diagnostics = linter::lint_source(source, "test.gd", &Config::default());
    let zeros: Vec<_> = diagnostics
        .iter()
        .filter(|d| {
            matches!(
                d.rule.as_str(),
                "format/float-literal-zeros" | "format/number-literals"
            )
        })
        .collect();
    assert_eq!(
        zeros.len(),
        1,
        "float zeros should not be reported by two rules"
    );
    assert_eq!(zeros[0].rule, "format/float-literal-zeros");
    let mut config = Config::default();
    config
        .rules
        .insert("format/float-literal-zeros".into(), RuleSeverityConfig::Off);
    assert_eq!(fix(source, &config), source);
    assert_eq!(fix("var value = 0xFF\n", &config), "var value = 0xff\n");
}
