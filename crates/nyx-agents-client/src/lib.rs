//! Client for the Nyx Agent Manager.
//!
//! [`Client`] speaks the control protocol; [`Attachment`] carries one
//! session's terminal. Both are blocking; GUI code runs them on a thread.

use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

use nyx_agents_core::paths;
pub use nyx_agents_proto::FrameKind;
use nyx_agents_proto::{
    Attach, AttachMode, Attached, Call, Event, Harness, Incoming, Launch, PROTOCOL_VERSION, Request, ResizeMsg, RpcError,
    Session, read_frame, write_frame,
};
use serde::de::DeserializeOwned;
use serde_json::Value;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Rpc(RpcError),
    Protocol(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) if e.kind() == io::ErrorKind::NotFound || e.kind() == io::ErrorKind::ConnectionRefused => {
                write!(f, "Nyx Agent Manager is not running ({e})")
            }
            Error::Io(e) => write!(f, "{e}"),
            Error::Rpc(e) => write!(f, "{}", e.message),
            Error::Protocol(m) => write!(f, "protocol error: {m}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub struct Client {
    w: UnixStream,
    r: BufReader<UnixStream>,
    next: u64,
    events: VecDeque<Event>,
}

impl Client {
    pub fn connect(name: &str) -> Result<Self> {
        let s = UnixStream::connect(paths::control_socket())?;
        let mut c = Self { r: BufReader::new(s.try_clone()?), w: s, next: 1, events: VecDeque::new() };
        c.call::<Value>(Call::Hello { client: name.into(), version: PROTOCOL_VERSION })?;
        Ok(c)
    }

    pub fn call<T: DeserializeOwned>(&mut self, call: Call) -> Result<T> {
        let id = self.next;
        self.next += 1;
        writeln!(self.w, "{}", serde_json::to_string(&Request { id, call }).unwrap())?;
        loop {
            let mut line = String::new();
            if self.r.read_line(&mut line)? == 0 {
                return Err(Error::Io(io::ErrorKind::UnexpectedEof.into()));
            }
            match serde_json::from_str::<Incoming>(&line) {
                Ok(Incoming::Response(r)) if r.id == id => {
                    if let Some(e) = r.error {
                        return Err(Error::Rpc(e));
                    }
                    return serde_json::from_value(r.result.unwrap_or(Value::Null)).map_err(|e| Error::Protocol(e.to_string()));
                }
                Ok(Incoming::Event(e)) => self.events.push_back(e),
                Ok(Incoming::Response(_)) => {}
                Err(e) => return Err(Error::Protocol(e.to_string())),
            }
        }
    }

    pub fn harnesses(&mut self, refresh: bool) -> Result<Vec<Harness>> {
        self.call(Call::ListHarnesses { refresh })
    }

    pub fn sessions(&mut self) -> Result<Vec<Session>> {
        self.call(Call::ListSessions)
    }

    pub fn session(&mut self, id: &str) -> Result<Session> {
        self.call(Call::GetSession { id: id.into() })
    }

    pub fn launch(&mut self, l: Launch) -> Result<Session> {
        self.call(Call::Launch(l))
    }

    pub fn resume(&mut self, id: &str) -> Result<Session> {
        self.call(Call::Resume { id: id.into() })
    }

    pub fn stop(&mut self, id: &str, force: bool) -> Result<Value> {
        self.call(Call::Stop { id: id.into(), force })
    }

    pub fn forget(&mut self, id: &str) -> Result<Value> {
        self.call(Call::Forget { id: id.into() })
    }

    /// Start receiving events; read them with [`Client::next_event`].
    pub fn subscribe(&mut self) -> Result<()> {
        self.call::<Value>(Call::Subscribe).map(drop)
    }

    pub fn next_event(&mut self) -> Result<Event> {
        if let Some(e) = self.events.pop_front() {
            return Ok(e);
        }
        loop {
            let mut line = String::new();
            if self.r.read_line(&mut line)? == 0 {
                return Err(Error::Io(io::ErrorKind::UnexpectedEof.into()));
            }
            if let Ok(Incoming::Event(e)) = serde_json::from_str::<Incoming>(&line) {
                return Ok(e);
            }
        }
    }

    /// Resolve a unique id prefix against the current sessions.
    pub fn resolve(&mut self, prefix: &str) -> Result<String> {
        let ids: Vec<String> = self.sessions()?.into_iter().map(|s| s.id).filter(|id| id.starts_with(prefix)).collect();
        match ids.as_slice() {
            [one] => Ok(one.clone()),
            [] => Err(Error::Protocol(format!("no session starts with {prefix}"))),
            _ => Err(Error::Protocol(format!("{prefix} matches {} sessions", ids.len()))),
        }
    }
}

/// One session's terminal stream.
pub struct Attachment {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
    pub cols: u16,
    pub rows: u16,
    pub mode: AttachMode,
}

impl Attachment {
    /// `colors` (`#rrggbb` foreground and background) are what the session
    /// answers colour queries with; only a `control` attachment sets them.
    pub fn open(id: &str, mode: AttachMode, colors: Option<(String, String)>) -> Result<Self> {
        let mut stream = UnixStream::connect(paths::session_socket(id))?;
        let (foreground, background) = colors.map(|(f, b)| (Some(f), Some(b))).unwrap_or((None, None));
        writeln!(stream, "{}", serde_json::to_string(&Attach { attach: mode, foreground, background }).unwrap())?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let a: Attached = serde_json::from_str(&line).map_err(|e| Error::Protocol(e.to_string()))?;
        if !a.ok {
            return Err(Error::Protocol(a.error.unwrap_or_else(|| "attach refused".into())));
        }
        Ok(Self { stream, reader, cols: a.cols, rows: a.rows, mode })
    }

    /// The next frame: a snapshot, output, or the exit notice.
    pub fn next_frame(&mut self) -> Result<(Option<FrameKind>, Vec<u8>)> {
        let (k, p) = read_frame(&mut self.reader)?;
        Ok((FrameKind::from_u8(k), p))
    }

    /// A handle for sending input and resizes from another thread.
    pub fn sender(&self) -> Result<Sender> {
        Ok(Sender(self.stream.try_clone()?))
    }
}

pub struct Sender(UnixStream);

impl Sender {
    pub fn input(&mut self, bytes: &[u8]) -> io::Result<()> {
        write_frame(&mut self.0, FrameKind::Input, bytes)
    }

    pub fn resize(&mut self, r: ResizeMsg) -> io::Result<()> {
        write_frame(&mut self.0, FrameKind::Resize, &serde_json::to_vec(&r).unwrap())
    }
}
