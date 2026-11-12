//! Source text as labels: on one line, without comments, capped in length.

use crate::source::collapse_ws;

/// Longest label kept in the IR; the view may shorten further.
const MAX_LABEL: usize = 160;

/// One line without comments, whitespace collapsed, `;` dropped, length capped.
pub(crate) fn one_line(s: &str) -> String {
    let text = collapse_ws(&strip_comments(s));
    let text = text.trim_end_matches(';').trim_end();
    if text.chars().count() > MAX_LABEL {
        let mut cut: String = text.chars().take(MAX_LABEL - 1).collect();
        cut.push('…');
        cut
    } else {
        text.to_string()
    }
}

/// `s` cut to `max` characters, the last one being `…` if something is missing.
pub(crate) fn short(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max - 1).chain(['…']).collect()
    }
}

/// Remove `//` and `/* */` comments outside string and char literals.
fn strip_comments(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        // Literals are copied unchanged, whatever looks like a comment inside.
        let literal_end = match c {
            '/' if next == Some('/') => {
                i = line_comment_end(&chars, i);
                continue;
            }
            '/' if next == Some('*') => {
                i = block_comment_end(&chars, i);
                out.push(' ');
                continue;
            }
            '"' => Some(string_end(&chars, i)),
            'r' if (next == Some('"') || next == Some('#')) && !ends_in_identifier(&out) => {
                raw_string_end(&chars, i)
            }
            '\'' => Some(char_literal_end(&chars, i)),
            _ => None,
        };
        match literal_end {
            Some(end) => {
                out.extend(&chars[i..end.min(chars.len())]);
                i = end;
            }
            None => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Whether the text so far ends inside an identifier (then an `r` continues
/// it and does not start a raw string).
fn ends_in_identifier(out: &str) -> bool {
    out.chars()
        .last()
        .is_some_and(|p| p.is_alphanumeric() || p == '_')
}

/// Index of the line break that ends the `//` comment at `i`.
fn line_comment_end(chars: &[char], mut i: usize) -> usize {
    while i < chars.len() && chars[i] != '\n' {
        i += 1;
    }
    i
}

/// Index behind the `/* */` comment that starts at `i`.
fn block_comment_end(chars: &[char], i: usize) -> usize {
    let mut i = i + 2;
    while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
        i += 1;
    }
    i + 2
}

/// Index behind the string literal that starts at `i`.
fn string_end(chars: &[char], i: usize) -> usize {
    let mut i = i + 1;
    while i < chars.len() && chars[i] != '"' {
        if chars[i] == '\\' {
            i += 1;
        }
        i += 1;
    }
    i + 1
}

/// Index behind the raw string (`r"..."`, `r#"..."#`) that starts at `i`;
/// `None` if the `r` does not start one.
fn raw_string_end(chars: &[char], i: usize) -> Option<usize> {
    let mut j = i + 1;
    let mut hashes = 0;
    while chars.get(j) == Some(&'#') {
        hashes += 1;
        j += 1;
    }
    if chars.get(j) != Some(&'"') {
        return None;
    }
    j += 1;
    while j < chars.len() {
        if chars[j] == '"' && (0..hashes).all(|k| chars.get(j + 1 + k) == Some(&'#')) {
            return Some(j + 1 + hashes);
        }
        j += 1;
    }
    Some(j)
}

/// Index behind the char literal (`'x'`, `'\n'`, `'\u{..}'`) that starts at
/// `i`, or behind the quote alone if it starts a lifetime (`'a`).
fn char_literal_end(chars: &[char], i: usize) -> usize {
    let closing = if chars.get(i + 1) == Some(&'\\') {
        (i + 2..chars.len()).find(|&k| chars[k] == '\'')
    } else if chars.get(i + 2) == Some(&'\'') {
        Some(i + 2)
    } else {
        None
    };
    closing.map_or(i + 1, |e| e + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_are_stripped_outside_literals() {
        assert_eq!(strip_comments("a // x\nb"), "a \nb");
        assert_eq!(strip_comments("a /* x */ b"), "a   b");
        assert_eq!(
            strip_comments(r#"f("// no", '/', 'a) // yes"#),
            r#"f("// no", '/', 'a) "#
        );
        assert_eq!(
            strip_comments(r##"r#"/* raw */"# // c"##),
            r##"r#"/* raw */"# "##
        );
        assert_eq!(strip_comments(r#"'\'' // c"#), r#"'\'' "#);
    }

    #[test]
    fn one_line_caps_length() {
        let long = "x".repeat(400);
        assert_eq!(one_line(&long).chars().count(), MAX_LABEL);
        assert_eq!(one_line("foo(\n  a,\n  b,\n);"), "foo(a, b)");
    }
}
