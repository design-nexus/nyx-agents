use std::ptr;

use crate::ffi::*;
use crate::{Result, Terminal};

/// A physical key, by its W3C `code` (libghostty's `GhosttyKey`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key(pub u32);

impl Key {
    pub const UNIDENTIFIED: Key = Key(GHOSTTY_KEY_UNIDENTIFIED);

    /// Map a Linux evdev scancode (a GDK hardware keycode minus 8).
    pub fn from_evdev(code: u32) -> Key {
        let k = match code {
            1 => GHOSTTY_KEY_ESCAPE,
            2..=10 => GHOSTTY_KEY_DIGIT_1 + (code - 2),
            11 => GHOSTTY_KEY_DIGIT_0,
            12 => GHOSTTY_KEY_MINUS,
            13 => GHOSTTY_KEY_EQUAL,
            14 => GHOSTTY_KEY_BACKSPACE,
            15 => GHOSTTY_KEY_TAB,
            16 => GHOSTTY_KEY_Q,
            17 => GHOSTTY_KEY_W,
            18 => GHOSTTY_KEY_E,
            19 => GHOSTTY_KEY_R,
            20 => GHOSTTY_KEY_T,
            21 => GHOSTTY_KEY_Y,
            22 => GHOSTTY_KEY_U,
            23 => GHOSTTY_KEY_I,
            24 => GHOSTTY_KEY_O,
            25 => GHOSTTY_KEY_P,
            26 => GHOSTTY_KEY_BRACKET_LEFT,
            27 => GHOSTTY_KEY_BRACKET_RIGHT,
            28 => GHOSTTY_KEY_ENTER,
            29 => GHOSTTY_KEY_CONTROL_LEFT,
            30 => GHOSTTY_KEY_A,
            31 => GHOSTTY_KEY_S,
            32 => GHOSTTY_KEY_D,
            33 => GHOSTTY_KEY_F,
            34 => GHOSTTY_KEY_G,
            35 => GHOSTTY_KEY_H,
            36 => GHOSTTY_KEY_J,
            37 => GHOSTTY_KEY_K,
            38 => GHOSTTY_KEY_L,
            39 => GHOSTTY_KEY_SEMICOLON,
            40 => GHOSTTY_KEY_QUOTE,
            41 => GHOSTTY_KEY_BACKQUOTE,
            42 => GHOSTTY_KEY_SHIFT_LEFT,
            43 => GHOSTTY_KEY_BACKSLASH,
            44 => GHOSTTY_KEY_Z,
            45 => GHOSTTY_KEY_X,
            46 => GHOSTTY_KEY_C,
            47 => GHOSTTY_KEY_V,
            48 => GHOSTTY_KEY_B,
            49 => GHOSTTY_KEY_N,
            50 => GHOSTTY_KEY_M,
            51 => GHOSTTY_KEY_COMMA,
            52 => GHOSTTY_KEY_PERIOD,
            53 => GHOSTTY_KEY_SLASH,
            54 => GHOSTTY_KEY_SHIFT_RIGHT,
            55 => GHOSTTY_KEY_NUMPAD_MULTIPLY,
            56 => GHOSTTY_KEY_ALT_LEFT,
            57 => GHOSTTY_KEY_SPACE,
            58 => GHOSTTY_KEY_CAPS_LOCK,
            59..=68 => GHOSTTY_KEY_F1 + (code - 59),
            69 => GHOSTTY_KEY_NUM_LOCK,
            70 => GHOSTTY_KEY_SCROLL_LOCK,
            71 => GHOSTTY_KEY_NUMPAD_7,
            72 => GHOSTTY_KEY_NUMPAD_8,
            73 => GHOSTTY_KEY_NUMPAD_9,
            74 => GHOSTTY_KEY_NUMPAD_SUBTRACT,
            75 => GHOSTTY_KEY_NUMPAD_4,
            76 => GHOSTTY_KEY_NUMPAD_5,
            77 => GHOSTTY_KEY_NUMPAD_6,
            78 => GHOSTTY_KEY_NUMPAD_ADD,
            79 => GHOSTTY_KEY_NUMPAD_1,
            80 => GHOSTTY_KEY_NUMPAD_2,
            81 => GHOSTTY_KEY_NUMPAD_3,
            82 => GHOSTTY_KEY_NUMPAD_0,
            83 => GHOSTTY_KEY_NUMPAD_DECIMAL,
            86 => GHOSTTY_KEY_INTL_BACKSLASH,
            87 => GHOSTTY_KEY_F11,
            88 => GHOSTTY_KEY_F12,
            89 => GHOSTTY_KEY_INTL_RO,
            96 => GHOSTTY_KEY_NUMPAD_ENTER,
            97 => GHOSTTY_KEY_CONTROL_RIGHT,
            98 => GHOSTTY_KEY_NUMPAD_DIVIDE,
            99 => GHOSTTY_KEY_PRINT_SCREEN,
            100 => GHOSTTY_KEY_ALT_RIGHT,
            102 => GHOSTTY_KEY_HOME,
            103 => GHOSTTY_KEY_ARROW_UP,
            104 => GHOSTTY_KEY_PAGE_UP,
            105 => GHOSTTY_KEY_ARROW_LEFT,
            106 => GHOSTTY_KEY_ARROW_RIGHT,
            107 => GHOSTTY_KEY_END,
            108 => GHOSTTY_KEY_ARROW_DOWN,
            109 => GHOSTTY_KEY_PAGE_DOWN,
            110 => GHOSTTY_KEY_INSERT,
            111 => GHOSTTY_KEY_DELETE,
            117 => GHOSTTY_KEY_NUMPAD_EQUAL,
            119 => GHOSTTY_KEY_PAUSE,
            121 => GHOSTTY_KEY_NUMPAD_COMMA,
            124 => GHOSTTY_KEY_INTL_YEN,
            125 => GHOSTTY_KEY_META_LEFT,
            126 => GHOSTTY_KEY_META_RIGHT,
            127 => GHOSTTY_KEY_CONTEXT_MENU,
            183..=194 => GHOSTTY_KEY_F13 + (code - 183),
            _ => GHOSTTY_KEY_UNIDENTIFIED,
        };
        Key(k)
    }

    pub fn is_modifier(self) -> bool {
        matches!(
            self.0,
            GHOSTTY_KEY_SHIFT_LEFT
                | GHOSTTY_KEY_SHIFT_RIGHT
                | GHOSTTY_KEY_CONTROL_LEFT
                | GHOSTTY_KEY_CONTROL_RIGHT
                | GHOSTTY_KEY_ALT_LEFT
                | GHOSTTY_KEY_ALT_RIGHT
                | GHOSTTY_KEY_META_LEFT
                | GHOSTTY_KEY_META_RIGHT
                | GHOSTTY_KEY_CAPS_LOCK
                | GHOSTTY_KEY_NUM_LOCK
        )
    }
}

/// Modifier keys held.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Mods(pub u16);

impl Mods {
    pub const SHIFT: u16 = GHOSTTY_MODS_SHIFT as u16;
    pub const CTRL: u16 = GHOSTTY_MODS_CTRL as u16;
    pub const ALT: u16 = GHOSTTY_MODS_ALT as u16;
    pub const SUPER: u16 = GHOSTTY_MODS_SUPER as u16;
    pub const CAPS_LOCK: u16 = GHOSTTY_MODS_CAPS_LOCK as u16;
    pub const NUM_LOCK: u16 = GHOSTTY_MODS_NUM_LOCK as u16;

    pub fn has(self, m: u16) -> bool {
        self.0 & m != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAction {
    Press,
    Repeat,
    Release,
}

/// Encodes key events the way the running program asked (legacy, xterm
/// modifyOtherKeys or the Kitty keyboard protocol).
pub struct KeyEncoder {
    enc: GhosttyKeyEncoder,
    ev: GhosttyKeyEvent,
}

unsafe impl Send for KeyEncoder {}

impl KeyEncoder {
    pub fn new() -> Result<Self> {
        let mut enc: GhosttyKeyEncoder = ptr::null_mut();
        let mut ev: GhosttyKeyEvent = ptr::null_mut();
        unsafe {
            check(ghostty_key_encoder_new(ptr::null(), &mut enc))?;
            check(ghostty_key_event_new(ptr::null(), &mut ev))?;
        }
        Ok(Self { enc, ev })
    }

    /// The bytes for a key event, or nothing when the key sends nothing.
    /// `text` is what the key typed (after layout and dead keys);
    /// `unshifted` is the key's codepoint without Shift.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        term: &Terminal,
        key: Key,
        action: KeyAction,
        mods: Mods,
        consumed: Mods,
        text: &str,
        unshifted: Option<char>,
        composing: bool,
    ) -> Vec<u8> {
        let action = match action {
            KeyAction::Press => GHOSTTY_KEY_ACTION_PRESS,
            KeyAction::Repeat => GHOSTTY_KEY_ACTION_REPEAT,
            KeyAction::Release => GHOSTTY_KEY_ACTION_RELEASE,
        };
        let mut buf = [0u8; 128];
        let mut n = 0usize;
        unsafe {
            ghostty_key_encoder_set_from_terminal(self.enc, term.raw());
            ghostty_key_event_set_action(self.ev, action);
            ghostty_key_event_set_key(self.ev, key.0);
            ghostty_key_event_set_mods(self.ev, mods.0);
            ghostty_key_event_set_consumed_mods(self.ev, consumed.0);
            ghostty_key_event_set_composing(self.ev, composing);
            ghostty_key_event_set_utf8(self.ev, gstr(text.as_bytes()));
            ghostty_key_event_set_unshifted_codepoint(self.ev, unshifted.map(|c| c as u32).unwrap_or(0));
            if ghostty_key_encoder_encode(self.enc, self.ev, buf.as_mut_ptr(), buf.len(), &mut n) != GHOSTTY_SUCCESS {
                return Vec::new();
            }
        }
        buf[..n].to_vec()
    }
}

impl Drop for KeyEncoder {
    fn drop(&mut self) {
        unsafe {
            ghostty_key_event_free(self.ev);
            ghostty_key_encoder_free(self.enc);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseAction {
    Press,
    Release,
    Motion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    None,
    Left,
    Middle,
    Right,
    /// Wheel up/down/left/right are buttons 4-7.
    WheelUp,
    WheelDown,
    WheelLeft,
    WheelRight,
}

/// Encodes pointer events for programs that turned on mouse reporting.
pub struct MouseEncoder {
    enc: GhosttyMouseEncoder,
    ev: GhosttyMouseEvent,
}

unsafe impl Send for MouseEncoder {}

impl MouseEncoder {
    pub fn new() -> Result<Self> {
        let mut enc: GhosttyMouseEncoder = ptr::null_mut();
        let mut ev: GhosttyMouseEvent = ptr::null_mut();
        unsafe {
            check(ghostty_mouse_encoder_new(ptr::null(), &mut enc))?;
            check(ghostty_mouse_event_new(ptr::null(), &mut ev))?;
            let track = true;
            ghostty_mouse_encoder_set(enc, GHOSTTY_MOUSE_ENCODER_OPT_TRACK_LAST_CELL, ptr(&track));
        }
        Ok(Self { enc, ev })
    }

    /// `x`/`y` are surface pixels; `width`/`height` the surface and
    /// `cell_w`/`cell_h` one cell, all in pixels.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        term: &Terminal,
        action: MouseAction,
        button: MouseButton,
        mods: Mods,
        any_pressed: bool,
        (x, y): (f32, f32),
        (width, height): (u32, u32),
        (cell_w, cell_h): (u32, u32),
    ) -> Vec<u8> {
        let size = sized::<GhosttyMouseEncoderSize>(|s, sz| {
            s.size = sz;
            s.screen_width = width.max(1);
            s.screen_height = height.max(1);
            s.cell_width = cell_w.max(1);
            s.cell_height = cell_h.max(1);
        });
        let b = match button {
            MouseButton::None => GHOSTTY_MOUSE_BUTTON_UNKNOWN,
            MouseButton::Left => GHOSTTY_MOUSE_BUTTON_LEFT,
            MouseButton::Middle => GHOSTTY_MOUSE_BUTTON_MIDDLE,
            MouseButton::Right => GHOSTTY_MOUSE_BUTTON_RIGHT,
            MouseButton::WheelUp => GHOSTTY_MOUSE_BUTTON_FOUR,
            MouseButton::WheelDown => GHOSTTY_MOUSE_BUTTON_FIVE,
            MouseButton::WheelLeft => GHOSTTY_MOUSE_BUTTON_SIX,
            MouseButton::WheelRight => GHOSTTY_MOUSE_BUTTON_SEVEN,
        };
        let a = match action {
            MouseAction::Press => GHOSTTY_MOUSE_ACTION_PRESS,
            MouseAction::Release => GHOSTTY_MOUSE_ACTION_RELEASE,
            MouseAction::Motion => GHOSTTY_MOUSE_ACTION_MOTION,
        };
        let mut buf = [0u8; 64];
        let mut n = 0usize;
        unsafe {
            ghostty_mouse_encoder_set_from_terminal(self.enc, term.raw());
            ghostty_mouse_encoder_set(self.enc, GHOSTTY_MOUSE_ENCODER_OPT_SIZE, ptr(&size));
            ghostty_mouse_encoder_set(self.enc, GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED, ptr(&any_pressed));
            ghostty_mouse_event_set_action(self.ev, a);
            if button == MouseButton::None {
                ghostty_mouse_event_clear_button(self.ev);
            } else {
                ghostty_mouse_event_set_button(self.ev, b);
            }
            ghostty_mouse_event_set_mods(self.ev, mods.0);
            ghostty_mouse_event_set_position(self.ev, GhosttyMousePosition { x, y });
            if ghostty_mouse_encoder_encode(self.enc, self.ev, buf.as_mut_ptr(), buf.len(), &mut n) != GHOSTTY_SUCCESS {
                return Vec::new();
            }
        }
        buf[..n].to_vec()
    }
}

impl Drop for MouseEncoder {
    fn drop(&mut self) {
        unsafe {
            ghostty_mouse_event_free(self.ev);
            ghostty_mouse_encoder_free(self.enc);
        }
    }
}

/// The focus report (CSI I / CSI O) for programs that asked for one (mode 1004).
pub fn focus_report(gained: bool) -> Vec<u8> {
    let mut buf = [0u8; 8];
    let mut n = 0usize;
    let ev = if gained { GHOSTTY_FOCUS_GAINED } else { GHOSTTY_FOCUS_LOST };
    if unsafe { ghostty_focus_encode(ev, buf.as_mut_ptr(), buf.len(), &mut n) } != GHOSTTY_SUCCESS {
        return Vec::new();
    }
    buf[..n].to_vec()
}
