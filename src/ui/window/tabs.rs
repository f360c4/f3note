//! Tabs: creating them, closing them, switching between them, and
//! building their contents the first time they are shown.

use super::*;

impl Window {
    // ---------------------------------------------------------------- tabs

    pub(super) fn allocate_id(&self) -> DocumentId {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        id
    }

    /// The placeholder a tab holds until it is first shown. Cheap on purpose:
    /// this is what makes restoring a large session fast.
    pub(super) fn new_page_container() -> gtk::Box {
        gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build()
    }

    pub(super) fn build_tab_label(self: &Rc<Self>, doc: &Rc<Document>) -> gtk::Box {
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

    pub(super) fn tab_label_widget(&self, page: &gtk::Widget) -> Option<gtk::Label> {
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

    pub(super) fn refresh_tab_label(&self, doc: &Rc<Document>) {
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

    pub(super) fn update_window_title(&self, doc: &Rc<Document>) {
        let marker = if doc.is_modified() { "*" } else { "" };
        self.window
            .set_title(Some(&format!("{marker}{} — f3note", doc.title())));
    }

    pub(super) fn index_of(&self, id: DocumentId) -> Option<usize> {
        self.docs.borrow().iter().position(|d| d.id == id)
    }

    pub(super) fn document_at(&self, index: usize) -> Option<Rc<Document>> {
        self.docs.borrow().get(index).cloned()
    }

    pub fn current_document(&self) -> Option<Rc<Document>> {
        let index = self.notebook.current_page()? as usize;
        self.document_at(index)
    }

    /// Add a document to the notebook without materialising it.
    pub(super) fn add_document(self: &Rc<Self>, doc: Rc<Document>, focus: bool) {
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

    pub(super) fn close_document(self: &Rc<Self>, id: DocumentId) {
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
    pub(super) fn materialise(self: &Rc<Self>, doc: &Rc<Document>) {
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
        self.watch_overwrite(&view);
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
    pub(super) fn apply_document_capabilities(
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
    pub(super) fn suppress_hyphens(buffer: &sourceview5::Buffer) {
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

    pub(super) fn announce_capabilities(self: &Rc<Self>, doc: &Rc<Document>) {
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
    pub(super) fn force_capabilities(self: &Rc<Self>, id: DocumentId) {
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

    pub(super) fn view_for(&self, index: usize) -> Option<sourceview5::View> {
        let page = self.notebook.nth_page(Some(index as u32))?;
        let container = page.downcast::<gtk::Box>().ok()?;
        let scroller = container
            .first_child()?
            .downcast::<gtk::ScrolledWindow>()
            .ok()?;
        scroller.child()?.downcast::<sourceview5::View>().ok()
    }

    pub(super) fn current_view(&self) -> Option<sourceview5::View> {
        self.view_for(self.notebook.current_page()? as usize)
    }

    // ------------------------------------------------------ Ctrl+P switcher

    /// Jump to a tab by typing part of its name.
    ///
    /// This is what makes a lot of open tabs workable. Past a dozen or so,
    /// nobody reads the tab bar any more; they type three letters.
    pub(super) fn open_switcher(self: &Rc<Self>) {
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

    pub(super) fn goto_tab_index(self: &Rc<Self>, index: usize) {
        self.select_tab(index);
    }
}
