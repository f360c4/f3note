//! Driving the autosave: deciding when to mirror a buffer, and flushing
//! everything when the window closes.

use super::*;

impl Window {
    // ------------------------------------------------------------ autosave

    /// The heartbeat that decides when to mirror buffers.
    ///
    /// One second is the resolution, not the frequency of writing: the
    /// schedule decides what is actually due, and on an idle editor the answer
    /// is nothing, so this costs a comparison per second and no disk access.
    pub(super) fn start_autosave(self: Rc<Self>) {
        let this = self.clone();
        glib::timeout_add_local(std::time::Duration::from_secs(1), move || {
            this.autosave_tick();
            glib::ControlFlow::Continue
        });
    }

    pub(super) fn autosave_tick(self: &Rc<Self>) {
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
    pub(super) fn mirror_document(self: &Rc<Self>, id: DocumentId) {
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
    pub(super) fn flush_all(self: &Rc<Self>) {
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
    pub(super) fn guard_close(self: Rc<Self>) {
        let this = self.clone();
        self.window.connect_close_request(move |_| {
            this.flush_all();
            glib::Propagation::Proceed
        });
    }
}
