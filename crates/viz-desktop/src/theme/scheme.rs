//! Base16 schemes: the built-in set, inline colors from `desktop.toml`, and
//! scheme files in tinted-theming's YAML format.
//!
//! The built-ins are vendored, unmodified, from
//! <https://github.com/tinted-theming/schemes> (`base16/`, commit `1bd5ad4`,
//! MIT: `themes/LICENSE`) and compiled in. To add one, copy its YAML into
//! `crates/viz-desktop/themes/` and list its file name below.

use std::{collections::BTreeMap, path::PathBuf};

use rune::Color;
use serde::Deserialize;

/// The schemes used when `[theme]` doesn't pick one, or picks one that fails.
pub const DEFAULT_DARK: &str = "github-dark";
pub const DEFAULT_LIGHT: &str = "github";

macro_rules! builtins {
    ($($slug:literal),* $(,)?) => {
        /// (name, YAML) of every built-in scheme; the name is the file's stem.
        const BUILTINS: &[(&str, &str)] =
            &[$(($slug, include_str!(concat!("../../themes/", $slug, ".yaml")))),*];
    };
}

builtins![
    "ayu-dark",
    "ayu-light",
    "catppuccin-latte",
    "catppuccin-mocha",
    "dracula",
    "everforest",
    "everforest-light-medium",
    "github",
    "github-dark",
    "gruvbox-dark",
    "gruvbox-light",
    "kanagawa",
    "nord",
    "nord-light",
    "one-light",
    "onedark",
    "rose-pine",
    "rose-pine-dawn",
    "solarized-dark",
    "solarized-light",
    "tokyo-night-dark",
    "tokyo-night-light",
    "tomorrow",
    "tomorrow-night",
];

/// A base16 scheme: `base[0x0]` through `base[0xF]`.
#[derive(Debug, Clone)]
pub struct Scheme {
    pub base: [Color; 16],
}

/// A tinted-theming scheme file. Base24 files work too: their extra colors
/// are ignored.
#[derive(Deserialize)]
struct SchemeFile {
    system: Option<String>,
    palette: BTreeMap<String, String>,
}

impl Scheme {
    /// A built-in scheme by name.
    pub fn builtin(name: &str) -> Result<Self, String> {
        let (_, yaml) = BUILTINS
            .iter()
            .find(|(slug, _)| *slug == name)
            .ok_or_else(|| {
                let names: Vec<_> = BUILTINS.iter().map(|(slug, _)| *slug).collect();
                format!("unknown scheme \"{name}\"; built in: {}", names.join(", "))
            })?;
        Self::from_yaml(yaml)
    }

    /// A scheme file; a leading `~/` is the home directory.
    pub fn from_file(path: &str) -> Result<Self, String> {
        let path = match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
            (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
            _ => PathBuf::from(path),
        };
        let yaml = std::fs::read_to_string(&path)
            .map_err(|e| format!("can't read {}: {e}", path.display()))?;
        Self::from_yaml(&yaml).map_err(|e| format!("{}: {e}", path.display()))
    }

    fn from_yaml(yaml: &str) -> Result<Self, String> {
        let file: SchemeFile = serde_saphyr::from_str(yaml).map_err(|e| e.to_string())?;
        if let Some(system) = file.system
            && !matches!(system.as_str(), "base16" | "base24")
        {
            return Err(format!("\"{system}\" is not a base16 or base24 scheme"));
        }
        Self::from_colors(&file.palette)
    }

    /// `base00` through `base0F` as `#rrggbb` (the `#` optional, digits in
    /// either case); all sixteen are required.
    pub fn from_colors(colors: &BTreeMap<String, String>) -> Result<Self, String> {
        let mut base = [None; 16];
        for (key, value) in colors {
            let index = key
                .strip_prefix("base")
                .filter(|digits| digits.len() == 2)
                .and_then(|digits| usize::from_str_radix(digits, 16).ok());
            match index {
                Some(i @ 0..16) => {
                    base[i] =
                        Some(hex(value).ok_or_else(|| {
                            format!("{key} = \"{value}\" is not a #rrggbb color")
                        })?);
                }
                // Base24's extra colors.
                Some(16..24) => {}
                _ => {
                    return Err(format!(
                        "unknown color \"{key}\"; expected base00 to base0F"
                    ));
                }
            }
        }
        let missing: Vec<_> = (0..16)
            .filter(|&i| base[i].is_none())
            .map(|i| format!("base{i:02X}"))
            .collect();
        if !missing.is_empty() {
            return Err(format!("missing {}", missing.join(", ")));
        }
        Ok(Self {
            base: base.map(Option::unwrap_or_default),
        })
    }
}

fn hex(value: &str) -> Option<Color> {
    let digits = value.strip_prefix('#').unwrap_or(value);
    (digits.len() == 6)
        .then(|| rune::color::hex(&format!("#{digits}")))
        .flatten()
}
