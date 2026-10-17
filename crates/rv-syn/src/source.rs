//! Reading and parsing source files, converting spans.

use std::path::{Path, PathBuf};

use rv_ir::{FileId, LineCol, Span};

/// `canonicalize` on Windows yields `\\?\C:\...`, which `code --goto` and
/// humans do not like.
pub fn strip_verbatim(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => path.to_path_buf(),
    }
}

pub fn is_skipped_dir(e: &walkdir::DirEntry) -> bool {
    if !e.file_type().is_dir() || e.depth() == 0 {
        return false;
    }
    let name = e.file_name().to_string_lossy();
    name == "target" || name.starts_with('.') || name == "node_modules"
}

pub struct Parsed {
    pub text: String,
    pub result: Result<syn::File, syn::Error>,
    /// Bytes stripped before parsing (UTF-8 BOM).
    base: u32,
}

/// Parse `text`, keeping byte offsets valid for the original text.
pub fn parse(text: String) -> Parsed {
    let base = if text.starts_with('\u{feff}') { 3 } else { 0 };
    let body = &text[base as usize..];
    // syn removes a shebang line itself and thereby shifts all offsets.
    // Blank it out instead.
    let result = if body.starts_with("#!") && !body.starts_with("#![") {
        let end = body.find('\n').unwrap_or(body.len());
        let blanked = format!("{}{}", " ".repeat(end), &body[end..]);
        syn::parse_file(&blanked)
    } else {
        syn::parse_file(body)
    };
    Parsed { text, result, base }
}

impl Parsed {
    pub fn span(&self, file: FileId, s: proc_macro2::Span) -> Span {
        let r = s.byte_range();
        let (start, end) = (s.start(), s.end());
        Span {
            file,
            bytes: self.base + r.start as u32..self.base + r.end as u32,
            start: LineCol {
                line: start.line as u32,
                col: start.column as u32,
            },
            end: LineCol {
                line: end.line as u32,
                col: end.column as u32,
            },
        }
    }

    /// Source text of a syntax node, whitespace collapsed.
    pub fn text_of(&self, s: proc_macro2::Span) -> String {
        let r = s.byte_range();
        let raw = self
            .text
            .get(self.base as usize + r.start..self.base as usize + r.end)
            .unwrap_or("");
        collapse_ws(raw)
    }
}

/// Collapse whitespace runs to single spaces and drop the spaces (and the
/// trailing comma) that multi-line formatting leaves inside brackets.
pub fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for word in s.split_whitespace() {
        let closes = word.starts_with([')', ']', '>', '}']);
        if closes && out.ends_with(',') {
            out.pop();
        }
        if !out.is_empty() && !out.ends_with(['(', '[', '<']) && !(closes && !word.starts_with('}'))
        {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

pub fn read_lossy(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::collapse_ws;

    #[test]
    fn collapse() {
        assert_eq!(
            collapse_ws(
                "fn f(
    a: u8,
    b: Vec<
  u8,
 >,
) -> X"
            ),
            "fn f(a: u8, b: Vec<u8>) -> X"
        );
        assert_eq!(collapse_ws("impl  Foo { }"), "impl Foo { }");
        assert_eq!(collapse_ws("f(a, b)"), "f(a, b)");
    }
}
