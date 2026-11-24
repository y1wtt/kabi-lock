//! 出力の情報（名前・説明）。

use smithay_client_toolkit::output::OutputInfo;

/// ロック中に覚えておく出力の情報。主画面の選択（Plan 4）とログに使う。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputMeta {
    /// 出力名（例: `DP-10`）。コンポジタが送ってこなければ空。
    pub name: String,
    /// wl_output の description。送ってこなければ空。
    pub description: String,
}

impl OutputMeta {
    /// SCTK の出力情報から作る。情報がまだ届いていなければ両方とも空。
    pub fn from_info(info: Option<&OutputInfo>) -> OutputMeta {
        OutputMeta::from_parts(
            info.and_then(|i| i.name.clone()),
            info.and_then(|i| i.description.clone()),
        )
    }

    pub fn from_parts(name: Option<String>, description: Option<String>) -> OutputMeta {
        OutputMeta {
            name: name.unwrap_or_default(),
            description: description.unwrap_or_default(),
        }
    }

    /// ログに出す名前。
    pub fn label(&self) -> &str {
        if self.name.is_empty() {
            "(unnamed output)"
        } else {
            &self.name
        }
    }

    /// `config::select_primary` に渡す形にする。
    pub fn to_output_info(&self) -> crate::config::OutputInfo {
        crate::config::OutputInfo {
            name: self.name.clone(),
            description: self.description.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_name_and_description() {
        let meta = OutputMeta::from_parts(
            Some("DP-10".into()),
            Some("LG Electronics LG ULTRAWIDE 311NTDV3Z418 (DP-10)".into()),
        );
        assert_eq!(meta.name, "DP-10");
        assert_eq!(
            meta.description,
            "LG Electronics LG ULTRAWIDE 311NTDV3Z418 (DP-10)"
        );
        assert_eq!(meta.label(), "DP-10");
    }

    #[test]
    fn missing_values_become_empty() {
        let meta = OutputMeta::from_parts(None, None);
        assert_eq!(meta.name, "");
        assert_eq!(meta.description, "");
        assert_eq!(meta.label(), "(unnamed output)");
    }

    #[test]
    fn converts_to_config_output_info() {
        let meta = OutputMeta::from_parts(Some("eDP-1".into()), Some("panel (eDP-1)".into()));
        let info = meta.to_output_info();
        assert_eq!(info.name, "eDP-1");
        assert_eq!(info.description, "panel (eDP-1)");
    }
}
