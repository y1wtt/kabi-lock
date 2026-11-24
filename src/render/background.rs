//! 背景: スクリーンショットの縮小ぼかし・黒からのフェード・ビネット・解除時のぼかし解除。
//!
//! ぼかしはロック前に 1 回だけ作る（縮小 → 横のガウス → 縦のガウス、FBO で 3 パス）。
//! 毎フレームの描画は、ぼかし済みと元画像の 2 枚を 1 回のパスで合成するだけ。

use std::collections::HashMap;
use std::time::Instant;

use glow::HasContext;
use log::{debug, warn};
use wayland_client::protocol::wl_shm;

use crate::anim::UnlockTiming;
use crate::render::egl::Gpu;
use crate::state::Phase;
use crate::wl::screencopy::Screenshot;

// ---- 純粋な計算 ----

/// 設定の `blur` が範囲外・NaN・負のときに使う値。
pub const DEFAULT_BLUR: f32 = 7.0;
/// 1 方向のガウスの最大半径（縮小後の画素）。シェーダーの配列の大きさと同じ。
pub const MAX_RADIUS: usize = 64;
/// 縮小の最大倍率。
pub const MAX_DOWNSCALE: u32 = 4;

/// `appearance.blur` を 0.5〜50 に丸める。NaN と負は 7。
pub fn clamp_blur(blur: f32) -> f32 {
    if blur.is_nan() || blur < 0.0 {
        DEFAULT_BLUR
    } else {
        blur.clamp(0.5, 50.0)
    }
}

/// `blur` を、スクリーンショットの画素で測ったガウスの σ に直す。
///
/// swaylock-effects の `--effect-blur <blur>x5`（半径 `blur` の箱型ぼかしを 5 回）と
/// 同じ分散になるようにする: 箱 1 回の分散は ((2r+1)² − 1) / 12 = r(r+1)/3。
/// `blur = 7` で σ ≈ 9.66。
pub fn sigma_for_blur(blur: f32) -> f32 {
    let r = clamp_blur(blur);
    (5.0 * r * (r + 1.0) / 3.0).sqrt()
}

/// σ（元画像の画素）に対する縮小倍率。σ の半分を超えない整数で、1〜4。
pub fn downscale_factor(sigma: f32) -> u32 {
    ((sigma / 2.0).floor() as u32).clamp(1, MAX_DOWNSCALE)
}

/// 縮小後の大きさ（切り上げ、最低 1）。
pub fn scaled_size(width: u32, height: u32, factor: u32) -> (u32, u32) {
    let f = factor.max(1);
    (width.div_ceil(f).max(1), height.div_ceil(f).max(1))
}

/// 1 方向のガウスの重み（中心から外へ、`weights[0]` が中心）。
///
/// 半径は ceil(3σ)（1〜`MAX_RADIUS`）。`weights[0] + 2 * (残りの和) = 1` に正規化する。
pub fn kernel(sigma: f32) -> Vec<f32> {
    let sigma = sigma.max(0.1);
    let radius = ((sigma * 3.0).ceil() as usize).clamp(1, MAX_RADIUS);
    let mut weights: Vec<f32> = (0..=radius)
        .map(|i| (-((i * i) as f32) / (2.0 * sigma * sigma)).exp())
        .collect();
    let sum = weights[0] + 2.0 * weights[1..].iter().sum::<f32>();
    for w in &mut weights {
        *w /= sum;
    }
    weights
}

/// スクリーンショットの形式をテクスチャにするときの指定。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureLayout {
    pub internal_format: u32,
    pub format: u32,
    pub ty: u32,
    /// 赤と青を入れ替えて読む（テクスチャのスウィズル）。
    pub swap_rb: bool,
}

/// wl_shm の形式 → テクスチャの指定。扱えない形式は `None`（その出力は黒）。
///
/// wl_shm の形式はリトルエンディアンの 32 ビット値の並びなので、`Xrgb8888` はメモリ上
/// B, G, R, X。GLES3 には BGRA の形式がないので RGBA として送り、赤と青をスウィズルで入れ替える。
/// アルファは使わない（合成のシェーダーが 1.0 にする）。
pub fn texture_layout(format: wl_shm::Format) -> Option<TextureLayout> {
    use wl_shm::Format as F;
    let (internal_format, ty) = match format {
        F::Xrgb8888 | F::Argb8888 | F::Xbgr8888 | F::Abgr8888 => (glow::RGBA8, glow::UNSIGNED_BYTE),
        F::Xrgb2101010 | F::Argb2101010 | F::Xbgr2101010 | F::Abgr2101010 => {
            (glow::RGB10_A2, glow::UNSIGNED_INT_2_10_10_10_REV)
        }
        _ => return None,
    };
    let swap_rb = matches!(
        format,
        F::Xrgb8888 | F::Argb8888 | F::Xrgb2101010 | F::Argb2101010
    );
    Some(TextureLayout {
        internal_format,
        format: glow::RGBA,
        ty,
        swap_rb,
    })
}

/// 黒 → ぼかしの進み具合（0.0〜1.0）。
///
/// `first_configure_ms` は `App` が最初の configure で記録した時刻（出力ごとではなく全体で 1 つ）。
/// まだ configure を受けていなければ 0.0。
pub fn fade_for(first_configure_ms: Option<u64>, now_ms: u64, fade_in_ms: u32) -> f32 {
    match first_configure_ms {
        None => 0.0,
        Some(start) => {
            crate::anim::fade_alpha(now_ms.saturating_sub(start) as f32, fade_in_ms as f32)
        }
    }
}

/// ぼかし → くっきりの進み具合。解除中だけ `timing` に従って進み、ロックを外した後（`Done`）は 1.0。
/// `timing` は状態機械と同じもの（`Machine::unlock_timing`）を渡す。
pub fn reveal_for(phase: Phase, now_ms: u64, timing: &UnlockTiming) -> f32 {
    match phase {
        Phase::Unlocking { .. } => {
            crate::anim::reveal_progress(phase.elapsed_ms(now_ms).unwrap_or(0.0), timing)
        }
        Phase::Done => 1.0,
        _ => 0.0,
    }
}

/// 背景がまだ動いている（フレームコールバックで描き直す必要がある）か。
pub fn needs_frames(fade: f32, phase: Phase) -> bool {
    fade < 1.0 || matches!(phase, Phase::Unlocking { .. })
}

// ---- GPU ----

/// 画面全体を覆う 1 枚の三角形（頂点バッファなし）。`v_uv` は左下が (0, 0)。
const VERTEX_SHADER: &str = r#"#version 300 es
out vec2 v_uv;
void main() {
    vec2 pos = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    v_uv = pos;
    gl_Position = vec4(pos * 2.0 - 1.0, 0.0, 1.0);
}
"#;

/// 縮小: ミップマップの段 `u_lod` を読む（元画像を箱型に平均したもの）。
const DOWNSCALE_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_src;
uniform float u_lod;
in vec2 v_uv;
out vec4 frag;
void main() {
    frag = vec4(textureLod(u_src, v_uv, u_lod).rgb, 1.0);
}
"#;

/// 1 方向のガウス。`u_step` は 1 画素ぶんの uv（横なら (1/w, 0)）。
const BLUR_SHADER: &str = r#"#version 300 es
precision highp float;
const int MAX_RADIUS = 64;
uniform sampler2D u_src;
uniform vec2 u_step;
uniform int u_radius;
uniform float u_weights[MAX_RADIUS + 1];
in vec2 v_uv;
out vec4 frag;
void main() {
    vec3 sum = texture(u_src, v_uv).rgb * u_weights[0];
    for (int i = 1; i <= u_radius; ++i) {
        vec2 offset = u_step * float(i);
        sum += (texture(u_src, v_uv + offset).rgb + texture(u_src, v_uv - offset).rgb) * u_weights[i];
    }
    frag = vec4(sum, 1.0);
}
"#;

/// 合成: ぼかし × ビネット × フェード（黒の上）→ 解除で元画像へ。
const COMPOSITE_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_blurred;
uniform sampler2D u_sharp;
uniform float u_fade;
uniform float u_reveal;
in vec2 v_uv;
out vec4 frag;
void main() {
    // テクスチャは 1 行目（t = 0）が画面の上。画面の下端が v_uv.y = 0 なので上下を返す
    vec2 uv = vec2(v_uv.x, 1.0 - v_uv.y);
    // swaylock-effects の --effect-vignette 0.5:0.5 と同じ式（中央 1.0、四辺 0.5）
    float vignette = clamp(0.5 + 0.5 * 16.0 * uv.x * uv.y * (1.0 - uv.x) * (1.0 - uv.y), 0.0, 1.0);
    vec3 blurred = texture(u_blurred, uv).rgb * vignette * u_fade;
    vec3 sharp = texture(u_sharp, uv).rgb;
    frag = vec4(mix(blurred, sharp, u_reveal), 1.0);
}
"#;

/// 縮小ぼかしの 1 パス。`step` が `None` なら縮小、`Some` なら 1 方向のガウス（1 画素ぶんの uv）。
struct Pass {
    src: glow::Texture,
    dst: glow::Texture,
    step: Option<[f32; 2]>,
}

struct Textures {
    /// 元画像（ミップマップ付き）。解除のときに使う。
    sharp: glow::Texture,
    /// 縮小してぼかしたもの。
    blurred: glow::Texture,
}

pub struct Background {
    vao: glow::VertexArray,
    downscale: glow::Program,
    blur: glow::Program,
    composite: glow::Program,
    /// 元画像の画素で測った σ。
    sigma: f32,
    textures: HashMap<String, Textures>,
}

impl Background {
    /// シェーダーをコンパイルする。`gpu` は current であること（`Gpu::new` の直後はそうなっている）。
    pub fn new(gpu: &Gpu, blur: f32) -> Result<Background, String> {
        let gl = gpu.gl();
        // SAFETY: current な GL コンテキストに対する、引数の正しい GL 呼び出しだけ。
        unsafe {
            let downscale = link(gl, DOWNSCALE_SHADER)?;
            let blur_program = link(gl, BLUR_SHADER)?;
            let composite = link(gl, COMPOSITE_SHADER)?;
            let vao = gl.create_vertex_array()?;
            let sigma = sigma_for_blur(blur);
            debug!(
                "background shaders ready (blur {} -> sigma {sigma:.2}px, downscale 1/{})",
                clamp_blur(blur),
                downscale_factor(sigma)
            );
            Ok(Background {
                vao,
                downscale,
                blur: blur_program,
                composite,
                sigma,
                textures: HashMap::new(),
            })
        }
    }

    /// スクリーンショットを GPU に送り、縮小ぼかしを作っておく。
    ///
    /// 扱えない形式や壊れた大きさなら警告して何もしない（その出力は黒）。
    /// 終わったら framebuffer 0 を戻す。
    pub fn upload(&mut self, gpu: &Gpu, shot: &Screenshot) {
        let started = Instant::now();
        let Some(layout) = texture_layout(shot.format) else {
            warn!(
                "screenshot of {}: unsupported format {:?}, using black",
                shot.output_name, shot.format
            );
            return;
        };
        let (w, h, stride) = (shot.width, shot.height, shot.stride);
        let needed = (stride as usize) * (h as usize).saturating_sub(1) + w as usize * 4;
        if w == 0 || h == 0 || stride % 4 != 0 || stride < w * 4 || shot.pixels.len() < needed {
            warn!(
                "screenshot of {}: bad size {w}x{h} stride {stride}, using black",
                shot.output_name
            );
            return;
        }
        let gl = gpu.gl();
        // SAFETY: current な GL コンテキスト。画素の長さは上で確かめた。
        let result = unsafe { self.make_textures(gl, shot, layout) };
        match result {
            Ok(textures) => {
                if let Some(old) = self.textures.insert(shot.output_name.clone(), textures) {
                    // SAFETY: もう参照しないテクスチャを消す
                    unsafe {
                        gl.delete_texture(old.sharp);
                        gl.delete_texture(old.blurred);
                    }
                }
                debug!(
                    "background for {} ({w}x{h}) uploaded in {:?}",
                    shot.output_name,
                    started.elapsed()
                );
            }
            Err(e) => warn!(
                "background for {} failed, using black: {e}",
                shot.output_name
            ),
        }
    }

    unsafe fn make_textures(
        &self,
        gl: &glow::Context,
        shot: &Screenshot,
        layout: TextureLayout,
    ) -> Result<Textures, String> {
        // SAFETY: current な GL コンテキストに対する GL 呼び出し。呼び出し側が引数の正しさを保証する。
        unsafe {
            let (w, h) = (shot.width as i32, shot.height as i32);

            // 元画像。行の長さは stride から決める
            let sharp = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(sharp));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
            gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, (shot.stride / 4) as i32);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                layout.internal_format as i32,
                w,
                h,
                0,
                layout.format,
                layout.ty,
                glow::PixelUnpackData::Slice(Some(&shot.pixels)),
            );
            gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, 0);
            if layout.swap_rb {
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_SWIZZLE_R, glow::BLUE as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_SWIZZLE_B, glow::RED as i32);
            }
            set_sampling(gl, glow::LINEAR_MIPMAP_LINEAR);
            gl.generate_mipmap(glow::TEXTURE_2D);

            // 縮小用の 2 枚（a に縮小 → b に横 → a に縦）
            let factor = downscale_factor(self.sigma);
            let (dw, dh) = scaled_size(shot.width, shot.height, factor);
            let a = target_texture(gl, dw, dh)?;
            let b = target_texture(gl, dw, dh)?;
            let fbo = gl.create_framebuffer()?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.viewport(0, 0, dw as i32, dh as i32);
            gl.disable(glow::BLEND);
            gl.disable(glow::SCISSOR_TEST);
            gl.bind_vertex_array(Some(self.vao));
            gl.active_texture(glow::TEXTURE0);

            let weights = kernel(self.sigma / factor as f32);
            let mut padded = [0.0f32; MAX_RADIUS + 1];
            padded[..weights.len()].copy_from_slice(&weights);
            let radius = (weights.len() - 1) as i32;

            let passes = [
                Pass {
                    src: sharp,
                    dst: a,
                    step: None,
                },
                Pass {
                    src: a,
                    dst: b,
                    step: Some([1.0 / dw as f32, 0.0]),
                },
                Pass {
                    src: b,
                    dst: a,
                    step: Some([0.0, 1.0 / dh as f32]),
                },
            ];
            let mut status = Ok(());
            for Pass { src, dst, step } in passes {
                gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    Some(dst),
                    0,
                );
                if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
                    status = Err("framebuffer incomplete".to_string());
                    break;
                }
                gl.bind_texture(glow::TEXTURE_2D, Some(src));
                match step {
                    None => {
                        gl.use_program(Some(self.downscale));
                        uniform_i32(gl, self.downscale, "u_src", 0);
                        uniform_f32(gl, self.downscale, "u_lod", (factor as f32).log2());
                    }
                    Some([sx, sy]) => {
                        gl.use_program(Some(self.blur));
                        uniform_i32(gl, self.blur, "u_src", 0);
                        uniform_i32(gl, self.blur, "u_radius", radius);
                        let step_loc = gl.get_uniform_location(self.blur, "u_step");
                        gl.uniform_2_f32(step_loc.as_ref(), sx, sy);
                        let weights_loc = gl.get_uniform_location(self.blur, "u_weights");
                        gl.uniform_1_f32_slice(weights_loc.as_ref(), &padded);
                    }
                }
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
            }

            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.delete_framebuffer(fbo);
            gl.delete_texture(b);
            let error = gl.get_error();
            if status.is_ok() && error != glow::NO_ERROR {
                status = Err(format!("GL error 0x{error:x}"));
            }
            match status {
                Ok(()) => Ok(Textures { sharp, blurred: a }),
                Err(e) => {
                    gl.delete_texture(sharp);
                    gl.delete_texture(a);
                    Err(e)
                }
            }
        }
    }

    /// 今 bind されている framebuffer（通常は出力の EGL サーフェス）に背景を描く。
    ///
    /// `fade` 0..1（黒→ぼかし）、`reveal` 0..1（ぼかし→くっきり）。スクショがない出力は黒。
    /// `width` / `height` はバッファの画素数。
    pub fn draw(
        &self,
        gpu: &Gpu,
        output_name: &str,
        width: u32,
        height: u32,
        fade: f32,
        reveal: f32,
    ) {
        let gl = gpu.gl();
        // SAFETY: current な GL コンテキストに対する、引数の正しい GL 呼び出しだけ。
        unsafe {
            gl.viewport(0, 0, width as i32, height as i32);
            gl.disable(glow::BLEND);
            gl.disable(glow::SCISSOR_TEST);
            let Some(textures) = self.textures.get(output_name) else {
                gl.clear_color(0.0, 0.0, 0.0, 1.0);
                gl.clear(glow::COLOR_BUFFER_BIT);
                return;
            };
            gl.use_program(Some(self.composite));
            gl.bind_vertex_array(Some(self.vao));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(textures.blurred));
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(textures.sharp));
            gl.active_texture(glow::TEXTURE0);
            uniform_i32(gl, self.composite, "u_blurred", 0);
            uniform_i32(gl, self.composite, "u_sharp", 1);
            uniform_f32(gl, self.composite, "u_fade", fade.clamp(0.0, 1.0));
            uniform_f32(gl, self.composite, "u_reveal", reveal.clamp(0.0, 1.0));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
        }
    }
}

/// 頂点シェーダー（共通）と `fragment` をリンクする。
unsafe fn link(gl: &glow::Context, fragment: &str) -> Result<glow::Program, String> {
    // SAFETY: current な GL コンテキストに対する GL 呼び出し。呼び出し側が引数の正しさを保証する。
    unsafe {
        let program = gl.create_program()?;
        let mut shaders = Vec::new();
        for (kind, source) in [
            (glow::VERTEX_SHADER, VERTEX_SHADER),
            (glow::FRAGMENT_SHADER, fragment),
        ] {
            let shader = gl.create_shader(kind)?;
            gl.shader_source(shader, source);
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                let log = gl.get_shader_info_log(shader);
                gl.delete_shader(shader);
                gl.delete_program(program);
                return Err(format!("shader compile failed: {log}"));
            }
            gl.attach_shader(program, shader);
            shaders.push(shader);
        }
        gl.link_program(program);
        for shader in shaders {
            gl.detach_shader(program, shader);
            gl.delete_shader(shader);
        }
        if !gl.get_program_link_status(program) {
            let log = gl.get_program_info_log(program);
            gl.delete_program(program);
            return Err(format!("program link failed: {log}"));
        }
        Ok(program)
    }
}

/// 縮小・ぼかしの描き込み先（RGBA8、ミップマップなし）。
unsafe fn target_texture(
    gl: &glow::Context,
    width: u32,
    height: u32,
) -> Result<glow::Texture, String> {
    // SAFETY: current な GL コンテキストに対する GL 呼び出し。呼び出し側が引数の正しさを保証する。
    unsafe {
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
        set_sampling(gl, glow::LINEAR);
        Ok(texture)
    }
}

/// bind 中のテクスチャの拡大縮小と端の扱い。
unsafe fn set_sampling(gl: &glow::Context, min_filter: u32) {
    // SAFETY: current な GL コンテキストに対する GL 呼び出し。呼び出し側が引数の正しさを保証する。
    unsafe {
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            min_filter as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MAG_FILTER,
            glow::LINEAR as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_S,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_T,
            glow::CLAMP_TO_EDGE as i32,
        );
    }
}

unsafe fn uniform_i32(gl: &glow::Context, program: glow::Program, name: &str, value: i32) {
    // SAFETY: current な GL コンテキストに対する GL 呼び出し。呼び出し側が引数の正しさを保証する。
    unsafe {
        let location = gl.get_uniform_location(program, name);
        gl.uniform_1_i32(location.as_ref(), value);
    }
}

unsafe fn uniform_f32(gl: &glow::Context, program: glow::Program, name: &str, value: f32) {
    // SAFETY: current な GL コンテキストに対する GL 呼び出し。呼び出し側が引数の正しさを保証する。
    unsafe {
        let location = gl.get_uniform_location(program, name);
        gl.uniform_1_f32(location.as_ref(), value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const D: UnlockTiming = UnlockTiming {
        open_ms: 800.0,
        reveal_ms: 700.0,
    };

    fn close(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn blur_is_clamped_and_defaults_on_garbage() {
        assert_eq!(clamp_blur(7.0), 7.0);
        assert_eq!(clamp_blur(0.0), 0.5);
        assert_eq!(clamp_blur(0.2), 0.5);
        assert_eq!(clamp_blur(100.0), 50.0);
        assert_eq!(clamp_blur(f32::INFINITY), 50.0);
        assert_eq!(clamp_blur(f32::NAN), DEFAULT_BLUR);
        assert_eq!(clamp_blur(-1.0), DEFAULT_BLUR);
        assert_eq!(clamp_blur(f32::NEG_INFINITY), DEFAULT_BLUR);
    }

    #[test]
    fn default_blur_matches_swaylock_effects_7x5() {
        // 半径 7 の箱型ぼかし 5 回: 分散 5 * (15² - 1) / 12 = 93.33
        assert!(close(sigma_for_blur(7.0), 93.333_f32.sqrt(), 1e-3));
    }

    #[test]
    fn sigma_grows_with_blur() {
        let mut last = 0.0;
        for b in [0.5, 1.0, 3.0, 7.0, 20.0, 50.0] {
            let s = sigma_for_blur(b);
            assert!(s > last);
            last = s;
        }
        assert_eq!(sigma_for_blur(f32::NAN), sigma_for_blur(7.0));
    }

    #[test]
    fn downscale_is_between_1_and_4() {
        assert_eq!(downscale_factor(sigma_for_blur(7.0)), 4);
        assert_eq!(downscale_factor(sigma_for_blur(0.5)), 1);
        assert_eq!(downscale_factor(sigma_for_blur(50.0)), 4);
        assert_eq!(downscale_factor(5.0), 2);
    }

    #[test]
    fn scaled_size_rounds_up() {
        assert_eq!(scaled_size(3440, 1440, 4), (860, 360));
        assert_eq!(scaled_size(1366, 768, 4), (342, 192));
        assert_eq!(scaled_size(1, 1, 4), (1, 1));
        assert_eq!(scaled_size(10, 10, 0), (10, 10));
    }

    #[test]
    fn kernel_is_normalized_and_decreasing() {
        for sigma in [0.3, 1.0, 2.4, 16.3] {
            let k = kernel(sigma);
            let sum = k[0] + 2.0 * k[1..].iter().sum::<f32>();
            assert!(close(sum, 1.0, 1e-4), "sigma {sigma}: sum {sum}");
            assert!(k.windows(2).all(|w| w[0] >= w[1]));
            assert!(k.len() - 1 <= MAX_RADIUS);
        }
    }

    #[test]
    fn kernel_radius_is_three_sigma() {
        assert_eq!(kernel(2.0).len(), 7);
        assert_eq!(kernel(0.0).len(), 2);
        assert_eq!(kernel(1000.0).len(), MAX_RADIUS + 1);
    }

    #[test]
    fn strongest_blur_fits_the_shader_array() {
        let sigma = sigma_for_blur(50.0);
        let k = kernel(sigma / downscale_factor(sigma) as f32);
        assert!(k.len() - 1 < MAX_RADIUS, "radius {}", k.len() - 1);
    }

    #[test]
    fn xrgb_is_swizzled_and_xbgr_is_not() {
        let x = texture_layout(wl_shm::Format::Xrgb8888).unwrap();
        assert_eq!(
            (x.internal_format, x.format, x.ty),
            (glow::RGBA8, glow::RGBA, glow::UNSIGNED_BYTE)
        );
        assert!(x.swap_rb);
        assert!(texture_layout(wl_shm::Format::Argb8888).unwrap().swap_rb);
        assert!(!texture_layout(wl_shm::Format::Xbgr8888).unwrap().swap_rb);
    }

    #[test]
    fn ten_bit_formats_use_rgb10_a2() {
        let x = texture_layout(wl_shm::Format::Xrgb2101010).unwrap();
        assert_eq!(
            (x.internal_format, x.ty),
            (glow::RGB10_A2, glow::UNSIGNED_INT_2_10_10_10_REV)
        );
        assert!(x.swap_rb);
        assert!(!texture_layout(wl_shm::Format::Xbgr2101010).unwrap().swap_rb);
    }

    #[test]
    fn other_formats_are_rejected() {
        assert_eq!(texture_layout(wl_shm::Format::Rgb565), None);
        assert_eq!(texture_layout(wl_shm::Format::Rgb888), None);
    }

    #[test]
    fn every_captured_format_has_a_layout() {
        use wl_shm::Format as F;
        for f in [
            F::Xrgb8888,
            F::Argb8888,
            F::Xbgr8888,
            F::Abgr8888,
            F::Xrgb2101010,
            F::Argb2101010,
            F::Xbgr2101010,
            F::Abgr2101010,
        ] {
            assert_eq!(
                crate::wl::screencopy::is_supported(f),
                texture_layout(f).is_some(),
                "{f:?}"
            );
        }
    }

    #[test]
    fn fade_runs_from_the_first_configure() {
        assert_eq!(fade_for(None, 500, 1500), 0.0);
        assert_eq!(fade_for(Some(100), 100, 1500), 0.0);
        assert!(close(fade_for(Some(100), 850, 1500), 0.5, 1e-6));
        assert_eq!(fade_for(Some(100), 5000, 1500), 1.0);
        // 時計が configure の時刻より前を指しても黒のまま
        assert_eq!(fade_for(Some(100), 50, 1500), 0.0);
    }

    #[test]
    fn zero_fade_time_shows_the_background_at_once() {
        assert_eq!(fade_for(Some(10), 10, 0), 1.0);
    }

    #[test]
    fn reveal_runs_only_while_unlocking() {
        assert_eq!(reveal_for(Phase::Idle, 5000, &D), 0.0);
        assert_eq!(reveal_for(Phase::Verifying, 5000, &D), 0.0);
        assert_eq!(reveal_for(Phase::Failed { since_ms: 0 }, 5000, &D), 0.0);
        let unlocking = Phase::Unlocking { since_ms: 1000 };
        assert_eq!(reveal_for(unlocking, 1000, &D), 0.0);
        // 初期値では 900ms から 700ms かけて晴れる
        assert!(close(
            reveal_for(unlocking, 1000 + 900 + 350, &D),
            0.5,
            1e-3
        ));
        assert_eq!(
            reveal_for(unlocking, 1000 + crate::state::UNLOCK_TOTAL_MS, &D),
            1.0
        );
        assert_eq!(reveal_for(Phase::Done, 0, &D), 1.0);
    }

    #[test]
    fn reveal_follows_the_unlock_timing() {
        let fast = UnlockTiming {
            open_ms: 400.0,
            reveal_ms: 300.0,
        };
        let unlocking = Phase::Unlocking { since_ms: 1000 };
        assert_eq!(reveal_for(unlocking, 1000 + 450, &fast), 0.0);
        assert!(close(reveal_for(unlocking, 1000 + 600, &fast), 0.5, 1e-3));
        assert_eq!(reveal_for(unlocking, 1000 + fast.total_ms(), &fast), 1.0);
    }

    #[test]
    fn frames_are_needed_while_fading_or_unlocking() {
        assert!(needs_frames(0.5, Phase::Idle));
        assert!(!needs_frames(1.0, Phase::Idle));
        assert!(!needs_frames(1.0, Phase::Verifying));
        assert!(needs_frames(1.0, Phase::Unlocking { since_ms: 0 }));
        assert!(!needs_frames(1.0, Phase::Done));
    }

    #[test]
    fn blur_shader_max_radius_matches_rust() {
        // u_weights の長さ（MAX_RADIUS + 1）がずれると、半径の大きいぼかしで重みが欠ける
        let declaration = format!("const int MAX_RADIUS = {MAX_RADIUS};");
        assert!(BLUR_SHADER.contains(&declaration), "{declaration}");
    }

    /// 単色の画像を渡して、フェード・ビネット・ぼかし解除と赤青の入れ替えを確かめる。
    /// 今のセッションの Wayland 接続で EGL を作る（ロックはしない）。`cargo test -- --ignored` で手動実行。
    #[test]
    #[ignore = "needs a running Wayland compositor and a GPU"]
    fn draws_fade_vignette_and_reveal() {
        let conn = wayland_client::Connection::connect_to_env().expect("wayland");
        let gpu = Gpu::new(&conn).expect("gpu");
        let mut bg = Background::new(&gpu, 7.0).expect("shaders");
        let (w, h) = (64u32, 32u32);
        // Xrgb8888 はメモリ上 B, G, R, X。赤 200・緑 20・青 10 の単色
        let pixels = [10u8, 20, 200, 0].repeat((w * h) as usize);
        bg.upload(
            &gpu,
            &Screenshot {
                output_name: "TEST-1".into(),
                width: w,
                height: h,
                stride: w * 4,
                format: wl_shm::Format::Xrgb8888,
                pixels,
            },
        );
        let gl = gpu.gl();
        let read = |name: &str, fade: f32, reveal: f32| -> Vec<u8> {
            // SAFETY: current な GL コンテキストに対する GL 呼び出し。呼び出し側が引数の正しさを保証する。
            unsafe {
                let target = target_texture(gl, w, h).unwrap();
                let fbo = gl.create_framebuffer().unwrap();
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
                gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    Some(target),
                    0,
                );
                bg.draw(&gpu, name, w, h, fade, reveal);
                let mut out = vec![0u8; (w * h * 4) as usize];
                gl.read_pixels(
                    0,
                    0,
                    w as i32,
                    h as i32,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelPackData::Slice(Some(&mut out)),
                );
                assert_eq!(gl.get_error(), glow::NO_ERROR);
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                gl.delete_framebuffer(fbo);
                gl.delete_texture(target);
                out
            }
        };
        let at = |px: &[u8], x: u32, y: u32| -> [u8; 3] {
            let i = ((y * w + x) * 4) as usize;
            [px[i], px[i + 1], px[i + 2]]
        };
        let near = |a: [u8; 3], b: [u8; 3]| {
            a.iter()
                .zip(b)
                .all(|(x, y)| (*x as i32 - y as i32).abs() <= 3)
        };

        // フェード 0 は黒
        assert_eq!(at(&read("TEST-1", 0.0, 0.0), 32, 16), [0, 0, 0]);
        // フェード 1: 中央はそのままの色（RGB の順に直っている）、角はビネットで約半分
        let full = read("TEST-1", 1.0, 0.0);
        assert!(
            near(at(&full, 32, 16), [200, 20, 10]),
            "{:?}",
            at(&full, 32, 16)
        );
        let corner = at(&full, 0, 0);
        assert!(near(corner, [100, 10, 5]), "{corner:?}");
        // 解除し切ると、フェードに関係なく元画像（ビネットなし）
        let revealed = read("TEST-1", 0.3, 1.0);
        assert!(
            near(at(&revealed, 0, 0), [200, 20, 10]),
            "{:?}",
            at(&revealed, 0, 0)
        );
        // スクショのない出力は黒
        assert_eq!(at(&read("NONE-1", 1.0, 1.0), 32, 16), [0, 0, 0]);
    }
}
