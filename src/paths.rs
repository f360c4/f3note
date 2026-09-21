//! Where f3note keeps things.
//!
//! Follows the XDG base directory spec, with the split that matters for this
//! editor: configuration the user writes lives under `XDG_CONFIG_HOME`, while
//! everything f3note generates and must survive a crash — the session index and
//! the backup store — lives under `XDG_STATE_HOME`. State is not cache: it must
//! not be on a volume anyone considers disposable.

use std::path::PathBuf;

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

fn xdg(var: &str, default_suffix: &str) -> PathBuf {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => home().join(default_suffix),
    }
}

pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("f3note")
}

/// Session index, buffer mirrors and the version store.
pub fn state_dir() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state").join("f3note")
}

/// Generated GtkSourceView style schemes. Regenerated from the palette, so it
/// is genuinely disposable.
pub fn data_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share").join("f3note")
}

/// Runtime sockets. `XDG_RUNTIME_DIR` is tmpfs owned by the user and cleared on
/// logout, which is exactly right for a single-instance lock: a socket left
/// behind by a killed process cannot outlive the session.
pub fn runtime_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        // Falling back to the state directory keeps single-instance working on
        // systems without XDG_RUNTIME_DIR, at the cost of needing stale-socket
        // detection there. See ipc.rs.
        _ => state_dir(),
    }
}

/// Omarchy's live theme palette, if this system has one.
pub fn omarchy_colors() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state").join("omarchy/current/theme/colors.toml")
}

/// The file Omarchy rewrites when the theme changes. Watching this as well as
/// the palette catches theme switches that replace the whole theme directory,
/// where a watch on the palette alone would be left pointing at a stale inode.
pub fn omarchy_theme_name() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state").join("omarchy/current/theme.name")
}

pub fn omarchy_shell_toml() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("omarchy/shell.toml")
}

/// The user's own palette, used when Omarchy is not present.
pub fn user_theme() -> PathBuf {
    config_dir().join("theme.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn honours_xdg_overrides() {
        // Set together and read together: these tests mutate process-wide
        // environment, so they must not be split into separate test functions
        // that could interleave.
        std::env::set_var("XDG_CONFIG_HOME", "/tmp/xdgcfg");
        std::env::set_var("XDG_STATE_HOME", "/tmp/xdgstate");
        assert_eq!(config_dir(), PathBuf::from("/tmp/xdgcfg/f3note"));
        assert_eq!(state_dir(), PathBuf::from("/tmp/xdgstate/f3note"));
        assert_eq!(
            omarchy_colors(),
            PathBuf::from("/tmp/xdgstate/omarchy/current/theme/colors.toml")
        );
        std::env::remove_var("XDG_CONFIG_HOME");
        std::env::remove_var("XDG_STATE_HOME");
    }

    #[test]
    fn empty_xdg_variables_fall_back_to_home_defaults() {
        std::env::set_var("XDG_DATA_HOME", "");
        let d = data_dir();
        std::env::remove_var("XDG_DATA_HOME");
        assert!(d.ends_with(".local/share/f3note"), "{d:?}");
    }
}
