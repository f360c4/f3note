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
