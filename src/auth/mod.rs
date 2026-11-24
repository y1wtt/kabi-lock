//! 認証。結果は `AuthEvent` としてイベントループへ送る。
//!
//! この下のモジュールでは `unsafe` を使わない。

#![forbid(unsafe_code)]

pub mod password;
