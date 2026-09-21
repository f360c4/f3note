//! The status line.
//!
//! Small, muted, and only ever showing things that answer a question the user
//! might actually ask: where is my caret, what encoding is this, which line
//! endings will be written, and why is this document not highlighted.

use gtk::prelude::*;

use crate::document::Document;
use crate::text::LineEnding;

/// A status-bar field the user can act on.
///
/// Flat, unpadded and the same colour as the text beside it, so the bar still
/// reads as a status line rather than a toolbar — but clickable, because
/// "UTF-8" and "CRLF" are exactly the things someone wants to change the
/// moment they notice them.
fn clickable(xalign: f32) -> gtk::Button {
    let label = gtk::Label::builder().xalign(xalign).build();
    let button = gtk::Button::builder()
        .child(&label)
        .has_frame(false)
        .build();
    button.add_css_class("f3note-status-action");
    button
}

fn set_button_label(button: &gtk::Button, text: &str) {
    if let Some(label) = button.child().and_then(|c| c.downcast::<gtk::Label>().ok()) {
        label.set_text(text);
    }
}

pub struct StatusBar {
    root: gtk::Box,
    position: gtk::Label,
    pub encoding: gtk::Button,
    pub line_ending: gtk::Button,
    notice: gtk::Label,
    unsaved: gtk::Label,
}

fn cell(xalign: f32) -> gtk::Label {
    gtk::Label::builder().xalign(xalign).build()
}

impl Default for StatusBar {
    fn default() -> Self {
        Self::new()
    }
}

impl StatusBar {
    pub fn new() -> StatusBar {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(14)
            .build();
        root.add_css_class("f3note-statusbar");

        let position = cell(0.0);
        let notice = cell(0.0);
        notice.set_hexpand(true);
        let unsaved = cell(1.0);
        let encoding = clickable(1.0);
        let line_ending = clickable(1.0);
        encoding.set_tooltip_text(Some("Change how this file is decoded"));
        line_ending.set_tooltip_text(Some("Change the line endings written on save"));

        root.append(&position);
        root.append(&notice);
        root.append(&unsaved);
        root.append(&encoding);
        root.append(&line_ending);

        StatusBar {
            root,
            position,
            encoding,
            line_ending,
            notice,
            unsaved,
        }
    }

    /// How many tabs hold changes that are not on disk.
    ///
    /// Shown permanently rather than raised in a dialog when the window
    /// closes. A warning at closing time is easy to dismiss without reading
    /// and arrives too late to act on; a count that is simply always there is
    /// what makes the state something the user knows rather than discovers.
    pub fn set_unsaved(&self, count: usize) {
        self.unsaved.set_text(&match count {
            0 => String::new(),
            1 => "1 unsaved".to_owned(),
            n => format!("{n} unsaved"),
        });
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    /// Line and column, both 1-based, as every editor shows them.
    pub fn set_position(&self, line: i32, column: i32) {
        self.position
            .set_text(&format!("Ln {}, Col {}", line + 1, column + 1));
    }

    pub fn set_document(&self, doc: &Document) {
        let meta = doc.meta();
        let mut encoding = meta.encoding.clone();
        if meta.had_bom {
            encoding.push_str(" BOM");
        }
        if meta.lossy {
            // The file could not be decoded cleanly; saving it would write
            // replacement characters over whatever was really there.
            encoding.push_str(" (lossy)");
        }
        set_button_label(&self.encoding, &encoding);
        set_button_label(&self.line_ending, meta.line_ending.label());

        match meta.large_file {
            Some(reason) => self
                .notice
                .set_text(&format!("reduced mode: {}", reason.short())),
            None => self.notice.set_text(""),
        }
    }

    pub fn clear(&self) {
        self.position.set_text("");
        set_button_label(&self.encoding, "");
        set_button_label(&self.line_ending, "");
        self.notice.set_text("");
    }

    pub fn line_ending_label(ending: LineEnding) -> &'static str {
        ending.label()
    }
}
