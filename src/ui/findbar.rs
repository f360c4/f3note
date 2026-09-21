//! Inline find and replace.
//!
//! A strip at the bottom of the window, not a dialog. It never takes focus away
//! from the document by force, never covers the text, and closing it with
//! Escape puts the caret back where it was. `GtkSourceSearchContext` does the
//! actual searching, which matters because it scans asynchronously and
//! highlights every match without blocking the interface on a large buffer.

use gtk::prelude::*;

pub struct FindBar {
    root: gtk::Revealer,
    pub query: gtk::SearchEntry,
    pub replacement: gtk::Entry,
    replace_row: gtk::Box,
    pub matches: gtk::Label,
}

impl Default for FindBar {
    fn default() -> Self {
        Self::new()
    }
}

impl FindBar {
    pub fn new() -> FindBar {
        let query = gtk::SearchEntry::builder()
            .placeholder_text("Find")
            .hexpand(true)
            .width_chars(24)
            .build();

        let matches = gtk::Label::builder().xalign(1.0).build();

        let find_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .build();
        find_row.append(&query);
        find_row.append(&matches);

        let replacement = gtk::Entry::builder()
            .placeholder_text("Replace with")
            .hexpand(true)
            .build();
        let replace_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .visible(false)
            .build();
        replace_row.append(&replacement);

        let container = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(3)
            .build();
        container.add_css_class("f3note-findbar");
        container.append(&find_row);
        container.append(&replace_row);

        let root = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideUp)
            .transition_duration(100)
            .reveal_child(false)
            .child(&container)
            .build();

        FindBar {
            root,
            query,
            replacement,
            replace_row,
            matches,
        }
    }

    pub fn widget(&self) -> &gtk::Revealer {
        &self.root
    }

    pub fn is_open(&self) -> bool {
        self.root.reveals_child()
    }

    /// Open the bar. `with_replace` decides whether the replacement row is
    /// shown, which is the only difference between Ctrl+F and Ctrl+H.
    pub fn open(&self, with_replace: bool, seed: Option<&str>) {
        self.replace_row.set_visible(with_replace);
        if let Some(text) = seed {
            if !text.is_empty() && !text.contains('\n') {
                self.query.set_text(text);
            }
        }
        self.root.set_reveal_child(true);
        self.query.grab_focus();
        self.query.select_region(0, -1);
    }

    pub fn close(&self) {
        self.root.set_reveal_child(false);
    }

    pub fn set_match_count(&self, current: i32, total: i32) {
        if self.query.text().is_empty() {
            self.matches.set_text("");
            self.query.remove_css_class("no-match");
            return;
        }
        if total == 0 {
            self.matches.set_text("No results");
            self.query.add_css_class("no-match");
        } else {
            self.query.remove_css_class("no-match");
            // GtkSourceSearchContext reports -1 for "the caret is not on a
            // match", which is the normal state right after typing.
            if current > 0 {
                self.matches.set_text(&format!("{current} of {total}"));
            } else {
                self.matches.set_text(&format!("{total} found"));
            }
        }
    }

    /// Wire the bar to a buffer's search context. Returns the context so the
    /// caller can drive next/previous and replace.
    pub fn attach(&self, buffer: &sourceview5::Buffer) -> sourceview5::SearchContext {
        let settings = sourceview5::SearchSettings::builder()
            .wrap_around(true)
            .case_sensitive(false)
            .build();
        sourceview5::SearchContext::builder()
            .buffer(buffer)
            .settings(&settings)
            .highlight(true)
            .build()
    }
}
