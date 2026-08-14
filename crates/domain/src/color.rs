use std::error::Error;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Straight (non-premultiplied) RGBA with components in `0..=1`. Hex parsing and
/// formatting live here so the text form has one definition.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Rgba {
    pub const TRANSPARENT: Rgba = Rgba { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };

    pub fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self {
            r: r.clamp(0.0, 1.0),
            g: g.clamp(0.0, 1.0),
            b: b.clamp(0.0, 1.0),
            a: a.clamp(0.0, 1.0),
        }
    }

    /// Accepts `#rgb`, `#rgba`, `#rrggbb` and `#rrggbbaa`. The leading `#` is optional.
    pub fn parse_hex(input: &str) -> Result<Self, ParseColorError> {
        let digits = input.strip_prefix('#').unwrap_or(input);
        if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(ParseColorError { input: input.to_owned() });
        }
        let channels: [u8; 4] = match digits.len() {
            3 | 4 => {
                let mut out = [255u8; 4];
                for (slot, digit) in out.iter_mut().zip(digits.chars()) {
                    let nibble = digit.to_digit(16).expect("checked above") as u8;
                    *slot = nibble << 4 | nibble;
                }
                out
            }
            6 | 8 => {
                let mut out = [255u8; 4];
                for (index, slot) in out.iter_mut().enumerate() {
                    let Some(pair) = digits.get(index * 2..index * 2 + 2) else {
                        break;
                    };
                    *slot = u8::from_str_radix(pair, 16).expect("checked above");
                }
                out
            }
            _ => {
                return Err(ParseColorError { input: input.to_owned() });
            }
        };
        Ok(Self::from_bytes(channels))
    }

    pub fn from_bytes([r, g, b, a]: [u8; 4]) -> Self {
        Self {
            r: f32::from(r) / 255.0,
            g: f32::from(g) / 255.0,
            b: f32::from(b) / 255.0,
            a: f32::from(a) / 255.0,
        }
    }

    pub fn to_bytes(self) -> [u8; 4] {
        [byte(self.r), byte(self.g), byte(self.b), byte(self.a)]
    }

    /// Multiplies alpha, leaving the chroma alone. Used to apply the configured tint
    /// opacity to a tint colour that may already carry its own alpha.
    pub fn with_alpha_scaled(self, factor: f32) -> Self {
        Self { a: (self.a * factor).clamp(0.0, 1.0), ..self }
    }
}

impl Default for Rgba {
    fn default() -> Self {
        Self::TRANSPARENT
    }
}

/// Emits `#rrggbb` when fully opaque, `#rrggbbaa` otherwise, so a round trip is lossless.
impl fmt::Display for Rgba {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [r, g, b, a] = self.to_bytes();
        if a == 255 {
            write!(f, "#{r:02x}{g:02x}{b:02x}")
        } else {
            write!(f, "#{r:02x}{g:02x}{b:02x}{a:02x}")
        }
    }
}

impl Serialize for Rgba {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Rgba {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Rgba::parse_hex(&text).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseColorError {
    input: String,
}

impl fmt::Display for ParseColorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid colour {:?}, expected #rgb, #rgba, #rrggbb or #rrggbbaa", self.input)
    }
}

impl Error for ParseColorError {}

fn byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_accepted_length() {
        assert_eq!(Rgba::parse_hex("#fff").unwrap(), Rgba::new(1.0, 1.0, 1.0, 1.0));
        assert_eq!(Rgba::parse_hex("#ffff").unwrap(), Rgba::new(1.0, 1.0, 1.0, 1.0));
        assert_eq!(Rgba::parse_hex("#1e1e2e").unwrap(), Rgba::from_bytes([30, 30, 46, 255]));
        assert_eq!(Rgba::parse_hex("1e1e2e80").unwrap(), Rgba::from_bytes([30, 30, 46, 128]));
    }

    #[test]
    fn short_form_duplicates_each_nibble() {
        assert_eq!(Rgba::parse_hex("#abc").unwrap(), Rgba::parse_hex("#aabbcc").unwrap());
    }

    #[test]
    fn rejects_bad_input() {
        for bad in ["", "#", "#12", "#12345", "#gggggg", "not a colour"] {
            assert!(Rgba::parse_hex(bad).is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn text_form_round_trips() {
        for text in ["#1e1e2e", "#1e1e2e80"] {
            let parsed = Rgba::parse_hex(text).unwrap();
            assert_eq!(parsed.to_string(), text);
            assert_eq!(Rgba::parse_hex(&parsed.to_string()).unwrap(), parsed);
        }
    }
}
