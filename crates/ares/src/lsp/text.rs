//! An open document's text and the conversion between byte offsets (what the
//! compiler's spans are) and LSP positions (line plus a column counted in the
//! negotiated encoding's code units).

/// How an LSP `character` counts: UTF-8 bytes, UTF-16 code units (the protocol
/// default), or Unicode scalar values.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Encoding {
    Utf8,
    Utf16,
    Utf32,
}

impl Encoding {
    pub fn name(self) -> &'static str {
        match self {
            Encoding::Utf8 => "utf-8",
            Encoding::Utf16 => "utf-16",
            Encoding::Utf32 => "utf-32",
        }
    }

    fn units(self, c: char) -> usize {
        match self {
            Encoding::Utf8 => c.len_utf8(),
            Encoding::Utf16 => c.len_utf16(),
            Encoding::Utf32 => 1,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Position {
    pub line: usize,
    pub character: usize,
}

pub struct Document {
    pub text: String,
    pub version: i64,
    line_starts: Vec<usize>,
}

impl Document {
    pub fn new(text: String, version: i64) -> Document {
        let line_starts = line_starts(&text);
        Document { text, version, line_starts }
    }

    /// The byte offset of `pos`, clamped into the document: a line past the end
    /// is the end, a column past the line's end is the line's end.
    pub fn offset(&self, pos: Position, enc: Encoding) -> usize {
        let Some(&start) = self.line_starts.get(pos.line) else {
            return self.text.len();
        };
        let end = self.line_end(pos.line);
        let mut units = 0;
        for (i, c) in self.text[start..end].char_indices() {
            if units >= pos.character {
                return start + i;
            }
            units += enc.units(c);
        }
        end
    }

    pub fn position(&self, offset: usize, enc: Encoding) -> Position {
        let offset = offset.min(self.text.len());
        let line = self.line_starts.partition_point(|&s| s <= offset).saturating_sub(1);
        let start = self.line_starts[line];
        let mut at = offset;
        while !self.text.is_char_boundary(at) {
            at -= 1;
        }
        let character = self.text[start..at].chars().map(|c| enc.units(c)).sum();
        Position { line, character }
    }

    /// Replace the text between two positions (an incremental `didChange`), or
    /// all of it when `range` is `None`.
    pub fn edit(&mut self, range: Option<(Position, Position)>, new: &str, enc: Encoding) {
        match range {
            None => self.text = new.to_string(),
            Some((a, b)) => {
                let (s, e) = (self.offset(a, enc), self.offset(b, enc));
                let (s, e) = (s.min(e), s.max(e));
                self.text.replace_range(s..e, new);
            }
        }
        self.line_starts = line_starts(&self.text);
    }

    /// The end of line `l`, before its newline (and a `\r` before that).
    fn line_end(&self, l: usize) -> usize {
        let next = self.line_starts.get(l + 1).map_or(self.text.len(), |&n| n - 1);
        let start = self.line_starts[l];
        if next > start && self.text.as_bytes().get(next - 1) == Some(&b'\r') && next < self.text.len() {
            next - 1
        } else {
            next
        }
    }
}

fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_counts_surrogate_pairs() {
        let d = Document::new("a😀b\nc".to_string(), 0);
        let b = d.text.find('b').unwrap();
        assert_eq!(d.position(b, Encoding::Utf16), Position { line: 0, character: 3 });
        assert_eq!(d.position(b, Encoding::Utf8), Position { line: 0, character: 5 });
        assert_eq!(d.position(b, Encoding::Utf32), Position { line: 0, character: 2 });
        assert_eq!(d.offset(Position { line: 0, character: 3 }, Encoding::Utf16), b);
    }

    #[test]
    fn positions_clamp_to_the_line_and_document() {
        let d = Document::new("ab\ncd".to_string(), 0);
        assert_eq!(d.offset(Position { line: 0, character: 99 }, Encoding::Utf16), 2);
        assert_eq!(d.offset(Position { line: 9, character: 0 }, Encoding::Utf16), 5);
        assert_eq!(d.position(4, Encoding::Utf8), Position { line: 1, character: 1 });
    }

    #[test]
    fn incremental_edit_replaces_a_range() {
        let mut d = Document::new("hello\nworld".to_string(), 0);
        d.edit(
            Some((Position { line: 1, character: 0 }, Position { line: 1, character: 5 })),
            "there",
            Encoding::Utf16,
        );
        assert_eq!(d.text, "hello\nthere");
        d.edit(Some((Position { line: 0, character: 5 }, Position { line: 0, character: 5 })), "!\n", Encoding::Utf16);
        assert_eq!(d.text, "hello!\n\nthere");
        assert_eq!(d.position(d.text.len(), Encoding::Utf16), Position { line: 2, character: 5 });
    }
}
