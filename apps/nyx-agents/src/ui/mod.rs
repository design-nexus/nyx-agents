//! The desktop app.

mod daemon;
mod inspector;
mod page;
mod prefs;
mod theme;
mod window;

use gtk::prelude::*;
use gtk::{gio, glib};

pub const APP_ID: &str = "io.github.design_nexus.nyx.Agents";

const USAGE: &str = "Usage: nyx-agents [--open session/ID]\n\
\n\
  --open session/ID   show that session (ID may be a unique prefix)\n\
\n\
Command line: nyx-agents help\n";

pub fn run() -> i32 {
    nyx_core::app::init(nyx_core::app::App { slug: "nyx-agents", name: "Agents" });
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{USAGE}");
        return 0;
    }
    let id = std::env::var("NYX_AGENTS_APP_ID").unwrap_or_else(|_| APP_ID.to_string());
    let app = gtk::Application::builder().application_id(id).flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE).build();
    app.connect_startup(|_| theme::install());
    app.connect_command_line(|app, cl| {
        let argv: Vec<String> = cl.arguments().iter().map(|a| a.to_string_lossy().to_string()).collect();
        window::present(app);
        if let Some(target) = argv.windows(2).find(|w| w[0] == "--open").map(|w| w[1].clone()) {
            window::open_target(&target);
        }
        glib::ExitCode::SUCCESS
    });
    app.connect_shutdown(|_| prefs::flush());
    i32::from(app.run())
}
