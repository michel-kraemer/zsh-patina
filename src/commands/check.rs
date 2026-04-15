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

pub fn check(
    config: &Config,
    config_file_path: &Option<PathBuf>,
    runtime_dir: &Path,
) -> Result<()> {
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
                "No configuration file found at `$ZSH_PATINA_CONFIG_PATH`, \
                `$XDG_CONFIG_HOME/zsh-patina/config.toml', or \
                `{}/.config/zsh-patina/config.toml'. \
                Using default settings.",
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
    if let Some(pid) = is_daemon_running(runtime_dir) {
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

    // check if `zsh-patina activate` is called in the zshrc file and if that
    // happens in the last line
    match Command::new("zsh")
        .args(["-i", "-c", "typeset -f zsh-patina"])
        .stderr(Stdio::null()) // suppress interactive shell noise
        .output()
    {
        Err(e) => {
            print_bullet(
                &format!(
                    "Failed to spawn a Zsh process to check if zsh-patina is \
                    `activated.\n\n{e}"
                ),
                MessageType::Warning,
            );
            has_warnings = true;
        }

        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let function_body = stdout.trim();

            if function_body.is_empty() {
                print_bullet(
                    "The `zsh-patina' shell function was not found in your \
                    interactive shell session. Please make sure zsh-patina \
                    is activated in your shell configuration.",
                    MessageType::Error,
                );
                has_errors = true;
            } else {
                // extract the binary path from a line of the form:
                // "/path/to/zsh-patina" "$@"
                let extracted_path = function_body.lines().find_map(|line| {
                    let trimmed = line.trim();
                    let without_args = trimmed.strip_suffix("\"$@\"")?.trim_end();
                    let inner = without_args.strip_prefix('"')?.strip_suffix('"')?;
                    Some(PathBuf::from(inner))
                });

                match extracted_path {
                    None => {
                        print_bullet(
                            "The `zsh-patina' shell function was found but its \
                            body could not be parsed to verify the binary path.",
                            MessageType::Warning,
                        );
                        has_warnings = true;
                    }

                    Some(function_path) => {
                        let current_exe = std::env::current_exe()?;

                        // Canonicalize both paths to resolve symlinks and
                        // normalize them before comparing. If the path stored
                        // in the function is stale, canonicalize will fail for
                        // it while succeeding for current_exe, which we treat
                        // as a mismatch.
                        let fp = function_path.canonicalize().ok();
                        let ce = current_exe.canonicalize().ok();

                        if fp.is_some() && fp == ce {
                            print_bullet(
                                "zsh-patina is activated correctly.",
                                MessageType::Success,
                            );
                        } else {
                            print_bullet(
                                &format!(
                                    "The `zsh-patina' shell function points to \
                                    `{}', but the current binary is at `{}'. \
                                    Please re-run the activation command to \
                                    update your shell configuration.",
                                    function_path.to_string_lossy(),
                                    current_exe.to_string_lossy()
                                ),
                                MessageType::Warning,
                            );
                            has_warnings = true;
                        }
                    }
                }
            }
        }
    }

    // check if the ZLE widget is registered correctly
    match Command::new("zsh")
        .args(["-i", "-c", "zle -l"])
        .stderr(Stdio::null())
        .output()
    {
        Err(e) => {
            print_bullet(
                &format!(
                    "Failed to spawn a Zsh process to check for the \
                    `_zsh_patina' ZLE widget.\n\n{e}"
                ),
                MessageType::Warning,
            );
            has_warnings = true;
        }

        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let widget_found = stdout.lines().any(|l| l.trim() == "_zsh_patina");

            if widget_found {
                print_bullet(
                    "The ZLE widget is registered correctly.",
                    MessageType::Success,
                );
            } else {
                print_bullet(
                    "The ZLE widget was not found in your interactive shell \
                    session. It might have been removed by another Zsh plugin. \
                    Please activate zsh-patina at the end of your .zshrc file.",
                    MessageType::Error,
                );
                has_errors = true;
            }
        }
    };

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
