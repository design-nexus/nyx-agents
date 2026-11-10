//! Desktop notifications (org.freedesktop.Notifications) for what needs the
//! user: an agent waiting for approval, a finished turn, an error, an
//! unexpected exit. Clicking one opens the session in Nyx Agents. The
//! session on screen in the app is not notified about.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use zbus::zvariant::Value;

const DEST: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const APP_ID: &str = "io.github.design_nexus.nyx.Agents";

pub struct Notifier {
    conn: zbus::blocking::Connection,
    /// Notification id → session id, for the Open action.
    shown: Arc<Mutex<HashMap<u32, String>>>,
}

/// `[notifications] enabled = false` in `~/.config/nyx-agents/config.toml` turns them off.
pub fn enabled() -> bool {
    let path = nyx_agents_core::paths::config_dir().join("config.toml");
    let Ok(text) = std::fs::read_to_string(path) else { return true };
    let table: toml::Table = toml::from_str(&text).unwrap_or_default();
    table.get("notifications").and_then(|n| n.get("enabled")).and_then(|v| v.as_bool()).unwrap_or(true)
}

impl Notifier {
    pub fn new() -> Option<Arc<Self>> {
        let conn = zbus::blocking::Connection::session().ok()?;
        let me = Arc::new(Self { conn: conn.clone(), shown: Arc::new(Mutex::new(HashMap::new())) });
        let shown = me.shown.clone();
        std::thread::spawn(move || {
            let rule = zbus::MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .interface(DEST)
                .and_then(|b| b.member("ActionInvoked"))
                .map(|b| b.build());
            let Ok(rule) = rule else { return };
            let Ok(iter) = zbus::blocking::MessageIterator::for_match_rule(rule, &conn, Some(16)) else { return };
            for msg in iter.flatten() {
                let Ok((id, action)) = msg.body().deserialize::<(u32, String)>() else { continue };
                let Some(session) = shown.lock().unwrap().remove(&id) else { continue };
                if action == "default" || action == "open" {
                    let _ = std::process::Command::new("nyx-agents")
                        .args(["--open", &format!("session/{session}")])
                        .stdin(std::process::Stdio::null())
                        .spawn();
                }
            }
        });
        Some(me)
    }

    pub fn send(&self, session: &str, summary: &str, body: &str, urgent: bool) {
        let mut hints: HashMap<&str, Value> = HashMap::new();
        hints.insert("desktop-entry", Value::from(APP_ID));
        hints.insert("urgency", Value::from(if urgent { 2u8 } else { 1u8 }));
        let actions = vec!["default", "Open", "open", "Open"];
        let reply = self.conn.call_method(
            Some(DEST),
            PATH,
            Some(DEST),
            "Notify",
            &("Agents", 0u32, APP_ID, summary, body, actions, hints, -1i32),
        );
        if let Ok(r) = reply
            && let Ok(id) = r.body().deserialize::<u32>()
        {
            let mut shown = self.shown.lock().unwrap();
            if shown.len() > 200 {
                shown.clear();
            }
            shown.insert(id, session.to_string());
        }
    }
}
