//! Key names, combos and the keymaps the virtual keyboard sends.
//!
//! Like `wtype`, the keyboard has no fixed layout: each keymap lists exactly
//! the keysyms needed, one per keycode, and modifiers are sent as state rather
//! than as key presses.

/// Real modifier bits, as the compositor's xkb state numbers them.
pub const SHIFT: u32 = 1;
pub const CTRL: u32 = 4;
pub const ALT: u32 = 8;
pub const SUPER: u32 = 64;

/// A key combo: modifiers held while `key` (an xkb keysym name) is pressed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Combo {
    pub mods: u32,
    pub key: String,
}

fn modifier(name: &str) -> Option<(u32, &'static str)> {
    Some(match name {
        "ctrl" | "control" | "control_l" | "control_r" => (CTRL, "Control_L"),
        "shift" | "shift_l" | "shift_r" => (SHIFT, "Shift_L"),
        "alt" | "alt_l" | "alt_r" | "meta" | "option" => (ALT, "Alt_L"),
        "super" | "super_l" | "super_r" | "cmd" | "command" | "win" | "logo" => (SUPER, "Super_L"),
        _ => return None,
    })
}

/// The keysym name for one key: aliases (`enter`, `esc`, `pgup`...), xkb
/// names as they are (`Return`, `F5`, `KP_Enter`), or a single character.
pub fn key_name(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let alias = match lower.as_str() {
        "enter" | "return" => "Return",
        "esc" | "escape" => "Escape",
        "tab" => "Tab",
        "backspace" | "back_space" => "BackSpace",
        "delete" | "del" => "Delete",
        "insert" | "ins" => "Insert",
        "space" => "space",
        "up" | "arrowup" => "Up",
        "down" | "arrowdown" => "Down",
        "left" | "arrowleft" => "Left",
        "right" | "arrowright" => "Right",
        "home" => "Home",
        "end" => "End",
        "pageup" | "page_up" | "pgup" | "prior" => "Page_Up",
        "pagedown" | "page_down" | "pgdn" | "next" => "Page_Down",
        "menu" => "Menu",
        "print" | "printscreen" => "Print",
        "capslock" | "caps_lock" => "Caps_Lock",
        _ => "",
    };
    if !alias.is_empty() {
        return Some(alias.into());
    }
    if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u8>().ok())
        && (1..=24).contains(&n)
    {
        return Some(format!("F{n}"));
    }
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.clone().next()) {
        return char_keysym(c);
    }
    // Anything else must look like an xkb keysym name.
    (name.len() <= 32 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')).then(|| name.to_string())
}

/// The keysym name typing `c` produces.
pub fn char_keysym(c: char) -> Option<String> {
    match c {
        '\n' | '\r' => Some("Return".into()),
        '\t' => Some("Tab".into()),
        ' ' => Some("space".into()),
        'a'..='z' | 'A'..='Z' | '0'..='9' => Some(c.to_string()),
        c if c.is_control() => None,
        c => Some(format!("U{:04X}", c as u32)),
    }
}

/// Parse `ctrl+shift+t`, `Return`, `alt+F4` or `super`. A combo of only
/// modifiers presses the last one.
pub fn parse_combo(combo: &str) -> Result<Combo, String> {
    let parts: Vec<&str> = if combo == "+" { vec!["+"] } else { combo.split('+').collect() };
    if parts.iter().any(|p| p.is_empty()) && combo != "+" {
        // "ctrl++" means ctrl and the plus key.
        if let Some(rest) = combo.strip_suffix("++") {
            return parse_combo(&format!("{rest}+plus"));
        }
        return Err(format!("not a key combo: {combo}"));
    }
    let mut mods = 0;
    let mut key = None;
    for (i, p) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        match modifier(&p.to_ascii_lowercase()) {
            Some((bit, sym)) => {
                mods |= bit;
                if last {
                    key = Some(sym.to_string());
                }
            }
            None if last => key = Some(key_name(p).ok_or_else(|| format!("unknown key: {p}"))?),
            None => return Err(format!("unknown modifier: {p}")),
        }
    }
    Ok(Combo { mods, key: key.ok_or_else(|| format!("not a key combo: {combo}"))? })
}

/// An xkb keymap with one keycode per keysym: `syms[i]` is evdev code `i + 1`.
pub fn keymap(syms: &[String]) -> String {
    let mut s = String::from("xkb_keymap {\nxkb_keycodes \"nyx\" {\nminimum = 8;\n");
    s.push_str(&format!("maximum = {};\n", syms.len() + 9));
    for i in 0..syms.len() {
        s.push_str(&format!("<K{}> = {};\n", i + 1, i + 9));
    }
    s.push_str("};\nxkb_types \"nyx\" { include \"complete\" };\nxkb_compatibility \"nyx\" { include \"complete\" };\n");
    s.push_str("xkb_symbols \"nyx\" {\n");
    for (i, sym) in syms.iter().enumerate() {
        s.push_str(&format!("key <K{}> {{[ {sym} ]}};\n", i + 1));
    }
    s.push_str("};\n};\n");
    s
}

/// The keysyms one keymap holds, added to as text needs new ones.
#[derive(Default)]
pub struct Layout {
    pub syms: Vec<String>,
}

/// More than this and the keymap is rebuilt from scratch.
const MAX_SYMS: usize = 200;

impl Layout {
    /// Make sure every keysym in `need` has a keycode. True when the keymap
    /// changed and has to be sent again.
    pub fn ensure(&mut self, need: &[String]) -> bool {
        let missing: Vec<&String> = need.iter().filter(|n| !self.syms.contains(n)).collect();
        if missing.is_empty() {
            return false;
        }
        if self.syms.len() + missing.len() > MAX_SYMS {
            self.syms.clear();
            for n in need {
                if !self.syms.contains(n) {
                    self.syms.push(n.clone());
                }
            }
        } else {
            for n in missing {
                if !self.syms.contains(n) {
                    self.syms.push(n.clone());
                }
            }
        }
        true
    }

    /// The evdev code for a keysym already in the keymap.
    pub fn code(&self, sym: &str) -> Option<u32> {
        self.syms.iter().position(|s| s == sym).map(|i| i as u32 + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combos_parse() {
        assert_eq!(parse_combo("ctrl+shift+t").unwrap(), Combo { mods: CTRL | SHIFT, key: "t".into() });
        assert_eq!(parse_combo("Return").unwrap(), Combo { mods: 0, key: "Return".into() });
        assert_eq!(parse_combo("alt+F4").unwrap(), Combo { mods: ALT, key: "F4".into() });
        assert_eq!(parse_combo("super").unwrap(), Combo { mods: SUPER, key: "Super_L".into() });
        assert_eq!(parse_combo("ctrl+-").unwrap(), Combo { mods: CTRL, key: "U002D".into() });
        assert_eq!(parse_combo("ctrl++").unwrap(), Combo { mods: CTRL, key: "plus".into() });
        assert_eq!(parse_combo("cmd+enter").unwrap(), Combo { mods: SUPER, key: "Return".into() });
        assert_eq!(parse_combo("KP_Enter").unwrap().key, "KP_Enter");
        assert!(parse_combo("hyper+x").is_err());
        assert!(parse_combo("ctrl+").is_err());
        assert!(parse_combo("not a key").is_err());
    }

    #[test]
    fn characters_map_to_keysyms() {
        assert_eq!(char_keysym('a').as_deref(), Some("a"));
        assert_eq!(char_keysym('Q').as_deref(), Some("Q"));
        assert_eq!(char_keysym('\n').as_deref(), Some("Return"));
        assert_eq!(char_keysym('.').as_deref(), Some("U002E"));
        assert_eq!(char_keysym('€').as_deref(), Some("U20AC"));
        assert_eq!(char_keysym('\u{7}'), None);
    }

    #[test]
    fn keymaps_list_each_keysym_once() {
        let syms = vec!["a".to_string(), "Return".to_string()];
        let k = keymap(&syms);
        assert!(k.contains("maximum = 11;"), "{k}");
        assert!(k.contains("<K1> = 9;") && k.contains("<K2> = 10;"), "{k}");
        assert!(k.contains("key <K2> {[ Return ]};"), "{k}");
        assert!(k.contains("include \"complete\""));
    }

    #[test]
    fn layouts_grow_then_reset() {
        let mut l = Layout::default();
        assert!(l.ensure(&["a".into(), "b".into()]));
        assert!(!l.ensure(&["b".into()]));
        assert_eq!(l.code("b"), Some(2));
        assert!(l.ensure(&["c".into()]));
        assert_eq!(l.code("c"), Some(3));
        let many: Vec<String> = (0..MAX_SYMS).map(|i| format!("U{:04X}", 0x400 + i)).collect();
        assert!(l.ensure(&many));
        assert_eq!(l.syms.len(), MAX_SYMS);
        assert_eq!(l.code("a"), None);
    }
}
