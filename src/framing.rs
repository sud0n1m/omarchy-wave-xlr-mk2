use serde_json::Value;

#[derive(Default)]
pub struct Framer {
    frame: Vec<u8>,
    dropping: bool,
}
impl Framer {
    pub fn feed(&mut self, byte: u8) -> Option<Result<Value, &'static str>> {
        if byte == b'\n' {
            let result = if !self.dropping && !self.frame.is_empty() {
                Some(serde_json::from_slice(&self.frame).map_err(|_| "Invalid JSON command"))
            } else {
                None
            };
            self.frame.clear();
            self.dropping = false;
            result
        } else if !self.dropping {
            self.frame.push(byte);
            if self.frame.len() > 4096 {
                self.frame.clear();
                self.dropping = true;
                Some(Err("Command exceeds 4096 bytes"))
            } else {
                None
            }
        } else {
            None
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oversized_frame_resynchronizes_without_retaining_input() {
        let mut f = Framer::default();
        let mut events = vec![];
        for b in vec![b'x'; 100_000]
            .into_iter()
            .chain(b"\n{\"id\":1}\n".iter().copied())
        {
            if let Some(e) = f.feed(b) {
                events.push(e);
            }
            assert!(f.frame.len() <= 4096);
        }
        assert_eq!(events.len(), 2);
        assert!(events[0].is_err());
        assert_eq!(events[1].as_ref().unwrap()["id"], 1);
    }
    #[test]
    fn fragmented_multiple_frames_and_bad_utf8() {
        let mut f = Framer::default();
        let mut events = vec![];
        for byte in b"\n{\"id\":1}\r\n\xff\n{\"id\":2}\n{\"id\":3}" {
            if let Some(e) = f.feed(*byte) {
                events.push(e);
            }
        }
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].as_ref().unwrap()["id"], 1);
        assert!(events[1].is_err());
        assert_eq!(events[2].as_ref().unwrap()["id"], 2);
    }
    #[test]
    fn malformed_and_non_finite_json_rejected() {
        let mut f = Framer::default();
        let events: Vec<_> = b"{broken}\n{\"value\":NaN}\n{\"value\":1e999}\n"
            .iter()
            .filter_map(|b| f.feed(*b))
            .collect();
        assert_eq!(events.len(), 3);
        assert!(events.iter().all(Result::is_err));
    }
}
