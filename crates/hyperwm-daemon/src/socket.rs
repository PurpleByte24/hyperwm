//! CLI socket server (architecture.md §1, §6; build unit 7): a Unix
//! domain listener at `hyperwm_config::protocol::socket_path()` serving
//! `hyperwm reload`/`status` requests from `hyperwm-cli`, using
//! `hyperwm_config::protocol`'s wire format and `hyperwm_config::load`'s
//! validation -- this module doesn't reimplement either. `hyperwm verify`
//! is not served here; it runs entirely in the CLI process (see
//! `hyperwm-cli`).
//!
//! # Why polling, not a real accept thread
//!
//! `DaemonState` lives behind a non-`Send` `Rc<RefCell<_>>` specifically so
//! every subsystem here (hyperkey `CGEventTap`, per-app `AXObserver`s, the
//! mouse-button tap, the Space-change poll) can stay on one thread without
//! synchronization (see `main.rs`'s `AssertSend` doc comment). Serving the
//! socket from a genuine second thread would need a way to hand a request
//! back to that thread and get a response back out -- effectively
//! reinventing a channel-based poll anyway. Instead, [`watch`] attaches a
//! `CFRunLoopTimer` to the current thread's `CFRunLoop`, the exact same
//! pattern `hyperwm_macos::workspace::watch_space_changes` already
//! establishes for polling Space changes (see that function's doc comment
//! for the fuller rationale on why polling over this daemon's single
//! `CFRunLoop`, rather than a second thread, is this codebase's established
//! answer to "something needs to happen periodically without blocking the
//! event sources").
//!
//! The listener itself is non-blocking (`set_nonblocking(true)`), so a poll
//! tick with no pending connection returns immediately rather than
//! blocking the run loop. Once a connection *is* accepted, it's handled
//! with a bounded read/write timeout ([`CONNECTION_TIMEOUT`]) rather than
//! fully non-blocking I/O: the wire protocol is exactly one line in, one
//! line out, from a client (`hyperwm-cli`) that writes its whole request
//! immediately after connecting and then blocks reading the reply, so a
//! short blocking read/write is simpler than a real non-blocking state
//! machine for a connection that -- in every real case -- resolves in
//! well under a millisecond. The only way this stalls the run loop for the
//! full timeout is a client that connects and then never sends anything at
//! all, which nothing in `hyperwm-cli` does.

use std::io::BufReader;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::{fmt, io};

use core_foundation::base::TCFType;
use core_foundation::date::CFDate;
use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop, CFRunLoopTimer};
use core_foundation_sys::runloop::{CFRunLoopTimerContext, CFRunLoopTimerInvalidate};
use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;

use hyperwm_config::protocol::{self, Request, Response};

use crate::state::DaemonState;

/// How often the listener is checked for a pending connection. Small
/// enough that `hyperwm reload`/`status`, run interactively by a human,
/// feel instant.
const POLL_INTERVAL_SECS: f64 = 0.1;

/// Bounded read/write timeout for one accepted connection -- see this
/// module's doc comment for why a real client never approaches it.
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(2);

/// [`bind`] couldn't produce a usable listener.
#[derive(Debug)]
pub enum BindError {
    /// A live daemon is already listening at this path (confirmed by
    /// successfully connecting to it, not just by the socket file's
    /// existence -- see [`bind`]'s doc comment).
    AlreadyRunning,
    Io(io::Error),
}

impl fmt::Display for BindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRunning => write!(f, "another hyperwm-daemon instance is already running"),
            Self::Io(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for BindError {}

/// Binds a Unix domain listener at `path`, called early in `main.rs`
/// startup (before any window-server state is touched) so a second
/// instance fails fast instead of fighting a running one over the same
/// windows.
///
/// A stale socket file (left behind by a daemon that didn't exit
/// cleanly -- e.g. killed rather than stopped; nothing currently
/// `unlink`s it on a graceful exit either, since under normal operation
/// `CFRunLoop::run_current()` in `main.rs` never returns) makes a plain
/// `bind` fail with `AddrInUse` even though nothing is actually
/// listening. This distinguishes the two cases by attempting to
/// *connect*: a live daemon accepts immediately; a stale file refuses the
/// connection (`ECONNREFUSED`, nothing on the other end) -- only then is
/// it safe to remove and rebind.
///
/// # Errors
///
/// [`BindError::AlreadyRunning`] if a live daemon answers at `path`;
/// [`BindError::Io`] for any other bind/connect/remove failure.
pub fn bind(path: &Path) -> Result<UnixListener, BindError> {
    match UnixListener::bind(path) {
        Ok(listener) => Ok(listener),
        Err(err) if err.kind() == io::ErrorKind::AddrInUse => match UnixStream::connect(path) {
            Ok(_) => Err(BindError::AlreadyRunning),
            Err(_) => {
                std::fs::remove_file(path).map_err(BindError::Io)?;
                UnixListener::bind(path).map_err(BindError::Io)
            }
        },
        Err(err) => Err(BindError::Io(err)),
    }
}

type PollCallback = Box<dyn FnMut() + 'static>;

/// Keeps the socket listener and its poll timer alive for as long as this
/// is held; invalidates the timer, reclaims the boxed callback, and
/// removes the socket file on `Drop` -- same shape as
/// `hyperwm_macos::workspace::SpaceChangeWatcher`.
pub struct SocketWatcher {
    path: PathBuf,
    _listener: Rc<UnixListener>,
    timer: CFRunLoopTimer,
    callback: *mut PollCallback,
}

impl Drop for SocketWatcher {
    fn drop(&mut self) {
        unsafe {
            CFRunLoopTimerInvalidate(self.timer.as_concrete_TypeRef());
            drop(Box::from_raw(self.callback));
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Attaches `listener` to the current thread's `CFRunLoop`, polling it
/// every [`POLL_INTERVAL_SECS`] for connections and serving each with
/// [`Request::Reload`]/[`Request::Status`] against `state`. `config_path`
/// is the file [`Request::Reload`] re-reads via `hyperwm_config::load` --
/// the same path `main.rs` loaded at startup, not necessarily whatever the
/// currently-running config's own notion of its origin is (there isn't
/// one; `Config` doesn't carry its source path).
#[must_use]
pub fn watch(listener: UnixListener, state: Rc<RefCell<DaemonState>>, config_path: PathBuf) -> SocketWatcher {
    listener
        .set_nonblocking(true)
        .expect("a freshly bound UnixListener should always accept set_nonblocking");
    let path = listener.local_addr().ok().and_then(|addr| addr.as_pathname().map(Path::to_path_buf));
    let listener = Rc::new(listener);

    let poll_listener = Rc::clone(&listener);
    let callback: PollCallback = Box::new(move || poll_once(&poll_listener, &state, &config_path));
    let callback: *mut PollCallback = Box::into_raw(Box::new(callback));

    // `CFRunLoopTimerCreate` copies this struct itself, but not what
    // `info` points to -- with no `retain`/`release` callbacks registered,
    // CF never manages `callback`'s lifetime, so `SocketWatcher::drop`
    // must (see its doc comment) -- identical reasoning to
    // `workspace::watch_space_changes`'s own timer context.
    let mut context = CFRunLoopTimerContext {
        version: 0,
        info: callback.cast(),
        retain: None,
        release: None,
        copyDescription: None,
    };

    let now = CFDate::now().abs_time();
    let timer = CFRunLoopTimer::new(
        now + POLL_INTERVAL_SECS,
        POLL_INTERVAL_SECS,
        0,
        0,
        poll_trampoline,
        &mut context,
    );
    CFRunLoop::get_current().add_timer(&timer, unsafe { kCFRunLoopCommonModes });

    SocketWatcher {
        path: path.unwrap_or_default(),
        _listener: listener,
        timer,
        callback,
    }
}

extern "C" fn poll_trampoline(
    _timer: core_foundation_sys::runloop::CFRunLoopTimerRef,
    info: *mut c_void,
) {
    // Safety: `info` is always the `callback` pointer `watch` handed to
    // `CFRunLoopTimer::new`, valid for the timer's lifetime (freed only in
    // `SocketWatcher::drop`, after which the timer has already been
    // invalidated and can't fire again) -- identical reasoning to
    // `workspace::poll_trampoline`.
    let callback = unsafe { &mut *(info.cast::<PollCallback>()) };
    callback();
}

/// Drains every currently pending connection (not just one) so a burst of
/// back-to-back CLI invocations doesn't wait multiple poll ticks to be
/// served.
fn poll_once(listener: &UnixListener, state: &Rc<RefCell<DaemonState>>, config_path: &Path) {
    loop {
        match listener.accept() {
            Ok((stream, _addr)) => handle_connection(stream, state, config_path),
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
            Err(err) => {
                eprintln!("hyperwm-daemon: socket accept error: {err}");
                break;
            }
        }
    }
}

fn handle_connection(stream: UnixStream, state: &Rc<RefCell<DaemonState>>, config_path: &Path) {
    let _ = stream.set_read_timeout(Some(CONNECTION_TIMEOUT));
    let _ = stream.set_write_timeout(Some(CONNECTION_TIMEOUT));
    let read_stream = match stream.try_clone() {
        Ok(s) => s,
        Err(err) => {
            eprintln!("hyperwm-daemon: couldn't clone accepted socket connection: {err}");
            return;
        }
    };
    let mut reader = BufReader::new(read_stream);

    let request: Request = match protocol::read_message(&mut reader) {
        Ok(Some(request)) => request,
        Ok(None) => return, // Client disconnected without sending anything.
        Err(err) => {
            eprintln!("hyperwm-daemon: malformed request on socket: {err}");
            return;
        }
    };

    let response = dispatch_request(request, state, config_path);

    let mut writer = stream;
    if let Err(err) = protocol::write_message(&mut writer, &response) {
        eprintln!("hyperwm-daemon: couldn't write socket response: {err}");
    }
}

fn dispatch_request(
    request: Request,
    state: &Rc<RefCell<DaemonState>>,
    config_path: &Path,
) -> Response {
    match request {
        Request::Reload => handle_reload(state, config_path),
        Request::Status => Response::Status(state.borrow().status_report(config_path)),
    }
}

/// `hyperwm reload` (architecture.md §6): re-reads and re-validates
/// `config_path` via `hyperwm_config::load` -- the exact same validation
/// path `main.rs` runs at startup and `hyperwm verify` runs in the CLI, so
/// there's no separate/looser reload-time check to drift from either. On
/// success, applies the new config via `DaemonState::reload_config`. On
/// failure, does **not** touch `state` at all -- the previous valid config
/// (and the router/tree it produced) is simply left in place, which is
/// exactly architecture.md §6's requirement: the daemon keeps running on
/// the old config and only *reports* the error back to the caller.
fn handle_reload(state: &Rc<RefCell<DaemonState>>, config_path: &Path) -> Response {
    match hyperwm_config::load(config_path) {
        Ok(config) => {
            let builtin_keybinds = config.keybinds.builtin.len();
            let script_keybinds = config.keybinds.scripts.len();
            state.borrow_mut().reload_config(config);
            println!(
                "hyperwm-daemon: reloaded {} ({builtin_keybinds} built-in keybinds, \
                 {script_keybinds} script keybinds)",
                config_path.display()
            );
            Response::Reloaded { builtin_keybinds, script_keybinds }
        }
        Err(err) => {
            eprintln!(
                "hyperwm-daemon: reload rejected ({} is invalid), keeping the previous config \
                 running: {err}",
                config_path.display()
            );
            Response::ReloadFailed { error: err.to_string() }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory under the OS temp dir, unique to this test process and
    /// test name, freshly emptied -- same convention as this crate's other
    /// test modules.
    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("hyperwm-daemon-socket-tests")
            .join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_config(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    /// A short, flat path for an actual `UnixListener::bind` in a test --
    /// unlike `scratch_dir`'s config-file paths, a socket path is bound by
    /// `sun_path`'s ~104-byte OS limit (confirmed via a real
    /// `InvalidInput`/"path must be shorter than SUN_LEN" failure using
    /// `scratch_dir`'s deeper, more descriptive nesting), so this
    /// deliberately skips that convention in favor of staying short.
    fn socket_scratch_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("hwm-test-{tag}-{}.sock", std::process::id()))
    }

    const VALID_CONFIG: &str = r#"
[hyperkey]
watch_keycode = "f18"

[gaps]
outer = 0
inner = 0
"#;

    #[test]
    fn bind_detects_a_live_daemon_and_reuses_a_stale_socket_file() {
        let path = socket_scratch_path("bind");
        let _ = std::fs::remove_file(&path);

        let first = bind(&path).expect("first bind should succeed");
        // A second bind at the same path, with the first still listening,
        // must be refused rather than silently stealing the socket.
        assert!(matches!(bind(&path), Err(BindError::AlreadyRunning)));

        drop(first); // Leaves the socket *file* behind (std doesn't unlink it).
        assert!(path.exists(), "test assumption: dropping UnixListener doesn't unlink the path");

        // Nothing is listening anymore, so this must be treated as a
        // stale file and cleaned up rather than reported as AlreadyRunning.
        let second = bind(&path);
        assert!(second.is_ok(), "a stale socket file must not block a fresh bind: {second:?}");
    }

    #[test]
    fn reload_over_socket_reports_success_and_applies_the_new_config() {
        let dir = scratch_dir("reload-ok");
        let config_path = write_config(&dir, "config.toml", VALID_CONFIG);
        let state = Rc::new(RefCell::new(DaemonState::new(
            hyperwm_config::load(&config_path).unwrap(),
        )));
        let f = hyperwm_macos::keycode::lookup("f").unwrap();
        assert_eq!(state.borrow().action_for(f, false), None);

        write_config(
            &dir,
            "config.toml",
            &format!("{VALID_CONFIG}\n[keybinds]\n\"hyper+f\" = \"toggle_float\"\n"),
        );

        let response = dispatch_request(Request::Reload, &state, &config_path);
        assert!(matches!(
            response,
            Response::Reloaded { builtin_keybinds: 1, script_keybinds: 0 }
        ));
        assert_eq!(
            state.borrow().action_for(f, false),
            Some(hyperwm_config::Action::ToggleFloat)
        );
    }

    // The critical case (architecture.md §6): a broken reload must leave
    // the daemon running exactly as it was, and report the specific error
    // -- never crash, never drop to an unconfigured state.
    #[test]
    fn reload_over_socket_with_broken_config_keeps_previous_config_and_reports_the_error() {
        let dir = scratch_dir("reload-broken");
        let config_path = write_config(&dir, "config.toml", VALID_CONFIG);
        let state = Rc::new(RefCell::new(DaemonState::new(
            hyperwm_config::load(&config_path).unwrap(),
        )));
        let f = hyperwm_macos::keycode::lookup("f").unwrap();
        assert_eq!(state.borrow().action_for(f, false), None);

        // Deliberately broken: unknown action name.
        write_config(
            &dir,
            "config.toml",
            &format!("{VALID_CONFIG}\n[keybinds]\n\"hyper+f\" = \"not_a_real_action\"\n"),
        );

        let response = dispatch_request(Request::Reload, &state, &config_path);
        match response {
            Response::ReloadFailed { error } => {
                assert!(error.contains("not_a_real_action"), "error should name the bad value: {error}");
            }
            other => panic!("expected ReloadFailed, got {other:?}"),
        }
        // Still running on the original config: the bad keybind never
        // took effect, and nothing else about the router changed either.
        assert_eq!(state.borrow().action_for(f, false), None);
    }

    #[test]
    fn status_over_socket_reports_this_process_pid() {
        let dir = scratch_dir("status");
        let config_path = write_config(&dir, "config.toml", VALID_CONFIG);
        let state = Rc::new(RefCell::new(DaemonState::new(
            hyperwm_config::load(&config_path).unwrap(),
        )));

        match dispatch_request(Request::Status, &state, &config_path) {
            Response::Status(report) => {
                assert_eq!(report.pid, std::process::id());
                assert_eq!(report.tiled.len(), 0);
                assert_eq!(report.floating.len(), 0);
                assert_eq!(report.focused, None);
            }
            other => panic!("expected Status, got {other:?}"),
        }
    }

    // End-to-end over a real accepted connection (not just
    // `dispatch_request` directly), confirming the framing in
    // `handle_connection` round-trips correctly for a real client.
    #[test]
    fn handle_connection_round_trips_a_real_client_request() {
        let dir = scratch_dir("handle-connection");
        let socket_path = socket_scratch_path("handle-connection");
        let _ = std::fs::remove_file(&socket_path);
        let config_path = write_config(&dir, "config.toml", VALID_CONFIG);
        let state = Rc::new(RefCell::new(DaemonState::new(
            hyperwm_config::load(&config_path).unwrap(),
        )));

        let listener = UnixListener::bind(&socket_path).unwrap();
        let mut client = UnixStream::connect(&socket_path).unwrap();
        protocol::write_message(&mut client, &Request::Status).unwrap();

        let (server_stream, _) = listener.accept().unwrap();
        handle_connection(server_stream, &state, &config_path);

        let mut reader = std::io::BufReader::new(client);
        let response: Response = protocol::read_message(&mut reader).unwrap().unwrap();
        assert!(matches!(response, Response::Status(_)));
    }
}
