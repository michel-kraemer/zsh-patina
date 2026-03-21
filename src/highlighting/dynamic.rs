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
    CharacterEscapeArguments,
    CharacterEscapeQuotedAnsi,
    StringQuotedBeginArguments,
    StringQuotedBeginCallable,
    StringQuotedEndArguments,
    StringQuotedEndCallable,
    StringQuotedSingleArguments,
    StringQuotedSingleCallable,
    StringQuotedSingleAnsiArguments,
    StringQuotedSingleAnsiCallable,
    StringQuotedDoubleArguments,
    StringQuotedDoubleCallable,
    TildeArguments,
    TildeCallable,
}

impl DynamicScope {
    fn as_str(&self) -> &str {
        match self {
            DynamicScope::Arguments => ARGUMENTS,
            DynamicScope::Callable => CALLABLE,
            DynamicScope::CharacterEscape => CHARACTER_ESCAPE,
            DynamicScope::CharacterEscapeArguments => CHARACTER_ESCAPE_ARGUMENTS,
            DynamicScope::CharacterEscapeQuotedAnsi => CHARACTER_ESCAPE_QUOTED_ANSI,
            DynamicScope::StringQuotedBeginArguments => STRING_QUOTED_BEGIN_ARGUMENTS,
            DynamicScope::StringQuotedBeginCallable => STRING_QUOTED_BEGIN_CALLABLE,
            DynamicScope::StringQuotedEndArguments => STRING_QUOTED_END_ARGUMENTS,
            DynamicScope::StringQuotedEndCallable => STRING_QUOTED_END_CALLABLE,
            DynamicScope::StringQuotedSingleArguments => STRING_QUOTED_SINGLE_ARGUMENTS,
            DynamicScope::StringQuotedSingleCallable => STRING_QUOTED_SINGLE_CALLABLE,
            DynamicScope::StringQuotedSingleAnsiArguments => STRING_QUOTED_SINGLE_ANSI_ARGUMENTS,
            DynamicScope::StringQuotedSingleAnsiCallable => STRING_QUOTED_SINGLE_ANSI_CALLABLE,
            DynamicScope::StringQuotedDoubleArguments => STRING_QUOTED_DOUBLE_ARGUMENTS,
            DynamicScope::StringQuotedDoubleCallable => STRING_QUOTED_DOUBLE_CALLABLE,
            DynamicScope::TildeArguments => TILDE_ARGUMENTS,
            DynamicScope::TildeCallable => TILDE_CALLABLE,
        }
    }
}

impl TryFrom<&str> for DynamicScope {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            ARGUMENTS => Ok(DynamicScope::Arguments),
            CALLABLE => Ok(DynamicScope::Callable),
            CHARACTER_ESCAPE => Ok(DynamicScope::CharacterEscape),
            CHARACTER_ESCAPE_ARGUMENTS => Ok(DynamicScope::CharacterEscapeArguments),
            CHARACTER_ESCAPE_QUOTED_ANSI => Ok(DynamicScope::CharacterEscapeQuotedAnsi),
            STRING_QUOTED_BEGIN_ARGUMENTS => Ok(DynamicScope::StringQuotedBeginArguments),
            STRING_QUOTED_BEGIN_CALLABLE => Ok(DynamicScope::StringQuotedBeginCallable),
            STRING_QUOTED_END_ARGUMENTS => Ok(DynamicScope::StringQuotedEndArguments),
            STRING_QUOTED_END_CALLABLE => Ok(DynamicScope::StringQuotedEndCallable),
            STRING_QUOTED_SINGLE_ARGUMENTS => Ok(DynamicScope::StringQuotedSingleArguments),
            STRING_QUOTED_SINGLE_CALLABLE => Ok(DynamicScope::StringQuotedSingleCallable),
            STRING_QUOTED_SINGLE_ANSI_ARGUMENTS => {
                Ok(DynamicScope::StringQuotedSingleAnsiArguments)
            }
            STRING_QUOTED_SINGLE_ANSI_CALLABLE => Ok(DynamicScope::StringQuotedSingleAnsiCallable),
            STRING_QUOTED_DOUBLE_ARGUMENTS => Ok(DynamicScope::StringQuotedDoubleArguments),
            STRING_QUOTED_DOUBLE_CALLABLE => Ok(DynamicScope::StringQuotedDoubleCallable),
            TILDE_ARGUMENTS => Ok(DynamicScope::TildeArguments),
            TILDE_CALLABLE => Ok(DynamicScope::TildeCallable),
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
                | DynamicScope::StringQuotedSingleArguments
                | DynamicScope::StringQuotedSingleCallable
                | DynamicScope::StringQuotedSingleAnsiArguments
                | DynamicScope::StringQuotedSingleAnsiCallable
                | DynamicScope::StringQuotedDoubleArguments
                | DynamicScope::StringQuotedDoubleCallable => {
                    let c = &line[t.byte_range.clone()];
                    let len = c.chars().count();
                    s.push_str(c);
                    end += len;
                }

                DynamicScope::CharacterEscapeArguments | DynamicScope::CharacterEscape => {
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

                DynamicScope::StringQuotedBeginArguments
                | DynamicScope::StringQuotedBeginCallable => {
                    end += line[t.byte_range.clone()].chars().count();
                }

                DynamicScope::StringQuotedEndArguments | DynamicScope::StringQuotedEndCallable => {
                    end += 1;
                }

                DynamicScope::TildeArguments | DynamicScope::TildeCallable => {
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
