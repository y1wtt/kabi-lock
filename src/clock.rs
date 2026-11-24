//! ロック開始からの単調な時計。

#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

/// ロック開始からの経過時間を測る時計。プログラム内の時刻はすべてこれで測る。
///
/// Wayland のキーやフレームのタイムスタンプ（`u32` のミリ秒、桁あふれあり、基準が違う）は使わない。
#[derive(Clone, Copy, Debug)]
pub struct Clock {
    start: Instant,
}

impl Clock {
    /// 今を 0ms とする時計を作る。
    pub fn start() -> Clock {
        Clock {
            start: Instant::now(),
        }
    }

    /// 開始からのミリ秒。単調増加する。
    pub fn now_ms(&self) -> u64 {
        millis(self.start.elapsed())
    }
}

/// `Duration` を切り捨てのミリ秒にする。`u64` に収まらなければ `u64::MAX`。
fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn millis_truncates_to_whole_milliseconds() {
        assert_eq!(millis(Duration::from_millis(1500)), 1500);
        assert_eq!(millis(Duration::from_micros(999)), 0);
        assert_eq!(millis(Duration::from_micros(2_999)), 2);
    }

    #[test]
    fn millis_saturates_instead_of_wrapping() {
        assert_eq!(millis(Duration::MAX), u64::MAX);
    }

    #[test]
    fn starts_near_zero() {
        let clock = Clock::start();
        assert!(clock.now_ms() < 1_000);
    }

    #[test]
    fn never_goes_backwards() {
        let clock = Clock::start();
        let a = clock.now_ms();
        std::thread::sleep(Duration::from_millis(5));
        let b = clock.now_ms();
        assert!(b >= a + 5, "a = {a}, b = {b}");
    }
}
