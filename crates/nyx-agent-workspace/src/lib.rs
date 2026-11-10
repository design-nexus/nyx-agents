//! Agent workspaces: each one a headless sway of its own, where an agent opens
//! GUI programs and drives them with a virtual pointer and keyboard.
//!
//! An [`Instance`] never touches the user's desktop:
//!
//! - sway runs with the headless backend and the pixman renderer, so it opens
//!   no DRM device, no login session and no other Wayland or X11 display;
//! - its environment is rebuilt from an allow-list, without the user's
//!   `WAYLAND_DISPLAY`, `DISPLAY`, Hyprland or D-Bus variables;
//! - it gets a private runtime folder as `XDG_RUNTIME_DIR` and a private D-Bus
//!   session bus, so programs inside cannot reach the user's compositor or hand
//!   their windows to a copy running on the user's desktop;
//! - a watchdog kills it at once if its log shows it opening a real backend.
//!
//! It is not a security boundary: programs inside run as the user and share
//! the user's files.
//!
//! [`Ipc`] speaks sway's i3-compatible IPC, [`Input`] sends pointer and
//! keyboard events, and [`Capturer`] copies the output into memory.

mod capture;
mod clipboard;
mod input;
mod instance;
mod ipc;
pub mod keys;
mod spawn;
mod wl;

pub use capture::{Capturer, Frame, base64, png, shrink, unbase64};
pub use clipboard::{Clipboard, TEXT_TYPES};
pub use input::{Button, Input};
pub use instance::{Instance, Options, app_env, bus_config, isolate_argv, remove_tree, sway_config, sway_env, x_display};
pub use ipc::{Ipc, Window, windows};
