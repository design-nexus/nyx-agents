//! The Nyx Agent Manager protocol, version 1.
//!
//! **Control** (`$XDG_RUNTIME_DIR/nyx-agents/agents.sock`): newline-delimited
//! JSON. Clients send [`Request`]s and get one [`Response`] each, matched by
//! `id`. After `events.subscribe` the daemon also sends [`Event`]s.
//!
//! **Terminal** (`$XDG_RUNTIME_DIR/nyx-agents/s/<session>.sock`, one per
//! session, served by the session's holder process): the client sends one
//! JSON [`Attach`] line, the holder answers one JSON [`Attached`] line, then
//! both sides exchange binary [`Frame`]s. An `observe` attachment receives
//! output only; the holder ignores anything it sends.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

pub type SessionId = String;
pub type ProjectId = String;
pub type WorkspaceId = String;

/// Milliseconds since the Unix epoch.
pub type Millis = u64;

pub fn now_ms() -> Millis {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

// ---------- Harnesses ----------

/// How deeply Nyx Agents integrates a harness.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tier {
    /// Launch, resume and structured data handled by a dedicated adapter.
    Native,
    /// Runs interactively in a terminal; nothing harness-specific yet.
    Terminal,
    /// A user-configured command.
    Generic,
}

/// What an adapter can actually do. The UI hides anything not listed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// Start a session with an id chosen up front, so its records are known.
    pub preset_session_id: bool,
    /// Reopen a previous session by its harness id.
    pub resume: bool,
    /// Reports OSC 7501 program status (working, blocked, done...).
    pub program_status: bool,
    /// Subagent activity is readable (and from where).
    pub subagents: Option<SubagentSource>,
    /// Token or cost usage is readable.
    pub usage: bool,
    /// Can run on another machine.
    pub remote: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubagentSource {
    /// Native structured events.
    Events,
    /// The subagent has its own terminal.
    Pty,
    /// Session logs written by the harness (shown as a transcript, not a terminal).
    Transcript,
    /// Only the parent's task summaries.
    TaskSummary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthState {
    /// Credentials appear to be configured (never read, only checked for).
    Ready,
    NotReady,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Harness {
    pub id: String,
    pub name: String,
    pub tier: Tier,
    /// Resolved executable, if installed.
    pub path: Option<String>,
    pub version: Option<String>,
    pub auth: AuthState,
    pub capabilities: Capabilities,
}

// ---------- Sessions ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Lifecycle {
    Starting,
    Running,
    /// The program exited.
    Exited,
    /// The holder is gone (for example after a reboot); history remains.
    Lost,
}

/// What the agent says it is doing (OSC 7501), or what the manager knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Activity {
    Unknown,
    Idle,
    Working,
    Blocked,
    Done,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BlockedOn {
    Permission,
    Question,
    Auth,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub activity: Activity,
    pub blocked_on: Option<BlockedOn>,
    /// 0..=100 when the agent reports progress.
    pub progress: Option<u8>,
    /// One line from the agent, sanitised.
    pub message: Option<String>,
    /// Increments each time the agent stops working (finishes a turn).
    pub turn: u64,
    pub since: Millis,
}

impl Default for Status {
    fn default() -> Self {
        Self { activity: Activity::Unknown, blocked_on: None, progress: None, message: None, turn: 0, since: now_ms() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub harness: String,
    pub title: String,
    pub project: Option<ProjectId>,
    pub workspace: Option<WorkspaceId>,
    /// Working directory the agent started in.
    pub cwd: String,
    /// Git worktree root and branch, when the cwd is in a repository.
    pub worktree: Option<Worktree>,
    /// The harness's own session id (for resume and transcripts).
    pub harness_session: Option<String>,
    pub lifecycle: Lifecycle,
    pub status: Status,
    pub exit_code: Option<i32>,
    pub pid: Option<u32>,
    pub created: Millis,
    pub ended: Option<Millis>,
    /// Terminal grid the program last ran at.
    pub cols: u16,
    pub rows: u16,
    /// The session can be reopened through its harness.
    pub resumable: bool,
    pub subagents: Vec<Subagent>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worktree {
    pub root: String,
    pub branch: Option<String>,
    /// True for a linked worktree rather than the main checkout.
    pub linked: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubagentState {
    Running,
    Waiting,
    Completed,
    Failed,
}

/// A subagent the parent started. Observed only; never controlled.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subagent {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub source: SubagentSource,
    pub state: SubagentState,
    pub started: Millis,
    pub ended: Option<Millis>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    /// Repository root (or the folder, outside git).
    pub path: String,
}

/// A group of sessions shown together. Maps to a Hyprland agent workspace
/// once that API exists; it is never a security boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentWorkspace {
    pub id: WorkspaceId,
    pub name: String,
    pub sessions: Vec<SessionId>,
    /// Opaque handle of the compositor's agent workspace, when linked.
    pub compositor: Option<String>,
}

// ---------- Control requests ----------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    #[serde(flatten)]
    pub call: Call,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "kebab-case")]
pub enum Call {
    Hello {
        client: String,
        version: u32,
    },
    #[serde(rename = "harnesses.list")]
    ListHarnesses {
        refresh: bool,
    },
    #[serde(rename = "sessions.list")]
    ListSessions,
    #[serde(rename = "sessions.get")]
    GetSession {
        id: SessionId,
    },
    #[serde(rename = "sessions.launch")]
    Launch(Launch),
    /// Start a new session continuing an ended one through its harness.
    #[serde(rename = "sessions.resume")]
    Resume {
        id: SessionId,
    },
    /// Ask the agent to end (SIGHUP to its process group; `force` sends SIGKILL).
    #[serde(rename = "sessions.stop")]
    Stop {
        id: SessionId,
        force: bool,
    },
    /// Drop an ended session's record and history.
    #[serde(rename = "sessions.forget")]
    Forget {
        id: SessionId,
    },
    #[serde(rename = "sessions.rename")]
    Rename {
        id: SessionId,
        title: String,
    },
    /// The session the user is looking at in the app (none when the window
    /// is not focused). It is not notified about.
    #[serde(rename = "sessions.viewing")]
    Viewing {
        id: Option<SessionId>,
    },
    #[serde(rename = "projects.list")]
    ListProjects,
    #[serde(rename = "workspaces.list")]
    ListWorkspaces,
    #[serde(rename = "events.subscribe")]
    Subscribe,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Launch {
    pub harness: String,
    pub cwd: String,
    /// Extra arguments after the adapter's own.
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub workspace: Option<WorkspaceId>,
    #[serde(default = "default_cols")]
    pub cols: u16,
    #[serde(default = "default_rows")]
    pub rows: u16,
}

fn default_cols() -> u16 {
    120
}

fn default_rows() -> u16 {
    36
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum Event {
    Session { session: Box<Session> },
    Removed { id: SessionId },
    Harnesses { harnesses: Vec<Harness> },
}

/// A line from the daemon: a response or an event.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Incoming {
    Response(Response),
    Event(Event),
}

// ---------- Terminal attach ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AttachMode {
    /// Drives the session: input and resizes are accepted.
    Control,
    /// Watches only. Nothing it sends reaches the program.
    Observe,
    /// Status and lifecycle events as JSON lines, no terminal data (the daemon).
    Events,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Attach {
    pub attach: AttachMode,
    /// Colours the holder should answer colour queries with: `#rrggbb`.
    #[serde(default)]
    pub foreground: Option<String>,
    #[serde(default)]
    pub background: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Attached {
    pub ok: bool,
    pub cols: u16,
    pub rows: u16,
    #[serde(default)]
    pub error: Option<String>,
}

/// What a holder reports on an `events` attachment, one JSON line each.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum HolderEvent {
    Started { pid: u32 },
    Status { status: Status },
    Title { title: String },
    Cwd { cwd: String },
    Grid { cols: u16, rows: u16 },
    Exited { code: Option<i32>, signal: Option<i32> },
}

/// Binary frames after the attach handshake: `[kind u8][len u32 LE][payload]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameKind {
    /// Holder → client: a libghostty-vt snapshot of the whole terminal.
    /// Sent first, and again in place of a backlog the client fell behind on.
    Snapshot = 1,
    /// Holder → client: program output.
    Output = 2,
    /// Holder → client: JSON `{code, signal}`; the program ended.
    Exited = 3,
    /// Client → holder: bytes for the program.
    Input = 16,
    /// Client → holder: JSON `{cols, rows, width, height}`.
    Resize = 17,
}

impl FrameKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            1 => Self::Snapshot,
            2 => Self::Output,
            3 => Self::Exited,
            16 => Self::Input,
            17 => Self::Resize,
            _ => return None,
        })
    }
}

/// Largest frame either side accepts.
pub const MAX_FRAME: usize = 64 << 20;

pub fn write_frame(w: &mut impl std::io::Write, kind: FrameKind, payload: &[u8]) -> std::io::Result<()> {
    let mut head = [0u8; 5];
    head[0] = kind as u8;
    head[1..].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    w.write_all(&head)?;
    w.write_all(payload)
}

pub fn read_frame(r: &mut impl std::io::Read) -> std::io::Result<(u8, Vec<u8>)> {
    let mut head = [0u8; 5];
    r.read_exact(&mut head)?;
    let len = u32::from_le_bytes(head[1..].try_into().unwrap()) as usize;
    if len > MAX_FRAME {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "frame too large"));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok((head[0], buf))
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ResizeMsg {
    pub cols: u16,
    pub rows: u16,
    pub width: u32,
    pub height: u32,
}

// ---------- AI Hub snapshot ----------

/// `$XDG_RUNTIME_DIR/nyx-agents/hub.json`, rewritten on every change.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Hub {
    pub version: u32,
    pub updated: Millis,
    pub agents: usize,
    pub blocked: usize,
    pub done: usize,
    pub working: usize,
    pub subagents_running: usize,
    pub sessions: Vec<HubSession>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HubSession {
    pub id: SessionId,
    pub harness: String,
    pub title: String,
    pub project: Option<String>,
    pub cwd: String,
    pub branch: Option<String>,
    pub activity: Activity,
    pub blocked_on: Option<BlockedOn>,
    pub turn: u64,
    pub started: Millis,
    pub status_since: Millis,
    pub subagents_running: usize,
    pub subagents_total: usize,
    pub workspace: Option<String>,
    /// Opens the session in Nyx Agents: run as an argv.
    pub open: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip() {
        let r =
            Request { id: 7, call: Call::Launch(Launch { harness: "claude".into(), cwd: "/tmp".into(), ..Default::default() }) };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains(r#""method":"sessions.launch""#), "{s}");
        let back: Request = serde_json::from_str(&s).unwrap();
        assert!(matches!(back.call, Call::Launch(l) if l.harness == "claude"));
        let r: Request = serde_json::from_str(r#"{"id":1,"method":"sessions.list"}"#).unwrap();
        assert!(matches!(r.call, Call::ListSessions));
    }

    #[test]
    fn frames_round_trip() {
        let mut buf = Vec::new();
        write_frame(&mut buf, FrameKind::Output, b"hello").unwrap();
        let (k, p) = read_frame(&mut buf.as_slice()).unwrap();
        assert_eq!((FrameKind::from_u8(k), p.as_slice()), (Some(FrameKind::Output), &b"hello"[..]));
    }

    #[test]
    fn incoming_tells_events_from_responses() {
        let e: Incoming = serde_json::from_str(r#"{"event":"removed","id":"x"}"#).unwrap();
        assert!(matches!(e, Incoming::Event(Event::Removed { .. })));
        let r: Incoming = serde_json::from_str(r#"{"id":3,"result":[]}"#).unwrap();
        assert!(matches!(r, Incoming::Response(_)));
    }
}
