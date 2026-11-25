//! 1 フレーム分の重ね物（時計・日付・光の柱・錠）の内容と配置を計算する。
//!
//! 描画（egui・GL）には依存しない純粋なモジュール。
#![forbid(unsafe_code)]

use std::fmt::Write as _;

use chrono::{DateTime, FixedOffset, Locale};

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
///
/// 本体に入る足は本体の上端で止める。解除中は錠ごと半透明になるので、本体の下に足を
/// 描いておくと透けて見えてしまう。足は本体の上辺の平らな所（角丸の外）にあるので、
/// 上端で止めても隙間はできない。
pub fn shackle_polyline(path: &ShacklePath, segments_per_arc: usize) -> Vec<[f32; 2]> {
    let mut points = full_shackle_polyline(path, segments_per_arc);
    let last = points.len() - 1;
    for i in [0, last] {
        points[i] = stop_at_body_top(points[i]);
    }
    points
}

/// 足の下端が本体の中にあれば、本体の上端まで引き上げる。
fn stop_at_body_top([x, y]: [f32; 2]) -> [f32; 2] {
    let over_body = (anim::BODY_X..=anim::BODY_X + anim::BODY_W).contains(&x);
    if over_body && y > anim::BODY_Y {
        [x, anim::BODY_Y]
    } else {
        [x, y]
    }
}

fn full_shackle_polyline(path: &ShacklePath, segments_per_arc: usize) -> Vec<[f32; 2]> {
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

use crate::anim::{self, PillarFrame, ShacklePath};
use crate::config::{self, Input, OutputInfo};
use crate::lanes::{self, Pillar};
use crate::state::{Machine, Phase};

// ---------------------------------------------------------------------------
// 1 フレーム分の内容

/// 錠。`path` はアイコン座標、`shake_x` は 1920x1080 基準の px（右が正）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LockDraw {
    pub path: ShacklePath,
    pub shake_x: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    /// 重ね物全体の不透明度。`fade_alpha * (1 - reveal_progress)`。
    pub opacity: f32,
    /// `HH:MM`。
    pub time_text: String,
    /// `date_format` で整形済みの日付。
    pub date_text: String,
    pub pillars: Vec<PillarDraw>,
    /// 錠を出すときだけ `Some`。
    pub lock: Option<LockDraw>,
}

/// 1 フレーム分の重ね物の内容を作る。
///
/// - `fade_alpha`: 背景と同じ黒からのフェードの不透明度（`anim::fade_alpha`）。
/// - `pillars`: `LaneAllocator::pillars(now_ms)` の結果（`input.pillar_fade_ms` と同じ寿命で割り当てたもの）。
/// - `wall`: 表示する時刻（呼び出し側で `Local::now().fixed_offset()`）。
pub fn build(
    machine: &Machine,
    now_ms: u64,
    fade_alpha: f32,
    pillars: &[Pillar],
    input: &Input,
    wall: &DateTime<FixedOffset>,
    date: &DateStyle,
) -> Scene {
    let phase = machine.phase();
    let elapsed = phase.elapsed_ms(now_ms).unwrap_or(0.0);
    let timing = machine.unlock_timing();
    let reveal = match phase {
        Phase::Unlocking { .. } => anim::reveal_progress(elapsed, &timing),
        Phase::Done => 1.0,
        _ => 0.0,
    };
    let fade = if fade_alpha.is_nan() {
        0.0
    } else {
        fade_alpha.clamp(0.0, 1.0)
    };

    let closed = anim::shackle_path(anim::ShackleGeom::CLOSED);
    let lock = machine.lock_visible().then(|| match phase {
        Phase::Unlocking { .. } => LockDraw {
            path: anim::shackle_path(anim::shackle_at(elapsed, &timing)),
            shake_x: 0.0,
        },
        Phase::Failed { .. } => LockDraw {
            path: closed,
            shake_x: anim::shake_offset(elapsed),
        },
        _ => LockDraw {
            path: closed,
            shake_x: 0.0,
        },
    });

    // 柱は入力を受け付けている間だけ。失敗中は、失敗した後に打った柱だけを出す
    // （Enter より前の柱は試作と同じく消す）。
    let shown_since = match phase {
        Phase::Idle => Some(0),
        Phase::Failed { since_ms } => Some(since_ms),
        _ => None,
    };
    let life_ms = input.pillar_fade_ms as f32;
    let pillars = match shown_since {
        Some(since) => pillars
            .iter()
            .filter(|p| p.started_ms >= since)
            .filter_map(|p| {
                let frame = anim::pillar_at(now_ms.saturating_sub(p.started_ms) as f32, life_ms)?;
                Some(PillarDraw {
                    x: lanes::lane_x(p.lane, input.band_width, input.pillar_width),
                    width: input.pillar_width as f32,
                    frame,
                })
            })
            .collect(),
        None => Vec::new(),
    };

    Scene {
        opacity: fade * (1.0 - reveal),
        time_text: wall.format("%H:%M").to_string(),
        date_text: date.format(wall),
        pillars,
        lock,
    }
}

// ---------------------------------------------------------------------------
// 主画面

/// 主画面の選択を覚えておき、出力の一覧が変わったときだけ選び直す。
#[derive(Debug)]
pub struct PrimaryTracker {
    rules: Vec<String>,
    focused: Option<String>,
    seen: Vec<OutputInfo>,
    primary: Option<usize>,
    /// 主画面の出力名（添字は出力が抜かれるとずれるので、同じ出力かは名前で見る）。
    name: Option<String>,
    /// 直前の `update` で主画面の出力名が替わったか。
    changed: bool,
    /// 替わる前の主画面の出力名。
    previous: Option<String>,
}

impl PrimaryTracker {
    /// `rules` は `display.primary`、`focused` はロック前にフォーカスしていた出力名（sway IPC）。
    pub fn new(rules: Vec<String>, focused: Option<String>) -> PrimaryTracker {
        PrimaryTracker {
            rules,
            focused,
            seen: Vec::new(),
            primary: None,
            name: None,
            changed: false,
            previous: None,
        }
    }

    /// 出力の一覧（出力名, description）を渡し、主画面の添字を返す。
    /// 一覧が前回と同じなら選び直さない（毎フレーム呼んでも文字列を確保しない）。
    pub fn update<'a, I>(&mut self, outputs: I) -> Option<usize>
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
        I::IntoIter: Clone,
    {
        let outputs = outputs.into_iter();
        let same = outputs.clone().count() == self.seen.len()
            && outputs
                .clone()
                .zip(&self.seen)
                .all(|((name, desc), seen)| seen.name == name && seen.description == desc);
        if !same {
            self.seen = outputs
                .map(|(name, desc)| OutputInfo {
                    name: name.to_owned(),
                    description: desc.to_owned(),
                })
                .collect();
            self.primary = config::select_primary(&self.rules, &self.seen, self.focused.as_deref());
            let name = self
                .primary
                .and_then(|i| self.seen.get(i))
                .map(|o| o.name.as_str());
            self.changed = name != self.name.as_deref();
            if self.changed {
                let name = name.map(str::to_owned);
                self.previous = std::mem::replace(&mut self.name, name);
            }
        } else {
            self.changed = false;
        }
        self.primary
    }

    /// 最後に `update` で選んだ主画面。
    pub fn primary(&self) -> Option<usize> {
        self.primary
    }

    /// 直前の `update` で主画面の出力が替わったか。添字ではなく出力名で比べるので、
    /// 主画面が抜かれて別の出力が同じ添字に詰めてきたときも替わったと分かる。
    pub fn changed(&self) -> bool {
        self.changed
    }

    /// 直前の `update` で主画面が替わったとき、前の主画面の出力名（前がなかったら `None`）。
    pub fn previous_name(&self) -> Option<&str> {
        if self.changed {
            self.previous.as_deref()
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::{ShackleGeom, UnlockTiming};
    use crate::state::{Event, FAIL_HOLD_MS};
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
        let path = anim::shackle_path(ShackleGeom::CLOSED);
        let pts = shackle_polyline(&path, 8);
        assert_eq!(pts.len(), 2 + 8 * 2 + 1);
        // 足の下端（26 と 40）は本体の上端（24）で止まる
        assert_eq!(pts[0], [22.0, 24.0]);
        assert_eq!(pts[1], [22.0, 18.0]);
        assert!(pts.contains(&[32.0, 8.0]));
        assert_eq!(*pts.last().unwrap(), [42.0, 24.0]);
        for p in &pts {
            assert!((22.0 - 1e-3..=42.0 + 1e-3).contains(&p[0]));
            assert!((8.0 - 1e-3..=24.0).contains(&p[1]));
        }
        // 同じ点が続かない（継ぎ目の計算が壊れないように）
        assert!(pts.windows(2).all(|w| w[0] != w[1]));
    }

    #[test]
    fn edge_on_shackle_is_a_single_rod() {
        let path = anim::shackle_path(anim::shackle_at(530.0, &UnlockTiming::default()));
        let pts = shackle_polyline(&path, 8);
        assert_eq!(pts.len(), 2);
        assert!(close(pts[0][0], anim::LONG_LEG_X));
        assert_eq!(pts[1], [path.long_leg_bottom[0], anim::BODY_Y]);
        assert!(pts[0][1] < pts[1][1]);
    }

    #[test]
    fn shackle_polyline_never_enters_the_body() {
        // 解除中は錠ごと半透明になるので、本体の下に足があると透けて見える
        let body_x = anim::BODY_X..=anim::BODY_X + anim::BODY_W;
        let mut geoms = vec![ShackleGeom::CLOSED];
        geoms.extend(
            (0..=800)
                .step_by(10)
                .map(|ms| anim::shackle_at(ms as f32, &UnlockTiming::default())),
        );
        for g in geoms {
            let pts = shackle_polyline(&anim::shackle_path(g), 8);
            for p in &pts {
                assert!(
                    !(body_x.contains(&p[0]) && p[1] > anim::BODY_Y),
                    "{g:?}: {p:?} is inside the body"
                );
            }
            assert!(pts.windows(2).all(|w| w[0] != w[1]), "{g:?}");
        }
    }

    #[test]
    fn swivelled_shackle_polyline_is_finite() {
        for ms in (0..=800).step_by(20) {
            let path = anim::shackle_path(anim::shackle_at(ms as f32, &UnlockTiming::default()));
            let pts = shackle_polyline(&path, 12);
            assert!(pts.len() >= 2);
            assert!(pts.iter().flatten().all(|v| v.is_finite()));
        }
    }

    // --- 1 フレーム分の内容

    fn input() -> Input {
        Input::default()
    }

    fn date() -> DateStyle {
        DateStyle::new("%m/%d", Locale::POSIX)
    }

    fn scene(m: &Machine, now: u64, fade: f32, pillars: &[Pillar]) -> Scene {
        build(
            m,
            now,
            fade,
            pillars,
            &input(),
            &jst(2026, 11, 24, 9, 5),
            &date(),
        )
    }

    fn failed_at(t: u64) -> Machine {
        let mut m = Machine::new();
        m.handle(Event::Submit { empty: false }, t);
        m.handle(Event::PasswordChecked { ok: false }, t);
        m
    }

    fn unlocking_at(t: u64) -> Machine {
        let mut m = Machine::new();
        m.handle(Event::U2fSucceeded, t);
        m
    }

    #[test]
    fn idle_shows_clock_and_date_without_lock() {
        let s = scene(&Machine::new(), 0, 1.0, &[]);
        assert_eq!(s.time_text, "09:05");
        assert_eq!(s.date_text, "11/24");
        assert_eq!(s.opacity, 1.0);
        assert!(s.lock.is_none());
        assert!(s.pillars.is_empty());
    }

    #[test]
    fn opacity_follows_the_fade_in() {
        assert_eq!(scene(&Machine::new(), 0, 0.0, &[]).opacity, 0.0);
        assert!(close(scene(&Machine::new(), 0, 0.4, &[]).opacity, 0.4));
        assert_eq!(scene(&Machine::new(), 0, 7.0, &[]).opacity, 1.0);
        assert_eq!(scene(&Machine::new(), 0, f32::NAN, &[]).opacity, 0.0);
    }

    #[test]
    fn opacity_is_fade_times_one_minus_reveal() {
        let m = unlocking_at(1000);
        // 初期値では 900ms から 700ms かけて晴れる
        let s = scene(&m, 1000 + 900 + 350, 0.8, &[]);
        assert!(close(s.opacity, 0.8 * 0.5));
        let s = scene(&m, 1000 + 900, 1.0, &[]);
        assert_eq!(s.opacity, 1.0);
        let s = scene(&m, 1000 + crate::state::UNLOCK_TOTAL_MS, 1.0, &[]);
        assert_eq!(s.opacity, 0.0);
    }

    #[test]
    fn verifying_shows_the_closed_lock() {
        let mut m = Machine::new();
        m.handle(Event::Submit { empty: false }, 10);
        let s = scene(&m, 500, 1.0, &[]);
        let lock = s.lock.unwrap();
        assert_eq!(lock.path, anim::shackle_path(ShackleGeom::CLOSED));
        assert_eq!(lock.shake_x, 0.0);
    }

    #[test]
    fn failure_shakes_the_closed_lock() {
        let m = failed_at(100);
        let at = 100 + (0.12 * anim::SHAKE_MS) as u64;
        let s = scene(&m, at, 1.0, &[]);
        let lock = s.lock.unwrap();
        assert!(close(lock.shake_x, anim::shake_offset(at as f32 - 100.0)));
        assert!(lock.shake_x < -9.0);
        assert_eq!(lock.path, anim::shackle_path(ShackleGeom::CLOSED));
        // 震え終わっても、入力待ちに戻るまでは錠を出したまま
        let s = scene(&m, 100 + FAIL_HOLD_MS - 1, 1.0, &[]);
        assert_eq!(s.lock.unwrap().shake_x, 0.0);
    }

    #[test]
    fn lock_hides_again_after_the_failure_hold() {
        let mut m = failed_at(100);
        m.handle(Event::Tick, 100 + FAIL_HOLD_MS);
        assert!(scene(&m, 100 + FAIL_HOLD_MS, 1.0, &[]).lock.is_none());
    }

    #[test]
    fn unlocking_opens_the_shackle() {
        let m = unlocking_at(1000);
        let s = scene(&m, 1000 + 400, 1.0, &[]);
        let lock = s.lock.unwrap();
        assert_eq!(
            lock.path,
            anim::shackle_path(anim::shackle_at(400.0, &UnlockTiming::default()))
        );
        assert_eq!(lock.shake_x, 0.0);
    }

    #[test]
    fn unlocking_follows_the_machine_unlock_timing() {
        let fast = UnlockTiming {
            open_ms: 400.0,
            reveal_ms: 300.0,
        };
        let mut m = Machine::with_unlock_timing(fast);
        m.handle(Event::U2fSucceeded, 1000);
        // 400ms で錠が開き切る
        let open = scene(&m, 1000 + 400, 1.0, &[]).lock.unwrap();
        assert_eq!(
            open.path,
            anim::shackle_path(anim::shackle_at(800.0, &UnlockTiming::default()))
        );
        // 450ms から 300ms かけて重ね物が消える
        assert_eq!(scene(&m, 1000 + 450, 1.0, &[]).opacity, 1.0);
        assert!(close(scene(&m, 1000 + 600, 1.0, &[]).opacity, 0.5));
        assert_eq!(scene(&m, 1000 + 750, 1.0, &[]).opacity, 0.0);
    }

    #[test]
    fn instant_unlock_timing_never_shows_the_closed_lock() {
        let mut m = Machine::with_unlock_timing(UnlockTiming {
            open_ms: 0.0,
            reveal_ms: 0.0,
        });
        m.handle(Event::U2fSucceeded, 1000);
        let s = scene(&m, 1000, 1.0, &[]);
        assert_ne!(
            s.lock.unwrap().path,
            anim::shackle_path(ShackleGeom::CLOSED)
        );
        assert_eq!(s.opacity, 0.0);
    }

    #[test]
    fn pillars_use_lane_positions_and_the_pillar_animation() {
        let pillars = [
            Pillar {
                lane: 0,
                started_ms: 1000,
            },
            Pillar {
                lane: 15,
                started_ms: 1100,
            },
        ];
        let s = scene(&Machine::new(), 1108, 1.0, &pillars);
        assert_eq!(s.pillars.len(), 2);
        assert_eq!(s.pillars[0].x, 8.0);
        assert_eq!(s.pillars[1].x, 728.0);
        assert_eq!(s.pillars[0].width, 32.0);
        assert_eq!(s.pillars[0].frame, anim::pillar_at(108.0, 900.0).unwrap());
        assert_eq!(s.pillars[1].frame, anim::pillar_at(8.0, 900.0).unwrap());
    }

    #[test]
    fn expired_pillars_are_not_drawn() {
        let pillars = [Pillar {
            lane: 3,
            started_ms: 0,
        }];
        assert!(
            scene(&Machine::new(), 900, 1.0, &pillars)
                .pillars
                .is_empty()
        );
    }

    #[test]
    fn pillars_hide_while_verifying_and_unlocking() {
        let pillars = [Pillar {
            lane: 3,
            started_ms: 0,
        }];
        let mut m = Machine::new();
        m.handle(Event::Submit { empty: false }, 10);
        assert!(scene(&m, 20, 1.0, &pillars).pillars.is_empty());
        assert!(
            scene(&unlocking_at(10), 20, 1.0, &pillars)
                .pillars
                .is_empty()
        );
    }

    #[test]
    fn after_a_failure_only_new_pillars_are_drawn() {
        let pillars = [
            Pillar {
                lane: 1,
                started_ms: 50,
            },
            Pillar {
                lane: 2,
                started_ms: 150,
            },
        ];
        let s = scene(&failed_at(100), 200, 1.0, &pillars);
        assert_eq!(s.pillars.len(), 1);
        assert_eq!(s.pillars[0].x, lanes::lane_x(2, 768, 32));
    }

    #[test]
    fn pillar_width_comes_from_the_config() {
        let wide = Input {
            pillar_width: 64,
            ..Input::default()
        };
        let pillars = [Pillar {
            lane: 0,
            started_ms: 0,
        }];
        let s = build(
            &Machine::new(),
            10,
            1.0,
            &pillars,
            &wide,
            &jst(2026, 11, 24, 9, 5),
            &date(),
        );
        assert_eq!(s.pillars[0].width, 64.0);
        assert_eq!(s.pillars[0].x, lanes::lane_x(0, 768, 64));
    }

    // --- 主画面

    fn outs<'a>(
        list: &'a [(&'a str, &'a str)],
    ) -> impl Iterator<Item = (&'a str, &'a str)> + Clone {
        list.iter().copied()
    }

    #[test]
    fn primary_uses_rules_then_focused_then_first() {
        let list = [
            ("eDP-1", "Lenovo (eDP-1)"),
            ("DP-10", "LG ULTRAWIDE (DP-10)"),
        ];
        let mut by_rule = PrimaryTracker::new(vec!["LG ULTRAWIDE".into()], Some("eDP-1".into()));
        assert_eq!(by_rule.update(outs(&list)), Some(1));
        let mut by_focus = PrimaryTracker::new(vec![], Some("DP-10".into()));
        assert_eq!(by_focus.update(outs(&list)), Some(1));
        let mut first = PrimaryTracker::new(vec![], None);
        assert_eq!(first.update(outs(&list)), Some(0));
        assert_eq!(first.primary(), Some(0));
    }

    #[test]
    fn primary_is_chosen_again_when_outputs_change() {
        let mut t = PrimaryTracker::new(vec!["DP-10".into()], None);
        let both = [("eDP-1", ""), ("DP-10", "")];
        assert_eq!(t.update(outs(&both)), Some(1));
        // 主画面が抜かれたら残りから選び直す
        let laptop = [("eDP-1", "")];
        assert_eq!(t.update(outs(&laptop)), Some(0));
        // 挿し直したら戻る（並びが変わっても名前で選ぶ）
        let again = [("DP-10", ""), ("eDP-1", "")];
        assert_eq!(t.update(outs(&again)), Some(0));
        assert_eq!(t.update(outs(&[])), None);
    }

    #[test]
    fn primary_change_is_reported_by_name_even_when_the_index_is_reused() {
        let mut t = PrimaryTracker::new(vec!["DP-10".into()], None);
        let both = [("DP-10", ""), ("eDP-1", "")];
        assert_eq!(t.update(outs(&both)), Some(0));
        assert!(t.changed());
        assert_eq!(t.previous_name(), None);
        // 一覧が同じなら替わっていない
        assert_eq!(t.update(outs(&both)), Some(0));
        assert!(!t.changed());
        // 主画面（添字 0）が抜かれ、残りの出力が同じ添字 0 になっても、替わったと分かる
        let laptop = [("eDP-1", "")];
        assert_eq!(t.update(outs(&laptop)), Some(0));
        assert!(t.changed());
        assert_eq!(t.previous_name(), Some("DP-10"));
        // 出力が増えても主画面が同じ出力なら替わっていない
        let more = [("eDP-1", ""), ("HDMI-A-1", "")];
        assert_eq!(t.update(outs(&more)), Some(0));
        assert!(!t.changed());
        assert_eq!(t.previous_name(), None);
    }

    #[test]
    fn primary_starts_unknown() {
        assert_eq!(PrimaryTracker::new(vec![], None).primary(), None);
    }
}
