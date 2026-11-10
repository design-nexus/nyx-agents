//! Untrusted text from agents (titles, status messages) shown outside the
//! terminal: no control or invisible formatting characters, bounded length.

pub fn line(s: &str, max_chars: usize) -> String {
    let mut out = String::with_capacity(s.len().min(max_chars * 4));
    let mut n = 0;
    let mut last_space = true;
    for c in s.chars() {
        let c = if c.is_whitespace() { ' ' } else { c };
        if c.is_control() || is_invisible(c) || (c == ' ' && last_space) {
            continue;
        }
        if n == max_chars {
            out.push('…');
            break;
        }
        last_space = c == ' ';
        out.push(c);
        n += 1;
    }
    out.trim_end().to_string()
}

/// Bidirectional overrides and zero-width characters that can disguise text.
fn is_invisible(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_controls_and_bidi() {
        assert_eq!(line("a\x1b[31mb\u{202E}c\n  d", 50), "a[31mbc d");
        assert_eq!(line("abcdef", 3), "abc…");
    }
}
