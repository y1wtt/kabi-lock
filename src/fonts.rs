//! 時計・日付のフォントを、fontconfig の一覧から選ぶ。
//!
//! fontconfig を呼ぶ・ファイルを読むのは `render::overlay`。ここは一覧の読み取りと、
//! どのファイルを使うか・いつ埋め込みの Inter に戻すかの判断だけを持つ純粋なモジュール。
#![forbid(unsafe_code)]

use std::fmt;

/// 時計の太さ（CSS / OpenType の weight）。
pub const CLOCK_WEIGHT: f32 = 500.0;
/// 日付の太さ。
pub const DATE_WEIGHT: f32 = 400.0;

/// `fc-list --format` に渡す書式。`parse_fc_list` はこの形を読む。
pub const FC_LIST_FORMAT: &str =
    "%{file}\t%{index}\t%{family}\t%{weight}\t%{slant}\t%{fontformat}\n";

/// fontconfig の weight。可変フォントは範囲（例: `[0 210]`）になる。値は fontconfig の単位。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FcWeight {
    Fixed(f32),
    Range(f32, f32),
}

/// `fc-list` が返したフェイス 1 つ。
#[derive(Clone, Debug, PartialEq)]
pub struct FcFace {
    pub file: String,
    /// fontconfig の index。下位 16 bit がファイルの中のフェイスの番号、
    /// 上位 16 bit が可変フォントの名前付きインスタンスの番号（0 ならインスタンスではない）。
    pub index: u32,
    /// ファミリー名（別名を含む。例: `Noto Sans`, `Noto Sans Medium`）。
    pub families: Vec<String>,
    pub weight: FcWeight,
    /// 0 なら直立（イタリック・斜体でない）。
    pub slant: i32,
    /// `TrueType`, `CFF` など。
    pub format: String,
}

/// 使うと決めたフォントファイル。
#[derive(Clone, Debug, PartialEq)]
pub struct FacePick {
    pub file: String,
    /// ファイルの中のフェイスの番号（egui の `FontData::index`）。
    pub face_index: u32,
    /// このフェイスで出る太さ（CSS の weight）。
    pub weight: f32,
    /// 可変フォントの `wght` 軸に設定する値。静的なフォントなら `None`。
    pub variable_weight: Option<f32>,
}

/// 埋め込みの Inter に戻した理由。ログに出す。
#[derive(Clone, Debug, PartialEq)]
pub enum Fallback {
    EmptyName,
    FontconfigUnavailable(String),
    NotFound(String),
    NoUsableFace(String),
    Unreadable { file: String, error: String },
    Unusable { file: String, error: String },
}

impl fmt::Display for Fallback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fallback::EmptyName => write!(f, "appearance.font is empty"),
            Fallback::FontconfigUnavailable(e) => write!(f, "fontconfig is unavailable: {e}"),
            Fallback::NotFound(family) => write!(f, "no installed font family named {family:?}"),
            Fallback::NoUsableFace(family) => write!(
                f,
                "font family {family:?} has no upright TrueType or OpenType face"
            ),
            Fallback::Unreadable { file, error } => write!(f, "cannot read {file}: {error}"),
            Fallback::Unusable { file, error } => write!(f, "{file} is not a usable font: {error}"),
        }
    }
}

/// fontconfig のパターンで特別な意味を持つ文字（`\` `-` `:` `,`）をエスケープする。
pub fn fc_pattern(family: &str) -> String {
    let mut out = String::with_capacity(family.len());
    for c in family.chars() {
        if matches!(c, '\\' | '-' | ':' | ',') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `fc-list --format=FC_LIST_FORMAT` の出力を読む。読めない行は飛ばす。
pub fn parse_fc_list(output: &str) -> Vec<FcFace> {
    output.lines().filter_map(parse_fc_line).collect()
}

fn parse_fc_line(line: &str) -> Option<FcFace> {
    let mut fields = line.split('\t');
    let file = fields.next()?.trim();
    let index = fields.next()?.trim().parse().ok()?;
    let families = fields
        .next()?
        .split(',')
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let weight = parse_weight(fields.next()?.trim())?;
    let slant = fields.next()?.trim().parse().ok()?;
    let format = fields.next()?.trim().to_owned();
    if file.is_empty() || families.is_empty() {
        return None;
    }
    Some(FcFace {
        file: file.to_owned(),
        index,
        families,
        weight,
        slant,
        format,
    })
}

fn parse_weight(text: &str) -> Option<FcWeight> {
    if let Some(range) = text.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
        let mut ends = range.split_whitespace().map(|v| v.parse::<f32>().ok());
        let (lo, hi) = (ends.next()??, ends.next()??);
        return Some(FcWeight::Range(lo.min(hi), lo.max(hi)));
    }
    text.parse().ok().map(FcWeight::Fixed)
}

/// fontconfig の weight を CSS / OpenType の weight にする（fontconfig の `FcWeightToOpenType` と同じ表）。
pub fn css_weight(fc: f32) -> f32 {
    const MAP: [(f32, f32); 12] = [
        (0.0, 100.0),
        (40.0, 200.0),
        (50.0, 300.0),
        (55.0, 350.0),
        (75.0, 380.0),
        (80.0, 400.0),
        (100.0, 500.0),
        (180.0, 600.0),
        (200.0, 700.0),
        (205.0, 800.0),
        (210.0, 900.0),
        (215.0, 1000.0),
    ];
    if fc <= MAP[0].0 {
        return MAP[0].1;
    }
    for w in MAP.windows(2) {
        let ((f0, c0), (f1, c1)) = (w[0], w[1]);
        if fc <= f1 {
            return c0 + (c1 - c0) * (fc - f0) / (f1 - f0);
        }
    }
    MAP[MAP.len() - 1].1
}

/// 欲しい太さ `desired` に対して、太さ `available` がどれだけ好ましいか（小さいほど良い）。
///
/// CSS Fonts 4 の「font-weight の照合」と同じ順番にする（ブラウザで設計を見たときと同じ選び方）:
/// 同じ太さ → 400〜500 を欲しいときは 500 までの太い方 → 細い方 → 500 より太い方。
/// 400 未満なら細い方 → 太い方、500 より上なら太い方 → 細い方。それぞれ近い順。
pub fn weight_preference(desired: f32, available: f32) -> (u8, u32) {
    let distance = ((available - desired).abs() * 10.0).round() as u32;
    let group = if available == desired {
        0
    } else if (400.0..=500.0).contains(&desired) {
        if available > desired && available <= 500.0 {
            1
        } else if available < desired {
            2
        } else {
            3
        }
    } else if desired < 400.0 {
        if available < desired { 1 } else { 2 }
    } else if available > desired {
        1
    } else {
        2
    };
    (group, distance)
}

/// ファミリー名の比較（fontconfig と同じく大文字・小文字と空白を無視する）。
fn same_family(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        s.chars()
            .filter(|c| !c.is_whitespace())
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    norm(a) == norm(b)
}

/// フェイスの種類の好み（静的 → 名前付きインスタンス → 可変の範囲）。
fn kind_rank(face: &FcFace) -> u8 {
    match face.weight {
        FcWeight::Fixed(_) if face.index >> 16 == 0 => 0,
        FcWeight::Fixed(_) => 1,
        FcWeight::Range(..) => 2,
    }
}

/// フェイス 1 つで出せる、`desired` に一番近い太さ（CSS の weight）。
fn provided_weight(face: &FcFace, desired: f32) -> f32 {
    match face.weight {
        FcWeight::Fixed(w) => css_weight(w),
        FcWeight::Range(lo, hi) => desired.clamp(css_weight(lo), css_weight(hi)),
    }
}

fn pick_one(faces: &[&FcFace], desired: f32) -> Option<FacePick> {
    let best = faces.iter().min_by(|a, b| {
        let key = |f: &FcFace| {
            (
                weight_preference(desired, provided_weight(f, desired)),
                kind_rank(f),
            )
        };
        key(a)
            .cmp(&key(b))
            .then_with(|| a.file.cmp(&b.file))
            .then_with(|| a.index.cmp(&b.index))
    })?;
    let weight = provided_weight(best, desired);
    let variable_weight = (kind_rank(best) > 0).then_some(weight);
    Some(FacePick {
        file: best.file.clone(),
        face_index: best.index & 0xFFFF,
        weight,
        variable_weight,
    })
}

/// `family` の中から時計用（500）と日付用（400）のフェイスを選ぶ。
///
/// 直立で、egui が読める形式（TrueType・CFF）のフェイスだけを使う。その太さがなければ
/// 同じファミリーの一番近い太さ（`weight_preference` の順）を使う。ファミリーがない・使える
/// フェイスがないときは `Err` で、呼び出し側は埋め込みの Inter を使う。
pub fn pick_faces(family: &str, faces: &[FcFace]) -> Result<(FacePick, FacePick), Fallback> {
    let family = family.trim();
    if family.is_empty() {
        return Err(Fallback::EmptyName);
    }
    let in_family: Vec<&FcFace> = faces
        .iter()
        .filter(|f| f.families.iter().any(|n| same_family(n, family)))
        .collect();
    if in_family.is_empty() {
        return Err(Fallback::NotFound(family.to_owned()));
    }
    let usable: Vec<&FcFace> = in_family
        .into_iter()
        .filter(|f| f.slant == 0 && matches!(f.format.as_str(), "TrueType" | "CFF"))
        .collect();
    match (
        pick_one(&usable, CLOCK_WEIGHT),
        pick_one(&usable, DATE_WEIGHT),
    ) {
        (Some(clock), Some(date)) => Ok((clock, date)),
        _ => Err(Fallback::NoUsableFace(family.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTO: &str = "\
/usr/share/fonts/noto/NotoSans-Black.ttf\t0\tNoto Sans,Noto Sans Black\t210\t0\tTrueType
/usr/share/fonts/noto/NotoSans-MediumItalic.ttf\t0\tNoto Sans,Noto Sans Medium\t100\t100\tTrueType
/usr/share/fonts/noto/NotoSans-Medium.ttf\t0\tNoto Sans,Noto Sans Medium\t100\t0\tTrueType
/usr/share/fonts/noto/NotoSans-Italic.ttf\t0\tNoto Sans\t80\t100\tTrueType
/usr/share/fonts/noto/NotoSans-Regular.ttf\t0\tNoto Sans\t80\t0\tTrueType
/usr/share/fonts/noto/NotoSans-Bold.ttf\t0\tNoto Sans\t200\t0\tTrueType
";

    const ADWAITA: &str = "\
/usr/share/fonts/Adwaita/AdwaitaSans-Italic.ttf\t0\tAdwaita Sans\t[0 210]\t100\tTrueType
/usr/share/fonts/Adwaita/AdwaitaSans-Regular.ttf\t0\tAdwaita Sans\t[0 210]\t0\tTrueType
";

    fn face(file: &str, index: u32, weight: FcWeight) -> FcFace {
        FcFace {
            file: file.into(),
            index,
            families: vec!["Test Sans".into()],
            weight,
            slant: 0,
            format: "TrueType".into(),
        }
    }

    fn fixed(list: &[(&str, f32)]) -> Vec<FcFace> {
        list.iter()
            .map(|(file, w)| face(file, 0, FcWeight::Fixed(*w)))
            .collect()
    }

    #[test]
    fn pattern_escapes_special_characters() {
        assert_eq!(fc_pattern("Noto Sans"), "Noto Sans");
        assert_eq!(fc_pattern(r"A-B:C,D\E"), r"A\-B\:C\,D\\E");
    }

    #[test]
    fn fc_list_output_is_parsed() {
        let faces = parse_fc_list(ADWAITA);
        assert_eq!(faces.len(), 2);
        assert_eq!(
            faces[1],
            FcFace {
                file: "/usr/share/fonts/Adwaita/AdwaitaSans-Regular.ttf".into(),
                index: 0,
                families: vec!["Adwaita Sans".into()],
                weight: FcWeight::Range(0.0, 210.0),
                slant: 0,
                format: "TrueType".into(),
            }
        );
        let noto = parse_fc_list(NOTO);
        assert_eq!(noto[0].families, vec!["Noto Sans", "Noto Sans Black"]);
        assert_eq!(noto[0].weight, FcWeight::Fixed(210.0));
    }

    #[test]
    fn broken_lines_are_skipped() {
        let text = "only-a-file\n/a.ttf\tx\tA\t80\t0\tTrueType\n\n/b.ttf\t0\tB\t80\t0\tTrueType\n";
        let faces = parse_fc_list(text);
        assert_eq!(faces.len(), 1);
        assert_eq!(faces[0].file, "/b.ttf");
    }

    #[test]
    fn fontconfig_weights_map_to_css_weights() {
        assert_eq!(css_weight(80.0), 400.0);
        assert_eq!(css_weight(100.0), 500.0);
        assert_eq!(css_weight(200.0), 700.0);
        assert_eq!(css_weight(0.0), 100.0);
        assert_eq!(css_weight(210.0), 900.0);
        assert_eq!(css_weight(90.0), 450.0);
        assert_eq!(css_weight(-5.0), 100.0);
        assert_eq!(css_weight(300.0), 1000.0);
    }

    #[test]
    fn weight_preference_follows_css_matching() {
        let order = |desired: f32, list: &[f32]| {
            let mut v = list.to_vec();
            v.sort_by_key(|w| weight_preference(desired, *w));
            v
        };
        let all = [100.0, 300.0, 400.0, 500.0, 600.0, 700.0];
        assert_eq!(
            order(500.0, &all),
            [500.0, 400.0, 300.0, 100.0, 600.0, 700.0]
        );
        assert_eq!(
            order(400.0, &all),
            [400.0, 500.0, 300.0, 100.0, 600.0, 700.0]
        );
        assert_eq!(
            order(300.0, &all),
            [300.0, 100.0, 400.0, 500.0, 600.0, 700.0]
        );
        assert_eq!(
            order(700.0, &all),
            [700.0, 600.0, 500.0, 400.0, 300.0, 100.0]
        );
    }

    #[test]
    fn static_family_gets_exact_weights() {
        let (clock, date) = pick_faces("Noto Sans", &parse_fc_list(NOTO)).unwrap();
        assert_eq!(
            clock,
            FacePick {
                file: "/usr/share/fonts/noto/NotoSans-Medium.ttf".into(),
                face_index: 0,
                weight: 500.0,
                variable_weight: None,
            }
        );
        assert_eq!(date.file, "/usr/share/fonts/noto/NotoSans-Regular.ttf");
        assert_eq!(date.weight, 400.0);
    }

    #[test]
    fn family_names_ignore_case_and_spaces() {
        let (clock, _) = pick_faces("  notosans ", &parse_fc_list(NOTO)).unwrap();
        assert_eq!(clock.weight, 500.0);
    }

    #[test]
    fn missing_weights_use_the_nearest_in_css_order() {
        // 400 と 700 しかない: 時計（500）は 400、日付は 400
        let faces = fixed(&[("/r.ttf", 80.0), ("/b.ttf", 200.0)]);
        let (clock, date) = pick_faces("Test Sans", &faces).unwrap();
        assert_eq!((clock.file.as_str(), clock.weight), ("/r.ttf", 400.0));
        assert_eq!(date.file, "/r.ttf");
        // 300 と 700: 500 は細い方を先に見る
        let faces = fixed(&[("/l.ttf", 50.0), ("/b.ttf", 200.0)]);
        assert_eq!(pick_faces("Test Sans", &faces).unwrap().0.file, "/l.ttf");
        // 500 と 700: 日付（400）は 500 までの太い方を先に見る
        let faces = fixed(&[("/m.ttf", 100.0), ("/b.ttf", 200.0)]);
        let (clock, date) = pick_faces("Test Sans", &faces).unwrap();
        assert_eq!(
            (clock.file.as_str(), date.file.as_str()),
            ("/m.ttf", "/m.ttf")
        );
        // 1 つの太さしかなければ両方それ
        let faces = fixed(&[("/black.ttf", 210.0)]);
        let (clock, date) = pick_faces("Test Sans", &faces).unwrap();
        assert_eq!((clock.weight, date.weight), (900.0, 900.0));
    }

    #[test]
    fn variable_font_is_set_to_the_exact_weight() {
        let (clock, date) = pick_faces("Adwaita Sans", &parse_fc_list(ADWAITA)).unwrap();
        let file = "/usr/share/fonts/Adwaita/AdwaitaSans-Regular.ttf";
        assert_eq!(
            clock,
            FacePick {
                file: file.into(),
                face_index: 0,
                weight: 500.0,
                variable_weight: Some(500.0),
            }
        );
        assert_eq!(date.file, file);
        assert_eq!(date.variable_weight, Some(400.0));
    }

    #[test]
    fn static_faces_win_over_variable_ones_of_the_same_weight() {
        let faces = vec![
            face("/var.ttf", 0, FcWeight::Range(0.0, 210.0)),
            face("/var.ttf", 4 << 16, FcWeight::Fixed(100.0)),
            face("/medium.ttf", 0, FcWeight::Fixed(100.0)),
        ];
        let (clock, date) = pick_faces("Test Sans", &faces).unwrap();
        assert_eq!(clock.file, "/medium.ttf");
        assert_eq!(clock.variable_weight, None);
        // 400 の静的なフェイスはないので、可変フォントで正確な太さにする
        assert_eq!(date.file, "/var.ttf");
        assert_eq!(date.variable_weight, Some(400.0));
    }

    #[test]
    fn named_instance_uses_its_face_and_weight() {
        let faces = vec![face("/var.ttf", (5 << 16) | 2, FcWeight::Fixed(100.0))];
        let (clock, _) = pick_faces("Test Sans", &faces).unwrap();
        assert_eq!(clock.face_index, 2);
        assert_eq!(clock.variable_weight, Some(500.0));
    }

    #[test]
    fn falls_back_when_the_family_is_unusable() {
        assert_eq!(
            pick_faces("", &parse_fc_list(NOTO)),
            Err(Fallback::EmptyName)
        );
        assert_eq!(
            pick_faces("Nope", &parse_fc_list(NOTO)),
            Err(Fallback::NotFound("Nope".into()))
        );
        assert_eq!(
            pick_faces("Nope", &[]),
            Err(Fallback::NotFound("Nope".into()))
        );
        // イタリックしかない
        let mut italic = fixed(&[("/i.ttf", 80.0)]);
        italic[0].slant = 100;
        assert_eq!(
            pick_faces("Test Sans", &italic),
            Err(Fallback::NoUsableFace("Test Sans".into()))
        );
        // egui が読めない形式しかない
        let mut bitmap = fixed(&[("/b.pcf.gz", 80.0)]);
        bitmap[0].format = "PCF".into();
        assert_eq!(
            pick_faces("Test Sans", &bitmap),
            Err(Fallback::NoUsableFace("Test Sans".into()))
        );
    }

    #[test]
    fn fallback_reasons_are_readable() {
        assert_eq!(
            Fallback::NotFound("Nope".into()).to_string(),
            "no installed font family named \"Nope\""
        );
        assert_eq!(
            Fallback::Unusable {
                file: "/x.ttf".into(),
                error: "bad".into()
            }
            .to_string(),
            "/x.ttf is not a usable font: bad"
        );
    }
}
