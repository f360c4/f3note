//! Drives the window through the crash-recovery demonstration, for a recording.
//!
//! Development tool. Typing with `wtype` would need the window focused, which
//! means taking the keyboard away from whoever is at the machine; this inserts
//! into the buffer instead, so the window can sit on an unfocused output while
//! a capture loop watches it.

use std::cell::Cell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use f3note::config::Config;
use f3note::theme::ThemeEngine;
use f3note::ui::window::Window;

/// What gets typed. Short enough to read in a few seconds, ordinary enough
/// that losing it would be annoying rather than catastrophic — which is the
/// case the editor is actually for.
const SCRIPT: &str = "Notes for Friday\n\n- migrate the database before the deploy\n\
                      \n- tell support the site will be down\n\n- check the backup restores\n";

fn main() -> glib::ExitCode {
    std::env::set_var("GSK_RENDERER", "cairo");
    let file = std::env::args().nth(1).expect("usage: gifbot <file>");

    let app = gtk::Application::builder()
        .application_id("io.github.f360c4.f3note.gifbot")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();

    app.connect_activate(move |app| {
        let (config, _) = Config::load();
        let engine = ThemeEngine::new(config);
        let window = Window::new(app, engine);
        window.window.set_default_size(940, 520);
        window.open_path(std::path::PathBuf::from(&file));
        window.window.present();

        let window: Rc<Window> = window;
        let typed = Cell::new(0usize);
        let chars: Vec<char> = SCRIPT.chars().collect();

        // A short pause before the first keystroke, so the recording opens on a
        // still window rather than mid-word, and then one character per tick at
        // something close to a person's typing speed.
        glib::timeout_add_local_once(std::time::Duration::from_millis(900), move || {
            glib::timeout_add_local(std::time::Duration::from_millis(38), move || {
                let i = typed.get();
                if i >= chars.len() {
                    println!("typed");
                    return glib::ControlFlow::Break;
                }
                if let Some(buffer) = window.current_document().and_then(|d| d.buffer()) {
                    let mut end = buffer.end_iter();
                    let mut s = [0u8; 4];
                    buffer.insert(&mut end, chars[i].encode_utf8(&mut s));
                }
                typed.set(i + 1);
                glib::ControlFlow::Continue
            });
        });
    });

    app.run_with_args::<&str>(&[])
}
