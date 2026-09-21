//! f3note — entry point.
//!
//! Two things happen before GTK is allowed to initialise, and both are
//! load-bearing:
//!
//! 1. The renderer is pinned to cairo. GTK4 picks Vulkan by default, which cost
//!    270-300ms to first frame on the reference machine against 130-140ms for
//!    cairo. The 200ms startup budget is not reachable otherwise, and the
//!    budget is a hard requirement, not an aspiration. See docs/PORTING.md.
//! 2. Startup is measured rather than assumed. `F3NOTE_BENCH=1 f3note` prints
//!    exec-to-first-frame and exits, so the budget stays a number anyone can
//!    check on their own hardware.

use std::env;
use std::fs;

use gtk::glib;
use gtk::prelude::*;
use sourceview5::prelude::*;

const APP_ID: &str = "io.github.f360c4.f3note";

/// Milliseconds since this process was exec'd.
///
/// Taken from /proc rather than from the top of `main`, because most of GTK's
/// startup cost is spent in the dynamic linker and in constructors that run
/// before our first instruction. Timing from `main` would hide exactly the part
/// we are trying to keep under budget.
fn ms_since_exec() -> Option<f64> {
    let stat = fs::read_to_string("/proc/self/stat").ok()?;
    // The comm field is parenthesised and may itself contain spaces and
    // parens, so fields are counted from after the *final* ')'.
    let after_comm = stat.get(stat.rfind(')')? + 1..)?;
    // starttime is field 22 overall; comm is field 2, so it is the 20th field
    // after comm.
    let start_ticks: f64 = after_comm.split_whitespace().nth(19)?.parse().ok()?;
    let uptime: f64 = fs::read_to_string("/proc/uptime")
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    // USER_HZ is 100 on every Linux target f3note supports.
    Some((uptime - start_ticks / 100.0) * 1000.0)
}

/// Pin the GSK renderer to cairo unless the user has an opinion.
///
/// An explicit `GSK_RENDERER` always wins: someone who set it knows what they
/// are doing and f3note has no business overriding them. `F3NOTE_RENDERER`
/// exists so users on hardware where Vulkan initialises quickly can opt back
/// into acceleration without having to know GSK's variable name.
fn pin_renderer() -> String {
    if let Some(existing) = env::var_os("GSK_RENDERER") {
        return existing.to_string_lossy().into_owned();
    }
    let choice = env::var("F3NOTE_RENDERER").unwrap_or_else(|_| "cairo".to_owned());
    env::set_var("GSK_RENDERER", &choice);
    choice
}

fn build_window(app: &gtk::Application) -> gtk::ApplicationWindow {
    let view = sourceview5::View::new();
    view.set_monospace(true);
    view.set_show_line_numbers(true);

    let scroller = gtk::ScrolledWindow::builder().child(&view).build();

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("f3note")
        .default_width(900)
        .default_height(640)
        .child(&scroller)
        .build();

    window
}

/// f3note is a single-window editor, so every activation reuses the window that
/// already exists. This is also what makes `f3note other.txt` from a terminal
/// land as a tab instead of a second process.
fn present(app: &gtk::Application, files: &[gtk::gio::File]) {
    let window = app
        .windows()
        .into_iter()
        .next()
        .and_then(|w| w.downcast::<gtk::ApplicationWindow>().ok())
        .unwrap_or_else(|| build_window(app));

    for file in files {
        // Milestone 1 only proves the file reaches the running instance; the
        // buffer manager that will actually load it does not exist yet.
        eprintln!(
            "open: {}",
            file.path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| file.uri().to_string())
        );
    }

    if env::var_os("F3NOTE_BENCH").is_some() {
        let app = app.clone();
        window.add_tick_callback(move |_, _| {
            if let Some(ms) = ms_since_exec() {
                let renderer = env::var("GSK_RENDERER").unwrap_or_else(|_| "default".to_owned());
                eprintln!("exec->first frame: {ms:.0} ms  (renderer: {renderer})");
            }
            app.quit();
            glib::ControlFlow::Break
        });
    }

    window.present();
}

fn main() -> glib::ExitCode {
    pin_renderer();

    let app = gtk::Application::builder()
        .application_id(APP_ID)
        .flags(gtk::gio::ApplicationFlags::HANDLES_OPEN)
        .build();

    app.connect_activate(|app| present(app, &[]));
    app.connect_open(|app, files, _hint| present(app, files));

    app.run()
}
