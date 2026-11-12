//! AI Hub, a Nyx Bar and Control Center plugin. It shows what Nyx Agents and Nyx
//! AI Core report: agents that need you and what's working, each agent's usage and
//! quota with its own page, past sessions to resume or tidy up, and local models.
//! It only reads and asks; the manager and AI Core do the work.

mod diff;
mod sources;
mod views;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use nyx_agents_proto::{Call, Event, Harness, Launch, PastSession, Session, Usage};
use nyx_plugin_sdk::protocol::{EventKind, Item, NodePatch, Patch, PluginMsg, Popover};
use nyx_plugin_sdk::view::*;
use nyx_plugin_sdk::{Input, Plugin, spawn};
use serde_json::Value;

use sources::{Data, Local};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Host {
    Bar,
    ControlCenter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Agents,
    Usage,
    Sessions,
    Local,
}

/// Which counts an agent page's model list shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Range {
    Today,
    Week,
    All,
}

pub struct State {
    pub host: Host,
    pub connected: bool,
    pub sessions: Vec<Session>,
    pub usage: Vec<Usage>,
    pub history: Vec<PastSession>,
    pub harnesses: Vec<Harness>,
    pub local: Option<Local>,
    pub local_checked: bool,
    pub tab: Tab,
    /// The agent page open under Usage.
    pub agent: Option<String>,
    pub range: Range,
    pub filter: Option<String>,
    pub query: String,
    /// The past session whose actions are open.
    pub picked: Option<(String, String)>,
    /// The past session whose Trash button was clicked once.
    pub armed: Option<(String, String)>,
    pub peek: Option<(String, String, Result<String, String>)>,
    /// A failure to mention, for a few seconds.
    pub notice: Option<String>,
    notice_at: Option<Instant>,
    /// A popover or detail view is showing.
    open: bool,
}

fn sort_usage(all: &mut [Usage]) {
    all.sort_by(|a, b| b.working.cmp(&a.working).then(b.last_used.cmp(&a.last_used)).then(a.name.cmp(&b.name)));
}

fn tab_named(s: &str) -> Tab {
    match s {
        "usage" => Tab::Usage,
        "sessions" => Tab::Sessions,
        "local" => Tab::Local,
        _ => Tab::Agents,
    }
}

impl State {
    fn new(host: Host) -> Self {
        Self {
            host,
            connected: false,
            sessions: vec![],
            usage: vec![],
            history: vec![],
            harnesses: vec![],
            local: None,
            local_checked: false,
            tab: Tab::Agents,
            agent: None,
            range: Range::Today,
            filter: None,
            query: String::new(),
            picked: None,
            armed: None,
            peek: None,
            notice: None,
            notice_at: None,
            open: false,
        }
    }

    fn say(&mut self, msg: String) {
        self.notice = Some(msg);
        self.notice_at = Some(Instant::now());
    }

    fn apply(&mut self, d: Data) {
        match d {
            Data::Snapshot { sessions, mut usage, history, harnesses } => {
                self.connected = true;
                self.sessions = sessions;
                sort_usage(&mut usage);
                self.usage = usage;
                self.history = history;
                self.harnesses = harnesses;
            }
            Data::Event(Event::Session { session }) => match self.sessions.iter_mut().find(|s| s.id == session.id) {
                Some(s) => *s = *session,
                None => self.sessions.push(*session),
            },
            Data::Event(Event::Removed { id }) => self.sessions.retain(|s| s.id != id),
            Data::Event(Event::Harnesses { harnesses }) => self.harnesses = harnesses,
            Data::Event(Event::Usage { usage }) => {
                self.usage.retain(|u| u.harness != usage.harness);
                if usage.has_records || usage.quota.enabled {
                    self.usage.push(*usage);
                }
                sort_usage(&mut self.usage);
            }
            Data::Event(_) => {}
            Data::Usage(mut list) => {
                sort_usage(&mut list);
                self.usage = list;
            }
            Data::History(h) => self.history = h,
            Data::Disconnected => {
                self.connected = false;
                self.sessions.clear();
            }
            Data::Local(l) => {
                self.local = l;
                self.local_checked = true;
            }
            Data::Peek { harness, id, text } => self.peek = Some((harness, id, text)),
            Data::Failed(e) => self.say(e),
            Data::Done | Data::Tick => {}
        }
        if self.notice_at.is_some_and(|t| t.elapsed() > Duration::from_secs(8)) {
            self.notice = None;
            self.notice_at = None;
        }
    }

    fn past(&self, key: &str) -> Option<&PastSession> {
        let (h, id) = key.split_once(':')?;
        self.history.iter().find(|p| p.harness == h && p.id == id)
    }
}

fn open_session(id: &str) {
    spawn(&["nyx-agents", "--open", &format!("session/{id}")]);
}

/// What a click on a row or button asks for.
fn action(st: &mut State, a: &str, send: &(impl Fn(Data) + Send + Clone + 'static)) {
    let (verb, rest) = a.split_once(':').unwrap_or((a, ""));
    let pair = || rest.split_once(':').map(|(h, id)| (h.to_string(), id.to_string()));
    match verb {
        "tab" => {
            st.tab = tab_named(rest);
            st.agent = None;
            if st.tab == Tab::Local {
                sources::local(send.clone());
            }
        }
        "open-app" => spawn(&["nyx-agents"]),
        "open" => open_session(rest),
        "stop" => sources::call::<Value>(Call::Stop { id: rest.into(), force: false }, send.clone(), |_| None),
        "resume" => sources::call::<Session>(Call::Resume { id: rest.into() }, send.clone(), |s| {
            open_session(&s.id);
            None
        }),
        "agent" => {
            st.tab = Tab::Usage;
            st.agent = Some(rest.into());
        }
        "back" => st.agent = None,
        "refresh" if rest.is_empty() => sources::call::<Vec<Usage>>(Call::ListUsage { refresh: true }, send.clone(), |v| Some(Data::Usage(v))),
        "refresh" => sources::call::<Usage>(Call::GetUsage { harness: rest.into(), refresh: true }, send.clone(), |u| {
            Some(Data::Event(Event::Usage { usage: Box::new(u) }))
        }),
        "new" => {
            // Where this agent last worked, else the home folder.
            let cwd = st
                .usage
                .iter()
                .find(|u| u.harness == rest)
                .and_then(|u| u.sessions.iter().find(|s| !s.cwd.is_empty() && std::path::Path::new(&s.cwd).is_dir()))
                .map(|s| s.cwd.clone())
                .unwrap_or_else(|| std::env::var("HOME").unwrap_or_else(|_| "/".into()));
            let l = Launch { harness: rest.into(), cwd, cols: 0, rows: 0, ..Default::default() };
            sources::call::<Session>(Call::Launch(l), send.clone(), |s| {
                open_session(&s.id);
                None
            });
        }
        "range" => {
            st.range = match rest {
                "week" => Range::Week,
                "all" => Range::All,
                _ => Range::Today,
            }
        }
        "sessions" => {
            st.tab = Tab::Sessions;
            st.agent = None;
            st.filter = Some(rest.into());
        }
        "filter" => st.filter = (rest != "all").then(|| rest.to_string()),
        "pick" => {
            let key = pair();
            st.picked = if st.picked == key { None } else { key };
            st.armed = None;
            st.peek = None;
        }
        "past" => {
            if let Some((harness, id)) = pair() {
                sources::call::<Session>(Call::ResumePast { harness, id }, send.clone(), |s| {
                    open_session(&s.id);
                    None
                });
            }
        }
        "pterm" => {
            if let Some((harness, id)) = pair() {
                sources::call::<Value>(Call::ResumeInTerminal { harness, id }, send.clone(), |r| {
                    let argv: Vec<String> = serde_json::from_value(r["argv"].clone()).unwrap_or_default();
                    let cwd = r["cwd"].as_str().unwrap_or("/").to_string();
                    if let Some((exe, args)) = argv.split_first() {
                        use std::os::unix::process::CommandExt;
                        let mut c = std::process::Command::new(exe);
                        c.args(args).current_dir(cwd).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).process_group(0);
                        if let Ok(mut child) = c.spawn() {
                            std::thread::spawn(move || child.wait());
                        }
                    }
                    None
                });
            }
        }
        "peek" => {
            if let Some((harness, id)) = pair() {
                let (h2, i2, s2) = (harness.clone(), id.clone(), send.clone());
                std::thread::spawn(move || {
                    let r = nyx_agents_client::Client::connect("nyx-ai-hub").and_then(|mut c| c.call::<Value>(Call::Peek { harness: h2.clone(), id: i2.clone() }));
                    let text = r.map(|v| v["text"].as_str().unwrap_or("").to_string()).map_err(|e| e.to_string());
                    s2(Data::Peek { harness: h2, id: i2, text });
                });
            }
        }
        "folder" => {
            if let Some(p) = st.past(rest).filter(|p| !p.cwd.is_empty()) {
                spawn(&["xdg-open", &p.cwd]);
            }
        }
        "copy" => {
            if let Some((_, id)) = pair() {
                spawn(&["wl-copy", &id]);
            }
        }
        "trash" => {
            let key = pair();
            if let Some((harness, id)) = key.clone().filter(|_| st.armed == key) {
                st.armed = None;
                st.picked = None;
                st.history.retain(|p| !(p.harness == harness && p.id == id));
                let s2 = send.clone();
                sources::call::<Value>(Call::TrashPast { harness, id }, send.clone(), move |_| {
                    sources::history(s2);
                    None
                });
            } else {
                st.armed = key;
            }
        }
        "rescan" => {
            sources::history(send.clone());
            sources::call::<Vec<Usage>>(Call::ListUsage { refresh: false }, send.clone(), |v| Some(Data::Usage(v)));
        }
        "load" => sources::model(true, rest.into(), send.clone()),
        "unload" => sources::model(false, rest.into(), send.clone()),
        "copy-endpoint" => spawn(&["wl-copy", &format!("http://{rest}/v1")]),
        "ai-settings" => spawn(&["nyx-settings", "--section", "local-ai"]),
        _ => {}
    }
}

/// What the host was last sent, so changes go out as patches when they can.
#[derive(Default)]
struct Sent {
    items: HashMap<String, Value>,
    popovers: HashMap<String, Value>,
}

fn patch(plugin: &Plugin<Data>, item: &str, p: Vec<(String, serde_json::Map<String, Value>)>) {
    if !p.is_empty() {
        let nodes = p.into_iter().map(|(id, set)| NodePatch { id, set }).collect();
        plugin.send(&PluginMsg::Patch(Patch { item: item.into(), monitor: None, nodes }));
    }
}

fn send_item(plugin: &Plugin<Data>, sent: &mut Sent, mut it: Item) {
    let id = it.id.clone();
    let view = it.view.take().map(|v| diff::with_ids(&v, "i"));
    let mut whole = serde_json::to_value(&it).unwrap_or(Value::Null);
    whole["view"] = view.clone().unwrap_or(Value::Null);
    match (sent.items.get(&id), &view) {
        (Some(old), _) if *old == whole => return,
        (Some(old), Some(v)) if strip_view(old) == strip_view(&whole) => {
            if let Some(p) = diff::patches(&old["view"], v) {
                patch(plugin, &id, p);
                sent.items.insert(id, whole);
                return;
            }
        }
        _ => {}
    }
    if let Ok(item) = serde_json::from_value::<Item>(whole.clone()) {
        plugin.item(item);
    }
    sent.items.insert(id, whole);
}

fn strip_view(v: &Value) -> Value {
    let mut v = v.clone();
    if let Some(o) = v.as_object_mut() {
        o.remove("view");
    }
    v
}

fn send_popover(plugin: &Plugin<Data>, sent: &mut Sent, item: &str, open: bool, view: &nyx_plugin_sdk::protocol::Node) {
    let v = diff::with_ids(view, "p");
    if let Some(old) = sent.popovers.get(item) {
        if *old == v {
            return;
        }
        // While it shows, patch it in place so it keeps its scroll position.
        if open && let Some(p) = diff::patches(old, &v) {
            patch(plugin, item, p);
            sent.popovers.insert(item.into(), v);
            return;
        }
    }
    if let Ok(node) = serde_json::from_value(v.clone()) {
        plugin.send(&PluginMsg::Popover(Popover { item: item.into(), monitor: None, view: Some(node) }));
    }
    sent.popovers.insert(item.into(), v);
}

fn render(plugin: &Plugin<Data>, st: &State, sent: &mut Sent) {
    let pop = views::popover(st);
    match st.host {
        Host::Bar => {
            let mut it = item("hub").view(views::bar_item(st));
            it.on_middle_click = None;
            it.on_right_click = Some(vec!["nyx-agents".into()]);
            send_item(plugin, sent, it);
            send_popover(plugin, sent, "hub", st.open, &pop);
        }
        Host::ControlCenter => {
            if plugin.enabled("overview") {
                let mut it = item("overview").view(views::overview_tile(st));
                it.tooltip = Some(views::summary(st));
                send_item(plugin, sent, it);
                send_popover(plugin, sent, "overview", st.open, &pop);
            }
            if plugin.enabled("compact") {
                let (status, on) = views::compact_status(st);
                let mut it = item("compact").icon("nyx-ai-hub-symbolic").text(status).tooltip(views::summary(st));
                if on {
                    it = it.class("on");
                }
                it.on_click = Some(vec!["nyx-agents".into()]);
                send_item(plugin, sent, it);
                send_popover(plugin, sent, "compact", st.open, &pop);
            }
        }
    }
}

fn main() {
    let (plugin, inputs) = Plugin::<Data>::start();
    let host = if plugin.hello.kind == "control-center" { Host::ControlCenter } else { Host::Bar };
    let mut st = State::new(host);
    let send = plugin.data();
    sources::follow(send.clone());
    sources::local(send.clone());
    {
        let tick = send.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(5));
                tick(Data::Tick);
            }
        });
    }
    let mut sent = Sent::default();
    let mut last_local = Instant::now();
    render(&plugin, &st, &mut sent);
    for input in inputs {
        match input {
            Input::Data(Data::Tick) => {
                // AI Core has no events: ask every 5 s while the hub shows, else every minute.
                let every = if st.open { 5 } else { 60 };
                if last_local.elapsed() >= Duration::from_secs(every) {
                    last_local = Instant::now();
                    sources::local(send.clone());
                }
                st.apply(Data::Tick);
            }
            Input::Data(d) => st.apply(d),
            Input::Event(e) => match e.kind {
                EventKind::PopoverOpen => {
                    st.open = true;
                    last_local = Instant::now();
                    sources::local(send.clone());
                }
                EventKind::PopoverClose => {
                    st.open = false;
                    st.armed = None;
                }
                EventKind::Action => {
                    if let Some(a) = e.control.as_deref() {
                        action(&mut st, a, &send);
                    }
                }
                EventKind::Submit if e.control.as_deref() == Some("query") => {
                    st.query = e.value.as_ref().and_then(Value::as_str).unwrap_or("").trim().to_string();
                }
                EventKind::Click if e.button == Some(2) => {
                    action(&mut st, "refresh", &send);
                }
                _ => {}
            },
            Input::Shutdown => break,
            _ => continue,
        }
        render(&plugin, &st, &mut sent);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn manifest_is_valid() {
        let m = nyx_plugin::Manifest::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))).expect("plugin.toml");
        assert_eq!(m.id, "nyx.ai-hub");
        assert!(m.has_kind(nyx_plugin::Kind::Bar) && m.has_kind(nyx_plugin::Kind::ControlCenter));
    }
}
