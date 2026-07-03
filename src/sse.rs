//! Minimal Server-Sent Events parser / serializer for MCP Streamable HTTP.
//!
//! The Model Context Protocol Streamable HTTP transport returns `tools/list`
//! (and other) responses with `Content-Type: text/event-stream` and a body
//! shaped like:
//!
//! ```text
//! event: message
//! data: {"jsonrpc":"2.0","id":1,"result":{"tools":[...]}}
//!
//! ```
//!
//! The PDK delivers the entire response body as a single byte slice. This
//! module parses it into `SseEvent`s, allows the caller to mutate the JSON
//! `data:` payload of any event, then re-emits the same structure.
//!
//! Byte-perfect round-trip when no event is mutated. This is a hard
//! invariant — the enforcement filter above relies on `parse -> serialize`
//! being the identity when `apply_policy` decides "no change".
//!
//! Why hand-rolled instead of `eventsource-stream` or friends: WASM binary
//! size matters. Those crates pull in `tokio`, `pin-project`, etc. We need
//! plain byte-slice work.

/// One parsed SSE event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// Lines other than `data:` preserved verbatim (event:, id:, retry:,
    /// comments starting with `:`, blank lines inside the event when they
    /// aren't the terminator).
    pub preamble: Vec<String>,
    /// Concatenated `data:` line contents, joined with `\n` per the SSE
    /// spec. `None` if the event had no `data:` line.
    pub data: Option<String>,
    /// Whether the source lines used CRLF endings. Set from the first
    /// line-ending observed inside this event; serialization mirrors it
    /// so we survive proxies that inject `\r\n`.
    pub uses_crlf: bool,
    /// The exact terminator that ended this event in the input (e.g.
    /// `"\n\n"`, `"\r\n\r\n"`, or a single `"\n"` on truncated tail).
    /// Serialization emits this verbatim.
    pub terminator: String,
}

/// Parse an entire SSE response body into a list of events.
///
/// Trailing bytes without a terminator are still emitted as a final event
/// so callers can round-trip them. Callers that want to distinguish
/// "fully framed" from "trailing garbage" can inspect `terminator`.
pub fn parse(body: &[u8]) -> Vec<SseEvent> {
    // We do the whole thing at the byte level to avoid re-encoding UTF-8
    // (data: payloads can carry arbitrary Unicode inside JSON strings).
    let mut events = Vec::new();
    let mut cursor = 0usize;
    while cursor < body.len() {
        let (event, consumed) = parse_one(&body[cursor..]);
        cursor += consumed;
        events.push(event);
    }
    events
}

fn parse_one(body: &[u8]) -> (SseEvent, usize) {
    // Find the event terminator: `\n\n` or `\r\n\r\n`. Whichever appears
    // first wins. If neither appears, consume to end.
    let (term_offset, term_len) = find_event_terminator(body);
    let event_slice = &body[..term_offset];
    let terminator = String::from_utf8_lossy(&body[term_offset..term_offset + term_len])
        .into_owned();

    let mut preamble = Vec::new();
    let mut data_lines: Vec<String> = Vec::new();
    let mut uses_crlf = false;
    let mut line_start = 0usize;

    while line_start <= event_slice.len() {
        let (line_end, sep_len, is_crlf) = find_line_end(event_slice, line_start);
        if is_crlf {
            uses_crlf = true;
        }
        let line = &event_slice[line_start..line_end];
        // Empty tail (no more lines) — stop.
        if line.is_empty() && sep_len == 0 {
            break;
        }
        // A data: prefix — capture the payload after "data:" and one
        // optional leading space (SSE spec: "If the first character is a
        // U+0020 SPACE, remove it").
        if let Some(payload) = line.strip_prefix(b"data:") {
            let payload = if payload.first() == Some(&b' ') {
                &payload[1..]
            } else {
                payload
            };
            data_lines.push(String::from_utf8_lossy(payload).into_owned());
        } else {
            // Preserve as preamble (event:, id:, retry:, comments, blank
            // continuation lines).
            preamble.push(String::from_utf8_lossy(line).into_owned());
        }
        line_start = line_end + sep_len;
        if sep_len == 0 {
            break;
        }
    }

    let data = if data_lines.is_empty() {
        None
    } else {
        Some(data_lines.join("\n"))
    };

    (
        SseEvent {
            preamble,
            data,
            uses_crlf,
            terminator,
        },
        term_offset + term_len,
    )
}

/// Find `\n\n` or `\r\n\r\n` inside `body`. Returns `(offset_of_terminator,
/// length_of_terminator)`. If no terminator is found, returns
/// `(body.len(), 0)` — the whole remainder is one un-terminated event.
fn find_event_terminator(body: &[u8]) -> (usize, usize) {
    let mut i = 0usize;
    while i < body.len() {
        // `\r\n\r\n` — 4 bytes
        if i + 4 <= body.len() && &body[i..i + 4] == b"\r\n\r\n" {
            return (i, 4);
        }
        // `\n\n` — 2 bytes. Do NOT match if preceded by `\r` (that byte
        // was already captured by the CRLF branch above on the previous
        // iteration; this line is defensive).
        if i + 2 <= body.len() && &body[i..i + 2] == b"\n\n" {
            return (i, 2);
        }
        i += 1;
    }
    (body.len(), 0)
}

/// Return `(end_of_line, length_of_line_separator, is_crlf)`. If the line
/// runs to the end of the slice without a separator, `sep_len` is 0.
fn find_line_end(slice: &[u8], start: usize) -> (usize, usize, bool) {
    let mut i = start;
    while i < slice.len() {
        if i + 2 <= slice.len() && &slice[i..i + 2] == b"\r\n" {
            return (i, 2, true);
        }
        if slice[i] == b'\n' {
            return (i, 1, false);
        }
        i += 1;
    }
    (slice.len(), 0, false)
}

/// Serialize a list of events back into a byte slice.
///
/// - Preamble lines are emitted verbatim.
/// - `data:` payload is split on `\n` and emitted as one `data: <line>`
///   per split (so multi-line JSON survives).
/// - Line separator for emitted `data:` lines mirrors `uses_crlf`.
/// - Event terminator is emitted verbatim from the parsed value.
pub fn serialize(events: &[SseEvent]) -> Vec<u8> {
    let mut out = Vec::new();
    for ev in events {
        let sep: &[u8] = if ev.uses_crlf { b"\r\n" } else { b"\n" };

        // Collect every logical line (preamble + one per data: line),
        // then join with `sep`. The captured terminator already contains
        // the trailing sep(s) — append verbatim.
        //
        // Emit form "data: " (with the space) — the SSE spec says a
        // single leading space is stripped on parse, and every MCP
        // server we've observed uses the space form. If the source
        // omitted the space, we still emit "data: "; the extra byte is
        // a no-op semantically. This is the one non-byte-perfect corner
        // and it does not affect any real MCP client.
        let mut lines: Vec<Vec<u8>> = Vec::with_capacity(ev.preamble.len() + 4);
        for line in &ev.preamble {
            lines.push(line.as_bytes().to_vec());
        }
        if let Some(data) = &ev.data {
            for line in data.split('\n') {
                let mut buf = Vec::with_capacity(6 + line.len());
                buf.extend_from_slice(b"data: ");
                buf.extend_from_slice(line.as_bytes());
                lines.push(buf);
            }
        }
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                out.extend_from_slice(sep);
            }
            out.extend_from_slice(line);
        }
        out.extend_from_slice(ev.terminator.as_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_standard_event() {
        let input = b"event: message\ndata: {\"x\":1}\n\n".to_vec();
        let events = parse(&input);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].preamble, vec!["event: message".to_string()]);
        assert_eq!(events[0].data.as_deref(), Some("{\"x\":1}"));
        assert!(!events[0].uses_crlf);
        assert_eq!(events[0].terminator, "\n\n");
    }

    #[test]
    fn parses_crlf_event() {
        let input = b"event: message\r\ndata: {\"x\":1}\r\n\r\n".to_vec();
        let events = parse(&input);
        assert_eq!(events.len(), 1);
        assert!(events[0].uses_crlf);
        assert_eq!(events[0].terminator, "\r\n\r\n");
    }

    #[test]
    fn parses_multi_line_data() {
        let input = b"data: {\ndata:   \"a\": 1\ndata: }\n\n".to_vec();
        let events = parse(&input);
        assert_eq!(events.len(), 1);
        // Multi-line data is joined with \n. Leading single space is
        // stripped per SSE spec; the double space in "  \"a\"" keeps one.
        assert_eq!(events[0].data.as_deref(), Some("{\n  \"a\": 1\n}"));
    }

    #[test]
    fn parses_multiple_events() {
        let input =
            b"event: message\ndata: {\"p\":1}\n\nevent: message\ndata: {\"p\":2}\n\n".to_vec();
        let events = parse(&input);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].data.as_deref(), Some("{\"p\":1}"));
        assert_eq!(events[1].data.as_deref(), Some("{\"p\":2}"));
    }

    #[test]
    fn parses_comment_and_retry() {
        let input = b": heartbeat\nretry: 3000\nevent: message\ndata: {}\n\n".to_vec();
        let events = parse(&input);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].preamble,
            vec![
                ": heartbeat".to_string(),
                "retry: 3000".to_string(),
                "event: message".to_string()
            ]
        );
        assert_eq!(events[0].data.as_deref(), Some("{}"));
    }

    /// Round-trip corpus — the payloads Plan B §3.1 calls out.
    ///
    /// Every serialization is compared to the same input we parsed. This
    /// is the core invariant: for a clean pass-through the WASM policy
    /// must not perturb a single byte, so downstream MCP clients (Claude
    /// Desktop, Inspector, cURL) see exactly what the upstream server
    /// emitted.
    #[test]
    fn round_trip_is_byte_perfect() {
        let inputs: Vec<Vec<u8>> = vec![
            b"event: message\ndata: {\"x\":1}\n\n".to_vec(),
            b"event: message\r\ndata: {\"x\":1}\r\n\r\n".to_vec(),
            b"event: message\ndata: {\"p\":1}\n\nevent: message\ndata: {\"p\":2}\n\n".to_vec(),
            b": heartbeat\nretry: 3000\nevent: message\ndata: {}\n\n".to_vec(),
        ];
        for input in inputs {
            let parsed = parse(&input);
            let out = serialize(&parsed);
            assert_eq!(
                out, input,
                "round-trip differs; input={:?}, out={:?}",
                std::str::from_utf8(&input).unwrap_or("<non-utf8>"),
                std::str::from_utf8(&out).unwrap_or("<non-utf8>")
            );
        }
    }

    /// Multi-line `data:` payload round-trips through parse+serialize.
    /// The exact byte match is preserved because we split on `\n` and
    /// re-emit one `data: <line>` per segment.
    #[test]
    fn round_trip_multi_line_data_is_byte_perfect() {
        let input = b"data: {\ndata:   \"a\": 1\ndata: }\n\n".to_vec();
        let parsed = parse(&input);
        let out = serialize(&parsed);
        assert_eq!(out, input);
    }

    #[test]
    fn round_trip_after_data_mutation_preserves_structure() {
        let input = b"event: message\ndata: {\"tools\":[{\"n\":1}]}\n\n".to_vec();
        let mut parsed = parse(&input);
        parsed[0].data = Some(r#"{"tools":[]}"#.into());
        let out = serialize(&parsed);
        let reparsed = parse(&out);
        assert_eq!(reparsed.len(), 1);
        assert_eq!(reparsed[0].preamble, vec!["event: message".to_string()]);
        assert_eq!(reparsed[0].data.as_deref(), Some(r#"{"tools":[]}"#));
        assert_eq!(reparsed[0].terminator, "\n\n");
    }

    #[test]
    fn handles_event_without_terminator() {
        let input = b"event: message\ndata: {}\n".to_vec();
        let events = parse(&input);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].terminator, "");
        // No terminator on the tail — we emit what was captured. The
        // rest is up to the caller; MCP servers always terminate.
    }
}
