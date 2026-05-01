---
name: highlighter-tests
description: >
  How to write highlighting tests for zsh-patina. Use this skill when adding,
  updating, or reviewing tests in src/highlighting/highlighter.rs. Covers the
  test infrastructure (TestCfg helpers, span types), the workflow for
  discovering expected span values, and how to map raw color numbers to scope
  constants.
---

# Writing highlighting tests for zsh-patina

## Test infrastructure

Tests live in the `mod tests` block at the bottom of
`src/highlighting/highlighter.rs`. A `TestCfg` is obtained via `test_cfg()`
and gives you:

- `cfg.highlight(command)` — highlight a command string with the default
  `HighlightingRequest` (uses a real temp directory as `$PWD`).
- `cfg.highlight_with_request(command, request)` — highlight with a custom
  request (e.g. to test `with_history_expansions(false)` or `with_cursor(n)`).
- `cfg.touch_file(name)` / `cfg.touch_script(name)` / `cfg.create_dir(name)` —
  create real files/directories inside `cfg.tempdir` so dynamic path
  highlighting can find them.
- `cfg.static_span(start, end, SCOPE_CONST)` — build a span backed by the
  test theme's static style for the given scope. Returns `Result<Span>`, so
  use `?`.
- `cfg.dynamic_span(start, end, "parsed_callable")` — build a
  `SpanStyle::Dynamic(Callable { parsed_callable })` span (no `?` needed).
- `cfg.mixed_span(start, end, SCOPE_A, SCOPE_B)` — merge two static styles
  (used when a span carries both a static scope and a dynamic path/callable
  overlay).

## Span indices

Indices are character positions into the original command string, *not* byte
offsets. Be careful with multi-byte characters (emoji, Unicode escapes).

## The three span kinds

| Helper | When to use |
|---|---|
| `dynamic_span` | The callable itself (command name), looked up at runtime |
| `static_span` | Any statically-scoped token (flag, string, operator, …) |
| `mixed_span` | A token that carries both a static scope and a dynamic overlay (e.g. a path argument that also exists on disk) |

## Scope constants

Scope constants are declared in `src/highlighting/mod.rs`. Most are
`#[cfg(test)]`. Always use the named constant rather than a string literal in
assertions.

Frequently used constants and their scopes:

| Constant | Scope |
|---|---|
| `ARGUMENTS` | `meta.function-call.arguments.shell` |
| `PARAMETER` | `variable.parameter.option.shell` |
| `PUNCTUATION_PARAMETER` | `punctuation.definition.parameter.shell` |
| `OPERATOR_END_OF_OPTIONS` | `keyword.operator.end-of-options.shell` |
| `OPERATOR_ASSIGNMENT` | `keyword.operator.assignment.shell` |
| `VAR_ASSIGN_NAME` | `variable.other.readwrite.assignment.shell` |
| `STRING_UNQUOTED` | `string.unquoted.shell` |
| `STRING_QUOTED_BEGIN` | `punctuation.definition.string.begin.shell` |
| `STRING_QUOTED_END` | `punctuation.definition.string.end.shell` |
| `STRING_QUOTED_SINGLE` | `string.quoted.single.shell` |
| `STRING_QUOTED_DOUBLE` | `string.quoted.double.shell` |
| `DYNAMIC_PATH_FILE_COMPLETE` | `dynamic.path.file.complete.shell` |
| `DYNAMIC_PATH_DIRECTORY_COMPLETE` | `dynamic.path.directory.complete.shell` |
| `DYNAMIC_CALLABLE_COMMAND` | `dynamic.callable.command.shell` |

If a test needs a scope that has no constant yet, add one in
`src/highlighting/mod.rs` (guarded by `#[cfg(test)]`). Sort them alphabetically.

## Workflow for writing a new test

### 1. Add a skeleton

Write the test function with `assert_eq!(highlighted, highlighted)` (a
no-op) as a placeholder for each case. This lets the test compile and run
without failing.

```rust
#[test]
fn my_command() -> Result<()> {
    let cfg = test_cfg()?;

    let highlighted = cfg.highlight("my-command -x foo")?;
    assert_eq!(highlighted, highlighted);

    Ok(())
}
```

### 2. Capture actual output

Replace each placeholder assertion with a `eprintln!` probe and a self-equal
assertion, then run the test with `--nocapture` to see the actual `Span`
values:

```rust
eprintln!("my-command -x foo: {highlighted:#?}");
assert_eq!(highlighted, highlighted);
```

```sh
cargo test highlighting::highlighter::tests::my_command -- --nocapture
```

### 3. Map color numbers to scope constants

The output contains raw color numbers (e.g. `foreground_color: Some("166")`).
Look them up in the test theme:

```sh
grep '"166"' target/debug/build/zsh-patina-*/out/test_theme.toml
# => "variable.parameter.option.shell" = 166
```

The test theme file is generated at build time and lives under
`target/debug/build/zsh-patina-<hash>/out/test_theme.toml`.

Map the scope string to the corresponding constant in
`src/highlighting/mod.rs` (see the table above). Add a new constant if none
exists.

### 4. Fill in real assertions

Replace the `eprintln!` probes with the proper `assert_eq!` calls using the
named constants, then run `cargo test` to confirm everything passes.

## Testing dynamic path highlighting

When a test case involves a file or directory argument that should trigger
dynamic path highlighting, always create a real file/directory in the temp
folder rather than using absolute system paths (which may not exist on all
machines):

```rust
cfg.touch_file("test.txt")?;      // file — use DYNAMIC_PATH_FILE_COMPLETE
cfg.create_dir("mydir")?;          // directory — use DYNAMIC_PATH_DIRECTORY_COMPLETE
cfg.touch_script("script.sh")?;    // executable — use DYNAMIC_CALLABLE_COMMAND
```

Pair an *existing* path case (which produces a `mixed_span`) with a
*non-existing* path case (which produces a plain `static_span` with
`ARGUMENTS` or similar) if necessary and if it does not produce an excessive number of tests that are almost similar (weigh benefits against drawbacks, test coverage vs. readability/conciseness):

```rust
// existing → mixed_span
let highlighted = cfg.highlight("env -C mydir ls")?;
assert_eq!(highlighted, vec![
    cfg.dynamic_span(0, 3, "env"),
    cfg.static_span(3, 5, PUNCTUATION_PARAMETER)?,
    cfg.static_span(5, 6, PARAMETER)?,
    cfg.mixed_span(7, 12, ARGUMENTS, DYNAMIC_PATH_DIRECTORY_COMPLETE)?,
    cfg.dynamic_span(13, 15, "ls"),
]);

// non-existing → plain ARGUMENTS
let highlighted = cfg.highlight("env -C foobar ls")?;
assert_eq!(highlighted, vec![
    cfg.dynamic_span(0, 3, "env"),
    cfg.static_span(3, 5, PUNCTUATION_PARAMETER)?,
    cfg.static_span(5, 6, PARAMETER)?,
    cfg.static_span(7, 13, ARGUMENTS)?,
    cfg.dynamic_span(14, 16, "ls"),
]);
```
