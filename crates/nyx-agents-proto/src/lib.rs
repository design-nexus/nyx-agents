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

/// A desktop of its own where agents open and drive GUI programs: a headless
/// sway run by the manager (see docs/WORKSPACES.md). Never a security
/// boundary: programs in it run as the user.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentWorkspace {
    pub id: WorkspaceId,
    pub name: String,
    pub sessions: Vec<SessionId>,
    /// The compositor behind it: `sway:<absolute Wayland socket path>`.
    /// Private to the user.
    pub compositor: Option<String>,
    #[serde(default)]
    pub state: WorkspaceState,
    /// Output size in pixels, and its scale.
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    #[serde(default = "one")]
    pub scale: f64,
    #[serde(default)]
    pub created: Millis,
    /// Windows open in it, and the focused one's title (sanitised).
    #[serde(default)]
    pub windows: usize,
    #[serde(default)]
    pub focused: Option<String>,
}

fn one() -> f64 {
    1.0
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspaceState {
    #[default]
    Running,
    /// Its compositor ended; the record goes once its sessions are gone.
    Ended,
}

impl AgentWorkspace {
    /// The Wayland socket of a sway-backed workspace.
    pub fn wayland_socket(&self) -> Option<&str> {
        self.compositor.as_deref()?.strip_prefix("sway:")
    }
}

/// A window inside an agent workspace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceWindow {
    /// The compositor's id for it (sway's container id).
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

/// One step of pointer or keyboard input in an agent workspace. Coordinates
/// are logical pixels from the top left, as in a screenshot at scale 1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum InputAction {
    Move {
        x: i32,
        y: i32,
    },
    /// Click at the pointer, or at `x`, `y`. `button`: left (default), right, middle.
    Click {
        #[serde(default)]
        x: Option<i32>,
        #[serde(default)]
        y: Option<i32>,
        #[serde(default)]
        button: Option<String>,
        /// 1 to 3 (a double click is 2).
        #[serde(default)]
        count: Option<u32>,
    },
    /// Press or release one button, for drags driven step by step.
    Button {
        #[serde(default)]
        button: Option<String>,
        pressed: bool,
    },
    Drag {
        from_x: i32,
        from_y: i32,
        to_x: i32,
        to_y: i32,
        #[serde(default)]
        button: Option<String>,
    },
    /// Wheel clicks: positive `dy` scrolls down, positive `dx` right.
    Scroll {
        #[serde(default)]
        x: Option<i32>,
        #[serde(default)]
        y: Option<i32>,
        #[serde(default)]
        dx: i32,
        #[serde(default)]
        dy: i32,
    },
    Type {
        text: String,
    },
    /// A combo such as `ctrl+l`, `Return` or `alt+F4`.
    Key {
        combo: String,
    },
    /// Pause, up to 5 seconds.
    Wait {
        ms: u64,
    },
}

/// A capture of an agent workspace.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Screenshot {
    pub width: u32,
    pub height: u32,
    /// Base64 PNG.
    pub png: String,
}

// ---------- Usage ----------

/// One agent's usage, read from its own records, plus its provider's quota when
/// the user turned quota checks on for it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub harness: String,
    pub name: String,
    /// Any records were found.
    pub has_records: bool,
    /// One of its sessions is running.
    pub working: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// When it was last used (Unix seconds), from its newest record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<u64>,
    pub today: UsageToday,
    /// Over every record read (Codex keeps 30 days).
    pub totals: UsageTotals,
    /// The last seven days, oldest first, ending today.
    pub days: Vec<UsageDay>,
    /// Most used first.
    pub models: Vec<ModelUsage>,
    /// Most used first, at most 12.
    pub tools: Vec<ToolUsage>,
    /// Newest first, at most 10.
    pub sessions: Vec<UsageSession>,
    pub quota: Quota,
    /// When this was scanned (Unix seconds).
    pub updated: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageToday {
    pub prompts: u64,
    pub sessions: u64,
    pub steps: u64,
    /// Input, output and cache writes; cache reads are `cached`.
    pub tokens: u64,
    pub cached: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageTotals {
    pub prompts: u64,
    pub sessions: u64,
    pub steps: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageDay {
    /// `YYYY-MM-DD`, local time.
    pub date: String,
    pub prompts: u64,
    pub steps: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelUsage {
    pub name: String,
    pub prompts: u64,
    pub steps: u64,
    pub today_prompts: u64,
    pub today_steps: u64,
    pub week_prompts: u64,
    pub week_steps: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolUsage {
    pub name: String,
    pub calls: u64,
}

/// A recent session as the agent's records describe it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageSession {
    /// The harness's own session id.
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub cwd: String,
    /// Unix seconds.
    pub updated: u64,
    pub prompts: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub tokens: u64,
    /// A running process of the harness has it open.
    pub active: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Quota {
    /// The agent's provider reports a quota that can be checked.
    #[serde(default)]
    pub available: bool,
    /// The user turned quota checks on for this agent (`[usage] quotas`).
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    pub windows: Vec<QuotaWindow>,
    /// Why the last check failed; `windows` are then the last good ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// When `windows` were fetched (Unix seconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<u64>,
}

/// One rate-limit window: the 5-hour window, the week, a model's week.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct QuotaWindow {
    pub id: String,
    pub label: String,
    /// 0–100.
    pub used_percent: f64,
    /// Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<u64>,
    /// Percent used per hour over the last six hours since it last refilled.
    pub burn_per_hour: f64,
    pub pace: Pace,
    /// At this pace, the percent left when it resets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_at_reset: Option<f64>,
    /// At this pace, seconds until it runs out, when that is before the reset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub empty_in: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Pace {
    /// Not being used up (or no reset time).
    #[default]
    Steady,
    OnPace,
    /// Under 15% left at reset.
    Tight,
    /// Runs out before it resets.
    RunsOut,
    /// The window has reset since these numbers were fetched.
    Stale,
}

/// A session in an agent's own records, whether or not Nyx Agents started it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PastSession {
    pub harness: String,
    /// The harness's own session id.
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub cwd: String,
    /// Unix seconds.
    pub updated: u64,
    pub prompts: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// A running process of the harness has it open.
    pub active: bool,
    /// The Nyx Agents session that runs or ran it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed: Option<SessionId>,
    /// It can be resumed (its harness supports that).
    pub resumable: bool,
    /// Its records can be moved to the Trash.
    pub can_trash: bool,
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
    /// Remove an ended session's worktree, if Nyx Agents made it and it is clean.
    #[serde(rename = "worktrees.remove")]
    RemoveWorktree {
        id: SessionId,
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
    #[serde(rename = "workspaces.get")]
    GetWorkspace {
        id: WorkspaceId,
    },
    /// Start a new agent workspace (default 1280×800 at scale 1).
    #[serde(rename = "workspaces.create")]
    CreateWorkspace {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        width: Option<u32>,
        #[serde(default)]
        height: Option<u32>,
        #[serde(default)]
        scale: Option<f64>,
    },
    /// End a workspace and everything running in it.
    #[serde(rename = "workspaces.remove")]
    RemoveWorkspace {
        id: WorkspaceId,
    },
    #[serde(rename = "workspaces.resize")]
    ResizeWorkspace {
        id: WorkspaceId,
        width: u32,
        height: u32,
        #[serde(default)]
        scale: Option<f64>,
    },
    /// Start a program in a workspace from an argv (never a shell string).
    #[serde(rename = "workspaces.launch")]
    LaunchInWorkspace {
        id: WorkspaceId,
        argv: Vec<String>,
        #[serde(default)]
        cwd: Option<String>,
    },
    #[serde(rename = "workspaces.windows")]
    WorkspaceWindows {
        id: WorkspaceId,
    },
    #[serde(rename = "workspaces.focus_window")]
    FocusWindow {
        id: WorkspaceId,
        window: i64,
    },
    /// A PNG of the workspace at logical size, shrunk to fit the maximums.
    #[serde(rename = "workspaces.screenshot")]
    CaptureWorkspace {
        id: WorkspaceId,
        #[serde(default)]
        max_width: Option<u32>,
        #[serde(default)]
        max_height: Option<u32>,
    },
    #[serde(rename = "workspaces.input")]
    WorkspaceInput {
        id: WorkspaceId,
        actions: Vec<InputAction>,
    },
    /// The workspace's clipboard: `{text}`, or `{png}` (base64) with `image`.
    #[serde(rename = "workspaces.clipboard_get")]
    WorkspaceClipboardGet {
        id: WorkspaceId,
        #[serde(default)]
        image: bool,
    },
    /// Put text, or a base64 PNG, on the workspace's clipboard.
    #[serde(rename = "workspaces.clipboard_set")]
    WorkspaceClipboardSet {
        id: WorkspaceId,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        png: Option<String>,
    },
    /// Every agent's usage that has records or a quota, last used first.
    /// `refresh` rescans now (and fetches quotas, ignoring the retry wait).
    #[serde(rename = "usage.list")]
    ListUsage {
        #[serde(default)]
        refresh: bool,
    },
    #[serde(rename = "usage.get")]
    GetUsage {
        harness: String,
        #[serde(default)]
        refresh: bool,
    },
    /// Past sessions from the agents' own records, newest first. `query`
    /// matches titles and folders, ignoring case.
    #[serde(rename = "history.list")]
    ListHistory {
        #[serde(default)]
        harness: Option<String>,
        #[serde(default)]
        query: Option<String>,
        #[serde(default)]
        limit: Option<usize>,
    },
    /// Continue a past session in Nyx Agents: its own session when Nyx Agents
    /// runs or ran it, else a new one resuming it.
    #[serde(rename = "history.resume")]
    ResumePast {
        harness: String,
        id: String,
    },
    /// The command that continues a past session in the user's terminal:
    /// `{argv, cwd}`, for the caller to run (never through a shell).
    #[serde(rename = "history.terminal")]
    ResumeInTerminal {
        harness: String,
        id: String,
    },
    /// The transcript as plain text (its last 64 KiB).
    #[serde(rename = "history.peek")]
    Peek {
        harness: String,
        id: String,
    },
    /// Move a past session's records to the Trash. Not while it runs.
    #[serde(rename = "history.trash")]
    TrashPast {
        harness: String,
        id: String,
    },
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
    /// An agent workspace to work in, or `"new"` to start one. The agent
    /// gets the `nyx-agents computer` tools for it.
    #[serde(default)]
    pub workspace: Option<WorkspaceId>,
    /// Start in a new git worktree of the folder's repository, on a new
    /// branch `agents/<name>` (empty: a name is chosen).
    #[serde(default)]
    pub worktree: Option<String>,
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
    Session {
        session: Box<Session>,
    },
    Removed {
        id: SessionId,
    },
    Harnesses {
        harnesses: Vec<Harness>,
    },
    /// A workspace was created or changed.
    Workspace {
        workspace: Box<AgentWorkspace>,
    },
    WorkspaceRemoved {
        id: WorkspaceId,
    },
    /// An agent's usage changed.
    Usage {
        usage: Box<Usage>,
    },
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
    /// Each agent with usage, last used first (`usage.list` has the details).
    #[serde(default)]
    pub usage: Vec<HubUsage>,
}

/// An agent's usage in brief: enough for a bar item or a small tile.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HubUsage {
    pub harness: String,
    pub name: String,
    pub working: bool,
    pub last_used: Option<u64>,
    pub today_prompts: u64,
    pub today_tokens: u64,
    /// The quota window closest to running out, when quota checks are on.
    pub quota: Option<QuotaWindow>,
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
    fn workspace_calls_round_trip() {
        let r: Request = serde_json::from_str(
            r#"{"id":2,"method":"workspaces.input","params":{"id":"w1","actions":[
                {"action":"click","x":10,"y":20},{"action":"type","text":"hi"},{"action":"key","combo":"ctrl+l"}]}}"#,
        )
        .unwrap();
        let Call::WorkspaceInput { id, actions } = r.call else { panic!("wrong call") };
        assert_eq!(id, "w1");
        assert_eq!(actions[0], InputAction::Click { x: Some(10), y: Some(20), button: None, count: None });
        assert_eq!(actions[2], InputAction::Key { combo: "ctrl+l".into() });
        let r: Request = serde_json::from_str(r#"{"id":3,"method":"workspaces.create","params":{}}"#).unwrap();
        assert!(matches!(r.call, Call::CreateWorkspace { width: None, .. }));
        let s = serde_json::to_string(&Request {
            id: 4,
            call: Call::LaunchInWorkspace { id: "w1".into(), argv: vec!["foot".into()], cwd: None },
        })
        .unwrap();
        assert!(s.contains(r#""method":"workspaces.launch""#), "{s}");
    }

    #[test]
    fn workspaces_name_their_socket() {
        let w: AgentWorkspace =
            serde_json::from_str(r#"{"id":"w","name":"W","sessions":[],"compositor":"sway:/run/x/wayland-0"}"#).unwrap();
        assert_eq!(w.wayland_socket(), Some("/run/x/wayland-0"));
        assert_eq!((w.state, w.scale), (WorkspaceState::Running, 1.0));
        let e: Incoming = serde_json::from_str(r#"{"event":"workspace-removed","id":"w"}"#).unwrap();
        assert!(matches!(e, Incoming::Event(Event::WorkspaceRemoved { .. })));
    }

    #[test]
    fn incoming_tells_events_from_responses() {
        let e: Incoming = serde_json::from_str(r#"{"event":"removed","id":"x"}"#).unwrap();
        assert!(matches!(e, Incoming::Event(Event::Removed { .. })));
        let r: Incoming = serde_json::from_str(r#"{"id":3,"result":[]}"#).unwrap();
        assert!(matches!(r, Incoming::Response(_)));
    }
}
