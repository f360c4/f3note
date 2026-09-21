//! Operations on lines, and zoom.

use super::*;

impl Window {
    // ---------------------------------------------------------------- zoom

    pub(super) fn bump_zoom(&self, delta: i32) {
        let next = (self.zoom.get() + delta).clamp(ZOOM_MIN, ZOOM_MAX);
        if next == self.zoom.get() {
            return;
        }
        self.zoom.set(next);
        self.apply_zoom();
    }

    pub(super) fn apply_zoom(&self) {
        let theme = self.engine.theme();
        let size = (theme.font.size + self.zoom.get()).max(4);
        let css = format!(
            "window.f3note textview {{ font-family: {}; font-size: {}pt; }}",
            theme.font.family, size
        );
        self.zoom_provider().load_from_string(&css);
    }

    pub(super) fn zoom_provider(&self) -> gtk::CssProvider {
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

    pub(super) fn cycle_tab(self: &Rc<Self>, direction: i32) {
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

    // ---------------------------------------------------- line operations

    /// The lines a line command should act on, from the current selection.
    ///
    /// Returns the first and last line, plus iterators spanning them whole
    /// including the trailing newline where there is one.
    pub(super) fn selected_lines(
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
    pub(super) fn duplicate_lines(self: &Rc<Self>) {
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
    pub(super) fn move_lines(self: &Rc<Self>, delta: i32) {
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
    pub(super) fn toggle_comment(self: &Rc<Self>) {
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
}
