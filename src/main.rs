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
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use sourceview5::prelude::*;

use f3note::config::Config;
use f3note::theme::ThemeEngine;

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

fn build_window(app: &gtk::Application, engine: &Rc<ThemeEngine>) -> gtk::ApplicationWindow {
    let view = sourceview5::View::new();
    view.set_monospace(true);
    view.set_left_margin(8);
    view.set_right_margin(8);
    view.set_top_margin(4);

    let scroller = gtk::ScrolledWindow::builder()
        .child(&view)
        .hexpand(true)
        .vexpand(true)
        .build();

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("f3note")
        .default_width(900)
        .default_height(640)
        .child(&scroller)
        .build();
    // Every rule in the generated stylesheet is scoped to this class, so
    // f3note styles itself without reaching into any other application's
    // widgets through the shared display provider.
    window.add_css_class("f3note");

    let buffer = view
        .buffer()
        .downcast::<sourceview5::Buffer>()
        .expect("a sourceview buffer");

    // Re-apply everything that lives outside CSS whenever the theme changes.
    let view_weak = view.downgrade();
    let buffer_weak = buffer.downgrade();
    engine.on_change(move |theme| {
        if let Some(view) = view_weak.upgrade() {
            view.set_show_line_numbers(true);
            view.set_wrap_mode(gtk::WrapMode::WordChar);
        }
        if let Some(buffer) = buffer_weak.upgrade() {
            let manager = sourceview5::StyleSchemeManager::default();
            if let Some(s) = manager.scheme(f3note::theme::scheme::SCHEME_ID) {
                buffer.set_style_scheme(Some(&s));
            }
            let _ = theme;
        }
    });

    window
}

/// f3note is a single-window editor, so every activation reuses the window that
/// already exists. This is also what makes `f3note other.txt` from a terminal
/// land as a tab instead of a second process.
fn present(app: &gtk::Application, engine: &Rc<ThemeEngine>, files: &[gtk::gio::File]) {
    let window = app
        .windows()
        .into_iter()
        .next()
        .and_then(|w| w.downcast::<gtk::ApplicationWindow>().ok())
        .unwrap_or_else(|| build_window(app, engine));

    for file in files {
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

    // The theme engine needs a display, so it cannot be built until GTK has
    // started up. It is created once on first use and shared from there.
    let engine: Rc<std::cell::RefCell<Option<Rc<ThemeEngine>>>> =
        Rc::new(std::cell::RefCell::new(None));

    let get_engine = {
        let engine = engine.clone();
        move || -> Rc<ThemeEngine> {
            let mut slot = engine.borrow_mut();
            if let Some(e) = slot.as_ref() {
                return e.clone();
            }
            let (config, err) = Config::load();
            if let Some(e) = err {
                eprintln!("f3note: {e}");
            }
            let e = ThemeEngine::new(config);
            *slot = Some(e.clone());
            e
        }
    };

    {
        let get_engine = get_engine.clone();
        app.connect_activate(move |app| present(app, &get_engine(), &[]));
    }
    app.connect_open(move |app, files, _hint| present(app, &get_engine(), files));

    app.run()
}
