//! 時間から見た目の値を計算する純粋関数。

#![forbid(unsafe_code)]

use std::f32::consts::PI;

/// 黒からのフェードの不透明度（0.0 = 真っ黒、1.0 = ぼかし画像が完全に見える）。
pub fn fade_alpha(elapsed_ms: f32, fade_ms: f32) -> f32 {
    if fade_ms <= 0.0 {
        return 1.0;
    }
    (elapsed_ms / fade_ms).clamp(0.0, 1.0)
}

/// 光の柱の 1 フレーム分の見た目。`scale_y` は下端を基準にした縦の倍率。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PillarFrame {
    pub scale_y: f32,
    pub opacity: f32,
}

/// 光の柱の見た目。寿命の 12% で一番明るく少し伸び（1.06 倍）、22% で 1.0 倍に戻り、
/// そこから寿命の終わりまでで消える。寿命を過ぎたら `None`。
pub fn pillar_at(elapsed_ms: f32, life_ms: f32) -> Option<PillarFrame> {
    if life_ms <= 0.0 || elapsed_ms < 0.0 || elapsed_ms >= life_ms {
        return None;
    }
    let t = elapsed_ms / life_ms;
    let opacity = if t < 0.12 {
        t / 0.12
    } else {
        1.0 - (t - 0.12) / 0.88
    };
    let scale_y = if t < 0.12 {
        0.35 + (1.06 - 0.35) * (t / 0.12)
    } else if t < 0.22 {
        1.06 - 0.06 * ((t - 0.12) / 0.10)
    } else {
        1.0
    };
    Some(PillarFrame { scale_y, opacity })
}

/// 解除（錠が開く → ぼかしが晴れる）の時間配分。設定の `appearance.unlock_open_ms` と
/// `appearance.unlock_reveal_ms` から作る。
///
/// 跳ね上がりと、開いてからぼかしが晴れ始めるまでの間は `open_ms` に比例させ、
/// 初期値（800 / 700）のときの形を保つ。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UnlockTiming {
    /// 錠が開き切るまで（跳ね上がり + 回転）。
    pub open_ms: f32,
    /// ぼかしが晴れ切るまで（重ね物が消えるのも同じ長さ）。
    pub reveal_ms: f32,
}

impl Default for UnlockTiming {
    fn default() -> Self {
        Self {
            open_ms: 800.0,
            reveal_ms: 700.0,
        }
    }
}

impl UnlockTiming {
    /// 跳ね上がりの長さ。初期値では 260ms（`open_ms` の 260/800）。
    pub fn lift_ms(&self) -> f32 {
        self.open_ms * 260.0 / 800.0
    }

    /// 解除開始から、ぼかしが晴れ始めるまで。初期値では 900ms（`open_ms` の 900/800）。
    pub fn reveal_start_ms(&self) -> f32 {
        self.open_ms * 900.0 / 800.0
    }

    /// 解除開始からロックを外すまで。ぼかしが晴れ切る前に外さないよう切り上げる。
    pub fn total_ms(&self) -> u64 {
        (self.reveal_start_ms() + self.reveal_ms).ceil() as u64
    }
}
/// アイコン座標（64x56）での、長い足（回転軸）の x。
pub const LONG_LEG_X: f32 = 42.0;
/// 閉じているときの短い足の x。
pub const SHORT_LEG_X_CLOSED: f32 = 22.0;
/// 錠のアイコン全体の大きさ（アイコン座標）。設計では 60x52 px の枠に縦横比を保って収める。
pub const ICON_W: f32 = 64.0;
pub const ICON_H: f32 = 56.0;
/// 錠の本体の左上（アイコン座標）。角丸の塗りつぶしで、鍵穴はない。
pub const BODY_X: f32 = 14.0;
pub const BODY_Y: f32 = 24.0;
/// 錠の本体の大きさ（アイコン座標）。y は 24〜52。
pub const BODY_W: f32 = 36.0;
pub const BODY_H: f32 = 28.0;
/// 錠の本体の角丸の半径（アイコン座標）。
pub const BODY_RADIUS: f32 = 6.0;
/// U 字の線の太さ（アイコン座標）。解除中も変えない（丸棒として扱うため）。
pub const SHACKLE_STROKE: f32 = 3.5;

/// U 字の状態。`lift_y` は上方向が負。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShackleGeom {
    pub lift_y: f32,
    pub short_leg_x: f32,
}

impl ShackleGeom {
    /// 閉じた錠の U 字（確認中・失敗中に出す形）。
    pub const CLOSED: ShackleGeom = ShackleGeom {
        lift_y: 0.0,
        short_leg_x: SHORT_LEG_X_CLOSED,
    };
}

/// `start_ms` から `len_ms` かけて 0.0 → 1.0 に進む割合。
///
/// `len_ms` が 0 以下なら 0 で割らず、`start_ms` に達した時点で 1.0 にする（一瞬で終わる）。
fn progress(elapsed_ms: f32, start_ms: f32, len_ms: f32) -> f32 {
    if len_ms <= 0.0 {
        return if elapsed_ms >= start_ms { 1.0 } else { 0.0 };
    }
    ((elapsed_ms - start_ms) / len_ms).clamp(0.0, 1.0)
}

/// 解除開始からの経過時間での U 字の状態。
///
/// 初期値（`open_ms` 800）では 0〜260ms で真上に跳ね（-11px まで行って -9px に戻る）、
/// 260〜800ms で長い足を軸に 180° 回る。`open_ms` を変えると同じ割合で伸び縮みする。
/// U 字は太さ一定の丸棒として扱い、短い足の x を cos で動かす
/// （左右反転のスケールは線が細くなって紙のように見えるので使わない）。
/// 閉じた形は `ShackleGeom::CLOSED`（`open_ms` が 0 だと経過 0ms でもう開いている）。
pub fn shackle_at(elapsed_ms: f32, timing: &UnlockTiming) -> ShackleGeom {
    let lift_ms = timing.lift_ms();
    let p = progress(elapsed_ms, 0.0, lift_ms);
    let lift_y = if p < 0.6 {
        -11.0 * (p / 0.6)
    } else {
        -11.0 + 2.0 * ((p - 0.6) / 0.4)
    };
    let q = progress(elapsed_ms, lift_ms, timing.open_ms - lift_ms);
    let eased = if q < 0.5 {
        2.0 * q * q
    } else {
        1.0 - (-2.0 * q + 2.0).powi(2) / 2.0
    };
    let angle = PI * eased;
    ShackleGeom {
        lift_y,
        short_leg_x: LONG_LEG_X - (LONG_LEG_X - SHORT_LEG_X_CLOSED) * angle.cos(),
    }
}

/// U 字の輪郭（アイコン座標 64x56）。描画側は
/// `short_leg_bottom → short_leg_top`（線分）、`arc1`・`arc2`（3 次ベジェ）、
/// `arc2` の終点 → `long_leg_bottom`（線分）の順につないで太さ一定の線で描く。
/// 長い足の下側は錠の本体（y 24〜52）に隠れる。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShacklePath {
    pub short_leg_bottom: [f32; 2],
    pub short_leg_top: [f32; 2],
    pub arc1: [[f32; 2]; 3],
    pub arc2: [[f32; 2]; 3],
    pub long_leg_bottom: [f32; 2],
}

pub fn shackle_path(g: ShackleGeom) -> ShacklePath {
    // 楕円の 4 分の 1 を 3 次ベジェで近似するときの係数
    const K: f32 = 0.5523;
    const LEG_TOP: f32 = 18.0;
    const APEX: f32 = 8.0;
    const SHORT_LEG_BOTTOM: f32 = 26.0;
    const LONG_LEG_BOTTOM: f32 = 40.0;

    let y = g.lift_y;
    let x1 = g.short_leg_x;
    let x2 = LONG_LEG_X;
    let half = (x2 - x1) / 2.0;
    let cx = (x1 + x2) / 2.0;
    let h = LEG_TOP - APEX;
    ShacklePath {
        short_leg_bottom: [x1, SHORT_LEG_BOTTOM + y],
        short_leg_top: [x1, LEG_TOP + y],
        arc1: [
            [x1, LEG_TOP - h * K + y],
            [cx - half * K, APEX + y],
            [cx, APEX + y],
        ],
        arc2: [
            [cx + half * K, APEX + y],
            [x2, LEG_TOP - h * K + y],
            [x2, LEG_TOP + y],
        ],
        long_leg_bottom: [x2, LONG_LEG_BOTTOM + y],
    }
}

/// 失敗時の震えの長さ。
pub const SHAKE_MS: f32 = 450.0;
/// 震えのキーフレーム（時間の割合, 横のずれ px）。
const SHAKE_KEYS: [(f32, f32); 8] = [
    (0.0, 0.0),
    (0.12, -10.0),
    (0.28, 9.0),
    (0.44, -7.0),
    (0.60, 5.0),
    (0.76, -3.0),
    (0.88, 1.0),
    (1.0, 0.0),
];

/// 失敗してからの経過時間での、錠の横のずれ（px）。
pub fn shake_offset(elapsed_ms: f32) -> f32 {
    if !(0.0..SHAKE_MS).contains(&elapsed_ms) {
        return 0.0;
    }
    let t = elapsed_ms / SHAKE_MS;
    for w in SHAKE_KEYS.windows(2) {
        let (t0, v0) = w[0];
        let (t1, v1) = w[1];
        if t <= t1 {
            return v0 + (v1 - v0) * ((t - t0) / (t1 - t0));
        }
    }
    0.0
}

/// 解除開始からの経過時間での、ぼかしの晴れ具合（0.0 = ぼけたまま、1.0 = くっきり）。
/// `timing.reveal_start_ms()` から `timing.reveal_ms` かけて進む。
/// 時計などの重ね物の不透明度は `1.0 - reveal_progress` にする。
pub fn reveal_progress(elapsed_ms: f32, timing: &UnlockTiming) -> f32 {
    progress(elapsed_ms, timing.reveal_start_ms(), timing.reveal_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    const D: UnlockTiming = UnlockTiming {
        open_ms: 800.0,
        reveal_ms: 700.0,
    };

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn fade_goes_from_black_to_full() {
        assert_eq!(fade_alpha(0.0, 1500.0), 0.0);
        assert!(close(fade_alpha(750.0, 1500.0), 0.5));
        assert_eq!(fade_alpha(1500.0, 1500.0), 1.0);
        assert_eq!(fade_alpha(9999.0, 1500.0), 1.0);
        assert_eq!(fade_alpha(-5.0, 1500.0), 0.0);
    }

    #[test]
    fn zero_fade_time_means_fully_visible() {
        assert_eq!(fade_alpha(0.0, 0.0), 1.0);
    }

    #[test]
    fn pillar_blooms_then_fades() {
        let start = pillar_at(0.0, 900.0).unwrap();
        assert!(close(start.opacity, 0.0));
        assert!(close(start.scale_y, 0.35));

        let peak = pillar_at(0.12 * 900.0, 900.0).unwrap();
        assert!(close(peak.opacity, 1.0));
        assert!(close(peak.scale_y, 1.06));

        let settled = pillar_at(0.22 * 900.0, 900.0).unwrap();
        assert!(close(settled.scale_y, 1.0));

        let late = pillar_at(899.0, 900.0).unwrap();
        assert!(late.opacity < 0.01);
        assert!(close(late.scale_y, 1.0));

        assert_eq!(pillar_at(900.0, 900.0), None);
        assert_eq!(pillar_at(-1.0, 900.0), None);
    }

    #[test]
    fn shackle_is_closed_at_start() {
        let g = shackle_at(0.0, &D);
        assert_eq!(g.lift_y, 0.0);
        assert!(close(g.short_leg_x, SHORT_LEG_X_CLOSED));
    }

    #[test]
    fn shackle_overshoots_then_settles() {
        // 0.6 × 260ms で一番高く（-11px）、260ms で -9px に落ち着く
        assert!(close(shackle_at(156.0, &D).lift_y, -11.0));
        assert!(close(shackle_at(260.0, &D).lift_y, -9.0));
        // 跳ね上がりの間は回らない
        assert!(close(shackle_at(260.0, &D).short_leg_x, SHORT_LEG_X_CLOSED));
    }

    #[test]
    fn shackle_swivels_around_the_long_leg() {
        // 回転の真ん中で短い足は長い足と重なる（真横から見た形）
        assert!(close(shackle_at(530.0, &D).short_leg_x, LONG_LEG_X));
        // 回り切ると反対側（42 + 20 = 62）に出る
        let open = shackle_at(D.open_ms, &D);
        assert!(close(open.short_leg_x, 62.0));
        assert!(close(open.lift_y, -9.0));
        assert_eq!(shackle_at(5000.0, &D), open);
    }

    #[test]
    fn closed_path_is_a_symmetric_semicircle() {
        let p = shackle_path(shackle_at(0.0, &D));
        assert_eq!(p.short_leg_bottom, [22.0, 26.0]);
        assert_eq!(p.short_leg_top, [22.0, 18.0]);
        // 頂点は左右の足の真ん中、高さ 8
        assert_eq!(p.arc1[2], [32.0, 8.0]);
        assert_eq!(p.arc2[2], [42.0, 18.0]);
        assert_eq!(p.long_leg_bottom, [42.0, 40.0]);
    }

    #[test]
    fn path_moves_with_the_lift() {
        let p = shackle_path(ShackleGeom {
            lift_y: -9.0,
            short_leg_x: 22.0,
        });
        assert_eq!(p.short_leg_bottom, [22.0, 17.0]);
        assert_eq!(p.arc1[2], [32.0, -1.0]);
    }

    #[test]
    fn edge_on_shackle_collapses_to_a_vertical_line() {
        let p = shackle_path(ShackleGeom {
            lift_y: 0.0,
            short_leg_x: LONG_LEG_X,
        });
        for point in [p.arc1[0], p.arc1[1], p.arc1[2], p.arc2[0], p.arc2[1]] {
            assert!(close(point[0], LONG_LEG_X));
        }
    }

    #[test]
    fn shake_starts_and_ends_at_rest() {
        assert_eq!(shake_offset(0.0), 0.0);
        assert!(close(shake_offset(0.12 * SHAKE_MS), -10.0));
        assert!(close(shake_offset(0.28 * SHAKE_MS), 9.0));
        assert_eq!(shake_offset(SHAKE_MS), 0.0);
        assert_eq!(shake_offset(-1.0), 0.0);
    }

    #[test]
    fn shake_amplitude_decays() {
        let peaks = [0.12, 0.28, 0.44, 0.60, 0.76, 0.88].map(|t| shake_offset(t * SHAKE_MS).abs());
        for w in peaks.windows(2) {
            assert!(w[0] > w[1]);
        }
    }

    #[test]
    fn reveal_runs_after_the_shackle_opens() {
        assert_eq!(reveal_progress(0.0, &D), 0.0);
        assert_eq!(reveal_progress(900.0, &D), 0.0);
        assert!(close(reveal_progress(900.0 + 350.0, &D), 0.5));
        assert_eq!(reveal_progress(1600.0, &D), 1.0);
        assert!(D.reveal_start_ms() >= D.open_ms);
    }

    #[test]
    fn faster_timing_opens_the_shackle_sooner() {
        let fast = UnlockTiming {
            open_ms: 400.0,
            reveal_ms: 300.0,
        };
        // 跳ね上がりは 130ms（一番高いのは 0.6 × 130 = 78ms）
        assert!(close(shackle_at(78.0, &fast).lift_y, -11.0));
        let lifted = shackle_at(130.0, &fast);
        assert!(close(lifted.lift_y, -9.0));
        assert!(close(lifted.short_leg_x, SHORT_LEG_X_CLOSED));
        // 回転の真ん中（130 + 135ms）で真横
        assert!(close(shackle_at(265.0, &fast).short_leg_x, LONG_LEG_X));
        // open_ms で開き切り、初期値で開き切った形と同じになる
        let open = shackle_at(400.0, &fast);
        assert_eq!(open, shackle_at(D.open_ms, &D));
        assert!(close(open.short_leg_x, 62.0));
        assert_eq!(shackle_at(5000.0, &fast), open);
    }

    #[test]
    fn faster_timing_reveals_sooner() {
        let fast = UnlockTiming {
            open_ms: 400.0,
            reveal_ms: 300.0,
        };
        assert_eq!(reveal_progress(450.0, &fast), 0.0);
        assert!(close(reveal_progress(600.0, &fast), 0.5));
        assert_eq!(reveal_progress(750.0, &fast), 1.0);
    }

    #[test]
    fn zero_open_time_opens_at_once() {
        let instant = UnlockTiming {
            open_ms: 0.0,
            reveal_ms: 700.0,
        };
        let g = shackle_at(0.0, &instant);
        assert!(g.lift_y.is_finite() && g.short_leg_x.is_finite());
        assert_eq!(g, shackle_at(D.open_ms, &D));
        // ぼかしは解除開始からすぐ晴れ始める
        assert!(close(reveal_progress(350.0, &instant), 0.5));
    }

    #[test]
    fn zero_reveal_time_clears_at_the_reveal_start() {
        let instant = UnlockTiming {
            open_ms: 800.0,
            reveal_ms: 0.0,
        };
        assert_eq!(reveal_progress(899.0, &instant), 0.0);
        assert_eq!(reveal_progress(900.0, &instant), 1.0);
        assert_eq!(reveal_progress(5000.0, &instant), 1.0);
        let both = UnlockTiming {
            open_ms: 0.0,
            reveal_ms: 0.0,
        };
        assert_eq!(reveal_progress(0.0, &both), 1.0);
    }

    #[test]
    fn default_unlock_timing_keeps_the_design() {
        let t = UnlockTiming::default();
        assert_eq!(t.open_ms, 800.0);
        assert_eq!(t.reveal_ms, 700.0);
        assert_eq!(t.lift_ms(), 260.0);
        assert_eq!(t.reveal_start_ms(), 900.0);
        assert_eq!(t.total_ms(), 1600);
    }

    #[test]
    fn unlock_timing_scales_with_the_open_time() {
        let t = UnlockTiming {
            open_ms: 400.0,
            reveal_ms: 300.0,
        };
        assert_eq!(t.lift_ms(), 130.0);
        assert_eq!(t.reveal_start_ms(), 450.0);
        assert_eq!(t.total_ms(), 750);
    }

    #[test]
    fn unlock_total_rounds_up_so_the_reveal_can_finish() {
        let t = UnlockTiming {
            open_ms: 333.0,
            reveal_ms: 100.0,
        };
        // 333 × 9/8 = 374.625 → 474.625 を切り上げ
        assert_eq!(t.total_ms(), 475);
    }

    #[test]
    fn zero_unlock_timing_has_zero_total() {
        let t = UnlockTiming {
            open_ms: 0.0,
            reveal_ms: 0.0,
        };
        assert_eq!(t.lift_ms(), 0.0);
        assert_eq!(t.reveal_start_ms(), 0.0);
        assert_eq!(t.total_ms(), 0);
    }

    #[test]
    fn closed_geometry_is_the_resting_shackle() {
        assert_eq!(
            ShackleGeom::CLOSED,
            ShackleGeom {
                lift_y: 0.0,
                short_leg_x: SHORT_LEG_X_CLOSED,
            }
        );
        assert_eq!(shackle_at(0.0, &D), ShackleGeom::CLOSED);
    }

    #[test]
    fn body_matches_the_design() {
        assert_eq!(
            (BODY_X, BODY_Y, BODY_X + BODY_W, BODY_Y + BODY_H),
            (14.0, 24.0, 50.0, 52.0)
        );
        assert_eq!(BODY_RADIUS, 6.0);
        // 本体はアイコンの左右中央にある
        assert_eq!(BODY_X + BODY_W / 2.0, ICON_W / 2.0);
        const { assert!(BODY_Y + BODY_H <= ICON_H) };
    }

    #[test]
    fn closed_shackle_ends_are_hidden_by_the_body() {
        let p = shackle_path(shackle_at(0.0, &D));
        for end in [p.short_leg_bottom, p.long_leg_bottom] {
            assert!(end[0] > BODY_X && end[0] < BODY_X + BODY_W);
            assert!(end[1] > BODY_Y && end[1] < BODY_Y + BODY_H);
        }
    }

    #[test]
    fn long_leg_stays_in_the_body_during_the_whole_unlock() {
        // 一番高く跳ねたとき（-11px）でも、長い足の下端（丸い端を含む）は本体に隠れる
        for ms in (0..=D.open_ms as u32).step_by(10) {
            let p = shackle_path(shackle_at(ms as f32, &D));
            assert!(p.long_leg_bottom[1] - SHACKLE_STROKE / 2.0 > BODY_Y);
        }
    }

    #[test]
    fn shackle_path_stays_finite_and_in_range() {
        for ms in (0..=D.open_ms as u32).step_by(5) {
            let p = shackle_path(shackle_at(ms as f32, &D));
            let points = [
                p.short_leg_bottom,
                p.short_leg_top,
                p.arc1[0],
                p.arc1[1],
                p.arc1[2],
                p.arc2[0],
                p.arc2[1],
                p.arc2[2],
                p.long_leg_bottom,
            ];
            assert!(points.iter().flatten().all(|v| v.is_finite()));
            assert!((SHORT_LEG_X_CLOSED..=62.0).contains(&p.short_leg_top[0]));
        }
    }
}
