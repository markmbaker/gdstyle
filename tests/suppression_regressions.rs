use gdstyle::{config::Config, linter};
use std::{fs, process::Command};

fn naming_lines(source: &str) -> Vec<usize> {
    linter::lint_source(source, "example.gd", &Config::default())
        .into_iter()
        .filter(|diagnostic| diagnostic.rule == "naming/variable-name-snake-case")
        .map(|diagnostic| diagnostic.span.line)
        .collect()
}

#[test]
fn directives_in_literals_do_not_suppress_diagnostics() {
    for literal in [
        "\"# gdstyle:ignore-file example\"",
        "'# gdstyle:ignore-file example'",
        "r\"# gdstyle:ignore-file example\"",
        "&\"# gdstyle:ignore-file example\"",
        "^\"# gdstyle:ignore-file example\"",
        "\"\"\"\n# gdstyle:ignore-file\n\"\"\"",
        "'''\n# gdstyle:ignore-file\n'''",
        "r\"\"\"\n# gdstyle:ignore-file\n\"\"\"",
        "r\"escaped \\\" # gdstyle:ignore-file example\"",
    ] {
        let source = format!("var text = {literal}\nvar BadName = 1\n");
        assert!(
            gdstyle::syntax::is_valid(&source),
            "invalid test literal: {literal}"
        );
        let diagnostics = linter::lint_source(&source, "example.gd", &Config::default());
        assert!(
            diagnostics
                .iter()
                .any(|d| d.rule == "naming/variable-name-snake-case"),
            "literal incorrectly suppressed naming diagnostics: {literal}"
        );
    }
}

#[test]
fn fake_inline_and_standalone_directives_do_not_hide_nearby_names() {
    let inline = "var BadName = \"# gdstyle:ignore example\"\n";
    assert_eq!(naming_lines(inline), vec![1]);
    let standalone = "var text = \"\"\"\n# gdstyle:ignore\n\"\"\"\nvar BadName = 1\n";
    assert_eq!(naming_lines(standalone), vec![4]);
}

#[test]
fn real_directives_keep_their_inline_next_line_and_file_scopes() {
    let source = "# gdstyle:ignore=naming/variable-name-snake-case\nvar FirstBad = 1\nvar SecondBad = 2 # gdstyle:ignore=naming/variable-name-snake-case\nvar ThirdBad = 3\n";
    assert_eq!(naming_lines(source), vec![4]);
    assert!(naming_lines("var BadName = 1 # gdstyle:ignore-file=naming/variable-name-snake-case\nvar AnotherBad = 2\n").is_empty());
    let mixed = "var text = \"# gdstyle:ignore-file example\" # gdstyle:ignore=naming/variable-name-snake-case\nvar BadName = 1\n";
    assert_eq!(naming_lines(mixed), vec![2]);
    let unicode = "var text = \"é\" # gdstyle:ignore-file=naming/variable-name-snake-case\r\nvar BadName = 1\r\n";
    assert!(naming_lines(unicode).is_empty());
}

#[test]
fn literal_directives_cannot_make_invalid_scripts_pass_the_cli() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("gdstyle.toml"), "").unwrap();
    for prelude in [
        "var text = \"# gdstyle:ignore-file example\"\n",
        "var text = \"\"\"\n# gdstyle:ignore-file\n\"\"\"\n",
    ] {
        let path = dir.path().join("example.gd");
        fs::write(&path, format!("{prelude}func broken( -> void:\n\tpass\n")).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_gdstyle"))
            .args(["check", "--no-color", "example.gd"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("syntax/parse-error"));
    }
}

#[test]
fn directive_mentions_in_comment_prose_and_doc_comments_are_inert() {
    let source = "# Documentation mentions # gdstyle:ignore-file as an example.\n## A doc comment quotes # gdstyle:ignore-file too.\nvar BadName = 1\n";
    assert_eq!(naming_lines(source), vec![3]);
}

#[test]
fn real_file_directives_can_still_suppress_syntax_diagnostics() {
    let source = "# gdstyle:ignore-file=syntax/parse-error\nfunc broken( -> void:\n\tpass\n";
    assert!(
        !linter::lint_source(source, "example.gd", &Config::default())
            .iter()
            .any(|diagnostic| diagnostic.rule == "syntax/parse-error")
    );
}
