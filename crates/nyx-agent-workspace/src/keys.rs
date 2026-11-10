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

/// Where a keysym sits on a US keyboard: its evdev code, and whether Shift is
/// held for it. Programs that read physical keys (terminals, games) see real
/// keys this way.
pub fn us_key(sym: &str) -> Option<(u32, bool)> {
    const LETTERS: &[u8; 26] = b"qwertyuiopasdfghjklzxcvbnm";
    const LETTER_CODES: [u32; 26] =
        [16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 30, 31, 32, 33, 34, 35, 36, 37, 38, 44, 45, 46, 47, 48, 49, 50];
    let named = match sym {
        "Escape" => 1,
        "BackSpace" => 14,
        "Tab" => 15,
        "Return" => 28,
        "Control_L" => 29,
        "Shift_L" => 42,
        "Alt_L" => 56,
        "space" => 57,
        "Caps_Lock" => 58,
        "KP_Enter" => 96,
        "Print" => 99,
        "Home" => 102,
        "Up" => 103,
        "Page_Up" => 104,
        "Left" => 105,
        "Right" => 106,
        "End" => 107,
        "Down" => 108,
        "Page_Down" => 109,
        "Insert" => 110,
        "Delete" => 111,
        "Super_L" => 125,
        "Menu" => 127,
        "F11" => 87,
        "F12" => 88,
        _ => 0,
    };
    if named != 0 {
        return Some((named, false));
    }
    if let Some(n) = sym.strip_prefix('F').and_then(|n| n.parse::<u32>().ok())
        && (1..=10).contains(&n)
    {
        return Some((58 + n, false));
    }
    let c = match sym {
        "plus" => '+',
        "minus" => '-',
        _ => {
            let mut chars = sym.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => c,
                _ => sym.strip_prefix('U').and_then(|h| u32::from_str_radix(h, 16).ok()).and_then(char::from_u32)?,
            }
        }
    };
    if c.is_ascii_alphabetic() {
        let i = LETTERS.iter().position(|l| *l == c.to_ascii_lowercase() as u8)?;
        return Some((LETTER_CODES[i], c.is_ascii_uppercase()));
    }
    Some(match c {
        '1'..='9' => (c as u32 - '1' as u32 + 2, false),
        '0' => (11, false),
        '!' => (2, true),
        '@' => (3, true),
        '#' => (4, true),
        '$' => (5, true),
        '%' => (6, true),
        '^' => (7, true),
        '&' => (8, true),
        '*' => (9, true),
        '(' => (10, true),
        ')' => (11, true),
        '-' => (12, false),
        '_' => (12, true),
        '=' => (13, false),
        '+' => (13, true),
        '[' => (26, false),
        '{' => (26, true),
        ']' => (27, false),
        '}' => (27, true),
        ';' => (39, false),
        ':' => (39, true),
        '\'' => (40, false),
        '"' => (40, true),
        '`' => (41, false),
        '~' => (41, true),
        '\\' => (43, false),
        '|' => (43, true),
        ',' => (51, false),
        '<' => (51, true),
        '.' => (52, false),
        '>' => (52, true),
        '/' => (53, false),
        '?' => (53, true),
        _ => return None,
    })
}

/// The first evdev code for keysyms a US keyboard doesn't have, and how many there are.
const EXTRA_FIRST: u32 = 200;
pub const EXTRA_CODES: usize = 48;

/// A US keymap, plus `extra` keysyms on spare keycodes (`extra[i]` is evdev
/// code `EXTRA_FIRST + i`).
pub fn keymap(extra: &[String]) -> String {
    let mut codes = String::new();
    let mut syms = String::new();
    for (i, sym) in extra.iter().enumerate() {
        codes.push_str(&format!("<NX{i:02}> = {};\n", EXTRA_FIRST as usize + i + 8));
        syms.push_str(&format!("key <NX{i:02}> {{[ {sym} ]}};\n"));
    }
    format!(
        "xkb_keymap {{\n\
         xkb_keycodes \"nyx\" {{ include \"evdev+aliases(qwerty)\"\n{codes}}};\n\
         xkb_types \"nyx\" {{ include \"complete\" }};\n\
         xkb_compatibility \"nyx\" {{ include \"complete\" }};\n\
         xkb_symbols \"nyx\" {{ include \"pc+us+inet(evdev)\"\n{syms}}};\n\
         }};\n"
    )
}

/// The keysyms on spare keycodes, added to as text needs new ones.
#[derive(Default)]
pub struct Layout {
    pub syms: Vec<String>,
}

impl Layout {
    /// Make sure every keysym in `need` has a spare keycode (at most
    /// `EXTRA_CODES` at once). True when the keymap changed and has to be sent again.
    pub fn ensure(&mut self, need: &[String]) -> bool {
        let missing: Vec<&String> = need.iter().filter(|n| !self.syms.contains(n)).collect();
        if missing.is_empty() {
            return false;
        }
        if self.syms.len() + missing.len() > EXTRA_CODES {
            self.syms.clear();
        }
        for n in need {
            if !self.syms.contains(n) && self.syms.len() < EXTRA_CODES {
                self.syms.push(n.clone());
            }
        }
        true
    }

    /// The evdev code for a keysym on a spare keycode.
    pub fn code(&self, sym: &str) -> Option<u32> {
        self.syms.iter().position(|s| s == sym).map(|i| EXTRA_FIRST + i as u32)
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
    fn keys_sit_where_a_us_keyboard_has_them() {
        assert_eq!(us_key("Return"), Some((28, false)));
        assert_eq!(us_key("Escape"), Some((1, false)));
        assert_eq!(us_key("a"), Some((30, false)));
        assert_eq!(us_key("M"), Some((50, true)));
        assert_eq!(us_key("l"), Some((38, false)));
        assert_eq!(us_key("0"), Some((11, false)));
        assert_eq!(us_key("U002D"), Some((12, false)));
        assert_eq!(us_key("U003F"), Some((53, true)));
        assert_eq!(us_key("plus"), Some((13, true)));
        assert_eq!(us_key("F5"), Some((63, false)));
        assert_eq!(us_key("F12"), Some((88, false)));
        assert_eq!(us_key("U20AC"), None);
    }

    #[test]
    fn keymaps_are_us_plus_extras() {
        let k = keymap(&["U20AC".to_string()]);
        assert!(k.contains("include \"pc+us+inet(evdev)\""), "{k}");
        assert!(k.contains("include \"evdev+aliases(qwerty)\""), "{k}");
        assert!(k.contains("<NX00> = 208;"), "{k}");
        assert!(k.contains("key <NX00> {[ U20AC ]};"), "{k}");
        assert!(!keymap(&[]).contains("<NX"));
    }

    #[test]
    fn layouts_grow_then_reset() {
        let mut l = Layout::default();
        assert!(l.ensure(&["U20AC".into(), "U00E9".into()]));
        assert!(!l.ensure(&["U00E9".into()]));
        assert_eq!(l.code("U00E9"), Some(201));
        let many: Vec<String> = (0..EXTRA_CODES).map(|i| format!("U{:04X}", 0x400 + i)).collect();
        assert!(l.ensure(&many));
        assert_eq!(l.syms.len(), EXTRA_CODES);
        assert_eq!(l.code("U20AC"), None);
    }
}
