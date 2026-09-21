//! Reading a file into something the editor can hold.
//!
//! This does the work GtkSourceView's own loader does not do well enough.
//! `GtkSourceFileLoader` guesses an encoding by trying a fixed candidate list —
//! UTF-8, the locale's encoding, ISO-8859-15, UTF-16 — and keeping the first
//! that converts without error. Every single-byte encoding converts without
//! error, so a Windows-1252 file is silently decoded as ISO-8859-15 and the
//! user gets mojibake with no indication anything went wrong. A real detector
//! is used here instead.
//!
//! The same pass also measures what the editor needs to protect itself.
//! GtkTextView lays out each line as one PangoLayout, so a single very long
//! line is pathological regardless of how small the file is: a 117 KB minified
//! stylesheet is enough to lock the widget up. Line length is therefore checked
//! alongside file size, and both are reported so the caller can say *which*
//! limit was hit rather than showing one vague message.

use std::path::Path;

/// Longest line, in characters, before the editor drops wrapping and
/// highlighting and says so.
///
/// Chosen from measurement rather than taste. GtkTextView builds one
/// PangoLayout per logical line, entire, however little of it is on screen, so
/// finding the caret's x position means shaping the whole line. Cost of moving
/// the caret along one line, measured on the reference machine:
///
/// ```text
///    500 chars    2 ms per move
///  1 000 chars    3 ms
///  2 000 chars    5 ms
///  4 000 chars    9 ms
///  8 000 chars   18 ms
/// 16 000 chars   20 ms
/// 36 000 chars   44 ms
/// ```
///
/// A frame at 60Hz is 16ms, so the stutter becomes visible somewhere around
/// 8 000. The limit sits below that, at the last point where moving the caret
/// is comfortably within one frame.
///
/// Note this is about line *length*, not file size or line count: 200 000
/// lines and 11 MB open in 370ms and navigate in 8ms, because only the visible
/// lines are laid out. It is a single enormous line that has no defence.
pub const LONG_LINE_CHARS: usize = 5_000;

/// File size, in bytes, before wrapping and highlighting are dropped.
pub const BIG_FILE_BYTES: u64 = 50 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
    Cr,
}

impl LineEnding {
    pub fn label(self) -> &'static str {
        match self {
            LineEnding::Lf => "LF",
            LineEnding::CrLf => "CRLF",
            LineEnding::Cr => "CR",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
            LineEnding::Cr => "\r",
        }
    }
}

/// Why the editor put a document into reduced-capability mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LargeFileReason {
    /// At least one line is long enough to make layout pathological.
    LongLines { longest: usize },
    /// The file as a whole is big enough that highlighting it is not worth it.
    BigFile { bytes: u64 },
}

impl LargeFileReason {
    /// The message shown in the banner.
    ///
    /// It says which limit was hit, because "large file" on a 36 KB stylesheet
    /// would only confuse. And for long lines it promises nothing it cannot
    /// deliver: turning off highlighting and wrapping roughly halves the cost,
    /// but moving the caret along a 36 000-character line still takes tens of
    /// milliseconds and there is no setting that changes that. Saying
    /// "highlighting and wrapping are off" implied the problem was handled.
    /// It is not, and the user is better served by knowing.
    pub fn message(self) -> String {
        match self {
            LargeFileReason::LongLines { longest } => format!(
                "Lines up to {longest} characters — moving the cursor in this \
                 file will stutter. Highlighting and wrapping are off to help."
            ),
            LargeFileReason::BigFile { bytes } => format!(
                "Large file ({:.0} MB) — highlighting and wrapping are off",
                bytes as f64 / (1024.0 * 1024.0)
            ),
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            LargeFileReason::LongLines { .. } => "long lines",
            LargeFileReason::BigFile { .. } => "large file",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LoadedText {
    /// Contents with line endings normalised to `\n`, which is what
    /// GtkTextBuffer stores. The original ending is remembered separately and
    /// restored on save.
    pub text: String,
    pub encoding: &'static str,
    pub had_bom: bool,
    pub line_ending: LineEnding,
    pub longest_line: usize,
    pub byte_len: u64,
    /// Set when the document must open with reduced capabilities.
    pub large_file: Option<LargeFileReason>,
    /// True when the bytes could not be decoded cleanly and replacement
    /// characters were substituted. Saving such a document risks destroying
    /// data, so the caller should warn before it does.
    pub had_errors: bool,
}

/// Inspect a byte-order mark. Returns the encoding and the length of the mark.
fn sniff_bom(bytes: &[u8]) -> Option<(&'static encoding_rs::Encoding, usize)> {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        Some((encoding_rs::UTF_8, 3))
    } else if bytes.starts_with(&[0xFF, 0xFE]) {
        Some((encoding_rs::UTF_16LE, 2))
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        Some((encoding_rs::UTF_16BE, 2))
    } else {
        None
    }
}

fn detect_encoding(bytes: &[u8]) -> (&'static encoding_rs::Encoding, bool, usize) {
    if let Some((enc, len)) = sniff_bom(bytes) {
        return (enc, true, len);
    }
    // Valid UTF-8 is taken as UTF-8 without consulting the detector. A
    // statistical guess can mistake short UTF-8 text for a legacy encoding, and
    // on a modern system being wrong about UTF-8 is the worst possible answer.
    if std::str::from_utf8(bytes).is_ok() {
        return (encoding_rs::UTF_8, false, 0);
    }
    let mut detector = chardetng::EncodingDetector::new();
    detector.feed(bytes, true);
    (detector.guess(None, true), false, 0)
}

/// Find the dominant line ending and the longest line, in one pass over the
/// decoded text.
fn scan(text: &str) -> (LineEnding, usize) {
    let (mut lf, mut crlf, mut cr) = (0usize, 0usize, 0usize);
    let mut longest = 0usize;
    let mut current = 0usize;
    let mut prev_was_cr = false;

    for ch in text.chars() {
        match ch {
            '\n' => {
                if prev_was_cr {
                    crlf += 1;
                    // The CR was counted as a bare CR a moment ago; undo that.
                    cr -= 1;
                } else {
                    lf += 1;
                }
                longest = longest.max(current);
                current = 0;
                prev_was_cr = false;
            }
            '\r' => {
                cr += 1;
                if prev_was_cr {
                    // Two CRs in a row: the first ended a line on its own.
                    longest = longest.max(current);
                    current = 0;
                } else {
                    longest = longest.max(current);
                    current = 0;
                }
                prev_was_cr = true;
            }
            _ => {
                current += 1;
                prev_was_cr = false;
            }
        }
    }
    longest = longest.max(current);

    // Ties go to LF: it is the right default on this platform, and a file with
    // no line breaks at all should not be reported as CRLF.
    let ending = if crlf > lf && crlf >= cr {
        LineEnding::CrLf
    } else if cr > lf && cr > crlf {
        LineEnding::Cr
    } else {
        LineEnding::Lf
    };
    (ending, longest)
}

/// Normalise any mixture of line endings to `\n`.
fn normalise(text: &str) -> String {
    if !text.contains('\r') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            // Consume the LF of a CRLF pair so it does not produce a blank line.
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

pub fn decode(bytes: &[u8]) -> LoadedText {
    decode_with_limit(bytes, LONG_LINE_CHARS)
}

/// Decode with an explicit long-line limit, so the user's configured value is
/// what decides rather than the compiled-in default.
pub fn decode_with_limit(bytes: &[u8], long_line_chars: usize) -> LoadedText {
    let byte_len = bytes.len() as u64;
    let (encoding, had_bom, bom_len) = detect_encoding(bytes);
    let (decoded, _, had_errors) = encoding.decode(&bytes[bom_len.min(bytes.len())..]);

    let (line_ending, longest_line) = scan(&decoded);
    let text = normalise(&decoded);

    // Line length is checked first: it is the failure mode that surprises
    // people, because it fires on files that are not big at all.
    let large_file = if longest_line > long_line_chars {
        Some(LargeFileReason::LongLines {
            longest: longest_line,
        })
    } else if byte_len > BIG_FILE_BYTES {
        Some(LargeFileReason::BigFile { bytes: byte_len })
    } else {
        None
    };

    LoadedText {
        text,
        encoding: encoding.name(),
        had_bom,
        line_ending,
        longest_line,
        byte_len,
        large_file,
        had_errors,
    }
}

pub fn load(path: &Path, long_line_chars: usize) -> std::io::Result<LoadedText> {
    Ok(decode_with_limit(&std::fs::read(path)?, long_line_chars))
}

/// Turn editor contents back into bytes for saving, restoring the document's
/// original line ending and byte-order mark.
pub fn encode(text: &str, line_ending: LineEnding, encoding_name: &str, bom: bool) -> Vec<u8> {
    let bodied = match line_ending {
        LineEnding::Lf => text.to_owned(),
        LineEnding::CrLf => text.replace('\n', "\r\n"),
        LineEnding::Cr => text.replace('\n', "\r"),
    };
    let encoding =
        encoding_rs::Encoding::for_label(encoding_name.as_bytes()).unwrap_or(encoding_rs::UTF_8);
    let (bytes, _, _) = encoding.encode(&bodied);

    let mut out = Vec::with_capacity(bytes.len() + 3);
    if bom {
        match encoding.name() {
            "UTF-8" => out.extend_from_slice(&[0xEF, 0xBB, 0xBF]),
            "UTF-16LE" => out.extend_from_slice(&[0xFF, 0xFE]),
            "UTF-16BE" => out.extend_from_slice(&[0xFE, 0xFF]),
            _ => {}
        }
    }
    out.extend_from_slice(&bytes);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_utf8_is_recognised_without_guessing() {
        let t = decode("olá, mundo\n".as_bytes());
        assert_eq!(t.encoding, "UTF-8");
        assert!(!t.had_bom);
        assert!(!t.had_errors);
        assert_eq!(t.text, "olá, mundo\n");
    }

    #[test]
    fn a_utf8_bom_is_detected_and_stripped() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice("texto".as_bytes());
        let t = decode(&bytes);
        assert_eq!(t.encoding, "UTF-8");
        assert!(t.had_bom);
        assert_eq!(t.text, "texto");
    }

    #[test]
    fn windows_1252_is_not_silently_mistaken_for_iso_8859_15() {
        // This is the case GtkSourceFileLoader gets wrong. 0x93/0x94 are curly
        // quotes in CP1252 and undefined control characters in ISO-8859-15.
        let bytes = b"He said \x93hello\x94 and left\n";
        let t = decode(bytes);
        assert_ne!(t.encoding, "ISO-8859-15");
        assert!(
            t.text.contains('\u{201C}') || t.text.contains('\u{201D}'),
            "expected curly quotes, got {:?}",
            t.text
        );
    }

    #[test]
    fn detects_crlf_and_normalises_it_for_the_buffer() {
        let t = decode(b"one\r\ntwo\r\nthree");
        assert_eq!(t.line_ending, LineEnding::CrLf);
        assert_eq!(t.text, "one\ntwo\nthree");
        assert!(!t.text.contains('\r'));
    }

    #[test]
    fn detects_classic_mac_line_endings() {
        let t = decode(b"one\rtwo\rthree");
        assert_eq!(t.line_ending, LineEnding::Cr);
        assert_eq!(t.text, "one\ntwo\nthree");
    }

    #[test]
    fn a_file_with_no_line_breaks_reports_lf() {
        let t = decode(b"just one line");
        assert_eq!(t.line_ending, LineEnding::Lf);
    }

    #[test]
    fn mixed_endings_pick_the_dominant_one() {
        let t = decode(b"a\r\nb\r\nc\r\nd\n");
        assert_eq!(t.line_ending, LineEnding::CrLf);
        assert_eq!(t.text, "a\nb\nc\nd\n");
    }

    #[test]
    fn the_long_line_message_does_not_promise_the_problem_is_solved() {
        let message = LargeFileReason::LongLines { longest: 36_000 }.message();
        assert!(message.contains("stutter"), "{message}");
        assert!(message.contains("36000"), "{message}");
    }

    #[test]
    fn a_small_file_with_one_enormous_line_triggers_reduced_mode() {
        // The bootstrap.min.css case: not a big file at all, but pathological
        // for a widget that lays out one PangoLayout per line.
        let line = "x".repeat(LONG_LINE_CHARS + 1);
        let t = decode(line.as_bytes());
        assert!(t.byte_len < 1024 * 1024, "this is a small file");
        match t.large_file {
            Some(LargeFileReason::LongLines { longest }) => {
                assert_eq!(longest, LONG_LINE_CHARS + 1)
            }
            other => panic!("expected long-line detection, got {other:?}"),
        }
    }

    #[test]
    fn an_ordinary_file_is_not_put_into_reduced_mode() {
        let t = decode("short\nlines\nonly\n".as_bytes());
        assert!(t.large_file.is_none());
        assert_eq!(t.longest_line, 5);
    }

    #[test]
    fn line_length_is_measured_in_characters_not_bytes() {
        // Five accented characters are ten bytes in UTF-8; the limit is about
        // layout cost, which follows characters.
        let t = decode("ááááá\n".as_bytes());
        assert_eq!(t.longest_line, 5);
    }

    #[test]
    fn round_trips_content_through_save() {
        let original = b"alpha\r\nbeta\r\n";
        let t = decode(original);
        let saved = encode(&t.text, t.line_ending, t.encoding, t.had_bom);
        assert_eq!(saved, original);
    }

    #[test]
    fn round_trips_a_bom_and_utf8_content() {
        let mut original = vec![0xEF, 0xBB, 0xBF];
        original.extend_from_slice("café\n".as_bytes());
        let t = decode(&original);
        let saved = encode(&t.text, t.line_ending, t.encoding, t.had_bom);
        assert_eq!(saved, original);
    }

    #[test]
    fn an_empty_file_loads_cleanly() {
        let t = decode(b"");
        assert_eq!(t.text, "");
        assert_eq!(t.longest_line, 0);
        assert!(t.large_file.is_none());
        assert!(!t.had_errors);
    }
}
