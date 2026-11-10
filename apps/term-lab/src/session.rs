//! A program on a local pty, wired to one main view and any number of
//! read-only mirrors.

use std::cell::RefCell;
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use nyx_pty::{Pty, Size, Spawn};
use nyx_term_gtk::TerminalView;

type OutputFn = Box<dyn Fn(&[u8])>;

pub struct Session {
    inner: Rc<Inner>,
}

struct Inner {
    pty: RefCell<Pty>,
    grid: std::cell::Cell<(u16, u16)>,
    mirrors: RefCell<Vec<TerminalView>>,
    on_exit: RefCell<Option<Box<dyn Fn()>>>,
    on_output: RefCell<Vec<OutputFn>>,
}

impl Session {
    pub fn spawn(argv: &[&str], view: &TerminalView) -> std::io::Result<Self> {
        let (cols, rows) = view.grid();
        let argv: Vec<&OsStr> = argv.iter().map(OsStr::new).collect();
        let env = [(OsStr::new("TERM"), OsStr::new("xterm-256color")), (OsStr::new("COLORTERM"), OsStr::new("truecolor"))];
        let pty = Pty::spawn(&Spawn {
            argv: &argv,
            cwd: None,
            env: &env,
            env_remove: &[],
            size: Size { cols, rows, ..Default::default() },
        })?;
        let mut reader = pty.try_clone_master()?;
        let inner = Rc::new(Inner {
            pty: RefCell::new(pty),
            grid: std::cell::Cell::new((cols, rows)),
            mirrors: RefCell::new(Vec::new()),
            on_exit: RefCell::new(None),
            on_output: RefCell::new(Vec::new()),
        });

        let (tx, rx) = async_channel::bounded::<Vec<u8>>(64);
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send_blocking(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let weak_view = view.downgrade();
        let weak = Rc::downgrade(&inner);
        glib::spawn_future_local(async move {
            while let Ok(chunk) = rx.recv().await {
                let Some(inner) = weak.upgrade() else { break };
                if let Some(v) = weak_view.upgrade() {
                    v.feed(&chunk);
                }
                for m in inner.mirrors.borrow().iter() {
                    m.feed(&chunk);
                }
                for f in inner.on_output.borrow().iter() {
                    f(&chunk);
                }
            }
            if let Some(inner) = weak.upgrade() {
                let _ = inner.pty.borrow_mut().try_wait();
                if let Some(f) = inner.on_exit.borrow().as_ref() {
                    f();
                }
            }
        });

        let weak = Rc::downgrade(&inner);
        view.connect_input(move |bytes| {
            if let Some(inner) = weak.upgrade() {
                let _ = inner.pty.borrow().master().write_all(bytes);
            }
        });
        let weak = Rc::downgrade(&inner);
        view.connect_resize(move |&(cols, rows, w, h)| {
            let Some(inner) = weak.upgrade() else { return };
            inner.grid.set((cols, rows));
            let _ = inner.pty.borrow().resize(Size { cols, rows, width_px: w as u16, height_px: h as u16 });
            // Mirrors draw at the program's grid, not their own.
            for m in inner.mirrors.borrow().iter() {
                m.terminal(|t| {
                    let _ = t.resize(cols, rows, 1, 1);
                });
            }
        });
        Ok(Self { inner })
    }

    pub fn add_mirror(&self, view: &TerminalView) {
        let (cols, rows) = self.inner.grid.get();
        view.terminal(|t| {
            let _ = t.resize(cols, rows, 1, 1);
        });
        self.inner.mirrors.borrow_mut().push(view.clone());
    }

    pub fn on_exit(&self, f: impl Fn() + 'static) {
        *self.inner.on_exit.borrow_mut() = Some(Box::new(f));
    }

    pub fn on_output(&self, f: impl Fn(&[u8]) + 'static) {
        self.inner.on_output.borrow_mut().push(Box::new(f));
    }

    pub fn write(&self, bytes: &[u8]) {
        let _ = self.inner.pty.borrow().master().write_all(bytes);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.inner.pty.borrow().signal(libc::SIGHUP);
    }
}
