//! History: every agent's past sessions, read from their own records. Resume
//! one here, peek at its transcript, continue it in a terminal, or move its
//! records to the Trash.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};
use nyx_agents_proto::{Call, PastSession, Session};

use super::daemon;

type Key = (String, String);

pub struct History {
    pub root: gtk::Box,
    chips: gtk::Box,
    search: gtk::SearchEntry,
    content: gtk::Box,
    items: RefCell<Vec<PastSession>>,
    loaded: std::cell::Cell<bool>,
    filter: RefCell<Option<String>>,
    open: RefCell<Option<Key>>,
    name: Rc<dyn Fn(&str) -> String>,
    /// Show a session of this app (resumed, or already running).
    show: Rc<dyn Fn(Session)>,
    show_id: Rc<dyn Fn(&str)>,
    toast: Rc<dyn Fn(&str)>,
}

impl History {
    pub fn new(
        name: Rc<dyn Fn(&str) -> String>,
        show: Rc<dyn Fn(Session)>,
        show_id: Rc<dyn Fn(&str)>,
        toast: Rc<dyn Fn(&str)>,
    ) -> Rc<History> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("history-page");
        root.set_hexpand(true);
        root.set_vexpand(true);
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        bar.add_css_class("session-bar");
        let title = gtk::Label::new(Some("History"));
        title.add_css_class("history-title");
        bar.append(&title);
        let chips = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        // Many agents mustn't widen the window: the chips scroll instead.
        let chip_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::External)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .hexpand(true)
            .child(&chips)
            .build();
        chip_scroll.set_margin_start(8);
        bar.append(&chip_scroll);
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Search titles and folders"));
        search.set_width_chars(26);
        bar.append(&search);
        let refresh = nyx_ui::widgets::bar_button("view-refresh-symbolic", "Look again");
        bar.append(&refresh);
        root.append(&bar);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 2);
        content.add_css_class("history-list");
        let scroll = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).child(&content).build();
        root.append(&scroll);
        let me = Rc::new(History {
            root,
            chips,
            search,
            content,
            items: RefCell::new(Vec::new()),
            loaded: Default::default(),
            filter: RefCell::new(None),
            open: RefCell::new(None),
            name,
            show,
            show_id,
            toast,
        });
        let weak = Rc::downgrade(&me);
        me.search.connect_search_changed(move |_| {
            if let Some(h) = weak.upgrade() {
                h.render();
            }
        });
        let weak = Rc::downgrade(&me);
        refresh.connect_clicked(move |_| {
            if let Some(h) = weak.upgrade() {
                h.load();
            }
        });
        me
    }

    /// Read the list again from the manager.
    pub fn load(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        daemon::request(
            |c| c.call::<Vec<PastSession>>(Call::ListHistory { harness: None, query: None, limit: Some(1000) }),
            move |r| {
                let Some(h) = weak.upgrade() else { return };
                match r {
                    Ok(list) => *h.items.borrow_mut() = list,
                    Err(e) => (h.toast)(&e),
                }
                h.loaded.set(true);
                h.render();
            },
        );
    }

    /// Show one agent's sessions (`None` for all).
    pub fn set_filter(self: &Rc<Self>, harness: Option<String>) {
        *self.filter.borrow_mut() = harness;
        self.render();
    }

    pub fn focus_search(&self) {
        self.search.grab_focus();
    }

    fn render(self: &Rc<Self>) {
        while let Some(c) = self.chips.first_child() {
            self.chips.remove(&c);
        }
        while let Some(c) = self.content.first_child() {
            self.content.remove(&c);
        }
        let items = self.items.borrow();
        let filter = self.filter.borrow().clone();
        let mut agents: Vec<String> = Vec::new();
        for p in items.iter() {
            if !agents.contains(&p.harness) {
                agents.push(p.harness.clone());
            }
        }
        agents.sort_by_key(|a| (self.name)(a));
        let chip = |label: &str, value: Option<String>| {
            let b = gtk::Button::with_label(label);
            b.add_css_class("chip");
            if filter == value {
                b.add_css_class("selected");
            }
            let weak = Rc::downgrade(self);
            b.connect_clicked(move |_| {
                if let Some(h) = weak.upgrade() {
                    h.set_filter(value.clone());
                }
            });
            b
        };
        self.chips.append(&chip("All", None));
        for a in &agents {
            self.chips.append(&chip(&(self.name)(a), Some(a.clone())));
        }

        let q = self.search.text().to_lowercase();
        let shown: Vec<&PastSession> = items
            .iter()
            .filter(|p| filter.as_deref().is_none_or(|f| f == p.harness))
            .filter(|p| q.is_empty() || p.title.to_lowercase().contains(&q) || p.cwd.to_lowercase().contains(&q))
            .collect();
        if shown.is_empty() {
            let text = if !self.loaded.get() {
                "Reading every agent's records…"
            } else if items.is_empty() {
                "No past sessions yet."
            } else {
                "Nothing matches."
            };
            let l = gtk::Label::new(Some(text));
            l.add_css_class("session-meta");
            l.set_margin_top(24);
            self.content.append(&l);
            return;
        }
        let today = glib::DateTime::now_local().ok().and_then(|d| d.format("%Y-%m-%d").ok()).map(|s| s.to_string());
        let yesterday = glib::DateTime::now_local()
            .ok()
            .and_then(|d| d.add_days(-1).ok())
            .and_then(|d| d.format("%Y-%m-%d").ok())
            .map(|s| s.to_string());
        let mut last_day = String::new();
        for p in shown.into_iter().take(300) {
            let when = glib::DateTime::from_unix_local(p.updated as i64).ok();
            let day = when.as_ref().and_then(|d| d.format("%Y-%m-%d").ok()).map(|s| s.to_string()).unwrap_or_default();
            if day != last_day {
                let label = if Some(&day) == today.as_ref() {
                    "Today".to_string()
                } else if Some(&day) == yesterday.as_ref() {
                    "Yesterday".to_string()
                } else {
                    when.as_ref().and_then(|d| d.format("%A %e %B").ok()).map(|s| s.to_string()).unwrap_or_default()
                };
                let l = gtk::Label::new(Some(&label));
                l.add_css_class("history-day");
                l.set_xalign(0.0);
                self.content.append(&l);
                last_day = day;
            }
            let time = when.as_ref().and_then(|d| d.format("%H:%M").ok()).map(|s| s.to_string()).unwrap_or_default();
            self.content.append(&self.row(p, &time));
        }
    }

    fn row(self: &Rc<Self>, p: &PastSession, time: &str) -> gtk::Box {
        let key = (p.harness.clone(), p.id.clone());
        let open = self.open.borrow().as_ref() == Some(&key);
        let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        outer.add_css_class("history-item");
        if open {
            outer.add_css_class("open");
        }
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        line.append(&nyx_ui::widgets::tag(&(self.name)(&p.harness), ""));
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        let title = gtk::Label::new(Some(if p.title.is_empty() { "Untitled" } else { &p.title }));
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let mut sub = vec![nyx_core::paths::pretty(std::path::Path::new(&p.cwd))];
        sub.push(format!("{} prompt{}", p.prompts, if p.prompts == 1 { "" } else { "s" }));
        if let Some(m) = &p.model {
            sub.push(m.clone());
        }
        let sub = gtk::Label::new(Some(&sub.join(" · ")));
        sub.add_css_class("session-meta");
        sub.set_xalign(0.0);
        sub.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        text.append(&title);
        text.append(&sub);
        line.append(&text);
        if p.active {
            line.append(&nyx_ui::widgets::tag("running", "accent"));
        } else if p.managed.is_some() {
            line.append(&nyx_ui::widgets::tag("in Agents", ""));
        }
        let t = gtk::Label::new(Some(time));
        t.add_css_class("session-meta");
        line.append(&t);
        let head = gtk::Button::new();
        head.add_css_class("flat");
        head.add_css_class("history-row");
        head.set_child(Some(&line));
        let weak = Rc::downgrade(self);
        head.connect_clicked(move |_| {
            let Some(h) = weak.upgrade() else { return };
            let mut o = h.open.borrow_mut();
            *o = if o.as_ref() == Some(&key) { None } else { Some(key.clone()) };
            drop(o);
            h.render();
        });
        outer.append(&head);
        if open {
            outer.append(&self.details(p));
        }
        outer
    }

    fn details(self: &Rc<Self>, p: &PastSession) -> gtk::Box {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 8);
        b.add_css_class("history-details");
        for (k, v) in [("Folder", p.cwd.as_str()), ("Session ID", p.id.as_str())] {
            let (row, value) = nyx_ui::widgets::kv(k);
            value.set_text(v);
            value.set_selectable(true);
            value.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            b.append(&row);
        }
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let (harness, id) = (p.harness.clone(), p.id.clone());
        if let Some(mid) = p.managed.clone().filter(|_| p.active) {
            let show = self.show_id.clone();
            buttons.append(&action("Open", true, move || show(&mid)));
        } else if p.resumable && !p.active {
            let (h, i, show, toast) = (harness.clone(), id.clone(), self.show.clone(), self.toast.clone());
            buttons.append(&action("Resume here", true, move || {
                let (h, i, show, toast) = (h.clone(), i.clone(), show.clone(), toast.clone());
                daemon::request(
                    move |c| c.call::<Session>(Call::ResumePast { harness: h, id: i }),
                    move |r| match r {
                        Ok(s) => show(s),
                        Err(e) => toast(&e),
                    },
                );
            }));
        }
        {
            let (h, i, toast) = (harness.clone(), id.clone(), self.toast.clone());
            let title = p.title.clone();
            buttons.append(&action("Peek", false, move || peek(&h, &i, &title, toast.clone())));
        }
        if p.resumable && !p.active {
            let (h, i, toast) = (harness.clone(), id.clone(), self.toast.clone());
            buttons.append(&action("In a terminal", false, move || in_terminal(&h, &i, toast.clone())));
        }
        if std::path::Path::new(&p.cwd).is_dir() {
            let cwd = p.cwd.clone();
            buttons.append(&action("Folder", false, move || {
                let _ = std::process::Command::new("xdg-open").arg(&cwd).spawn();
            }));
        }
        {
            let (i, toast) = (id.clone(), self.toast.clone());
            buttons.append(&action("Copy ID", false, move || {
                if let Some(d) = gdk::Display::default() {
                    d.clipboard().set_text(&i);
                    toast("Copied");
                }
            }));
        }
        if p.can_trash {
            let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            spacer.set_hexpand(true);
            buttons.append(&spacer);
            let weak = Rc::downgrade(self);
            let trash = nyx_ui::widgets::two_click("Move to Trash", "Click again to move it", move || {
                let (h, i, weak) = (harness.clone(), id.clone(), weak.clone());
                daemon::request(
                    move |c| c.call::<serde_json::Value>(Call::TrashPast { harness: h, id: i }),
                    move |r| {
                        let Some(me) = weak.upgrade() else { return };
                        match r {
                            Ok(_) => (me.toast)("Moved to the Trash"),
                            Err(e) => (me.toast)(&e),
                        }
                        me.load();
                    },
                );
            });
            buttons.append(&trash);
        }
        b.append(&buttons);
        b
    }
}

fn action(label: &str, primary: bool, run: impl Fn() + 'static) -> gtk::Button {
    let b = gtk::Button::with_label(label);
    if primary {
        b.add_css_class("suggested-action");
    }
    b.connect_clicked(move |_| run());
    b
}

/// The session's transcript, read-only, in a dialog.
fn peek(harness: &str, id: &str, title: &str, toast: Rc<dyn Fn(&str)>) {
    let (h, i, title) = (harness.to_string(), id.to_string(), title.to_string());
    daemon::request(
        move |c| c.call::<serde_json::Value>(Call::Peek { harness: h, id: i }),
        move |r| match r {
            Ok(v) => {
                let text = v["text"].as_str().unwrap_or_default().to_string();
                let (dialog, card) = nyx_ui::widgets::dialog(if title.is_empty() { "Transcript" } else { &title }, 720);
                dialog.set_default_height(560);
                let view = gtk::TextView::new();
                view.set_editable(false);
                view.set_monospace(true);
                view.set_wrap_mode(gtk::WrapMode::WordChar);
                view.buffer().set_text(&text);
                let scroll = gtk::ScrolledWindow::builder().vexpand(true).child(&view).build();
                scroll.set_min_content_height(420);
                card.append(&scroll);
                dialog.present();
            }
            Err(e) => toast(&e),
        },
    );
}

/// Continue the session in your own terminal, outside Agents.
fn in_terminal(harness: &str, id: &str, toast: Rc<dyn Fn(&str)>) {
    let (h, i) = (harness.to_string(), id.to_string());
    daemon::request(
        move |c| c.call::<serde_json::Value>(Call::ResumeInTerminal { harness: h, id: i }),
        move |r| match r {
            Ok(v) => {
                let argv: Vec<String> = serde_json::from_value(v["argv"].clone()).unwrap_or_default();
                let cwd = v["cwd"].as_str().unwrap_or("/").to_string();
                let Some((exe, args)) = argv.split_first() else { return };
                use std::os::unix::process::CommandExt;
                let r = std::process::Command::new(exe)
                    .args(args)
                    .current_dir(cwd)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .process_group(0)
                    .spawn();
                match r {
                    Ok(mut child) => {
                        std::thread::spawn(move || child.wait());
                    }
                    Err(e) => toast(&format!("Could not open a terminal: {e}")),
                }
            }
            Err(e) => toast(&e),
        },
    );
}
