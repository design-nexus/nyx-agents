//! The window: the 2.0 header, the sidebar of sessions (needs you, active,
//! recent) and History, and a tab per open session.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use glib::translate::IntoGlib;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use nyx_agents_proto::{Activity, Event, Harness, Launch, Lifecycle, Session};
use nyx_ui::sidebar::Sidebar;
use nyx_ui::tabs::{Tab, TabRow};
use nyx_ui::{chrome, menu};

use super::daemon::{self, Msg};
use super::history::History;
use super::page::{self, Actions, Page, live};
use super::prefs::{self, TabPref};

/// Agents that get the computer-use tools for a desktop of their own.
const DESKTOP_AGENTS: &[&str] = &["claude", "codex", "muse"];

/// New session: an agent, a folder, two options and Start.
struct NewMenu {
    button: gtk::MenuButton,
    agents: gtk::FlowBox,
    folder_label: gtk::Label,
    folders: gtk::FlowBox,
    worktree: gtk::Switch,
    worktree_row: gtk::Box,
    desktop: gtk::Switch,
    desktop_row: gtk::Box,
    start: gtk::Button,
    choice: RefCell<String>,
}

/// The page with nothing open: the installed agents, one click to start.
struct StartPage {
    tiles: gtk::FlowBox,
    folder_label: gtk::Label,
}

struct Ui {
    window: gtk::ApplicationWindow,
    overlay: gtk::Overlay,
    sidebar: Rc<Sidebar>,
    tab_row: Rc<TabRow>,
    stack: gtk::Stack,
    search: gtk::SearchEntry,
    new: NewMenu,
    start: StartPage,
    history: Rc<History>,
    showing_history: Cell<bool>,
    folder: RefCell<String>,
    pages: RefCell<Vec<Rc<RefCell<Page>>>>,
    active: Cell<usize>,
    sessions: RefCell<HashMap<String, Session>>,
    harnesses: RefCell<Vec<Harness>>,
    /// Sessions asked to stop that are still running (Stop then forces).
    stopping: RefCell<HashSet<String>>,
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
/// `--open workspace/ID`: show that agent workspace's live view.
/// `--open history[/AGENT]`: show History, all agents or one.
pub fn open_target(target: &str) {
    let parts: Vec<&str> = target.split('/').collect();
    // Before the manager answers, restoring the saved tabs would replace it.
    if parts.first() == Some(&"history")
        && let Some(ui) = cur().filter(|ui| !ui.connected.get())
    {
        *ui.pending_open.borrow_mut() = Some(target.to_string());
        return;
    }
    match parts.as_slice() {
        ["workspace", id] => return show_workspace(id),
        ["history"] => return show_history(None),
        ["history", h] => return show_history(Some(h.to_string())),
        _ => {}
    }
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

fn open_settings() {
    if let Some(ui) = cur() {
        let harnesses = ui.harnesses.borrow().clone();
        super::settings::open(&ui.overlay, &ui.window, &harnesses);
    }
}

fn copy_text(text: &str) {
    if let Some(d) = gdk::Display::default() {
        d.clipboard().set_text(text);
        toast("Copied");
    }
}

fn open_git_at(root: &str) {
    if let Err(e) = std::process::Command::new("nyx-git").arg(root).spawn() {
        toast(&format!("Could not open Nyx Git: {e}"));
    }
}

/// The agent's name for a harness id: "Claude Code", "Terminal".
fn agent_name(id: &str) -> String {
    if id == "shell" {
        return "Terminal".into();
    }
    cur()
        .and_then(|ui| ui.harnesses.borrow().iter().find(|h| h.id == id).map(|h| h.name.clone()))
        .unwrap_or_else(|| id.to_string())
}

fn is_stopping(id: &str) -> bool {
    cur().is_some_and(|ui| ui.stopping.borrow().contains(id))
}

/// The tab under a point in the tab row.
fn tab_at(row: &gtk::Widget, x: f64, y: f64) -> Option<usize> {
    let mut w = row.pick(x, y, gtk::PickFlags::DEFAULT)?;
    while !w.has_css_class("tab") {
        w = w.parent()?;
        if w == *row {
            return None;
        }
    }
    let mut i = 0;
    let mut c = w.prev_sibling();
    while let Some(p) = c {
        i += 1;
        c = p.prev_sibling();
    }
    Some(i)
}

fn page_of(id: &str) -> Option<Rc<RefCell<Page>>> {
    cur().and_then(|ui| ui.pages.borrow().iter().find(|p| p.borrow().id == id).cloned())
}

/// What can be done with a session: its More menu and right-click menus.
fn session_entries(id: &str) -> Vec<menu::Entry> {
    let Some(ui) = cur() else { return Vec::new() };
    let Some(s) = ui.sessions.borrow().get(id).cloned() else { return Vec::new() };
    let live = live(&s);
    let failed = page::failed_start(&s);
    let stopping = is_stopping(id);
    let page = page_of(id);
    let host = ui
        .pages
        .borrow()
        .iter()
        .find(|p| p.borrow().split.as_ref().is_some_and(|(o, _)| o.borrow().id == id))
        .map(|p| p.borrow().id.clone());
    let id = id.to_string();
    let mut e = Vec::new();
    if let Some(p) = page.clone() {
        e.push(menu::item("Find", move || p.borrow().open_find()).icon("system-search-symbolic").key("Ctrl+F"));
    }
    if let Some(host) = host {
        e.push(menu::item("Close this terminal", move || unsplit(&host)).icon("window-close-symbolic"));
    } else if live && page.as_ref().is_none_or(|p| p.borrow().split.is_none()) {
        let (a, b) = (id.clone(), id.clone());
        e.push(
            menu::item("Terminal beside", move || {
                open_session(&a);
                split(&a, false);
            })
            .icon("view-dual-symbolic")
            .key("Ctrl+Shift+O"),
        );
        e.push(
            menu::item("Terminal below", move || {
                open_session(&b);
                split(&b, true);
            })
            .icon("view-paged-symbolic")
            .key("Ctrl+Shift+E"),
        );
    }
    if let Some(p) = page.clone()
        && p.borrow().inspector.has_subagents()
    {
        let shown = !p.borrow().inspector.is_hidden();
        e.push(menu::check("Subagents", shown, move |on| p.borrow().inspector.set_hidden(!on)));
    }
    if let Some(w) = s.workspace.clone().filter(|_| live) {
        if let Some(p) = page.clone() {
            let (shown, w2, a) = (p.borrow().desktop_shown(), w.clone(), ui.actions.clone());
            e.push(menu::check("Desktop", shown, move |on| p.borrow().set_desktop(on.then_some(w2.as_str()), &a)));
        }
        e.push(menu::item("Open the desktop in a window", move || show_workspace(&w)).icon("window-new-symbolic"));
    }
    e.push(menu::separator());
    if let Some(root) = s.worktree.as_ref().map(|w| w.root.clone()) {
        e.push(menu::item("Open in Nyx Git", move || open_git_at(&root)).icon("nyx-agents-branch-symbolic"));
    }
    let cwd = s.cwd.clone();
    e.push(menu::item("Copy folder path", move || copy_text(&cwd)).icon("folder-symbolic"));
    let rid = id.clone();
    e.push(menu::item("Rename…", move || rename(&rid)).icon("document-edit-symbolic").key("F2"));
    e.push(menu::separator());
    if live {
        if !stopping {
            let sid = id.clone();
            let label = if s.harness == "shell" { "Close" } else { "Stop" };
            e.push(menu::item(label, move || stop(&sid)).icon("media-playback-stop-symbolic"));
        }
        let fid = id.clone();
        e.push(menu::item("Force stop", move || force_stop(&fid)).icon("process-stop-symbolic"));
    } else if failed {
        let rid = id.clone();
        e.push(menu::item("Try again", move || retry(&rid)).icon("view-refresh-symbolic"));
    } else if s.resumable {
        let rid = id.clone();
        e.push(menu::item("Resume", move || resume(&rid)).icon("media-playback-start-symbolic"));
    }
    e.push(menu::item("Discard…", move || discard(&id)).icon("user-trash-symbolic").danger());
    e
}

fn show_workspace(id: &str) {
    let parent = cur().map(|ui| ui.window.clone().upcast::<gtk::Window>());
    super::workspace::open(id, parent, toast);
}

/// Ask the session to stop; asked again while it still runs, end it now.
fn stop(id: &str) {
    let Some(ui) = cur() else { return };
    let force = ui.stopping.borrow().contains(id);
    ui.stopping.borrow_mut().insert(id.to_string());
    refresh_session(id);
    let sid = id.to_string();
    daemon::request(
        move |c| c.stop(&sid, force),
        |r| {
            if let Err(e) = r {
                toast(&e);
            }
        },
    );
    if force {
        return;
    }
    let id = id.to_string();
    glib::timeout_add_local_once(std::time::Duration::from_secs(4), move || {
        let Some(ui) = cur() else { return };
        let running = ui.sessions.borrow().get(&id).is_some_and(live);
        if running && ui.stopping.borrow().contains(&id) {
            let who = ui.sessions.borrow().get(&id).map(|s| agent_name(&s.harness)).unwrap_or_default();
            toast(&format!("{who} is still running. Press Force stop to end it now."));
        }
    });
}

fn force_stop(id: &str) {
    if let Some(ui) = cur() {
        ui.stopping.borrow_mut().insert(id.to_string());
    }
    stop(id);
}

fn resume(id: &str) {
    let id = id.to_string();
    daemon::request(
        move |c| c.resume(&id),
        |r| {
            if let Err(e) = r {
                toast(&e);
            }
        },
    );
}

/// Start a session that didn't start again, in the same folder, in its place.
fn retry(id: &str) {
    let Some(ui) = cur() else { return };
    let Some(s) = ui.sessions.borrow().get(id).cloned() else { return };
    let old = id.to_string();
    let l = Launch { harness: s.harness.clone(), cwd: s.cwd.clone(), title: Some(s.title.clone()), workspace: s.workspace.clone(), ..Default::default() };
    daemon::request(
        move |c| {
            let new = c.launch(l)?;
            let _ = c.forget(&old);
            Ok((new, old))
        },
        |r| match r {
            Ok((s, old)) => {
                close_session_tab(&old);
                if let Some(ui) = cur() {
                    ui.sessions.borrow_mut().entry(s.id.clone()).or_insert_with(|| s.clone());
                }
                open_session(&s.id);
            }
            Err(e) => toast(&e),
        },
    );
}

fn rename(id: &str) {
    let Some(ui) = cur() else { return };
    let Some(title) = ui.sessions.borrow().get(id).map(|s| s.title.clone()) else { return };
    let id = id.to_string();
    nyx_ui::widgets::ask_text("Rename session", "", &title, "Rename", move |t| {
        let id = id.clone();
        daemon::request(
            move |c| c.call::<Session>(nyx_agents_proto::Call::Rename { id, title: t }),
            |r| {
                if let Err(e) = r {
                    toast(&e);
                }
            },
        );
    });
}

/// A session's own worktree under Agents' worktree folder, if it still exists.
fn own_worktree(s: &Session) -> Option<String> {
    let w = s.worktree.as_ref().filter(|w| w.linked)?;
    let root = std::path::Path::new(&w.root);
    (root.starts_with(nyx_agents_core::worktree::root()) && root.exists()).then(|| w.root.clone())
}

/// Remove the session from Agents, once asked; a live one is stopped first.
fn discard(id: &str) {
    let Some(ui) = cur() else { return };
    let Some(s) = ui.sessions.borrow().get(id).cloned() else { return };
    let worktree = own_worktree(&s);
    // Nothing to lose: a start that failed, or an ended terminal.
    if worktree.is_none() && !live(&s) && (page::failed_start(&s) || s.harness == "shell") {
        return discard_now(id, false);
    }
    let (dialog, card) = nyx_ui::widgets::dialog(&format!("Discard “{}”?", s.title), 440);
    let mut text = String::from("Removes the session from Agents with its saved screen.");
    if s.harness != "shell" {
        text.push_str(&format!(" The conversation stays in {}'s own history.", agent_name(&s.harness)));
    }
    if live(&s) {
        text.push_str(" It is still running; Agents stops it first.");
    }
    let desc = nyx_ui::widgets::label(&text, "dim");
    desc.set_wrap(true);
    desc.set_xalign(0.0);
    card.append(&desc);
    let check = gtk::CheckButton::with_label("Also remove its worktree");
    if let Some(root) = &worktree {
        check.set_active(true);
        check.set_tooltip_text(Some(&format!("{} (only if it has no uncommitted changes; the branch is kept)", nyx_core::paths::pretty(std::path::Path::new(root)))));
        card.append(&check);
    }
    let buttons = nyx_ui::widgets::hbox(8);
    buttons.set_halign(gtk::Align::End);
    buttons.set_margin_top(6);
    let cancel = gtk::Button::with_label("Cancel");
    let ok = gtk::Button::with_label("Discard");
    ok.add_css_class("destructive-action");
    buttons.append(&cancel);
    buttons.append(&ok);
    card.append(&buttons);
    let d = dialog.clone();
    cancel.connect_clicked(move |_| d.close());
    let (d, id) = (dialog.clone(), id.to_string());
    let has_worktree = worktree.is_some();
    ok.connect_clicked(move |_| {
        d.close();
        discard_now(&id, has_worktree && check.is_active());
    });
    dialog.present();
    ok.grab_focus();
}

/// Wait until the session no longer runs; false if it still does.
fn wait_ended(c: &mut nyx_agents_client::Client, id: &str, secs: u64) -> bool {
    for _ in 0..secs * 10 {
        match c.session(id) {
            Ok(s) if live(&s) => std::thread::sleep(std::time::Duration::from_millis(100)),
            _ => return true,
        }
    }
    false
}

/// Stop the session if it runs, then forget it.
fn end_and_forget(c: &mut nyx_agents_client::Client, id: &str) -> nyx_agents_client::Result<()> {
    if c.session(id).is_ok_and(|s| live(&s)) {
        c.stop(id, false)?;
        if !wait_ended(c, id, 5) {
            c.stop(id, true)?;
            wait_ended(c, id, 3);
        }
    }
    let mut last = Ok(serde_json::Value::Null);
    for _ in 0..10 {
        last = c.forget(id);
        if last.is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    last.map(drop)
}

fn discard_now(id: &str, remove_worktree: bool) {
    close_session_tab(id);
    let id = id.to_string();
    daemon::request(
        move |c| {
            let mut note = None;
            if c.session(&id).is_ok_and(|s| live(&s)) {
                c.stop(&id, false)?;
                if !wait_ended(c, &id, 5) {
                    c.stop(&id, true)?;
                    wait_ended(c, &id, 3);
                }
            }
            if remove_worktree
                && let Err(e) = c.call::<serde_json::Value>(nyx_agents_proto::Call::RemoveWorktree { id: id.clone() })
            {
                note = Some(format!("The worktree was kept: {e}"));
            }
            end_and_forget(c, &id)?;
            Ok(note)
        },
        |r| match r {
            Ok(Some(note)) => toast(&note),
            Ok(None) => {}
            Err(e) => toast(&e),
        },
    );
}

fn actions() -> Rc<Actions> {
    Rc::new(Actions {
        stop: Rc::new(stop),
        resume: Rc::new(resume),
        retry: Rc::new(retry),
        discard: Rc::new(discard),
        rename: Rc::new(rename),
        unsplit: Rc::new(unsplit),
        show_workspace: Rc::new(show_workspace),
        ended: Rc::new(|_| {}),
        subagent_source: Rc::new(|harness| {
            cur().and_then(|ui| ui.harnesses.borrow().iter().find(|h| h.id == harness).and_then(|h| h.capabilities.subagents))
        }),
        session_menu: Rc::new(session_entries),
        agent_name: Rc::new(agent_name),
        stopping: Rc::new(is_stopping),
    })
}

fn build(app: &gtk::Application) {
    let window =
        gtk::ApplicationWindow::builder().application(app).title("Agents").default_width(1280).default_height(800).build();
    window.add_css_class("agents-window");
    window.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));
    window.set_icon_name(Some("io.github.design_nexus.nyx.Agents"));
    nyx_ui::keys::install(&window, nyx_ui::keys::Common { close: false, settings: Some(open_settings), ..Default::default() });

    let sidebar = Sidebar::new();
    sidebar.follow_window(&window);
    sidebar.set_collapsed(prefs::get().sidebar_collapsed);

    let stack = gtk::Stack::new();
    stack.set_hexpand(true);
    stack.set_vexpand(true);
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.set_transition_duration(if prefs::get().reduce_motion { 0 } else { 120 });
    let (start_root, start) = start_page();
    stack.add_named(&start_root, Some("empty"));
    let history = History::new(
        Rc::new(agent_name),
        Rc::new(|s: Session| {
            if let Some(ui) = cur() {
                ui.sessions.borrow_mut().entry(s.id.clone()).or_insert_with(|| s.clone());
            }
            open_session(&s.id);
        }),
        Rc::new(open_session),
        Rc::new(toast),
    );
    stack.add_named(&history.root, Some("history"));

    let tab_row = TabRow::new("New session (Ctrl+T)");
    tab_row.connect_select(select_tab);
    tab_row.connect_close(close_tab);
    tab_row.connect_reorder(reorder);
    menu::on_right_click(&tab_row.root, |w, x, y| {
        let Some(i) = tab_at(w, x, y) else { return };
        let Some(id) = cur().and_then(|ui| ui.pages.borrow().get(i).map(|p| p.borrow().id.clone())) else { return };
        let mut entries = vec![menu::item("Close tab", move || close_tab(i)).icon("window-close-symbolic")];
        entries.push(menu::separator());
        entries.extend(session_entries(&id));
        menu::popup_at(w, x, y, entries);
    });

    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    column.set_hexpand(true);
    column.append(&tab_row.root);
    column.append(&stack);

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
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search sessions  (Ctrl+Shift+F)"));
    search.add_css_class("bar-search");
    search.set_width_chars(28);
    search.update_property(&[gtk::accessible::Property::Label("Search sessions")]);
    search.connect_search_changed(|_| refresh_sidebar());
    search.connect_stop_search(|e| {
        e.set_text("");
        focus_active();
    });
    search.connect_activate(|e| {
        // Enter opens the first match.
        let q = e.text().to_lowercase();
        let Some(ui) = cur() else { return };
        let mut list: Vec<Session> = ui.sessions.borrow().values().filter(|s| matches(s, &q)).cloned().collect();
        list.sort_by_key(rank);
        if let Some(s) = list.first() {
            let id = s.id.clone();
            e.set_text("");
            open_session(&id);
        }
    });
    header.center.append(&search);

    let new = new_session_menu();
    header.end.append(&new.button);
    header.end.append(&chrome::settings_button(open_settings));
    {
        let nb = new.button.clone();
        tab_row.connect_new(move || nb.popup());
    }

    // The tabs sit at the top of the page; the terminals reach the sheet's edges.
    let frame = nyx_ui::frame::AppFrame::new(&header.root);
    frame.set_sidebar(&sidebar.root);
    frame.set_page(&column);
    frame.set_flush(true);
    let overlay = frame.install(&window);

    let folder = {
        let last = prefs::get().last_folder;
        if !last.is_empty() && std::path::Path::new(&last).is_dir() {
            last
        } else {
            nyx_core::paths::home().display().to_string()
        }
    };
    let ui = Rc::new(Ui {
        window: window.clone(),
        overlay,
        sidebar,
        tab_row,
        stack,
        search,
        new,
        start,
        history,
        showing_history: Cell::new(false),
        folder: RefCell::new(String::new()),
        pages: RefCell::new(Vec::new()),
        active: Cell::new(0),
        sessions: RefCell::new(HashMap::new()),
        harnesses: RefCell::new(Vec::new()),
        stopping: RefCell::new(HashSet::new()),
        connected: Cell::new(false),
        restored: Cell::new(false),
        actions: actions(),
        pending_open: RefCell::new(None),
        viewing: RefCell::new(None),
    });
    UI.with(|u| *u.borrow_mut() = Some(ui));
    set_folder(&folder);

    install_keys(&window);
    nyx_theme::subscribe(&window, || {
        if let Some(ui) = cur() {
            for p in ui.pages.borrow().iter() {
                p.borrow().restyle();
            }
        }
    });
    window.connect_is_active_notify(|_| report_viewing());
    window.connect_default_width_notify(|w| {
        if nyx_ui::settings_card::is_open() {
            nyx_ui::settings_card::fit(w);
        }
    });
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
        let active = if ui.showing_history.get() { None } else { ui.pages.borrow().get(ui.active.get()).cloned() };
        match key.to_lower() {
            gdk::Key::Escape if nyx_ui::settings_card::is_open() => nyx_ui::settings_card::escape(),
            gdk::Key::t if ctrl && !shift => ui.new.button.popup(),
            gdk::Key::f if ctrl && shift => {
                ui.search.grab_focus();
            }
            gdk::Key::f if ctrl && !shift && ui.showing_history.get() => ui.history.focus_search(),
            gdk::Key::f if ctrl && !shift => {
                if let Some(p) = active {
                    p.borrow().open_find();
                }
            }
            gdk::Key::F2 if !ctrl && !shift => {
                if let Some(p) = active {
                    let id = p.borrow().id.clone();
                    rename(&id);
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
            ui.stopping.borrow_mut().retain(|id| ui.sessions.borrow().get(id).is_some_and(live));
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
            if !live(&session) {
                ui.stopping.borrow_mut().remove(&session.id);
            }
            ui.sessions.borrow_mut().insert(session.id.clone(), (*session).clone());
            update_pages(&session);
        }
        Msg::Event(Event::Removed { id }) => {
            ui.sessions.borrow_mut().remove(&id);
            ui.stopping.borrow_mut().remove(&id);
            close_session_tab(&id);
            let hosts: Vec<Rc<RefCell<Page>>> = ui
                .pages
                .borrow()
                .iter()
                .filter(|p| p.borrow().split.as_ref().is_some_and(|(o, _)| o.borrow().id == id))
                .cloned()
                .collect();
            for p in hosts {
                p.borrow_mut().set_split(None, &ui.actions);
                save_tabs();
            }
        }
        Msg::Event(Event::Harnesses { harnesses }) => {
            *ui.harnesses.borrow_mut() = harnesses;
            fill_new_menu();
        }
        Msg::Event(Event::Workspace { workspace }) => super::workspace::changed(&workspace),
        Msg::Event(Event::WorkspaceRemoved { id }) => super::workspace::removed(&id),
        // The app has no usage view; the AI Hub shows it.
        Msg::Event(Event::Usage { .. }) => return,
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

/// Redraw what shows one session after a change made here (Stop pressed).
fn refresh_session(id: &str) {
    let Some(s) = cur().and_then(|ui| ui.sessions.borrow().get(id).cloned()) else { return };
    update_pages(&s);
    refresh_sidebar();
    refresh_tabs();
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

/// The icon and its colour class for a session's state.
fn status_icon(s: &Session) -> (&'static str, &'static str) {
    match (s.lifecycle, s.status.activity) {
        _ if page::failed_start(s) => ("dialog-error-symbolic", "failed"),
        (Lifecycle::Running, Activity::Blocked) => ("dialog-warning-symbolic", "blocked"),
        (Lifecycle::Running, Activity::Error) => ("dialog-warning-symbolic", "blocked"),
        (Lifecycle::Running, Activity::Done) => ("object-select-symbolic", "done"),
        (Lifecycle::Running, Activity::Working) => ("emblem-synchronizing-symbolic", "working"),
        (Lifecycle::Running | Lifecycle::Starting, _) => ("utilities-terminal-symbolic", ""),
        (Lifecycle::Lost, _) => ("network-offline-symbolic", ""),
        _ if s.exit_code == Some(0) => ("object-select-symbolic", ""),
        _ => ("media-playback-stop-symbolic", ""),
    }
}

/// A session matches a search by title, agent, folder or branch.
fn matches(s: &Session, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let branch = s.worktree.as_ref().and_then(|w| w.branch.as_deref()).unwrap_or_default();
    let agent = agent_name(&s.harness);
    [s.title.as_str(), s.harness.as_str(), agent.as_str(), s.cwd.as_str(), branch].iter().any(|f| f.to_lowercase().contains(query))
}

/// Give the keyboard back to the session on screen.
fn focus_active() {
    if let Some(ui) = cur()
        && let Some(p) = ui.pages.borrow().get(ui.active.get())
    {
        p.borrow().focus();
    }
}

/// What a sidebar row says under its title: the agent, what it is doing, where.
fn row_subtitle(s: &Session) -> String {
    let mut bits = vec![agent_name(&s.harness)];
    let (state, _) = page::status_text(s, is_stopping(&s.id));
    let running_subs = s.subagents.iter().filter(|a| a.state == nyx_agents_proto::SubagentState::Running).count();
    bits.push(match (state, running_subs) {
        ("Working", n) if n > 0 => format!("working · {n} subagent{}", if n == 1 { "" } else { "s" }),
        ("Running", _) => String::new(),
        _ if !live(s) && s.exit_code == Some(0) && s.harness != "shell" => "finished".into(),
        (state, _) => state.to_lowercase(),
    });
    if let Some(b) = s.worktree.as_ref().and_then(|w| w.branch.clone()).filter(|_| !page::failed_start(s)) {
        bits.push(b);
    }
    bits.retain(|b| !b.is_empty());
    bits.join(" · ")
}

fn session_row(s: &Session, active: bool) -> gtk::Button {
    let b = gtk::Button::new();
    b.add_css_class("nav-item");
    b.add_css_class("session-row");
    if active {
        b.add_css_class("active");
    }
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let (icon, kind) = status_icon(s);
    let image = gtk::Image::from_icon_name(icon);
    image.add_css_class("nav-icon");
    if !kind.is_empty() {
        image.add_css_class(kind);
    }
    image.set_valign(gtk::Align::Start);
    content.append(&image);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    text.add_css_class("compact-hide");
    text.set_hexpand(true);
    let title = nyx_ui::widgets::label(&s.title, "nav-label");
    title.set_xalign(0.0);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let sub = nyx_ui::widgets::label(&row_subtitle(s), "session-sub");
    sub.set_xalign(0.0);
    sub.set_ellipsize(gtk::pango::EllipsizeMode::End);
    text.append(&title);
    text.append(&sub);
    content.append(&text);
    b.set_child(Some(&content));
    b.set_tooltip_text(Some(&format!("{}\n{}", s.title, nyx_core::paths::pretty(std::path::Path::new(&s.cwd)))));
    b.update_property(&[gtk::accessible::Property::Label(&s.title)]);
    b
}

fn refresh_sidebar() {
    let Some(ui) = cur() else { return };
    let sb = &ui.sidebar;
    sb.clear();
    let active_id = (!ui.showing_history.get()).then(|| ui.pages.borrow().get(ui.active.get()).map(|p| p.borrow().id.clone())).flatten();
    // Terminals opened beside an agent belong to its tab; every other session is listed.
    let splits: HashSet<String> =
        ui.pages.borrow().iter().filter_map(|p| p.borrow().split.as_ref().map(|(o, _)| o.borrow().id.clone())).collect();
    let query = ui.search.text().to_lowercase();
    let mut list: Vec<Session> =
        ui.sessions.borrow().values().filter(|s| !splits.contains(&s.id) && matches(s, &query)).cloned().collect();
    list.sort_by_key(rank);
    let live_list: Vec<&Session> = list.iter().filter(|s| live(s)).collect();
    let mut first = true;
    let group = |title: &str, items: Vec<&Session>, first: &mut bool| {
        if items.is_empty() {
            return;
        }
        sb.append(&nyx_ui::sidebar::group_label(title, *first));
        *first = false;
        for s in items {
            let item = session_row(s, active_id.as_deref() == Some(s.id.as_str()));
            let id = s.id.clone();
            item.connect_clicked(move |_| open_session(&id));
            let id = s.id.clone();
            menu::on_right_click(&item, move |w, x, y| {
                let open = id.clone();
                let mut entries = vec![menu::item("Open", move || open_session(&open)).icon("go-next-symbolic")];
                entries.push(menu::separator());
                entries.extend(session_entries(&id));
                menu::popup_at(w, x, y, entries);
            });
            sb.append(&item);
        }
    };
    group("Needs you", live_list.iter().filter(|s| s.status.activity == Activity::Blocked).copied().collect(), &mut first);
    group("Active", live_list.iter().filter(|s| s.status.activity != Activity::Blocked).copied().collect(), &mut first);
    let mut ended: Vec<&Session> = list.iter().filter(|s| !live(s)).collect();
    ended.sort_by_key(|s| std::cmp::Reverse(s.ended.unwrap_or(s.created)));
    if query.is_empty() {
        ended.truncate(8);
    }
    group("Recent", ended, &mut first);
    if !query.is_empty() && list.is_empty() {
        let hint = gtk::Label::new(Some("No session matches"));
        hint.add_css_class("dim-label");
        hint.set_margin_start(12);
        sb.append(&hint);
    }
    if !ui.connected.get() && ui.sessions.borrow().is_empty() {
        let hint = gtk::Label::new(Some("Connecting to the Nyx Agent Manager…"));
        hint.add_css_class("dim-label");
        hint.set_wrap(true);
        hint.set_margin_start(12);
        hint.set_margin_end(12);
        sb.append(&hint);
    }
    let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
    sep.add_css_class("sidebar-sep");
    sb.append(&sep);
    let history = nyx_ui::sidebar::nav_item("document-open-recent-symbolic", "History", "Every agent's past sessions");
    history.set_active(ui.showing_history.get());
    history.button.connect_clicked(|_| show_history(None));
    sb.append(&history.button);
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
                icon: s.map(|s| status_icon(s).0.to_string()),
                tooltip: s.map(|s| format!("{} · {}", agent_name(&s.harness), page::status_text(s, is_stopping(&s.id)).0)),
                dirty: s.is_some_and(|s| s.lifecycle == Lifecycle::Running && s.status.activity == Activity::Blocked),
            }
        })
        .collect();
    drop(sessions);
    let n = tabs.len();
    ui.tab_row.set_tabs(&tabs, ui.active.get().min(n.saturating_sub(1)));
    // With nothing open the start page offers the agents; a lone `+` would float under the header.
    ui.tab_row.root.set_visible(n > 0);
    if n == 0 && !ui.showing_history.get() {
        ui.stack.set_visible_child_name("empty");
        chrome::set_title(&ui.window, "Agents", "");
    }
}

/// Show History in the page area, all agents or one.
fn show_history(harness: Option<String>) {
    let Some(ui) = cur() else { return };
    ui.showing_history.set(true);
    ui.stack.set_visible_child_name("history");
    chrome::set_title(&ui.window, "Agents", "History");
    ui.history.set_filter(harness);
    ui.history.load();
    report_viewing();
    refresh_sidebar();
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
    ui.showing_history.set(false);
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
    let id = (ui.window.is_active() && !ui.showing_history.get())
        .then(|| ui.pages.borrow().get(ui.active.get()).map(|p| p.borrow().id.clone()))
        .flatten();
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
    if ui.showing_history.get() {
        ui.active.set(ui.active.get().min(n.saturating_sub(1)));
    } else if n > 0 {
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
    let Some(p) = page_of(id) else { return };
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
                // The reply was built before the program started; events may be newer.
                let s = ui.sessions.borrow_mut().entry(s.id.clone()).or_insert(s).clone();
                if let Some(p) = ui.pages.borrow().iter().find(|p| p.borrow().id == id) {
                    p.borrow_mut().set_split(Some((&s, down)), &ui.actions);
                }
                save_tabs();
                refresh_sidebar();
            }
            Err(e) => toast(&e),
        },
    );
}

/// Close the terminal beside a session: it exists only for that split, so end it.
fn unsplit(id: &str) {
    let Some(ui) = cur() else { return };
    let Some(p) = page_of(id) else { return };
    let other = p.borrow().split.as_ref().map(|(s, _)| s.borrow().id.clone());
    p.borrow_mut().set_split(None, &ui.actions);
    save_tabs();
    if let Some(other) = other
        && ui.sessions.borrow().get(&other).is_some_and(|s| s.harness == "shell")
    {
        daemon::request(
            move |c| end_and_forget(c, &other),
            |r| {
                if let Err(e) = r {
                    toast(&e);
                }
            },
        );
    }
}

/// The folders recent sessions ran in (worktrees as their repository), newest first.
fn recent_folders() -> Vec<String> {
    let Some(ui) = cur() else { return Vec::new() };
    let mut list: Vec<Session> = ui.sessions.borrow().values().filter(|s| s.harness != "shell").cloned().collect();
    list.sort_by_key(|s| std::cmp::Reverse(s.created));
    let mut out = vec![ui.folder.borrow().clone()];
    for s in list {
        let root = nyx_agents_core::git::context(std::path::Path::new(&s.cwd))
            .map(|(_, main)| main.display().to_string())
            .unwrap_or_else(|| s.cwd.clone());
        if !out.contains(&root) && std::path::Path::new(&root).is_dir() {
            out.push(root);
        }
        if out.len() >= 6 {
            break;
        }
    }
    out
}

fn fill_folder_chips() {
    let Some(ui) = cur() else { return };
    let chips = &ui.new.folders;
    while let Some(c) = chips.first_child() {
        chips.remove(&c);
    }
    let current = ui.folder.borrow().clone();
    for f in recent_folders() {
        let name = std::path::Path::new(&f).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| f.clone());
        let b = gtk::Button::with_label(&name);
        b.add_css_class("chip");
        if f == current {
            b.add_css_class("selected");
        }
        b.set_tooltip_text(Some(&nyx_core::paths::pretty(std::path::Path::new(&f))));
        b.connect_clicked(move |_| set_folder(&f));
        chips.insert(&b, -1);
    }
}

fn set_folder(path: &str) {
    let Some(ui) = cur() else { return };
    *ui.folder.borrow_mut() = path.to_string();
    let pretty = nyx_core::paths::pretty(std::path::Path::new(path));
    ui.new.folder_label.set_label(&pretty);
    ui.start.folder_label.set_label(&pretty);
    update_new_options();
    if ui.new.button.is_active() {
        fill_folder_chips();
    }
}

/// Pick the folder new sessions start in; `reopen` brings New session back after.
fn choose_folder(reopen: bool) {
    let Some(ui) = cur() else { return };
    let dialog = gtk::FileDialog::builder().title("Start the agent in").modal(true).build();
    dialog.set_initial_folder(Some(&gio::File::for_path(&*ui.folder.borrow())));
    ui.new.button.popdown();
    dialog.select_folder(Some(&ui.window), gio::Cancellable::NONE, move |r| {
        if let Ok(f) = r
            && let Some(p) = f.path()
        {
            set_folder(&p.display().to_string());
        }
        if reopen && let Some(ui) = cur() {
            ui.new.button.popup();
        }
    });
}

/// What New session offers for the chosen agent and folder.
fn update_new_options() {
    let Some(ui) = cur() else { return };
    let choice = ui.new.choice.borrow().clone();
    let in_repo = nyx_agents_core::git::context(std::path::Path::new(&*ui.folder.borrow())).is_some();
    ui.new.worktree_row.set_sensitive(in_repo);
    ui.new.worktree_row.set_tooltip_text((!in_repo).then_some("This folder isn't in a git repository"));
    ui.new.desktop_row.set_visible(DESKTOP_AGENTS.contains(&choice.as_str()));
    ui.new.start.set_label(&format!("Start {}", agent_name(&choice)));
    ui.new.start.set_sensitive(!choice.is_empty());
    let mut c = ui.new.agents.first_child();
    while let Some(child) = c {
        if let Some(b) = child.first_child() {
            if b.widget_name() == choice.as_str() {
                b.add_css_class("selected");
            } else {
                b.remove_css_class("selected");
            }
        }
        c = child.next_sibling();
    }
}

fn option_row(title: &str, desc: &str, switch: &gtk::Switch) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("new-option");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    let t = gtk::Label::new(Some(title));
    t.set_xalign(0.0);
    let d = gtk::Label::new(Some(desc));
    d.add_css_class("session-meta");
    d.set_xalign(0.0);
    d.set_wrap(true);
    d.set_max_width_chars(36);
    text.append(&t);
    text.append(&d);
    switch.set_valign(gtk::Align::Center);
    row.append(&text);
    row.append(switch);
    row
}

fn heading(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class("menu-heading");
    l.set_xalign(0.0);
    l
}

/// New session: the installed agents, the folder (recent ones as chips), Own
/// worktree, Own desktop, and Start. The options are kept for next time.
fn new_session_menu() -> NewMenu {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    content.add_css_class("new-session-menu");
    let agents = gtk::FlowBox::new();
    agents.set_selection_mode(gtk::SelectionMode::None);
    agents.set_min_children_per_line(2);
    agents.set_max_children_per_line(2);
    agents.set_homogeneous(true);
    agents.set_row_spacing(6);
    agents.set_column_spacing(6);
    let folder_label = gtk::Label::new(Some("~"));
    folder_label.set_xalign(0.0);
    folder_label.set_hexpand(true);
    folder_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    let folder_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    folder_box.append(&gtk::Image::from_icon_name("folder-symbolic"));
    folder_box.append(&folder_label);
    folder_box.append(&gtk::Image::from_icon_name("go-next-symbolic"));
    let folder = gtk::Button::new();
    folder.add_css_class("new-session-folder");
    folder.set_child(Some(&folder_box));
    folder.set_tooltip_text(Some("Choose the folder the agent starts in"));
    folder.connect_clicked(|_| choose_folder(true));
    let folders = gtk::FlowBox::new();
    folders.set_selection_mode(gtk::SelectionMode::None);
    folders.set_max_children_per_line(6);
    folders.set_row_spacing(4);
    folders.set_column_spacing(4);
    let p = prefs::get();
    let worktree = gtk::Switch::new();
    worktree.set_active(p.new_worktree);
    worktree.connect_active_notify(|s| prefs::update(|p| p.new_worktree = s.is_active()));
    let worktree_row = option_row("Own worktree", "A new branch in its own folder, so agents can work in parallel", &worktree);
    let desktop = gtk::Switch::new();
    desktop.set_active(p.new_desktop);
    desktop.connect_active_notify(|s| prefs::update(|p| p.new_desktop = s.is_active()));
    let desktop_row = option_row("Own desktop", "A separate screen it can drive; watch it beside the terminal and take over", &desktop);
    let start = gtk::Button::with_label("Start");
    start.add_css_class("suggested-action");
    let cancel = gtk::Button::with_label("Cancel");
    cancel.add_css_class("flat");
    let foot = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    foot.set_margin_top(6);
    let hint = gtk::Label::new(Some("Enter starts"));
    hint.add_css_class("session-meta");
    hint.set_hexpand(true);
    hint.set_xalign(0.0);
    foot.append(&hint);
    foot.append(&cancel);
    foot.append(&start);

    content.append(&heading("Agent"));
    content.append(&agents);
    content.append(&heading("Folder"));
    content.append(&folder);
    content.append(&folders);
    content.append(&heading("Options"));
    content.append(&worktree_row);
    content.append(&desktop_row);
    content.append(&foot);

    let popover = gtk::Popover::new();
    popover.set_has_arrow(false);
    popover.add_css_class("menu-popover");
    popover.set_child(Some(&content));
    popover.connect_show(|_| {
        fill_folder_chips();
        update_new_options();
    });
    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed(|_, key, _, _| {
        if matches!(key, gdk::Key::Return | gdk::Key::KP_Enter) {
            start_chosen();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    popover.add_controller(keys);
    let button = gtk::MenuButton::new();
    button.set_icon_name("list-add-symbolic");
    button.set_tooltip_text(Some("New session (Ctrl+T)"));
    button.add_css_class("bar-button");
    button.set_valign(gtk::Align::Center);
    button.set_popover(Some(&popover));
    let b = button.clone();
    cancel.connect_clicked(move |_| b.popdown());
    start.connect_clicked(|_| start_chosen());
    NewMenu {
        button,
        agents,
        folder_label,
        folders,
        worktree,
        worktree_row,
        desktop,
        desktop_row,
        start,
        choice: RefCell::new(String::new()),
    }
}

/// The page with nothing open.
fn start_page() -> (gtk::Box, StartPage) {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 14);
    root.add_css_class("start-page");
    root.set_valign(gtk::Align::Center);
    root.set_halign(gtk::Align::Center);
    root.set_vexpand(true);
    let title = gtk::Label::new(Some("Start a session"));
    title.add_css_class("empty-title");
    let desc = gtk::Label::new(Some("Pick an agent. It starts in this folder and opens here."));
    desc.add_css_class("dim");
    let folder_label = gtk::Label::new(Some("~"));
    folder_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    folder_label.set_max_width_chars(48);
    let fb = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    fb.append(&gtk::Image::from_icon_name("folder-symbolic"));
    fb.append(&folder_label);
    fb.append(&gtk::Image::from_icon_name("pan-down-symbolic"));
    let folder = gtk::Button::new();
    folder.add_css_class("new-session-folder");
    folder.set_child(Some(&fb));
    folder.set_halign(gtk::Align::Center);
    folder.set_tooltip_text(Some("Choose the folder"));
    folder.connect_clicked(|_| choose_folder(false));
    let tiles = gtk::FlowBox::new();
    tiles.set_selection_mode(gtk::SelectionMode::None);
    tiles.set_max_children_per_line(4);
    tiles.set_min_children_per_line(2);
    tiles.set_homogeneous(true);
    tiles.set_row_spacing(10);
    tiles.set_column_spacing(10);
    tiles.set_halign(gtk::Align::Center);
    let keys = gtk::Label::new(Some("Ctrl+T: New session with options  ·  Ctrl+Shift+F: Find a session"));
    keys.add_css_class("session-meta");
    root.append(&title);
    root.append(&desc);
    root.append(&folder);
    root.append(&tiles);
    root.append(&keys);
    (root, StartPage { tiles, folder_label })
}

/// The version number in a harness's `--version` line ("codex-cli 0.167.0" is "0.167.0").
fn short_version(v: &str) -> String {
    v.split(|c: char| c.is_whitespace() || c == '/')
        .map(|t| t.trim_start_matches('v').trim_matches(|c: char| c == '(' || c == ')' || c == ','))
        .find(|t| t.starts_with(|c: char| c.is_ascii_digit()))
        .unwrap_or(v)
        .to_string()
}

fn capabilities(h: &Harness) -> String {
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
    if DESKTOP_AGENTS.contains(&h.id.as_str()) {
        can.push("can drive a desktop");
    }
    format!("{}: {}", h.name, can.join(", "))
}

fn fill_new_menu() {
    let Some(ui) = cur() else { return };
    for b in [&ui.new.agents, &ui.start.tiles] {
        while let Some(c) = b.first_child() {
            b.remove(&c);
        }
    }
    let last = crate::settings::default_harness().filter(|h| !h.is_empty()).unwrap_or_else(|| prefs::get().last_harness);
    let mut list: Vec<Harness> = ui.harnesses.borrow().iter().filter(|h| h.path.is_some()).cloned().collect();
    list.sort_by_key(|h| (h.id != last, h.id == "shell", h.name.clone()));
    {
        let mut choice = ui.new.choice.borrow_mut();
        if !list.iter().any(|h| h.id == *choice) {
            *choice = list.first().map(|h| h.id.clone()).unwrap_or_default();
        }
    }
    for h in &list {
        let name = if h.id == "shell" { "Terminal".to_string() } else { h.name.clone() };
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let n = gtk::Label::new(Some(&name));
        n.set_xalign(0.0);
        n.set_hexpand(true);
        n.set_ellipsize(gtk::pango::EllipsizeMode::End);
        row.append(&n);
        if h.id == last {
            let l = gtk::Label::new(Some("last"));
            l.add_css_class("harness-version");
            row.append(&l);
        }
        let b = gtk::Button::new();
        b.add_css_class("new-agent");
        b.set_widget_name(&h.id);
        b.set_child(Some(&row));
        b.set_tooltip_text(Some(&capabilities(h)));
        let id = h.id.clone();
        b.connect_clicked(move |_| {
            if let Some(ui) = cur() {
                *ui.new.choice.borrow_mut() = id.clone();
            }
            update_new_options();
        });
        ui.new.agents.insert(&b, -1);

        let tile = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let t = gtk::Label::new(Some(&name));
        t.add_css_class("start-tile-name");
        t.set_xalign(0.0);
        let v = gtk::Label::new(Some(&match (&h.version, h.id.as_str()) {
            (_, "shell") => "your shell".to_string(),
            _ if h.id == last => "last used".to_string(),
            (Some(v), _) => short_version(v),
            (None, _) => String::new(),
        }));
        v.add_css_class("harness-version");
        v.set_xalign(0.0);
        v.set_ellipsize(gtk::pango::EllipsizeMode::End);
        tile.append(&t);
        tile.append(&v);
        let b = gtk::Button::new();
        b.add_css_class("start-tile");
        b.set_child(Some(&tile));
        b.set_tooltip_text(Some(&capabilities(h)));
        let id = h.id.clone();
        b.connect_clicked(move |_| launch(&id, false));
        ui.start.tiles.insert(&b, -1);
    }
    update_new_options();
}

fn start_chosen() {
    let Some(choice) = cur().map(|ui| ui.new.choice.borrow().clone()).filter(|c| !c.is_empty()) else { return };
    launch(&choice, true);
}

/// Start `harness` in the chosen folder; `with_options` applies Own worktree and Own desktop.
fn launch(harness: &str, with_options: bool) {
    let Some(ui) = cur() else { return };
    ui.new.button.popdown();
    let cwd = ui.folder.borrow().clone();
    let worktree = (with_options && ui.new.worktree.is_active() && ui.new.worktree_row.is_sensitive()).then(String::new);
    let workspace = (with_options && ui.new.desktop.is_active() && DESKTOP_AGENTS.contains(&harness)).then(|| "new".to_string());
    let harness = harness.to_string();
    prefs::update(|p| {
        p.last_folder = cwd.clone();
        if harness != "shell" {
            p.last_harness = harness.clone();
        }
    });
    daemon::request(
        move |c| c.launch(Launch { harness, cwd, worktree, workspace, ..Default::default() }),
        |r| match r {
            Ok(s) => {
                let Some(ui) = cur() else { return };
                // The reply was built before the program started; events may be newer.
                ui.sessions.borrow_mut().entry(s.id.clone()).or_insert_with(|| s.clone());
                open_session(&s.id);
                refresh_sidebar();
            }
            Err(e) => toast(&e),
        },
    );
}
