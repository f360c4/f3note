//! Searching across files.
//!
//! Two halves. Matching inside a piece of text is straightforward and lives
//! here so it can be tested without a display. Deciding *which* files to look
//! at is the part that goes wrong: a naive walk wanders into `.git`, spends a
//! minute reading `node_modules`, tries to match against a JPEG, and follows a
//! symlink back to where it started.
//!
//! f3note is a notepad, not a code search tool — `ripgrep` exists and is
//! better at this. The scope here is deliberately small: the files you have
//! open, and optionally the folder you are working in, bounded hard enough
//! that it always returns quickly.

use std::path::{Path, PathBuf};

/// Where a match was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// Zero-based, as the editor counts lines.
    pub line: i32,
    /// Character offset of the match within its line.
    pub column: i32,
    /// The whole line, for showing in the results.
    pub text: String,
}

/// Find every occurrence of `query` in `haystack`.
///
/// Overlapping matches are not reported twice: the search continues after the
/// end of each match, which is what a person scanning a page does.
pub fn find_in_text(haystack: &str, query: &str, case_sensitive: bool) -> Vec<Match> {
    if query.is_empty() {
        return Vec::new();
    }
    let needle = if case_sensitive {
        query.to_owned()
    } else {
        query.to_lowercase()
    };

    let mut found = Vec::new();
    for (index, line) in haystack.lines().enumerate() {
        let hay = if case_sensitive {
            line.to_owned()
        } else {
            line.to_lowercase()
        };
        // Lowercasing can change byte lengths, which would make byte offsets
        // from the folded string wrong for the original. Comparing by
        // characters keeps the column honest.
        let hay_chars: Vec<char> = hay.chars().collect();
        let needle_chars: Vec<char> = needle.chars().collect();
        let mut at = 0usize;
        while at + needle_chars.len() <= hay_chars.len() {
            if hay_chars[at..at + needle_chars.len()] == needle_chars[..] {
                found.push(Match {
                    line: index as i32,
                    column: at as i32,
                    text: line.to_owned(),
                });
                at += needle_chars.len();
            } else {
                at += 1;
            }
        }
    }
    found
}

/// Limits on a folder search, so it always finishes.
#[derive(Debug, Clone)]
pub struct Limits {
    pub max_files: usize,
    pub max_file_bytes: u64,
    pub max_depth: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_files: 2_000,
            max_file_bytes: 2 * 1024 * 1024,
            max_depth: 8,
        }
    }
}

/// Directories never worth walking into for a text search.
///
/// Not a gitignore implementation — that is a rabbit hole, and this is a
/// notepad. These are the handful that turn a fast search into a slow one on
/// almost every real project.
const SKIP_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    "target",
    "build",
    "dist",
    ".venv",
    "venv",
    "__pycache__",
    ".cache",
    ".next",
    "vendor",
];

pub fn should_skip_dir(name: &str) -> bool {
    SKIP_DIRS.contains(&name)
}

/// Whether a file's first bytes look like text.
///
/// A NUL byte is the giveaway, and it is what every other tool uses. Matching
/// a regular expression against a JPEG wastes time and can produce results
/// that are nonsense to display.
pub fn looks_like_text(head: &[u8]) -> bool {
    !head.contains(&0)
}

/// Collect the files worth searching under `root`.
///
/// Symlinks to directories are not followed, which is what stops a link
/// pointing at a parent from walking forever.
pub fn collect_files(root: &Path, limits: &Limits) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut queue = vec![(root.to_path_buf(), 0usize)];

    while let Some((dir, depth)) = queue.pop() {
        if depth > limits.max_depth || found.len() >= limits.max_files {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if found.len() >= limits.max_files {
                break;
            }
            let path = entry.path();
            // symlink_metadata rather than metadata: this must describe the
            // link itself, or a link to a parent directory loops.
            let Ok(meta) = entry.path().symlink_metadata() else {
                continue;
            };
            if meta.is_symlink() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if meta.is_dir() {
                if !should_skip_dir(&name) {
                    queue.push((path, depth + 1));
                }
            } else if meta.len() <= limits.max_file_bytes {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_every_occurrence_with_its_position() {
        let found = find_in_text("one two\nthree two four\n", "two", true);
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].line, found[0].column), (0, 4));
        assert_eq!((found[1].line, found[1].column), (1, 6));
        assert_eq!(found[1].text, "three two four");
    }

    #[test]
    fn case_insensitive_by_request() {
        assert_eq!(find_in_text("Hello hello", "hello", false).len(), 2);
        assert_eq!(find_in_text("Hello hello", "hello", true).len(), 1);
    }

    #[test]
    fn columns_are_characters_not_bytes() {
        // Each accented character is two bytes; the column must still be 3.
        let found = find_in_text("ááá needle", "needle", true);
        assert_eq!(found[0].column, 4);
    }

    #[test]
    fn repeated_text_does_not_produce_overlapping_matches() {
        // "aaaa" contains "aa" twice as a person would count it, not three
        // times as an overlapping scan would.
        let found = find_in_text("aaaa", "aa", true);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].column, 0);
        assert_eq!(found[1].column, 2);
    }

    #[test]
    fn an_empty_query_matches_nothing() {
        assert!(find_in_text("anything", "", true).is_empty());
    }

    #[test]
    fn noisy_directories_are_skipped() {
        assert!(should_skip_dir(".git"));
        assert!(should_skip_dir("node_modules"));
        assert!(should_skip_dir("target"));
        assert!(!should_skip_dir("src"));
        assert!(!should_skip_dir("gitlab"));
    }

    #[test]
    fn binary_content_is_recognised_by_a_nul_byte() {
        assert!(looks_like_text(b"plain text"));
        assert!(!looks_like_text(b"PNG\x00\x01"));
        assert!(looks_like_text("acentuação".as_bytes()));
    }

    #[test]
    fn walking_respects_every_limit() {
        let root = std::env::temp_dir().join(format!("f3note_search_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("node_modules")).unwrap();

        std::fs::write(root.join("a.txt"), b"x").unwrap();
        std::fs::write(root.join("src/b.txt"), b"x").unwrap();
        std::fs::write(root.join(".git/config"), b"x").unwrap();
        std::fs::write(root.join("node_modules/huge.js"), b"x").unwrap();
        std::fs::write(root.join("big.bin"), vec![0u8; 4096]).unwrap();

        let files = collect_files(
            &root,
            &Limits {
                max_file_bytes: 1024,
                ..Limits::default()
            },
        );
        let names: Vec<String> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();

        assert!(names.contains(&"a.txt".to_owned()));
        assert!(names.contains(&"b.txt".to_owned()));
        assert!(!names.contains(&"config".to_owned()), "should skip .git");
        assert!(
            !names.contains(&"huge.js".to_owned()),
            "should skip node_modules"
        );
        assert!(
            !names.contains(&"big.bin".to_owned()),
            "should respect the size limit"
        );

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_symlink_loop_does_not_hang_the_walk() {
        let root = std::env::temp_dir().join(format!("f3note_loop_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("inner")).unwrap();
        std::fs::write(root.join("inner/file.txt"), b"x").unwrap();
        // A link pointing back at the root would walk forever if followed.
        let _ = std::os::unix::fs::symlink(&root, root.join("inner/loop"));

        let files = collect_files(&root, &Limits::default());
        assert_eq!(files.len(), 1);
        std::fs::remove_dir_all(root).ok();
    }
}
