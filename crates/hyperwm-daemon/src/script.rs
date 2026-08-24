//! Script Runner (architecture.md §5): spawns a `[keybinds.scripts]`
//! binding's resolved path as a detached child process. `hyperwm_config`'s
//! validation already guarantees, by the time a path reaches [`run`], that
//! it exists and is executable (`hyperwm-config`'s `validate_keybinds`) --
//! this module only spawns and reaps, it doesn't re-check either.
//!
//! "Detached" (`Command::spawn`, not waited on -- architecture.md §5) means
//! the daemon's own run loop never blocks on a script. Reaping the child
//! still has to happen *somewhere*, though: an un-waited `Child` left to
//! `Drop` becomes a zombie process until the daemon itself exits, and the
//! daemon also needs the child's stderr/exit code to log per §5. So each
//! invocation gets its own background `std::thread` that waits on exactly
//! that one child and logs the result -- the run loop dispatching the
//! keypress is free again as soon as [`run`] returns, well before the
//! script (or the thread waiting on it) finishes.

use std::io::{self, BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread::{self, JoinHandle};

/// Spawns `path` as a detached child process and logs its stderr / a
/// non-zero exit code once it finishes, without blocking the caller.
pub fn run(path: &Path) {
    if let Err(err) = spawn(path) {
        eprintln!("hyperwm-daemon: couldn't spawn script {}: {err}", path.display());
    }
    // The returned `JoinHandle` is intentionally dropped here rather than
    // joined: dropping it detaches the reaping thread instead of waiting
    // for it, which is what leaves this function (and so the run loop's
    // keypress dispatch) non-blocking. The thread keeps running regardless.
}

/// Does the actual spawn + hand-off to a reaping thread. Split out from
/// [`run`] so tests can join the returned handle and assert on the
/// [`ExitStatus`] it produces.
fn spawn(path: &Path) -> io::Result<JoinHandle<ExitStatus>> {
    let child = Command::new(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let path = path.to_path_buf();
    Ok(thread::spawn(move || {
        let mut child = child;
        reap(&mut child, &path, log_stderr_line)
    }))
}

fn log_stderr_line(path: &Path, line: &str) {
    eprintln!("hyperwm-daemon: script {} stderr: {line}", path.display());
}

/// Streams `child`'s stderr to `on_line` one line at a time as it's
/// produced (architecture.md §5's "log the script's stderr"), then waits
/// for exit and logs a non-zero code. Deliberately *not*
/// `wait_with_output()`, which buffers the whole pipe and only hands it
/// back once the process has already exited -- for a script that logs
/// progress and then runs long (e.g. "starting..." followed by a slow
/// step), that would silently hold the "starting..." line back until the
/// script was already done, making a merely-slow script look stuck rather
/// than logging its own progress live. `on_line` is a parameter (rather
/// than always `eprintln!`) purely so tests can observe *when* a line
/// arrives, not just its content -- production always passes
/// [`log_stderr_line`].
fn reap(child: &mut Child, path: &Path, mut on_line: impl FnMut(&Path, &str)) -> ExitStatus {
    if let Some(stderr) = child.stderr.take() {
        for line in BufReader::new(stderr).lines() {
            match line {
                Ok(line) => on_line(path, &line),
                Err(err) => {
                    eprintln!(
                        "hyperwm-daemon: couldn't read stderr from script {}: {err}",
                        path.display()
                    );
                    break;
                }
            }
        }
    }
    let status = child.wait().unwrap_or_else(|err| {
        eprintln!("hyperwm-daemon: couldn't wait on script {}: {err}", path.display());
        // wait() only fails if the OS wait call itself errors (not on a
        // non-zero exit, which is Ok(_) with a failing ExitStatus) --
        // effectively unreachable once spawn() has already succeeded, but
        // a placeholder failing status lets this stay a total function
        // instead of dragging an Option/panic through `run`'s otherwise-
        // infallible fire-and-forget path.
        ExitStatus::default()
    });
    if !status.success() {
        eprintln!("hyperwm-daemon: script {} exited with {status}", path.display());
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// A directory under the OS temp dir, unique to this test process and
    /// test name, freshly emptied -- same convention as
    /// `hyperwm-config`'s `tests/config_tests.rs::scratch_dir`.
    fn scratch_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join("hyperwm-daemon-script-tests")
            .join(format!("{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_script(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        fs::write(&path, body).unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }

    #[test]
    fn successful_script_exits_cleanly() {
        let dir = scratch_dir("success");
        let script = write_script(&dir, "ok.sh", "#!/bin/sh\nexit 0\n");

        let status = spawn(&script).unwrap().join().unwrap();
        assert!(status.success());
    }

    #[test]
    fn nonzero_exit_is_captured() {
        let dir = scratch_dir("nonzero-exit");
        let script = write_script(&dir, "fail.sh", "#!/bin/sh\nexit 7\n");

        let status = spawn(&script).unwrap().join().unwrap();
        assert!(!status.success());
        assert_eq!(status.code(), Some(7));
    }

    #[test]
    fn stderr_lines_are_logged_as_they_arrive_not_buffered_until_exit() {
        let dir = scratch_dir("streamed-stderr");
        let script = write_script(
            &dir,
            "streamed.sh",
            "#!/bin/sh\necho first >&2\nsleep 3\necho second >&2\nexit 0\n",
        );

        let mut child = Command::new(&script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let (tx, rx) = mpsc::channel();
        let path = script.clone();
        let handle = thread::spawn(move || {
            reap(&mut child, &path, move |_, line| {
                let _ = tx.send((line.to_string(), Instant::now()));
            })
        });

        let started = Instant::now();
        let (first_line, first_at) = rx
            .recv_timeout(Duration::from_secs(1))
            .expect("first stderr line should arrive well before the 3s sleep finishes");
        assert_eq!(first_line, "first");
        assert!(
            first_at.duration_since(started) < Duration::from_secs(1),
            "first line was held back instead of streamed"
        );

        let (second_line, _) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(second_line, "second");

        assert!(handle.join().unwrap().success());
    }

    #[test]
    fn run_does_not_block_on_a_slow_script() {
        let dir = scratch_dir("slow-script");
        let script = write_script(&dir, "slow.sh", "#!/bin/sh\nsleep 5\nexit 0\n");

        let started = Instant::now();
        run(&script);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "run() must return immediately, not wait on the script"
        );
    }

    #[test]
    fn spawn_of_missing_path_errs() {
        let dir = scratch_dir("missing-path");
        let err = spawn(&dir.join("does_not_exist.sh")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
