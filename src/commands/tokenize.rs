use std::{
    fs,
    io::{self, Read, Write},
};

use anyhow::{Context, Result};
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};

use crate::{
    config::Config,
    highlighting::{HighlighterBuilder, Token},
    theme::Theme,
};

/// Tokenize an input file and print the identified tokens to stdout. If the
/// input file is `None`, read from stdin.
pub fn tokenize(config: &Config, input_file: &Option<String>) -> Result<()> {
    let theme = Theme::load(&config.highlighting.theme)?;

    // read input
    let input = if let Some(input_file) = input_file {
        fs::read_to_string(input_file)
            .with_context(|| format!("Failed to read file '{input_file}'"))?
    } else {
        let mut buf = String::new();
        io::stdin()
            .read_to_string(&mut buf)
            .context("Failed to read from stdin")?;
        buf
    };

    // tokenize
    let highlighter = HighlighterBuilder::new(&config.highlighting).build()?;
    let tokens = highlighter.tokenize(&input)?;

    // join consecutive tokens
    let tokens = tokens.into_iter().fold(Vec::<Token>::new(), |mut acc, t| {
        if let Some(last) = acc.last_mut()
            && last.scope == t.scope
            && last.range.end == t.range.start
        {
            last.range.end = t.range.end;
            acc
        } else {
            acc.push(t);
            acc
        }
    });

    // print tokens
    let mut stdout = StandardStream::stdout(ColorChoice::Auto);
    for t in tokens {
        if t.scope == "source.shell.bash" {
            // don't print the whole command
            continue;
        }
        if t.range.is_empty() {
            // don't print empty tokens
            continue;
        }

        stdout.set_color(ColorSpec::new().set_fg(Some(Color::Yellow)))?;
        write!(stdout, "╭─")?;
        stdout.set_color(ColorSpec::new().set_fg(Some(Color::Blue)))?;
        write!(stdout, "[{}:{}] ", t.line, t.column)?;
        stdout.set_color(ColorSpec::new().set_fg(Some(Color::Rgb(160, 160, 160))))?;
        writeln!(stdout, "{}", t.scope)?;

        let mut contents = input[t.range].to_string();
        contents.push('\n');
        for l in contents.lines() {
            stdout.set_color(ColorSpec::new().set_fg(Some(Color::Yellow)))?;
            write!(stdout, "│ ")?;

            let leading_spaces = l.chars().take_while(|c| c.is_whitespace()).count();
            let trailing_spaces = l.chars().rev().take_while(|c| c.is_whitespace()).count();

            let color_spec = if let Some(style) = theme.resolve(&t.scope) {
                let mut color_spec = ColorSpec::new();
                if let Some(fg) = &style.foreground {
                    color_spec.set_fg(Some(fg.into()));
                }
                if let Some(bg) = &style.background {
                    color_spec.set_bg(Some(bg.into()));
                }
                if style.bold {
                    color_spec.set_bold(true);
                }
                if style.underline {
                    color_spec.set_underline(true);
                }
                color_spec
            } else {
                let mut color_spec = ColorSpec::new();
                color_spec.set_fg(Some(Color::White));
                color_spec
            };

            if leading_spaces > 0 {
                let mut color_spec = color_spec.clone();
                color_spec.set_fg(Some(Color::Rgb(96, 96, 96)));
                stdout.set_color(&color_spec)?;
                write!(stdout, "{}", "·".repeat(leading_spaces))?;
            }

            stdout.set_color(&color_spec)?;
            write!(stdout, "{}", l.trim())?;

            if trailing_spaces > 0 {
                let mut color_spec = color_spec.clone();
                color_spec.set_fg(Some(Color::Rgb(96, 96, 96)));
                stdout.set_color(&color_spec)?;
                write!(stdout, "{}", "·".repeat(trailing_spaces))?;
            }

            stdout.reset()?;
            writeln!(stdout)?;
        }

        writeln!(stdout)?;
    }
    stdout.reset()?;

    Ok(())
}
