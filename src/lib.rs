//! kabi-lock: sway 向けの画面ロッカー。
//!
//! `anim` `config` `lanes` `password` `state` `clock` `input` `scene` `fonts` は Wayland にも GPU にも依存しない
//! 純粋なモジュール。`wl` と `render` が Wayland と描画、`auth` が認証、`app` がそれらを結ぶ
//! イベントループを受け持つ。`unsafe` は `wl` と `render` の中だけで使ってよい。

pub mod anim;
pub mod app;
pub mod auth;
pub mod clock;
pub mod config;
pub mod fonts;
pub mod input;
pub mod lanes;
pub mod password;
pub mod render;
pub mod scene;
pub mod state;
pub mod wl;
