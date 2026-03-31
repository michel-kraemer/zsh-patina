use std::fmt::Formatter;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Deserializer, de::Visitor};
use termcolor::Color as TermColor;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Color {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    Ansi256(u8),
    Rgb(u8, u8, u8),
}

/// Convert a color in the format #RRGGBB or #RGB to a `Color`
fn from_hex(s: &str) -> Result<Color> {
    let s = s.strip_prefix('#').context("Color must start with '#'")?;
    if s.len() == 6 {
        let r = u8::from_str_radix(&s[0..2], 16)?;
        let g = u8::from_str_radix(&s[2..4], 16)?;
        let b = u8::from_str_radix(&s[4..6], 16)?;
        Ok(Color::Rgb(r, g, b))
    } else if s.len() == 3 {
        let mut r = u8::from_str_radix(&s[0..1], 16)?;
        let mut g = u8::from_str_radix(&s[1..2], 16)?;
        let mut b = u8::from_str_radix(&s[2..3], 16)?;
        r |= r << 4;
        g |= g << 4;
        b |= b << 4;
        Ok(Color::Rgb(r, g, b))
    } else {
        bail!("Color must be in the format #RRGGBB or #RGB");
    }
}

fn to_hex(r: u8, g: u8, b: u8) -> String {
    format!("#{:0>2x}{:0>2x}{:0>2x}", r, g, b)
}

impl TryFrom<&str> for Color {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value.to_ascii_lowercase().as_str() {
            "black" => Ok(Color::Black),
            "red" => Ok(Color::Red),
            "green" => Ok(Color::Green),
            "yellow" => Ok(Color::Yellow),
            "blue" => Ok(Color::Blue),
            "magenta" => Ok(Color::Magenta),
            "cyan" => Ok(Color::Cyan),
            "white" => Ok(Color::White),
            _ if value.starts_with('#') => from_hex(value),
            _ => Ok(Color::Ansi256(value.parse().context(
                r##"Color must either be a string referencing one of the eight ANSI colors "black", "red", "green", "yellow", "blue", "magenta", "cyan", or "white", a hex color in the format "#RRGGBB" or "#RGB", or an integer in the range 0-255 specifying an 8-bit ANSI color code."##,
            )?)),
        }
    }
}

impl From<Color> for TermColor {
    fn from(value: Color) -> Self {
        (&value).into()
    }
}

impl From<&Color> for TermColor {
    fn from(value: &Color) -> Self {
        match value {
            Color::Black => TermColor::Black,
            Color::Red => TermColor::Red,
            Color::Green => TermColor::Green,
            Color::Yellow => TermColor::Yellow,
            Color::Blue => TermColor::Blue,
            Color::Magenta => TermColor::Magenta,
            Color::Cyan => TermColor::Cyan,
            Color::White => TermColor::White,
            Color::Ansi256(c) => TermColor::Ansi256(*c),
            Color::Rgb(r, g, b) => TermColor::Rgb(*r, *g, *b),
        }
    }
}

impl Color {
    /// Convert a Color to an ANSI color string
    pub fn to_ansi_color(self) -> String {
        match self {
            Color::Black => "black".to_string(),
            Color::Red => "red".to_string(),
            Color::Green => "green".to_string(),
            Color::Yellow => "yellow".to_string(),
            Color::Blue => "blue".to_string(),
            Color::Magenta => "magenta".to_string(),
            Color::Cyan => "cyan".to_string(),
            Color::White => "white".to_string(),
            Color::Ansi256(c) => c.to_string(),
            Color::Rgb(r, g, b) => to_hex(r, g, b),
        }
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct StringOrInt;

        impl<'de> Visitor<'de> for StringOrInt {
            type Value = Color;

            fn expecting(&self, formatter: &mut Formatter) -> std::fmt::Result {
                formatter.write_str("string or 8-bit number")
            }

            fn visit_str<E>(self, value: &str) -> Result<Color, E>
            where
                E: serde::de::Error,
            {
                Color::try_from(value).map_err(E::custom)
            }

            fn visit_i64<E>(self, value: i64) -> Result<Color, E>
            where
                E: serde::de::Error,
            {
                let value = u8::try_from(value).map_err(E::custom)?;
                Ok(Color::Ansi256(value))
            }
        }

        deserializer.deserialize_any(StringOrInt)
    }
}
