//! Parsing and resolution of an Omarchy-style `colors.toml` palette.
//!
//! The file is read with a lenient line parser rather than a TOML crate, on
//! purpose. Themes are third-party content: users install them from anywhere,
//! and Omarchy's own `omarchy-theme-color` reads them line by line, accepting
//! things a strict TOML parser would reject. Matching that behaviour means a
//! theme that works everywhere else in the user's desktop also works here,
//! instead of f3note being the one application that refuses it.
//!
//! The alias and fallback cascade below mirrors `omarchy-theme-color` so that
//! f3note resolves the exact same palette as every other themed application on
//! the system. A theme that only defines ANSI `color0..color15`, or the older
//! short names (`bg`, `fg`), still produces a complete palette here.

use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Dark,
    Light,
}

impl Mode {
    pub fn is_dark(self) -> bool {
        self == Mode::Dark
    }
}

#[derive(Debug, Clone)]
pub struct Palette {
    colors: HashMap<String, String>,
    pub mode: Mode,
}

/// Characters accepted in a value. Palettes carry more than hex: `rgb()` and
/// `rgba()` lists, gradient angles like `-45deg`, decimals and bare words all
/// appear in real themes. Anything outside this set is dropped rather than
/// passed through, because these values end up in generated CSS.
fn value_is_safe(value: &str) -> bool {
    value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "#(),._+/% -".contains(c))
}

fn key_is_safe(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Blend two `#rrggbb` colors. `amount` is 0.0..=1.0, where 0 is all `start`.
fn mix(start: &str, end: &str, amount: f32) -> Option<String> {
    let parse = |s: &str| -> Option<[u8; 3]> {
        let h = s.strip_prefix('#')?;
        if h.len() != 6 {
            return None;
        }
        Some([
            u8::from_str_radix(&h[0..2], 16).ok()?,
            u8::from_str_radix(&h[2..4], 16).ok()?,
            u8::from_str_radix(&h[4..6], 16).ok()?,
        ])
    };
    let (a, b) = (parse(start)?, parse(end)?);
    let t = amount.clamp(0.0, 1.0);
    let ch = |i: usize| (a[i] as f32 * (1.0 - t) + b[i] as f32 * t).round() as u8;
    Some(format!("#{:02x}{:02x}{:02x}", ch(0), ch(1), ch(2)))
}

/// Relative luminance test used only to guess dark/light when a theme omits
/// `mode`. Deliberately the same crude sum-of-channels test Omarchy uses, so a
/// borderline theme is classified identically in both.
fn looks_light(hex: &str) -> Option<bool> {
    let h = hex.strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let sum: u32 = (0..3)
        .filter_map(|i| u32::from_str_radix(&h[i * 2..i * 2 + 2], 16).ok())
        .sum();
    Some(sum > 382)
}

impl Palette {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.colors.get(key).map(String::as_str)
    }

    /// Look up `key`, falling back to `fallback` and then to a hardcoded
    /// literal. Used by CSS generation so a missing key can never produce an
    /// empty CSS value.
    pub fn get_or(&self, key: &str, fallback: &str) -> String {
        self.get(key).unwrap_or(fallback).to_owned()
    }

    pub fn parse(text: &str) -> Palette {
        let mut colors: HashMap<String, String> = HashMap::new();

        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
                continue;
            }
            let Some((raw_key, raw_value)) = line.split_once('=') else {
                continue;
            };
            let key: String = raw_key
                .chars()
                .filter(|c| !matches!(c, '"' | '\'' | ' ' | '\t'))
                .collect();
            if !key_is_safe(&key) {
                continue;
            }

            // Quoted values win: taking what is between the quotes also strips
            // trailing inline comments, which unquoted values do not have.
            let value = match raw_value.find(['"', '\'']) {
                Some(start) => {
                    let rest = &raw_value[start + 1..];
                    match rest.find(['"', '\'']) {
                        Some(end) => rest[..end].to_owned(),
                        None => continue,
                    }
                }
                None => raw_value.trim().to_owned(),
            };
            if value.is_empty() || !value_is_safe(&value) {
                continue;
            }
            colors.insert(key, value);
        }

        Self::resolve(colors)
    }

    pub fn load(path: &Path) -> Option<Palette> {
        std::fs::read_to_string(path).ok().map(|t| Self::parse(&t))
    }

    /// A neutral palette used when nothing else resolves. Not a theme anyone
    /// would choose, but it guarantees f3note always has a complete, readable
    /// set of colors rather than an empty stylesheet.
    pub fn fallback(mode: Mode) -> Palette {
        let src = if mode.is_dark() {
            include_str!("fallback_dark.toml")
        } else {
            include_str!("fallback_light.toml")
        };
        Self::parse(src)
    }

    fn resolve(mut c: HashMap<String, String>) -> Palette {
        // Alias helper: only fills `key` when it is absent and `from` exists.
        fn alias(c: &mut HashMap<String, String>, key: &str, from: &str) {
            if !c.contains_key(key) {
                if let Some(v) = c.get(from).cloned() {
                    c.insert(key.to_owned(), v);
                }
            }
        }

        // Accept the legacy short-name palette before anything else, so a theme
        // written against the old names is complete by the time the ANSI
        // fallbacks and derived shades run.
        const SHORT: [(&str, &str); 8] = [
            ("background", "bg"),
            ("dark_background", "dark_bg"),
            ("darker_background", "darker_bg"),
            ("lighter_background", "lighter_bg"),
            ("foreground", "fg"),
            ("dark_foreground", "dark_fg"),
            ("light_foreground", "light_fg"),
            ("bright_foreground", "bright_fg"),
        ];
        for (canonical, short) in SHORT {
            alias(&mut c, canonical, short);
        }

        // Themes predating the semantic palette may only define ANSI names.
        alias(&mut c, "background", "color0");
        alias(&mut c, "foreground", "color7");
        if let Some(v) = c.get("background").cloned() {
            c.insert("color0".into(), v);
        }
        if let Some(v) = c.get("foreground").cloned() {
            c.insert("color7".into(), v);
        }

        const ANSI: [(&str, &str); 12] = [
            ("red", "color1"),
            ("green", "color2"),
            ("yellow", "color3"),
            ("blue", "color4"),
            ("magenta", "color5"),
            ("cyan", "color6"),
            ("bright_red", "color9"),
            ("bright_green", "color10"),
            ("bright_yellow", "color11"),
            ("bright_blue", "color12"),
            ("bright_magenta", "color13"),
            ("bright_cyan", "color14"),
        ];
        for (name, ansi) in ANSI {
            alias(&mut c, name, ansi);
        }
        alias(&mut c, "magenta", "purple");
        alias(&mut c, "bright_magenta", "bright_purple");

        // Everything below has to end up defined, so each step falls through a
        // chain that bottoms out in a key guaranteed by the steps above.
        let bg = c.get("background").cloned().unwrap_or("#1e1e1e".into());
        let fg = c.get("foreground").cloned().unwrap_or("#d0d0d0".into());
        c.entry("background".into()).or_insert_with(|| bg.clone());
        c.entry("foreground".into()).or_insert_with(|| fg.clone());

        alias(&mut c, "light_foreground", "color7");
        c.entry("light_foreground".into())
            .or_insert_with(|| fg.clone());
        alias(&mut c, "bright_foreground", "color15");
        c.entry("bright_foreground".into())
            .or_insert_with(|| fg.clone());
        let bright_fg = c["bright_foreground"].clone();
        c.insert("cursor".into(), bright_fg);

        alias(&mut c, "lighter_background", "color0");
        c.entry("lighter_background".into())
            .or_insert_with(|| bg.clone());
        alias(&mut c, "dark_foreground", "color8");
        c.entry("dark_foreground".into())
            .or_insert_with(|| fg.clone());
        alias(&mut c, "muted", "color8");
        let dark_fg = c["dark_foreground"].clone();
        c.entry("muted".into()).or_insert(dark_fg);

        alias(&mut c, "selection", "selection_background");
        alias(&mut c, "selection", "color8");
        alias(&mut c, "selection", "color0");
        c.entry("selection".into()).or_insert_with(|| bg.clone());
        let selection = c["selection"].clone();
        c.entry("selection_background".into()).or_insert(selection);
        let bright_fg = c["bright_foreground"].clone();
        c.entry("selection_foreground".into()).or_insert(bright_fg);

        alias(&mut c, "orange", "yellow");
        if !c.contains_key("brown") {
            if let Some(m) = c.get("orange").and_then(|o| mix(o, "#000000", 0.5)) {
                c.insert("brown".into(), m);
            }
        }

        // Derived shades, only when the theme did not supply them.
        for (key, base, toward, amount) in [
            ("dark_background", "background", "#000000", 0.25),
            ("darker_background", "background", "#000000", 0.50),
            ("bright_red", "red", "#ffffff", 0.20),
            ("bright_yellow", "yellow", "#ffffff", 0.20),
            ("bright_green", "green", "#ffffff", 0.20),
            ("bright_cyan", "cyan", "#ffffff", 0.20),
            ("bright_blue", "blue", "#ffffff", 0.20),
            ("bright_magenta", "magenta", "#ffffff", 0.20),
        ] {
            if !c.contains_key(key) {
                if let Some(m) = c.get(base).and_then(|b| mix(b, toward, amount)) {
                    c.insert(key.to_owned(), m);
                }
            }
        }
        alias(&mut c, "purple", "magenta");
        alias(&mut c, "bright_purple", "bright_magenta");
        alias(&mut c, "accent", "blue");
        alias(&mut c, "accent", "bright_foreground");

        // Map back to ANSI names for anything that still queries them.
        const BACK: [(&str, &str); 16] = [
            ("color0", "background"),
            ("color1", "red"),
            ("color2", "green"),
            ("color3", "yellow"),
            ("color4", "blue"),
            ("color5", "magenta"),
            ("color6", "cyan"),
            ("color7", "foreground"),
            ("color8", "muted"),
            ("color9", "bright_red"),
            ("color10", "bright_green"),
            ("color11", "bright_yellow"),
            ("color12", "bright_blue"),
            ("color13", "bright_magenta"),
            ("color14", "bright_cyan"),
            ("color15", "bright_foreground"),
        ];
        for (ansi, name) in BACK {
            alias(&mut c, ansi, name);
        }
        for (canonical, short) in SHORT {
            if let Some(v) = c.get(canonical).cloned() {
                c.insert(short.to_owned(), v);
            }
        }

        let mode = match c.get("mode").or_else(|| c.get("theme_type")) {
            Some(m) if m.eq_ignore_ascii_case("light") => Mode::Light,
            Some(_) => Mode::Dark,
            None => match c.get("background").and_then(|b| looks_light(b)) {
                Some(true) => Mode::Light,
                _ => Mode::Dark,
            },
        };

        Palette { colors: c, mode }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_real_omarchy_palette() {
        let p = Palette::parse(
            r##"
mode = "dark"
accent = "#7aa2f7"
background = "#1a1b26"
foreground = "#a9b1d6"
"##,
        );
        assert_eq!(p.mode, Mode::Dark);
        assert_eq!(p.get("accent"), Some("#7aa2f7"));
        // Derived because the theme did not define it.
        assert_eq!(p.get("darker_background"), Some("#0d0e13"));
    }

    #[test]
    fn accepts_a_legacy_ansi_only_theme() {
        let p = Palette::parse(
            r##"
color0 = "#000000"
color7 = "#ffffff"
color1 = "#ff0000"
"##,
        );
        assert_eq!(p.get("background"), Some("#000000"));
        assert_eq!(p.get("foreground"), Some("#ffffff"));
        assert_eq!(p.get("red"), Some("#ff0000"));
        // No `mode` key, so luminance decides: black background is dark.
        assert_eq!(p.mode, Mode::Dark);
    }

    #[test]
    fn guesses_light_mode_from_a_pale_background() {
        let p = Palette::parse(r##"background = "#fafafa""##);
        assert_eq!(p.mode, Mode::Light);
    }

    #[test]
    fn rejects_values_that_could_escape_into_css() {
        let p = Palette::parse(
            r##"
background = "#111111"
accent = "red; } * { background: url(http://evil)"
"##,
        );
        assert_eq!(p.get("background"), Some("#111111"));
        // The hostile value is dropped, so `accent` falls back through the
        // alias chain instead of reaching the stylesheet.
        assert_ne!(
            p.get("accent"),
            Some("red; } * { background: url(http://evil)")
        );
    }

    #[test]
    fn mixes_channels_correctly() {
        assert_eq!(mix("#000000", "#ffffff", 0.5).as_deref(), Some("#808080"));
        assert_eq!(mix("#ff0000", "#0000ff", 1.0).as_deref(), Some("#0000ff"));
    }
}
