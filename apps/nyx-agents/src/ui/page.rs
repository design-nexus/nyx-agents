//! One tab: the session bar (title, state, where it runs, Stop or Resume and
//! Discard, More), its terminal (and optionally a terminal split beside it),
//! a find bar, the subagents under it and the agent's desktop beside it.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gdk, glib};
use nyx_agents_proto::{Activity, AttachMode, Lifecycle, Session};
use nyx_term_gtk::TerminalView;

use nyx_ui::menu;

use super::daemon::{self, Attached};
use super::inspector::Inspector;
use super::theme;
use super::workspace;

/// A session shown in a pane: its view and the live attachment, if any.
pub struct Pane {
    pub id: String,
    pub root: gtk::Box,
    pub view: Option<TerminalView>,
    attached: Option<Attached>,
    banner: gtk::Box,
    banner_icon: gtk::Image,
    banner_title: gtk::Label,
    banner_text: gtk::Label,
    open_log: gtk::Button,
    shown_lifecycle: Option<Lifecycle>,
}

/// Under the title: the agent, branch, folder and what it says, each with its icon.
struct Meta {
    root: gtk::Box,
    agent: gtk::Label,
    branch_row: gtk::Box,
    branch: gtk::Label,
    folder: gtk::Label,
    msg_row: gtk::Box,
    msg: gtk::Label,
}

impl Meta {
    fn new() -> Meta {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        root.add_css_class("session-meta");
        root.set_hexpand(true);
        let sep = || {
            let l = gtk::Label::new(Some("·"));
            l.add_css_class("meta-sep");
            l
        };
        let piece = |icon: Option<&str>| {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 5);
            if let Some(i) = icon {
                let img = gtk::Image::from_icon_name(i);
                img.add_css_class("meta-icon");
                row.append(&img);
            }
            let l = gtk::Label::new(None);
            l.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            row.append(&l);
            (row, l)
        };
        let (agent_row, agent) = piece(None);
        let (branch_row, branch) = piece(Some("nyx-agents-branch-symbolic"));
        let (folder_row, folder) = piece(Some("folder-symbolic"));
        folder.set_max_width_chars(48);
        let (msg_row, msg) = piece(None);
        msg.add_css_class("meta-msg");
        root.append(&agent_row);
        let s1 = sep();
        root.append(&s1);
        root.append(&branch_row);
        let s2 = sep();
        root.append(&s2);
        root.append(&folder_row);
        let s3 = sep();
        root.append(&s3);
        root.append(&msg_row);
        // A separator shows only before a piece that shows.
        for (s, row) in [(&s1, &branch_row), (&s3, &msg_row)] {
            let s = s.clone();
            row.connect_visible_notify(move |r| s.set_visible(r.is_visible()));
        }
        Meta { root, agent, branch_row, branch, folder, msg_row, msg }
    }
}

pub struct Page {
    pub id: String,
    pub root: gtk::Box,
    title: gtk::Label,
    meta: Meta,
    pill: gtk::Box,
    pill_text: gtk::Label,
    stop: gtk::Button,
    stop_label: gtk::Label,
    resume: gtk::Button,
    retry: gtk::Button,
    discard: gtk::Button,
    find_bar: gtk::Revealer,
    find_entry: gtk::SearchEntry,
    find_count: gtk::Label,
    area: gtk::Box,
    pub main: Rc<RefCell<Pane>>,
    pub split: Option<(Rc<RefCell<Pane>>, bool)>,
    pub inspector: Rc<Inspector>,
    /// The terminal column on the left, the agent's desktop on the right.
    side: gtk::Paned,
    desktop: Rc<RefCell<Option<workspace::Pane>>>,
    cwd: String,
}

type IdFn = Rc<dyn Fn(&str)>;
type SourceFn = Rc<dyn Fn(&str) -> Option<nyx_agents_proto::SubagentSource>>;
type MenuFn = Rc<dyn Fn(&str) -> Vec<menu::Entry>>;

pub struct Actions {
    /// Stop the session; Force stop when it was already asked to stop.
    pub stop: IdFn,
    pub resume: IdFn,
    /// Start a session that didn't start again.
    pub retry: IdFn,
    /// Remove an ended session (asking first), stopping a live one.
    pub discard: IdFn,
    pub rename: IdFn,
    pub unsplit: IdFn,
    /// Show an agent workspace in a window of its own.
    pub show_workspace: IdFn,
    /// A pane's session ended or its holder went away.
    pub ended: IdFn,
    /// Where a harness's subagent activity can be read, if anywhere.
    pub subagent_source: SourceFn,
    /// A session's actions, for its More and right-click menus.
    pub session_menu: MenuFn,
    /// The agent's name ("Claude Code") for a harness id.
    pub agent_name: Rc<dyn Fn(&str) -> String>,
    /// Whether the session was asked to stop and is still running.
    pub stopping: Rc<dyn Fn(&str) -> bool>,
}

fn button(icon: &str, tooltip: &str) -> gtk::Button {
    nyx_ui::widgets::bar_button(icon, tooltip)
}

/// A button with an icon before its label.
pub fn icon_label_button(icon: &str, label: &str) -> (gtk::Button, gtk::Label) {
    let b = gtk::Button::new();
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.append(&gtk::Image::from_icon_name(icon));
    let l = gtk::Label::new(Some(label));
    row.append(&l);
    b.set_child(Some(&row));
    b.add_css_class("icon-label");
    (b, l)
}

/// An agent's mark: its letter on its colour, `size` pixels square.
pub fn logo(harness: &str, size: i32) -> gtk::Label {
    let letter = match harness {
        "claude" => "C".to_string(),
        "codex" => "X".to_string(),
        "antigravity" => "A".to_string(),
        "muse" => "M".to_string(),
        "grok" => "G".to_string(),
        "cursor" => "▲".to_string(),
        "omp" => "π".to_string(),
        "crush" => "C".to_string(),
        "opencode" => "O".to_string(),
        "hermes" => "H".to_string(),
        "shell" => "›_".to_string(),
        other => other.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default(),
    };
    let l = gtk::Label::new(Some(&letter));
    l.add_css_class("agent-logo");
    l.add_css_class(&format!("logo-{harness}"));
    if size >= 24 {
        l.add_css_class("large");
    }
    l.set_size_request(size, size);
    l.set_halign(gtk::Align::Start);
    l.set_valign(gtk::Align::Center);
    l
}

/// "now", "4m", "2h", "3d": how long ago a time in milliseconds was.
pub fn ago(ms: u64) -> String {
    let secs = nyx_agents_proto::now_ms().saturating_sub(ms) / 1000;
    match secs {
        0..60 => "now".into(),
        60..3600 => format!("{}m", secs / 60),
        3600..86400 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86400),
    }
}

pub fn live(s: &Session) -> bool {
    matches!(s.lifecycle, Lifecycle::Running | Lifecycle::Starting)
}

/// Ended without its program ever starting (no pid was ever reported).
pub fn failed_start(s: &Session) -> bool {
    !live(s) && s.pid.is_none()
}

pub fn status_text(s: &Session, stopping: bool) -> (&'static str, &'static str) {
    if stopping && live(s) {
        return ("Stopping…", "");
    }
    match s.lifecycle {
        Lifecycle::Starting => ("Starting", ""),
        _ if failed_start(s) => ("Didn't start", "failed"),
        Lifecycle::Exited => ("Ended", ""),
        Lifecycle::Lost => ("Not running", ""),
        Lifecycle::Running => match s.status.activity {
            Activity::Working => ("Working", "working"),
            Activity::Blocked => ("Needs you", "blocked"),
            Activity::Done => ("Done", "done"),
            Activity::Error => ("Error", "blocked"),
            Activity::Idle => ("Idle", ""),
            Activity::Unknown => ("Running", ""),
        },
    }
}

/// Copy, paste and select all for a terminal.
fn terminal_entries(view: &TerminalView, live: bool) -> Vec<menu::Entry> {
    let (v1, v2, v3) = (view.clone(), view.clone(), view.clone());
    let mut entries = vec![
        menu::item("Copy", move || {
            v1.copy_selection();
        })
        .icon("edit-copy-symbolic")
        .key("Ctrl+C")
        .enabled(view.has_selection()),
    ];
    if live {
        entries.push(menu::item("Paste", move || v2.paste_clipboard()).icon("edit-paste-symbolic").key("Ctrl+V"));
    }
    entries.push(menu::item("Select all", move || v3.select_all()).icon("edit-select-all-symbolic"));
    entries
}

impl Pane {
    fn new(s: &Session, a: &Rc<Actions>) -> Rc<RefCell<Pane>> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.set_hexpand(true);
        root.set_vexpand(true);
        let banner = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        banner.add_css_class("ended-banner");
        let banner_icon = gtk::Image::from_icon_name("object-select-symbolic");
        banner_icon.set_valign(gtk::Align::Start);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        let banner_title = gtk::Label::new(None);
        banner_title.add_css_class("ended-title");
        banner_title.set_xalign(0.0);
        let banner_text = gtk::Label::new(None);
        banner_text.add_css_class("session-meta");
        banner_text.set_xalign(0.0);
        banner_text.set_wrap(true);
        text.append(&banner_title);
        text.append(&banner_text);
        let open_log = gtk::Button::with_label("Open log");
        open_log.add_css_class("flat");
        open_log.set_valign(gtk::Align::Center);
        let log = nyx_agents_core::paths::session_dir(&s.id).join("holder.log");
        open_log.connect_clicked(move |_| {
            let _ = std::process::Command::new("xdg-open").arg(&log).spawn();
        });
        banner.append(&banner_icon);
        banner.append(&text);
        banner.append(&open_log);
        banner.set_visible(false);
        root.append(&banner);
        let pane = Rc::new(RefCell::new(Pane {
            id: s.id.clone(),
            root,
            view: None,
            attached: None,
            banner,
            banner_icon,
            banner_title,
            banner_text,
            open_log,
            shown_lifecycle: None,
        }));
        Pane::update(&pane, s, a);
        pane
    }

    /// Show the live terminal while the session runs, its history after.
    fn update(pane: &Rc<RefCell<Pane>>, s: &Session, a: &Rc<Actions>) {
        let mut p = pane.borrow_mut();
        let live = live(s);
        let was_live = matches!(p.shown_lifecycle, Some(Lifecycle::Running | Lifecycle::Starting));
        let reconnect = live && p.attached.as_ref().is_none_or(|a| !a.is_connected()) && p.shown_lifecycle != Some(s.lifecycle);
        if p.shown_lifecycle.is_some() && was_live == live && !reconnect {
            p.shown_lifecycle = Some(s.lifecycle);
            p.update_banner(s, a);
            return;
        }
        p.shown_lifecycle = Some(s.lifecycle);
        if let Some(v) = p.view.take() {
            p.root.remove(&v);
        }
        if let Some(c) = p.root.last_child().filter(|c| c.has_css_class("empty-state")) {
            p.root.remove(&c);
        }
        p.attached = None;
        let view = if live {
            let v = TerminalView::new(theme::view_style(), false);
            // The holder answers the program's terminal queries.
            v.set_answers_queries(false);
            // An agent with a workspace reads the clipboard there; hand a pasted
            // image over before passing Ctrl+V on.
            if let Some(w) = s.workspace.clone() {
                let weak = v.downgrade();
                v.connect_image_paste(move |_| {
                    let weak = weak.clone();
                    super::workspace::push_clipboard(&w, move |_| {
                        if let Some(v) = weak.upgrade() {
                            v.send_input(b"\x16");
                        }
                    });
                });
            }
            let ended = a.ended.clone();
            let id = s.id.clone();
            p.attached = Some(daemon::attach(&v, &s.id, AttachMode::Control, move |_| ended(&id)));
            Some(v)
        } else {
            let v = daemon::history_view(&s.id);
            if let Some(v) = &v {
                v.add_css_class("history-view");
            }
            v
        };
        match &view {
            Some(v) => {
                v.set_hexpand(true);
                v.set_vexpand(true);
                let (id, session_menu) = (s.id.clone(), a.session_menu.clone());
                menu::on_right_click(v, move |w, x, y| {
                    let Some(view) = w.downcast_ref::<TerminalView>() else { return };
                    let mut entries = terminal_entries(view, live);
                    entries.push(menu::separator());
                    entries.extend(session_menu(&id));
                    menu::popup_at(w, x, y, entries);
                });
                p.root.append(v);
                if live {
                    let v = v.clone();
                    glib::idle_add_local_once(move || {
                        v.grab_focus();
                    });
                }
            }
            None => {
                let empty = gtk::Label::new(Some(if failed_start(s) {
                    "Nothing ran, so there is no screen to show."
                } else {
                    "It left nothing on screen."
                }));
                empty.add_css_class("session-meta");
                empty.add_css_class("empty-state");
                empty.set_vexpand(true);
                p.root.append(&empty);
            }
        }
        p.view = view;
        p.update_banner(s, a);
    }

    /// Why the session ended, over its last screen.
    fn update_banner(&self, s: &Session, a: &Rc<Actions>) {
        let ended = !live(s);
        self.banner.set_visible(ended);
        if !ended {
            return;
        }
        let who = if s.harness == "shell" { "The terminal".to_string() } else { (a.agent_name)(&s.harness) };
        let failed = failed_start(s);
        let (icon, kind, title, text) = match (s.lifecycle, s.exit_code) {
            _ if failed => (
                "dialog-warning-symbolic",
                "failed",
                format!("{who} didn't start"),
                "The program ended before it opened a terminal. Its log says why.".to_string(),
            ),
            (Lifecycle::Lost, _) => (
                "dialog-information-symbolic",
                "",
                format!("{who} stopped running"),
                "Its holder went away (for example after a restart). Below is its last screen.".to_string(),
            ),
            (_, Some(0)) => ("object-select-symbolic", "done", format!("{who} finished"), "Below is its last screen.".to_string()),
            (_, Some(c)) if c > 128 => (
                "media-playback-stop-symbolic",
                "",
                format!("{who} was stopped"),
                format!("It ended on signal {}. Below is its last screen.", c - 128),
            ),
            (_, Some(c)) => (
                "dialog-information-symbolic",
                "",
                format!("{who} exited with code {c}"),
                "Below is its last screen.".to_string(),
            ),
            _ => ("media-playback-stop-symbolic", "", format!("{who} ended"), "Below is its last screen.".to_string()),
        };
        let text = if s.resumable && !failed { format!("{text} Resume picks up the conversation.") } else { text };
        self.banner_icon.set_icon_name(Some(icon));
        for c in ["failed", "done"] {
            self.banner.remove_css_class(c);
        }
        if !kind.is_empty() {
            self.banner.add_css_class(kind);
        }
        self.banner_title.set_text(&title);
        self.banner_text.set_text(&text);
        self.open_log.set_visible(failed);
    }
}

impl Page {
    pub fn new(s: &Session, a: &Rc<Actions>) -> Page {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.set_hexpand(true);
        root.set_vexpand(true);

        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        bar.add_css_class("session-bar");
        let title = gtk::Label::new(None);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title.set_max_width_chars(40);
        let title_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        title_row.append(&title);
        let pencil = gtk::Image::from_icon_name("document-edit-symbolic");
        pencil.add_css_class("title-pencil");
        title_row.append(&pencil);
        let title_button = gtk::Button::new();
        title_button.set_child(Some(&title_row));
        title_button.add_css_class("flat");
        title_button.add_css_class("session-title");
        title_button.set_valign(gtk::Align::Center);
        title_button.set_tooltip_text(Some("Rename (F2)"));
        let pill = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        pill.add_css_class("status-pill");
        pill.set_valign(gtk::Align::Center);
        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.add_css_class("pill-dot");
        dot.set_valign(gtk::Align::Center);
        let pill_text = gtk::Label::new(None);
        pill.append(&dot);
        pill.append(&pill_text);
        let meta = Meta::new();
        let (stop, stop_label) = icon_label_button("media-playback-stop-symbolic", "Stop");
        stop.add_css_class("stop-button");
        let (resume, _) = icon_label_button("view-refresh-symbolic", "Resume");
        resume.add_css_class("suggested-action");
        resume.set_tooltip_text(Some("Continue this conversation in a new run of the agent"));
        let (retry, _) = icon_label_button("view-refresh-symbolic", "Try again");
        retry.set_tooltip_text(Some("Start it again in the same folder"));
        let (discard, _) = icon_label_button("user-trash-symbolic", "Discard");
        discard.set_tooltip_text(Some("Remove this session from Agents"));
        let more = button("view-more-horizontal-symbolic", "More");
        for b in [&stop, &resume, &retry, &discard] {
            b.set_valign(gtk::Align::Center);
        }
        bar.append(&title_button);
        bar.append(&pill);
        bar.append(&meta.root);
        bar.append(&resume);
        bar.append(&retry);
        bar.append(&stop);
        bar.append(&discard);
        bar.append(&more);
        root.append(&bar);

        let find_entry = gtk::SearchEntry::new();
        find_entry.set_placeholder_text(Some("Find in output"));
        find_entry.set_hexpand(true);
        let find_count = gtk::Label::new(None);
        find_count.add_css_class("session-meta");
        let find_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        find_box.add_css_class("find-bar");
        find_box.append(&find_entry);
        find_box.append(&find_count);
        let find_bar = gtk::Revealer::new();
        find_bar.set_child(Some(&find_box));
        root.append(&find_bar);

        let area = gtk::Box::new(gtk::Orientation::Vertical, 0);
        area.set_hexpand(true);
        area.set_vexpand(true);
        // The agent's terminal above, its subagents below (read-only).
        let column = gtk::Paned::new(gtk::Orientation::Vertical);
        let inspector = Inspector::new(&column);
        column.set_start_child(Some(&area));
        column.set_end_child(Some(&inspector.root));
        column.set_resize_end_child(false);
        column.set_shrink_end_child(false);
        column.set_shrink_start_child(false);
        column.set_vexpand(true);
        let side = gtk::Paned::new(gtk::Orientation::Horizontal);
        side.set_start_child(Some(&column));
        side.set_shrink_start_child(false);
        side.set_shrink_end_child(false);
        side.set_wide_handle(false);
        side.set_vexpand(true);
        root.append(&side);

        let main = Pane::new(s, a);
        area.append(&main.borrow().root);

        let id = s.id.clone();
        let act = a.rename.clone();
        title_button.connect_clicked(move |_| act(&id));
        let id = s.id.clone();
        let act = a.stop.clone();
        stop.connect_clicked(move |_| act(&id));
        let id = s.id.clone();
        let act = a.resume.clone();
        resume.connect_clicked(move |_| act(&id));
        let id = s.id.clone();
        let act = a.retry.clone();
        retry.connect_clicked(move |_| act(&id));
        let id = s.id.clone();
        let act = a.discard.clone();
        discard.connect_clicked(move |_| act(&id));
        let id = s.id.clone();
        let menu_for = a.session_menu.clone();
        more.connect_clicked(move |b| menu::popup_below(b, menu_for(&id)));

        let page = Page {
            id: s.id.clone(),
            root,
            title,
            meta,
            pill,
            pill_text,
            stop,
            stop_label,
            resume,
            retry,
            discard,
            find_bar,
            find_entry,
            find_count,
            area,
            main,
            split: None,
            inspector,
            side,
            desktop: Rc::new(RefCell::new(None)),
            cwd: s.cwd.clone(),
        };
        page.update(s, a);
        // A session with a desktop shows it beside the terminal.
        if live(s) && s.workspace.is_some() {
            page.set_desktop(s.workspace.as_deref(), a);
        }
        page
    }

    fn views(&self) -> Vec<TerminalView> {
        let mut v: Vec<TerminalView> = self.main.borrow().view.iter().cloned().collect();
        if let Some((p, _)) = &self.split {
            v.extend(p.borrow().view.iter().cloned());
        }
        v
    }

    /// Wire the find bar to the focused (or main) terminal.
    pub fn install_find(self_: &Rc<RefCell<Page>>) {
        let p = self_.borrow();
        let weak = Rc::downgrade(self_);
        p.find_entry.connect_search_changed(move |e| {
            let Some(page) = weak.upgrade() else { return };
            let page = page.borrow();
            if let Some(v) = page.views().first() {
                let n = v.find(&e.text());
                page.find_count.set_text(&match (e.text().is_empty(), n) {
                    (true, _) => String::new(),
                    (false, 0) => "No matches".into(),
                    (false, 1) => "1 match".into(),
                    (false, n) => format!("{n} matches"),
                });
            }
        });
        let weak = Rc::downgrade(self_);
        p.find_entry.connect_activate(move |_| {
            if let Some(page) = weak.upgrade()
                && let Some(v) = page.borrow().views().first()
            {
                let shift = gtk::gdk::Display::default()
                    .and_then(|d| d.default_seat())
                    .and_then(|s| s.keyboard())
                    .is_some_and(|k| k.modifier_state().contains(gdk::ModifierType::SHIFT_MASK));
                v.find_step(!shift);
            }
        });
        let weak = Rc::downgrade(self_);
        p.find_entry.connect_stop_search(move |e| {
            e.set_text("");
            if let Some(page) = weak.upgrade() {
                let page = page.borrow();
                page.find_bar.set_reveal_child(false);
                if let Some(v) = page.views().first() {
                    v.find("");
                    v.grab_focus();
                }
            }
        });
    }

    pub fn open_find(&self) {
        self.find_bar.set_reveal_child(true);
        self.find_entry.grab_focus();
    }

    pub fn update(&self, s: &Session, a: &Rc<Actions>) {
        let stopping = (a.stopping)(&s.id);
        let (text, class) = status_text(s, stopping);
        self.pill_text.set_text(text);
        for c in ["working", "blocked", "done", "failed"] {
            self.pill.remove_css_class(c);
        }
        if !class.is_empty() {
            self.pill.add_css_class(class);
        }
        self.title.set_text(&s.title);
        self.meta.agent.set_text(&(a.agent_name)(&s.harness));
        let branch = s.worktree.as_ref().and_then(|w| w.branch.as_ref().map(|b| if w.linked { format!("{b} (worktree)") } else { b.clone() }));
        self.meta.branch_row.set_visible(branch.is_some());
        self.meta.branch.set_text(branch.as_deref().unwrap_or_default());
        self.meta.folder.set_text(&nyx_core::paths::pretty(std::path::Path::new(&s.cwd)));
        self.meta.root.set_tooltip_text(Some(&s.cwd));
        let msg = s.status.message.clone().filter(|_| live(s));
        self.meta.msg_row.set_visible(msg.is_some());
        self.meta.msg.set_text(msg.as_deref().unwrap_or_default());
        let live = live(s);
        let failed = failed_start(s);
        self.stop.set_visible(live);
        self.stop_label.set_label(if stopping {
            "Force stop"
        } else if s.harness == "shell" {
            "Close"
        } else {
            "Stop"
        });
        self.stop.set_tooltip_text(Some(if stopping {
            "End it now, without waiting for it to finish"
        } else {
            "Ask it to end; press again to force it"
        }));
        self.retry.set_visible(failed);
        self.resume.set_visible(!live && !failed && s.resumable);
        self.discard.set_visible(!live);
        self.inspector.update(s, (a.subagent_source)(&s.harness));
        Pane::update(&self.main, s, a);
        if !live && self.desktop_shown() {
            self.set_desktop(None, a);
        }
    }

    pub fn update_split(&self, s: &Session, a: &Rc<Actions>) {
        if let Some((p, _)) = &self.split
            && p.borrow().id == s.id
        {
            Pane::update(p, s, a);
        }
    }

    pub fn desktop_shown(&self) -> bool {
        self.desktop.borrow().is_some()
    }

    /// Show workspace `ws` beside the terminal, or hide it (`None`).
    pub fn set_desktop(&self, ws: Option<&str>, a: &Rc<Actions>) {
        let Some(ws) = ws else {
            self.side.set_end_child(None::<&gtk::Widget>);
            self.desktop.borrow_mut().take();
            return;
        };
        if self.desktop_shown() {
            return;
        }
        let (slot, side): (Weak<RefCell<Option<workspace::Pane>>>, _) = (Rc::downgrade(&self.desktop), self.side.downgrade());
        let hide = move || {
            if let Some(side) = side.upgrade() {
                side.set_end_child(None::<&gtk::Widget>);
            }
            if let Some(slot) = slot.upgrade() {
                // Dropped after this click has been handled.
                let pane = slot.borrow_mut().take();
                glib::idle_add_local_once(move || drop(pane));
            }
        };
        let (open, w) = (a.show_workspace.clone(), ws.to_string());
        let pane = workspace::pane(ws, move || open(&w), hide);
        self.side.set_end_child(Some(&pane.root));
        let side = self.side.clone();
        glib::idle_add_local_once(move || {
            let w = side.width();
            if w > 0 {
                side.set_position(w * 54 / 100);
            }
        });
        *self.desktop.borrow_mut() = Some(pane);
    }

    /// Put `s` beside (or below) the main session.
    pub fn set_split(&mut self, s: Option<(&Session, bool)>, a: &Rc<Actions>) {
        while let Some(c) = self.area.first_child() {
            self.area.remove(&c);
        }
        self.split = None;
        match s {
            None => self.area.append(&self.main.borrow().root),
            Some((s, down)) => {
                let other = Pane::new(s, a);
                let paned = gtk::Paned::new(if down { gtk::Orientation::Vertical } else { gtk::Orientation::Horizontal });
                paned.set_start_child(Some(&self.main.borrow().root));
                paned.set_end_child(Some(&other.borrow().root));
                paned.set_wide_handle(false);
                paned.set_vexpand(true);
                paned.set_hexpand(true);
                self.area.append(&paned);
                // Half and half once allocated.
                let p2 = paned.clone();
                glib::idle_add_local_once(move || {
                    let total = if down { p2.height() } else { p2.width() };
                    if total > 0 {
                        p2.set_position(total / 2);
                    }
                });
                // The terminal's own header: what it is, where, and its ×.
                let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                head.add_css_class("pane-head");
                head.append(&gtk::Image::from_icon_name("utilities-terminal-symbolic"));
                let name = gtk::Label::new(Some(if s.harness == "shell" { "Terminal" } else { &s.title }));
                name.add_css_class("pane-title");
                name.set_xalign(0.0);
                name.set_hexpand(true);
                head.append(&name);
                let place = gtk::Label::new(Some(&nyx_core::paths::pretty(std::path::Path::new(&s.cwd))));
                place.add_css_class("session-meta");
                place.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                head.append(&place);
                let close = button("window-close-symbolic", "Close the terminal");
                let id = self.id.clone();
                let act = a.unsplit.clone();
                close.connect_clicked(move |_| act(&id));
                head.append(&close);
                other.borrow().root.prepend(&head);
                self.split = Some((other, down));
            }
        }
    }

    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    pub fn focus(&self) {
        if let Some(v) = self.views().first() {
            v.grab_focus();
        }
    }

    pub fn restyle(&self) {
        for v in self.views() {
            v.set_style(theme::view_style());
        }
    }
}