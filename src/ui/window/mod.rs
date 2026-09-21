//! The editor window.
//!
//! One window, many tabs — the Notepad++ shape the project is aiming at. Two
//! design points are worth calling out because they are not obvious from the
//! code alone.
//!
//! **Pages are built lazily.** A tab restored from a session starts as an empty
//! placeholder holding nothing but a path. The `View`, the `Buffer` and the
//! file contents arrive the first time the tab is actually shown. Opening a
//! session therefore costs the same whether it has three tabs or three hundred,
//! and unvisited tabs cost no layout, no highlighting and no memory beyond
//! their name.
//!
//! **Scroll position is restored by hand.** GTK discards a buffer's cached line
//! heights when a view stops showing it, so switching tabs would otherwise jump
//! to the top of the document every time. Each document keeps a mark, and the
//! view is scrolled back to it after the switch.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use sourceview5::prelude::*;

use crate::config::Config;
use crate::document::{Document, DocumentId};
use crate::mru::Mru;
use crate::session::autosave::{Job, Outcome, Schedule, Worker};
use crate::session::index::{Entry, Session};
use crate::session::store::DocStore;
use crate::theme::{Theme, ThemeEngine};
use crate::ui::banner::{Banner, Level};
use crate::ui::findbar::FindBar;
use crate::ui::statusbar::StatusBar;

/// Zoom is applied as a delta on the configured font size rather than a scale
/// factor, so the result is always a whole point size the font actually has.
const ZOOM_MIN: i32 = -6;
const ZOOM_MAX: i32 = 24;

/// One entry in a status-bar menu: what it says, and what it does.
type MenuOption = (String, Box<dyn Fn(&Rc<Window>)>);

pub struct Window {
    pub window: gtk::ApplicationWindow,
    notebook: gtk::Notebook,
    banner: Banner,
    findbar: FindBar,
    status: StatusBar,
    engine: Rc<ThemeEngine>,
    docs: RefCell<Vec<Rc<Document>>>,
    mru: RefCell<Mru<DocumentId>>,
    next_id: Cell<DocumentId>,
    next_untitled: Cell<u32>,
    zoom: Cell<i32>,
    search: RefCell<Option<sourceview5::SearchContext>>,
    /// Set while the code is changing the notebook itself, so the resulting
    /// switch-page signal does not get mistaken for the user clicking a tab.
    suppress_switch: Cell<bool>,
    state_root: PathBuf,
    autosave: Worker,
    schedule: RefCell<Schedule>,
    /// Documents whose contents came back from a mirror rather than from disk,
    /// so the tab can be shown as modified without having been typed into.
    recovered: RefCell<Vec<DocumentId>>,
    /// True once a disk error has been reported, so the banner does not repeat
    /// the same message on every tick.
    reported_failure: Cell<bool>,
    /// Set when the tab list or the active tab changed. The session is written
    /// from the tick rather than immediately, so opening twenty files at once
    /// costs one write instead of twenty.
    session_dirty: Cell<bool>,
    /// The popover currently on screen, if any. Only one is ever shown: a
    /// second grabbing popup over the first is both wrong for the user and
    /// refused by GDK.
    popover: RefCell<Option<gtk::Popover>>,
    /// Whether the overwrite-mode notice has already been shown this session.
    warned_about_overwrite: Cell<bool>,
}

mod autosave;
mod editing;
mod files;
mod find;
mod panels;
mod signals;
mod tabs;
mod testing;

impl Window {
    pub fn new(app: &gtk::Application, engine: Rc<ThemeEngine>) -> Rc<Window> {
        let state_root = crate::paths::state_dir();
        let editor = engine.config().editor;

        let notebook = gtk::Notebook::builder()
            .scrollable(true)
            .show_border(false)
            .hexpand(true)
            .vexpand(true)
            .build();

        let banner = Banner::new();
        let findbar = FindBar::new();
        let status = StatusBar::new();

        let layout = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        layout.append(banner.widget());
        layout.append(&notebook);
        layout.append(findbar.widget());
        layout.append(status.widget());

        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("f3note")
            .default_width(940)
            .default_height(660)
            .child(&layout)
            .build();
        // Every generated rule is scoped to this class, so f3note styles itself
        // without reaching into other applications through the shared display
        // provider.
        window.add_css_class("f3note");

        let this = Rc::new(Window {
            window,
            notebook,
            banner,
            findbar,
            status,
            engine,
            docs: RefCell::new(Vec::new()),
            mru: RefCell::new(Mru::default()),
            next_id: Cell::new(1),
            next_untitled: Cell::new(1),
            zoom: Cell::new(0),
            search: RefCell::new(None),
            suppress_switch: Cell::new(false),
            state_root: state_root.clone(),
            autosave: Worker::spawn(state_root),
            schedule: RefCell::new(Schedule::new(
                editor.autosave_idle_seconds,
                editor.autosave_max_seconds,
            )),
            recovered: RefCell::new(Vec::new()),
            reported_failure: Cell::new(false),
            session_dirty: Cell::new(false),
            popover: RefCell::new(None),
            warned_about_overwrite: Cell::new(false),
        });

        this.clone().accept_dropped_files();
        this.clone().connect_signals();
        this.clone().install_actions(app);
        this.clone().follow_theme();
        this.connect_status_actions();
        this.clone().start_autosave();
        this.clone().guard_close();
        this
    }
}
