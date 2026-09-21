//! Detects the main loop going unresponsive.
//!
//! A user opened a 36 000-character single-line stylesheet and the compositor
//! put up "Application Not Responding". Opening it is fast — measured — so the
//! stall is somewhere after the first frame. Micro-benchmarks around
//! individual calls reported nothing, because GtkTextView defers line
//! validation to an idle and the cost lands there rather than in the call.
//!
//! So this measures the thing that actually matters: how long the main loop
//! goes without running. A heartbeat is scheduled every 50ms; whenever it
//! fires late, the gap is the length of the stall it just sat through.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::prelude::*;

use f3note::config::Config;
use f3note::theme::ThemeEngine;
use f3note::ui::window::Window;

/// One named thing a person does to the open file.
type Step = (&'static str, Box<dyn Fn(&Rc<Window>)>);

/// Anything longer than this is visible as a hitch; a compositor starts
/// offering to kill the application at a few seconds.
const REPORT_ABOVE: Duration = Duration::from_millis(250);

fn main() -> glib::ExitCode {
    std::env::set_var("GSK_RENDERER", "cairo");
    let path = std::env::args().nth(1).expect("a file to open");

    let app = gtk::Application::builder()
        .application_id("io.github.f360c4.f3note.stallcheck")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();

    app.connect_activate(move |app| {
        let (config, _) = Config::load();
        let engine = ThemeEngine::new(config);
        let window = Window::new(app, engine);
        window.open_path(std::path::PathBuf::from(&path));
        window.window.present();

        let worst = Rc::new(RefCell::new(Duration::ZERO));
        start_heartbeat(worst.clone());
        drive(window, worst);
    });
    app.run_with_args::<&str>(&[])
}

fn start_heartbeat(worst: Rc<RefCell<Duration>>) {
    let last = Rc::new(RefCell::new(Instant::now()));
    glib::timeout_add_local(Duration::from_millis(50), move || {
        let now = Instant::now();
        let gap = now.duration_since(*last.borrow());
        *last.borrow_mut() = now;
        // The scheduled interval is expected; anything beyond it is time the
        // main loop spent unable to answer.
        let stall = gap.saturating_sub(Duration::from_millis(50));
        if stall > REPORT_ABOVE && std::env::var_os("F3NOTE_STALL_QUICK").is_none() {
            println!(
                "    main loop stalled {:.0} ms",
                stall.as_secs_f64() * 1000.0
            );
        }
        if stall > *worst.borrow() {
            *worst.borrow_mut() = stall;
        }
        glib::ControlFlow::Continue
    });
}

/// Walk through what a person does with a file like this, pausing between
/// steps so the heartbeat can observe each one separately.
fn drive(window: Rc<Window>, worst: Rc<RefCell<Duration>>) {
    // F3NOTE_STALL_QUICK reduces the run to the one case that matters for
    // finding the threshold: navigating a long line in the mode the editor
    // actually opens it in.
    let quick = std::env::var_os("F3NOTE_STALL_QUICK").is_some();
    let steps: Vec<Step> = if quick {
        vec![
            ("settle", Box::new(|_: &Rc<Window>| {})),
            ("walk", Box::new(|w: &Rc<Window>| w.walk_caret_for_test(40))),
        ]
    } else {
        vec![
            ("settling after open", Box::new(|_: &Rc<Window>| {})),
            (
                "REDUCED MODE: walking the caret across the line",
                Box::new(|w: &Rc<Window>| w.walk_caret_for_test(40)),
            ),
            (
                "REDUCED MODE: zooming in and out",
                Box::new(|w: &Rc<Window>| {
                    for _ in 0..6 {
                        w.zoom_for_test(1);
                    }
                    for _ in 0..6 {
                        w.zoom_for_test(-1);
                    }
                }),
            ),
            (
                "[Enable anyway] — wrapping back on",
                Box::new(|w: &Rc<Window>| w.force_capabilities_for_test()),
            ),
            (
                "WRAPPED: walking the caret across the line",
                Box::new(|w: &Rc<Window>| w.walk_caret_for_test(40)),
            ),
        ]
    };

    let index = Rc::new(RefCell::new(0usize));
    let interval = if std::env::var_os("F3NOTE_STALL_QUICK").is_some() {
        Duration::from_millis(1200)
    } else {
        Duration::from_secs(3)
    };
    glib::timeout_add_local(interval, move || {
        let i = *index.borrow();
        if i >= steps.len() {
            let w = *worst.borrow();
            println!();
            println!("worst stall: {:.0} ms", w.as_secs_f64() * 1000.0);
            std::process::exit(0);
        }
        *index.borrow_mut() = i + 1;
        println!("  {}", steps[i].0);
        (steps[i].1)(&window);
        glib::ControlFlow::Continue
    });
}
