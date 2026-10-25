//! Bounded terminal output with a cursor that survives eviction and paged reads.

pub(crate) const MAX_BUFFER_CHARS: usize = 200_000;

#[derive(Default)]
pub(crate) struct OutputBuffer {
    text: String,
    chars: usize,
    cursor: usize,
}

impl OutputBuffer {
    pub fn append(&mut self, text: &str) {
        self.chars += text.chars().count();
        self.text.push_str(text);
        if self.chars > MAX_BUFFER_CHARS {
            let remove = self.chars - MAX_BUFFER_CHARS / 2;
            let bytes = self
                .text
                .char_indices()
                .nth(remove)
                .map_or(self.text.len(), |(i, _)| i);
            self.text.drain(..bytes);
            self.cursor = self.cursor.saturating_sub(bytes);
            self.chars -= remove;
        }
    }

    pub fn read(&mut self, max_chars: usize) -> String {
        let unread = &self.text[self.cursor..];
        let bytes = unread
            .char_indices()
            .nth(max_chars)
            .map_or(unread.len(), |(i, _)| i);
        let page = unread[..bytes].to_string();
        self.cursor += bytes;
        page
    }

    pub fn has_unread(&self) -> bool {
        self.cursor < self.text.len()
    }
    pub fn discard(&mut self) {
        self.cursor = self.text.len();
    }
}

/// Preserve UTF-8 code points split across pipe reads; an invalid byte must not
/// stop the reader or mark an otherwise live terminal as closed.
#[derive(Default)]
pub(crate) struct Utf8Stream {
    pending: Vec<u8>,
}

impl Utf8Stream {
    pub fn decode(&mut self, chunk: &[u8], eof: bool) -> String {
        self.pending.extend_from_slice(chunk);
        let mut text = String::new();
        let mut used = 0;
        loop {
            match std::str::from_utf8(&self.pending[used..]) {
                Ok(valid) => {
                    text.push_str(valid);
                    used = self.pending.len();
                    break;
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    text.push_str(std::str::from_utf8(&self.pending[used..used + valid]).unwrap());
                    used += valid;
                    match error.error_len() {
                        Some(invalid) => {
                            text.push('\u{fffd}');
                            used += invalid;
                        }
                        None => break,
                    }
                }
            }
        }
        self.pending.drain(..used);
        if eof && !self.pending.is_empty() {
            text.push('\u{fffd}');
            self.pending.clear();
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paging_preserves_unicode_remainder() {
        let mut buffer = OutputBuffer::default();
        buffer.append("甲乙🙂abc");
        assert_eq!(buffer.read(3), "甲乙🙂");
        assert!(buffer.has_unread());
        assert_eq!(buffer.read(99), "abc");
        assert!(!buffer.has_unread());
    }
    #[test]
    fn eviction_adjusts_the_cursor_without_replaying_old_output() {
        let mut buffer = OutputBuffer::default();
        buffer.append(&"旧".repeat(MAX_BUFFER_CHARS));
        buffer.discard();
        buffer.append("新");
        assert_eq!(buffer.read(99), "新");
    }
    #[test]
    fn a_long_line_is_bounded() {
        let mut buffer = OutputBuffer::default();
        buffer.append(&"a".repeat(MAX_BUFFER_CHARS * 3));
        assert_eq!(buffer.read(usize::MAX).len(), MAX_BUFFER_CHARS / 2);
    }
    #[test]
    fn split_and_invalid_utf8_do_not_lose_valid_text() {
        let mut stream = Utf8Stream::default();
        assert_eq!(stream.decode(&[0xe4, 0xb8], false), "");
        assert_eq!(stream.decode(&[0xad, 0xff, b'a'], false), "中�a");
        assert_eq!(stream.decode(&[0xe4], true), "�");
    }
}
