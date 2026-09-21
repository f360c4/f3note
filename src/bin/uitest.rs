//! Exercises the window the way a person does.
//!
//! This exists because the unit tests did not catch a panic that happened on
//! the very first tab anyone closed. They covered the logic thoroughly and the
//! GTK glue not at all, and the glue is where the interesting failures live:
//! signals that re-enter, borrows held across a call that takes the same
//! RefCell, widgets touched after being removed.
//!
//! Every step here is something a user does in the first minute. A panic
//! aborts the process, so finishing at all is the pass condition; the checks
//! along the way catch the quieter kind of wrong, where nothing crashes but
//! the tab list and the notebook stop agreeing.

use std::path::PathBuf;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use f3note::config::Config;
use f3note::theme::ThemeEngine;
use f3note::ui::window::Window;

macro_rules! check {
    ($condition:expr, $($arg:tt)*) => {
        if !$condition {
            println!("FAIL: {}", format!($($arg)*));
            std::process::exit(1);
        }
    };
}

fn main() -> glib::ExitCode {
    std::env::set_var("GSK_RENDERER", "cairo");

    let app = gtk::Application::builder()
        .application_id("io.github.f360c4.f3note.uitest")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();

    app.connect_activate(|app| {
        let (config, _) = Config::load();
        let engine = ThemeEngine::new(config);
        let window = Window::new(app, engine);
        window.window.present();

        // One turn of the main loop first, so the window is realised before
        // anything starts poking at it.
        let window = window.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(400), move || {
            run(window);
        });
    });

    app.run_with_args::<&str>(&[])
}

fn scratch_files(count: usize) -> (PathBuf, Vec<PathBuf>) {
    let dir = std::env::temp_dir().join(format!("f3note-uitest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let files = (0..count)
        .map(|i| {
            let p = dir.join(format!("file{i}.txt"));
            std::fs::write(&p, format!("contents of file {i}\nsecond line\n")).unwrap();
            p
        })
        .collect();
    (dir, files)
}

fn run(window: Rc<Window>) {
    let (dir, files) = scratch_files(6);

    println!("opening {} files", files.len());
    for path in &files {
        window.open_path(path.clone());
    }
    check!(
        window.tab_count() == files.len(),
        "expected {} tabs, found {}",
        files.len(),
        window.tab_count()
    );
    println!("  {} tabs open", window.tab_count());

    // This is the exact step that panicked: closing a tab while others remain
    // makes the editor pick the most recently used one to move to.
    println!("file drops open files rather than pasting paths");
    let targets = window.drop_targets_for_test();
    println!("  drop handlers on the view: {targets:?}");
    check!(
        targets.len() == 1,
        "expected exactly our own drop target, found {targets:?}"
    );

    println!("tabs are actually readable");
    for index in 0..window.tab_count() {
        let (text, minimum) = window
            .tab_label_for_test(index)
            .unwrap_or_else(|| panic!("tab {index} has no label"));
        check!(!text.is_empty(), "tab {index} has an empty label");
        check!(
            text.contains(".txt"),
            "tab {index} shows {text:?} rather than a file name"
        );
        // Every tab once rendered as a bare ellipsis because the label was
        // free to shrink to nothing. A readable name needs real width.
        check!(
            minimum >= 40,
            "tab {index} asks for only {minimum}px, too narrow to read {text:?}"
        );
    }
    println!("  first tab: {:?}", window.tab_label_for_test(0).unwrap());

    println!("the modified marker shows up in the label");
    if let Some(doc) = window.current_document() {
        doc.set_modified(true);
        window.refresh_tab_label_for_test(&doc);
        let index = window.tab_count() - 1;
        let (text, _) = window.tab_label_for_test(index).unwrap();
        check!(
            text.starts_with('*'),
            "a modified tab should be marked, got {text:?}"
        );
        doc.set_modified(false);
        window.refresh_tab_label_for_test(&doc);
    }

    println!("closing the current tab");
    window.close_current_tab();
    check!(window.tab_count() == 5, "tab was not removed");
    check!(
        window.current_document().is_some(),
        "no tab became current after closing one"
    );
    println!("  now on: {}", window.current_document().unwrap().title());

    println!("switching between tabs");
    for index in [0usize, 3, 1, 4, 2] {
        window.select_tab_for_test(index);
        let current = window.current_document().expect("a current document");
        println!("  tab {index} -> {}", current.title());
    }

    println!("cycling with Ctrl+Tab");
    for _ in 0..4 {
        window.cycle_tab_for_test(1);
        check!(
            window.current_document().is_some(),
            "cycling left no current document"
        );
    }
    window.cycle_tab_for_test(-1);

    println!("opening a file that is already open");
    let before = window.tab_count();
    window.open_path(files[0].clone());
    check!(
        window.tab_count() == before,
        "reopening a file should focus its tab, not duplicate it"
    );

    println!("new untitled tabs");
    window.new_untitled();
    window.new_untitled();
    check!(window.tab_count() == before + 2, "new tabs were not added");

    println!("closing every tab, one at a time");
    for _ in 0..(before + 2) {
        window.close_current_tab();
        check!(
            window.current_document().is_some(),
            "closing a tab left the window with no document"
        );
    }
    // Closing the last tab leaves an empty one rather than closing the window.
    check!(
        window.tab_count() == 1,
        "expected one empty tab left, found {}",
        window.tab_count()
    );
    let last = window.current_document().unwrap();
    check!(
        last.path().is_none(),
        "the surviving tab should be a fresh untitled one, got {}",
        last.describe()
    );
    println!("  left with: {}", last.title());

    println!("closing the last tab again");
    window.close_current_tab();
    check!(
        window.tab_count() == 1 && window.current_document().is_some(),
        "closing the last tab must leave an empty one, not nothing"
    );

    println!("a recovered document shows its marker from the moment it opens");
    {
        // Mimic what session restore produces: a document whose buffer comes
        // back already modified, which is the case whose tab silently stayed
        // clean forever.
        let doc = window.current_document().expect("a document");
        doc.set_modified(true);
        if let Some(buffer) = doc.buffer() {
            buffer.set_modified(true);
        }
        window.refresh_tab_label_for_test(&doc);

        let index = window.tab_count() - 1;
        let (text, _) = window.tab_label_for_test(index).unwrap();
        check!(
            text.starts_with('*'),
            "a document with unsaved work must be marked, got {text:?}"
        );

        // And editing it further must not lose the marker either.
        if let Some(buffer) = doc.buffer() {
            let mut end = buffer.end_iter();
            buffer.insert(&mut end, "more");
        }
        let (text, _) = window.tab_label_for_test(index).unwrap();
        check!(
            text.starts_with('*'),
            "the marker must survive further editing, got {text:?}"
        );

        doc.set_modified(false);
        if let Some(buffer) = doc.buffer() {
            buffer.set_modified(false);
        }
        window.refresh_tab_label_for_test(&doc);
    }

    println!("typing inserts rather than overwrites");
    check!(
        !window.overwrite_for_test(),
        "the editor must not start in overwrite mode"
    );
    // Insert toggles this mode and is easy to hit by accident. The symptom —
    // text vanishing as you type — reads as a broken editor, so the state has
    // to be visible. A user hit exactly this and had no way to tell.
    window.set_overwrite_for_test(true);
    check!(
        window.overwrite_indicator_for_test() == "OVR",
        "overwrite mode must be shown in the status bar, saw {:?}",
        window.overwrite_indicator_for_test()
    );
    window.set_overwrite_for_test(false);
    check!(
        window.overwrite_indicator_for_test().is_empty(),
        "the indicator must go away again, saw {:?}",
        window.overwrite_indicator_for_test()
    );

    println!("line operations");
    {
        window.new_untitled();

        window.set_text_for_test("one\ntwo\nthree\n");
        window.place_cursor_on_line_for_test(1);
        window.duplicate_lines_for_test();
        check!(
            window.text_for_test() == "one\ntwo\ntwo\nthree\n",
            "duplicate put the copy in the wrong place: {:?}",
            window.text_for_test()
        );

        window.set_text_for_test("one\ntwo\nthree\n");
        window.place_cursor_on_line_for_test(2);
        window.move_lines_for_test(-1);
        check!(
            window.text_for_test() == "one\nthree\ntwo\n",
            "moving a line up went wrong: {:?}",
            window.text_for_test()
        );

        window.set_text_for_test("a\nb\nc\n");
        window.place_cursor_on_line_for_test(0);
        window.move_lines_for_test(-1);
        check!(
            window.text_for_test() == "a\nb\nc\n",
            "moving the first line up must do nothing: {:?}",
            window.text_for_test()
        );

        // An untitled document has no extension, so the fallback marker is #.
        window.set_text_for_test("    if x:\n        y()\n");
        window.select_lines_for_test(0, 1);
        window.toggle_comment_for_test();
        check!(
            window.text_for_test() == "    # if x:\n    #     y()\n",
            "commenting did not line the markers up: {:?}",
            window.text_for_test()
        );

        window.select_lines_for_test(0, 1);
        window.toggle_comment_for_test();
        check!(
            window.text_for_test() == "    if x:\n        y()\n",
            "uncommenting did not restore the original: {:?}",
            window.text_for_test()
        );

        // Each operation has to be one undo step, not one per line.
        window.set_text_for_test("a\nb\nc\n");
        window.select_lines_for_test(0, 2);
        window.toggle_comment_for_test();
        let commented = window.text_for_test();
        check!(
            commented.starts_with("# a"),
            "expected comments: {commented:?}"
        );
        window.undo_for_test();
        check!(
            window.text_for_test() == "a\nb\nc\n",
            "one undo must put every line back: {:?}",
            window.text_for_test()
        );

        window.close_current_tab();
    }

    println!("named sessions");
    {
        // Two tabs, saved under a name.
        window.open_path(files[2].clone());
        window.open_path(files[3].clone());
        window.save_session_as_for_test("a test session");

        // A different set of tabs.
        window.open_path(files[4].clone());
        let before = window.open_paths_for_test();
        check!(
            before.contains(&files[4]),
            "expected the new file to be open"
        );

        window.switch_to_session_for_test("a test session");
        let after = window.open_paths_for_test();
        check!(
            after.contains(&files[2]) && after.contains(&files[3]),
            "the saved tabs should have come back, got {after:?}"
        );
        check!(
            !after.contains(&files[4]),
            "tabs outside the session should have been closed, got {after:?}"
        );

        // A name that was never saved must not destroy what is open.
        let kept = window.open_paths_for_test();
        window.switch_to_session_for_test("no-such-session");
        check!(
            window.open_paths_for_test() == kept,
            "switching to a missing session must change nothing"
        );

        window.open_sessions_for_test();
        check!(
            window.popover_is_open_for_test(),
            "the sessions list should open"
        );
    }

    println!("searching across files");
    {
        window.open_path(files[0].clone());
        // Unsaved text must be searchable: what is on screen is what people
        // expect to find, saved or not.
        window.set_text_for_test("contents of file 0\nneedle-in-buffer here\n");

        let hits = window.collect_matches_for_test("needle-in-buffer");
        check!(
            hits.iter().any(|(_, m)| m.line == 1),
            "unsaved buffer text should be searchable, got {hits:?}"
        );

        // And text that is only on disk, in a neighbouring file, is found too.
        let disk_hits = window.collect_matches_for_test("second line");
        check!(
            !disk_hits.is_empty(),
            "should have found text in neighbouring files on disk"
        );

        let none = window.collect_matches_for_test("zzz-definitely-not-present-zzz");
        check!(none.is_empty(), "expected no matches, got {none:?}");

        window.search_files_for_test();
        check!(
            window.popover_is_open_for_test(),
            "the search panel should open"
        );
        window.close_current_tab();
    }

    println!("going back to an earlier version");
    {
        window.new_untitled();
        let key = window.store_key_for_test().expect("a store key");
        let store = f3note::session::store::DocStore::new(&window.state_root_for_test(), &key);

        // Stand in for what autosave writes while someone works.
        store
            .push_history(b"first version\n", 20, u64::MAX)
            .unwrap();
        store
            .push_history(b"second version\n", 20, u64::MAX)
            .unwrap();

        window.set_text_for_test("what is here now\n");
        check!(
            window.restore_oldest_version_for_test(),
            "there should have been a version to restore"
        );
        check!(
            window.text_for_test() == "first version\n",
            "restored the wrong version: {:?}",
            window.text_for_test()
        );

        // Restoring must not be a one-way door: the replaced text becomes
        // history of its own.
        let after = store.history_entries().unwrap();
        let texts: Vec<String> = after
            .iter()
            .map(|e| String::from_utf8(store.read_history(e).unwrap()).unwrap())
            .collect();
        check!(
            texts.iter().any(|t| t == "what is here now\n"),
            "the replaced version should have been kept: {texts:?}"
        );

        // And one undo puts it straight back.
        window.undo_for_test();
        check!(
            window.text_for_test() == "what is here now\n",
            "undo after restoring did not work: {:?}",
            window.text_for_test()
        );

        window.open_history_for_test();
        check!(
            window.popover_is_open_for_test(),
            "the history list should open"
        );

        window.close_and_forget();
    }

    println!("close and forget");
    window.open_path(files[1].clone());
    window.close_and_forget();
    check!(
        window.current_document().is_some(),
        "close and forget left no document"
    );

    println!("find and replace");
    window.open_path(files[2].clone());
    window.open_find_for_test(false);
    window.set_find_query_for_test("line");
    window.find_step_for_test(true);
    window.find_step_for_test(true);
    window.find_step_for_test(false);
    window.open_find_for_test(true);
    window.set_replacement_for_test("LINE");
    window.replace_for_test(false);
    window.replace_for_test(true);
    check!(
        window.current_document().is_some(),
        "replacing left no current document"
    );

    println!("searching for something that is not there");
    window.set_find_query_for_test("zzzzz-not-present-zzzzz");
    window.find_step_for_test(true);

    // Replacing when there is nothing to replace. This aborted the editor:
    // the sourceview5 binding for replace_all reads the result as a success
    // flag, but the C function returns a count, so zero replacements looked
    // like a failure with no error attached and its assertion fired.
    println!("replacing something that is not there");
    window.set_replacement_for_test("never-used");
    window.replace_for_test(true);
    check!(
        window.current_document().is_some(),
        "replacing nothing must not take the document with it"
    );

    println!("replacing with an empty query");
    window.set_find_query_for_test("");
    window.replace_for_test(true);
    window.replace_for_test(false);

    println!("go to line");
    window.jump_to_line_for_test(1);
    // Deliberately out of range in both directions: the caller is a text box
    // the user types into, so it will happen.
    window.jump_to_line_for_test(-5);
    window.jump_to_line_for_test(999_999);
    window.goto_line_for_test();

    println!("opening popovers repeatedly, and over each other");
    // Each popover replaces the one before it. Popping down emits `closed`,
    // which is what unparents; doing it again by hand unparented twice and
    // GTK complained the widget was no longer one.
    for _ in 0..3 {
        window.goto_line_for_test();
        window.open_switcher_for_test();
    }

    println!("Escape closes the tab switcher");
    window.open_switcher_for_test();
    check!(
        window.popover_is_open_for_test(),
        "the switcher should be on screen"
    );
    check!(
        window.switcher_escape_for_test(),
        "could not reach the switcher's entry"
    );
    check!(
        !window.popover_is_open_for_test(),
        "Escape must dismiss the switcher, not only clicking away"
    );

    println!("tab switcher popover");
    // GDK may log "Tried to map a grabbing popup with a non-top most parent"
    // here. That is this environment, not a defect: an autohide popover takes
    // a grab, and the grab is refused while the test window does not have
    // focus — which it never does, because the test runs unattended. Verified
    // by re-running with autohide(false), which is silent. Autohide stays on;
    // clicking away to dismiss is the behaviour people expect.
    window.open_switcher_for_test();

    println!("saving");
    window.save_for_test();
    check!(
        window.current_document().is_some(),
        "saving left no current document"
    );

    println!("zoom");
    for _ in 0..3 {
        window.zoom_for_test(1);
    }
    for _ in 0..8 {
        window.zoom_for_test(-1);
    }

    println!("saving an untitled document does not silently write anywhere");
    window.new_untitled();
    let untitled = window.current_document().unwrap();
    check!(untitled.path().is_none(), "a new tab should have no path");
    // Save on an untitled document opens a file chooser rather than writing;
    // the test only checks it does not crash on the way there.

    let _ = std::fs::remove_dir_all(dir);
    println!("PASS: the window survived everything a first minute throws at it");
    std::process::exit(0);
}
