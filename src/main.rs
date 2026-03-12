use std::{
    env, fs,
    io::{self, Read, Write},
    path::PathBuf,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use figment::{
    Figment,
    providers::{Format, Serialized, Toml},
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};

use crate::{
    daemon::{start_daemon, status_daemon, stop_daemon},
    highlighter::{Highlighter, Token},
    theme::{Theme, ThemeSource},
};

mod daemon;
mod highlighter;
mod theme;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Start the highlighter daemon if it's not already running
    Start,

    /// Stop the highlighter daemon if it's not already stopped
    Stop,

    /// Restart the highlighter daemon or make sure it is started if it's not
    /// running
    Restart,

    /// Check whether the highlighter daemon is running
    Status,

    /// Tokenize a command (from a file or from stdin) and print the identified
    /// tokens
    Tokenize {
        /// The input file to tokenize. If this parameter is not provided, the
        /// command will be read from stdin.
        input_file: Option<String>,
    },

    /// List all scopes that can be used in a theme for highlighting (sorted
    /// alphabetically)
    ListScopes,
}

#[derive(Serialize, Deserialize, Default)]
pub struct Config {
    pub highlighting: HighlightingConfig,
}

#[derive(Serialize, Deserialize)]
pub struct HighlightingConfig {
    /// For performance reasons, highlighting is disabled for very long lines.
    /// This option specifies the maximum length of a line (in bytes) up to
    /// which highlighting is applied.
    pub max_line_length: usize,

    /// The maximum time (in milliseconds) to spend on highlighting a command.
    /// If highlighting takes longer, it will be aborted and the command will be
    /// partially highlighted.
    ///
    /// Note that the timeout only applies to multi-line commands. Highlighting
    /// cannot be aborted in the middle of a line. If you often deal with long
    /// lines that take longer to highlight than the timeout, consider reducing
    /// [max_line_length](Self::max_line_length).
    #[serde(
        rename = "timeout_ms",
        serialize_with = "serialize_duration_ms",
        deserialize_with = "deserialize_duration_ms"
    )]
    pub timeout: Duration,

    /// Either the name of a built-in theme (`"simple"`, `"patina"`,
    /// `"lavender"`) or a string in the form `"file:mytheme.toml"` pointing to
    /// a custom theme toml file.
    pub theme: ThemeSource,
}

fn serialize_duration_ms<S: Serializer>(duration: &Duration, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_u64(duration.as_millis() as u64)
}

fn deserialize_duration_ms<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
    let ms = u64::deserialize(d)?;
    Ok(Duration::from_millis(ms))
}

impl Default for HighlightingConfig {
    fn default() -> Self {
        Self {
            max_line_length: 20000,
            timeout: Duration::from_millis(500),
            theme: ThemeSource::Patina,
        }
    }
}

/// Parse a color in the format #RRGGBB, #RGB, or an ANSI name to a terminal
/// color
pub fn parse_term_color(s: &str) -> Result<Color> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "white" => Color::White,
        _ => {
            let s = s.strip_prefix('#').context("Color must start with '#'")?;
            if s.len() == 6 {
                let r = u8::from_str_radix(&s[0..2], 16)?;
                let g = u8::from_str_radix(&s[2..4], 16)?;
                let b = u8::from_str_radix(&s[4..6], 16)?;
                Color::Rgb(r, g, b)
            } else if s.len() == 3 {
                let mut r = u8::from_str_radix(&s[0..1], 16)?;
                let mut g = u8::from_str_radix(&s[1..2], 16)?;
                let mut b = u8::from_str_radix(&s[2..3], 16)?;
                r |= r << 4;
                g |= g << 4;
                b |= b << 4;
                Color::Rgb(r, g, b)
            } else {
                bail!("Color must be in the format #RRGGBB or #RGB");
            }
        }
    })
}

/// Tokenize an input file and print the identified tokens to stdout. If the
/// input file is `None`, read from stdin.
fn tokenize(config: &Config, input_file: &Option<String>) -> Result<()> {
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
    let highlighter = Highlighter::new(&config.highlighting)?;
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

            let (fg, bg) = if let Some(style) = theme.resolve(&t.scope) {
                (
                    parse_term_color(&style.foreground)?,
                    style
                        .background
                        .as_ref()
                        .map(|c| parse_term_color(c))
                        .transpose()?,
                )
            } else {
                (Color::White, None)
            };

            let mut color_spec = ColorSpec::new();
            if let Some(bg) = bg {
                color_spec.set_bg(Some(bg));
            }

            if leading_spaces > 0 {
                color_spec.set_fg(Some(Color::Rgb(96, 96, 96)));
                stdout.set_color(&color_spec)?;
                write!(stdout, "{}", "·".repeat(leading_spaces))?;
            }

            color_spec.set_fg(Some(fg));
            stdout.set_color(&color_spec)?;
            write!(stdout, "{}", l.trim())?;

            if trailing_spaces > 0 {
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

/// Print all scopes that can be used in a theme for highlighting (sorted
/// alphabetically)
fn list_scopes() -> Result<()> {
    let scopes = include!(concat!(env!("OUT_DIR"), "/scopes.rs"));
    for t in scopes {
        println!("{t}");
    }
    Ok(())
}

fn main() -> Result<()> {
    let home = PathBuf::from(env::var("HOME").context("$HOME not set")?);
    let config_dir = home.join(".config/zsh-patina");
    let data_dir = home.join(".local/share/zsh-patina");

    // parse arguments
    let args = Args::parse();

    // load config file
    let config_file_path = config_dir.join("config.toml");
    let config = if fs::exists(&config_file_path)? {
        Figment::new()
            .merge(Serialized::defaults(Config::default()))
            .merge(Toml::file(&config_file_path))
            .extract()
            .with_context(|| format!("Unable to read config file {config_file_path:?}"))?
    } else {
        Config::default()
    };

    match args.command {
        Command::Start => start_daemon(&data_dir, &config),
        Command::Stop => stop_daemon(&data_dir),
        Command::Restart => {
            stop_daemon(&data_dir)?;
            start_daemon(&data_dir, &config)
        }
        Command::Status => status_daemon(&data_dir),
        Command::Tokenize { input_file } => tokenize(&config, &input_file),
        Command::ListScopes => list_scopes(),
    }
}
