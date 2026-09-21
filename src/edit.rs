//! Line operations, as pure text.
//!
//! The GTK side of these is a handful of buffer calls; the part worth testing
//! is the decision — which lines are affected, where the caret lands, whether
//! a comment is being added or removed. That is all here, operating on plain
//! strings, so it can be exercised without a display.

/// The lines a command applies to, given a selection.
///
/// A selection that ends exactly at the start of a line does not include that
/// line: the user dragged to the beginning of it, not into it. Getting this
/// wrong means commenting one line more than was highlighted, every time.
pub fn affected_lines(start_line: i32, end_line: i32, end_at_line_start: bool) -> (i32, i32) {
    let (first, last) = if start_line <= end_line {
        (start_line, end_line)
    } else {
        (end_line, start_line)
    };
    if end_at_line_start && last > first {
        (first, last - 1)
    } else {
        (first, last)
    }
}

/// Whether a comment command should comment or uncomment.
///
/// Uncomments only when *every* line is already commented. Mixed selections
/// comment the lot, which is what makes the command feel predictable: press
/// once and everything is commented, press again and nothing is.
pub fn should_uncomment(lines: &[&str], prefix: &str) -> bool {
    let mut saw_content = false;
    for line in lines {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        saw_content = true;
        if !trimmed.starts_with(prefix) {
            return false;
        }
    }
    saw_content
}

/// The column where a comment marker should go for a block of lines.
///
/// The shallowest indentation among them, so a commented block keeps its
/// shape instead of having markers scattered at each line's own indent.
pub fn comment_column(lines: &[&str]) -> usize {
    lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0)
}

/// Add a comment marker to a line at the given column.
pub fn comment_line(line: &str, prefix: &str, column: usize) -> String {
    if line.trim().is_empty() {
        return line.to_owned();
    }
    let column = column.min(line.len());
    let (indent, rest) = line.split_at(column);
    format!("{indent}{prefix} {rest}")
}

/// Remove a comment marker from a line, if it has one.
///
/// Also eats a single space after the marker, because that is what was added.
/// Anything else after it is the user's and stays.
pub fn uncomment_line(line: &str, prefix: &str) -> String {
    let indent_len = line.len() - line.trim_start().len();
    let (indent, rest) = line.split_at(indent_len);
    let Some(after) = rest.strip_prefix(prefix) else {
        return line.to_owned();
    };
    let after = after.strip_prefix(' ').unwrap_or(after);
    format!("{indent}{after}")
}

/// The line comment marker for a file, by extension.
///
/// Used only when the syntax engine has nothing to say — either because
/// highlighting is off, which is the default, or because the language is
/// unknown. Small on purpose: this is a fallback, not a language database.
pub fn comment_prefix_for(path: Option<&std::path::Path>) -> &'static str {
    let extension = path
        .and_then(|p| p.extension())
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match extension.as_str() {
        "rs" | "c" | "h" | "cpp" | "hpp" | "cc" | "js" | "ts" | "jsx" | "tsx" | "go" | "java"
        | "kt" | "swift" | "cs" | "php" | "scala" | "dart" | "zig" | "qml" | "proto" => "//",
        "lua" | "sql" | "hs" | "elm" | "ada" => "--",
        "vim" => "\"",
        "lisp" | "clj" | "el" | "scm" => ";",
        "tex" | "erl" => "%",
        "bat" | "cmd" => "REM",
        _ => "#",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn a_selection_ending_at_a_line_start_excludes_that_line() {
        // Dragging from line 2 to the very start of line 5 highlights 2..4.
        assert_eq!(affected_lines(2, 5, true), (2, 4));
        // Ending anywhere inside line 5 includes it.
        assert_eq!(affected_lines(2, 5, false), (2, 5));
    }

    #[test]
    fn a_backwards_selection_is_the_same_range() {
        assert_eq!(affected_lines(7, 3, false), (3, 7));
    }

    #[test]
    fn a_single_line_is_never_reduced_to_nothing() {
        assert_eq!(affected_lines(4, 4, true), (4, 4));
    }

    #[test]
    fn uncomments_only_when_everything_is_commented() {
        assert!(should_uncomment(&["# one", "# two"], "#"));
        assert!(!should_uncomment(&["# one", "two"], "#"));
        // Blank lines do not count either way.
        assert!(should_uncomment(&["# one", "", "# two"], "#"));
        // Nothing but blanks is not "all commented".
        assert!(!should_uncomment(&["", "   "], "#"));
    }

    #[test]
    fn markers_line_up_at_the_shallowest_indent() {
        let lines = ["    if x:", "        y()", "    return"];
        assert_eq!(comment_column(&lines), 4);
        assert_eq!(comment_line(lines[1], "#", 4), "    #     y()");
    }

    #[test]
    fn blank_lines_are_left_alone() {
        assert_eq!(comment_line("", "#", 0), "");
        assert_eq!(comment_line("   ", "#", 0), "   ");
    }

    #[test]
    fn commenting_and_uncommenting_round_trip() {
        for line in ["x = 1", "    indented", "no-space#inside"] {
            let column = comment_column(&[line]);
            let commented = comment_line(line, "#", column);
            assert_eq!(uncomment_line(&commented, "#"), line);
        }
    }

    #[test]
    fn uncommenting_keeps_extra_spacing_the_user_typed() {
        // One space after the marker is ours and is removed; the rest is
        // theirs and stays.
        assert_eq!(uncomment_line("#   spaced", "#"), "  spaced");
    }

    #[test]
    fn uncommenting_a_line_without_a_marker_changes_nothing() {
        assert_eq!(uncomment_line("plain", "#"), "plain");
    }

    #[test]
    fn comment_markers_follow_the_file_type() {
        assert_eq!(comment_prefix_for(Some(Path::new("a.rs"))), "//");
        assert_eq!(comment_prefix_for(Some(Path::new("a.lua"))), "--");
        assert_eq!(comment_prefix_for(Some(Path::new("a.py"))), "#");
        assert_eq!(comment_prefix_for(Some(Path::new("a.RS"))), "//");
        assert_eq!(comment_prefix_for(Some(Path::new("noext"))), "#");
        assert_eq!(comment_prefix_for(None), "#");
    }
}
