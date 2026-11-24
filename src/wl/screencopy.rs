//! ロック前のスクリーンショット（wlr-screencopy）。
//!
//! メインの接続とは別の Wayland 接続を別スレッドで開き、全出力を同時に撮る。
//! 返す画像は「論理的な向き・1 行目が画面の上」にそろえてある
//! （出力の transform と、フレームの Y_INVERT をここで戻す）。
#![forbid(unsafe_code)]

use wayland_client::protocol::{wl_output, wl_shm};

/// 32 ビット / 画素で、背景の描画側（`render::background`）が扱える形式か。
pub(crate) fn is_supported(format: wl_shm::Format) -> bool {
    use wl_shm::Format as F;
    matches!(
        format,
        F::Xrgb8888
            | F::Argb8888
            | F::Xbgr8888
            | F::Abgr8888
            | F::Xrgb2101010
            | F::Argb2101010
            | F::Xbgr2101010
            | F::Abgr2101010
    )
}

/// 撮った 1 画素 4 バイトの画像を、論理的な向き（1 行目が画面の上）に直す。
///
/// screencopy の画像は出力の物理的な向きのままなので、transform の逆をかける。
/// 90 は時計回りに 90°、270 は 270° 回して戻す。flipped の 4 つは左右反転してから
/// 回すが、反転を挟むと回す向きが逆になるので flipped-90 は 270°、flipped-270 は 90° 回す
/// （grim の出力と画素単位で一致することを headless の sway で確かめた）。
/// `y_invert` は回転の前に上下を返す。
/// 返り値は（幅, 高さ, stride, 画素）。向きが変わらなければ元の stride を保つ。
pub(crate) fn normalize(
    src: &[u8],
    width: u32,
    height: u32,
    stride: u32,
    transform: wl_output::Transform,
    y_invert: bool,
) -> (u32, u32, u32, Vec<u8>) {
    use wl_output::Transform as T;
    let (w, h, s) = (width as usize, height as usize, stride as usize);
    let row = w * 4;

    if transform == T::Normal {
        if !y_invert {
            return (width, height, stride, src[..s * h].to_vec());
        }
        let mut out = vec![0u8; s * h];
        for y in 0..h {
            let from = (h - 1 - y) * s;
            out[y * s..y * s + row].copy_from_slice(&src[from..from + row]);
        }
        return (width, height, stride, out);
    }

    let flipped = matches!(
        transform,
        T::Flipped | T::Flipped90 | T::Flipped180 | T::Flipped270
    );
    let quarter_turns = match transform {
        T::_90 | T::Flipped270 => 1,
        T::_180 | T::Flipped180 => 2,
        T::_270 | T::Flipped90 => 3,
        _ => 0,
    };
    let (lw, lh) = if quarter_turns % 2 == 1 {
        (h, w)
    } else {
        (w, h)
    };
    let mut out = vec![0u8; lw * lh * 4];
    for by in 0..h {
        for bx in 0..w {
            let fx = if flipped { w - 1 - bx } else { bx };
            let fy = if y_invert { h - 1 - by } else { by };
            // 画面の座標（y は下向き）で時計回りに 90° ずつ回す
            let (lx, ly) = match quarter_turns {
                1 => (h - 1 - fy, fx),
                2 => (w - 1 - fx, h - 1 - fy),
                3 => (fy, w - 1 - fx),
                _ => (fx, fy),
            };
            let from = by * s + bx * 4;
            let to = (ly * lw + lx) * 4;
            out[to..to + 4].copy_from_slice(&src[from..from + 4]);
        }
    }
    (lw as u32, lh as u32, (lw * 4) as u32, out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wl_output::Transform as T;

    /// 幅 3・高さ 2 の画像。画素の 1 バイト目に番号を入れる（a=1 … f=6）。
    ///
    /// ```text
    /// a b c
    /// d e f
    /// ```
    fn image(stride: u32) -> Vec<u8> {
        let mut v = vec![0u8; stride as usize * 2];
        for (i, n) in (1u8..=6).enumerate() {
            let (x, y) = (i % 3, i / 3);
            v[y * stride as usize + x * 4] = n;
        }
        v
    }

    /// 結果の画素番号を行ごとに並べる。
    fn ids(w: u32, h: u32, stride: u32, px: &[u8]) -> Vec<Vec<u8>> {
        (0..h as usize)
            .map(|y| {
                (0..w as usize)
                    .map(|x| px[y * stride as usize + x * 4])
                    .collect()
            })
            .collect()
    }

    fn run(transform: T, y_invert: bool) -> (u32, u32, Vec<Vec<u8>>) {
        let (w, h, s, px) = normalize(&image(12), 3, 2, 12, transform, y_invert);
        (w, h, ids(w, h, s, &px))
    }

    #[test]
    fn normal_keeps_pixels_and_stride() {
        let (w, h, s, px) = normalize(&image(16), 3, 2, 16, T::Normal, false);
        assert_eq!((w, h, s), (3, 2, 16));
        assert_eq!(ids(w, h, s, &px), vec![vec![1, 2, 3], vec![4, 5, 6]]);
    }

    #[test]
    fn y_invert_flips_rows() {
        assert_eq!(
            run(T::Normal, true),
            (3, 2, vec![vec![4, 5, 6], vec![1, 2, 3]])
        );
    }

    #[test]
    fn rotate_90_turns_clockwise() {
        // 上の行 a b c が右の列になる
        assert_eq!(
            run(T::_90, false),
            (2, 3, vec![vec![4, 1], vec![5, 2], vec![6, 3]])
        );
    }

    #[test]
    fn rotate_180() {
        assert_eq!(
            run(T::_180, false),
            (3, 2, vec![vec![6, 5, 4], vec![3, 2, 1]])
        );
    }

    #[test]
    fn rotate_270_turns_counter_clockwise() {
        assert_eq!(
            run(T::_270, false),
            (2, 3, vec![vec![3, 6], vec![2, 5], vec![1, 4]])
        );
    }

    #[test]
    fn flipped_mirrors_horizontally() {
        assert_eq!(
            run(T::Flipped, false),
            (3, 2, vec![vec![3, 2, 1], vec![6, 5, 4]])
        );
    }

    #[test]
    fn flipped_90_is_a_transpose() {
        assert_eq!(
            run(T::Flipped90, false),
            (2, 3, vec![vec![1, 4], vec![2, 5], vec![3, 6]])
        );
    }

    #[test]
    fn flipped_270_is_an_anti_transpose() {
        assert_eq!(
            run(T::Flipped270, false),
            (2, 3, vec![vec![6, 3], vec![5, 2], vec![4, 1]])
        );
    }

    #[test]
    fn y_invert_applies_before_rotation() {
        assert_eq!(
            run(T::_90, true),
            (2, 3, vec![vec![1, 4], vec![2, 5], vec![3, 6]])
        );
    }

    #[test]
    fn supported_formats_are_32_bit() {
        assert!(is_supported(wl_shm::Format::Xrgb8888));
        assert!(is_supported(wl_shm::Format::Argb8888));
        assert!(is_supported(wl_shm::Format::Xbgr2101010));
        assert!(!is_supported(wl_shm::Format::Rgb565));
        assert!(!is_supported(wl_shm::Format::Rgb888));
    }
}
