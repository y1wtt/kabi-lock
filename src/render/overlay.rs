//! egui（egui_glow）による重ね描き: 時計・日付・光の柱・錠。主画面だけに描く。
//!
//! 何をどこに描くかは `scene`（純粋）で決め、ここでは egui の図形にして GL で描くだけにする。

use std::process::Command;
use std::sync::Arc;

use egui::epaint::text::VariationCoords;
use egui::text::LayoutJob;
use egui::{
    Color32, Context, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, FontTweak,
    LayerId, Mesh, Painter, RawInput, Rect, Shape, Stroke, TextFormat, pos2, vec2,
};
use glow::HasContext as _;

use crate::anim;
use crate::config;
use crate::fonts::{self, FacePick, Fallback};
use crate::render::egl::Gpu;
use crate::scene::{self, Layout, Scene};

/// 埋め込みの Inter（weight 500、時計用）。OFL。ライセンスは `assets/fonts/LICENSE.txt`。
/// `appearance.font` が使えないときの予備で、使えるときも字形が足りない分を補う。
static INTER_MEDIUM: &[u8] = include_bytes!("../../assets/fonts/Inter-Medium.ttf");
/// 埋め込みの Inter（weight 400、日付用）。
static INTER_REGULAR: &[u8] = include_bytes!("../../assets/fonts/Inter-Regular.ttf");

/// egui のファミリー名（時計・日付）。
const CLOCK_FONT: &str = "clock";
const DATE_FONT: &str = "date";
/// egui のフォント名。
const CHOSEN_CLOCK: &str = "chosen-clock";
const CHOSEN_DATE: &str = "chosen-date";
const INTER_MEDIUM_NAME: &str = "inter-medium";
const INTER_REGULAR_NAME: &str = "inter-regular";
const FALLBACK_FONT: &str = "fallback";

/// 時計と日付の色（白の不透明度。重ね物全体の不透明度を掛ける前の値）。
const CLOCK_ALPHA: f32 = 0.94;
const DATE_ALPHA: f32 = 0.74;
/// U 字のベジェ 1 本あたりの分割数。
const SHACKLE_SEGMENTS: usize = 16;

/// 時計・日付・光の柱・錠を描く。GL の準備（シェーダー・フォント・アトラス）はロック前に済ませる。
pub struct Overlay {
    ctx: Context,
    painter: egui_glow::Painter,
}

impl Overlay {
    /// 時計・日付のフォントを選んで登録し、`date_needs_fallback` なら fontconfig の
    /// 日本語などのフォントを補いに足す。
    ///
    /// フォントは fontconfig で `font_family`（`appearance.font`）を探し、太さ 500（時計）と
    /// 400（日付）のファイルを読む。使えなければ埋め込みの Inter にする（理由はログに出す）。
    /// 呼ぶときは `gpu` のコンテキストが current であること（`Gpu::new` の直後はサーフェスなしで current）。
    pub fn new(gpu: &Gpu, font_family: &str, date_needs_fallback: bool) -> Result<Overlay, String> {
        let painter = egui_glow::Painter::new(gpu.gl().clone(), "", None, false)
            .map_err(|e| format!("egui_glow painter: {e}"))?;
        let ctx = Context::default();
        let chosen = match system_fonts(font_family) {
            Ok((clock, date)) => Some((clock, date)),
            Err(reason) => {
                if reason == Fallback::NotFound(config::DEFAULT_FONT.into()) {
                    // 初期値の Inter が入っていないのは普通のこと
                    log::info!("font \"Inter\" is not installed; using the embedded Inter");
                } else {
                    log::warn!(
                        "font {font_family:?} is not used ({reason}); using the embedded Inter"
                    );
                }
                None
            }
        };
        let fallback = if date_needs_fallback {
            fallback_font()
        } else {
            None
        };
        ctx.set_fonts(font_definitions(chosen, fallback));
        let mut overlay = Overlay { ctx, painter };
        overlay.warm_up();
        Ok(overlay)
    }

    /// 1920x1080 基準の配置を、出力の高さに合わせて拡大して描く。
    ///
    /// `width`・`height` は描画先のピクセル数。呼ぶ前に描画先のサーフェスを current にし、
    /// 背景を描いておく（重ね物は上に重ねるだけで、画面を消さない）。
    pub fn draw(
        &mut self,
        gpu: &Gpu,
        width: u32,
        height: u32,
        scene: &Scene,
        input: &config::Input,
    ) {
        if scene.opacity <= 0.0 || width == 0 || height == 0 {
            return;
        }
        let layout = scene::layout(width, height, input.band_width);
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                vec2(width as f32, height as f32),
            )),
            ..Default::default()
        };
        let out = self.ctx.run_pass(raw, |ctx| {
            paint_scene(&ctx.layer_painter(LayerId::background()), &layout, scene);
        });
        let primitives = self.ctx.tessellate(out.shapes, out.pixels_per_point);
        let mut textures = out.textures_delta;
        self.painter.paint_and_update_textures(
            [width, height],
            out.pixels_per_point,
            &primitives,
            &mut textures,
        );
        // egui が変えた状態のうち、背景の描画に響くものを既定に戻す
        // （SCISSOR_TEST は egui_glow が自分で戻す）
        unsafe {
            gpu.gl().disable(glow::BLEND);
            gpu.gl().use_program(None);
        }
    }

    /// フォントを読み込み、時計の数字と日付の字形をアトラスに載せて GPU へ送っておく。
    /// 1080p の大きさで作るので、それ以外の高さの主画面では最初のフレームで足りない字形を作る。
    fn warm_up(&mut self) {
        let layout = scene::layout(1920, 1080, 768);
        let out = self.ctx.run_pass(RawInput::default(), |ctx| {
            let painter = ctx.layer_painter(LayerId::background());
            let _ =
                painter.layout_no_wrap("0123456789:".into(), clock_font(&layout), Color32::WHITE);
            let _ =
                painter.layout_no_wrap("0123456789/".into(), date_font(&layout), Color32::WHITE);
        });
        let mut textures = out.textures_delta;
        // 何も描かずにテクスチャだけ送る（サーフェスなしの current でも描画命令は出さない）
        self.painter
            .paint_and_update_textures([1, 1], out.pixels_per_point, &[], &mut textures);
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        // GL の後片付け。終了時にコンテキストがなくても GL 呼び出しが無視されるだけで害はない
        self.painter.destroy();
    }
}

fn clock_font(l: &Layout) -> FontId {
    FontId::new(l.clock_size, FontFamily::Name(CLOCK_FONT.into()))
}

fn date_font(l: &Layout) -> FontId {
    FontId::new(l.date_size, FontFamily::Name(DATE_FONT.into()))
}

/// 白に不透明度を付けた色（egui の色は乗算済みアルファ）。
fn white(alpha: f32) -> Color32 {
    Color32::from_white_alpha((alpha.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// egui に登録するフォント。
///
/// 時計は 選んだ太さ 500 → 埋め込みの Inter 500 → `fallback`、日付は 選んだ太さ 400 → Inter 400 →
/// `fallback` の順に字形を探す（選んだフォントにない文字は Inter で補う）。
fn font_definitions(
    chosen: Option<(FontData, FontData)>,
    fallback: Option<FontData>,
) -> FontDefinitions {
    let mut fonts = FontDefinitions::empty();
    let mut insert = |name: &str, data: FontData| {
        fonts.font_data.insert(name.to_owned(), Arc::new(data));
    };
    insert(INTER_MEDIUM_NAME, FontData::from_static(INTER_MEDIUM));
    insert(INTER_REGULAR_NAME, FontData::from_static(INTER_REGULAR));
    let mut clock = vec![INTER_MEDIUM_NAME.to_owned()];
    let mut date = vec![INTER_REGULAR_NAME.to_owned()];
    if let Some((clock_data, date_data)) = chosen {
        insert(CHOSEN_CLOCK, clock_data);
        insert(CHOSEN_DATE, date_data);
        clock.insert(0, CHOSEN_CLOCK.to_owned());
        date.insert(0, CHOSEN_DATE.to_owned());
    }
    if let Some(data) = fallback {
        insert(FALLBACK_FONT, data);
        clock.push(FALLBACK_FONT.to_owned());
        date.push(FALLBACK_FONT.to_owned());
    }
    fonts
        .families
        .insert(FontFamily::Name(CLOCK_FONT.into()), clock);
    fonts
        .families
        .insert(FontFamily::Name(DATE_FONT.into()), date.clone());
    // egui が内部で使う既定のファミリーも空にしない
    fonts
        .families
        .insert(FontFamily::Proportional, date.clone());
    fonts.families.insert(FontFamily::Monospace, date);
    fonts
}

/// `appearance.font` のファミリーの、時計用（500）と日付用（400）のフォントを読む。
///
/// fontconfig の `fc-list` で一覧を取り（`fc-match` は似たフォントで代用してしまうので使わない）、
/// どれを使うかは `fonts::pick_faces` が決める。読めない・egui が扱えないファイルなら `Err`。
fn system_fonts(family: &str) -> Result<(FontData, FontData), Fallback> {
    if family.trim().is_empty() {
        return Err(Fallback::EmptyName);
    }
    let output = Command::new("fc-list")
        .arg(format!("--format={}", fonts::FC_LIST_FORMAT))
        .arg(fonts::fc_pattern(family.trim()))
        .output()
        .map_err(|e| Fallback::FontconfigUnavailable(e.to_string()))?;
    if !output.status.success() {
        return Err(Fallback::FontconfigUnavailable(format!(
            "fc-list exited with {}",
            output.status
        )));
    }
    let faces = fonts::parse_fc_list(&String::from_utf8_lossy(&output.stdout));
    let (clock, date) = fonts::pick_faces(family, &faces)?;
    // 可変フォントなどで同じファイルを 2 回読まない
    let clock_bytes = read_font_file(&clock.file)?;
    let date_bytes = if date.file == clock.file {
        clock_bytes.clone()
    } else {
        read_font_file(&date.file)?
    };
    let clock_data = face_data(&clock, clock_bytes)?;
    let date_data = face_data(&date, date_bytes)?;
    log::info!(
        "clock font: {family} weight {} ({} face {}); date font: weight {} ({} face {})",
        clock.weight,
        clock.file,
        clock.face_index,
        date.weight,
        date.file,
        date.face_index
    );
    Ok((clock_data, date_data))
}

fn read_font_file(file: &str) -> Result<Arc<Vec<u8>>, Fallback> {
    std::fs::read(file)
        .map(Arc::new)
        .map_err(|e| Fallback::Unreadable {
            file: file.to_owned(),
            error: e.to_string(),
        })
}

/// 選んだフェイスを egui のフォントにする。egui は読めないフォントで panic するので、
/// 同じ方法（skrifa）で先に確かめる。可変フォントは `wght` 軸を選んだ太さにする。
fn face_data(pick: &FacePick, bytes: Arc<Vec<u8>>) -> Result<FontData, Fallback> {
    if let Err(e) = skrifa::FontRef::from_index(&bytes, pick.face_index) {
        return Err(Fallback::Unusable {
            file: pick.file.clone(),
            error: e.to_string(),
        });
    }
    let data = FontData::from_blob(bytes, pick.face_index);
    Ok(match pick.variable_weight {
        Some(weight) => data.tweak(FontTweak {
            coords: VariationCoords::new([(b"wght", weight)]),
            ..Default::default()
        }),
        None => data,
    })
}

/// 日時のロケールの言語で fontconfig にフォントを尋ね、読み込む。
///
/// `fc-match` を子プロセスで呼ぶ（C のライブラリを直接つながないので unsafe もビルド時の依存も要らず、
/// デスクトップの他のアプリと同じ規則で選ばれる）。見つからない・読めない・壊れているときは `None`
/// （その文字は表示されないが、ロックは続ける）。
fn fallback_font() -> Option<FontData> {
    let lc_all = std::env::var("LC_ALL").ok();
    let lc_time = std::env::var("LC_TIME").ok();
    let lang = std::env::var("LANG").ok();
    let locale = scene::time_locale_name(lc_all.as_deref(), lc_time.as_deref(), lang.as_deref());
    let pattern = match scene::language(locale) {
        Some(lang) => format!("sans-serif:lang={lang}"),
        None => "sans-serif".to_owned(),
    };
    let output = match Command::new("fc-match")
        .arg("--format=%{file}\n%{index}")
        .arg(&pattern)
        .output()
    {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            log::warn!("fc-match {pattern} failed: {}", output.status);
            return None;
        }
        Err(e) => {
            log::warn!("cannot run fc-match: {e}");
            return None;
        }
    };
    let (file, index) = scene::parse_fc_match(&String::from_utf8_lossy(&output.stdout))?;
    let bytes = match std::fs::read(&file) {
        Ok(bytes) => bytes,
        Err(e) => {
            log::warn!("cannot read fallback font {file}: {e}");
            return None;
        }
    };
    // egui は読めないフォントで panic するので、同じ方法（skrifa）で先に確かめる
    if let Err(e) = skrifa::FontRef::from_index(&bytes, index) {
        log::warn!("fallback font {file} (index {index}) is not usable: {e}");
        return None;
    }
    log::debug!("fallback font for {pattern}: {file} (index {index})");
    Some(FontData::from_blob(Arc::new(bytes), index))
}

/// 1 フレーム分の図形を積む。描く順は 光の柱 → 時計 → 日付 → 錠（U 字 → 本体）。
fn paint_scene(painter: &Painter, l: &Layout, scene: &Scene) {
    for pillar in &scene.pillars {
        let [left, top, right, bottom] = scene::pillar_rect(l, pillar);
        let rect = Rect::from_min_max(pos2(left.round(), top), pos2(right.round(), bottom));
        painter.add(Shape::mesh(pillar_mesh(
            rect,
            scene.opacity * pillar.frame.opacity,
        )));
    }
    paint_clock(
        painter,
        l,
        &scene.time_text,
        white(CLOCK_ALPHA * scene.opacity),
    );
    paint_date(
        painter,
        l,
        &scene.date_text,
        white(DATE_ALPHA * scene.opacity),
    );
    if let Some(lock) = &scene.lock {
        paint_lock(painter, l, lock, white(scene.opacity));
    }
}

/// 柱を 1 本、頂点ごとに色を持つ格子のメッシュで作る。
///
/// 不透明度は「横のグラデーション × 縦のマスク」の積で、三角形 1 枚の中では直線補間しかできない。
/// 横は区切り（`PILLAR_COLUMNS`）に頂点を置くので正確、縦は `PILLAR_ROWS` 段に分けて
/// 積の誤差を 1% 未満にする。
fn pillar_mesh(rect: Rect, opacity: f32) -> Mesh {
    let cols = scene::PILLAR_COLUMNS;
    let rows = scene::PILLAR_ROWS;
    let mut mesh = Mesh::default();
    for r in 0..=rows {
        let v = r as f32 / rows as f32;
        let y = rect.top() + v * rect.height();
        for u in cols {
            let x = rect.left() + u * rect.width();
            mesh.colored_vertex(pos2(x, y), white(opacity * scene::pillar_alpha(u, v)));
        }
    }
    let w = cols.len() as u32;
    for r in 0..rows as u32 {
        for c in 0..w - 1 {
            let i = r * w + c;
            mesh.add_triangle(i, i + 1, i + w);
            mesh.add_triangle(i + 1, i + w + 1, i + w);
        }
    }
    mesh
}

/// 時計。数字は等幅（一番広い数字の幅の枠の中央に置く）、字間は -2px（1080p 基準）。
///
/// egui は OpenType の機能（`tnum`）を指定できないので、1 文字ずつ並べて等幅数字にする。
fn paint_clock(painter: &Painter, l: &Layout, text: &str, color: Color32) {
    let font = clock_font(l);
    let glyph = |c: char| painter.layout_no_wrap(c.to_string(), font.clone(), Color32::WHITE);
    let digit_w = ('0'..='9')
        .map(|c| glyph(c).size().x)
        .fold(0.0_f32, f32::max);
    let glyphs: Vec<_> = text
        .chars()
        .map(|c| {
            let g = glyph(c);
            let cell = if c.is_ascii_digit() {
                digit_w
            } else {
                g.size().x
            };
            (g, cell)
        })
        .collect();
    let gaps = glyphs.len().saturating_sub(1) as f32;
    let total: f32 =
        glyphs.iter().map(|(_, cell)| cell).sum::<f32>() + l.clock_letter_spacing * gaps;
    let mut x = l.center_x - total / 2.0;
    for (g, cell) in glyphs {
        let pos = pos2(
            x + (cell - g.size().x) / 2.0,
            l.clock_center_y - g.size().y / 2.0,
        );
        painter.add(Shape::galley_with_override_text_color(pos, g, color));
        x += cell + l.clock_letter_spacing;
    }
}

/// 日付。字間 1px（1080p 基準）、行の上端を `date_top` に合わせて中央寄せ。
fn paint_date(painter: &Painter, l: &Layout, text: &str, color: Color32) {
    let job = LayoutJob::single_section(
        text.to_owned(),
        TextFormat {
            font_id: date_font(l),
            extra_letter_spacing: l.date_letter_spacing,
            color: Color32::WHITE,
            ..Default::default()
        },
    );
    let galley = painter.layout_job(job);
    let pos = pos2(l.center_x - galley.size().x / 2.0, l.date_top);
    painter.add(Shape::galley_with_override_text_color(pos, galley, color));
}

/// 錠。U 字を太さ一定の線で描き、その上に本体（角丸の塗りつぶし）を重ねて足の下端を隠す。
fn paint_lock(painter: &Painter, l: &Layout, lock: &scene::LockDraw, color: Color32) {
    let unit = l.icon_unit;
    let origin = [l.icon_origin[0] + lock.shake_x * l.scale, l.icon_origin[1]];
    let to_screen = |p: [f32; 2]| pos2(origin[0] + p[0] * unit, origin[1] + p[1] * unit);

    let points: Vec<_> = scene::shackle_polyline(&lock.path, SHACKLE_SEGMENTS)
        .into_iter()
        .map(to_screen)
        .collect();
    let stroke = anim::SHACKLE_STROKE * unit;
    // egui の線は端が平らなので、見える方の端（最初の点）に丸を足す（設計の stroke-linecap: round）
    painter.add(Shape::circle_filled(points[0], stroke / 2.0, color));
    painter.add(Shape::line(points, Stroke::new(stroke, color)));

    let body = Rect::from_min_max(
        to_screen([anim::BODY_X, anim::BODY_Y]),
        to_screen([anim::BODY_X + anim::BODY_W, anim::BODY_Y + anim::BODY_H]),
    );
    let radius = (anim::BODY_RADIUS * unit).round().clamp(0.0, 255.0) as u8;
    painter.add(Shape::rect_filled(body, CornerRadius::same(radius), color));
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::epaint::text::{Fonts, TextOptions};
    use skrifa::MetadataProvider as _;
    use skrifa::attribute::Weight;
    use skrifa::string::StringId;

    #[test]
    fn embedded_fonts_are_inter_with_the_right_weights() {
        for (bytes, weight) in [
            (INTER_MEDIUM, Weight::MEDIUM),
            (INTER_REGULAR, Weight::NORMAL),
        ] {
            let font = skrifa::FontRef::new(bytes).unwrap();
            assert_eq!(font.attributes().weight, weight);
            let family: String = font
                .localized_strings(StringId::FAMILY_NAME)
                .english_or_first()
                .unwrap()
                .chars()
                .collect();
            assert!(
                family.starts_with("Inter") && !family.contains("Display"),
                "{family}"
            );
        }
    }

    #[test]
    fn egui_can_lay_out_the_clock_and_date() {
        let mut fonts = Fonts::new(TextOptions::default(), font_definitions(None, None));
        let mut view = fonts.with_pixels_per_point(1.0);
        let l = scene::layout(1920, 1080, 768);
        assert!(view.has_glyphs(&clock_font(&l), "0123456789:"));
        assert!(view.has_glyphs(&date_font(&l), "0123456789/ Tue Nov"));
        assert!(!view.has_glyphs(&date_font(&l), "火"));
    }

    #[test]
    fn embedded_inter_is_used_without_a_chosen_font() {
        let defs = font_definitions(None, None);
        assert_eq!(
            defs.families[&FontFamily::Name(CLOCK_FONT.into())],
            [INTER_MEDIUM_NAME]
        );
        assert_eq!(
            defs.families[&FontFamily::Name(DATE_FONT.into())],
            [INTER_REGULAR_NAME]
        );
    }

    #[test]
    fn chosen_font_comes_first_and_inter_fills_the_gaps() {
        let chosen = (
            FontData::from_static(INTER_MEDIUM),
            FontData::from_static(INTER_REGULAR),
        );
        let defs = font_definitions(Some(chosen), Some(FontData::from_static(INTER_REGULAR)));
        assert_eq!(
            defs.families[&FontFamily::Name(CLOCK_FONT.into())],
            [CHOSEN_CLOCK, INTER_MEDIUM_NAME, FALLBACK_FONT]
        );
        assert_eq!(
            defs.families[&FontFamily::Name(DATE_FONT.into())],
            [CHOSEN_DATE, INTER_REGULAR_NAME, FALLBACK_FONT]
        );
    }

    #[test]
    fn variable_faces_get_the_weight_axis() {
        let pick = FacePick {
            file: "embedded".into(),
            face_index: 0,
            weight: 500.0,
            variable_weight: Some(500.0),
        };
        let data = face_data(&pick, Arc::new(INTER_REGULAR.to_vec())).unwrap();
        assert_eq!(data.tweak.coords, VariationCoords::new([(b"wght", 500.0)]));
        let fixed = FacePick {
            variable_weight: None,
            ..pick
        };
        let data = face_data(&fixed, Arc::new(INTER_REGULAR.to_vec())).unwrap();
        assert_eq!(data.tweak.coords, VariationCoords::default());
    }

    #[test]
    fn unusable_font_files_are_rejected() {
        let pick = FacePick {
            file: "/broken.ttf".into(),
            face_index: 0,
            weight: 400.0,
            variable_weight: None,
        };
        let err = face_data(&pick, Arc::new(b"not a font".to_vec())).unwrap_err();
        assert!(matches!(err, Fallback::Unusable { ref file, .. } if file == "/broken.ttf"));
        assert!(matches!(
            read_font_file("/nonexistent/kabi-lock.ttf"),
            Err(Fallback::Unreadable { .. })
        ));
    }

    #[test]
    fn white_is_premultiplied() {
        assert_eq!(white(1.0), Color32::WHITE);
        assert_eq!(white(0.0), Color32::TRANSPARENT);
        assert_eq!(
            white(0.5),
            Color32::from_rgba_premultiplied(128, 128, 128, 128)
        );
        assert_eq!(white(2.0), Color32::WHITE);
    }
}
