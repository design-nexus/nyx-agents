//! Updates to a view already on screen as patches, so an open popover keeps its
//! scroll position and focus. Every node gets an id from its place in the tree;
//! when only patchable properties changed, those are sent, else the whole view.

use nyx_plugin_sdk::protocol::Node;
use serde_json::{Map, Value};

/// Properties a host can patch in place.
const PATCHABLE: &[&str] = &[
    "text", "icon", "name", "title", "subtitle", "label", "active", "value", "values", "max", "commands", "visible", "tooltip",
    "class", "color", "background",
];

/// Give each node without an id one from its path, so any node can be patched.
pub fn with_ids(node: &Node, prefix: &str) -> Value {
    let mut v = serde_json::to_value(node).unwrap_or(Value::Null);
    fill(&mut v, prefix);
    v
}

fn fill(v: &mut Value, path: &str) {
    if v.get("id").is_none_or(Value::is_null) {
        v["id"] = Value::String(path.to_string());
    }
    if let Some(children) = v.get_mut("children").and_then(Value::as_array_mut) {
        for (i, c) in children.iter_mut().enumerate() {
            fill(c, &format!("{path}.{i}"));
        }
    }
    if let Some(child) = v.get_mut("child").filter(|c| c.is_object()) {
        fill(child, &format!("{path}.c"));
    }
}

/// The patches that turn `old` into `new`, or None when their shapes differ.
pub fn patches(old: &Value, new: &Value) -> Option<Vec<(String, Map<String, Value>)>> {
    let mut out = vec![];
    walk(old, new, &mut out).then_some(out)
}

fn walk(old: &Value, new: &Value, out: &mut Vec<(String, Map<String, Value>)>) -> bool {
    let (Some(o), Some(n)) = (old.as_object(), new.as_object()) else { return old == new };
    if o.get("node") != n.get("node") || o.get("id") != n.get("id") {
        return false;
    }
    let mut set = Map::new();
    let keys: std::collections::BTreeSet<&String> = o.keys().chain(n.keys()).collect();
    for k in keys {
        if matches!(k.as_str(), "children" | "child" | "id" | "node") {
            continue;
        }
        match (o.get(k), n.get(k)) {
            (a, b) if a == b => {}
            (_, Some(b)) if PATCHABLE.contains(&k.as_str()) => {
                set.insert(k.clone(), b.clone());
            }
            _ => return false,
        }
    }
    let empty = vec![];
    let (oc, nc) = (o.get("children").and_then(Value::as_array).unwrap_or(&empty), n.get("children").and_then(Value::as_array).unwrap_or(&empty));
    if oc.len() != nc.len() {
        return false;
    }
    if !set.is_empty() {
        out.push((n["id"].as_str().unwrap_or_default().to_string(), set));
    }
    if !oc.iter().zip(nc).all(|(a, b)| walk(a, b, out)) {
        return false;
    }
    match (o.get("child"), n.get("child")) {
        (Some(a), Some(b)) => walk(a, b, out),
        (None, None) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyx_plugin_sdk::view::*;

    #[test]
    fn text_changes_patch_and_new_rows_resend() {
        let a = with_ids(&vbox([label("2 agents"), row(None, "Fix it", Some("codex".into()), Some("open:1"))]), "p");
        let b = with_ids(&vbox([label("3 agents"), row(None, "Fix it", Some("codex · 1 min".into()), Some("open:1"))]), "p");
        let p = patches(&a, &b).unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].0, "p.0");
        assert_eq!(p[0].1["text"], "3 agents");
        assert_eq!(p[1].1["subtitle"], "codex · 1 min");
        let c = with_ids(&vbox([label("3 agents")]), "p");
        assert!(patches(&b, &c).is_none());
        assert_eq!(patches(&a, &a).unwrap().len(), 0);
    }
}
