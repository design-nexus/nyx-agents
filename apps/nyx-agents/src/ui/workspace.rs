//! The live view of an agent workspace: its screen, redrawn when it changes
//! (at most about 15 times a second), and an explicit Take over switch that
//! sends your pointer and keys into it. View only until you switch it on.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gdk, glib};
use nyx_agent_workspace::{Capturer, Frame, Input};
use nyx_agents_proto::{AgentWorkspace, Call, InputAction};
use nyx_ui::chrome;

use super::daemon;

const FRAME_GAP: Duration = Duration::from_millis(66);

thread_local! {
    static OPEN: RefCell<HashMap<String, gtk::Window>> = RefCell::new(HashMap::new());
    /// Panes' labels for the window in front on their workspace.
    static FOCUSED: RefCell<Vec<(String, glib::WeakRef<gtk::Label>)>> = const { RefCell::new(Vec::new()) };
}

/// Show workspace `id` (a unique prefix will do), or say why not.
pub fn open(id: &str, parent: Option<gtk::Window>, toast: impl Fn(&str) + 'static) {
    let prefix = id.to_string();
    daemon::request(
        move |c| c.call::<Vec<AgentWorkspace>>(Call::ListWorkspaces),
        move |r| {
            let list = match r {
                Ok(l) => l,
                Err(e) => return toast(&e),
            };
            let found: Vec<&AgentWorkspace> = list.iter().filter(|w| w.id.starts_with(&prefix)).collect();
            match found.as_slice() {
                [w] => present(w, parent.as_ref()),
                [] => toast("That workspace no longer exists"),
                _ => toast("That id matches more than one workspace"),
            }
        },
    );
}

/// Put your clipboard (text, else an image) on workspace `id`'s clipboard.
pub fn push_clipboard(id: &str, done: impl FnOnce(Result<(), String>) + 'static) {
    let Some(display) = gdk::Display::default() else { return done(Err("no display".into())) };
    let clip = display.clipboard();
    let id = id.to_string();
    let text = super::text_on(&clip.formats());
    glib::spawn_future_local(async move {
        let call = if text {
            match clip.read_text_future().await {
                Ok(Some(t)) => Call::WorkspaceClipboardSet { id, text: Some(t.to_string()), png: None },
                _ => return done(Err("your clipboard is empty".into())),
            }
        } else {
            match clip.read_texture_future().await {
                Ok(Some(t)) => {
                    let png = nyx_agent_workspace::base64(&t.save_to_png_bytes());
                    Call::WorkspaceClipboardSet { id, text: None, png: Some(png) }
                }
                _ => return done(Err("your clipboard holds no text or image".into())),
            }
        };
        daemon::request(move |c| c.call::<serde_json::Value>(call).map(drop), done);
    });
}

/// Put workspace `id`'s clipboard (text, else an image) on yours.
pub fn pull_clipboard(id: &str, done: impl FnOnce(Result<(), String>) + 'static) {
    let id = id.to_string();
    daemon::request(
        move |c| {
            let t: serde_json::Value = c.call(Call::WorkspaceClipboardGet { id: id.clone(), image: false })?;
            if let Some(text) = t["text"].as_str() {
                return Ok((Some(text.to_string()), None));
            }
            let i: serde_json::Value = c.call(Call::WorkspaceClipboardGet { id, image: true })?;
            Ok((None, i["png"].as_str().and_then(nyx_agent_workspace::unbase64)))
        },
        move |r| {
            let Some(display) = gdk::Display::default() else { return };
            match r {
                Ok((Some(text), _)) => {
                    display.clipboard().set_text(&text);
                    done(Ok(()))
                }
                Ok((None, Some(png))) => match gdk::Texture::from_bytes(&glib::Bytes::from_owned(png)) {
                    Ok(t) => {
                        display.clipboard().set_texture(&t);
                        done(Ok(()))
                    }
                    Err(e) => done(Err(e.to_string())),
                },
                Ok(_) => done(Err("the workspace's clipboard is empty".into())),
                Err(e) => done(Err(e)),
            }
        },
    );
}

/// Keep an open view's title current.
pub fn changed(w: &AgentWorkspace) {
    FOCUSED.with(|f| {
        f.borrow_mut().retain(|(id, l)| {
            let Some(l) = l.upgrade() else { return false };
            if *id == w.id {
                l.set_text(w.focused.as_deref().unwrap_or_default());
            }
            true
        })
    });
    if let Some(win) = OPEN.with(|o| o.borrow().get(&w.id).cloned()) {
        let focused = w.focused.as_deref().map(|f| format!(" · {f}")).unwrap_or_default();
        win.set_title(Some(&format!("{}{focused} · Agent workspace", w.name)));
    }
}

/// The workspace ended: close its view.
pub fn removed(id: &str) {
    if let Some(win) = OPEN.with(|o| o.borrow_mut().remove(id)) {
        win.close();
    }
}

/// The live picture with its Take over switch, for a window or a session's pane.
struct View {
    body: gtk::Overlay,
    take: gtk::ToggleButton,
    driving: gtk::Label,
    stop: Arc<AtomicBool>,
}

fn present(w: &AgentWorkspace, parent: Option<&gtk::Window>) {
    if let Some(win) = OPEN.with(|o| o.borrow().get(&w.id).cloned()) {
        win.present();
        return;
    }
    let window =
        gtk::Window::builder().title(format!("{} · Agent workspace", w.name)).default_width(1000).default_height(680).build();
    window.add_css_class("agents-window");
    window.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));
    window.set_icon_name(Some("io.github.design_nexus.nyx.Agents"));
    if let Some(p) = parent {
        window.set_transient_for(Some(p));
    }
    let close: Rc<dyn Fn()> = {
        let win = window.downgrade();
        Rc::new(move || {
            if let Some(w) = win.upgrade() {
                w.close();
            }
        })
    };
    let Some(v) = build_view(w, close) else { return };
    let header = chrome::app_header(&window, "video-display-symbolic", "Agent workspace");
    header.set_subtitle(&w.name);
    header.end.append(&v.driving);
    header.end.append(&v.take);
    let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    frame.add_css_class("window-frame");
    frame.append(&header.root);
    frame.append(&v.body);
    window.set_child(Some(&frame));

    let id = w.id.clone();
    let stop = v.stop.clone();
    window.connect_close_request(move |_| {
        stop.store(true, Ordering::SeqCst);
        OPEN.with(|o| o.borrow_mut().remove(&id));
        glib::Propagation::Proceed
    });
    OPEN.with(|o| o.borrow_mut().insert(w.id.clone(), window.clone()));
    window.present();
}

/// The agent's desktop in its session's tab: a slim header (Take over, Open in
/// a window, Hide) over the live picture.
pub struct Pane {
    pub root: gtk::Box,
    stop: Rc<RefCell<Option<Arc<AtomicBool>>>>,
}

impl Drop for Pane {
    fn drop(&mut self) {
        if let Some(s) = self.stop.borrow_mut().take() {
            s.store(true, Ordering::SeqCst);
        }
    }
}

pub fn pane(id: &str, on_window: impl Fn() + 'static, on_hide: impl Fn() + 'static) -> Pane {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("desktop-pane");
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    head.add_css_class("desktop-head");
    head.append(&gtk::Image::from_icon_name("video-display-symbolic"));
    let title = gtk::Label::new(Some("Desktop"));
    title.add_css_class("desktop-title");
    title.set_xalign(0.0);
    title.set_hexpand(true);
    head.append(&title);
    let focused = gtk::Label::new(None);
    focused.add_css_class("session-meta");
    focused.set_ellipsize(gtk::pango::EllipsizeMode::End);
    focused.set_max_width_chars(24);
    head.append(&focused);
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    head.append(&controls);
    let open = nyx_ui::widgets::bar_button("window-new-symbolic", "Open the desktop in a window");
    let hide = nyx_ui::widgets::bar_button("window-close-symbolic", "Hide the desktop");
    head.append(&open);
    head.append(&hide);
    open.connect_clicked(move |_| on_window());
    let on_hide: Rc<dyn Fn()> = Rc::new(on_hide);
    {
        let on_hide = on_hide.clone();
        hide.connect_clicked(move |_| on_hide());
    }
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.add_css_class("desktop-screen");
    body.set_vexpand(true);
    body.set_overflow(gtk::Overflow::Hidden);
    let status = gtk::Label::new(Some("Connecting…"));
    status.add_css_class("session-meta");
    status.set_vexpand(true);
    body.append(&status);
    let note = gtk::Label::new(Some("The agent is driving. Take over sends your pointer and keys in; it can still act too."));
    note.add_css_class("desktop-note");
    note.set_xalign(0.0);
    note.set_wrap(true);
    root.append(&head);
    root.append(&body);
    root.append(&note);

    let stop: Rc<RefCell<Option<Arc<AtomicBool>>>> = Rc::new(RefCell::new(None));
    let prefix = id.to_string();
    let (slot, controls2, stop2) = (body.downgrade(), controls.downgrade(), stop.clone());
    daemon::request(
        move |c| c.call::<Vec<AgentWorkspace>>(Call::ListWorkspaces),
        move |r| {
            let (Some(body), Some(controls)) = (slot.upgrade(), controls2.upgrade()) else { return };
            let found = r.ok().and_then(|l| l.into_iter().find(|w| w.id.starts_with(&prefix)));
            let Some(w) = found else {
                status.set_text("This desktop has ended.");
                return;
            };
            let Some(v) = build_view(&w, on_hide.clone()) else {
                status.set_text("This desktop has ended.");
                return;
            };
            focused.set_text(w.focused.as_deref().unwrap_or_default());
            FOCUSED.with(|f| f.borrow_mut().push((w.id.clone(), focused.downgrade())));
            body.remove(&status);
            v.body.set_vexpand(true);
            body.append(&v.body);
            // In a pane, a labelled switch; the note under the picture says who drives.
            let label = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            label.append(&gtk::Image::from_icon_name("input-mouse-symbolic"));
            label.append(&gtk::Label::new(Some("Take over")));
            v.take.set_child(Some(&label));
            v.take.remove_css_class("bar-button");
            v.take.add_css_class("take-over");
            v.take.connect_toggled(move |b| {
                note.set_text(if b.is_active() {
                    "You're driving: your pointer and keys go to the desktop. The agent can still act too."
                } else {
                    "The agent is driving. Take over sends your pointer and keys in; it can still act too."
                });
            });
            controls.append(&v.take);
            *stop2.borrow_mut() = Some(v.stop);
        },
    );
    Pane { root, stop }
}

/// The picture, its capture and input, and its right-click menu; `close`
/// closes whatever shows it.
fn build_view(w: &AgentWorkspace, close: Rc<dyn Fn()>) -> Option<View> {
    let socket = w.wayland_socket().map(PathBuf::from)?;
    let logical = ((w.width as f64 / w.scale).round().max(1.0), (w.height as f64 / w.scale).round().max(1.0));
    let driving = gtk::Label::new(Some("You are driving: your pointer and keys go to the workspace"));
    driving.add_css_class("session-meta");
    driving.set_visible(false);
    let take = gtk::ToggleButton::new();
    take.set_icon_name("input-mouse-symbolic");
    take.add_css_class("bar-button");
    take.set_valign(gtk::Align::Center);
    take.set_tooltip_text(Some("Take over: send your pointer and keys into the workspace (the agent can still act too)"));
    take.update_property(&[gtk::accessible::Property::Label("Take over")]);

    let picture = gtk::Picture::new();
    picture.set_content_fit(gtk::ContentFit::Contain);
    picture.set_can_shrink(true);
    picture.set_hexpand(true);
    picture.set_vexpand(true);
    picture.set_focusable(true);
    let status = gtk::Label::new(Some("Connecting…"));
    status.add_css_class("session-meta");
    let body = gtk::Overlay::new();
    body.set_child(Some(&picture));
    status.set_halign(gtk::Align::Center);
    status.set_valign(gtk::Align::Center);
    body.add_overlay(&status);

    let stop = Arc::new(AtomicBool::new(false));
    follow(&socket, &picture, &status, stop.clone());
    let input = Rc::new(RefCell::new(None::<async_channel::Sender<InputAction>>));
    let texture_size = Rc::new(Cell::new((0.0f64, 0.0f64)));
    {
        let ts = texture_size.clone();
        picture.connect_paintable_notify(move |p| {
            if let Some(t) = p.paintable() {
                ts.set((t.intrinsic_width() as f64, t.intrinsic_height() as f64));
            }
        });
    }
    {
        let input = input.clone();
        let socket = socket.clone();
        let driving = driving.clone();
        let ws_id = w.id.clone();
        let picture = picture.clone();
        take.connect_toggled(move |b| {
            driving.set_visible(b.is_active());
            if b.is_active() {
                *input.borrow_mut() = Some(drive(&socket, logical));
                picture.grab_focus();
                // What you copied is there to paste while you drive.
                push_clipboard(&ws_id, |_| {});
            } else {
                input.borrow_mut().take();
            }
        });
    }
    controllers(&picture, input, texture_size, logical);
    {
        let (take, id) = (take.clone(), w.id.clone());
        nyx_ui::menu::on_right_click(&picture, move |pw, x, y| {
            // While you drive, right-clicks belong to the workspace.
            if take.is_active() {
                return;
            }
            let (t, wn, id) = (take.clone(), close.clone(), id.clone());
            let (push_id, pull_id) = (id.clone(), id.clone());
            let note = |what: &'static str| {
                move |r: Result<(), String>| {
                    if let Err(e) = r {
                        eprintln!("nyx-agents: {what}: {e}");
                    }
                }
            };
            let entries = vec![
                nyx_ui::menu::check("Take over", false, move |on| t.set_active(on)),
                nyx_ui::menu::separator(),
                nyx_ui::menu::item("Paste my clipboard into the workspace", move || {
                    push_clipboard(&push_id, note("copying your clipboard in"))
                })
                .icon("edit-paste-symbolic"),
                nyx_ui::menu::item("Copy the workspace's clipboard", move || {
                    pull_clipboard(&pull_id, note("copying the workspace's clipboard"))
                })
                .icon("edit-copy-symbolic"),
                nyx_ui::menu::separator(),
                nyx_ui::menu::item("Close the view", move || wn()).icon("window-close-symbolic"),
                nyx_ui::menu::item("End this workspace", move || {
                    let id = id.clone();
                    daemon::request(move |c| c.call::<serde_json::Value>(Call::RemoveWorkspace { id }), |_| {});
                })
                .icon("user-trash-symbolic")
                .danger(),
            ];
            nyx_ui::menu::popup_at(pw, x, y, entries);
        });
    }
    Some(View { body, take, driving, stop })
}

/// Copy frames on a thread when the screen changes, at most every FRAME_GAP.
fn follow(socket: &std::path::Path, picture: &gtk::Picture, status: &gtk::Label, stop: Arc<AtomicBool>) {
    let (tx, rx) = async_channel::bounded::<Result<Frame, String>>(2);
    let socket = socket.to_path_buf();
    std::thread::spawn(move || {
        let mut cap = match Capturer::connect(&socket) {
            Ok(c) => c.with_cursor(),
            Err(e) => {
                let _ = tx.send_blocking(Err(e));
                return;
            }
        };
        while !stop.load(Ordering::SeqCst) && !tx.is_closed() {
            let started = Instant::now();
            match cap.next_change(Duration::from_millis(500)) {
                Ok(Some(f)) => {
                    // A full channel means the window is behind; skip this frame.
                    let _ = tx.try_send(Ok(f));
                }
                Ok(None) => continue,
                Err(e) => {
                    let _ = tx.send_blocking(Err(e));
                    return;
                }
            }
            if let Some(rest) = FRAME_GAP.checked_sub(started.elapsed()) {
                std::thread::sleep(rest);
            }
        }
    });
    let picture = picture.downgrade();
    let status = status.downgrade();
    glib::spawn_future_local(async move {
        while let Ok(r) = rx.recv().await {
            let (Some(picture), Some(status)) = (picture.upgrade(), status.upgrade()) else { break };
            match r {
                Ok(f) => {
                    status.set_visible(false);
                    let bytes = glib::Bytes::from_owned(f.data);
                    let tex = gdk::MemoryTexture::new(
                        f.width as i32,
                        f.height as i32,
                        gdk::MemoryFormat::R8g8b8a8,
                        &bytes,
                        f.width as usize * 4,
                    );
                    picture.set_paintable(Some(&tex));
                }
                Err(e) => {
                    status.set_text(&format!("The workspace can't be shown: {e}"));
                    status.set_visible(true);
                }
            }
        }
    });
}

/// Your own pointer and keyboard in the workspace, on a thread.
fn drive(socket: &std::path::Path, logical: (f64, f64)) -> async_channel::Sender<InputAction> {
    let (tx, rx) = async_channel::unbounded::<InputAction>();
    let socket = socket.to_path_buf();
    std::thread::spawn(move || {
        let Ok(mut input) = Input::connect(&socket, logical.0 as u32, logical.1 as u32) else { return };
        while let Ok(first) = rx.recv_blocking() {
            let mut batch = vec![first];
            while let Ok(a) = rx.try_recv() {
                batch.push(a);
            }
            for (i, a) in batch.iter().enumerate() {
                // Pointer motion piles up faster than it is sent; skip moves another move replaces.
                if matches!(a, InputAction::Move { .. }) && matches!(batch.get(i + 1), Some(InputAction::Move { .. })) {
                    continue;
                }
                if input.apply(a).is_err() && !input.alive() {
                    return;
                }
            }
        }
    });
    tx
}

/// Map a point in the picture to workspace coordinates, if it is on the image.
fn to_workspace(picture: &gtk::Picture, texture: (f64, f64), logical: (f64, f64), x: f64, y: f64) -> Option<(i32, i32)> {
    let (tw, th) = texture;
    let (ww, wh) = (picture.width() as f64, picture.height() as f64);
    if tw <= 0.0 || th <= 0.0 || ww <= 0.0 || wh <= 0.0 {
        return None;
    }
    let scale = (ww / tw).min(wh / th);
    let (ox, oy) = ((ww - tw * scale) / 2.0, (wh - th * scale) / 2.0);
    let (fx, fy) = ((x - ox) / (tw * scale), (y - oy) / (th * scale));
    if !(0.0..1.0).contains(&fx) || !(0.0..1.0).contains(&fy) {
        return None;
    }
    Some(((fx * logical.0) as i32, (fy * logical.1) as i32))
}

fn button_name(b: u32) -> Option<String> {
    let name = match b {
        1 => "left",
        2 => "middle",
        3 => "right",
        _ => return None,
    };
    Some(name.into())
}

fn controllers(
    picture: &gtk::Picture,
    input: Rc<RefCell<Option<async_channel::Sender<InputAction>>>>,
    texture: Rc<Cell<(f64, f64)>>,
    logical: (f64, f64),
) {
    let send = move |a: InputAction| {
        if let Some(tx) = input.borrow().as_ref() {
            let _ = tx.send_blocking(a);
        }
    };
    let send = Rc::new(send);

    let click = gtk::GestureClick::new();
    click.set_button(0);
    {
        let (send, texture, p) = (send.clone(), texture.clone(), picture.clone());
        click.connect_pressed(move |g, _, x, y| {
            p.grab_focus();
            if let Some((x, y)) = to_workspace(&p, texture.get(), logical, x, y) {
                send(InputAction::Move { x, y });
                send(InputAction::Button { button: button_name(g.current_button()), pressed: true });
            }
        });
    }
    {
        let send = send.clone();
        click.connect_released(move |g, _, _, _| {
            send(InputAction::Button { button: button_name(g.current_button()), pressed: false });
        });
    }
    picture.add_controller(click);

    let motion = gtk::EventControllerMotion::new();
    {
        let (send, texture, p) = (send.clone(), texture.clone(), picture.clone());
        motion.connect_motion(move |_, x, y| {
            if let Some((x, y)) = to_workspace(&p, texture.get(), logical, x, y) {
                send(InputAction::Move { x, y });
            }
        });
    }
    picture.add_controller(motion);

    let scroll =
        gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES | gtk::EventControllerScrollFlags::DISCRETE);
    {
        let send = send.clone();
        scroll.connect_scroll(move |_, dx, dy| {
            send(InputAction::Scroll { x: None, y: None, dx: dx.round() as i32, dy: dy.round() as i32 });
            glib::Propagation::Stop
        });
    }
    picture.add_controller(scroll);

    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed(move |_, keyval, _, state| match key_action(keyval, state) {
        Some(a) => {
            send(a);
            glib::Propagation::Stop
        }
        None => glib::Propagation::Proceed,
    });
    picture.add_controller(keys);
}

/// A key press as the workspace input it stands for.
fn key_action(keyval: gdk::Key, state: gdk::ModifierType) -> Option<InputAction> {
    let name = keyval.name()?.to_string();
    if name.starts_with("Shift_") || name.starts_with("Control_") || name.starts_with("Alt_") || name.starts_with("Super_") {
        return None;
    }
    let command = state.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK | gdk::ModifierType::SUPER_MASK);
    if !command && let Some(c) = keyval.to_unicode().filter(|c| !c.is_control()) {
        return Some(InputAction::Type { text: c.to_string() });
    }
    let mut combo = Vec::new();
    for (mask, m) in [
        (gdk::ModifierType::CONTROL_MASK, "ctrl"),
        (gdk::ModifierType::ALT_MASK, "alt"),
        (gdk::ModifierType::SUPER_MASK, "super"),
        (gdk::ModifierType::SHIFT_MASK, "shift"),
    ] {
        if state.contains(mask) {
            combo.push(m);
        }
    }
    combo.push(&name);
    Some(InputAction::Key { combo: combo.join("+") })
}
