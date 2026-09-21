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
}

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

    // ------------------------------------------------------------ autosave

    /// The heartbeat that decides when to mirror buffers.
    ///
    /// One second is the resolution, not the frequency of writing: the
    /// schedule decides what is actually due, and on an idle editor the answer
    /// is nothing, so this costs a comparison per second and no disk access.
    fn start_autosave(self: Rc<Self>) {
        let this = self.clone();
        glib::timeout_add_local(std::time::Duration::from_secs(1), move || {
            this.autosave_tick();
            glib::ControlFlow::Continue
        });
    }

    fn autosave_tick(self: &Rc<Self>) {
        for outcome in self.autosave.drain() {
            if let Outcome::Failed { key, error } = outcome {
                eprintln!("f3note: autosave failed for {key}: {error}");
                // Said once, not every second: a full disk would otherwise
                // bury the interface in identical banners.
                if !self.reported_failure.get() {
                    self.reported_failure.set(true);
                    self.banner.info(
                        &format!("Autosave failed: {error}. Your work is not being protected."),
                        Level::Error,
                    );
                }
            }
        }

        let due = self.schedule.borrow().due(std::time::Instant::now());
        for id in due {
            self.mirror_document(id);
            self.schedule.borrow_mut().clear(id);
            self.session_dirty.set(true);
        }

        // The session records which tabs are open, not just which have unsaved
        // changes. Writing it only when a buffer was dirty would mean opening
        // files and closing cleanly lost the tab list entirely.
        if self.session_dirty.replace(false) {
            self.save_session();
        }
    }

    /// Hand one document's current contents to the worker.
    fn mirror_document(self: &Rc<Self>, id: DocumentId) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        let Some(doc) = self.document_at(index) else {
            return;
        };
        let Some(buffer) = doc.buffer() else { return };

        // Reading the buffer must happen here, on the main thread. Everything
        // after this point is handed to the worker.
        let contents = doc.encode(&Self::buffer_text(&buffer));
        let limits = self.engine.config().editor;
        let keep_history = contents.len() as u64 <= limits.history_max_bytes;

        self.autosave.submit(Job {
            key: doc.store_key(),
            contents,
            keep_history,
            history_limit: limits.history_versions,
            history_max_total_bytes: limits.history_max_total_bytes,
        });
    }

    /// Write everything that is still pending, synchronously.
    ///
    /// Called when the window is closing, where there is no later tick to rely
    /// on. This is the path that runs when the compositor closes the window
    /// with Super+W, so it has to be complete rather than best-effort.
    fn flush_all(self: &Rc<Self>) {
        let docs = self.docs.borrow().clone();
        let limits = self.engine.config().editor;
        for doc in docs {
            let Some(buffer) = doc.buffer() else { continue };
            if !doc.is_modified() {
                continue;
            }
            let contents = doc.encode(&Self::buffer_text(&buffer));
            let store = DocStore::new(&self.state_root, &doc.store_key());
            match store.write_mirror(&contents) {
                Ok(Some(_)) if contents.len() as u64 <= limits.history_max_bytes => {
                    let _ = store.push_history(
                        &contents,
                        limits.history_versions,
                        limits.history_max_total_bytes,
                    );
                }
                Ok(_) => {}
                Err(e) => eprintln!("f3note: could not mirror {} on exit: {e}", doc.describe()),
            }
        }
        self.save_session();
    }

    /// f3note never asks "do you want to save?".
    ///
    /// That dialog exists because an editor cannot otherwise promise to keep
    /// unsaved work. This one can: everything is mirrored, so closing the
    /// window is safe by construction and the right thing to do is get out of
    /// the way. Reopening restores exactly what was there.
    fn guard_close(self: Rc<Self>) {
        let this = self.clone();
        self.window.connect_close_request(move |_| {
            this.flush_all();
            glib::Propagation::Proceed
        });
    }

    // ------------------------------------------------------------- session

    fn save_session(self: &Rc<Self>) {
        // Cloned rather than borrowed for the whole loop: holding a borrow
        // across this much code is how the tab-closing panic happened, and the
        // clone is a handful of reference-count bumps.
        let docs = self.docs.borrow().clone();
        let mut session = Session {
            version: crate::session::index::VERSION,
            documents: Vec::with_capacity(docs.len()),
            active: self.notebook.current_page().unwrap_or(0) as usize,
            next_untitled: self.next_untitled.get(),
        };

        for doc in docs.iter() {
            let meta = doc.meta();
            let mut entry = Entry {
                key: doc.store_key(),
                path: meta.path.clone(),
                untitled_number: meta.untitled_number,
                cursor: meta.cursor_offset,
                modified: doc.is_modified(),
                encoding: meta.encoding.clone(),
                line_ending: String::new(),
                had_bom: meta.had_bom,
                disk_size: meta.disk.size,
                disk_mtime_secs: meta.disk.mtime.and_then(|t| {
                    t.duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .map(|d| d.as_secs())
                }),
            };
            entry.set_line_ending(meta.line_ending);
            if entry.is_worth_restoring() {
                session.documents.push(entry);
            }
        }

        if let Err(e) = session.save(&self.state_root) {
            eprintln!("f3note: could not write the session: {e}");
        }
    }

    /// Bring back the tabs from the previous run.
    ///
    /// Returns true if anything was restored, so the caller knows whether an
    /// empty tab is still needed.
    pub fn restore_session(self: &Rc<Self>) -> bool {
        let session = Session::load(&self.state_root);
        self.restore_from(session)
    }

    /// Open the tabs a session describes.
    fn restore_from(self: &Rc<Self>, session: Session) -> bool {
        if session.documents.is_empty() {
            return false;
        }
        self.next_untitled.set(session.next_untitled.max(1));

        let mut highest_id = self.next_id.get();
        for entry in &session.documents {
            let id = highest_id;
            highest_id += 1;

            let meta = crate::document::Meta {
                path: entry.path.clone(),
                untitled_number: entry.untitled_number,
                encoding: entry.encoding.clone(),
                line_ending: entry.line_ending(),
                had_bom: entry.had_bom,
                large_file: None,
                disk: crate::document::DiskStamp {
                    mtime: entry
                        .disk_mtime_secs
                        .map(|s| std::time::UNIX_EPOCH + std::time::Duration::from_secs(s)),
                    size: entry.disk_size,
                },
                cursor_offset: entry.cursor,
                lossy: false,
            };
            let doc = Rc::new(Document::restored(id, entry.key.clone(), meta));
            if entry.modified {
                self.recovered.borrow_mut().push(id);
            }
            self.add_document(doc, false);
        }
        self.next_id.set(highest_id);

        let active = session
            .active
            .min(self.docs.borrow().len().saturating_sub(1));
        self.notebook.set_current_page(Some(active as u32));
        self.activate_document(active);

        let unsaved = self.recovered.borrow().len();
        if unsaved > 0 {
            self.banner.info(
                &if unsaved == 1 {
                    "Restored 1 document with unsaved changes".to_owned()
                } else {
                    format!("Restored {unsaved} documents with unsaved changes")
                },
                Level::Info,
            );
        }
        true
    }

    /// Close a tab and delete everything f3note kept about it.
    ///
    /// The mirror is a real copy of whatever was in the buffer, including
    /// anything pasted into a scratch tab that should not outlive it. Offering
    /// to close without offering to forget would be quietly dishonest.
    pub fn close_and_forget(self: &Rc<Self>) {
        let Some(doc) = self.current_document() else {
            return;
        };
        let store = DocStore::new(&self.state_root, &doc.store_key());
        if let Err(e) = store.forget() {
            eprintln!("f3note: could not remove the stored copy: {e}");
        }
        self.schedule.borrow_mut().forget(doc.id);
        self.close_document(doc.id);
        self.save_session();
    }

    // ---------------------------------------------------------------- tabs

    fn allocate_id(&self) -> DocumentId {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        id
    }

    /// The placeholder a tab holds until it is first shown. Cheap on purpose:
    /// this is what makes restoring a large session fast.
    fn new_page_container() -> gtk::Box {
        gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build()
    }

    fn build_tab_label(self: &Rc<Self>, doc: &Rc<Document>) -> gtk::Box {
        // width_chars is the minimum, max_width_chars the natural size. With
        // ellipsizing on and no minimum, GtkLabel is free to shrink to a bare
        // "…" — which is exactly what every tab showed: no name, no modified
        // marker, nothing to tell one tab from another. Reserving a minimum
        // means a tab is always readable, and the notebook scrolls instead of
        // squeezing when there are many.
        let label = gtk::Label::builder()
            .label(doc.tab_label())
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .width_chars(12)
            .max_width_chars(24)
            .single_line_mode(true)
            .build();

        let close = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .has_frame(false)
            .tooltip_text("Close tab")
            .build();

        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .build();
        row.append(&label);
        row.append(&close);
        row.set_tooltip_text(Some(&doc.describe()));

        let this = self.clone();
        let id = doc.id;
        close.connect_clicked(move |_| this.close_document(id));

        // The label widget is looked up by name later to update the asterisk.
        label.set_widget_name("tab-label");
        row
    }

    fn tab_label_widget(&self, page: &gtk::Widget) -> Option<gtk::Label> {
        let tab = self.notebook.tab_label(page)?;
        let row = tab.downcast::<gtk::Box>().ok()?;
        let mut child = row.first_child();
        while let Some(w) = child {
            if w.widget_name() == "tab-label" {
                return w.downcast::<gtk::Label>().ok();
            }
            child = w.next_sibling();
        }
        None
    }

    fn refresh_tab_label(&self, doc: &Rc<Document>) {
        let Some(index) = self.index_of(doc.id) else {
            return;
        };
        let Some(page) = self.notebook.nth_page(Some(index as u32)) else {
            return;
        };
        if let Some(label) = self.tab_label_widget(&page) {
            label.set_text(&doc.tab_label());
            if doc.is_modified() {
                label.add_css_class("modified");
            } else {
                label.remove_css_class("modified");
            }
        }
        if self.current_document().map(|d| d.id) == Some(doc.id) {
            self.update_window_title(doc);
        }
    }

    fn update_window_title(&self, doc: &Rc<Document>) {
        let marker = if doc.is_modified() { "*" } else { "" };
        self.window
            .set_title(Some(&format!("{marker}{} — f3note", doc.title())));
    }

    fn index_of(&self, id: DocumentId) -> Option<usize> {
        self.docs.borrow().iter().position(|d| d.id == id)
    }

    fn document_at(&self, index: usize) -> Option<Rc<Document>> {
        self.docs.borrow().get(index).cloned()
    }

    pub fn current_document(&self) -> Option<Rc<Document>> {
        let index = self.notebook.current_page()? as usize;
        self.document_at(index)
    }

    /// Add a document to the notebook without materialising it.
    fn add_document(self: &Rc<Self>, doc: Rc<Document>, focus: bool) {
        let page = Self::new_page_container();
        let tab = self.build_tab_label(&doc);

        self.suppress_switch.set(true);
        let index = self.notebook.append_page(&page, Some(&tab));
        self.notebook.set_tab_reorderable(&page, true);
        self.docs.borrow_mut().push(doc.clone());
        self.suppress_switch.set(false);

        self.session_dirty.set(true);
        if focus {
            self.select_tab(index as usize);
        }
    }

    pub fn new_untitled(self: &Rc<Self>) {
        let number = self.next_untitled.get();
        self.next_untitled.set(number + 1);
        let doc = Rc::new(Document::untitled(self.allocate_id(), number));
        self.add_document(doc, true);
    }

    /// Open a path, focusing the tab that already holds it if there is one.
    pub fn open_path(self: &Rc<Self>, path: PathBuf) {
        let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        let existing = self
            .docs
            .borrow()
            .iter()
            .position(|d| d.path().map(|p| p == canonical).unwrap_or(false));
        if let Some(index) = existing {
            self.select_tab(index);
            return;
        }

        crate::recent::Recent::new(&self.state_root).record(&canonical);
        let doc = Rc::new(Document::deferred(self.allocate_id(), canonical, 0));
        self.add_document(doc, true);
    }

    /// Restore a tab without reading the file, for session restore.
    pub fn add_deferred(self: &Rc<Self>, path: PathBuf, cursor: i32, focus: bool) {
        let doc = Rc::new(Document::deferred(self.allocate_id(), path, cursor));
        self.add_document(doc, focus);
    }

    fn close_document(self: &Rc<Self>, id: DocumentId) {
        let Some(index) = self.index_of(id) else {
            return;
        };

        self.suppress_switch.set(true);
        self.notebook.remove_page(Some(index as u32));
        let doc = self.docs.borrow_mut().remove(index);
        self.mru.borrow_mut().remove(doc.id);
        self.suppress_switch.set(false);
        self.session_dirty.set(true);

        // Closing the last tab leaves an empty one rather than closing the
        // window, which is what Notepad++ does. Quitting is Super+W, handled by
        // the compositor, or Ctrl+Q.
        if self.docs.borrow().is_empty() {
            self.new_untitled();
            return;
        }

        // Move to whatever was used most recently rather than to the tab that
        // happens to be at the same index.
        //
        // The borrow is read into a local first, deliberately. Writing this as
        // `if let Some(next) = self.mru.borrow().front()` keeps the guard alive
        // for the whole block — an `if let` scrutinee is not a terminating
        // scope the way an `if` condition is — and the activation below takes
        // the same RefCell mutably. That exact shape panicked on closing a tab.
        let next = self.mru.borrow().front();
        let target = next.and_then(|id| self.index_of(id));
        if let Some(index) = target {
            self.select_tab(index);
        }
    }

    // -------------------------------------------------------- materialising

    /// Build the view and buffer for a document and read its file. Called the
    /// first time a tab is shown, and never again for that document.
    fn materialise(self: &Rc<Self>, doc: &Rc<Document>) {
        if doc.is_materialised() {
            return;
        }

        let buffer = sourceview5::Buffer::new(None);
        buffer.set_enable_undo(true);
        buffer.set_highlight_matching_brackets(false);

        let long_line_chars = self.engine.config().appearance.long_line_chars;
        let loaded = match doc.load(long_line_chars) {
            Ok(l) => Some(l),
            Err(e) => {
                self.banner
                    .info(&format!("{}: {e}", doc.describe()), Level::Error);
                None
            }
        };

        // Crash recovery. A mirror that differs from what is on disk means the
        // editor went away before these changes were saved, so the mirror is
        // the newer truth and the file is not. This is the whole point of the
        // store: the user gets their work back without being asked anything.
        let recovered = self.recovered.borrow().contains(&doc.id);
        let loaded = if recovered {
            let store = DocStore::new(&self.state_root, &doc.store_key());
            match store.read_mirror() {
                Some(bytes) => {
                    let mut from_mirror = crate::text::decode_with_limit(&bytes, long_line_chars);
                    // Size and line-length limits still apply to recovered
                    // contents; a pathological file does not become safe
                    // because it came back from a mirror.
                    if let Some(disk) = &loaded {
                        from_mirror.large_file = from_mirror.large_file.or(disk.large_file);
                    }
                    Some(from_mirror)
                }
                None => loaded,
            }
        } else {
            loaded
        };

        if let Some(loaded) = &loaded {
            // Loading is one undoable unit that cannot be undone: the user did
            // not type this, and Ctrl+Z should never empty the document.
            buffer.begin_irreversible_action();
            buffer.set_text(&loaded.text);
            buffer.end_irreversible_action();
            // Recovered contents are, by definition, not what is on disk, so
            // the tab keeps its asterisk until the user saves.
            buffer.set_modified(recovered);
            doc.set_modified(recovered);
        }

        let view = sourceview5::View::builder()
            .buffer(&buffer)
            .monospace(true)
            .left_margin(8)
            .right_margin(8)
            .top_margin(4)
            .bottom_margin(4)
            .build();

        let config = self.engine.config();
        self.apply_document_capabilities(&view, &buffer, doc, &config);
        self.take_over_drops(&view);

        let scroller = gtk::ScrolledWindow::builder()
            .child(&view)
            .hexpand(true)
            .vexpand(true)
            .build();

        if let Some(index) = self.index_of(doc.id) {
            if let Some(page) = self.notebook.nth_page(Some(index as u32)) {
                if let Ok(container) = page.downcast::<gtk::Box>() {
                    while let Some(child) = container.first_child() {
                        container.remove(&child);
                    }
                    container.append(&scroller);
                }
            }
        }

        doc.set_buffer(buffer.clone());

        // Restore the caret where the session left it.
        let offset = doc.meta().cursor_offset;
        if offset > 0 {
            let iter = buffer.iter_at_offset(offset.min(buffer.char_count()));
            buffer.place_cursor(&iter);
            // Scrolling has to wait until the view has been allocated, or the
            // line heights it needs do not exist yet.
            let view = view.clone();
            let buffer = buffer.clone();
            glib::idle_add_local_once(move || {
                let mark = buffer.get_insert();
                view.scroll_to_mark(&mark, 0.0, true, 0.0, 0.3);
            });
        }

        self.connect_buffer(doc, &buffer);
        self.connect_view(&view);
        self.announce_capabilities(doc);

        // The label has to be refreshed explicitly here, and the reason is
        // worth writing down. A document restored from its mirror arrives
        // already modified, and that state is set before the signal above is
        // connected — so nothing updates the tab. Worse, because the buffer is
        // *already* modified, editing it does not change `is_modified()` and
        // `modified-changed` never fires at all. The tab would then show a
        // clean name for the rest of its life, on precisely the document that
        // most needs the marker: the one holding unsaved work.
        self.refresh_tab_label(doc);
    }

    /// Apply everything that depends on whether the document is in reduced mode.
    fn apply_document_capabilities(
        &self,
        view: &sourceview5::View,
        buffer: &sourceview5::Buffer,
        doc: &Rc<Document>,
        config: &Config,
    ) {
        let reduced = doc.large_file().is_some();

        view.set_show_line_numbers(config.appearance.line_numbers);
        view.set_highlight_current_line(config.appearance.highlight_current_line && !reduced);
        view.set_tab_width(config.editor.tab_width);
        view.set_insert_spaces_instead_of_tabs(config.editor.insert_spaces);

        // Wrapping is the expensive part on a pathological line: it forces the
        // widget to lay the whole line out to find break points.
        let wrapping = !reduced && config.appearance.wrap;
        view.set_wrap_mode(if wrapping {
            gtk::WrapMode::WordChar
        } else {
            gtk::WrapMode::None
        });
        if wrapping {
            Self::suppress_hyphens(buffer);
        }

        buffer.set_highlight_matching_brackets(!reduced);

        let manager = sourceview5::StyleSchemeManager::default();
        if let Some(scheme) = manager.scheme(crate::theme::scheme::SCHEME_ID) {
            buffer.set_style_scheme(Some(&scheme));
        }

        // Syntax highlighting is off by default: f3note should open looking
        // like a notepad. In reduced mode it is off regardless — the context
        // engine will spend up to two seconds on a single long line before
        // giving up, and it blocks the interface while it does.
        if config.appearance.syntax_highlighting && !reduced {
            buffer.set_highlight_syntax(true);
            if let Some(path) = doc.path() {
                // Touching the language manager scans hundreds of definition
                // files, so it happens here — after the window is up — and only
                // for a document that asked for highlighting.
                let manager = sourceview5::LanguageManager::default();
                if let Some(lang) = manager.guess_language(path.to_str(), None) {
                    buffer.set_language(Some(&lang));
                }
            }
        } else {
            buffer.set_highlight_syntax(false);
        }
    }

    /// Stop Pango inserting a hyphen where it breaks inside a word.
    ///
    /// `WrapMode::WordChar` breaks mid-word when a word will not fit, and
    /// Pango marks the break with an automatic hyphen. That is right for prose
    /// and wrong for an editor: the character is not in the file. It looks
    /// like part of the text, and in a URL, a path or an identifier it is
    /// actively misleading about what is there.
    ///
    /// The alternative, `WrapMode::Word`, also inserts nothing but refuses to
    /// break inside a word at all, so a long token runs off the edge and the
    /// window grows a horizontal scrollbar. Suppressing the hyphen keeps the
    /// wrapping and drops the invention.
    ///
    /// Only applied when wrapping is on, which means never on the files with
    /// pathological lines, where tagging the whole buffer would cost.
    fn suppress_hyphens(buffer: &sourceview5::Buffer) {
        const TAG: &str = "f3note-no-hyphens";
        let table = buffer.tag_table();
        let tag = match table.lookup(TAG) {
            Some(tag) => tag,
            None => {
                let tag = gtk::TextTag::builder()
                    .name(TAG)
                    .insert_hyphens(false)
                    .build();
                table.add(&tag);
                tag
            }
        };
        let (start, end) = buffer.bounds();
        buffer.apply_tag(&tag, &start, &end);

        // Text typed later is not covered by a tag applied now, so the tag is
        // re-applied whenever the buffer changes. Cheap here because this only
        // runs on documents whose lines are short enough to wrap.
        let tag = tag.clone();
        buffer.connect_changed(move |b| {
            let (start, end) = b.bounds();
            b.apply_tag(&tag, &start, &end);
        });
    }

    fn announce_capabilities(self: &Rc<Self>, doc: &Rc<Document>) {
        let Some(reason) = doc.large_file() else {
            return;
        };
        let this = self.clone();
        let id = doc.id;
        self.banner.offer(
            &reason.message(),
            Level::Warning,
            "Enable anyway",
            move || this.force_capabilities(id),
        );
    }

    /// The user asked for full capabilities on a document we put into reduced
    /// mode. Their machine, their call.
    fn force_capabilities(self: &Rc<Self>, id: DocumentId) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        let Some(doc) = self.document_at(index) else {
            return;
        };
        doc.force_full_capabilities();
        let config = self.engine.config();
        if let (Some(view), Some(buffer)) = (self.view_for(index), doc.buffer()) {
            self.apply_document_capabilities(&view, &buffer, &doc, &config);
        }
        self.status.set_document(&doc);
    }

    fn view_for(&self, index: usize) -> Option<sourceview5::View> {
        let page = self.notebook.nth_page(Some(index as u32))?;
        let container = page.downcast::<gtk::Box>().ok()?;
        let scroller = container
            .first_child()?
            .downcast::<gtk::ScrolledWindow>()
            .ok()?;
        scroller.child()?.downcast::<sourceview5::View>().ok()
    }

    fn current_view(&self) -> Option<sourceview5::View> {
        self.view_for(self.notebook.current_page()? as usize)
    }

    // ------------------------------------------------------------- signals

    fn connect_buffer(self: &Rc<Self>, doc: &Rc<Document>, buffer: &sourceview5::Buffer) {
        let this = self.clone();
        let doc_ref = doc.clone();
        buffer.connect_modified_changed(move |b| {
            doc_ref.set_modified(b.is_modified());
            this.refresh_tab_label(&doc_ref);
            this.status.set_unsaved(this.unsaved_count());
        });

        // Every edit arms the autosave schedule. The schedule, not this
        // signal, decides when anything is actually written.
        let this = self.clone();
        let id = doc.id;
        buffer.connect_changed(move |_| {
            this.schedule
                .borrow_mut()
                .mark_dirty(id, std::time::Instant::now());
        });

        let this = self.clone();
        let doc_ref = doc.clone();
        buffer.connect_cursor_position_notify(move |b| {
            let iter = b.iter_at_mark(&b.get_insert());
            doc_ref.set_cursor_offset(iter.offset());
            if this.current_document().map(|d| d.id) == Some(doc_ref.id) {
                this.status.set_position(iter.line(), iter.line_offset());
            }
        });
    }

    fn connect_view(self: &Rc<Self>, view: &sourceview5::View) {
        // Ctrl+Scroll zoom. Claiming the event stops the scrolled window from
        // also scrolling the document underneath the pointer.
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        let this = self.clone();
        scroll.connect_scroll(move |controller, _, dy| {
            if !controller
                .current_event_state()
                .contains(gdk::ModifierType::CONTROL_MASK)
            {
                return glib::Propagation::Proceed;
            }
            this.bump_zoom(if dy < 0.0 { 1 } else { -1 });
            glib::Propagation::Stop
        });
        view.add_controller(scroll);
    }

    /// Open files dragged onto the window.
    ///
    /// Worth having beyond convenience: f3note has no menu bar and no toolbar,
    /// so Ctrl+O is the only way in and nothing on screen says so. Dropping a
    /// file is what people try first, and it working means the editor is not
    /// a dead end for anyone who has not read the manual.
    ///
    /// Both COPY and MOVE are accepted on purpose. A file manager may offer
    /// either depending on modifiers, and a drop target that advertises only
    /// COPY silently refuses drags that arrive proposing MOVE — which is what
    /// a plain drag from some file managers does.
    fn accept_dropped_files(self: Rc<Self>) {
        self.window.add_controller(self.file_drop_target());
    }

    /// A drop target that opens files rather than pasting their paths.
    fn file_drop_target(self: &Rc<Self>) -> gtk::DropTarget {
        let drop = gtk::DropTarget::new(
            gdk::FileList::static_type(),
            gdk::DragAction::COPY | gdk::DragAction::MOVE,
        );
        let this = self.clone();
        drop.connect_drop(move |_, value, _, _| {
            let Ok(list) = value.get::<gdk::FileList>() else {
                return false;
            };
            let mut opened = false;
            for file in list.files() {
                if let Some(path) = file.path() {
                    this.open_path(path);
                    opened = true;
                }
            }
            if opened {
                this.window.present();
            }
            opened
        });
        drop
    }

    /// Take file drops away from the text view.
    ///
    /// GtkTextView installs its own drop target, and it reaches the drop
    /// before anything on the window does. Dropping a file on the text
    /// therefore pasted its path in as a string instead of opening it — the
    /// editor did exactly the wrong thing with the gesture people try first.
    ///
    /// The built-in target is removed and replaced with one that opens files.
    /// Dropping selected *text* still works: that is a separate handler.
    fn take_over_drops(self: &Rc<Self>, view: &sourceview5::View) {
        let controllers = view.observe_controllers();
        let mut existing = Vec::new();
        for index in 0..controllers.n_items() {
            let Some(object) = controllers.item(index) else {
                continue;
            };
            if let Ok(controller) = object.downcast::<gtk::EventController>() {
                // Both spellings exist depending on GTK version; neither is
                // wanted here.
                let name = controller.type_().name();
                if name == "GtkDropTarget" || name == "GtkDropTargetAsync" {
                    existing.push(controller);
                }
            }
        }
        for controller in existing {
            view.remove_controller(&controller);
        }
        view.add_controller(self.file_drop_target());
    }

    fn connect_signals(self: &Rc<Self>) {
        let this = self.clone();
        self.notebook.connect_switch_page(move |_, _, index| {
            if this.suppress_switch.get() {
                return;
            }
            this.activate_document(index as usize);
        });

        // Escape closes the find bar and returns the caret to the document.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let this = self.clone();
        keys.connect_key_pressed(move |_, key, _, state| {
            let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
            let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
            match key {
                gdk::Key::Escape if this.findbar.is_open() => {
                    this.close_find();
                    glib::Propagation::Stop
                }
                // Tab is focus navigation in GTK, so Ctrl+Tab has to be taken
                // in the capture phase before the default handler sees it.
                gdk::Key::Tab | gdk::Key::ISO_Left_Tab if ctrl => {
                    this.cycle_tab(if shift { -1 } else { 1 });
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        self.window.add_controller(keys);
    }

    /// Make a tab current and activate it exactly once.
    ///
    /// `set_current_page` emits `switch-page`, which activates the document on
    /// its own. Calling `activate_document` alongside it would do the work
    /// twice, so the signal is suppressed and the activation made explicit —
    /// that also covers the case where the page is already current and no
    /// signal would fire at all.
    fn select_tab(self: &Rc<Self>, index: usize) {
        if index >= self.docs.borrow().len() {
            return;
        }
        self.suppress_switch.set(true);
        self.notebook.set_current_page(Some(index as u32));
        self.suppress_switch.set(false);
        self.activate_document(index);
    }

    /// Called whenever a different tab becomes current.
    fn activate_document(self: &Rc<Self>, index: usize) {
        let Some(doc) = self.document_at(index) else {
            return;
        };
        self.materialise(&doc);
        self.mru.borrow_mut().touch(doc.id);
        self.session_dirty.set(true);
        self.status.set_document(&doc);
        self.status.set_unsaved(self.unsaved_count());
        self.update_window_title(&doc);

        if let Some(buffer) = doc.buffer() {
            let iter = buffer.iter_at_mark(&buffer.get_insert());
            self.status.set_position(iter.line(), iter.line_offset());
            // The search context belongs to a buffer, so it is rebuilt when the
            // document changes rather than kept across tabs.
            if self.findbar.is_open() {
                *self.search.borrow_mut() = Some(self.findbar.attach(&buffer));
                self.run_search();
            }
        }

        // Notice a file that changed underneath us while the tab was in the
        // background. Checked here rather than on a timer, so an editor left
        // open overnight is not stat()ing files it is not looking at.
        if doc.changed_on_disk() {
            let this = self.clone();
            let id = doc.id;
            self.banner.offer(
                &format!("{} changed on disk", doc.title()),
                Level::Warning,
                "Reload",
                move || this.reload_document(id),
            );
        }

        if let Some(view) = self.view_for(index) {
            view.grab_focus();
        }
    }

    /// Throw away unsaved changes and go back to what is on disk.
    ///
    /// This is the piece the design was missing. f3note never asks "do you
    /// want to save?", because it does not need to — nothing is lost on
    /// closing. But that also meant an edit made by accident followed you
    /// around forever: close, reopen, and there it was again, with no way to
    /// say "forget it, give me the file". The dialog other editors show is a
    /// crude version of this, offered only at the one moment they remember to
    /// ask. Offering it whenever the user notices is strictly better.
    ///
    /// Destructive, so it confirms first — in the banner rather than a modal,
    /// like everything else here.
    fn revert(self: &Rc<Self>) {
        let Some(doc) = self.current_document() else {
            return;
        };
        if doc.path().is_none() {
            self.banner.info(
                "This tab has never been saved, so there is nothing on disk to go back to.",
                Level::Warning,
            );
            return;
        }
        if !doc.is_modified() {
            self.banner
                .info("No unsaved changes in this tab.", Level::Info);
            return;
        }

        let this = self.clone();
        let id = doc.id;
        self.banner.offer(
            &format!("Discard unsaved changes to {}?", doc.title()),
            Level::Warning,
            "Discard",
            move || {
                // The version being thrown away is still in the history, so
                // this is recoverable even after confirming.
                this.reload_document(id);
                this.banner.info(
                    "Reverted. The discarded version is still in this document's history.",
                    Level::Info,
                );
            },
        );
    }

    /// How many open tabs hold changes that are not on disk.
    fn unsaved_count(&self) -> usize {
        self.docs
            .borrow()
            .iter()
            .filter(|d| d.is_modified())
            .count()
    }

    fn reload_document(self: &Rc<Self>, id: DocumentId) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        let Some(doc) = self.document_at(index) else {
            return;
        };
        let Some(buffer) = doc.buffer() else { return };

        let offset = {
            let iter = buffer.iter_at_mark(&buffer.get_insert());
            iter.offset()
        };
        match doc.load(self.engine.config().appearance.long_line_chars) {
            Ok(loaded) => {
                buffer.begin_irreversible_action();
                buffer.set_text(&loaded.text);
                buffer.end_irreversible_action();
                buffer.set_modified(false);
                let iter = buffer.iter_at_offset(offset.min(buffer.char_count()));
                buffer.place_cursor(&iter);
                doc.set_modified(false);
                self.refresh_tab_label(&doc);
                self.status.set_document(&doc);
            }
            Err(e) => self
                .banner
                .info(&format!("{}: {e}", doc.describe()), Level::Error),
        }
    }

    fn follow_theme(self: &Rc<Self>) {
        let this = self.clone();
        self.engine.on_change(move |_theme: &Theme| {
            let config = this.engine.config();
            let docs = this.docs.borrow().clone();
            for (index, doc) in docs.iter().enumerate() {
                if let (Some(view), Some(buffer)) = (this.view_for(index), doc.buffer()) {
                    this.apply_document_capabilities(&view, &buffer, doc, &config);
                }
            }
            this.apply_zoom();
        });
    }

    // ---------------------------------------------------------------- zoom

    fn bump_zoom(&self, delta: i32) {
        let next = (self.zoom.get() + delta).clamp(ZOOM_MIN, ZOOM_MAX);
        if next == self.zoom.get() {
            return;
        }
        self.zoom.set(next);
        self.apply_zoom();
    }

    fn apply_zoom(&self) {
        let theme = self.engine.theme();
        let size = (theme.font.size + self.zoom.get()).max(4);
        let css = format!(
            "window.f3note textview {{ font-family: {}; font-size: {}pt; }}",
            theme.font.family, size
        );
        self.zoom_provider().load_from_string(&css);
    }

    fn zoom_provider(&self) -> gtk::CssProvider {
        // Held on the window rather than the display so zoom never leaks into
        // another f3note window or another application.
        thread_local! {
            static PROVIDER: gtk::CssProvider = gtk::CssProvider::new();
        }
        PROVIDER.with(|p| {
            let provider = p.clone();
            if let Some(display) = gdk::Display::default() {
                gtk::style_context_add_provider_for_display(
                    &display,
                    &provider,
                    gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
                );
            }
            provider
        })
    }

    fn cycle_tab(self: &Rc<Self>, direction: i32) {
        let target = {
            let mru = self.mru.borrow();
            if mru.len() < 2 {
                return;
            }
            if direction > 0 {
                mru.nth(1)
            } else {
                mru.nth(mru.len() - 1)
            }
        };
        if let Some(index) = target.and_then(|id| self.index_of(id)) {
            self.select_tab(index);
        }
    }

    // ------------------------------------------------------------ searching

    fn run_search(&self) {
        let Some(context) = self.search.borrow().clone() else {
            return;
        };
        let text = self.findbar.query.text();
        context.settings().set_search_text(Some(text.as_str()));
        self.findbar
            .set_match_count(-1, context.occurrences_count());
    }

    fn install_actions(self: Rc<Self>, app: &gtk::Application) {
        let window = self.window.clone();

        // One boxed handler per action; named so the signature does not read as
        // a wall of angle brackets at every call site.
        type Handler = Box<dyn Fn(&Rc<Window>)>;
        let add = |name: &str, accels: &[&str], f: Handler| {
            let action = gio::SimpleAction::new(name, None);
            let this = self.clone();
            action.connect_activate(move |_, _| f(&this));
            window.add_action(&action);
            if !accels.is_empty() {
                app.set_accels_for_action(&format!("win.{name}"), accels);
            }
        };

        add("new-tab", &["<Control>t"], Box::new(|w| w.new_untitled()));
        add(
            "close-tab",
            &["<Control>w"],
            Box::new(|w| {
                if let Some(doc) = w.current_document() {
                    w.close_document(doc.id);
                }
            }),
        );
        add("quit", &["<Control>q"], Box::new(|w| w.window.close()));
        add("find", &["<Control>f"], Box::new(|w| w.open_find(false)));
        add("replace", &["<Control>h"], Box::new(|w| w.open_find(true)));
        add(
            "zoom-in",
            &["<Control>plus", "<Control>equal"],
            Box::new(|w| w.bump_zoom(1)),
        );
        add(
            "zoom-out",
            &["<Control>minus"],
            Box::new(|w| w.bump_zoom(-1)),
        );
        add(
            "zoom-reset",
            &["<Control>0"],
            Box::new(|w| {
                w.zoom.set(0);
                w.apply_zoom();
            }),
        );
        add("open", &["<Control>o"], Box::new(|w| w.open_dialog()));
        add("revert", &["<Control><Shift>r"], Box::new(|w| w.revert()));
        add(
            "close-and-forget",
            &["<Control><Shift>w"],
            Box::new(|w| w.close_and_forget()),
        );
        add("save", &["<Control>s"], Box::new(|w| w.save()));
        add("save-as", &["<Control><Shift>s"], Box::new(|w| w.save_as()));
        add("goto-line", &["<Control>g"], Box::new(|w| w.goto_line()));
        add(
            "duplicate-line",
            &["<Control>d"],
            Box::new(|w| w.duplicate_lines()),
        );
        add("move-line-up", &["<Alt>Up"], Box::new(|w| w.move_lines(-1)));
        add(
            "move-line-down",
            &["<Alt>Down"],
            Box::new(|w| w.move_lines(1)),
        );
        add(
            "toggle-comment",
            &["<Control>slash"],
            Box::new(|w| w.toggle_comment()),
        );
        add("switcher", &["<Control>p"], Box::new(|w| w.open_switcher()));
        add(
            "history",
            &["<Control><Shift>h"],
            Box::new(|w| w.open_history()),
        );
        add(
            "search-files",
            &["<Control><Shift>f"],
            Box::new(|w| w.search_files()),
        );
        add(
            "sessions",
            &["<Control><Shift>e"],
            Box::new(|w| w.open_sessions()),
        );
        add(
            "find-next",
            &["<Control>k"],
            Box::new(|w| w.find_step(true)),
        );
        add(
            "find-previous",
            &["<Control><Shift>k"],
            Box::new(|w| w.find_step(false)),
        );

        // Alt+1..9 jumps to a tab by position. Alt+9 is the last tab rather
        // than the ninth, matching what browsers and Notepad++ both do.
        for n in 1..=9usize {
            let action = gio::SimpleAction::new(&format!("goto-tab-{n}"), None);
            let this = self.clone();
            action.connect_activate(move |_, _| {
                if n == 9 {
                    let last = this.docs.borrow().len().saturating_sub(1);
                    this.goto_tab_index(last);
                } else {
                    this.goto_tab_index(n - 1);
                }
            });
            window.add_action(&action);
            app.set_accels_for_action(&format!("win.goto-tab-{n}"), &[&format!("<Alt>{n}")]);
        }

        self.connect_findbar();
    }

    fn connect_findbar(self: &Rc<Self>) {
        // GtkSearchEntry turns Escape into `stop-search`. Handling that signal
        // is more reliable than racing the entry for the key event, and it
        // also covers the entry's own clear button.
        let this = self.clone();
        self.findbar
            .query
            .connect_stop_search(move |_| this.close_find());
        let this = self.clone();
        self.findbar
            .replacement
            .connect_activate(move |_| this.replace_current(false));

        let this = self.clone();
        self.findbar.query.connect_search_changed(move |_| {
            this.run_search();
            // Jumping to the first hit as the user types is what makes an
            // inline find feel like a find rather than a filter.
            this.find_step(true);
        });

        let this = self.clone();
        self.findbar
            .query
            .connect_activate(move |_| this.find_step(true));

        let this = self.clone();
        self.findbar
            .replacement
            .connect_activate(move |_| this.replace_current(false));

        // Shift+Enter in the query steps backwards, which saves a second
        // shortcut for something people do constantly.
        let keys = gtk::EventControllerKey::new();
        let this = self.clone();
        keys.connect_key_pressed(move |_, key, _, state| {
            let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
            let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
            match key {
                gdk::Key::Return | gdk::Key::KP_Enter if shift => {
                    this.find_step(false);
                    glib::Propagation::Stop
                }
                // Ctrl+Enter in the replacement field replaces everything.
                gdk::Key::Return | gdk::Key::KP_Enter if ctrl => {
                    this.replace_current(true);
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        self.findbar.query.add_controller(keys);

        let keys = gtk::EventControllerKey::new();
        let this = self.clone();
        keys.connect_key_pressed(move |_, key, _, state| {
            if matches!(key, gdk::Key::Return | gdk::Key::KP_Enter)
                && state.contains(gdk::ModifierType::CONTROL_MASK)
            {
                this.replace_current(true);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.findbar.replacement.add_controller(keys);
    }

    // ------------------------------------------------------------- saving

    /// Contents of a document's buffer, as the editor currently holds them.
    fn buffer_text(buffer: &sourceview5::Buffer) -> String {
        let (start, end) = buffer.bounds();
        buffer.text(&start, &end, true).to_string()
    }

    /// Write a document to its own path. Untitled documents are sent to
    /// Save As instead.
    pub fn save(self: &Rc<Self>) {
        let Some(doc) = self.current_document() else {
            return;
        };
        if doc.path().is_none() {
            self.save_as();
            return;
        }
        self.write_document(&doc);
    }

    fn write_document(self: &Rc<Self>, doc: &Rc<Document>) {
        let Some(buffer) = doc.buffer() else { return };
        let Some(path) = doc.path() else { return };

        if doc.meta().lossy {
            // The file could not be decoded cleanly when it was read, so what
            // is in the buffer contains replacement characters. Writing that
            // back would destroy whatever those bytes really were.
            self.banner.info(
                "This file could not be decoded cleanly; saving would lose data. Use Save As.",
                Level::Error,
            );
            return;
        }

        let bytes = doc.encode(&Self::buffer_text(&buffer));
        match crate::atomic::write(&path, &bytes) {
            Ok(()) => {
                doc.mark_saved();
                buffer.set_modified(false);
                self.refresh_tab_label(doc);
                self.status.set_document(doc);
            }
            Err(e) => self.banner.info(
                &format!("Could not save {}: {e}", path.display()),
                Level::Error,
            ),
        }
    }

    pub fn save_as(self: &Rc<Self>) {
        let Some(doc) = self.current_document() else {
            return;
        };

        let dialog = gtk::FileDialog::builder()
            .title("Save as")
            .initial_name(doc.title())
            .modal(true)
            .build();
        if let Some(parent) = doc.path().and_then(|p| p.parent().map(gio::File::for_path)) {
            dialog.set_initial_folder(Some(&parent));
        }

        let this = self.clone();
        let doc = doc.clone();
        dialog.save(Some(&self.window), gio::Cancellable::NONE, move |result| {
            let Ok(file) = result else {
                // Cancelling is a normal outcome, not an error worth reporting.
                return;
            };
            let Some(path) = file.path() else { return };
            doc.set_path(path);
            this.write_document(&doc);
            this.refresh_tab_label(&doc);
        });
    }

    pub fn open_dialog(self: &Rc<Self>) {
        let dialog = gtk::FileDialog::builder().title("Open").modal(true).build();
        let this = self.clone();
        dialog.open_multiple(Some(&self.window), gio::Cancellable::NONE, move |result| {
            let Ok(files) = result else { return };
            for item in files.iter::<gio::File>().flatten() {
                if let Some(path) = item.path() {
                    this.open_path(path);
                }
            }
        });
    }

    // ---------------------------------------------------- line operations

    /// The lines a line command should act on, from the current selection.
    ///
    /// Returns the first and last line, plus iterators spanning them whole
    /// including the trailing newline where there is one.
    fn selected_lines(
        buffer: &sourceview5::Buffer,
    ) -> Option<(i32, i32, gtk::TextIter, gtk::TextIter)> {
        let (sel_start, sel_end) = match buffer.selection_bounds() {
            Some(bounds) => bounds,
            None => {
                let iter = buffer.iter_at_mark(&buffer.get_insert());
                (iter, iter)
            }
        };
        let (first, last) =
            crate::edit::affected_lines(sel_start.line(), sel_end.line(), sel_end.starts_line());

        let start = buffer.iter_at_line(first)?;
        // The end of the block is the start of the line after it, so the
        // trailing newline travels with the block. On the last line there is
        // no line after, so the end of the buffer stands in.
        let end = match buffer.iter_at_line(last + 1) {
            Some(iter) => iter,
            None => buffer.end_iter(),
        };
        Some((first, last, start, end))
    }

    /// Copy the current line, or the selected lines, below themselves.
    fn duplicate_lines(self: &Rc<Self>) {
        let Some(buffer) = self.current_document().and_then(|d| d.buffer()) else {
            return;
        };
        let Some((_, _, start, end)) = Self::selected_lines(&buffer) else {
            return;
        };
        let block = buffer.text(&start, &end, true).to_string();
        // A block that does not end in a newline is the last line of the file;
        // one has to be added or the copy joins onto it.
        let insert = if block.ends_with('\n') {
            block.clone()
        } else {
            format!("\n{block}")
        };

        buffer.begin_user_action();
        let mut at = end;
        buffer.insert(&mut at, &insert);
        buffer.end_user_action();
    }

    /// Move the current line, or the selected lines, up or down.
    fn move_lines(self: &Rc<Self>, delta: i32) {
        let Some(buffer) = self.current_document().and_then(|d| d.buffer()) else {
            return;
        };
        let Some((first, last, start, end)) = Self::selected_lines(&buffer) else {
            return;
        };
        let line_count = buffer.line_count();
        let target = first + delta;
        if target < 0 || last + delta > line_count - 1 {
            return;
        }

        let block = buffer.text(&start, &end, true).to_string();
        // Normalise: the block is re-inserted with a trailing newline, so a
        // block taken from the last line does not weld itself to its new
        // neighbour.
        let had_newline = block.ends_with('\n');
        let body = block.trim_end_matches('\n').to_string();

        buffer.begin_user_action();
        let (mut cut_start, mut cut_end) = (start, end);
        buffer.delete(&mut cut_start, &mut cut_end);

        // Deleting the block may have left the buffer without the newline that
        // separated it from what followed; put the caret at the target line
        // and re-insert with the separator the position needs.
        let mut at = buffer
            .iter_at_line(target.min(buffer.line_count() - 1))
            .unwrap_or_else(|| buffer.end_iter());
        let at_end_of_buffer = at.is_end();
        let insert = if at_end_of_buffer && !had_newline {
            format!("\n{body}")
        } else {
            format!("{body}\n")
        };
        buffer.insert(&mut at, &insert);

        // Keep the same lines selected so the command can be repeated.
        let moved_first = target;
        let moved_last = target + (last - first);
        if let (Some(sel_start), sel_end) = (
            buffer.iter_at_line(moved_first),
            buffer
                .iter_at_line(moved_last + 1)
                .unwrap_or_else(|| buffer.end_iter()),
        ) {
            buffer.select_range(&sel_start, &sel_end);
        }
        buffer.end_user_action();

        if let Some(view) = self.current_view() {
            view.scroll_to_mark(&buffer.get_insert(), 0.0, false, 0.0, 0.0);
        }
    }

    /// Comment the selected lines, or uncomment them if they all already are.
    fn toggle_comment(self: &Rc<Self>) {
        let Some(doc) = self.current_document() else {
            return;
        };
        let Some(buffer) = doc.buffer() else { return };
        let Some((_, _, start, end)) = Self::selected_lines(&buffer) else {
            return;
        };

        // The syntax engine knows the right marker when it knows the language.
        // It usually does not here, because highlighting is off by default, so
        // the extension decides in that case.
        let prefix = buffer
            .language()
            .and_then(|lang| lang.metadata("line-comment-start"))
            .map(|m| m.to_string())
            .unwrap_or_else(|| crate::edit::comment_prefix_for(doc.path().as_deref()).to_owned());

        let block = buffer.text(&start, &end, true).to_string();
        let trailing_newline = block.ends_with('\n');
        let body = block.strip_suffix('\n').unwrap_or(&block);
        let lines: Vec<&str> = body.split('\n').collect();

        let uncomment = crate::edit::should_uncomment(&lines, &prefix);
        let column = crate::edit::comment_column(&lines);
        let mut rewritten: Vec<String> = lines
            .iter()
            .map(|line| {
                if uncomment {
                    crate::edit::uncomment_line(line, &prefix)
                } else {
                    crate::edit::comment_line(line, &prefix, column)
                }
            })
            .collect();
        if trailing_newline {
            rewritten.push(String::new());
        }
        let replacement = rewritten.join("\n");

        buffer.begin_user_action();
        let (mut cut_start, mut cut_end) = (start, end);
        buffer.delete(&mut cut_start, &mut cut_end);
        buffer.insert(&mut cut_start, &replacement);
        buffer.end_user_action();
    }

    // ------------------------------------------------------- go to line

    /// Anchor a popover so it appears over the document, centred near the top.
    ///
    /// Parenting to the window itself produces "Tried to map a grabbing popup
    /// with a non-top most parent" and puts the popup somewhere arbitrary. The
    /// notebook is the right anchor: it is the widget the popover is logically
    /// about, and pointing at a thin rectangle across its top gives the
    /// command-palette placement people expect.
    /// Take the current popover out of its slot, leaving it empty.
    ///
    /// A method rather than an inline `self.popover.borrow_mut().take()`
    /// because the borrow guard must not outlive the statement: `popdown()`
    /// emits `closed` synchronously, and that handler reads the same RefCell.
    /// Written inline inside an `if let`, the guard survives the whole block
    /// and the editor aborts. Keeping every access behind a method that
    /// returns owned data makes that shape impossible to write by accident.
    fn take_popover(&self) -> Option<gtk::Popover> {
        self.popover.borrow_mut().take()
    }

    fn set_popover(&self, popover: Option<gtk::Popover>) {
        *self.popover.borrow_mut() = popover;
    }

    fn popover_is(&self, popover: &gtk::Popover) -> bool {
        self.popover
            .borrow()
            .as_ref()
            .map(|p| p == popover)
            .unwrap_or(false)
    }

    /// Close a popover on Escape without relying on the implicit grab.
    ///
    /// An autohide popover is supposed to take a grab and handle Escape and
    /// clicking away itself. When the grab is refused — which GDK reports as
    /// "Tried to map a grabbing popup with a non-top most parent" — neither
    /// works and the popover is stuck on screen with no way out. Handling the
    /// key directly means Escape closes it either way.
    fn escape_closes(popover: &gtk::Popover) {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let popover_ref = popover.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                popover_ref.popdown();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        popover.add_controller(keys);
    }

    fn anchor_popover(&self, popover: &gtk::Popover) {
        // Only one popover at a time. Pressing Ctrl+P while the go-to-line box
        // is open is a normal thing to do, and stacking a second grabbing
        // popup on the first is refused by GDK with "Tried to map a grabbing
        // popup with a non-top most parent".
        let previous = self.take_popover();
        if let Some(previous) = previous {
            // Only pop down. `popdown` emits `closed` synchronously, and that
            // handler is what unparents; calling `unparent` here as well
            // unparents twice and GTK complains about a widget that is no
            // longer one.
            previous.popdown();
        }
        self.set_popover(Some(popover.clone()));

        popover.set_parent(&self.notebook);
        popover.set_position(gtk::PositionType::Bottom);
        popover.set_halign(gtk::Align::Center);
        popover.set_has_arrow(false);
        let width = self.notebook.width().max(1);
        popover.set_pointing_to(Some(&gdk::Rectangle::new(width / 2, 0, 1, 1)));
        popover.add_css_class("f3note-switcher");
        Self::escape_closes(popover);
    }

    fn goto_line(self: &Rc<Self>) {
        let Some(buffer) = self.current_document().and_then(|d| d.buffer()) else {
            return;
        };
        let total = buffer.line_count();

        let entry = gtk::Entry::builder()
            .placeholder_text(format!("Line (1-{total})"))
            .input_purpose(gtk::InputPurpose::Digits)
            .activates_default(true)
            .build();

        // A small popover, not a modal dialog: the point of an inline editor is
        // that nothing blocks.
        let popover = gtk::Popover::builder().child(&entry).autohide(true).build();
        self.anchor_popover(&popover);

        let this = self.clone();
        let popover_ref = popover.clone();
        entry.connect_activate(move |e| {
            if let Ok(line) = e.text().trim().parse::<i32>() {
                this.jump_to_line(line - 1);
            }
            popover_ref.popdown();
        });
        let this = self.clone();
        popover.connect_closed(move |p| this.dismiss_popover(p));

        popover.popup();
        entry.grab_focus();
    }

    /// Tear down a popover once it closes, and forget it if it was the current
    /// one. Guarded so a popover replaced by a newer one does not clear the
    /// newer one's slot on its way out.
    fn dismiss_popover(&self, popover: &gtk::Popover) {
        if self.popover_is(popover) {
            self.set_popover(None);
        }
        // Unparenting is deferred by one main-loop turn on purpose. GTK is
        // still finishing the close when `closed` runs: it goes on to restore
        // focus to whatever had it before, which needs the popover's root.
        // Unparenting here leaves it rootless mid-sequence, and GTK reports
        // "gtk_window_get_focus: assertion GTK_IS_WINDOW (window) failed"
        // followed by a failed ancestor check. Letting the turn finish first
        // costs nothing and keeps focus handling intact — which is what makes
        // Escape and clicking away work.
        let popover = popover.clone();
        glib::idle_add_local_once(move || {
            if popover.parent().is_some() {
                popover.unparent();
            }
        });
    }

    fn jump_to_line(self: &Rc<Self>, line: i32) {
        let Some(buffer) = self.current_document().and_then(|d| d.buffer()) else {
            return;
        };
        let line = line.clamp(0, (buffer.line_count() - 1).max(0));
        let Some(iter) = buffer.iter_at_line(line) else {
            return;
        };
        buffer.place_cursor(&iter);
        if let Some(view) = self.current_view() {
            view.scroll_to_mark(&buffer.get_insert(), 0.0, true, 0.0, 0.3);
            view.grab_focus();
        }
    }

    // ------------------------------------------------- named sessions

    /// Save and restore named sets of tabs.
    ///
    /// The point is switching context without losing one. Working on a config
    /// change, then a note, then back — each is a handful of files, and
    /// keeping them all open at once turns the tab bar into a haystack.
    /// Switching sessions never loses anything: the tabs being closed are
    /// mirrored like any others, and the session you leave is saved first.
    fn open_sessions(self: &Rc<Self>) {
        let entry = gtk::Entry::builder()
            .placeholder_text("Save these tabs as…")
            .activates_default(true)
            .build();

        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Browse)
            .build();

        let popover = gtk::Popover::builder().autohide(true).build();

        let names = crate::session::index::list_named(&self.state_root);
        if names.is_empty() {
            let empty = gtk::Label::builder()
                .label("No saved sessions yet")
                .xalign(0.0)
                .build();
            empty.add_css_class("path");
            list.append(&gtk::ListBoxRow::builder().child(&empty).build());
        }
        for name in &names {
            let label = gtk::Label::builder().label(name).xalign(0.0).build();
            let remove = gtk::Button::builder()
                .icon_name("window-close-symbolic")
                .has_frame(false)
                .tooltip_text("Forget this session")
                .build();
            let row_box = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(8)
                .build();
            label.set_hexpand(true);
            row_box.append(&label);
            row_box.append(&remove);
            let row = gtk::ListBoxRow::builder().child(&row_box).build();
            unsafe { row.set_data("f3note-session", name.clone()) };
            list.append(&row);

            let this = self.clone();
            let name = name.clone();
            let popover_ref = popover.clone();
            remove.connect_clicked(move |_| {
                let _ = crate::session::index::Session::delete_named(&this.state_root, &name);
                popover_ref.popdown();
                this.banner
                    .info(&format!("Forgot the session \"{name}\"."), Level::Info);
            });
        }

        let scroller = gtk::ScrolledWindow::builder()
            .child(&list)
            .min_content_height(160)
            .max_content_height(320)
            .propagate_natural_height(true)
            .build();

        let heading = gtk::Label::builder().label("Sessions").xalign(0.0).build();
        heading.add_css_class("path");
        let layout = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .build();
        layout.append(&heading);
        layout.append(&scroller);
        layout.append(&entry);
        popover.set_child(Some(&layout));
        self.anchor_popover(&popover);

        let this = self.clone();
        let popover_ref = popover.clone();
        list.connect_row_activated(move |_, row| {
            let name = unsafe { row.data::<String>("f3note-session") };
            popover_ref.popdown();
            if let Some(name) = name {
                let name = unsafe { name.as_ref().clone() };
                this.switch_to_session(&name);
            }
        });

        let this = self.clone();
        let popover_ref = popover.clone();
        entry.connect_activate(move |e| {
            let name = e.text().to_string();
            popover_ref.popdown();
            if name.trim().is_empty() {
                return;
            }
            this.save_session_as(&name);
        });

        let popover_ref = popover.clone();
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                popover_ref.popdown();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        popover.add_controller(keys);

        let this = self.clone();
        popover.connect_closed(move |p| this.dismiss_popover(p));
        popover.popup();
        entry.grab_focus();
    }

    /// Build a session description of the tabs currently open.
    fn current_session(&self) -> crate::session::index::Session {
        let docs = self.docs.borrow().clone();
        let mut session = crate::session::index::Session {
            version: crate::session::index::VERSION,
            documents: Vec::with_capacity(docs.len()),
            active: self.notebook.current_page().unwrap_or(0) as usize,
            next_untitled: self.next_untitled.get(),
        };
        for doc in docs.iter() {
            let meta = doc.meta();
            let mut entry = Entry {
                key: doc.store_key(),
                path: meta.path.clone(),
                untitled_number: meta.untitled_number,
                cursor: meta.cursor_offset,
                modified: doc.is_modified(),
                encoding: meta.encoding.clone(),
                line_ending: String::new(),
                had_bom: meta.had_bom,
                disk_size: meta.disk.size,
                disk_mtime_secs: meta.disk.mtime.and_then(|t| {
                    t.duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .map(|d| d.as_secs())
                }),
            };
            entry.set_line_ending(meta.line_ending);
            if entry.is_worth_restoring() {
                session.documents.push(entry);
            }
        }
        session
    }

    fn save_session_as(self: &Rc<Self>, name: &str) {
        let session = self.current_session();
        match session.save_named(&self.state_root, name) {
            Ok(()) => self.banner.info(
                &format!(
                    "Saved {} tabs as \"{}\".",
                    session.documents.len(),
                    crate::session::index::sanitise_name(name)
                ),
                Level::Info,
            ),
            Err(e) => self
                .banner
                .info(&format!("Could not save the session: {e}"), Level::Error),
        }
    }

    /// Close what is open and bring back a named set of tabs.
    fn switch_to_session(self: &Rc<Self>, name: &str) {
        let Some(session) = crate::session::index::Session::load_named(&self.state_root, name)
        else {
            self.banner
                .info(&format!("No session called \"{name}\"."), Level::Warning);
            return;
        };

        // Everything about to be closed is mirrored first, so switching away
        // from unsaved work costs nothing.
        self.flush_all();

        let ids: Vec<DocumentId> = self.docs.borrow().iter().map(|d| d.id).collect();
        self.suppress_switch.set(true);
        for _ in 0..ids.len() {
            self.notebook.remove_page(Some(0));
        }
        self.docs.borrow_mut().clear();
        *self.mru.borrow_mut() = Mru::default();
        self.suppress_switch.set(false);

        self.restore_from(session);
        if self.docs.borrow().is_empty() {
            self.new_untitled();
        }
        self.session_dirty.set(true);
        self.banner
            .info(&format!("Switched to \"{name}\"."), Level::Info);
    }

    // --------------------------------------------- search across files

    /// Search the open tabs and the current file's folder.
    ///
    /// Scope is deliberately modest. This is a notepad; `ripgrep` exists and
    /// is better at searching a codebase. What is worth having here is finding
    /// the thing you know is in one of the files you are working on, without
    /// leaving the editor to do it.
    fn search_files(self: &Rc<Self>) {
        let entry = gtk::SearchEntry::builder()
            .placeholder_text("Search open tabs and this folder")
            .width_chars(44)
            .build();
        let summary = gtk::Label::builder().xalign(0.0).build();
        summary.add_css_class("path");

        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Browse)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .child(&list)
            .min_content_height(260)
            .max_content_height(420)
            .propagate_natural_height(true)
            .build();

        let layout = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .build();
        layout.append(&entry);
        layout.append(&summary);
        layout.append(&scroller);

        let popover = gtk::Popover::builder()
            .child(&layout)
            .autohide(true)
            .build();
        self.anchor_popover(&popover);

        let this = self.clone();
        let list_ref = list.clone();
        let summary_ref = summary.clone();
        entry.connect_search_changed(move |e| {
            let query = e.text().to_string();
            while let Some(child) = list_ref.first_child() {
                list_ref.remove(&child);
            }
            // One or two characters match nearly everything and make the list
            // useless while costing the most to build.
            if query.chars().count() < 2 {
                summary_ref.set_text("");
                return;
            }

            let results = this.collect_matches(&query);
            summary_ref.set_text(&match results.len() {
                0 => "No matches".to_owned(),
                1 => "1 match".to_owned(),
                n => format!("{n} matches"),
            });

            for (path, m) in results.iter().take(500) {
                let where_ = gtk::Label::builder()
                    .label(format!(
                        "{}:{}",
                        path.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.display().to_string()),
                        m.line + 1
                    ))
                    .xalign(0.0)
                    .build();
                let text = gtk::Label::builder()
                    .label(m.text.trim())
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .build();
                text.add_css_class("path");

                let row_box = gtk::Box::builder()
                    .orientation(gtk::Orientation::Vertical)
                    .build();
                row_box.append(&where_);
                row_box.append(&text);
                let row = gtk::ListBoxRow::builder().child(&row_box).build();
                unsafe { row.set_data("f3note-hit-path", path.display().to_string()) };
                unsafe { row.set_data("f3note-hit-line", m.line) };
                list_ref.append(&row);
            }
            if let Some(first) = list_ref.row_at_index(0) {
                list_ref.select_row(Some(&first));
            }
        });

        let jump = {
            let this = self.clone();
            let popover = popover.clone();
            move |row: &gtk::ListBoxRow| {
                let path = unsafe { row.data::<String>("f3note-hit-path") };
                let line = unsafe { row.data::<i32>("f3note-hit-line") };
                popover.popdown();
                let (Some(path), Some(line)) = (path, line) else {
                    return;
                };
                let path = unsafe { path.as_ref().clone() };
                let line = unsafe { *line.as_ref() };
                this.open_path(std::path::PathBuf::from(path));
                this.jump_to_line(line);
            }
        };

        let jump_row = jump.clone();
        list.connect_row_activated(move |_, row| jump_row(row));

        let list_ref = list.clone();
        let jump_entry = jump.clone();
        entry.connect_activate(move |_| {
            if let Some(row) = list_ref.selected_row() {
                jump_entry(&row);
            }
        });

        let keys = gtk::EventControllerKey::new();
        let list_ref = list.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            let delta = match key {
                gdk::Key::Down => 1,
                gdk::Key::Up => -1,
                _ => return glib::Propagation::Proceed,
            };
            let current = list_ref.selected_row().map(|r| r.index()).unwrap_or(0);
            if let Some(next) = list_ref.row_at_index(current + delta) {
                list_ref.select_row(Some(&next));
            }
            glib::Propagation::Stop
        });
        entry.add_controller(keys);

        let popover_ref = popover.clone();
        entry.connect_stop_search(move |_| popover_ref.popdown());
        let this = self.clone();
        popover.connect_closed(move |p| this.dismiss_popover(p));

        popover.popup();
        entry.grab_focus();
    }

    /// Everything matching `query`, in the open tabs and the current folder.
    fn collect_matches(
        self: &Rc<Self>,
        query: &str,
    ) -> Vec<(std::path::PathBuf, crate::search::Match)> {
        use std::collections::HashSet;

        let mut results = Vec::new();
        let mut searched: HashSet<std::path::PathBuf> = HashSet::new();

        // Open tabs first, and from the buffer rather than from disk: what the
        // user can see is what they expect to search, unsaved changes and all.
        let docs = self.docs.borrow().clone();
        for doc in docs.iter() {
            let Some(buffer) = doc.buffer() else { continue };
            let (start, end) = buffer.bounds();
            let text = buffer.text(&start, &end, true).to_string();
            let path = doc
                .path()
                .unwrap_or_else(|| std::path::PathBuf::from(doc.title()));
            if let Some(real) = doc.path() {
                searched.insert(real);
            }
            for m in crate::search::find_in_text(&text, query, false) {
                results.push((path.clone(), m));
            }
        }

        // Then the folder the current file lives in.
        let folder = self
            .current_document()
            .and_then(|d| d.path())
            .and_then(|p| p.parent().map(|p| p.to_path_buf()));
        let Some(folder) = folder else {
            return results;
        };

        let limits = crate::search::Limits::default();
        for path in crate::search::collect_files(&folder, &limits) {
            if searched.contains(&path) {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            if !crate::search::looks_like_text(&bytes[..bytes.len().min(1024)]) {
                continue;
            }
            let text = String::from_utf8_lossy(&bytes);
            for m in crate::search::find_in_text(&text, query, false) {
                results.push((path.clone(), m));
            }
        }
        results
    }

    // ------------------------------------------------ status bar actions

    /// One entry in a status-bar menu: what it says, and what it does.
    fn status_menu(self: &Rc<Self>, anchor: &gtk::Button, title: &str, options: Vec<MenuOption>) {
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();

        let popover = gtk::Popover::builder().autohide(true).build();

        for (label, action) in options {
            let row = gtk::ListBoxRow::builder()
                .child(&gtk::Label::builder().label(&label).xalign(0.0).build())
                .build();
            list.append(&row);
            let this = self.clone();
            let popover_ref = popover.clone();
            let gesture = gtk::GestureClick::new();
            gesture.connect_released(move |_, _, _, _| {
                popover_ref.popdown();
                action(&this);
            });
            row.add_controller(gesture);
        }

        let heading = gtk::Label::builder().label(title).xalign(0.0).build();
        heading.add_css_class("path");
        let layout = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .build();
        layout.append(&heading);
        layout.append(&list);
        popover.set_child(Some(&layout));

        // Anchored to the button rather than the notebook: this menu is about
        // that field, and it should appear beside it.
        popover.set_parent(anchor);
        popover.set_position(gtk::PositionType::Top);
        popover.add_css_class("f3note-switcher");
        let this = self.clone();
        popover.connect_closed(move |p| this.dismiss_popover(p));
        popover.popup();
    }

    fn connect_status_actions(self: &Rc<Self>) {
        let this = self.clone();
        let button = self.status.line_ending.clone();
        self.status.line_ending.connect_clicked(move |anchor| {
            let _ = &button;
            let options: Vec<MenuOption> = vec![
                (
                    "LF — Unix, macOS".to_owned(),
                    Box::new(|w: &Rc<Window>| w.set_line_ending(crate::text::LineEnding::Lf)),
                ),
                (
                    "CRLF — Windows".to_owned(),
                    Box::new(|w: &Rc<Window>| w.set_line_ending(crate::text::LineEnding::CrLf)),
                ),
                (
                    "CR — classic Mac".to_owned(),
                    Box::new(|w: &Rc<Window>| w.set_line_ending(crate::text::LineEnding::Cr)),
                ),
            ];
            this.status_menu(anchor, "Line endings, on save", options);
        });

        let this = self.clone();
        self.status.encoding.connect_clicked(move |anchor| {
            let options: Vec<MenuOption> = vec![
                (
                    "UTF-8".to_owned(),
                    Box::new(|w: &Rc<Window>| w.set_encoding("UTF-8", false)),
                ),
                (
                    "UTF-8 with BOM".to_owned(),
                    Box::new(|w: &Rc<Window>| w.set_encoding("UTF-8", true)),
                ),
                (
                    "Windows-1252".to_owned(),
                    Box::new(|w: &Rc<Window>| w.set_encoding("windows-1252", false)),
                ),
                (
                    "ISO-8859-1".to_owned(),
                    Box::new(|w: &Rc<Window>| w.set_encoding("ISO-8859-1", false)),
                ),
                (
                    "UTF-16 LE".to_owned(),
                    Box::new(|w: &Rc<Window>| w.set_encoding("UTF-16LE", true)),
                ),
            ];
            this.status_menu(anchor, "Encoding, on save", options);
        });
    }

    fn set_line_ending(self: &Rc<Self>, ending: crate::text::LineEnding) {
        let Some(doc) = self.current_document() else {
            return;
        };
        doc.set_line_ending(ending);
        self.status.set_document(&doc);
        // The file on disk still has the old endings; the change takes effect
        // when saved, so the tab is marked to say so.
        if let Some(buffer) = doc.buffer() {
            buffer.set_modified(true);
        }
        self.banner.info(
            &format!("Will be saved with {} line endings.", ending.label()),
            Level::Info,
        );
    }

    fn set_encoding(self: &Rc<Self>, name: &str, bom: bool) {
        let Some(doc) = self.current_document() else {
            return;
        };
        doc.set_encoding(name, bom);
        self.status.set_document(&doc);
        if let Some(buffer) = doc.buffer() {
            buffer.set_modified(true);
        }
        self.banner
            .info(&format!("Will be saved as {name}."), Level::Info);
    }

    // ---------------------------------------------------------- history

    /// Go back to an earlier version of this document.
    ///
    /// The snapshots have been written since the first release; this is the
    /// interface for them. It is the one thing here that neither Notepad++ nor
    /// a plain editor offers: the version list survives saving, so "I broke
    /// this an hour ago and saved it" is recoverable, which is exactly the
    /// case a save prompt cannot help with.
    fn open_history(self: &Rc<Self>) {
        let Some(doc) = self.current_document() else {
            return;
        };
        let store = DocStore::new(&self.state_root, &doc.store_key());
        let entries = store.history_entries().unwrap_or_default();

        if entries.is_empty() {
            self.banner.info(
                "No earlier versions of this document yet. They are kept as you work.",
                Level::Info,
            );
            return;
        }

        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Browse)
            .build();

        let now = std::time::SystemTime::now();
        // Newest first: the version someone wants back is almost always a
        // recent one.
        for entry in entries.iter().rev() {
            let when = gtk::Label::builder()
                .label(entry.age(now))
                .xalign(0.0)
                .build();
            let size = gtk::Label::builder()
                .label(format!(
                    "{:.1} KB compressed",
                    entry.stored_bytes as f64 / 1024.0
                ))
                .xalign(0.0)
                .build();
            size.add_css_class("path");

            let row_box = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .build();
            row_box.append(&when);
            row_box.append(&size);
            let row = gtk::ListBoxRow::builder().child(&row_box).build();
            unsafe { row.set_data("f3note-history-seq", entry.sequence) };
            list.append(&row);
        }
        if let Some(first) = list.row_at_index(0) {
            list.select_row(Some(&first));
        }

        let scroller = gtk::ScrolledWindow::builder()
            .child(&list)
            .min_content_height(220)
            .max_content_height(360)
            .propagate_natural_height(true)
            .build();

        let heading = gtk::Label::builder()
            .label(format!("{} — earlier versions", doc.title()))
            .xalign(0.0)
            .build();
        heading.add_css_class("path");

        let layout = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .build();
        layout.append(&heading);
        layout.append(&scroller);

        let popover = gtk::Popover::builder()
            .child(&layout)
            .autohide(true)
            .build();
        self.anchor_popover(&popover);

        let restore = {
            let this = self.clone();
            let popover = popover.clone();
            let doc = doc.clone();
            move |row: &gtk::ListBoxRow| {
                let sequence = unsafe { row.data::<u64>("f3note-history-seq") };
                popover.popdown();
                let Some(sequence) = sequence else { return };
                let sequence = unsafe { *sequence.as_ref() };
                this.restore_version(&doc, sequence);
            }
        };

        let restore_row = restore.clone();
        list.connect_row_activated(move |_, row| restore_row(row));

        let keys = gtk::EventControllerKey::new();
        let list_ref = list.clone();
        let popover_ref = popover.clone();
        keys.connect_key_pressed(move |_, key, _, _| match key {
            gdk::Key::Return | gdk::Key::KP_Enter => {
                if let Some(row) = list_ref.selected_row() {
                    restore(&row);
                }
                glib::Propagation::Stop
            }
            gdk::Key::Escape => {
                popover_ref.popdown();
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        });
        list.add_controller(keys);

        let this = self.clone();
        popover.connect_closed(move |p| this.dismiss_popover(p));

        popover.popup();
        list.grab_focus();
    }

    /// Put an earlier version into the buffer.
    ///
    /// As one undo step, and the current contents are mirrored first, so the
    /// version being replaced becomes history in its own right. Going back is
    /// therefore never a one-way door.
    fn restore_version(self: &Rc<Self>, doc: &Rc<Document>, sequence: u64) {
        let Some(buffer) = doc.buffer() else { return };
        let store = DocStore::new(&self.state_root, &doc.store_key());

        let entries = store.history_entries().unwrap_or_default();
        let Some(entry) = entries.iter().find(|e| e.sequence == sequence) else {
            self.banner
                .info("That version is no longer available.", Level::Warning);
            return;
        };

        let bytes = match store.read_history(entry) {
            Ok(bytes) => bytes,
            Err(e) => {
                self.banner
                    .info(&format!("Could not read that version: {e}"), Level::Error);
                return;
            }
        };

        // Keep what is about to be replaced, so this is reversible even after
        // the undo stack is gone.
        let limits = self.engine.config().editor;
        let current = doc.encode(&Self::buffer_text(&buffer));
        let _ = store.write_mirror(&current);
        if current.len() as u64 <= limits.history_max_bytes {
            let _ = store.push_history(
                &current,
                limits.history_versions,
                limits.history_max_total_bytes,
            );
        }

        let age = entry.age(std::time::SystemTime::now());
        let decoded =
            crate::text::decode_with_limit(&bytes, self.engine.config().appearance.long_line_chars);

        buffer.begin_user_action();
        let (mut start, mut end) = buffer.bounds();
        buffer.delete(&mut start, &mut end);
        buffer.insert(&mut start, &decoded.text);
        buffer.end_user_action();
        buffer.set_modified(true);

        self.banner.info(
            &format!("Restored the version from {age}. Ctrl+Z puts it back."),
            Level::Info,
        );
    }

    // ------------------------------------------------------ Ctrl+P switcher

    /// Jump to a tab by typing part of its name.
    ///
    /// This is what makes a lot of open tabs workable. Past a dozen or so,
    /// nobody reads the tab bar any more; they type three letters.
    fn open_switcher(self: &Rc<Self>) {
        let entry = gtk::SearchEntry::builder()
            .placeholder_text("Go to tab")
            .width_chars(36)
            .build();
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Browse)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .child(&list)
            .min_content_height(220)
            .max_content_height(360)
            .propagate_natural_height(true)
            .build();

        let layout = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .build();
        layout.append(&entry);
        layout.append(&scroller);

        let popover = gtk::Popover::builder()
            .child(&layout)
            .autohide(true)
            .build();
        self.anchor_popover(&popover);

        // Open tabs first, in most-recently-used order, so an empty query
        // already shows the tab you most likely want. Recently closed files
        // follow, so the same keystroke reopens something you closed — there
        // is no reason to make that a second shortcut to remember.
        //
        // An entry with an id is a tab to switch to; one without is a file to
        // open.
        let candidates: Vec<(Option<DocumentId>, String, String)> = {
            let mru = self.mru.borrow();
            let docs = self.docs.borrow();
            let mut ordered: Vec<(Option<DocumentId>, String, String)> = mru
                .as_slice()
                .iter()
                .filter_map(|id| docs.iter().find(|d| d.id == *id))
                .map(|d| (Some(d.id), d.title(), d.describe()))
                .collect();
            for d in docs.iter() {
                if !ordered.iter().any(|(id, _, _)| *id == Some(d.id)) {
                    ordered.push((Some(d.id), d.title(), d.describe()));
                }
            }

            let open_paths: Vec<_> = docs.iter().filter_map(|d| d.path()).collect();
            for path in crate::recent::Recent::new(&self.state_root).existing() {
                if open_paths.contains(&path) {
                    continue;
                }
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                ordered.push((None, name, path.display().to_string()));
            }
            ordered
        };

        let refill = {
            let list = list.clone();
            let candidates = candidates.clone();
            move |query: &str| {
                while let Some(child) = list.first_child() {
                    list.remove(&child);
                }
                let ranked = crate::fuzzy::rank(query, &candidates, |c| (c.1.clone(), c.2.clone()));
                for (item, _) in ranked.iter().take(200) {
                    let name = gtk::Label::builder().label(&item.1).xalign(0.0).build();
                    let path = gtk::Label::builder()
                        .label(if item.0.is_some() {
                            item.2.clone()
                        } else {
                            format!("{}  ·  not open", item.2)
                        })
                        .xalign(0.0)
                        .ellipsize(gtk::pango::EllipsizeMode::Start)
                        .build();
                    path.add_css_class("path");
                    let row_box = gtk::Box::builder()
                        .orientation(gtk::Orientation::Vertical)
                        .build();
                    row_box.append(&name);
                    row_box.append(&path);
                    let row = gtk::ListBoxRow::builder().child(&row_box).build();
                    // What to do travels with the row, so activation does not
                    // depend on the list's current ordering.
                    match item.0 {
                        Some(id) => unsafe { row.set_data("f3note-doc-id", id) },
                        None => unsafe { row.set_data("f3note-path", item.2.clone()) },
                    }
                    list.append(&row);
                }
                if let Some(first) = list.row_at_index(0) {
                    list.select_row(Some(&first));
                }
            }
        };
        refill("");

        let refill_cb = refill.clone();
        entry.connect_search_changed(move |e| refill_cb(e.text().as_str()));

        // GtkSearchEntry consumes Escape to emit `stop-search`, so a key
        // controller on the popover never sees it and the switcher could only
        // be dismissed by clicking away. Connecting the signal the widget
        // actually emits is the way to hear it. The go-to-line popover uses a
        // plain GtkEntry, which does not consume Escape — which is exactly why
        // that one closed and this one did not.
        let popover_ref = popover.clone();
        entry.connect_stop_search(move |_| popover_ref.popdown());

        let activate = {
            let this = self.clone();
            let popover = popover.clone();
            move |row: &gtk::ListBoxRow| {
                popover.popdown();
                if let Some(id) = unsafe { row.data::<DocumentId>("f3note-doc-id") } {
                    let id = unsafe { *id.as_ref() };
                    if let Some(index) = this.index_of(id) {
                        this.select_tab(index);
                    }
                    return;
                }
                if let Some(path) = unsafe { row.data::<String>("f3note-path") } {
                    let path = unsafe { path.as_ref().clone() };
                    this.open_path(std::path::PathBuf::from(path));
                }
            }
        };

        let activate_row = activate.clone();
        list.connect_row_activated(move |_, row| activate_row(row));

        // Enter in the entry activates whatever is selected in the list, so the
        // user never has to move focus down to it.
        let list_ref = list.clone();
        let activate_entry = activate.clone();
        entry.connect_activate(move |_| {
            if let Some(row) = list_ref.selected_row() {
                activate_entry(&row);
            }
        });

        // Up and down move the selection while the caret stays in the entry.
        let keys = gtk::EventControllerKey::new();
        let list_ref = list.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            let delta = match key {
                gdk::Key::Down => 1,
                gdk::Key::Up => -1,
                _ => return glib::Propagation::Proceed,
            };
            let current = list_ref.selected_row().map(|r| r.index()).unwrap_or(0);
            if let Some(next) = list_ref.row_at_index(current + delta) {
                list_ref.select_row(Some(&next));
            }
            glib::Propagation::Stop
        });
        entry.add_controller(keys);

        let this = self.clone();
        popover.connect_closed(move |p| this.dismiss_popover(p));

        popover.popup();
        entry.grab_focus();
    }

    fn goto_tab_index(self: &Rc<Self>, index: usize) {
        self.select_tab(index);
    }

    // -------------------------------------------------- find next/previous

    fn find_step(self: &Rc<Self>, forward: bool) {
        let Some(context) = self.search.borrow().clone() else {
            return;
        };
        let Some(buffer) = self.current_document().and_then(|d| d.buffer()) else {
            return;
        };
        let start = buffer.iter_at_mark(&buffer.get_insert());
        let found = if forward {
            context.forward(&start)
        } else {
            context.backward(&start)
        };
        if let Some((mut match_start, match_end, _)) = found {
            buffer.select_range(&match_start, &match_end);
            if let Some(view) = self.current_view() {
                view.scroll_to_iter(&mut match_start, 0.0, true, 0.0, 0.3);
            }
            let position = context.occurrence_position(&match_start, &match_end);
            self.findbar
                .set_match_count(position, context.occurrences_count());
        } else {
            self.findbar.set_match_count(0, 0);
        }
    }

    /// Replace every match, returning how many were replaced.
    ///
    /// The `sourceview5` binding for this cannot be used. It declares the
    /// result as a success flag:
    ///
    /// ```ignore
    /// assert_eq!(is_ok == 0, !error.is_null());
    /// ```
    ///
    /// but `gtk_source_search_context_replace_all` returns a `guint` count of
    /// replacements, not a boolean. Replacing nothing returns 0 with no error
    /// set, the assertion fails, and the process aborts — so searching for
    /// something absent and asking to replace it crashes the editor. Calling
    /// the C function directly gives the correct semantics, and the count is
    /// what we want to report anyway. Worth sending upstream.
    fn replace_all(
        context: &sourceview5::SearchContext,
        replacement: &str,
    ) -> Result<u32, glib::Error> {
        use gtk::glib::translate::{from_glib_full, ToGlibPtr};
        unsafe {
            let mut error = std::ptr::null_mut();
            let replaced = sourceview5::ffi::gtk_source_search_context_replace_all(
                context.to_glib_none().0,
                replacement.to_glib_none().0,
                replacement.len() as i32,
                &mut error,
            );
            if error.is_null() {
                Ok(replaced)
            } else {
                Err(from_glib_full(error))
            }
        }
    }

    fn replace_current(self: &Rc<Self>, all: bool) {
        let Some(context) = self.search.borrow().clone() else {
            return;
        };
        let replacement = self.findbar.replacement.text();
        if all {
            if self.findbar.query.text().is_empty() {
                return;
            }
            // One undo step for the whole operation. Without this, undoing a
            // replace-all means pressing Ctrl+Z once per occurrence, which on
            // a file with fifty matches is not undo in any useful sense.
            let buffer = self.current_document().and_then(|d| d.buffer());
            if let Some(b) = &buffer {
                b.begin_user_action();
            }
            let outcome = Self::replace_all(&context, replacement.as_str());
            if let Some(b) = &buffer {
                b.end_user_action();
            }
            match outcome {
                Ok(0) => self.banner.info("Nothing to replace", Level::Info),
                Ok(1) => self.banner.info("1 occurrence replaced", Level::Info),
                Ok(n) => self
                    .banner
                    .info(&format!("{n} occurrences replaced"), Level::Info),
                Err(e) => self.banner.info(&format!("{e}"), Level::Error),
            }
            return;
        }
        let Some(buffer) = self.current_document().and_then(|d| d.buffer()) else {
            return;
        };
        if let Some((mut start, mut end)) = buffer.selection_bounds() {
            let _ = context.replace(&mut start, &mut end, replacement.as_str());
        }
        self.find_step(true);
    }

    // ---------------------------------------------------- test entry points
    //
    // The window drives itself from signals and actions, which a test cannot
    // press. These expose the same operations directly so `src/bin/uitest.rs`
    // can exercise the real widget tree — the layer where the panic on closing
    // a tab lived, and which the unit tests could not reach.

    /// The text on a tab, and the minimum width its label asks for.
    ///
    /// Both matter. The text stayed correct throughout the bug that made every
    /// tab render as a bare "…" — what was wrong was the width the label was
    /// willing to shrink to, so a test that only checked the string would have
    /// passed while no tab was readable.
    pub fn tab_label_for_test(&self, index: usize) -> Option<(String, i32)> {
        let page = self.notebook.nth_page(Some(index as u32))?;
        let label = self.tab_label_widget(&page)?;
        let (minimum, _, _, _) = label.measure(gtk::Orientation::Horizontal, -1);
        Some((label.text().to_string(), minimum))
    }

    /// Names of the drop-handling controllers on the current text view.
    ///
    /// GtkTextView ships its own, which reached file drops first and pasted
    /// the path in as text. The test asserts exactly one survives — ours.
    pub fn drop_targets_for_test(&self) -> Vec<String> {
        let Some(view) = self.current_view() else {
            return Vec::new();
        };
        let controllers = view.observe_controllers();
        let mut names = Vec::new();
        for index in 0..controllers.n_items() {
            if let Some(object) = controllers.item(index) {
                let name = object.type_().name().to_string();
                if name.contains("DropTarget") {
                    names.push(name);
                }
            }
        }
        names
    }

    pub fn set_text_for_test(&self, text: &str) {
        if let Some(buffer) = self.current_document().and_then(|d| d.buffer()) {
            buffer.set_text(text);
        }
    }

    pub fn text_for_test(&self) -> String {
        self.current_document()
            .and_then(|d| d.buffer())
            .map(|b| {
                let (start, end) = b.bounds();
                b.text(&start, &end, true).to_string()
            })
            .unwrap_or_default()
    }

    pub fn select_lines_for_test(&self, first: i32, last: i32) {
        let Some(buffer) = self.current_document().and_then(|d| d.buffer()) else {
            return;
        };
        let Some(start) = buffer.iter_at_line(first) else {
            return;
        };
        let end = buffer
            .iter_at_line(last + 1)
            .unwrap_or_else(|| buffer.end_iter());
        buffer.select_range(&start, &end);
    }

    pub fn place_cursor_on_line_for_test(&self, line: i32) {
        let Some(buffer) = self.current_document().and_then(|d| d.buffer()) else {
            return;
        };
        if let Some(iter) = buffer.iter_at_line(line) {
            buffer.place_cursor(&iter);
        }
    }

    pub fn duplicate_lines_for_test(self: &Rc<Self>) {
        self.duplicate_lines();
    }

    pub fn move_lines_for_test(self: &Rc<Self>, delta: i32) {
        self.move_lines(delta);
    }

    pub fn toggle_comment_for_test(self: &Rc<Self>) {
        self.toggle_comment();
    }

    pub fn undo_for_test(&self) {
        if let Some(buffer) = self.current_document().and_then(|d| d.buffer()) {
            buffer.undo();
        }
    }

    pub fn tab_count(&self) -> usize {
        self.docs.borrow().len()
    }

    pub fn refresh_tab_label_for_test(&self, doc: &Rc<Document>) {
        self.refresh_tab_label(doc);
    }

    pub fn close_current_tab(self: &Rc<Self>) {
        if let Some(doc) = self.current_document() {
            self.close_document(doc.id);
        }
    }

    pub fn select_tab_for_test(self: &Rc<Self>, index: usize) {
        self.select_tab(index);
    }

    pub fn cycle_tab_for_test(self: &Rc<Self>, direction: i32) {
        self.cycle_tab(direction);
    }

    pub fn open_find_for_test(self: &Rc<Self>, with_replace: bool) {
        self.open_find(with_replace);
    }

    pub fn force_capabilities_for_test(self: &Rc<Self>) {
        if let Some(doc) = self.current_document() {
            self.force_capabilities(doc.id);
        }
    }

    pub fn scroll_to_end_for_test(self: &Rc<Self>) {
        if let Some(buffer) = self.current_document().and_then(|d| d.buffer()) {
            let end = buffer.end_iter();
            buffer.place_cursor(&end);
            if let Some(view) = self.current_view() {
                view.scroll_to_mark(&buffer.get_insert(), 0.0, true, 0.0, 0.0);
            }
        }
    }

    /// Step the caret along the current line, forcing the view to work out an
    /// x position at many points — which is what horizontal scrolling and
    /// clicking around in a long line do.
    pub fn walk_caret_for_test(self: &Rc<Self>, steps: i32) {
        let Some(buffer) = self.current_document().and_then(|d| d.buffer()) else {
            return;
        };
        let total = buffer.char_count();
        if total == 0 {
            return;
        }
        for step in 0..steps {
            let offset = (total / steps.max(1)) * step;
            let iter = buffer.iter_at_offset(offset);
            buffer.place_cursor(&iter);
            if let Some(view) = self.current_view() {
                view.scroll_to_mark(&buffer.get_insert(), 0.0, true, 0.0, 0.0);
            }
        }
    }

    pub fn zoom_for_test(&self, delta: i32) {
        self.bump_zoom(delta);
    }

    pub fn goto_line_for_test(self: &Rc<Self>) {
        self.goto_line();
    }

    pub fn jump_to_line_for_test(self: &Rc<Self>, line: i32) {
        self.jump_to_line(line);
    }

    pub fn save_session_as_for_test(self: &Rc<Self>, name: &str) {
        self.save_session_as(name);
    }

    pub fn switch_to_session_for_test(self: &Rc<Self>, name: &str) {
        self.switch_to_session(name);
    }

    pub fn open_sessions_for_test(self: &Rc<Self>) {
        self.open_sessions();
    }

    pub fn open_paths_for_test(&self) -> Vec<std::path::PathBuf> {
        self.docs.borrow().iter().filter_map(|d| d.path()).collect()
    }

    pub fn search_files_for_test(self: &Rc<Self>) {
        self.search_files();
    }

    pub fn collect_matches_for_test(
        self: &Rc<Self>,
        query: &str,
    ) -> Vec<(std::path::PathBuf, crate::search::Match)> {
        self.collect_matches(query)
    }

    pub fn open_history_for_test(self: &Rc<Self>) {
        self.open_history();
    }

    /// Restore the oldest version this document has, and report whether there
    /// was one to restore.
    pub fn restore_oldest_version_for_test(self: &Rc<Self>) -> bool {
        let Some(doc) = self.current_document() else {
            return false;
        };
        let store = crate::session::store::DocStore::new(&self.state_root, &doc.store_key());
        let Some(entry) = store
            .history_entries()
            .unwrap_or_default()
            .into_iter()
            .next()
        else {
            return false;
        };
        self.restore_version(&doc, entry.sequence);
        true
    }

    pub fn store_key_for_test(&self) -> Option<String> {
        self.current_document().map(|d| d.store_key())
    }

    pub fn state_root_for_test(&self) -> std::path::PathBuf {
        self.state_root.clone()
    }

    pub fn open_switcher_for_test(self: &Rc<Self>) {
        self.open_switcher();
    }

    pub fn popover_is_open_for_test(&self) -> bool {
        self.popover
            .borrow()
            .as_ref()
            .map(|p| p.is_visible())
            .unwrap_or(false)
    }

    /// Fire what pressing Escape in the switcher fires.
    ///
    /// GtkSearchEntry turns the key into `stop-search` and consumes it, so a
    /// test cannot reach the behaviour through a key controller any more than
    /// the popover could. Emitting the signal exercises the same path the key
    /// takes.
    pub fn switcher_escape_for_test(&self) -> bool {
        fn find_entry(widget: &gtk::Widget) -> Option<gtk::SearchEntry> {
            if let Ok(entry) = widget.clone().downcast::<gtk::SearchEntry>() {
                return Some(entry);
            }
            let mut child = widget.first_child();
            while let Some(w) = child {
                if let Some(found) = find_entry(&w) {
                    return Some(found);
                }
                child = w.next_sibling();
            }
            None
        }

        let popover = self.popover.borrow().clone();
        let Some(popover) = popover else { return false };
        let Some(child) = popover.child() else {
            return false;
        };
        let Some(entry) = find_entry(&child) else {
            return false;
        };
        entry.emit_by_name::<()>("stop-search", &[]);
        true
    }

    pub fn find_step_for_test(self: &Rc<Self>, forward: bool) {
        self.find_step(forward);
    }

    pub fn replace_for_test(self: &Rc<Self>, all: bool) {
        self.replace_current(all);
    }

    pub fn save_for_test(self: &Rc<Self>) {
        self.save();
    }

    pub fn set_find_query_for_test(&self, text: &str) {
        self.findbar.query.set_text(text);
    }

    pub fn set_replacement_for_test(&self, text: &str) {
        self.findbar.replacement.set_text(text);
    }

    /// Close the find bar and put the caret back in the document.
    pub fn close_find(self: &Rc<Self>) {
        if !self.findbar.is_open() {
            return;
        }
        self.findbar.close();

        // Clearing the search term also clears the highlighting. Leaving every
        // match lit up after the bar is gone looks like the editor is still
        // searching, and there is then no visible way to turn it off. The
        // text itself stays in the entry, so reopening with Ctrl+F still
        // offers the previous search.
        let context = self.search.borrow().clone();
        if let Some(context) = context {
            context.settings().set_search_text(None);
        }

        if let Some(view) = self.current_view() {
            view.grab_focus();
        }
    }

    fn open_find(self: &Rc<Self>, with_replace: bool) {
        let seed = self
            .current_document()
            .and_then(|d| d.buffer())
            .and_then(|b| {
                let (start, end) = b.selection_bounds()?;
                Some(b.text(&start, &end, false).to_string())
            });
        if let Some(buffer) = self.current_document().and_then(|d| d.buffer()) {
            *self.search.borrow_mut() = Some(self.findbar.attach(&buffer));
        }
        self.findbar.open(with_replace, seed.as_deref());
        self.run_search();
    }
}
