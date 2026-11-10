//! Terminal prototype and benchmark.
//!
//!   nyx-term-lab                   a shell in a TerminalView
//!   nyx-term-lab --mirror          the same, with a read-only mirror below
//!   nyx-term-lab --bench NAME      run one benchmark, print JSON, quit
//!
//! Benchmarks: throughput, scrollback, latency, multi, resize, mirror.
//! NYX_TERM_LAB_APP_ID overrides the application id (for test copies).

mod bench;
mod session;

use gtk::prelude::*;
use gtk::{gio, glib, pango};
use nyx_term_gtk::{TerminalView, ViewStyle};

const APP_ID: &str = "io.github.design_nexus.nyx.TermLab";

fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let bench = args.iter().position(|a| a == "--bench").and_then(|i| args.get(i + 1)).cloned();
    let mirror = args.iter().any(|a| a == "--mirror");
    let app_id = std::env::var("NYX_TERM_LAB_APP_ID").unwrap_or_else(|_| APP_ID.to_string());
    let app = gtk::Application::builder().application_id(app_id).flags(gio::ApplicationFlags::NON_UNIQUE).build();
    app.connect_activate(move |app| match &bench {
        Some(name) => bench::run(app, name),
        None => demo(app, mirror),
    });
    app.run_with_args::<&str>(&[])
}

pub fn style() -> ViewStyle {
    let font = gtk::gio::Settings::new("org.gnome.desktop.interface").string("monospace-font-name");
    ViewStyle { font: pango::FontDescription::from_string(&font), ..Default::default() }
}

fn demo(app: &gtk::Application, mirror: bool) {
    let window =
        gtk::ApplicationWindow::builder().application(app).title("Terminal lab").default_width(960).default_height(640).build();
    let main = TerminalView::new(style(), false);
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let s = session::Session::spawn(&[shell.as_str()], &main).expect("spawn shell");
    let w = window.clone();
    main.connect_title(move |t| w.set_title(Some(t)));
    if mirror {
        let viewer = TerminalView::new(style(), true);
        s.add_mirror(&viewer);
        let paned = gtk::Paned::new(gtk::Orientation::Vertical);
        paned.set_start_child(Some(&main));
        paned.set_end_child(Some(&viewer));
        paned.set_position(400);
        window.set_child(Some(&paned));
    } else {
        window.set_child(Some(&main));
    }
    let app = app.clone();
    s.on_exit(move || app.quit());
    std::mem::forget(s);
    window.present();
    main.grab_focus();
}
