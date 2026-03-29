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
    config::HighlightingConfig,
    highlighting::dynamic::{DynamicScopes, DynamicTokenGroupBuilder, DynamicType},
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

pub struct Highlighter {
    home_dir: String,
    max_line_length: usize,
    timeout: Duration,
    dynamic_callables_enabled: bool,
    dynamic_arguments_enabled: bool,
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
            dynamic_arguments_enabled: config.dynamic.paths,
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

    pub fn highlight<P>(&self, command: &str, pwd: Option<&str>, predicate: P) -> Result<Vec<Span>>
    where
        P: Fn(&Range<usize>) -> bool,
    {
        let start = Instant::now();

        let syntax = self.syntax_set.find_syntax_by_extension("sh").unwrap();

        let mut parse_state = ParseState::new(syntax);
        let syntect_highlighter = SyntectHighlighter::new(&self.syntect_theme);
        let mut highlight_state = HighlightState::new(&syntect_highlighter, ScopeStack::new());

        let mut dynamic_builder = DynamicTokenGroupBuilder::new(self.dynamic_scopes);
        let mut mixins = Vec::new();

        let mut i = 0;
        let mut byte_offset = 0;
        let mut result = Vec::new();
        for line in LinesWithEndings::from(command.trim_ascii_end()) {
            if line.len() > self.max_line_length {
                // skip lines that are too long
                byte_offset += line.len();
                continue;
            }

            if start.elapsed() > self.timeout {
                // stop if highlighting takes too long
                return Ok(result);
            }

            let ops = parse_state.parse_line(line, &self.syntax_set)?;
            let ranges =
                HighlightIterator::new(&mut highlight_state, &ops, line, &syntect_highlighter);

            for r in ranges {
                if r.1.is_empty() {
                    continue;
                }

                // this is O(n) but necessary in case the command contains
                // multi-byte characters
                let len = r.1.chars().count();

                if let Some(scope) = self.scope_mapping.decode(&r.0.foreground) {
                    let range = i..i + len;
                    if predicate(&range) {
                        self.highlight_other(range, scope, &mut result);
                    }
                }

                i += len;
            }

            // perform dynamic highlighting
            if (self.dynamic_callables_enabled || self.dynamic_arguments_enabled)
                && let Some(pwd) = pwd
            {
                for g in dynamic_builder.build(&ops, byte_offset) {
                    if self.should_highlight_dynamic(&g.dynamic_type)
                        && let Ok(group_spans) =
                            g.highlight(command, pwd, &self.home_dir, &self.theme)
                    {
                        mixins.extend(group_spans);
                    }
                }
            }

            byte_offset += line.len();
        }

        // perform dynamic highlighting for the remaining groups
        if (self.dynamic_callables_enabled || self.dynamic_arguments_enabled)
            && let Some(pwd) = pwd
        {
            for g in dynamic_builder.finish(byte_offset) {
                if self.should_highlight_dynamic(&g.dynamic_type)
                    && let Ok(group_spans) = g.highlight(command, pwd, &self.home_dir, &self.theme)
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
                    .filter(|m| predicate(&(m.start..m.end)))
                    .collect(),
            );
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

    fn should_highlight_dynamic(&self, dynamic_type: &DynamicType) -> bool {
        match dynamic_type {
            DynamicType::Unknown => true,
            DynamicType::Callable => self.dynamic_callables_enabled,
            DynamicType::Arguments => self.dynamic_arguments_enabled,
        }
    }

    pub fn tokenize(&self, command: &str) -> Result<Vec<Token>> {
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
        path::PathBuf,
    };

    use crate::config::DynamicConfig;

    use super::*;
    use anyhow::Result;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    fn test_config() -> HighlightingConfig {
        HighlightingConfig {
            timeout: Duration::from_secs(3600),
            ..Default::default()
        }
    }

    struct TestCfg {
        highlighter: Highlighter,
        _homedir: TempDir,
        tempdir: TempDir,
        pwd: String,
    }

    impl TestCfg {
        fn highlight(&self, command: &str) -> Result<Vec<Span>> {
            self.highlighter
                .highlight(command, Some(&self.pwd), |_| true)
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

        fn static_span(&self, start: usize, end: usize, scope: &str) -> Result<Span> {
            let style = resolve_static_style(scope, &self.highlighter.theme)
                .with_context(|| format!("Unable to resolve style for scope {scope}"))?;
            Ok(Span {
                start,
                end,
                style: SpanStyle::Static(style),
            })
        }

        fn dynamic_span(&self, start: usize, end: usize, parsed_callable: &str) -> Span {
            Span {
                start,
                end,
                style: SpanStyle::Dynamic(DynamicStyle::Callable {
                    parsed_callable: parsed_callable.to_string(),
                }),
            }
        }

        fn mixed_span(&self, start: usize, end: usize, a: &str, b: &str) -> Result<Span> {
            let a_style = resolve_static_style(a, &self.highlighter.theme)
                .with_context(|| format!("Unable to resolve style for scope {a}"))?;
            let b_style = resolve_static_style(b, &self.highlighter.theme)
                .with_context(|| format!("Unable to resolve style for scope {b}"))?;
            Ok(Span {
                start,
                end,
                style: mix_styles(&SpanStyle::Static(a_style), &SpanStyle::Static(b_style)),
            })
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
            _homedir: homedir,
            tempdir: dir,
            pwd,
        })
    }

    /// Test if a simple `echo` command is highlighted correctly
    #[test]
    fn echo() -> Result<()> {
        let cfg = test_cfg()?;
        let highlighted = cfg.highlight("echo")?;
        assert_eq!(highlighted, vec![cfg.dynamic_span(0, 4, "echo")]);
        Ok(())
    }

    #[test]
    fn path_with_emoji() -> Result<()> {
        let cfg = test_cfg()?;
        cfg.touch_file("test🐑.txt")?;
        cfg.touch_file("🐑")?;

        let highlighted = cfg.highlight(r#"cp🐑 "test🐑.txt" 🐑"#)?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 3, "cp🐑"),
                cfg.mixed_span(4, 15, STRING_QUOTED_DOUBLE, DYNAMIC_PATH_FILE)?,
                cfg.static_span(16, 17, DYNAMIC_PATH_FILE)?,
            ]
        );
        Ok(())
    }

    #[test]
    fn dynamic_highlighting_disabled() -> Result<()> {
        let mut config = test_config();
        config.dynamic = DynamicConfig {
            callables: false,
            paths: false,
        };
        let mut cfg = test_cfg_with(config)?;
        cfg.touch_file("test.txt")?;

        let highlighted = cfg.highlight("ls test.txt")?;
        assert_eq!(highlighted, vec![cfg.static_span(0, 2, CALLABLE)?]);

        let mut config = test_config();
        config.dynamic = DynamicConfig {
            callables: true,
            paths: false,
        };
        cfg.highlighter = HighlighterBuilder::new(&config).build()?;

        let highlighted = cfg.highlight("ls test.txt")?;
        assert_eq!(highlighted, vec![cfg.dynamic_span(0, 2, "ls")]);

        config.dynamic = DynamicConfig {
            callables: false,
            paths: true,
        };
        cfg.highlighter = HighlighterBuilder::new(&config).build()?;

        let highlighted = cfg.highlight("ls test.txt")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.static_span(0, 2, CALLABLE)?,
                cfg.static_span(3, 11, DYNAMIC_PATH_FILE)?
            ]
        );

        Ok(())
    }

    /// Test if a command referring to a file is highlighted correctly
    #[test]
    fn argument_is_file() -> Result<()> {
        let cfg = test_cfg()?;
        cfg.touch_file("test.txt")?;
        cfg.touch_file("test 1.txt")?;

        let highlighted = cfg.highlight("cp test.txt dest.txt")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "cp"),
                cfg.static_span(3, 11, DYNAMIC_PATH_FILE)?,
            ]
        );

        let highlighted = cfg.highlight(r#"cp "test.txt" dest.txt"#)?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "cp"),
                cfg.mixed_span(3, 13, STRING_QUOTED_DOUBLE, DYNAMIC_PATH_FILE)?,
            ]
        );

        let highlighted = cfg.highlight(r#"cp   "test.txt"   "dest.txt""#)?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "cp"),
                cfg.mixed_span(5, 15, STRING_QUOTED_DOUBLE, DYNAMIC_PATH_FILE)?,
                cfg.static_span(18, 28, STRING_QUOTED_DOUBLE)?,
            ]
        );

        let highlighted = cfg.highlight(r#"cp " test.txt" "dest.txt""#)?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "cp"),
                cfg.static_span(3, 14, STRING_QUOTED_DOUBLE)?,
                cfg.static_span(15, 25, STRING_QUOTED_DOUBLE)?,
            ]
        );

        let highlighted = cfg.highlight(r#"cp te"st.tx"t dest.txt"#)?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "cp"),
                cfg.static_span(3, 5, DYNAMIC_PATH_FILE)?,
                cfg.mixed_span(5, 12, STRING_QUOTED_DOUBLE, DYNAMIC_PATH_FILE)?,
                cfg.static_span(12, 13, DYNAMIC_PATH_FILE)?,
            ]
        );

        let highlighted = cfg.highlight(r#"cp "test 1.txt" dest.txt"#)?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "cp"),
                cfg.mixed_span(3, 15, STRING_QUOTED_DOUBLE, DYNAMIC_PATH_FILE)?,
            ]
        );

        let highlighted = cfg.highlight(r#"cp test\ 1.txt dest.txt"#)?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "cp"),
                cfg.static_span(3, 7, DYNAMIC_PATH_FILE)?,
                cfg.mixed_span(7, 9, CHARACTER_ESCAPE, DYNAMIC_PATH_FILE)?,
                cfg.static_span(9, 14, DYNAMIC_PATH_FILE)?,
            ]
        );

        let highlighted = cfg.highlight(r#"cp 'test.txt' dest.txt"#)?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "cp"),
                cfg.mixed_span(3, 13, STRING_QUOTED_DOUBLE, DYNAMIC_PATH_FILE)?,
            ]
        );

        let highlighted = cfg.highlight(r#"cp $'test.txt' dest.txt"#)?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "cp"),
                cfg.mixed_span(3, 14, STRING_QUOTED_DOUBLE, DYNAMIC_PATH_FILE)?,
            ]
        );

        Ok(())
    }

    /// Test if a command referring to a directory is highlighted correctly
    #[test]
    fn argument_is_directory() -> Result<()> {
        let cfg = test_cfg()?;
        cfg.touch_file("test.txt")?;
        cfg.create_dir("dest")?;

        let highlighted = cfg.highlight("cp test.txt dest")?;

        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "cp"),
                cfg.static_span(3, 11, DYNAMIC_PATH_FILE)?,
                cfg.static_span(12, 16, DYNAMIC_PATH_DIRECTORY)?,
            ]
        );

        Ok(())
    }

    /// Test if a command starting with a tilde is highlighted correctly
    #[test]
    fn command_with_tilde() -> Result<()> {
        let cfg = test_cfg()?;

        let highlighted = cfg.highlight("~")?;
        assert_eq!(
            highlighted,
            vec![cfg.static_span(0, 1, DYNAMIC_CALLABLE_COMMAND)?]
        );

        let highlighted = cfg.highlight("~/")?;
        assert_eq!(
            highlighted,
            vec![cfg.static_span(0, 2, DYNAMIC_CALLABLE_COMMAND)?]
        );

        let highlighted = cfg.highlight("~ echo")?;
        assert_eq!(
            highlighted,
            vec![cfg.static_span(0, 1, DYNAMIC_CALLABLE_COMMAND)?]
        );

        let highlighted = cfg.highlight("~doesnotexist")?;
        assert_eq!(highlighted, vec![cfg.dynamic_span(0, 13, "~doesnotexist")]);

        let highlighted = cfg.highlight(r#""~""#)?;
        assert_eq!(highlighted, vec![cfg.dynamic_span(0, 3, "~")]);

        let highlighted = cfg.highlight(r#""~/""#)?;
        assert_eq!(highlighted, vec![cfg.dynamic_span(0, 4, "~/")]);

        Ok(())
    }

    /// Test if a path starting with a tilde is highlighted correctly
    #[test]
    fn path_with_tilde() -> Result<()> {
        let cfg = test_cfg()?;
        cfg.touch_file("test.txt")?;

        let highlighted = cfg.highlight("ls ~")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "ls"),
                cfg.mixed_span(3, 4, TILDE_VARIABLE, DYNAMIC_PATH_DIRECTORY)?,
            ]
        );

        let highlighted = cfg.highlight("ls ~/")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "ls"),
                cfg.mixed_span(3, 4, TILDE_VARIABLE, DYNAMIC_PATH_DIRECTORY)?,
                cfg.static_span(4, 5, DYNAMIC_PATH_DIRECTORY)?,
            ]
        );

        let highlighted = cfg.highlight("ls ~/ test.txt")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "ls"),
                cfg.mixed_span(3, 4, TILDE_VARIABLE, DYNAMIC_PATH_DIRECTORY)?,
                cfg.static_span(4, 5, DYNAMIC_PATH_DIRECTORY)?,
                cfg.static_span(6, 14, DYNAMIC_PATH_FILE)?,
            ]
        );

        let highlighted = cfg.highlight(r#"ls "~/""#)?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "ls"),
                cfg.static_span(3, 7, STRING_QUOTED_DOUBLE)?
            ]
        );

        let highlighted = cfg.highlight("ls '~/'")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "ls"),
                cfg.static_span(3, 7, STRING_QUOTED_DOUBLE)?
            ]
        );

        let highlighted = cfg.highlight("ls $'~/'")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "ls"),
                cfg.static_span(3, 8, STRING_QUOTED_DOUBLE)?
            ]
        );

        let highlighted = cfg.highlight("ls ~/this/path/does/not/exist")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "ls"),
                cfg.static_span(3, 4, TILDE_VARIABLE)?,
            ]
        );

        let highlighted = cfg.highlight("ls test/~/")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "ls"),
                cfg.static_span(8, 9, TILDE_VARIABLE)?
            ]
        );

        Ok(())
    }

    #[test]
    fn path_followed_by_parameter() -> Result<()> {
        let cfg = test_cfg()?;
        cfg.touch_file("test.txt")?;

        let highlighted = cfg.highlight("foo test.txt -C")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 3, "foo"),
                cfg.static_span(4, 12, DYNAMIC_PATH_FILE)?,
                cfg.static_span(12, 15, PARAMETER)?,
            ]
        );

        Ok(())
    }

    #[test]
    fn redirection() -> Result<()> {
        let cfg = test_cfg()?;
        cfg.touch_file("test.txt")?;

        let highlighted = cfg.highlight("echo hello > test.txt")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 4, "echo"),
                cfg.static_span(11, 12, REDIRECTION)?,
                cfg.static_span(13, 21, DYNAMIC_PATH_FILE)?,
            ]
        );

        let highlighted = cfg.highlight("echo hello>test.txt")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 4, "echo"),
                cfg.static_span(10, 11, REDIRECTION)?,
                cfg.static_span(11, 19, DYNAMIC_PATH_FILE)?,
            ]
        );

        let highlighted = cfg.highlight("echo ${FOO}hello>test.txt")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 4, "echo"),
                cfg.static_span(5, 11, ENVIRONMENT_VARIABLE)?,
                cfg.static_span(16, 17, REDIRECTION)?,
                cfg.static_span(17, 25, DYNAMIC_PATH_FILE)?,
            ]
        );

        let highlighted = cfg.highlight("echo hello$FOO>test.txt")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 4, "echo"),
                cfg.static_span(10, 14, ENVIRONMENT_VARIABLE)?,
                cfg.static_span(14, 15, REDIRECTION)?,
                cfg.static_span(15, 23, DYNAMIC_PATH_FILE)?,
            ]
        );

        Ok(())
    }

    #[test]
    fn double_quoted_callable() -> Result<()> {
        let cfg = test_cfg()?;

        let highlighted = cfg.highlight("\"ls\"")?;
        assert_eq!(highlighted, vec![cfg.dynamic_span(0, 4, "ls")]);

        let highlighted = cfg.highlight("l\"s\"")?;
        assert_eq!(highlighted, vec![cfg.dynamic_span(0, 4, "ls")]);

        cfg.touch_script("script.sh")?;

        let highlighted = cfg.highlight("\"./script.sh\"")?;
        assert_eq!(
            highlighted,
            vec![cfg.static_span(0, 13, DYNAMIC_CALLABLE_COMMAND)?]
        );

        cfg.create_dir("foo/bar")?;

        let highlighted = cfg.highlight("foo/\"bar\"/")?;
        assert_eq!(
            highlighted,
            vec![cfg.static_span(0, 10, DYNAMIC_CALLABLE_COMMAND)?]
        );

        Ok(())
    }

    #[test]
    fn escape_unquoted_at_beginning() -> Result<()> {
        let cfg = test_cfg()?;
        cfg.touch_file("test.txt")?;
        cfg.touch_script("script.sh")?;
        cfg.touch_script("s")?;

        let highlighted = cfg.highlight(r"\script.sh")?;
        assert_eq!(highlighted, vec![cfg.dynamic_span(0, 10, "script.sh")]);

        let highlighted = cfg.highlight(r"\./script.sh")?;
        assert_eq!(
            highlighted,
            vec![cfg.static_span(0, 12, DYNAMIC_CALLABLE_COMMAND)?]
        );

        // parser cannot differentiate between normal unquoted character escapes
        // and those that are at the beginning of a callable
        let highlighted = cfg.highlight(r"\s")?;
        assert_eq!(highlighted, vec![cfg.static_span(0, 2, CHARACTER_ESCAPE)?]);

        let highlighted = cfg.highlight(r"touch \test.txt")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 5, "touch"),
                cfg.mixed_span(6, 8, CHARACTER_ESCAPE, DYNAMIC_PATH_FILE)?,
                cfg.static_span(8, 15, DYNAMIC_PATH_FILE)?,
            ]
        );

        Ok(())
    }

    #[test]
    fn path_with_escape_unquoted() -> Result<()> {
        let cfg = test_cfg()?;

        let highlighted = cfg.highlight(r"cp test\u2580.txt dest.txt")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "cp"),
                cfg.static_span(7, 9, CHARACTER_ESCAPE)?
            ]
        );

        cfg.touch_file("testu2580.txt")?;

        let highlighted = cfg.highlight(r"cp test\u2580.txt dest.txt")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "cp"),
                cfg.static_span(3, 7, DYNAMIC_PATH_FILE)?,
                cfg.mixed_span(7, 9, CHARACTER_ESCAPE, DYNAMIC_PATH_FILE)?,
                cfg.static_span(9, 17, DYNAMIC_PATH_FILE)?,
            ]
        );

        Ok(())
    }

    #[test]
    fn path_with_escape_quoted() -> Result<()> {
        let cfg = test_cfg()?;
        cfg.touch_file("test▀.txt")?;
        cfg.touch_file("test  1.txt")?;

        let highlighted = cfg.highlight(r"cp test\u2580.txt dest.txt")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "cp"),
                cfg.static_span(7, 9, CHARACTER_ESCAPE)?
            ]
        );

        let highlighted = cfg.highlight(r#"cp "test\u2580.txt" dest.txt"#)?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "cp"),
                cfg.static_span(3, 19, STRING_QUOTED_DOUBLE)?
            ]
        );

        let highlighted = cfg.highlight(r#"cp 'test\u2580.txt' dest.txt"#)?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "cp"),
                cfg.static_span(3, 19, STRING_QUOTED_DOUBLE)?
            ]
        );

        let highlighted = cfg.highlight(r#"cp $'test\u2580.txt' dest.txt"#)?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "cp"),
                cfg.mixed_span(3, 9, STRING_QUOTED_DOUBLE, DYNAMIC_PATH_FILE)?,
                cfg.mixed_span(9, 15, CHARACTER_ESCAPE, DYNAMIC_PATH_FILE)?,
                cfg.mixed_span(15, 20, STRING_QUOTED_DOUBLE, DYNAMIC_PATH_FILE)?,
            ]
        );

        let highlighted = cfg.highlight(r#"cp test\ \ 1.txt dest.txt"#)?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "cp"),
                cfg.static_span(3, 7, DYNAMIC_PATH_FILE)?,
                cfg.mixed_span(7, 11, CHARACTER_ESCAPE, DYNAMIC_PATH_FILE)?,
                cfg.static_span(11, 16, DYNAMIC_PATH_FILE)?,
            ]
        );

        let highlighted = cfg.highlight(r#"cp "test\ \ 1.txt" dest.txt"#)?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "cp"),
                cfg.static_span(3, 18, STRING_QUOTED_DOUBLE)?,
            ]
        );

        let highlighted = cfg.highlight(r#"cp $'test\ \ 1.txt' dest.txt"#)?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "cp"),
                cfg.static_span(3, 19, STRING_QUOTED_DOUBLE)?,
            ]
        );

        let highlighted = cfg.highlight(r#"cp $'test\x20\x201.txt' dest.txt"#)?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "cp"),
                cfg.mixed_span(3, 9, STRING_QUOTED_DOUBLE, DYNAMIC_PATH_FILE)?,
                cfg.mixed_span(9, 17, CHARACTER_ESCAPE, DYNAMIC_PATH_FILE)?,
                cfg.mixed_span(17, 23, STRING_QUOTED_DOUBLE, DYNAMIC_PATH_FILE)?,
            ]
        );

        let highlighted = cfg.highlight(r#"cp test$'\x20\x20'1.txt dest.txt"#)?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "cp"),
                cfg.static_span(3, 7, DYNAMIC_PATH_FILE)?,
                cfg.mixed_span(7, 9, STRING_QUOTED_DOUBLE, DYNAMIC_PATH_FILE)?,
                cfg.mixed_span(9, 17, CHARACTER_ESCAPE, DYNAMIC_PATH_FILE)?,
                cfg.mixed_span(17, 18, STRING_QUOTED_DOUBLE, DYNAMIC_PATH_FILE)?,
                cfg.static_span(18, 23, DYNAMIC_PATH_FILE)?,
            ]
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

        let highlighted = cfg.highlight(r"$'sub/test\xF0\x9F\x98\x8E.sh'")?;
        assert_eq!(
            highlighted,
            vec![cfg.static_span(0, 30, DYNAMIC_CALLABLE_COMMAND)?]
        );

        let highlighted = cfg.highlight(r"$'sub/test\xF0\237\x98\x8E.sh'")?;
        assert_eq!(
            highlighted,
            vec![cfg.static_span(0, 30, DYNAMIC_CALLABLE_COMMAND)?]
        );

        Ok(())
    }

    #[test]
    fn path_with_multibyte_escape() -> Result<()> {
        let cfg = test_cfg()?;
        cfg.touch_file("test😎.txt")?;

        let highlighted = cfg.highlight(r"cp $'test\xF0\x9F\x98\x8E.txt' dest.txt")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "cp"),
                cfg.mixed_span(3, 9, STRING_QUOTED_DOUBLE, DYNAMIC_PATH_FILE)?,
                cfg.mixed_span(9, 25, CHARACTER_ESCAPE, DYNAMIC_PATH_FILE)?,
                cfg.mixed_span(25, 30, STRING_QUOTED_DOUBLE, DYNAMIC_PATH_FILE)?,
            ]
        );

        let highlighted = cfg.highlight(r"cp $'test\xF0\237\x98\x8E.txt' dest.txt")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "cp"),
                cfg.mixed_span(3, 9, STRING_QUOTED_DOUBLE, DYNAMIC_PATH_FILE)?,
                cfg.mixed_span(9, 25, CHARACTER_ESCAPE, DYNAMIC_PATH_FILE)?,
                cfg.mixed_span(25, 30, STRING_QUOTED_DOUBLE, DYNAMIC_PATH_FILE)?,
            ]
        );

        Ok(())
    }

    #[test]
    fn multiline() -> Result<()> {
        let cfg = test_cfg()?;
        cfg.touch_file("test.txt")?;

        let highlighted = cfg.highlight(
            "foo commit -m \"This is\na multi-line commit\nmessage\" && touch test.txt",
        )?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 3, "foo"),
                cfg.static_span(10, 13, PARAMETER)?,
                cfg.static_span(14, 51, STRING_QUOTED_DOUBLE)?,
                cfg.static_span(52, 54, OPERATOR_LOGICAL_AND)?,
                cfg.dynamic_span(55, 60, "touch"),
                cfg.static_span(61, 69, DYNAMIC_PATH_FILE)?,
            ]
        );

        Ok(())
    }

    #[test]
    fn path_with_env_variable() -> Result<()> {
        let cfg = test_cfg()?;
        cfg.touch_file("test.txt")?;
        cfg.touch_file("test.txt$FOOBAR")?;

        let highlighted = cfg.highlight(r"ls test.txt$FOOBAR")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "ls"),
                cfg.static_span(11, 18, ENVIRONMENT_VARIABLE)?,
            ]
        );

        let highlighted = cfg.highlight(r"ls ${FOOBAR}test.txt test.txt")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "ls"),
                cfg.static_span(3, 12, ENVIRONMENT_VARIABLE)?,
                cfg.static_span(21, 29, DYNAMIC_PATH_FILE)?,
            ]
        );

        let highlighted = cfg.highlight(r"ls test.txt${FOOBAR}test.txt test.txt")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "ls"),
                cfg.static_span(11, 20, ENVIRONMENT_VARIABLE)?,
                cfg.static_span(29, 37, DYNAMIC_PATH_FILE)?,
            ]
        );

        Ok(())
    }

    #[test]
    fn path_with_command_substitution() -> Result<()> {
        let cfg = test_cfg()?;
        cfg.touch_file("test.txt")?;
        cfg.touch_file("test.txtFOOBAR")?;

        let highlighted = cfg.highlight(r"ls test.txt$(echo FOOBAR)")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "ls"),
                cfg.static_span(11, 13, ENVIRONMENT_VARIABLE)?,
                cfg.dynamic_span(13, 17, "echo"),
                cfg.static_span(24, 25, ENVIRONMENT_VARIABLE)?,
            ]
        );

        cfg.touch_file("FOOBAR")?;

        let highlighted = cfg.highlight(r"ls test.txt$(echo FOOBAR)")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "ls"),
                cfg.static_span(11, 13, ENVIRONMENT_VARIABLE)?,
                cfg.dynamic_span(13, 17, "echo"),
                cfg.static_span(18, 24, DYNAMIC_PATH_FILE)?,
                cfg.static_span(24, 25, ENVIRONMENT_VARIABLE)?,
            ]
        );

        cfg.touch_file("test2.txt")?;

        let highlighted = cfg.highlight(r"ls test.txt test.txt$(echo FOOBAR) test2.txt")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.dynamic_span(0, 2, "ls"),
                cfg.static_span(3, 11, DYNAMIC_PATH_FILE)?,
                cfg.static_span(20, 22, ENVIRONMENT_VARIABLE)?,
                cfg.dynamic_span(22, 26, "echo"),
                cfg.static_span(27, 33, DYNAMIC_PATH_FILE)?,
                cfg.static_span(33, 34, ENVIRONMENT_VARIABLE)?,
                cfg.static_span(35, 44, DYNAMIC_PATH_FILE)?,
            ]
        );

        Ok(())
    }

    #[test]
    fn repeat() -> Result<()> {
        let cfg = test_cfg()?;

        let highlighted = cfg.highlight(r"repeat 5 echo Hello")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.static_span(0, 6, CONTROL_REPEAT)?,
                cfg.dynamic_span(9, 13, "echo"),
            ]
        );

        // arbitrary expressions
        let highlighted = cfg.highlight(r"repeat 1+9/2 echo Hello")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.static_span(0, 6, CONTROL_REPEAT)?,
                cfg.static_span(8, 9, OPERATOR_ARITHMETIC)?,
                cfg.static_span(10, 11, OPERATOR_ARITHMETIC)?,
                cfg.dynamic_span(13, 17, "echo"),
            ]
        );

        // arbitrary expressions
        let highlighted = cfg.highlight(r"repeat 1 do; echo Hello; done")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.static_span(0, 6, CONTROL_REPEAT)?,
                cfg.static_span(9, 12, CONTROL_DO)?,
                cfg.dynamic_span(13, 17, "echo"),
                cfg.static_span(23, 24, OPERATOR_LOGICAL_CONTINUE)?,
                cfg.static_span(25, 29, CONTROL_DONE)?,
            ]
        );

        // missing number should still work
        let highlighted = cfg.highlight(r"repeat echo Hello")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.static_span(0, 6, CONTROL_REPEAT)?,
                cfg.dynamic_span(7, 11, "echo"),
            ]
        );

        let highlighted = cfg.highlight(r"repeat do; echo Hello; done")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.static_span(0, 6, CONTROL_REPEAT)?,
                cfg.static_span(7, 10, CONTROL_DO)?,
                cfg.dynamic_span(11, 15, "echo"),
                cfg.static_span(21, 22, OPERATOR_LOGICAL_CONTINUE)?,
                cfg.static_span(23, 27, CONTROL_DONE)?,
            ]
        );

        Ok(())
    }

    #[test]
    fn time() -> Result<()> {
        let cfg = test_cfg()?;

        let highlighted = cfg.highlight(r"time sleep 2")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.static_span(0, 4, CONTROL_TIME)?,
                cfg.dynamic_span(5, 10, "sleep"),
            ]
        );

        Ok(())
    }

    #[test]
    fn nocorrect() -> Result<()> {
        let cfg = test_cfg()?;

        let highlighted = cfg.highlight(r"nocorrect slep 2")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.static_span(0, 9, CONTROL_NOCORRECT)?,
                cfg.dynamic_span(10, 14, "slep"),
            ]
        );

        Ok(())
    }

    #[test]
    fn select() -> Result<()> {
        let cfg = test_cfg()?;

        let highlighted = cfg.highlight(r"select x in a b c; do echo $x; done")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.static_span(0, 6, CONTROL_SELECT)?,
                cfg.static_span(9, 11, CONTROL_IN)?,
                cfg.static_span(17, 18, OPERATOR_LOGICAL_CONTINUE)?,
                cfg.static_span(19, 21, CONTROL_DO)?,
                cfg.dynamic_span(22, 26, "echo"),
                cfg.static_span(27, 29, ENVIRONMENT_VARIABLE)?,
                cfg.static_span(29, 30, OPERATOR_LOGICAL_CONTINUE)?,
                cfg.static_span(31, 35, CONTROL_DONE)?,
            ]
        );

        // without list
        let highlighted = cfg.highlight(r"select x; do echo $x; done")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.static_span(0, 6, CONTROL_SELECT)?,
                cfg.static_span(8, 9, OPERATOR_LOGICAL_CONTINUE)?,
                cfg.static_span(10, 12, CONTROL_DO)?,
                cfg.dynamic_span(13, 17, "echo"),
                cfg.static_span(18, 20, ENVIRONMENT_VARIABLE)?,
                cfg.static_span(20, 21, OPERATOR_LOGICAL_CONTINUE)?,
                cfg.static_span(22, 26, CONTROL_DONE)?,
            ]
        );

        // with break
        let highlighted = cfg.highlight(r"select x in a b; do break; done")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.static_span(0, 6, CONTROL_SELECT)?,
                cfg.static_span(9, 11, CONTROL_IN)?,
                cfg.static_span(15, 16, OPERATOR_LOGICAL_CONTINUE)?,
                cfg.static_span(17, 19, CONTROL_DO)?,
                cfg.static_span(20, 25, CONTROL_BREAK)?,
                cfg.static_span(25, 26, OPERATOR_LOGICAL_CONTINUE)?,
                cfg.static_span(27, 31, CONTROL_DONE)?,
            ]
        );

        // newlines instead of semicolons
        let highlighted = cfg.highlight("select x in a b\ndo\necho $x\ndone")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.static_span(0, 6, CONTROL_SELECT)?,
                cfg.static_span(9, 11, CONTROL_IN)?,
                cfg.static_span(16, 18, CONTROL_DO)?,
                cfg.dynamic_span(19, 23, "echo"),
                cfg.static_span(24, 26, ENVIRONMENT_VARIABLE)?,
                cfg.static_span(27, 31, CONTROL_DONE)?,
            ]
        );

        // select with case
        let highlighted =
            cfg.highlight(r"select x in a b; do case $x in a) echo yes;; esac; done")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.static_span(0, 6, CONTROL_SELECT)?,
                cfg.static_span(9, 11, CONTROL_IN)?,
                cfg.static_span(15, 16, OPERATOR_LOGICAL_CONTINUE)?,
                cfg.static_span(17, 19, CONTROL_DO)?,
                cfg.static_span(20, 24, CONTROL_CASE)?,
                cfg.static_span(25, 27, ENVIRONMENT_VARIABLE)?,
                cfg.static_span(28, 30, CONTROL_IN)?,
                cfg.static_span(32, 33, CONTROL_CASE_ITEM)?,
                cfg.dynamic_span(34, 38, "echo"),
                cfg.static_span(45, 50, CONTROL_ESAC)?,
                cfg.static_span(51, 55, CONTROL_DONE)?,
            ]
        );

        Ok(())
    }

    #[test]
    fn foreach() -> Result<()> {
        let cfg = test_cfg()?;

        let highlighted = cfg.highlight(r"foreach x (a b c); echo $x; end")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.static_span(0, 7, CONTROL_FOREACH)?,
                cfg.static_span(17, 18, OPERATOR_LOGICAL_CONTINUE)?,
                cfg.dynamic_span(19, 23, "echo"),
                cfg.static_span(24, 26, ENVIRONMENT_VARIABLE)?,
                cfg.static_span(26, 27, OPERATOR_LOGICAL_CONTINUE)?,
                cfg.static_span(28, 31, CONTROL_END)?,
            ]
        );

        // newlines instead of semicolons
        let highlighted = cfg.highlight("foreach x (a b c)\necho $x\nend")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.static_span(0, 7, CONTROL_FOREACH)?,
                cfg.dynamic_span(18, 22, "echo"),
                cfg.static_span(23, 25, ENVIRONMENT_VARIABLE)?,
                cfg.static_span(26, 29, CONTROL_END)?,
            ]
        );

        // foreach with break
        let highlighted = cfg.highlight("foreach x (a b c);echo $x; break ; end")?;
        assert_eq!(
            highlighted,
            vec![
                cfg.static_span(0, 7, CONTROL_FOREACH)?,
                cfg.static_span(17, 18, OPERATOR_LOGICAL_CONTINUE)?,
                cfg.dynamic_span(18, 22, "echo"),
                cfg.static_span(23, 25, ENVIRONMENT_VARIABLE)?,
                cfg.static_span(25, 26, OPERATOR_LOGICAL_CONTINUE)?,
                cfg.static_span(27, 32, CONTROL_BREAK)?,
                cfg.static_span(33, 34, OPERATOR_LOGICAL_CONTINUE)?,
                cfg.static_span(35, 38, CONTROL_END)?,
            ]
        );

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
