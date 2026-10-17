use std::ops::Range;

use crate::FileId;

/// 1-based line, 0-based column counted in `char`s (as proc-macro2 reports it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LineCol {
    pub line: u32,
    pub col: u32,
}

/// A source location. Every IR element that comes from source carries one.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Span {
    pub file: FileId,
    pub bytes: Range<u32>,
    pub start: LineCol,
    pub end: LineCol,
}

impl Span {
    pub fn line_count(&self) -> u32 {
        self.end.line.saturating_sub(self.start.line) + 1
    }
}

/// Byte offsets of line starts, for byte -> line lookups.
#[derive(Clone, Debug, Default)]
pub struct LineIndex {
    starts: Vec<u32>,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(
            text.bytes()
                .enumerate()
                .filter(|&(_, b)| b == b'\n')
                .map(|(i, _)| i as u32 + 1),
        );
        LineIndex { starts }
    }

    pub fn line_count(&self) -> usize {
        self.starts.len()
    }

    /// Byte range of the 1-based `line`, without the line terminator.
    pub fn line_range(&self, text: &str, line: u32) -> Range<usize> {
        let i = (line.max(1) - 1) as usize;
        let Some(&start) = self.starts.get(i) else {
            return text.len()..text.len();
        };
        let end = self
            .starts
            .get(i + 1)
            .map_or(text.len(), |&e| e as usize - 1);
        let line_text = &text[start as usize..end];
        let end = start as usize + line_text.trim_end_matches('\r').len();
        start as usize..end
    }

    /// 1-based line containing `byte`.
    pub fn line_of(&self, byte: u32) -> u32 {
        self.starts.partition_point(|&s| s <= byte) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_index() {
        let text = "ab\r\ncd\n\nef";
        let idx = LineIndex::new(text);
        assert_eq!(idx.line_count(), 4);
        assert_eq!(&text[idx.line_range(text, 1)], "ab");
        assert_eq!(&text[idx.line_range(text, 2)], "cd");
        assert_eq!(&text[idx.line_range(text, 3)], "");
        assert_eq!(&text[idx.line_range(text, 4)], "ef");
        assert_eq!(idx.line_of(0), 1);
        assert_eq!(idx.line_of(4), 2);
        assert_eq!(idx.line_of(9), 4);
    }
}
