//! Manual verification harness for build unit 4's definition of done:
//! move and resize a real window via AX calls driven by a hardcoded rect.
//!
//! Usage:
//!
//! ```text
//! cargo run -p hyperwm-macos --example move_window -- "<app name>"
//! ```
//!
//! e.g. `cargo run -p hyperwm-macos --example move_window -- TextEdit`.
//! Moves and resizes that app's focused window to a hardcoded rect
//! (100, 100, 800x600), prints the before/after position and size read
//! back from the window, then restores its original geometry.
//!
//! Requires Accessibility permission granted to whatever binary runs this
//! (your terminal app, if running via `cargo run` directly) --
//! architecture.md §2.3. The first run triggers the system prompt; grant
//! it in System Settings > Privacy & Security > Accessibility and re-run.

use std::process::ExitCode;

use accessibility_sys::{error_string, kAXFocusedWindowAttribute, kAXTitleAttribute, AXError};
use core_graphics::geometry::{CGPoint, CGSize};
use hyperwm_macos::ax::{self, AXUIElement};
use hyperwm_macos::permissions;

fn main() -> ExitCode {
    let Some(app_name) = std::env::args().nth(1) else {
        eprintln!("usage: move_window <app name>");
        return ExitCode::FAILURE;
    };

    let status = permissions::check();
    if !status.accessibility {
        eprintln!(
            "missing Accessibility permission; grant it, then re-run:\n{}",
            status.instructions()
        );
        return ExitCode::FAILURE;
    }

    let Some(pid) = ax::pid_for_app_name(&app_name) else {
        eprintln!(
            "no on-screen window is owned by an app named \"{app_name}\" (exact \
             kCGWindowOwnerName match, case-insensitive -- is it running and not fully hidden?)"
        );
        return ExitCode::FAILURE;
    };
    println!("found \"{app_name}\" at pid {pid}");

    let app = AXUIElement::application(pid);
    let window = match app.element_attribute(kAXFocusedWindowAttribute) {
        Ok(window) => window,
        Err(err) => {
            eprintln!("couldn't get the app's focused window: {}", describe(err));
            return ExitCode::FAILURE;
        }
    };

    let title = window
        .string_attribute(kAXTitleAttribute)
        .unwrap_or_else(|_| "<untitled>".to_string());
    println!("target window: {title:?}");

    let original_position = report("position before", window.position());
    let original_size = report("size before", window.size());

    let target_position = CGPoint::new(100.0, 100.0);
    let target_size = CGSize::new(800.0, 600.0);
    println!(
        "moving to ({}, {}), resizing to {}x{}",
        target_position.x, target_position.y, target_size.width, target_size.height
    );

    if let Err(err) = window.set_position(target_position) {
        eprintln!("set_position failed: {}", describe(err));
        return ExitCode::FAILURE;
    }
    if let Err(err) = window.set_size(target_size) {
        eprintln!("set_size failed: {}", describe(err));
        return ExitCode::FAILURE;
    }

    let after_position = report("position after", window.position());
    let after_size = report("size after", window.size());
    println!(
        "read back: position ({:?}) size ({:?}) -- compare against the values requested above \
         to see whether this app honored the move/resize exactly (some apps, notably \
         Chrome/Electron-based ones, are known to clamp or ignore programmatic AX \
         resize/move)",
        after_position, after_size
    );

    if let (Some(position), Some(size)) = (original_position, original_size) {
        println!("restoring original geometry");
        let _ = window.set_position(position);
        let _ = window.set_size(size);
    }

    ExitCode::SUCCESS
}

fn report<T: std::fmt::Debug>(label: &str, value: Result<T, AXError>) -> Option<T> {
    match value {
        Ok(value) => {
            println!("{label}: {value:?}");
            Some(value)
        }
        Err(err) => {
            eprintln!("{label}: failed to read ({})", describe(err));
            None
        }
    }
}

fn describe(err: AXError) -> String {
    format!("{} ({err})", error_string(err))
}
