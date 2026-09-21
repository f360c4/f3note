//! A thin, non-modal notice strip.
//!
//! Used for things the user needs to know but must not be interrupted by:
//! reduced-capability mode on a pathological file, a file changed underneath
//! them, a config that would not parse. It never blocks, never steals focus and
//! never needs a click to dismiss — an informational notice fades by itself
//! after a few seconds, while one offering an action stays until answered.

use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

/// How long a purely informational banner stays up.
const AUTO_HIDE: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warning,
    Error,
}

pub struct Banner {
    root: gtk::Revealer,
    label: gtk::Label,
    action: gtk::Button,
    container: gtk::Box,
    /// Identifies the pending auto-hide so a newer banner cancels an older
    /// one's timer instead of being dismissed by it.
    generation: Rc<RefCell<u64>>,
    /// The button is reused for every offer, so the previous handler has to be
    /// disconnected rather than left to accumulate.
    action_handler: RefCell<Option<glib::SignalHandlerId>>,
}

impl Default for Banner {
    fn default() -> Self {
        Self::new()
    }
}

impl Banner {
    pub fn new() -> Banner {
        let label = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();

        let action = gtk::Button::builder()
            .has_frame(false)
            .visible(false)
            .build();

        let container = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .build();
        container.add_css_class("f3note-banner");
        container.append(&label);
        container.append(&action);

        let root = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .transition_duration(120)
            .reveal_child(false)
            .child(&container)
            .build();

        Banner {
            root,
            label,
            action,
            container,
            generation: Rc::new(RefCell::new(0)),
            action_handler: RefCell::new(None),
        }
    }

    pub fn widget(&self) -> &gtk::Revealer {
        &self.root
    }

    fn set_level(&self, level: Level) {
        self.container.remove_css_class("warning");
        self.container.remove_css_class("error");
        match level {
            Level::Info => {}
            Level::Warning => self.container.add_css_class("warning"),
            Level::Error => self.container.add_css_class("error"),
        }
    }

    fn bump(&self) -> u64 {
        let mut g = self.generation.borrow_mut();
        *g += 1;
        *g
    }

    /// Show a notice that disappears on its own.
    pub fn info(&self, text: &str, level: Level) {
        self.label.set_text(text);
        self.action.set_visible(false);
        self.set_level(level);
        self.root.set_reveal_child(true);

        let generation = self.bump();
        let gen_cell = self.generation.clone();
        let root = self.root.clone();
        glib::timeout_add_local_once(AUTO_HIDE, move || {
            // Only hide if nothing newer has been shown in the meantime.
            if *gen_cell.borrow() == generation {
                root.set_reveal_child(false);
            }
        });
    }

    /// Show a notice with one action, which stays until the user answers or
    /// something else replaces it.
    pub fn offer<F: Fn() + 'static>(&self, text: &str, level: Level, button: &str, on_click: F) {
        self.label.set_text(text);
        self.set_level(level);
        self.action.set_label(button);
        self.action.set_visible(true);
        self.root.set_reveal_child(true);
        self.bump();

        // Replace any previous handler rather than accumulating them: the same
        // button is reused for every offer the banner makes.
        if let Some(id) = self.action_handler.borrow_mut().take() {
            self.action.disconnect(id);
        }
        let root = self.root.clone();
        let id = self.action.connect_clicked(move |_| {
            root.set_reveal_child(false);
            on_click();
        });
        *self.action_handler.borrow_mut() = Some(id);
    }

    pub fn hide(&self) {
        self.bump();
        self.root.set_reveal_child(false);
    }
}
