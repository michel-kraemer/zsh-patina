use std::{
    ops::Range,
    time::{Duration, Instant},
};

use anyhow::Result;
use syntect::{
    easy::HighlightLines,
    highlighting::{Color, Style, Theme},
    parsing::{ClearAmount, ParseState, ScopeStackOp, SyntaxSet},
    util::LinesWithEndings,
};

fn to_hex(color: Color) -> String {
    format!("#{:0>2x}{:0>2x}{:0>2x}", color.r, color.g, color.b)
}

/// This is similar to how the ansi theme works in Bat
/// (https://github.com/sharkdp/bat): Colors are specified in the form #RRGGBBAA
/// where AA can have the following values:
///
/// * 00: The red channel specifies which ANSI color to use. Valid values are
///   00-07 (black, red, green, yellow, blue, magenta, cyan, white in this
///   order).
/// * 01: In this case the terminal's default foreground color is used
/// * else: the color is used as-is without the alpha channel (i.e. #RRGGBB)
fn to_ansi_color(color: Color) -> Option<String> {
    if color.a == 0 {
        Some(match color.r {
            0x00 => "black".to_string(),
            0x01 => "red".to_string(),
            0x02 => "green".to_string(),
            0x03 => "yellow".to_string(),
            0x04 => "blue".to_string(),
            0x05 => "magenta".to_string(),
            0x06 => "cyan".to_string(),
            0x07 => "white".to_string(),
            _ => to_hex(color),
        })
    } else if color.a == 1 {
        None
    } else {
        Some(to_hex(color))
    }
}

/// A span of text with a foreground color. The range is specified in terms of
/// character indices, not byte indices.
pub struct Span {
    /// The starting character index of the span (inclusive)
    pub start: usize,

    /// The ending character index of the span (exclusive)
    pub end: usize,

    /// The foreground color of the span
    pub foreground_color: String,
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
    theme: Theme,
}

impl Highlighter {
    pub fn new(max_line_length: usize, timeout: Duration) -> Self {
        let syntax_set: SyntaxSet = syntect::dumps::from_uncompressed_data(include_bytes!(
            concat!(env!("OUT_DIR"), "/syntax_set.packdump")
        ))
        .expect("Unable to load shell syntax");

        let theme: Theme = syntect::dumps::from_reader(
            &include_bytes!(concat!(env!("OUT_DIR"), "/theme.themedump"))[..],
        )
        .expect("Unable to load theme");

        Self {
            max_line_length,
            timeout,
            syntax_set,
            theme,
        }
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

        let mut h = HighlightLines::new(syntax, &self.theme);
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
                let fg = to_ansi_color(r.0.foreground);

                // this is O(n) but necessary in case the command contains
                // multi-byte characters
                let len = r.1.chars().count();

                // highlighting `None` or `white` (i.e. default terminal color)
                // is not necessary
                if let Some(fg) = fg
                    && fg != "white"
                {
                    result.push(Span {
                        start: i,
                        end: i + len,
                        foreground_color: fg,
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
