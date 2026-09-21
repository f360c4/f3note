//! Recently opened files.
//!
//! A plain list of paths, newest first, in the state directory. Deliberately
//! not a database: it is a convenience, it must survive being edited by hand,
//! and a corrupt entry should cost one line rather than the list.

use std::path::{Path, PathBuf};

/// How many paths to remember. Enough to cover "the thing I had open
/// yesterday", short enough that the list stays scannable.
const LIMIT: usize = 50;

pub struct Recent {
    path: PathBuf,
}

impl Recent {
    pub fn new(state_dir: &Path) -> Recent {
        Recent {
            path: state_dir.join("recent"),
        }
    }

    pub fn load(&self) -> Vec<PathBuf> {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return Vec::new();
        };
        Self::parse(&text)
    }

    fn parse(text: &str) -> Vec<PathBuf> {
        let mut seen = std::collections::HashSet::new();
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(PathBuf::from)
            // A path can appear twice if the file was written oddly; keep the
            // first, which is the most recent.
            .filter(|path| seen.insert(path.clone()))
            .take(LIMIT)
            .collect()
    }

    fn render(paths: &[PathBuf]) -> String {
        paths
            .iter()
            .filter(|p| !p.to_string_lossy().contains('\n'))
            .take(LIMIT)
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    /// Move a path to the front of the list.
    pub fn record(&self, path: &Path) {
        let mut paths = self.load();
        paths.retain(|p| p != path);
        paths.insert(0, path.to_path_buf());
        paths.truncate(LIMIT);

        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Best effort: failing to remember a recent file is not worth
        // interrupting anyone over.
        let _ = crate::atomic::write(&self.path, Self::render(&paths).as_bytes());
    }

    /// The list, with files that no longer exist dropped.
    ///
    /// Checked at read time rather than pruned on write, so a file on a drive
    /// that happens to be unmounted comes back when it is mounted again
    /// instead of being forgotten.
    pub fn existing(&self) -> Vec<PathBuf> {
        self.load().into_iter().filter(|p| p.exists()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("f3note_recent_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_most_recent_is_first() {
        let d = scratch("order");
        let r = Recent::new(&d);
        r.record(Path::new("/tmp/a.txt"));
        r.record(Path::new("/tmp/b.txt"));
        assert_eq!(
            r.load(),
            vec![PathBuf::from("/tmp/b.txt"), PathBuf::from("/tmp/a.txt")]
        );
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn reopening_moves_a_path_up_without_duplicating_it() {
        let d = scratch("dedup");
        let r = Recent::new(&d);
        r.record(Path::new("/tmp/a.txt"));
        r.record(Path::new("/tmp/b.txt"));
        r.record(Path::new("/tmp/a.txt"));
        assert_eq!(
            r.load(),
            vec![PathBuf::from("/tmp/a.txt"), PathBuf::from("/tmp/b.txt")]
        );
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn the_list_is_capped() {
        let d = scratch("cap");
        let r = Recent::new(&d);
        for i in 0..(LIMIT + 20) {
            r.record(&PathBuf::from(format!("/tmp/file{i}.txt")));
        }
        let loaded = r.load();
        assert_eq!(loaded.len(), LIMIT);
        // The newest survive.
        assert_eq!(
            loaded[0],
            PathBuf::from(format!("/tmp/file{}.txt", LIMIT + 19))
        );
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn a_missing_list_is_empty_rather_than_an_error() {
        let d = scratch("absent");
        assert!(Recent::new(&d).load().is_empty());
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn blank_lines_and_stray_whitespace_are_ignored() {
        let parsed = Recent::parse("/tmp/a.txt\n\n   \n  /tmp/b.txt  \n");
        assert_eq!(
            parsed,
            vec![PathBuf::from("/tmp/a.txt"), PathBuf::from("/tmp/b.txt")]
        );
    }

    #[test]
    fn duplicates_in_a_hand_edited_file_keep_the_first() {
        let parsed = Recent::parse("/tmp/a.txt\n/tmp/b.txt\n/tmp/a.txt\n");
        assert_eq!(
            parsed,
            vec![PathBuf::from("/tmp/a.txt"), PathBuf::from("/tmp/b.txt")]
        );
    }

    #[test]
    fn files_that_no_longer_exist_are_left_out_of_the_offered_list() {
        let d = scratch("existing");
        let real = d.join("here.txt");
        std::fs::write(&real, b"x").unwrap();
        let r = Recent::new(&d);
        r.record(Path::new("/tmp/definitely-not-here-12345.txt"));
        r.record(&real);
        assert_eq!(r.existing(), vec![real.clone()]);
        // But nothing is deleted: the full list still holds both.
        assert_eq!(r.load().len(), 2);
        std::fs::remove_dir_all(d).ok();
    }
}
