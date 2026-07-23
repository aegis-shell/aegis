//! Server-sent events line parser shared by the provider implementations.
//! Handles events split arbitrarily across byte chunks, CRLF endings,
//! multi-line `data:` payloads, and `:` comment lines.

/// One complete SSE event. `data` joins multiple `data:` lines with `\n`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

/// Incremental parser: feed byte chunks, drain complete events.
#[derive(Default)]
pub(crate) struct SseParser {
    buffer: Vec<u8>,
    event: Option<String>,
    data: Vec<String>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one chunk and return every event completed by it.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        while let Some(pos) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let mut line: Vec<u8> = self.buffer.drain(..=pos).collect();
            debug_assert_eq!(line.pop(), Some(b'\n'));
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.process_line(&line, &mut events);
        }
        events
    }

    /// Flush a trailing unterminated line and any pending event at EOF.
    pub fn finish(&mut self) -> Vec<SseEvent> {
        let mut events = Vec::new();
        if !self.buffer.is_empty() {
            let line = std::mem::take(&mut self.buffer);
            self.process_line(&line, &mut events);
        }
        self.dispatch(&mut events);
        events
    }

    fn process_line(&mut self, line: &[u8], events: &mut Vec<SseEvent>) {
        if line.is_empty() {
            self.dispatch(events);
            return;
        }
        if line[0] == b':' {
            return;
        }
        if let Some(value) = field(line, b"data") {
            self.data.push(value);
        } else if let Some(value) = field(line, b"event") {
            self.event = Some(value);
        }
        // `id:` and `retry:` carry no meaning for chat streams.
    }

    fn dispatch(&mut self, events: &mut Vec<SseEvent>) {
        if self.data.is_empty() {
            self.event = None;
            return;
        }
        events.push(SseEvent {
            event: self.event.take(),
            data: self.data.join("\n"),
        });
        self.data.clear();
    }
}

/// Parse a `name: value` line, stripping one optional leading space.
fn field(line: &[u8], name: &[u8]) -> Option<String> {
    let rest = line.strip_prefix(name)?;
    let rest = rest.strip_prefix(b":")?;
    let value = rest.strip_prefix(b" ").unwrap_or(rest);
    Some(String::from_utf8_lossy(value).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_events_split_across_chunks() {
        let mut parser = SseParser::new();
        assert!(parser.feed(b"event: message_start\nda").is_empty());
        let events = parser.feed(b"ta: {\"one\":1}\n\nevent: ping\ndata: {}\n\n");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event.as_deref(), Some("message_start"));
        assert_eq!(events[0].data, "{\"one\":1}");
        assert_eq!(events[1].event.as_deref(), Some("ping"));
        assert_eq!(events[1].data, "{}");
    }

    #[test]
    fn joins_multi_line_data_and_ignores_comments() {
        let mut parser = SseParser::new();
        let events = parser.feed(b": keep-alive\ndata: first\ndata: second\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "first\nsecond");
        assert_eq!(events[0].event, None);
    }

    #[test]
    fn tolerates_crlf_and_unterminated_tail() {
        let mut parser = SseParser::new();
        assert_eq!(parser.feed(b"data: one\r\n\r\ndata: two").len(), 1);
        let events = parser.finish();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "two");
    }

    #[test]
    fn openai_done_marker_passes_through_as_data() {
        let mut parser = SseParser::new();
        let events = parser.feed(b"data: [DONE]\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "[DONE]");
    }
}
