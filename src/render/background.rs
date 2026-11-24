//! 背景: スクリーンショットの縮小ぼかし・黒からのフェード・ビネット・解除時のぼかし解除。
//!
//! ぼかしはロック前に 1 回だけ作る（縮小 → 横のガウス → 縦のガウス、FBO で 3 パス）。
//! 毎フレームの描画は、ぼかし済みと元画像の 2 枚を 1 回のパスで合成するだけ。

use wayland_client::protocol::wl_shm;

use crate::state::Phase;

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

/// ぼかし → くっきりの進み具合。解除中だけ進み、ロックを外した後（`Done`）は 1.0。
pub fn reveal_for(phase: Phase, now_ms: u64) -> f32 {
    match phase {
        Phase::Unlocking { .. } => {
            crate::anim::reveal_progress(phase.elapsed_ms(now_ms).unwrap_or(0.0))
        }
        Phase::Done => 1.0,
        _ => 0.0,
    }
}

/// 背景がまだ動いている（フレームコールバックで描き直す必要がある）か。
pub fn needs_frames(fade: f32, phase: Phase) -> bool {
    fade < 1.0 || matches!(phase, Phase::Unlocking { .. })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(reveal_for(Phase::Idle, 5000), 0.0);
        assert_eq!(reveal_for(Phase::Verifying, 5000), 0.0);
        assert_eq!(reveal_for(Phase::Failed { since_ms: 0 }, 5000), 0.0);
        let unlocking = Phase::Unlocking { since_ms: 1000 };
        assert_eq!(reveal_for(unlocking, 1000), 0.0);
        let mid = 1000 + (crate::anim::REVEAL_START_MS + crate::anim::REVEAL_MS / 2.0) as u64;
        assert!(close(reveal_for(unlocking, mid), 0.5, 1e-3));
        assert_eq!(
            reveal_for(unlocking, 1000 + crate::state::UNLOCK_TOTAL_MS),
            1.0
        );
        assert_eq!(reveal_for(Phase::Done, 0), 1.0);
    }

    #[test]
    fn frames_are_needed_while_fading_or_unlocking() {
        assert!(needs_frames(0.5, Phase::Idle));
        assert!(!needs_frames(1.0, Phase::Idle));
        assert!(!needs_frames(1.0, Phase::Verifying));
        assert!(needs_frames(1.0, Phase::Unlocking { since_ms: 0 }));
        assert!(!needs_frames(1.0, Phase::Done));
    }
}
