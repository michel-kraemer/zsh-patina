use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
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

#[derive(Clone, Copy, PartialEq, Eq)]
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

/// Check if the configuration file can be loaded
fn check_config_file(config_file_path: &Option<PathBuf>) -> (String, MessageType) {
    if let Some(path) = config_file_path {
        (
            format!("Using configuration file at `{}'.", path.to_string_lossy()),
            MessageType::Info,
        )
    } else {
        (
            format!(
                "No configuration file found at `$ZSH_PATINA_CONFIG_PATH`, \
                `$XDG_CONFIG_HOME/zsh-patina/config.toml', or \
                `{}/.config/zsh-patina/config.toml'. Using default settings.",
                dirs::home_dir()
                    .and_then(|p| p.to_str().map(|s| s.to_string()))
                    .unwrap_or_else(|| "~".to_string())
            ),
            MessageType::Info,
        )
    }
}

/// Check if the configured theme can be loaded
fn check_theme(config: &Config) -> (String, MessageType) {
    match Theme::load(&config.highlighting.theme) {
        Err(e) => (format!("{e:?}"), MessageType::Error),
        Ok(_) => (
            format!("Theme `{}' loaded successfully.", config.highlighting.theme),
            MessageType::Success,
        ),
    }
}

/// Check if the daemon is running
fn check_daemon(runtime_dir: &Path) -> (String, MessageType) {
    match is_daemon_running(runtime_dir) {
        Some(pid) => (
            format!("Daemon is running. PID {pid}."),
            MessageType::Success,
        ),
        None => (
            "Daemon is stopped or PID file could not be accessed.".to_string(),
            MessageType::Warning,
        ),
    }
}

/// Check if zsh-patina is activated in an interactive shell session
fn check_activation() -> (String, MessageType) {
    let output = match Command::new("zsh")
        .args(["-i", "-c", "typeset -f zsh-patina"])
        .stderr(Stdio::null()) // suppress interactive shell noise
        .output()
    {
        Err(e) => {
            return (
                format!(
                    "Failed to spawn a Zsh process to check if zsh-patina is \
                    `activated.\n\n{e}"
                ),
                MessageType::Warning,
            );
        }
        Ok(output) => output,
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let function_body = stdout.trim();

    if function_body.is_empty() {
        return (
            "The `zsh-patina' shell function was not found in your interactive \
            shell session. Please make sure zsh-patina is activated in your \
            shell configuration."
                .to_string(),
            MessageType::Error,
        );
    }

    // extract the binary path from a line of the form:
    // "/path/to/zsh-patina" "$@"
    let Some(function_path) = function_body.lines().find_map(|line| {
        let trimmed = line.trim();
        let without_args = trimmed.strip_suffix("\"$@\"")?.trim_end();
        let inner = without_args.strip_prefix('"')?.strip_suffix('"')?;
        Some(PathBuf::from(inner))
    }) else {
        return (
            "The `zsh-patina' shell function was found but its body could not \
            be parsed to verify the binary path."
                .to_string(),
            MessageType::Warning,
        );
    };

    let current_exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            return (
                format!("Failed to determine the current executable path.\n\n{e}"),
                MessageType::Warning,
            );
        }
    };

    // Canonicalize both paths to resolve symlinks and normalize them before
    // comparing. If the path stored in the function is stale, canonicalize
    // will fail for it while succeeding for current_exe, which we treat as a
    // mismatch.
    let fp = function_path.canonicalize().ok();
    let ce = current_exe.canonicalize().ok();

    if fp.is_some() && fp == ce {
        ("zsh-patina is activated.".to_string(), MessageType::Success)
    } else {
        (
            format!(
                "The `zsh-patina' shell function points to `{}', but the \
                current binary is at `{}'. Please re-run the activation \
                command to update your shell configuration.",
                function_path.to_string_lossy(),
                current_exe.to_string_lossy()
            ),
            MessageType::Warning,
        )
    }
}

/// Check if the ZLE widget is registered
fn check_zle_widget() -> (String, MessageType) {
    let output = match Command::new("zsh")
        .args(["-i", "-c", "zle -l"])
        .stderr(Stdio::null())
        .output()
    {
        Err(e) => {
            return (
                format!(
                    "Failed to spawn a Zsh process to check for the ZLE \
                    widget.\n\n{e}"
                ),
                MessageType::Warning,
            );
        }
        Ok(output) => output,
    };

    let stdout = String::from_utf8_lossy(&output.stdout);

    if stdout.lines().any(|l| l.trim() == "_zsh_patina") {
        (
            "The ZLE widget is registered.".to_string(),
            MessageType::Success,
        )
    } else {
        (
            "The ZLE widget was not found in your interactive shell session. \
            It might have been removed by another Zsh plugin. Please activate \
            zsh-patina at the end of your .zshrc file."
                .to_string(),
            MessageType::Error,
        )
    }
}

pub fn check(
    config: &Config,
    config_file_path: &Option<PathBuf>,
    runtime_dir: &Path,
) -> Result<()> {
    let mut has_errors = false;
    let mut has_warnings = false;

    let (msg, t) = check_config_file(config_file_path);
    match t {
        MessageType::Error => has_errors = true,
        MessageType::Warning => has_warnings = true,
        _ => {}
    }
    print_bullet(&msg, t);

    let (msg, t) = check_theme(config);
    match t {
        MessageType::Error => has_errors = true,
        MessageType::Warning => has_warnings = true,
        _ => {}
    }
    print_bullet(&msg, t);

    let (msg, t) = check_activation();
    match t {
        MessageType::Error => has_errors = true,
        MessageType::Warning => has_warnings = true,
        _ => {}
    }
    print_bullet(&msg, t);

    if t == MessageType::Success {
        let (msg, t) = check_zle_widget();
        match t {
            MessageType::Error => has_errors = true,
            MessageType::Warning => has_warnings = true,
            _ => {}
        }
        print_bullet(&msg, t);
    }

    let (msg, t) = check_daemon(runtime_dir);
    match t {
        MessageType::Error => has_errors = true,
        MessageType::Warning => has_warnings = true,
        _ => {}
    }
    print_bullet(&msg, t);

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
