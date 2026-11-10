//! The Subagent Inspector: a row of tabs under the agent's terminal, one per
//! subagent the agent started, and a smaller viewer below for the selected
//! one. Strictly read-only: it reads the harness's own records and never
//! sends anything to the agent or its subagents. The main agent alone
//! manages them.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};
use nyx_agents_core::claude;
use nyx_agents_proto::{Session, Subagent, SubagentSource, SubagentState};
use nyx_term_gtk::TerminalView;

use super::theme;

struct Viewer {
    id: String,
    view: TerminalView,
    _monitor: gio::FileMonitor,
}

pub struct Inspector {
    /// The panel under the terminal (tabs, then the viewer).
    pub root: gtk::Box,
    tabs: gtk::Box,
    buttons: RefCell<HashMap<String, gtk::ToggleButton>>,
    viewer_box: gtk::Box,
    viewer_title: gtk::Label,
    viewer: RefCell<Option<Viewer>>,
    session: RefCell<Option<(String, String)>>,
    subs: RefCell<Vec<Subagent>>,
    paned: gtk::Paned,
}

fn state_icon(s: SubagentState) -> &'static str {
    match s {
        SubagentState::Running => "emblem-synchronizing-symbolic",
        SubagentState::Waiting => "media-playback-pause-symbolic",
        SubagentState::Completed => "object-select-symbolic",
        SubagentState::Failed => "dialog-error-symbolic",
    }
}

fn state_word(s: SubagentState) -> &'static str {
    match s {
        SubagentState::Running => "running",
        SubagentState::Waiting => "waiting",
        SubagentState::Completed => "completed",
        SubagentState::Failed => "failed",
    }
}

impl Inspector {
    /// `paned` holds the terminal area above and this panel below.
    pub fn new(paned: &gtk::Paned) -> Rc<Inspector> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("inspector");
        root.set_visible(false);
        let tabs = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        tabs.add_css_class("subagent-tabs");
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::External)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .child(&tabs)
            .build();
        let label = gtk::Label::new(Some("Subagents"));
        label.add_css_class("inspector-label");
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.add_css_class("subagent-row");
        row.append(&label);
        scroller.set_hexpand(true);
        row.append(&scroller);
        root.append(&row);

        let viewer_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        viewer_box.set_visible(false);
        viewer_box.set_vexpand(true);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        head.add_css_class("viewer-head");
        let viewer_title = gtk::Label::new(None);
        viewer_title.set_xalign(0.0);
        viewer_title.set_hexpand(true);
        viewer_title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let badge = nyx_ui::widgets::tag("Transcript · read-only", "");
        badge.set_tooltip_text(Some(
            "Rendered from the agent's session records. Not a terminal: nothing here reaches the subagent.",
        ));
        let collapse = nyx_ui::widgets::bar_button("go-down-symbolic", "Hide the subagent viewer");
        head.append(&viewer_title);
        head.append(&badge);
        head.append(&collapse);
        viewer_box.append(&head);
        root.append(&viewer_box);

        let me = Rc::new(Inspector {
            root,
            tabs,
            buttons: RefCell::new(HashMap::new()),
            viewer_box,
            viewer_title,
            viewer: RefCell::new(None),
            session: RefCell::new(None),
            subs: RefCell::new(Vec::new()),
            paned: paned.clone(),
        });
        let weak = Rc::downgrade(&me);
        collapse.connect_clicked(move |_| {
            if let Some(i) = weak.upgrade() {
                i.close_viewer();
            }
        });
        me
    }

    /// Follow the session's subagent list. Harnesses without readable
    /// subagent records never show the panel.
    pub fn update(self: &Rc<Self>, s: &Session, supported: Option<SubagentSource>) {
        let Some(hs) = s.harness_session.clone().filter(|_| supported.is_some()) else {
            self.root.set_visible(false);
            return;
        };
        *self.session.borrow_mut() = Some((s.cwd.clone(), hs));
        *self.subs.borrow_mut() = s.subagents.clone();
        self.root.set_visible(!s.subagents.is_empty());
        let mut buttons = self.buttons.borrow_mut();
        for sub in &s.subagents {
            let b = buttons.entry(sub.id.clone()).or_insert_with(|| {
                let b = gtk::ToggleButton::new();
                b.add_css_class("subagent-tab");
                let weak = Rc::downgrade(self);
                let id = sub.id.clone();
                b.connect_clicked(move |b| {
                    let Some(i) = weak.upgrade() else { return };
                    if b.is_active() { i.select(&id) } else { i.close_viewer() }
                });
                self.tabs.append(&b);
                b
            });
            let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            content.append(&gtk::Image::from_icon_name(state_icon(sub.state)));
            let name = sub.description.clone().unwrap_or_else(|| sub.name.clone());
            let label = gtk::Label::new(Some(&name));
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            label.set_max_width_chars(28);
            content.append(&label);
            b.set_child(Some(&content));
            b.set_tooltip_text(Some(&format!("{} · {} · {}", sub.name, name, state_word(sub.state))));
        }
        drop(buttons);
        if let Some(v) = self.viewer.borrow().as_ref()
            && let Some(sub) = s.subagents.iter().find(|x| x.id == v.id)
        {
            self.viewer_title.set_text(&self.title_for(sub));
        }
    }

    fn title_for(&self, sub: &Subagent) -> String {
        let what = sub.description.clone().unwrap_or_default();
        format!("{} — {} ({})", sub.name, what, state_word(sub.state))
    }

    /// Show one subagent's activity in the viewer, live while it runs.
    pub fn select(self: &Rc<Self>, id: &str) {
        let Some((cwd, hs)) = self.session.borrow().clone() else { return };
        let Some(sub) = self.subs.borrow().iter().find(|s| s.id == id).cloned() else { return };
        for (k, b) in self.buttons.borrow().iter() {
            b.set_active(k == id);
        }
        if self.viewer.borrow().as_ref().is_some_and(|v| v.id == id) {
            return;
        }
        self.close_viewer_widget();
        let path = claude::subagent_transcript(&cwd, &hs, &sub.id);
        let view = TerminalView::new(theme::view_style(), true);
        view.set_fit(true);
        view.set_vexpand(true);
        let tail = Rc::new(RefCell::new(claude::Tail::default()));
        let feed = {
            let view = view.downgrade();
            let tail = tail.clone();
            let path = path.clone();
            move || {
                let Some(v) = view.upgrade() else { return };
                loop {
                    let recs = tail.borrow_mut().read(&path);
                    if recs.is_empty() {
                        break;
                    }
                    v.feed(claude::render(&recs).as_bytes());
                }
            }
        };
        feed();
        let monitor = gio::File::for_path(&path).monitor_file(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE);
        let Ok(monitor) = monitor else { return };
        monitor.connect_changed(move |_, _, _, ev| {
            if matches!(
                ev,
                gio::FileMonitorEvent::Changed | gio::FileMonitorEvent::ChangesDoneHint | gio::FileMonitorEvent::Created
            ) {
                feed();
            }
        });
        self.viewer_title.set_text(&self.title_for(&sub));
        self.viewer_box.append(&view);
        self.viewer_box.set_visible(true);
        *self.viewer.borrow_mut() = Some(Viewer { id: id.to_string(), view, _monitor: monitor });
        // The agent's terminal keeps most of the room.
        let paned = self.paned.clone();
        glib::idle_add_local_once(move || {
            let h = paned.height();
            if h > 0 {
                paned.set_position(h * 3 / 5);
            }
        });
    }

    fn close_viewer_widget(&self) {
        if let Some(v) = self.viewer.borrow_mut().take() {
            self.viewer_box.remove(&v.view);
        }
    }

    pub fn close_viewer(&self) {
        self.close_viewer_widget();
        self.viewer_box.set_visible(false);
        for b in self.buttons.borrow().values() {
            b.set_active(false);
        }
        // Back to the tab row alone under the terminal.
        let paned = self.paned.clone();
        glib::idle_add_local_once(move || {
            let h = paned.height();
            if h > 0 {
                paned.set_position(h);
            }
        });
    }
}
