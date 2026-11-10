//! The app's preferences (`~/.config/nyx-agents/settings.toml`).

pub use nyx_theme::ThemeMode;
use serde::{Deserialize, Serialize};

/// An open tab: a session, and optionally a second one split beside it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TabPref {
    pub session: String,
    pub split: Option<String>,
    /// Split top/bottom rather than side by side.
    pub split_down: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub mode: ThemeMode,
    pub theme: String,
    pub reduce_motion: bool,
    pub glow: bool,
    pub sidebar_collapsed: bool,
    pub tabs: Vec<TabPref>,
    pub active: usize,
    /// Where the last new session started.
    pub last_folder: String,
    pub last_harness: String,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            mode: ThemeMode::Omarchy,
            theme: "catppuccin-mocha".into(),
            reduce_motion: false,
            glow: true,
            sidebar_collapsed: false,
            tabs: Vec::new(),
            active: 0,
            last_folder: String::new(),
            last_harness: "claude".into(),
        }
    }
}

nyx_core::prefs_store!(Prefs);
