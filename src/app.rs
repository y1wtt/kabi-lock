//! Wayland のイベントループ。ロックを掛け、入力と認証の結果で状態を進め、解除する。

use std::fmt;

use wayland_client::protocol::wl_buffer;

/// `run` が返すエラー。どれも終了コード 1 にする。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppError {
    /// Wayland に接続できない、または初期化に失敗した。
    NoWayland(String),
    /// ext-session-lock-v1 がない。
    LockUnsupported,
    /// ロックを拒否された（`finished` が `locked` より先に来た）。
    LockDenied,
    /// そのほか。
    Other(String),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::NoWayland(reason) => {
                write!(f, "cannot connect to the Wayland compositor: {reason}")
            }
            AppError::LockUnsupported => {
                write!(f, "the compositor does not support ext-session-lock-v1")
            }
            AppError::LockDenied => write!(
                f,
                "the compositor refused to lock the session (is another locker running?)"
            ),
            AppError::Other(reason) => write!(f, "{reason}"),
        }
    }
}

impl std::error::Error for AppError {}

/// ロック中の状態。中身は Task 11 で足す。
///
/// `render::shm::ShmBlack` が `QueueHandle<App>` を受け取るので、型だけ先に置く。
pub struct App {}

// 黒いバッファの wl_buffer のイベント（release）は使わない
wayland_client::delegate_noop!(App: ignore wl_buffer::WlBuffer);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_explain_themselves() {
        assert_eq!(
            AppError::NoWayland("WAYLAND_DISPLAY is not set".into()).to_string(),
            "cannot connect to the Wayland compositor: WAYLAND_DISPLAY is not set"
        );
        assert_eq!(
            AppError::LockUnsupported.to_string(),
            "the compositor does not support ext-session-lock-v1"
        );
        assert_eq!(
            AppError::LockDenied.to_string(),
            "the compositor refused to lock the session (is another locker running?)"
        );
        assert_eq!(
            AppError::Other("event loop failed".into()).to_string(),
            "event loop failed"
        );
    }
}
