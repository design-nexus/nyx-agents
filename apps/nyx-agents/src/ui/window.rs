//! The window: the 2.0 header, the sidebar of sessions (needs you, active,
//! recent) and projects, and a tab per open session.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use glib::translate::IntoGlib;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use nyx_agents_proto::{Activity, Event, Harness, Launch, Lifecycle, Session};
use nyx_ui::sidebar::Sidebar;
use nyx_ui::tabs::{Tab, TabRow};
use nyx_ui::{chrome, menu};

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
    search: gtk::SearchEntry,
    new_list: gtk::ListBox,
    folder_label: gtk::Label,
    worktree_check: gtk::CheckButton,
    workspace_check: gtk::CheckButton,
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
/// `--open workspace/ID`: show that agent workspace's live view.
pub fn open_target(target: &str) {
    let parts: Vec<&str> = target.split('/').collect();
    if let ["workspace", id] = parts.as_slice() {
        show_workspace(id);
        return;
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

/// What can be done with a session, for its right-click menus.
fn session_entries(id: &str) -> Vec<menu::Entry> {
    let Some(ui) = cur() else { return Vec::new() };
    let Some(s) = ui.sessions.borrow().get(id).cloned() else { return Vec::new() };
    let live = matches!(s.lifecycle, Lifecycle::Running | Lifecycle::Starting);
    let (page, host) = {
        let pages = ui.pages.borrow();
        let page = pages.iter().find(|p| p.borrow().id == id).cloned();
        let host = pages
            .iter()
            .find(|p| p.borrow().split.as_ref().is_some_and(|(o, _)| o.borrow().id == id))
            .map(|p| p.borrow().id.clone());
        (page, host)
    };
    let id = id.to_string();
    let mut e = Vec::new();
    if let Some(host) = host {
        e.push(menu::item("Close this split", move || unsplit(&host)).icon("window-close-symbolic"));
    } else if live && page.as_ref().is_none_or(|p| p.borrow().split.is_none()) {
        let (a, b) = (id.clone(), id.clone());
        e.push(
            menu::item("Open a terminal beside", move || {
                open_session(&a);
                split(&a, false);
            })
            .icon("view-dual-symbolic")
            .key("Ctrl+Shift+O"),
        );
        e.push(
            menu::item("Open a terminal below", move || {
                open_session(&b);
                split(&b, true);
            })
            .icon("view-paged-symbolic")
            .key("Ctrl+Shift+E"),
        );
    }
    if let Some(p) = page {
        e.push(menu::item("Find…", move || p.borrow().open_find()).icon("system-search-symbolic").key("Ctrl+F"));
    }
    if let Some(w) = s.workspace.clone() {
        e.push(menu::item("Show the agent's workspace", move || show_workspace(&w)).icon("video-display-symbolic"));
    }
    if let Some(root) = s.worktree.as_ref().map(|w| w.root.clone()) {
        e.push(menu::item("Open in Nyx Git", move || open_git_at(&root)).icon("nyx-agents-branch-symbolic"));
    }
    let (rid, title, cwd) = (id.clone(), s.title.clone(), s.cwd.clone());
    e.push(
        menu::item("Rename…", move || {
            let id = rid.clone();
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
        })
        .icon("document-edit-symbolic"),
    );
    e.push(menu::item("Copy folder path", move || copy_text(&cwd)).icon("folder-symbolic"));
    e.push(menu::separator());
    let a = ui.actions.clone();
    if live {
        let (a, id) = (a.clone(), id.clone());
        let label = if s.harness == "shell" { "Close the terminal" } else { "Stop the agent" };
        e.push(menu::item(label, move || (a.stop)(&id)).icon("media-playback-stop-symbolic").danger());
    } else {
        if s.resumable {
            let (a, id) = (a.clone(), id.clone());
            e.push(menu::item("Resume", move || (a.resume)(&id)).icon("media-playback-start-symbolic"));
        }
        e.push(menu::item("Forget", move || (a.forget)(&id)).icon("user-trash-symbolic").danger());
    }
    e
}

fn show_workspace(id: &str) {
    let parent = cur().map(|ui| ui.window.clone().upcast::<gtk::Window>());
    super::workspace::open(id, parent, toast);
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
        show_workspace: Rc::new(show_workspace),
        open_git: Rc::new(|id| {
            let Some(ui) = cur() else { return };
            let Some(root) = ui.sessions.borrow().get(id).and_then(|s| s.worktree.as_ref().map(|w| w.root.clone())) else {
                return;
            };
            if let Err(e) = std::process::Command::new("nyx-git").arg(&root).spawn() {
                toast(&format!("Could not open Nyx Git: {e}"));
            }
        }),
        remove_worktree: Rc::new(|id| {
            let id = id.to_string();
            daemon::request(
                move |c| c.call::<serde_json::Value>(nyx_agents_proto::Call::RemoveWorktree { id }),
                |r| match r {
                    Ok(_) => toast("Worktree removed; its branch is kept"),
                    Err(e) => toast(&e),
                },
            );
        }),
        ended: Rc::new(|_| {}),
        subagent_source: Rc::new(|harness| {
            cur().and_then(|ui| ui.harnesses.borrow().iter().find(|h| h.id == harness).and_then(|h| h.capabilities.subagents))
        }),
        session_menu: Rc::new(session_entries),
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

    let (new_button, new_list, folder_label, worktree_check, workspace_check) = new_session_menu();
    header.end.append(&new_button);
    header.end.append(&chrome::settings_button(open_settings));
    {
        let nb = new_button.clone();
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
    folder_label.set_label(&nyx_core::paths::pretty(std::path::Path::new(&folder)));
    worktree_check.set_sensitive(nyx_agents_core::git::context(std::path::Path::new(&folder)).is_some());
    let ui = Rc::new(Ui {
        window: window.clone(),
        overlay,
        sidebar,
        tab_row,
        stack,
        new_button,
        search,
        new_list,
        folder_label,
        worktree_check,
        workspace_check,
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
        let active = ui.pages.borrow().get(ui.active.get()).cloned();
        match key.to_lower() {
            gdk::Key::Escape if nyx_ui::settings_card::is_open() => nyx_ui::settings_card::escape(),
            gdk::Key::t if ctrl && !shift => ui.new_button.popup(),
            gdk::Key::f if ctrl && shift => {
                ui.search.grab_focus();
            }
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

/// A session matches a search by title, agent, folder or branch.
fn matches(s: &Session, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let branch = s.worktree.as_ref().and_then(|w| w.branch.as_deref()).unwrap_or_default();
    [s.title.as_str(), s.harness.as_str(), s.cwd.as_str(), branch].iter().any(|f| f.to_lowercase().contains(query))
}

/// The agent's short name for the sidebar: "Claude" for Claude Code.
fn agent_name(harnesses: &[Harness], id: &str) -> String {
    harnesses
        .iter()
        .find(|h| h.id == id)
        .and_then(|h| h.name.split_whitespace().next().map(String::from))
        .unwrap_or_else(|| id.to_string())
}

/// Give the keyboard back to the session on screen.
fn focus_active() {
    if let Some(ui) = cur()
        && let Some(p) = ui.pages.borrow().get(ui.active.get())
    {
        p.borrow().focus();
    }
}

fn refresh_sidebar() {
    let Some(ui) = cur() else { return };
    let sb = &ui.sidebar;
    sb.clear();
    let active_id = ui.pages.borrow().get(ui.active.get()).map(|p| p.borrow().id.clone());
    // Terminals opened beside an agent belong to its tab; every other session is listed.
    let splits: std::collections::HashSet<String> =
        ui.pages.borrow().iter().filter_map(|p| p.borrow().split.as_ref().map(|(o, _)| o.borrow().id.clone())).collect();
    let query = ui.search.text().to_lowercase();
    let mut list: Vec<Session> =
        ui.sessions.borrow().values().filter(|s| !splits.contains(&s.id) && matches(s, &query)).cloned().collect();
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
            let agent = agent_name(&ui.harnesses.borrow(), &s.harness);
            if s.harness != "shell"
                && !s.title.to_lowercase().contains(&agent.to_lowercase())
                && let Some(content) = item.button.child().and_downcast::<gtk::Box>()
            {
                let tag = gtk::Label::new(Some(&agent));
                tag.add_css_class("nav-readout");
                tag.add_css_class("compact-hide");
                content.append(&tag);
            }
            let id = s.id.clone();
            item.button.connect_clicked(move |_| open_session(&id));
            let id = s.id.clone();
            menu::on_right_click(&item.button, move |w, x, y| {
                let open = id.clone();
                let mut entries = vec![menu::item("Open", move || open_session(&open)).icon("go-next-symbolic")];
                entries.push(menu::separator());
                entries.extend(session_entries(&id));
                menu::popup_at(w, x, y, entries);
            });
            sb.append(&item.button);
        }
    };
    group("Needs you", needs.iter().map(|s| **s).collect(), &mut first);
    group("Active", live.iter().filter(|s| s.status.activity != Activity::Blocked).copied().collect(), &mut first);
    let mut ended: Vec<&Session> =
        list.iter().filter(|s| !matches!(s.lifecycle, Lifecycle::Running | Lifecycle::Starting)).collect();
    ended.sort_by_key(|s| std::cmp::Reverse(s.ended.unwrap_or(s.created)));
    if query.is_empty() {
        ended.truncate(8);
    }
    group("Recent", ended, &mut first);

    // Projects: folders of past sessions, for starting another there.
    let mut projects: Vec<(String, String)> = Vec::new();
    for s in &list {
        // Worktree sessions belong to their main repository.
        let root = nyx_agents_core::git::context(std::path::Path::new(&s.cwd))
            .map(|(_, main)| main.display().to_string())
            .unwrap_or_else(|| s.cwd.clone());
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
            let r = root.clone();
            item.button.connect_clicked(move |_| {
                set_folder(&r);
                if let Some(ui) = cur() {
                    ui.new_button.popup();
                }
            });
            menu::on_right_click(&item.button, move |w, x, y| {
                let (a, b, c) = (root.clone(), root.clone(), root.clone());
                let entries = vec![
                    menu::item("New session here…", move || {
                        set_folder(&a);
                        if let Some(ui) = cur() {
                            ui.new_button.popup();
                        }
                    })
                    .icon("list-add-symbolic"),
                    menu::item("Open in Nyx Git", move || open_git_at(&b)).icon("nyx-agents-branch-symbolic"),
                    menu::item("Copy path", move || copy_text(&c)).icon("edit-copy-symbolic"),
                ];
                menu::popup_at(w, x, y, entries);
            });
            sb.append(&item.button);
        }
    }
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
    // With nothing open the empty page offers New session; a lone `+` would float under the header.
    ui.tab_row.root.set_visible(n > 0);
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
    ui.folder_label.set_label(&nyx_core::paths::pretty(std::path::Path::new(path)));
    let in_repo = nyx_agents_core::git::context(std::path::Path::new(path)).is_some();
    ui.worktree_check.set_sensitive(in_repo);
    if !in_repo {
        ui.worktree_check.set_active(false);
    }
}

/// The New session menu: a folder, and the installed harnesses to start there.
fn new_session_menu() -> (gtk::MenuButton, gtk::ListBox, gtk::Label, gtk::CheckButton, gtk::CheckButton) {
    let list = gtk::ListBox::new();
    list.add_css_class("new-session-list");
    list.set_selection_mode(gtk::SelectionMode::None);
    list.set_activate_on_single_click(true);
    let folder_label = gtk::Label::new(Some("~"));
    folder_label.set_xalign(0.0);
    folder_label.set_hexpand(true);
    folder_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    let folder_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    folder_box.append(&gtk::Image::from_icon_name("folder-symbolic"));
    folder_box.append(&folder_label);
    folder_box.append(&gtk::Image::from_icon_name("go-next-symbolic"));
    let folder = gtk::Button::new();
    folder.add_css_class("flat");
    folder.add_css_class("new-session-folder");
    folder.set_child(Some(&folder_box));
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
    let heading = |text: &str| {
        let l = gtk::Label::new(Some(text));
        l.add_css_class("menu-heading");
        l.set_xalign(0.0);
        l
    };
    let content = gtk::Box::new(gtk::Orientation::Vertical, 4);
    content.add_css_class("new-session-menu");
    let worktree = gtk::CheckButton::with_label("In a new worktree");
    worktree.set_tooltip_text(Some("Start on a new branch in its own git worktree, so agents can work in parallel"));
    let workspace = gtk::CheckButton::with_label("With a desktop of its own");
    workspace.set_tooltip_text(Some(
        "Start an agent workspace: a separate desktop where the agent opens and drives programs. You can watch it and take over. Claude Code and Codex get the tools for it.",
    ));
    content.append(&heading("Start in"));
    content.append(&folder);
    content.append(&worktree);
    content.append(&workspace);
    content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    content.append(&heading("Agent"));
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .max_content_height(340)
        .child(&list)
        .build();
    content.append(&scroll);
    let popover = gtk::Popover::new();
    popover.set_has_arrow(false);
    popover.add_css_class("menu-popover");
    popover.set_child(Some(&content));
    let button = gtk::MenuButton::new();
    button.set_icon_name("list-add-symbolic");
    button.set_tooltip_text(Some("New session (Ctrl+T)"));
    button.add_css_class("bar-button");
    button.set_valign(gtk::Align::Center);
    button.set_popover(Some(&popover));
    list.connect_row_activated(|_, row| {
        let id = row.widget_name().to_string();
        launch(&id);
    });
    (button, list, folder_label, worktree, workspace)
}

/// The version number in a harness's `--version` line ("codex-cli 0.167.0" is "0.167.0").
fn short_version(v: &str) -> String {
    v.split(|c: char| c.is_whitespace() || c == '/')
        .map(|t| t.trim_start_matches('v').trim_matches(|c: char| c == '(' || c == ')' || c == ','))
        .find(|t| t.starts_with(|c: char| c.is_ascii_digit()))
        .unwrap_or(v)
        .to_string()
}

fn fill_new_menu() {
    let Some(ui) = cur() else { return };
    while let Some(c) = ui.new_list.first_child() {
        ui.new_list.remove(&c);
    }
    let last = crate::settings::default_harness().filter(|h| !h.is_empty()).unwrap_or_else(|| prefs::get().last_harness);
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
            let v = gtk::Label::new(Some(&short_version(v)));
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
    let worktree = ui.worktree_check.is_active().then(String::new);
    ui.worktree_check.set_active(false);
    let workspace = (ui.workspace_check.is_active() && harness != "shell").then(|| "new".to_string());
    ui.workspace_check.set_active(false);
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
                ui.sessions.borrow_mut().insert(s.id.clone(), s.clone());
                open_session(&s.id);
                refresh_sidebar();
            }
            Err(e) => toast(&e),
        },
    );
}
