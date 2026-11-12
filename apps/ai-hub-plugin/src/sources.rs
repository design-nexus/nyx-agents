//! Where the hub's data comes from: nyx-agentd (sessions, usage, past sessions)
//! over its control socket, and Nyx AI Core (models, memory) over its own.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use nyx_agents_client::Client;
use nyx_agents_proto::{Call, Event, Harness, PastSession, Session, Usage};
use serde_json::{Value, json};

pub enum Data {
    /// Connected (again): everything the manager knows.
    Snapshot { sessions: Vec<Session>, usage: Vec<Usage>, history: Vec<PastSession>, harnesses: Vec<Harness> },
    Event(Event),
    History(Vec<PastSession>),
    Usage(Vec<Usage>),
    Disconnected,
    Local(Option<Local>),
    Peek { harness: String, id: String, text: Result<String, String> },
    /// An action failed; say so.
    Failed(String),
    /// An action finished; refresh what it may have changed.
    Done,
    /// Every five seconds: times move on, and AI Core is asked again.
    Tick,
}

const HISTORY_LIMIT: usize = 300;

/// Follow the manager: a snapshot on connect, then its events. Connecting starts it
/// (socket activation); after it goes away, try again every few seconds.
pub fn follow(send: impl Fn(Data) + Send + Clone + 'static) {
    std::thread::spawn(move || {
        loop {
            if let Ok(mut c) = Client::connect("nyx-ai-hub") {
                let snap = (|| -> Option<Data> {
                    Some(Data::Snapshot {
                        sessions: c.sessions().ok()?,
                        usage: c.call(Call::ListUsage { refresh: false }).ok()?,
                        history: c.call(Call::ListHistory { harness: None, query: None, limit: Some(HISTORY_LIMIT) }).ok()?,
                        harnesses: c.harnesses(false).ok()?,
                    })
                })();
                if let Some(s) = snap {
                    send(s);
                    if c.subscribe().is_ok() {
                        while let Ok(e) = c.next_event() {
                            let usage = matches!(e, Event::Usage { .. });
                            send(Data::Event(e));
                            // Past sessions change as the records do.
                            if usage {
                                history(send.clone());
                            }
                        }
                    }
                }
                send(Data::Disconnected);
            }
            std::thread::sleep(Duration::from_secs(5));
        }
    });
}

pub fn history(send: impl Fn(Data) + Send + 'static) {
    std::thread::spawn(move || {
        let list = Client::connect("nyx-ai-hub")
            .and_then(|mut c| c.call::<Vec<PastSession>>(Call::ListHistory { harness: None, query: None, limit: Some(HISTORY_LIMIT) }));
        if let Ok(l) = list {
            send(Data::History(l));
        }
    });
}

/// Run one call on its own connection, off the plugin's thread.
pub fn call<T: serde::de::DeserializeOwned + Send + 'static>(
    call: Call,
    send: impl Fn(Data) + Send + 'static,
    then: impl FnOnce(T) -> Option<Data> + Send + 'static,
) {
    std::thread::spawn(move || match Client::connect("nyx-ai-hub").and_then(|mut c| c.call::<T>(call)) {
        Ok(v) => send(then(v).unwrap_or(Data::Done)),
        Err(e) => send(Data::Failed(e.to_string())),
    });
}

// ---------- Nyx AI Core ----------

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub state: String,
    pub size: u64,
    pub capabilities: Vec<String>,
    pub update: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Local {
    pub worker: String,
    pub backends: Vec<String>,
    pub loaded: usize,
    pub queued: u64,
    pub vram_total: u64,
    pub ram_total: u64,
    pub ram_available: u64,
    pub models: Vec<Model>,
    /// The OpenAI-compatible endpoint's address, when it's turned on.
    pub endpoint: Option<String>,
}

fn ai_socket() -> PathBuf {
    std::env::var_os("NYX_AI_SOCKET").map(PathBuf::from).unwrap_or_else(|| {
        let run = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"));
        run.join("nyx-ai/ai.sock")
    })
}

/// One request to AI Core on a short-lived connection: its result, or its error.
fn ai(ops: &[Value]) -> Result<Vec<Value>, String> {
    let s = UnixStream::connect(ai_socket()).map_err(|_| "Nyx AI Core isn't running".to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(20))).ok();
    let mut w = s.try_clone().map_err(|e| e.to_string())?;
    let mut r = BufReader::new(s);
    let mut line = String::new();
    writeln!(w, "{}", json!({ "hello": { "api_version": 1, "client": "nyx-ai-hub" } })).map_err(|e| e.to_string())?;
    r.read_line(&mut line).map_err(|e| e.to_string())?;
    if !line.contains("\"hello\"") {
        return Err("Nyx AI Core speaks another version".into());
    }
    for (i, op) in ops.iter().enumerate() {
        writeln!(w, "{}", json!({ "request": { "id": i + 1, "op": op } })).map_err(|e| e.to_string())?;
    }
    let mut out = vec![Value::Null; ops.len()];
    let mut left = ops.len();
    while left > 0 {
        line.clear();
        if r.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            return Err("Nyx AI Core closed the connection".into());
        }
        let v: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
        if let Some(res) = v.get("result") {
            let i = res["id"].as_u64().unwrap_or(0) as usize;
            if (1..=ops.len()).contains(&i) {
                out[i - 1] = res["result"].clone();
                left -= 1;
            }
        } else if let Some(err) = v.get("error") {
            return Err(err["error"]["message"].as_str().unwrap_or("Nyx AI Core refused").to_string());
        }
    }
    Ok(out)
}

fn friendly(id: &str) -> String {
    // org/Repo-GGUF:QUANT → Repo · QUANT
    let (repo, quant) = id.split_once(':').unwrap_or((id, ""));
    let name = repo.rsplit('/').next().unwrap_or(repo).trim_end_matches("-GGUF").trim_end_matches("-gguf");
    if quant.is_empty() { name.to_string() } else { format!("{name} · {quant}") }
}

pub fn local(send: impl Fn(Data) + Send + 'static) {
    std::thread::spawn(move || {
        let r = ai(&[json!({ "type": "resource_status" }), json!({ "type": "worker_status" }), json!({ "type": "list_models" }), json!({ "type": "service_settings" })]);
        let l = r.ok().map(|v| {
            let (res, worker, models) = (&v[0], &v[1], &v[2]["models"]);
            let mut models: Vec<Model> = models
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|m| Model {
                            id: m["id"].as_str().unwrap_or_default().to_string(),
                            name: friendly(m["id"].as_str().unwrap_or_default()),
                            state: m["state"].as_str().unwrap_or("installed").to_string(),
                            size: m["size"].as_u64().unwrap_or(0),
                            capabilities: m["capabilities"].as_array().map(|c| c.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
                            update: m["update_available"] == true,
                        })
                        .collect()
                })
                .unwrap_or_default();
            let rank = |s: &str| match s {
                "busy" | "ready" | "loading" => 0,
                "downloading" | "validating" => 1,
                _ => 2,
            };
            models.sort_by(|a, b| rank(&a.state).cmp(&rank(&b.state)).then(a.name.cmp(&b.name)));
            Local {
                worker: worker["state"].as_str().unwrap_or("unknown").to_string(),
                backends: res["gpu_backends"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
                loaded: res["loaded"].as_array().map_or(0, Vec::len),
                queued: res["queued"].as_u64().unwrap_or(0),
                vram_total: res["vram_total"].as_u64().unwrap_or(0),
                ram_total: res["ram_total"].as_u64().unwrap_or(0),
                ram_available: res["ram_available"].as_u64().unwrap_or(0),
                models,
                endpoint: v[3]["openai_endpoint"].as_str().map(String::from),
            }
        });
        send(Data::Local(l));
    });
}

/// Load or unload a model, then report the new state.
pub fn model(load: bool, id: String, send: impl Fn(Data) + Send + Clone + 'static) {
    std::thread::spawn(move || {
        let op = if load { json!({ "type": "load_model", "model": id, "keep": false }) } else { json!({ "type": "unload_model", "model": id }) };
        if let Err(e) = ai(&[op]) {
            send(Data::Failed(e));
        }
        local(send);
    });
}
