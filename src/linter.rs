use crate::ast::ScriptFile;
use crate::config::Config;
use crate::diagnostic::Diagnostic;
use crate::lexer::Lexer;
use crate::parser::Parser;
use crate::rules;
use crate::syntax;
use crate::token::TokenKind;

/// Lint a single GDScript source string and return diagnostics.
///
/// # Example
///
/// ```
/// use gdstyle::{config::Config, linter};
///
/// let diagnostics = linter::lint_source("func BadName():\n\tpass\n", "demo.gd", &Config::default());
/// assert!(diagnostics.iter().any(|d| d.rule == "naming/function-name-snake-case"));
/// ```
pub fn lint_source(source: &str, file_path: &str, config: &Config) -> Vec<Diagnostic> {
    // Normalize line endings up front. The rest of the pipeline mixes
    // `source.lines()` (strips `\r`) and `source.split('\n')` (keeps `\r`),
    // so on Windows-encoded files every byte offset disagrees by one per
    // line, autofix replacements then land mid-`\r\n` and trailing-
    // whitespace fixes leave the `\r` behind.
    let normalized = normalize_line_endings(source);
    let source = normalized.as_str();

    // Parse the full language once. Declaration-based rules use the
    // Tree-sitter projection below, and syntax diagnostics reuse this same
    // tree instead of paying for a second parse.
    let syntax_document = syntax::SyntaxDocument::parse(source);

    // Only actual comments in the authoritative tree may suppress diagnostics.
    // String contents can contain directive text without changing lint policy.
    let suppressed_lines =
        parse_suppression_comments_from_document(source, syntax_document.as_ref());

    // Tokenize.
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize();

    // Project declarations from the authoritative grammar. Retain the native
    // parser only as an effectively unreachable fallback for a cancelled
    // Tree-sitter parse; token-based rules and fixes still use the lexer.
    let members = syntax_document.as_ref().map_or_else(
        || {
            let mut parser = Parser::new(&tokens);
            parser.parse()
        },
        syntax::SyntaxDocument::class_members,
    );

    // Build the script file representation.
    // Use split('\n') instead of lines() to preserve trailing newline detection.
    // split('\n') on "hello\n" gives ["hello", ""], where the final "" signals the
    // trailing newline. The trailing-newline rule checks if the last element is empty.
    let lines: Vec<String> = source.split('\n').map(|l| l.to_string()).collect();
    let file = ScriptFile {
        path: file_path.to_string(),
        members,
        lines,
    };

    // Run all rules.
    let mut diagnostics = rules::run_all_rules_with_document(
        &file,
        &tokens,
        config,
        Some(source),
        syntax_document.as_ref(),
    );

    let parse_errors_enabled = config.is_rule_enabled("syntax/parse-error");

    // The legacy lexer is intentionally incomplete and can reject valid
    // constructs such as Unicode combining marks, raw-string backslashes, and
    // the magnitude of i64::MIN before its unary minus is applied. Keep its
    // diagnostics as a compatibility fallback only when the authoritative
    // Tree-sitter syntax rule is disabled.
    if config.is_rule_enabled("syntax/lex-error") && !parse_errors_enabled {
        for token in &tokens {
            if let crate::token::TokenKind::Error(ref message) = token.kind {
                diagnostics.push(Diagnostic::error(
                    "syntax/lex-error",
                    message.clone(),
                    token.span,
                    file_path,
                ));
            }
        }
    }

    // Run the complete Godot 4.7 Tree-sitter grammar in addition to gdstyle's
    // lightweight lexer/parser. The latter intentionally extracts only the
    // structure needed by lint rules, so it cannot validate the full language.
    // Tree-sitter reports recoverable syntax failures as ERROR or MISSING
    // nodes, allowing linting to continue and surface every useful diagnostic.
    if parse_errors_enabled {
        diagnostics.extend(syntax_document.as_ref().map_or_else(
            || syntax::parse_diagnostics(source, file_path),
            |document| document.diagnostics(file_path),
        ));
    }

    // Rules declare their natural severity at the diagnostic site. Apply
    // project configuration last so both warning→error and error→warning
    // overrides affect CLI exit status consistently.
    for diagnostic in &mut diagnostics {
        if let Some(severity) = config.severity_for(&diagnostic.rule, diagnostic.severity) {
            diagnostic.severity = severity;
        }
    }

    diagnostics.sort_by_key(|diagnostic| (diagnostic.span.line, diagnostic.span.column));

    // Filter out suppressed diagnostics.
    diagnostics.retain(|d| !is_suppressed(d, &suppressed_lines));

    diagnostics
}

/// Convert every `\r\n` outside string literals to `\n`. We do this before
/// any byte-offset computation so the rest of the pipeline can assume
/// LF-only line breaks between tokens.
///
/// Two things are left alone. A string that spans lines keeps its bytes,
/// `\r\n` included: Godot keeps a `\r` inside a string as part of its value,
/// and leaving it in place means no pass has to put it back. A `\r` that
/// isn't followed by `\n` stays too: Godot doesn't treat it as a line break
/// (it is part of a string or comment, or a "stray carriage return" error in
/// code), so turning it into `\n` would split the line.
///
/// # Example
///
/// ```
/// use gdstyle::linter::normalize_line_endings;
///
/// let source = "var a = 1\r\nvar s = \"\"\"x\r\ny\"\"\"\r\n";
/// assert_eq!(normalize_line_endings(source), "var a = 1\nvar s = \"\"\"x\r\ny\"\"\"\n");
/// ```
pub fn normalize_line_endings(source: &str) -> String {
    if !source.contains("\r\n") {
        return source.to_string();
    }
    map_outside_spanning_strings(source, |code| code.replace("\r\n", "\n"))
}

/// Put `source`'s line-ending style back on `normalized`: the output of
/// [`normalize_line_endings`] for `source`, after any edits.
///
/// The style comes from `source`'s line breaks outside string literals:
/// CRLF when most of them are `\r\n`, LF otherwise. It is applied to the line
/// breaks outside strings. Strings that span lines were never normalized, so
/// they keep their own line endings whatever the file's style is.
///
/// # Example
///
/// ```
/// use gdstyle::linter::{normalize_line_endings, restore_line_endings};
///
/// // A CRLF file whose string holds an LF line break.
/// let source = "var a = 1  \r\nvar s = \"\"\"x\ny\"\"\"\r\n";
/// let edited = normalize_line_endings(source).replace("1  ", "1");
/// assert_eq!(restore_line_endings(source, &edited), "var a = 1\r\nvar s = \"\"\"x\ny\"\"\"\r\n");
/// ```
pub fn restore_line_endings(source: &str, normalized: &str) -> String {
    if !source.contains("\r\n") {
        return normalized.to_string();
    }
    let mut crlf_breaks = 0;
    let mut lf_breaks = 0;
    map_outside_spanning_strings(source, |code| {
        crlf_breaks += code.matches("\r\n").count();
        lf_breaks += code.matches('\n').count();
        String::new()
    });
    // `lf_breaks` counted every `\n`, the CRLF ones included.
    if crlf_breaks * 2 <= lf_breaks {
        return normalized.to_string();
    }
    map_outside_spanning_strings(normalized, |code| code.replace('\n', "\r\n"))
}

/// `source` with `map` applied to each stretch of text between the string
/// literals that span lines. The literals themselves are copied unchanged.
fn map_outside_spanning_strings(source: &str, mut map: impl FnMut(&str) -> String) -> String {
    let mut mapped = String::with_capacity(source.len() + source.len() / 16);
    let mut cursor = 0;
    for token in Lexer::new(source).tokenize() {
        if !matches!(token.kind, TokenKind::String(_)) || !token.text.contains('\n') {
            continue;
        }
        let start = token.span.offset;
        mapped.push_str(&map(&source[cursor..start]));
        mapped.push_str(&token.text);
        cursor = start + token.span.length;
    }
    mapped.push_str(&map(&source[cursor..]));
    mapped
}

/// Lint a file from disk.
pub fn lint_file(path: &std::path::Path, config: &Config) -> Result<Vec<Diagnostic>, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {}", path.display(), e))?;

    let file_path = path.to_string_lossy().to_string();
    Ok(lint_source(&source, &file_path, config))
}

/// Per-line and per-file suppression state. For each scope, `None` in
/// the rule list means "suppress all diagnostics in this scope"; `Some`
/// means "suppress only these rules".
#[derive(Default)]
pub(crate) struct Suppressions {
    /// Per-line: indexed by line number so `is_suppressed` is O(1).
    per_line: std::collections::HashMap<usize, Vec<Option<Vec<String>>>>,
    /// Per-file: all `# gdstyle:ignore-file` directives merged together,
    /// regardless of where they appear in the source.
    file_level: Vec<Option<Vec<String>>>,
}

impl Suppressions {
    /// Parse every suppression directive out of `source`.
    pub(crate) fn parse(source: &str) -> Self {
        parse_suppression_comments(source)
    }

    /// True when `rule` is suppressed for a member whose header spans lines
    /// `first..=last` (1-based, inclusive) — that is, a `# gdstyle:ignore-file`
    /// covering the rule, or a per-line `# gdstyle:ignore` that resolves onto
    /// any line of that header. Used both to pin a member against reordering
    /// and to exempt it from the `order/class-member-order` check, so `fmt` and
    /// `check` agree on which members opt out.
    pub(crate) fn suppresses_member(&self, first: usize, last: usize, rule: &str) -> bool {
        if self.file_level.iter().any(|r| matches_rule_list(r, rule)) {
            return true;
        }
        (first..=last).any(|line| {
            self.per_line
                .get(&line)
                .is_some_and(|lists| lists.iter().any(|r| matches_rule_list(r, rule)))
        })
    }
}

/// Parse suppression comments. Two forms are recognised:
///
/// 1. `# gdstyle:ignore[=rule1,rule2]` — per-line scope. On its own line
///    it suppresses the NEXT line; placed at the end of a code line it
///    suppresses the SAME line.
/// 2. `# gdstyle:ignore-file[=rule1,rule2]` — per-file scope. Suppresses
///    the listed rules (or all rules, when no `=...` is given) for every
///    diagnostic in the file, regardless of line. Anywhere in the file
///    works; convention is at the top so it's visible to readers.
fn parse_suppression_comments(source: &str) -> Suppressions {
    let document = syntax::SyntaxDocument::parse(source);
    parse_suppression_comments_from_document(source, document.as_ref())
}

fn parse_suppression_comments_from_document(
    source: &str,
    document: Option<&syntax::SyntaxDocument<'_>>,
) -> Suppressions {
    let mut suppressions = Suppressions::default();
    let Some(document) = document else {
        return suppressions;
    };

    fn visit(node: tree_sitter::Node<'_>, source: &str, suppressions: &mut Suppressions) {
        if node.kind() == "comment" {
            let comment = source[node.byte_range()].trim_end();
            // File scope must come first: `ignore` is a prefix of `ignore-file`.
            if let Some(rest) = comment.strip_prefix("# gdstyle:ignore-file") {
                if directive_terminator_ok(rest) {
                    suppressions.file_level.push(parse_suppression_rules(rest));
                }
                return;
            }
            if let Some(rest) = comment.strip_prefix("# gdstyle:ignore") {
                if directive_terminator_ok(rest) {
                    let line_start = source[..node.start_byte()]
                        .rfind('\n')
                        .map_or(0, |index| index + 1);
                    let standalone = source[line_start..node.start_byte()].trim().is_empty();
                    let line = node.start_position().row + 1 + usize::from(standalone);
                    suppressions
                        .per_line
                        .entry(line)
                        .or_default()
                        .push(parse_suppression_rules(rest));
                }
            }
            return;
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            visit(child, source, suppressions);
        }
    }

    visit(document.root_node(), source, &mut suppressions);
    suppressions
}

/// After stripping the directive prefix, the remainder must either be
/// empty / pure whitespace / a comment continuation, or start with `=`.
/// Without this check `# gdstyle:ignore-foo` would also match the
/// `# gdstyle:ignore` prefix and be treated as a bare-suppression.
fn directive_terminator_ok(rest: &str) -> bool {
    match rest.chars().next() {
        None => true,
        Some(c) => c == '=' || c.is_whitespace(),
    }
}

fn parse_suppression_rules(rest: &str) -> Option<Vec<String>> {
    let rest = rest.trim();
    rest.strip_prefix('=').map(|rules_str| {
        rules_str
            .split(',')
            .map(|r| r.trim().to_string())
            .filter(|r| !r.is_empty())
            .collect()
    })
}

fn matches_rule_list(rules: &Option<Vec<String>>, rule: &str) -> bool {
    match rules {
        None => true, // bare directive suppresses everything
        Some(rs) => rs.iter().any(|r| r == rule),
    }
}

fn is_suppressed(diagnostic: &Diagnostic, suppressions: &Suppressions) -> bool {
    // File-level wins first: a `# gdstyle:ignore-file` at any line
    // covers diagnostics anywhere in the file. This is the right home
    // for class/file-scope limits like `quality/max-public-methods`,
    // which report against the class header itself and can't be
    // suppressed with a same-line comment without uglifying the
    // signature.
    if suppressions
        .file_level
        .iter()
        .any(|rules| matches_rule_list(rules, &diagnostic.rule))
    {
        return true;
    }
    let Some(line_suppressions) = suppressions.per_line.get(&diagnostic.span.line) else {
        return false;
    };
    line_suppressions
        .iter()
        .any(|rules| matches_rule_list(rules, &diagnostic.rule))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lint_clean_source() {
        let source = r#"class_name Player
extends CharacterBody2D

signal health_changed(old_value: int, new_value: int)

const MAX_SPEED: float = 200.0

@export var speed: float = 100.0

var health: int = 100

@onready var label: Label = $Label

func _ready() -> void:
	pass

func take_damage(amount: int) -> void:
	pass
"#;
        let config = Config::default();
        let diagnostics = lint_source(source, "player.gd", &config);
        assert!(
            diagnostics.is_empty(),
            "clean source should produce no diagnostics, got: {:?}",
            diagnostics
        );
    }

    #[test]
    fn lint_detects_bad_class_name() {
        let source = "class_name my_player\n";
        let config = Config::default();
        let diagnostics = lint_source(source, "test.gd", &config);
        assert!(diagnostics
            .iter()
            .any(|d| d.rule == "naming/class-name-pascal-case"));
    }

    #[test]
    fn lint_detects_bad_function_name() {
        let source = "func takeDamage() -> void:\n\tpass\n";
        let config = Config::default();
        let diagnostics = lint_source(source, "test.gd", &config);
        assert!(diagnostics
            .iter()
            .any(|d| d.rule == "naming/function-name-snake-case"));
    }

    #[test]
    fn lint_surfaces_unterminated_string_as_error() {
        let source = "var x = \"oops\nvar y = 5\n";
        let config = Config::default();
        let diagnostics = lint_source(source, "test.gd", &config);
        let parse_errors: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.rule == "syntax/parse-error")
            .collect();
        assert!(
            !parse_errors.is_empty(),
            "unterminated string must produce a syntax/parse-error diagnostic"
        );
        assert_eq!(parse_errors[0].severity, crate::diagnostic::Severity::Error);
    }

    #[test]
    fn legacy_lexer_errors_remain_available_as_a_fallback() {
        let source = "var x = \"oops\nvar y = 5\n";
        let mut config = Config::default();
        config.rules.insert(
            "syntax/parse-error".to_string(),
            crate::config::RuleSeverityConfig::Off,
        );
        let diagnostics = lint_source(source, "test.gd", &config);

        assert!(diagnostics
            .iter()
            .any(|diagnostic| diagnostic.rule == "syntax/lex-error"));
    }

    #[test]
    fn configured_rule_severity_changes_diagnostics() {
        let source = "func takeDamage() -> void:\n\tpass\n";
        let mut config = Config::default();
        config.rules.insert(
            "naming/function-name-snake-case".to_string(),
            crate::config::RuleSeverityConfig::Error,
        );
        let diagnostics = lint_source(source, "test.gd", &config);

        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.rule == "naming/function-name-snake-case")
            .expect("function naming diagnostic");
        assert_eq!(diagnostic.severity, crate::diagnostic::Severity::Error);
    }

    #[test]
    fn lint_detects_trailing_whitespace() {
        let source = "var x = 5   \n";
        let config = Config::default();
        let diagnostics = lint_source(source, "test.gd", &config);
        assert!(diagnostics
            .iter()
            .any(|d| d.rule == "format/trailing-whitespace"));
    }

    #[test]
    fn lint_detects_long_line() {
        let long_line = format!("var x = \"{}\"", "a".repeat(110));
        let source = format!("{}\n", long_line);
        let config = Config::default();
        let diagnostics = lint_source(&source, "test.gd", &config);
        assert!(diagnostics
            .iter()
            .any(|d| d.rule == "format/max-line-length"));
    }

    #[test]
    fn inline_suppression_works() {
        let source = "class_name my_player  # gdstyle:ignore=naming/class-name-pascal-case\n";
        let config = Config::default();
        let diagnostics = lint_source(source, "test.gd", &config);
        assert!(
            !diagnostics
                .iter()
                .any(|d| d.rule == "naming/class-name-pascal-case"),
            "suppressed rule should not appear"
        );
    }

    #[test]
    fn standalone_suppression_works() {
        let source = "# gdstyle:ignore=naming/class-name-pascal-case\nclass_name my_player\n";
        let config = Config::default();
        let diagnostics = lint_source(source, "test.gd", &config);
        assert!(
            !diagnostics
                .iter()
                .any(|d| d.rule == "naming/class-name-pascal-case"),
            "suppressed rule should not appear"
        );
    }

    /// Build a 25-public-method class for the file-scope tests below.
    /// We rebuild it inline because every test needs to start from a
    /// known-clean source — sharing a `const` across tests would make
    /// failures harder to trace.
    fn bloated_class(prelude: &str) -> String {
        let mut s = String::from(prelude);
        s.push_str("class_name Bloated\nextends Node\n");
        for i in 0..25 {
            s.push_str(&format!("\nfunc method_{}() -> void:\n\tpass\n", i));
        }
        s
    }

    #[test]
    fn file_suppression_silences_class_level_rule() {
        // Regression for the user-reported gap: a `# gdstyle:ignore`
        // standalone above the class header doesn't catch
        // `quality/max-public-methods` because the diagnostic anchors
        // at line 1 and the standalone form suppresses the NEXT line.
        // The file-level directive sidesteps that mismatch entirely.
        let source = bloated_class("# gdstyle:ignore-file=quality/max-public-methods\n");
        let config = Config::default();
        let diagnostics = lint_source(&source, "test.gd", &config);
        assert!(
            !diagnostics
                .iter()
                .any(|d| d.rule == "quality/max-public-methods"),
            "file-level suppression should silence max-public-methods, got: {:?}",
            diagnostics
        );
    }

    #[test]
    fn file_suppression_bare_form_silences_everything() {
        // `# gdstyle:ignore-file` (no `=...`) drops every diagnostic
        // in the file — for generated code or third-party drops.
        let source = "# gdstyle:ignore-file\nclass_name my_bad_name\nvar AlsoBad: int = 5\n";
        let config = Config::default();
        let diagnostics = lint_source(source, "test.gd", &config);
        assert!(
            diagnostics.is_empty(),
            "bare ignore-file should drop everything, got: {:?}",
            diagnostics
        );
    }

    #[test]
    fn file_suppression_anywhere_in_file() {
        // The directive doesn't have to live on line 1 — anywhere in
        // the file works. This matters for files that already start
        // with a class-level `##` docstring before the suppression.
        let mut source = String::from("## Docstring for Bloated.\n");
        source.push_str(&bloated_class(""));
        source.push_str("\n# gdstyle:ignore-file=quality/max-public-methods\n");
        let config = Config::default();
        let diagnostics = lint_source(&source, "test.gd", &config);
        assert!(
            !diagnostics
                .iter()
                .any(|d| d.rule == "quality/max-public-methods"),
            "file-level suppression must apply regardless of position, got: {:?}",
            diagnostics
        );
    }

    #[test]
    fn file_suppression_narrows_to_listed_rules() {
        // A scoped `# gdstyle:ignore-file=...` only silences the
        // listed rules — others still fire. The unlisted naming rule
        // on `BadName` must remain.
        let source = "# gdstyle:ignore-file=quality/max-public-methods\nclass_name Bloated\nextends Node\nvar BadName: int = 5\n";
        let config = Config::default();
        let diagnostics = lint_source(source, "test.gd", &config);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.rule == "naming/variable-name-snake-case"),
            "scoped ignore-file should NOT silence unlisted rules, got: {:?}",
            diagnostics
        );
    }

    #[test]
    fn file_suppression_does_not_swallow_per_line_directive() {
        // Regression guard: the `ignore-file` parsing path runs before
        // `ignore` and `continue`s when it matches. Make sure a
        // regular per-line `# gdstyle:ignore` directive still routes
        // through the per-line path.
        let source = "# gdstyle:ignore=naming/class-name-pascal-case\nclass_name my_player\n";
        let config = Config::default();
        let diagnostics = lint_source(source, "test.gd", &config);
        assert!(
            !diagnostics
                .iter()
                .any(|d| d.rule == "naming/class-name-pascal-case"),
            "per-line directive still works alongside ignore-file support"
        );
    }

    #[test]
    fn suppress_all_rules_on_line() {
        let source = "# gdstyle:ignore\nclass_name my_player\n";
        let config = Config::default();
        let diagnostics = lint_source(source, "test.gd", &config);
        // All diagnostics on line 2 should be suppressed.
        let line_2_diags: Vec<_> = diagnostics.iter().filter(|d| d.span.line == 2).collect();
        assert!(line_2_diags.is_empty());
    }

    #[test]
    fn config_disables_rule() {
        let source = "class_name my_player\n";
        let mut config = Config::default();
        config.rules.insert(
            "naming/class-name-pascal-case".to_string(),
            crate::config::RuleSeverityConfig::Off,
        );
        let diagnostics = lint_source(source, "test.gd", &config);
        assert!(!diagnostics
            .iter()
            .any(|d| d.rule == "naming/class-name-pascal-case"));
    }

    #[test]
    fn lint_real_world_script() {
        let source = r#"@tool
class_name StateMachine
extends Node
## Hierarchical state machine for the player.
##
## Initializes states and delegates engine callbacks to the state.

signal state_changed(previous: String, current: String)

@export var initial_state: Node

var is_active: bool = true

@onready var _state: Node = $State

func _init() -> void:
	add_to_group("state_machine")

func _ready() -> void:
	state_changed.connect(_on_state_changed)

func _physics_process(delta: float) -> void:
	_state._physics_process(delta)

func transition_to(target_path: String) -> void:
	pass

func _on_state_changed(previous: String, current: String) -> void:
	pass
"#;
        let config = Config::default();
        let diagnostics = lint_source(source, "state_machine.gd", &config);
        assert!(
            diagnostics.is_empty(),
            "real-world clean script should have no issues, got: {:?}",
            diagnostics
        );
    }
}
