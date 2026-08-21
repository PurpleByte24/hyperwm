use std::path::{Path, PathBuf};

use hyperwm_config::{Action, ConfigError, FloatPlacement};
use hyperwm_core::InsertHeuristic;

/// A directory under the OS temp dir, unique to this test process and test
/// name, freshly emptied. Used for tests that need real files on disk
/// (a config.toml plus, sometimes, a scripts/ subdirectory).
fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("hyperwm-config-tests")
        .join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_config(dir: &Path, contents: &str) -> PathBuf {
    let path = dir.join("config.toml");
    std::fs::write(&path, contents).unwrap();
    path
}

/// Minimal config with every required field present and no keybinds, so
/// tests can append just the bit they care about.
const MINIMAL_REQUIRED: &str = r#"
[hyperkey]
watch_keycode = "f18"

[gaps]
outer = 12
inner = 8
"#;

#[test]
fn example_config_parses_with_no_errors() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/config.toml");
    let config = hyperwm_config::load(&path).expect("examples/config.toml must load cleanly");

    assert_eq!(config.hyperkey.watch_keycode.as_str(), "f18");
    assert_eq!(config.tiling.max_tiled_windows, 4);
    assert_eq!(config.tiling.split_ratio_default, 0.5);
    assert_eq!(config.tiling.insert_heuristic, InsertHeuristic::AspectRatio);
    assert_eq!(config.tiling.resize.step, 0.05);
    assert_eq!(config.gaps.outer, 12.0);
    assert_eq!(config.gaps.inner, 8.0);
    assert_eq!(
        config.floating.always_float_apps,
        vec![
            "com.apple.systempreferences",
            "com.1password.1password",
            "com.apple.calculator",
        ]
    );
    assert_eq!(config.floating.new_float_placement, FloatPlacement::Cascade);
    assert_eq!(config.keybinds.builtin.len(), 10);
    assert_eq!(config.keybinds.scripts.len(), 2);
    assert!(config.scripts.dir.ends_with("examples/scripts"));
}

#[test]
fn missing_required_field_is_reported_specifically() {
    let dir = scratch_dir("missing-required-field");
    // gaps.inner is left out entirely.
    let path = write_config(
        &dir,
        r#"
[hyperkey]
watch_keycode = "f18"

[gaps]
outer = 12
"#,
    );

    let err = hyperwm_config::load(&path).unwrap_err();
    match &err {
        ConfigError::MissingField(field) => assert_eq!(*field, "gaps.inner"),
        other => panic!("expected MissingField(\"gaps.inner\"), got {other:?}"),
    }
    assert!(err.to_string().contains("gaps.inner"));
}

#[test]
fn missing_hyperkey_table_is_reported_specifically() {
    let dir = scratch_dir("missing-hyperkey-table");
    let path = write_config(
        &dir,
        r#"
[gaps]
outer = 12
inner = 8
"#,
    );

    let err = hyperwm_config::load(&path).unwrap_err();
    match &err {
        ConfigError::MissingField(field) => assert_eq!(*field, "hyperkey.watch_keycode"),
        other => panic!("expected MissingField(\"hyperkey.watch_keycode\"), got {other:?}"),
    }
}

#[test]
fn bad_keybind_syntax_missing_hyper_prefix() {
    let dir = scratch_dir("bad-keybind-missing-hyper");
    let mut contents = MINIMAL_REQUIRED.to_string();
    contents.push_str(
        r#"
[keybinds]
"ctrl+h" = "move_left"
"#,
    );
    let path = write_config(&dir, &contents);

    let err = hyperwm_config::load(&path).unwrap_err();
    match &err {
        ConfigError::InvalidKeybind { raw, .. } => assert_eq!(raw, "ctrl+h"),
        other => panic!("expected InvalidKeybind, got {other:?}"),
    }
    assert!(err.to_string().contains("ctrl+h"));
}

#[test]
fn bad_keybind_syntax_unknown_key_name() {
    let dir = scratch_dir("bad-keybind-unknown-key");
    let mut contents = MINIMAL_REQUIRED.to_string();
    contents.push_str(
        r#"
[keybinds]
"hyper+doesnotexist" = "move_left"
"#,
    );
    let path = write_config(&dir, &contents);

    let err = hyperwm_config::load(&path).unwrap_err();
    assert!(matches!(err, ConfigError::InvalidKeybind { .. }));
}

#[test]
fn bad_keybind_syntax_unknown_modifier() {
    let dir = scratch_dir("bad-keybind-unknown-modifier");
    let mut contents = MINIMAL_REQUIRED.to_string();
    contents.push_str(
        r#"
[keybinds]
"hyper+ctrl+h" = "move_left"
"#,
    );
    let path = write_config(&dir, &contents);

    let err = hyperwm_config::load(&path).unwrap_err();
    assert!(matches!(err, ConfigError::InvalidKeybind { .. }));
}

#[test]
fn unknown_builtin_action_is_reported_specifically() {
    let dir = scratch_dir("unknown-action");
    let mut contents = MINIMAL_REQUIRED.to_string();
    contents.push_str(
        r#"
[keybinds]
"hyper+h" = "teleport_left"
"#,
    );
    let path = write_config(&dir, &contents);

    let err = hyperwm_config::load(&path).unwrap_err();
    match &err {
        ConfigError::UnknownAction { keybind, action } => {
            assert_eq!(keybind, "hyper+h");
            assert_eq!(action, "teleport_left");
        }
        other => panic!("expected UnknownAction, got {other:?}"),
    }
}

#[test]
fn duplicate_keybind_across_builtin_and_scripts_is_rejected() {
    let dir = scratch_dir("duplicate-keybind");
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    let script_path = dir.join("scripts/foo.sh");
    std::fs::write(&script_path, "#!/bin/sh\n").unwrap();
    set_executable(&script_path);

    let mut contents = MINIMAL_REQUIRED.to_string();
    contents.push_str(
        r#"
[keybinds]
"hyper+g" = "toggle_float"

[keybinds.scripts]
"hyper+g" = "foo.sh"
"#,
    );
    let path = write_config(&dir, &contents);

    let err = hyperwm_config::load(&path).unwrap_err();
    match &err {
        ConfigError::DuplicateKeybind { keybind } => assert_eq!(keybind, "hyper+g"),
        other => panic!("expected DuplicateKeybind, got {other:?}"),
    }
}

#[test]
fn script_reference_to_nonexistent_file_is_rejected() {
    let dir = scratch_dir("script-nonexistent");
    let mut contents = MINIMAL_REQUIRED.to_string();
    contents.push_str(
        r#"
[scripts]
dir = "scripts"

[keybinds.scripts]
"hyper+g" = "does_not_exist.sh"
"#,
    );
    let path = write_config(&dir, &contents);

    let err = hyperwm_config::load(&path).unwrap_err();
    match &err {
        ConfigError::ScriptNotFound { keybind, script, .. } => {
            assert_eq!(keybind, "hyper+g");
            assert_eq!(script, "does_not_exist.sh");
        }
        other => panic!("expected ScriptNotFound, got {other:?}"),
    }
}

#[test]
fn script_reference_to_non_executable_file_is_rejected() {
    let dir = scratch_dir("script-non-executable");
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    let script_path = dir.join("scripts/not_executable.sh");
    std::fs::write(&script_path, "#!/bin/sh\n").unwrap();
    // Deliberately not setting the executable bit.

    let mut contents = MINIMAL_REQUIRED.to_string();
    contents.push_str(
        r#"
[keybinds.scripts]
"hyper+g" = "not_executable.sh"
"#,
    );
    let path = write_config(&dir, &contents);

    let err = hyperwm_config::load(&path).unwrap_err();
    match &err {
        ConfigError::ScriptNotExecutable { keybind, script, .. } => {
            assert_eq!(keybind, "hyper+g");
            assert_eq!(script, "not_executable.sh");
        }
        other => panic!("expected ScriptNotExecutable, got {other:?}"),
    }
}

#[test]
fn script_reference_to_executable_file_succeeds() {
    let dir = scratch_dir("script-executable-ok");
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    let script_path = dir.join("scripts/runnable.sh");
    std::fs::write(&script_path, "#!/bin/sh\n").unwrap();
    set_executable(&script_path);

    let mut contents = MINIMAL_REQUIRED.to_string();
    contents.push_str(
        r#"
[keybinds.scripts]
"hyper+g" = "runnable.sh"
"#,
    );
    let path = write_config(&dir, &contents);

    let config = hyperwm_config::load(&path).expect("valid executable script should load");
    let (keybind, script) = config.keybinds.scripts.iter().next().unwrap();
    assert_eq!(keybind.to_string(), "hyper+g");
    assert_eq!(script, "runnable.sh");
}

#[test]
fn generic_toml_syntax_error_is_reported() {
    let dir = scratch_dir("bad-toml-syntax");
    let path = write_config(&dir, "this is not [ valid toml");

    let err = hyperwm_config::load(&path).unwrap_err();
    assert!(matches!(err, ConfigError::Toml(_)));
}

#[test]
fn missing_config_file_is_reported() {
    let dir = scratch_dir("missing-file");
    let path = dir.join("does-not-exist.toml");

    let err = hyperwm_config::load(&path).unwrap_err();
    assert!(matches!(err, ConfigError::Io { .. }));
}

#[test]
fn out_of_range_value_is_rejected() {
    let dir = scratch_dir("out-of-range");
    let mut contents = MINIMAL_REQUIRED.to_string();
    contents.push_str(
        r#"
[tiling]
split_ratio_default = 1.5
"#,
    );
    let path = write_config(&dir, &contents);

    let err = hyperwm_config::load(&path).unwrap_err();
    match &err {
        ConfigError::InvalidValue { field, .. } => {
            assert_eq!(*field, "tiling.split_ratio_default");
        }
        other => panic!("expected InvalidValue, got {other:?}"),
    }
}

#[test]
fn builtin_action_names_round_trip() {
    for (name, action) in [
        ("toggle_float", Action::ToggleFloat),
        ("maximize", Action::Maximize),
        ("move_left", Action::MoveLeft),
        ("move_down", Action::MoveDown),
        ("move_up", Action::MoveUp),
        ("move_right", Action::MoveRight),
        ("resize_left", Action::ResizeLeft),
        ("resize_down", Action::ResizeDown),
        ("resize_up", Action::ResizeUp),
        ("resize_right", Action::ResizeRight),
    ] {
        assert_eq!(action.as_str(), name);
    }
}

fn set_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}
