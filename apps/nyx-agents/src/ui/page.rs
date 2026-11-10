//! One tab: a session's bar, its terminal (and optionally a second session
//! split beside it), a find bar, and the slot where the subagent inspector
//! will sit.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};
use nyx_agents_proto::{Activity, AttachMode, Lifecycle, Session};
use nyx_term_gtk::TerminalView;

use nyx_ui::menu;

use super::daemon::{self, Attached};
use super::inspector::Inspector;
use super::theme;

/// A session shown in a pane: its view and the live attachment, if any.
pub struct Pane {
    pub id: String,
    pub root: gtk::Box,
    pub view: Option<TerminalView>,
    attached: Option<Attached>,
    banner: gtk::Box,
    banner_text: gtk::Label,
    resume: gtk::Button,
    forget: gtk::Button,
    remove_worktree: gtk::Button,
    shown_lifecycle: Option<Lifecycle>,
}

pub struct Page {
    pub id: String,
    pub root: gtk::Box,
    meta: gtk::Label,
    pill: gtk::Label,
    stop: gtk::Button,
    git: gtk::Button,
    workspace: gtk::Button,
    split_right: gtk::Button,
    split_down: gtk::Button,
    find_bar: gtk::Revealer,
    find_entry: gtk::SearchEntry,
    find_count: gtk::Label,
    area: gtk::Box,
    pub main: Rc<RefCell<Pane>>,
    pub split: Option<(Rc<RefCell<Pane>>, bool)>,
    pub inspector: Rc<Inspector>,
    cwd: String,
}

type IdFn = Rc<dyn Fn(&str)>;
type SplitFn = Rc<dyn Fn(&str, bool)>;
type SourceFn = Rc<dyn Fn(&str) -> Option<nyx_agents_proto::SubagentSource>>;
type MenuFn = Rc<dyn Fn(&str) -> Vec<menu::Entry>>;

pub struct Actions {
    pub stop: IdFn,
    pub resume: IdFn,
    pub forget: IdFn,
    /// Open a terminal beside (`false`) or below (`true`) the session.
    pub split: SplitFn,
    pub unsplit: IdFn,
    pub open_git: IdFn,
    /// Show an agent workspace's live view.
    pub show_workspace: IdFn,
    pub remove_worktree: IdFn,
    /// A pane's session ended or its holder went away.
    pub ended: IdFn,
    /// Where a harness's subagent activity can be read, if anywhere.
    pub subagent_source: SourceFn,
    /// A session's actions, for its right-click menus.
    pub session_menu: MenuFn,
}

fn button(icon: &str, tooltip: &str) -> gtk::Button {
    nyx_ui::widgets::bar_button(icon, tooltip)
}

pub fn status_text(s: &Session) -> (&'static str, &'static str) {
    match s.lifecycle {
        Lifecycle::Starting => ("Starting", ""),
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
        let banner = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        banner.add_css_class("ended-banner");
        let banner_text = gtk::Label::new(None);
        banner_text.set_xalign(0.0);
        banner_text.set_hexpand(true);
        let resume = gtk::Button::with_label("Resume");
        resume.set_tooltip_text(Some("Continue this conversation in a new run of the agent"));
        let forget = gtk::Button::with_label("Forget");
        forget.set_tooltip_text(Some("Remove this ended session and its history"));
        let remove_worktree = gtk::Button::with_label("Remove worktree");
        remove_worktree
            .set_tooltip_text(Some("Delete this session's worktree folder if it has no uncommitted changes; the branch is kept"));
        banner.append(&banner_text);
        banner.append(&resume);
        banner.append(&remove_worktree);
        banner.append(&forget);
        banner.set_visible(false);
        root.append(&banner);
        let id = s.id.clone();
        let act = a.resume.clone();
        resume.connect_clicked(move |_| act(&id));
        let id = s.id.clone();
        let act = a.forget.clone();
        forget.connect_clicked(move |_| act(&id));
        let id = s.id.clone();
        let act = a.remove_worktree.clone();
        remove_worktree.connect_clicked(move |_| act(&id));
        let pane = Rc::new(RefCell::new(Pane {
            id: s.id.clone(),
            root,
            view: None,
            attached: None,
            banner,
            banner_text,
            resume,
            forget,
            remove_worktree,
            shown_lifecycle: None,
        }));
        Pane::update(&pane, s, a);
        pane
    }

    /// Show the live terminal while the session runs, its history after.
    fn update(pane: &Rc<RefCell<Pane>>, s: &Session, a: &Rc<Actions>) {
        let mut p = pane.borrow_mut();
        let live = matches!(s.lifecycle, Lifecycle::Running | Lifecycle::Starting);
        let was_live = matches!(p.shown_lifecycle, Some(Lifecycle::Running | Lifecycle::Starting));
        let reconnect = live && p.attached.as_ref().is_none_or(|a| !a.is_connected()) && p.shown_lifecycle != Some(s.lifecycle);
        if p.shown_lifecycle.is_some() && was_live == live && !reconnect {
            p.shown_lifecycle = Some(s.lifecycle);
            p.update_banner(s);
            return;
        }
        p.shown_lifecycle = Some(s.lifecycle);
        if let Some(v) = p.view.take() {
            p.root.remove(&v);
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
            daemon::history_view(&s.id)
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
                let empty = nyx_ui::widgets::empty_state(
                    "utilities-terminal-symbolic",
                    "No history",
                    "This session left nothing on screen.",
                    None,
                );
                p.root.append(&empty);
            }
        }
        p.view = view;
        p.update_banner(s);
    }

    fn update_banner(&self, s: &Session) {
        let ended = !matches!(s.lifecycle, Lifecycle::Running | Lifecycle::Starting);
        self.banner.set_visible(ended);
        if ended {
            let why = match (s.lifecycle, s.exit_code) {
                (Lifecycle::Lost, _) => "This session stopped when its holder went away (for example a restart).".to_string(),
                (_, Some(0)) => "The agent finished.".to_string(),
                (_, Some(c)) if c > 128 => format!("The agent was stopped (signal {}).", c - 128),
                (_, Some(c)) => format!("The agent exited with code {c}."),
                _ => "The agent ended.".to_string(),
            };
            self.banner_text.set_text(&format!("{why} Showing its last screen."));
            self.resume.set_visible(s.resumable);
        }
        let own_worktree = s.worktree.as_ref().is_some_and(|w| {
            w.linked
                && std::path::Path::new(&w.root).starts_with(nyx_agents_core::worktree::root())
                && std::path::Path::new(&w.root).exists()
        });
        self.remove_worktree.set_visible(ended && own_worktree);
        self.forget.set_visible(ended);
    }
}

impl Page {
    pub fn new(s: &Session, a: &Rc<Actions>) -> Page {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.set_hexpand(true);
        root.set_vexpand(true);

        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        bar.add_css_class("session-bar");
        let pill = gtk::Label::new(None);
        pill.add_css_class("status-pill");
        let meta = gtk::Label::new(None);
        meta.add_css_class("session-meta");
        meta.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        meta.set_hexpand(true);
        meta.set_xalign(0.0);
        let find = button("system-search-symbolic", "Find in the terminal (Ctrl+F)");
        let split_right = button("view-dual-symbolic", "Open a terminal beside the agent (Ctrl+Shift+O)");
        let split_down = button("view-paged-symbolic", "Open a terminal below the agent (Ctrl+Shift+E)");
        let git = button("nyx-agents-branch-symbolic", "Open this repository in Nyx Git");
        let workspace = button("video-display-symbolic", "Show the agent's workspace");
        let stop = button("media-playback-stop-symbolic", "Stop the agent");
        for w in [pill.upcast_ref::<gtk::Widget>(), meta.upcast_ref(), find.upcast_ref()] {
            bar.append(w);
        }
        bar.append(&split_right);
        bar.append(&split_down);
        bar.append(&git);
        bar.append(&workspace);
        bar.append(&stop);
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
        root.append(&column);

        let main = Pane::new(s, a);
        area.append(&main.borrow().root);

        let id = s.id.clone();
        let act = a.stop.clone();
        stop.connect_clicked(move |_| act(&id));
        let id = s.id.clone();
        let act = a.open_git.clone();
        git.connect_clicked(move |_| act(&id));
        if let Some(w) = s.workspace.clone() {
            let act = a.show_workspace.clone();
            workspace.connect_clicked(move |_| act(&w));
        }
        let id = s.id.clone();
        let act = a.split.clone();
        split_right.connect_clicked(move |_| act(&id, false));
        let id = s.id.clone();
        let act = a.split.clone();
        split_down.connect_clicked(move |_| act(&id, true));

        let page = Page {
            id: s.id.clone(),
            root,
            meta,
            pill,
            stop,
            git,
            workspace,
            split_right,
            split_down,
            find_bar,
            find_entry,
            find_count,
            area,
            main,
            split: None,
            inspector,
            cwd: s.cwd.clone(),
        };
        let fb = page.find_bar.clone();
        let fe = page.find_entry.clone();
        find.connect_clicked(move |_| {
            fb.set_reveal_child(true);
            fe.grab_focus();
        });
        page.update(s, a);
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
        let (text, class) = status_text(s);
        self.pill.set_text(text);
        for c in ["working", "blocked", "done"] {
            self.pill.remove_css_class(c);
        }
        if !class.is_empty() {
            self.pill.add_css_class(class);
        }
        let mut meta = vec![s.harness.clone()];
        if let Some(w) = &s.worktree
            && let Some(b) = &w.branch
        {
            meta.push(if w.linked { format!("{b} (worktree)") } else { b.clone() });
        }
        meta.push(nyx_core::paths::pretty(std::path::Path::new(&s.cwd)));
        if let Some(m) = &s.status.message {
            meta.push(m.clone());
        }
        self.meta.set_text(&meta.join("  ·  "));
        self.meta.set_tooltip_text(Some(&s.cwd));
        let live = matches!(s.lifecycle, Lifecycle::Running | Lifecycle::Starting);
        self.stop.set_visible(live);
        self.git.set_visible(s.worktree.is_some());
        self.workspace.set_visible(s.workspace.is_some());
        self.split_right.set_visible(self.split.is_none());
        self.split_down.set_visible(self.split.is_none());
        self.inspector.update(s, (a.subagent_source)(&s.harness));
        Pane::update(&self.main, s, a);
    }

    pub fn update_split(&self, s: &Session, a: &Rc<Actions>) {
        if let Some((p, _)) = &self.split
            && p.borrow().id == s.id
        {
            Pane::update(p, s, a);
        }
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
                // Closing the split: a small button on the second pane's banner row.
                let close = button("window-close-symbolic", "Close this split");
                close.set_halign(gtk::Align::End);
                let id = self.id.clone();
                let act = a.unsplit.clone();
                close.connect_clicked(move |_| act(&id));
                other.borrow().root.prepend(&close);
                self.split = Some((other, down));
            }
        }
        self.split_right.set_visible(self.split.is_none());
        self.split_down.set_visible(self.split.is_none());
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
