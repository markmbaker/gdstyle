use gdstyle::{
    config::{Config, RuleSeverityConfig},
    formatter::format_source,
    syntax,
};

fn format(source: &str, config: &Config) -> String {
    assert!(syntax::is_valid(source));
    let result = format_source(source, config);
    assert!(syntax::is_valid(&result));
    assert_ne!(result, source, "long line was not wrapped");
    assert_eq!(format_source(&result, config), result, "not idempotent");
    result
}

fn config(disabled: &str) -> Config {
    let mut config = Config {
        max_line_length: 40,
        ..Config::default()
    };
    config
        .rules
        .insert(disabled.into(), RuleSeverityConfig::Off);
    config
}

#[test]
fn wrapping_preserves_space_indentation_when_its_rule_is_disabled() {
    let config = config("format/no-tabs-as-spaces");
    for source in [
        "func f():\n    print(aaaaaaaaaaaaaa, bbbbbbbbbbbbbb, ccccccccccccc)\n",
        "func f():\n    if first_very_long_condition and second_very_long_condition:\n        pass\n",
    ] {
        let result = format(source, &config);
        assert!(!result.contains('\t'), "wrapping inserted tabs: {result:?}");
    }
}

#[test]
fn wrapping_preserves_tab_indentation_when_its_rule_is_disabled() {
    let mut config = config("format/no-tabs-as-spaces");
    config.use_tabs = false;
    for source in [
        "func f():\n\tprint(aaaaaaaaaaaaaa, bbbbbbbbbbbbbb, ccccccccccccc)\n",
        "func f():\n\tif first_very_long_condition and second_very_long_condition:\n\t\tpass\n",
    ] {
        let result = format(source, &config);
        assert!(
            result
                .lines()
                .all(|line| !line.starts_with(' ') && !line.starts_with("\t ")),
            "wrapping inserted space indentation: {result:?}"
        );
    }
}

#[test]
fn disabled_trailing_comma_rule_does_not_add_a_comma_when_wrapping() {
    let config = config("format/trailing-comma");
    for source in [
        "func f():\n\tprint(aaaaaaaaaaaaaa, bbbbbbbbbbbbbb, ccccccccccccc)\n",
        "var values = [aaaaaaaaaaaaaa, bbbbbbbbbbbbbb, ccccccccccccc]\n",
        "var values = {\"aaaaaaaaaaaa\": 1, \"bbbbbbbbbbbb\": 2, \"cccccccccccc\": 3}\n",
    ] {
        let result = format(source, &config);
        let lines: Vec<_> = result.lines().collect();
        let last_item = lines
            .iter()
            .rposition(|line| line.trim().starts_with([')', ']', '}']))
            .unwrap()
            - 1;
        assert!(
            !lines[last_item].trim_end().ends_with(','),
            "wrapping added trailing comma: {result:?}"
        );
    }
}

#[test]
fn disabled_trailing_comma_rule_keeps_an_existing_comma_when_wrapping() {
    let source = "func f():\n\tprint(aaaaaaaaaaaaaa, bbbbbbbbbbbbbb, ccccccccccccc,)\n";
    let result = format(source, &config("format/trailing-comma"));
    assert!(result.contains("ccccccccccccc,\n"));
}

#[test]
fn enabled_wrapping_rules_still_apply_configured_indentation_and_trailing_comma() {
    let source = "func f():\n    print(aaaaaaaaaaaaaa, bbbbbbbbbbbbbb, ccccccccccccc)\n";
    let config = Config {
        max_line_length: 40,
        ..Config::default()
    };
    let result = format(source, &config);
    assert!(result.contains("\t\tccccccccccccc,\n"));
    assert!(result.lines().all(|line| !line.starts_with(' ')));
}
