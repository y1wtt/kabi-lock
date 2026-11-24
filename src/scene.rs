//! 1 フレーム分の重ね物（時計・日付・光の柱・錠）の内容と配置を計算する。
//!
//! 描画（egui・GL）には依存しない純粋なモジュール。
#![forbid(unsafe_code)]

use std::fmt::Write as _;

use chrono::{DateTime, FixedOffset, Locale};

use crate::anim::{self, PillarFrame, ShacklePath};
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

// ---------------------------------------------------------------------------
// 配置（設計書 3 章。1920x1080 基準の値を出力の高さに比例させる）

/// 配置の基準にする画面の高さ。
pub const REFERENCE_HEIGHT: f32 = 1080.0;
/// 錠の領域の上端（時計グループの上端）。
const LOCK_TOP: f32 = 130.0;
/// 錠の領域の高さ。アイコン（64x56）を 60x52 の枠に縦横比を保って収める。
const LOCK_BOX_H: f32 = 52.0;
/// 時計の行の中心。錠 52 + 下余白 18 + 間隔 14 の下に、行の高さ 104（line-height 1）。
const CLOCK_CENTER_Y: f32 = LOCK_TOP + LOCK_BOX_H + 18.0 + 14.0 + CLOCK_SIZE / 2.0;
const CLOCK_SIZE: f32 = 104.0;
const CLOCK_LETTER_SPACING: f32 = -2.0;
/// 日付の行の上端（時計の行の下端 + 間隔 14）。
const DATE_TOP: f32 = CLOCK_CENTER_Y + CLOCK_SIZE / 2.0 + 14.0;
const DATE_SIZE: f32 = 26.0;
const DATE_LETTER_SPACING: f32 = 1.0;
/// 日付の行の高さ。Inter の行の高さ（(ascender + descender) / UPM ≈ 1.21）× 26px。
const DATE_LINE_H: f32 = 31.5;
/// 時計グループの下端から帯の上端まで。
const BAND_GAP: f32 = 374.0;
/// 帯（光の柱）の高さ。
pub const BAND_H: f32 = 72.0;

/// 出力 1 枚分の配置（px、出力の左上が原点）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    /// 1920x1080 基準からの拡大率（出力の高さ / 1080）。
    pub scale: f32,
    pub center_x: f32,
    /// 錠のアイコン座標 (0, 0) の位置。
    pub icon_origin: [f32; 2],
    /// アイコン座標 1 単位の長さ。
    pub icon_unit: f32,
    pub clock_center_y: f32,
    pub clock_size: f32,
    pub clock_letter_spacing: f32,
    pub date_top: f32,
    pub date_size: f32,
    pub date_letter_spacing: f32,
    pub band_left: f32,
    pub band_top: f32,
    pub band_bottom: f32,
}

pub fn layout(width: u32, height: u32, band_width: u32) -> Layout {
    let s = height as f32 / REFERENCE_HEIGHT;
    let center_x = width as f32 / 2.0;
    let icon_unit = LOCK_BOX_H / anim::ICON_H * s;
    let band_top = DATE_TOP + DATE_LINE_H + BAND_GAP;
    Layout {
        scale: s,
        center_x,
        icon_origin: [center_x - anim::ICON_W / 2.0 * icon_unit, LOCK_TOP * s],
        icon_unit,
        clock_center_y: CLOCK_CENTER_Y * s,
        clock_size: CLOCK_SIZE * s,
        clock_letter_spacing: CLOCK_LETTER_SPACING * s,
        date_top: DATE_TOP * s,
        date_size: DATE_SIZE * s,
        date_letter_spacing: DATE_LETTER_SPACING * s,
        band_left: center_x - band_width as f32 * s / 2.0,
        band_top: band_top * s,
        band_bottom: (band_top + BAND_H) * s,
    }
}

/// 表示中の光の柱 1 本。`x`・`width` は帯の左端を 0 とした 1920x1080 基準の px。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PillarDraw {
    pub x: f32,
    pub width: f32,
    pub frame: PillarFrame,
}

/// 柱を描く四角形（左, 上, 右, 下）。下端を基準に縦に `scale_y` 倍する。
pub fn pillar_rect(l: &Layout, p: &PillarDraw) -> [f32; 4] {
    let left = l.band_left + p.x * l.scale;
    let right = left + p.width * l.scale;
    let top = l.band_bottom - BAND_H * l.scale * p.frame.scale_y;
    [left, top, right, l.band_bottom]
}

/// 柱の横方向の色の区切り（0 = 左端、1 = 右端）。両端 0.85 → 22% で 0.38 → 78% まで 0.38 → 右端 0.85。
/// 区切りの間は直線なので、この位置に頂点を置けば横方向は正確になる。
/// 端の区間は縦の変化と掛け合わさるので 2 つに分けて誤差を減らす。
pub const PILLAR_COLUMNS: [f32; 6] = [0.0, 0.11, 0.22, 0.78, 0.89, 1.0];
/// 柱の縦方向の分割数。
pub const PILLAR_ROWS: usize = 8;

/// 柱の中の位置（`u` 横 0〜1、`v` 縦 0 = 上端〜1 = 下端）の不透明度。
/// 横のグラデーションと、上が透明・下が不透明の縦のマスクの積。
pub fn pillar_alpha(u: f32, v: f32) -> f32 {
    const EDGE: f32 = 0.85;
    const MID: f32 = 0.38;
    let u = u.clamp(0.0, 1.0);
    let across = if u < 0.22 {
        EDGE + (MID - EDGE) * (u / 0.22)
    } else if u <= 0.78 {
        MID
    } else {
        MID + (EDGE - MID) * ((u - 0.78) / 0.22)
    };
    across * v.clamp(0.0, 1.0)
}

/// 短い足と長い足がこれより近いときは、真横から見た 1 本の棒として描く。
const EDGE_ON: f32 = 0.5;

/// U 字を折れ線（アイコン座標）にする。短い足の下端から始まり、長い足の下端で終わる。
/// 各ベジェは `segments_per_arc` 個に分ける。描画側は最初の点に丸い端を付ける。
///
/// 短い足が長い足と重なる（真横から見た）ときは、頂点から長い足の下端までの 2 点を返す
/// （行って戻る折れ線は線の継ぎ目が壊れるため）。
pub fn shackle_polyline(path: &ShacklePath, segments_per_arc: usize) -> Vec<[f32; 2]> {
    let apex = path.arc1[2];
    if (path.short_leg_top[0] - path.arc2[2][0]).abs() < EDGE_ON {
        return vec![apex, path.long_leg_bottom];
    }
    let n = segments_per_arc.max(1);
    let mut points = vec![path.short_leg_bottom, path.short_leg_top];
    for (start, [c1, c2, end]) in [(path.short_leg_top, path.arc1), (apex, path.arc2)] {
        for i in 1..=n {
            points.push(cubic(start, c1, c2, end, i as f32 / n as f32));
        }
    }
    points.push(path.long_leg_bottom);
    points
}

fn cubic(p0: [f32; 2], p1: [f32; 2], p2: [f32; 2], p3: [f32; 2], t: f32) -> [f32; 2] {
    let u = 1.0 - t;
    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    [
        a * p0[0] + b * p1[0] + c * p2[0] + d * p3[0],
        a * p0[1] + b * p1[1] + c * p2[1] + d * p3[1],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

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

    // --- 配置

    #[test]
    fn layout_matches_the_design_at_1080p() {
        let l = layout(1920, 1080, 768);
        assert_eq!(l.scale, 1.0);
        assert_eq!(l.center_x, 960.0);
        assert_eq!(l.icon_origin[1], 130.0);
        // アイコン 64 単位の幅が中央にくる
        assert!(close(l.icon_origin[0] + 32.0 * l.icon_unit, 960.0));
        assert!(close(l.icon_unit * 56.0, 52.0));
        assert_eq!(l.clock_center_y, 266.0);
        assert_eq!(l.clock_size, 104.0);
        assert_eq!(l.date_top, 332.0);
        assert_eq!(l.date_size, 26.0);
        assert_eq!(l.band_left, 576.0);
        assert_eq!(l.band_top, 737.5);
        assert_eq!(l.band_bottom, 809.5);
    }

    #[test]
    fn layout_scales_with_the_output_height() {
        let l = layout(3440, 1440, 768);
        let s = 1440.0 / 1080.0;
        assert!(close(l.scale, s));
        assert_eq!(l.center_x, 1720.0);
        assert!(close(l.clock_size, 104.0 * s));
        assert!(close(l.band_bottom, 809.5 * s));
        assert!(close(l.band_left, 1720.0 - 384.0 * s));
        assert!(close(l.clock_letter_spacing, -2.0 * s));
    }

    #[test]
    fn pillar_grows_from_the_band_bottom() {
        let l = layout(1920, 1080, 768);
        let p = PillarDraw {
            x: 8.0,
            width: 32.0,
            frame: PillarFrame {
                scale_y: 0.5,
                opacity: 1.0,
            },
        };
        assert_eq!(pillar_rect(&l, &p), [584.0, 773.5, 616.0, 809.5]);
    }

    #[test]
    fn pillar_alpha_has_bright_edges_and_fades_upward() {
        assert!(close(pillar_alpha(0.0, 1.0), 0.85));
        assert!(close(pillar_alpha(1.0, 1.0), 0.85));
        assert!(close(pillar_alpha(0.22, 1.0), 0.38));
        assert!(close(pillar_alpha(0.5, 1.0), 0.38));
        assert!(close(pillar_alpha(0.78, 1.0), 0.38));
        assert!(close(pillar_alpha(0.11, 1.0), (0.85 + 0.38) / 2.0));
        assert_eq!(pillar_alpha(0.0, 0.0), 0.0);
        assert!(close(pillar_alpha(0.5, 0.5), 0.19));
    }

    #[test]
    fn pillar_columns_include_the_gradient_stops() {
        for stop in [0.0, 0.22, 0.78, 1.0] {
            assert!(PILLAR_COLUMNS.contains(&stop));
        }
        assert!(PILLAR_COLUMNS.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn closed_shackle_polyline_runs_from_short_leg_to_long_leg() {
        let path = anim::shackle_path(anim::shackle_at(0.0));
        let pts = shackle_polyline(&path, 8);
        assert_eq!(pts.len(), 2 + 8 * 2 + 1);
        assert_eq!(pts[0], [22.0, 26.0]);
        assert_eq!(pts[1], [22.0, 18.0]);
        assert!(pts.contains(&[32.0, 8.0]));
        assert_eq!(*pts.last().unwrap(), [42.0, 40.0]);
        for p in &pts {
            assert!((22.0 - 1e-3..=42.0 + 1e-3).contains(&p[0]));
            assert!((8.0 - 1e-3..=40.0).contains(&p[1]));
        }
        // 同じ点が続かない（継ぎ目の計算が壊れないように）
        assert!(pts.windows(2).all(|w| w[0] != w[1]));
    }

    #[test]
    fn edge_on_shackle_is_a_single_rod() {
        let path = anim::shackle_path(anim::shackle_at(530.0));
        let pts = shackle_polyline(&path, 8);
        assert_eq!(pts.len(), 2);
        assert!(close(pts[0][0], anim::LONG_LEG_X));
        assert_eq!(pts[1], path.long_leg_bottom);
        assert!(pts[0][1] < pts[1][1]);
    }

    #[test]
    fn swivelled_shackle_polyline_is_finite() {
        for ms in (0..=800).step_by(20) {
            let path = anim::shackle_path(anim::shackle_at(ms as f32));
            let pts = shackle_polyline(&path, 12);
            assert!(pts.len() >= 2);
            assert!(pts.iter().flatten().all(|v| v.is_finite()));
        }
    }
}
