use riven_core::lexer::token::Span;

/// Stores the byte offset of each line start for fast position lookups.
pub struct LineIndex {
    /// line_starts[i] = byte offset where line i begins (0-indexed)
    line_starts: Vec<u32>,
    /// The full source text (needed for UTF-16 conversion)
    source: String,
}

impl LineIndex {
    pub fn new(source: &str) -> Self {
        let mut line_starts = vec![0u32];
        for (i, byte) in source.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push((i + 1) as u32);
            }
        }
        Self {
            line_starts,
            source: source.to_string(),
        }
    }

    /// Convert a byte offset to an LSP Position.
    pub fn position_of(&self, byte_offset: usize) -> lsp_types::Position {
        let byte_offset = byte_offset.min(self.source.len());
        let line = self
            .line_starts
            .partition_point(|&start| (start as usize) <= byte_offset)
            .saturating_sub(1);
        let line_start = self.line_starts[line] as usize;

        // Convert byte column to UTF-16 column
        let end = byte_offset.min(self.source.len());
        let line_text = &self.source[line_start..end];
        let utf16_col = line_text.encode_utf16().count();

        lsp_types::Position {
            line: line as u32,
            character: utf16_col as u32,
        }
    }

    /// Convert an LSP Position to a byte offset.
    pub fn byte_offset_of(&self, position: lsp_types::Position) -> usize {
        let line = position.line as usize;
        if line >= self.line_starts.len() {
            return self.source.len();
        }
        let line_start = self.line_starts[line] as usize;

        let line_end = self
            .line_starts
            .get(line + 1)
            .map(|&s| s as usize)
            .unwrap_or(self.source.len());
        let line_text = &self.source[line_start..line_end];

        let mut utf16_count = 0u32;
        let mut byte_offset = line_start;
        for ch in line_text.chars() {
            if utf16_count >= position.character {
                break;
            }
            utf16_count += ch.len_utf16() as u32;
            byte_offset += ch.len_utf8();
        }
        byte_offset
    }

    /// Convert a Riven Span to an LSP Range.
    pub fn span_to_range(&self, span: &Span) -> lsp_types::Range {
        lsp_types::Range {
            start: self.position_of(span.start),
            end: self.position_of(span.end),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_line() {
        let idx = LineIndex::new("hello");
        let pos = idx.position_of(3);
        assert_eq!(pos.line, 0);
        assert_eq!(pos.character, 3);
    }

    #[test]
    fn multi_line() {
        let idx = LineIndex::new("aaa\nbbb\nccc");
        assert_eq!(
            idx.position_of(4),
            lsp_types::Position {
                line: 1,
                character: 0
            }
        );
        assert_eq!(
            idx.position_of(8),
            lsp_types::Position {
                line: 2,
                character: 0
            }
        );
    }

    #[test]
    fn utf8_multi_byte() {
        // e-acute is 2 bytes in UTF-8, 1 code unit in UTF-16
        let idx = LineIndex::new("caf\u{00e9}!");
        let pos = idx.position_of(5);
        assert_eq!(pos.line, 0);
        assert_eq!(pos.character, 4);
    }

    #[test]
    fn emoji_utf16_surrogate_pair() {
        // Emoji is 4 bytes in UTF-8, 2 code units in UTF-16
        let idx = LineIndex::new("a\u{1F600}b");
        let pos = idx.position_of(5);
        assert_eq!(pos.line, 0);
        assert_eq!(pos.character, 3);
    }

    #[test]
    fn roundtrip_byte_offset() {
        let idx = LineIndex::new("let x = 42\nlet y = 99\n");
        let pos = idx.position_of(11); // '\n' at end of first line
        let offset = idx.byte_offset_of(pos);
        assert_eq!(offset, 11);
    }

    #[test]
    fn empty_source() {
        let idx = LineIndex::new("");
        let pos = idx.position_of(0);
        assert_eq!(pos.line, 0);
        assert_eq!(pos.character, 0);
    }

    #[test]
    fn span_to_range() {
        let idx = LineIndex::new("let x = 42\nlet y = 99\n");
        let span = Span {
            start: 4,
            end: 5,
            line: 1,
            column: 5,
        };
        let range = idx.span_to_range(&span);
        assert_eq!(
            range.start,
            lsp_types::Position {
                line: 0,
                character: 4
            }
        );
        assert_eq!(
            range.end,
            lsp_types::Position {
                line: 0,
                character: 5
            }
        );
    }

    #[test]
    fn byte_offset_of_second_line() {
        let idx = LineIndex::new("let x = 42\nlet y = 99\n");
        let offset = idx.byte_offset_of(lsp_types::Position {
            line: 1,
            character: 4,
        });
        // line 1 starts at byte 11, char 4 = byte 15
        assert_eq!(offset, 15);
    }

    #[test]
    fn byte_offset_zero_is_line_zero_col_zero() {
        let idx = LineIndex::new("hello\nworld");
        let pos = idx.position_of(0);
        assert_eq!(pos.line, 0);
        assert_eq!(pos.character, 0);
    }

    #[test]
    fn end_of_first_line() {
        let idx = LineIndex::new("abcd\nefgh");
        // Position at end of "abcd" (byte 4, which is the '\n')
        let pos = idx.position_of(4);
        assert_eq!(pos.line, 0);
        assert_eq!(pos.character, 4);
    }

    #[test]
    fn crlf_line_endings_treat_cr_as_char() {
        // "\r\n" — the LineIndex only breaks on '\n', so '\r' is part of the line.
        // This documents observed behavior.
        let idx = LineIndex::new("ab\r\ncd");
        // byte 3 is '\n' — that's still on line 0
        let pos = idx.position_of(2);
        assert_eq!(pos.line, 0);
        // After '\n' (byte 4), we are on line 1
        let pos = idx.position_of(4);
        assert_eq!(pos.line, 1);
        assert_eq!(pos.character, 0);
    }

    #[test]
    fn out_of_range_byte_offset_saturates() {
        let src = "abc";
        let idx = LineIndex::new(src);
        // Any offset beyond source length should saturate to end
        let pos = idx.position_of(1000);
        assert_eq!(pos.line, 0);
        assert_eq!(pos.character as usize, src.chars().count());
    }

    #[test]
    fn byte_offset_of_line_past_eof_returns_end() {
        let src = "abc\ndef";
        let idx = LineIndex::new(src);
        let pos = lsp_types::Position {
            line: 100,
            character: 0,
        };
        let offset = idx.byte_offset_of(pos);
        assert_eq!(offset, src.len());
    }

    #[test]
    fn roundtrip_multiple_lines() {
        let src = "aaa\nbbb\nccc\nddd\n";
        let idx = LineIndex::new(src);
        for b in [0, 3, 4, 7, 8, 11, 12, 15] {
            let pos = idx.position_of(b);
            let back = idx.byte_offset_of(pos);
            assert_eq!(back, b, "Round trip failed for byte {}", b);
        }
    }

    #[test]
    fn roundtrip_utf8_multibyte() {
        // "é" is 2 bytes in UTF-8
        let src = "aé\nb";
        let idx = LineIndex::new(src);
        // Start of 'é' is byte 1
        let pos = idx.position_of(1);
        let back = idx.byte_offset_of(pos);
        assert_eq!(back, 1);
        // After 'é' is byte 3
        let pos = idx.position_of(3);
        let back = idx.byte_offset_of(pos);
        assert_eq!(back, 3);
    }

    #[test]
    fn position_of_second_line_first_char() {
        let idx = LineIndex::new("abc\ndef");
        let pos = idx.position_of(4);
        assert_eq!(pos.line, 1);
        assert_eq!(pos.character, 0);
    }

    #[test]
    fn span_multiline_covers_correct_range() {
        let src = "abc\ndef\nghi";
        let idx = LineIndex::new(src);
        // Span covers from byte 2 (line 0, col 2) to byte 9 (line 2, col 1)
        let span = Span {
            start: 2,
            end: 9,
            line: 0,
            column: 2,
        };
        let range = idx.span_to_range(&span);
        assert_eq!(range.start.line, 0);
        assert_eq!(range.start.character, 2);
        assert_eq!(range.end.line, 2);
        assert_eq!(range.end.character, 1);
    }

    #[test]
    fn empty_line_handled() {
        let idx = LineIndex::new("a\n\nb");
        // Line 1 is empty
        let pos = idx.position_of(2);
        assert_eq!(pos.line, 1);
        assert_eq!(pos.character, 0);
        // Line 2 is "b"
        let pos = idx.position_of(3);
        assert_eq!(pos.line, 2);
        assert_eq!(pos.character, 0);
    }

    #[test]
    fn emoji_byte_offset_roundtrip() {
        // Emoji is 4 bytes, 2 UTF-16 code units
        let src = "a\u{1F600}b";
        let idx = LineIndex::new(src);
        // Byte 1 is start of emoji
        let pos = idx.position_of(1);
        let back = idx.byte_offset_of(pos);
        assert_eq!(back, 1);
        // Byte 5 is start of 'b'
        let pos = idx.position_of(5);
        let back = idx.byte_offset_of(pos);
        assert_eq!(back, 5);
    }

    #[test]
    fn byte_offset_of_char_exceeds_line_length_saturates() {
        let idx = LineIndex::new("abc\ndef");
        // Ask for character 100 on line 0 — should not go beyond '\n' (byte 3)
        let offset = idx.byte_offset_of(lsp_types::Position {
            line: 0,
            character: 100,
        });
        assert!(
            offset <= 4,
            "Expected offset not to overshoot, got {}",
            offset
        );
    }

    #[test]
    fn final_line_without_trailing_newline() {
        let src = "line1\nline2";
        let idx = LineIndex::new(src);
        let pos = idx.position_of(src.len());
        assert_eq!(pos.line, 1);
        assert_eq!(pos.character, 5);
    }

    #[test]
    fn trailing_newline_gives_empty_final_line() {
        let src = "abc\n";
        let idx = LineIndex::new(src);
        // Byte 4 (past '\n') is on line 1, col 0
        let pos = idx.position_of(4);
        assert_eq!(pos.line, 1);
        assert_eq!(pos.character, 0);
    }
}
