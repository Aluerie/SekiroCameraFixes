//! Settings read from `SekiroCameraFixes.ini` next to the DLL.

/// Which patches to apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// "Disable camera auto rotate on movement".
    pub disable_auto_rotate: bool,
    /// "Disable camera reset on lock-on".
    pub disable_lockon_reset: bool,
    /// Skip the pitch-on-XY-movement patch, which breaks pitch and other stick
    /// controls on gamepads. Matches answering "No" to the original tool's
    /// "Are you using a mouse as input?" prompt.
    pub controller: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            disable_auto_rotate: true,
            disable_lockon_reset: true,
            controller: false,
        }
    }
}

/// Default file contents written when no config file exists.
pub const DEFAULT_INI: &str = "\
; SekiroCameraFixes settings. Restart the game after changing them.
[SekiroCameraFixes]
; Stop the camera from turning by itself when you move.
disable_auto_rotate = true
; Pressing lock-on with nothing in range no longer snaps the camera forward.
disable_lockon_reset = true
; Set to true if you play with a controller. Leaves the vertical auto-adjust on
; XY movement alone, which otherwise breaks stick pitch and deadzones.
controller = false
";

impl Config {
    /// Parses INI text, starting from defaults. Section headers are ignored, and
    /// `;` or `#` starts a comment, also after a value. A leading UTF-8 byte order
    /// mark is skipped. Returns the config and one warning per line that couldn't be used.
    pub fn parse(text: &str) -> (Self, Vec<String>) {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut config = Self::default();
        let mut warnings = Vec::new();
        for (number, raw) in text.lines().enumerate() {
            let number = number + 1;
            let line = raw.split([';', '#']).next().unwrap_or_default().trim();
            if line.is_empty() || line.starts_with('[') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                warnings.push(format!("line {number}: expected `key = value`"));
                continue;
            };
            let key = key.trim().to_ascii_lowercase();
            let setting = match key.as_str() {
                "disable_auto_rotate" => &mut config.disable_auto_rotate,
                "disable_lockon_reset" => &mut config.disable_lockon_reset,
                "controller" => &mut config.controller,
                _ => {
                    warnings.push(format!("line {number}: unknown setting `{key}`"));
                    continue;
                }
            };
            let value = value.trim();
            match parse_bool(value) {
                Some(value) => *setting = value,
                None => warnings.push(format!("line {number}: `{value}` is not true/false")),
            }
        }
        (config, warnings)
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_file_matches_defaults() {
        let (config, warnings) = Config::parse(DEFAULT_INI);
        assert_eq!(config, Config::default());
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn overrides_and_warnings() {
        let text = "[x]\ncontroller=1\nDisable_LockOn_Reset = off\nbogus = true\nnope\ncontroller = maybe\n";
        let (config, warnings) = Config::parse(text);
        assert!(config.controller);
        assert!(!config.disable_lockon_reset);
        assert!(config.disable_auto_rotate);
        assert_eq!(warnings.len(), 3);
    }

    #[test]
    fn inline_comments() {
        let (config, warnings) = Config::parse("controller = true ; I use a gamepad\n");
        assert!(config.controller);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn byte_order_mark_is_skipped() {
        let (config, warnings) = Config::parse("\u{feff}controller = true\n");
        assert!(config.controller);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn unknown_key_reported_before_bad_value() {
        let (_, warnings) = Config::parse("bogus = maybe\n");
        assert_eq!(warnings, ["line 1: unknown setting `bogus`"]);
    }
}
