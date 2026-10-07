//! Harvesting what an agent reports about itself.
//!
//! Some CLIs can emit newline-delimited JSON events (`claude -p
//! --output-format stream-json`, `opencode run --format json`). When the
//! adapter declares one of those formats we parse a *copy* of the output bytes
//! to fill `TaskRun.sessionId / turns / costUsd / tokensIn / tokensOut /
//! summary`.
//!
//! This is strictly best-effort: in slice A runs are interactive, so the bytes
//! are usually a TUI redraw and nothing parses. Unknown shapes are ignored,
//! nothing here can fail a run, and the parser never touches the byte stream
//! the UI renders.

use serde_json::Value;

use crate::model::{AgentStream, TaskRun};

/// A partial line longer than this is a redraw, not JSON; drop it.
const MAX_LINE: usize = 1024 * 1024;
/// How deep to look for a known key in an event of unknown shape.
const MAX_DEPTH: u8 = 6;
const MAX_SUMMARY: usize = 4_000;

/// What a run reported about itself. Every field stays `None` until something
/// actually parses.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Harvest {
    pub session_id: Option<String>,
    pub turns: Option<i64>,
    pub cost_usd: Option<f64>,
    pub tokens_in: Option<i64>,
    pub tokens_out: Option<i64>,
    pub summary: Option<String>,
}

impl Harvest {
    pub fn is_empty(&self) -> bool {
        *self == Harvest::default()
    }

    /// Copies the harvested fields onto a run record.
    pub fn apply(&self, run: &mut TaskRun) {
        run.session_id = self.session_id.clone().or_else(|| run.session_id.clone());
        run.turns = self.turns.or(run.turns);
        run.cost_usd = self.cost_usd.or(run.cost_usd);
        run.tokens_in = self.tokens_in.or(run.tokens_in);
        run.tokens_out = self.tokens_out.or(run.tokens_out);
        run.summary = self.summary.clone().or_else(|| run.summary.clone());
    }
}

/// Line-buffering JSON-event parser for one run.
pub struct StreamParser {
    format: AgentStream,
    /// Bytes of the line currently being assembled across batches.
    line: Vec<u8>,
    /// Assistant messages seen, used when no event reports a turn count.
    assistant_turns: i64,
    harvest: Harvest,
}

impl StreamParser {
    pub fn new(format: AgentStream) -> Self {
        Self { format, line: Vec::new(), assistant_turns: 0, harvest: Harvest::default() }
    }

    pub fn harvest(&self) -> Harvest {
        let mut h = self.harvest.clone();
        if h.turns.is_none() && self.assistant_turns > 0 {
            h.turns = Some(self.assistant_turns);
        }
        h
    }

    /// Feeds a copy of an output batch. Never fails, never mutates `data`.
    pub fn feed(&mut self, data: &[u8]) {
        if self.format == AgentStream::None {
            return;
        }
        for &byte in data {
            match byte {
                b'\n' => {
                    let line = std::mem::take(&mut self.line);
                    self.line_done(&line);
                }
                // A PTY turns "\n" into "\r\n"; JSON never contains a bare CR.
                b'\r' => {}
                _ if self.line.len() >= MAX_LINE => self.line.clear(),
                _ => self.line.push(byte),
            }
        }
    }

    /// Flushes a trailing line that never got its newline (process exited).
    pub fn finish(&mut self) {
        let line = std::mem::take(&mut self.line);
        self.line_done(&line);
    }

    fn line_done(&mut self, line: &[u8]) {
        let text = line.trim_ascii();
        // Cheap guard so TUI redraws never reach serde_json.
        if !text.starts_with(b"{") {
            return;
        }
        let Ok(event) = serde_json::from_slice::<Value>(text) else { return };
        match self.format {
            AgentStream::ClaudeJson => self.claude_event(&event),
            AgentStream::OpencodeJson => self.opencode_event(&event),
            AgentStream::CursorJson => self.cursor_event(&event),
            AgentStream::None => {}
        }
    }

    // ---- per-format handlers ---------------------------------------------

    /// `{"type":"system","subtype":"init","session_id":…}`, assistant messages
    /// with `usage`, and a final `{"type":"result",…}` carrying
    /// `total_cost_usd`, `num_turns` and `result`.
    fn claude_event(&mut self, event: &Value) {
        if let Some(id) = event.get("session_id").and_then(Value::as_str) {
            self.harvest.session_id = Some(id.to_string());
        }
        let kind = event.get("type").and_then(Value::as_str).unwrap_or_default();
        if kind == "assistant" {
            self.assistant_turns += 1;
        }
        // Usage lives on the message for assistant events and at the top level
        // for the result event; the result's numbers are the authoritative ones.
        let usage = event.get("usage").or_else(|| event.pointer("/message/usage"));
        if let Some(usage) = usage {
            let tokens = |keys: [&str; 2]| keys.iter().filter_map(|k| as_i64(usage.get(*k))).next();
            if let Some(n) = tokens(["input_tokens", "inputTokens"]) {
                self.harvest.tokens_in = Some(self.harvest.tokens_in.unwrap_or(0).max(n));
            }
            if let Some(n) = tokens(["output_tokens", "outputTokens"]) {
                self.harvest.tokens_out = Some(self.harvest.tokens_out.unwrap_or(0).max(n));
            }
        }
        if kind != "result" {
            return;
        }
        if let Some(cost) = event.get("total_cost_usd").and_then(Value::as_f64) {
            self.harvest.cost_usd = Some(cost);
        }
        if let Some(turns) = as_i64(event.get("num_turns")) {
            self.harvest.turns = Some(turns);
        }
        if let Some(text) = event.get("result").and_then(Value::as_str) {
            self.harvest.summary = Some(truncate(text));
        }
    }

    /// opencode's own event objects. The shape moves between releases, so this
    /// looks for known key names anywhere shallow in the event instead of
    /// hard-coding a path.
    fn opencode_event(&mut self, event: &Value) {
        if let Some(id) = find_str(event, &["sessionID", "session_id", "sessionId"]) {
            self.harvest.session_id = Some(id);
        }
        if let Some(cost) = find_f64(event, &["cost", "costUsd", "total_cost_usd"]) {
            // Costs are reported per step; keep the largest, which is the total.
            self.harvest.cost_usd = Some(self.harvest.cost_usd.unwrap_or(0.0).max(cost));
        }
        if let Some(tokens) = find_obj(event, &["tokens", "usage"]) {
            if let Some(n) = find_f64(tokens, &["input", "input_tokens", "inputTokens"]) {
                self.harvest.tokens_in = Some(self.harvest.tokens_in.unwrap_or(0).max(n as i64));
            }
            if let Some(n) = find_f64(tokens, &["output", "output_tokens", "outputTokens"]) {
                self.harvest.tokens_out = Some(self.harvest.tokens_out.unwrap_or(0).max(n as i64));
            }
        }
    }

    /// cursor-agent's stream-json: an init event with the chat id and a final
    /// result event. No cost is reported.
    fn cursor_event(&mut self, event: &Value) {
        if let Some(id) = find_str(event, &["session_id", "sessionId", "chatId", "chat_id"]) {
            self.harvest.session_id = Some(id);
        }
        if event.get("type").and_then(Value::as_str) == Some("assistant") {
            self.assistant_turns += 1;
        }
        if let Some(text) = event.get("result").and_then(Value::as_str) {
            self.harvest.summary = Some(truncate(text));
        }
    }
}

// ---- defensive lookups ----------------------------------------------------

/// Accepts a number or a numeric string (CLIs disagree).
fn as_i64(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn truncate(text: &str) -> String {
    if text.len() <= MAX_SUMMARY {
        return text.to_string();
    }
    let mut cut = MAX_SUMMARY;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &text[..cut])
}

/// Breadth-first hunt for the first value under any of `keys`.
fn find<'a>(value: &'a Value, keys: &[&str], want: fn(&Value) -> bool) -> Option<&'a Value> {
    let mut level = vec![value];
    for _ in 0..MAX_DEPTH {
        let mut next = Vec::new();
        for node in level {
            if let Some(map) = node.as_object() {
                for key in keys {
                    if let Some(hit) = map.get(*key).filter(|v| want(v)) {
                        return Some(hit);
                    }
                }
                next.extend(map.values());
            } else if let Some(items) = node.as_array() {
                next.extend(items);
            }
        }
        if next.is_empty() {
            return None;
        }
        level = next;
    }
    None
}

fn find_str(value: &Value, keys: &[&str]) -> Option<String> {
    find(value, keys, |v| v.as_str().is_some_and(|s| !s.is_empty()))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn find_f64(value: &Value, keys: &[&str]) -> Option<f64> {
    find(value, keys, |v| v.as_f64().is_some()).and_then(Value::as_f64)
}

fn find_obj<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    find(value, keys, Value::is_object)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-written from the shape `claude -p --output-format stream-json
    /// --verbose` emits (2.1.289).
    const CLAUDE_FIXTURE: &str = concat!(
        r#"{"type":"system","subtype":"init","cwd":"/tmp/p","session_id":"6f1c-aaaa","tools":["Bash","Edit"],"model":"claude-opus-4-1"}"#,
        "\n",
        r#"{"type":"assistant","message":{"id":"msg_1","role":"assistant","content":[{"type":"text","text":"Looking at the test."}],"usage":{"input_tokens":1200,"output_tokens":40}},"session_id":"6f1c-aaaa"}"#,
        "\n",
        r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"ok"}]},"session_id":"6f1c-aaaa"}"#,
        "\n",
        r#"{"type":"assistant","message":{"id":"msg_2","role":"assistant","content":[{"type":"text","text":"Fixed it."}],"usage":{"input_tokens":2400,"output_tokens":90}},"session_id":"6f1c-aaaa"}"#,
        "\n",
        r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":18234,"num_turns":3,"result":"Fixed the failing test in src/lib.rs.","total_cost_usd":0.0731,"session_id":"6f1c-aaaa","usage":{"input_tokens":2400,"output_tokens":132}}"#,
        "\n",
    );

    #[test]
    fn parses_claude_stream_json() {
        let mut p = StreamParser::new(AgentStream::ClaudeJson);
        p.feed(CLAUDE_FIXTURE.as_bytes());
        let h = p.harvest();
        assert_eq!(h.session_id.as_deref(), Some("6f1c-aaaa"));
        assert_eq!(h.turns, Some(3));
        assert_eq!(h.cost_usd, Some(0.0731));
        assert_eq!(h.tokens_in, Some(2400));
        assert_eq!(h.tokens_out, Some(132));
        assert_eq!(h.summary.as_deref(), Some("Fixed the failing test in src/lib.rs."));
    }

    #[test]
    fn survives_pty_crlf_and_split_batches() {
        let mut p = StreamParser::new(AgentStream::ClaudeJson);
        // A PTY rewrites "\n" as "\r\n", and batches cut lines anywhere.
        let crlf = CLAUDE_FIXTURE.replace('\n', "\r\n");
        let bytes = crlf.as_bytes();
        for chunk in bytes.chunks(17) {
            p.feed(chunk);
        }
        p.finish();
        assert_eq!(p.harvest().turns, Some(3));
        assert_eq!(p.harvest().session_id.as_deref(), Some("6f1c-aaaa"));
    }

    #[test]
    fn interactive_noise_harvests_nothing() {
        let mut p = StreamParser::new(AgentStream::ClaudeJson);
        p.feed(b"\x1b[2J\x1b[H\xe2\x95\xad welcome to Claude Code \xe2\x95\xae\r\n");
        p.feed(b"{not json at all}\r\n");
        p.feed(b"{\"type\":\"assistant\"\r\n"); // truncated
        p.feed(&[0xff, 0xfe, 0x00, b'\n']); // not even UTF-8
        p.finish();
        // A bare `{"type":"assistant"}` never arrived, so not even a turn count.
        assert!(p.harvest().is_empty(), "{:?}", p.harvest());
    }

    #[test]
    fn assistant_turns_stand_in_for_a_missing_result_event() {
        let mut p = StreamParser::new(AgentStream::ClaudeJson);
        p.feed(br#"{"type":"assistant","session_id":"s1"}"#);
        p.feed(b"\n");
        p.feed(br#"{"type":"assistant","session_id":"s1"}"#);
        p.finish();
        let h = p.harvest();
        assert_eq!((h.turns, h.session_id.as_deref()), (Some(2), Some("s1")));
        assert_eq!(h.cost_usd, None);
    }

    #[test]
    fn opencode_events_are_read_defensively() {
        let mut p = StreamParser::new(AgentStream::OpencodeJson);
        p.feed(
            concat!(
                r#"{"type":"message.updated","properties":{"info":{"id":"m1","sessionID":"ses_42","cost":0.004,"tokens":{"input":900,"output":12,"reasoning":0}}}}"#,
                "\n",
                r#"{"type":"step.finished","properties":{"info":{"sessionID":"ses_42","cost":0.012,"tokens":{"input":1800,"output":64}}}}"#,
                "\n",
                r#"{"type":"something.new.we.have.never.seen","properties":{"nested":[{"deep":true}]}}"#,
                "\n"
            )
            .as_bytes(),
        );
        let h = p.harvest();
        assert_eq!(h.session_id.as_deref(), Some("ses_42"));
        assert_eq!(h.cost_usd, Some(0.012));
        assert_eq!((h.tokens_in, h.tokens_out), (Some(1800), Some(64)));
    }

    #[test]
    fn none_format_never_parses_and_harvest_apply_keeps_known_values() {
        let mut p = StreamParser::new(AgentStream::None);
        p.feed(CLAUDE_FIXTURE.as_bytes());
        assert!(p.harvest().is_empty());

        let mut run = TaskRun::queued("r".into(), "t");
        run.session_id = Some("kept".into());
        Harvest { turns: Some(1), ..Harvest::default() }.apply(&mut run);
        assert_eq!(run.session_id.as_deref(), Some("kept"));
        assert_eq!(run.turns, Some(1));
        assert_eq!(run.cost_usd, None);
    }

    #[test]
    fn oversized_redraw_line_is_dropped_without_growing() {
        let mut p = StreamParser::new(AgentStream::ClaudeJson);
        p.feed(&vec![b'x'; MAX_LINE + 5_000]);
        assert!(p.line.len() < MAX_LINE);
        p.feed(b"\n");
        p.feed(br#"{"type":"result","num_turns":1,"session_id":"s"}"#);
        p.finish();
        assert_eq!(p.harvest().turns, Some(1));
    }
}
