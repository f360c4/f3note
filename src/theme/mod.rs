//! Resolving, generating and installing the editor's appearance.
//!
//! The palette comes from a cascade, checked in order:
//!
//! 1. Omarchy's live theme (`~/.local/state/omarchy/current/theme/colors.toml`)
//! 2. The user's own palette (`~/.config/f3note/theme.toml`)
//! 3. A built-in palette matching the desktop's light/dark preference
//!
//! Nothing in f3note requires Omarchy — it is simply first in line when it is
//! there, so that on a themed desktop the editor arrives already wearing the
//! right colors with no configuration at all. On any other distribution the
//! second and third steps cover it.
//!
//! Appearance is also live. A change to any file in the cascade re-resolves and
//! re-installs the stylesheet on the running window, so `omarchy theme set
//! gruvbox` recolors the editor without restarting it or disturbing open tabs.

pub mod css;
pub mod font;
pub mod palette;
pub mod scheme;

pub use font::Font;
pub use palette::{Mode, Palette};

use crate::config::{Config, ThemePreference};
use crate::paths;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

/// Which file the palette came from. Reported in the status bar so a user who
/// wonders why the colors look like that can find out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Omarchy(PathBuf),
    User(PathBuf),
    BuiltIn(Mode),
}

impl Source {
    pub fn describe(&self) -> String {
        match self {
            Source::Omarchy(_) => "omarchy".to_owned(),
            Source::User(_) => "user".to_owned(),
            Source::BuiltIn(Mode::Dark) => "built-in dark".to_owned(),
            Source::BuiltIn(Mode::Light) => "built-in light".to_owned(),
        }
    }
}

pub struct Theme {
    pub palette: Palette,
    pub font: Font,
    pub opacity: f64,
    pub source: Source,
}

impl Theme {
    /// Work through the cascade and build the appearance to install.
    ///
    /// `prefers_dark` is passed in rather than read here so this stays callable
    /// without a display connection, which is what makes it testable.
    pub fn resolve(config: &Config, prefers_dark: bool) -> Theme {
        let (palette, source) = Self::resolve_palette(config, prefers_dark);

        let mut f = Font::default();
        if let Some(size) = Font::size_from_omarchy_shell(&paths::omarchy_shell_toml()) {
            f.size = size;
        }
        // An explicit setting outranks everything, including the system font.
        if let Some(spec) = config.appearance.font.as_deref() {
            if let Some(parsed) = parse_font_spec(spec) {
                f = parsed;
            }
        }

        Theme {
            palette,
            font: f,
            opacity: config.appearance.opacity,
            source,
        }
    }

    fn resolve_palette(config: &Config, prefers_dark: bool) -> (Palette, Source) {
        match config.appearance.theme {
            // An explicit dark/light choice means "stop following the system",
            // so the cascade is skipped entirely rather than being reordered.
            ThemePreference::Dark => {
                return (Palette::fallback(Mode::Dark), Source::BuiltIn(Mode::Dark))
            }
            ThemePreference::Light => {
                return (Palette::fallback(Mode::Light), Source::BuiltIn(Mode::Light))
            }
            ThemePreference::Auto => {}
        }

        let omarchy = paths::omarchy_colors();
        if let Some(p) = Palette::load(&omarchy) {
            return (p, Source::Omarchy(omarchy));
        }
        let user = paths::user_theme();
        if let Some(p) = Palette::load(&user) {
            return (p, Source::User(user));
        }
        let mode = if prefers_dark {
            Mode::Dark
        } else {
            Mode::Light
        };
        (Palette::fallback(mode), Source::BuiltIn(mode))
    }

    pub fn stylesheet(&self) -> String {
        css::stylesheet(&css::Appearance {
            palette: &self.palette,
            font: &self.font,
            opacity: self.opacity,
        })
    }
}

/// Parse a Pango-style description such as `"JetBrainsMono Nerd Font 12"`.
fn parse_font_spec(spec: &str) -> Option<Font> {
    let spec = spec.trim();
    if spec.is_empty() {
        return None;
    }
    match spec.rsplit_once(' ') {
        Some((family, size)) => match size.parse::<i32>() {
            Ok(n) if n > 0 && n < 200 => Some(Font {
                family: family.trim().to_owned(),
                size: n,
            }),
            // No trailing number: the whole string is a family name.
            _ => Some(Font {
                family: spec.to_owned(),
                ..Font::default()
            }),
        },
        None => Some(Font {
            family: spec.to_owned(),
            ..Font::default()
        }),
    }
}

/// Writes the generated style scheme to disk and makes GtkSourceView aware of
/// it. Returns the scheme id if it could be registered.
///
/// The file is only rewritten when its contents actually change. On a normal
/// start it already matches, so this costs one read instead of a write, and the
/// user's disk is not touched every time the editor opens.
fn install_scheme(palette: &Palette) -> Option<&'static str> {
    let dir = paths::data_dir().join("styles");
    let path = dir.join(format!("{}.xml", scheme::SCHEME_ID));
    let wanted = scheme::generate(palette);

    let current = std::fs::read_to_string(&path).ok();
    let rewritten = current.as_deref() != Some(wanted.as_str());
    if rewritten {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            eprintln!("f3note: cannot create {}: {e}", dir.display());
            return None;
        }
        if let Err(e) = crate::atomic::write(&path, wanted.as_bytes()) {
            eprintln!("f3note: cannot write {}: {e}", path.display());
            return None;
        }
    }

    let manager = sourceview5::StyleSchemeManager::default();
    let dir_str = dir.to_string_lossy().into_owned();
    let known = manager.search_path().iter().any(|p| p.as_str() == dir_str);
    if !known {
        manager.append_search_path(&dir_str);
    }

    // force_rescan walks every style-scheme directory the manager knows about,
    // including the system ones, and was measured costing hundreds of
    // milliseconds. On a normal start the scheme on disk already matches the
    // palette, so this must not run: it is only needed when the file actually
    // changed under a path that has already been scanned.
    if rewritten && known {
        manager.force_rescan();
    }
    Some(scheme::SCHEME_ID)
}

/// Owns the live appearance: the installed stylesheet, the file watches that
/// notice a theme change, and the callbacks that want to know about one.
pub struct ThemeEngine {
    provider: gtk::CssProvider,
    theme: RefCell<Rc<Theme>>,
    config: RefCell<Config>,
    listeners: RefCell<Vec<Box<dyn Fn(&Theme)>>>,
    #[allow(dead_code)] // held only to keep the watches alive
    monitors: RefCell<Vec<gio::FileMonitor>>,
}

impl ThemeEngine {
    pub fn new(config: Config) -> Rc<ThemeEngine> {
        let provider = gtk::CssProvider::new();

        // A stylesheet f3note generated should never fail to parse. If one
        // does, that is a bug in the generator rather than in the user's theme,
        // and it needs to be loud: on GTK before 4.22 malformed CSS is not
        // merely ignored. The real protection is upstream of here — palette
        // values are re-parsed as colors instead of being pasted into the
        // template as text — but this catches template mistakes immediately.
        provider.connect_parsing_error(|_, section, error| {
            eprintln!(
                "f3note: generated stylesheet failed to parse at {}: {error}",
                section.to_str()
            );
            debug_assert!(false, "f3note generated invalid CSS: {error}");
        });

        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }

        let prefers_dark = system_prefers_dark();
        let theme = Rc::new(Theme::resolve(&config, prefers_dark));

        let engine = Rc::new(ThemeEngine {
            provider,
            theme: RefCell::new(theme),
            config: RefCell::new(config),
            listeners: RefCell::new(Vec::new()),
            monitors: RefCell::new(Vec::new()),
        });

        engine.apply();
        engine.clone().watch();
        engine
    }

    pub fn theme(&self) -> Rc<Theme> {
        self.theme.borrow().clone()
    }

    pub fn config(&self) -> Config {
        self.config.borrow().clone()
    }

    /// Register a callback to run whenever the appearance changes. It is called
    /// immediately with the current theme, so callers do not need a separate
    /// path for initial setup.
    pub fn on_change<F: Fn(&Theme) + 'static>(&self, f: F) {
        f(&self.theme.borrow());
        self.listeners.borrow_mut().push(Box::new(f));
    }

    fn apply(&self) {
        let theme = self.theme.borrow().clone();
        self.provider.load_from_string(&theme.stylesheet());
        install_scheme(&theme.palette);
        for listener in self.listeners.borrow().iter() {
            listener(&theme);
        }
    }

    /// Re-read everything and reinstall. Cheap enough to call on any file event.
    pub fn reload(&self) {
        let (config, err) = Config::load();
        if let Some(e) = err {
            eprintln!("f3note: {e}");
        }
        let theme = Rc::new(Theme::resolve(&config, system_prefers_dark()));
        *self.config.borrow_mut() = config;
        *self.theme.borrow_mut() = theme;
        self.apply();
    }

    fn watch(self: Rc<Self>) {
        // theme.name is watched as well as colors.toml because switching themes
        // can replace the whole theme directory. A watch on the palette alone
        // would then be left pointing at an inode nobody writes to again.
        let targets = [
            paths::omarchy_theme_name(),
            paths::omarchy_colors(),
            paths::user_theme(),
            Config::path(),
            paths::omarchy_shell_toml(),
        ];

        let mut monitors = Vec::new();
        for path in targets {
            let file = gio::File::for_path(&path);
            let monitor =
                match file.monitor(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE) {
                    Ok(m) => m,
                    // A path that does not exist yet is not an error: the user may
                    // create ~/.config/f3note/theme.toml later, and GIO watches the
                    // name rather than the inode.
                    Err(_) => continue,
                };
            let engine = self.clone();
            monitor.connect_changed(move |_, _, _, event| {
                use gio::FileMonitorEvent::*;
                if matches!(
                    event,
                    ChangesDoneHint | Created | Deleted | MovedIn | MovedOut | Renamed
                ) {
                    // Coalesce: a theme switch rewrites several of these files
                    // in quick succession, and reloading once at the end is
                    // both cheaper and free of intermediate flicker.
                    let engine = engine.clone();
                    glib::timeout_add_local_once(std::time::Duration::from_millis(60), move || {
                        engine.reload();
                    });
                }
            });
            monitors.push(monitor);
        }
        *self.monitors.borrow_mut() = monitors;
    }
}

fn system_prefers_dark() -> bool {
    gtk::Settings::default()
        .map(|s| s.is_gtk_application_prefer_dark_theme())
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_dark_preference_ignores_the_system_cascade() {
        let mut c = Config::default();
        c.appearance.theme = ThemePreference::Dark;
        let (p, source) = Theme::resolve_palette(&c, false);
        assert_eq!(p.mode, Mode::Dark);
        assert_eq!(source, Source::BuiltIn(Mode::Dark));
    }

    #[test]
    fn auto_falls_back_to_the_desktop_preference() {
        // Point the cascade at directories that cannot contain a palette, so
        // only the built-in step can answer.
        std::env::set_var("XDG_STATE_HOME", "/nonexistent-f3note-state");
        std::env::set_var("XDG_CONFIG_HOME", "/nonexistent-f3note-config");
        let c = Config::default();
        assert_eq!(
            Theme::resolve_palette(&c, true).1,
            Source::BuiltIn(Mode::Dark)
        );
        assert_eq!(
            Theme::resolve_palette(&c, false).1,
            Source::BuiltIn(Mode::Light)
        );
        std::env::remove_var("XDG_STATE_HOME");
        std::env::remove_var("XDG_CONFIG_HOME");
    }

    #[test]
    fn parses_pango_font_descriptions() {
        let f = parse_font_spec("JetBrainsMono Nerd Font 12").unwrap();
        assert_eq!(f.family, "JetBrainsMono Nerd Font");
        assert_eq!(f.size, 12);
    }

    #[test]
    fn a_family_without_a_size_keeps_the_default_size() {
        let f = parse_font_spec("Iosevka").unwrap();
        assert_eq!(f.family, "Iosevka");
        assert_eq!(f.size, Font::default().size);
    }

    #[test]
    fn a_family_ending_in_a_word_is_not_mistaken_for_a_size() {
        let f = parse_font_spec("Fira Code").unwrap();
        assert_eq!(f.family, "Fira Code");
        assert_eq!(f.size, Font::default().size);
    }
}
