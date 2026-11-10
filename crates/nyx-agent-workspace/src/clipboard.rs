//! The workspace's clipboard through `wlr-data-control`: read what a program
//! in the workspace copied, or put text or an image there for it to paste.
//!
//! A [`Clipboard`] keeps its connection on a thread of its own, because a
//! selection it sets is only pasteable while its owner answers requests.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop, event_created_child};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1 as device, zwlr_data_control_manager_v1 as manager, zwlr_data_control_offer_v1 as offer,
    zwlr_data_control_source_v1 as source,
};

use crate::wl;

/// Mime types that mean text, best first.
pub const TEXT_TYPES: &[&str] = &["text/plain;charset=utf-8", "text/plain", "UTF8_STRING", "TEXT", "STRING"];
/// Largest clipboard content passed either way.
pub const MAX_BYTES: usize = 32 << 20;

/// A clipboard's content with its mime type, if it has any in the types asked for.
type Got = Result<Option<(String, Vec<u8>)>, String>;

enum Op {
    Set(Vec<(String, Vec<u8>)>, Sender<Result<(), String>>),
    Get(Vec<String>, Sender<Got>),
}

#[derive(Default)]
struct State {
    seat: Option<wl_seat::WlSeat>,
    manager: Option<manager::ZwlrDataControlManagerV1>,
    offers: HashMap<u32, (offer::ZwlrDataControlOfferV1, Vec<String>)>,
    selection: Option<offer::ZwlrDataControlOfferV1>,
    /// What this connection is offering, by mime type, while it owns the selection.
    own: Option<(source::ZwlrDataControlSourceV1, HashMap<String, Vec<u8>>)>,
}

/// A handle to the clipboard thread of one workspace.
pub struct Clipboard {
    ops: Sender<Op>,
}

impl Clipboard {
    pub fn connect(socket: &Path) -> Result<Self, String> {
        let conn = wl::connect(socket)?;
        let mut queue: EventQueue<State> = conn.new_event_queue();
        let qh = queue.handle();
        conn.display().get_registry(&qh, ());
        let mut state = State::default();
        queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
        let (Some(seat), Some(m)) = (state.seat.clone(), state.manager.clone()) else {
            return Err("the workspace doesn't offer clipboard access".into());
        };
        let dev = m.get_data_device(&seat, &qh, ());
        queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name("nyx-agent-workspace-clipboard".into())
            .spawn(move || serve(conn, queue, qh, state, dev, rx))
            .map_err(|e| e.to_string())?;
        Ok(Self { ops: tx })
    }

    /// Offer `items` (mime type and bytes) as the workspace's clipboard.
    pub fn set(&self, items: Vec<(String, Vec<u8>)>) -> Result<(), String> {
        if items.iter().map(|(_, d)| d.len()).sum::<usize>() > MAX_BYTES {
            return Err("that is too large for the clipboard".into());
        }
        let (tx, rx) = channel();
        self.ops.send(Op::Set(items, tx)).map_err(|_| "the workspace has ended".to_string())?;
        rx.recv_timeout(Duration::from_secs(5)).map_err(|_| "the clipboard did not answer".to_string())?
    }

    pub fn set_text(&self, text: &str) -> Result<(), String> {
        self.set(TEXT_TYPES.iter().map(|m| (m.to_string(), text.as_bytes().to_vec())).collect())
    }

    /// The clipboard in the first of `types` it offers, with that type.
    pub fn get(&self, types: &[&str]) -> Got {
        let (tx, rx) = channel();
        let types = types.iter().map(|t| t.to_string()).collect();
        self.ops.send(Op::Get(types, tx)).map_err(|_| "the workspace has ended".to_string())?;
        rx.recv_timeout(Duration::from_secs(5)).map_err(|_| "the clipboard did not answer".to_string())?
    }

    pub fn get_text(&self) -> Result<Option<String>, String> {
        Ok(self.get(TEXT_TYPES)?.map(|(_, d)| String::from_utf8_lossy(&d).into_owned()))
    }
}

fn serve(
    conn: Connection,
    mut queue: EventQueue<State>,
    qh: QueueHandle<State>,
    mut state: State,
    dev: device::ZwlrDataControlDeviceV1,
    ops: Receiver<Op>,
) {
    loop {
        // Answer paste requests and follow selection changes between operations.
        if wl::dispatch_until(&mut queue, &mut state, Some(Instant::now() + Duration::from_millis(50)), |_| false).is_err() {
            return;
        }
        let op = match ops.try_recv() {
            Ok(op) => op,
            Err(std::sync::mpsc::TryRecvError::Empty) => continue,
            Err(_) => return,
        };
        match op {
            Op::Set(items, reply) => {
                let Some(m) = state.manager.clone() else { return };
                let src = m.create_data_source(&qh, ());
                let mut data = HashMap::new();
                for (mime, bytes) in items {
                    src.offer(mime.clone());
                    data.insert(mime, bytes);
                }
                dev.set_selection(Some(&src));
                if let Some((old, _)) = state.own.replace((src, data)) {
                    old.destroy();
                }
                let r = queue.roundtrip(&mut state).map(drop).map_err(|e| e.to_string());
                let _ = reply.send(r);
            }
            Op::Get(types, reply) => {
                let _ = reply.send(read(&conn, &mut queue, &mut state, &types));
            }
        }
        let _ = conn.flush();
    }
}

fn read(conn: &Connection, queue: &mut EventQueue<State>, state: &mut State, types: &[String]) -> Got {
    queue.roundtrip(state).map_err(|e| e.to_string())?;
    // What this connection itself offers is answered without a round trip to itself.
    if let Some((_, data)) = &state.own
        && let Some(t) = types.iter().find(|t| data.contains_key(*t))
    {
        return Ok(Some((t.clone(), data[t].clone())));
    }
    let Some(sel) = state.selection.clone() else { return Ok(None) };
    let offered = state.offers.get(&sel.id().protocol_id()).map(|(_, m)| m.clone()).unwrap_or_default();
    let Some(mime) = types.iter().find(|t| offered.contains(t)).cloned() else { return Ok(None) };
    let (r, w) = pipe()?;
    sel.receive(mime.clone(), w.as_fd());
    conn.flush().map_err(|e| e.to_string())?;
    drop(w);
    let mut file = std::fs::File::from(r);
    let mut out = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut buf = [0u8; 65536];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err("the program that copied did not hand the clipboard over".into());
        }
        let mut pfd = libc::pollfd { fd: file.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        if unsafe { libc::poll(&mut pfd, 1, left.as_millis() as i32) } <= 0 {
            continue;
        }
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                out.extend_from_slice(&buf[..n]);
                if out.len() > MAX_BYTES {
                    return Err("the clipboard holds too much to pass on".into());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(Some((mime, out)))
}

fn pipe() -> Result<(OwnedFd, OwnedFd), String> {
    let mut fds = [0; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(s: &mut Self, r: &wl_registry::WlRegistry, e: wl_registry::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_registry::Event::Global { name, interface, version } = e {
            match interface.as_str() {
                "wl_seat" if s.seat.is_none() => s.seat = Some(r.bind(name, version.min(5), qh, ())),
                "zwlr_data_control_manager_v1" => s.manager = Some(r.bind(name, version.min(2), qh, ())),
                _ => {}
            }
        }
    }
}

impl Dispatch<device::ZwlrDataControlDeviceV1, ()> for State {
    fn event(s: &mut Self, _: &device::ZwlrDataControlDeviceV1, e: device::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match e {
            device::Event::DataOffer { id } => {
                s.offers.insert(id.id().protocol_id(), (id, Vec::new()));
            }
            device::Event::Selection { id } => {
                if let Some(old) = s.selection.take()
                    && Some(&old) != id.as_ref()
                {
                    s.offers.remove(&old.id().protocol_id());
                    old.destroy();
                }
                s.selection = id;
            }
            device::Event::PrimarySelection { id: Some(o) } => {
                // Only the clipboard is passed on; let go of the primary selection.
                s.offers.remove(&o.id().protocol_id());
                o.destroy();
            }
            device::Event::Finished => s.manager = None,
            _ => {}
        }
    }

    event_created_child!(State, device::ZwlrDataControlDeviceV1, [
        device::EVT_DATA_OFFER_OPCODE => (offer::ZwlrDataControlOfferV1, ())
    ]);
}

impl Dispatch<offer::ZwlrDataControlOfferV1, ()> for State {
    fn event(s: &mut Self, o: &offer::ZwlrDataControlOfferV1, e: offer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let offer::Event::Offer { mime_type } = e
            && let Some((_, types)) = s.offers.get_mut(&o.id().protocol_id())
        {
            types.push(mime_type);
        }
    }
}

impl Dispatch<source::ZwlrDataControlSourceV1, ()> for State {
    fn event(
        s: &mut Self,
        src: &source::ZwlrDataControlSourceV1,
        e: source::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match e {
            source::Event::Send { mime_type, fd } => {
                if let Some((own, data)) = &s.own
                    && own == src
                    && let Some(bytes) = data.get(&mime_type)
                {
                    let bytes = bytes.clone();
                    // A slow reader must not hold up the clipboard thread.
                    std::thread::spawn(move || {
                        let mut f = std::fs::File::from(fd);
                        let _ = f.write_all(&bytes);
                    });
                }
            }
            source::Event::Cancelled => {
                if s.own.as_ref().is_some_and(|(own, _)| own == src) {
                    s.own = None;
                }
                src.destroy();
            }
            _ => {}
        }
    }
}

delegate_noop!(State: ignore wl_seat::WlSeat);
delegate_noop!(State: manager::ZwlrDataControlManagerV1);
