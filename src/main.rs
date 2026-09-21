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

use std::cell::RefCell;
use std::env;
use std::fs;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use f3note::config::Config;
use f3note::ipc;
use f3note::theme::ThemeEngine;
use f3note::ui::window::Window;

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
fn pin_renderer() {
    if env::var_os("GSK_RENDERER").is_some() {
        return;
    }
    let choice = env::var("F3NOTE_RENDERER").unwrap_or_else(|_| "cairo".to_owned());
    env::set_var("GSK_RENDERER", choice);
}

/// f3note is a single-window editor, so every activation reuses the window
/// that already exists. This is also what makes `f3note other.txt` from a
/// terminal land as a tab instead of spawning a second process.
fn present(app: &gtk::Application, state: &State, files: &[gtk::gio::File]) {
    let window = state.window();

    if files.is_empty() {
        if window.current_document().is_none() {
            window.new_untitled();
        }
    } else {
        for file in files {
            match file.path() {
                Some(path) => window.open_path(path),
                None => eprintln!("f3note: cannot open {}", file.uri()),
            }
        }
    }

    if env::var_os("F3NOTE_BENCH").is_some() {
        let app = app.clone();
        window.window.add_tick_callback(move |_, _| {
            if let Some(ms) = ms_since_exec() {
                let renderer = env::var("GSK_RENDERER").unwrap_or_else(|_| "default".to_owned());
                eprintln!("exec->first frame: {ms:.0} ms  (renderer: {renderer})");
            }
            app.quit();
            glib::ControlFlow::Break
        });
    }

    window.window.present();
    let _ = app;
}

/// Lazily built so nothing touches GTK before the application has started up.
#[derive(Clone)]
struct State {
    app: gtk::Application,
    window: Rc<RefCell<Option<Rc<Window>>>>,
    /// Handed to the window once it exists, so the socket is only served by a
    /// process that actually owns one.
    listener: Rc<RefCell<Option<std::os::unix::net::UnixListener>>>,
}

impl State {
    fn new(app: &gtk::Application, listener: Option<std::os::unix::net::UnixListener>) -> State {
        State {
            app: app.clone(),
            window: Rc::new(RefCell::new(None)),
            listener: Rc::new(RefCell::new(listener)),
        }
    }

    fn window(&self) -> Rc<Window> {
        let mut slot = self.window.borrow_mut();
        if let Some(w) = slot.as_ref() {
            return w.clone();
        }
        let (config, err) = Config::load();
        let engine = ThemeEngine::new(config);
        let window = Window::new(&self.app, engine);
        if let Some(e) = err {
            eprintln!("f3note: {e}");
        }
        window.restore_session();

        // Serve the single-instance socket. GApplication has already handled
        // this when a session bus exists; the socket is what covers the case
        // where there is none, and where GApplication would otherwise let a
        // second process start and write the same recovery state.
        if let Some(listener) = self.listener.borrow_mut().take() {
            let window = window.clone();
            if let Err(e) = ipc::listen(listener, move |paths| {
                for path in paths {
                    window.open_path(path);
                }
                window.window.present();
            }) {
                eprintln!("f3note: cannot serve the single-instance socket: {e}");
            }
        }

        *slot = Some(window.clone());
        window
    }
}

fn main() -> glib::ExitCode {
    pin_renderer();

    // Claim the single-instance socket before GTK starts. Without a session
    // bus GApplication does not detect a second instance at all — it prints a
    // warning and lets every process become primary, which for this editor
    // would mean two of them writing the same crash-recovery state.
    let paths: Vec<std::path::PathBuf> = env::args_os()
        .skip(1)
        .map(std::path::PathBuf::from)
        .filter(|p| !p.to_string_lossy().starts_with('-'))
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
        .collect();

    let listener = match ipc::claim(&paths) {
        Ok(ipc::Role::Primary(listener)) => Some(listener),
        Ok(ipc::Role::Delegated) => return glib::ExitCode::SUCCESS,
        Err(e) => {
            // Better to run without the fallback than to refuse to start.
            eprintln!("f3note: single-instance socket unavailable ({e}); continuing");
            None
        }
    };

    let app = gtk::Application::builder()
        .application_id(APP_ID)
        .flags(gtk::gio::ApplicationFlags::HANDLES_OPEN)
        .build();

    let state = State::new(&app, listener);

    {
        let state = state.clone();
        app.connect_activate(move |app| present(app, &state, &[]));
    }
    app.connect_open(move |app, files, _hint| present(app, &state, files));

    app.run()
}
