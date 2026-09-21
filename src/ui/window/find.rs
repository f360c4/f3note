//! Finding: the inline bar, go to line, and searching across files.

use super::*;

impl Window {
    // ------------------------------------------------------------ searching

    pub(super) fn run_search(&self) {
        let Some(context) = self.search.borrow().clone() else {
            return;
        };
        let text = self.findbar.query.text();
        context.settings().set_search_text(Some(text.as_str()));
        self.findbar
            .set_match_count(-1, context.occurrences_count());
    }

    pub(super) fn install_actions(self: Rc<Self>, app: &gtk::Application) {
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

    pub(super) fn connect_findbar(self: &Rc<Self>) {
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
    pub(super) fn take_popover(&self) -> Option<gtk::Popover> {
        self.popover.borrow_mut().take()
    }

    pub(super) fn set_popover(&self, popover: Option<gtk::Popover>) {
        *self.popover.borrow_mut() = popover;
    }

    pub(super) fn popover_is(&self, popover: &gtk::Popover) -> bool {
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
    pub(super) fn escape_closes(popover: &gtk::Popover) {
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

    pub(super) fn anchor_popover(&self, popover: &gtk::Popover) {
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

        // Point at the notebook's full width so GTK centres the popover over
        // it. Pointing at a single pixel in the middle looked equivalent and
        // was not: the width is read before the widget has been allocated in
        // some paths, and a width of zero put the popover against the left
        // edge. Screenshots of the same command taken moments apart landed in
        // different places, which is how this was noticed.
        let width = self.notebook.width();
        let width = if width > 0 {
            width
        } else {
            self.window.width().max(1)
        };
        popover.set_pointing_to(Some(&gdk::Rectangle::new(0, 0, width, 1)));
        popover.add_css_class("f3note-switcher");
        Self::escape_closes(popover);
    }

    pub(super) fn goto_line(self: &Rc<Self>) {
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
    pub(super) fn dismiss_popover(&self, popover: &gtk::Popover) {
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

    pub(super) fn jump_to_line(self: &Rc<Self>, line: i32) {
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

    // --------------------------------------------- search across files

    /// Search the open tabs and the current file's folder.
    ///
    /// Scope is deliberately modest. This is a notepad; `ripgrep` exists and
    /// is better at searching a codebase. What is worth having here is finding
    /// the thing you know is in one of the files you are working on, without
    /// leaving the editor to do it.
    pub(super) fn search_files(self: &Rc<Self>) {
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
    pub(super) fn collect_matches(
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

    // -------------------------------------------------- find next/previous

    pub(super) fn find_step(self: &Rc<Self>, forward: bool) {
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
    pub(super) fn replace_all(
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

    pub(super) fn replace_current(self: &Rc<Self>, all: bool) {
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

    pub(super) fn open_find(self: &Rc<Self>, with_replace: bool) {
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
