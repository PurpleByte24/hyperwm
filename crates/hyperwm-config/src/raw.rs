//! Serde mirror of `examples/config.toml`'s shape. Every field is `Option`
//! so that missing-vs-present is decided in `validate.rs`, where we control
//! the error text — we don't want two different error styles (serde's
//! generic "missing field" vs. our own messages) depending on which field
//! was left out.

use std::collections::HashMap;

use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawConfig {
    pub hyperkey: Option<RawHyperkey>,
    pub tiling: Option<RawTiling>,
    pub gaps: Option<RawGaps>,
    pub floating: Option<RawFloating>,
    pub keybinds: Option<RawKeybinds>,
    pub scripts: Option<RawScripts>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawHyperkey {
    pub watch_keycode: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawTiling {
    pub max_tiled_windows: Option<i64>,
    pub split_ratio_default: Option<f64>,
    pub insert_heuristic: Option<String>,
    pub resize: Option<RawResize>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawResize {
    pub step: Option<f64>,
    pub min_ratio: Option<f64>,
    pub max_ratio: Option<f64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawGaps {
    pub outer: Option<f64>,
    pub inner: Option<f64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawFloating {
    pub always_float_apps: Option<Vec<String>>,
    pub new_float_placement: Option<String>,
    pub float_move_step: Option<f64>,
}

/// `[keybinds]` holds "hyper+<key>" = "<action>" entries directly, plus a
/// nested `[keybinds.scripts]` subtable — hence the flatten/named split.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct RawKeybinds {
    pub scripts: Option<HashMap<String, String>>,
    #[serde(flatten)]
    pub builtin: HashMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawScripts {
    pub dir: Option<String>,
}
