//! kabi-lock: sway 向けの画面ロッカー。
//!
//! Plan 1 では Wayland にも GPU にも依存しない部分だけを持つ。

pub mod anim;
pub mod config;
pub mod lanes;
pub mod password;
pub mod state;
