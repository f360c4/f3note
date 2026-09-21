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
        });

        this.clone().connect_signals();
        this.clone().install_actions(app);
        this.clone().follow_theme();
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
                    let _ = store.push_history(&contents, limits.history_versions);
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
        let label = gtk::Label::builder()
            .label(doc.tab_label())
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .max_width_chars(22)
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

        let loaded = match doc.load() {
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
                    let mut from_mirror = crate::text::decode(&bytes);
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
        view.set_wrap_mode(if reduced || !config.appearance.wrap {
            gtk::WrapMode::None
        } else {
            gtk::WrapMode::WordChar
        });

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
                    this.findbar.close();
                    if let Some(view) = this.current_view() {
                        view.grab_focus();
                    }
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
        match doc.load() {
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
        add(
            "close-and-forget",
            &["<Control><Shift>w"],
            Box::new(|w| w.close_and_forget()),
        );
        add("save", &["<Control>s"], Box::new(|w| w.save()));
        add("save-as", &["<Control><Shift>s"], Box::new(|w| w.save_as()));
        add("goto-line", &["<Control>g"], Box::new(|w| w.goto_line()));
        add("switcher", &["<Control>p"], Box::new(|w| w.open_switcher()));
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

    // ------------------------------------------------------- go to line

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

        // A small popover at the caret, not a modal dialog: the point of an
        // inline editor is that nothing blocks.
        let popover = gtk::Popover::builder().child(&entry).autohide(true).build();
        popover.set_parent(&self.window);
        popover.add_css_class("f3note-switcher");

        let this = self.clone();
        let popover_ref = popover.clone();
        entry.connect_activate(move |e| {
            if let Ok(line) = e.text().trim().parse::<i32>() {
                this.jump_to_line(line - 1);
            }
            popover_ref.popdown();
        });
        let popover_ref = popover.clone();
        popover.connect_closed(move |_| {
            popover_ref.unparent();
        });

        popover.popup();
        entry.grab_focus();
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
        popover.set_parent(&self.window);
        popover.add_css_class("f3note-switcher");

        // Candidates start in most-recently-used order, so an empty query
        // already shows the tab you most likely want.
        let candidates: Vec<(DocumentId, String, String)> = {
            let mru = self.mru.borrow();
            let docs = self.docs.borrow();
            let mut ordered: Vec<(DocumentId, String, String)> = mru
                .as_slice()
                .iter()
                .filter_map(|id| docs.iter().find(|d| d.id == *id))
                .map(|d| (d.id, d.title(), d.describe()))
                .collect();
            for d in docs.iter() {
                if !ordered.iter().any(|(id, _, _)| *id == d.id) {
                    ordered.push((d.id, d.title(), d.describe()));
                }
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
                        .label(&item.2)
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
                    // The document id travels with the row so activation does
                    // not depend on the list's current ordering.
                    unsafe { row.set_data("f3note-doc-id", item.0) };
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

        let activate = {
            let this = self.clone();
            let popover = popover.clone();
            move |row: &gtk::ListBoxRow| {
                let id = unsafe { row.data::<DocumentId>("f3note-doc-id") };
                if let Some(id) = id {
                    let id = unsafe { *id.as_ref() };
                    if let Some(index) = this.index_of(id) {
                        this.select_tab(index);
                    }
                }
                popover.popdown();
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

        let popover_ref = popover.clone();
        popover.connect_closed(move |_| popover_ref.unparent());

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

    fn replace_current(self: &Rc<Self>, all: bool) {
        let Some(context) = self.search.borrow().clone() else {
            return;
        };
        let replacement = self.findbar.replacement.text();
        if all {
            // The count has to be read before replacing: afterwards the
            // context reports how many of the *replacement* text it finds,
            // which is not what the user wants to be told.
            let total = context.occurrences_count();
            match context.replace_all(replacement.as_str()) {
                Ok(()) => {
                    let n = total.max(0);
                    self.banner.info(
                        &if n == 1 {
                            "1 occurrence replaced".to_owned()
                        } else {
                            format!("{n} occurrences replaced")
                        },
                        Level::Info,
                    );
                }
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

    pub fn tab_count(&self) -> usize {
        self.docs.borrow().len()
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

    pub fn zoom_for_test(&self, delta: i32) {
        self.bump_zoom(delta);
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
