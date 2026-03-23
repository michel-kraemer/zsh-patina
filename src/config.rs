use std::time::Duration;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{MapAccess, Visitor, value::MapAccessDeserializer},
};

use crate::theme::ThemeSource;

#[derive(Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub highlighting: HighlightingConfig,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HighlightingConfig {
    /// Either the name of a built-in theme (`"simple"`, `"patina"`,
    /// `"lavender"`) or a string in the form `"file:mytheme.toml"` pointing to
    /// a custom theme toml file.
    pub theme: ThemeSource,

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
            theme: ThemeSource::Patina,
            dynamic: DynamicConfig::default(),
            max_line_length: 20000,
            timeout: Duration::from_millis(500),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DynamicConfig {
    pub callables: bool,
    pub paths: bool,
}

impl Default for DynamicConfig {
    fn default() -> Self {
        Self {
            callables: true,
            paths: true,
        }
    }
}

impl Serialize for DynamicConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if self.callables == self.paths {
            serializer.serialize_bool(self.callables)
        } else {
            use serde::ser::SerializeMap;
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
                    paths: v,
                })
            }

            fn visit_map<M>(self, map: M) -> Result<DynamicConfig, M::Error>
            where
                M: MapAccess<'de>,
            {
                #[derive(Deserialize)]
                struct Helper {
                    #[serde(default = "default_true")]
                    callables: bool,
                    #[serde(default = "default_true")]
                    paths: bool,
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
