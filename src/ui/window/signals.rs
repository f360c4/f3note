//! Wiring: what happens when a buffer changes, a tab is switched, a key
//! is pressed, or a file is dropped.

use super::*;

impl Window {
    // ------------------------------------------------------------- signals

    pub(super) fn connect_buffer(
        self: &Rc<Self>,
        doc: &Rc<Document>,
        buffer: &sourceview5::Buffer,
    ) {
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

    /// Keep the status bar honest about overwrite mode.
    pub(super) fn watch_overwrite(self: &Rc<Self>, view: &sourceview5::View) {
        self.status.set_overwrite(view.overwrites());
        let this = self.clone();
        view.connect_overwrite_notify(move |v| {
            this.status.set_overwrite(v.overwrites());
            // Said out loud the first time it happens in a session, because
            // the symptom — text disappearing as you type — reads as a broken
            // editor rather than as a mode you switched into.
            if v.overwrites() && !this.warned_about_overwrite.replace(true) {
                this.banner.info(
                    "Overwrite mode: typing now replaces what is there. Press Insert to go back.",
                    Level::Warning,
                );
            }
        });
    }

    pub(super) fn connect_view(self: &Rc<Self>, view: &sourceview5::View) {
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
    pub(super) fn accept_dropped_files(self: Rc<Self>) {
        self.window.add_controller(self.file_drop_target());
    }

    /// A drop target that opens files rather than pasting their paths.
    pub(super) fn file_drop_target(self: &Rc<Self>) -> gtk::DropTarget {
        let drop = gtk::DropTarget::new(
            gdk::FileList::static_type(),
            gdk::DragAction::COPY | gdk::DragAction::MOVE,
        );
        let this = self.clone();
        drop.connect_drop(move |_, value, _, _| {
            let Ok(list) = value.get::<gdk::FileList>() else {
                return false;
            };
            let paths: Vec<std::path::PathBuf> =
                list.files().iter().filter_map(|f| f.path()).collect();
            if paths.is_empty() {
                return false;
            }

            // Opening is deferred by one turn of the main loop, and that is
            // not incidental. GTK finishes a drop by returning focus to the
            // widget that received it — the view of the tab the file was
            // dropped on — and GtkNotebook follows focus to the page that
            // widget lives in. Selecting the new tab during the drop is
            // therefore undone a moment later, which looked like the file
            // opening and then jumping back to the first tab.
            let this = this.clone();
            glib::idle_add_local_once(move || {
                for path in paths {
                    this.open_path(path);
                }
                this.window.present();
            });
            true
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
    pub(super) fn take_over_drops(self: &Rc<Self>, view: &sourceview5::View) {
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

    pub(super) fn connect_signals(self: &Rc<Self>) {
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
    pub(super) fn select_tab(self: &Rc<Self>, index: usize) {
        if index >= self.docs.borrow().len() {
            return;
        }
        self.suppress_switch.set(true);
        self.notebook.set_current_page(Some(index as u32));
        self.suppress_switch.set(false);
        self.activate_document(index);
    }

    /// Called whenever a different tab becomes current.
    pub(super) fn activate_document(self: &Rc<Self>, index: usize) {
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
    pub(super) fn revert(self: &Rc<Self>) {
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
    pub(super) fn unsaved_count(&self) -> usize {
        self.docs
            .borrow()
            .iter()
            .filter(|d| d.is_modified())
            .count()
    }

    pub(super) fn reload_document(self: &Rc<Self>, id: DocumentId) {
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

    pub(super) fn follow_theme(self: &Rc<Self>) {
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
}
