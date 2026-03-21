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

use super::dynamic::*;
use super::*;
use crate::{
    HighlightingConfig,
    theme::{ScopeMapping, Style, Theme, ThemeSource},
};

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

fn insert_marker_style(theme: &mut Theme, scope: &str) {
    if !theme.contains(scope) {
        if let Some(style) = theme.resolve(scope) {
            theme.insert(scope.to_string(), style);
        } else {
            theme.insert(scope.to_string(), Style::default());
        }
    }
}

fn insert_marker_style_with_fallback(theme: &mut Theme, scope: &str, fallbacks: &[&str]) {
    if !theme.contains(scope) {
        for f in fallbacks {
            if let Some(style) = theme.resolve(f) {
                theme.insert(scope.to_string(), style);
                return;
            }
        }
        theme.insert(scope.to_string(), Style::default());
    }
}

pub fn update_groups(
    scope: &str,
    range: &Range<usize>,
    byte_range: &Range<usize>,
    groups: &mut Vec<DynamicTokenGroup>,
) {
    let Ok(dynamic_scope) = DynamicScope::try_from(scope) else {
        return;
    };

    // try to extend last group
    let dynamic_type = match scope {
        ARGUMENTS
        | STRING_QUOTED_SINGLE_ARGUMENTS
        | STRING_QUOTED_SINGLE_ANSI_ARGUMENTS
        | STRING_QUOTED_DOUBLE_ARGUMENTS
        | STRING_QUOTED_BEGIN_ARGUMENTS
        | STRING_QUOTED_END_ARGUMENTS
        | CHARACTER_ESCAPE_ARGUMENTS
        | TILDE_ARGUMENTS => {
            if let Some(group) = groups.last_mut()
                && group.range.end == range.start
                && group.dynamic_type != DynamicType::Callable
            {
                group.range.end = range.end;
                group.byte_range.end = byte_range.end;
                group.dynamic_type = DynamicType::Arguments;
                group
                    .tokens
                    .push(DynamicToken::new(range, byte_range, dynamic_scope));
                return;
            } else {
                DynamicType::Arguments
            }
        }

        CALLABLE
        | STRING_QUOTED_SINGLE_CALLABLE
        | STRING_QUOTED_SINGLE_ANSI_CALLABLE
        | STRING_QUOTED_DOUBLE_CALLABLE
        | STRING_QUOTED_BEGIN_CALLABLE
        | STRING_QUOTED_END_CALLABLE
        | TILDE_CALLABLE => {
            if let Some(group) = groups.last_mut()
                && group.range.end == range.start
                && group.dynamic_type != DynamicType::Arguments
            {
                group.range.end = range.end;
                group.byte_range.end = byte_range.end;
                group.dynamic_type = DynamicType::Callable;
                group
                    .tokens
                    .push(DynamicToken::new(range, byte_range, dynamic_scope));
                return;
            } else {
                DynamicType::Callable
            }
        }

        // parser cannot differentiate between normal unquoted character escapes
        // and those that are at the beginning of a callable
        CHARACTER_ESCAPE => {
            if let Some(group) = groups.last_mut()
                && group.range.end == range.start
            {
                group.range.end = range.end;
                group.byte_range.end = byte_range.end;
                group
                    .tokens
                    .push(DynamicToken::new(range, byte_range, dynamic_scope));
                return;
            } else {
                DynamicType::Unknown
            }
        }

        // this can only happen if we're inside an ANSI-quoted string, so we
        // already know if the group is a command or arguments
        CHARACTER_ESCAPE_QUOTED_ANSI => {
            if let Some(group) = groups.last_mut()
                && group.range.end == range.start
            {
                group.range.end = range.end;
                group.byte_range.end = byte_range.end;
                group
                    .tokens
                    .push(DynamicToken::new(range, byte_range, dynamic_scope));
            }
            return;
        }

        _ => return,
    };

    // create new group
    groups.push(DynamicTokenGroup::new(
        range,
        byte_range,
        dynamic_type,
        dynamic_scope,
    ));
}

fn mix_spans(base: Vec<Span>, mixins: Vec<Span>) -> Vec<Span> {
    // collect all boundary positions where the active state changes
    let mut positions = Vec::new();
    for s in base.iter().chain(mixins.iter()) {
        positions.push(s.start);
        positions.push(s.end);
    }
    positions.sort_unstable();
    positions.dedup();

    let mut result = Vec::new();
    let mut bi = 0;
    let mut mi = 0;

    for w in positions.windows(2) {
        let (lo, hi) = (w[0], w[1]);

        // advance past spans that end at or before lo
        while bi < base.len() && base[bi].end <= lo {
            bi += 1;
        }
        while mi < mixins.len() && mixins[mi].end <= lo {
            mi += 1;
        }

        let active_base = base.get(bi).filter(|s| s.start <= lo && hi <= s.end);
        let active_mixin = mixins.get(mi).filter(|s| s.start <= lo && hi <= s.end);

        let style = match (active_base, active_mixin) {
            (Some(b), Some(m)) => Some(mix_styles(&b.style, &m.style)),
            (Some(b), None) => Some(b.style.clone()),
            (None, Some(m)) => Some(m.style.clone()),
            (None, None) => None,
        };

        if let Some(style) = style {
            // merge with previous span if styles match
            if let Some(last) = result.last_mut() {
                let last: &mut Span = last;
                if last.end == lo && last.style == style {
                    last.end = hi;
                    continue;
                }
            }
            result.push(Span {
                start: lo,
                end: hi,
                style,
            });
        }
    }

    result
}

fn mix_styles(base: &SpanStyle, mixin: &SpanStyle) -> SpanStyle {
    match (base, mixin) {
        (SpanStyle::Static(b), SpanStyle::Static(m)) => SpanStyle::Static(StaticStyle {
            foreground_color: if m.foreground_color.is_some() {
                m.foreground_color.clone()
            } else {
                b.foreground_color.clone()
            },
            background_color: if m.background_color.is_some() {
                m.background_color.clone()
            } else {
                b.background_color.clone()
            },
            bold: if m.bold { true } else { b.bold },
            underline: if m.underline { true } else { b.underline },
        }),

        (_, SpanStyle::Dynamic(m)) => SpanStyle::Dynamic(m.clone()),

        // this should actually be unreachable since base should always only
        // contain static span styles
        (SpanStyle::Dynamic(_), SpanStyle::Static(m)) => SpanStyle::Static(m.clone()),
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
        insert_marker_style(&mut theme, CALLABLE);

        // Do the same for other scopes
        insert_marker_style(&mut theme, ARGUMENTS);
        insert_marker_style(&mut theme, CHARACTER_ESCAPE);
        insert_marker_style_with_fallback(
            &mut theme,
            CHARACTER_ESCAPE_ARGUMENTS,
            &[CHARACTER_ESCAPE],
        );
        insert_marker_style_with_fallback(
            &mut theme,
            CHARACTER_ESCAPE_QUOTED_ANSI,
            &[CHARACTER_ESCAPE],
        );
        insert_marker_style_with_fallback(&mut theme, TILDE_ARGUMENTS, &[TILDE]);
        insert_marker_style_with_fallback(&mut theme, TILDE_CALLABLE, &[TILDE]);
        insert_marker_style_with_fallback(
            &mut theme,
            STRING_QUOTED_SINGLE_CALLABLE,
            &[STRING_QUOTED_SINGLE],
        );
        insert_marker_style_with_fallback(
            &mut theme,
            STRING_QUOTED_SINGLE_ARGUMENTS,
            &[STRING_QUOTED_SINGLE],
        );
        insert_marker_style_with_fallback(
            &mut theme,
            STRING_QUOTED_SINGLE_ANSI_CALLABLE,
            &[STRING_QUOTED_SINGLE_ANSI],
        );
        insert_marker_style_with_fallback(
            &mut theme,
            STRING_QUOTED_SINGLE_ANSI_ARGUMENTS,
            &[STRING_QUOTED_SINGLE_ANSI],
        );
        insert_marker_style_with_fallback(
            &mut theme,
            STRING_QUOTED_DOUBLE_CALLABLE,
            &[STRING_QUOTED_DOUBLE],
        );
        insert_marker_style_with_fallback(
            &mut theme,
            STRING_QUOTED_DOUBLE_ARGUMENTS,
            &[STRING_QUOTED_DOUBLE],
        );
        insert_marker_style_with_fallback(
            &mut theme,
            STRING_QUOTED_BEGIN_CALLABLE,
            &[STRING_QUOTED_BEGIN, STRING_QUOTED_DOUBLE],
        );
        insert_marker_style_with_fallback(
            &mut theme,
            STRING_QUOTED_BEGIN_ARGUMENTS,
            &[STRING_QUOTED_BEGIN, STRING_QUOTED_DOUBLE],
        );
        insert_marker_style_with_fallback(
            &mut theme,
            STRING_QUOTED_END_CALLABLE,
            &[STRING_QUOTED_END, STRING_QUOTED_DOUBLE],
        );
        insert_marker_style_with_fallback(
            &mut theme,
            STRING_QUOTED_END_ARGUMENTS,
            &[STRING_QUOTED_END, STRING_QUOTED_DOUBLE],
        );

        let scope_mapping = ScopeMapping::new(&theme);

        let syntect_theme =
            theme
                .to_syntect(&scope_mapping)
                .with_context(|| match &config.theme {
                    ThemeSource::Lavender => "Failed to parse lavender theme".to_string(),
                    ThemeSource::Nord => "Failed to parse nord theme".to_string(),
                    ThemeSource::Patina => "Failed to parse default theme".to_string(),
                    ThemeSource::Simple => "Failed to parse simple theme".to_string(),
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

    pub fn highlight<P>(&self, command: &str, pwd: Option<&str>, predicate: P) -> Result<Vec<Span>>
    where
        P: Fn(&Range<usize>) -> bool + Copy,
    {
        if let Some(rest) = find_prefix_split(command) {
            let mut spans = self.highlight_internal(&command[0..rest], pwd, predicate)?;
            spans.extend(
                self.highlight(&command[rest..], pwd, predicate)?
                    .into_iter()
                    .map(|mut s| {
                        s.start += rest;
                        s.end += rest;
                        s
                    }),
            );
            Ok(spans)
        } else {
            self.highlight_internal(command, pwd, predicate)
        }
    }

    fn highlight_internal<P>(
        &self,
        command: &str,
        pwd: Option<&str>,
        predicate: P,
    ) -> Result<Vec<Span>>
    where
        P: Fn(&Range<usize>) -> bool,
    {
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

            let mut groups = Vec::new();

            let mut bi = 0;
            for r in ranges {
                if r.1.is_empty() {
                    continue;
                }

                // this is O(n) but necessary in case the command contains
                // multi-byte characters
                let len = r.1.chars().count();

                if let Some(scope) = self.scope_mapping.decode(&r.0.foreground) {
                    let range = i..i + len;
                    let brange = bi..bi + r.1.len();
                    if predicate(&range) {
                        update_groups(scope, &range, &brange, &mut groups);
                        self.highlight_other(range, scope, &mut result);
                    }
                }

                i += len;
                bi += r.1.len();
            }

            // highlight all groups
            if let Some(pwd) = pwd {
                let mut mixins = Vec::new();
                for g in groups {
                    if let Ok(group_spans) = g.highlight(line, pwd, &self.theme) {
                        mixins.extend(group_spans);
                    }
                }

                // mix into result
                if !mixins.is_empty() {
                    result = mix_spans(result, mixins);
                }
            }
        }

        Ok(result)
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
    use std::{
        fs::{self, Permissions},
        os::unix::fs::PermissionsExt,
    };

    use super::*;
    use anyhow::Result;
    use pretty_assertions::assert_eq;

    fn test_config() -> HighlightingConfig {
        HighlightingConfig::default()
    }

    /// Test if a simple `echo` command is highlighted correctly
    #[test]
    fn echo() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let pwd = Some(dir.path().to_str().unwrap());
        let highlighter = Highlighter::new(&test_config())?;
        let highlighted = highlighter.highlight("echo", pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![Span {
                start: 0,
                end: 4,
                style: SpanStyle::Dynamic(DynamicStyle::Callable {
                    parsed_callable: "echo".to_string()
                })
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
        let test_path1 = dir.path().join("test 1.txt");
        fs::write(test_path1, "test contents")?;
        let pwd = Some(dir.path().to_str().unwrap());

        let highlighter = Highlighter::new(&test_config())?;
        let dynamic_file_style =
            resolve_static_style(DYNAMIC_PATH_FILE, &highlighter.theme).unwrap();
        let string_style = resolve_static_style(STRING_QUOTED_DOUBLE, &highlighter.theme).unwrap();
        let escape_style = resolve_static_style(CHARACTER_ESCAPE, &highlighter.theme).unwrap();
        let dynamic_string_file_style = mix_styles(
            &SpanStyle::Static(string_style.clone()),
            &SpanStyle::Static(dynamic_file_style.clone()),
        );
        let dynamic_escape_file_style = mix_styles(
            &SpanStyle::Static(escape_style.clone()),
            &SpanStyle::Static(dynamic_file_style.clone()),
        );

        let highlighted = highlighter.highlight("cp test.txt dest.txt", pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![
                Span {
                    start: 0,
                    end: 2,
                    style: SpanStyle::Dynamic(DynamicStyle::Callable {
                        parsed_callable: "cp".to_string()
                    })
                },
                Span {
                    start: 3,
                    end: 11,
                    style: SpanStyle::Static(dynamic_file_style.clone()),
                }
            ]
        );

        let highlighted = highlighter.highlight(r#"cp "test.txt" dest.txt"#, pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![
                Span {
                    start: 0,
                    end: 2,
                    style: SpanStyle::Dynamic(DynamicStyle::Callable {
                        parsed_callable: "cp".to_string()
                    })
                },
                Span {
                    start: 3,
                    end: 13,
                    style: dynamic_string_file_style.clone(),
                }
            ]
        );

        let highlighted =
            highlighter.highlight(r#"cp   "test.txt"   "dest.txt""#, pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![
                Span {
                    start: 0,
                    end: 2,
                    style: SpanStyle::Dynamic(DynamicStyle::Callable {
                        parsed_callable: "cp".to_string()
                    })
                },
                Span {
                    start: 5,
                    end: 15,
                    style: dynamic_string_file_style.clone(),
                },
                Span {
                    start: 18,
                    end: 28,
                    style: SpanStyle::Static(string_style.clone()),
                }
            ]
        );

        let highlighted = highlighter.highlight(r#"cp " test.txt" "dest.txt""#, pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![
                Span {
                    start: 0,
                    end: 2,
                    style: SpanStyle::Dynamic(DynamicStyle::Callable {
                        parsed_callable: "cp".to_string()
                    })
                },
                Span {
                    start: 3,
                    end: 14,
                    style: SpanStyle::Static(string_style.clone()),
                },
                Span {
                    start: 15,
                    end: 25,
                    style: SpanStyle::Static(string_style.clone()),
                }
            ]
        );

        let highlighted = highlighter.highlight(r#"cp te"st.tx"t dest.txt"#, pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![
                Span {
                    start: 0,
                    end: 2,
                    style: SpanStyle::Dynamic(DynamicStyle::Callable {
                        parsed_callable: "cp".to_string()
                    })
                },
                Span {
                    start: 3,
                    end: 5,
                    style: SpanStyle::Static(dynamic_file_style.clone()),
                },
                Span {
                    start: 5,
                    end: 12,
                    style: dynamic_string_file_style.clone(),
                },
                Span {
                    start: 12,
                    end: 13,
                    style: SpanStyle::Static(dynamic_file_style.clone()),
                }
            ]
        );

        let highlighted = highlighter.highlight(r#"cp "test 1.txt" dest.txt"#, pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![
                Span {
                    start: 0,
                    end: 2,
                    style: SpanStyle::Dynamic(DynamicStyle::Callable {
                        parsed_callable: "cp".to_string()
                    })
                },
                Span {
                    start: 3,
                    end: 15,
                    style: dynamic_string_file_style.clone(),
                },
            ]
        );

        let highlighted = highlighter.highlight(r#"cp test\ 1.txt dest.txt"#, pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![
                Span {
                    start: 0,
                    end: 2,
                    style: SpanStyle::Dynamic(DynamicStyle::Callable {
                        parsed_callable: "cp".to_string()
                    })
                },
                Span {
                    start: 3,
                    end: 7,
                    style: SpanStyle::Static(dynamic_file_style.clone()),
                },
                Span {
                    start: 7,
                    end: 9,
                    style: dynamic_escape_file_style.clone(),
                },
                Span {
                    start: 9,
                    end: 14,
                    style: SpanStyle::Static(dynamic_file_style.clone()),
                },
            ]
        );

        let highlighted = highlighter.highlight(r#"cp 'test.txt' dest.txt"#, pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![
                Span {
                    start: 0,
                    end: 2,
                    style: SpanStyle::Dynamic(DynamicStyle::Callable {
                        parsed_callable: "cp".to_string()
                    })
                },
                Span {
                    start: 3,
                    end: 13,
                    style: dynamic_string_file_style.clone(),
                }
            ]
        );

        let highlighted = highlighter.highlight(r#"cp $'test.txt' dest.txt"#, pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![
                Span {
                    start: 0,
                    end: 2,
                    style: SpanStyle::Dynamic(DynamicStyle::Callable {
                        parsed_callable: "cp".to_string()
                    })
                },
                Span {
                    start: 3,
                    end: 14,
                    style: dynamic_string_file_style.clone(),
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
        let pwd = Some(dir.path().to_str().unwrap());

        let highlighter = Highlighter::new(&test_config())?;
        let highlighted = highlighter.highlight("cp test.txt dest", pwd, |_| true)?;

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
                    style: SpanStyle::Dynamic(DynamicStyle::Callable {
                        parsed_callable: "cp".to_string()
                    })
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

    /// Test if a command starting with a tilde is highlighted correctly
    #[test]
    fn command_with_tilde() -> Result<()> {
        let home = std::env::var("HOME").unwrap();
        let dir = tempfile::tempdir()?;
        let pwd = Some(dir.path().to_str().unwrap());

        let highlighter = Highlighter::new(&test_config())?;
        let dynamic_command_style =
            resolve_static_style(DYNAMIC_CALLABLE_COMMAND, &highlighter.theme).unwrap();

        let highlighted = highlighter.highlight("~", pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![Span {
                start: 0,
                end: 1,
                style: SpanStyle::Dynamic(DynamicStyle::Callable {
                    parsed_callable: home.clone()
                })
            }]
        );

        let highlighted = highlighter.highlight("~/", pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![Span {
                start: 0,
                end: 2,
                style: SpanStyle::Static(dynamic_command_style.clone())
            }]
        );

        let highlighted = highlighter.highlight("~ echo", pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![Span {
                start: 0,
                end: 1,
                style: SpanStyle::Dynamic(DynamicStyle::Callable {
                    parsed_callable: home.clone()
                })
            }]
        );

        let highlighted = highlighter.highlight("~doesnotexist", pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![Span {
                start: 0,
                end: 13,
                style: SpanStyle::Dynamic(DynamicStyle::Callable {
                    parsed_callable: "~doesnotexist".to_string()
                })
            }]
        );

        let highlighted = highlighter.highlight(r#""~""#, pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![Span {
                start: 0,
                end: 3,
                style: SpanStyle::Dynamic(DynamicStyle::Callable {
                    parsed_callable: "~".to_string()
                })
            }]
        );

        let highlighted = highlighter.highlight(r#""~/""#, pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![Span {
                start: 0,
                end: 4,
                style: SpanStyle::Dynamic(DynamicStyle::Callable {
                    parsed_callable: "~/".to_string()
                })
            }]
        );

        Ok(())
    }

    /// Test if a path starting with a tilde is highlighted correctly
    #[test]
    fn path_with_tilde() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let pwd = Some(dir.path().to_str().unwrap());

        let highlighter = Highlighter::new(&test_config())?;
        let tilde_style = resolve_static_style(TILDE, &highlighter.theme).unwrap();
        let string_style = resolve_static_style(STRING_QUOTED_DOUBLE, &highlighter.theme).unwrap();
        let dynamic_directory_style =
            resolve_static_style(DYNAMIC_PATH_DIRECTORY, &highlighter.theme).unwrap();
        let dynamic_tilde_directory_style = mix_styles(
            &SpanStyle::Static(tilde_style.clone()),
            &SpanStyle::Static(dynamic_directory_style.clone()),
        );

        let highlighted = highlighter.highlight("ls ~", pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![
                Span {
                    start: 0,
                    end: 2,
                    style: SpanStyle::Dynamic(DynamicStyle::Callable {
                        parsed_callable: "ls".to_string()
                    })
                },
                Span {
                    start: 3,
                    end: 4,
                    style: dynamic_tilde_directory_style.clone()
                }
            ]
        );

        let highlighted = highlighter.highlight("ls ~/", pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![
                Span {
                    start: 0,
                    end: 2,
                    style: SpanStyle::Dynamic(DynamicStyle::Callable {
                        parsed_callable: "ls".to_string()
                    })
                },
                Span {
                    start: 3,
                    end: 4,
                    style: dynamic_tilde_directory_style.clone()
                },
                Span {
                    start: 4,
                    end: 5,
                    style: SpanStyle::Static(dynamic_directory_style)
                }
            ]
        );

        let highlighted = highlighter.highlight(r#"ls "~/""#, pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![
                Span {
                    start: 0,
                    end: 2,
                    style: SpanStyle::Dynamic(DynamicStyle::Callable {
                        parsed_callable: "ls".to_string()
                    })
                },
                Span {
                    start: 3,
                    end: 7,
                    style: SpanStyle::Static(string_style.clone())
                }
            ]
        );

        let highlighted = highlighter.highlight("ls '~/'", pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![
                Span {
                    start: 0,
                    end: 2,
                    style: SpanStyle::Dynamic(DynamicStyle::Callable {
                        parsed_callable: "ls".to_string()
                    })
                },
                Span {
                    start: 3,
                    end: 7,
                    style: SpanStyle::Static(string_style.clone())
                }
            ]
        );

        let highlighted = highlighter.highlight("ls $'~/'", pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![
                Span {
                    start: 0,
                    end: 2,
                    style: SpanStyle::Dynamic(DynamicStyle::Callable {
                        parsed_callable: "ls".to_string()
                    })
                },
                Span {
                    start: 3,
                    end: 8,
                    style: SpanStyle::Static(string_style.clone())
                }
            ]
        );

        let highlighted = highlighter.highlight("ls ~/this/path/does/not/exist", pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![
                Span {
                    start: 0,
                    end: 2,
                    style: SpanStyle::Dynamic(DynamicStyle::Callable {
                        parsed_callable: "ls".to_string()
                    })
                },
                Span {
                    start: 3,
                    end: 4,
                    style: SpanStyle::Static(tilde_style.clone())
                }
            ]
        );

        let highlighted = highlighter.highlight("ls test/~/", pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![
                Span {
                    start: 0,
                    end: 2,
                    style: SpanStyle::Dynamic(DynamicStyle::Callable {
                        parsed_callable: "ls".to_string()
                    })
                },
                Span {
                    start: 8,
                    end: 9,
                    style: SpanStyle::Static(tilde_style.clone())
                }
            ]
        );

        Ok(())
    }

    #[test]
    fn double_quoted_callable() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let pwd = Some(dir.path().to_str().unwrap());

        let highlighter = Highlighter::new(&test_config())?;

        let highlighted = highlighter.highlight("\"ls\"", pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![Span {
                start: 0,
                end: 4,
                style: SpanStyle::Dynamic(DynamicStyle::Callable {
                    parsed_callable: "ls".to_string()
                })
            }]
        );

        let highlighted = highlighter.highlight("l\"s\"", pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![Span {
                start: 0,
                end: 4,
                style: SpanStyle::Dynamic(DynamicStyle::Callable {
                    parsed_callable: "ls".to_string()
                })
            }]
        );

        let file_path = dir.path().join("script.sh");
        fs::write(&file_path, "#!/bin/sh")?;
        fs::set_permissions(&file_path, Permissions::from_mode(0o755))?;

        let dynamic_callable_style =
            resolve_static_style(DYNAMIC_CALLABLE_COMMAND, &highlighter.theme).unwrap();

        let highlighted = highlighter.highlight("\"./script.sh\"", pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![Span {
                start: 0,
                end: 13,
                style: SpanStyle::Static(dynamic_callable_style.clone())
            }]
        );

        let directory_path = dir.path().join("foo/bar");
        fs::create_dir_all(&directory_path)?;

        let highlighted = highlighter.highlight("foo/\"bar\"/", pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![Span {
                start: 0,
                end: 10,
                style: SpanStyle::Static(dynamic_callable_style)
            }]
        );

        Ok(())
    }

    #[test]
    fn escape_unquoted_at_beginning() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let test_path = dir.path().join("test.txt");
        fs::write(test_path, "test contents")?;
        let script_path = dir.path().join("script.sh");
        fs::write(&script_path, "#!/bin/sh")?;
        fs::set_permissions(&script_path, Permissions::from_mode(0o755))?;
        let s_path = dir.path().join("s");
        fs::write(&s_path, "#!/bin/sh")?;
        fs::set_permissions(&s_path, Permissions::from_mode(0o755))?;
        let pwd = Some(dir.path().to_str().unwrap());

        let highlighter = Highlighter::new(&test_config())?;
        let escape_style = resolve_static_style(CHARACTER_ESCAPE, &highlighter.theme).unwrap();
        let dynamic_callable_style =
            resolve_static_style(DYNAMIC_CALLABLE_COMMAND, &highlighter.theme).unwrap();
        let dynamic_file_style =
            resolve_static_style(DYNAMIC_PATH_FILE, &highlighter.theme).unwrap();
        let dynamic_escape_file_style = mix_styles(
            &SpanStyle::Static(escape_style.clone()),
            &SpanStyle::Static(dynamic_file_style.clone()),
        );

        let highlighted = highlighter.highlight(r"\script.sh", pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![Span {
                start: 0,
                end: 10,
                style: SpanStyle::Dynamic(DynamicStyle::Callable {
                    parsed_callable: "script.sh".to_string()
                })
            }]
        );

        let highlighted = highlighter.highlight(r"\./script.sh", pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![Span {
                start: 0,
                end: 12,
                style: SpanStyle::Static(dynamic_callable_style.clone())
            }]
        );

        // parser cannot differentiate between normal unquoted character escapes
        // and those that are at the beginning of a callable
        let highlighted = highlighter.highlight(r"\s", pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![Span {
                start: 0,
                end: 2,
                style: SpanStyle::Static(escape_style.clone())
            }]
        );

        let highlighted = highlighter.highlight(r"touch \test.txt", pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![
                Span {
                    start: 0,
                    end: 5,
                    style: SpanStyle::Dynamic(DynamicStyle::Callable {
                        parsed_callable: "touch".to_string()
                    })
                },
                Span {
                    start: 6,
                    end: 8,
                    style: dynamic_escape_file_style.clone()
                },
                Span {
                    start: 8,
                    end: 15,
                    style: SpanStyle::Static(dynamic_file_style.clone())
                }
            ]
        );

        Ok(())
    }

    #[test]
    fn path_with_escape_unquoted() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let pwd = Some(dir.path().to_str().unwrap());

        let highlighter = Highlighter::new(&test_config())?;
        let dynamic_file_style =
            resolve_static_style(DYNAMIC_PATH_FILE, &highlighter.theme).unwrap();
        let escape_style = resolve_static_style(CHARACTER_ESCAPE, &highlighter.theme).unwrap();
        let dynamic_escape_file_style = mix_styles(
            &SpanStyle::Static(escape_style.clone()),
            &SpanStyle::Static(dynamic_file_style.clone()),
        );

        let highlighted = highlighter.highlight(r"cp test\u2580.txt dest.txt", pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![
                Span {
                    start: 0,
                    end: 2,
                    style: SpanStyle::Dynamic(DynamicStyle::Callable {
                        parsed_callable: "cp".to_string()
                    })
                },
                Span {
                    start: 7,
                    end: 9,
                    style: SpanStyle::Static(escape_style.clone())
                }
            ]
        );

        let test_path = dir.path().join("testu2580.txt");
        fs::write(test_path, "test contents")?;

        let highlighted = highlighter.highlight(r"cp test\u2580.txt dest.txt", pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![
                Span {
                    start: 0,
                    end: 2,
                    style: SpanStyle::Dynamic(DynamicStyle::Callable {
                        parsed_callable: "cp".to_string()
                    })
                },
                Span {
                    start: 3,
                    end: 7,
                    style: SpanStyle::Static(dynamic_file_style.clone())
                },
                Span {
                    start: 7,
                    end: 9,
                    style: dynamic_escape_file_style.clone()
                },
                Span {
                    start: 9,
                    end: 17,
                    style: SpanStyle::Static(dynamic_file_style.clone())
                }
            ]
        );

        Ok(())
    }

    #[test]
    fn path_with_escape_quoted() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let test_path = dir.path().join("test▀.txt");
        fs::write(test_path, "test contents")?;
        let test_path1 = dir.path().join("test  1.txt");
        fs::write(test_path1, "test contents")?;
        let pwd = Some(dir.path().to_str().unwrap());

        let highlighter = Highlighter::new(&test_config())?;
        let dynamic_file_style =
            resolve_static_style(DYNAMIC_PATH_FILE, &highlighter.theme).unwrap();
        let string_style = resolve_static_style(STRING_QUOTED_DOUBLE, &highlighter.theme).unwrap();
        let escape_style = resolve_static_style(CHARACTER_ESCAPE, &highlighter.theme).unwrap();
        let dynamic_string_file_style = mix_styles(
            &SpanStyle::Static(string_style.clone()),
            &SpanStyle::Static(dynamic_file_style.clone()),
        );
        let dynamic_escape_file_style = mix_styles(
            &SpanStyle::Static(escape_style.clone()),
            &SpanStyle::Static(dynamic_file_style.clone()),
        );

        let highlighted = highlighter.highlight(r"cp test\u2580.txt dest.txt", pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![
                Span {
                    start: 0,
                    end: 2,
                    style: SpanStyle::Dynamic(DynamicStyle::Callable {
                        parsed_callable: "cp".to_string()
                    })
                },
                Span {
                    start: 7,
                    end: 9,
                    style: SpanStyle::Static(escape_style.clone())
                }
            ]
        );

        let highlighted =
            highlighter.highlight(r#"cp "test\u2580.txt" dest.txt"#, pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![
                Span {
                    start: 0,
                    end: 2,
                    style: SpanStyle::Dynamic(DynamicStyle::Callable {
                        parsed_callable: "cp".to_string()
                    })
                },
                Span {
                    start: 3,
                    end: 19,
                    style: SpanStyle::Static(string_style.clone())
                }
            ]
        );

        let highlighted =
            highlighter.highlight(r#"cp 'test\u2580.txt' dest.txt"#, pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![
                Span {
                    start: 0,
                    end: 2,
                    style: SpanStyle::Dynamic(DynamicStyle::Callable {
                        parsed_callable: "cp".to_string()
                    })
                },
                Span {
                    start: 3,
                    end: 19,
                    style: SpanStyle::Static(string_style.clone())
                }
            ]
        );

        let highlighted =
            highlighter.highlight(r#"cp $'test\u2580.txt' dest.txt"#, pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![
                Span {
                    start: 0,
                    end: 2,
                    style: SpanStyle::Dynamic(DynamicStyle::Callable {
                        parsed_callable: "cp".to_string()
                    })
                },
                Span {
                    start: 3,
                    end: 9,
                    style: dynamic_string_file_style.clone(),
                },
                Span {
                    start: 9,
                    end: 15,
                    style: dynamic_escape_file_style.clone()
                },
                Span {
                    start: 15,
                    end: 20,
                    style: dynamic_string_file_style.clone(),
                }
            ]
        );

        let highlighted = highlighter.highlight(r#"cp test\ \ 1.txt dest.txt"#, pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![
                Span {
                    start: 0,
                    end: 2,
                    style: SpanStyle::Dynamic(DynamicStyle::Callable {
                        parsed_callable: "cp".to_string()
                    })
                },
                Span {
                    start: 3,
                    end: 7,
                    style: SpanStyle::Static(dynamic_file_style.clone())
                },
                Span {
                    start: 7,
                    end: 11,
                    style: dynamic_escape_file_style.clone()
                },
                Span {
                    start: 11,
                    end: 16,
                    style: SpanStyle::Static(dynamic_file_style.clone())
                }
            ]
        );

        let highlighted = highlighter.highlight(r#"cp "test\ \ 1.txt" dest.txt"#, pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![
                Span {
                    start: 0,
                    end: 2,
                    style: SpanStyle::Dynamic(DynamicStyle::Callable {
                        parsed_callable: "cp".to_string()
                    })
                },
                Span {
                    start: 3,
                    end: 18,
                    style: SpanStyle::Static(string_style.clone())
                }
            ]
        );

        let highlighted =
            highlighter.highlight(r#"cp $'test\ \ 1.txt' dest.txt"#, pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![
                Span {
                    start: 0,
                    end: 2,
                    style: SpanStyle::Dynamic(DynamicStyle::Callable {
                        parsed_callable: "cp".to_string()
                    })
                },
                Span {
                    start: 3,
                    end: 19,
                    style: SpanStyle::Static(string_style.clone())
                }
            ]
        );

        let highlighted =
            highlighter.highlight(r#"cp $'test\x20\x201.txt' dest.txt"#, pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![
                Span {
                    start: 0,
                    end: 2,
                    style: SpanStyle::Dynamic(DynamicStyle::Callable {
                        parsed_callable: "cp".to_string()
                    })
                },
                Span {
                    start: 3,
                    end: 9,
                    style: dynamic_string_file_style.clone()
                },
                Span {
                    start: 9,
                    end: 17,
                    style: dynamic_escape_file_style.clone()
                },
                Span {
                    start: 17,
                    end: 23,
                    style: dynamic_string_file_style.clone()
                }
            ]
        );

        let highlighted =
            highlighter.highlight(r#"cp test$'\x20\x20'1.txt dest.txt"#, pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![
                Span {
                    start: 0,
                    end: 2,
                    style: SpanStyle::Dynamic(DynamicStyle::Callable {
                        parsed_callable: "cp".to_string()
                    })
                },
                Span {
                    start: 3,
                    end: 7,
                    style: SpanStyle::Static(dynamic_file_style.clone())
                },
                Span {
                    start: 7,
                    end: 9,
                    style: dynamic_string_file_style.clone()
                },
                Span {
                    start: 9,
                    end: 17,
                    style: dynamic_escape_file_style.clone()
                },
                Span {
                    start: 17,
                    end: 18,
                    style: dynamic_string_file_style.clone()
                },
                Span {
                    start: 18,
                    end: 23,
                    style: SpanStyle::Static(dynamic_file_style.clone())
                }
            ]
        );

        Ok(())
    }

    #[test]
    fn command_with_multibyte_escape() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let subdir = dir.path().join("sub");
        fs::create_dir_all(&subdir)?;
        let test_path = subdir.join("test😎.sh");
        fs::write(&test_path, "#!/bin/sh")?;
        fs::set_permissions(&test_path, Permissions::from_mode(0o755))?;
        let pwd = Some(dir.path().to_str().unwrap());

        let highlighter = Highlighter::new(&test_config())?;
        let dynamic_command_style =
            resolve_static_style(DYNAMIC_CALLABLE_COMMAND, &highlighter.theme).unwrap();

        let highlighted =
            highlighter.highlight(r"$'sub/test\xF0\x9F\x98\x8E.sh'", pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![Span {
                start: 0,
                end: 30,
                style: SpanStyle::Static(dynamic_command_style.clone())
            }]
        );

        let highlighted =
            highlighter.highlight(r"$'sub/test\xF0\237\x98\x8E.sh'", pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![Span {
                start: 0,
                end: 30,
                style: SpanStyle::Static(dynamic_command_style.clone())
            }]
        );

        Ok(())
    }

    #[test]
    fn path_with_multibyte_escape() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let test_path = dir.path().join("test😎.txt");
        fs::write(test_path, "test contents")?;
        let pwd = Some(dir.path().to_str().unwrap());

        let highlighter = Highlighter::new(&test_config())?;
        let dynamic_file_style =
            resolve_static_style(DYNAMIC_PATH_FILE, &highlighter.theme).unwrap();
        let string_style = resolve_static_style(STRING_QUOTED_DOUBLE, &highlighter.theme).unwrap();
        let escape_style = resolve_static_style(CHARACTER_ESCAPE, &highlighter.theme).unwrap();
        let dynamic_string_file_style = mix_styles(
            &SpanStyle::Static(string_style.clone()),
            &SpanStyle::Static(dynamic_file_style.clone()),
        );
        let dynamic_escape_file_style = mix_styles(
            &SpanStyle::Static(escape_style.clone()),
            &SpanStyle::Static(dynamic_file_style.clone()),
        );

        let highlighted =
            highlighter.highlight(r"cp $'test\xF0\x9F\x98\x8E.txt' dest.txt", pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![
                Span {
                    start: 0,
                    end: 2,
                    style: SpanStyle::Dynamic(DynamicStyle::Callable {
                        parsed_callable: "cp".to_string()
                    })
                },
                Span {
                    start: 3,
                    end: 9,
                    style: dynamic_string_file_style.clone(),
                },
                Span {
                    start: 9,
                    end: 25,
                    style: dynamic_escape_file_style.clone(),
                },
                Span {
                    start: 25,
                    end: 30,
                    style: dynamic_string_file_style.clone(),
                }
            ]
        );

        let highlighted =
            highlighter.highlight(r"cp $'test\xF0\237\x98\x8E.txt' dest.txt", pwd, |_| true)?;
        assert_eq!(
            highlighted,
            vec![
                Span {
                    start: 0,
                    end: 2,
                    style: SpanStyle::Dynamic(DynamicStyle::Callable {
                        parsed_callable: "cp".to_string()
                    })
                },
                Span {
                    start: 3,
                    end: 9,
                    style: dynamic_string_file_style.clone(),
                },
                Span {
                    start: 9,
                    end: 25,
                    style: dynamic_escape_file_style.clone(),
                },
                Span {
                    start: 25,
                    end: 30,
                    style: dynamic_string_file_style.clone(),
                }
            ]
        );

        Ok(())
    }

    fn static_span(
        start: usize,
        end: usize,
        fg: Option<&str>,
        bg: Option<&str>,
        bold: bool,
        underline: bool,
    ) -> Span {
        Span {
            start,
            end,
            style: SpanStyle::Static(StaticStyle {
                foreground_color: fg.map(String::from),
                background_color: bg.map(String::from),
                bold,
                underline,
            }),
        }
    }

    /// Both base and mixins are empty
    #[test]
    fn mix_spans_empty() {
        assert_eq!(mix_spans(vec![], vec![]), vec![]);
    }

    /// No mixins: base spans are returned as-is
    #[test]
    fn mix_spans_no_mixins() {
        let base = vec![static_span(0, 5, Some("red"), None, false, false)];
        assert_eq!(mix_spans(base.clone(), vec![]), base);
    }

    /// A mixin that doesn't overlap any base span is still kept
    #[test]
    fn mix_spans_non_overlapping_mixin_kept() {
        let base = vec![static_span(0, 3, Some("red"), None, false, false)];
        let mixins = vec![static_span(5, 8, Some("blue"), None, false, false)];
        assert_eq!(
            mix_spans(base, mixins),
            vec![
                static_span(0, 3, Some("red"), None, false, false),
                static_span(5, 8, Some("blue"), None, false, false),
            ]
        );
    }

    /// A mixin that fully covers a base span: the entire base is overridden
    #[test]
    fn mix_spans_mixin_fully_covers_base() {
        let base = vec![static_span(2, 6, Some("red"), None, false, false)];
        let mixins = vec![static_span(2, 6, Some("blue"), None, true, false)];
        assert_eq!(
            mix_spans(base, mixins),
            vec![static_span(2, 6, Some("blue"), None, true, false)]
        );
    }

    /// A mixin that partially overlaps the start of a base span
    #[test]
    fn mix_spans_mixin_overlaps_start() {
        let base = vec![static_span(2, 8, Some("red"), None, false, false)];
        let mixins = vec![static_span(0, 4, Some("blue"), None, false, false)];
        assert_eq!(
            mix_spans(base, mixins),
            vec![
                // mixin before base + intersection merged (same style)
                static_span(0, 4, Some("blue"), None, false, false),
                // remainder of base (4..8)
                static_span(4, 8, Some("red"), None, false, false),
            ]
        );
    }

    /// A mixin that partially overlaps the end of a base span
    #[test]
    fn mix_spans_mixin_overlaps_end() {
        let base = vec![static_span(0, 5, Some("red"), None, false, false)];
        let mixins = vec![static_span(3, 8, Some("blue"), None, false, false)];
        assert_eq!(
            mix_spans(base, mixins),
            vec![
                // base before overlap (0..3)
                static_span(0, 3, Some("red"), None, false, false),
                // intersection + mixin after base merged (same style)
                static_span(3, 8, Some("blue"), None, false, false),
            ]
        );
    }

    /// A mixin fully contained within a base span splits it into three parts
    #[test]
    fn mix_spans_mixin_inside_base() {
        let base = vec![static_span(0, 10, Some("red"), Some("white"), false, false)];
        let mixins = vec![static_span(3, 7, None, Some("black"), true, false)];
        assert_eq!(
            mix_spans(base, mixins),
            vec![
                // base before overlap
                static_span(0, 3, Some("red"), Some("white"), false, false),
                // intersection: mixin bg overrides, mixin bold overrides, base fg kept (mixin fg is None)
                static_span(3, 7, Some("red"), Some("black"), true, false),
                // base after overlap
                static_span(7, 10, Some("red"), Some("white"), false, false),
            ]
        );
    }

    /// Multiple mixins overlapping a single base span
    #[test]
    fn mix_spans_multiple_mixins_one_base() {
        let base = vec![static_span(0, 10, Some("red"), None, false, false)];
        let mixins = vec![
            static_span(1, 3, Some("green"), None, false, false),
            static_span(5, 7, Some("blue"), None, false, false),
        ];
        assert_eq!(
            mix_spans(base, mixins),
            vec![
                static_span(0, 1, Some("red"), None, false, false),
                static_span(1, 3, Some("green"), None, false, false),
                static_span(3, 5, Some("red"), None, false, false),
                static_span(5, 7, Some("blue"), None, false, false),
                static_span(7, 10, Some("red"), None, false, false),
            ]
        );
    }

    /// Mixin with None fg preserves base fg; mixin with Some bg overrides
    #[test]
    fn mix_spans_style_merge_none_preserved() {
        let base = vec![static_span(0, 4, Some("red"), Some("white"), true, false)];
        let mixins = vec![static_span(0, 4, None, Some("black"), false, true)];
        assert_eq!(
            mix_spans(base, mixins),
            vec![
                // fg: base (mixin is None), bg: mixin, bold: base (mixin is false), underline: mixin (true)
                static_span(0, 4, Some("red"), Some("black"), true, true),
            ]
        );
    }

    /// Non-overlapping mixin between two base spans is kept in order
    #[test]
    fn mix_spans_mixin_between_bases() {
        let base = vec![
            static_span(0, 3, Some("red"), None, false, false),
            static_span(7, 10, Some("green"), None, false, false),
        ];
        let mixins = vec![static_span(4, 6, Some("blue"), None, false, false)];
        assert_eq!(
            mix_spans(base, mixins),
            vec![
                static_span(0, 3, Some("red"), None, false, false),
                static_span(4, 6, Some("blue"), None, false, false),
                static_span(7, 10, Some("green"), None, false, false),
            ]
        );
    }

    /// A single mixin spanning across two base spans
    #[test]
    fn mix_spans_mixin_spans_two_bases() {
        let base = vec![
            static_span(0, 4, Some("red"), None, false, false),
            static_span(6, 10, Some("green"), None, false, false),
        ];
        let mixins = vec![static_span(2, 8, Some("blue"), None, false, false)];
        assert_eq!(
            mix_spans(base, mixins),
            vec![
                // base[0] before overlap
                static_span(0, 2, Some("red"), None, false, false),
                // base[0] ∩ mixin + gap + base[1] ∩ mixin merged (same style)
                static_span(2, 8, Some("blue"), None, false, false),
                // base[1] after overlap
                static_span(8, 10, Some("green"), None, false, false),
            ]
        );
    }

    /// No base spans: all mixins are kept
    #[test]
    fn mix_spans_no_base() {
        let mixins = vec![
            static_span(0, 3, Some("blue"), None, false, false),
            static_span(5, 8, Some("green"), None, false, false),
        ];
        assert_eq!(mix_spans(vec![], mixins.clone()), mixins,);
    }
}
