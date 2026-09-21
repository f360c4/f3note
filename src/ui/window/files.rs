//! Opening, saving and reverting documents, and the session file that
//! remembers which ones were open.

use super::*;

impl Window {
    // ------------------------------------------------------------- session

    pub(super) fn save_session(self: &Rc<Self>) {
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
    pub(super) fn restore_from(self: &Rc<Self>, session: Session) -> bool {
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

    // ------------------------------------------------------------- saving

    /// Contents of a document's buffer, as the editor currently holds them.
    pub(super) fn buffer_text(buffer: &sourceview5::Buffer) -> String {
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

    pub(super) fn write_document(self: &Rc<Self>, doc: &Rc<Document>) {
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
}
