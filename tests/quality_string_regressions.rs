use gdstyle::{
    config::{Config, RuleSeverityConfig},
    linter, syntax,
};

fn quality(source: &str, config: &Config) -> Vec<(String, usize)> {
    assert!(syntax::is_valid(source), "invalid test input: {source}");
    linter::lint_source(source, "example.gd", config)
        .into_iter()
        .filter(|d| d.rule.starts_with("quality/"))
        .map(|d| (d.rule, d.span.line))
        .collect()
}

#[test]
fn multiline_string_pseudo_code_does_not_trigger_structural_quality_rules() {
    let source = "func f():\n\tvar text = \"\"\"\n\treturn 1\n\treturn 2\n\tpass\n\tfor item in items:\n\t\tawait work()\n\t\tObject.new()\n\tif value:\n\telif value:\n\tmatch value:\n\tvar first = 1\n\tvar second = 2\n\t\t\t\t\t\tdeeply indented text\n\t\"\"\"\n\tprint(text)\n";
    let config = Config {
        max_returns: 1,
        max_branches: 1,
        max_local_variables: 1,
        max_nesting_depth: 1,
        ..Config::default()
    };
    for source in [
        source.to_string(),
        source.replace('\n', "\r\n"),
        source.replace("\"\"\"", "'''"),
        source.replacen("\"\"\"", "r\"\"\"", 1),
    ] {
        assert!(
            quality(&source, &config).is_empty(),
            "unexpected warnings: {:?}",
            quality(&source, &config)
        );
    }
}

#[test]
fn actual_unreachable_statements_are_detected_after_multiline_and_inline_returns() {
    for source in [
        "func f():\n\treturn (\n\t\t1\n\t)\n\tprint(1)\n",
        "func f():\n\treturn; print(1)\n",
        "func f():\n\twhile true:\n\t\tbreak; print(1)\n",
    ] {
        assert_eq!(
            quality(source, &Config::default())
                .iter()
                .filter(|(r, _)| r == "quality/unreachable-code")
                .count(),
            1
        );
    }
}

#[test]
fn return_counts_use_statements_and_exclude_lambda_returns() {
    let config = Config {
        max_returns: 1,
        ..Config::default()
    };
    let source = "func f(value):\n\tvar callback = func():\n\t\treturn 1\n\tif value: return(1)\n\treturn-1\n";
    assert_eq!(
        quality(source, &config)
            .iter()
            .filter(|(r, _)| r == "quality/max-returns")
            .count(),
        1
    );
    let source =
        "func f():\n\tvar callback = func():\n\t\tif true: return 1\n\t\treturn 2\n\treturn 0\n";
    assert!(!quality(source, &config)
        .iter()
        .any(|(r, _)| r == "quality/max-returns"));
}

#[test]
fn actual_loop_operations_are_detected_but_deferred_lambda_bodies_are_not_loops() {
    let source = "func f():\n\tfor item in items:\n\t\tawait work()\n\t\tObject.new()\n\t\tvar callback = func():\n\t\t\tawait work()\n\t\t\tObject.new()\n";
    let results = quality(source, &Config::default());
    assert_eq!(
        results
            .iter()
            .filter(|(r, _)| r == "quality/await-in-loop")
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|(r, _)| r == "quality/allocation-in-loop")
            .count(),
        1
    );
}

#[test]
fn real_complexity_and_pass_statements_still_trigger_rules() {
    let mut config = Config {
        max_branches: 1,
        max_local_variables: 1,
        max_nesting_depth: 1,
        ..Config::default()
    };
    config
        .rules
        .insert("quality/no-else-return".into(), RuleSeverityConfig::Off);
    let source =
        "func f():\n\tvar a = 1\n\tvar b = 2\n\tif a:\n\t\tif b:\n\t\t\tprint(a)\n\tpass\n";
    let results = quality(source, &config);
    for rule in [
        "quality/max-branches",
        "quality/max-local-variables",
        "quality/max-nesting-depth",
        "quality/unnecessary-pass",
    ] {
        assert!(
            results.iter().any(|(r, _)| r == rule),
            "missing {rule}: {results:?}"
        );
    }
}

#[test]
fn literal_contents_and_deferred_returns_do_not_leak_into_the_enclosing_block() {
    let config = Config {
        max_returns: 1,
        ..Config::default()
    };
    let source = "func f():\n\tvar callback = func():\n\t\treturn 1\n\tprint(callback)\n\tvar text = \"\"\"\nreturn 1\n\"\"\"; return 0\n";
    assert!(quality(source, &config).is_empty());
    let source = "func f():\n\t#region body\n\tpass\n\t#endregion\n";
    assert!(quality(source, &config).is_empty());
}

#[test]
fn for_iterables_are_outside_loop_context_but_while_conditions_are_repeated() {
    let source = "func f():\n\tfor item in get_items(Object.new()):\n\t\tpass\n\twhile await ready():\n\t\tpass\n\twhile is_ready(Object.new()):\n\t\tpass\n\tvar callback = func():\n\t\tfor item in items:\n\t\t\tawait work()\n\t\t\tObject.new()\n";
    let results = quality(source, &Config::default());
    let lines = |rule: &str| {
        results
            .iter()
            .filter(|(r, _)| r == rule)
            .map(|(_, line)| *line)
            .collect::<Vec<_>>()
    };
    assert_eq!(lines("quality/await-in-loop"), vec![4, 10]);
    assert_eq!(lines("quality/allocation-in-loop"), vec![6, 11]);
}

#[test]
fn complexity_diagnostics_point_to_the_function_below_its_annotation() {
    let config = Config {
        max_returns: 1,
        max_branches: 0,
        max_local_variables: 0,
        max_nesting_depth: 0,
        ..Config::default()
    };
    let source = "@warning_ignore(\"unused_parameter\")\nfunc f(value):\n\tvar a = 1\n\tif value:\n\t\treturn a\n\treturn 0\n";
    let results = quality(source, &config);
    for rule in [
        "quality/max-returns",
        "quality/max-branches",
        "quality/max-local-variables",
    ] {
        assert!(
            results.contains(&(rule.into(), 2)),
            "wrong declaration line for {rule}: {results:?}"
        );
    }
    assert!(results.contains(&("quality/max-nesting-depth".into(), 5)));
}
