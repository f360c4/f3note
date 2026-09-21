//! A document: one tab's worth of state.
//!
//! The important property here is that a document can exist without a buffer.
//! Restoring a session should not cost anything proportional to the number of
//! tabs, so a tab that has not been looked at yet holds only its path and a
//! remembered cursor position. The `sourceview5::Buffer` and the `View` that
//! draws it are built the first time the tab is actually shown. This is the
//! same shape Notepad++ uses, and it is what lets a session with a lot of tabs
//! open as quickly as a session with one.
//!
//! Swapping buffers has one trap worth knowing about. `gtk_text_view_set_buffer`
//! calls `_gtk_text_btree_remove_view`, which discards every cached line height
//! for the outgoing buffer — so scroll position is *not* preserved across a
//! switch. A mark is kept per document and scrolled back to explicitly.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::text::{self, LargeFileReason, LineEnding, LoadedText};

pub type DocumentId = u64;

/// What the document knows about the file on disk when it was last read or
/// written. Used to notice that something else changed the file underneath us.
#[derive(Debug, Clone, Copy, Default)]
pub struct DiskStamp {
    pub mtime: Option<SystemTime>,
    pub size: Option<u64>,
}

impl DiskStamp {
    pub fn of(path: &Path) -> DiskStamp {
        match std::fs::metadata(path) {
            Ok(m) => DiskStamp {
                mtime: m.modified().ok(),
                size: Some(m.len()),
            },
            Err(_) => DiskStamp::default(),
        }
    }

    /// True when the file on disk no longer matches what we last saw.
    ///
    /// A stamp with no mtime (an unreadable or absent file) is deliberately not
    /// treated as a change: it would fire constantly on network mounts and
    /// during another program's atomic replace.
    pub fn differs_from(&self, other: &DiskStamp) -> bool {
        match (self.mtime, other.mtime) {
            (Some(a), Some(b)) => a != b || self.size != other.size,
            _ => false,
        }
    }
}

/// Everything about a document that is not a GTK object.
#[derive(Debug, Clone)]
pub struct Meta {
    pub path: Option<PathBuf>,
    /// Number shown for a document that has never been saved: "Untitled 3".
    pub untitled_number: u32,
    pub encoding: String,
    pub line_ending: LineEnding,
    pub had_bom: bool,
    pub large_file: Option<LargeFileReason>,
    pub disk: DiskStamp,
    /// Byte offset of the caret, remembered for a document whose buffer has not
    /// been built yet.
    pub cursor_offset: i32,
    /// Decoding produced replacement characters; saving would lose data.
    pub lossy: bool,
}

pub struct Document {
    pub id: DocumentId,
    meta: RefCell<Meta>,
    buffer: RefCell<Option<sourceview5::Buffer>>,
    modified: Cell<bool>,
    /// Directory name under the state store holding this document's mirror and
    /// history. Derived from the path when there is one, so history survives
    /// closing and reopening a file; opaque and generated otherwise.
    store_key: RefCell<String>,
}

impl Document {
    pub fn untitled(id: DocumentId, number: u32) -> Document {
        Document {
            id,
            meta: RefCell::new(Meta {
                path: None,
                untitled_number: number,
                encoding: "UTF-8".to_owned(),
                line_ending: LineEnding::Lf,
                had_bom: false,
                large_file: None,
                disk: DiskStamp::default(),
                cursor_offset: 0,
                lossy: false,
            }),
            buffer: RefCell::new(None),
            modified: Cell::new(false),
            store_key: RefCell::new(crate::session::store::new_untitled_key()),
        }
    }

    /// A document that points at a file but has not read it yet.
    pub fn deferred(id: DocumentId, path: PathBuf, cursor_offset: i32) -> Document {
        let d = Document::untitled(id, 0);
        *d.store_key.borrow_mut() = crate::session::store::key_for_path(&path);
        {
            let mut m = d.meta.borrow_mut();
            m.path = Some(path);
            m.cursor_offset = cursor_offset;
        }
        d
    }

    /// Restore a document whose state directory is already known, so its
    /// mirror and history are found again after a restart.
    pub fn restored(id: DocumentId, key: String, meta: Meta) -> Document {
        Document {
            id,
            meta: RefCell::new(meta),
            buffer: RefCell::new(None),
            modified: Cell::new(false),
            store_key: RefCell::new(key),
        }
    }

    pub fn store_key(&self) -> String {
        self.store_key.borrow().clone()
    }

    pub fn meta(&self) -> Meta {
        self.meta.borrow().clone()
    }

    pub fn path(&self) -> Option<PathBuf> {
        self.meta.borrow().path.clone()
    }

    /// Give the document a path, as Save As does.
    ///
    /// The store key changes with it, so the document's future history is
    /// filed under the file it now is. Whatever was recorded under the old key
    /// is left alone: the caller decides whether that was a scratch buffer to
    /// forget or history worth keeping.
    pub fn set_path(&self, path: PathBuf) {
        let stamp = DiskStamp::of(&path);
        *self.store_key.borrow_mut() = crate::session::store::key_for_path(&path);
        let mut m = self.meta.borrow_mut();
        m.path = Some(path);
        m.disk = stamp;
    }

    pub fn is_modified(&self) -> bool {
        self.modified.get()
    }

    pub fn set_modified(&self, value: bool) {
        self.modified.set(value);
    }

    pub fn large_file(&self) -> Option<LargeFileReason> {
        self.meta.borrow().large_file
    }

    /// Clear reduced-capability mode because the user asked for it explicitly.
    pub fn force_full_capabilities(&self) {
        self.meta.borrow_mut().large_file = None;
    }

    pub fn buffer(&self) -> Option<sourceview5::Buffer> {
        self.buffer.borrow().clone()
    }

    pub fn set_buffer(&self, buffer: sourceview5::Buffer) {
        *self.buffer.borrow_mut() = Some(buffer);
    }

    pub fn is_materialised(&self) -> bool {
        self.buffer.borrow().is_some()
    }

    /// The name shown on the tab, without the modified marker.
    pub fn title(&self) -> String {
        let m = self.meta.borrow();
        match &m.path {
            Some(p) => p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.display().to_string()),
            None => format!("Untitled {}", m.untitled_number),
        }
    }

    /// The tab label, with Notepad++'s leading asterisk for unsaved changes.
    pub fn tab_label(&self) -> String {
        if self.modified.get() {
            format!("*{}", self.title())
        } else {
            self.title()
        }
    }

    /// The full path, for the window title and the tab tooltip.
    pub fn describe(&self) -> String {
        // Cloned out before matching: `title()` reads the same RefCell, and a
        // guard held across the arms is the shape that has twice aborted this
        // editor from inside a GTK callback.
        let path = self.meta.borrow().path.clone();
        match path {
            Some(p) => p.display().to_string(),
            None => self.title(),
        }
    }

    /// Read the file and record everything learned about it.
    ///
    /// `long_line_chars` comes from configuration rather than being read here,
    /// so the document model stays free of it.
    pub fn load(&self, long_line_chars: usize) -> std::io::Result<LoadedText> {
        let path = {
            let m = self.meta.borrow();
            match m.path.clone() {
                Some(p) => p,
                None => return Ok(text::decode(b"")),
            }
        };
        let stamp = DiskStamp::of(&path);
        let loaded = text::load(&path, long_line_chars)?;
        {
            let mut m = self.meta.borrow_mut();
            m.encoding = loaded.encoding.to_owned();
            m.line_ending = loaded.line_ending;
            m.had_bom = loaded.had_bom;
            m.large_file = loaded.large_file;
            m.lossy = loaded.had_errors;
            m.disk = stamp;
        }
        Ok(loaded)
    }

    /// Serialise the current contents the way this document should be written.
    pub fn encode(&self, contents: &str) -> Vec<u8> {
        let m = self.meta.borrow();
        text::encode(contents, m.line_ending, &m.encoding, m.had_bom)
    }

    /// Note that the document now matches what is on disk.
    pub fn mark_saved(&self) {
        self.modified.set(false);
        let path = self.meta.borrow().path.clone();
        if let Some(p) = path {
            let stamp = DiskStamp::of(&p);
            self.meta.borrow_mut().disk = stamp;
        }
    }

    /// True when something outside f3note has written to this file since we
    /// last read or wrote it.
    pub fn changed_on_disk(&self) -> bool {
        let m = self.meta.borrow();
        match m.path.as_ref() {
            Some(p) => DiskStamp::of(p).differs_from(&m.disk),
            None => false,
        }
    }

    pub fn set_cursor_offset(&self, offset: i32) {
        self.meta.borrow_mut().cursor_offset = offset;
    }

    pub fn set_line_ending(&self, ending: LineEnding) {
        self.meta.borrow_mut().line_ending = ending;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_untitled_document_gets_an_opaque_store_key() {
        let a = Document::untitled(1, 1);
        let b = Document::untitled(2, 2);
        assert!(a.store_key().starts_with("u-"));
        assert_ne!(a.store_key(), b.store_key());
    }

    #[test]
    fn a_file_backed_document_keys_off_its_path() {
        let a = Document::deferred(1, PathBuf::from("/tmp/x.txt"), 0);
        let b = Document::deferred(2, PathBuf::from("/tmp/x.txt"), 0);
        assert_eq!(
            a.store_key(),
            b.store_key(),
            "history must survive reopening"
        );
        assert!(a.store_key().starts_with("f-"));
    }

    #[test]
    fn saving_an_untitled_document_moves_it_to_a_path_key() {
        let d = Document::untitled(1, 1);
        assert!(d.store_key().starts_with("u-"));
        d.set_path(PathBuf::from("/tmp/named.txt"));
        assert_eq!(
            d.store_key(),
            crate::session::store::key_for_path(&PathBuf::from("/tmp/named.txt"))
        );
    }

    #[test]
    fn an_untitled_document_is_numbered() {
        let d = Document::untitled(1, 3);
        assert_eq!(d.title(), "Untitled 3");
        assert_eq!(d.tab_label(), "Untitled 3");
    }

    #[test]
    fn the_modified_marker_is_a_leading_asterisk() {
        let d = Document::untitled(1, 1);
        d.set_modified(true);
        assert_eq!(d.tab_label(), "*Untitled 1");
    }

    #[test]
    fn a_titled_document_shows_its_file_name_only() {
        let d = Document::deferred(1, PathBuf::from("/home/u/notes/todo.txt"), 0);
        assert_eq!(d.title(), "todo.txt");
        assert_eq!(d.describe(), "/home/u/notes/todo.txt");
        d.set_modified(true);
        assert_eq!(d.tab_label(), "*todo.txt");
    }

    #[test]
    fn a_deferred_document_holds_no_buffer_until_asked() {
        let d = Document::deferred(7, PathBuf::from("/tmp/whatever.txt"), 42);
        assert!(!d.is_materialised());
        assert_eq!(d.meta().cursor_offset, 42);
    }

    #[test]
    fn disk_stamps_compare_only_when_both_are_known() {
        let known_a = DiskStamp {
            mtime: Some(SystemTime::UNIX_EPOCH),
            size: Some(10),
        };
        let known_b = DiskStamp {
            mtime: Some(SystemTime::now()),
            size: Some(10),
        };
        assert!(known_a.differs_from(&known_b));
        assert!(!known_a.differs_from(&known_a));
        // An unreadable file must not be reported as a change: that would fire
        // constantly on network mounts and mid atomic-replace.
        assert!(!DiskStamp::default().differs_from(&known_a));
        assert!(!known_a.differs_from(&DiskStamp::default()));
    }

    #[test]
    fn an_untitled_document_never_looks_changed_on_disk() {
        let d = Document::untitled(1, 1);
        assert!(!d.changed_on_disk());
    }

    #[test]
    fn saving_preserves_the_documents_original_line_ending() {
        let d = Document::untitled(1, 1);
        d.set_line_ending(LineEnding::CrLf);
        assert_eq!(d.encode("a\nb"), b"a\r\nb");
    }
}
