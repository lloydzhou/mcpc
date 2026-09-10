/// A minimal Server-Sent Events parser that mirrors the C `agc_sse` helper.
///
/// It splits input into lines, accumulates `data:` and `event:` fields,
/// and dispatches complete events on empty lines.  Multiple `data:` lines
/// are joined with newlines.
pub struct SseParser {
    line: String,
    event: String,
    data: String,
    saw_field: bool,
    skip_lf: bool,
    error: bool,
    finished_event: Option<(String, String)>,
}

impl SseParser {
    pub fn new() -> Self {
        Self {
            line: String::new(),
            event: String::new(),
            data: String::new(),
            saw_field: false,
            skip_lf: false,
            error: false,
            finished_event: None,
        }
    }

    /// Feed more bytes into the parser.  Each time a complete event is
    /// assembled it is stored and can be retrieved with `take_event`.
    pub fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            if self.error {
                return;
            }
            if self.skip_lf {
                self.skip_lf = false;
                if b == b'\n' {
                    continue;
                }
            }
            if b == b'\r' || b == b'\n' {
                if !self.line.is_empty() {
                    self.process_field();
                } else {
                    self.dispatch();
                }
                if b == b'\r' {
                    self.skip_lf = true;
                }
            } else {
                self.line.push(b as char);
            }
        }
    }

    /// Signal end-of-stream and dispatch any pending event.
    pub fn finish(&mut self) {
        if !self.line.is_empty() {
            self.process_field();
        }
        self.dispatch();
    }

    /// Return and clear the most recently completed event, if any.
    pub fn take_event(&mut self) -> Option<(String, String)> {
        self.finished_event.take()
    }

    fn process_field(&mut self) {
        if self.error {
            return;
        }
        let line = std::mem::take(&mut self.line);
        if line.starts_with("event:") {
            self.event = line[6..].trim_start().to_string();
            self.saw_field = true;
        } else if line.starts_with("data:") {
            let value = line[5..].trim_start();
            if !self.data.is_empty() {
                self.data.push('\n');
            }
            self.data.push_str(value);
            self.saw_field = true;
        } else if line.starts_with("id:") || line.starts_with("retry:") {
            self.saw_field = true;
        } else {
            // unknown fields are ignored
        }
    }

    fn dispatch(&mut self) {
        if !self.saw_field {
            return;
        }
        let event = if self.event.is_empty() {
            "message".to_string()
        } else {
            std::mem::take(&mut self.event)
        };
        let data = std::mem::take(&mut self.data);
        self.saw_field = false;
        self.finished_event = Some((event, data));
    }
}

impl Default for SseParser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_message() {
        let mut p = SseParser::new();
        p.feed(b"event: message\ndata: hello\n\n");
        assert_eq!(p.take_event(), Some(("message".into(), "hello".into())));
    }

    #[test]
    fn test_multiline_data() {
        let mut p = SseParser::new();
        p.feed(b"data: line1\ndata: line2\n\n");
        assert_eq!(p.take_event(), Some(("message".into(), "line1\nline2".into())));
    }

    #[test]
    fn test_crlf() {
        let mut p = SseParser::new();
        p.feed(b"event: msg\r\ndata: x\r\n\r\n");
        assert_eq!(p.take_event(), Some(("msg".into(), "x".into())));
    }
}
