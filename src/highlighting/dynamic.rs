use std::ops::Range;

use anyhow::{Context, Result};
use syntect::parsing::{ClearAmount, Scope, ScopeStackOp};

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
    Redirection,
    PoisonPill,
}

#[derive(Debug)]
pub struct DynamicToken {
    pub byte_range: Range<usize>,
    pub scope: DynamicScope,
}

impl DynamicToken {
    pub fn new(byte_range: Range<usize>, scope: DynamicScope) -> Self {
        Self { byte_range, scope }
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
    pub dynamic_type: DynamicType,
    pub tokens: Vec<DynamicToken>,
}

impl DynamicTokenGroup {
    pub fn highlight(
        &self,
        line: &str,
        pwd: &str,
        home_dir: &str,
        theme: &Theme,
    ) -> Result<Vec<Span>> {
        match self.dynamic_type {
            DynamicType::Unknown => Ok(Vec::new()), // nothing to do
            DynamicType::Callable => self.highlight_callable(line, pwd, home_dir, theme),
            DynamicType::Arguments => self.highlight_arguments(line, pwd, home_dir, theme),
        }
    }

    fn highlight_callable(
        &self,
        line: &str,
        pwd: &str,
        home_dir: &str,
        theme: &Theme,
    ) -> Result<Vec<Span>> {
        let mut result = Vec::new();

        let parsed = self.parse(line, home_dir)?;
        for (p, range) in parsed.into_iter().take(1) {
            log::trace!("Dynamically highlighting callable: {p}");
            let span_style = if p.contains('/') && is_path_executable(&p, pwd) {
                log::trace!("Callable `{p}' is executable.");
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

    fn highlight_arguments(
        &self,
        line: &str,
        pwd: &str,
        home_dir: &str,
        theme: &Theme,
    ) -> Result<Vec<Span>> {
        let mut result = Vec::new();

        let parsed = self.parse(line, home_dir)?;
        for (p, range) in parsed {
            log::trace!("Dynamically highlighting argument: {p}");
            if let Some(t) = path_type(&p, pwd) {
                log::trace!("Argument `{p}' is {t:?}.");
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

    fn parse(&self, line: &str, home_dir: &str) -> Result<Vec<(String, Range<usize>)>> {
        if self.tokens.is_empty() {
            return Ok(Vec::new());
        }

        struct State<'a> {
            home_dir: &'a str,
            s: String,
            start: usize,
            end: usize,
            utf8_buf: Vec<u8>,
            resolve_tilde: bool,
            is_poison: bool,
            result: Vec<(String, Range<usize>)>,
        }

        impl State<'_> {
            fn flush_utf8(&mut self) -> Result<()> {
                if !self.utf8_buf.is_empty() {
                    let decoded = std::str::from_utf8(&self.utf8_buf).with_context(|| {
                        format!("Invalid UTF-8 byte sequence: {:02x?}", self.utf8_buf)
                    })?;
                    self.s.push_str(decoded);
                    self.utf8_buf.clear();
                }
                Ok(())
            }

            fn push_string(&mut self) -> Result<()> {
                if !self.s.is_empty() && !self.is_poison {
                    // resolve tilde only if the whole string is a tilde or if it starts
                    // with '~/', because '~foobar', for example, should not be resolved
                    if self.resolve_tilde && (self.s == "~" || self.s.starts_with("~/")) {
                        self.s.replace_range(0..1, self.home_dir);
                    }

                    self.result
                        .push((std::mem::take(&mut self.s), self.start..self.end));
                } else {
                    self.s = String::new();
                }

                self.is_poison = false;
                self.resolve_tilde = false;

                Ok(())
            }
        }

        let chars_count = line[0..self.tokens[0].byte_range.start].chars().count();
        let mut state = State {
            home_dir,
            s: String::new(),
            start: chars_count,
            end: chars_count,
            utf8_buf: Vec::new(),
            resolve_tilde: false,
            is_poison: false,
            result: Vec::new(),
        };

        for t in &self.tokens {
            if t.scope != DynamicScope::CharacterEscapeQuotedAnsi && !state.utf8_buf.is_empty() {
                state.flush_utf8()?;
            }

            match t.scope {
                DynamicScope::Arguments => {
                    let mut args = line[t.byte_range.clone()].chars().peekable();
                    while args.peek().is_some() {
                        if let Some(c) = args.peek()
                            && c.is_whitespace()
                        {
                            state.push_string()?;

                            // skip whitespace
                            while let Some(c) = args.peek()
                                && c.is_whitespace()
                            {
                                args.next().unwrap();
                                state.end += 1;
                            }

                            state.start = state.end;
                        }

                        if args.peek().is_none() {
                            break;
                        }

                        while let Some(c) = args.peek()
                            && !c.is_whitespace()
                        {
                            state.s.push(args.next().unwrap());
                            state.end += 1;
                        }
                    }
                }

                DynamicScope::Callable
                | DynamicScope::StringQuotedSingle
                | DynamicScope::StringQuotedSingleAnsi
                | DynamicScope::StringQuotedDouble => {
                    let c = &line[t.byte_range.clone()];
                    let len = c.chars().count();
                    state.s.push_str(c);
                    state.end += len;
                }

                DynamicScope::CharacterEscape => {
                    let c = &line[t.byte_range.clone()];
                    let len = c.chars().count();
                    state.s.push_str(&c[1..]); // trim leading '\'
                    state.end += len;
                }

                DynamicScope::CharacterEscapeQuotedAnsi => {
                    let c = &line[t.byte_range.clone()];
                    let len = c.chars().count();
                    if let Some(byte) = c.zsh_unescape_utf8_byte()? {
                        state.utf8_buf.push(byte);
                    } else {
                        state.s.push(c.zsh_unescape_char()?);
                    }
                    state.end += len;
                }

                DynamicScope::StringQuotedBegin => {
                    state.end += line[t.byte_range.clone()].chars().count();
                }

                DynamicScope::StringQuotedEnd => {
                    state.end += 1;
                }

                DynamicScope::Tilde => {
                    let c = &line[t.byte_range.clone()];

                    // resolve tilde at the beginning of a string
                    if state.start == state.end {
                        state.resolve_tilde = true;
                    }
                    state.s.push_str(c);

                    let len = c.chars().count();
                    state.end += len;
                }

                DynamicScope::Redirection => {
                    state.push_string()?;
                    let c = &line[t.byte_range.clone()];
                    let len = c.chars().count();
                    state.end += len;
                    state.start = state.end;
                }

                DynamicScope::PoisonPill => {
                    // A poison pill means that either the current or the next
                    // string contains a scope that prevents it from being
                    // dynamically highlighted (e.g. an environment variable or
                    // a command substitution).

                    let c = &line[t.byte_range.clone()];
                    let len = c.chars().count();
                    if len > 0 && c.starts_with(|b: char| b.is_whitespace()) {
                        // the poison pill starts with a whitespace, which means
                        // we must keep the current string and throw away the
                        // next one
                        state.push_string()?;
                        state.is_poison = true;
                    } else {
                        // The poison pill does not start with a whitespace,
                        // which means it's part of the current string. Throw it
                        // away and also throw away anything else until the next
                        // whitespace.
                        state.is_poison = true;
                    }

                    // skip poison pill contents
                    state.end += len;
                    state.start = state.end;
                }
            }
        }

        state.flush_utf8()?;
        state.push_string()?;

        Ok(state.result)
    }
}

#[derive(Clone, Copy)]
pub struct DynamicScopes {
    arguments_scope: Scope,
    callable_scope: Scope,
    character_escape_scope: Scope,
    string_quoted_begin_scope: Scope,
    string_quoted_end_scope: Scope,
    string_quoted_single_scope: Scope,
    string_quoted_sigle_ansi_scope: Scope,
    string_quoted_double_scope: Scope,
    tilde_variable_scope: Scope,
    tilde_meta_scope: Scope,
    redirection_scope: Scope,
}

impl DynamicScopes {
    pub fn new() -> Self {
        let arguments_scope = Scope::new(ARGUMENTS).unwrap();
        let callable_scope = Scope::new(CALLABLE).unwrap();
        let character_escape_scope = Scope::new(CHARACTER_ESCAPE).unwrap();
        let string_quoted_begin_scope = Scope::new(STRING_QUOTED_BEGIN).unwrap();
        let string_quoted_end_scope = Scope::new(STRING_QUOTED_END).unwrap();
        let string_quoted_single_scope = Scope::new(STRING_QUOTED_SINGLE).unwrap();
        let string_quoted_sigle_ansi_scope = Scope::new(STRING_QUOTED_SINGLE_ANSI).unwrap();
        let string_quoted_double_scope = Scope::new(STRING_QUOTED_DOUBLE).unwrap();
        let tilde_variable_scope = Scope::new(TILDE_VARIABLE).unwrap();
        let tilde_meta_scope = Scope::new(TILDE_META).unwrap();
        let redirection_scope = Scope::new(REDIRECTION).unwrap();
        Self {
            arguments_scope,
            callable_scope,
            character_escape_scope,
            string_quoted_begin_scope,
            string_quoted_end_scope,
            string_quoted_single_scope,
            string_quoted_sigle_ansi_scope,
            string_quoted_double_scope,
            tilde_variable_scope,
            tilde_meta_scope,
            redirection_scope,
        }
    }
}

struct TemporaryGroup {
    dynamic_type: DynamicType,
    current_scope: Vec<DynamicScope>,
    current_start: usize,
    tokens: Vec<DynamicToken>,
}

pub struct DynamicTokenGroupBuilder {
    scopes: DynamicScopes,
    stack: Vec<Scope>,
    stash: Vec<Vec<Scope>>,
    group_stack: Vec<TemporaryGroup>,
    group_stash: Vec<Vec<TemporaryGroup>>,
    character_escape_buf: Vec<DynamicToken>,
}

impl DynamicTokenGroupBuilder {
    pub fn new(scopes: DynamicScopes) -> Self {
        Self {
            scopes,
            stack: Vec::new(),
            stash: Vec::new(),
            group_stack: Vec::new(),
            group_stash: Vec::new(),
            character_escape_buf: Vec::new(),
        }
    }

    fn on_pop(&mut self, i: usize, result: &mut Vec<DynamicTokenGroup>) {
        let scope = self.stack.pop().unwrap();
        if let Some(current_group) = self.group_stack.last_mut()
            && (scope == self.scopes.arguments_scope
                || scope == self.scopes.callable_scope
                || scope == self.scopes.character_escape_scope
                || scope == self.scopes.string_quoted_begin_scope
                || scope == self.scopes.string_quoted_end_scope
                || scope == self.scopes.string_quoted_single_scope
                || scope == self.scopes.string_quoted_sigle_ansi_scope
                || scope == self.scopes.string_quoted_double_scope
                || scope == self.scopes.tilde_variable_scope
                || scope == self.scopes.redirection_scope
                || current_group.current_scope.last() == Some(&DynamicScope::PoisonPill))
        {
            if let Some(current_scope) = current_group.current_scope.pop()
                && i != current_group.current_start
            {
                current_group.tokens.push(DynamicToken::new(
                    current_group.current_start..i,
                    current_scope,
                ));
            }
            current_group.current_start = i;
        } else if let Some(current_group) = self.group_stack.last_mut()
            && scope == self.scopes.tilde_meta_scope
        {
            // result of pop can be ignored - tilde will be caught by
            // `tilde_variable_scope`
            current_group.current_scope.pop();
            current_group.current_start = i;
        } else if self.group_stack.is_empty()
            && scope == self.scopes.character_escape_scope
            && let Some(ce) = self.character_escape_buf.last_mut()
        {
            ce.byte_range.end = i;
        }

        if (scope == self.scopes.arguments_scope || scope == self.scopes.callable_scope)
            && let Some(g) = self.group_stack.pop()
            && !g.tokens.is_empty()
        {
            result.push(DynamicTokenGroup {
                dynamic_type: g.dynamic_type,
                tokens: g.tokens,
            });
        }
    }

    pub fn build(
        &mut self,
        ops: &[(usize, ScopeStackOp)],
        offset: usize,
    ) -> Vec<DynamicTokenGroup> {
        let mut result = Vec::new();

        for (i, s) in ops {
            let i = i + offset;

            match s {
                ScopeStackOp::Push(scope) => {
                    if *scope == self.scopes.arguments_scope {
                        self.group_stack.push(TemporaryGroup {
                            dynamic_type: DynamicType::Arguments,
                            current_scope: vec![DynamicScope::Arguments],
                            current_start: i,
                            tokens: Vec::new(),
                        });
                    } else if *scope == self.scopes.callable_scope {
                        if let Some(l) = self.character_escape_buf.last()
                            && l.byte_range.end != i
                        {
                            self.character_escape_buf.clear();
                        }
                        self.group_stack.push(TemporaryGroup {
                            dynamic_type: DynamicType::Callable,
                            current_scope: vec![DynamicScope::Callable],
                            current_start: i,
                            tokens: std::mem::take(&mut self.character_escape_buf),
                        });
                    } else if self.group_stack.is_empty()
                        && *scope == self.scopes.character_escape_scope
                    {
                        if let Some(l) = self.character_escape_buf.last()
                            && l.byte_range.end != i
                        {
                            self.character_escape_buf.clear();
                        }
                        self.character_escape_buf
                            .push(DynamicToken::new(i..i, DynamicScope::CharacterEscape));
                    } else if let Some(current_group) = self.group_stack.last_mut() {
                        let new_dynamic_scope = if *scope == self.scopes.character_escape_scope {
                            if current_group
                                .current_scope
                                .contains(&DynamicScope::StringQuotedSingleAnsi)
                            {
                                DynamicScope::CharacterEscapeQuotedAnsi
                            } else {
                                DynamicScope::CharacterEscape
                            }
                        } else if *scope == self.scopes.string_quoted_begin_scope {
                            DynamicScope::StringQuotedBegin
                        } else if *scope == self.scopes.string_quoted_end_scope {
                            DynamicScope::StringQuotedEnd
                        } else if *scope == self.scopes.string_quoted_single_scope {
                            DynamicScope::StringQuotedSingle
                        } else if *scope == self.scopes.string_quoted_sigle_ansi_scope {
                            DynamicScope::StringQuotedSingleAnsi
                        } else if *scope == self.scopes.string_quoted_double_scope {
                            DynamicScope::StringQuotedDouble
                        } else if *scope == self.scopes.tilde_variable_scope
                            || *scope == self.scopes.tilde_meta_scope
                        {
                            DynamicScope::Tilde
                        } else if *scope == self.scopes.redirection_scope {
                            DynamicScope::Redirection
                        } else {
                            // Unknown token found. We should not dynamically
                            // highlight this group. Insert a poison pill so
                            // [DynamicTokenGroup::parse()] will later skip it.
                            DynamicScope::PoisonPill
                        };

                        if let Some(current_scope) = current_group.current_scope.last()
                            && i != current_group.current_start
                        {
                            current_group.tokens.push(DynamicToken::new(
                                current_group.current_start..i,
                                *current_scope,
                            ));
                        }
                        current_group.current_scope.push(new_dynamic_scope);
                        current_group.current_start = i;
                    }
                    self.stack.push(*scope);
                }

                ScopeStackOp::Pop(count) => {
                    for _ in 0..*count {
                        self.on_pop(i, &mut result);
                    }
                }

                ScopeStackOp::Clear(clear_amount) => {
                    // similar to ::Pop, but store popped items in stash so
                    // we can restore them if necessary
                    let count = match *clear_amount {
                        ClearAmount::TopN(n) => n.min(self.stack.len()),
                        ClearAmount::All => self.stack.len(),
                    };

                    let mut to_stash = Vec::new();
                    let mut to_group_stash = Vec::new();
                    for _ in 0..count {
                        to_stash.push(self.stack.pop().unwrap());
                        to_group_stash.push(self.group_stack.pop().unwrap());
                    }
                    self.stash.push(to_stash);
                    self.group_stash.push(to_group_stash);
                }

                ScopeStackOp::Restore => {
                    // restore items from the stash (see ::Clear)
                    if let Some(mut s) = self.stash.pop() {
                        while let Some(e) = s.pop() {
                            self.stack.push(e);
                        }
                    }
                    if let Some(mut s) = self.group_stash.pop() {
                        while let Some(g) = s.pop() {
                            self.group_stack.push(g);
                        }
                    }
                }

                ScopeStackOp::Noop => {}
            }
        }

        result
    }

    pub fn finish(mut self, end: usize) -> Vec<DynamicTokenGroup> {
        let mut result = Vec::new();

        // consume the remaining items on the stack
        while !self.stack.is_empty() {
            self.on_pop(end, &mut result);
        }

        if !self.character_escape_buf.is_empty() {
            result.push(DynamicTokenGroup {
                dynamic_type: DynamicType::Unknown,
                tokens: self.character_escape_buf,
            });
        }

        result
    }
}
