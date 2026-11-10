//! The agent's pointer and keyboard: `wlr-virtual-pointer` and
//! `virtual-keyboard-unstable-v1` on the workspace's own seat.

use std::io::Write;
use std::os::fd::AsFd;
use std::path::Path;
use std::time::{Duration, Instant};

use wayland_client::protocol::{wl_pointer, wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, delegate_noop};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1 as kbd_manager, zwp_virtual_keyboard_v1 as kbd,
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1 as ptr_manager, zwlr_virtual_pointer_v1 as ptr,
};

use nyx_agents_proto::InputAction;

use crate::keys::{self, Layout};
use crate::wl;

/// Linux input event codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left = 0x110,
    Right = 0x111,
    Middle = 0x112,
}

impl Button {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "left" | "" => Self::Left,
            "right" => Self::Right,
            "middle" => Self::Middle,
            _ => return None,
        })
    }
}

#[derive(Default)]
struct State {
    seat: Option<wl_seat::WlSeat>,
    pointers: Option<ptr_manager::ZwlrVirtualPointerManagerV1>,
    keyboards: Option<kbd_manager::ZwpVirtualKeyboardManagerV1>,
}

/// Units one wheel click scrolls, as libinput reports it.
const WHEEL_STEP: f64 = 15.0;
/// Time between key and button events, so clients see distinct events.
const GAP: Duration = Duration::from_millis(8);

pub struct Input {
    conn: Connection,
    queue: EventQueue<State>,
    state: State,
    pointer: ptr::ZwlrVirtualPointerV1,
    keyboard: kbd::ZwpVirtualKeyboardV1,
    layout: Layout,
    /// Logical size of the output, which absolute motion is relative to.
    size: (u32, u32),
    pos: (u32, u32),
    start: Instant,
}

impl Input {
    pub fn connect(socket: &Path, width: u32, height: u32) -> Result<Self, String> {
        let conn = wl::connect(socket)?;
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();
        conn.display().get_registry(&qh, ());
        let mut state = State::default();
        queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
        let (Some(seat), Some(pm), Some(km)) = (state.seat.clone(), state.pointers.clone(), state.keyboards.clone()) else {
            return Err("the workspace doesn't offer virtual input".into());
        };
        let pointer = pm.create_virtual_pointer(Some(&seat), &qh, ());
        let keyboard = km.create_virtual_keyboard(&seat, &qh, ());
        let mut input = Self {
            conn,
            queue,
            state,
            pointer,
            keyboard,
            layout: Layout::default(),
            size: (width.max(1), height.max(1)),
            pos: (width / 2, height / 2),
            start: Instant::now(),
        };
        // A keyboard without a keymap is ignored.
        input.send_keymap()?;
        Ok(input)
    }

    pub fn set_size(&mut self, width: u32, height: u32) {
        self.size = (width.max(1), height.max(1));
    }

    fn time(&self) -> u32 {
        self.start.elapsed().as_millis() as u32
    }

    fn sync(&mut self) -> Result<(), String> {
        self.queue.roundtrip(&mut self.state).map_err(|e| e.to_string())?;
        std::thread::sleep(GAP);
        Ok(())
    }

    fn clamp(&self, x: i32, y: i32) -> (u32, u32) {
        (x.clamp(0, self.size.0 as i32 - 1) as u32, y.clamp(0, self.size.1 as i32 - 1) as u32)
    }

    pub fn move_to(&mut self, x: i32, y: i32) -> Result<(), String> {
        let (x, y) = self.clamp(x, y);
        self.pos = (x, y);
        self.pointer.motion_absolute(self.time(), x, y, self.size.0, self.size.1);
        self.pointer.frame();
        self.sync()
    }

    pub fn button(&mut self, b: Button, pressed: bool) -> Result<(), String> {
        let state = if pressed { wl_pointer::ButtonState::Pressed } else { wl_pointer::ButtonState::Released };
        self.pointer.button(self.time(), b as u32, state);
        self.pointer.frame();
        self.sync()
    }

    /// Click `count` times at the pointer, or at `at` after moving there.
    pub fn click(&mut self, at: Option<(i32, i32)>, b: Button, count: u32) -> Result<(), String> {
        if let Some((x, y)) = at {
            self.move_to(x, y)?;
        }
        for _ in 0..count.clamp(1, 3) {
            self.button(b, true)?;
            self.button(b, false)?;
        }
        Ok(())
    }

    /// Press at `from`, move to `to` in steps, release.
    pub fn drag(&mut self, from: (i32, i32), to: (i32, i32), b: Button) -> Result<(), String> {
        self.move_to(from.0, from.1)?;
        self.button(b, true)?;
        let steps = 12;
        for i in 1..=steps {
            let x = from.0 + (to.0 - from.0) * i / steps;
            let y = from.1 + (to.1 - from.1) * i / steps;
            self.move_to(x, y)?;
        }
        self.button(b, false)
    }

    /// Scroll by wheel clicks: positive `dy` scrolls down, positive `dx` right.
    pub fn scroll(&mut self, at: Option<(i32, i32)>, dx: i32, dy: i32) -> Result<(), String> {
        if let Some((x, y)) = at {
            self.move_to(x, y)?;
        }
        let t = self.time();
        self.pointer.axis_source(wl_pointer::AxisSource::Wheel);
        for (axis, n) in [(wl_pointer::Axis::VerticalScroll, dy), (wl_pointer::Axis::HorizontalScroll, dx)] {
            if n != 0 {
                let n = n.clamp(-50, 50);
                self.pointer.axis_discrete(t, axis, WHEEL_STEP * n as f64, n);
            }
        }
        self.pointer.frame();
        self.sync()
    }

    fn send_keymap(&mut self) -> Result<(), String> {
        let mut text = keys::keymap(&self.layout.syms).into_bytes();
        text.push(0);
        let mut f = wl::memfd(0)?;
        f.write_all(&text).map_err(|e| e.to_string())?;
        // 1 is XKB_V1.
        self.keyboard.keymap(1, f.as_fd(), text.len() as u32);
        self.sync()
    }

    fn ensure(&mut self, syms: &[String]) -> Result<(), String> {
        let extra: Vec<String> = syms.iter().filter(|s| keys::us_key(s).is_none()).cloned().collect();
        if !extra.is_empty() && self.layout.ensure(&extra) {
            self.send_keymap()?;
        }
        Ok(())
    }

    /// The key for a keysym and the modifiers it needs: a US key, or a spare
    /// keycode (which must have been given it with `ensure`).
    fn locate(&self, sym: &str, mods: u32) -> Result<(u32, u32), String> {
        if let Some((code, shift)) = keys::us_key(sym) {
            return Ok((code, if shift { mods | keys::SHIFT } else { mods }));
        }
        self.layout.code(sym).map(|c| (c, mods)).ok_or_else(|| format!("no keycode for {sym}"))
    }

    fn tap(&mut self, sym: &str, mods: u32) -> Result<(), String> {
        let (code, mods) = self.locate(sym, mods)?;
        if mods != 0 {
            self.keyboard.modifiers(mods, 0, 0, 0);
        }
        self.keyboard.key(self.time(), code, 1);
        self.sync()?;
        self.keyboard.key(self.time(), code, 0);
        if mods != 0 {
            self.keyboard.modifiers(0, 0, 0, 0);
        }
        self.sync()
    }

    /// Press a combo such as `ctrl+l` or `Return`.
    pub fn key(&mut self, combo: &str) -> Result<(), String> {
        let c = keys::parse_combo(combo)?;
        self.ensure(std::slice::from_ref(&c.key))?;
        self.tap(&c.key, c.mods)
    }

    /// Type text as key presses, whatever the characters.
    pub fn type_text(&mut self, text: &str) -> Result<(), String> {
        let syms: Vec<String> = text.chars().filter_map(keys::char_keysym).collect();
        for chunk in syms.chunks(keys::EXTRA_CODES) {
            self.ensure(chunk)?;
            for s in chunk {
                self.tap(s, 0)?;
            }
        }
        Ok(())
    }

    /// Run one protocol input step.
    pub fn apply(&mut self, a: &InputAction) -> Result<(), String> {
        let button = |name: &Option<String>| {
            let n = name.as_deref().unwrap_or("left");
            Button::parse(n).ok_or_else(|| format!("unknown button {n}"))
        };
        match a {
            InputAction::Move { x, y } => self.move_to(*x, *y),
            InputAction::Click { x, y, button: b, count } => self.click(x.zip(*y), button(b)?, count.unwrap_or(1)),
            InputAction::Button { button: b, pressed } => self.button(button(b)?, *pressed),
            InputAction::Drag { from_x, from_y, to_x, to_y, button: b } => {
                self.drag((*from_x, *from_y), (*to_x, *to_y), button(b)?)
            }
            InputAction::Scroll { x, y, dx, dy } => self.scroll(x.zip(*y), *dx, *dy),
            InputAction::Type { text } if text.chars().count() > 10_000 => Err("text is limited to 10,000 characters".into()),
            InputAction::Type { text } => self.type_text(text),
            InputAction::Key { combo } => self.key(combo),
            InputAction::Wait { ms } => {
                std::thread::sleep(Duration::from_millis((*ms).min(5000)));
                Ok(())
            }
        }
    }

    pub fn position(&self) -> (u32, u32) {
        self.pos
    }

    /// The connection is alive (the workspace has not gone away).
    pub fn alive(&mut self) -> bool {
        self.conn.flush().is_ok() && self.queue.roundtrip(&mut self.state).is_ok()
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        self.pointer.destroy();
        self.keyboard.destroy();
        let _ = self.conn.flush();
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(s: &mut Self, r: &wl_registry::WlRegistry, e: wl_registry::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_registry::Event::Global { name, interface, version } = e {
            match interface.as_str() {
                "wl_seat" if s.seat.is_none() => s.seat = Some(r.bind(name, version.min(5), qh, ())),
                "zwlr_virtual_pointer_manager_v1" => s.pointers = Some(r.bind(name, 1, qh, ())),
                "zwp_virtual_keyboard_manager_v1" => s.keyboards = Some(r.bind(name, 1, qh, ())),
                _ => {}
            }
        }
    }
}

delegate_noop!(State: ignore wl_seat::WlSeat);
delegate_noop!(State: ptr_manager::ZwlrVirtualPointerManagerV1);
delegate_noop!(State: ignore ptr::ZwlrVirtualPointerV1);
delegate_noop!(State: kbd_manager::ZwpVirtualKeyboardManagerV1);
delegate_noop!(State: ignore kbd::ZwpVirtualKeyboardV1);
