use std::{
    ops::Range,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use rustc_hash::FxHashMap;
use syntect::{
    easy::HighlightLines,
    highlighting::Theme as SyntectTheme,
    parsing::{ClearAmount, ParseState, ScopeStackOp, SyntaxSet},
    util::LinesWithEndings,
};

use crate::{
    HighlightingConfig,
    path::{PathType, is_path_executable, path_type},
    theme::{ScopeMapping, Style, Theme, ThemeSource},
};

const ARGUMENTS: &str = "meta.function-call.arguments.shell";
const DYNAMIC_PATH_DIRECTORY: &str = "dynamic.path.directory.shell";
const DYNAMIC_PATH_FILE: &str = "dynamic.path.file.shell";

const CALLABLE: &str = "variable.function.shell";
const DYNAMIC_CALLABLE_ALIAS: &str = "dynamic.callable.alias.shell";
const DYNAMIC_CALLABLE_BUILTIN: &str = "dynamic.callable.builtin.shell";
const DYNAMIC_CALLABLE_COMMAND: &str = "dynamic.callable.command.shell";
const DYNAMIC_CALLABLE_FUNCTION: &str = "dynamic.callable.function.shell";
const DYNAMIC_CALLABLE_MISSING: &str = "dynamic.callable.missing.shell";

/// A span of text with a foreground color. The range is specified in terms of
/// character indices, not byte indices.
#[derive(PartialEq, Eq, Debug)]
pub struct Span {
    /// The starting character index of the span (inclusive)
    pub start: usize,

    /// The ending character index of the span (exclusive)
    pub end: usize,

    /// The span's style
    pub style: SpanStyle,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct StaticStyle {
    /// The foreground color of the span
    pub foreground_color: String,

    /// The background color of the span
    pub background_color: Option<String>,

    /// `true` if the text should be shown in bold
    pub bold: bool,

    /// `true` if the text should be shown underlined
    pub underline: bool,
}

#[derive(PartialEq, Eq, Debug)]
pub enum DynamicStyle {
    Callable,
}

#[derive(PartialEq, Eq, Debug)]
pub enum SpanStyle {
    Static(StaticStyle),
    Dynamic(DynamicStyle),
}

/// A token with a scope, line and column number, and range in the input command
/// (byte indices). The line and column numbers are 1-based.
pub struct Token {
    /// The scope of the token (e.g. `keyword.control.for.shell`)
    pub scope: String,

    /// The line number of the token (1-based)
    pub line: usize,

    /// The column of the token (1-based)
    pub column: usize,

    /// The range of the token in the input command (byte indices)
    pub range: Range<usize>,
}

/// If the command starts with a prefix keyword (e.g. `time`), returns the byte
/// offset where the rest of the command begins. This can be used to split the
/// command and process the prefix and the rest separately.
fn find_prefix_split(command: &str) -> Option<usize> {
    if command.trim_ascii_start().starts_with("time ") {
        Some(command.find("time ").unwrap() + 5)
    } else {
        None
    }
}

/// Lookup a scope in a theme and convert the retrieved style to a
/// [`StaticStyle`] struct
fn resolve_static_style(scope: &str, theme: &Theme) -> Option<StaticStyle> {
    let style = theme.resolve(scope)?;

    let fg = style
        .foreground
        .map(|c| c.to_ansi_color())
        .unwrap_or_else(|| "white".to_string());
    let bg = style.background.map(|c| c.to_ansi_color());

    // highlighting `white` (i.e. default terminal color) is not necessary
    if fg == "white" && bg.is_none() && !style.bold && !style.underline {
        None
    } else {
        Some(StaticStyle {
            foreground_color: fg,
            background_color: bg,
            bold: style.bold,
            underline: style.underline,
        })
    }
}

pub struct Highlighter {
    max_line_length: usize,
    timeout: Duration,
    syntax_set: SyntaxSet,
    theme: Theme,
    scope_mapping: ScopeMapping,
    syntect_theme: SyntectTheme,
    callable_choices: Vec<(String, StaticStyle)>,
}

impl Highlighter {
    pub fn new(config: &HighlightingConfig) -> Result<Self> {
        let syntax_set: SyntaxSet = syntect::dumps::from_uncompressed_data(include_bytes!(
            concat!(env!("OUT_DIR"), "/syntax_set.packdump")
        ))
        .expect("Unable to load shell syntax");

        let mut theme = Theme::load(&config.theme)?;

        // Insert dummy style for callables into the theme. We need it as a
        // marker so Syntect returns a token for it.
        if !theme.contains(CALLABLE) {
            if let Some(callable_style) = theme.resolve(CALLABLE) {
                // Try to fallback to the style for callables so our dynamic
                // style gets a valid `else` option.
                theme.insert(CALLABLE.to_string(), callable_style);
            } else {
                // It doesn't matter what we insert as a fallback. It will be
                // overwritten by our dynamic style later anyhow.
                theme.insert(CALLABLE.to_string(), Style::default());
            }
        }

        // Insert dummy style for arguments into the theme
        if !theme.contains(ARGUMENTS) {
            theme.insert(ARGUMENTS.to_string(), Style::default());
        }

        let scope_mapping = ScopeMapping::new(&theme);

        let syntect_theme =
            theme
                .to_syntect(&scope_mapping)
                .with_context(|| match &config.theme {
                    ThemeSource::Simple => "Failed to parse simple theme".to_string(),
                    ThemeSource::Patina => "Failed to parse default theme".to_string(),
                    ThemeSource::Lavender => "Failed to parse lavender theme".to_string(),
                    ThemeSource::TokyoNight => "Failed to parse tokyonight theme".to_string(),
                    ThemeSource::File(path) => format!("Failed to parse theme file `{path}'"),
                })?;

        let mut callable_choices: FxHashMap<StaticStyle, String> = FxHashMap::default();
        if let Some(alias_style) = resolve_static_style(DYNAMIC_CALLABLE_ALIAS, &theme) {
            callable_choices.entry(alias_style).or_default().push('a');
        }
        if let Some(builtin_style) = resolve_static_style(DYNAMIC_CALLABLE_BUILTIN, &theme) {
            callable_choices.entry(builtin_style).or_default().push('b');
        }
        if let Some(command_style) = resolve_static_style(DYNAMIC_CALLABLE_COMMAND, &theme) {
            callable_choices.entry(command_style).or_default().push('c');
        }
        if let Some(function_style) = resolve_static_style(DYNAMIC_CALLABLE_FUNCTION, &theme) {
            callable_choices
                .entry(function_style)
                .or_default()
                .push('f');
        }
        if let Some(missing_style) = resolve_static_style(DYNAMIC_CALLABLE_MISSING, &theme) {
            callable_choices.entry(missing_style).or_default().push('m');
        }
        if let Some(else_style) = resolve_static_style(CALLABLE, &theme) {
            callable_choices.entry(else_style).or_default().push('e');
        }
        let callable_choices = callable_choices
            .into_iter()
            .map(|(k, v)| (v, k))
            .collect::<Vec<_>>();

        Ok(Self {
            max_line_length: config.max_line_length,
            timeout: config.timeout,
            syntax_set,
            theme,
            scope_mapping,
            syntect_theme,
            callable_choices,
        })
    }

    /// Return a list of dynamic style choices the plugin has for callables
    pub fn callable_choices(&self) -> &[(String, StaticStyle)] {
        &self.callable_choices
    }

    pub fn highlight(&self, command: &str, pwd: Option<&str>) -> Result<Vec<Span>> {
        if let Some(rest) = find_prefix_split(command) {
            let mut spans = self.highlight_internal(&command[0..rest], pwd)?;
            spans.extend(
                self.highlight(&command[rest..], pwd)?
                    .into_iter()
                    .map(|mut s| {
                        s.start += rest;
                        s.end += rest;
                        s
                    }),
            );
            Ok(spans)
        } else {
            self.highlight_internal(command, pwd)
        }
    }

    fn highlight_internal(&self, command: &str, pwd: Option<&str>) -> Result<Vec<Span>> {
        let start = Instant::now();

        let syntax = self.syntax_set.find_syntax_by_extension("sh").unwrap();

        let mut h = HighlightLines::new(syntax, &self.syntect_theme);
        let mut i = 0;
        let mut result = Vec::new();
        for line in LinesWithEndings::from(command.trim_ascii_end()) {
            if line.len() > self.max_line_length {
                // skip lines that are too long
                continue;
            }

            if start.elapsed() > self.timeout {
                // stop if highlighting takes too long
                break;
            }

            let ranges = h.highlight_line(line, &self.syntax_set)?;

            for r in ranges {
                // this is O(n) but necessary in case the command contains
                // multi-byte characters
                let len = r.1.chars().count();

                if let Some(scope) = self.scope_mapping.decode(&r.0.foreground) {
                    self.highlight_scope(r.1, i..i + len, scope, pwd, &mut result);
                }

                i += len;
            }
        }

        Ok(result)
    }

    fn highlight_scope(
        &self,
        token: &str,
        range: Range<usize>,
        scope: &str,
        pwd: Option<&str>,
        result: &mut Vec<Span>,
    ) {
        match scope {
            ARGUMENTS => self.highlight_arguments(token, range, scope, pwd, result),
            CALLABLE => self.highlight_callable(token, range, pwd, result),
            _ => self.highlight_other(range, scope, result),
        }
    }

    fn highlight_arguments(
        &self,
        token: &str,
        range: Range<usize>,
        scope: &str,
        pwd: Option<&str>,
        result: &mut Vec<Span>,
    ) {
        // highlighting argument is only necessary (and possible) if we have a
        // current working directory
        let Some(pwd) = pwd else {
            // fallback to static styling
            if let Some(style) = resolve_static_style(scope, &self.theme) {
                result.push(Span {
                    start: range.start,
                    end: range.end,
                    style: SpanStyle::Static(style),
                });
            }
            return;
        };

        // split the current token into sub-tokens of consecutive whitespaces or
        // consecutive non-whitespaces
        let mut start = 0;
        let bytes = token.as_bytes();
        while start < bytes.len() {
            let is_whitespace = bytes[start].is_ascii_whitespace();
            let end = bytes[start..]
                .iter()
                .position(|b| b.is_ascii_whitespace() != is_whitespace)
                .map_or(bytes.len(), |p| start + p);

            let style = if !is_whitespace && let Some(t) = path_type(&token[start..end], pwd) {
                // every non-whitespace sub-token that is a path should be
                // highlighted with the dynamic path style
                let dynamic_scope = match t {
                    PathType::File => DYNAMIC_PATH_FILE,
                    PathType::Directory => DYNAMIC_PATH_DIRECTORY,
                };
                resolve_static_style(dynamic_scope, &self.theme)
            } else {
                None
            };

            let style = style.or_else(|| {
                // fallback to the normal style for this token
                resolve_static_style(scope, &self.theme)
            });

            if let Some(style) = style {
                result.push(Span {
                    start: range.start + start,
                    end: range.start + end,
                    style: SpanStyle::Static(style),
                });
            }

            start = end;
        }
    }

    fn highlight_callable(
        &self,
        token: &str,
        range: Range<usize>,
        pwd: Option<&str>,
        result: &mut Vec<Span>,
    ) {
        let style = if let Some(pwd) = pwd
            && token.contains('/')
            && is_path_executable(token, pwd)
        {
            // We have a current working directory and the token is a path to an
            // executable. Highlight it as a command if this style is available
            // or fall back to the static style for callables.
            if let Some(style) = resolve_static_style(DYNAMIC_CALLABLE_COMMAND, &self.theme) {
                Some(SpanStyle::Static(style))
            } else {
                resolve_static_style(CALLABLE, &self.theme).map(SpanStyle::Static)
            }
        } else {
            // highlight the token as a dynamic callable and let the Zsh client
            // script decide whether it is an alias, a builtin, a command, or a
            // function
            Some(SpanStyle::Dynamic(DynamicStyle::Callable))
        };

        if let Some(style) = style {
            result.push(Span {
                start: range.start,
                end: range.end,
                style,
            });
        }
    }

    fn highlight_other(&self, range: Range<usize>, scope: &str, result: &mut Vec<Span>) {
        if let Some(style) = resolve_static_style(scope, &self.theme) {
            result.push(Span {
                start: range.start,
                end: range.end,
                style: SpanStyle::Static(style),
            });
        }
    }

    pub fn tokenize(&self, command: &str) -> Result<Vec<Token>> {
        if let Some(rest) = find_prefix_split(command) {
            let mut tokens = self.tokenize_internal(&command[0..rest])?;
            tokens.extend(self.tokenize(&command[rest..])?.into_iter().map(|mut t| {
                if t.line == 1 {
                    t.column += rest;
                }
                t.range = (t.range.start + rest)..(t.range.end + rest);
                t
            }));
            Ok(tokens)
        } else {
            self.tokenize_internal(command)
        }
    }

    fn tokenize_internal(&self, command: &str) -> Result<Vec<Token>> {
        let syntax = self.syntax_set.find_syntax_by_extension("sh").unwrap();

        let mut offset = 0;
        let mut ps = ParseState::new(syntax);
        let mut result = Vec::new();
        let mut stack = Vec::new();
        let mut stash = Vec::new();
        for (line_number, line) in LinesWithEndings::from(command.trim_ascii_end()).enumerate() {
            let tokens = ps.parse_line(line, &self.syntax_set)?;

            for (i, s) in tokens {
                match s {
                    ScopeStackOp::Push(scope) => {
                        stack.push((
                            scope,
                            line_number + 1,
                            line[0..i].chars().count() + 1,
                            offset + i,
                        ));
                    }

                    ScopeStackOp::Pop(count) => {
                        for _ in 0..count {
                            let (scope, ln, col, start) = stack.pop().unwrap();
                            if offset + i >= start {
                                result.push(Token {
                                    scope: scope.build_string(),
                                    line: ln,
                                    column: col,
                                    range: start..offset + i,
                                });
                            }
                        }
                    }

                    ScopeStackOp::Clear(clear_amount) => {
                        // similar to ::Pop, but store popped items in stash so
                        // we can restore them if necessary
                        let count = match clear_amount {
                            ClearAmount::TopN(n) => n.min(stack.len()),
                            ClearAmount::All => stack.len(),
                        };

                        let mut to_stash = Vec::new();
                        for _ in 0..count {
                            let (scope, ln, col, start) = stack.pop().unwrap();
                            if offset + i >= start {
                                result.push(Token {
                                    scope: scope.build_string(),
                                    line: ln,
                                    column: col,
                                    range: start..offset + i,
                                });
                            }
                            to_stash.push((scope, ln, col, start));
                        }
                        stash.push(to_stash);
                    }

                    ScopeStackOp::Restore => {
                        // restore items from the stash (see ::Clear)
                        if let Some(mut s) = stash.pop() {
                            while let Some(e) = s.pop() {
                                stack.push(e);
                            }
                        }
                    }

                    ScopeStackOp::Noop => {}
                }
            }

            offset += line.len();
        }

        // consume the remaining items on the stack
        while let Some((scope, ln, col, start)) = stack.pop() {
            result.push(Token {
                scope: scope.build_string(),
                line: ln,
                column: col,
                range: start..command.len(),
            });
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use anyhow::Result;

    fn test_config() -> HighlightingConfig {
        HighlightingConfig::default()
    }

    /// Test if a simple `echo` command is highlighted correctly
    #[test]
    fn echo() -> Result<()> {
        let highlighter = Highlighter::new(&test_config())?;
        let highlighted = highlighter.highlight("echo", None)?;
        assert_eq!(
            highlighted,
            vec![Span {
                start: 0,
                end: 4,
                style: SpanStyle::Dynamic(DynamicStyle::Callable)
            }]
        );
        Ok(())
    }

    /// Test if a command referring to a file is highlighted correctly
    #[test]
    fn argument_is_file() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let test_path = dir.path().join("test.txt");
        fs::write(test_path, "test contents")?;

        let highlighter = Highlighter::new(&test_config())?;
        let highlighted = highlighter.highlight(
            r#"cp test.txt dest.txt"#,
            Some(dir.path().to_str().unwrap()),
        )?;

        let dynamic_file_style =
            resolve_static_style(DYNAMIC_PATH_FILE, &highlighter.theme).unwrap();

        assert_eq!(
            highlighted,
            vec![
                Span {
                    start: 0,
                    end: 2,
                    style: SpanStyle::Dynamic(DynamicStyle::Callable)
                },
                Span {
                    start: 3,
                    end: 11,
                    style: SpanStyle::Static(dynamic_file_style),
                }
            ]
        );

        Ok(())
    }

    /// Test if a command referring to a directory is highlighted correctly
    #[test]
    fn argument_is_directory() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let test_path = dir.path().join("test.txt");
        fs::write(test_path, "test contents")?;
        let dest_path = dir.path().join("dest");
        fs::create_dir(dest_path)?;

        let highlighter = Highlighter::new(&test_config())?;
        let highlighted =
            highlighter.highlight(r#"cp test.txt dest"#, Some(dir.path().to_str().unwrap()))?;

        let dynamic_file_style =
            resolve_static_style(DYNAMIC_PATH_FILE, &highlighter.theme).unwrap();
        let dynamic_directory_style =
            resolve_static_style(DYNAMIC_PATH_DIRECTORY, &highlighter.theme).unwrap();

        assert_eq!(
            highlighted,
            vec![
                Span {
                    start: 0,
                    end: 2,
                    style: SpanStyle::Dynamic(DynamicStyle::Callable)
                },
                Span {
                    start: 3,
                    end: 11,
                    style: SpanStyle::Static(dynamic_file_style),
                },
                Span {
                    start: 12,
                    end: 16,
                    style: SpanStyle::Static(dynamic_directory_style),
                }
            ]
        );

        Ok(())
    }
}
