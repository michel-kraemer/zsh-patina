use std::{env, ops::Range};

use anyhow::{Context, Result, bail};

use crate::{
    path::{PathType, is_path_executable, path_type},
    unescape::ZshUnescape,
};

use super::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DynamicScope {
    Arguments,
    Callable,
    CharacterEscape,
    CharacterEscapeQuotedAnsi,
    StringQuotedBegin,
    StringQuotedEnd,
    StringQuotedSingle,
    StringQuotedSingleAnsi,
    StringQuotedDouble,
    Tilde,
}

impl TryFrom<&str> for DynamicScope {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            ARGUMENTS => Ok(DynamicScope::Arguments),
            CALLABLE => Ok(DynamicScope::Callable),
            CHARACTER_ESCAPE | CHARACTER_ESCAPE_ARGUMENTS => Ok(DynamicScope::CharacterEscape),
            CHARACTER_ESCAPE_QUOTED_ANSI => Ok(DynamicScope::CharacterEscapeQuotedAnsi),
            STRING_QUOTED_BEGIN_ARGUMENTS | STRING_QUOTED_BEGIN_CALLABLE => {
                Ok(DynamicScope::StringQuotedBegin)
            }
            STRING_QUOTED_END_ARGUMENTS | STRING_QUOTED_END_CALLABLE => {
                Ok(DynamicScope::StringQuotedEnd)
            }
            STRING_QUOTED_SINGLE_ARGUMENTS | STRING_QUOTED_SINGLE_CALLABLE => {
                Ok(DynamicScope::StringQuotedSingle)
            }
            STRING_QUOTED_SINGLE_ANSI_ARGUMENTS | STRING_QUOTED_SINGLE_ANSI_CALLABLE => {
                Ok(DynamicScope::StringQuotedSingleAnsi)
            }
            STRING_QUOTED_DOUBLE_ARGUMENTS | STRING_QUOTED_DOUBLE_CALLABLE => {
                Ok(DynamicScope::StringQuotedDouble)
            }
            TILDE_ARGUMENTS | TILDE_CALLABLE => Ok(DynamicScope::Tilde),
            _ => bail!("Unknown dynamic scope: {value}"),
        }
    }
}

#[derive(Debug)]
pub struct DynamicToken {
    range: Range<usize>,
    byte_range: Range<usize>,
    scope: DynamicScope,
}

impl DynamicToken {
    pub fn new(range: &Range<usize>, byte_range: &Range<usize>, scope: DynamicScope) -> Self {
        Self {
            range: range.clone(),
            byte_range: byte_range.clone(),
            scope,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DynamicType {
    Unknown,
    Callable,
    Arguments,
}

#[derive(Debug)]
pub struct DynamicTokenGroup {
    pub range: Range<usize>,
    pub byte_range: Range<usize>,
    pub dynamic_type: DynamicType,
    pub tokens: Vec<DynamicToken>,
}

impl DynamicTokenGroup {
    pub fn new(
        range: &Range<usize>,
        byte_range: &Range<usize>,
        dynamic_type: DynamicType,
        scope: DynamicScope,
    ) -> Self {
        Self {
            range: range.clone(),
            byte_range: byte_range.clone(),
            dynamic_type,
            tokens: vec![DynamicToken::new(range, byte_range, scope)],
        }
    }

    pub fn highlight(&self, line: &str, pwd: &str, theme: &Theme) -> Result<Vec<Span>> {
        match self.dynamic_type {
            DynamicType::Unknown => Ok(Vec::new()), // nothing to do
            DynamicType::Callable => self.highlight_callable(line, pwd, theme),
            DynamicType::Arguments => self.highlight_arguments(line, pwd, theme),
        }
    }

    fn highlight_callable(&self, line: &str, pwd: &str, theme: &Theme) -> Result<Vec<Span>> {
        let mut result = Vec::new();

        let parsed = self.parse(line)?;
        for (p, range) in parsed.into_iter().take(1) {
            let span_style = if p.contains('/') && is_path_executable(&p, pwd) {
                if let Some(style) = resolve_static_style(DYNAMIC_CALLABLE_COMMAND, theme) {
                    Some(SpanStyle::Static(style))
                } else {
                    resolve_static_style(CALLABLE, theme).map(SpanStyle::Static)
                }
            } else {
                Some(SpanStyle::Dynamic(DynamicStyle::Callable {
                    parsed_callable: p,
                }))
            };

            if let Some(span_style) = span_style {
                result.push(Span {
                    start: range.start,
                    end: range.end,
                    style: span_style,
                });
            }
        }

        Ok(result)
    }

    fn highlight_arguments(&self, line: &str, pwd: &str, theme: &Theme) -> Result<Vec<Span>> {
        let mut result = Vec::new();

        let parsed = self.parse(line)?;
        for (p, range) in parsed {
            if let Some(t) = path_type(&p, pwd) {
                let dynamic_scope = match t {
                    PathType::File => DYNAMIC_PATH_FILE,
                    PathType::Directory => DYNAMIC_PATH_DIRECTORY,
                };
                if let Some(style) = resolve_static_style(dynamic_scope, theme) {
                    result.push(Span {
                        start: range.start,
                        end: range.end,
                        style: SpanStyle::Static(style),
                    });
                }
            }
        }

        Ok(result)
    }

    fn parse(&self, line: &str) -> Result<Vec<(String, Range<usize>)>> {
        let mut result = Vec::new();

        let mut s = String::new();
        let mut start = self.range.start;
        let mut end = start;
        let mut utf8_buf: Vec<u8> = Vec::new();
        let mut resolve_tilde = false;

        let flush_utf8 = |buf: &mut Vec<u8>, s: &mut String| -> Result<()> {
            if !buf.is_empty() {
                let decoded = std::str::from_utf8(buf)
                    .with_context(|| format!("Invalid UTF-8 byte sequence: {buf:02x?}"))?;
                s.push_str(decoded);
                buf.clear();
            }
            Ok(())
        };

        for t in &self.tokens {
            if t.scope != DynamicScope::CharacterEscapeQuotedAnsi && !utf8_buf.is_empty() {
                flush_utf8(&mut utf8_buf, &mut s)?;
            }

            match t.scope {
                DynamicScope::Arguments => {
                    let mut args = line[t.byte_range.clone()].chars().peekable();
                    while args.peek().is_some() {
                        if let Some(c) = args.peek()
                            && c.is_whitespace()
                        {
                            if !s.is_empty() {
                                result.push((s, start..end));
                            }

                            // skip whitespace
                            while let Some(c) = args.peek()
                                && c.is_whitespace()
                            {
                                args.next().unwrap();
                                end += 1;
                            }

                            s = String::new();
                            start = end;
                        }

                        if args.peek().is_none() {
                            break;
                        }

                        while let Some(c) = args.peek()
                            && !c.is_whitespace()
                        {
                            s.push(args.next().unwrap());
                            end += 1;
                        }
                    }
                }

                DynamicScope::Callable
                | DynamicScope::StringQuotedSingle
                | DynamicScope::StringQuotedSingleAnsi
                | DynamicScope::StringQuotedDouble => {
                    let c = &line[t.byte_range.clone()];
                    let len = c.chars().count();
                    s.push_str(c);
                    end += len;
                }

                DynamicScope::CharacterEscape => {
                    let c = &line[t.byte_range.clone()];
                    let len = c.chars().count();
                    s.push_str(&c[1..]); // trim leading '\'
                    end += len;
                }

                DynamicScope::CharacterEscapeQuotedAnsi => {
                    let c = &line[t.byte_range.clone()];
                    let len = c.chars().count();
                    if let Some(byte) = c.zsh_unescape_utf8_byte()? {
                        utf8_buf.push(byte);
                    } else {
                        s.push(c.zsh_unescape_char()?);
                    }
                    end += len;
                }

                DynamicScope::StringQuotedBegin => {
                    end += line[t.byte_range.clone()].chars().count();
                }

                DynamicScope::StringQuotedEnd => {
                    end += 1;
                }

                DynamicScope::Tilde => {
                    let c = &line[t.byte_range.clone()];

                    // resolve tilde at the beginning of a string
                    if start == end {
                        resolve_tilde = true;
                    }
                    s.push_str(c);

                    let len = c.chars().count();
                    end += len;
                }
            }
        }

        flush_utf8(&mut utf8_buf, &mut s)?;

        // resolve tilde only if the whole string is a tilde or if it starts
        // with '~/', because '~foobar', for example, should not be resolved
        if resolve_tilde && (s == "~" || s.starts_with("~/")) {
            let home = env::var_os("HOME").context("$HOME not set")?;
            s.replace_range(
                0..1,
                home.to_str().context("Unable to convert $HOME to string")?,
            );
        }

        if !s.is_empty() {
            result.push((s, start..end));
        }

        Ok(result)
    }
}
