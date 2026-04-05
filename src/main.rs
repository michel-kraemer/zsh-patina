use std::{io::Write, process};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use figment::{
    Figment,
    providers::{Format, Serialized, Toml},
};
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};

use crate::{
    commands::{check, list_scopes, list_themes, tokenize},
    config::{Config, config_file_path, runtime_dir},
    daemon::{activate, start_daemon, status_daemon, stop_daemon},
};

mod color;
mod commands;
pub mod config;
mod daemon;
mod highlighting;
mod path;
mod theme;
mod unescape;

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
    ///     eval "$(/path/to/zsh-patina activate)"
    ///
    /// This initializes zsh-patina in the current shell session and starts the background daemon (if it is not already running).
    ///
    /// If you want to initialize it for all future Zsh sessions, run the following command:
    ///
    ///     echo 'eval "$(/path/to/zsh-patina activate)"' >> $HOME/.zshrc
    #[command(verbatim_doc_comment)]
    Activate,

    /// Start the highlighter daemon if it's not already running
    Start {
        /// Start the highlighter in foreground mode
        #[arg(long, default_value = "false")]
        no_daemon: bool,
    },

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

    /// List available themes with small examples for preview
    ListThemes,
}

fn run() -> Result<()> {
    // initialize logger and configure custom format
    env_logger::builder()
        .format(|buf, record| {
            let timestamp = buf.timestamp_micros();
            let level = record.level();
            let file = record.file();
            let line = record.line();
            let thread_id = std::thread::current().id();
            if let Some(file) = file
                && let Some(line) = line
            {
                writeln!(
                    buf,
                    "[{} {} {}:{} {:?}] {}",
                    timestamp,
                    level,
                    file,
                    line,
                    thread_id,
                    record.args()
                )
            } else {
                writeln!(
                    buf,
                    "[{} {} {:?}] {}",
                    timestamp,
                    level,
                    thread_id,
                    record.args()
                )
            }
        })
        .init();

    let config_file_path = config_file_path()?;
    let runtime_dir = runtime_dir()?;

    // parse arguments
    let args = Args::parse();

    // load config file
    let config = if let Some(config_file_path) = &config_file_path {
        Figment::new()
            .merge(Serialized::defaults(Config::default()))
            .merge(Toml::file(config_file_path))
            .extract()
            .with_context(|| format!("Unable to read config file {config_file_path:?}"))?
    } else {
        Config::default()
    };

    match args.command {
        Command::Activate => activate(&runtime_dir, &config),
        Command::Start { no_daemon } => start_daemon(&runtime_dir, &config, no_daemon),
        Command::Stop => {
            stop_daemon(&runtime_dir);
            Ok(())
        }
        Command::Restart => {
            stop_daemon(&runtime_dir);
            start_daemon(&runtime_dir, &config, false)
        }
        Command::Status => status_daemon(&runtime_dir),
        Command::Check => check(&config, &config_file_path, &runtime_dir),
        Command::Tokenize { input_file } => tokenize(&config, &input_file),
        Command::ListScopes => list_scopes(),
        Command::ListThemes => list_themes(&config),
    }
}

fn main() -> Result<()> {
    if let Err(e) = run() {
        let mut stderr = StandardStream::stderr(ColorChoice::Auto);
        stderr.set_color(ColorSpec::new().set_fg(Some(Color::Red)).set_bold(true))?;
        write!(stderr, "zsh-patina: ")?;
        stderr.reset()?;
        writeln!(stderr, "{e:?}")?;
        process::exit(1);
    }
    Ok(())
}
