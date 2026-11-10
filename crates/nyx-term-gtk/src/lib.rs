//! `TerminalView`: a GTK4 widget that draws a [`nyx_term::Terminal`].
//!
//! The view owns the terminal state. The host feeds it program output with
//! [`TerminalView::feed`] and receives the bytes meant for the program
//! (keys, mouse reports, pastes, query replies) through
//! [`TerminalView::connect_input`]. The view never touches a pty itself, so
//! the same widget draws a local shell, a session owned by a daemon, or a
//! read-only replay.
//!
//! A read-only view sends nothing: no keys, no query replies, no resizes. It
//! draws the terminal at the grid it is given and still allows selecting,
//! copying and searching.

mod draw;
mod links;
mod sprites;
mod view;

pub use view::{Stats, TerminalView, ViewStyle};
