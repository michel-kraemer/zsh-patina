use std::{
    env, fs,
    io::{self, Read, Write},
    path::PathBuf,
    time::Duration,
};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use figment::{
    Figment,
    providers::{Format, Serialized, Toml},
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};

use crate::{
    check::check_config,
    daemon::{activate, start_daemon, status_daemon, stop_daemon},
    highlighter::{Highlighter, Token},
    theme::{Theme, ThemeSource},
};

mod check;
mod color;
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
    /// Initialize zsh-patina in the current shell session.
    ///
    /// The command prints out a Zsh script that should be eval'd as follows:
    ///
    ///     eval "$(~/.zsh-patina/target/release/zsh-patina activate)"
    ///
    /// This initializes zsh-patina in the current shell session and starts the background daemon (if it is not already running).
    ///
    /// If you want to initialize it for all future Zsh sessions, run the following command:
    ///
    ///     echo 'eval "$(~/.zsh-patina/target/release/zsh-patina activate)"' >> $HOME/.zshrc
    #[command(verbatim_doc_comment)]
    Activate,

    /// Start the highlighter daemon if it's not already running
    Start,

    /// Stop the highlighter daemon if it's not already stopped
    Stop,

    /// Restart the highlighter daemon or make sure it is started if it's not
    /// running
    Restart,

    /// Check whether the highlighter daemon is running
    Status,

    /// Check user configuration and custom theme (if applicable) for errors
    Check,

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

            let color_spec = if let Some(style) = theme.resolve(&t.scope) {
                let mut color_spec = ColorSpec::new();
                color_spec.set_fg(Some(style.foreground.into()));
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
        Command::Activate => activate(&data_dir, &config),
        Command::Start => start_daemon(&data_dir, &config),
        Command::Stop => stop_daemon(&data_dir),
        Command::Restart => {
            stop_daemon(&data_dir)?;
            start_daemon(&data_dir, &config)
        }
        Command::Status => status_daemon(&data_dir),
        Command::Check => {
            check_config(&config)?;
            println!("Everything is OK.");
            Ok(())
        }
        Command::Tokenize { input_file } => tokenize(&config, &input_file),
        Command::ListScopes => list_scopes(),
    }
}
