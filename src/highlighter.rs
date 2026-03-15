use std::{
    ops::Range,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use syntect::{
    easy::HighlightLines,
    highlighting::{Style, Theme as SyntectTheme},
    parsing::{ClearAmount, ParseState, ScopeStackOp, SyntaxSet},
    util::LinesWithEndings,
};

use crate::{
    HighlightingConfig,
    theme::{ScopeMapping, Theme, ThemeSource},
};

/// A span of text with a foreground color. The range is specified in terms of
/// character indices, not byte indices.
pub struct Span {
    /// The starting character index of the span (inclusive)
    pub start: usize,

    /// The ending character index of the span (exclusive)
    pub end: usize,

    /// The foreground color of the span
    pub foreground_color: String,

    /// The background color of the span
    pub background_color: Option<String>,

    /// `true` if the text should be shown in bold
    pub bold: bool,

    /// `true` if the text should be shown underlined
    pub underline: bool,
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

pub struct Highlighter {
    max_line_length: usize,
    timeout: Duration,
    syntax_set: SyntaxSet,
    syntect_theme: SyntectTheme,
    theme: Theme,
    scope_mapping: ScopeMapping,
}

impl Highlighter {
    pub fn new(config: &HighlightingConfig) -> Result<Self> {
        let syntax_set: SyntaxSet = syntect::dumps::from_uncompressed_data(include_bytes!(
            concat!(env!("OUT_DIR"), "/syntax_set.packdump")
        ))
        .expect("Unable to load shell syntax");

        let theme = Theme::load(&config.theme)?;
        let scope_mapping = ScopeMapping::new(&theme);

        Ok(Self {
            max_line_length: config.max_line_length,
            timeout: config.timeout,
            syntax_set,
            syntect_theme: theme.to_syntect(&scope_mapping).with_context(|| {
                match &config.theme {
                    ThemeSource::Simple => "Failed to parse simple theme".to_string(),
                    ThemeSource::Patina => "Failed to parse default theme".to_string(),
                    ThemeSource::Lavender => "Failed to parse lavender theme".to_string(),
                    ThemeSource::TokyoNight => "Failed to parse tokyonight theme".to_string(),
                    ThemeSource::File(path) => format!("Failed to parse theme file `{path}'"),
                }
            })?,
            theme,
            scope_mapping,
        })
    }

    pub fn highlight(&self, command: &str) -> Result<Vec<Span>> {
        if let Some(rest) = find_prefix_split(command) {
            let mut spans = self.highlight_internal(&command[0..rest])?;
            spans.extend(self.highlight(&command[rest..])?.into_iter().map(|mut s| {
                s.start += rest;
                s.end += rest;
                s
            }));
            Ok(spans)
        } else {
            self.highlight_internal(command)
        }
    }

    fn highlight_internal(&self, command: &str) -> Result<Vec<Span>> {
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

            let ranges: Vec<(Style, &str)> = h.highlight_line(line, &self.syntax_set)?;

            for r in ranges {
                let style = self
                    .scope_mapping
                    .decode(&r.0.foreground, &self.theme)
                    .unwrap_or_default();

                let fg = style
                    .foreground
                    .map(|c| c.to_ansi_color())
                    .unwrap_or_else(|| "white".to_string());
                let bg = style.background.map(|c| c.to_ansi_color());

                // this is O(n) but necessary in case the command contains
                // multi-byte characters
                let len = r.1.chars().count();

                // highlighting `None` or `white` (i.e. default terminal color)
                // is not necessary
                if fg != "white" || bg.is_some() || style.bold || style.underline {
                    result.push(Span {
                        start: i,
                        end: i + len,
                        foreground_color: fg,
                        background_color: bg,
                        bold: style.bold,
                        underline: style.underline,
                    });
                }

                i += len;
            }
        }

        Ok(result)
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
