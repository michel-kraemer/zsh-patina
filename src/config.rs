use std::{env, fs::DirBuilder, os::unix::fs::DirBuilderExt, path::PathBuf, time::Duration};

use anyhow::{Context, Result, bail};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{MapAccess, Visitor, value::MapAccessDeserializer},
    ser::SerializeMap,
};

use crate::theme::{ThemeConfig, ThemeSource};

#[derive(Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub highlighting: HighlightingConfig,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct HighlightingConfig {
    /// Either the name of a built-in theme (`"simple"`, `"patina"`,
    /// `"lavender"`), a string in the form `"file:mytheme.toml"` pointing to a
    /// custom theme toml file, or an adaptive table that follows the operating
    /// system's light/dark appearance, e.g.
    /// `{ light = "catppuccin-latte", dark = "catppuccin-mocha" }`.
    pub theme: ThemeConfig,

    /// If enabled, zsh-patina will highlight callables (aliases, builtins,
    /// commands, and functions) as well as files and directories dynamically
    /// based on whether they exist (and the user has permission to
    /// execute/access them).
    ///
    /// Callables that cannot be called are highlighted with the theme's
    /// `dynamic.callable.missing.shell` scope (`red` by default) and with the
    /// scopes `dynamic.callable.alias.shell`, `dynamic.callable.builtin.shell`,
    /// `dynamic.callable.command.shell`, or `dynamic.callable.function.shell`
    /// if they do exist and are executable. Files and directories that exist
    /// and can be accessed are highlighted with the scopes
    /// `dynamic.path.file.shell` and `dynamic.path.directory.shell`,
    /// respectively.
    ///
    /// The styles of the dynamic scopes are mixed into the normal styles, which
    /// means, first the normal styles are applied, and then every attribute of
    /// the dynamic style overwrites the normal style's attribute with the same
    /// name. For example, if `variable.function.shell` (the normal style for
    /// callables if dynamic highlighting is disabled) specifies that a callable
    /// should be highlighted in blue, and `dynamic.callable.command.shell`
    /// specifies `underline = true`, then any command that exists and can be
    /// executed will be highlighted in blue and underlined.
    ///
    /// This option can be set to `true` or `false` to enable or disable all
    /// dynamic highlighting, or it can be set to a table with the keys
    /// `callables` and `paths` to enable or disable dynamic highlighting for
    /// callables and paths separately. For example:
    ///
    /// ```toml
    /// [highlighting.dynamic]
    /// callables = true
    /// paths = false
    /// ```
    pub dynamic: DynamicConfig,

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

    /// A list of custom precommands to recognise in addition to the built-in
    /// ones (`sudo`, `env`, `nohup`, `nice`, and others). Each entry describes
    /// the precommand's name, the mode used to highlight what follows, and the
    /// options it accepts. Defaults to an empty list.
    ///
    /// See [`PrecommandConfig`] for details and a configuration example.
    pub precommands: Vec<PrecommandConfig>,
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
            theme: ThemeConfig::Single(ThemeSource::Patina),
            dynamic: DynamicConfig::default(),
            max_line_length: 20000,
            timeout: Duration::from_millis(500),
            precommands: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Default, Debug)]
pub enum DynamicConfigType {
    /// Disable dynamic highlighting for paths
    None,

    /// Dynamically highlight paths even if only a prefix has been entered
    Partial,

    /// Dynamically highlight paths only if they have been entered completely
    #[default]
    Complete,
}

impl<'de> Deserialize<'de> for DynamicConfigType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct DynamicConfigTypeVisitor;

        impl<'de> Visitor<'de> for DynamicConfigTypeVisitor {
            type Value = DynamicConfigType;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str(r#"a string with value "none", "partial", "complete""#)
            }

            fn visit_bool<E>(self, v: bool) -> Result<DynamicConfigType, E> {
                Ok(if v {
                    DynamicConfigType::default()
                } else {
                    DynamicConfigType::None
                })
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match v {
                    "none" => Ok(DynamicConfigType::None),
                    "partial" => Ok(DynamicConfigType::Partial),
                    "complete" => Ok(DynamicConfigType::Complete),
                    "true" => Ok(DynamicConfigType::default()),
                    "false" => Ok(DynamicConfigType::None),
                    _ => Err(E::custom(format!(
                        r#"Invalid value: `{v}'. Expected one of "none", "partial", "complete""#,
                    ))),
                }
            }
        }

        deserializer.deserialize_any(DynamicConfigTypeVisitor)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DynamicConfig {
    pub callables: bool,
    pub paths: DynamicConfigType,
}

impl Default for DynamicConfig {
    fn default() -> Self {
        Self {
            callables: true,
            paths: DynamicConfigType::default(),
        }
    }
}

impl Serialize for DynamicConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if (self.callables && self.paths == DynamicConfigType::default())
            || (!self.callables && self.paths == DynamicConfigType::None)
        {
            serializer.serialize_bool(self.callables)
        } else {
            let mut map = serializer.serialize_map(Some(2))?;
            map.serialize_entry("callables", &self.callables)?;
            map.serialize_entry("paths", &self.paths)?;
            map.end()
        }
    }
}

impl<'de> Deserialize<'de> for DynamicConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct DynamicConfigVisitor;

        impl<'de> Visitor<'de> for DynamicConfigVisitor {
            type Value = DynamicConfig;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a boolean or a table with 'callables' and/or 'paths' keys")
            }

            fn visit_bool<E>(self, v: bool) -> Result<DynamicConfig, E> {
                Ok(DynamicConfig {
                    callables: v,
                    paths: if v {
                        DynamicConfigType::default()
                    } else {
                        DynamicConfigType::None
                    },
                })
            }

            fn visit_map<M>(self, map: M) -> Result<DynamicConfig, M::Error>
            where
                M: MapAccess<'de>,
            {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Helper {
                    #[serde(default = "default_true")]
                    callables: bool,
                    #[serde(default)]
                    paths: DynamicConfigType,
                }

                fn default_true() -> bool {
                    true
                }

                let h = Helper::deserialize(MapAccessDeserializer::new(map))?;
                Ok(DynamicConfig {
                    callables: h.callables,
                    paths: h.paths,
                })
            }
        }

        deserializer.deserialize_any(DynamicConfigVisitor)
    }
}

/// Controls how zsh-patina highlights the word that follows a precommand's
/// options.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PrecommandMode {
    /// The next word is treated as a callable -- a command, alias, function, or
    /// builtin -- and highlighting continues as if that callable were at the
    /// start of the line. This is the default and is appropriate for
    /// precommands such as `sudo` or `env` that prefix another command.
    #[default]
    Default,

    /// The remaining words are treated as plain arguments rather than a
    /// callable followed by its arguments. This is appropriate for precommands
    /// such as `sudoedit` that take file names instead of a command to run.
    Arguments,
}

/// Specifies the mode to switch to after a particular option has been
/// processed.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PrecommandSwitchTo {
    /// After the option (and its argument, if any) has been consumed, the
    /// remaining words are treated as plain arguments rather than a callable
    /// followed by its arguments. This is analogous to `sudo`'s `-e`/`--edit`
    /// option, which causes `sudo` to behave like `sudoedit`.
    Arguments,
}

/// Specifies whether an option takes an argument, and if so, whether it is
/// required.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PrecommandArg {
    /// The option must be followed by an argument. The argument is consumed
    /// before highlighting continues. This is the default.
    #[default]
    Required,

    /// The option may be followed by an argument. Whether the next word is
    /// treated as an argument or as the start of the command depends on
    /// context.
    Optional,

    /// The option takes no argument.
    None,
}

/// Describes a single option accepted by a precommand.
///
/// At least one of `short` or `long` should be set. Both may be set if the
/// option has both a short and a long form.
#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct PrecommandOption {
    /// The short form of the option, without the leading dash (e.g. `"u"` for
    /// `-u`).
    pub short: Option<String>,

    /// The long form of the option, without the leading dashes (e.g. `"user"`
    /// for `--user`).
    pub long: Option<String>,

    /// Whether this option takes an argument, and if so, whether the argument
    /// is required or optional. Defaults to [`PrecommandArg::Required`].
    #[serde(default)]
    pub arg: PrecommandArg,

    /// If set, switches the highlighting mode after this option (and its
    /// argument, if any) has been consumed. This is useful for options that
    /// fundamentally change the nature of the remaining arguments, such as
    /// `sudo`'s `-e`/`--edit` option.
    pub switch_to_mode: Option<PrecommandSwitchTo>,
}

/// Configures a custom precommand — a command that, when followed by another
/// command or arguments, causes zsh-patina to highlight the subsequent words in
/// a specific way. Examples of built-in precommands include `sudo`, `env`, and
/// `nohup`.
///
/// Custom precommands can be added to the `precommands` list in the
/// `[highlighting]` section of the configuration file. For example:
///
/// ```toml
/// [[highlighting.precommands]]
/// name = "mywrapper"
/// options = [
///     { short = "u", long = "user", arg = "required" },
///     { short = "n", arg = "none" },
/// ]
/// ```
#[derive(Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct PrecommandConfig {
    /// The name of the precommand as it appears in the shell
    pub name: String,

    /// Controls how the word following the precommand's options is highlighted.
    /// Defaults to [`PrecommandMode::Default`].
    #[serde(default)]
    pub mode: PrecommandMode,

    /// The options that the precommand accepts before the command or arguments.
    /// Each entry describes one option, its argument behavior, and an optional
    /// mode switch. Defaults to an empty list.
    #[serde(default)]
    pub options: Vec<PrecommandOption>,
}

impl HighlightingConfig {
    /// Checks the `precommands` list for configuration errors that would
    /// prevent highlighting from working correctly.
    pub fn validate(&self) -> Result<()> {
        let mut seen_names = std::collections::HashSet::new();
        for precommand in &self.precommands {
            if precommand.name.is_empty() {
                bail!("precommand name must not be empty");
            }

            if !seen_names.insert(precommand.name.as_str()) {
                bail!("duplicate precommand name: {:?}", precommand.name);
            }

            for option in &precommand.options {
                if option.short.is_none() && option.long.is_none() {
                    bail!(
                        "precommand {:?}: option must have at least one of `short` or `long`",
                        precommand.name
                    );
                }

                if let Some(short) = &option.short
                    && (short.len() != 1
                        || !short.chars().next().is_some_and(|c| c.is_alphanumeric()))
                {
                    bail!(
                        "precommand {:?}: short option {:?} must be a single ASCII letter, digit, or underscore",
                        precommand.name,
                        short
                    );
                }

                if let Some(long) = &option.long
                    && long.is_empty()
                {
                    bail!(
                        "precommand {:?}: long option must not be empty",
                        precommand.name
                    );
                }
            }
        }

        Ok(())
    }
}

/// Returns the path to the configuration file if it exists. The configuration
/// file is searched in the following locations (in order):
///
/// 1. `$ZSH_PATINA_CONFIG_PATH` if it is set.
/// 2. `$XDG_CONFIG_HOME/zsh-patina/config.toml` if the `XDG_CONFIG_HOME`
///    environment variable is set and points to an absolute path
/// 3. `~/.config/zsh-patina/config.toml`
///
/// If no configuration file is found, the function returns `Ok(None)`.
pub fn config_file_path() -> Result<Option<PathBuf>> {
    if let Some(config_file) = env::var_os("ZSH_PATINA_CONFIG_PATH")
        && !config_file.is_empty()
    {
        return Ok(Some(PathBuf::from(config_file)));
    }

    if let Some(xdg) = env::var_os("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        let xdg = PathBuf::from(xdg);
        if xdg.is_absolute() {
            let result = xdg.join("zsh-patina/config.toml");
            // for backwards compatibility, we fall through to looking at the
            // default location of there is no config file in $XDG_CONFIG_HOME
            if result.exists() {
                return Ok(Some(result));
            }
        }
    }

    let home = dirs::home_dir().context("Unable to find home directory")?;
    let result = home.join(".config/zsh-patina/config.toml");
    if result.exists() {
        Ok(Some(result))
    } else {
        Ok(None)
    }
}

/// Returns the path to the runtime directory, which is used for storing the PID
/// file and the daemon's Unix socket. The runtime directory is either:
///
/// 1. `$XDG_RUNTIME_DIR/zsh-patina` if the `XDG_RUNTIME_DIR` environment
///    variable is set and points to an absolute path,
/// 2. on macOS, `$TMPDIR/zsh-patina` (where `$TMPDIR` is typically something
///    like `/var/folders/.../T/` and is user-specific),
/// 3. or a user-owned subdirectory of the temporary directory (e.g.
///    `/tmp/zsh-patina-1000`), which is created if it doesn't already exists.
///    The user-owned subdirectory is necessary because the temporary directory
///    is typically world-writable.
pub fn runtime_dir() -> std::io::Result<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_RUNTIME_DIR")
        && !xdg.is_empty()
    {
        let xdg = PathBuf::from(xdg);
        if xdg.is_absolute() {
            return Ok(xdg.join("zsh-patina"));
        }
    }

    #[cfg(target_os = "macos")]
    {
        // On macOS, temp_dir is user specific. So that's all we need to do.
        let tmp = std::env::temp_dir();
        if tmp.is_dir() {
            return Ok(tmp.join("zsh-patina"));
        }
    }

    // Fallback to temporary directory ...

    // SAFETY: getuid() never fails and just returns a u32
    let uid = unsafe { libc::getuid() };

    // create user-owned subdirectory because the temporary might be
    // world-writable
    let path = std::env::temp_dir().join(format!("zsh-patina-{uid}"));
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&path)?;

    Ok(path)
}
