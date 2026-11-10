//! The window's link to the manager. All socket work happens on threads;
//! results come back to the main loop.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use nyx_agents_client::{Attachment, Client, FrameKind, Sender};
use nyx_agents_proto::{AttachMode, Event, Harness, ResizeMsg, Session};
use nyx_term::{Terminal, TerminalOptions};
use nyx_term_gtk::TerminalView;

pub enum Msg {
    Connected { sessions: Vec<Session>, harnesses: Vec<Harness> },
    Event(Event),
    Disconnected(String),
}

/// Follow the manager: a full list on (re)connect, then events. Reconnects
/// on its own after the manager restarts.
pub fn watch(on: impl Fn(Msg) + 'static) {
    let (tx, rx) = async_channel::unbounded::<Msg>();
    std::thread::spawn(move || {
        let mut tried_start = false;
        loop {
            let r = (|| -> nyx_agents_client::Result<()> {
                let mut c = Client::connect("nyx-agents")?;
                let sessions = c.sessions()?;
                let harnesses = c.harnesses(false)?;
                c.subscribe()?;
                if tx.send_blocking(Msg::Connected { sessions, harnesses }).is_err() {
                    return Ok(());
                }
                loop {
                    let e = c.next_event()?;
                    if tx.send_blocking(Msg::Event(e)).is_err() {
                        return Ok(());
                    }
                }
            })();
            if tx.is_closed() {
                return;
            }
            if let Err(e) = r {
                let _ = tx.send_blocking(Msg::Disconnected(e.to_string()));
            }
            // An installed manager starts from its socket unit; ask for it once.
            if !tried_start {
                tried_start = true;
                let _ = std::process::Command::new("systemctl")
                    .args(["--user", "start", "nyx-agentd.socket"])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    });
    glib::spawn_future_local(async move {
        while let Ok(m) = rx.recv().await {
            on(m);
        }
    });
}

/// Run one request on a thread; `done` gets the answer on the main loop.
pub fn request<T: Send + 'static>(
    f: impl FnOnce(&mut Client) -> nyx_agents_client::Result<T> + Send + 'static,
    done: impl FnOnce(Result<T, String>) + 'static,
) {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let r = Client::connect("nyx-agents").and_then(|mut c| f(&mut c)).map_err(|e| e.to_string());
        let _ = tx.send_blocking(r);
    });
    glib::spawn_future_local(async move {
        if let Ok(r) = rx.recv().await {
            done(r);
        }
    });
}

enum Frame {
    Snapshot(Vec<u8>),
    Output(Vec<u8>),
    Exited,
    Gone,
}

/// A view attached to a session's holder. Dropping it detaches.
pub struct Attached {
    sender: Rc<RefCell<Option<Sender>>>,
    _alive: async_channel::Sender<()>,
}

impl Attached {
    pub fn is_connected(&self) -> bool {
        self.sender.borrow().is_some()
    }
}

/// Attach `view` to session `id`. A `control` view drives the program; an
/// `observe` view only watches. `on_end` runs when the program exits or the
/// holder goes away.
pub fn attach(view: &TerminalView, id: &str, mode: AttachMode, on_end: impl Fn(bool) + 'static) -> Attached {
    let (tx, rx) = async_channel::unbounded::<Frame>();
    let (alive_tx, alive_rx) = async_channel::bounded::<()>(1);
    let (sender_tx, sender_rx) = async_channel::bounded::<Sender>(1);
    let style = view.style();
    let hex = |c: nyx_term::Rgb| format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b);
    let colors = Some((hex(style.colors.foreground), hex(style.colors.background)));
    let id = id.to_string();
    std::thread::spawn(move || {
        let mut a = match Attachment::open(&id, mode, colors) {
            Ok(a) => a,
            Err(_) => {
                let _ = tx.send_blocking(Frame::Gone);
                return;
            }
        };
        if let Ok(s) = a.sender() {
            let _ = sender_tx.send_blocking(s);
        }
        loop {
            if alive_rx.is_closed() {
                return;
            }
            let f = match a.next_frame() {
                Ok((Some(FrameKind::Snapshot), p)) => Frame::Snapshot(p),
                Ok((Some(FrameKind::Output), p)) => Frame::Output(p),
                Ok((Some(FrameKind::Exited), _)) => Frame::Exited,
                Ok(_) => continue,
                Err(_) => Frame::Gone,
            };
            let last = matches!(f, Frame::Gone);
            if tx.send_blocking(f).is_err() || last {
                return;
            }
        }
    });

    let sender: Rc<RefCell<Option<Sender>>> = Rc::new(RefCell::new(None));
    {
        let sender = sender.clone();
        let weak = view.downgrade();
        glib::spawn_future_local(async move {
            if let Ok(mut s) = sender_rx.recv().await {
                // Tell the program the size it is shown at, in case the view
                // laid out before the attachment was ready.
                if let Some(v) = weak.upgrade().filter(|_| mode == AttachMode::Control) {
                    let (cols, rows) = v.grid();
                    let (cw, ch, _) = v.cell_size();
                    let (width, height) = ((cols as f32 * cw) as u32, (rows as f32 * ch) as u32);
                    if cols > 1 {
                        let _ = s.resize(ResizeMsg { cols, rows, width, height });
                    }
                }
                *sender.borrow_mut() = Some(s);
            }
        });
    }
    let weak = view.downgrade();
    let opts = TerminalOptions { colors: style.colors.clone(), ..Default::default() };
    {
        let sender = sender.clone();
        glib::spawn_future_local(async move {
            while let Ok(f) = rx.recv().await {
                let Some(view) = weak.upgrade() else { break };
                match f {
                    Frame::Snapshot(bytes) => {
                        if let Ok(mut t) = Terminal::from_snapshot(&bytes, &opts) {
                            t.set_colors(&opts.colors);
                            view.replace_terminal(t);
                        }
                    }
                    Frame::Output(bytes) => view.feed(&bytes),
                    Frame::Exited => on_end(true),
                    Frame::Gone => {
                        sender.borrow_mut().take();
                        on_end(false);
                        break;
                    }
                }
            }
        });
    }
    if mode == AttachMode::Control {
        let s = sender.clone();
        view.connect_input(move |bytes| {
            if let Some(tx) = s.borrow_mut().as_mut() {
                let _ = tx.input(bytes);
            }
        });
        let s = sender.clone();
        view.connect_resize(move |&(cols, rows, width, height)| {
            if let Some(tx) = s.borrow_mut().as_mut() {
                let _ = tx.resize(ResizeMsg { cols, rows, width, height });
            }
        });
    }
    Attached { sender, _alive: alive_tx }
}

/// A read-only view of an ended session's last screen and scrollback.
pub fn history_view(id: &str) -> Option<TerminalView> {
    let bytes = std::fs::read(nyx_agents_core::store::snapshot_path(id)).ok()?;
    let style = super::theme::view_style();
    let mut t = Terminal::from_snapshot(&bytes, &TerminalOptions::default()).ok()?;
    t.set_colors(&style.colors);
    Some(TerminalView::with_terminal(t, style, true))
}
