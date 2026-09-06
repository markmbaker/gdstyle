## gdstyle 0.3.0

This is the first release of the
[markmbaker/gdstyle](https://github.com/markmbaker/gdstyle) fork. It combines
gdstyle's formatter, fixer, rule set, CLI, and Godot plugin with the improved
[markmbaker/tree-sitter-gdscript](https://github.com/markmbaker/tree-sitter-gdscript)
grammar.

### Tree-sitter integration

- Tree-sitter is authoritative for GDScript syntax validation. Valid modern
  Godot syntax is no longer rejected by limitations in gdstyle's legacy lexer.
- Declaration-based lint rules consume a class-member projection built from
  the concrete syntax tree. The legacy declaration parser remains only as a
  cancellation fallback.
- The formatter and rename fixer reuse Tree-sitter declarations while keeping
  gdstyle's token stream for comments, whitespace, and precise replacements.
- Formatting and autofixes are syntax guarded: gdstyle will not turn valid
  input into invalid GDScript.

### Fixes and compatibility

- Configured diagnostic severity overrides now apply consistently.
- `fmt --check` reports the number of files that would actually change rather
  than the number scanned.
- Enum documentation comments remain attached to their declarations during
  formatting.
- The grammar is pinned to an exact fork commit for reproducible builds.
- The implementation was checked against 314 valid Godot parser/analyzer/runtime
  fixtures and 1,139 GDScript files from Triad, with no syntax false positives.

### Install

Until the release tag exists, install the development branch:

```bash
cargo install --git https://github.com/markmbaker/gdstyle.git \
  --branch codex/tree-sitter-gdscript
```

For a tagged release, download a pre-built CLI archive or
`gdstyle-godot-plugin.zip` from the release page. The fork intentionally does
not publish under upstream's `gdstyle` crates.io name.

For [pre-commit](https://pre-commit.com), use:

```yaml
- repo: https://github.com/markmbaker/gdstyle
  rev: v0.3.0
  hooks:
    - id: gdstyle
    - id: gdstyle-fmt
```
