//! Raw-byte output buffer and the Codex alt-screen filter.

use std::collections::VecDeque;

/// Output cap per agent (Hydra `MAX_BUFFER_CHARS`, ~2 MB). Bytes here, not UTF-16 units.
pub const MAX_BUFFER_BYTES: usize = 2_000_000;

/// Ring buffer of raw PTY bytes; the oldest bytes are dropped first.
#[derive(Debug)]
pub struct OutputBuffer {
    bytes: VecDeque<u8>,
    cap: usize,
}

impl Default for OutputBuffer {
    fn default() -> Self {
        Self::with_cap(MAX_BUFFER_BYTES)
    }
}

impl OutputBuffer {
    /// A buffer that keeps at most `cap` bytes (free terminals size it from their scrollback setting).
    pub fn with_cap(cap: usize) -> Self {
        Self {
            bytes: VecDeque::new(),
            cap: cap.max(1),
        }
    }

    pub fn push(&mut self, data: &[u8]) {
        if data.len() >= self.cap {
            self.bytes.clear();
            self.bytes.extend(&data[data.len() - self.cap..]);
            return;
        }
        self.bytes.extend(data);
        let excess = self.bytes.len().saturating_sub(self.cap);
        if excess > 0 {
            self.bytes.drain(..excess);
        }
    }

    pub fn clear(&mut self) {
        self.bytes.clear();
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// The whole buffer as contiguous bytes.
    pub fn to_vec(&self) -> Vec<u8> {
        let (a, b) = self.bytes.as_slices();
        let mut v = Vec::with_capacity(self.bytes.len());
        v.extend_from_slice(a);
        v.extend_from_slice(b);
        v
    }

    /// The last `max_bytes` bytes as text. The cut never splits a UTF-8 character
    /// (continuation bytes at the front are skipped); invalid bytes become U+FFFD.
    pub fn recent_text(&self, max_bytes: usize) -> String {
        let take = max_bytes.min(self.bytes.len());
        let mut start = self.bytes.len() - take;
        while start < self.bytes.len() && (self.bytes[start] & 0xC0) == 0x80 {
            start += 1;
        }
        let v: Vec<u8> = self.bytes.range(start..).copied().collect();
        String::from_utf8_lossy(&v).into_owned()
    }
}

const ALT_SCREEN_SEQUENCES: [&[u8]; 6] = [
    b"\x1b[?47h",
    b"\x1b[?47l",
    b"\x1b[?1047h",
    b"\x1b[?1047l",
    b"\x1b[?1049h",
    b"\x1b[?1049l",
];

/// Strips `ESC[?47h/l`, `ESC[?1047h/l` and `ESC[?1049h/l` (Codex toggles the
/// alternate screen, which would kill scrollback).
///
/// Works on bytes. A sequence split across two reads is handled by holding back
/// a trailing partial match (at most 7 bytes) until the next chunk arrives. Limit: a
/// chunk that ends in a lone `ESC` (or `ESC[`, `ESC[?4`, ...) is delayed until more
/// output (or EOF, see [`AltScreenFilter::flush`]) arrives; real programs write
/// whole escape sequences at once, so in practice this is invisible.
#[derive(Debug, Default)]
pub struct AltScreenFilter {
    held: Vec<u8>,
}

impl AltScreenFilter {
    pub fn filter(&mut self, input: &[u8]) -> Vec<u8> {
        let mut buf = std::mem::take(&mut self.held);
        buf.extend_from_slice(input);
        let mut out = Vec::with_capacity(buf.len());
        let mut i = 0;
        while i < buf.len() {
            if buf[i] != 0x1b {
                let next = buf[i..]
                    .iter()
                    .position(|&b| b == 0x1b)
                    .map_or(buf.len(), |p| i + p);
                out.extend_from_slice(&buf[i..next]);
                i = next;
                continue;
            }
            let rest = &buf[i..];
            if let Some(seq) = ALT_SCREEN_SEQUENCES.iter().find(|s| rest.starts_with(s)) {
                i += seq.len();
            } else if ALT_SCREEN_SEQUENCES
                .iter()
                .any(|s| s.len() > rest.len() && s.starts_with(rest))
            {
                self.held = rest.to_vec();
                break;
            } else {
                out.push(0x1b);
                i += 1;
            }
        }
        out
    }

    /// Bytes held back waiting for the rest of a possible sequence (call at EOF).
    pub fn flush(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.held)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_alt_screen_sequences() {
        let mut f = AltScreenFilter::default();
        assert_eq!(f.filter(b"\x1b[?1049hhello\x1b[?1049l"), b"hello");
        assert_eq!(f.filter(b"a\x1b[?47hb\x1b[?1047lc"), b"abc");
    }

    #[test]
    fn keeps_other_escapes() {
        let mut f = AltScreenFilter::default();
        assert_eq!(
            f.filter(b"\x1b[31mred\x1b[0m \x1b[?25l"),
            b"\x1b[31mred\x1b[0m \x1b[?25l"
        );
        assert_eq!(f.filter(b"\x1b[?1048h"), b"\x1b[?1048h");
    }

    #[test]
    fn handles_sequences_split_across_chunks() {
        let mut f = AltScreenFilter::default();
        assert_eq!(f.filter(b"x\x1b[?10"), b"x");
        assert_eq!(f.filter(b"49hy"), b"y");
        assert_eq!(f.filter(b"\x1b"), b"");
        assert_eq!(f.filter(b"[?47"), b"");
        assert_eq!(f.filter(b"lz"), b"z");
    }

    #[test]
    fn partial_non_matching_prefix_is_released() {
        let mut f = AltScreenFilter::default();
        assert_eq!(f.filter(b"\x1b[?10"), b"");
        assert_eq!(f.filter(b"5h"), b"\x1b[?105h");
        assert_eq!(f.filter(b"\x1b["), b"");
        assert_eq!(f.flush(), b"\x1b[");
        assert_eq!(f.flush(), b"");
    }

    #[test]
    fn buffer_caps_and_trims_from_the_front() {
        let mut b = OutputBuffer::default();
        b.push(&vec![b'a'; MAX_BUFFER_BYTES - 1]);
        b.push(b"bcd");
        assert_eq!(b.len(), MAX_BUFFER_BYTES);
        let v = b.to_vec();
        assert_eq!(&v[v.len() - 3..], b"bcd");
        b.push(&vec![b'z'; MAX_BUFFER_BYTES + 10]);
        assert_eq!(b.len(), MAX_BUFFER_BYTES);
        assert!(b.to_vec().iter().all(|&c| c == b'z'));
    }

    #[test]
    fn recent_text_never_splits_utf8() {
        let mut b = OutputBuffer::default();
        b.push("héllo wörld".as_bytes());
        assert_eq!(b.recent_text(100), "héllo wörld");
        // "ö" is 2 bytes; cutting inside it must skip the orphan continuation byte.
        assert_eq!(b.recent_text(4), "rld");
        assert_eq!(b.recent_text(5), "örld");
    }
}
