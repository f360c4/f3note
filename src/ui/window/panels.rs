//! The popovers — sessions, version history, and the menus hanging off
//! the status bar.

use super::*;

impl Window {
    // ------------------------------------------------- named sessions

    /// Save and restore named sets of tabs.
    ///
    /// The point is switching context without losing one. Working on a config
    /// change, then a note, then back — each is a handful of files, and
    /// keeping them all open at once turns the tab bar into a haystack.
    /// Switching sessions never loses anything: the tabs being closed are
    /// mirrored like any others, and the session you leave is saved first.
    pub(super) fn open_sessions(self: &Rc<Self>) {
        let open_count = self.docs.borrow().len();
        let entry = gtk::Entry::builder()
            .placeholder_text(match open_count {
                1 => "Name for this tab, then Enter".to_owned(),
                n => format!("Name for these {n} tabs, then Enter"),
            })
            .activates_default(true)
            .build();

        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Browse)
            .build();

        let popover = gtk::Popover::builder().autohide(true).build();

        let names = crate::session::index::list_named(&self.state_root);
        if names.is_empty() {
            // The empty state has to teach, not just report. A list saying
            // "none" above a text box is a puzzle: a user reached this screen,
            // saw nothing, and could not work out that the box was how you
            // make the first one.
            let empty = gtk::Label::builder()
                .label(
                    "A session is a set of tabs with a name.\n\n\
                     Save the tabs you have open under a name below, then \
                     open other files and save those under another. \
                     Picking a name here closes what is open and brings \
                     that set back — nothing is lost either way, unsaved \
                     changes included.",
                )
                .xalign(0.0)
                .wrap(true)
                .max_width_chars(46)
                .build();
            empty.add_css_class("path");
            let row = gtk::ListBoxRow::builder()
                .child(&empty)
                .selectable(false)
                .activatable(false)
                .build();
            list.append(&row);
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

        let heading = gtk::Label::builder()
            .label(if names.is_empty() {
                "Sessions"
            } else {
                "Sessions — pick one to switch, or name these tabs below"
            })
            .xalign(0.0)
            .build();
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
    pub(super) fn current_session(&self) -> crate::session::index::Session {
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

    pub(super) fn save_session_as(self: &Rc<Self>, name: &str) {
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
    pub(super) fn switch_to_session(self: &Rc<Self>, name: &str) {
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

    // ------------------------------------------------ status bar actions

    /// One entry in a status-bar menu: what it says, and what it does.
    pub(super) fn status_menu(
        self: &Rc<Self>,
        anchor: &gtk::Button,
        title: &str,
        options: Vec<MenuOption>,
    ) {
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

    pub(super) fn connect_status_actions(self: &Rc<Self>) {
        // Clicking the indicator is the way out for someone who does not know
        // which key put them here.
        let this = self.clone();
        self.status.overwrite.connect_clicked(move |_| {
            if let Some(view) = this.current_view() {
                view.set_overwrite(false);
            }
        });

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

    pub(super) fn set_line_ending(self: &Rc<Self>, ending: crate::text::LineEnding) {
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

    pub(super) fn set_encoding(self: &Rc<Self>, name: &str, bom: bool) {
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
    pub(super) fn open_history(self: &Rc<Self>) {
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
                .label(format!("{} compressed", entry.size()))
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
    pub(super) fn restore_version(self: &Rc<Self>, doc: &Rc<Document>, sequence: u64) {
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
}
