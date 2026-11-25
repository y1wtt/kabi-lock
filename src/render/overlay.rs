//! egui（egui_glow）による重ね描き: 時計・日付・光の柱・錠。主画面だけに描く。
//!
//! 何をどこに描くかは `scene`（純粋）で決め、ここでは egui の図形にして GL で描くだけにする。

use std::fmt;
use std::io::{self, Read as _};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use egui::epaint::ClippedShape;
use egui::epaint::text::VariationCoords;
use egui::text::LayoutJob;
use egui::{
    Color32, Context, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, FontTweak,
    LayerId, Mesh, Painter, Pos2, RawInput, Rect, Shape, Stroke, TextFormat, TextureId, Vec2, pos2,
    vec2,
};
use glow::HasContext as _;

use crate::anim;
use crate::config::{self, Config};
use crate::fonts::{self, FacePick, Fallback};
use crate::lanes::{self, LaneAllocator};
use crate::render::egl::Gpu;
use crate::scene::{self, DateStyle, Layout, PrimaryTracker, Scene};
use crate::state::{Machine, Phase};

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
/// fontconfig の子プロセス（`fc-list`・`fc-match`）を待つ期限（2 つ合わせて、集め始めてから）。
/// ロック要求の前に待つので、これを過ぎたら埋め込みの Inter・補助フォントなしで続ける。
const FONTCONFIG_TIMEOUT: Duration = Duration::from_secs(1);
/// 子プロセスが終わったかを確かめる間隔。
const CHILD_POLL: Duration = Duration::from_millis(5);

/// 時計・日付・光の柱・錠を描く。GL の準備（シェーダー・フォント・アトラス）はロック前に済ませる。
pub struct Overlay {
    ctx: Context,
    painter: egui_glow::Painter,
    /// 錠を描き置くテクスチャ。最初に錠を出すときに作る。
    lock_layer: Option<LockLayer>,
}

/// 錠を不透明で描き置くテクスチャと、その描画先。
///
/// 錠は図形が少し重なっているので、半透明のまま図形ごとに重ねると重なりが濃く出る。
/// ここに不透明で描いてから 1 枚の絵として重ねれば、錠全体が同じ不透明度になる。
/// テクスチャは egui_glow に登録してあり、`Painter::destroy` で消える。
struct LockLayer {
    fbo: glow::Framebuffer,
    id: TextureId,
    size: [u32; 2],
}

impl Overlay {
    /// 時計・日付のフォントを選んで登録し、日付に要れば fontconfig の日本語などのフォントを補いに足す。
    ///
    /// フォントは `queries`（先に起動しておいた fontconfig）の答えで `appearance.font` の
    /// 太さ 500（時計）と 400（日付）のファイルを読む。使えなければ埋め込みの Inter にする
    /// （理由はログに出す）。fontconfig の答えは `FONTCONFIG_TIMEOUT` まで待つ。
    /// 呼ぶときは `gpu` のコンテキストが current であること（`Gpu::new` の直後はサーフェスなしで current）。
    pub fn new(gpu: &Gpu, queries: FontQueries) -> Result<Overlay, String> {
        let painter = egui_glow::Painter::new(gpu.gl().clone(), "", None, false)
            .map_err(|e| format!("egui_glow painter: {e}"))?;
        let ctx = Context::default();
        let deadline = Instant::now() + FONTCONFIG_TIMEOUT;
        let FontQueries {
            family: font_family,
            list,
            fallback,
        } = queries;
        let chosen = match system_fonts(&font_family, list, deadline) {
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
        let fallback =
            fallback.and_then(|(pattern, child)| fallback_font(&pattern, child, deadline));
        ctx.set_fonts(font_definitions(chosen, fallback));
        let mut overlay = Overlay {
            ctx,
            painter,
            lock_layer: None,
        };
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
        let lock_rect = scene
            .lock
            .as_ref()
            .map(|lock| lock_layer_rect(&layout, lock));
        let lock_layer = lock_rect.and_then(|rect| {
            let id = self.lock_layer_for(gpu, rect.size())?;
            Some((id, rect))
        });
        let out = self.ctx.run_pass(raw, |ctx| {
            paint_scene(
                &ctx.layer_painter(LayerId::background()),
                &layout,
                scene,
                lock_layer,
            );
        });
        let ppp = out.pixels_per_point;
        let mut textures = out.textures_delta;
        // 錠を描き置く前に字形のアトラスを送る（錠の図形もアトラスの白い画素を使う）
        #[expect(clippy::iter_over_hash_type)] // 順番は関係ない
        for (id, deltas) in textures.set.drain() {
            for delta in deltas {
                self.painter.set_texture(id, &delta);
            }
        }
        if let (Some(lock), Some((_, rect))) = (&scene.lock, lock_layer) {
            self.render_lock_layer(gpu, &layout, lock, rect, ppp);
        }
        let primitives = self.ctx.tessellate(out.shapes, ppp);
        self.painter
            .paint_primitives([width, height], ppp, &primitives);
        #[expect(clippy::iter_over_hash_type)] // 順番は関係ない
        for id in textures.free.drain() {
            self.painter.free_texture(id);
        }
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

    /// `size` の大きさの描き置き先を用意して、egui のテクスチャ ID を返す。
    /// 大きさが変わったら作り直す。作れなければ `None`（錠は図形を直接描く）。
    fn lock_layer_for(&mut self, gpu: &Gpu, size: Vec2) -> Option<TextureId> {
        let size = [size.x as u32, size.y as u32];
        if let Some(layer) = &self.lock_layer
            && layer.size == size
        {
            return Some(layer.id);
        }
        if let Some(old) = self.lock_layer.take() {
            self.painter.free_texture(old.id);
            // SAFETY: current な GL コンテキストで、自分が作った FBO を消すだけ。
            unsafe { gpu.gl().delete_framebuffer(old.fbo) };
        }
        // SAFETY: current な GL コンテキストに対する、引数の正しい GL 呼び出しだけ。
        let made = unsafe { new_lock_layer(gpu.gl(), size) };
        match made {
            Ok((texture, fbo)) => {
                let id = self.painter.register_native_texture(texture);
                self.lock_layer = Some(LockLayer { fbo, id, size });
                Some(id)
            }
            Err(e) => {
                log::warn!("cannot make the padlock layer, drawing it shape by shape: {e}");
                None
            }
        }
    }

    /// 錠を不透明の白で描き置く。`rect` は画面上の位置（`lock_layer_rect`）。
    fn render_lock_layer(
        &mut self,
        gpu: &Gpu,
        layout: &Layout,
        lock: &scene::LockDraw,
        rect: Rect,
        pixels_per_point: f32,
    ) {
        let Some(layer) = &self.lock_layer else {
            return;
        };
        let (fbo, size) = (layer.fbo, layer.size);
        let gl = gpu.gl();
        // SAFETY: current な GL コンテキストに対する、引数の正しい GL 呼び出しだけ。
        let previous = unsafe {
            let previous = gl.get_parameter_framebuffer(glow::FRAMEBUFFER_BINDING);
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.disable(glow::SCISSOR_TEST);
            gl.clear_color(0.0, 0.0, 0.0, 0.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            previous
        };
        let shapes = lock_shapes(layout, lock, Color32::WHITE, rect.min.to_vec2())
            .into_iter()
            .map(|shape| ClippedShape::new(Rect::EVERYTHING, shape))
            .collect();
        let primitives = self.ctx.tessellate(shapes, pixels_per_point);
        self.painter
            .paint_primitives(size, pixels_per_point, &primitives);
        // SAFETY: 元の描画先（画面のサーフェスなど）に戻すだけ。
        unsafe { gl.bind_framebuffer(glow::FRAMEBUFFER, previous) };
    }
}

/// 錠の描き置き先（RGBA8 のテクスチャと FBO）を作る。
///
/// # Safety
/// `gl` が current であること。
unsafe fn new_lock_layer(
    gl: &glow::Context,
    [width, height]: [u32; 2],
) -> Result<(glow::Texture, glow::Framebuffer), String> {
    // SAFETY: 呼び出し側が current を保証する。引数はここで作ったものだけ。
    unsafe {
        let previous = gl.get_parameter_framebuffer(glow::FRAMEBUFFER_BINDING);
        let texture = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            width as i32,
            height as i32,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(None),
        );
        // 画面と 1:1 で重ねるので補間しない
        for (name, value) in [
            (glow::TEXTURE_MIN_FILTER, glow::NEAREST),
            (glow::TEXTURE_MAG_FILTER, glow::NEAREST),
            (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
            (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
        ] {
            gl.tex_parameter_i32(glow::TEXTURE_2D, name, value as i32);
        }
        gl.bind_texture(glow::TEXTURE_2D, None);
        let fbo = match gl.create_framebuffer() {
            Ok(fbo) => fbo,
            Err(e) => {
                gl.delete_texture(texture);
                return Err(e);
            }
        };
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(texture),
            0,
        );
        let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
        gl.bind_framebuffer(glow::FRAMEBUFFER, previous);
        if status != glow::FRAMEBUFFER_COMPLETE {
            gl.delete_framebuffer(fbo);
            gl.delete_texture(texture);
            return Err(format!("framebuffer incomplete: 0x{status:x}"));
        }
        Ok((texture, fbo))
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        // GL の後片付け。終了時にコンテキストがなくても GL 呼び出しが無視されるだけで害はない
        // （錠のテクスチャは egui_glow に登録してあるので destroy で消える）
        if let Some(layer) = self.lock_layer.take() {
            // SAFETY: 自分が作った FBO を消すだけ。
            unsafe { self.painter.gl().delete_framebuffer(layer.fbo) };
        }
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

/// ロック前に起動しておく fontconfig の問い合わせ。撮影と並行に走らせ、`Overlay::new` で
/// 期限付きで答えを集める（設計書 5 章 2）。
pub struct FontQueries {
    /// `appearance.font`。
    family: String,
    /// `fc-list`（一覧を取る。`fc-match` は似たフォントで代用してしまうので使わない）。
    /// 名前が空・起動できないときは埋め込みの Inter に戻す理由。
    list: Result<PendingChild, Fallback>,
    /// 日付に補助フォントが要るときの `fc-match`（パターン, 子プロセス）。
    fallback: Option<(String, io::Result<PendingChild>)>,
}

impl FontQueries {
    /// `fc-list` と、`date_needs_fallback` なら `fc-match` を起動する（待たない）。
    pub fn spawn(family: &str, date_needs_fallback: bool) -> FontQueries {
        let list = if family.trim().is_empty() {
            Err(Fallback::EmptyName)
        } else {
            PendingChild::spawn(
                Command::new("fc-list")
                    .arg(format!("--format={}", fonts::FC_LIST_FORMAT))
                    .arg(fonts::fc_pattern(family.trim())),
            )
            .map_err(|e| Fallback::FontconfigUnavailable(format!("cannot run fc-list: {e}")))
        };
        let fallback = date_needs_fallback.then(|| {
            let pattern = fallback_pattern();
            let child = PendingChild::spawn(
                Command::new("fc-match")
                    .arg("--format=%{file}\n%{index}")
                    .arg(&pattern),
            );
            (pattern, child)
        });
        FontQueries {
            family: family.to_owned(),
            list,
            fallback,
        }
    }
}

/// 起動した子プロセス。標準出力は別スレッドで最後まで読む（パイプが詰まって子が止まらないように）。
///
/// 集めずに捨てたとき・期限を過ぎたときは kill し、回収は別スレッドで行う（ここで止まらない）。
struct PendingChild {
    child: Option<Child>,
    stdout: mpsc::Receiver<Vec<u8>>,
}

/// 子プロセスの答えが得られなかった理由。ログに出す。
#[derive(Debug, PartialEq)]
enum ChildError {
    TimedOut,
    Failed(String),
    Io(String),
}

impl fmt::Display for ChildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChildError::TimedOut => write!(f, "timed out"),
            ChildError::Failed(status) => write!(f, "exited with {status}"),
            ChildError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl PendingChild {
    /// 標準入力・標準エラーは捨て、標準出力だけをパイプでつないで起動する。
    fn spawn(command: &mut Command) -> io::Result<PendingChild> {
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdout = child.stdout.take();
        let (tx, rx) = mpsc::channel();
        let pending = PendingChild {
            child: Some(child),
            stdout: rx,
        };
        if let Some(mut out) = stdout {
            // thread::spawn は起動に失敗すると panic する（ロック要求の前なので、ロックせずに終わってしまう）。
            // 起動できなければ Err を返し、pending の Drop が子を止める
            thread::Builder::new().spawn(move || {
                let mut buf = Vec::new();
                let _ = out.read_to_end(&mut buf);
                let _ = tx.send(buf);
            })?;
        }
        Ok(pending)
    }

    /// `deadline` までに正常に終われば、その標準出力を返す。
    /// 過ぎたら kill して `TimedOut`（回収は `Drop` が別スレッドで行う）。
    fn wait_until(mut self, deadline: Instant) -> Result<Vec<u8>, ChildError> {
        let Some(child) = self.child.as_mut() else {
            return Err(ChildError::Io("no child process".into()));
        };
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    let now = Instant::now();
                    if now >= deadline {
                        return Err(ChildError::TimedOut);
                    }
                    thread::sleep(CHILD_POLL.min(deadline - now));
                }
                Err(e) => return Err(ChildError::Io(e.to_string())),
            }
        };
        if !status.success() {
            return Err(ChildError::Failed(status.to_string()));
        }
        // 子が終わればパイプが閉じて読み終わる。孫がパイプを持ち続けるときに備えて期限を付ける
        let remaining = deadline.saturating_duration_since(Instant::now());
        self.stdout
            .recv_timeout(remaining)
            .map_err(|_| ChildError::TimedOut)
    }
}

impl Drop for PendingChild {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        let _ = child.kill();
        // 止まっている子（ディスク待ちなど）の回収でロック要求を遅らせない。
        // スレッドを起動できなければ回収しない（panic しない。ゾンビはプロセスの終わりで消える）
        let _ = thread::Builder::new().spawn(move || {
            let _ = child.wait();
        });
    }
}

/// `appearance.font` のファミリーの、時計用（500）と日付用（400）のフォントを読む。
///
/// `list` は先に起動しておいた `fc-list`。答えは `deadline` まで待つ。
/// どれを使うかは `fonts::pick_faces` が決める。fontconfig が答えない・読めない・
/// egui が扱えないファイルなら `Err`。
fn system_fonts(
    family: &str,
    list: Result<PendingChild, Fallback>,
    deadline: Instant,
) -> Result<(FontData, FontData), Fallback> {
    let stdout = list?
        .wait_until(deadline)
        .map_err(|e| Fallback::FontconfigUnavailable(format!("fc-list {e}")))?;
    let faces = fonts::parse_fc_list(&String::from_utf8_lossy(&stdout));
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

/// 日時のロケールの言語で fontconfig に尋ねるパターン（`sans-serif:lang=ja` など）。
fn fallback_pattern() -> String {
    let lc_all = std::env::var("LC_ALL").ok();
    let lc_time = std::env::var("LC_TIME").ok();
    let lang = std::env::var("LANG").ok();
    let locale = scene::time_locale_name(lc_all.as_deref(), lc_time.as_deref(), lang.as_deref());
    match scene::language(locale) {
        Some(lang) => format!("sans-serif:lang={lang}"),
        None => "sans-serif".to_owned(),
    }
}

/// 日時のロケールの言語のフォントを、先に起動しておいた `fc-match` の答えで読み込む。
///
/// `fc-match` を子プロセスで呼ぶ（C のライブラリを直接つながないので unsafe もビルド時の依存も要らず、
/// デスクトップの他のアプリと同じ規則で選ばれる）。答えは `deadline` まで待つ。答えない・
/// 見つからない・読めない・壊れているときは `None`（その文字は表示されないが、ロックは続ける）。
fn fallback_font(
    pattern: &str,
    child: io::Result<PendingChild>,
    deadline: Instant,
) -> Option<FontData> {
    let stdout = match child.map(|c| c.wait_until(deadline)) {
        Ok(Ok(stdout)) => stdout,
        Ok(Err(e)) => {
            log::warn!("fc-match {pattern} {e}; no fallback font for the date");
            return None;
        }
        Err(e) => {
            log::warn!("cannot run fc-match: {e}; no fallback font for the date");
            return None;
        }
    };
    let (file, index) = scene::parse_fc_match(&String::from_utf8_lossy(&stdout))?;
    let bytes = match std::fs::read(&file) {
        Ok(bytes) => bytes,
        Err(e) => {
            log::warn!("cannot read fallback font {file}: {e}");
            return None;
        }
    };
    // 可変フォントの名前付きインスタンス（例: 0x50000）は上位 16 bit を捨ててフェイスの番号にする
    let face = fonts::face_number(index);
    // egui は読めないフォントで panic するので、同じ方法（skrifa）で先に確かめる
    if let Err(e) = skrifa::FontRef::from_index(&bytes, face) {
        log::warn!("fallback font {file} (index {index}) is not usable: {e}");
        return None;
    }
    log::debug!("fallback font for {pattern}: {file} (index {index}, face {face})");
    Some(FontData::from_blob(Arc::new(bytes), face))
}

/// 1 フレーム分の図形を積む。描く順は 光の柱 → 時計 → 日付 → 錠。
///
/// `lock_layer` は錠を描き置いたテクスチャとその位置。あれば 1 枚の絵として重ね、
/// なければ（作れなかったとき）図形を直接描く。
fn paint_scene(
    painter: &Painter,
    l: &Layout,
    scene: &Scene,
    lock_layer: Option<(TextureId, Rect)>,
) {
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
        match lock_layer {
            // テクスチャの 1 行目は描き置いた絵の下端なので、uv の上下を返す
            Some((id, rect)) => {
                painter.add(Shape::image(
                    id,
                    rect,
                    Rect::from_min_max(pos2(0.0, 1.0), pos2(1.0, 0.0)),
                    white(scene.opacity),
                ));
            }
            None => painter.extend(lock_shapes(l, lock, white(scene.opacity), Vec2::ZERO)),
        }
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

/// 錠の図形（U 字の丸い端・U 字・本体）。`shift` だけ左上へずらす（描き置き用）。
///
/// 足は本体の上端で止めてある（`scene::shackle_polyline`）。ただ丸い端と線、線と本体の
/// 継ぎ目は少し重なるので、半透明のまま 1 つずつ重ねると重なりが濃く出る。
/// 半透明で出すときは `LockLayer` に不透明で描いてから、1 枚の絵として重ねる。
fn lock_shapes(l: &Layout, lock: &scene::LockDraw, color: Color32, shift: Vec2) -> Vec<Shape> {
    let unit = l.icon_unit;
    let origin = lock_origin(l, lock) - shift;
    let to_screen = |p: [f32; 2]| pos2(origin.x + p[0] * unit, origin.y + p[1] * unit);

    let points: Vec<_> = scene::shackle_polyline(&lock.path, SHACKLE_SEGMENTS)
        .into_iter()
        .map(to_screen)
        .collect();
    let stroke = anim::SHACKLE_STROKE * unit;
    let body = Rect::from_min_max(
        to_screen([anim::BODY_X, anim::BODY_Y]),
        to_screen([anim::BODY_X + anim::BODY_W, anim::BODY_Y + anim::BODY_H]),
    );
    let radius = (anim::BODY_RADIUS * unit).round().clamp(0.0, 255.0) as u8;
    vec![
        // egui の線は端が平らなので、見える方の端（最初の点）に丸を足す（設計の stroke-linecap: round）
        Shape::circle_filled(points[0], stroke / 2.0, color),
        Shape::line(points, Stroke::new(stroke, color)),
        Shape::rect_filled(body, CornerRadius::same(radius), color),
    ]
}

/// 錠のアイコン座標の原点（画面のピクセル）。震えの分だけ横にずれる。
fn lock_origin(l: &Layout, lock: &scene::LockDraw) -> Pos2 {
    pos2(l.icon_origin[0] + lock.shake_x * l.scale, l.icon_origin[1])
}

/// 錠が解除中に動く範囲（アイコン座標）。U 字は跳ね上がりで y -4.75 まで、
/// 回転で x 63.75 まで行く（線の太さ込み）。本体は x 14〜50、y 24〜52。
const LOCK_LAYER_MIN: [f32; 2] = [12.0, -7.0];
const LOCK_LAYER_MAX: [f32; 2] = [66.0, 54.0];

/// 錠を描き置く矩形（画面の整数ピクセル）。大きさは配置だけで決まり、位置は震えで動く。
/// 整数ピクセルにそろえるので、描き置いた絵を 1:1 で重ねられる（にじまない）。
fn lock_layer_rect(l: &Layout, lock: &scene::LockDraw) -> Rect {
    let unit = l.icon_unit;
    let origin = lock_origin(l, lock);
    // 線の縁のぼかし（1px）がはみ出さないよう 1px ずつ余白をとる
    let width = ((LOCK_LAYER_MAX[0] - LOCK_LAYER_MIN[0]) * unit).ceil() + 2.0;
    let height = ((LOCK_LAYER_MAX[1] - LOCK_LAYER_MIN[1]) * unit).ceil() + 2.0;
    let left = (origin.x + LOCK_LAYER_MIN[0] * unit).floor() - 1.0;
    let top = (origin.y + LOCK_LAYER_MIN[1] * unit).floor() - 1.0;
    Rect::from_min_size(pos2(left, top), vec2(width, height))
}

/// 重ね物を描くのに要る状態一式（App が 1 つ持つ）。
///
/// EGL が使えないとき（`overlay` が `None`）も、柱のレーンなどは動かしてよい（描かないだけ）。
pub struct OverlayHost {
    overlay: Option<Overlay>,
    lanes: LaneAllocator,
    primary: PrimaryTracker,
    date: DateStyle,
    input: config::Input,
    /// 先に起動しておいた fontconfig の問い合わせ。`attach_gpu` で答えを集める。
    font_queries: Option<FontQueries>,
    /// 最後に柱を出した時刻。これから `pillar_fade_ms` の間は柱が動いている。
    last_keystroke_ms: Option<u64>,
    /// 最後に描いた内容（変わったときだけ描き直すため）。
    last: Option<Drawn>,
    /// 主画面が替わったときの前の主画面の出力名（重ね物を消すために描き直すまで持つ）。
    previous_primary: Option<String>,
}

/// 最後に描いた内容のうち、時間で変わるもの。
struct Drawn {
    phase: Phase,
    time_text: String,
    date_text: String,
}

impl OverlayHost {
    /// App を作るときに呼ぶ（ロックの前、撮影より先）。GL はまだ使わない。
    /// fontconfig の子プロセスをここで起動して撮影と並行に走らせ、答えは `attach_gpu` で集める。
    /// ロック前にフォーカスしていた出力を sway IPC で尋ねるのもここ。
    pub fn new(config: &Config) -> OverlayHost {
        let lc_all = std::env::var("LC_ALL").ok();
        let lc_time = std::env::var("LC_TIME").ok();
        let lang = std::env::var("LANG").ok();
        let locale = scene::chrono_locale(scene::time_locale_name(
            lc_all.as_deref(),
            lc_time.as_deref(),
            lang.as_deref(),
        ));
        let date = DateStyle::new(&config.appearance.date_format, locale);
        let font_queries =
            FontQueries::spawn(&config.appearance.font, scene::needs_fallback_font(&date));
        let focused = crate::wl::outputs::focused_output();
        log::info!("focused output before locking: {focused:?}");
        let input = config.input.clone();
        // レーン数は拡大前の設定値で決める（主画面の大きさでレーン数が変わらないように）
        let count = lanes::lane_count(input.band_width, input.pillar_width);
        OverlayHost {
            overlay: None,
            lanes: LaneAllocator::new(count, u64::from(input.pillar_fade_ms), fastrand::Rng::new()),
            primary: PrimaryTracker::new(config.display.primary.clone(), focused),
            date,
            input,
            font_queries: Some(font_queries),
            last_keystroke_ms: None,
            last: None,
            previous_primary: None,
        }
    }

    /// GPU が使えるようになったら（ロックの前に）呼ぶ。シェーダー・フォント・アトラスを準備する。
    /// 失敗したら重ね物なしで続ける（背景とパスワード入力は動く）。
    pub fn attach_gpu(&mut self, gpu: &Gpu) {
        let Some(queries) = self.font_queries.take() else {
            return;
        };
        match Overlay::new(gpu, queries) {
            Ok(overlay) => self.overlay = Some(overlay),
            Err(e) => log::warn!("overlay disabled: {e}"),
        }
    }

    /// 重ね物の GL の状態（egui_glow の painter）を捨て、以後は重ね物を描かない。
    /// GPU の描画が panic して shm の黒に切り替えるときに、`Gpu` を捨てる前に呼ぶ。
    pub fn disable_gpu(&mut self) {
        self.overlay = None;
        self.font_queries = None;
    }

    /// パスワードバッファが打鍵を受け付けたとき（`push_str` / `backspace` が `true`）に呼ぶ。
    pub fn keystroke(&mut self, now_ms: u64) {
        self.lanes.spawn(now_ms);
        self.last_keystroke_ms = Some(now_ms);
    }

    /// Enter でパスワードを確かめ始めたとき（確認中になったとき）に呼ぶ。
    ///
    /// Enter より前の柱を消す。消さないと、`pillar_fade_ms` が失敗の表示と PAM の時間より長いとき、
    /// 入力待ちに戻ったところで再び描かれる。確認中に柱のためのフレームを続けないよう、最後の打鍵も忘れる。
    pub fn submitted(&mut self) {
        self.lanes.clear();
        self.last_keystroke_ms = None;
    }

    /// 出力の一覧（出力名, description）を渡して主画面の添字を返す。一覧が変わったときだけ選び直す。
    pub fn refresh_primary<'a, I>(&mut self, outputs: I) -> Option<usize>
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
        I::IntoIter: Clone,
    {
        let index = self.primary.update(outputs);
        // 添字ではなく出力名で比べる（主画面が抜かれて別の出力が同じ添字に詰めてきたときも描き直す）
        if self.primary.changed() {
            log::info!("overlay output index: {index:?}");
            // 新しい主画面には最初から描き直す
            self.last = None;
            // 前の主画面には重ね物が残っているので、App に重ね物なしで描き直してもらう
            if self.overlay.is_some() {
                self.previous_primary = self.primary.previous_name().map(str::to_owned);
            }
        }
        index
    }

    /// 主画面が替わったとき、前の主画面の出力名を 1 度だけ返す（`refresh_primary` の後で呼ぶ）。
    /// その出力がまだあれば、重ね物を消すために描き直す。
    pub fn take_previous_primary(&mut self) -> Option<String> {
        self.previous_primary.take()
    }

    /// 出力 `index` の重ね物が動き続けている（毎フレーム描き直す）か:
    /// 主画面で、柱が表示中・錠が震えている・解除のアニメーション中。
    pub fn animating(&self, index: usize, machine: &Machine, now_ms: u64) -> bool {
        if self.overlay.is_none() || self.primary.primary() != Some(index) {
            return false;
        }
        let shaking = matches!(machine.phase(), Phase::Failed { .. })
            && machine
                .phase()
                .elapsed_ms(now_ms)
                .is_some_and(|ms| ms < anim::SHAKE_MS);
        // 一番新しい柱が消えるまでは、どれかの柱が動いている
        let pillars = self
            .last_keystroke_ms
            .is_some_and(|t| now_ms.saturating_sub(t) < u64::from(self.input.pillar_fade_ms));
        shaking || pillars || matches!(machine.phase(), Phase::Unlocking { .. })
    }

    /// 主画面の重ね物が、前に描いたときから変わったか（時刻・日付・状態・主画面）、動き続けているか。
    /// `Tick` ごとに（`refresh_primary` の後で）呼び、`true` なら主画面を描き直す。
    pub fn wants_frame(&self, machine: &Machine, now_ms: u64) -> bool {
        let Some(index) = self.primary.primary() else {
            return false;
        };
        if self.overlay.is_none() {
            return false;
        }
        if self.animating(index, machine, now_ms) {
            return true;
        }
        let Some(last) = &self.last else {
            return true;
        };
        let wall = chrono::Local::now().fixed_offset();
        last.phase != machine.phase()
            || last.time_text != wall.format("%H:%M").to_string()
            || last.date_text != self.date.format(&wall)
    }

    /// 出力 `index` が主画面なら重ね物を描く。描画先は current にしておき、背景は描いておく。
    /// `size` は `Background::draw` に渡したのと同じ幅と高さ、`fade` は同じフェードの不透明度。
    pub fn draw(
        &mut self,
        gpu: &Gpu,
        index: usize,
        size: (u32, u32),
        machine: &Machine,
        now_ms: u64,
        fade: f32,
    ) {
        let Some(overlay) = self.overlay.as_mut() else {
            return;
        };
        if self.primary.primary() != Some(index) {
            return;
        }
        let wall = chrono::Local::now().fixed_offset();
        let pillars = self.lanes.pillars(now_ms);
        let scene = scene::build(
            machine,
            now_ms,
            fade,
            pillars,
            &self.input,
            &wall,
            &self.date,
        );
        overlay.draw(gpu, size.0, size.1, &scene, &self.input);
        self.last = Some(Drawn {
            phase: machine.phase(),
            time_text: scene.time_text,
            date_text: scene.date_text,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Event, FAIL_HOLD_MS};
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

    fn sh(script: &str) -> PendingChild {
        PendingChild::spawn(Command::new("sh").arg("-c").arg(script)).unwrap()
    }

    #[test]
    fn child_output_is_collected_before_the_deadline() {
        let deadline = Instant::now() + Duration::from_secs(10);
        assert_eq!(sh("echo x").wait_until(deadline), Ok(b"x\n".to_vec()));
        // パイプの容量（64 KiB）を超える出力でも詰まらない
        let big = sh("head -c 200000 /dev/zero").wait_until(deadline).unwrap();
        assert_eq!(big.len(), 200_000);
    }

    #[test]
    fn failing_child_is_an_error() {
        let deadline = Instant::now() + Duration::from_secs(10);
        assert!(matches!(
            sh("exit 3").wait_until(deadline),
            Err(ChildError::Failed(_))
        ));
        assert!(PendingChild::spawn(&mut Command::new("/nonexistent/kabi-lock-test")).is_err());
    }

    #[test]
    fn slow_child_is_killed_at_the_deadline() {
        let start = Instant::now();
        let child = PendingChild::spawn(Command::new("sleep").arg("5")).unwrap();
        let result = child.wait_until(start + Duration::from_millis(200));
        let elapsed = start.elapsed();
        assert_eq!(result, Err(ChildError::TimedOut));
        assert!(elapsed >= Duration::from_millis(200), "{elapsed:?}");
        assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
    }

    #[test]
    fn dropping_an_uncollected_child_does_not_block() {
        let start = Instant::now();
        drop(PendingChild::spawn(Command::new("sleep").arg("5")).unwrap());
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    /// GL も fontconfig も sway IPC も使わない OverlayHost。
    fn host(pillar_fade_ms: u32) -> OverlayHost {
        let input = config::Input {
            pillar_fade_ms,
            ..Default::default()
        };
        let count = lanes::lane_count(input.band_width, input.pillar_width);
        OverlayHost {
            overlay: None,
            lanes: LaneAllocator::new(
                count,
                u64::from(pillar_fade_ms),
                fastrand::Rng::with_seed(1),
            ),
            primary: PrimaryTracker::new(Vec::new(), None),
            date: DateStyle::new("%m/%d", chrono::Locale::POSIX),
            input,
            font_queries: None,
            last_keystroke_ms: None,
            last: None,
            previous_primary: None,
        }
    }

    #[test]
    fn pillars_from_before_enter_do_not_come_back_after_a_failure() {
        // 柱が消えるまでの時間が、失敗の表示（1.2 秒）と PAM の時間より長い
        let mut host = host(5_000);
        let mut m = Machine::new();
        host.keystroke(0); // Enter より前の柱
        m.handle(Event::Submit { empty: false }, 100);
        host.submitted();
        m.handle(Event::PasswordChecked { ok: false }, 400);
        host.keystroke(500); // 失敗中に打った柱
        let now = 400 + FAIL_HOLD_MS;
        m.handle(Event::Tick, now);
        assert_eq!(m.phase(), Phase::Idle);
        let pillars = host.lanes.pillars(now).to_vec();
        let wall = chrono::Local::now().fixed_offset();
        let scene = scene::build(&m, now, 1.0, &pillars, &host.input, &wall, &host.date);
        // Enter より前の柱は戻ってこず、失敗中に打った柱は入力待ちに戻っても残る
        assert_eq!(scene.pillars.len(), 1);
        assert_eq!(
            pillars.iter().map(|p| p.started_ms).collect::<Vec<_>>(),
            [500]
        );
    }

    #[test]
    fn submitting_stops_the_pillar_animation() {
        let mut host = host(900);
        host.keystroke(0);
        host.submitted();
        // 確認中に、Enter の直前の打鍵のせいで主画面を毎フレーム描き続けない
        assert_eq!(host.last_keystroke_ms, None);
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

    #[test]
    fn lock_layer_holds_the_whole_padlock() {
        // 跳ね上がり・回転・震えのどの瞬間でも、錠（線の縁のぼかし 0.5px 込み）が描き置き先に収まる
        for height in [720u32, 1080, 2160] {
            let l = scene::layout(height * 16 / 9, height, 768);
            let mut paths = vec![anim::shackle_path(anim::ShackleGeom::CLOSED)];
            paths.extend((0..=800).step_by(10).map(|ms| {
                anim::shackle_path(anim::shackle_at(ms as f32, &anim::UnlockTiming::default()))
            }));
            for path in paths {
                for shake_x in [-12.0, -3.3, 0.0, 7.7, 12.0] {
                    let lock = scene::LockDraw { path, shake_x };
                    let rect = lock_layer_rect(&l, &lock);
                    assert_eq!(rect.min, rect.min.round(), "{rect:?}");
                    assert_eq!(rect.size(), rect.size().round(), "{rect:?}");
                    for shape in lock_shapes(&l, &lock, Color32::WHITE, Vec2::ZERO) {
                        let bounds = shape.visual_bounding_rect().expand(0.5);
                        assert!(
                            rect.contains_rect(bounds),
                            "{height}p {lock:?}: {bounds:?} not in {rect:?}"
                        );
                    }
                }
            }
        }
    }

    /// 半透明の錠を描いて、図形の重なり（U 字の丸い端、足と本体の継ぎ目）が濃く出ないことを確かめる。
    /// 今のセッションの Wayland 接続で EGL を作る（ロックはしない）。`cargo test -- --ignored` で手動実行。
    #[test]
    #[ignore = "needs a running Wayland compositor and a GPU"]
    fn translucent_padlock_has_one_opacity() {
        let conn = wayland_client::Connection::connect_to_env().expect("wayland");
        let gpu = Gpu::new(&conn).expect("gpu");
        let mut overlay = Overlay::new(&gpu, FontQueries::spawn("Inter", false)).expect("overlay");
        let (w, h) = (1920u32, 1080u32);
        const BG: u8 = 38;
        // 白を不透明度 0.5 で 1 回だけ重ねた明るさ
        let once = BG as f32 * 0.5 + 255.0 * 0.5;
        let gl = gpu.gl();
        for path in [
            anim::shackle_path(anim::ShackleGeom::CLOSED),
            anim::shackle_path(anim::shackle_at(600.0, &anim::UnlockTiming::default())),
        ] {
            let lock = scene::LockDraw { path, shake_x: 0.0 };
            let scene = Scene {
                opacity: 0.5,
                time_text: String::new(),
                date_text: String::new(),
                pillars: vec![],
                lock: Some(lock),
            };
            let mut pixels = vec![0u8; (w * h * 4) as usize];
            // SAFETY: current な GL コンテキストに対する、引数の正しい GL 呼び出しだけ。
            unsafe {
                let (texture, fbo) = new_lock_layer(gl, [w, h]).expect("target");
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
                gl.viewport(0, 0, w as i32, h as i32);
                let bg = BG as f32 / 255.0;
                gl.clear_color(bg, bg, bg, 1.0);
                gl.clear(glow::COLOR_BUFFER_BIT);
                overlay.draw(&gpu, w, h, &scene, &config::Input::default());
                gl.read_pixels(
                    0,
                    0,
                    w as i32,
                    h as i32,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelPackData::Slice(Some(&mut pixels)),
                );
                assert_eq!(gl.get_error(), glow::NO_ERROR);
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                gl.delete_framebuffer(fbo);
                gl.delete_texture(texture);
            }
            // read_pixels は下の行から並ぶ
            let at = |x: u32, y: u32| pixels[(((h - 1 - y) * w + x) * 4) as usize];
            let l = scene::layout(w, h, config::Input::default().band_width);
            let rect = lock_layer_rect(&l, &lock);
            let mut brightest = 0;
            for y in rect.min.y as u32..rect.max.y as u32 {
                for x in rect.min.x as u32..rect.max.x as u32 {
                    brightest = brightest.max(at(x, y));
                }
            }
            assert!(
                (brightest as f32 - once).abs() <= 2.0,
                "{lock:?}: brightest {brightest}, expected {once}"
            );
            // 本体の真ん中は 1 回だけ重ねた明るさ（錠が描けている）
            let center = lock_origin(&l, &lock)
                + vec2(
                    anim::BODY_X + anim::BODY_W / 2.0,
                    anim::BODY_Y + anim::BODY_H / 2.0,
                ) * l.icon_unit;
            let body = at(center.x as u32, center.y as u32) as f32;
            assert!((body - once).abs() <= 2.0, "body {body}, expected {once}");
        }
    }
}
