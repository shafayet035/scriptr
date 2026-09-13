//! PTY plumbing: spawning, the blocking reader thread, the 16 ms / 8 KB
//! batcher, the per-script ring buffer and output fan-out, and the ANSI
//! stripping log matcher used by `log` gates.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use regex::Regex;
use tokio::sync::{mpsc, oneshot, watch};

use crate::lock;

pub const RING_MAX_LINES: usize = 5_000;
pub const RING_MAX_BYTES: usize = 2 * 1024 * 1024;
const READ_BUF: usize = 16 * 1024;
const FLUSH_BYTES: usize = 8 * 1024;
const FLUSH_INTERVAL: Duration = Duration::from_millis(16);
/// Rolling window of ANSI-stripped text the log gate matches against.
const LOG_WINDOW_BYTES: usize = 64 * 1024;
pub const DEFAULT_SIZE: (u16, u16) = (120, 32);

// ---- ring buffer ----------------------------------------------------------

/// Byte ring buffer capped by line count and size; trims whole lines from the
/// front so a snapshot never starts mid-line (unless a single line exceeds
/// the byte cap).
pub struct RingBuffer {
    buf: VecDeque<u8>,
    newlines: usize,
    max_lines: usize,
    max_bytes: usize,
}

impl RingBuffer {
    pub fn new(max_lines: usize, max_bytes: usize) -> Self {
        Self { buf: VecDeque::new(), newlines: 0, max_lines, max_bytes }
    }

    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend(data);
        self.newlines += data.iter().filter(|b| **b == b'\n').count();
        self.trim();
    }

    fn trim(&mut self) {
        let excess_lines = self.newlines.saturating_sub(self.max_lines);
        let excess_bytes = self.buf.len().saturating_sub(self.max_bytes);
        if excess_lines == 0 && excess_bytes == 0 {
            return;
        }
        // Earliest line boundary that drops enough lines *and* bytes.
        let mut seen = 0;
        let boundary = self.buf.iter().enumerate().find_map(|(i, b)| {
            if *b == b'\n' {
                seen += 1;
                if seen >= excess_lines && i + 1 >= excess_bytes {
                    return Some((i + 1, seen));
                }
            }
            None
        });
        // No such boundary means the tail is one oversized line: cut bytes exactly.
        let (cut, lines) = boundary.unwrap_or_else(|| {
            (excess_bytes, self.buf.range(..excess_bytes).filter(|b| **b == b'\n').count())
        });
        self.buf.drain(..cut);
        self.newlines -= lines;
    }

    pub fn snapshot(&self) -> Vec<u8> {
        self.buf.iter().copied().collect()
    }
}

// ---- output fan-out ---------------------------------------------------------

/// Receives output bytes; returns false when the receiver is gone.
pub type DataSink = Box<dyn Fn(Vec<u8>) -> bool + Send + Sync>;

struct OutputInner {
    ring: RingBuffer,
    sink: Option<DataSink>,
}

/// Per-script output: the ring buffer plus the currently attached frontend
/// sink. Survives restarts of the script.
pub struct ScriptOutput {
    inner: Mutex<OutputInner>,
    size: Mutex<(u16, u16)>,
}

impl Default for ScriptOutput {
    fn default() -> Self {
        Self {
            inner: Mutex::new(OutputInner {
                ring: RingBuffer::new(RING_MAX_LINES, RING_MAX_BYTES),
                sink: None,
            }),
            size: Mutex::new(DEFAULT_SIZE),
        }
    }
}

impl ScriptOutput {
    /// Appends to the ring buffer and forwards to the attached sink.
    pub fn write(&self, data: &[u8]) {
        let mut inner = lock(&self.inner);
        inner.ring.push(data);
        if let Some(sink) = &inner.sink {
            if !sink(data.to_vec()) {
                inner.sink = None;
            }
        }
    }

    /// Replaces the attached sink. The snapshot is sent first while holding
    /// the lock so no live bytes can slip in between.
    pub fn attach(&self, sink: DataSink) {
        let mut inner = lock(&self.inner);
        let alive = sink(inner.ring.snapshot());
        inner.sink = alive.then_some(sink);
    }

    pub fn size(&self) -> (u16, u16) {
        *lock(&self.size)
    }

    pub fn set_size(&self, cols: u16, rows: u16) {
        *lock(&self.size) = (cols.max(1), rows.max(1));
    }
}

// ---- ANSI stripping & log matching ------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum AnsiState {
    Text,
    Esc,
    Csi,
    /// OSC/DCS/APC string; `true` once an ESC (possible ST) was seen.
    Str(bool),
}

/// Streaming ANSI escape stripper; state carries across batch boundaries.
pub struct AnsiStripper {
    state: AnsiState,
}

impl Default for AnsiStripper {
    fn default() -> Self {
        Self { state: AnsiState::Text }
    }
}

impl AnsiStripper {
    pub fn strip(&mut self, input: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(input.len());
        for &b in input {
            self.state = match (self.state, b) {
                (AnsiState::Text, 0x1b) => AnsiState::Esc,
                (AnsiState::Text, b'\r') => AnsiState::Text,
                (AnsiState::Text, _) => {
                    out.push(b);
                    AnsiState::Text
                }
                (AnsiState::Esc, b'[') => AnsiState::Csi,
                (AnsiState::Esc, b']' | b'P' | b'_' | b'^') => AnsiState::Str(false),
                // Two-byte escapes (ESC 7, ESC =, ...).
                (AnsiState::Esc, _) => AnsiState::Text,
                (AnsiState::Csi, 0x40..=0x7e) => AnsiState::Text,
                (AnsiState::Csi, _) => AnsiState::Csi,
                (AnsiState::Str(_), 0x07) => AnsiState::Text,
                (AnsiState::Str(true), b'\\') => AnsiState::Text,
                (AnsiState::Str(_), 0x1b) => AnsiState::Str(true),
                (AnsiState::Str(_), _) => AnsiState::Str(false),
            };
        }
        out
    }
}

enum Pattern {
    Regex(Regex),
    Literal(String),
}

/// Matches a `log` gate pattern against ANSI-stripped output since start.
/// Publishes `true` on its watch channel once matched.
pub struct LogMatcher {
    pattern: Pattern,
    stripper: AnsiStripper,
    window: String,
    matched: watch::Sender<bool>,
}

impl LogMatcher {
    /// Falls back to a literal substring when `pattern` isn't a valid regex.
    pub fn new(pattern: &str) -> (Self, watch::Receiver<bool>) {
        let pattern = match Regex::new(pattern) {
            Ok(r) => Pattern::Regex(r),
            Err(_) => Pattern::Literal(pattern.to_string()),
        };
        let (tx, rx) = watch::channel(false);
        (Self { pattern, stripper: AnsiStripper::default(), window: String::new(), matched: tx }, rx)
    }

    pub fn feed(&mut self, data: &[u8]) {
        if *self.matched.borrow() {
            return;
        }
        let text = self.stripper.strip(data);
        self.window.push_str(&String::from_utf8_lossy(&text));
        if self.window.len() > LOG_WINDOW_BYTES {
            // Keep the newest half; cut on a char boundary.
            let mut cut = self.window.len() - LOG_WINDOW_BYTES / 2;
            while !self.window.is_char_boundary(cut) {
                cut += 1;
            }
            self.window.drain(..cut);
        }
        let hit = match &self.pattern {
            Pattern::Regex(r) => r.is_match(&self.window),
            Pattern::Literal(l) => self.window.contains(l.as_str()),
        };
        if hit {
            self.matched.send_replace(true);
            self.window = String::new();
        }
    }
}

// ---- spawning ----------------------------------------------------------------

pub struct SpawnSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub size: (u16, u16),
}

/// Handles to a live PTY process that outlive the spawn call.
pub struct PtyIo {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
}

impl PtyIo {
    pub fn write(&self, data: &[u8]) -> Result<(), String> {
        let mut w = lock(&self.writer);
        w.write_all(data).and_then(|_| w.flush()).map_err(|e| e.to_string())
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        lock(&self.master)
            .resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| e.to_string())
    }
}

/// The child's process group. On Unix portable-pty calls `setsid()` in the
/// child, so the pgid equals the pid and signalling the group reaches every
/// descendant that didn't start its own session.
pub struct ProcessGroup {
    pid: u32,
    #[cfg(windows)]
    killer: Mutex<Box<dyn portable_pty::ChildKiller + Send + Sync>>,
}

impl ProcessGroup {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    #[cfg(unix)]
    fn signal(&self, signal: Option<nix::sys::signal::Signal>) -> bool {
        use nix::unistd::Pid;
        // pgid 0/1 would target our own group or init.
        match i32::try_from(self.pid) {
            Ok(pid) if pid > 1 => nix::sys::signal::killpg(Pid::from_raw(pid), signal).is_ok(),
            _ => false,
        }
    }

    /// Polite stop: SIGTERM to the whole group.
    pub fn terminate(&self) {
        #[cfg(unix)]
        self.signal(Some(nix::sys::signal::Signal::SIGTERM));
        // TODO(windows): assign children to a Job Object and send CTRL_BREAK
        // for a graceful stop; for now terminate the direct child only.
        #[cfg(windows)]
        let _ = lock(&self.killer).kill();
    }

    pub fn kill(&self) {
        #[cfg(unix)]
        self.signal(Some(nix::sys::signal::Signal::SIGKILL));
        #[cfg(windows)]
        let _ = lock(&self.killer).kill();
    }

    /// True while any member of the group exists.
    pub fn alive(&self) -> bool {
        #[cfg(unix)]
        return self.signal(None);
        #[cfg(windows)]
        return false;
    }
}

pub struct Spawned {
    pub group: ProcessGroup,
    pub io: Arc<PtyIo>,
    /// Resolves with the exit code once the child is reaped (signal → `None`).
    pub exited: oneshot::Receiver<Option<i32>>,
    /// Resolves when all output has been read and flushed.
    pub drained: oneshot::Receiver<()>,
}

/// Spawns `spec` in a fresh PTY. Output batches are passed to `on_batch`
/// (from a tokio task) until EOF. Must be called inside a tokio runtime.
pub fn spawn<F>(spec: SpawnSpec, mut on_batch: F) -> Result<Spawned, String>
where
    F: FnMut(&[u8]) + Send + 'static,
{
    let (cols, rows) = spec.size;
    let pair = native_pty_system()
        .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| format!("openpty: {e}"))?;

    let mut cmd = CommandBuilder::new(&spec.program);
    cmd.args(&spec.args);
    cmd.cwd(&spec.cwd);
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }
    let child = pair.slave.spawn_command(cmd).map_err(|e| format!("spawn {}: {e}", spec.program))?;
    // Close our copy of the slave so reads hit EOF once the child side closes.
    drop(pair.slave);

    let pid = child.process_id().ok_or("spawned process has no pid")?;
    let reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
    let group = ProcessGroup {
        pid,
        #[cfg(windows)]
        killer: Mutex::new(child.clone_killer()),
    };

    let (exit_tx, exited) = oneshot::channel();
    wait_child(child, exit_tx);

    let (chunk_tx, chunk_rx) = mpsc::channel::<Vec<u8>>(64);
    std::thread::Builder::new()
        .name(format!("pty-read-{pid}"))
        .spawn(move || read_loop(reader, chunk_tx))
        .map_err(|e| e.to_string())?;

    let (drain_tx, drained) = oneshot::channel();
    tokio::spawn(async move {
        batch_loop(chunk_rx, &mut on_batch).await;
        let _ = drain_tx.send(());
    });

    Ok(Spawned {
        group,
        io: Arc::new(PtyIo { master: Mutex::new(pair.master), writer: Mutex::new(writer) }),
        exited,
        drained,
    })
}

fn wait_child(mut child: Box<dyn Child + Send + Sync>, tx: oneshot::Sender<Option<i32>>) {
    tokio::task::spawn_blocking(move || {
        let code = match child.wait() {
            // portable-pty reports signal deaths as exit code 1 with a signal
            // name; surface those as `None` like std does.
            Ok(status) if status.signal().is_some() => None,
            Ok(status) => Some(status.exit_code() as i32),
            Err(_) => None,
        };
        let _ = tx.send(code);
    });
}

/// Blocking reads on the PTY master; the bounded channel applies backpressure
/// to the child when the UI can't keep up.
fn read_loop(mut reader: Box<dyn Read + Send>, tx: mpsc::Sender<Vec<u8>>) {
    let mut buf = vec![0u8; READ_BUF];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if tx.blocking_send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            // EIO when the slave side closes on macOS/Linux.
            Err(_) => break,
        }
    }
}

/// Coalesces chunks: flush 16 ms after the first byte of a batch, or as soon
/// as 8 KB are buffered, whichever comes first.
async fn batch_loop<F: FnMut(&[u8])>(mut rx: mpsc::Receiver<Vec<u8>>, on_batch: &mut F) {
    let mut batch = Vec::with_capacity(FLUSH_BYTES * 2);
    while let Some(first) = rx.recv().await {
        batch.extend_from_slice(&first);
        let deadline = tokio::time::Instant::now() + FLUSH_INTERVAL;
        let mut closed = false;
        while batch.len() < FLUSH_BYTES {
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Some(chunk)) => batch.extend_from_slice(&chunk),
                Ok(None) => {
                    closed = true;
                    break;
                }
                Err(_) => break,
            }
        }
        on_batch(&batch);
        batch.clear();
        if closed {
            break;
        }
    }
}

/// Splits a shell setting like `/bin/zsh -lc` into program + leading args.
/// Falls back to `$SHELL -lc` when the configured program doesn't exist.
pub fn shell_command(shell: &str, cmd: &str) -> (String, Vec<String>) {
    let mut parts: Vec<String> = shell.split_whitespace().map(String::from).collect();
    if parts.is_empty() {
        parts = crate::model::default_shell().split_whitespace().map(String::from).collect();
    }
    #[cfg(unix)]
    if parts[0].starts_with('/') && !std::path::Path::new(&parts[0]).exists() {
        if let Ok(sh) = std::env::var("SHELL") {
            parts = vec![sh, "-lc".into()];
        }
    }
    let program = parts.remove(0);
    parts.push(cmd.to_string());
    (program, parts)
}

/// Parses `KEY=VALUE` lines (optional `export ` prefix, quotes, `#` comments).
pub fn parse_env_file(text: &str) -> Vec<(String, String)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let l = l.strip_prefix("export ").unwrap_or(l);
            let (k, v) = l.split_once('=')?;
            let v = v.trim();
            let v = v
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .or_else(|| v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
                .unwrap_or(v);
            Some((k.trim().to_string(), v.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_trims_by_lines_at_boundaries() {
        let mut r = RingBuffer::new(3, 1_000);
        r.push(b"a\nb\nc\nd\ne");
        assert_eq!(r.snapshot(), b"b\nc\nd\ne");
        r.push(b"\nf\n");
        assert_eq!(r.snapshot(), b"d\ne\nf\n");
    }

    #[test]
    fn ring_trims_by_bytes_at_line_boundary() {
        let mut r = RingBuffer::new(100, 10);
        r.push(b"12345\n6789\nab\n");
        // 14 bytes; must drop >= 4 → cut after first line.
        assert_eq!(r.snapshot(), b"6789\nab\n");
        assert!(r.snapshot().len() <= 10);
    }

    #[test]
    fn ring_handles_one_giant_line() {
        let mut r = RingBuffer::new(100, 8);
        r.push(b"0123456789abcdef");
        assert_eq!(r.snapshot(), b"89abcdef");
        r.push(b"\nxy\n");
        assert!(r.snapshot().len() <= 8);
        assert!(r.snapshot().ends_with(b"xy\n"));
    }

    #[test]
    fn ansi_strip_across_boundaries() {
        let mut s = AnsiStripper::default();
        let mut out = s.strip(b"\x1b[32mhel");
        out.extend(s.strip(b"lo\x1b"));
        out.extend(s.strip(b"[0m wor\x1b]0;title\x07ld\r\n"));
        out.extend(s.strip(b"\x1b]8;;http://x\x1b\\link"));
        assert_eq!(String::from_utf8(out).unwrap(), "hello world\nlink");
    }

    #[test]
    fn log_matcher_spans_batches_and_falls_back_to_literal() {
        let (mut m, rx) = LogMatcher::new(r"listening on :\d+");
        m.feed(b"server \x1b[1mlisten");
        assert!(!*rx.borrow());
        m.feed(b"ing\x1b[0m on :8080\r\n");
        assert!(*rx.borrow());

        let (mut lit, rx) = LogMatcher::new("ready (");
        lit.feed(b"is ready (really)");
        assert!(*rx.borrow());
    }

    #[test]
    fn shell_split_and_env_file() {
        let (p, a) = shell_command("/bin/sh -c", "echo hi");
        assert_eq!(p, "/bin/sh");
        assert_eq!(a, vec!["-c", "echo hi"]);
        let env = parse_env_file("# c\nexport A=1\nB = \"two words\"\nC='x'\nbad\n");
        assert_eq!(
            env,
            vec![("A".into(), "1".into()), ("B".into(), "two words".into()), ("C".into(), "x".into())]
        );
    }
}
