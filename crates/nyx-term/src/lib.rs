//! Terminal emulation for Nyx Agents, on libghostty-vt 1.0.
//!
//! [`Terminal`] holds the screen, scrollback and modes. Write program output
//! into it, then drain [`Terminal::take_effects`] for what the program asked
//! of the host (replies to send back, title changes, OSC 7501 status,
//! clipboard writes, bells). [`Renderer`] turns the terminal into a [`Frame`]
//! of styled text runs that a GUI can draw without touching the C API.
//!
//! Nothing here is thread-safe on its own: a terminal and its renderer,
//! encoders and searches must be used from one thread at a time.

mod ffi;
mod input;
mod render;
mod search;
mod terminal;

pub use input::{Key, KeyAction, KeyEncoder, Mods, MouseAction, MouseButton, MouseEncoder, focus_report};
pub use render::{Cursor, CursorStyle, Dirty, Frame, Renderer, Row, Run, StyleFlags, Underline};
pub use search::{Search, SearchMatch};
pub use terminal::{
    ClipboardWrite, Effects, ProgramState, ScrollTo, Scrollbar, SelectGesture, Selection, StatusKind, StatusReport, Terminal,
    TerminalOptions,
};

/// An sRGB colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Parse `#rrggbb`.
    pub fn parse_hex(s: &str) -> Option<Self> {
        let h = s.trim().strip_prefix('#')?;
        if h.len() != 6 {
            return None;
        }
        let v = u32::from_str_radix(h, 16).ok()?;
        Some(Self::new((v >> 16) as u8, (v >> 8) as u8, v as u8))
    }
}

/// The colours a terminal starts with: default foreground, background and
/// cursor, and the 16 ANSI colours (the rest of the 256 are derived).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Colors {
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Option<Rgb>,
    pub ansi: [Rgb; 16],
}

impl Colors {
    /// Catppuccin Mocha, the NyxOS default theme.
    pub fn catppuccin_mocha() -> Self {
        let h = |s| Rgb::parse_hex(s).unwrap();
        Self {
            foreground: h("#cdd6f4"),
            background: h("#1e1e2e"),
            cursor: Some(h("#f5e0dc")),
            ansi: [
                h("#45475a"),
                h("#f38ba8"),
                h("#a6e3a1"),
                h("#f9e2af"),
                h("#89b4fa"),
                h("#f5c2e7"),
                h("#94e2d5"),
                h("#bac2de"),
                h("#585b70"),
                h("#f38ba8"),
                h("#a6e3a1"),
                h("#f9e2af"),
                h("#89b4fa"),
                h("#f5c2e7"),
                h("#94e2d5"),
                h("#a6adc8"),
            ],
        }
    }

    /// The full 256-colour palette: these 16, the 6x6x6 cube and the grey ramp.
    pub fn palette(&self) -> [Rgb; 256] {
        let mut p = [Rgb::default(); 256];
        p[..16].copy_from_slice(&self.ansi);
        let level = |v: usize| if v == 0 { 0 } else { (55 + v * 40) as u8 };
        for i in 0..216 {
            p[16 + i] = Rgb::new(level(i / 36), level((i / 6) % 6), level(i % 6));
        }
        for i in 0..24 {
            let v = (8 + i * 10) as u8;
            p[232 + i] = Rgb::new(v, v, v);
        }
        p
    }
}

/// An error from libghostty-vt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error(pub i32);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let what = match self.0 {
            -1 => "out of memory",
            -2 => "invalid value",
            -3 => "out of space",
            -4 => "no value",
            -5 => "unsupported",
            _ => "libghostty-vt error",
        };
        write!(f, "{what} ({})", self.0)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// The libghostty-vt version this was built against.
pub fn version() -> &'static str {
    unsafe { std::ffi::CStr::from_ptr(ghostty_vt_sys::ghostty_version()).to_str().unwrap_or("?") }
}
