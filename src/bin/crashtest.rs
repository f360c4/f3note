//! End-to-end check of the crash-recovery path.
//!
//! Development tool, not part of the editor. It drives a real window with a
//! real GTK main loop, but edits the buffer programmatically instead of relying
//! on injected keystrokes — which depend on which window happens to have focus
//! and are therefore neither reliable nor safe to run unattended.
//!
//! Two phases, run in sequence by `scripts/crashtest.sh`:
//!
//!   write  open a file, type into it, wait for autosave, then abort the
//!          process with SIGKILL — no unwinding, no destructors, no exit
//!          handlers. As close to pulling the power cord as a test can get.
//!   read   start again and report what came back.

use std::path::PathBuf;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use f3note::config::Config;
use f3note::theme::ThemeEngine;
use f3note::ui::window::Window;

const TYPED: &str = "UNSAVED TEXT: acentuação ção ñ ü\nsecond line typed\n";

fn main() -> glib::ExitCode {
    std::env::set_var("GSK_RENDERER", "cairo");

    let phase = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "write".to_owned());
    let target = std::env::args().nth(2).map(PathBuf::from);

    let app = gtk::Application::builder()
        .application_id("io.github.f360c4.f3note.crashtest")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();

    app.connect_activate(move |app| {
        let (config, _) = Config::load();
        let engine = ThemeEngine::new(config);
        let window = Window::new(app, engine);

        match phase.as_str() {
            "write" => {
                let path = target.clone().expect("a file to open");
                window.open_path(path);
                window.window.present();
                run_write_phase(window.clone());
            }
            _ => {
                let restored = window.restore_session();
                window.window.present();
                run_read_phase(window.clone(), restored);
            }
        }
    });

    app.run_with_args::<&str>(&[])
}

fn buffer_text(window: &Rc<Window>) -> Option<String> {
    let buffer = window.current_document()?.buffer()?;
    let (start, end) = buffer.bounds();
    Some(buffer.text(&start, &end, true).to_string())
}

fn run_write_phase(window: Rc<Window>) {
    // Give the window a turn of the main loop to materialise the document.
    glib::timeout_add_local_once(std::time::Duration::from_millis(600), move || {
        let Some(doc) = window.current_document() else {
            println!("FAIL: no document was opened");
            std::process::exit(1);
        };
        let Some(buffer) = doc.buffer() else {
            println!("FAIL: the document has no buffer");
            std::process::exit(1);
        };

        println!("opened: {}", doc.describe());
        println!(
            "on disk before typing: {:?}",
            std::fs::read_to_string(doc.path().unwrap())
        );

        // Edit exactly as typing would: through the buffer, so every signal the
        // editor listens for fires the way it does in real use.
        let mut end = buffer.end_iter();
        buffer.insert(&mut end, TYPED);
        println!("typed {} characters", TYPED.chars().count());

        // Long enough for the idle debounce to elapse and a tick to run.
        glib::timeout_add_local_once(std::time::Duration::from_secs(5), || {
            println!("killing this process with SIGKILL");
            // Deliberately not exit(): no unwinding, no destructors, no flush.
            unsafe { kill_self() }
        });
    });
}

/// Terminate without any chance to clean up, the way a power cut does.
unsafe fn kill_self() -> ! {
    // Writing to /proc is enough to abort without going through Rust's exit
    // path, and avoids pulling in libc for one call.
    let pid = std::process::id();
    let _ = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status();
    // The signal arrives asynchronously; park until it does.
    loop {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn run_read_phase(window: Rc<Window>, restored: bool) {
    glib::timeout_add_local_once(std::time::Duration::from_millis(900), move || {
        println!("session restored: {restored}");
        match window.current_document() {
            Some(doc) => {
                println!("restored document: {}", doc.describe());
                println!("marked modified: {}", doc.is_modified());
                match buffer_text(&window) {
                    Some(text) => {
                        let recovered = text.contains("UNSAVED TEXT");
                        println!("--- buffer contents ---\n{text}--- end ---");
                        println!(
                            "{}",
                            if recovered {
                                "PASS: unsaved work came back"
                            } else {
                                "FAIL: unsaved work was lost"
                            }
                        );
                        std::process::exit(if recovered { 0 } else { 1 });
                    }
                    None => {
                        println!("FAIL: no buffer");
                        std::process::exit(1);
                    }
                }
            }
            None => {
                println!("FAIL: nothing was restored");
                std::process::exit(1);
            }
        }
    });
}
