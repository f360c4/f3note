//! Opens the editor in a given state and holds it there, for screenshots.
//!
//! Development tool. Popovers close as soon as focus moves, so capturing one
//! by hand is a race; this opens the thing and waits.

use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use f3note::config::Config;
use f3note::theme::ThemeEngine;
use f3note::ui::window::Window;

fn main() -> glib::ExitCode {
    std::env::set_var("GSK_RENDERER", "cairo");
    let what = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "plain".to_owned());
    let files: Vec<String> = std::env::args().skip(2).collect();

    let app = gtk::Application::builder()
        .application_id("io.github.f360c4.f3note.shotbot")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();

    app.connect_activate(move |app| {
        let (config, _) = Config::load();
        let engine = ThemeEngine::new(config);
        let window = Window::new(app, engine);
        for f in &files {
            window.open_path(std::path::PathBuf::from(f));
        }
        window.window.present();

        let what = what.clone();
        let window: Rc<Window> = window;
        glib::timeout_add_local_once(std::time::Duration::from_millis(4000), move || {
            match what.as_str() {
                "history" => {
                    // A few versions to list.
                    if let Some(doc) = window.current_document() {
                        let store = f3note::session::store::DocStore::new(
                            &window.state_root_for_test(),
                            &doc.store_key(),
                        );
                        for (i, text) in [
                            "# Release notes\n\nFirst draft.\n",
                            "# Release notes\n\nSecond pass, more detail.\n",
                            "# Release notes\n\nAlmost there.\n",
                        ]
                        .iter()
                        .enumerate()
                        {
                            let _ = store.push_history(text.as_bytes(), 20, u64::MAX);
                            let _ = i;
                        }
                    }
                    window.open_history_for_test();
                }
                "search" => {
                    window.search_files_for_test();
                    window.set_search_query_for_test("version");
                }
                "sessions" => {
                    window.save_session_as_for_test("config");
                    window.save_session_as_for_test("notes");
                    window.open_sessions_for_test();
                }
                "switcher" => window.open_switcher_for_test(),
                _ => {}
            }
        });
    });

    app.run_with_args::<&str>(&[])
}
