use std::{collections::HashMap, fmt::Formatter, fs, str::FromStr};

use anyhow::{Context, Result, bail};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{Error, MapAccess, Visitor, value::MapAccessDeserializer},
};
use syntect::{
    highlighting::{
        Color, ScopeSelector, ScopeSelectors, StyleModifier, Theme as SyntectTheme, ThemeItem,
        ThemeSettings,
    },
    parsing::ScopeStack,
};

/// Convert a color in the format #RRGGBB or #RGB to a `Color`
fn from_hex(s: &str) -> Result<Color> {
    let s = s.strip_prefix('#').context("Color must start with '#'")?;
    if s.len() == 6 {
        let r = u8::from_str_radix(&s[0..2], 16)?;
        let g = u8::from_str_radix(&s[2..4], 16)?;
        let b = u8::from_str_radix(&s[4..6], 16)?;
        Ok(Color { r, g, b, a: 255 })
    } else if s.len() == 3 {
        let mut r = u8::from_str_radix(&s[0..1], 16)?;
        let mut g = u8::from_str_radix(&s[1..2], 16)?;
        let mut b = u8::from_str_radix(&s[2..3], 16)?;
        r |= r << 4;
        g |= g << 4;
        b |= b << 4;
        Ok(Color { r, g, b, a: 255 })
    } else {
        bail!("Color must be in the format #RRGGBB or #RGB");
    }
}

/// Parse a color in the format #RRGGBB, #RGB, or an ANSI name
fn parse_color(s: &str) -> Result<Color> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "black" => Color {
            r: 0,
            g: 0,
            b: 0,
            a: 0,
        },
        "red" => Color {
            r: 1,
            g: 0,
            b: 0,
            a: 0,
        },
        "green" => Color {
            r: 2,
            g: 0,
            b: 0,
            a: 0,
        },
        "yellow" => Color {
            r: 3,
            g: 0,
            b: 0,
            a: 0,
        },
        "blue" => Color {
            r: 4,
            g: 0,
            b: 0,
            a: 0,
        },
        "magenta" => Color {
            r: 5,
            g: 0,
            b: 0,
            a: 0,
        },
        "cyan" => Color {
            r: 6,
            g: 0,
            b: 0,
            a: 0,
        },
        "white" => Color {
            r: 7,
            g: 0,
            b: 0,
            a: 0,
        },
        _ => from_hex(s)?,
    })
}

#[derive(Clone, PartialEq, Eq)]
pub enum ThemeSource {
    Simple,
    Patina,
    Lavender,
    File(String),
}

impl Serialize for ThemeSource {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            ThemeSource::Simple => serializer.serialize_str("simple"),
            ThemeSource::Patina => serializer.serialize_str("patina"),
            ThemeSource::Lavender => serializer.serialize_str("lavender"),
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
            "simple" => Ok(ThemeSource::Simple),
            "patina" => Ok(ThemeSource::Patina),
            "lavender" => Ok(ThemeSource::Lavender),
            _ if s.starts_with("file:") => Ok(ThemeSource::File(s[5..].to_string())),
            _ => Err(Error::custom(format!("Unsupported theme source: {s}"))),
        }
    }
}

#[derive(Default)]
pub struct Style {
    pub foreground: String,
    pub background: Option<String>,
}

impl TryFrom<&Style> for StyleModifier {
    type Error = anyhow::Error;

    fn try_from(style: &Style) -> Result<Self> {
        Ok(Self {
            foreground: Some(parse_color(&style.foreground)?),
            background: style
                .background
                .as_ref()
                .map(|f| parse_color(f))
                .transpose()?,
            ..Default::default()
        })
    }
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
                    foreground: value.to_string(),
                    ..Default::default()
                })
            }

            fn visit_map<M>(self, map: M) -> Result<Style, M::Error>
            where
                M: MapAccess<'de>,
            {
                #[derive(Deserialize)]
                struct Helper {
                    foreground: String,
                    background: Option<String>,
                }
                let h = Helper::deserialize(MapAccessDeserializer::new(map))?;
                Ok(Style {
                    foreground: h.foreground,
                    background: h.background,
                })
            }
        }

        deserializer.deserialize_any(StringOrStruct)
    }
}

#[derive(Deserialize)]
pub struct Theme {
    #[serde(flatten)]
    scopes: HashMap<String, Style>,
}

impl Theme {
    /// Load a built-in theme or a custom one from a file
    pub fn load(source: &ThemeSource) -> Result<Self> {
        Ok(match source {
            ThemeSource::Simple => toml::from_slice(include_bytes!("../themes/simple.toml"))
                .expect("Unable to load simple theme"),
            ThemeSource::Patina => toml::from_slice(include_bytes!("../themes/patina.toml"))
                .expect("Unable to load default theme"),
            ThemeSource::Lavender => toml::from_slice(include_bytes!("../themes/lavender.toml"))
                .expect("Unable to load lavender theme"),
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
    pub fn resolve<'a>(&'a self, scope: &str) -> Option<&'a Style> {
        let mut s = scope;
        while !s.is_empty() {
            if let Some(c) = self.scopes.get(s) {
                return Some(c);
            }
            s = s.rsplit_once('.')?.0;
        }
        None
    }
}

impl TryFrom<Theme> for SyntectTheme {
    type Error = anyhow::Error;

    fn try_from(theme: Theme) -> Result<Self> {
        Ok(SyntectTheme {
            settings: ThemeSettings {
                foreground: Some(Color {
                    r: 7,
                    g: 0,
                    b: 0,
                    a: 0,
                }),
                // this will be converted to `None` in the highlighter module:
                background: Some(Color {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 1,
                }),
                ..Default::default()
            },
            scopes: theme
                .scopes
                .iter()
                .map(|s| {
                    Ok(ThemeItem {
                        scope: ScopeSelectors {
                            selectors: vec![ScopeSelector {
                                path: ScopeStack::from_str(s.0)?,
                                ..Default::default()
                            }],
                        },
                        style: s.1.try_into()?,
                    })
                })
                .collect::<Result<_>>()?,
            ..Default::default()
        })
    }
}
