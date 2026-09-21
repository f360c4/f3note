//! The session file: which tabs were open, and where to find their contents.
//!
//! Written atomically, and always kept in two copies. The second copy is not
//! belt and braces — the session file is rewritten constantly while someone
//! works, so it is the file most likely to be caught mid-update by a power cut.
//! If the primary will not parse, the previous good copy is one file away, and
//! losing the last few seconds of tab bookkeeping is a far better outcome than
//! starting up with no idea what was open.
//!
//! Recovery is also forgiving by design. A document whose mirror has gone
//! missing is restored from its file instead of being dropped, and an entry
//! that cannot be understood at all is skipped rather than failing the whole
//! restore. A session file is a convenience; it must never be the reason the
//! editor will not start.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::atomic;
use crate::text::LineEnding;

/// Bumped when the on-disk shape changes incompatibly. An older or newer
/// version is ignored rather than guessed at.
pub const VERSION: u32 = 1;

fn line_ending_name(e: LineEnding) -> &'static str {
    match e {
        LineEnding::Lf => "lf",
        LineEnding::CrLf => "crlf",
        LineEnding::Cr => "cr",
    }
}

fn line_ending_from(name: &str) -> LineEnding {
    match name {
        "crlf" => LineEnding::CrLf,
        "cr" => LineEnding::Cr,
        _ => LineEnding::Lf,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    /// Directory name under `docs/` holding this document's mirror.
    pub key: String,
    /// Absent for a document that has never been saved.
    #[serde(default)]
    pub path: Option<PathBuf>,
    #[serde(default)]
    pub untitled_number: u32,
    #[serde(default)]
    pub cursor: i32,
    /// True when the mirror differs from what is on disk — that is, when this
    /// tab had unsaved changes.
    #[serde(default)]
    pub modified: bool,
    #[serde(default = "default_encoding")]
    pub encoding: String,
    #[serde(default)]
    pub line_ending: String,
    #[serde(default)]
    pub had_bom: bool,
    /// Size and modification time of the file when f3note last read or wrote
    /// it. Used to notice that something else changed it while we were closed.
    #[serde(default)]
    pub disk_size: Option<u64>,
    #[serde(default)]
    pub disk_mtime_secs: Option<u64>,
}

fn default_encoding() -> String {
    "UTF-8".to_owned()
}

impl Entry {
    pub fn line_ending(&self) -> LineEnding {
        line_ending_from(&self.line_ending)
    }

    pub fn set_line_ending(&mut self, e: LineEnding) {
        self.line_ending = line_ending_name(e).to_owned();
    }

    /// A tab worth restoring: either it points at a file, or it has unsaved
    /// contents of its own. An untitled tab that was never typed into is not
    /// worth bringing back.
    pub fn is_worth_restoring(&self) -> bool {
        self.path.is_some() || self.modified
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub version: u32,
    #[serde(default)]
    pub documents: Vec<Entry>,
    /// Index into `documents` of the tab that was in front.
    #[serde(default)]
    pub active: usize,
    #[serde(default)]
    pub next_untitled: u32,
}

impl Default for Session {
    fn default() -> Self {
        Session {
            version: VERSION,
            documents: Vec::new(),
            active: 0,
            next_untitled: 1,
        }
    }
}

fn primary(root: &Path) -> PathBuf {
    root.join("session.json")
}

/// Where a named session lives.
///
/// The name is reduced to something that is safe as a file name. A session
/// called "../../etc/passwd" is a session called "etc-passwd", not a path
/// traversal, and one called "work: 2026" keeps its meaning without keeping
/// characters that break on some filesystem somewhere.
pub fn sanitise_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let collapsed = cleaned
        .split(['-', ' '])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let trimmed = collapsed.trim_matches('-');
    if trimmed.is_empty() {
        "session".to_owned()
    } else {
        trimmed.chars().take(64).collect()
    }
}

fn named(root: &Path, name: &str) -> PathBuf {
    root.join("sessions")
        .join(format!("{}.json", sanitise_name(name)))
}

/// Every named session, alphabetically.
pub fn list_named(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root.join("sessions")) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.strip_suffix(".json").map(str::to_owned)
        })
        .collect();
    names.sort();
    names
}

fn backup(root: &Path) -> PathBuf {
    root.join("session.json.bak")
}

impl Session {
    /// Read the session, preferring the primary copy and falling back to the
    /// backup. Returns the defaults if neither can be used.
    /// Save this set of tabs under a name, to come back to later.
    pub fn save_named(&self, root: &Path, name: &str) -> std::io::Result<()> {
        let path = named(root, name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let encoded = serde_json::to_vec_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        atomic::write(&path, &encoded)
    }

    /// Read back a named session, if it is there and usable.
    pub fn load_named(root: &Path, name: &str) -> Option<Session> {
        let bytes = std::fs::read(named(root, name)).ok()?;
        let session: Session = serde_json::from_slice(&bytes).ok()?;
        (session.version == VERSION).then(|| session.sanitised())
    }

    pub fn delete_named(root: &Path, name: &str) -> std::io::Result<()> {
        std::fs::remove_file(named(root, name))
    }

    pub fn load(root: &Path) -> Session {
        for path in [primary(root), backup(root)] {
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            match serde_json::from_slice::<Session>(&bytes) {
                Ok(s) if s.version == VERSION => return s.sanitised(),
                Ok(s) => {
                    eprintln!(
                        "f3note: ignoring {} written by a different version ({} rather than {VERSION})",
                        path.display(),
                        s.version
                    );
                }
                Err(e) => eprintln!("f3note: {} is unusable ({e})", path.display()),
            }
        }
        Session::default()
    }

    /// Drop entries that cannot mean anything, and make the active index point
    /// at something that exists.
    fn sanitised(mut self) -> Session {
        self.documents.retain(|e| !e.key.is_empty());
        if self.documents.is_empty() {
            self.active = 0;
        } else if self.active >= self.documents.len() {
            self.active = self.documents.len() - 1;
        }
        self.next_untitled = self.next_untitled.max(1);
        self
    }

    /// Write the session, keeping the previous copy as the backup.
    ///
    /// The order matters: the current primary is copied to the backup *before*
    /// the primary is replaced, so at every instant at least one of the two
    /// files is a complete, parseable session.
    pub fn save(&self, root: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(root)?;
        let encoded = serde_json::to_vec_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        if let Ok(previous) = std::fs::read(primary(root)) {
            // A failed backup is not worth refusing to save over: the new
            // primary is still written atomically.
            let _ = atomic::write(&backup(root), &previous);
        }
        atomic::write(&primary(root), &encoded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("f3note_sess_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn entry(key: &str, path: Option<&str>) -> Entry {
        Entry {
            key: key.to_owned(),
            path: path.map(PathBuf::from),
            untitled_number: 1,
            cursor: 0,
            modified: false,
            encoding: "UTF-8".to_owned(),
            line_ending: "lf".to_owned(),
            had_bom: false,
            disk_size: None,
            disk_mtime_secs: None,
        }
    }

    #[test]
    fn round_trips_a_session() {
        let root = scratch("roundtrip");
        let mut s = Session::default();
        s.documents.push(entry("k1", Some("/tmp/a.txt")));
        s.documents.push(entry("k2", None));
        s.active = 1;
        s.next_untitled = 4;
        s.save(&root).unwrap();

        let loaded = Session::load(&root);
        assert_eq!(loaded.documents.len(), 2);
        assert_eq!(loaded.active, 1);
        assert_eq!(loaded.next_untitled, 4);
        assert_eq!(loaded.documents[0].path, Some(PathBuf::from("/tmp/a.txt")));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_corrupt_primary_falls_back_to_the_backup() {
        let root = scratch("fallback");
        let mut good = Session::default();
        good.documents.push(entry("k1", Some("/tmp/kept.txt")));
        good.save(&root).unwrap();

        // Second save moves the good copy to the backup.
        let mut newer = Session::default();
        newer.documents.push(entry("k2", Some("/tmp/newer.txt")));
        newer.save(&root).unwrap();

        // Now destroy the primary, as a power cut mid-write would.
        std::fs::write(root.join("session.json"), b"{ truncated").unwrap();

        let loaded = Session::load(&root);
        assert_eq!(loaded.documents.len(), 1);
        assert_eq!(
            loaded.documents[0].path,
            Some(PathBuf::from("/tmp/kept.txt")),
            "should have recovered the previous good session"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn both_copies_unusable_yields_an_empty_session_not_an_error() {
        let root = scratch("both-bad");
        std::fs::write(root.join("session.json"), b"garbage").unwrap();
        std::fs::write(root.join("session.json.bak"), b"also garbage").unwrap();
        let loaded = Session::load(&root);
        assert!(loaded.documents.is_empty());
        assert_eq!(loaded.next_untitled, 1);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn no_session_at_all_is_not_an_error() {
        let root = scratch("absent");
        let loaded = Session::load(&root);
        assert!(loaded.documents.is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_session_from_another_version_is_ignored() {
        let root = scratch("version");
        let raw = format!(
            r#"{{"version": {}, "documents": [], "active": 0}}"#,
            VERSION + 7
        );
        std::fs::write(root.join("session.json"), raw).unwrap();
        let loaded = Session::load(&root);
        assert_eq!(loaded.version, VERSION);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn an_out_of_range_active_index_is_brought_back_in_range() {
        let root = scratch("active");
        let mut s = Session::default();
        s.documents.push(entry("k1", Some("/tmp/a.txt")));
        s.active = 99;
        s.save(&root).unwrap();
        assert_eq!(Session::load(&root).active, 0);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn entries_without_a_key_are_dropped() {
        let root = scratch("nokey");
        let mut s = Session::default();
        s.documents.push(entry("", Some("/tmp/a.txt")));
        s.documents.push(entry("k2", Some("/tmp/b.txt")));
        s.save(&root).unwrap();
        let loaded = Session::load(&root);
        assert_eq!(loaded.documents.len(), 1);
        assert_eq!(loaded.documents[0].key, "k2");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn session_names_become_safe_file_names() {
        assert_eq!(sanitise_name("work"), "work");
        assert_eq!(sanitise_name("work: 2026"), "work-2026");
        assert_eq!(sanitise_name("my config files"), "my-config-files");
        // No traversal, whatever the user types.
        assert_eq!(sanitise_name("../../etc/passwd"), "etc-passwd");
        assert_eq!(sanitise_name("/"), "session");
        assert_eq!(sanitise_name(""), "session");
        assert!(sanitise_name(&"x".repeat(200)).len() <= 64);
    }

    #[test]
    fn named_sessions_round_trip_and_list() {
        let root = scratch("named");
        let mut s = Session::default();
        s.documents.push(entry("k1", Some("/tmp/work.txt")));
        s.save_named(&root, "work").unwrap();

        let mut other = Session::default();
        other.documents.push(entry("k2", Some("/tmp/config.txt")));
        other.save_named(&root, "config").unwrap();

        assert_eq!(list_named(&root), vec!["config", "work"]);

        let loaded = Session::load_named(&root, "work").unwrap();
        assert_eq!(loaded.documents.len(), 1);
        assert_eq!(
            loaded.documents[0].path,
            Some(PathBuf::from("/tmp/work.txt"))
        );

        Session::delete_named(&root, "work").unwrap();
        assert_eq!(list_named(&root), vec!["config"]);
        assert!(Session::load_named(&root, "work").is_none());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_named_session_that_is_not_there_is_none_rather_than_an_error() {
        let root = scratch("nonamed");
        assert!(Session::load_named(&root, "absent").is_none());
        assert!(list_named(&root).is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn an_untouched_untitled_tab_is_not_worth_restoring() {
        let clean = entry("k", None);
        assert!(!clean.is_worth_restoring());

        let mut typed_in = entry("k", None);
        typed_in.modified = true;
        assert!(typed_in.is_worth_restoring());

        assert!(entry("k", Some("/tmp/a.txt")).is_worth_restoring());
    }

    #[test]
    fn missing_fields_take_sensible_defaults() {
        let root = scratch("partial");
        let raw = format!(r#"{{"version": {VERSION}, "documents": [{{"key": "k1"}}]}}"#);
        std::fs::write(root.join("session.json"), raw).unwrap();
        let loaded = Session::load(&root);
        assert_eq!(loaded.documents.len(), 1);
        assert_eq!(loaded.documents[0].encoding, "UTF-8");
        assert_eq!(loaded.documents[0].line_ending(), LineEnding::Lf);
        std::fs::remove_dir_all(root).ok();
    }
}
