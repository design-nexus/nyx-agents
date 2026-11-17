//! Read-only AI Lens companion. Private stdio; no new daemon, credentials or API writes.
use nyx_agents_proto::{Call, Incoming, PROTOCOL_VERSION, Request};
use serde_json::{Value, json};
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    time::Duration,
};
const LIMIT: u64 = 16 * 1024 * 1024;
fn line(r: &mut BufReader<UnixStream>) -> Result<Value, String> {
    let mut b = Vec::new();
    r.take(LIMIT + 1).read_until(b'\n', &mut b).map_err(|e| e.to_string())?;
    if b.is_empty() || b.len() as u64 > LIMIT {
        return Err("Invalid or oversized telemetry response".into());
    }
    serde_json::from_slice(&b).map_err(|e| e.to_string())
}
fn socket(p: PathBuf) -> Result<(UnixStream, BufReader<UnixStream>), String> {
    let s = UnixStream::connect(p).map_err(|_| "Service unavailable".to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(4))).map_err(|e| e.to_string())?;
    s.set_write_timeout(Some(Duration::from_secs(4))).map_err(|e| e.to_string())?;
    Ok((s.try_clone().map_err(|e| e.to_string())?, BufReader::new(s)))
}
fn runtime() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("nyx-agents-{}", unsafe { libc::getuid() })))
}
fn manager(refresh: bool) -> Result<Value, String> {
    let path = std::env::var_os("NYX_AGENTS_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| runtime().join("nyx-agents"))
        .join("agents.sock");
    let (mut w, mut r) = socket(path)?;
    let calls = [
        Call::Hello { client: "nyx-ai-lens".into(), version: PROTOCOL_VERSION },
        Call::ListHarnesses { refresh: false },
        Call::ListSessions,
        Call::ListUsage { refresh },
    ];
    let mut out = vec![];
    for (i, call) in calls.into_iter().enumerate() {
        // Enabled provider checks can take up to five seconds each. Keep the
        // ordinary control timeout short, but allow their bounded batch to finish.
        r.get_ref()
            .set_read_timeout(Some(Duration::from_secs(if matches!(&call, Call::ListUsage { .. }) { 40 } else { 4 })))
            .map_err(|e| e.to_string())?;
        writeln!(w, "{}", serde_json::to_string(&Request { id: i as u64 + 1, call }).unwrap()).map_err(|e| e.to_string())?;
        loop {
            let v: Incoming = serde_json::from_value(line(&mut r)?).map_err(|e| e.to_string())?;
            if let Incoming::Response(res) = v {
                if res.id == i as u64 + 1 {
                    if let Some(e) = res.error {
                        return Err(e.message);
                    }
                    out.push(res.result.unwrap_or(Value::Null));
                    break;
                }
            }
        }
    }
    Ok(json!({"harnesses":out[1],"sessions":out[2],"usage":out[3]}))
}
fn local() -> Result<Value, String> {
    let p = std::env::var_os("NYX_AI_SOCKET").map(PathBuf::from).unwrap_or_else(|| runtime().join("nyx-ai/ai.sock"));
    let (mut w, mut r) = socket(p)?;
    writeln!(w, "{}", json!({"hello":{"api_version":1,"client":"nyx-ai-lens"}})).map_err(|e| e.to_string())?;
    if line(&mut r)?.get("hello").is_none() {
        return Err("Unsupported AI Core protocol".into());
    }
    let mut out = serde_json::Map::new();
    for (i, op) in ["resource_status", "worker_status", "list_models", "service_settings"].iter().enumerate() {
        writeln!(w, "{}", json!({"request":{"id":i+1,"op":{"type":op}}})).map_err(|e| e.to_string())?;
        loop {
            let v = line(&mut r)?;
            if v.get("error").is_some() {
                return Err("AI Core status request failed".into());
            }
            if v["result"]["id"] == json!(i + 1) {
                out.insert(op.to_string(), v["result"]["result"].clone());
                break;
            }
        }
    }
    Ok(Value::Object(out))
}
fn snapshot(refresh: bool) -> Value {
    let (a, b) = std::thread::scope(|s| {
        let h = s.spawn(|| manager(refresh));
        let l = local();
        (h.join().unwrap_or_else(|_| Err("Agent telemetry failed".into())), l)
    });
    json!({"manager":a.as_ref().ok(),"manager_error":a.err(),"local":b.as_ref().ok(),"local_error":b.err(),"checked":nyx_agents_proto::now_ms()})
}
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--snapshot") {
        println!("{}", snapshot(false));
        return;
    }
    if args.iter().any(|a| a == "--stdio") {
        let stdin = io::stdin();
        let mut input = stdin.lock();
        loop {
            let mut data = Vec::new();
            if input.by_ref().take(65537).read_until(b'\n', &mut data).unwrap_or(0) == 0 {
                break;
            }
            if data.len() > 65536 {
                break;
            }
            let Ok(v) = serde_json::from_slice::<Value>(&data) else { continue };
            if !matches!(v["op"].as_str(), Some("snapshot" | "refresh")) {
                println!("{}", json!({"id":v["id"],"error":"Unsupported read-only operation"}));
                continue;
            }
            println!("{}", json!({"id":v["id"],"result":snapshot(v["op"]=="refresh")}));
            let _ = io::stdout().flush();
        }
        return;
    }
    let dirs = data_dirs();
    if toggle_existing(&dirs) {
        return;
    }
    let Some(standalone) = find(&dirs, "nyx-ai-lens/qml/Standalone.qml") else {
        eprintln!("nyx-ai-lens: its QML (nyx-ai-lens/qml) isn't installed");
        std::process::exit(1)
    };
    let status = std::process::Command::new("qs").args(["-p"]).arg(standalone).status();
    if !status.is_ok_and(|s| s.success()) {
        std::process::exit(1)
    }
}

/// Where its QML and Nyx Shell's can be: the user's data folder (install-ai-lens.sh),
/// the package's own `share` beside this program (NyxOS), then the system's data folders.
fn data_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")),
    ];
    if let Some(share) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.canonicalize().ok())
        .and_then(|exe| exe.parent().and_then(|bin| bin.parent()).map(|prefix| prefix.join("share")))
    {
        dirs.push(share);
    }
    let system = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    dirs.extend(system.split(':').filter(|d| !d.is_empty()).map(PathBuf::from));
    dirs
}

/// The first copy of `relative` in `dirs`, with symbolic links resolved (Quickshell names an
/// instance by its configuration's real path).
fn find(dirs: &[PathBuf], relative: &str) -> Option<PathBuf> {
    dirs.iter().map(|d| d.join(relative)).find(|p| p.exists()).map(|p| p.canonicalize().unwrap_or(p))
}

fn toggle_existing(dirs: &[PathBuf]) -> bool {
    let quiet = |program: &str, args: Vec<String>| {
        std::process::Command::new(program)
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    if std::env::var_os("OMARCHY_PATH").is_some()
        && quiet("omarchy-shell", vec!["shell".into(), "toggle".into(), "design-nexus.ai-lens".into(), "{}".into()])
    {
        return true;
    }
    for qml in ["nyx-shell/qml/shell.qml", "nyx-ai-lens/qml/Standalone.qml"].into_iter().filter_map(|q| find(dirs, q)) {
        if quiet(
            "qs",
            vec![
                "-p".into(),
                qml.to_string_lossy().into_owned(),
                "ipc".into(),
                "call".into(),
                "nyx-ai-lens".into(),
                "toggle".into(),
            ],
        ) {
            return true;
        }
    }
    false
}
