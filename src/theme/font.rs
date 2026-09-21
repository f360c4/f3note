//! Resolving which font the editor draws with.
//!
//! The family is deliberately left as the literal string `monospace` rather
//! than being resolved to a concrete family name at startup. That is not
//! laziness: fontconfig is the canonical source of truth on this kind of
//! system, and `omarchy font set` works by prepending the chosen family to the
//! `monospace` alias in `~/.config/fontconfig/fonts.conf`. Pango resolves the
//! alias itself, so binding to `monospace` means f3note follows the system font
//! automatically, for free, with no process spawn on the startup path.
//!
//! `omarchy font current` is itself just `fc-match monospace`, so this is the
//! same answer that command gives, obtained without running it.

use std::path::Path;

/// Point size used when nothing configures one. Omarchy's shell uses 12 as its
/// rem root, but that is a bar metric; an editor wants slightly more room.
const DEFAULT_SIZE: i32 = 11;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Font {
    pub family: String,
    pub size: i32,
}

impl Default for Font {
    fn default() -> Self {
        Font {
            family: "monospace".to_owned(),
            size: DEFAULT_SIZE,
        }
    }
}

impl Font {
    /// A Pango font description string, e.g. `monospace 11`.
    pub fn pango(&self) -> String {
        format!("{} {}", self.family, self.size)
    }

    /// The size Omarchy's `[font] base-size` asks for, if the user set one.
    ///
    /// Only the size is taken from here. The family stays on the fontconfig
    /// alias, matching Omarchy's own rule that "the family stays system-wide".
    pub fn size_from_omarchy_shell(path: &Path) -> Option<i32> {
        let text = std::fs::read_to_string(path).ok()?;
        let mut in_font_section = false;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_font_section = line == "[font]";
                continue;
            }
            if !in_font_section {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            if key.trim().trim_matches(['"', '\'']) != "base-size" {
                continue;
            }
            let n: i32 = value.trim().trim_matches(['"', '\'']).parse().ok()?;
            if n > 0 && n < 200 {
                return Some(n);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Tests run in parallel, so the name has to distinguish the caller as
    /// well as the process: two tests sharing one path overwrite each other.
    fn temp_with(name: &str, contents: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("f3note_font_{}_{}.toml", std::process::id(), name));
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        p
    }

    #[test]
    fn default_font_tracks_the_fontconfig_alias() {
        assert_eq!(Font::default().family, "monospace");
        assert_eq!(Font::default().pango(), "monospace 11");
    }

    #[test]
    fn reads_base_size_from_the_font_section_only() {
        let p = temp_with(
            "section",
            "[bar]\nbase-size = 99\n\n[font]\nbase-size = 14\n",
        );
        assert_eq!(Font::size_from_omarchy_shell(&p), Some(14));
        std::fs::remove_file(p).ok();
    }

    #[test]
    fn empty_font_section_yields_no_size() {
        let p = temp_with("empty", "[font]\n");
        assert_eq!(Font::size_from_omarchy_shell(&p), None);
        std::fs::remove_file(p).ok();
    }
}
