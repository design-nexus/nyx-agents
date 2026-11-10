//! sway's IPC (the i3 protocol): `"i3-ipc"`, payload length and message type
//! as native-endian u32s, then a JSON payload.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

const MAGIC: &[u8; 6] = b"i3-ipc";
const HEADER: usize = 14;
const MAX_REPLY: usize = 64 << 20;

pub const RUN_COMMAND: u32 = 0;
pub const GET_OUTPUTS: u32 = 3;
pub const GET_TREE: u32 = 4;

pub fn encode(ty: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER + payload.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(payload.len() as u32).to_ne_bytes());
    out.extend_from_slice(&ty.to_ne_bytes());
    out.extend_from_slice(payload);
    out
}

/// The payload length and message type from a reply header.
pub fn decode_header(h: &[u8; HEADER]) -> io::Result<(usize, u32)> {
    if &h[..6] != MAGIC {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "not an i3-ipc reply"));
    }
    let len = u32::from_ne_bytes(h[6..10].try_into().unwrap()) as usize;
    let ty = u32::from_ne_bytes(h[10..14].try_into().unwrap());
    if len > MAX_REPLY {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "i3-ipc reply too large"));
    }
    Ok((len, ty))
}

pub struct Ipc {
    stream: UnixStream,
}

impl Ipc {
    pub fn connect(path: &Path) -> io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        Ok(Self { stream })
    }

    pub fn send(&mut self, ty: u32, payload: &str) -> io::Result<Value> {
        self.stream.write_all(&encode(ty, payload.as_bytes()))?;
        let mut h = [0u8; HEADER];
        self.stream.read_exact(&mut h)?;
        let (len, _) = decode_header(&h)?;
        let mut body = vec![0u8; len];
        self.stream.read_exact(&mut body)?;
        serde_json::from_slice(&body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    /// Run sway commands; the first failure's message is the error.
    pub fn run(&mut self, command: &str) -> Result<(), String> {
        let reply = self.send(RUN_COMMAND, command).map_err(|e| e.to_string())?;
        command_result(&reply)
    }

    pub fn tree(&mut self) -> io::Result<Value> {
        self.send(GET_TREE, "")
    }

    pub fn outputs(&mut self) -> io::Result<Value> {
        self.send(GET_OUTPUTS, "")
    }
}

pub fn command_result(reply: &Value) -> Result<(), String> {
    let list = reply.as_array().ok_or("unexpected reply from sway")?;
    for r in list {
        if r["success"].as_bool() != Some(true) {
            return Err(r["error"].as_str().unwrap_or("sway refused the command").to_string());
        }
    }
    Ok(())
}

/// A window in the workspace, from sway's tree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    /// sway's container id (`[con_id=…]`).
    pub id: i64,
    pub app_id: Option<String>,
    pub title: String,
    pub pid: Option<u32>,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub focused: bool,
    pub floating: bool,
}

/// Every window in a `get_tree` reply, in tree order.
pub fn windows(tree: &Value) -> Vec<Window> {
    let mut out = Vec::new();
    walk(tree, false, &mut out);
    out
}

fn walk(node: &Value, floating: bool, out: &mut Vec<Window>) {
    let is_window = node["pid"].is_u64() || node["app_id"].is_string() || node["window_properties"].is_object();
    if is_window && matches!(node["type"].as_str(), Some("con" | "floating_con")) {
        let r = &node["rect"];
        let int = |v: &Value| v.as_i64().unwrap_or(0) as i32;
        let app_id =
            node["app_id"].as_str().map(String::from).or_else(|| node["window_properties"]["class"].as_str().map(String::from));
        out.push(Window {
            id: node["id"].as_i64().unwrap_or(0),
            app_id,
            title: node["name"].as_str().unwrap_or_default().to_string(),
            pid: node["pid"].as_u64().map(|p| p as u32),
            x: int(&r["x"]),
            y: int(&r["y"]),
            width: int(&r["width"]),
            height: int(&r["height"]),
            focused: node["focused"].as_bool().unwrap_or(false),
            floating: floating || node["type"] == "floating_con",
        });
    }
    for child in node["nodes"].as_array().into_iter().flatten() {
        walk(child, floating, out);
    }
    for child in node["floating_nodes"].as_array().into_iter().flatten() {
        walk(child, true, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn messages_round_trip() {
        let m = encode(GET_TREE, b"");
        assert_eq!(&m[..6], b"i3-ipc");
        let (len, ty) = decode_header(&m[..14].try_into().unwrap()).unwrap();
        assert_eq!((len, ty), (0, GET_TREE));
        let m = encode(RUN_COMMAND, b"exec foot");
        let (len, ty) = decode_header(&m[..14].try_into().unwrap()).unwrap();
        assert_eq!((len, ty, &m[14..]), (9, RUN_COMMAND, &b"exec foot"[..]));
    }

    #[test]
    fn bad_headers_are_refused() {
        let mut h = [0u8; 14];
        h[..6].copy_from_slice(b"i3-ipX");
        assert!(decode_header(&h).is_err());
        let mut h: [u8; 14] = encode(0, b"")[..14].try_into().unwrap();
        h[6..10].copy_from_slice(&u32::MAX.to_ne_bytes());
        assert!(decode_header(&h).is_err());
    }

    #[test]
    fn command_results_report_the_first_error() {
        assert!(command_result(&json!([{"success": true}])).is_ok());
        let e = command_result(&json!([{"success": true}, {"success": false, "error": "Unknown command"}]));
        assert_eq!(e.unwrap_err(), "Unknown command");
        assert!(command_result(&json!({})).is_err());
    }

    #[test]
    fn windows_come_from_the_tree() {
        let tree = json!({
            "id": 1, "type": "root", "nodes": [{
                "id": 2, "type": "output", "name": "HEADLESS-1", "nodes": [{
                    "id": 3, "type": "workspace", "name": "1",
                    "nodes": [
                        {"id": 7, "type": "con", "name": "foot", "app_id": "foot", "pid": 42, "focused": true,
                         "rect": {"x": 0, "y": 24, "width": 1280, "height": 776}, "nodes": []}
                    ],
                    "floating_nodes": [
                        {"id": 9, "type": "floating_con", "name": "Open File", "app_id": "org.gtk.Dialog", "pid": 43,
                         "rect": {"x": 300, "y": 200, "width": 600, "height": 400}, "nodes": []}
                    ]
                }]
            }]
        });
        let w = windows(&tree);
        assert_eq!(w.len(), 2);
        assert_eq!((w[0].id, w[0].pid, w[0].focused, w[0].floating), (7, Some(42), true, false));
        assert_eq!(w[0].app_id.as_deref(), Some("foot"));
        assert_eq!((w[1].id, w[1].floating, w[1].width), (9, true, 600));
    }
}
