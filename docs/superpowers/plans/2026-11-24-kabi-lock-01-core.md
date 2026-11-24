# kabi-lock Plan 1: コアロジック 実装計画

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wayland にも GPU にも依存しない kabi-lock の中核（設定・アニメーション計算・レーン割り当て・状態遷移・パスワードバッファ）を、テスト付きのライブラリとして作る。

**Architecture:** 1 つの Cargo パッケージに `lib`（`src/lib.rs`）と `bin`（`src/main.rs`）を置く。Plan 1 では `lib` の純粋なモジュールだけを実装し、`cargo test` で検証する。`bin` は「まだロックできない」と表示して終了するだけ。Wayland・EGL・認証は Plan 2 以降。

**Tech Stack:** Rust（edition 2024、rustc 1.100）、serde + toml + serde_ignored、zeroize、fastrand、log + env_logger

**Spec:** `docs/superpowers/specs/2026-11-24-kabi-lock-design.md`

## 全体のロードマップ（Plan 1〜6）

設計書は複数のサブシステムにまたがるため、計画を 6 つに分ける。各計画はそれだけで動作確認できる成果物を出す。
Plan 2 以降は、前の計画が終わってから、その時点のクレートの API を確認して書く。

| Plan | 内容 | 成果物 |
|---|---|---|
| **1（この文書）** | 設定・アニメーション計算・レーン・状態遷移・パスワードバッファ | `cargo test` が通るライブラリ |
| 2 | Wayland の骨組み: ext-session-lock、`wl_shm` の黒画面、キー入力、PAM パスワード認証、出力の抜き挿し | 黒い画面でロックし、パスワードで解除できる最小のロッカー |
| 3 | EGL・wlr-screencopy・ぼかし・黒からのフェード・解除時のぼかし解除、EGL 失敗時の `wl_shm` フォールバック | 背景が設計どおりに動くロッカー |
| 4 | egui の重ね描き: 時計・日付・光の柱・錠のアニメーション、主画面の選択（sway IPC） | 見た目が設計どおりのロッカー |
| 5 | YubiKey（pam_u2f）スレッド、KeePassXC 連携（zbus）、通知 | 認証まわりが設計どおりのロッカー |
| 6 | 配布: `/etc/pam.d/kabi-lock`、U2F 用 PAM の例、PKGBUILD、README、Inter の同梱 | Arch でインストールできるパッケージ |

## Global Constraints

- Rust edition 2024。`unsafe` は使わない（Plan 1 の範囲）。
- ライセンス: BSD-3-Clause（著作権表記: `Copyright (c) 2026, yuga iwata`）。
- コミットメッセージは Conventional Commits（`feat:` `fix:` `test:` `chore:` `docs:` など）。本文は英語。末尾に `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`。
- 1 コミットは原則 1 ファイル。`git commit` の前に必ず `git status` と `git diff` で変更を確認する。
- コード中のコメントは日本語、ログメッセージは英語。
- パスワードの内容はログやデバッグ出力に出さない（`Debug` を実装しない）。
- 設定の初期値（設計書 7 章）:
  - `appearance.fade_in_ms = 1500`、`appearance.blur = 7`、`appearance.date_format = "%m/%d"`
  - `input.pillar_fade_ms = 900`、`input.pillar_width = 32`、`input.band_width = 768`
  - `auth.pam_service = "kabi-lock"`、`auth.u2f.enabled = true`、`auth.u2f.pam_service = "kabi-lock-u2f"`
  - `keepassxc.lock_before = true`、`keepassxc.unlock_after = true`、`display.primary = []`
- レーンの間隔は 16px。レーン数は `(band_width + 16) / (pillar_width + 16)` の切り捨て（最低 1）。
- アニメーションの時間（設計書 3 章）: 錠の跳ね上がり 260ms、回転は 800ms まで、ぼかし解除は 900ms から 700ms、震え 450ms、失敗後に錠が消えるまで 1200ms。

## ファイル構成

| ファイル | 責務 |
|---|---|
| `Cargo.toml` | パッケージ定義と依存 |
| `src/lib.rs` | モジュールの宣言だけ |
| `src/main.rs` | 起動するとメッセージを出して終了（Plan 2 で置き換え） |
| `src/password.rs` | パスワードバッファ（`zeroize`、容量固定） |
| `src/lanes.rs` | レーン数・レーンの x 座標・レーン割り当て |
| `src/anim.rs` | フェード・光の柱・錠の形・震え・ぼかし解除の計算 |
| `src/state.rs` | ロック中の状態遷移 |
| `src/config.rs` | 設定の型・初期値・読み込み・設定ファイルの場所・主画面の選択 |
| `LICENSE` | BSD-3-Clause |
| `.gitignore` | `/target` |

テストは各モジュールの末尾の `#[cfg(test)] mod tests` に置く（1 タスク 1 ファイルで完結させるため）。

---

### Task 1: プロジェクトの土台

**Files:**
- Create: `Cargo.toml`, `Cargo.lock`, `src/main.rs`, `src/lib.rs`, `src/password.rs`, `src/lanes.rs`, `src/anim.rs`, `src/state.rs`, `src/config.rs`, `LICENSE`, `.gitignore`

**Interfaces:**
- Consumes: なし
- Produces: クレート名 `kabi_lock`（ライブラリ）と同名のバイナリ。モジュール `kabi_lock::{anim, config, lanes, password, state}`（中身は空）。依存クレートが `Cargo.toml` に揃っている。

- [ ] **Step 1: Cargo パッケージを作る**

Run（リポジトリのルート `~/oss/kabi-lock` で）:

```bash
cargo init --name kabi-lock --edition 2024
```

Expected: `Creating binary (application) package` と出て、`Cargo.toml` と `src/main.rs` ができる。`.gitignore` も作られる（中身は `/target`）。

- [ ] **Step 2: 依存クレートを追加する**

Run:

```bash
cargo add serde --features derive
cargo add toml serde_ignored zeroize fastrand log env_logger
```

Expected: `Cargo.toml` の `[dependencies]` に 7 つのクレートが入る。

- [ ] **Step 3: `Cargo.toml` のパッケージ情報を整える**

`Cargo.toml` の `[package]` を次の内容にする（`version` と `edition` は init のまま、ほかを足す）:

```toml
[package]
name = "kabi-lock"
version = "0.1.0"
edition = "2024"
description = "A screen locker for sway (ext-session-lock) with a blurred, fading lock screen"
license = "BSD-3-Clause"
repository = "https://github.com/y1wtt/kabi-lock"
```

- [ ] **Step 4: `src/lib.rs` を書く**

```rust
//! kabi-lock: sway 向けの画面ロッカー。
//!
//! Plan 1 では Wayland にも GPU にも依存しない部分だけを持つ。

pub mod anim;
pub mod config;
pub mod lanes;
pub mod password;
pub mod state;
```

- [ ] **Step 5: 空のモジュールファイルを作る**

それぞれ 1 行だけのファイルを作る。

`src/password.rs`:

```rust
//! パスワードの入力バッファ。
```

`src/lanes.rs`:

```rust
//! 光の柱のレーン。
```

`src/anim.rs`:

```rust
//! 時間から見た目の値を計算する純粋関数。
```

`src/state.rs`:

```rust
//! ロック中の状態遷移。
```

`src/config.rs`:

```rust
//! 設定ファイルの読み込みと、主画面の選択。
```

- [ ] **Step 6: `src/main.rs` を書く**

```rust
fn main() {
    env_logger::init();
    eprintln!(
        "kabi-lock {}: locking is not implemented yet",
        env!("CARGO_PKG_VERSION")
    );
    std::process::exit(1);
}
```

- [ ] **Step 7: ビルドとテストが通ることを確認する**

Run: `cargo build && cargo test`
Expected: エラーなくビルドでき、`test result: ok. 0 passed` が出る。

Run: `cargo run`
Expected: `kabi-lock 0.1.0: locking is not implemented yet` と出て、終了コード 1。

- [ ] **Step 8: `LICENSE` を書く**

```text
BSD 3-Clause License

Copyright (c) 2026, yuga iwata

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

3. Neither the name of the copyright holder nor the names of its
   contributors may be used to endorse or promote products derived from
   this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

- [ ] **Step 9: コミットする（ファイルの種類ごとに分ける）**

```bash
git status
git diff
git add .gitignore
git commit -m "chore: ignore cargo build output

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git add LICENSE
git commit -m "chore: add BSD-3-Clause license

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git add Cargo.toml Cargo.lock src/main.rs src/lib.rs src/password.rs src/lanes.rs src/anim.rs src/state.rs src/config.rs
git commit -m "chore: scaffold kabi-lock crate with empty core modules

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

（最後のコミットは複数ファイルだが、`cargo init` の雛形と空モジュールの作成という 1 つの操作なのでまとめる。）

---

### Task 2: パスワードバッファ

**Files:**
- Modify: `src/password.rs`

**Interfaces:**
- Consumes: `zeroize`
- Produces:
  - `pub const MAX_LEN: usize`（= 256、UTF-8 のバイト数）
  - `pub struct PasswordBuffer`（`Default` 実装あり、`Debug` はなし）
  - `PasswordBuffer::new() -> PasswordBuffer`
  - `PasswordBuffer::push_str(&mut self, s: &str) -> bool`（入れられたら `true`。`MAX_LEN` を超えるなら何もせず `false`）
  - `PasswordBuffer::backspace(&mut self) -> bool`（1 文字消したら `true`）
  - `PasswordBuffer::clear(&mut self)`
  - `PasswordBuffer::is_empty(&self) -> bool`
  - `PasswordBuffer::take(&mut self) -> zeroize::Zeroizing<String>`（中身を取り出し、バッファは空にする）

- [ ] **Step 1: 失敗するテストを書く**

`src/password.rs` を次の内容にする（実装はまだ書かない）:

```rust
//! パスワードの入力バッファ。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_empty() {
        let buf = PasswordBuffer::new();
        assert!(buf.is_empty());
    }

    #[test]
    fn push_and_take() {
        let mut buf = PasswordBuffer::new();
        assert!(buf.push_str("ab"));
        assert!(buf.push_str("c"));
        let taken = buf.take();
        assert_eq!(taken.as_str(), "abc");
        assert!(buf.is_empty());
    }

    #[test]
    fn backspace_removes_one_char_even_if_multibyte() {
        let mut buf = PasswordBuffer::new();
        buf.push_str("aあ");
        assert!(buf.backspace());
        assert_eq!(buf.take().as_str(), "a");
    }

    #[test]
    fn backspace_on_empty_returns_false() {
        let mut buf = PasswordBuffer::new();
        assert!(!buf.backspace());
    }

    #[test]
    fn clear_empties_the_buffer() {
        let mut buf = PasswordBuffer::new();
        buf.push_str("secret");
        buf.clear();
        assert!(buf.is_empty());
        assert_eq!(buf.take().as_str(), "");
    }

    #[test]
    fn rejects_input_beyond_max_len() {
        let mut buf = PasswordBuffer::new();
        let full = "x".repeat(MAX_LEN);
        assert!(buf.push_str(&full));
        assert!(!buf.push_str("y"));
        assert_eq!(buf.take().len(), MAX_LEN);
    }
}
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test password`
Expected: コンパイルエラー（`cannot find type PasswordBuffer` など）。

- [ ] **Step 3: 実装を書く**

`src/password.rs` のテストモジュールの上に追加する:

```rust
use zeroize::{Zeroize, Zeroizing};

/// 受け付けるパスワードの最大長（UTF-8 のバイト数）。
///
/// 最初にこの容量を確保し、超える入力は捨てる。`String` が再確保すると古い領域に
/// パスワードが残ったまま解放されるため、容量を固定して再確保を起こさない。
pub const MAX_LEN: usize = 256;

/// パスワードの入力バッファ。中身はドロップ時と `clear` 時にゼロで上書きされる。
///
/// 内容が漏れないよう `Debug` は実装しない。
pub struct PasswordBuffer {
    text: Zeroizing<String>,
}

impl PasswordBuffer {
    pub fn new() -> Self {
        Self {
            text: Zeroizing::new(String::with_capacity(MAX_LEN)),
        }
    }

    /// 文字列を末尾に足す。足すと `MAX_LEN` を超える場合は何もせず `false` を返す。
    pub fn push_str(&mut self, s: &str) -> bool {
        if self.text.len() + s.len() > MAX_LEN {
            return false;
        }
        self.text.push_str(s);
        true
    }

    /// 最後の 1 文字を消す。消せたら `true`。
    pub fn backspace(&mut self) -> bool {
        self.text.pop().is_some()
    }

    /// 中身をゼロで上書きして空にする（容量はそのまま）。
    pub fn clear(&mut self) {
        self.text.zeroize();
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// 中身を取り出してバッファを空にする。取り出した値もドロップ時にゼロで上書きされる。
    pub fn take(&mut self) -> Zeroizing<String> {
        let mut out = Zeroizing::new(String::with_capacity(MAX_LEN));
        out.push_str(&self.text);
        self.clear();
        out
    }
}

impl Default for PasswordBuffer {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test password`
Expected: `test result: ok. 6 passed`

- [ ] **Step 5: コミットする**

```bash
git status
git diff
git add src/password.rs
git commit -m "feat: add zeroizing password buffer

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: レーン

**Files:**
- Modify: `src/lanes.rs`

**Interfaces:**
- Consumes: `fastrand`
- Produces:
  - `pub const GAP: u32`（= 16）
  - `pub fn lane_count(band_width: u32, pillar_width: u32) -> usize`
  - `pub fn lane_x(lane: usize, band_width: u32, pillar_width: u32) -> f32`（帯の左端を 0 としたレーン左端の x）
  - `pub struct Pillar { pub lane: usize, pub started_ms: u64 }`（`Clone, Copy, Debug, PartialEq, Eq`）
  - `pub struct LaneAllocator`
  - `LaneAllocator::new(count: usize, life_ms: u64, rng: fastrand::Rng) -> LaneAllocator`（`count` が 0 なら 1 として扱う）
  - `LaneAllocator::spawn(&mut self, now_ms: u64) -> usize`（選んだレーン番号）
  - `LaneAllocator::pillars(&mut self, now_ms: u64) -> &[Pillar]`（まだ表示中の柱。古い順）
  - `LaneAllocator::clear(&mut self)`

- [ ] **Step 1: 失敗するテストを書く**

`src/lanes.rs` を次の内容にする:

```rust
//! 光の柱のレーン。

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
    fn zero_count_is_treated_as_one() {
        let mut a = alloc(0, 900);
        assert_eq!(a.spawn(0), 0);
    }
}
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test lanes`
Expected: コンパイルエラー（`cannot find function lane_count` など）。

- [ ] **Step 3: 実装を書く**

`src/lanes.rs` のテストモジュールの上に追加する:

```rust
/// 柱どうしのすき間（px）。
pub const GAP: u32 = 16;

/// 帯に並ぶレーンの数。最低 1 本。
pub fn lane_count(band_width: u32, pillar_width: u32) -> usize {
    (((band_width + GAP) / (pillar_width + GAP)) as usize).max(1)
}

/// レーン左端の x。帯の左端を 0 とし、レーン全体を帯の中央に寄せる。
pub fn lane_x(lane: usize, band_width: u32, pillar_width: u32) -> f32 {
    let n = lane_count(band_width, pillar_width) as f32;
    let pitch = (pillar_width + GAP) as f32;
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
```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test lanes`
Expected: `test result: ok. 9 passed`

- [ ] **Step 5: コミットする**

```bash
git status
git diff
git add src/lanes.rs
git commit -m "feat: add lane layout and non-overlapping lane allocator

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: アニメーション計算

**Files:**
- Modify: `src/anim.rs`

**Interfaces:**
- Consumes: なし
- Produces:
  - `pub fn fade_alpha(elapsed_ms: f32, fade_ms: f32) -> f32`（0.0〜1.0）
  - `pub struct PillarFrame { pub scale_y: f32, pub opacity: f32 }`（`Clone, Copy, Debug, PartialEq`）
  - `pub fn pillar_at(elapsed_ms: f32, life_ms: f32) -> Option<PillarFrame>`（消え終わったら `None`）
  - `pub const SHACKLE_MS: f32`（= 800）、`pub const LONG_LEG_X: f32`（= 42）、`pub const SHORT_LEG_X_CLOSED: f32`（= 22）
  - `pub struct ShackleGeom { pub lift_y: f32, pub short_leg_x: f32 }`（`Clone, Copy, Debug, PartialEq`）
  - `pub fn shackle_at(elapsed_ms: f32) -> ShackleGeom`（解除開始からの経過）
  - `pub struct ShacklePath { pub short_leg_bottom: [f32; 2], pub short_leg_top: [f32; 2], pub arc1: [[f32; 2]; 3], pub arc2: [[f32; 2]; 3], pub long_leg_bottom: [f32; 2] }`（アイコン座標 64x56。`arc1` / `arc2` は 3 次ベジェの「制御点 1・制御点 2・終点」）
  - `pub fn shackle_path(g: ShackleGeom) -> ShacklePath`
  - `pub const SHAKE_MS: f32`（= 450）
  - `pub fn shake_offset(elapsed_ms: f32) -> f32`（px、横方向）
  - `pub const REVEAL_START_MS: f32`（= 900）、`pub const REVEAL_MS: f32`（= 700）
  - `pub fn reveal_progress(elapsed_ms: f32) -> f32`（解除開始からの経過 → 0.0〜1.0）

- [ ] **Step 1: 失敗するテストを書く**

`src/anim.rs` を次の内容にする:

```rust
//! 時間から見た目の値を計算する純粋関数。

#[cfg(test)]
mod tests {
    use super::*;

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
        let g = shackle_at(0.0);
        assert_eq!(g.lift_y, 0.0);
        assert!(close(g.short_leg_x, SHORT_LEG_X_CLOSED));
    }

    #[test]
    fn shackle_overshoots_then_settles() {
        // 0.6 × 260ms で一番高く（-11px）、260ms で -9px に落ち着く
        assert!(close(shackle_at(156.0).lift_y, -11.0));
        assert!(close(shackle_at(260.0).lift_y, -9.0));
        // 跳ね上がりの間は回らない
        assert!(close(shackle_at(260.0).short_leg_x, SHORT_LEG_X_CLOSED));
    }

    #[test]
    fn shackle_swivels_around_the_long_leg() {
        // 回転の真ん中で短い足は長い足と重なる（真横から見た形）
        assert!(close(shackle_at(530.0).short_leg_x, LONG_LEG_X));
        // 回り切ると反対側（42 + 20 = 62）に出る
        let open = shackle_at(SHACKLE_MS);
        assert!(close(open.short_leg_x, 62.0));
        assert!(close(open.lift_y, -9.0));
        assert_eq!(shackle_at(5000.0), open);
    }

    #[test]
    fn closed_path_is_a_symmetric_semicircle() {
        let p = shackle_path(shackle_at(0.0));
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
        let peaks = [0.12, 0.28, 0.44, 0.60, 0.76, 0.88]
            .map(|t| shake_offset(t * SHAKE_MS).abs());
        for w in peaks.windows(2) {
            assert!(w[0] > w[1]);
        }
    }

    #[test]
    fn reveal_runs_after_the_shackle_opens() {
        assert_eq!(reveal_progress(0.0), 0.0);
        assert_eq!(reveal_progress(REVEAL_START_MS), 0.0);
        assert!(close(reveal_progress(REVEAL_START_MS + REVEAL_MS / 2.0), 0.5));
        assert_eq!(reveal_progress(REVEAL_START_MS + REVEAL_MS), 1.0);
        assert!(REVEAL_START_MS >= SHACKLE_MS);
    }
}
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test anim`
Expected: コンパイルエラー（`cannot find function fade_alpha` など）。

- [ ] **Step 3: 実装を書く**

`src/anim.rs` のテストモジュールの上に追加する:

```rust
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

/// 錠の解除アニメーション全体（跳ね上がり + 回転）の長さ。
pub const SHACKLE_MS: f32 = 800.0;
/// 跳ね上がりの長さ。
const LIFT_MS: f32 = 260.0;
/// アイコン座標（64x56）での、長い足（回転軸）の x。
pub const LONG_LEG_X: f32 = 42.0;
/// 閉じているときの短い足の x。
pub const SHORT_LEG_X_CLOSED: f32 = 22.0;

/// U 字の状態。`lift_y` は上方向が負。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShackleGeom {
    pub lift_y: f32,
    pub short_leg_x: f32,
}

/// 解除開始からの経過時間での U 字の状態。
///
/// 0〜260ms で真上に跳ね（-11px まで行って -9px に戻る）、260〜800ms で長い足を軸に
/// 180° 回る。U 字は太さ一定の丸棒として扱い、短い足の x を cos で動かす
/// （左右反転のスケールは線が細くなって紙のように見えるので使わない）。
pub fn shackle_at(elapsed_ms: f32) -> ShackleGeom {
    let lift_y = if elapsed_ms <= 0.0 {
        0.0
    } else {
        let p = (elapsed_ms / LIFT_MS).min(1.0);
        if p < 0.6 {
            -11.0 * (p / 0.6)
        } else {
            -11.0 + 2.0 * ((p - 0.6) / 0.4)
        }
    };
    let q = ((elapsed_ms - LIFT_MS) / (SHACKLE_MS - LIFT_MS)).clamp(0.0, 1.0);
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

/// 解除開始から、ぼかしが晴れ始めるまで。
pub const REVEAL_START_MS: f32 = 900.0;
/// ぼかしが晴れ切るまでの長さ。
pub const REVEAL_MS: f32 = 700.0;

/// 解除開始からの経過時間での、ぼかしの晴れ具合（0.0 = ぼけたまま、1.0 = くっきり）。
/// 時計などの重ね物の不透明度は `1.0 - reveal_progress` にする。
pub fn reveal_progress(elapsed_ms: f32) -> f32 {
    ((elapsed_ms - REVEAL_START_MS) / REVEAL_MS).clamp(0.0, 1.0)
}
```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test anim`
Expected: `test result: ok. 12 passed`

- [ ] **Step 5: コミットする**

```bash
git status
git diff
git add src/anim.rs
git commit -m "feat: add animation curves for fade, pillars, padlock and reveal

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: 状態遷移

**Files:**
- Modify: `src/state.rs`

**Interfaces:**
- Consumes: `crate::anim::{REVEAL_START_MS, REVEAL_MS}`（定数の整合を確かめるテストだけで使う）
- Produces:
  - `pub const FAIL_HOLD_MS: u64`（= 1200）、`pub const UNLOCK_TOTAL_MS: u64`（= 1600）
  - `pub enum Phase { Idle, Verifying, Failed { since_ms: u64 }, Unlocking { since_ms: u64 }, Done }`（`Clone, Copy, Debug, PartialEq, Eq`）
  - `pub enum Event { Submit { empty: bool }, PasswordChecked { ok: bool }, U2fSucceeded, Tick }`（同上）
  - `pub enum Action { CheckPassword, ReleaseLock }`（同上）
  - `pub struct Machine`（`Default` 実装あり）
  - `Machine::new() -> Machine`、`Machine::phase(&self) -> Phase`
  - `Machine::accepts_input(&self) -> bool`（キー入力をバッファに入れてよいか）
  - `Machine::lock_visible(&self) -> bool`（錠を表示するか）
  - `Machine::handle(&mut self, event: Event, now_ms: u64) -> Option<Action>`

呼び出し側（Plan 2）の約束:
- Enter で `Event::Submit { empty }` を渡し、`Some(Action::CheckPassword)` が返ったら `PasswordBuffer::take()` した内容を認証スレッドへ送る。
- 毎フレーム（または一定間隔で）`Event::Tick` を渡し、`Some(Action::ReleaseLock)` が返ったらロックを外して終了処理へ進む。

- [ ] **Step 1: 失敗するテストを書く**

`src/state.rs` を次の内容にする:

```rust
//! ロック中の状態遷移。

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
    fn unlock_length_matches_the_animation() {
        assert_eq!(
            UNLOCK_TOTAL_MS as f32,
            crate::anim::REVEAL_START_MS + crate::anim::REVEAL_MS
        );
    }
}
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test state`
Expected: コンパイルエラー（`cannot find type Machine` など）。

- [ ] **Step 3: 実装を書く**

`src/state.rs` のテストモジュールの上に追加する:

```rust
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
```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test state`
Expected: `test result: ok. 11 passed`

- [ ] **Step 5: コミットする**

```bash
git status
git diff
git add src/state.rs
git commit -m "feat: add lock state machine

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: 設定の読み込み

**Files:**
- Modify: `src/config.rs`

**Interfaces:**
- Consumes: `serde`, `toml`, `serde_ignored`, `log`
- Produces:
  - `pub struct Config { pub display: Display, pub appearance: Appearance, pub input: Input, pub auth: Auth, pub keepassxc: KeePassXc }`
  - `pub struct Display { pub primary: Vec<String> }`
  - `pub struct Appearance { pub fade_in_ms: u32, pub blur: f32, pub date_format: String }`
  - `pub struct Input { pub pillar_fade_ms: u32, pub pillar_width: u32, pub band_width: u32 }`
  - `pub struct Auth { pub pam_service: String, pub u2f: U2f }`、`pub struct U2f { pub enabled: bool, pub pam_service: String }`
  - `pub struct KeePassXc { pub lock_before: bool, pub unlock_after: bool }`
  - すべて `Clone, Debug, PartialEq, Default, Deserialize`（`Default` は Global Constraints の初期値）
  - `pub struct Loaded { pub config: Config, pub unknown_keys: Vec<String> }`
  - `pub struct ConfigError(pub String)`（`Debug`, `Display`, `std::error::Error`）
  - `pub fn parse(text: &str) -> Result<Loaded, ConfigError>`
  - `pub fn config_path(xdg_config_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf>`
  - `pub fn load() -> Config`（失敗しても初期値で返す。警告はログへ）

- [ ] **Step 1: 失敗するテストを書く**

`src/config.rs` を次の内容にする:

```rust
//! 設定ファイルの読み込みと、主画面の選択。

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::path::PathBuf;

    #[test]
    fn empty_file_gives_defaults() {
        let loaded = parse("").unwrap();
        let c = loaded.config;
        assert!(loaded.unknown_keys.is_empty());
        assert!(c.display.primary.is_empty());
        assert_eq!(c.appearance.fade_in_ms, 1500);
        assert_eq!(c.appearance.blur, 7.0);
        assert_eq!(c.appearance.date_format, "%m/%d");
        assert_eq!(c.input.pillar_fade_ms, 900);
        assert_eq!(c.input.pillar_width, 32);
        assert_eq!(c.input.band_width, 768);
        assert_eq!(c.auth.pam_service, "kabi-lock");
        assert!(c.auth.u2f.enabled);
        assert_eq!(c.auth.u2f.pam_service, "kabi-lock-u2f");
        assert!(c.keepassxc.lock_before);
        assert!(c.keepassxc.unlock_after);
        assert_eq!(c, Config::default());
    }

    #[test]
    fn partial_file_overrides_only_given_keys() {
        let text = r#"
            [display]
            primary = ["DP-10", "LG Electronics LG ULTRAWIDE 311NTDV3Z418"]

            [appearance]
            fade_in_ms = 1000

            [auth.u2f]
            pam_service = "u2f-touch"

            [keepassxc]
            unlock_after = false
        "#;
        let c = parse(text).unwrap().config;
        assert_eq!(
            c.display.primary,
            vec!["DP-10", "LG Electronics LG ULTRAWIDE 311NTDV3Z418"]
        );
        assert_eq!(c.appearance.fade_in_ms, 1000);
        assert_eq!(c.appearance.date_format, "%m/%d");
        assert_eq!(c.auth.u2f.pam_service, "u2f-touch");
        assert!(c.auth.u2f.enabled);
        assert!(c.keepassxc.lock_before);
        assert!(!c.keepassxc.unlock_after);
    }

    #[test]
    fn unknown_keys_are_reported_not_rejected() {
        let text = r#"
            colour = "red"
            [appearance]
            fade_in_ms = 1200
            fade_out_ms = 300
        "#;
        let loaded = parse(text).unwrap();
        assert_eq!(loaded.config.appearance.fade_in_ms, 1200);
        assert_eq!(loaded.unknown_keys.len(), 2);
        assert!(loaded.unknown_keys.iter().any(|k| k.contains("colour")));
        assert!(loaded.unknown_keys.iter().any(|k| k.contains("fade_out_ms")));
    }

    #[test]
    fn wrong_type_is_an_error() {
        assert!(parse("[appearance]\nfade_in_ms = \"slow\"").is_err());
    }

    #[test]
    fn broken_toml_is_an_error() {
        assert!(parse("[appearance").is_err());
    }

    #[test]
    fn config_path_prefers_xdg_config_home() {
        assert_eq!(
            config_path(Some(OsString::from("/x")), Some(OsString::from("/home/u"))),
            Some(PathBuf::from("/x/kabi-lock/config.toml"))
        );
    }

    #[test]
    fn config_path_falls_back_to_home() {
        assert_eq!(
            config_path(None, Some(OsString::from("/home/u"))),
            Some(PathBuf::from("/home/u/.config/kabi-lock/config.toml"))
        );
        assert_eq!(
            config_path(Some(OsString::new()), Some(OsString::from("/home/u"))),
            Some(PathBuf::from("/home/u/.config/kabi-lock/config.toml"))
        );
    }

    #[test]
    fn config_path_needs_some_base() {
        assert_eq!(config_path(None, None), None);
    }
}
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test config`
Expected: コンパイルエラー（`cannot find function parse` など）。

- [ ] **Step 3: 実装を書く**

`src/config.rs` のテストモジュールの上に追加する:

```rust
use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;

use serde::Deserialize;

#[derive(Clone, Debug, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub display: Display,
    pub appearance: Appearance,
    pub input: Input,
    pub auth: Auth,
    pub keepassxc: KeePassXc,
}

#[derive(Clone, Debug, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct Display {
    /// 時計などを出す画面の候補（出力名またはモニターの説明文）。上から順に照合する。
    pub primary: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub fade_in_ms: u32,
    pub blur: f32,
    pub date_format: String,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            fade_in_ms: 1500,
            blur: 7.0,
            date_format: "%m/%d".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct Input {
    pub pillar_fade_ms: u32,
    pub pillar_width: u32,
    pub band_width: u32,
}

impl Default for Input {
    fn default() -> Self {
        Self {
            pillar_fade_ms: 900,
            pillar_width: 32,
            band_width: 768,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct Auth {
    pub pam_service: String,
    pub u2f: U2f,
}

impl Default for Auth {
    fn default() -> Self {
        Self {
            pam_service: "kabi-lock".into(),
            u2f: U2f::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct U2f {
    pub enabled: bool,
    pub pam_service: String,
}

impl Default for U2f {
    fn default() -> Self {
        Self {
            enabled: true,
            pam_service: "kabi-lock-u2f".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct KeePassXc {
    pub lock_before: bool,
    pub unlock_after: bool,
}

impl Default for KeePassXc {
    fn default() -> Self {
        Self {
            lock_before: true,
            unlock_after: true,
        }
    }
}

/// 読み込んだ設定と、知らないキーのパス（警告用）。
pub struct Loaded {
    pub config: Config,
    pub unknown_keys: Vec<String>,
}

#[derive(Debug)]
pub struct ConfigError(pub String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid config: {}", self.0)
    }
}

impl std::error::Error for ConfigError {}

/// TOML の文字列から設定を読む。書かれていないキーは初期値、知らないキーは `unknown_keys` に入れる。
pub fn parse(text: &str) -> Result<Loaded, ConfigError> {
    let value: toml::Value = toml::from_str(text).map_err(|e| ConfigError(e.to_string()))?;
    let mut unknown_keys = Vec::new();
    let config = serde_ignored::deserialize(value, |path| unknown_keys.push(path.to_string()))
        .map_err(|e| ConfigError(e.to_string()))?;
    Ok(Loaded {
        config,
        unknown_keys,
    })
}

/// 設定ファイルの場所。`$XDG_CONFIG_HOME/kabi-lock/config.toml`、なければ `~/.config/...`。
pub fn config_path(xdg_config_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let base = match xdg_config_home.filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(home.filter(|v| !v.is_empty())?).join(".config"),
    };
    Some(base.join("kabi-lock").join("config.toml"))
}

/// 設定を読む。ファイルがない・読めない・壊れているときは初期値を返す（ロックは必ず続けるため）。
pub fn load() -> Config {
    let Some(path) = config_path(std::env::var_os("XDG_CONFIG_HOME"), std::env::var_os("HOME"))
    else {
        return Config::default();
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Config::default(),
        Err(e) => {
            log::warn!("cannot read {}: {e}; using defaults", path.display());
            return Config::default();
        }
    };
    match parse(&text) {
        Ok(loaded) => {
            for key in &loaded.unknown_keys {
                log::warn!("{}: unknown key `{key}` ignored", path.display());
            }
            loaded.config
        }
        Err(e) => {
            log::warn!("{}: {e}; using defaults", path.display());
            Config::default()
        }
    }
}
```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test config`
Expected: `test result: ok. 8 passed`

（`toml` 1.2.2 + `serde_ignored` 0.1.14 で、`toml::Value` 経由でも知らないキーが `["appearance.fade_out_ms", "colour"]` のように報告されることを確認済み。）

- [ ] **Step 5: コミットする**

```bash
git status
git diff
git add src/config.rs
git commit -m "feat: add config loading with defaults and unknown-key warnings

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: 主画面の選択

**Files:**
- Modify: `src/config.rs`

**Interfaces:**
- Consumes: Task 6 の `config` モジュール
- Produces:
  - `pub struct OutputInfo { pub name: String, pub description: String }`（`Clone, Debug, PartialEq, Eq`）
  - `pub fn select_primary(rules: &[String], outputs: &[OutputInfo], focused: Option<&str>) -> Option<usize>`
    （`outputs` の添字。出力がなければ `None`）

照合のルール（設計書 7 章）:
1. `rules` を上から順に見て、どれかの出力の「出力名」「説明文」「説明文から末尾の ` (出力名)` を外したもの」のいずれかと完全一致したら、その出力。
2. どのルールにも一致しなければ、`focused`（出力名）と一致する出力。
3. それもなければ最初の出力。

- [ ] **Step 1: 失敗するテストを書く**

`src/config.rs` の `mod tests` の末尾（最後の `}` の手前）に追加する:

```rust
    fn outputs() -> Vec<OutputInfo> {
        vec![
            OutputInfo {
                name: "eDP-1".into(),
                description: "Lenovo Group Limited 0x403A (eDP-1)".into(),
            },
            OutputInfo {
                name: "DP-8".into(),
                description: "LG Electronics LG FULL HD 0x01010101 (DP-8)".into(),
            },
            OutputInfo {
                name: "DP-10".into(),
                description: "LG Electronics LG ULTRAWIDE 311NTDV3Z418 (DP-10)".into(),
            },
        ]
    }

    fn rules(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn selects_by_output_name() {
        assert_eq!(select_primary(&rules(&["DP-10"]), &outputs(), None), Some(2));
    }

    #[test]
    fn selects_by_description_without_name_suffix() {
        let r = rules(&["LG Electronics LG ULTRAWIDE 311NTDV3Z418"]);
        assert_eq!(select_primary(&r, &outputs(), None), Some(2));
    }

    #[test]
    fn selects_by_full_description() {
        let r = rules(&["LG Electronics LG FULL HD 0x01010101 (DP-8)"]);
        assert_eq!(select_primary(&r, &outputs(), None), Some(1));
    }

    #[test]
    fn earlier_rule_wins() {
        let r = rules(&["HDMI-A-1", "DP-8", "DP-10"]);
        assert_eq!(select_primary(&r, &outputs(), None), Some(1));
    }

    #[test]
    fn partial_name_does_not_match() {
        let r = rules(&["DP-1"]);
        assert_eq!(select_primary(&r, &outputs(), Some("DP-8")), Some(1));
    }

    #[test]
    fn falls_back_to_focused_output() {
        assert_eq!(select_primary(&[], &outputs(), Some("DP-10")), Some(2));
    }

    #[test]
    fn falls_back_to_first_output() {
        assert_eq!(select_primary(&[], &outputs(), None), Some(0));
        assert_eq!(select_primary(&[], &outputs(), Some("gone")), Some(0));
    }

    #[test]
    fn no_outputs_means_none() {
        assert_eq!(select_primary(&rules(&["DP-10"]), &[], Some("DP-10")), None);
    }
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test config`
Expected: コンパイルエラー（`cannot find struct OutputInfo` など）。

- [ ] **Step 3: 実装を書く**

`src/config.rs` の `load` 関数の下（テストモジュールの上）に追加する:

```rust
/// 主画面を選ぶための出力の情報。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputInfo {
    /// 出力名（例: `DP-10`）。
    pub name: String,
    /// wl_output の description。sway では `make model serial (出力名)`。
    pub description: String,
}

impl OutputInfo {
    /// description から末尾の ` (出力名)` を外したもの。なければそのまま。
    fn description_without_name(&self) -> &str {
        let suffix = format!(" ({})", self.name);
        self.description
            .strip_suffix(suffix.as_str())
            .unwrap_or(&self.description)
    }

    fn matches(&self, rule: &str) -> bool {
        self.name == rule || self.description == rule || self.description_without_name() == rule
    }
}

/// 時計などを出す出力を選ぶ（`outputs` の添字）。
///
/// `rules` を上から順に照合し、最初に一致した出力。一致しなければ `focused`（出力名）、
/// それもなければ最初の出力。出力がなければ `None`。
pub fn select_primary(rules: &[String], outputs: &[OutputInfo], focused: Option<&str>) -> Option<usize> {
    for rule in rules {
        if let Some(i) = outputs.iter().position(|o| o.matches(rule)) {
            return Some(i);
        }
    }
    if let Some(name) = focused {
        if let Some(i) = outputs.iter().position(|o| o.name == name) {
            return Some(i);
        }
    }
    if outputs.is_empty() { None } else { Some(0) }
}
```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test config`
Expected: `test result: ok. 16 passed`

Run: `cargo test`
Expected: すべてのモジュールのテストが通る（`password` 6、`lanes` 9、`anim` 12、`state` 11、`config` 16、合計 54 件）。

- [ ] **Step 5: lint を通す**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: 警告なし。警告が出たら、その指摘どおりに直してからもう一度実行する（直した内容はこのタスクのコミットに含める）。

Run: `cargo fmt --check`
Expected: 差分なし。差分が出たら `cargo fmt` を実行する。

- [ ] **Step 6: コミットする**

```bash
git status
git diff
git add src/config.rs
git commit -m "feat: select the primary output by name or description

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

`cargo fmt` や clippy の修正で `src/config.rs` 以外のファイルも変わった場合は、ファイルごとに `style: apply rustfmt` / `fix: address clippy warnings` として別にコミットする。

---

## この計画で扱わない設計書の項目

以下は Plan 2〜6 で扱う（ロードマップ参照）。Plan 1 の完了時点で未実装なのは想定どおり。

- 設計書 4〜5 章の Wayland・EGL・egui・起動順（Plan 2〜4）
- 6 章の PAM・U2F・KeePassXC（Plan 2、5）
- 7 章のフォント（Plan 4、6）
- 8 章のエラー時の動き（各 Plan で該当する部分）
- 9 章の手動確認（Plan 2 以降）
- 10 章の配布（Plan 6）
