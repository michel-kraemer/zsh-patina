use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

use anyhow::Result;

use crate::{config::Config, daemon::is_daemon_running, theme::Theme};

/// This function can be called at the start of the program to quickly check the
/// configuration (e.g. when the daemon is activated). Unlike [`check`], it does
/// not print anything. Also, it only checks for configuration-related issues
/// that prevent the program from starting.
pub fn check_config(config: &Config) -> Result<()> {
    // At this point, it has already been checked if the program's main
    // configuration is parseable. Otherwise, we would not have a `config`
    // object.

    // check if we can load the custom theme and if it's syntax is OK
    Theme::load(&config.highlighting.theme)?;

    Ok(())
}

enum MessageType {
    Success,
    Info,
    Warning,
    Error,
}

fn print_bullet(message: &str, t: MessageType) {
    let wrap_options = textwrap::Options::with_termwidth().subsequent_indent("   ");

    let message = match t {
        MessageType::Success => format!("✅ {message}"),
        MessageType::Info => format!("ℹ️ {message}"),
        MessageType::Warning => format!("⚠️ {message}"),
        MessageType::Error => format!("❌ {message}"),
    };

    for l in textwrap::wrap(&message, wrap_options) {
        println!("{l}");
    }
}

pub fn check(config: &Config, config_file_path: &Option<PathBuf>, data_dir: &Path) -> Result<()> {
    let mut has_errors = false;
    let mut has_warnings = false;

    if let Some(config_file_path) = config_file_path {
        print_bullet(
            &format!(
                "Using configuration file at `{}'.",
                config_file_path.to_string_lossy()
            ),
            MessageType::Info,
        );
    } else {
        print_bullet(
            &format!(
                "No configuration file found at `$XDG_CONFIG_HOME/zsh-patina/config.toml' \
                or `{}/.config/zsh-patina/config.toml'. Using default settings.",
                dirs::home_dir()
                    .and_then(|p| p.to_str().map(|s| s.to_string()))
                    .unwrap_or_else(|| "~".to_string())
            ),
            MessageType::Info,
        );
    }

    // check if we can load the custom theme and if it's syntax is OK
    if let Err(e) = Theme::load(&config.highlighting.theme) {
        print_bullet(&format!("{e:?}"), MessageType::Error);
        has_errors = true;
    } else {
        print_bullet(
            &format!("Theme `{}' loaded successfully.", config.highlighting.theme),
            MessageType::Success,
        );
    }

    // check if the daemon is running
    if let Some(pid) = is_daemon_running(data_dir) {
        print_bullet(
            &format!("Daemon is running. PID {pid}."),
            MessageType::Success,
        );
    } else {
        print_bullet(
            "Daemon is stopped or PID file could not be accessed.",
            MessageType::Warning,
        );
        has_warnings = true;
    }

    // check if `zsh-patina activate` is called in the zshrc file
    let zshrc_path = zshrc_path();
    match File::open(&zshrc_path) {
        Ok(f) => {
            let reader = BufReader::new(f);
            let mut activate_found = false;
            for l in reader.lines() {
                let l = l?;
                if l.is_empty() || l.trim().starts_with('#') {
                    continue;
                }
                if l.contains("zsh-patina activate") {
                    activate_found = true;
                    break;
                }
            }
            if !activate_found {
                print_bullet(
                    &format!(
                        "The string `zsh-patina activate' was not found \
                        in your .zshrc file at `{}'. Please make \
                        sure zsh-patina is activated when your shell is \
                        started.",
                        zshrc_path.display()
                    ),
                    MessageType::Warning,
                );
                has_warnings = true;
            } else {
                print_bullet(
                    &format!(
                        "zsh-patina is activated correctly in your \
                        .zshrc file at `{}'.",
                        zshrc_path.display()
                    ),
                    MessageType::Success,
                );
            }
        }
        Err(e) => {
            print_bullet(
                &format!(
                    "Failed to read `{}'. Unable to check if \
                    zsh-patina is activated when the shell is started.\n\n{e}",
                    zshrc_path.display()
                ),
                MessageType::Warning,
            );
            has_warnings = true;
        }
    }

    println!();
    if has_errors {
        println!("There were errors! zsh-patina will not work.");
    } else if has_warnings {
        println!("There were warnings. zsh-patina might not work as expected.");
    } else {
        println!("Everything is OK.");
    }

    Ok(())
}

fn zshrc_path() -> PathBuf {
    if let Some(zdotdir) = std::env::var_os("ZDOTDIR") {
        PathBuf::from(zdotdir).join(".zshrc")
    } else {
        dirs::home_dir()
            .map(|h| h.join(".zshrc"))
            .unwrap_or_else(|| PathBuf::from("~/.zshrc"))
    }
}
