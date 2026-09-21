//! What a single very long line costs GtkTextView, measured on a realised view.
//!
//! The editor detects long lines and opens with wrapping and highlighting off,
//! and opening is genuinely fast. Something else hung the interface, so this
//! times the operations that happen *after* the first frame — turning wrapping
//! back on, and moving the caret along the line — on a widget that is actually
//! on screen. Measuring an unrealised widget reports zero and means nothing.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use gtk::glib;
use gtk::prelude::*;
use sourceview5::prelude::*;

struct Case {
    label: &'static str,
    chars: usize,
    wrap: bool,
}

const CASES: &[Case] = &[
    Case {
        label: "10k chars, turn wrapping on",
        chars: 10_000,
        wrap: true,
    },
    Case {
        label: "36k chars, turn wrapping on",
        chars: 36_000,
        wrap: true,
    },
    Case {
        label: "36k chars, caret to end",
        chars: 36_000,
        wrap: false,
    },
    Case {
        label: "100k chars, turn wrapping on",
        chars: 100_000,
        wrap: true,
    },
];

fn line_of(chars: usize) -> String {
    let unit = "body{margin:0;padding:0}";
    unit.repeat(chars / unit.len() + 1)[..chars].to_owned()
}

fn main() -> glib::ExitCode {
    std::env::set_var("GSK_RENDERER", "cairo");
    let app = gtk::Application::builder()
        .application_id("io.github.f360c4.f3note.linebench")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();

    app.connect_activate(|app| {
        let index = Rc::new(RefCell::new(0usize));
        next_case(app.clone(), index);
    });
    app.run_with_args::<&str>(&[])
}

fn next_case(app: gtk::Application, index: Rc<RefCell<usize>>) {
    let i = *index.borrow();
    if i >= CASES.len() {
        app.quit();
        return;
    }
    *index.borrow_mut() = i + 1;
    let case = &CASES[i];

    let buffer = sourceview5::Buffer::new(None);
    buffer.set_highlight_syntax(false);
    buffer.set_highlight_matching_brackets(false);
    buffer.set_text(&line_of(case.chars));

    let view = sourceview5::View::builder().buffer(&buffer).build();
    view.set_monospace(true);
    view.set_wrap_mode(gtk::WrapMode::None);

    let scroller = gtk::ScrolledWindow::builder().child(&view).build();
    let window = gtk::ApplicationWindow::builder()
        .application(&app)
        .default_width(900)
        .default_height(600)
        .child(&scroller)
        .build();
    window.present();

    // Wait for the window to actually draw once, so the measurement below is
    // of the operation and not of realising the widget.
    let done = Rc::new(RefCell::new(false));
    window.add_tick_callback(move |w, _| {
        if *done.borrow() {
            return glib::ControlFlow::Break;
        }
        *done.borrow_mut() = true;

        let start = Instant::now();
        if case.wrap {
            view.set_wrap_mode(gtk::WrapMode::WordChar);
        } else {
            let end = buffer.end_iter();
            buffer.place_cursor(&end);
            view.scroll_to_mark(&buffer.get_insert(), 0.0, true, 0.0, 0.0);
        }
        // Force the widget to settle rather than deferring to an idle.
        let (_, _, _, _) = view.measure(gtk::Orientation::Vertical, w.width());
        println!(
            "{:<32} {:>9.0} ms",
            case.label,
            start.elapsed().as_secs_f64() * 1000.0
        );

        w.close();
        let app = app.clone();
        let index = index.clone();
        glib::idle_add_local_once(move || next_case(app, index));
        glib::ControlFlow::Break
    });
}
