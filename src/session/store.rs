//! Per-document crash storage: the latest mirror, and the version history.
//!
//! Layout under the state directory:
//!
//! ```text
//! docs/<key>/mirror           latest buffer contents, verbatim
//! docs/<key>/meta.json        what the mirror is, and what it came from
//! docs/<key>/history/NNNN-<hash>.zst   older versions, newest last
//! ```
//!
//! The mirror is what crash recovery reads: one file, written atomically, that
//! always holds the most recent thing the user typed. The history is a bonus on
//! top — it is what makes it possible to go back to an earlier version even
//! after saving — and it is deliberately separate so that a problem with
//! history can never endanger recovery.
//!
//! Two rules keep the store from growing without bound. A snapshot is only
//! written when the content hash actually changed, so idle time costs nothing.
//! And history is capped per document, oldest first, because twenty versions of
//! a file is plenty and two thousand is a disk leak.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::atomic;

/// A content hash, hex-encoded. Truncated to 32 hex characters: this is
/// deduplication, not security, and 128 bits is far beyond what is needed to
/// never see a collision in one user's edit history.
pub type Hash = String;

pub fn hash_of(bytes: &[u8]) -> Hash {
    blake3::hash(bytes).to_hex()[..32].to_owned()
}

/// A stable directory name for a document.
///
/// File-backed documents key off their path so their history survives being
/// closed and reopened. Untitled documents get an opaque key that is recorded
/// in the session, because there is nothing else to identify them by.
pub fn key_for_path(path: &Path) -> String {
    format!("f-{}", hash_of(path.to_string_lossy().as_bytes()))
}

pub fn new_untitled_key() -> String {
    // Enough entropy to never collide within a session, without pulling in a
    // UUID dependency for something only this machine will ever read.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!(
        "u-{}",
        hash_of(format!("{}-{}", std::process::id(), nanos).as_bytes())
    )
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MirrorMeta {
    /// Hash of the mirror's contents, so an unchanged buffer is not rewritten.
    pub hash: Hash,
    /// Unix seconds when the mirror was written.
    pub written_at: u64,
    /// The highest history sequence number used so far.
    pub sequence: u64,
}

pub struct DocStore {
    dir: PathBuf,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl DocStore {
    pub fn new(root: &Path, key: &str) -> DocStore {
        DocStore {
            dir: root.join("docs").join(key),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn mirror_path(&self) -> PathBuf {
        self.dir.join("mirror")
    }

    fn meta_path(&self) -> PathBuf {
        self.dir.join("meta.json")
    }

    fn history_dir(&self) -> PathBuf {
        self.dir.join("history")
    }

    pub fn meta(&self) -> MirrorMeta {
        std::fs::read(self.meta_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn read_mirror(&self) -> Option<Vec<u8>> {
        std::fs::read(self.mirror_path()).ok()
    }

    pub fn has_mirror(&self) -> bool {
        self.mirror_path().exists()
    }

    /// Write the buffer contents as the current mirror.
    ///
    /// Returns `Ok(None)` when the contents were already there, which is the
    /// common case while someone is thinking rather than typing.
    pub fn write_mirror(&self, contents: &[u8]) -> io::Result<Option<Hash>> {
        let hash = hash_of(contents);
        let mut meta = self.meta();
        if meta.hash == hash && self.has_mirror() {
            return Ok(None);
        }

        std::fs::create_dir_all(&self.dir)?;
        // Contents first, then the metadata that describes them. A crash
        // between the two leaves a mirror whose recorded hash is stale, which
        // costs one redundant rewrite later — the opposite order would leave
        // metadata pointing at contents that were never written.
        atomic::write(&self.mirror_path(), contents)?;

        meta.hash = hash.clone();
        meta.written_at = now_secs();
        let encoded = serde_json::to_vec_pretty(&meta)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        atomic::write(&self.meta_path(), &encoded)?;

        Ok(Some(hash))
    }

    /// Add a compressed snapshot to the history, if this content is new.
    ///
    /// History failures are never fatal: the mirror is what recovery needs, and
    /// a full disk should not stop the editor from protecting the latest text.
    pub fn push_history(
        &self,
        contents: &[u8],
        keep: usize,
        max_total_bytes: u64,
    ) -> io::Result<bool> {
        let hash = hash_of(contents);
        let history = self.history_dir();

        // Nothing to do if the newest snapshot already holds this content.
        if let Some(latest) = self.history_entries()?.last() {
            if latest.hash == hash {
                return Ok(false);
            }
        }

        std::fs::create_dir_all(&history)?;
        let mut meta = self.meta();
        meta.sequence += 1;
        let name = format!("{:08}-{}.zst", meta.sequence, hash);

        let compressed = zstd::encode_all(contents, 3)?;
        atomic::write(&history.join(&name), &compressed)?;

        let encoded = serde_json::to_vec_pretty(&meta)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        atomic::write(&self.meta_path(), &encoded)?;

        self.gc(keep, max_total_bytes)?;
        Ok(true)
    }

    /// One entry in the version history.
    pub fn history_entries(&self) -> io::Result<Vec<HistoryEntry>> {
        let dir = self.history_dir();
        let Ok(read) = std::fs::read_dir(&dir) else {
            return Ok(Vec::new());
        };
        let mut entries: Vec<HistoryEntry> = read
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let stem = name.strip_suffix(".zst")?;
                let (seq, hash) = stem.split_once('-')?;
                let metadata = e.metadata().ok();
                Some(HistoryEntry {
                    sequence: seq.parse().ok()?,
                    hash: hash.to_owned(),
                    path: e.path(),
                    written_at: metadata.as_ref().and_then(|m| m.modified().ok()),
                    stored_bytes: metadata.as_ref().map(|m| m.len()).unwrap_or(0),
                })
            })
            .collect();
        entries.sort_by_key(|e| e.sequence);
        Ok(entries)
    }

    pub fn read_history(&self, entry: &HistoryEntry) -> io::Result<Vec<u8>> {
        let compressed = std::fs::read(&entry.path)?;
        zstd::decode_all(compressed.as_slice())
    }

    /// Drop the oldest snapshots beyond either limit.
    ///
    /// Two limits, because neither alone bounds anything useful. A count alone
    /// cannot bound disk use, since two hundred versions of a large file is
    /// not the same as two hundred of a small one. A size alone would throw
    /// away recent history on a big file while keeping thousands of versions
    /// of a tiny one. Whichever is reached first wins, and the newest are
    /// always what survive.
    ///
    /// Deleting is idempotent, so an interrupted collection simply resumes on
    /// the next pass: there is no state to repair.
    pub fn gc(&self, keep: usize, max_total_bytes: u64) -> io::Result<()> {
        let entries = self.history_entries()?;

        let mut drop_count = entries.len().saturating_sub(keep);

        // Walk from newest to oldest, accumulating size, and mark everything
        // past the ceiling for removal.
        let mut running = 0u64;
        for (index, entry) in entries.iter().enumerate().rev() {
            running += std::fs::metadata(&entry.path).map(|m| m.len()).unwrap_or(0);
            if running > max_total_bytes {
                drop_count = drop_count.max(index + 1);
                break;
            }
        }

        if drop_count == 0 {
            return Ok(());
        }
        for entry in &entries[..drop_count.min(entries.len())] {
            let _ = std::fs::remove_file(&entry.path);
        }
        let _ = atomic::sync_dir(&self.history_dir());
        Ok(())
    }

    /// Remove every trace of this document.
    ///
    /// This is what "close and forget" calls. An untitled tab where someone
    /// pasted a password must not survive in the state directory, and the only
    /// honest way to offer that is to actually delete it.
    pub fn forget(&self) -> io::Result<()> {
        if self.dir.exists() {
            std::fs::remove_dir_all(&self.dir)?;
            if let Some(parent) = self.dir.parent() {
                let _ = atomic::sync_dir(parent);
            }
        }
        Ok(())
    }

    /// Total bytes this document occupies, for the global size cap.
    pub fn size_on_disk(&self) -> u64 {
        fn walk(path: &Path) -> u64 {
            let Ok(read) = std::fs::read_dir(path) else {
                return 0;
            };
            read.filter_map(|e| e.ok())
                .map(|e| match e.metadata() {
                    Ok(m) if m.is_dir() => walk(&e.path()),
                    Ok(m) => m.len(),
                    Err(_) => 0,
                })
                .sum()
        }
        walk(&self.dir)
    }
}

#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub sequence: u64,
    pub hash: Hash,
    pub path: PathBuf,
    /// When the snapshot was written, from the file's own timestamp.
    ///
    /// Taken from the filesystem rather than recorded in the name, so a
    /// history directory copied or restored from a backup still describes
    /// itself honestly.
    pub written_at: Option<std::time::SystemTime>,
    /// Size on disk, compressed.
    pub stored_bytes: u64,
}

impl HistoryEntry {
    /// The stored size, in units that are not silly.
    ///
    /// A snapshot of a small note compresses to a few hundred bytes, and
    /// "0.0 KB" reads like an empty file rather than a small one.
    pub fn size(&self) -> String {
        match self.stored_bytes {
            0 => "empty".to_owned(),
            n if n < 1024 => format!("{n} bytes"),
            n if n < 1024 * 1024 => format!("{:.0} KB", n as f64 / 1024.0),
            n => format!("{:.1} MB", n as f64 / (1024.0 * 1024.0)),
        }
    }

    /// How long ago this was written, in words.
    ///
    /// Rough on purpose. "4 minutes ago" is what someone looking for the
    /// version from before they broke something needs; a timestamp to the
    /// second is noise they have to translate.
    pub fn age(&self, now: std::time::SystemTime) -> String {
        let Some(written) = self.written_at else {
            return "unknown".to_owned();
        };
        let Ok(elapsed) = now.duration_since(written) else {
            return "just now".to_owned();
        };
        let seconds = elapsed.as_secs();
        match seconds {
            0..=9 => "just now".to_owned(),
            10..=59 => format!("{seconds} seconds ago"),
            60..=119 => "a minute ago".to_owned(),
            120..=3599 => format!("{} minutes ago", seconds / 60),
            3600..=7199 => "an hour ago".to_owned(),
            7200..=86399 => format!("{} hours ago", seconds / 3600),
            86400..=172799 => "yesterday".to_owned(),
            _ => format!("{} days ago", seconds / 86400),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("f3note_store_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_path_always_produces_the_same_key() {
        let a = key_for_path(Path::new("/home/u/notes.txt"));
        let b = key_for_path(Path::new("/home/u/notes.txt"));
        assert_eq!(a, b);
        assert_ne!(a, key_for_path(Path::new("/home/u/other.txt")));
    }

    #[test]
    fn untitled_keys_are_unique() {
        let a = new_untitled_key();
        let b = new_untitled_key();
        assert_ne!(a, b);
    }

    #[test]
    fn the_mirror_round_trips() {
        let root = scratch("mirror");
        let s = DocStore::new(&root, "k");
        assert!(s.read_mirror().is_none());
        s.write_mirror(b"hello").unwrap();
        assert_eq!(s.read_mirror().unwrap(), b"hello");
        s.write_mirror(b"goodbye").unwrap();
        assert_eq!(s.read_mirror().unwrap(), b"goodbye");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn unchanged_content_is_not_rewritten() {
        let root = scratch("unchanged");
        let s = DocStore::new(&root, "k");
        assert!(s.write_mirror(b"same").unwrap().is_some());
        // The second call reports that there was nothing to do, which is what
        // makes an idle editor cost no disk writes at all.
        assert!(s.write_mirror(b"same").unwrap().is_none());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn history_snapshots_are_compressed_and_readable() {
        let root = scratch("history");
        let s = DocStore::new(&root, "k");
        let text = "repetitive text ".repeat(500);
        s.push_history(text.as_bytes(), 20, u64::MAX).unwrap();
        let entries = s.history_entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(s.read_history(&entries[0]).unwrap(), text.as_bytes());
        // Compression should be a real saving on text like this.
        let stored = std::fs::metadata(&entries[0].path).unwrap().len();
        assert!(stored < text.len() as u64 / 4, "stored {stored} bytes");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn identical_content_does_not_add_a_snapshot() {
        let root = scratch("dedup");
        let s = DocStore::new(&root, "k");
        assert!(s.push_history(b"one", 20, u64::MAX).unwrap());
        assert!(!s.push_history(b"one", 20, u64::MAX).unwrap());
        assert!(s.push_history(b"two", 20, u64::MAX).unwrap());
        assert_eq!(s.history_entries().unwrap().len(), 2);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn history_is_capped_oldest_first() {
        let root = scratch("cap");
        let s = DocStore::new(&root, "k");
        for i in 0..10 {
            s.push_history(format!("version {i}").as_bytes(), 3, u64::MAX)
                .unwrap();
        }
        let entries = s.history_entries().unwrap();
        assert_eq!(entries.len(), 3);
        // The survivors must be the newest three, in order.
        let text: Vec<String> = entries
            .iter()
            .map(|e| String::from_utf8(s.read_history(e).unwrap()).unwrap())
            .collect();
        assert_eq!(text, vec!["version 7", "version 8", "version 9"]);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn history_is_also_capped_by_total_size() {
        let root = scratch("sizecap");
        let s = DocStore::new(&root, "k");

        // Content that does not compress away, so the stored size is close to
        // the content size. A run of identical characters would shrink to
        // almost nothing under zstd and never reach any ceiling worth testing.
        let mut seed = 1u64;
        let mut noisy = |n: usize| -> Vec<u8> {
            (0..n)
                .map(|_| {
                    seed = seed
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    (seed >> 33) as u8
                })
                .collect()
        };

        // Each snapshot is distinct, so none are deduplicated away.
        for _ in 0..20 {
            s.push_history(&noisy(2000), 1000, 4096).unwrap();
        }
        let entries = s.history_entries().unwrap();
        assert!(
            entries.len() < 20,
            "the size ceiling should have collected something, kept {}",
            entries.len()
        );
        assert!(
            s.size_on_disk() < 64 * 1024,
            "kept {} bytes",
            s.size_on_disk()
        );
        // The survivors must be a contiguous run ending at the newest: the
        // collector drops from the old end, never out of the middle.
        let sequences: Vec<u64> = entries.iter().map(|e| e.sequence).collect();
        assert_eq!(*sequences.last().unwrap(), 20, "the newest must survive");
        assert!(
            sequences.windows(2).all(|w| w[1] == w[0] + 1),
            "gaps in the kept history: {sequences:?}"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn ages_are_described_in_words_people_use() {
        let now = std::time::SystemTime::now();
        let entry = |ago: u64| HistoryEntry {
            sequence: 1,
            hash: "x".into(),
            path: PathBuf::new(),
            written_at: Some(now - std::time::Duration::from_secs(ago)),
            stored_bytes: 0,
        };
        assert_eq!(entry(3).age(now), "just now");
        assert_eq!(entry(30).age(now), "30 seconds ago");
        assert_eq!(entry(90).age(now), "a minute ago");
        assert_eq!(entry(600).age(now), "10 minutes ago");
        assert_eq!(entry(5400).age(now), "an hour ago");
        assert_eq!(entry(10800).age(now), "3 hours ago");
        assert_eq!(entry(90000).age(now), "yesterday");
        assert_eq!(entry(300000).age(now), "3 days ago");
    }

    #[test]
    fn sizes_read_sensibly_at_every_scale() {
        let entry = |bytes: u64| HistoryEntry {
            sequence: 1,
            hash: "x".into(),
            path: PathBuf::new(),
            written_at: None,
            stored_bytes: bytes,
        };
        assert_eq!(entry(0).size(), "empty");
        // A small note compresses to a few hundred bytes; "0.0 KB" would read
        // like an empty file.
        assert_eq!(entry(312).size(), "312 bytes");
        assert_eq!(entry(4096).size(), "4 KB");
        assert_eq!(entry(3 * 1024 * 1024).size(), "3.0 MB");
    }

    #[test]
    fn an_entry_with_no_timestamp_says_so_rather_than_guessing() {
        let entry = HistoryEntry {
            sequence: 1,
            hash: "x".into(),
            path: PathBuf::new(),
            written_at: None,
            stored_bytes: 0,
        };
        assert_eq!(entry.age(std::time::SystemTime::now()), "unknown");
    }

    #[test]
    fn history_entries_carry_their_size_and_time() {
        let root = scratch("entrymeta");
        let s = DocStore::new(&root, "k");
        s.push_history(b"some content", 20, u64::MAX).unwrap();
        let entries = s.history_entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].stored_bytes > 0);
        assert!(entries[0].written_at.is_some());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn forgetting_removes_everything() {
        let root = scratch("forget");
        let s = DocStore::new(&root, "k");
        s.write_mirror(b"secret").unwrap();
        s.push_history(b"secret", 20, u64::MAX).unwrap();
        assert!(s.size_on_disk() > 0);
        s.forget().unwrap();
        assert!(!s.dir().exists());
        assert!(s.read_mirror().is_none());
        assert_eq!(s.size_on_disk(), 0);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_mirror_survives_being_read_back_after_reopening_the_store() {
        let root = scratch("reopen");
        DocStore::new(&root, "k")
            .write_mirror(b"persisted")
            .unwrap();
        let again = DocStore::new(&root, "k");
        assert_eq!(again.read_mirror().unwrap(), b"persisted");
        assert_eq!(again.meta().hash, hash_of(b"persisted"));
        std::fs::remove_dir_all(root).ok();
    }
}
