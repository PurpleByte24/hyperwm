//! Turns a parsed [`RawConfig`] into a validated [`Config`], filling in
//! documented defaults (architecture.md, examples/config.toml) for fields
//! that have one and rejecting anything out of range. This is the one place
//! both the daemon and `hyperwm verify` funnel through — see
//! [`crate::load`].

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use hyperwm_core::{Gaps, InsertHeuristic};

use crate::action::Action;
use crate::error::ConfigError;
use crate::keybind::{parse_keybind, Keybind, KeyName};
use crate::raw::RawConfig;
use crate::{
    Config, FloatPlacement, FloatingConfig, HyperkeyConfig, KeybindsConfig, ResizeConfig,
    ScriptsConfig, TilingConfig,
};

pub(crate) fn validate(raw: RawConfig, config_dir: &Path) -> Result<Config, ConfigError> {
    let hyperkey = validate_hyperkey(raw.hyperkey.unwrap_or_default())?;
    let tiling = validate_tiling(raw.tiling.unwrap_or_default())?;
    let gaps = validate_gaps(raw.gaps.unwrap_or_default())?;
    let floating = validate_floating(raw.floating.unwrap_or_default())?;
    let scripts = validate_scripts(raw.scripts.unwrap_or_default(), config_dir)?;
    let keybinds = validate_keybinds(raw.keybinds.unwrap_or_default(), &scripts.dir)?;

    Ok(Config { hyperkey, tiling, gaps, floating, keybinds, scripts })
}

fn validate_hyperkey(raw: crate::raw::RawHyperkey) -> Result<HyperkeyConfig, ConfigError> {
    let raw_keycode = raw
        .watch_keycode
        .ok_or(ConfigError::MissingField("hyperkey.watch_keycode"))?;
    let watch_keycode = KeyName::parse(&raw_keycode).ok_or_else(|| ConfigError::InvalidValue {
        field: "hyperkey.watch_keycode",
        reason: format!("\"{raw_keycode}\" isn't a recognized key name"),
    })?;
    Ok(HyperkeyConfig { watch_keycode })
}

fn validate_tiling(raw: crate::raw::RawTiling) -> Result<TilingConfig, ConfigError> {
    let max_tiled_windows = raw.max_tiled_windows.unwrap_or(4);
    let max_tiled_windows = usize::try_from(max_tiled_windows).map_err(|_| {
        ConfigError::InvalidValue {
            field: "tiling.max_tiled_windows",
            reason: "must be a positive integer".to_string(),
        }
    })?;
    if max_tiled_windows < 1 {
        return Err(ConfigError::InvalidValue {
            field: "tiling.max_tiled_windows",
            reason: "must be at least 1".to_string(),
        });
    }

    let split_ratio_default = raw.split_ratio_default.unwrap_or(0.5);
    require_unit_range("tiling.split_ratio_default", split_ratio_default)?;

    let insert_heuristic = match raw.insert_heuristic.as_deref().unwrap_or("aspect_ratio") {
        "aspect_ratio" => InsertHeuristic::AspectRatio,
        "always_vertical" => InsertHeuristic::AlwaysVertical,
        "always_horizontal" => InsertHeuristic::AlwaysHorizontal,
        other => {
            return Err(ConfigError::InvalidValue {
                field: "tiling.insert_heuristic",
                reason: format!(
                    "\"{other}\" is not one of \"aspect_ratio\", \"always_vertical\", \
                     \"always_horizontal\""
                ),
            })
        }
    };

    let resize = validate_resize(raw.resize.unwrap_or_default())?;

    Ok(TilingConfig { max_tiled_windows, split_ratio_default, insert_heuristic, resize })
}

fn validate_resize(raw: crate::raw::RawResize) -> Result<ResizeConfig, ConfigError> {
    let step = raw.step.unwrap_or(0.05);
    if !(step > 0.0 && step <= 1.0) {
        return Err(ConfigError::InvalidValue {
            field: "tiling.resize.step",
            reason: "must be greater than 0.0 and at most 1.0".to_string(),
        });
    }

    let min_ratio = raw.min_ratio.unwrap_or(0.1);
    require_unit_range("tiling.resize.min_ratio", min_ratio)?;
    let max_ratio = raw.max_ratio.unwrap_or(0.9);
    require_unit_range("tiling.resize.max_ratio", max_ratio)?;
    if min_ratio >= max_ratio {
        return Err(ConfigError::InvalidValue {
            field: "tiling.resize.max_ratio",
            reason: format!(
                "must be greater than tiling.resize.min_ratio ({min_ratio}), got {max_ratio}"
            ),
        });
    }

    Ok(ResizeConfig { step, min_ratio, max_ratio })
}

fn validate_gaps(raw: crate::raw::RawGaps) -> Result<Gaps, ConfigError> {
    let outer = raw.outer.ok_or(ConfigError::MissingField("gaps.outer"))?;
    require_non_negative("gaps.outer", outer)?;
    let inner = raw.inner.ok_or(ConfigError::MissingField("gaps.inner"))?;
    require_non_negative("gaps.inner", inner)?;
    Ok(Gaps { outer, inner })
}

fn validate_floating(raw: crate::raw::RawFloating) -> Result<FloatingConfig, ConfigError> {
    let always_float_apps = raw.always_float_apps.unwrap_or_default();

    let new_float_placement = match raw.new_float_placement.as_deref().unwrap_or("cascade") {
        "cascade" => FloatPlacement::Cascade,
        "center" => FloatPlacement::Center,
        other => {
            return Err(ConfigError::InvalidValue {
                field: "floating.new_float_placement",
                reason: format!("\"{other}\" is not one of \"cascade\", \"center\""),
            })
        }
    };

    let float_move_step = raw.float_move_step.unwrap_or(40.0);
    require_non_negative("floating.float_move_step", float_move_step)?;

    Ok(FloatingConfig { always_float_apps, new_float_placement, float_move_step })
}

fn validate_scripts(
    raw: crate::raw::RawScripts,
    config_dir: &Path,
) -> Result<ScriptsConfig, ConfigError> {
    let dir = raw.dir.unwrap_or_else(|| "scripts".to_string());
    if dir.trim().is_empty() {
        return Err(ConfigError::InvalidValue {
            field: "scripts.dir",
            reason: "must not be empty".to_string(),
        });
    }
    let dir = Path::new(&dir);
    let dir = if dir.is_absolute() { dir.to_path_buf() } else { config_dir.join(dir) };
    Ok(ScriptsConfig { dir })
}

fn validate_keybinds(
    raw: crate::raw::RawKeybinds,
    scripts_dir: &Path,
) -> Result<KeybindsConfig, ConfigError> {
    let mut seen: HashMap<Keybind, String> = HashMap::new();
    let mut builtin = HashMap::new();
    let mut scripts = HashMap::new();

    let mut builtin_entries: Vec<(String, String)> = raw.builtin.into_iter().collect();
    builtin_entries.sort_by(|a, b| a.0.cmp(&b.0));
    for (raw_keybind, raw_action) in builtin_entries {
        let keybind = parse_and_check_duplicate(&raw_keybind, &mut seen)?;
        let action = Action::parse(&raw_action).ok_or_else(|| ConfigError::UnknownAction {
            keybind: raw_keybind.clone(),
            action: raw_action.clone(),
        })?;
        builtin.insert(keybind, action);
    }

    let mut script_entries: Vec<(String, String)> =
        raw.scripts.unwrap_or_default().into_iter().collect();
    script_entries.sort_by(|a, b| a.0.cmp(&b.0));
    for (raw_keybind, script_name) in script_entries {
        let keybind = parse_and_check_duplicate(&raw_keybind, &mut seen)?;
        let script_path = scripts_dir.join(&script_name);
        let metadata = fs::metadata(&script_path).map_err(|_| ConfigError::ScriptNotFound {
            keybind: raw_keybind.clone(),
            script: script_name.clone(),
            path: script_path.clone(),
        })?;
        if !is_executable(&metadata) {
            return Err(ConfigError::ScriptNotExecutable {
                keybind: raw_keybind.clone(),
                script: script_name.clone(),
                path: script_path,
            });
        }
        scripts.insert(keybind, script_name);
    }

    Ok(KeybindsConfig { builtin, scripts })
}

fn parse_and_check_duplicate(
    raw_keybind: &str,
    seen: &mut HashMap<Keybind, String>,
) -> Result<Keybind, ConfigError> {
    let keybind = parse_keybind(raw_keybind).map_err(|reason| ConfigError::InvalidKeybind {
        raw: raw_keybind.to_string(),
        reason,
    })?;
    if let Some(first) = seen.insert(keybind.clone(), raw_keybind.to_string()) {
        return Err(ConfigError::DuplicateKeybind { keybind: first });
    }
    Ok(keybind)
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

fn require_unit_range(field: &'static str, value: f64) -> Result<(), ConfigError> {
    if !(0.0..=1.0).contains(&value) {
        return Err(ConfigError::InvalidValue {
            field,
            reason: format!("must be between 0.0 and 1.0, got {value}"),
        });
    }
    Ok(())
}

fn require_non_negative(field: &'static str, value: f64) -> Result<(), ConfigError> {
    if value < 0.0 {
        return Err(ConfigError::InvalidValue {
            field,
            reason: format!("must not be negative, got {value}"),
        });
    }
    Ok(())
}
