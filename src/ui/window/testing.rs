//! Entry points for `src/bin/uitest.rs`.
//!
//! The window drives itself from signals and actions, which a test cannot
//! press. These expose the same operations directly, so the tests reach the
//! real widget tree — the layer where signal re-entry and borrow conflicts
//! live, and where every crash a user has found here originated.

use super::*;

impl Window {
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

    pub fn overwrite_for_test(&self) -> bool {
        self.current_view().map(|v| v.overwrites()).unwrap_or(false)
    }

    /// What the status bar is showing about overwrite mode.
    pub fn overwrite_indicator_for_test(&self) -> String {
        if !self.status.overwrite.is_visible() {
            return String::new();
        }
        self.status
            .overwrite
            .child()
            .and_then(|c| c.downcast::<gtk::Label>().ok())
            .map(|l| l.text().to_string())
            .unwrap_or_default()
    }

    pub fn set_overwrite_for_test(&self, on: bool) {
        if let Some(view) = self.current_view() {
            view.set_overwrite(on);
        }
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

    /// Type into the search panel's entry, for screenshots and tests.
    pub fn set_search_query_for_test(&self, query: &str) {
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
        if let Some(entry) = popover.and_then(|p| p.child()).and_then(|c| find_entry(&c)) {
            entry.set_text(query);
        }
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
}
