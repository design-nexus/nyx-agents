//! Theming: the shared Nyx theme engine, and terminal colours that follow it.

use gtk::{gdk, pango};
use nyx_term::{Colors, Rgb};
use nyx_term_gtk::ViewStyle;

use super::prefs;

pub fn install() {
    nyx_theme::use_design_2();
    nyx_theme::install(nyx_theme::Style {
        window_class: "agents-window",
        base_skip: &[],
        css: include_str!("style.css"),
        no_glow_css: "",
        extra_colors: None,
        prefs: || {
            let p = prefs::get();
            nyx_theme::ThemePrefs { mode: p.mode, theme: p.theme, glow: p.glow, reduce_motion: p.reduce_motion }
        },
        set_theme: Some(|mode, theme| {
            prefs::update(|p| {
                p.mode = mode;
                p.theme = theme;
            })
        }),
    });
}

/// The 16 ANSI colours from Omarchy's current theme, when it has one.
fn omarchy_ansi() -> Option<[Rgb; 16]> {
    let text = std::fs::read_to_string(nyx_core::paths::omarchy_colors()).ok()?;
    let table: toml::Table = toml::from_str(&text).ok()?;
    let get = |k: &str| table.get(k).and_then(|v| v.as_str()).and_then(Rgb::parse_hex);
    // Omarchy names its colours; terminals number them.
    let names = [
        ("color0", "lighter_background"),
        ("color1", "red"),
        ("color2", "green"),
        ("color3", "yellow"),
        ("color4", "blue"),
        ("color5", "magenta"),
        ("color6", "cyan"),
        ("color7", "light_foreground"),
        ("color8", "muted"),
        ("color9", "bright_red"),
        ("color10", "bright_green"),
        ("color11", "bright_yellow"),
        ("color12", "bright_blue"),
        ("color13", "bright_magenta"),
        ("color14", "bright_cyan"),
        ("color15", "bright_foreground"),
    ];
    let mut out = Colors::catppuccin_mocha().ansi;
    let mut found = 0;
    for (i, (numbered, named)) in names.iter().enumerate() {
        if let Some(c) = get(numbered).or_else(|| get(named)) {
            out[i] = c;
            found += 1;
        }
    }
    (found >= 8).then_some(out)
}

fn rgba(c: &str, alpha: f32) -> gdk::RGBA {
    let mut c = gdk::RGBA::parse(c).unwrap_or(gdk::RGBA::new(0.5, 0.5, 0.5, 1.0));
    c.set_alpha(alpha);
    c
}

/// How terminals look under the current theme.
pub fn view_style() -> ViewStyle {
    let p = nyx_theme::palette();
    let mut colors = Colors::catppuccin_mocha();
    if let Some(bg) = Rgb::parse_hex(&p.bg) {
        colors.background = bg;
    }
    if let Some(fg) = Rgb::parse_hex(&p.text) {
        colors.foreground = fg;
    }
    colors.cursor = Rgb::parse_hex(&p.accent).or(colors.cursor);
    if let Some(ansi) = omarchy_ansi() {
        colors.ansi = ansi;
    }
    let font =
        gtk::gio::prelude::SettingsExt::string(&gtk::gio::Settings::new("org.gnome.desktop.interface"), "monospace-font-name");
    ViewStyle {
        colors,
        font: pango::FontDescription::from_string(if font.is_empty() { "monospace 11" } else { font.as_str() }),
        selection: rgba(&p.accent, 0.32),
        search: rgba(&p.highlight, 0.35),
        search_current: rgba(&p.accent, 0.55),
        padding: 8.0,
    }
}
