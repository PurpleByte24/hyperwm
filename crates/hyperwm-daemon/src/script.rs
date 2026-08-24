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

use std::io;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
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
/// [`Output`] it produces, instead of scraping stderr log lines.
fn spawn(path: &Path) -> io::Result<JoinHandle<Output>> {
    let child = Command::new(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let path = path.to_path_buf();
    Ok(thread::spawn(move || reap(child, &path)))
}

/// Waits for `child` to exit, logging its stderr and a non-zero exit code
/// (architecture.md §5) -- but not stdout, which §5 doesn't ask the daemon
/// to log and which the script's own hyper-key trigger already implies the
/// user doesn't need surfaced (it's not a query for output).
fn reap(child: Child, path: &Path) -> Output {
    let output = child.wait_with_output().unwrap_or_else(|err| {
        eprintln!("hyperwm-daemon: couldn't wait on script {}: {err}", path.display());
        // wait_with_output() only fails if the initial OS wait call itself
        // errors (not on a non-zero exit, which is Ok(_) with a failing
        // ExitStatus) -- effectively unreachable once spawn() has already
        // succeeded, but a placeholder failing status lets this stay a
        // total function instead of dragging an Option/panic through
        // `run`'s otherwise-infallible fire-and-forget path.
        Output { status: ExitStatus::default(), stdout: Vec::new(), stderr: Vec::new() }
    });
    if !output.stderr.is_empty() {
        eprintln!(
            "hyperwm-daemon: script {} wrote to stderr:\n{}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    if !output.status.success() {
        eprintln!("hyperwm-daemon: script {} exited with {}", path.display(), output.status);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

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
    fn successful_script_is_reaped_with_empty_stderr() {
        let dir = scratch_dir("success");
        let script = write_script(&dir, "ok.sh", "#!/bin/sh\nexit 0\n");

        let output = spawn(&script).unwrap().join().unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn nonzero_exit_is_captured() {
        let dir = scratch_dir("nonzero-exit");
        let script = write_script(&dir, "fail.sh", "#!/bin/sh\nexit 7\n");

        let output = spawn(&script).unwrap().join().unwrap();
        assert!(!output.status.success());
        assert_eq!(output.status.code(), Some(7));
    }

    #[test]
    fn stderr_output_is_captured() {
        let dir = scratch_dir("stderr-output");
        let script = write_script(&dir, "loud.sh", "#!/bin/sh\necho oops >&2\nexit 0\n");

        let output = spawn(&script).unwrap().join().unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stderr).trim(), "oops");
    }

    #[test]
    fn run_does_not_block_on_a_slow_script() {
        let dir = scratch_dir("slow-script");
        let script = write_script(&dir, "slow.sh", "#!/bin/sh\nsleep 5\nexit 0\n");

        let started = std::time::Instant::now();
        run(&script);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
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
