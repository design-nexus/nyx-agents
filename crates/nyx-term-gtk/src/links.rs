//! Finding links in plain terminal text.

/// Schemes a click may open. Anything else in program output is ignored.
const SCHEMES: &[&str] = &["https://", "http://", "file://", "mailto:"];

/// The URL covering character column `col` of `line`, if any.
pub fn url_at(line: &str, col: usize) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();
    if col >= chars.len() {
        return None;
    }
    let is_url_char = |c: char| !c.is_whitespace() && !matches!(c, '"' | '\'' | '<' | '>' | '`' | '|' | '\u{0}'..='\u{1f}');
    let mut start = col;
    while start > 0 && is_url_char(chars[start - 1]) {
        start -= 1;
    }
    let mut end = col;
    while end < chars.len() && is_url_char(chars[end]) {
        end += 1;
    }
    let word: String = chars[start..end].iter().collect();
    let begin = SCHEMES.iter().filter_map(|s| word.find(s)).min()?;
    // The scheme must start at or before the clicked column.
    if start + word[..begin].chars().count() > col {
        return None;
    }
    let mut url = &word[begin..];
    // Trailing punctuation belongs to the sentence, not the link.
    while let Some(last) = url.chars().last() {
        let unbalanced_paren = last == ')' && url.matches('(').count() < url.matches(')').count();
        if matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | ']' | '}') || unbalanced_paren {
            url = &url[..url.len() - last.len_utf8()];
        } else {
            break;
        }
    }
    let scheme_len = SCHEMES.iter().find(|s| url.starts_with(**s)).map(|s| s.len())?;
    (url.len() > scheme_len).then(|| url.to_string())
}

/// Whether a link from program output is safe to hand to the desktop.
pub fn allowed(uri: &str) -> bool {
    SCHEMES.iter().any(|s| uri.starts_with(s)) && !uri.chars().any(|c| c.is_control())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_urls_and_trims_punctuation() {
        let line = "see https://example.com/a(b). ok";
        assert_eq!(url_at(line, 8).as_deref(), Some("https://example.com/a(b)"));
        assert_eq!(url_at(line, 4).as_deref(), Some("https://example.com/a(b)"));
        assert_eq!(url_at(line, 1), None);
        assert_eq!(url_at(line, 31), None);
        assert_eq!(url_at("(http://x.org/y)", 3).as_deref(), Some("http://x.org/y"));
        assert_eq!(url_at("javascript:alert(1)", 3), None);
    }

    #[test]
    fn only_known_schemes_open() {
        assert!(allowed("https://a.b"));
        assert!(!allowed("javascript:alert(1)"));
        assert!(!allowed("https://a.b/\u{7}"));
    }
}
