use std::{
    ops::Range,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use rustc_hash::FxHashMap;
use syntect::{
    highlighting::{
        HighlightIterator, HighlightState, Highlighter as SyntectHighlighter, Theme as SyntectTheme,
    },
    parsing::{ClearAmount, ParseState, ScopeStack, ScopeStackOp, SyntaxSet},
    util::LinesWithEndings,
};

use super::*;
use crate::{
    config::{DynamicConfigType, HighlightingConfig},
    highlighting::{
        dynamic::{
            DynamicHighlightingOptions, DynamicScopes, DynamicTokenGroupBuilder, DynamicType,
        },
        historyexpansion::HistoryExpanded,
    },
    theme::{ScopeMapping, Theme, ThemeSource},
};

fn mix_spans(base: Vec<Span>, mut mixins: Vec<Span>) -> Vec<Span> {
    // make sure mixins are sorted
    mixins.sort_unstable_by(|a, b| a.start.cmp(&b.start).then(a.end.cmp(&b.end)));

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

pub struct HighlighterBuilder<'a> {
    config: &'a HighlightingConfig,
    home_dir: Option<String>,
}

impl<'a> HighlighterBuilder<'a> {
    pub fn new(config: &'a HighlightingConfig) -> Self {
        Self {
            config,
            home_dir: None,
        }
    }

    #[cfg(test)]
    pub fn home_dir(mut self, home_dir: String) -> Self {
        self.home_dir = Some(home_dir);
        self
    }

    pub fn build(self) -> Result<Highlighter> {
        let home_dir = self.home_dir.map(anyhow::Ok).unwrap_or_else(|| {
            let home = dirs::home_dir().context("Unable to find home directory")?;
            Ok(home
                .to_str()
                .context("Unable to convert home directory to string")?
                .to_owned())
        })?;
        Highlighter::new(self.config, home_dir)
    }
}

/// Options for the calling [`Highlighter::highlight`]
pub struct HighlightingRequest<'a, P>
where
    P: Fn(&Range<usize>) -> bool + Copy,
{
    cursor: Option<usize>,
    pwd: Option<&'a str>,
    history_expansions_enabled: bool,
    predicate: P,
}

impl<'a, P> HighlightingRequest<'a, P>
where
    P: Fn(&Range<usize>) -> bool + Copy,
{
    /// Set the cursor position in the command (character index)
    pub fn with_cursor(&self, cursor: usize) -> Self {
        Self {
            cursor: Some(cursor),
            ..*self
        }
    }

    /// Set the current working directory
    pub fn with_pwd<'b, O>(&self, pwd: O) -> HighlightingRequest<'b, P>
    where
        O: Into<Option<&'b str>>,
    {
        HighlightingRequest {
            pwd: pwd.into(),
            ..*self
        }
    }

    /// Enable or disable highlighting of history expansions
    pub fn with_history_expansions(&self, enabled: bool) -> Self {
        Self {
            history_expansions_enabled: enabled,
            ..*self
        }
    }

    /// Set the predicate function that determines which spans should be
    /// highlighted The predicate function takes a character index range and
    /// returns `true` if the span within that range should be highlighted, and
    /// `false` otherwise.
    pub fn with_predicate<Q>(&self, predicate: Q) -> HighlightingRequest<'a, Q>
    where
        Q: Fn(&Range<usize>) -> bool + Copy,
    {
        HighlightingRequest {
            cursor: self.cursor,
            pwd: self.pwd,
            history_expansions_enabled: self.history_expansions_enabled,
            predicate,
        }
    }
}

impl Default for HighlightingRequest<'_, fn(&Range<usize>) -> bool> {
    fn default() -> Self {
        Self {
            cursor: None,
            pwd: None,
            history_expansions_enabled: true,
            predicate: |_: &Range<usize>| true,
        }
    }
}

pub struct Highlighter {
    home_dir: String,
    max_line_length: usize,
    timeout: Duration,
    dynamic_callables_enabled: bool,
    dynamic_arguments_type: DynamicConfigType,
    syntax_set: SyntaxSet,
    theme: Theme,
    scope_mapping: ScopeMapping,
    syntect_theme: SyntectTheme,
    callable_choices: Vec<(String, StaticStyle)>,
    dynamic_scopes: DynamicScopes,
}

impl Highlighter {
    pub fn new(config: &HighlightingConfig, home_dir: String) -> Result<Self> {
        let syntax_set: SyntaxSet = syntect::dumps::from_uncompressed_data(include_bytes!(
            concat!(env!("OUT_DIR"), "/syntax_set.packdump")
        ))
        .expect("Unable to load shell syntax");

        let theme = Theme::load(&config.theme)?;
        let scope_mapping = ScopeMapping::new(&theme);
        let syntect_theme =
            theme
                .to_syntect(&scope_mapping)
                .with_context(|| match &config.theme {
                    ThemeSource::CatppuccinFrappe => {
                        "Failed to parse catppuccin-frappe theme".to_string()
                    }
                    ThemeSource::CatppuccinLatte => {
                        "Failed to parse catppuccin-latte theme".to_string()
                    }
                    ThemeSource::CatppuccinMacchiato => {
                        "Failed to parse catppuccin-macchiato theme".to_string()
                    }
                    ThemeSource::CatppuccinMocha => {
                        "Failed to parse catppuccin-mocha theme".to_string()
                    }
                    ThemeSource::Classic => "Failed to parse classic theme".to_string(),
                    ThemeSource::Kanagawa => "Failed to parse kanagawa theme".to_string(),
                    ThemeSource::Lavender => "Failed to parse lavender theme".to_string(),
                    ThemeSource::Nord => "Failed to parse nord theme".to_string(),
                    ThemeSource::Patina => "Failed to parse default theme".to_string(),
                    ThemeSource::Simple => "Failed to parse simple theme".to_string(),
                    ThemeSource::Solarized => "Failed to parse solarized theme".to_string(),
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
            home_dir,
            max_line_length: config.max_line_length,
            timeout: config.timeout,
            dynamic_callables_enabled: config.dynamic.callables,
            dynamic_arguments_type: config.dynamic.paths,
            syntax_set,
            theme,
            scope_mapping,
            syntect_theme,
            callable_choices,
            dynamic_scopes: DynamicScopes::new(),
        })
    }

    /// Return the theme used for highlighting
    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// Return a list of dynamic style choices the plugin has for callables
    pub fn callable_choices(&self) -> &[(String, StaticStyle)] {
        &self.callable_choices
    }

    fn should_highlight_dynamic(&self, dynamic_type: &DynamicType) -> bool {
        match dynamic_type {
            DynamicType::Unknown => true,
            DynamicType::Callable => self.dynamic_callables_enabled,
            DynamicType::Arguments => self.dynamic_arguments_type != DynamicConfigType::None,
        }
    }

    pub fn highlight<P>(&self, command: &str, request: &HighlightingRequest<P>) -> Result<Vec<Span>>
    where
        P: Fn(&Range<usize>) -> bool + Copy,
    {
        let start = Instant::now();

        let syntax = self.syntax_set.find_syntax_by_extension("sh").unwrap();

        let mut parse_state = ParseState::new(syntax);
        let syntect_highlighter = SyntectHighlighter::new(&self.syntect_theme);
        let mut highlight_state = HighlightState::new(&syntect_highlighter, ScopeStack::new());

        let mut dynamic_builder = DynamicTokenGroupBuilder::new(self.dynamic_scopes);
        let mut mixins = Vec::new();

        let dynamic_highlighting_options = request.pwd.map(|pwd| {
            DynamicHighlightingOptions::new(
                request.cursor,
                pwd,
                &self.home_dir,
                &self.theme,
                self.dynamic_arguments_type == DynamicConfigType::Partial,
            )
        });

        let mut i = 0;
        let mut byte_offset = 0;
        let mut result = Vec::new();
        let mut history_expanded =
            HistoryExpanded::wrap(LinesWithEndings::from(command.trim_ascii_end()));
        if !request.history_expansions_enabled {
            history_expanded.disable();
        }

        while let Some((line, expansions)) = history_expanded.next(&highlight_state.path.scopes) {
            if line.len() > self.max_line_length {
                // skip lines that are too long
                byte_offset += line.len();
                continue;
            }

            if start.elapsed() > self.timeout {
                // stop if highlighting takes too long
                return Ok(result);
            }

            let ops = expansions.apply(parse_state.parse_line(&line, &self.syntax_set)?);
            let ranges =
                HighlightIterator::new(&mut highlight_state, &ops, &line, &syntect_highlighter);

            for r in ranges {
                if r.1.is_empty() {
                    continue;
                }

                // this is O(n) but necessary in case the command contains
                // multi-byte characters
                let len = r.1.chars().count();

                if let Some(scope) = self.scope_mapping.decode(&r.0.foreground) {
                    let range = i..i + len;
                    if (request.predicate)(&range)
                        && let Some(style) = resolve_static_style(scope, &self.theme)
                    {
                        result.push(Span {
                            start: range.start,
                            end: range.end,
                            style: SpanStyle::Static(style),
                        });
                    }
                }

                i += len;
            }

            // perform dynamic highlighting
            if (self.dynamic_callables_enabled
                || self.dynamic_arguments_type != DynamicConfigType::None)
                && let Some(dynamic_highlighting_options) = &dynamic_highlighting_options
            {
                for g in dynamic_builder.build(&ops, byte_offset) {
                    if self.should_highlight_dynamic(&g.dynamic_type)
                        && let Ok(group_spans) = g.highlight(command, dynamic_highlighting_options)
                    {
                        mixins.extend(group_spans);
                    }
                }
            }

            byte_offset += line.len();
        }

        // perform dynamic highlighting for the remaining groups
        if (self.dynamic_callables_enabled
            || self.dynamic_arguments_type != DynamicConfigType::None)
            && let Some(dynamic_highlighting_options) = &dynamic_highlighting_options
        {
            for g in dynamic_builder.finish(byte_offset) {
                if self.should_highlight_dynamic(&g.dynamic_type)
                    && let Ok(group_spans) = g.highlight(command, dynamic_highlighting_options)
                {
                    mixins.extend(group_spans);
                }
            }
        }

        // mix into result
        if !mixins.is_empty() {
            result = mix_spans(
                result,
                mixins
                    .into_iter()
                    .filter(|m| (request.predicate)(&(m.start..m.end)))
                    .collect(),
            );
        }

        Ok(result)
    }

    pub fn tokenize(&self, command: &str) -> Result<Vec<Token>> {
        let syntax = self.syntax_set.find_syntax_by_extension("sh").unwrap();

        let mut offset = 0;
        let mut ps = ParseState::new(syntax);
        let mut result = Vec::new();
        let mut stack = Vec::new();
        let mut stash = Vec::new();
        let mut line_number = 0;
        let mut history_expanded =
            HistoryExpanded::wrap(LinesWithEndings::from(command.trim_ascii_end()));
        while let Some((line, expansions)) =
            history_expanded.next(&stack.iter().map(|(op, _, _, _)| *op).collect::<Vec<_>>())
        {
            let tokens = expansions.apply(ps.parse_line(&line, &self.syntax_set)?);

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
            line_number += 1;
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
        fmt::{Display, Formatter},
        fs::{self, Permissions},
        io::Write,
        os::unix::fs::PermissionsExt,
        path::PathBuf,
    };

    use crate::config::DynamicConfig;

    use super::*;
    use anyhow::Result;
    use insta::assert_snapshot;
    use pretty_assertions::assert_eq;
    use tabwriter::TabWriter;
    use tempfile::TempDir;

    fn test_config() -> HighlightingConfig {
        HighlightingConfig {
            theme: ThemeSource::File(concat!(env!("OUT_DIR"), "/test_theme.toml").to_string()),
            timeout: Duration::from_secs(3600),
            ..Default::default()
        }
    }

    struct TestCfg {
        highlighter: Highlighter,
        homedir: TempDir,
        tempdir: TempDir,
        pwd: String,
    }

    struct AssertableSpans {
        command: String,
        spans: Vec<Span>,
    }

    impl Display for AssertableSpans {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            let scopes = include!(concat!(env!("OUT_DIR"), "/scopes.rs"));

            writeln!(f, "{}\n", self.command)?;

            let mut tw = TabWriter::new(vec![]).minwidth(1);
            for span in &self.spans {
                write!(
                    tw,
                    "{}\t{}\t`{}`\t",
                    span.start,
                    span.end,
                    self.command
                        .chars()
                        .skip(span.start)
                        .take(span.end - span.start)
                        .collect::<String>()
                )
                .unwrap();
                match &span.style {
                    SpanStyle::Static(static_style) => {
                        if let Some(fg) = &static_style.foreground_color {
                            write!(
                                tw,
                                "{}",
                                scopes[fg.parse::<usize>().unwrap()]
                                    .strip_suffix(".shell")
                                    .unwrap()
                            )
                            .unwrap();
                        }
                        if let Some(bg) = &static_style.background_color {
                            if static_style.foreground_color.is_some() {
                                write!(tw, " + ").unwrap();
                            }
                            write!(
                                tw,
                                "{}",
                                scopes[bg.parse::<usize>().unwrap()]
                                    .strip_suffix(".shell")
                                    .unwrap()
                            )
                            .unwrap();
                        }
                    }
                    SpanStyle::Dynamic(dynamic_style) => match dynamic_style {
                        DynamicStyle::Callable { parsed_callable } => {
                            write!(tw, "CALLABLE `{parsed_callable}`").unwrap();
                        }
                    },
                }
                writeln!(tw).unwrap();
            }
            tw.flush().unwrap();

            let result = String::from_utf8(tw.into_inner().unwrap()).unwrap();
            write!(f, "{result}")?;

            Ok(())
        }
    }

    impl TestCfg {
        fn highlight(&self, command: &str) -> Result<AssertableSpans> {
            let request = HighlightingRequest::default().with_pwd(self.pwd.as_str());
            Ok(AssertableSpans {
                command: command.to_string(),
                spans: self.highlighter.highlight(command, &request)?,
            })
        }

        fn highlight_with_request<P>(
            &self,
            command: &str,
            request: HighlightingRequest<P>,
        ) -> Result<AssertableSpans>
        where
            P: Fn(&Range<usize>) -> bool + Copy,
        {
            Ok(AssertableSpans {
                command: command.to_string(),
                spans: self.highlighter.highlight(command, &request)?,
            })
        }

        fn touch_file(&self, name: &str) -> Result<PathBuf> {
            let test_path = self.tempdir.path().join(name);
            fs::write(&test_path, "test contents")?;
            Ok(test_path)
        }

        fn create_dir(&self, name: &str) -> Result<PathBuf> {
            let dest_path = self.tempdir.path().join(name);
            fs::create_dir_all(&dest_path)?;
            Ok(dest_path)
        }

        fn touch_script(&self, name: &str) -> Result<PathBuf> {
            let file_path = self.tempdir.path().join(name);
            fs::write(&file_path, "#!/bin/sh")?;
            fs::set_permissions(&file_path, Permissions::from_mode(0o755))?;
            Ok(file_path)
        }
    }

    fn test_cfg() -> Result<TestCfg> {
        test_cfg_with(test_config())
    }

    fn test_cfg_with(config: HighlightingConfig) -> Result<TestCfg> {
        let dir = tempfile::tempdir()?;
        let homedir = tempfile::tempdir()?;
        let pwd = dir.path().to_str().unwrap().to_owned();

        let highlighter = HighlighterBuilder::new(&config)
            .home_dir(homedir.path().to_str().unwrap().to_owned())
            .build()?;

        Ok(TestCfg {
            highlighter,
            homedir,
            tempdir: dir,
            pwd,
        })
    }

    /// Test if a simple `echo` command is highlighted correctly
    #[test]
    fn echo() -> Result<()> {
        let cfg = test_cfg()?;

        assert_snapshot!(cfg.highlight("echo")?);

        Ok(())
    }

    #[test]
    fn path_with_emoji() -> Result<()> {
        let cfg = test_cfg()?;
        cfg.touch_file("test🐑.txt")?;
        cfg.touch_file("🐑")?;

        assert_snapshot!(cfg.highlight(r#"cp🐑 "test🐑.txt" 🐑"#)?);

        Ok(())
    }

    #[test]
    fn dynamic_highlighting_disabled() -> Result<()> {
        let mut config = test_config();
        config.dynamic = DynamicConfig {
            callables: false,
            paths: DynamicConfigType::None,
        };
        let mut cfg = test_cfg_with(config)?;
        cfg.touch_file("test.txt")?;

        assert_snapshot!(
            "dynamic_highlighting_disabled__no_callables_no_paths",
            cfg.highlight("ls test.txt")?
        );

        let mut config = test_config();
        config.dynamic = DynamicConfig {
            callables: true,
            paths: DynamicConfigType::None,
        };
        cfg.highlighter = HighlighterBuilder::new(&config).build()?;

        assert_snapshot!(
            "dynamic_highlighting_disabled__no_paths",
            cfg.highlight("ls test.txt")?
        );

        config.dynamic = DynamicConfig {
            callables: false,
            paths: DynamicConfigType::default(),
        };
        cfg.highlighter = HighlighterBuilder::new(&config).build()?;

        assert_snapshot!(
            "dynamic_highlighting_disabled__no_callables",
            cfg.highlight("ls test.txt")?
        );

        Ok(())
    }

    /// Test if a command referring to a file is highlighted correctly
    #[test]
    fn argument_is_file() -> Result<()> {
        let cfg = test_cfg()?;
        cfg.touch_file("test.txt")?;
        cfg.touch_file("test 1.txt")?;

        assert_snapshot!(
            "argument_is_file__simple",
            cfg.highlight("cp test.txt dest.txt")?
        );
        assert_snapshot!(
            "argument_is_file__double_quotes",
            cfg.highlight(r#"cp "test.txt" dest.txt"#)?
        );
        assert_snapshot!(
            "argument_is_file__spaces",
            cfg.highlight(r#"cp   "test.txt"   "dest.txt""#)?
        );
        assert_snapshot!(
            "argument_is_file__filename_with_spaces_is_not_a_path",
            cfg.highlight(r#"cp " test.txt" "dest.txt""#)?
        );
        assert_snapshot!(
            "argument_is_file__double_quotes_inside_filenames",
            cfg.highlight(r#"cp te"st.tx"t dest.txt"#)?
        );
        assert_snapshot!(
            "argument_is_file__double_quotes_test_1",
            cfg.highlight(r#"cp "test 1.txt" dest.txt"#)?
        );
        assert_snapshot!(
            "argument_is_file__escape_whitespace",
            cfg.highlight(r#"cp test\ 1.txt dest.txt"#)?
        );
        assert_snapshot!(
            "argument_is_file__single_quotes",
            cfg.highlight(r#"cp 'test.txt' dest.txt"#)?
        );
        assert_snapshot!(
            "argument_is_file__ansi_quotes",
            cfg.highlight(r#"cp $'test.txt' dest.txt"#)?
        );

        Ok(())
    }

    /// Test if a command referring to a directory is highlighted correctly
    #[test]
    fn argument_is_directory() -> Result<()> {
        let cfg = test_cfg()?;
        cfg.touch_file("test.txt")?;
        cfg.create_dir("dest")?;

        assert_snapshot!(
            "argument_is_directory__simple",
            cfg.highlight("cp test.txt dest")?
        );

        Ok(())
    }

    #[test]
    fn argument_is_partial_file() -> Result<()> {
        let mut config = test_config();
        config.dynamic = DynamicConfig {
            callables: true,
            paths: DynamicConfigType::Partial,
        };
        let cfg = test_cfg_with(config)?;
        cfg.touch_file("test.txt")?;

        let request = HighlightingRequest::default()
            .with_cursor(5)
            .with_pwd(cfg.pwd.as_str());

        assert_snapshot!(
            "argument_is_partial_file__simple",
            AssertableSpans {
                command: "ls te".to_string(),
                spans: cfg.highlighter.highlight("ls te", &request)?,
            }
        );

        Ok(())
    }

    #[test]
    fn argument_is_partial_directory() -> Result<()> {
        let mut config = test_config();
        config.dynamic = DynamicConfig {
            callables: true,
            paths: DynamicConfigType::Partial,
        };
        let cfg = test_cfg_with(config)?;
        cfg.create_dir("dest")?;

        let request = HighlightingRequest::default()
            .with_cursor(5)
            .with_pwd(cfg.pwd.as_str());

        assert_snapshot!(
            "argument_is_partial_directory__simple",
            AssertableSpans {
                command: "rm de".to_string(),
                spans: cfg.highlighter.highlight("rm de", &request)?,
            }
        );

        Ok(())
    }

    /// Test if a command starting with a tilde is highlighted correctly
    #[test]
    fn command_with_tilde() -> Result<()> {
        let cfg = test_cfg()?;

        assert_snapshot!("command_with_tilde__tilde", cfg.highlight("~")?);
        assert_snapshot!("command_with_tilde__tilde_slash", cfg.highlight("~/")?);
        assert_snapshot!(
            "command_with_tilde__tilde_command",
            cfg.highlight("~ echo")?
        );
        assert_snapshot!(
            "command_with_tilde__doesnotexist",
            cfg.highlight("~doesnotexist")?
        );
        assert_snapshot!(
            "command_with_tilde__double_quoted",
            cfg.highlight(r#""~""#)?
        );
        assert_snapshot!(
            "command_with_tilde__double_quoted_slash",
            cfg.highlight(r#""~/""#)?
        );

        Ok(())
    }

    /// Test if a path starting with a tilde is highlighted correctly
    #[test]
    fn path_with_tilde() -> Result<()> {
        let cfg = test_cfg()?;
        cfg.touch_file("test.txt")?;

        assert_snapshot!("path_with_tilde__simple", cfg.highlight("ls ~")?);
        assert_snapshot!("path_with_tilde__simple_slash", cfg.highlight("ls ~/")?);
        assert_snapshot!(
            "path_with_tilde__with_file",
            cfg.highlight("ls ~/ test.txt")?
        );
        assert_snapshot!(
            "path_with_tilde__double_quoted",
            cfg.highlight(r#"ls "~/""#)?
        );
        assert_snapshot!("path_with_tilde__single_quoted", cfg.highlight("ls '~/'")?);
        assert_snapshot!("path_with_tilde__ansi_quoted", cfg.highlight("ls $'~/'")?);
        assert_snapshot!(
            "path_with_tilde__path_does_not_exist",
            cfg.highlight("ls ~/this/path/does/not/exist")?
        );
        assert_snapshot!(
            "path_with_tilde__path_with_tilde_does_not_exist",
            cfg.highlight("ls test/~/")?
        );

        Ok(())
    }

    #[test]
    fn path_followed_by_parameter() -> Result<()> {
        let cfg = test_cfg()?;
        cfg.touch_file("test.txt")?;

        assert_snapshot!(
            "path_followed_by_parameter__simple",
            cfg.highlight("foo test.txt -C")?
        );

        Ok(())
    }

    #[test]
    fn two_commands_followed_by_comment() -> Result<()> {
        let cfg = test_cfg()?;

        // two commands referring two shell scripts that don't exist
        assert_snapshot!(
            "two_commands_followed_by_comment__shell_scripts_do_not_exist",
            cfg.highlight("~/foo/a.sh && ~/foo/b.sh")?
                .to_string()
                .replace(cfg.homedir.path().to_str().unwrap(), "<HOME>")
        );

        // two commands followed by a comment
        assert_snapshot!(
            "two_commands_followed_by_comment__with_comment",
            cfg.highlight("~/foo/a.sh && ~/foo/b.sh # comment")?
                .to_string()
                .replace(cfg.homedir.path().to_str().unwrap(), "<HOME>")
        );

        // two commands but the second path is invalid
        assert_snapshot!(
            "two_commands_followed_by_comment__second_invalid",
            cfg.highlight("~/foo/a.sh && ~/foo/b.sh# comment")?
                .to_string()
                .replace(cfg.homedir.path().to_str().unwrap(), "<HOME>")
        );

        Ok(())
    }

    #[test]
    fn redirection() -> Result<()> {
        let cfg = test_cfg()?;
        cfg.touch_file("test.txt")?;

        assert_snapshot!(
            "redirection__simple",
            cfg.highlight("echo hello > test.txt")?
        );
        assert_snapshot!(
            "redirection__no_spaces",
            cfg.highlight("echo hello>test.txt")?
        );
        assert_snapshot!(
            "redirection__with_env_var_before_string",
            cfg.highlight("echo ${FOO}hello>test.txt")?
        );
        assert_snapshot!(
            "redirection__with_env_var_after_string",
            cfg.highlight("echo hello$FOO>test.txt")?
        );

        Ok(())
    }

    #[test]
    fn double_quoted_callable() -> Result<()> {
        let cfg = test_cfg()?;

        assert_snapshot!("double_quoted_callable__simple", cfg.highlight(r#""ls""#)?);
        assert_snapshot!("double_quoted_callable__inside", cfg.highlight(r#"l"s""#)?);

        cfg.touch_script("script.sh")?;

        assert_snapshot!(
            "double_quoted_callable__script",
            cfg.highlight(r#""./script.sh""#)?
        );

        cfg.create_dir("foo/bar")?;

        assert_snapshot!(
            "double_quoted_callable__dir",
            cfg.highlight(r#"foo/"bar"/"#)?
        );

        Ok(())
    }

    #[test]
    fn escape_unquoted_at_beginning() -> Result<()> {
        let cfg = test_cfg()?;
        cfg.touch_file("test.txt")?;
        cfg.touch_script("script.sh")?;
        cfg.touch_script("s")?;

        assert_snapshot!(
            "escape_unquoted_at_beginning__escape_script",
            cfg.highlight(r"\script.sh")?
        );
        assert_snapshot!(
            "escape_unquoted_at_beginning__escape_dot",
            cfg.highlight(r"\./script.sh")?
        );

        // parser cannot differentiate between normal unquoted character escapes
        // and those that are at the beginning of a callable
        assert_snapshot!(
            "escape_unquoted_at_beginning__escape_s",
            cfg.highlight(r"\s")?
        );

        assert_snapshot!(
            "escape_unquoted_at_beginning__escape_file",
            cfg.highlight(r"touch \test.txt")?
        );

        Ok(())
    }

    #[test]
    fn path_with_escape_unquoted() -> Result<()> {
        let cfg = test_cfg()?;

        assert_snapshot!(
            "path_with_escape_unquoted__does_not_exist",
            cfg.highlight(r"cp test\u2580.txt dest.txt")?
        );

        cfg.touch_file("testu2580.txt")?;

        assert_snapshot!(
            "path_with_escape_unquoted__exists",
            cfg.highlight(r"cp test\u2580.txt dest.txt")?
        );

        Ok(())
    }

    #[test]
    fn path_with_escape_quoted() -> Result<()> {
        let cfg = test_cfg()?;
        cfg.touch_file("test▀.txt")?;
        cfg.touch_file("test  1.txt")?;

        assert_snapshot!(
            "path_with_escape_quoted__unicode_unquoted",
            cfg.highlight(r"cp test\u2580.txt dest.txt")?
        );
        assert_snapshot!(
            "path_with_escape_quoted__unicode_double_quoted",
            cfg.highlight(r#"cp "test\u2580.txt" dest.txt"#)?
        );
        assert_snapshot!(
            "path_with_escape_quoted__unicode_single_quoted",
            cfg.highlight(r#"cp 'test\u2580.txt' dest.txt"#)?
        );
        assert_snapshot!(
            "path_with_escape_quoted__unicode_ansi_quoted",
            cfg.highlight(r#"cp $'test\u2580.txt' dest.txt"#)?
        );
        assert_snapshot!(
            "path_with_escape_quoted__whitespace_unquoted",
            cfg.highlight(r#"cp test\ \ 1.txt dest.txt"#)?
        );
        assert_snapshot!(
            "path_with_escape_quoted__whitespace_double_quoted",
            cfg.highlight(r#"cp "test\ \ 1.txt" dest.txt"#)?
        );
        assert_snapshot!(
            "path_with_escape_quoted__whitespace_ansi_quoted",
            cfg.highlight(r#"cp $'test\ \ 1.txt' dest.txt"#)?
        );
        assert_snapshot!(
            "path_with_escape_quoted__hex_ansi_quoted",
            cfg.highlight(r#"cp $'test\x20\x201.txt' dest.txt"#)?
        );
        assert_snapshot!(
            "path_with_escape_quoted__hex_ansi_quoted_inside",
            cfg.highlight(r#"cp test$'\x20\x20'1.txt dest.txt"#)?
        );

        Ok(())
    }

    #[test]
    fn command_with_multibyte_escape() -> Result<()> {
        let cfg = test_cfg()?;
        let subdir = cfg.create_dir("sub")?;
        let test_path = subdir.join("test😎.sh");
        fs::write(&test_path, "#!/bin/sh")?;
        fs::set_permissions(&test_path, Permissions::from_mode(0o755))?;

        assert_snapshot!(
            "command_with_multibyte_escape__all_hex",
            cfg.highlight(r"$'sub/test\xF0\x9F\x98\x8E.sh'")?
        );
        assert_snapshot!(
            "command_with_multibyte_escape__with_oct",
            cfg.highlight(r"$'sub/test\xF0\237\x98\x8E.sh'")?
        );

        Ok(())
    }

    #[test]
    fn path_with_multibyte_escape() -> Result<()> {
        let cfg = test_cfg()?;
        cfg.touch_file("test😎.txt")?;

        assert_snapshot!(
            "path_with_multibyte_escape__all_hex",
            cfg.highlight(r"cp $'test\xF0\x9F\x98\x8E.txt' dest.txt")?
        );
        assert_snapshot!(
            "path_with_multibyte_escape__with_oct",
            cfg.highlight(r"cp $'test\xF0\237\x98\x8E.txt' dest.txt")?
        );

        Ok(())
    }

    #[test]
    fn multiline() -> Result<()> {
        let cfg = test_cfg()?;
        cfg.touch_file("test.txt")?;

        assert_snapshot!(
            "multiline__commit",
            cfg.highlight(
                "foo commit -m \"This is\na multi-line commit\nmessage\" && touch test.txt"
            )?
        );

        Ok(())
    }

    #[test]
    fn path_with_env_variable() -> Result<()> {
        let cfg = test_cfg()?;
        cfg.touch_file("test.txt")?;
        cfg.touch_file("test.txt$FOOBAR")?;

        assert_snapshot!(
            "path_with_env_variable__after_file",
            cfg.highlight(r"ls test.txt$FOOBAR")?
        );
        assert_snapshot!(
            "path_with_env_variable__before_file",
            cfg.highlight(r"ls ${FOOBAR}test.txt test.txt")?
        );
        assert_snapshot!(
            "path_with_env_variable__inside_file_does_not_exist",
            cfg.highlight(r"ls test.txt${FOOBAR}test.txt test.txt")?
        );

        Ok(())
    }

    #[test]
    fn path_with_command_substitution() -> Result<()> {
        let cfg = test_cfg()?;
        cfg.touch_file("test.txt")?;
        cfg.touch_file("test.txtFOOBAR")?;

        assert_snapshot!(
            "path_with_command_substitution__does_not_exist",
            cfg.highlight(r"ls test.txt$(echo FOOBAR)")?
        );

        cfg.touch_file("FOOBAR")?;

        assert_snapshot!(
            "path_with_command_substitution__exists",
            cfg.highlight(r"ls test.txt$(echo FOOBAR)")?
        );

        cfg.touch_file("test2.txt")?;

        assert_snapshot!(
            "path_with_command_substitution__two_files",
            cfg.highlight(r"ls test.txt test.txt$(echo FOOBAR) test2.txt")?
        );

        Ok(())
    }

    #[test]
    fn keyword_dash() -> Result<()> {
        let cfg = test_cfg()?;

        assert_snapshot!("keyword_dash__foobar", cfg.highlight(r"- foobar")?);

        Ok(())
    }

    #[test]
    fn builtin() -> Result<()> {
        let cfg = test_cfg()?;

        assert_snapshot!("builtin__echo", cfg.highlight(r"builtin echo")?);

        Ok(())
    }

    #[test]
    fn command() -> Result<()> {
        let cfg = test_cfg()?;

        assert_snapshot!("command__echo", cfg.highlight(r"command echo")?);
        assert_snapshot!("command__p", cfg.highlight(r"command -p echo")?);
        assert_snapshot!(
            "command__p_with_file",
            cfg.highlight("command -p echo file")?
        );
        assert_snapshot!(
            "command__p_with_double_quoted_file",
            cfg.highlight(r#"command -p echo "file""#)?
        );
        assert_snapshot!(
            "command__p_command_with_bigv",
            cfg.highlight("command -p echo -V file")?
        );
        assert_snapshot!("command__v", cfg.highlight("command -v echo")?);
        assert_snapshot!(
            "command__v_two_commands",
            cfg.highlight("command -v echo file")?
        );
        assert_snapshot!(
            "command__v_two_commands_double_quoted",
            cfg.highlight(r#"command -v echo "file""#)?
        );
        assert_snapshot!("command__bigv", cfg.highlight("command -V echo")?);
        assert_snapshot!(
            "command__bigv_two_commands",
            cfg.highlight("command -V echo file")?
        );
        assert_snapshot!(
            "command__p_bigv_two_commands",
            cfg.highlight("command -p -V echo file")?
        );
        assert_snapshot!(
            "command__bigv_p_two_commands",
            cfg.highlight("command -V -p echo file")?
        );
        assert_snapshot!(
            "command__bigv_command_with_p",
            cfg.highlight("command -V echo -p file")?
        );
        assert_snapshot!("command__p_v_bigv", cfg.highlight("command -pvV echo")?);
        assert_snapshot!(
            "command__p_v_bigv_two_commands",
            cfg.highlight("command -pvV echo file")?
        );
        assert_snapshot!("command__end_of_options", cfg.highlight("command -- echo")?);
        assert_snapshot!(
            "command__p_end_of_options",
            cfg.highlight("command -p -- echo")?
        );
        assert_snapshot!(
            "command__v_end_of_options_two_commands",
            cfg.highlight("command -v -- echo file")?
        );

        Ok(())
    }

    #[test]
    fn exec() -> Result<()> {
        let cfg = test_cfg()?;

        assert_snapshot!("exec__foobar", cfg.highlight("exec foobar")?);
        assert_snapshot!("exec__c", cfg.highlight("exec -c foobar")?);
        assert_snapshot!("exec__l", cfg.highlight("exec -l foobar")?);
        assert_snapshot!("exec__cl", cfg.highlight("exec -cl foobar")?);
        assert_snapshot!("exec__a", cfg.highlight("exec -a zsh foobar $0")?);
        assert_snapshot!(
            "exec__a_double_quoted",
            cfg.highlight(r#"exec -a "zsh" foobar $0"#)?
        );
        assert_snapshot!("exec__c_a", cfg.highlight("exec -c -a zsh foobar $0")?);
        assert_snapshot!("exec__c_a_l", cfg.highlight("exec -c -a zsh -l foobar $0")?);
        assert_snapshot!("exec__ca", cfg.highlight("exec -ca zsh foobar $0")?);
        assert_snapshot!("exec__end_of_options", cfg.highlight("exec -- foobar")?);
        assert_snapshot!(
            "exec__subshell",
            cfg.highlight(r#"(exec -a foobar -- zsh -c 'echo "$0"')"#)?
        );
        assert_snapshot!(
            "exec__subshell_a_is_double_dash",
            cfg.highlight(r#"(exec -a -- zsh -c 'echo "$0"')"#)?
        );
        assert_snapshot!(
            "exec__subshell_a_is_double_dash_end_of_options",
            cfg.highlight(r#"(exec -a -- -- zsh -c 'echo "$0"')"#)?
        );

        Ok(())
    }

    #[test]
    fn noglob() -> Result<()> {
        let cfg = test_cfg()?;

        assert_snapshot!("noglob__toml", cfg.highlight("noglob ls *.toml")?);

        Ok(())
    }

    #[test]
    fn repeat() -> Result<()> {
        let cfg = test_cfg()?;

        assert_snapshot!("repeat__5", cfg.highlight("repeat 5 echo Hello")?);
        assert_snapshot!(
            "repeat__expression",
            cfg.highlight("repeat 1+9/2 echo Hello")?
        );
        assert_snapshot!(
            "repeat__do_done",
            cfg.highlight("repeat 1 do; echo Hello; done")?
        );
        assert_snapshot!(
            "repeat__missing_number",
            cfg.highlight("repeat echo Hello")?
        );
        assert_snapshot!(
            "repeat__missing_number_do_done",
            cfg.highlight("repeat do; echo Hello; done")?
        );

        Ok(())
    }

    #[test]
    fn time() -> Result<()> {
        let cfg = test_cfg()?;

        assert_snapshot!("time__sleep", cfg.highlight("time sleep 2")?);

        Ok(())
    }

    #[test]
    fn nocorrect() -> Result<()> {
        let cfg = test_cfg()?;

        assert_snapshot!("nocorrect__typo", cfg.highlight("nocorrect slep 2")?);

        Ok(())
    }

    #[test]
    fn select() -> Result<()> {
        let cfg = test_cfg()?;

        assert_snapshot!(
            "select__simple",
            cfg.highlight("select x in a b c; do echo $x; done")?
        );
        assert_snapshot!(
            "select__without_list",
            cfg.highlight("select x; do echo $x; done")?
        );
        assert_snapshot!(
            "select__with_break",
            cfg.highlight("select x in a b; do break; done")?
        );
        assert_snapshot!(
            "select__newlines_instead_of_semicolons",
            cfg.highlight("select x in a b\ndo\necho $x\ndone")?
        );
        assert_snapshot!(
            "select__with_case",
            cfg.highlight("select x in a b; do case $x in a) echo yes;; esac; done")?
        );

        Ok(())
    }

    #[test]
    fn foreach() -> Result<()> {
        let cfg = test_cfg()?;

        assert_snapshot!(
            "foreach__simple",
            cfg.highlight("foreach x (a b c); echo $x; end")?
        );
        assert_snapshot!(
            "foreach__newlines_instead_of_semicolons",
            cfg.highlight("foreach x (a b c)\necho $x\nend")?
        );
        assert_snapshot!(
            "foreach__with_break",
            cfg.highlight("foreach x (a b c);echo $x; break ; end")?
        );

        Ok(())
    }

    #[test]
    fn doas() -> Result<()> {
        let cfg = test_cfg()?;

        assert_snapshot!(
            "doas__n_u_c",
            cfg.highlight("doas -n -u root -C doas.conf ls")?
        );

        cfg.touch_file("doas.conf")?;

        assert_snapshot!(
            "doas__n_u_c_conf_file_exists",
            cfg.highlight("doas -n -uroot -Cdoas.conf -- ls")?
        );

        Ok(())
    }

    #[test]
    fn env() -> Result<()> {
        let cfg = test_cfg()?;
        cfg.create_dir("mydir")?;

        assert_snapshot!("env__i", cfg.highlight("env -i ls")?);
        assert_snapshot!(
            "env__ignore_environment",
            cfg.highlight("env --ignore-environment ls")?
        );
        assert_snapshot!("env__double_i", cfg.highlight("env -ii ls")?);
        assert_snapshot!(
            "env__c_existing_directory",
            cfg.highlight("env -C mydir ls")?
        );
        assert_snapshot!(
            "env__c_missing_directory",
            cfg.highlight("env -C foobar ls")?
        );
        assert_snapshot!(
            "env__c_existing_directory_no_space",
            cfg.highlight("env -Cmydir ls")?
        );
        assert_snapshot!(
            "env__c_missing_directory_no_space",
            cfg.highlight("env -Cfoobar ls")?
        );
        assert_snapshot!("env__i_u", cfg.highlight("env -i -u _ env")?);
        assert_snapshot!("env__i_unset", cfg.highlight("env -i --unset=_ env")?);
        assert_snapshot!("env__iu", cfg.highlight("env -iu _ env")?);
        assert_snapshot!(
            "env__p_existing_directory",
            cfg.highlight("env -P mydir ls")?
        );
        assert_snapshot!(
            "env__p_missing_directory",
            cfg.highlight("env -P foobar ls")?
        );
        assert_snapshot!(
            "env__s_c_existing_directory",
            cfg.highlight("env -S -C mydir ls")?
        );
        assert_snapshot!(
            "env__s_c_missing_directory",
            cfg.highlight("env -S -C foobar ls")?
        );
        assert_snapshot!(
            "env__s_c_existing_directory_no_space",
            cfg.highlight("env -S -Cmydir ls")?
        );
        assert_snapshot!(
            "env__s_p_existing_directory",
            cfg.highlight("env -S -P mydir ls")?
        );
        assert_snapshot!(
            "env__s_p_missing_directory",
            cfg.highlight("env -S -P foobar ls")?
        );
        assert_snapshot!(
            "env__s_p_existing_directory_no_space",
            cfg.highlight("env -S -Pmydir ls")?
        );
        assert_snapshot!(
            "env__s_single_quoted_c",
            cfg.highlight("env -S '-C target' ls")?
        );
        assert_snapshot!(
            "env__s_single_quoted_p",
            cfg.highlight("env -S '-P mydir' ls")?
        );
        assert_snapshot!("env__u", cfg.highlight("env -u _ env")?);
        assert_snapshot!("env__u_no_space", cfg.highlight("env -u_ env")?);
        assert_snapshot!("env__i_var", cfg.highlight("env -i bar=foo env")?);
        assert_snapshot!(
            "env__i_var_end_of_options",
            cfg.highlight("env -i -- bar=foo env")?
        );
        assert_snapshot!("env__unset", cfg.highlight("env --unset _ env")?);
        assert_snapshot!("env__chdir", cfg.highlight("env --chdir mydir env")?);
        assert_snapshot!(
            "env__split_string",
            cfg.highlight(r#"env --split-string "-C target" env"#)?
        );

        Ok(())
    }

    #[test]
    fn nice() -> Result<()> {
        let cfg = test_cfg()?;

        assert_snapshot!("nice__simple", cfg.highlight("nice ls")?);
        assert_snapshot!(
            "nice__version_help",
            cfg.highlight("nice --version && nice --help")?
        );
        assert_snapshot!("nice__n", cfg.highlight("nice -n 5 date")?);
        assert_snapshot!("nice__n_no_space", cfg.highlight("nice -n5 date")?);
        assert_snapshot!("nice__twice", cfg.highlight("nice -n 16 nice -n -35 date")?);
        assert_snapshot!("nice__end_of_options", cfg.highlight("nice -n 5 -- date")?);
        assert_snapshot!(
            "nice__adjustment_equals",
            cfg.highlight("nice --adjustment=5 date")?
        );
        assert_snapshot!(
            "nice__adjustment",
            cfg.highlight("nice --adjustment 5 date")?
        );

        Ok(())
    }

    #[test]
    fn nohup() -> Result<()> {
        let cfg = test_cfg()?;

        assert_snapshot!("nohup__simple", cfg.highlight("nohup ls")?);
        assert_snapshot!(
            "nohup__version_help",
            cfg.highlight("nohup --version && nohup --help")?
        );
        assert_snapshot!("nohup__end_of_options", cfg.highlight("nohup -- ls")?);

        Ok(())
    }

    #[test]
    fn sudo() -> Result<()> {
        let cfg = test_cfg()?;

        assert_snapshot!("sudo__simple", cfg.highlight("sudo ls")?);
        assert_snapshot!("sudo__n", cfg.highlight("sudo -n ls")?);
        assert_snapshot!(
            "sudo__n_u_end_of_options",
            cfg.highlight("sudo -n -u root -- ls")?
        );
        assert_snapshot!(
            "sudo__ng_end_of_options",
            cfg.highlight("sudo -ng wheel -- ls")?
        );
        assert_snapshot!(
            "sudo__version_help",
            cfg.highlight("sudo --version && sudo --help")?
        );
        assert_snapshot!("sudo__user", cfg.highlight("sudo --user=root ls")?);

        cfg.create_dir("mydir")?;

        assert_snapshot!("sudo__chdir", cfg.highlight("sudo --chdir mydir ls")?);
        assert_snapshot!("sudo__h_host", cfg.highlight("sudo -h localhost -l")?);
        assert_snapshot!("sudo__h_i", cfg.highlight("sudo -h -i ls")?);
        assert_snapshot!("sudo__h_help", cfg.highlight("sudo -h && ls")?);

        Ok(())
    }

    #[test]
    fn sudoedit() -> Result<()> {
        let cfg = test_cfg()?;

        cfg.touch_file("file1")?;
        cfg.touch_file("file2")?;

        assert_snapshot!("sudoedit__sudo_e", cfg.highlight("sudo -e file1 file2")?);
        assert_snapshot!(
            "sudoedit__sudo_i_e",
            cfg.highlight("sudo -i -e -- file1 file2")?
        );
        assert_snapshot!(
            "sudoedit__sudo_ie",
            cfg.highlight("sudo -ie -- file1 file2")?
        );
        assert_snapshot!(
            "sudoedit__u_g",
            cfg.highlight("sudoedit -u user -g wheel file1 file2")?
        );

        Ok(())
    }

    #[test]
    fn history_expansions() -> Result<()> {
        let cfg = test_cfg()?;

        assert_snapshot!("history_expansions__last_command", cfg.highlight("!!")?);
        assert_snapshot!(
            "history_expansions__ls_last_command",
            cfg.highlight("ls !!")?
        );
        assert_snapshot!(
            "history_expansions__ls_then_last_command",
            cfg.highlight("ls; !!")?
        );
        assert_snapshot!(
            "history_expansions__echo_last_command",
            cfg.highlight(r#"echo !! "!!""#)?
        );
        assert_snapshot!(
            "history_expansions__back_twenty_foobar",
            cfg.highlight("!-20 foobar")?
        );
        assert_snapshot!("history_expansions__fourth_echo", cfg.highlight("!4echo")?);
        assert_snapshot!(
            "history_expansions__vi_first_word",
            cfg.highlight("vi !!:0")?
        );
        assert_snapshot!(
            "history_expansions__vi_twentieth_word_bak",
            cfg.highlight("vi !!:20.bak")?
        );
        assert_snapshot!(
            "history_expansions__command_last_word_with_parameters",
            cfg.highlight("command !!:$ next parameter")?
        );
        assert_snapshot!(
            "history_expansions__ls_last_word",
            cfg.highlight("ls -l !!:$")?
        );
        assert_snapshot!(
            "history_expansions__most_recent_search",
            cfg.highlight("!% stop")?
        );
        assert_snapshot!(
            "history_expansions__backward_search",
            cfg.highlight("!zsh-patina")?
        );
        assert_snapshot!(
            "history_expansions__backward_search_delimited",
            cfg.highlight(r#"echo "!?cbr?-i""#)?
        );

        Ok(())
    }

    #[test]
    fn history_expansions_disabled() -> Result<()> {
        let cfg = test_cfg()?;

        assert_snapshot!(
            "history_expansions_disabled__last_command",
            cfg.highlight_with_request(
                "ls !!",
                HighlightingRequest::default()
                    .with_history_expansions(false)
                    .with_pwd(cfg.pwd.as_str()),
            )?
        );

        Ok(())
    }

    #[test]
    fn percent_jobs_are_still_job_expansions() -> Result<()> {
        let cfg = test_cfg()?;

        assert_snapshot!(
            "percent_jobs_are_still_job_expansions__fg",
            cfg.highlight("fg %")?
        );
        assert_snapshot!(
            "percent_jobs_are_still_job_expansions__fg1",
            cfg.highlight("fg %1")?
        );

        Ok(())
    }

    #[test]
    fn percent_formats_are_not_job_expansions() -> Result<()> {
        let cfg = test_cfg()?;

        assert_snapshot!(
            "percent_formats_are_not_job_expansions__date",
            cfg.highlight("date +%s")?
        );
        assert_snapshot!(
            "percent_formats_are_not_job_expansions__git_log",
            cfg.highlight("git log --pretty=%aD")?
        );

        Ok(())
    }

    /// see https://github.com/michel-kraemer/zsh-patina/issues/45
    #[test]
    fn case_with_heredoc() -> Result<()> {
        let cmd = r#"
            case "$a" in
                *) cat
            esac <<'EOF'
            A
            B
            C
            EOF"#;

        let cfg = test_cfg()?;
        assert_snapshot!(cfg.highlight(cmd)?);

        Ok(())
    }

    fn static_span_with(
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
        let base = vec![static_span_with(0, 5, Some("red"), None, false, false)];
        assert_eq!(mix_spans(base.clone(), vec![]), base);
    }

    /// A mixin that doesn't overlap any base span is still kept
    #[test]
    fn mix_spans_non_overlapping_mixin_kept() {
        let base = vec![static_span_with(0, 3, Some("red"), None, false, false)];
        let mixins = vec![static_span_with(5, 8, Some("blue"), None, false, false)];
        assert_eq!(
            mix_spans(base, mixins),
            vec![
                static_span_with(0, 3, Some("red"), None, false, false),
                static_span_with(5, 8, Some("blue"), None, false, false),
            ]
        );
    }

    /// A mixin that fully covers a base span: the entire base is overridden
    #[test]
    fn mix_spans_mixin_fully_covers_base() {
        let base = vec![static_span_with(2, 6, Some("red"), None, false, false)];
        let mixins = vec![static_span_with(2, 6, Some("blue"), None, true, false)];
        assert_eq!(
            mix_spans(base, mixins),
            vec![static_span_with(2, 6, Some("blue"), None, true, false)]
        );
    }

    /// A mixin that partially overlaps the start of a base span
    #[test]
    fn mix_spans_mixin_overlaps_start() {
        let base = vec![static_span_with(2, 8, Some("red"), None, false, false)];
        let mixins = vec![static_span_with(0, 4, Some("blue"), None, false, false)];
        assert_eq!(
            mix_spans(base, mixins),
            vec![
                // mixin before base + intersection merged (same style)
                static_span_with(0, 4, Some("blue"), None, false, false),
                // remainder of base (4..8)
                static_span_with(4, 8, Some("red"), None, false, false),
            ]
        );
    }

    /// A mixin that partially overlaps the end of a base span
    #[test]
    fn mix_spans_mixin_overlaps_end() {
        let base = vec![static_span_with(0, 5, Some("red"), None, false, false)];
        let mixins = vec![static_span_with(3, 8, Some("blue"), None, false, false)];
        assert_eq!(
            mix_spans(base, mixins),
            vec![
                // base before overlap (0..3)
                static_span_with(0, 3, Some("red"), None, false, false),
                // intersection + mixin after base merged (same style)
                static_span_with(3, 8, Some("blue"), None, false, false),
            ]
        );
    }

    /// A mixin fully contained within a base span splits it into three parts
    #[test]
    fn mix_spans_mixin_inside_base() {
        let base = vec![static_span_with(
            0,
            10,
            Some("red"),
            Some("white"),
            false,
            false,
        )];
        let mixins = vec![static_span_with(3, 7, None, Some("black"), true, false)];
        assert_eq!(
            mix_spans(base, mixins),
            vec![
                // base before overlap
                static_span_with(0, 3, Some("red"), Some("white"), false, false),
                // intersection: mixin bg overrides, mixin bold overrides, base fg kept (mixin fg is None)
                static_span_with(3, 7, Some("red"), Some("black"), true, false),
                // base after overlap
                static_span_with(7, 10, Some("red"), Some("white"), false, false),
            ]
        );
    }

    /// Multiple mixins overlapping a single base span
    #[test]
    fn mix_spans_multiple_mixins_one_base() {
        let base = vec![static_span_with(0, 10, Some("red"), None, false, false)];
        let mixins = vec![
            static_span_with(1, 3, Some("green"), None, false, false),
            static_span_with(5, 7, Some("blue"), None, false, false),
        ];
        assert_eq!(
            mix_spans(base, mixins),
            vec![
                static_span_with(0, 1, Some("red"), None, false, false),
                static_span_with(1, 3, Some("green"), None, false, false),
                static_span_with(3, 5, Some("red"), None, false, false),
                static_span_with(5, 7, Some("blue"), None, false, false),
                static_span_with(7, 10, Some("red"), None, false, false),
            ]
        );
    }

    /// Mixin with None fg preserves base fg; mixin with Some bg overrides
    #[test]
    fn mix_spans_style_merge_none_preserved() {
        let base = vec![static_span_with(
            0,
            4,
            Some("red"),
            Some("white"),
            true,
            false,
        )];
        let mixins = vec![static_span_with(0, 4, None, Some("black"), false, true)];
        assert_eq!(
            mix_spans(base, mixins),
            vec![
                // fg: base (mixin is None), bg: mixin, bold: base (mixin is false), underline: mixin (true)
                static_span_with(0, 4, Some("red"), Some("black"), true, true),
            ]
        );
    }

    /// Non-overlapping mixin between two base spans is kept in order
    #[test]
    fn mix_spans_mixin_between_bases() {
        let base = vec![
            static_span_with(0, 3, Some("red"), None, false, false),
            static_span_with(7, 10, Some("green"), None, false, false),
        ];
        let mixins = vec![static_span_with(4, 6, Some("blue"), None, false, false)];
        assert_eq!(
            mix_spans(base, mixins),
            vec![
                static_span_with(0, 3, Some("red"), None, false, false),
                static_span_with(4, 6, Some("blue"), None, false, false),
                static_span_with(7, 10, Some("green"), None, false, false),
            ]
        );
    }

    /// A single mixin spanning across two base spans
    #[test]
    fn mix_spans_mixin_spans_two_bases() {
        let base = vec![
            static_span_with(0, 4, Some("red"), None, false, false),
            static_span_with(6, 10, Some("green"), None, false, false),
        ];
        let mixins = vec![static_span_with(2, 8, Some("blue"), None, false, false)];
        assert_eq!(
            mix_spans(base, mixins),
            vec![
                // base[0] before overlap
                static_span_with(0, 2, Some("red"), None, false, false),
                // base[0] ∩ mixin + gap + base[1] ∩ mixin merged (same style)
                static_span_with(2, 8, Some("blue"), None, false, false),
                // base[1] after overlap
                static_span_with(8, 10, Some("green"), None, false, false),
            ]
        );
    }

    /// No base spans: all mixins are kept
    #[test]
    fn mix_spans_no_base() {
        let mixins = vec![
            static_span_with(0, 3, Some("blue"), None, false, false),
            static_span_with(5, 8, Some("green"), None, false, false),
        ];
        assert_eq!(mix_spans(vec![], mixins.clone()), mixins,);
    }
}
