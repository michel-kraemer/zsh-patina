use std::{
    fmt::{self, Display, Formatter},
    fs,
    str::FromStr,
};

use anyhow::{Context, Result, bail};
use rustc_hash::FxHashMap;
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{Error, MapAccess, Visitor, value::MapAccessDeserializer},
};
use syntect::{
    highlighting::{
        Color as SyntectColor, ScopeSelector, ScopeSelectors, StyleModifier, Theme as SyntectTheme,
        ThemeItem, ThemeSettings,
    },
    parsing::ScopeStack,
};

use crate::color::Color;

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ThemeSource {
    Classic,
    Lavender,
    Nord,
    Patina,
    Simple,
    TokyoNight,
    File(String),
}

impl Serialize for ThemeSource {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            ThemeSource::Classic => serializer.serialize_str("classic"),
            ThemeSource::Lavender => serializer.serialize_str("lavender"),
            ThemeSource::Nord => serializer.serialize_str("nord"),
            ThemeSource::Patina => serializer.serialize_str("patina"),
            ThemeSource::Simple => serializer.serialize_str("simple"),
            ThemeSource::TokyoNight => serializer.serialize_str("tokyonight"),
            ThemeSource::File(path) => serializer.serialize_str(&format!("file:{path}")),
        }
    }
}

impl<'de> Deserialize<'de> for ThemeSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.as_str() {
            "classic" => Ok(ThemeSource::Classic),
            "lavender" => Ok(ThemeSource::Lavender),
            "nord" => Ok(ThemeSource::Nord),
            "patina" => Ok(ThemeSource::Patina),
            "simple" => Ok(ThemeSource::Simple),
            "tokyonight" => Ok(ThemeSource::TokyoNight),
            _ if s.starts_with("file:") => Ok(ThemeSource::File(
                shellexpand::full(&s[5..])
                    .map_err(D::Error::custom)?
                    .to_string(),
            )),
            _ => Err(Error::custom(format!("Unsupported theme source: {s}"))),
        }
    }
}

impl Display for ThemeSource {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            ThemeSource::Classic => write!(f, "classic"),
            ThemeSource::Lavender => write!(f, "lavender"),
            ThemeSource::Nord => write!(f, "nord"),
            ThemeSource::Patina => write!(f, "patina"),
            ThemeSource::Simple => write!(f, "simple"),
            ThemeSource::TokyoNight => write!(f, "tokyonight"),
            ThemeSource::File(path) => write!(f, "file:{path}"),
        }
    }
}

#[derive(Deserialize, Default, Debug)]
pub struct ThemeMetadata {
    pub extends: Option<ThemeSource>,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Style {
    pub foreground: Option<Color>,
    pub background: Option<Color>,
    pub bold: bool,
    pub underline: bool,
}

impl<'de> Deserialize<'de> for Style {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct StringOrStruct;

        impl<'de> Visitor<'de> for StringOrStruct {
            type Value = Style;

            fn expecting(&self, formatter: &mut Formatter) -> std::fmt::Result {
                formatter.write_str("string or style struct")
            }

            fn visit_str<E>(self, value: &str) -> Result<Style, E>
            where
                E: serde::de::Error,
            {
                Ok(Style {
                    foreground: Some(Color::try_from(value).map_err(E::custom)?),
                    ..Default::default()
                })
            }

            fn visit_map<M>(self, map: M) -> Result<Style, M::Error>
            where
                M: MapAccess<'de>,
            {
                #[derive(Deserialize)]
                struct Helper {
                    foreground: Option<String>,
                    background: Option<String>,
                    #[serde(default)]
                    bold: bool,
                    #[serde(default)]
                    underline: bool,
                }

                let h = Helper::deserialize(MapAccessDeserializer::new(map))?;

                Ok(Style {
                    foreground: h
                        .foreground
                        .map(|fg| Color::try_from(fg.as_str()).map_err(M::Error::custom))
                        .transpose()?,
                    background: h
                        .background
                        .map(|bg| Color::try_from(bg.as_str()).map_err(M::Error::custom))
                        .transpose()?,
                    bold: h.bold,
                    underline: h.underline,
                })
            }
        }

        deserializer.deserialize_any(StringOrStruct)
    }
}

#[derive(Debug, Deserialize)]
pub struct Theme {
    #[serde(default)]
    metadata: Option<ThemeMetadata>,

    #[serde(flatten)]
    scopes: FxHashMap<String, Style>,
}

impl Theme {
    /// Load a built-in theme or a custom one from a file.
    ///
    /// If the theme has a `[metadata]` table with an `extends` key, the
    /// referenced base theme is loaded first and the current theme's scopes are
    /// merged on top (child scopes override parent scopes with the same key).
    /// Multi-level chaining is supported. Cycles are detected and reported as
    /// errors.
    pub fn load(source: &ThemeSource) -> Result<Self> {
        Self::load_inner(source, &mut Vec::new())
    }

    fn load_inner(source: &ThemeSource, visited: &mut Vec<ThemeSource>) -> Result<Self> {
        if let Some(pos) = visited.iter().position(|s| s == source) {
            let cycle = visited[pos..]
                .iter()
                .chain(std::iter::once(source))
                .map(|s| s.to_string())
                .collect::<Vec<_>>();
            bail!("Cycle detected in theme inheritance: {}", cycle.join(" > "));
        }
        visited.push(source.clone());

        let mut theme: Theme = match source {
            ThemeSource::Classic => toml::from_slice(include_bytes!("../themes/classic.toml"))
                .context("Unable to load classic theme")?,
            ThemeSource::Lavender => toml::from_slice(include_bytes!("../themes/lavender.toml"))
                .context("Unable to load lavender theme")?,
            ThemeSource::Nord => toml::from_slice(include_bytes!("../themes/nord.toml"))
                .context("Unable to load nord theme")?,
            ThemeSource::Patina => toml::from_slice(include_bytes!("../themes/patina.toml"))
                .context("Unable to load default theme")?,
            ThemeSource::Simple => toml::from_slice(include_bytes!("../themes/simple.toml"))
                .context("Unable to load simple theme")?,
            ThemeSource::TokyoNight => {
                toml::from_slice(include_bytes!("../themes/tokyonight.toml"))
                    .context("Unable to load tokyonight theme")?
            }
            ThemeSource::File(path) => {
                let theme_source = fs::read_to_string(path)
                    .with_context(|| format!("Failed to read theme file `{path}'"))?;
                toml::from_str(&theme_source)
                    .with_context(|| format!("Failed to parse theme file `{path}'"))?
            }
        };

        if let Some(parent_source) = theme.metadata.as_ref().and_then(|m| m.extends.as_ref()) {
            let parent = Self::load_inner(parent_source, visited)?;
            let mut merged_scopes = parent.scopes;
            merged_scopes.extend(theme.scopes);
            theme.scopes = merged_scopes;
        }

        Ok(theme)
    }

    /// Resolve a scope to a color by looking it up in the theme. If the scope
    /// is not found, its parent scopes are tried until a match is found or
    /// there are no more parent scopes left.
    pub fn resolve(&self, scope: &str) -> Option<Style> {
        let mut s = scope;
        while !s.is_empty() {
            if let Some(c) = self.scopes.get(s) {
                return Some(*c);
            }
            s = s.rsplit_once('.')?.0;
        }
        None
    }

    pub fn to_syntect(&self, scope_mapping: &ScopeMapping) -> Result<SyntectTheme> {
        Ok(SyntectTheme {
            settings: ThemeSettings {
                foreground: Some(ScopeMapping::NONE),
                background: Some(ScopeMapping::NONE),
                ..Default::default()
            },
            scopes: self
                .scopes
                .iter()
                .map(|s| {
                    let foreground = scope_mapping
                        .encode(s.0)
                        .with_context(|| format!("Missing scope mapping for `{}'", s.0))?;
                    let style = StyleModifier {
                        foreground: Some(foreground),
                        ..Default::default()
                    };

                    Ok(ThemeItem {
                        scope: ScopeSelectors {
                            selectors: vec![ScopeSelector {
                                path: ScopeStack::from_str(s.0)?,
                                ..Default::default()
                            }],
                        },
                        style,
                    })
                })
                .collect::<Result<_>>()?,
            ..Default::default()
        })
    }
}

pub struct ScopeMapping {
    forward_mapping: FxHashMap<String, u32>,
    backward_mapping: Vec<String>,
}

impl ScopeMapping {
    pub const NONE: SyntectColor = SyntectColor {
        r: u8::MAX,
        g: u8::MAX,
        b: u8::MAX,
        a: u8::MAX,
    };

    pub fn new(theme: &Theme) -> Self {
        let mut forward_mapping = FxHashMap::default();
        let mut backward_mapping = Vec::new();
        for scope in theme.scopes.keys() {
            let id = backward_mapping.len();
            forward_mapping.insert(scope.clone(), id as u32);
            backward_mapping.push(scope.clone());
        }
        Self {
            forward_mapping,
            backward_mapping,
        }
    }

    pub fn encode(&self, scope: &str) -> Option<SyntectColor> {
        let id = self.forward_mapping.get(scope)?;
        Some(SyntectColor {
            r: ((id >> 24) & 0xFF) as u8,
            g: ((id >> 16) & 0xFF) as u8,
            b: ((id >> 8) & 0xFF) as u8,
            a: (id & 0xFF) as u8,
        })
    }

    pub fn decode(&self, color: &SyntectColor) -> Option<&str> {
        let id = (color.r as u32) << 24
            | (color.g as u32) << 16
            | (color.b as u32) << 8
            | (color.a as u32);
        match id {
            u32::MAX => None,
            _ => self.backward_mapping.get(id as usize).map(|s| s.as_str()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_theme(name: &str) -> ThemeSource {
        ThemeSource::File(format!(
            "{}/tests/themes/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
    }

    #[test]
    fn load_builtin_without_metadata() {
        let theme = Theme::load(&ThemeSource::Patina).unwrap();
        assert!(theme.resolve("comment").is_some());
    }

    #[test]
    fn extends_builtin_theme() {
        let theme = Theme::load(&test_theme("extends-nord.toml")).unwrap();

        // "comment" is overridden to red in the child
        let comment_style = theme.resolve("comment").unwrap();
        assert_eq!(
            comment_style.foreground,
            Some(Color::try_from("red").unwrap())
        );

        // "string" is inherited from nord (#A3BE8C)
        let string_style = theme.resolve("string").unwrap();
        assert_eq!(
            string_style.foreground,
            Some(Color::try_from("#A3BE8C").unwrap())
        );
    }

    #[test]
    fn cycle_detected() {
        let result = Theme::load(&test_theme("cycle-a.toml"));
        let err = result.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("Cycle"), "Expected cycle error, got: {msg}");
    }

    #[test]
    fn multi_level_chain() {
        // chain-a extends chain-b extends chain-c
        // chain-c: comment=green, string=yellow, keyword=magenta
        // chain-b: comment=red (overrides green)
        // chain-a: string=blue (overrides yellow)
        let theme = Theme::load(&test_theme("chain-a.toml")).unwrap();

        // string overridden by chain-a
        let string_style = theme.resolve("string").unwrap();
        assert_eq!(
            string_style.foreground,
            Some(Color::try_from("blue").unwrap())
        );

        // comment overridden by chain-b
        let comment_style = theme.resolve("comment").unwrap();
        assert_eq!(
            comment_style.foreground,
            Some(Color::try_from("red").unwrap())
        );

        // keyword from chain-c (base)
        let keyword_style = theme.resolve("keyword").unwrap();
        assert_eq!(
            keyword_style.foreground,
            Some(Color::try_from("magenta").unwrap())
        );
    }
}
