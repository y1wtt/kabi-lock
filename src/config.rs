//! 設定ファイルの読み込みと、主画面の選択。

#![forbid(unsafe_code)]

use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;

use serde::Deserialize;

#[derive(Clone, Debug, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub display: Display,
    pub appearance: Appearance,
    pub input: Input,
    pub auth: Auth,
    pub keepassxc: KeePassXc,
}

#[derive(Clone, Debug, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct Display {
    /// 時計などを出す画面の候補（出力名またはモニターの説明文）。上から順に照合する。
    pub primary: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub fade_in_ms: u32,
    pub blur: f32,
    pub date_format: String,
    /// 時計・日付のフォント（fontconfig のファミリー名）。見つからなければ埋め込みの Inter。
    pub font: String,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            fade_in_ms: 1500,
            blur: 7.0,
            date_format: DEFAULT_DATE_FORMAT.into(),
            font: DEFAULT_FONT.into(),
        }
    }
}

/// `appearance.font` の初期値。バイナリに埋め込んだフォントと同じファミリー。
pub const DEFAULT_FONT: &str = "Inter";

/// `appearance.date_format` の初期値。不正な書式のときもこれに戻す。
pub const DEFAULT_DATE_FORMAT: &str = "%m/%d";

/// chrono の strftime 形式として正しいか。
///
/// 不正な書式のまま `to_string()` すると panic するので、読み込み時に確かめる。
pub fn is_valid_date_format(format: &str) -> bool {
    chrono::format::StrftimeItems::new(format).parse().is_ok()
}

/// 不正な `date_format` を初期値に戻す。
fn sanitize_date_format(appearance: &mut Appearance) {
    if !is_valid_date_format(&appearance.date_format) {
        log::warn!(
            "appearance.date_format {:?} is not a valid strftime format; using {:?}",
            appearance.date_format,
            DEFAULT_DATE_FORMAT
        );
        appearance.date_format = DEFAULT_DATE_FORMAT.into();
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct Input {
    pub pillar_fade_ms: u32,
    pub pillar_width: u32,
    pub band_width: u32,
}

impl Default for Input {
    fn default() -> Self {
        Self {
            pillar_fade_ms: 900,
            pillar_width: 32,
            band_width: 768,
        }
    }
}

/// `input.band_width` と `input.pillar_width` の上限（1920x1080 基準の px）。
///
/// 巨大な値を入れるとレーン数が膨らみ、打鍵ごとの割り当てが重くなるので丸める。
pub const MAX_BAND_WIDTH: u32 = 1920;

/// `input.pillar_fade_ms` の上限。
///
/// 長すぎると、打つのをやめた後も長い間 主画面を毎フレーム描き続けるので丸める。
pub const MAX_PILLAR_FADE_MS: u32 = 10_000;

/// 帯と柱の幅、柱の消える時間を上限で丸める。
fn clamp_input(input: &mut Input) {
    if input.pillar_fade_ms > MAX_PILLAR_FADE_MS {
        log::warn!(
            "input.pillar_fade_ms {} is too large; using {MAX_PILLAR_FADE_MS}",
            input.pillar_fade_ms
        );
        input.pillar_fade_ms = MAX_PILLAR_FADE_MS;
    }
    if input.band_width > MAX_BAND_WIDTH {
        log::warn!(
            "input.band_width {} is too large; using {MAX_BAND_WIDTH}",
            input.band_width
        );
        input.band_width = MAX_BAND_WIDTH;
    }
    if input.pillar_width > MAX_BAND_WIDTH {
        log::warn!(
            "input.pillar_width {} is too large; using {MAX_BAND_WIDTH}",
            input.pillar_width
        );
        input.pillar_width = MAX_BAND_WIDTH;
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct Auth {
    pub pam_service: String,
    pub u2f: U2f,
}

impl Default for Auth {
    fn default() -> Self {
        Self {
            pam_service: "kabi-lock".into(),
            u2f: U2f::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct U2f {
    pub enabled: bool,
    pub pam_service: String,
}

impl Default for U2f {
    fn default() -> Self {
        Self {
            enabled: true,
            pam_service: "kabi-lock-u2f".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct KeePassXc {
    pub lock_before: bool,
    pub unlock_after: bool,
}

impl Default for KeePassXc {
    fn default() -> Self {
        Self {
            lock_before: true,
            unlock_after: true,
        }
    }
}

/// 読み込んだ設定と、知らないキーのパス（警告用）。
pub struct Loaded {
    pub config: Config,
    pub unknown_keys: Vec<String>,
}

#[derive(Debug)]
pub struct ConfigError(pub String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid config: {}", self.0)
    }
}

impl std::error::Error for ConfigError {}

/// TOML の文字列から設定を読む。書かれていないキーは初期値、知らないキーは `unknown_keys` に入れる。
pub fn parse(text: &str) -> Result<Loaded, ConfigError> {
    let value: toml::Value = toml::from_str(text).map_err(|e| ConfigError(e.to_string()))?;
    let mut unknown_keys = Vec::new();
    let mut config: Config =
        serde_ignored::deserialize(value, |path| unknown_keys.push(path.to_string()))
            .map_err(|e| ConfigError(e.to_string()))?;
    sanitize_date_format(&mut config.appearance);
    clamp_input(&mut config.input);
    Ok(Loaded {
        config,
        unknown_keys,
    })
}

/// 設定ファイルの場所。`$XDG_CONFIG_HOME/kabi-lock/config.toml`、なければ `~/.config/...`。
pub fn config_path(xdg_config_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let base = match xdg_config_home.filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(home.filter(|v| !v.is_empty())?).join(".config"),
    };
    Some(base.join("kabi-lock").join("config.toml"))
}

/// 設定を読む。ファイルがない・読めない・壊れているときは初期値を返す（ロックは必ず続けるため）。
pub fn load() -> Config {
    let Some(path) = config_path(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    ) else {
        return Config::default();
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Config::default(),
        Err(e) => {
            log::warn!("cannot read {}: {e}; using defaults", path.display());
            return Config::default();
        }
    };
    match parse(&text) {
        Ok(loaded) => {
            for key in &loaded.unknown_keys {
                log::warn!("{}: unknown key `{key}` ignored", path.display());
            }
            loaded.config
        }
        Err(e) => {
            log::warn!("{}: {e}; using defaults", path.display());
            Config::default()
        }
    }
}

/// 主画面を選ぶための出力の情報。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputInfo {
    /// 出力名（例: `DP-10`）。
    pub name: String,
    /// wl_output の description。sway では `make model serial (出力名)`。
    pub description: String,
}

impl OutputInfo {
    /// description から末尾の ` (出力名)` を外したもの。なければそのまま。
    fn description_without_name(&self) -> &str {
        let suffix = format!(" ({})", self.name);
        self.description
            .strip_suffix(suffix.as_str())
            .unwrap_or(&self.description)
    }

    fn matches(&self, rule: &str) -> bool {
        self.name == rule || self.description == rule || self.description_without_name() == rule
    }
}

/// 時計などを出す出力を選ぶ（`outputs` の添字）。
///
/// `rules` を上から順に照合し、最初に一致した出力。一致しなければ `focused`（出力名）、
/// それもなければ最初の出力。出力がなければ `None`。
/// 空（空白だけを含む）のルールは無視する（description が空の出力に一致してしまうため）。
pub fn select_primary(
    rules: &[String],
    outputs: &[OutputInfo],
    focused: Option<&str>,
) -> Option<usize> {
    for rule in rules.iter().filter(|r| !r.trim().is_empty()) {
        if let Some(i) = outputs.iter().position(|o| o.matches(rule)) {
            return Some(i);
        }
    }
    if let Some(name) = focused
        && let Some(i) = outputs.iter().position(|o| o.name == name)
    {
        return Some(i);
    }
    if outputs.is_empty() { None } else { Some(0) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::path::PathBuf;

    #[test]
    fn empty_file_gives_defaults() {
        let loaded = parse("").unwrap();
        let c = loaded.config;
        assert!(loaded.unknown_keys.is_empty());
        assert!(c.display.primary.is_empty());
        assert_eq!(c.appearance.fade_in_ms, 1500);
        assert_eq!(c.appearance.blur, 7.0);
        assert_eq!(c.appearance.date_format, "%m/%d");
        assert_eq!(c.input.pillar_fade_ms, 900);
        assert_eq!(c.input.pillar_width, 32);
        assert_eq!(c.input.band_width, 768);
        assert_eq!(c.auth.pam_service, "kabi-lock");
        assert!(c.auth.u2f.enabled);
        assert_eq!(c.auth.u2f.pam_service, "kabi-lock-u2f");
        assert!(c.keepassxc.lock_before);
        assert!(c.keepassxc.unlock_after);
        assert_eq!(c, Config::default());
    }

    #[test]
    fn partial_file_overrides_only_given_keys() {
        let text = r#"
            [display]
            primary = ["DP-10", "LG Electronics LG ULTRAWIDE 311NTDV3Z418"]

            [appearance]
            fade_in_ms = 1000

            [auth.u2f]
            pam_service = "u2f-touch"

            [keepassxc]
            unlock_after = false
        "#;
        let c = parse(text).unwrap().config;
        assert_eq!(
            c.display.primary,
            vec!["DP-10", "LG Electronics LG ULTRAWIDE 311NTDV3Z418"]
        );
        assert_eq!(c.appearance.fade_in_ms, 1000);
        assert_eq!(c.appearance.date_format, "%m/%d");
        assert_eq!(c.auth.u2f.pam_service, "u2f-touch");
        assert!(c.auth.u2f.enabled);
        assert!(c.keepassxc.lock_before);
        assert!(!c.keepassxc.unlock_after);
    }

    #[test]
    fn unknown_keys_are_reported_not_rejected() {
        let text = r#"
            colour = "red"
            [appearance]
            fade_in_ms = 1200
            fade_out_ms = 300
        "#;
        let loaded = parse(text).unwrap();
        assert_eq!(loaded.config.appearance.fade_in_ms, 1200);
        assert_eq!(loaded.unknown_keys.len(), 2);
        assert!(loaded.unknown_keys.iter().any(|k| k.contains("colour")));
        assert!(
            loaded
                .unknown_keys
                .iter()
                .any(|k| k.contains("fade_out_ms"))
        );
    }

    #[test]
    fn valid_date_formats_are_kept() {
        for format in ["%m/%d", "%Y-%m-%d (%a)", "%-d %B", ""] {
            assert!(is_valid_date_format(format), "{format}");
            let text = format!("[appearance]\ndate_format = {format:?}");
            assert_eq!(parse(&text).unwrap().config.appearance.date_format, format);
        }
    }

    #[test]
    fn invalid_date_format_falls_back_to_default() {
        for format in ["%Q", "%", "%m/%"] {
            assert!(!is_valid_date_format(format), "{format}");
            let text = format!("[appearance]\ndate_format = {format:?}");
            assert_eq!(
                parse(&text).unwrap().config.appearance.date_format,
                DEFAULT_DATE_FORMAT
            );
        }
    }

    #[test]
    fn huge_band_and_pillar_widths_are_clamped() {
        let text = "[input]\nband_width = 4294967295\npillar_width = 99999";
        let input = parse(text).unwrap().config.input;
        assert_eq!(input.band_width, MAX_BAND_WIDTH);
        assert_eq!(input.pillar_width, MAX_BAND_WIDTH);
    }

    #[test]
    fn huge_pillar_fade_is_clamped() {
        let text = "[input]\npillar_fade_ms = 4294967295";
        let input = parse(text).unwrap().config.input;
        assert_eq!(input.pillar_fade_ms, MAX_PILLAR_FADE_MS);
        assert_eq!(MAX_PILLAR_FADE_MS, 10_000);
        // 上限以下はそのまま
        let input = parse("[input]\npillar_fade_ms = 2500")
            .unwrap()
            .config
            .input;
        assert_eq!(input.pillar_fade_ms, 2500);
    }

    #[test]
    fn normal_widths_are_kept() {
        let input = parse("[input]\nband_width = 960\npillar_width = 24")
            .unwrap()
            .config
            .input;
        assert_eq!((input.band_width, input.pillar_width), (960, 24));
    }

    #[test]
    fn font_defaults_to_inter() {
        let loaded = parse("").unwrap();
        assert_eq!(loaded.config.appearance.font, "Inter");
        assert_eq!(Appearance::default().font, DEFAULT_FONT);
    }

    #[test]
    fn font_can_be_chosen() {
        let loaded = parse("[appearance]\nfont = \"Noto Sans\"").unwrap();
        assert!(loaded.unknown_keys.is_empty());
        assert_eq!(loaded.config.appearance.font, "Noto Sans");
        // ほかの見た目の設定は初期値のまま
        assert_eq!(loaded.config.appearance.fade_in_ms, 1500);
        assert_eq!(loaded.config.appearance.date_format, DEFAULT_DATE_FORMAT);
    }

    #[test]
    fn font_must_be_a_string() {
        assert!(parse("[appearance]\nfont = 500").is_err());
    }

    #[test]
    fn wrong_type_is_an_error() {
        assert!(parse("[appearance]\nfade_in_ms = \"slow\"").is_err());
    }

    #[test]
    fn broken_toml_is_an_error() {
        assert!(parse("[appearance").is_err());
    }

    #[test]
    fn config_path_prefers_xdg_config_home() {
        assert_eq!(
            config_path(Some(OsString::from("/x")), Some(OsString::from("/home/u"))),
            Some(PathBuf::from("/x/kabi-lock/config.toml"))
        );
    }

    #[test]
    fn config_path_falls_back_to_home() {
        assert_eq!(
            config_path(None, Some(OsString::from("/home/u"))),
            Some(PathBuf::from("/home/u/.config/kabi-lock/config.toml"))
        );
        assert_eq!(
            config_path(Some(OsString::new()), Some(OsString::from("/home/u"))),
            Some(PathBuf::from("/home/u/.config/kabi-lock/config.toml"))
        );
    }

    #[test]
    fn config_path_needs_some_base() {
        assert_eq!(config_path(None, None), None);
    }

    fn outputs() -> Vec<OutputInfo> {
        vec![
            OutputInfo {
                name: "eDP-1".into(),
                description: "Lenovo Group Limited 0x403A (eDP-1)".into(),
            },
            OutputInfo {
                name: "DP-8".into(),
                description: "LG Electronics LG FULL HD 0x01010101 (DP-8)".into(),
            },
            OutputInfo {
                name: "DP-10".into(),
                description: "LG Electronics LG ULTRAWIDE 311NTDV3Z418 (DP-10)".into(),
            },
        ]
    }

    fn rules(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn selects_by_output_name() {
        assert_eq!(
            select_primary(&rules(&["DP-10"]), &outputs(), None),
            Some(2)
        );
    }

    #[test]
    fn selects_by_description_without_name_suffix() {
        let r = rules(&["LG Electronics LG ULTRAWIDE 311NTDV3Z418"]);
        assert_eq!(select_primary(&r, &outputs(), None), Some(2));
    }

    #[test]
    fn selects_by_full_description() {
        let r = rules(&["LG Electronics LG FULL HD 0x01010101 (DP-8)"]);
        assert_eq!(select_primary(&r, &outputs(), None), Some(1));
    }

    #[test]
    fn earlier_rule_wins() {
        let r = rules(&["HDMI-A-1", "DP-8", "DP-10"]);
        assert_eq!(select_primary(&r, &outputs(), None), Some(1));
    }

    #[test]
    fn partial_name_does_not_match() {
        let r = rules(&["DP-1"]);
        assert_eq!(select_primary(&r, &outputs(), Some("DP-8")), Some(1));
    }

    #[test]
    fn falls_back_to_focused_output() {
        assert_eq!(select_primary(&[], &outputs(), Some("DP-10")), Some(2));
    }

    #[test]
    fn falls_back_to_first_output() {
        assert_eq!(select_primary(&[], &outputs(), None), Some(0));
        assert_eq!(select_primary(&[], &outputs(), Some("gone")), Some(0));
    }

    #[test]
    fn empty_rules_are_ignored() {
        let mut outs = outputs();
        outs[1].description = String::new();
        // 空のルールが description の空な出力（DP-8）に一致してはいけない
        assert_eq!(select_primary(&rules(&[""]), &outs, None), Some(0));
        assert_eq!(select_primary(&rules(&["  "]), &outs, None), Some(0));
        assert_eq!(select_primary(&rules(&["", "DP-10"]), &outs, None), Some(2));
    }

    #[test]
    fn no_outputs_means_none() {
        assert_eq!(select_primary(&rules(&["DP-10"]), &[], Some("DP-10")), None);
    }
}
