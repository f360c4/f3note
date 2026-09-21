//! User configuration: `~/.config/f3note/config.toml`.
//!
//! Every field has a working default, so the file is optional and f3note never
//! writes one uninvited. A malformed file is reported once and then ignored
//! rather than being fatal — losing your editor because a TOML comma slipped is
//! not an acceptable trade.

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// How the palette should be chosen.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemePreference {
    /// Follow the system: Omarchy's current theme if present, else the desktop
    /// light/dark preference.
    #[default]
    Auto,
    Dark,
    Light,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Appearance {
    /// Explicit Pango font description, e.g. `"JetBrainsMono Nerd Font 12"`.
    /// Leaving this unset is the better choice on a themed desktop: the editor
    /// then follows the system monospace font automatically.
    pub font: Option<String>,
    /// Window background alpha, 0.0..=1.0.
    ///
    /// Default is opaque. Transparency is opt-in because it is only useful
    /// under a compositor blur rule; without one, a translucent editor is just
    /// a harder-to-read editor. Omarchy in particular ships with blur disabled
    /// and applies its own window opacity compositor-side.
    pub opacity: f64,
    pub line_numbers: bool,
    pub wrap: bool,
    /// Off by default: f3note should open looking like a notepad, not an IDE.
    pub syntax_highlighting: bool,
    pub theme: ThemePreference,
    /// Highlight the line the caret is on.
    pub highlight_current_line: bool,
    /// Longest line, in characters, before the editor drops wrapping and
    /// highlighting and warns that navigation will stutter.
    ///
    /// Configurable because the right value depends on the machine: the
    /// default comes from measuring where moving the caret stops fitting in
    /// one frame on the reference hardware.
    pub long_line_chars: usize,
}

impl Default for Appearance {
    fn default() -> Self {
        Appearance {
            font: None,
            opacity: 1.0,
            line_numbers: true,
            wrap: true,
            syntax_highlighting: false,
            theme: ThemePreference::Auto,
            highlight_current_line: false,
            long_line_chars: crate::text::LONG_LINE_CHARS,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Editor {
    pub tab_width: u32,
    pub insert_spaces: bool,
    /// Seconds of idle typing before the buffer is mirrored to disk.
    pub autosave_idle_seconds: u64,
    /// Hard ceiling between mirrors while typing continuously.
    pub autosave_max_seconds: u64,
    /// Above this many bytes, snapshots stop and only the latest mirror is
    /// kept. Version history on a large file costs more disk than it is worth.
    pub history_max_bytes: u64,
    /// Versions retained per document before the oldest are collected.
    ///
    /// The old default of 20 was far too small to be useful. Autosave writes
    /// a snapshot every few seconds while someone works, so twenty versions
    /// covered barely two minutes — worthless for recovering an accidental
    /// replace-all noticed half an hour later, which is precisely what
    /// history is for.
    pub history_versions: usize,
    /// Ceiling on the total bytes of history kept per document.
    ///
    /// Paired with the count because the count alone cannot bound disk use:
    /// two hundred versions of a small file is nothing, and two hundred of a
    /// large one is not. Whichever limit is reached first wins.
    pub history_max_total_bytes: u64,
}

impl Default for Editor {
    fn default() -> Self {
        Editor {
            tab_width: 4,
            insert_spaces: false,
            autosave_idle_seconds: 2,
            autosave_max_seconds: 7,
            history_max_bytes: 8 * 1024 * 1024,
            history_versions: 200,
            history_max_total_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub appearance: Appearance,
    pub editor: Editor,
}

impl Config {
    pub fn dir() -> PathBuf {
        crate::paths::config_dir()
    }

    pub fn path() -> PathBuf {
        Self::dir().join("config.toml")
    }

    /// Load the config, or the defaults. The second return value is a message
    /// worth showing the user when their file could not be used.
    pub fn load_from(path: &Path) -> (Config, Option<String>) {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Config::default(), None),
            Err(e) => return (Config::default(), Some(format!("{}: {e}", path.display()))),
        };
        match toml::from_str::<Config>(&text) {
            Ok(mut c) => {
                c.appearance.opacity = c.appearance.opacity.clamp(0.1, 1.0);
                // No ceiling: someone who raises this has decided to live with
                // the stutter, and that is their call. The floor stops a typo
                // from flagging every ordinary file.
                c.appearance.long_line_chars = c.appearance.long_line_chars.max(200);
                c.editor.tab_width = c.editor.tab_width.clamp(1, 16);
                c.editor.autosave_idle_seconds = c.editor.autosave_idle_seconds.clamp(1, 600);
                c.editor.autosave_max_seconds = c
                    .editor
                    .autosave_max_seconds
                    .clamp(c.editor.autosave_idle_seconds, 3600);
                (c, None)
            }
            Err(e) => (Config::default(), Some(format!("{}: {e}", path.display()))),
        }
    }

    pub fn load() -> (Config, Option<String>) {
        Self::load_from(&Self::path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp(name: &str, contents: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("f3note_cfg_{}_{}.toml", std::process::id(), name));
        std::fs::File::create(&p)
            .unwrap()
            .write_all(contents.as_bytes())
            .unwrap();
        p
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        let (c, err) = Config::load_from(Path::new("/nonexistent/f3note/config.toml"));
        assert!(err.is_none());
        assert_eq!(c.appearance.opacity, 1.0);
        assert!(!c.appearance.syntax_highlighting);
    }

    #[test]
    fn partial_config_keeps_defaults_for_the_rest() {
        let p = temp("partial", "[appearance]\nopacity = 0.9\n");
        let (c, err) = Config::load_from(&p);
        assert!(err.is_none(), "{err:?}");
        assert_eq!(c.appearance.opacity, 0.9);
        assert!(c.appearance.line_numbers);
        assert_eq!(c.editor.tab_width, 4);
        std::fs::remove_file(p).ok();
    }

    #[test]
    fn malformed_config_falls_back_instead_of_failing() {
        let p = temp("bad", "[appearance\nopacity = ");
        let (c, err) = Config::load_from(&p);
        assert!(err.is_some(), "a broken file should be reported");
        assert_eq!(c.appearance.opacity, 1.0);
        std::fs::remove_file(p).ok();
    }

    #[test]
    fn absurd_values_are_clamped_rather_than_obeyed() {
        let p = temp(
            "clamp",
            "[appearance]\nopacity = 5.0\n[editor]\ntab_width = 900\nautosave_idle_seconds = 0\n",
        );
        let (c, _) = Config::load_from(&p);
        assert_eq!(c.appearance.opacity, 1.0);
        assert_eq!(c.editor.tab_width, 16);
        assert_eq!(c.editor.autosave_idle_seconds, 1);
        std::fs::remove_file(p).ok();
    }

    #[test]
    fn autosave_ceiling_can_never_fall_below_the_idle_delay() {
        let p = temp(
            "order",
            "[editor]\nautosave_idle_seconds = 30\nautosave_max_seconds = 5\n",
        );
        let (c, _) = Config::load_from(&p);
        assert!(c.editor.autosave_max_seconds >= c.editor.autosave_idle_seconds);
        std::fs::remove_file(p).ok();
    }
}
