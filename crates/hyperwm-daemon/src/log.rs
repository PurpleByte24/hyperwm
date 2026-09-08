//! Timestamp prefix for the daemon's stdout/stderr lines. `launchd` redirects
//! both straight to `~/Library/Logs/hyperwm/daemon.log`/`daemon.err.log`
//! (see `hyperwm-cli`'s `daemon.rs`) with no timestamping of its own, so
//! without this every line in those files is unordered relative to
//! anything else happening on the system -- [`tlog!`]/[`telog!`] are drop-in
//! replacements for `println!`/`eprintln!` that prepend one.
//!
//! UTC, computed by hand (no `chrono`/`time` dependency) via the same
//! days-since-epoch civil calendar algorithm Howard Hinnant's
//! `chrono`-precursor papers describe -- accurate for any date this daemon
//! will ever log, no leap-second handling needed for a log timestamp.

use std::time::{SystemTime, UNIX_EPOCH};

#[must_use]
pub fn timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (days, secs_of_day) = (secs / 86_400, secs % 86_400);
    let (hour, min, sec) = (secs_of_day / 3600, (secs_of_day / 60) % 60, secs_of_day % 60);

    // Howard Hinnant's civil_from_days: days-since-epoch -> proleptic
    // Gregorian (y, m, d), valid for the entire range of dates a `SystemTime`
    // can represent.
    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!("{y:04}-{m:02}-{d:02} {hour:02}:{min:02}:{sec:02} UTC")
}

/// Drop-in `println!` replacement that prefixes a `[<timestamp>] ` tag.
#[macro_export]
macro_rules! tlog {
    ($($arg:tt)*) => {{
        println!("[{}] {}", $crate::log::timestamp(), format!($($arg)*));
    }};
}

/// Drop-in `eprintln!` replacement that prefixes a `[<timestamp>] ` tag.
#[macro_export]
macro_rules! telog {
    ($($arg:tt)*) => {{
        eprintln!("[{}] {}", $crate::log::timestamp(), format!($($arg)*));
    }};
}
