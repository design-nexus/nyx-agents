//! Settings, as the suite's card over the window (Ctrl+,).

use gtk::prelude::*;
use nyx_agents_proto::Harness;
use nyx_ui::settings_card::{self, About, Spec, Standard};
use nyx_ui::widgets;

use super::prefs;

const GROUPS: &[(&str, &str, &str)] = &[
    ("Sessions", "utilities-terminal-symbolic", "The agent and folder new sessions start with"),
    ("Notifications", "preferences-system-notifications-symbolic", "When an agent needs you"),
];

pub const SHORTCUTS: &[(&[&str], &str)] = &[
    (&["Ctrl", "T"], "New session"),
    (&["Ctrl", "F"], "Find in the terminal"),
    (&["Ctrl", "Shift", "F"], "Search sessions"),
    (&["Ctrl", "C"], "Copy the selection (interrupts the agent when nothing is selected)"),
    (&["Ctrl", "V"], "Paste"),
    (&["Ctrl", "Shift", "C"], "Copy"),
    (&["Ctrl", "Shift", "V"], "Paste"),
    (&["Shift", "Drag"], "Send the mouse to the program instead of selecting"),
    (&["Ctrl", "Click"], "Open a link"),
    (&["Shift", "Page Up"], "Scroll back"),
    (&["Ctrl", "Shift", "O"], "Open a terminal beside the agent"),
    (&["Ctrl", "Shift", "E"], "Open a terminal below the agent"),
    (&["Ctrl", "Page Down"], "Next tab"),
    (&["Ctrl", "Page Up"], "Previous tab"),
    (&["Ctrl", "1…9"], "Go to a tab"),
    (&["Ctrl", "B"], "Collapse the sidebar"),
    (&["Ctrl", ","], "Settings"),
];

const SPEC: Spec = Spec {
    groups: GROUPS,
    search_text: None,
    standard: Some(Standard {
        app: "agents",
        about: About {
            name: "Agents",
            icon: "io.github.design_nexus.nyx.Agents",
            version: env!("CARGO_PKG_VERSION"),
            description: "AI coding agents in terminals that keep running when the window closes.",
            licence: "MIT",
            website: "",
            issues: "",
            credits: &[("libghostty-vt", "the terminal"), ("sway", "agent workspaces")],
        },
        shortcuts: SHORTCUTS,
        reduce_motion: || prefs::get().reduce_motion,
        set_reduce_motion: |on| prefs::update(|p| p.reduce_motion = on),
    }),
};

pub fn open(overlay: &gtk::Overlay, window: &gtk::ApplicationWindow, harnesses: &[Harness]) {
    let (root, body) = page(harnesses);
    settings_card::open_page(overlay, window, SPEC, &root, &body);
}

fn group(body: &gtk::Box, title: &str) -> gtk::Box {
    let wrap = widgets::vbox(10);
    wrap.set_widget_name(title);
    wrap.add_css_class("settings-group");
    wrap.append(&widgets::label(&title.to_uppercase(), "group-title"));
    let card = widgets::vbox(0);
    card.add_css_class("group-list");
    wrap.append(&card);
    body.append(&wrap);
    card
}

fn page(harnesses: &[Harness]) -> (gtk::Widget, gtk::Box) {
    let body = widgets::vbox(24);
    body.set_margin_top(16);
    body.set_margin_bottom(24);
    body.set_margin_start(20);
    body.set_margin_end(20);

    let sessions = group(&body, "Sessions");
    let mut options = vec![(String::new(), "The last one used".to_string())];
    options.extend(harnesses.iter().filter(|h| h.path.is_some() && h.id != "shell").map(|h| (h.id.clone(), h.name.clone())));
    let current = crate::settings::default_harness().unwrap_or_default();
    let (row, _) = widgets::choice_row("Agent", "Listed first in New session.", options, &current, |id| {
        if let Err(e) = crate::settings::set_default_harness(&id) {
            eprintln!("nyx-agents: could not save the default agent: {e}");
        }
    });
    sessions.append(&row);
    let folder = crate::settings::default_folder();
    let (row, _) = widgets::button_row(
        "Folder",
        "Where new sessions start. New session goes back to it after each start.",
        &nyx_core::paths::pretty(std::path::Path::new(&folder)),
        |b| {
            let dialog = gtk::FileDialog::builder().title("New sessions start in").modal(true).build();
            dialog.set_initial_folder(Some(&gtk::gio::File::for_path(crate::settings::default_folder())));
            let b = b.clone();
            dialog.select_folder(widgets::main_window().as_ref(), gtk::gio::Cancellable::NONE, move |r| {
                let Some(path) = r.ok().and_then(|f| f.path()) else { return };
                let path = path.display().to_string();
                if let Err(e) = crate::settings::set_default_folder(&path) {
                    eprintln!("nyx-agents: could not save the default folder: {e}");
                    return;
                }
                b.set_label(&nyx_core::paths::pretty(std::path::Path::new(&path)));
                super::window::set_folder(&path);
            });
        },
    );
    sessions.append(&row);

    let notifications = group(&body, "Notifications");
    let (row, _) = widgets::switch_row(
        "Notify when an agent needs you",
        "Approval requests, finished turns, errors and agents that end unexpectedly. The session on screen is not notified about.",
        crate::settings::notifications(),
        |on| {
            if let Err(e) = crate::settings::set_notifications(on) {
                eprintln!("nyx-agents: could not save notifications: {e}");
            }
        },
    );
    notifications.append(&row);

    let root = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::External)
        .vexpand(true)
        .child(&body)
        .build();
    (root.upcast(), body)
}
