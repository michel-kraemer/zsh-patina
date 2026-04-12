use std::{fs, io::Write, time::Duration};

use anyhow::Result;
use strum::IntoEnumIterator;
use tempfile::tempdir;
use termcolor::{BufferWriter, Color as TermColor, ColorChoice, ColorSpec, WriteColor};

use crate::{
    color::Color,
    config::{Config, HighlightingConfig},
    highlighting::{Highlighter, Span, SpanStyle},
    theme::{Style, Theme, ThemeSource},
};

/// Convert a style to a termcolor ColorSpec
fn style_to_color_spec(style: Style) -> ColorSpec {
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
}

/// Print a command with syntax highlighting to `stdout`
fn print_command<W>(command: &str, spans: &[Span], theme: &Theme, stdout: &mut W) -> Result<()>
where
    W: WriteColor,
{
    stdout.set_color(ColorSpec::new().set_fg(Some(TermColor::White)))?;
    write!(stdout, "$ ")?;

    let chars = command.chars().collect::<Vec<_>>();

    let mut last_pos = 0;
    for s in spans {
        if s.start > last_pos {
            stdout.reset()?;
            write!(
                stdout,
                "{}",
                chars[last_pos..s.start].iter().collect::<String>()
            )?;
        }
        last_pos = s.end;

        let substring = chars[s.start..s.end].iter().collect::<String>();
        let color_spec = match s.style {
            SpanStyle::Static(ref static_style) => {
                let mut color_spec = ColorSpec::new();
                if let Some(fg) = &static_style.foreground_color {
                    color_spec.set_fg(Some(TermColor::from(Color::try_from(fg.as_str())?)));
                }
                if let Some(bg) = &static_style.background_color {
                    color_spec.set_bg(Some(TermColor::from(Color::try_from(bg.as_str())?)));
                }
                if static_style.bold {
                    color_spec.set_bold(true);
                }
                if static_style.underline {
                    color_spec.set_underline(true);
                }
                color_spec
            }

            SpanStyle::Dynamic(_) => match substring.as_str() {
                "gh" | "git" => {
                    let style = theme
                        .resolve("dynamic.callable.command.shell")
                        .or_else(|| theme.resolve("variable.function.shell"));
                    style.map(style_to_color_spec).unwrap_or_default()
                }

                "cd" | "print" | "printf" | "read" => {
                    let style = theme
                        .resolve("dynamic.callable.builtin.shell")
                        .or_else(|| theme.resolve("variable.function.shell"));
                    style.map(style_to_color_spec).unwrap_or_default()
                }

                _ => unreachable!("Examples are self-contained"),
            },
        };

        stdout.set_color(&color_spec)?;
        write!(stdout, "{substring}")?;
    }

    if last_pos < chars.len() {
        stdout.reset()?;
        write!(stdout, "{}", chars[last_pos..].iter().collect::<String>())?;
    }

    stdout.reset()?;
    writeln!(stdout)?;

    Ok(())
}

fn list_theme<W>(theme_source: ThemeSource, stdout: &mut W) -> Result<()>
where
    W: WriteColor,
{
    let config = HighlightingConfig {
        theme: theme_source,
        timeout: Duration::from_secs(3600),
        ..Default::default()
    };
    let temp_dir = tempdir()?;
    let home_dir = temp_dir.path().to_string_lossy().to_string();

    let highlighter = Highlighter::new(&config, home_dir.clone())?;

    let cmd = "gh repo fork michel-kraemer/zsh-patina --clone --remote";
    let spans = highlighter.highlight(cmd, None, Some(&home_dir), |_| true)?;
    print_command(cmd, &spans, highlighter.theme(), stdout)?;

    let zsh_patina_dir = temp_dir.path().join("zsh-patina");
    let themes_dir = zsh_patina_dir.join("themes");
    fs::create_dir_all(&themes_dir)?;
    let patina_toml = themes_dir.join("patina.toml");
    fs::write(patina_toml, "")?;

    let cmd = "cd zsh-patina";
    let spans = highlighter.highlight(cmd, None, Some(&home_dir), |_| true)?;
    print_command(cmd, &spans, highlighter.theme(), stdout)?;

    let project_dir = zsh_patina_dir.to_string_lossy().to_string();

    let cmd = r##"while read -r line; do [[ $line =~ ^(.*=).*\"[^\"]+\"$ ]] && print "${match[1]} $(($RANDOM%255))" || print "$line"; done < themes/patina.toml > themes/random.toml"##;
    let spans = highlighter.highlight(cmd, None, Some(&project_dir), |_| true)?;
    print_command(cmd, &spans, highlighter.theme(), stdout)?;

    let random_toml = themes_dir.join("random.toml");
    fs::write(random_toml, "")?;

    let cmd = "git add themes/random.toml";
    let spans = highlighter.highlight(cmd, None, Some(&project_dir), |_| true)?;
    print_command(cmd, &spans, highlighter.theme(), stdout)?;

    let cmd = r#"git commit -m "🌈 Add my super duper random color theme""#;
    let spans = highlighter.highlight(cmd, None, Some(&project_dir), |_| true)?;
    print_command(cmd, &spans, highlighter.theme(), stdout)?;

    let cmd = "git push -u origin main";
    let spans = highlighter.highlight(cmd, None, Some(&project_dir), |_| true)?;
    print_command(cmd, &spans, highlighter.theme(), stdout)?;

    Ok(())
}

pub fn list_themes(config: &Config) -> Result<()> {
    let bufwtr = BufferWriter::stdout(ColorChoice::Auto);
    let mut buffer = bufwtr.buffer();

    for mut theme in ThemeSource::iter() {
        if matches!(theme, ThemeSource::File(_)) {
            if matches!(config.highlighting.theme, ThemeSource::File(_)) {
                theme = config.highlighting.theme.clone();
            } else {
                continue;
            }
        }

        buffer.set_color(
            ColorSpec::new()
                .set_fg(Some(TermColor::White))
                .set_bold(true),
        )?;
        write!(buffer, "{theme}")?;
        buffer.set_color(ColorSpec::new().set_fg(Some(TermColor::Rgb(160, 160, 160))))?;

        let active = theme == config.highlighting.theme;
        let default = theme == HighlightingConfig::default().theme;

        if active || default {
            write!(buffer, " (")?;
        }
        if default {
            write!(buffer, "default")?;
        }
        if active && default {
            write!(buffer, ", ")?;
        }
        if active {
            write!(buffer, "active")?;
        }
        if active || default {
            write!(buffer, ")")?;
        }
        writeln!(buffer, "\n")?;

        list_theme(theme, &mut buffer)?;
        writeln!(buffer, "\n")?;
    }

    buffer.reset()?;

    writeln!(
        buffer,
        "Create your own custom theme! For more information, see:\nhttps://github.com/michel-kraemer/zsh-patina#creating-a-custom-theme"
    )?;

    bufwtr.print(&buffer)?;

    Ok(())
}
