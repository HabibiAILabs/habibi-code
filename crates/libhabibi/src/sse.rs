use anyhow::{Result, ensure};

/// Incremental SSE framing: preserve UTF-8 across network chunks and join data lines.
#[derive(Default)]
pub(crate) struct Decoder {
    pending: Vec<u8>,
    data: String,
}

impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>> {
        const LIMIT: usize = 8 * 1024 * 1024;
        ensure!(
            self.pending.len() + self.data.len() + bytes.len() <= LIMIT,
            "SSE frame exceeds 8 MiB"
        );
        self.pending.extend_from_slice(bytes);
        let mut frames = Vec::new();
        let mut consumed = 0;
        while let Some(end) = self.pending[consumed..].iter().position(|b| *b == b'\n') {
            let end = consumed + end;
            let line = std::str::from_utf8(&self.pending[consumed..end])?.trim_end_matches('\r');
            if line.is_empty() {
                if !self.data.is_empty() {
                    frames.push(std::mem::take(&mut self.data));
                }
            } else if let Some(data) = line.strip_prefix("data:") {
                if !self.data.is_empty() {
                    self.data.push('\n');
                }
                self.data.push_str(data.strip_prefix(' ').unwrap_or(data));
            }
            consumed = end + 1;
        }
        self.pending.drain(..consumed);
        Ok(frames)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arbitrary_chunk_boundaries_and_multiline() {
        let bytes = "event: update\r\ndata: héllo\r\ndata: world\r\n\r\n: ping\n\ndata: next\n\n"
            .as_bytes();
        for chunk_size in 1..bytes.len() {
            let mut decoder = Decoder::default();
            let mut frames = Vec::new();
            for chunk in bytes.chunks(chunk_size) {
                frames.extend(decoder.push(chunk).unwrap());
            }
            assert_eq!(frames, ["héllo\nworld", "next"]);
        }
    }
    #[test]
    fn bounds_unterminated_frames() {
        assert!(
            Decoder::default()
                .push(&vec![b'x'; 8 * 1024 * 1024 + 1])
                .is_err()
        );
    }
}
