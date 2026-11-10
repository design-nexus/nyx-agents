//! The window: the 2.0 header, the sidebar of sessions (needs you, active,
//! recent) and projects, and a tab per open session.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use glib::translate::IntoGlib;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use nyx_agents_proto::{Activity, Event, Harness, Launch, Lifecycle, Session};
use nyx_ui::chrome;
use nyx_ui::sidebar::Sidebar;
use nyx_ui::tabs::{Tab, TabRow};

use super::daemon::{self, Msg};
use super::page::{self, Actions, Page};
use super::prefs::{self, TabPref};

struct Ui {
    window: gtk::ApplicationWindow,
    overlay: gtk::Overlay,
    sidebar: Rc<Sidebar>,
    tab_row: Rc<TabRow>,
    stack: gtk::Stack,
    new_button: gtk::MenuButton,
    new_list: gtk::ListBox,
    folder_button: gtk::Button,
    folder: RefCell<String>,
    pages: RefCell<Vec<Rc<RefCell<Page>>>>,
    active: Cell<usize>,
    sessions: RefCell<HashMap<String, Session>>,
    harnesses: RefCell<Vec<Harness>>,
    connected: Cell<bool>,
    restored: Cell<bool>,
    actions: Rc<Actions>,
    pending_open: RefCell<Option<String>>,
    viewing: RefCell<Option<String>>,
}

thread_local! {
    static UI: RefCell<Option<Rc<Ui>>> = const { RefCell::new(None) };
}

fn cur() -> Option<Rc<Ui>> {
    UI.with(|u| u.borrow().clone())
}

fn toast(msg: &str) {
    if let Some(ui) = cur() {
        nyx_ui::toast::show(&ui.overlay, msg);
    }
}

pub fn present(app: &gtk::Application) {
    if cur().is_none() {
        build(app);
    }
    if let Some(ui) = cur() {
        ui.window.present();
    }
}

/// `--open session/ID[/subagent/ID]`: show that session (ID may be a prefix).
pub fn open_target(target: &str) {
    let parts: Vec<&str> = target.split('/').collect();
    let sub = match parts.as_slice() {
        ["session", _, "subagent", sub] => Some(sub.to_string()),
        _ => None,
    };
    let Some(id) = (match parts.as_slice() {
        ["session", id, ..] => Some(*id),
        _ => None,
    }) else {
        toast("Nothing to open there");
        return;
    };
    let Some(ui) = cur() else { return };
    let found: Vec<String> = ui.sessions.borrow().keys().filter(|k| k.starts_with(id)).cloned().collect();
    match found.as_slice() {
        [one] => {
            open_session(one);
            if let Some(sub) = sub
                && let Some(p) = ui.pages.borrow().iter().find(|p| p.borrow().id == *one)
            {
                let inspector = p.borrow().inspector.clone();
                inspector.select(&sub);
            }
        }
        [] if !ui.connected.get() => *ui.pending_open.borrow_mut() = Some(target.to_string()),
        [] => toast("That session no longer exists"),
        _ => toast("That id matches more than one session"),
    }
}

fn actions() -> Rc<Actions> {
    Rc::new(Actions {
        stop: Rc::new(|id| {
            let id = id.to_string();
            daemon::request(
                move |c| c.stop(&id, false),
                |r| {
                    if let Err(e) = r {
                        toast(&e);
                    }
                },
            );
        }),
        resume: Rc::new(|id| {
            let id = id.to_string();
            daemon::request(
                move |c| c.resume(&id),
                |r| {
                    if let Err(e) = r {
                        toast(&e);
                    }
                },
            );
        }),
        forget: Rc::new(|id| {
            let id = id.to_string();
            close_session_tab(&id);
            daemon::request(
                move |c| c.forget(&id),
                |r| {
                    if let Err(e) = r {
                        toast(&e);
                    }
                },
            );
        }),
        split: Rc::new(split),
        unsplit: Rc::new(unsplit),
        open_git: Rc::new(|id| {
            let Some(ui) = cur() else { return };
            let Some(root) = ui.sessions.borrow().get(id).and_then(|s| s.worktree.as_ref().map(|w| w.root.clone())) else {
                return;
            };
            if let Err(e) = std::process::Command::new("nyx-git").arg(&root).spawn() {
                toast(&format!("Could not open Nyx Git: {e}"));
            }
        }),
        ended: Rc::new(|_| {}),
        subagent_source: Rc::new(|harness| {
            cur().and_then(|ui| ui.harnesses.borrow().iter().find(|h| h.id == harness).and_then(|h| h.capabilities.subagents))
        }),
    })
}

fn build(app: &gtk::Application) {
    let window =
        gtk::ApplicationWindow::builder().application(app).title("Agents").default_width(1280).default_height(800).build();
    window.add_css_class("agents-window");
    window.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));
    window.set_icon_name(Some("io.github.design_nexus.nyx.Agents"));
    nyx_ui::keys::install(&window, nyx_ui::keys::Common { close: false, ..Default::default() });

    let sidebar = Sidebar::new();
    sidebar.follow_window(&window);
    sidebar.set_collapsed(prefs::get().sidebar_collapsed);

    let stack = gtk::Stack::new();
    stack.set_hexpand(true);
    stack.set_vexpand(true);
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.set_transition_duration(if prefs::get().reduce_motion { 0 } else { 120 });
    let empty = nyx_ui::widgets::empty_state(
        "utilities-terminal-symbolic",
        "No session open",
        "Start an agent with New session, or pick one in the sidebar.",
        None,
    );
    stack.add_named(&empty, Some("empty"));

    let tab_row = TabRow::new("New session (Ctrl+T)");
    tab_row.connect_select(select_tab);
    tab_row.connect_close(close_tab);
    tab_row.connect_reorder(reorder);

    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    column.set_hexpand(true);
    column.append(&tab_row.root);
    column.append(&stack);
    let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    body.set_vexpand(true);
    body.append(&sidebar.root);
    body.append(&column);

    let header = chrome::app_header(&window, "utilities-terminal-symbolic", "Agents");
    let collapse = nyx_ui::widgets::bar_button("sidebar-show-symbolic", "Collapse the sidebar (Ctrl+B)");
    {
        let sb = sidebar.clone();
        collapse.connect_clicked(move |_| {
            sb.toggle();
            prefs::update(|p| p.sidebar_collapsed = sb.is_collapsed());
        });
    }
    header.start.append(&collapse);

    let (new_button, new_list, folder_button) = new_session_menu();
    header.end.append(&new_button);
    {
        let nb = new_button.clone();
        tab_row.connect_new(move || nb.popup());
    }

    let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    frame.add_css_class("window-frame");
    frame.append(&header.root);
    frame.append(&body);
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&frame));
    window.set_child(Some(&overlay));

    let folder = {
        let last = prefs::get().last_folder;
        if !last.is_empty() && std::path::Path::new(&last).is_dir() {
            last
        } else {
            nyx_core::paths::home().display().to_string()
        }
    };
    folder_button.set_label(&nyx_core::paths::pretty(std::path::Path::new(&folder)));
    let ui = Rc::new(Ui {
        window: window.clone(),
        overlay,
        sidebar,
        tab_row,
        stack,
        new_button,
        new_list,
        folder_button,
        folder: RefCell::new(folder),
        pages: RefCell::new(Vec::new()),
        active: Cell::new(0),
        sessions: RefCell::new(HashMap::new()),
        harnesses: RefCell::new(Vec::new()),
        connected: Cell::new(false),
        restored: Cell::new(false),
        actions: actions(),
        pending_open: RefCell::new(None),
        viewing: RefCell::new(None),
    });
    UI.with(|u| *u.borrow_mut() = Some(ui));

    install_keys(&window);
    nyx_theme::subscribe(&window, || {
        if let Some(ui) = cur() {
            for p in ui.pages.borrow().iter() {
                p.borrow().restyle();
            }
        }
    });
    window.connect_is_active_notify(|_| report_viewing());
    window.connect_close_request(|_| {
        save_tabs();
        prefs::flush();
        glib::Propagation::Proceed
    });

    refresh_sidebar();
    daemon::watch(on_msg);
}

fn install_keys(window: &gtk::ApplicationWindow) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed(|_, key, _, mods| {
        let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
        let Some(ui) = cur() else { return glib::Propagation::Proceed };
        let active = ui.pages.borrow().get(ui.active.get()).cloned();
        match key.to_lower() {
            gdk::Key::t if ctrl && !shift => ui.new_button.popup(),
            gdk::Key::f if ctrl && !shift => {
                if let Some(p) = active {
                    p.borrow().open_find();
                }
            }
            gdk::Key::b if ctrl && !shift => {
                ui.sidebar.toggle();
                prefs::update(|p| p.sidebar_collapsed = ui.sidebar.is_collapsed());
            }
            gdk::Key::o if ctrl && shift => {
                if let Some(p) = active {
                    let id = p.borrow().id.clone();
                    split(&id, false);
                }
            }
            gdk::Key::e if ctrl && shift => {
                if let Some(p) = active {
                    let id = p.borrow().id.clone();
                    split(&id, true);
                }
            }
            gdk::Key::Page_Down if ctrl && !shift => select_tab((ui.active.get() + 1) % ui.pages.borrow().len().max(1)),
            gdk::Key::Page_Up if ctrl && !shift => {
                let n = ui.pages.borrow().len().max(1);
                select_tab((ui.active.get() + n - 1) % n)
            }
            k if ctrl && !shift && (gdk::Key::_1..=gdk::Key::_9).contains(&k) => {
                let i = (k.into_glib() - gdk::Key::_1.into_glib()) as usize;
                if i < ui.pages.borrow().len() {
                    select_tab(i);
                }
            }
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    });
    window.add_controller(keys);
}

fn on_msg(m: Msg) {
    let Some(ui) = cur() else { return };
    match m {
        Msg::Connected { sessions, harnesses } => {
            ui.connected.set(true);
            *ui.harnesses.borrow_mut() = harnesses;
            *ui.sessions.borrow_mut() = sessions.into_iter().map(|s| (s.id.clone(), s)).collect();
            fill_new_menu();
            if !ui.restored.replace(true) {
                restore_tabs();
            }
            let all: Vec<Session> = ui.sessions.borrow().values().cloned().collect();
            for s in &all {
                update_pages(s);
            }
            if let Some(t) = ui.pending_open.borrow_mut().take() {
                open_target(&t);
            }
        }
        Msg::Event(Event::Session { session }) => {
            ui.sessions.borrow_mut().insert(session.id.clone(), (*session).clone());
            update_pages(&session);
        }
        Msg::Event(Event::Removed { id }) => {
            ui.sessions.borrow_mut().remove(&id);
            close_session_tab(&id);
        }
        Msg::Event(Event::Harnesses { harnesses }) => {
            *ui.harnesses.borrow_mut() = harnesses;
            fill_new_menu();
        }
        Msg::Disconnected(why) => {
            if ui.connected.replace(false) {
                toast("Lost the Nyx Agent Manager; reconnecting…");
            } else if ui.sessions.borrow().is_empty() {
                eprintln!("nyx-agents: {why}");
            }
        }
    }
    refresh_sidebar();
    refresh_tabs();
}

fn update_pages(s: &Session) {
    let Some(ui) = cur() else { return };
    for p in ui.pages.borrow().iter() {
        let p = p.borrow();
        if p.id == s.id {
            p.update(s, &ui.actions);
        }
        p.update_split(s, &ui.actions);
    }
}

fn rank(s: &Session) -> (u8, std::cmp::Reverse<u64>) {
    let r = match (s.lifecycle, s.status.activity) {
        (Lifecycle::Running, Activity::Blocked) => 0,
        (Lifecycle::Running, Activity::Done) => 1,
        (Lifecycle::Running | Lifecycle::Starting, Activity::Working) => 2,
        (Lifecycle::Running | Lifecycle::Starting, _) => 3,
        _ => 4,
    };
    (r, std::cmp::Reverse(s.status.since.max(s.created)))
}

fn status_icon(s: &Session) -> &'static str {
    match (s.lifecycle, s.status.activity) {
        (Lifecycle::Running, Activity::Blocked) => "dialog-warning-symbolic",
        (Lifecycle::Running, Activity::Done) => "object-select-symbolic",
        (Lifecycle::Running, Activity::Working) => "emblem-synchronizing-symbolic",
        (Lifecycle::Running | Lifecycle::Starting, _) => "utilities-terminal-symbolic",
        (Lifecycle::Lost, _) => "network-offline-symbolic",
        _ => "media-playback-stop-symbolic",
    }
}

fn refresh_sidebar() {
    let Some(ui) = cur() else { return };
    let sb = &ui.sidebar;
    sb.clear();
    let active_id = ui.pages.borrow().get(ui.active.get()).map(|p| p.borrow().id.clone());
    let mut list: Vec<Session> = ui.sessions.borrow().values().filter(|s| s.harness != "shell").cloned().collect();
    list.sort_by_key(rank);
    let live: Vec<&Session> = list.iter().filter(|s| matches!(s.lifecycle, Lifecycle::Running | Lifecycle::Starting)).collect();
    let needs: Vec<&&Session> = live.iter().filter(|s| s.status.activity == Activity::Blocked).collect();
    let mut first = true;
    let group = |title: &str, items: Vec<&Session>, first: &mut bool| {
        if items.is_empty() {
            return;
        }
        sb.append(&nyx_ui::sidebar::group_label(title, *first));
        *first = false;
        for s in items {
            let (state, _) = page::status_text(s);
            let branch = s.worktree.as_ref().and_then(|w| w.branch.clone()).map(|b| format!(" · {b}")).unwrap_or_default();
            let item = nyx_ui::sidebar::nav_item(status_icon(s), &s.title, &format!("{} · {state}{branch}", s.harness));
            item.set_active(active_id.as_deref() == Some(s.id.as_str()));
            let id = s.id.clone();
            item.button.connect_clicked(move |_| open_session(&id));
            sb.append(&item.button);
        }
    };
    group("Needs you", needs.iter().map(|s| **s).collect(), &mut first);
    group("Active", live.iter().filter(|s| s.status.activity != Activity::Blocked).copied().collect(), &mut first);
    let mut ended: Vec<&Session> =
        list.iter().filter(|s| !matches!(s.lifecycle, Lifecycle::Running | Lifecycle::Starting)).collect();
    ended.sort_by_key(|s| std::cmp::Reverse(s.ended.unwrap_or(s.created)));
    ended.truncate(8);
    group("Recent", ended, &mut first);

    // Projects: folders of past sessions, for starting another there.
    let mut projects: Vec<(String, String)> = Vec::new();
    for s in &list {
        let root = s.worktree.as_ref().map(|w| w.root.clone()).unwrap_or_else(|| s.cwd.clone());
        if !projects.iter().any(|(r, _)| *r == root) {
            let name =
                std::path::Path::new(&root).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.clone());
            projects.push((root, name));
        }
    }
    if !projects.is_empty() {
        sb.append(&nyx_ui::sidebar::group_label("Projects", first));
        for (root, name) in projects.into_iter().take(10) {
            let item = nyx_ui::sidebar::nav_item(
                "folder-symbolic",
                &name,
                &format!("New session in {}", nyx_core::paths::pretty(std::path::Path::new(&root))),
            );
            item.button.connect_clicked(move |_| {
                set_folder(&root);
                if let Some(ui) = cur() {
                    ui.new_button.popup();
                }
            });
            sb.append(&item.button);
        }
    }
    if !ui.connected.get() && ui.sessions.borrow().is_empty() {
        let hint = gtk::Label::new(Some("Connecting to the Nyx Agent Manager…"));
        hint.add_css_class("dim-label");
        hint.set_wrap(true);
        hint.set_margin_start(12);
        hint.set_margin_end(12);
        sb.append(&hint);
    }
    sb.refresh();
}

fn refresh_tabs() {
    let Some(ui) = cur() else { return };
    let sessions = ui.sessions.borrow();
    let tabs: Vec<Tab> = ui
        .pages
        .borrow()
        .iter()
        .map(|p| {
            let p = p.borrow();
            let s = sessions.get(&p.id);
            Tab {
                title: s.map(|s| s.title.clone()).unwrap_or_else(|| "Session".into()),
                icon: s.map(|s| status_icon(s).to_string()),
                tooltip: s.map(|s| format!("{} · {}", s.harness, page::status_text(s).0)),
                dirty: s.is_some_and(|s| s.lifecycle == Lifecycle::Running && s.status.activity == Activity::Blocked),
            }
        })
        .collect();
    drop(sessions);
    let n = tabs.len();
    ui.tab_row.set_tabs(&tabs, ui.active.get().min(n.saturating_sub(1)));
    if n == 0 {
        ui.stack.set_visible_child_name("empty");
        chrome::set_title(&ui.window, "Agents", "");
    }
}

pub fn open_session(id: &str) {
    let Some(ui) = cur() else { return };
    let existing = ui.pages.borrow().iter().position(|p| p.borrow().id == id);
    if let Some(i) = existing {
        select_tab(i);
        return;
    }
    let Some(s) = ui.sessions.borrow().get(id).cloned() else { return };
    let page = Rc::new(RefCell::new(Page::new(&s, &ui.actions)));
    Page::install_find(&page);
    ui.stack.add_named(&page.borrow().root, Some(&format!("s-{id}")));
    ui.pages.borrow_mut().push(page);
    let i = ui.pages.borrow().len() - 1;
    select_tab(i);
    save_tabs();
}

fn select_tab(i: usize) {
    let Some(ui) = cur() else { return };
    let Some(p) = ui.pages.borrow().get(i).cloned() else { return };
    ui.active.set(i);
    let p = p.borrow();
    ui.stack.set_visible_child(&p.root);
    p.focus();
    if let Some(s) = ui.sessions.borrow().get(&p.id) {
        chrome::set_title(&ui.window, "Agents", &s.title);
    }
    drop(p);
    refresh_tabs();
    refresh_sidebar();
    prefs::update(|pr| pr.active = i);
    report_viewing();
}

/// Tell the manager which session is on screen, so it doesn't notify about it.
fn report_viewing() {
    let Some(ui) = cur() else { return };
    let id = ui.window.is_active().then(|| ui.pages.borrow().get(ui.active.get()).map(|p| p.borrow().id.clone())).flatten();
    if ui.viewing.replace(id.clone()) == id {
        return;
    }
    daemon::request(move |c| c.call::<serde_json::Value>(nyx_agents_proto::Call::Viewing { id }), |_| {});
}

fn close_tab(i: usize) {
    let Some(ui) = cur() else { return };
    if i >= ui.pages.borrow().len() {
        return;
    }
    // Closing a tab only hides the session; the agent keeps running.
    let p = ui.pages.borrow_mut().remove(i);
    ui.stack.remove(&p.borrow().root);
    let n = ui.pages.borrow().len();
    if n > 0 {
        select_tab(ui.active.get().min(n - 1).saturating_sub(usize::from(i < ui.active.get())));
    } else {
        ui.active.set(0);
        report_viewing();
    }
    refresh_tabs();
    refresh_sidebar();
    save_tabs();
}

fn close_session_tab(id: &str) {
    let Some(ui) = cur() else { return };
    let i = ui.pages.borrow().iter().position(|p| p.borrow().id == id);
    if let Some(i) = i {
        close_tab(i);
    }
}

fn reorder(from: usize, to: usize) {
    let Some(ui) = cur() else { return };
    {
        let mut pages = ui.pages.borrow_mut();
        if from >= pages.len() || to >= pages.len() {
            return;
        }
        let p = pages.remove(from);
        pages.insert(to, p);
    }
    ui.active.set(to);
    refresh_tabs();
    save_tabs();
}

fn save_tabs() {
    let Some(ui) = cur() else { return };
    let tabs: Vec<TabPref> = ui
        .pages
        .borrow()
        .iter()
        .map(|p| {
            let p = p.borrow();
            TabPref {
                session: p.id.clone(),
                split: p.split.as_ref().map(|(s, _)| s.borrow().id.clone()),
                split_down: p.split.as_ref().is_some_and(|(_, d)| *d),
            }
        })
        .collect();
    let active = ui.active.get();
    prefs::update(|p| {
        p.tabs = tabs;
        p.active = active;
    });
}

/// Reopen last time's tabs whose sessions still exist.
fn restore_tabs() {
    let Some(ui) = cur() else { return };
    let saved = prefs::get();
    for t in &saved.tabs {
        if ui.sessions.borrow().contains_key(&t.session) {
            open_session(&t.session);
            if let Some(other) = t.split.as_ref().and_then(|o| ui.sessions.borrow().get(o).cloned())
                && let Some(p) = ui.pages.borrow().iter().find(|p| p.borrow().id == t.session)
            {
                p.borrow_mut().set_split(Some((&other, t.split_down)), &ui.actions);
            }
        }
    }
    let n = ui.pages.borrow().len();
    if n > 0 {
        select_tab(saved.active.min(n - 1));
    }
}

/// Open a plain terminal in the session's folder beside (or below) it.
fn split(id: &str, down: bool) {
    let Some(ui) = cur() else { return };
    let Some(p) = ui.pages.borrow().iter().find(|p| p.borrow().id == id).cloned() else { return };
    if p.borrow().split.is_some() {
        return;
    }
    let cwd = p.borrow().cwd().to_string();
    let id = id.to_string();
    daemon::request(
        move |c| c.launch(Launch { harness: "shell".into(), cwd, title: Some("Terminal".into()), ..Default::default() }),
        move |r| match r {
            Ok(s) => {
                let Some(ui) = cur() else { return };
                ui.sessions.borrow_mut().insert(s.id.clone(), s.clone());
                if let Some(p) = ui.pages.borrow().iter().find(|p| p.borrow().id == id) {
                    p.borrow_mut().set_split(Some((&s, down)), &ui.actions);
                }
                save_tabs();
            }
            Err(e) => toast(&e),
        },
    );
}

fn unsplit(id: &str) {
    let Some(ui) = cur() else { return };
    let Some(p) = ui.pages.borrow().iter().find(|p| p.borrow().id == id).cloned() else { return };
    let other = p.borrow().split.as_ref().map(|(s, _)| s.borrow().id.clone());
    p.borrow_mut().set_split(None, &ui.actions);
    save_tabs();
    // A split's shell exists only for that split; end it.
    if let Some(other) = other
        && ui.sessions.borrow().get(&other).is_some_and(|s| s.harness == "shell")
    {
        daemon::request(
            move |c| {
                let _ = c.stop(&other, false);
                std::thread::sleep(std::time::Duration::from_millis(400));
                c.forget(&other)
            },
            |_| {},
        );
    }
}

fn set_folder(path: &str) {
    let Some(ui) = cur() else { return };
    *ui.folder.borrow_mut() = path.to_string();
    ui.folder_button.set_label(&nyx_core::paths::pretty(std::path::Path::new(path)));
}

/// The New session menu: a folder, and the installed harnesses to start there.
fn new_session_menu() -> (gtk::MenuButton, gtk::ListBox, gtk::Button) {
    let list = gtk::ListBox::new();
    list.add_css_class("new-session-list");
    list.set_selection_mode(gtk::SelectionMode::None);
    list.set_activate_on_single_click(true);
    let folder = gtk::Button::with_label("~");
    folder.set_tooltip_text(Some("Choose the folder the agent starts in"));
    folder.connect_clicked(|_| {
        let Some(ui) = cur() else { return };
        let dialog = gtk::FileDialog::builder().title("Start the agent in").modal(true).build();
        dialog.set_initial_folder(Some(&gio::File::for_path(&*ui.folder.borrow())));
        ui.new_button.popdown();
        dialog.select_folder(Some(&ui.window), gio::Cancellable::NONE, |r| {
            if let Ok(f) = r
                && let Some(p) = f.path()
            {
                set_folder(&p.display().to_string());
                if let Some(ui) = cur() {
                    ui.new_button.popup();
                }
            }
        });
    });
    let heading = gtk::Label::new(Some("Start in"));
    heading.add_css_class("dim-label");
    heading.set_xalign(0.0);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    content.set_margin_top(8);
    content.set_margin_bottom(8);
    content.set_margin_start(8);
    content.set_margin_end(8);
    content.append(&heading);
    content.append(&folder);
    content.append(&list);
    let popover = gtk::Popover::new();
    popover.set_child(Some(&content));
    let button = gtk::MenuButton::new();
    button.set_icon_name("list-add-symbolic");
    button.set_tooltip_text(Some("New session (Ctrl+T)"));
    button.add_css_class("suggested-action");
    button.set_popover(Some(&popover));
    list.connect_row_activated(|_, row| {
        let id = row.widget_name().to_string();
        launch(&id);
    });
    (button, list, folder)
}

fn fill_new_menu() {
    let Some(ui) = cur() else { return };
    while let Some(c) = ui.new_list.first_child() {
        ui.new_list.remove(&c);
    }
    let last = prefs::get().last_harness;
    let mut list: Vec<Harness> = ui.harnesses.borrow().iter().filter(|h| h.path.is_some()).cloned().collect();
    list.sort_by_key(|h| (h.id != last, h.id == "shell", h.name.clone()));
    for h in list {
        let row = gtk::ListBoxRow::new();
        row.set_widget_name(&h.id);
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let name = gtk::Label::new(Some(&h.name));
        name.set_xalign(0.0);
        name.set_hexpand(true);
        b.append(&name);
        if let Some(v) = &h.version {
            let v = gtk::Label::new(Some(v));
            v.add_css_class("harness-version");
            v.set_ellipsize(gtk::pango::EllipsizeMode::End);
            v.set_max_width_chars(24);
            b.append(&v);
        }
        row.set_child(Some(&b));
        let c = &h.capabilities;
        let mut can = vec!["runs in a terminal"];
        if c.program_status {
            can.push("reports its status");
        }
        if c.resume {
            can.push("can be resumed");
        }
        if c.subagents.is_some() {
            can.push("subagents shown");
        }
        row.set_tooltip_text(Some(&format!("{}: {}", h.name, can.join(", "))));
        ui.new_list.append(&row);
    }
}

fn launch(harness: &str) {
    let Some(ui) = cur() else { return };
    ui.new_button.popdown();
    let cwd = ui.folder.borrow().clone();
    let harness = harness.to_string();
    prefs::update(|p| {
        p.last_folder = cwd.clone();
        if harness != "shell" {
            p.last_harness = harness.clone();
        }
    });
    daemon::request(
        move |c| c.launch(Launch { harness, cwd, ..Default::default() }),
        |r| match r {
            Ok(s) => {
                let Some(ui) = cur() else { return };
                ui.sessions.borrow_mut().insert(s.id.clone(), s.clone());
                open_session(&s.id);
                refresh_sidebar();
            }
            Err(e) => toast(&e),
        },
    );
}
