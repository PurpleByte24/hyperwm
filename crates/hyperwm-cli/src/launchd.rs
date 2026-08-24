//! Thin wrapper around the `launchctl` CLI (build unit 8) for managing
//! hyperwm's `launchd` user-agent jobs -- the daemon itself
//! (`daemon start|stop|restart`) and the Caps Lock remap's persistence
//! agent (`install-keymap`). Both go through the same `gui/<uid>/<label>`
//! domain target and the same bootstrap/bootout/kickstart/print verbs, so
//! that logic lives here once rather than twice.
//!
//! # Why `launchctl print`, not `launchctl list`
//!
//! `launchctl list <label>` only reports whether a job is *loaded*, not
//! whether it's currently *running* -- a crashed or `bootout`-half-way job
//! can still show up there. `launchctl print gui/<uid>/<label>` prints a
//! `state = running` (or `not running`) line for a loaded job and exits
//! non-zero if the job isn't loaded (bootstrapped) at all, which gives us
//! both facts (bootstrapped? running?) from one call -- this is what
//! `daemon start`'s idempotency fix depends on (see `daemon.rs`'s module
//! doc): deciding whether to bootstrap/kickstart/no-op requires knowing
//! *running*, not just *loaded*.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The calling user's numeric uid, used to build the `gui/<uid>` `launchd`
/// domain target. Shelling out to `id -u` rather than an FFI `getuid()`
/// call keeps `hyperwm-cli` dependency-free (it already only depends on
/// `hyperwm-config`) -- this runs once per invocation, not on any hot path.
fn uid() -> io::Result<String> {
    let output = Command::new("id").arg("-u").output()?;
    if !output.status.success() {
        return Err(io::Error::other("`id -u` failed"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// The `gui/<uid>/<label>` domain target `launchctl bootstrap`/`bootout`/
/// `kickstart`/`print` all address a job by.
fn domain_target(label: &str) -> io::Result<String> {
    Ok(format!("gui/{}/{label}", uid()?))
}

/// Whether `label` is currently bootstrapped (loaded) in the user's
/// `launchd` domain, and if so, whether its `state` is `running`. `None`
/// for "not bootstrapped at all" (i.e. `launchctl print` exited non-zero).
fn state(label: &str) -> io::Result<Option<JobState>> {
    let target = domain_target(label)?;
    let output = Command::new("launchctl").args(["print", &target]).output()?;
    if !output.status.success() {
        // Not bootstrapped -- `launchctl print` exits non-zero (and
        // prints "Could not find service ..." to stderr) when the label
        // has no job in this domain at all.
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let running = text.lines().any(|line| line.trim() == "state = running");
    Ok(Some(if running { JobState::Running } else { JobState::Loaded }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobState {
    /// Bootstrapped but not currently running (e.g. a one-shot job that
    /// already ran and exited, or a crashed job with no `KeepAlive`).
    Loaded,
    Running,
}

/// `true` if `label` is bootstrapped in the user's `launchd` domain at
/// all (running or not).
pub fn is_bootstrapped(label: &str) -> io::Result<bool> {
    Ok(state(label)?.is_some())
}

/// `true` if `label` is bootstrapped *and* its `state` is `running`. This
/// is the check `daemon start` uses to decide whether starting is a true
/// no-op -- see `daemon.rs`.
pub fn is_running(label: &str) -> io::Result<bool> {
    Ok(state(label)? == Some(JobState::Running))
}

/// `launchctl bootstrap gui/<uid> <plist_path>` -- registers and (if the
/// plist sets `RunAtLoad`) immediately starts the job. Fails if `label` is
/// already bootstrapped; callers must check [`is_bootstrapped`] first.
pub fn bootstrap(label: &str, plist_path: &Path) -> Result<(), String> {
    let target = format!("gui/{}", uid().map_err(|e| e.to_string())?);
    run_launchctl(&["bootstrap", &target, &plist_path.display().to_string()], label)
}

/// `launchctl bootout gui/<uid>/<label>` -- unloads a bootstrapped job,
/// stopping it if running.
pub fn bootout(label: &str) -> Result<(), String> {
    let target = domain_target(label).map_err(|e| e.to_string())?;
    run_launchctl(&["bootout", &target], label)
}

/// `launchctl kickstart [-k] gui/<uid>/<label>` -- (re)starts an already-
/// bootstrapped job. `kill_first` maps to `-k`: without it, `kickstart` on
/// an already-running job is a no-op-ish "already started" error from
/// `launchctl` itself, which is why `daemon start` decides via
/// [`is_running`] up front rather than leaning on that.
pub fn kickstart(label: &str, kill_first: bool) -> Result<(), String> {
    let target = domain_target(label).map_err(|e| e.to_string())?;
    let mut args = vec!["kickstart"];
    if kill_first {
        args.push("-k");
    }
    args.push(&target);
    run_launchctl(&args, label)
}

/// `~/Library/LaunchAgents/<label>.plist` -- the standard per-user
/// `launchd` agent directory, independent of `hyperwm.dir`/`.config`
/// (this is `launchd`'s own namespace, not hyperwm's).
pub fn plist_path(home: &Path, label: &str) -> PathBuf {
    home.join("Library/LaunchAgents").join(format!("{label}.plist"))
}

/// Writes a minimal `launchd` agent plist for `label` running
/// `program_args` (first element is the executable), creating
/// `~/Library/LaunchAgents` if needed and overwriting any previous plist
/// at the same path -- both `daemon start`/`restart` and `install-keymap`
/// call this every time they run, so the plist always reflects the
/// current binary path (e.g. after a `brew upgrade`) rather than whatever
/// was on disk from a previous install.
///
/// `stdout_path`/`stderr_path` are plain strings (not XML-escaped) --
/// callers control both and neither ever contains XML metacharacters
/// (they're always paths this crate constructs itself under
/// `~/Library/Logs/hyperwm`, never user input), so escaping is deferred
/// to [`xml_escape`] only for values that could plausibly need it
/// (`program_args`, which can include a config-derived key name).
pub fn write_plist(
    home: &Path,
    label: &str,
    program_args: &[String],
    run_at_load: bool,
    stdout_path: &Path,
    stderr_path: &Path,
) -> io::Result<PathBuf> {
    let path = plist_path(home, label);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let args_xml: String = program_args
        .iter()
        .map(|arg| format!("        <string>{}</string>\n", xml_escape(arg)))
        .collect();

    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \x20   <key>Label</key>\n\
         \x20   <string>{label}</string>\n\
         \x20   <key>ProgramArguments</key>\n\
         \x20   <array>\n\
         {args_xml}\
         \x20   </array>\n\
         \x20   <key>RunAtLoad</key>\n\
         \x20   <{run_at_load}/>\n\
         \x20   <key>StandardOutPath</key>\n\
         \x20   <string>{}</string>\n\
         \x20   <key>StandardErrorPath</key>\n\
         \x20   <string>{}</string>\n\
         </dict>\n\
         </plist>\n",
        xml_escape(&stdout_path.display().to_string()),
        xml_escape(&stderr_path.display().to_string()),
    );

    std::fs::write(&path, plist)?;
    Ok(path)
}

fn xml_escape(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn run_launchctl(args: &[&str], label: &str) -> Result<(), String> {
    let output = Command::new("launchctl")
        .args(args)
        .output()
        .map_err(|err| format!("couldn't run launchctl {}: {err}", args.join(" ")))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "launchctl {} failed for {label}: {}",
            args.join(" "),
            stderr.trim()
        ))
    }
}
