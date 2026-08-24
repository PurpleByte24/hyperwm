//! The Unix-domain-socket wire protocol `hyperwm-daemon` and `hyperwm-cli`
//! use for `reload`/`status` (architecture.md §1, §6; build unit 7). This
//! is the one place both ends implement request/response framing and
//! (de)serialization, so they can't drift apart -- see [`write_message`]/
//! [`read_message`]. `verify` is *not* part of this protocol: it runs
//! [`crate::load`] directly in the CLI process (see that subcommand's own
//! doc in `hyperwm-cli`), so it works without a daemon running at all.
//!
//! # Wire format
//!
//! One connection, one request, one response, then the client closes the
//! connection -- no persistent session, no multiplexing. Each message is a
//! single line: a JSON value (default *externally tagged* enum
//! representation -- no `#[serde(tag = ...)]`) followed by exactly one
//! `\n`. A JSON value never contains a raw, unescaped newline, so `\n` is
//! an unambiguous message boundary; this is why a newline-delimited format
//! was chosen over a length-prefixed one (architecture.md's unit 7
//! instructions call out either as acceptable -- this just needs no
//! separate length header to keep in sync with the payload).
//!
//! Concretely, each [`Request`] variant serializes as:
//! - `Request::Reload` → `"reload"`
//! - `Request::Status` → `"status"`
//!
//! and each [`Response`] variant as:
//! - `Response::Reloaded{..}` → `{"reloaded":{"builtin_keybinds":5,"script_keybinds":2}}`
//! - `Response::ReloadFailed{..}` → `{"reload_failed":{"error":"..."}}`
//! - `Response::Status(report)` → `{"status":{...StatusReport fields...}}`

use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The Unix domain socket the daemon listens on and the CLI connects to.
/// Under `$TMPDIR` (already per-user and mode `0700` on macOS) rather than
/// next to the config file -- this is a runtime/IPC artifact, not
/// user-editable config, so it doesn't belong under
/// `~/.config/hyperwm/`.
#[must_use]
pub fn socket_path() -> PathBuf {
    std::env::temp_dir().join("hyperwm.sock")
}

/// A CLI subcommand's request to the running daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Request {
    /// `hyperwm reload`: re-read and re-validate the config file at its
    /// default location and apply it going forward (architecture.md §6).
    Reload,
    /// `hyperwm status`: current tree/floating state, focused window, and
    /// daemon health.
    Status,
}

/// The daemon's reply to a [`Request`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Response {
    /// [`Request::Reload`] succeeded: the daemon is now running the newly
    /// loaded config.
    Reloaded { builtin_keybinds: usize, script_keybinds: usize },
    /// [`Request::Reload`]'s new config failed validation. Per
    /// architecture.md §6 this is the *only* thing that happens on a bad
    /// reload -- the daemon keeps running on the previous valid config
    /// (silently, from this response's point of view -- it never crashes
    /// or drops to an unconfigured state), and this is just the error
    /// reported back to the caller.
    ReloadFailed { error: String },
    /// [`Request::Status`]'s reply.
    Status(StatusReport),
}

/// A snapshot of the daemon's current tiling state for `hyperwm status`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusReport {
    pub pid: u32,
    pub config_path: String,
    pub tiled: Vec<WindowSummary>,
    pub floating: Vec<WindowSummary>,
    pub focused: Option<WindowSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowSummary {
    pub app: String,
    pub title: String,
    pub rect: RectSummary,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RectSummary {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Writes `value` as one newline-terminated JSON line to `writer` and
/// flushes -- the framing half of this module's wire format (see the
/// module doc for the exact shape each [`Request`]/[`Response`] variant
/// produces).
///
/// # Errors
///
/// Returns `Err` if `value` can't be serialized (never in practice for
/// [`Request`]/[`Response`] -- both are plain data with no types serde
/// can fail on) or if the write itself fails (a broken pipe, e.g. the
/// peer already hung up).
pub fn write_message<W: Write, T: Serialize>(writer: &mut W, value: &T) -> io::Result<()> {
    let mut line = serde_json::to_string(value)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    writer.flush()
}

/// Reads one newline-terminated JSON line from `reader` and deserializes
/// it. `Ok(None)` means the peer closed the connection before sending
/// anything (EOF on the very first read) -- distinct from a malformed
/// message, which is `Err`.
///
/// # Errors
///
/// Returns `Err` if the underlying read fails, or if a line was read but
/// isn't valid JSON for `T`.
pub fn read_message<R: BufRead, T: for<'de> Deserialize<'de>>(
    reader: &mut R,
) -> io::Result<Option<T>> {
    let mut line = String::new();
    let bytes_read = reader.read_line(&mut line)?;
    if bytes_read == 0 {
        return Ok(None);
    }
    serde_json::from_str(line.trim_end())
        .map(Some)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips_through_the_wire_format() {
        for request in [Request::Reload, Request::Status] {
            let mut buf: Vec<u8> = Vec::new();
            write_message(&mut buf, &request).unwrap();
            assert_eq!(buf.last(), Some(&b'\n'), "message must be newline-terminated");

            let mut reader = io::BufReader::new(buf.as_slice());
            let decoded: Request = read_message(&mut reader).unwrap().unwrap();
            assert_eq!(
                serde_json::to_string(&decoded).unwrap(),
                serde_json::to_string(&request).unwrap()
            );
        }
    }

    #[test]
    fn response_round_trips_through_the_wire_format() {
        let responses = vec![
            Response::Reloaded { builtin_keybinds: 5, script_keybinds: 2 },
            Response::ReloadFailed { error: "missing required field \"gaps.outer\"".to_string() },
            Response::Status(StatusReport {
                pid: 1234,
                config_path: "/home/user/.config/hyperwm/config.toml".to_string(),
                tiled: vec![WindowSummary {
                    app: "Terminal".to_string(),
                    title: "zsh".to_string(),
                    rect: RectSummary { x: 0.0, y: 0.0, width: 400.0, height: 600.0 },
                }],
                floating: vec![],
                focused: None,
            }),
        ];
        for response in responses {
            let mut buf: Vec<u8> = Vec::new();
            write_message(&mut buf, &response).unwrap();

            let mut reader = io::BufReader::new(buf.as_slice());
            let decoded: Response = read_message(&mut reader).unwrap().unwrap();
            assert_eq!(
                serde_json::to_string(&decoded).unwrap(),
                serde_json::to_string(&response).unwrap()
            );
        }
    }

    #[test]
    fn read_message_on_immediate_eof_is_none_not_an_error() {
        let mut reader = io::BufReader::new(&b""[..]);
        let decoded: Option<Request> = read_message(&mut reader).unwrap();
        assert_eq!(decoded, None);
    }

    #[test]
    fn malformed_json_is_an_error_not_a_panic() {
        let mut reader = io::BufReader::new(&b"not json\n"[..]);
        let result: io::Result<Option<Request>> = read_message(&mut reader);
        assert!(result.is_err());
    }
}
