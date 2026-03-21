use std::{fmt::Formatter, fs, str::FromStr};

use anyhow::{Context, Result};
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

#[derive(Clone, PartialEq, Eq)]
pub enum ThemeSource {
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

#[derive(Clone, Copy, Default)]
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

#[derive(Deserialize)]
pub struct Theme {
    #[serde(flatten)]
    scopes: FxHashMap<String, Style>,
}

impl Theme {
    /// Load a built-in theme or a custom one from a file
    pub fn load(source: &ThemeSource) -> Result<Self> {
        Ok(match source {
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
        })
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
