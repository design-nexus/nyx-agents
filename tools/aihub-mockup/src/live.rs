//! A small stand-in host for the real plugin: start it, say hello, draw what it
//! sends with nyx-plugin-view (items, popovers, patches) and send it the events
//! in MOCK_SCRIPT (`open;action:tab:usage;submit:query=nyx`, one every 700 ms).

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::mpsc;

use gtk::prelude::*;
use gtk::glib;
use nyx_plugin_protocol::PluginMsg;
use nyx_plugin_view as view;
use serde_json::json;

pub struct Live {
    /// Where the first item's view goes.
    pub item_box: gtk::Box,
    /// Where its popover (or detail view) goes.
    pub popover_box: gtk::Box,
    pub kind: String,
    pub item: String,
    /// The Control Center's native 2×1 tile, for the `compact` item.
    pub compact: Option<gtk::Box>,
}

fn clear(b: &gtk::Box) {
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
}

pub fn start(live: Live, ctx_item: view::Ctx, ctx_pop: view::Ctx) {
    let exe = std::env::var("MOCK_PLUGIN").unwrap();
    let mut child = Command::new(&exe).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().expect("plugin");
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut items: Vec<&str> = vec![live.item.as_str()];
    if live.compact.is_some() {
        items.push("compact");
    }
    writeln!(
        stdin,
        "{}",
        json!({ "type": "hello", "api_version": 1, "kind": live.kind, "plugin": "nyx.ai-hub", "items": items, "bar_height": 32, "theme": { "light": false, "name": "omarchy" } })
    )
    .unwrap();
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for l in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(l).is_err() {
                break;
            }
        }
    });
    let item_r: Rc<std::cell::RefCell<Option<view::Rendered>>> = Rc::default();
    let pop_r: Rc<std::cell::RefCell<Option<view::Rendered>>> = Rc::default();
    let live = Rc::new(live);
    {
        let (live, item_r, pop_r) = (live.clone(), item_r.clone(), pop_r.clone());
        glib::timeout_add_local(std::time::Duration::from_millis(30), move || {
            while let Ok(line) = rx.try_recv() {
                if line.starts_with("{\"type\":\"popover\"") {
                    eprintln!("host: popover {} bytes, transcript: {}", line.len(), line.contains("\"transcript\""));
                }
                let Ok(msg) = serde_json::from_str::<PluginMsg>(&line) else {
                    eprintln!("host: unreadable line {line}");
                    continue;
                };
                match msg {
                    PluginMsg::Item(it) if it.id == "compact" && live.compact.is_some() => {
                        let b = live.compact.as_ref().unwrap();
                        clear(b);
                        let on = it.class.iter().any(|c| c == "on");
                        let tile = crate::cc_split(it.icon.as_deref().unwrap_or(""), "AI Hub", it.text.as_deref().unwrap_or(""), on, Some("nyx.ai-hub"));
                        b.append(&tile);
                    }
                    PluginMsg::Item(it) if it.id == live.item => {
                        let node = it.view.clone().unwrap_or_else(|| view::simple(&it));
                        let r = view::render(&node, &ctx_item);
                        clear(&live.item_box);
                        live.item_box.append(&r.root);
                        if live.kind == "control-center" {
                            live.item_box.set_tooltip_text(it.text.as_deref());
                        }
                        *item_r.borrow_mut() = Some(r);
                    }
                    PluginMsg::Popover(p) if p.item == live.item => {
                        if let Some(node) = p.view {
                            let r = view::render(&node, &ctx_pop);
                            crate::left_align_rows(&r.root);
                            clear(&live.popover_box);
                            live.popover_box.append(&r.root);
                            *pop_r.borrow_mut() = Some(r);
                        }
                    }
                    PluginMsg::Patch(p) if p.item == live.item => {
                        for n in &p.nodes {
                            if let Some(r) = item_r.borrow().as_ref() {
                                r.patch(n, &ctx_item);
                            }
                            if let Some(r) = pop_r.borrow().as_ref() {
                                r.patch(n, &ctx_pop);
                            }
                        }
                        eprintln!("host: patched {} nodes", p.nodes.len());
                    }
                    PluginMsg::Log { message, .. } => eprintln!("plugin: {message}"),
                    _ => {}
                }
            }
            glib::ControlFlow::Continue
        });
    }
    // The scripted clicks.
    let script = std::env::var("MOCK_SCRIPT").unwrap_or_default();
    let steps: Vec<String> = script.split(';').filter(|s| !s.is_empty()).map(String::from).collect();
    let stdin = Rc::new(std::cell::RefCell::new(stdin));
    let item = live.item.clone();
    for (i, step) in steps.into_iter().enumerate() {
        let (stdin, item) = (stdin.clone(), item.clone());
        glib::timeout_add_local_once(std::time::Duration::from_millis(900 + 700 * i as u64), move || {
            let ev = if step == "open" {
                json!({ "type": "event", "item": item, "kind": "popover_open" })
            } else if let Some(a) = step.strip_prefix("action:") {
                json!({ "type": "event", "item": item, "kind": "action", "control": a })
            } else if let Some(rest) = step.strip_prefix("submit:") {
                let (control, value) = rest.split_once('=').unwrap_or((rest, ""));
                json!({ "type": "event", "item": item, "kind": "submit", "control": control, "value": value })
            } else {
                return;
            };
            let _ = writeln!(stdin.borrow_mut(), "{ev}");
        });
    }
    // A host keeps the plugin's stdin open; closing it tells the plugin to quit.
    std::mem::forget(stdin);
    std::mem::forget(child);
}
