//! ロック中の状態遷移。

/// 失敗してから錠が消えて入力待ちに戻るまで。
pub const FAIL_HOLD_MS: u64 = 1200;
/// 解除開始からロックを外すまで（錠が開く → ぼかしが晴れる）。
pub const UNLOCK_TOTAL_MS: u64 = 1600;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// 入力待ち。錠は出さない。
    Idle,
    /// パスワードを確認中。錠を出し、キー入力は受け付けない。
    Verifying,
    /// 認証に失敗。錠が震え、`FAIL_HOLD_MS` 後に入力待ちへ戻る。キー入力は受け付ける。
    Failed { since_ms: u64 },
    /// 解除のアニメーション中。
    Unlocking { since_ms: u64 },
    /// ロックを外した。以後のイベントは無視する。
    Done,
}

impl Phase {
    /// この状態に入ってからの経過ミリ秒。開始時刻を持つ `Failed` と `Unlocking` だけ `Some`。
    ///
    /// `anim` の関数に渡すため `f32` で返す。`now_ms` が開始より前でも 0 にとどめる。
    pub fn elapsed_ms(&self, now_ms: u64) -> Option<f32> {
        match *self {
            Phase::Failed { since_ms } | Phase::Unlocking { since_ms } => {
                Some(now_ms.saturating_sub(since_ms) as f32)
            }
            Phase::Idle | Phase::Verifying | Phase::Done => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// Enter が押された。`empty` はパスワードバッファが空かどうか。
    Submit { empty: bool },
    /// パスワード認証の結果が出た。
    PasswordChecked { ok: bool },
    /// YubiKey のタッチで認証できた。
    U2fSucceeded,
    /// 時間が進んだ。
    Tick,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// パスワードバッファの中身を認証スレッドへ送る。
    CheckPassword,
    /// ロックを外して終了処理へ進む。
    ReleaseLock,
}

pub struct Machine {
    phase: Phase,
}

impl Machine {
    pub fn new() -> Self {
        Self { phase: Phase::Idle }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// キー入力をパスワードバッファに入れてよいか。
    pub fn accepts_input(&self) -> bool {
        matches!(self.phase, Phase::Idle | Phase::Failed { .. })
    }

    /// 錠を表示するか。
    pub fn lock_visible(&self) -> bool {
        matches!(
            self.phase,
            Phase::Verifying | Phase::Failed { .. } | Phase::Unlocking { .. }
        )
    }

    pub fn handle(&mut self, event: Event, now_ms: u64) -> Option<Action> {
        match (self.phase, event) {
            (Phase::Done, _) => None,
            (Phase::Unlocking { since_ms }, Event::Tick)
                if now_ms.saturating_sub(since_ms) >= UNLOCK_TOTAL_MS =>
            {
                self.phase = Phase::Done;
                Some(Action::ReleaseLock)
            }
            (Phase::Unlocking { .. }, _) => None,
            (_, Event::U2fSucceeded) => {
                self.phase = Phase::Unlocking { since_ms: now_ms };
                None
            }
            (Phase::Idle | Phase::Failed { .. }, Event::Submit { empty: false }) => {
                self.phase = Phase::Verifying;
                Some(Action::CheckPassword)
            }
            (Phase::Verifying, Event::PasswordChecked { ok: true }) => {
                self.phase = Phase::Unlocking { since_ms: now_ms };
                None
            }
            (Phase::Verifying, Event::PasswordChecked { ok: false }) => {
                self.phase = Phase::Failed { since_ms: now_ms };
                None
            }
            (Phase::Failed { since_ms }, Event::Tick)
                if now_ms.saturating_sub(since_ms) >= FAIL_HOLD_MS =>
            {
                self.phase = Phase::Idle;
                None
            }
            _ => None,
        }
    }
}

impl Default for Machine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn submit(m: &mut Machine, now: u64) -> Option<Action> {
        m.handle(Event::Submit { empty: false }, now)
    }

    #[test]
    fn starts_idle_without_lock() {
        let m = Machine::new();
        assert_eq!(m.phase(), Phase::Idle);
        assert!(m.accepts_input());
        assert!(!m.lock_visible());
    }

    #[test]
    fn empty_submit_does_nothing() {
        let mut m = Machine::new();
        assert_eq!(m.handle(Event::Submit { empty: true }, 0), None);
        assert_eq!(m.phase(), Phase::Idle);
    }

    #[test]
    fn submit_starts_verification() {
        let mut m = Machine::new();
        assert_eq!(submit(&mut m, 10), Some(Action::CheckPassword));
        assert_eq!(m.phase(), Phase::Verifying);
        assert!(!m.accepts_input());
        assert!(m.lock_visible());
    }

    #[test]
    fn submit_while_verifying_is_ignored() {
        let mut m = Machine::new();
        submit(&mut m, 10);
        assert_eq!(submit(&mut m, 20), None);
        assert_eq!(m.phase(), Phase::Verifying);
    }

    #[test]
    fn wrong_password_fails_then_returns_to_idle() {
        let mut m = Machine::new();
        submit(&mut m, 10);
        m.handle(Event::PasswordChecked { ok: false }, 100);
        assert_eq!(m.phase(), Phase::Failed { since_ms: 100 });
        assert!(m.accepts_input());
        assert!(m.lock_visible());

        m.handle(Event::Tick, 100 + FAIL_HOLD_MS - 1);
        assert_eq!(m.phase(), Phase::Failed { since_ms: 100 });
        m.handle(Event::Tick, 100 + FAIL_HOLD_MS);
        assert_eq!(m.phase(), Phase::Idle);
    }

    #[test]
    fn can_submit_again_while_failed() {
        let mut m = Machine::new();
        submit(&mut m, 10);
        m.handle(Event::PasswordChecked { ok: false }, 100);
        assert_eq!(submit(&mut m, 200), Some(Action::CheckPassword));
        assert_eq!(m.phase(), Phase::Verifying);
    }

    #[test]
    fn right_password_unlocks_then_releases() {
        let mut m = Machine::new();
        submit(&mut m, 10);
        m.handle(Event::PasswordChecked { ok: true }, 100);
        assert_eq!(m.phase(), Phase::Unlocking { since_ms: 100 });
        assert!(!m.accepts_input());
        assert!(m.lock_visible());

        assert_eq!(m.handle(Event::Tick, 100 + UNLOCK_TOTAL_MS - 1), None);
        assert_eq!(
            m.handle(Event::Tick, 100 + UNLOCK_TOTAL_MS),
            Some(Action::ReleaseLock)
        );
        assert_eq!(m.phase(), Phase::Done);
        assert_eq!(m.handle(Event::Tick, 99999), None);
    }

    #[test]
    fn u2f_unlocks_from_idle_failed_and_verifying() {
        for setup in [0, 1, 2] {
            let mut m = Machine::new();
            if setup >= 1 {
                submit(&mut m, 10);
            }
            if setup == 2 {
                m.handle(Event::PasswordChecked { ok: false }, 20);
            }
            m.handle(Event::U2fSucceeded, 50);
            assert_eq!(m.phase(), Phase::Unlocking { since_ms: 50 });
        }
    }

    #[test]
    fn late_password_result_is_ignored_while_unlocking() {
        let mut m = Machine::new();
        submit(&mut m, 10);
        m.handle(Event::U2fSucceeded, 50);
        m.handle(Event::PasswordChecked { ok: false }, 60);
        assert_eq!(m.phase(), Phase::Unlocking { since_ms: 50 });
    }

    #[test]
    fn second_u2f_success_does_not_restart_unlocking() {
        let mut m = Machine::new();
        m.handle(Event::U2fSucceeded, 50);
        m.handle(Event::U2fSucceeded, 400);
        assert_eq!(m.phase(), Phase::Unlocking { since_ms: 50 });
    }

    #[test]
    fn elapsed_is_none_without_a_start_time() {
        assert_eq!(Phase::Idle.elapsed_ms(500), None);
        assert_eq!(Phase::Verifying.elapsed_ms(500), None);
        assert_eq!(Phase::Done.elapsed_ms(500), None);
    }

    #[test]
    fn elapsed_counts_from_entering_the_phase() {
        assert_eq!(Phase::Failed { since_ms: 100 }.elapsed_ms(350), Some(250.0));
        assert_eq!(
            Phase::Unlocking { since_ms: 1_000 }.elapsed_ms(2_600),
            Some(1_600.0)
        );
    }

    #[test]
    fn elapsed_never_goes_negative() {
        assert_eq!(Phase::Failed { since_ms: 500 }.elapsed_ms(100), Some(0.0));
    }

    #[test]
    fn unlock_length_matches_the_animation() {
        assert_eq!(
            UNLOCK_TOTAL_MS as f32,
            crate::anim::REVEAL_START_MS + crate::anim::REVEAL_MS
        );
    }
}
