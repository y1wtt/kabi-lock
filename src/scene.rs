//! 1 フレーム分の重ね物（時計・日付・光の柱・錠）の内容と配置を計算する。
//!
//! 描画（egui・GL）には依存しない純粋なモジュール。
#![forbid(unsafe_code)]

use std::fmt::Write as _;

use chrono::{DateTime, FixedOffset, Locale};

use crate::config;

// ---------------------------------------------------------------------------
// 日付の書式とロケール

/// glibc と同じ優先順（`LC_ALL` → `LC_TIME` → `LANG`）で、日時のロケール名を選ぶ。
/// 空の値は飛ばす。どれもなければ `None`。
pub fn time_locale_name<'a>(
    lc_all: Option<&'a str>,
    lc_time: Option<&'a str>,
    lang: Option<&'a str>,
) -> Option<&'a str> {
    [lc_all, lc_time, lang]
        .into_iter()
        .flatten()
        .find(|v| !v.is_empty())
}

/// ロケール名（例: `ja_JP.UTF-8`、`de_DE.UTF-8@euro`）を chrono のロケールにする。
/// 文字コードの部分は外す。知らない名前・`C`・`POSIX`・`None` は `POSIX`。
pub fn chrono_locale(name: Option<&str>) -> Locale {
    let Some(name) = name else {
        return Locale::POSIX;
    };
    let (base, modifier) = match name.split_once('@') {
        Some((base, modifier)) => (base, Some(modifier)),
        None => (name, None),
    };
    let base = base.split('.').next().unwrap_or(base);
    if let Some(modifier) = modifier
        && let Ok(locale) = Locale::try_from(format!("{base}@{modifier}").as_str())
    {
        return locale;
    }
    Locale::try_from(base).unwrap_or(Locale::POSIX)
}

/// ロケール名から言語（`ja_JP.UTF-8` → `ja`）を取り出す。fontconfig の `lang=` に使う。
/// `C`・`POSIX`・英小文字 2〜3 字でないものは `None`（fontconfig のパターンに余計な文字を入れない）。
pub fn language(name: Option<&str>) -> Option<String> {
    let name = name?;
    let lang = name.split(['_', '.', '@']).next().unwrap_or("");
    let valid = (2..=3).contains(&lang.len()) && lang.bytes().all(|b| b.is_ascii_lowercase());
    valid.then(|| lang.to_owned())
}

/// 日付の書式とロケール。書式は作るときに確かめ、不正なら初期値に戻す。
#[derive(Clone, Debug, PartialEq)]
pub struct DateStyle {
    format: String,
    locale: Locale,
}

impl DateStyle {
    pub fn new(format: &str, locale: Locale) -> DateStyle {
        let format = if config::is_valid_date_format(format) {
            format
        } else {
            config::DEFAULT_DATE_FORMAT
        };
        DateStyle {
            format: format.to_owned(),
            locale,
        }
    }

    /// 日付を整形する。整形に失敗しても panic せず、初期値の書式で整形し直す。
    pub fn format(&self, t: &DateTime<FixedOffset>) -> String {
        let mut out = String::new();
        if write!(out, "{}", t.format_localized(&self.format, self.locale)).is_ok() {
            return out;
        }
        t.format(config::DEFAULT_DATE_FORMAT).to_string()
    }
}

/// Inter が持っている文字か（ラテン・ギリシャ・キリル文字と主な記号）。
/// ここにない文字（日本語など）は fontconfig で探したフォントで補う。
pub fn inter_covers(c: char) -> bool {
    matches!(
        c as u32,
        0x20..=0x7E
            | 0xA0..=0x24F
            | 0x370..=0x3FF
            | 0x400..=0x52F
            | 0x1E00..=0x1EFF
            | 0x2000..=0x206F
            | 0x20A0..=0x20CF
            | 0x2100..=0x214F
    )
}

/// この書式とロケールの日付に、Inter にない文字が出うるか。
/// 12 か月と 7 曜日、午前・午後を整形して確かめる。
pub fn needs_fallback_font(style: &DateStyle) -> bool {
    let utc = FixedOffset::east_opt(0).expect("zero offset is valid");
    let months = (1..=12).map(|m| (2026, m, 1, 0));
    // 2026-11-23 は月曜日。正午にして午後の表記も確かめる
    let weekdays = (23..=29).map(|d| (2026, 11, d, 12));
    months.chain(weekdays).any(|(y, m, d, h)| {
        let t = chrono::TimeZone::with_ymd_and_hms(&utc, y, m, d, h, 0, 0)
            .single()
            .expect("sample dates are valid");
        !style.format(&t).chars().all(inter_covers)
    })
}

/// `fc-match --format='%{file}\n%{index}'` の出力からファイル名とフェイスの番号を取り出す。
pub fn parse_fc_match(output: &str) -> Option<(String, u32)> {
    let mut lines = output.lines();
    let file = lines.next()?.trim();
    if file.is_empty() {
        return None;
    }
    let index = lines
        .next()
        .and_then(|l| l.trim().parse().ok())
        .unwrap_or(0);
    Some((file.to_owned(), index))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn jst(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<FixedOffset> {
        FixedOffset::east_opt(9 * 3600)
            .unwrap()
            .with_ymd_and_hms(y, m, d, h, min, 0)
            .unwrap()
    }

    #[test]
    fn locale_name_follows_glibc_priority() {
        let ja = Some("ja_JP.UTF-8");
        let en = Some("en_US.UTF-8");
        assert_eq!(time_locale_name(Some("de_DE"), ja, en), Some("de_DE"));
        assert_eq!(time_locale_name(Some(""), ja, en), Some("ja_JP.UTF-8"));
        assert_eq!(time_locale_name(None, None, en), Some("en_US.UTF-8"));
        assert_eq!(time_locale_name(None, Some(""), None), None);
    }

    #[test]
    fn locale_name_maps_to_chrono_locale() {
        assert_eq!(chrono_locale(Some("ja_JP.UTF-8")), Locale::ja_JP);
        assert_eq!(chrono_locale(Some("en_DK.UTF-8")), Locale::en_DK);
        assert_eq!(chrono_locale(Some("de_DE.UTF-8@euro")), Locale::de_DE_euro);
        assert_eq!(chrono_locale(Some("C.UTF-8")), Locale::POSIX);
        assert_eq!(chrono_locale(Some("POSIX")), Locale::POSIX);
        assert_eq!(chrono_locale(Some("xx_YY")), Locale::POSIX);
        assert_eq!(chrono_locale(None), Locale::POSIX);
    }

    #[test]
    fn language_is_the_lowercase_prefix() {
        assert_eq!(language(Some("ja_JP.UTF-8")).as_deref(), Some("ja"));
        assert_eq!(language(Some("fil_PH")).as_deref(), Some("fil"));
        assert_eq!(language(Some("C.UTF-8")), None);
        assert_eq!(language(Some("POSIX")), None);
        assert_eq!(language(Some("x:y")), None);
        assert_eq!(language(None), None);
    }

    #[test]
    fn default_date_format_is_month_and_day() {
        let style = DateStyle::new("%m/%d", Locale::POSIX);
        assert_eq!(style.format(&jst(2026, 11, 24, 10, 30)), "11/24");
    }

    #[test]
    fn date_follows_the_locale() {
        let t = jst(2026, 11, 24, 10, 30);
        assert_eq!(DateStyle::new("%a", Locale::ja_JP).format(&t), "火");
        assert_eq!(DateStyle::new("%a", Locale::POSIX).format(&t), "Tue");
    }

    #[test]
    fn invalid_date_format_uses_the_default() {
        let style = DateStyle::new("%Q", Locale::POSIX);
        assert_eq!(style.format(&jst(2026, 1, 2, 0, 0)), "01/02");
    }

    #[test]
    fn fallback_font_is_needed_only_for_uncovered_text() {
        let needs =
            |format: &str, locale: Locale| needs_fallback_font(&DateStyle::new(format, locale));
        assert!(!needs("%m/%d", Locale::ja_JP));
        assert!(!needs("%a %d %b", Locale::en_US));
        assert!(!needs("%A %B", Locale::ru_RU));
        assert!(!needs("%A %B", Locale::de_DE));
        assert!(needs("%m/%d (%a)", Locale::ja_JP));
        assert!(needs("%m月%d日", Locale::POSIX));
    }

    #[test]
    fn fc_match_output_is_parsed() {
        assert_eq!(
            parse_fc_match("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc\n2"),
            Some((
                "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc".into(),
                2
            ))
        );
        assert_eq!(parse_fc_match("/a/b.ttf\n"), Some(("/a/b.ttf".into(), 0)));
        assert_eq!(parse_fc_match(""), None);
        assert_eq!(parse_fc_match("\n0"), None);
    }
}
