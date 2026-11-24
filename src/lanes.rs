//! 光の柱のレーン。

/// 柱どうしのすき間（px）。
pub const GAP: u32 = 16;

/// 帯に並ぶレーンの数。最低 1 本。
///
/// 設定に巨大な値が入っても桁あふれしないよう、足し算は `saturating_add` で行う。
pub fn lane_count(band_width: u32, pillar_width: u32) -> usize {
    ((band_width.saturating_add(GAP) / pillar_width.saturating_add(GAP)) as usize).max(1)
}

/// レーン左端の x。帯の左端を 0 とし、レーン全体を帯の中央に寄せる。
///
/// 帯が柱 1 本より狭いときは負になる（柱が帯からはみ出して中央に置かれる）。
pub fn lane_x(lane: usize, band_width: u32, pillar_width: u32) -> f32 {
    let n = lane_count(band_width, pillar_width) as f32;
    let pitch = pillar_width as f32 + GAP as f32;
    let used = n * pitch - GAP as f32;
    (band_width as f32 - used) / 2.0 + lane as f32 * pitch
}

/// 表示中の光の柱。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pillar {
    pub lane: usize,
    pub started_ms: u64,
}

/// 打鍵ごとに、表示中の柱と重ならないレーンをランダムに選ぶ。
pub struct LaneAllocator {
    count: usize,
    life_ms: u64,
    live: Vec<Pillar>,
    rng: fastrand::Rng,
}

impl LaneAllocator {
    pub fn new(count: usize, life_ms: u64, rng: fastrand::Rng) -> Self {
        Self {
            count: count.max(1),
            life_ms,
            live: Vec::new(),
            rng,
        }
    }

    /// 新しい柱を出すレーンを選んで記録する。
    ///
    /// 空いているレーンがあればその中からランダムに選ぶ。
    /// すべて表示中なら、一番古い柱のレーンを使い回す。
    pub fn spawn(&mut self, now_ms: u64) -> usize {
        self.prune(now_ms);
        let free: Vec<usize> = (0..self.count)
            .filter(|lane| !self.live.iter().any(|p| p.lane == *lane))
            .collect();
        let lane = if free.is_empty() {
            let oldest = self
                .live
                .iter()
                .enumerate()
                .min_by_key(|(_, p)| p.started_ms)
                .map(|(i, _)| i)
                .expect("count >= 1, so a full allocator has a live pillar");
            self.live.remove(oldest).lane
        } else {
            free[self.rng.usize(..free.len())]
        };
        self.live.push(Pillar {
            lane,
            started_ms: now_ms,
        });
        lane
    }

    /// まだ表示中の柱（古い順）。
    pub fn pillars(&mut self, now_ms: u64) -> &[Pillar] {
        self.prune(now_ms);
        &self.live
    }

    pub fn clear(&mut self) {
        self.live.clear();
    }

    fn prune(&mut self, now_ms: u64) {
        let life = self.life_ms;
        self.live
            .retain(|p| now_ms.saturating_sub(p.started_ms) < life);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alloc(count: usize, life_ms: u64) -> LaneAllocator {
        LaneAllocator::new(count, life_ms, fastrand::Rng::with_seed(42))
    }

    #[test]
    fn default_band_has_16_lanes() {
        assert_eq!(lane_count(768, 32), 16);
    }

    #[test]
    fn original_band_had_20_lanes() {
        assert_eq!(lane_count(960, 32), 20);
    }

    #[test]
    fn narrow_band_still_has_one_lane() {
        assert_eq!(lane_count(10, 32), 1);
    }

    #[test]
    fn lanes_are_centered_and_evenly_spaced() {
        // 16 本 × 48px − 16px = 752px を 768px の中央に置くので左右 8px ずつ空く
        assert_eq!(lane_x(0, 768, 32), 8.0);
        assert_eq!(lane_x(1, 768, 32), 56.0);
        assert_eq!(lane_x(15, 768, 32), 728.0);
    }

    #[test]
    fn live_pillars_never_share_a_lane() {
        let mut a = alloc(16, 900);
        let mut used = Vec::new();
        for i in 0..16 {
            used.push(a.spawn(i));
        }
        used.sort();
        used.dedup();
        assert_eq!(used.len(), 16);
    }

    #[test]
    fn when_all_lanes_are_busy_the_oldest_lane_is_reused() {
        let mut a = alloc(4, 900);
        let first = a.spawn(0);
        for t in 1..4 {
            a.spawn(t);
        }
        let reused = a.spawn(4);
        assert_eq!(reused, first);
        // 使い回したレーンは 1 本だけ表示中のまま
        let lanes: Vec<usize> = a.pillars(4).iter().map(|p| p.lane).collect();
        assert_eq!(lanes.iter().filter(|l| **l == first).count(), 1);
        assert_eq!(lanes.len(), 4);
    }

    #[test]
    fn expired_pillars_free_their_lane() {
        let mut a = alloc(1, 900);
        assert_eq!(a.spawn(0), 0);
        assert_eq!(a.pillars(899).len(), 1);
        assert_eq!(a.pillars(900).len(), 0);
        assert_eq!(a.spawn(900), 0);
    }

    #[test]
    fn clear_removes_everything() {
        let mut a = alloc(4, 900);
        a.spawn(0);
        a.spawn(1);
        a.clear();
        assert!(a.pillars(2).is_empty());
    }

    #[test]
    fn huge_widths_do_not_overflow() {
        assert_eq!(lane_count(u32::MAX, 32), (u32::MAX / 48) as usize);
        assert_eq!(lane_count(768, u32::MAX), 1);
        assert_eq!(lane_count(u32::MAX, u32::MAX), 1);
        assert!(lane_x(0, u32::MAX, 32).is_finite());
        assert!(lane_x(0, 768, u32::MAX).is_finite());
    }

    #[test]
    fn zero_count_is_treated_as_one() {
        let mut a = alloc(0, 900);
        assert_eq!(a.spawn(0), 0);
    }
}
