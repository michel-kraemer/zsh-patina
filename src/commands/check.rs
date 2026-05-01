use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::Result;
use log::{Level, LevelFilter, Metadata, Record};

use crate::{config::Config, daemon::is_daemon_running, theme::Theme};

static CHECK_LOGGER: CheckLogger = CheckLogger;

struct CheckLogger;

impl log::Log for CheckLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Info
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let t = match record.metadata().level() {
                log::Level::Error => MessageType::Error,
                log::Level::Warn => MessageType::Warning,
                log::Level::Info => MessageType::Info,
                _ => return,
            };
            print_bullet(&format!("{}", record.args()), t);
        }
    }

    fn flush(&self) {}
}

/// Initializes the logger for the `check` command. This logger always prints to
/// the console (regardless of any environment variable), and it uses the same
/// format as the `check` command.
pub fn init_check_logger() {
    log::set_logger(&CHECK_LOGGER).unwrap();
    log::set_max_level(LevelFilter::Info);
}

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

/// Gets `$ZDOTDIR` either from the environment variable or by spawning a zsh
/// process. Returns `None` if `$ZDOTDIR` is not set or if there was an error
/// while trying to get it.
fn get_zdotdir() -> Result<Option<String>> {
    if let Some(zdotdir) = std::env::var_os("ZDOTDIR")
        && let Some(zdotdir) = zdotdir.to_str()
    {
        return Ok(Some(zdotdir.to_string()));
    }

    let output = Command::new("zsh").args(["-c", "echo $ZDOTDIR"]).output()?;

    let val = String::from_utf8(output.stdout)?;
    let val = val.trim().to_string();
    Ok(if val.is_empty() { None } else { Some(val) })
}

/// Returns the path to the user's `.zshrc` file. If `$ZDOTDIR` is set, it returns
/// `$ZDOTDIR/.zshrc`. Otherwise, it returns `~/.zshrc`.
fn zshrc_path() -> Result<PathBuf> {
    if let Some(zdotdir) = get_zdotdir()? {
        Ok(PathBuf::from(zdotdir).join(".zshrc"))
    } else {
        Ok(PathBuf::from(shellexpand::full("~/.zshrc")?.as_ref()))
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

/// Check if the installed version of Zsh is at least 5.9
fn check_zsh_version() -> (String, MessageType) {
    let output = match Command::new("zsh").args(["--version"]).output() {
        Ok(o) => o,
        Err(e) => {
            return (
                format!("Failed to run `zsh --version'\n\n{e:?}"),
                MessageType::Error,
            );
        }
    };

    let output = String::from_utf8_lossy(&output.stdout);
    let Some(version) = output.split_whitespace().nth(1) else {
        return (
            format!(
                "Unable to evaluate installed Zsh version. Failed to parse `{}'.",
                output.trim()
            ),
            MessageType::Warning,
        );
    };

    let mut parts = version.split('.');
    let major = match parts
        .next()
        .expect("Version can never be empty")
        .parse::<u32>()
    {
        Ok(major) => major,
        Err(e) => {
            return (
                format!("Unable to parse installed Zsh version.\n\n{e:?}"),
                MessageType::Warning,
            );
        }
    };
    let minor = match parts.next() {
        Some(minor) => match minor.parse::<u32>() {
            Ok(minor) => minor,
            Err(e) => {
                return (
                    format!("Unable to parse installed Zsh version.\n\n{e:?}"),
                    MessageType::Warning,
                );
            }
        },
        None => 0,
    };

    if major > 5 || (major == 5 && minor >= 9) {
        (
            format!("Installed Zsh version is {version}."),
            MessageType::Success,
        )
    } else {
        (
            format!("Unsupported Zsh version {version}. zsh-patina requires at least version 5.9."),
            MessageType::Error,
        )
    }
}

/// Check if the configured theme can be loaded
fn check_theme(config: &Config) -> (String, MessageType) {
    match Theme::load(&config.highlighting.theme) {
        Err(e) => (
            format!("Theme could not be loaded\n\n{e:?}"),
            MessageType::Error,
        ),
        Ok(_) => (
            format!("Theme `{}' loaded successfully.", config.highlighting.theme),
            MessageType::Success,
        ),
    }
}

/// Check if zsh-patina is active in the current shell session
fn check_activation_current_shell() -> (String, MessageType) {
    let path = std::env::var_os("_ZSH_PATINA_PATH").unwrap_or_default();
    if path.is_empty() {
        (
            "The `$_ZSH_PATINA_PATH' environment variable was not found in the \
            current shell session. Please make sure zsh-patina is activated in \
            your .zshrc file and restart your current shell."
                .to_string(),
            MessageType::Error,
        )
    } else {
        (
            "zsh-patina is active in the current shell session.".to_string(),
            MessageType::Success,
        )
    }
}

/// Check if `zsh-patina activate` is called in the zshrc file and that it
/// happens in the last line
fn check_activate_in_zshrc(active_in_current_shell: bool) -> Result<(String, MessageType)> {
    let add = if active_in_current_shell {
        " Since zsh-patina is active in the current shell session, this might \
        not be a problem. If everything is working correctly, you can ignore \
        this warning."
    } else {
        ""
    };

    let zshrc_path = match zshrc_path() {
        Ok(zshrc_path) => zshrc_path,
        Err(e) => {
            return Ok((
                format!(
                    "Failed to resolve path to .zshrc. Unable to check if \
                    zsh-patina is activated when the shell is started.\n\n{e}"
                ),
                MessageType::Warning,
            ));
        }
    };

    let f = match File::open(&zshrc_path) {
        Ok(f) => f,
        Err(e) => {
            return Ok((
                format!(
                    "Failed to read `{}'. Unable to check if zsh-patina is \
                    activated when the shell is started.\n\n{e}",
                    zshrc_path.to_string_lossy()
                ),
                MessageType::Warning,
            ));
        }
    };

    let reader = BufReader::new(f);
    let mut activate_found = false;
    let mut more_lines = false;
    for l in reader.lines() {
        let l = l?;
        if l.is_empty() || l.trim().starts_with('#') {
            continue;
        } else {
            more_lines = true;
        }
        if l.contains("zsh-patina activate")
            || (l.contains("zinit") && l.contains("michel-kraemer/zsh-patina"))
        {
            activate_found = true;
            more_lines = false;
        }
    }

    if !activate_found {
        return Ok((
            format!(
                "The string `zsh-patina activate' was not found in your .zshrc \
                file at `{}'. Please make sure zsh-patina is activated when \
                your shell is started.{add}",
                zshrc_path.to_string_lossy()
            ),
            MessageType::Warning,
        ));
    }

    if more_lines {
        Ok((
            format!(
                "zsh-patina is not activated last in your .zshrc file at \
                `{}'. Make sure the `zsh-patina activate' call happens at the \
                end of the file.{add}",
                zshrc_path.to_string_lossy()
            ),
            MessageType::Warning,
        ))
    } else {
        Ok((
            "zsh-patina is activated correctly in your .zshrc file.".to_string(),
            MessageType::Success,
        ))
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
            MessageType::Error,
        ),
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

    let (msg, t) = check_zsh_version();
    match t {
        MessageType::Error => has_errors = true,
        MessageType::Warning => has_warnings = true,
        _ => {}
    }
    print_bullet(&msg, t);

    let (msg, t) = check_activation_current_shell();
    match t {
        MessageType::Error => has_errors = true,
        MessageType::Warning => has_warnings = true,
        _ => {}
    }
    print_bullet(&msg, t);
    let active_in_current_shell = t == MessageType::Success;

    let (msg, t) = check_activate_in_zshrc(active_in_current_shell)?;
    match t {
        MessageType::Error => has_errors = true,
        MessageType::Warning => has_warnings = true,
        _ => {}
    }
    print_bullet(&msg, t);

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
