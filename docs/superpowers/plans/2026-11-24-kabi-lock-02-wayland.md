# kabi-lock Plan 2: Wayland の骨組み 実装計画

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** ext-session-lock で全出力を `wl_shm` の黒い画面で覆い、キー入力したパスワードを PAM で確かめて解除する、最小の動くロッカーを作る。ロック中の出力の抜き挿しにも追従する。

**Architecture:** Plan 1 の純粋なコア（`state::Machine`、`password::PasswordBuffer` など）はそのまま使い、そのまわりに Wayland の層を足す。`app.rs` の `App` が SCTK の各ハンドラを実装し、calloop のイベントループで「Wayland のイベント」「PAM スレッドからの結果（`calloop::channel`）」「50ms ごとの `Tick`（`calloop::timer`）」の 3 つを受ける。時刻は `clock::Clock` だけを使う。キーは `input::classify` で分類してからパスワードバッファに入れ、Enter で `PasswordChecker`（別スレッドの PAM）に `Zeroizing<String>` のまま渡す。各出力には `render::shm::ShmBlack` で黒いバッファを出す。`main.rs` は引数・設定・終了コードを受け持つ。

**Tech Stack:** Rust（edition 2024、rustc 1.100）、smithay-client-toolkit 0.21.1（calloop 0.14 と calloop-wayland-source 0.4 は SCTK の再輸出を使う）、wayland-client 0.31.15、pam-client 0.5.0（`default-features = false`）、nix 0.32.0（`user` のみ）、zeroize、log + env_logger

**Spec:** `docs/superpowers/specs/2026-11-24-kabi-lock-design.md`
**契約:** `docs/superpowers/plans/2026-11-24-kabi-lock-contract.md`（4 章の「Plan 2 が作るもの」を、この計画で名前・引数どおりに作る）
**Plan 1 の引き継ぎ:** `docs/superpowers/plans/2026-11-24-kabi-lock-01-core-handoff.md`（「早めにやる」と「Plan 2 で必ず守る」をこの計画で扱う）
**試作:** `docs/superpowers/spikes/2026-11-24-api-spike.rs`（SCTK まわりのコードはこれを元にしている）

## Global Constraints

- Rust edition 2024。`unsafe` は `render/` と `wl/` の中だけで使ってよい（Plan 2 ではどこでも使わない）。
  `anim` `config` `lanes` `password` `state` `clock` `input` と `auth/`（下のモジュールを含む）の先頭に `#![forbid(unsafe_code)]` を付ける。
- 契約 4 章の公開 API（`Clock`、`Phase::elapsed_ms`、`KeyAction` と `classify`、`AuthEvent`、`PasswordChecker`、`OutputRender`、`ShmBlack::draw`、`OutputMeta`、`App`、`run`、`AppError`、`LockOutput`、`App::draw_output`）は名前と引数を変えない。足すのはかまわない（この計画で足したものは末尾の「契約への補足」にまとめた）。
- パスワード・入力された文字・その長さは、ログにもデバッグ出力にも出さない。キー入力そのものもログに出さない。
- パスワードは `Zeroizing<String>` のまま扱う。`PasswordBuffer::take()` は `Machine::handle` が `Action::CheckPassword` を返してから呼び、そのまま `PasswordChecker::check` に move する。xkbcommon が返す一時的な `String` も `Zeroizing` に包んで捨てる。
- 時刻は `Clock::now_ms()` だけを使う。Wayland のキーやフレームのタイムスタンプは使わない。`Event::Tick` は calloop の `Timer` から 50ms ごとに送る。
- PAM の確認スレッドは、`check` 1 回につき必ず 1 回 `AuthEvent::PasswordChecked` を送る（PAM エラー・パニック・スレッド停止のときは `ok: false`）。
- 本物のセッションはロックしない。動作確認は入れ子の sway で行う（Task 12）。PAM で実際のパスワードを試すのは 1 回だけにし、間違ったパスワードを繰り返し送らない（faillock でアカウントがロックされるおそれがある）。
- コード中のコメントは日本語、ログメッセージは英語。
- コミットメッセージは Conventional Commits。本文は英語。末尾に、そのコミットを実際に書いたモデルの `Co-Authored-By` 行（例: `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`）。各タスクのコミット例にある `Claude Opus 5.5` は、書いたモデルの名前に置き換える。
- 1 コミットは原則 1 ファイル（依存の追加は `Cargo.toml` と `Cargo.lock` を 1 コミットにする）。`git commit` の前に必ず `git status` と `git diff` で変更を確認する。
- 各タスクの最後に `cargo clippy --all-targets -- -D warnings` と `cargo fmt --check` を通す。`cargo fmt` で別のファイルが変わったら、`style: apply rustfmt` として別にコミットする。

## ファイル構成

| ファイル | 状態 | 責務 |
|---|---|---|
| `Cargo.toml` / `Cargo.lock` | 変更 | smithay-client-toolkit、wayland-client、pam-client、nix を足す |
| `src/lib.rs` | 変更 | モジュール宣言を足し、doc コメントの「Plan 1 では」を直す |
| `src/lanes.rs` | 変更 | 幅の足し算を `saturating_add` にする（引き継ぎ） |
| `src/config.rs` | 変更 | 空の主画面ルールを無視する（引き継ぎ） |
| `src/state.rs` | 変更 | `Phase::elapsed_ms` を足す（引き継ぎ・契約） |
| `src/anim.rs` / `src/password.rs` | 変更 | `#![forbid(unsafe_code)]` を足すだけ |
| `src/clock.rs` | 新規 | 単調時計 `Clock`（純粋） |
| `src/input.rs` | 新規 | `KeyAction` と `classify`（純粋） |
| `src/auth/mod.rs` | 新規 | `AuthEvent`、`state::Event` への変換、ユーザー名の取得 |
| `src/auth/password.rs` | 新規 | `PasswordChecker`（PAM の確認スレッド） |
| `src/wl/mod.rs` | 新規 | モジュール宣言 |
| `src/wl/outputs.rs` | 新規 | `OutputMeta` |
| `src/render/mod.rs` | 新規 | `OutputRender` |
| `src/render/shm.rs` | 新規 | `ShmBlack`（黒い ARGB8888 バッファ） |
| `src/app.rs` | 新規 | `AppError`、`App`、`LockOutput`、`run`、SCTK のハンドラ |
| `src/main.rs` | 置き換え | 引数（`--version` `--help`）、設定の読み込み、`app::run`、終了コード |

テストは Plan 1 と同じく各モジュールの末尾の `#[cfg(test)] mod tests` に置く。Wayland と PAM を使う部分（`ShmBlack::draw`、`App`、`run`、`pam_check`）は単体テストできないので、ビルドと Task 12 の手動確認で確かめる。

## この計画で決めたこと

- **ユーザー名:** `nix` の `User::from_uid(Uid::current())`（`getpwuid_r`）で引き、引けなければ `$USER` を使う。
  `$USER` は `su` や `sudo -E` の後で実際のユーザーと食い違うことがあるので後回しにする。`libc` を直接呼ぶと
  `auth/` で `unsafe` が要るので、安全な API を持つ `nix`（`default-features = false, features = ["user"]`）を選んだ。
  名前が決まらないときはロックせずに終了する（パスワードで解除できないロックを掛けないため）。
- **PAM:** 1 回の確認ごとに `Context::new(service, Some(user), conv)` → `authenticate` → `acct_mgmt`。
  `acct_mgmt` が `NEW_AUTHTOK_REQD`（パスワードの期限切れ）を返したときは、本人確認は済んでいるので解除してよいことにする
  （ここで拒むと二度と解除できない）。会話関数は「エコーなしの入力要求」にだけパスワードを返し、ほかの入力要求は `CONV_ERR` にする。
- **pam-client の制約:** 会話関数が返す `CString` は、pam-client が `strdup` で C 側に写したあとゼロで消さずに解放する
  （`resp_buf.rs` の `ResponseBuffer::put`）。`strdup` した C 側の写しの扱いも libpam とモジュール次第。kabi-lock からは消せないので、
  `CString` を作るときに再確保が起きないよう容量を先に取ることだけを行い、この制約を `auth/password.rs` の doc に書く。
- **パニック時の方針:** 確認関数のパニックは `catch_unwind` で捕まえて `ok: false` を送り、スレッドは次の依頼を待ち続ける。
  加えて、結果を送る前に捨てられたら `ok: false` を送るドロップガード（`Reply`）を置く。スレッドが起動できない・止まっている
  ときは、`check` がその場で `ok: false` を送る。
- **起動順:** 設定 → ユーザー名 → Wayland 接続 → グローバルの bind → PAM スレッド起動 → チャンネルとタイマーの登録 → roundtrip（出力情報）→
  `WaylandSource` の登録 → `lock` → 全出力の lock surface 作成 → イベントループ。失敗しうる準備はすべて `lock` より前に済ませる
  （ロックした後に止まると、sway は赤い画面のまま残る）。`locked` は待たずに lock surface を作る。
- **解除:** `ReleaseLock` を受けたら `unlock()` → `roundtrip()` で終える。`locked` を受ける前に `unlock_and_destroy` を送るのは
  プロトコル違反なので、その場合は `locked` を受けたときに改めて解除する（`release_pending`）。
- **抜き挿し:** ロック後に増えた出力には `OutputHandler::new_output` で lock surface を作り、`output_destroyed` で `LockOutput` ごと捨てる
  （`SessionLockSurface` のドロップで lock surface と wl_surface が、`ShmBlack` のドロップで wl_buffer が破棄される）。
  抜かれた出力を探すため、`LockOutput` に `output: WlOutput` を持たせる。
- **キーリピート:** 使わない（`get_keyboard` で受ける。Backspace を押し続けても 1 文字だけ消える）。
- **ログ:** `RUST_LOG` がなければ `warn` 以上を出す（設定ファイルの警告を見えるようにするため）。致命的なエラーは `RUST_LOG` によらず
  `kabi-lock: <理由>` を標準エラーに出す。
- **`App` と `ShmBlack` の順番:** `ShmBlack::draw` は `QueueHandle<App>` を受け取るので、Task 9 で `App` の型と
  `wl_buffer` の `Dispatch` だけを先に置き、Task 11 で中身を足す（どのコミットでもビルドが通るようにするため）。

---

### Task 1: 引き継ぎの修正（レーンの桁あふれと空の主画面ルール）

**Files:**
- Modify: `src/lanes.rs`, `src/config.rs`

**Interfaces:**
- Consumes: Plan 1 の `lanes::{lane_count, lane_x}`、`config::select_primary`
- Produces: 公開 API は変えない。`lane_count` / `lane_x` は `u32::MAX` を渡しても panic しない。`select_primary` は空（空白だけ）のルールを飛ばす。

- [ ] **Step 1: レーンの失敗するテストを書く**

`src/lanes.rs` のテストモジュールの `zero_count_is_treated_as_one` の直前に追加する:

```rust
    #[test]
    fn huge_widths_do_not_overflow() {
        assert_eq!(lane_count(u32::MAX, 32), (u32::MAX / 48) as usize);
        assert_eq!(lane_count(768, u32::MAX), 1);
        assert_eq!(lane_count(u32::MAX, u32::MAX), 1);
        assert!(lane_x(0, u32::MAX, 32).is_finite());
        assert!(lane_x(0, 768, u32::MAX).is_finite());
    }
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test lanes::`
Expected: `huge_widths_do_not_overflow` が `attempt to add with overflow` で panic し、`test result: FAILED. 9 passed; 1 failed`。

- [ ] **Step 3: 足し算を桁あふれしない形にする**

`src/lanes.rs` の `lane_count` と `lane_x` を次のものに置き換える（`lane_x` の本体の残り 2 行はそのまま）:

```rust
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
```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test lanes::`
Expected: `test result: ok. 10 passed`

- [ ] **Step 5: lanes をコミットする**

```bash
git status
git diff
git add src/lanes.rs
git commit -m "fix: keep lane math from overflowing on huge widths

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 6: 主画面の失敗するテストを書く**

`src/config.rs` のテストモジュールの `no_outputs_means_none` の直前に追加する:

```rust
    #[test]
    fn empty_rules_are_ignored() {
        let mut outs = outputs();
        outs[1].description = String::new();
        // 空のルールが description の空な出力（DP-8）に一致してはいけない
        assert_eq!(select_primary(&rules(&[""]), &outs, None), Some(0));
        assert_eq!(select_primary(&rules(&["  "]), &outs, None), Some(0));
        assert_eq!(select_primary(&rules(&["", "DP-10"]), &outs, None), Some(2));
    }
```

- [ ] **Step 7: テストが失敗することを確認する**

Run: `cargo test config::`
Expected: `empty_rules_are_ignored` が `left: Some(1)` / `right: Some(0)` で失敗し、`test result: FAILED. 16 passed; 1 failed`。

- [ ] **Step 8: 空のルールを飛ばす**

`src/config.rs` の `select_primary` の doc コメントの最後に 1 行足し、`for` の行を書き換える。変更後の関数の頭は次のとおり（`for` の中身から下は変えない）:

```rust
/// 時計などを出す出力を選ぶ（`outputs` の添字）。
///
/// `rules` を上から順に照合し、最初に一致した出力。一致しなければ `focused`（出力名）、
/// それもなければ最初の出力。出力がなければ `None`。
/// 空（空白だけを含む）のルールは無視する（description が空の出力に一致してしまうため）。
pub fn select_primary(
    rules: &[String],
    outputs: &[OutputInfo],
    focused: Option<&str>,
) -> Option<usize> {
    for rule in rules.iter().filter(|r| !r.trim().is_empty()) {
```

- [ ] **Step 9: テストと lint を通す**

Run: `cargo test config::`
Expected: `test result: ok. 17 passed`

Run: `cargo test`
Expected: `test result: ok. 56 passed`（lib）。

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: 警告なし、差分なし。

- [ ] **Step 10: config をコミットする**

```bash
git status
git diff
git add src/config.rs
git commit -m "fix: ignore empty primary output rules

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: 状態に入ってからの経過時間（`Phase::elapsed_ms`）

**Files:**
- Modify: `src/state.rs`

**Interfaces:**
- Consumes: `state::Phase`
- Produces: `impl Phase { pub fn elapsed_ms(&self, now_ms: u64) -> Option<f32> }`（`Failed` / `Unlocking` だけ `Some`、`saturating_sub`）

- [ ] **Step 1: 失敗するテストを書く**

`src/state.rs` のテストモジュールの `unlock_length_matches_the_animation` の直前に追加する:

```rust
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
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test state::`
Expected: コンパイルエラー（`no method named `elapsed_ms` found for enum `state::Phase``）。

- [ ] **Step 3: 実装を書く**

`src/state.rs` の `pub enum Phase { ... }` の閉じ括弧の直後（`pub enum Event` の `#[derive]` の上）に追加する:

```rust
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
```

- [ ] **Step 4: テストと lint を通す**

Run: `cargo test state::`
Expected: `test result: ok. 14 passed`

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: 警告なし、差分なし。

- [ ] **Step 5: コミットする**

```bash
git status
git diff
git add src/state.rs
git commit -m "feat: add Phase::elapsed_ms for animation timing

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: コアの `unsafe` 禁止と、Plan 2 のモジュールの土台

**Files:**
- Modify: `src/anim.rs`, `src/config.rs`, `src/lanes.rs`, `src/password.rs`, `src/state.rs`, `src/lib.rs`
- Create: `src/clock.rs`, `src/input.rs`, `src/app.rs`, `src/auth/mod.rs`, `src/auth/password.rs`, `src/render/mod.rs`, `src/render/shm.rs`, `src/wl/mod.rs`, `src/wl/outputs.rs`

**Interfaces:**
- Consumes: なし
- Produces: コアの 5 モジュールに `#![forbid(unsafe_code)]`。モジュール `kabi_lock::{app, auth, auth::password, clock, input, render, render::shm, wl, wl::outputs}`（中身は doc コメントだけ。`clock` `input` `auth` には `#![forbid(unsafe_code)]`）。

- [ ] **Step 1: コアの 5 ファイルに `#![forbid(unsafe_code)]` を入れる**

5 ファイルとも 1 行目は `//!` の doc コメント 1 行なので、その直後に空行と `#![forbid(unsafe_code)]` を入れる。

Run:

```bash
for f in anim config lanes password state; do
  sed -i '1a\\n#![forbid(unsafe_code)]' "src/$f.rs"
done
head -5 src/lanes.rs
```

Expected: 次の 5 行が出る（ほかの 4 ファイルも 1 行目の doc コメント以外は同じ形）。

```rust
//! 光の柱のレーン。

#![forbid(unsafe_code)]

/// 柱どうしのすき間（px）。
```

Run: `grep -c 'forbid(unsafe_code)' src/anim.rs src/config.rs src/lanes.rs src/password.rs src/state.rs`
Expected: 5 ファイルとも `:1`。

Run: `cargo test && cargo fmt --check`
Expected: `test result: ok. 59 passed`、fmt の差分なし。

- [ ] **Step 2: コアの変更をコミットする**

5 ファイルに同じ操作をしたので 1 コミットにまとめる。

```bash
git status
git diff
git add src/anim.rs src/config.rs src/lanes.rs src/password.rs src/state.rs
git commit -m "chore: forbid unsafe code in the core modules

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 3: `src/lib.rs` を書き換える**

```rust
//! kabi-lock: sway 向けの画面ロッカー。
//!
//! `anim` `config` `lanes` `password` `state` `clock` `input` は Wayland にも GPU にも依存しない
//! 純粋なモジュール。`wl` と `render` が Wayland と描画、`auth` が認証、`app` がそれらを結ぶ
//! イベントループを受け持つ。`unsafe` は `wl` と `render` の中だけで使ってよい。

pub mod anim;
pub mod app;
pub mod auth;
pub mod clock;
pub mod config;
pub mod input;
pub mod lanes;
pub mod password;
pub mod render;
pub mod state;
pub mod wl;
```

- [ ] **Step 4: 空のモジュールファイルを作る**

Run: `mkdir -p src/auth src/render src/wl`

`src/clock.rs`:

```rust
//! ロック開始からの単調な時計。

#![forbid(unsafe_code)]
```

`src/input.rs`:

```rust
//! キー入力の分類。

#![forbid(unsafe_code)]
```

`src/app.rs`:

```rust
//! Wayland のイベントループ。ロックを掛け、入力と認証の結果で状態を進め、解除する。
```

`src/auth/mod.rs`:

```rust
//! 認証。結果は `AuthEvent` としてイベントループへ送る。
//!
//! この下のモジュールでは `unsafe` を使わない。

#![forbid(unsafe_code)]

pub mod password;
```

`src/auth/password.rs`:

```rust
//! PAM によるパスワードの確認（別スレッド）。
```

`src/render/mod.rs`:

```rust
//! 描画。出力ごとの描画手段を `OutputRender` で持つ。

pub mod shm;
```

`src/render/shm.rs`:

```rust
//! `wl_shm` の黒い画面。EGL が使えないときの予備にもなる。
```

`src/wl/mod.rs`:

```rust
//! Wayland まわりの補助。

pub mod outputs;
```

`src/wl/outputs.rs`:

```rust
//! 出力の情報（名前・説明）。
```

- [ ] **Step 5: ビルドと lint を通す**

Run: `cargo build && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: エラー・警告・差分なし。

- [ ] **Step 6: コミットする**

モジュールの雛形（doc コメントだけの空のファイルと、その宣言）を作るという、全ファイルで同じ 1 つの操作なので、1 ファイル 1 コミットの例外としてまとめる（中身のあるコードはまだない。各ファイルの実装は Task 4 以降でファイルごとにコミットする）。

```bash
git status
git diff
git add src/lib.rs src/clock.rs src/input.rs src/app.rs src/auth/mod.rs src/auth/password.rs src/render/mod.rs src/render/shm.rs src/wl/mod.rs src/wl/outputs.rs
git commit -m "chore: add empty modules for the Wayland locker

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: 単調時計（`clock`）

**Files:**
- Modify: `src/clock.rs`

**Interfaces:**
- Consumes: なし
- Produces: `pub struct Clock`（`Clone, Copy, Debug`）、`Clock::start() -> Clock`、`Clock::now_ms(&self) -> u64`

- [ ] **Step 1: 失敗するテストを書く**

`src/clock.rs` を次の内容にする:

```rust
//! ロック開始からの単調な時計。

#![forbid(unsafe_code)]

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
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test clock::`
Expected: コンパイルエラー（`cannot find function `millis` in this scope`、`Clock` が見つからない）。

- [ ] **Step 3: 実装を書く**

`src/clock.rs` のテストモジュール（`#[cfg(test)]`）より上をすべて次の内容に置き換える:

```rust
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
```

- [ ] **Step 4: テストと lint を通す**

Run: `cargo test clock::`
Expected: `test result: ok. 4 passed`

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: 警告なし、差分なし。

- [ ] **Step 5: コミットする**

```bash
git status
git diff
git add src/clock.rs
git commit -m "feat: add a monotonic lock clock

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: キー入力の分類（`input`）

**Files:**
- Modify: `Cargo.toml`, `Cargo.lock`, `src/input.rs`

**Interfaces:**
- Consumes: `smithay_client_toolkit::seat::keyboard::Keysym`
- Produces:
  - `pub use smithay_client_toolkit::seat::keyboard::Keysym`（`input::Keysym` としても使える）
  - `pub enum KeyAction { Submit, Backspace, Clear, Insert, Ignore }`（`Clone, Copy, Debug, PartialEq, Eq`）
  - `pub fn classify(keysym: Keysym, utf8: Option<&str>, ctrl: bool) -> KeyAction`

- [ ] **Step 1: smithay-client-toolkit を追加する**

Run: `cargo add smithay-client-toolkit@0.21.1`
Expected: `Cargo.toml` の `[dependencies]` に `smithay-client-toolkit = "0.21.1"` が入る（既定の機能 `calloop` と `xkbcommon` を使う）。

Run: `cargo build`
Expected: ビルドできる（`libxkbcommon` は `pkg-config` で見つかる）。

- [ ] **Step 2: 依存の追加をコミットする**

```bash
git status
git diff
git add Cargo.toml Cargo.lock
git commit -m "build: add smithay-client-toolkit

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 3: 失敗するテストを書く**

`src/input.rs` を次の内容にする:

```rust
//! キー入力の分類。

#![forbid(unsafe_code)]

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_submits() {
        assert_eq!(
            classify(Keysym::Return, Some("\r"), false),
            KeyAction::Submit
        );
        assert_eq!(
            classify(Keysym::KP_Enter, Some("\r"), false),
            KeyAction::Submit
        );
        assert_eq!(
            classify(Keysym::Return, Some("\r"), true),
            KeyAction::Submit
        );
    }

    #[test]
    fn backspace_deletes() {
        assert_eq!(
            classify(Keysym::BackSpace, Some("\x08"), false),
            KeyAction::Backspace
        );
    }

    #[test]
    fn escape_and_ctrl_u_clear() {
        assert_eq!(
            classify(Keysym::Escape, Some("\x1b"), false),
            KeyAction::Clear
        );
        assert_eq!(classify(Keysym::u, Some("\x15"), true), KeyAction::Clear);
        assert_eq!(classify(Keysym::U, Some("\x15"), true), KeyAction::Clear);
    }

    #[test]
    fn u_without_ctrl_is_text() {
        assert_eq!(classify(Keysym::u, Some("u"), false), KeyAction::Insert);
    }

    #[test]
    fn tab_is_ignored() {
        assert_eq!(classify(Keysym::Tab, Some("\t"), false), KeyAction::Ignore);
        assert_eq!(
            classify(Keysym::ISO_Left_Tab, Some("\t"), false),
            KeyAction::Ignore
        );
    }

    #[test]
    fn control_characters_are_ignored() {
        // Ctrl+A は "\x01"、Delete は "\x7f" になる
        assert_eq!(classify(Keysym::a, Some("\x01"), true), KeyAction::Ignore);
        assert_eq!(
            classify(Keysym::Delete, Some("\x7f"), false),
            KeyAction::Ignore
        );
    }

    #[test]
    fn keys_without_text_are_ignored() {
        assert_eq!(classify(Keysym::Shift_L, None, false), KeyAction::Ignore);
        assert_eq!(classify(Keysym::a, Some(""), false), KeyAction::Ignore);
    }

    #[test]
    fn printable_text_is_inserted() {
        assert_eq!(classify(Keysym::a, Some("a"), false), KeyAction::Insert);
        assert_eq!(
            classify(Keysym::adiaeresis, Some("ä"), false),
            KeyAction::Insert
        );
        // Ctrl を押していても文字になるキー（数字など）はそのまま入れる
        assert_eq!(classify(Keysym::_1, Some("1"), true), KeyAction::Insert);
    }
}
```

- [ ] **Step 4: テストが失敗することを確認する**

Run: `cargo test input::`
Expected: コンパイルエラー（`classify`、`KeyAction`、`Keysym` が見つからない）。

- [ ] **Step 5: 実装を書く**

`src/input.rs` のテストモジュールより上をすべて次の内容に置き換える:

```rust
//! キー入力の分類。

#![forbid(unsafe_code)]

pub use smithay_client_toolkit::seat::keyboard::Keysym;

/// 1 回のキー押下で行う操作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAction {
    /// パスワードを確認に回す。
    Submit,
    /// 最後の 1 文字を消す。
    Backspace,
    /// 全部消す。
    Clear,
    /// xkbcommon が返した文字列をパスワードに足す。
    Insert,
    /// 何もしない。
    Ignore,
}

/// keysym と、xkbcommon が返した文字列と、Ctrl の押下状態から操作を決める。
///
/// 特別なキーは keysym で先に判定する（Enter や Esc は文字列としても `"\r"` や `"\x1b"` を返すため）。
/// 残りは文字列で判定し、空文字と、制御文字を 1 つでも含む文字列（Ctrl+A の `"\x01"` など）は入れない。
pub fn classify(keysym: Keysym, utf8: Option<&str>, ctrl: bool) -> KeyAction {
    match keysym {
        Keysym::Return | Keysym::KP_Enter => return KeyAction::Submit,
        Keysym::BackSpace => return KeyAction::Backspace,
        Keysym::Escape => return KeyAction::Clear,
        Keysym::u | Keysym::U if ctrl => return KeyAction::Clear,
        Keysym::Tab | Keysym::ISO_Left_Tab => return KeyAction::Ignore,
        _ => {}
    }
    match utf8 {
        Some(text) if !text.is_empty() && !text.chars().any(char::is_control) => KeyAction::Insert,
        _ => KeyAction::Ignore,
    }
}
```

契約は「制御文字だけの文字列 → Ignore」としているが、ここでは制御文字を 1 つでも含めば `Ignore` にする（制御文字だけの文字列も含むので、契約の場合分けは満たす。パスワードに制御文字が混ざらないようにするため）。

- [ ] **Step 6: テストと lint を通す**

Run: `cargo test input::`
Expected: `test result: ok. 8 passed`

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: 警告なし、差分なし。

- [ ] **Step 7: コミットする**

```bash
git status
git diff
git add src/input.rs
git commit -m "feat: classify key presses for the password prompt

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: 認証イベントとユーザー名（`auth/mod.rs`）

**Files:**
- Modify: `Cargo.toml`, `Cargo.lock`, `src/auth/mod.rs`

**Interfaces:**
- Consumes: `state::Event`、`nix::unistd::{User, Uid}`
- Produces:
  - `pub enum AuthEvent { PasswordChecked { ok: bool }, U2fSucceeded }`（`Clone, Copy, Debug, PartialEq, Eq`）
  - `impl From<AuthEvent> for state::Event`
  - `pub fn current_user() -> Option<String>`（passwd の名前、なければ `$USER`）

- [ ] **Step 1: nix を追加する**

Run: `cargo add nix@0.32.0 --no-default-features --features user`
Expected: `Cargo.toml` に `nix = { version = "0.32.0", default-features = false, features = ["user"] }` が入る。

- [ ] **Step 2: 依存の追加をコミットする**

```bash
git status
git diff
git add Cargo.toml Cargo.lock
git commit -m "build: add nix for looking up the user name

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 3: 失敗するテストを書く**

`src/auth/mod.rs` を次の内容にする:

```rust
//! 認証。結果は `AuthEvent` としてイベントループへ送る。
//!
//! この下のモジュールでは `unsafe` を使わない。

#![forbid(unsafe_code)]

pub mod password;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Event;

    #[test]
    fn auth_events_map_to_state_events() {
        assert_eq!(
            Event::from(AuthEvent::PasswordChecked { ok: true }),
            Event::PasswordChecked { ok: true }
        );
        assert_eq!(
            Event::from(AuthEvent::PasswordChecked { ok: false }),
            Event::PasswordChecked { ok: false }
        );
        assert_eq!(Event::from(AuthEvent::U2fSucceeded), Event::U2fSucceeded);
    }

    #[test]
    fn passwd_name_wins_over_environment() {
        assert_eq!(
            pick_user(Some("alice".into()), Some("bob".into())),
            Some("alice".to_string())
        );
    }

    #[test]
    fn environment_is_the_fallback() {
        assert_eq!(pick_user(None, Some("bob".into())), Some("bob".to_string()));
        assert_eq!(
            pick_user(Some(String::new()), Some("bob".into())),
            Some("bob".to_string())
        );
    }

    #[test]
    fn no_name_at_all_is_none() {
        assert_eq!(pick_user(None, None), None);
        assert_eq!(pick_user(None, Some(String::new())), None);
    }
}
```

- [ ] **Step 4: テストが失敗することを確認する**

Run: `cargo test auth::`
Expected: コンパイルエラー（`AuthEvent`、`pick_user` が見つからない）。

- [ ] **Step 5: 実装を書く**

`src/auth/mod.rs` のテストモジュールより上をすべて次の内容に置き換える:

```rust
//! 認証。結果は `AuthEvent` としてイベントループへ送る。
//!
//! この下のモジュールでは `unsafe` を使わない。

#![forbid(unsafe_code)]

pub mod password;

use crate::state::Event;

/// 認証スレッドからイベントループへ送る結果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthEvent {
    /// パスワードの確認が終わった。`check` 1 回につき必ず 1 回送られる。
    PasswordChecked { ok: bool },
    /// YubiKey のタッチで認証できた（Plan 5）。
    U2fSucceeded,
}

impl From<AuthEvent> for Event {
    fn from(event: AuthEvent) -> Event {
        match event {
            AuthEvent::PasswordChecked { ok } => Event::PasswordChecked { ok },
            AuthEvent::U2fSucceeded => Event::U2fSucceeded,
        }
    }
}

/// PAM に渡すユーザー名。
///
/// 実ユーザー ID から passwd を引いた名前を使い、引けなければ `$USER` を使う。
/// `$USER` は `su` や `sudo -E` の後で実際のユーザーと食い違うことがあるので、後回しにする。
pub fn current_user() -> Option<String> {
    pick_user(passwd_name(), std::env::var("USER").ok())
}

fn passwd_name() -> Option<String> {
    match nix::unistd::User::from_uid(nix::unistd::Uid::current()) {
        Ok(Some(user)) => Some(user.name),
        Ok(None) => {
            log::warn!("no passwd entry for the current uid");
            None
        }
        Err(e) => {
            log::warn!("cannot look up the current user: {e}");
            None
        }
    }
}

/// passwd の名前を優先し、なければ環境変数の名前。空の名前は使わない。
fn pick_user(from_passwd: Option<String>, from_env: Option<String>) -> Option<String> {
    from_passwd
        .filter(|name| !name.is_empty())
        .or_else(|| from_env.filter(|name| !name.is_empty()))
}
```

- [ ] **Step 6: テストと lint を通す**

Run: `cargo test auth::`
Expected: `test result: ok. 4 passed`

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: 警告なし、差分なし。

- [ ] **Step 7: コミットする**

```bash
git status
git diff
git add src/auth/mod.rs
git commit -m "feat: add auth events and current user lookup

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: PAM のパスワード確認スレッド（`auth/password.rs`）

**Files:**
- Modify: `Cargo.toml`, `Cargo.lock`, `src/auth/password.rs`

**Interfaces:**
- Consumes: `auth::AuthEvent`、`smithay_client_toolkit::reexports::calloop::channel::Sender`、`pam_client::{Context, ConversationHandler, ErrorCode, Flag}`、`zeroize::Zeroizing`
- Produces:
  - `pub struct PasswordChecker`（`Debug` なし）
  - `PasswordChecker::spawn(service: String, user: String, events: calloop::channel::Sender<AuthEvent>) -> PasswordChecker`
  - `PasswordChecker::check(&self, password: Zeroizing<String>)`
  - `check` 1 回につき `AuthEvent::PasswordChecked` が必ず 1 回届く

- [ ] **Step 1: pam-client を追加する**

Run: `cargo add pam-client@0.5.0 --no-default-features`
Expected: `Cargo.toml` に `pam-client = { version = "0.5.0", default-features = false }` が入る（既定の `cli` 機能＝端末からのパスワード入力は使わない）。

- [ ] **Step 2: 依存の追加をコミットする**

```bash
git status
git diff
git add Cargo.toml Cargo.lock
git commit -m "build: add pam-client without the cli feature

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 3: 失敗するテストを書く**

PAM そのものは単体テストで呼ばない（本物のアカウントに認証を試すことになるため）。確認関数を差し替えられる
`spawn_with` を通して、「必ず 1 回返す」「パニックは失敗」「スレッドがいなくても失敗を返す」を確かめる。

`src/auth/password.rs` を次の内容にする:

```rust
//! PAM によるパスワードの確認（別スレッド）。

#[cfg(test)]
mod tests {
    use super::*;
    use smithay_client_toolkit::reexports::calloop::channel::{Channel, channel};
    use std::sync::mpsc;

    fn secret(text: &str) -> Zeroizing<String> {
        Zeroizing::new(text.to_string())
    }

    fn checked(ok: bool) -> AuthEvent {
        AuthEvent::PasswordChecked { ok }
    }

    /// 正しいパスワードを "right" とする偽の確認関数で動かす。
    fn fake_checker() -> (PasswordChecker, Channel<AuthEvent>) {
        let (tx, rx) = channel();
        let checker = PasswordChecker::spawn_with(tx, |password: Zeroizing<String>| {
            if password.as_str() == "boom" {
                panic!("simulated PAM crash");
            }
            password.as_str() == "right"
        });
        (checker, rx)
    }

    #[test]
    fn results_are_forwarded() {
        let (checker, rx) = fake_checker();
        checker.check(secret("right"));
        assert_eq!(rx.recv().unwrap(), checked(true));
        checker.check(secret("wrong"));
        assert_eq!(rx.recv().unwrap(), checked(false));
    }

    #[test]
    fn panic_counts_as_failure_and_the_worker_survives() {
        let (checker, rx) = fake_checker();
        checker.check(secret("boom"));
        assert_eq!(rx.recv().unwrap(), checked(false));
        checker.check(secret("right"));
        assert_eq!(rx.recv().unwrap(), checked(true));
    }

    #[test]
    fn each_check_gets_exactly_one_reply() {
        let (checker, rx) = fake_checker();
        for word in ["right", "boom", "wrong"] {
            checker.check(secret(word));
        }
        assert_eq!(rx.recv().unwrap(), checked(true));
        assert_eq!(rx.recv().unwrap(), checked(false));
        assert_eq!(rx.recv().unwrap(), checked(false));
        // 送信側をすべて手放すと、余分な結果がなければ「閉じた」が返る
        drop(checker);
        assert!(rx.recv().is_err());
    }

    #[test]
    fn a_dead_worker_still_replies_failure() {
        let (tx, rx) = channel();
        let (requests, worker_side) = mpsc::channel::<Zeroizing<String>>();
        drop(worker_side);
        let checker = PasswordChecker {
            requests,
            events: tx,
        };
        checker.check(secret("right"));
        assert_eq!(rx.recv().unwrap(), checked(false));
    }

    #[test]
    fn response_keeps_the_bytes() {
        let response = response_cstring("pässword").unwrap();
        assert_eq!(response.as_bytes(), "pässword".as_bytes());
    }

    #[test]
    fn response_rejects_nul() {
        assert!(response_cstring("a\0b").is_none());
    }
}
```

- [ ] **Step 4: テストが失敗することを確認する**

Run: `cargo test auth::password`
Expected: コンパイルエラー（`PasswordChecker`、`AuthEvent`、`Zeroizing`、`response_cstring` が見つからない）。

- [ ] **Step 5: 実装を書く**

`src/auth/password.rs` のテストモジュールより上をすべて次の内容に置き換える:

```rust
//! PAM によるパスワードの確認（別スレッド）。
//!
//! パスワードのメモリについて: こちらで持つ `Zeroizing<String>` は使い終わるとゼロで消える。
//! ただし PAM の会話関数が返す `CString` は pam-client が C 側へ `strdup` した後、
//! ゼロで消さずに解放する。`strdup` された C 側の写しの扱いも PAM（libpam とモジュール）次第。
//! この 2 つは kabi-lock からは消せない（pam-client 0.5.0 の制約）。

use std::ffi::{CStr, CString};
use std::panic::{self, AssertUnwindSafe};
use std::sync::mpsc;
use std::thread;

use pam_client::{ConversationHandler, ErrorCode, Flag};
use smithay_client_toolkit::reexports::calloop::channel::Sender;
use zeroize::Zeroizing;

use super::AuthEvent;

/// PAM の確認スレッドへの窓口。`check` を呼ぶと、結果が `AuthEvent::PasswordChecked` で 1 回届く。
pub struct PasswordChecker {
    requests: mpsc::Sender<Zeroizing<String>>,
    events: Sender<AuthEvent>,
}

impl PasswordChecker {
    /// PAM の確認スレッドを起動する。
    ///
    /// 結果は `events` に `check` 1 回につき必ず 1 回送る。PAM がエラーを返したとき、
    /// 確認中にパニックしたとき、スレッドが動いていないときも `ok: false` を送る
    /// （送らないと状態が確認中のまま止まり、解除できなくなる）。
    pub fn spawn(service: String, user: String, events: Sender<AuthEvent>) -> PasswordChecker {
        Self::spawn_with(events, move |password| pam_check(&service, &user, password))
    }

    /// 確認関数を差し替えて起動する（テスト用に分けてある）。
    fn spawn_with<F>(events: Sender<AuthEvent>, verify: F) -> PasswordChecker
    where
        F: FnMut(Zeroizing<String>) -> bool + Send + 'static,
    {
        let (requests, queue) = mpsc::channel::<Zeroizing<String>>();
        let worker_events = events.clone();
        let spawned = thread::Builder::new()
            .name("kabi-lock-pam".into())
            .spawn(move || serve(queue, worker_events, verify));
        if let Err(e) = spawned {
            // 受け口が閉じるので、以後の check はすぐ失敗を返す
            log::error!("cannot start the PAM thread: {e}");
        }
        PasswordChecker { requests, events }
    }

    /// パスワードを確認スレッドへ渡す。`Zeroizing<String>` のまま move する。
    pub fn check(&self, password: Zeroizing<String>) {
        if let Err(mpsc::SendError(password)) = self.requests.send(password) {
            drop(password);
            log::error!("the PAM thread is not running; treating the attempt as a failure");
            let _ = self.events.send(AuthEvent::PasswordChecked { ok: false });
        }
    }
}

/// 確認スレッドの本体。依頼を 1 つずつ確認し、必ず 1 回結果を送る。
fn serve<F>(queue: mpsc::Receiver<Zeroizing<String>>, events: Sender<AuthEvent>, mut verify: F)
where
    F: FnMut(Zeroizing<String>) -> bool,
{
    while let Ok(password) = queue.recv() {
        let reply = Reply::new(&events);
        // パニックは失敗として扱い、スレッドは次の依頼を待ち続ける。
        // パスワードはクロージャへ move しているので、巻き戻しの途中でもゼロで消える。
        let ok = match panic::catch_unwind(AssertUnwindSafe(|| verify(password))) {
            Ok(ok) => ok,
            Err(_) => {
                log::error!("the PAM check panicked; treating it as a failure");
                false
            }
        };
        reply.send(ok);
    }
}

/// 結果を 1 回だけ送る。送らずに捨てられたら（想定外の巻き戻しなど）失敗を送る。
struct Reply<'a> {
    events: &'a Sender<AuthEvent>,
    sent: bool,
}

impl<'a> Reply<'a> {
    fn new(events: &'a Sender<AuthEvent>) -> Self {
        Reply {
            events,
            sent: false,
        }
    }

    fn send(mut self, ok: bool) {
        self.sent = true;
        // 受け手（イベントループ）がもういなければ、送れなくてもかまわない
        let _ = self.events.send(AuthEvent::PasswordChecked { ok });
    }
}

impl Drop for Reply<'_> {
    fn drop(&mut self) {
        if !self.sent {
            let _ = self.events.send(AuthEvent::PasswordChecked { ok: false });
        }
    }
}

/// PAM で 1 回確認する。`authenticate` と `acct_mgmt` の両方が通れば `true`。
fn pam_check(service: &str, user: &str, password: Zeroizing<String>) -> bool {
    let conversation = PasswordConversation { password };
    let mut context = match pam_client::Context::new(service, Some(user), conversation) {
        Ok(context) => context,
        Err(e) => {
            log::error!("cannot start PAM service `{service}`: {e}");
            return false;
        }
    };
    if let Err(e) = context.authenticate(Flag::NONE) {
        log::warn!("PAM authentication failed: {e}");
        return false;
    }
    match context.acct_mgmt(Flag::NONE) {
        Ok(()) => true,
        // パスワードの期限切れでも本人確認は済んでいる。ここで拒むと二度と解除できない
        Err(e) if e.code() == ErrorCode::NEW_AUTHTOK_REQD => {
            log::warn!("password has expired; unlocking anyway: {e}");
            true
        }
        Err(e) => {
            log::warn!("PAM account check failed: {e}");
            false
        }
    }
}

/// PAM の会話関数。パスワードの入力要求にだけ答える。
struct PasswordConversation {
    password: Zeroizing<String>,
}

impl ConversationHandler for PasswordConversation {
    fn prompt_echo_on(&mut self, _prompt: &CStr) -> Result<CString, ErrorCode> {
        // ユーザー名などの入力要求には答えない（ユーザー名は Context::new で渡している）
        Err(ErrorCode::CONV_ERR)
    }

    fn prompt_echo_off(&mut self, _prompt: &CStr) -> Result<CString, ErrorCode> {
        response_cstring(&self.password).ok_or(ErrorCode::CONV_ERR)
    }

    fn text_info(&mut self, msg: &CStr) {
        log::info!("PAM: {}", msg.to_string_lossy());
    }

    fn error_msg(&mut self, msg: &CStr) {
        log::warn!("PAM: {}", msg.to_string_lossy());
    }
}

/// PAM へ返す応答を作る。NUL を含むパスワードは渡せないので `None`。
///
/// 終端の NUL の分まで先に確保して、`CString::new` の中で再確保（古い領域に写しが残る）を起こさない。
/// 返した `CString` は pam-client がゼロで消さずに解放する（モジュール先頭の説明を参照）。
fn response_cstring(password: &str) -> Option<CString> {
    let bytes = password.as_bytes();
    if bytes.contains(&0) {
        return None;
    }
    let mut buffer = Vec::with_capacity(bytes.len() + 1);
    buffer.extend_from_slice(bytes);
    CString::new(buffer).ok()
}
```

- [ ] **Step 6: テストと lint を通す**

Run: `cargo test auth::password`
Expected: `test result: ok. 6 passed`。`panic_counts_as_failure_and_the_worker_survives` などの実行中に
`thread 'kabi-lock-pam' panicked at ... simulated PAM crash` が標準エラーに出るのは想定どおり（わざとパニックさせている）。

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: 警告なし、差分なし。

- [ ] **Step 7: コミットする**

```bash
git status
git diff
git add src/auth/password.rs
git commit -m "feat: check passwords with PAM on a worker thread

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: 出力の情報（`wl/outputs.rs`）

**Files:**
- Modify: `src/wl/outputs.rs`

**Interfaces:**
- Consumes: `smithay_client_toolkit::output::OutputInfo`、`config::OutputInfo`
- Produces:
  - `pub struct OutputMeta { pub name: String, pub description: String }`（`Clone, Debug, PartialEq, Eq`）
  - `OutputMeta::from_info(info: Option<&smithay_client_toolkit::output::OutputInfo>) -> OutputMeta`
  - `OutputMeta::from_parts(name: Option<String>, description: Option<String>) -> OutputMeta`
  - `OutputMeta::label(&self) -> &str`（ログ用。名前が空なら `(unnamed output)`）
  - `OutputMeta::to_output_info(&self) -> config::OutputInfo`（Plan 4 の `select_primary` 用）

- [ ] **Step 1: 失敗するテストを書く**

`src/wl/outputs.rs` を次の内容にする:

```rust
//! 出力の情報（名前・説明）。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_name_and_description() {
        let meta = OutputMeta::from_parts(
            Some("DP-10".into()),
            Some("LG Electronics LG ULTRAWIDE 311NTDV3Z418 (DP-10)".into()),
        );
        assert_eq!(meta.name, "DP-10");
        assert_eq!(
            meta.description,
            "LG Electronics LG ULTRAWIDE 311NTDV3Z418 (DP-10)"
        );
        assert_eq!(meta.label(), "DP-10");
    }

    #[test]
    fn missing_values_become_empty() {
        let meta = OutputMeta::from_parts(None, None);
        assert_eq!(meta.name, "");
        assert_eq!(meta.description, "");
        assert_eq!(meta.label(), "(unnamed output)");
    }

    #[test]
    fn converts_to_config_output_info() {
        let meta = OutputMeta::from_parts(Some("eDP-1".into()), Some("panel (eDP-1)".into()));
        let info = meta.to_output_info();
        assert_eq!(info.name, "eDP-1");
        assert_eq!(info.description, "panel (eDP-1)");
    }
}
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test wl::`
Expected: コンパイルエラー（`cannot find type `OutputMeta``）。

- [ ] **Step 3: 実装を書く**

`src/wl/outputs.rs` のテストモジュールより上をすべて次の内容に置き換える:

```rust
//! 出力の情報（名前・説明）。

use smithay_client_toolkit::output::OutputInfo;

/// ロック中に覚えておく出力の情報。主画面の選択（Plan 4）とログに使う。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputMeta {
    /// 出力名（例: `DP-10`）。コンポジタが送ってこなければ空。
    pub name: String,
    /// wl_output の description。送ってこなければ空。
    pub description: String,
}

impl OutputMeta {
    /// SCTK の出力情報から作る。情報がまだ届いていなければ両方とも空。
    pub fn from_info(info: Option<&OutputInfo>) -> OutputMeta {
        OutputMeta::from_parts(
            info.and_then(|i| i.name.clone()),
            info.and_then(|i| i.description.clone()),
        )
    }

    pub fn from_parts(name: Option<String>, description: Option<String>) -> OutputMeta {
        OutputMeta {
            name: name.unwrap_or_default(),
            description: description.unwrap_or_default(),
        }
    }

    /// ログに出す名前。
    pub fn label(&self) -> &str {
        if self.name.is_empty() {
            "(unnamed output)"
        } else {
            &self.name
        }
    }

    /// `config::select_primary` に渡す形にする。
    pub fn to_output_info(&self) -> crate::config::OutputInfo {
        crate::config::OutputInfo {
            name: self.name.clone(),
            description: self.description.clone(),
        }
    }
}
```

- [ ] **Step 4: テストと lint を通す**

Run: `cargo test wl::`
Expected: `test result: ok. 3 passed`

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: 警告なし、差分なし。

- [ ] **Step 5: コミットする**

```bash
git status
git diff
git add src/wl/outputs.rs
git commit -m "feat: keep output names and descriptions

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: `AppError` と `App` の型（`app.rs`）

**Files:**
- Modify: `Cargo.toml`, `Cargo.lock`, `src/app.rs`

**Interfaces:**
- Consumes: `wayland_client::{delegate_noop, protocol::wl_buffer}`
- Produces:
  - `pub enum AppError { NoWayland(String), LockUnsupported, LockDenied, Other(String) }`（`Clone, Debug, PartialEq, Eq`、`Display`、`std::error::Error`）
  - `pub struct App`（この時点では中身なし。Task 11 で中身を足す）と `Dispatch<wl_buffer::WlBuffer, ()> for App`（Task 10 の `ShmBlack` が使う）

- [ ] **Step 1: wayland-client を追加する**

Run: `cargo add wayland-client@0.31.15`
Expected: `Cargo.toml` に `wayland-client = "0.31.15"` が入る（SCTK が使うものと同じ版）。

- [ ] **Step 2: 依存の追加をコミットする**

```bash
git status
git diff
git add Cargo.toml Cargo.lock
git commit -m "build: add wayland-client

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 3: 失敗するテストを書く**

`src/app.rs` を次の内容にする:

```rust
//! Wayland のイベントループ。ロックを掛け、入力と認証の結果で状態を進め、解除する。

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
```

- [ ] **Step 4: テストが失敗することを確認する**

Run: `cargo test app::`
Expected: コンパイルエラー（`cannot find type `AppError``）。

- [ ] **Step 5: 実装を書く**

`src/app.rs` のテストモジュールより上をすべて次の内容に置き換える:

```rust
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
```

- [ ] **Step 6: テストと lint を通す**

Run: `cargo test app::`
Expected: `test result: ok. 1 passed`

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: 警告なし、差分なし。

- [ ] **Step 7: コミットする**

```bash
git status
git diff
git add src/app.rs
git commit -m "feat: add AppError with readable messages

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: `wl_shm` の黒い画面（`render`）

**Files:**
- Modify: `src/render/shm.rs`, `src/render/mod.rs`

**Interfaces:**
- Consumes: `smithay_client_toolkit::shm::{Shm, raw::RawPool}`、`app::App`（`Dispatch<WlBuffer, ()>`）
- Produces:
  - `pub struct ShmBlack`（`Default`）、`ShmBlack::new() -> ShmBlack`
  - `ShmBlack::draw(&mut self, shm: &Shm, surface: &wl_surface::WlSurface, width: u32, height: u32, qh: &QueueHandle<crate::app::App>)`
  - `pub enum OutputRender { Shm(shm::ShmBlack) }`

- [ ] **Step 1: 失敗するテストを書く**

バッファの大きさの計算と塗りつぶしは純粋な関数に分けてテストする。`wl_buffer` を作る部分は Task 12 の手動確認で確かめる。

`src/render/shm.rs` を次の内容にする:

```rust
//! `wl_shm` の黒い画面。EGL が使えないときの予備にもなる。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_len_is_four_bytes_per_pixel() {
        assert_eq!(buffer_len(1920, 1080), Some(1920 * 1080 * 4));
        assert_eq!(buffer_len(3440, 1440), Some(3440 * 1440 * 4));
    }

    #[test]
    fn buffer_len_rejects_empty_and_huge_sizes() {
        assert_eq!(buffer_len(0, 1080), None);
        assert_eq!(buffer_len(1920, 0), None);
        // wl_shm のプールの大きさは i32 なので、それを超えるものは作れない
        assert_eq!(buffer_len(65_536, 65_536), None);
        assert_eq!(buffer_len(u32::MAX, 1), None);
    }

    #[test]
    fn fill_black_writes_opaque_black_argb() {
        let mut pixels = vec![0x55u8; 4 * 3];
        fill_black(&mut pixels);
        // ARGB8888 はリトルエンディアンで B, G, R, A の順に並ぶ
        assert_eq!(pixels, [0, 0, 0, 0xff].repeat(3));
    }
}
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test render::`
Expected: コンパイルエラー（`buffer_len`、`fill_black` が見つからない）。

- [ ] **Step 3: 実装を書く**

`src/render/shm.rs` のテストモジュールより上をすべて次の内容に置き換える:

```rust
//! `wl_shm` の黒い画面。EGL が使えないときの予備にもなる。

use smithay_client_toolkit::shm::{Shm, raw::RawPool};
use wayland_client::QueueHandle;
use wayland_client::protocol::{wl_buffer, wl_shm, wl_surface};

use crate::app::App;

/// 出力 1 つ分の黒い画面。大きさが変わるまで同じバッファを使い回す。
#[derive(Default)]
pub struct ShmBlack {
    buffer: Option<BlackBuffer>,
}

impl ShmBlack {
    pub fn new() -> ShmBlack {
        ShmBlack::default()
    }

    /// 黒い ARGB8888 バッファを作って attach + commit する。
    ///
    /// ext-session-lock では configure の大きさと同じバッファを出さなければならないので、
    /// `width` x `height` が前回と違えば作り直す。作れなければログを出して何もしない。
    pub fn draw(
        &mut self,
        shm: &Shm,
        surface: &wl_surface::WlSurface,
        width: u32,
        height: u32,
        qh: &QueueHandle<App>,
    ) {
        let reusable = self
            .buffer
            .as_ref()
            .is_some_and(|b| b.width == width && b.height == height);
        // 古いバッファは、新しいバッファを commit してから捨てる
        let mut old = None;
        if !reusable {
            match BlackBuffer::new(shm, width, height, qh) {
                Ok(buffer) => old = self.buffer.replace(buffer),
                Err(e) => {
                    log::error!("cannot create a {width}x{height} black buffer: {e}");
                    return;
                }
            }
        }
        let Some(buffer) = &self.buffer else { return };
        surface.attach(Some(&buffer.buffer), 0, 0);
        surface.damage_buffer(0, 0, width as i32, height as i32);
        surface.commit();
        drop(old);
    }
}

/// 黒で塗った wl_buffer と、その元のメモリプール。
struct BlackBuffer {
    // プールより先に wl_buffer を破棄する（Drop を参照）
    buffer: wl_buffer::WlBuffer,
    _pool: RawPool,
    width: u32,
    height: u32,
}

impl BlackBuffer {
    fn new(shm: &Shm, width: u32, height: u32, qh: &QueueHandle<App>) -> Result<Self, String> {
        let len = buffer_len(width, height).ok_or_else(|| "invalid size".to_string())?;
        let mut pool = RawPool::new(len, shm).map_err(|e| e.to_string())?;
        fill_black(&mut pool.mmap()[..len]);
        // buffer_len が i32 に収まることを確かめている
        let buffer = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            (width * 4) as i32,
            wl_shm::Format::Argb8888,
            (),
            qh,
        );
        Ok(BlackBuffer {
            buffer,
            _pool: pool,
            width,
            height,
        })
    }
}

impl Drop for BlackBuffer {
    fn drop(&mut self) {
        self.buffer.destroy();
    }
}

/// ARGB8888 のバッファのバイト数。0 や、wl_shm のプールの上限（`i32::MAX`）を超える大きさは `None`。
fn buffer_len(width: u32, height: u32) -> Option<usize> {
    if width == 0 || height == 0 {
        return None;
    }
    let len = u64::from(width) * u64::from(height) * 4;
    if len > i32::MAX as u64 {
        return None;
    }
    usize::try_from(len).ok()
}

/// 不透明な黒で塗る。ARGB8888 はリトルエンディアンで B, G, R, A の順。
fn fill_black(pixels: &mut [u8]) {
    for pixel in pixels.as_chunks_mut::<4>().0 {
        *pixel = [0, 0, 0, 0xff];
    }
}
```

- [ ] **Step 4: テストと lint を通す**

Run: `cargo test render::`
Expected: `test result: ok. 3 passed`

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: 警告なし、差分なし。

- [ ] **Step 5: shm をコミットする**

```bash
git status
git diff
git add src/render/shm.rs
git commit -m "feat: draw opaque black shm buffers

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 6: `OutputRender` を書く**

`src/render/mod.rs` を次の内容にする:

```rust
//! 描画。出力ごとの描画手段を `OutputRender` で持つ。

pub mod shm;

/// 出力 1 つの描画手段。
pub enum OutputRender {
    /// `wl_shm` の黒い画面。
    Shm(shm::ShmBlack),
    // Plan 3 が追加: Gpu(egl::GpuSurface),
}
```

Run: `cargo build && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: エラー・警告・差分なし。

- [ ] **Step 7: render/mod をコミットする**

```bash
git status
git diff
git add src/render/mod.rs
git commit -m "feat: add OutputRender with the shm variant

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: `App` とイベントループ（`app.rs`）

**Files:**
- Modify: `src/app.rs`

**Interfaces:**
- Consumes: Task 4〜10 のすべて、`state::{Machine, Event, Action}`、`password::PasswordBuffer`、`config::Config`、SCTK（`compositor`、`output`、`registry`、`seat`、`session_lock`、`shm`、`reexports::calloop`、`reexports::calloop_wayland_source`）
- Produces:
  - `pub fn run(config: Config) -> Result<(), AppError>`
  - `pub struct App`（フィールドはすべて `pub(crate)`。Plan 3〜5 はここに足す）
  - `pub(crate) struct LockOutput { output, meta, lock_surface, size, render, frame_pending }`
  - `impl App { pub(crate) fn draw_output(&mut self, index: usize) }`
  - SCTK のハンドラ: `SessionLockHandler`、`KeyboardHandler`、`SeatHandler`、`CompositorHandler`、`OutputHandler`、`ShmHandler`、`ProvidesRegistryState`

このタスクは Wayland と PAM に依存するので単体テストを足さない。ビルドと lint を通し、動作は Task 12 で確かめる。

- [ ] **Step 1: `src/app.rs` を書き換える**

`src/app.rs` 全体を次の内容にする（Task 9 の `AppError` とテストはそのまま含まれている。`pub struct App {}` は中身のある定義に置き換わる）:

```rust
//! Wayland のイベントループ。ロックを掛け、入力と認証の結果で状態を進め、解除する。

use std::fmt;
use std::time::Duration;

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_dispatch2, delegate_registry,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{
            EventLoop, channel,
            timer::{TimeoutAction, Timer},
        },
        calloop_wayland_source::WaylandSource,
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
    },
    session_lock::{
        SessionLock, SessionLockHandler, SessionLockState, SessionLockSurface,
        SessionLockSurfaceConfigure,
    },
    shm::{Shm, ShmHandler},
};
use wayland_client::{
    Connection, Proxy, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_buffer, wl_keyboard, wl_output, wl_seat, wl_surface},
};
use zeroize::Zeroizing;

use crate::auth::{self, AuthEvent, password::PasswordChecker};
use crate::clock::Clock;
use crate::config::Config;
use crate::input::{self, KeyAction};
use crate::password::PasswordBuffer;
use crate::render::{OutputRender, shm::ShmBlack};
use crate::state::{Action, Event, Machine};
use crate::wl::outputs::OutputMeta;

/// `Event::Tick` を送る間隔。画面が消えていてフレームコールバックが来なくても状態を進めるため。
const TICK: Duration = Duration::from_millis(50);

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

/// 出力 1 つ分のロック画面。
pub(crate) struct LockOutput {
    /// 抜かれたときに探すための wl_output。
    pub(crate) output: wl_output::WlOutput,
    pub(crate) meta: OutputMeta,
    pub(crate) lock_surface: SessionLockSurface,
    /// configure の論理サイズ。configure 前は (0, 0)。
    pub(crate) size: (u32, u32),
    /// configure 前は `None`。
    pub(crate) render: Option<OutputRender>,
    /// フレームコールバック待ち。Plan 2 はフレームコールバックを要求しない（Plan 3 で使う）。
    pub(crate) frame_pending: bool,
}

/// ロック中の状態。SCTK の各ハンドラはこの型に実装する。
pub struct App {
    pub(crate) conn: Connection,
    pub(crate) qh: QueueHandle<App>,
    /// Plan 3〜5 で見た目や認証の設定を読む。
    #[allow(dead_code)]
    pub(crate) config: Config,
    /// PAM に渡したユーザー名。Plan 5 の U2F でも使う。
    #[allow(dead_code)]
    pub(crate) user: String,
    pub(crate) registry: RegistryState,
    pub(crate) compositor: CompositorState,
    pub(crate) output_state: OutputState,
    pub(crate) seat_state: SeatState,
    pub(crate) shm: Shm,
    pub(crate) lock_state: SessionLockState,
    /// `lock` を要求してから、解除するか `finished` を受けるまで `Some`。
    pub(crate) lock: Option<SessionLock>,
    /// `ReleaseLock` を受けたが、まだ `unlock` していない。
    pub(crate) release_pending: bool,
    pub(crate) outputs: Vec<LockOutput>,
    pub(crate) keyboards: Vec<(wl_seat::WlSeat, wl_keyboard::WlKeyboard)>,
    pub(crate) ctrl: bool,
    pub(crate) machine: Machine,
    pub(crate) password: PasswordBuffer,
    pub(crate) checker: PasswordChecker,
    pub(crate) clock: Clock,
    /// 最初の configure を受けた時刻。フェードはここから測る（Plan 3）。
    pub(crate) first_configure_ms: Option<u64>,
    /// `Some` になったらイベントループを抜けて、これを `run` の結果にする。
    pub(crate) exit: Option<Result<(), AppError>>,
}

/// ロックして、解除されるまでイベントループを回す。
///
/// ext-session-lock が使えない／拒否されたら `Err`。
/// 失敗しうる準備はすべて `lock` の要求より前に済ませる（ロックした後に止まると画面が赤のまま残る）。
pub fn run(config: Config) -> Result<(), AppError> {
    let clock = Clock::start();
    let user = auth::current_user()
        .ok_or_else(|| AppError::Other("cannot determine the current user name".into()))?;

    let conn = Connection::connect_to_env().map_err(|e| AppError::NoWayland(e.to_string()))?;
    let (globals, mut queue) =
        registry_queue_init::<App>(&conn).map_err(|e| AppError::NoWayland(e.to_string()))?;
    let qh = queue.handle();
    let compositor = CompositorState::bind(&globals, &qh)
        .map_err(|e| AppError::NoWayland(format!("wl_compositor: {e}")))?;
    let shm = Shm::bind(&globals, &qh).map_err(|e| AppError::NoWayland(format!("wl_shm: {e}")))?;

    let mut event_loop: EventLoop<'static, App> =
        EventLoop::try_new().map_err(|e| AppError::Other(format!("event loop: {e}")))?;
    let (auth_tx, auth_rx) = channel::channel::<AuthEvent>();
    let checker = PasswordChecker::spawn(config.auth.pam_service.clone(), user.clone(), auth_tx);

    let mut app = App {
        conn: conn.clone(),
        qh: qh.clone(),
        config,
        user,
        registry: RegistryState::new(&globals),
        compositor,
        output_state: OutputState::new(&globals, &qh),
        seat_state: SeatState::new(&globals, &qh),
        shm,
        lock_state: SessionLockState::new(&globals, &qh),
        lock: None,
        release_pending: false,
        outputs: Vec::new(),
        keyboards: Vec::new(),
        ctrl: false,
        machine: Machine::new(),
        password: PasswordBuffer::new(),
        checker,
        clock,
        first_configure_ms: None,
        exit: None,
    };

    let handle = event_loop.handle();
    handle
        .insert_source(auth_rx, |event, _, app: &mut App| {
            if let channel::Event::Msg(event) = event {
                app.handle_auth(event);
            }
        })
        .map_err(|e| AppError::Other(format!("auth channel: {}", e.error)))?;
    handle
        .insert_source(Timer::from_duration(TICK), |_, _, app: &mut App| {
            app.handle_event(Event::Tick);
            TimeoutAction::ToDuration(TICK)
        })
        .map_err(|e| AppError::Other(format!("tick timer: {}", e.error)))?;

    // 出力の名前と説明を受け取る
    queue
        .roundtrip(&mut app)
        .map_err(|e| AppError::NoWayland(e.to_string()))?;
    for output in app.output_state.outputs() {
        let meta = OutputMeta::from_info(app.output_state.info(&output).as_ref());
        log::info!("output {}: {}", meta.label(), meta.description);
    }
    WaylandSource::new(conn, queue)
        .insert(handle)
        .map_err(|e| AppError::Other(format!("wayland source: {}", e.error)))?;

    let lock = app
        .lock_state
        .lock(&qh)
        .map_err(|_| AppError::LockUnsupported)?;
    log::info!("lock requested after {} ms", app.clock.now_ms());
    app.lock = Some(lock);
    // locked を待たずに、全出力の lock surface を作る
    for output in app.output_state.outputs() {
        app.add_output(output);
    }

    while app.exit.is_none() {
        event_loop
            .dispatch(None, &mut app)
            .map_err(|e| AppError::Other(format!("event loop failed: {e}")))?;
    }
    app.exit.take().unwrap_or(Ok(()))
}

impl App {
    /// 描画の入口。出力 `index` の今の状態を描いて commit する。
    pub(crate) fn draw_output(&mut self, index: usize) {
        let Some(out) = self.outputs.get_mut(index) else {
            return;
        };
        let (width, height) = out.size;
        if width == 0 || height == 0 {
            return;
        }
        // Plan 3 が Gpu の分岐を足すので、if let ではなく match にしておく
        match out.render.as_mut() {
            Some(OutputRender::Shm(black)) => {
                black.draw(
                    &self.shm,
                    out.lock_surface.wl_surface(),
                    width,
                    height,
                    &self.qh,
                );
            }
            None => {}
        }
    }

    /// 出力に lock surface を作る。ロック前や、作成済みの出力なら情報だけ更新する。
    fn add_output(&mut self, output: wl_output::WlOutput) {
        let meta = OutputMeta::from_info(self.output_state.info(&output).as_ref());
        if let Some(existing) = self.outputs.iter_mut().find(|o| o.output == output) {
            existing.meta = meta;
            return;
        }
        let Some(lock) = &self.lock else {
            return;
        };
        let surface = self.compositor.create_surface(&self.qh);
        let lock_surface = lock.create_lock_surface(surface, &output, &self.qh);
        log::info!("lock surface created for {}", meta.label());
        self.outputs.push(LockOutput {
            output,
            meta,
            lock_surface,
            size: (0, 0),
            render: None,
            frame_pending: false,
        });
    }

    /// 抜かれた出力の lock surface とバッファを片付ける。
    fn remove_output(&mut self, output: &wl_output::WlOutput) {
        if let Some(index) = self.outputs.iter().position(|o| &o.output == output) {
            let removed = self.outputs.remove(index);
            log::info!("output removed: {}", removed.meta.label());
        }
    }

    /// キーを 1 回分処理する。
    fn handle_key(&mut self, event: KeyEvent) {
        // xkbcommon が作った文字列も、使い終わったらゼロで消す
        let text = event.utf8.map(Zeroizing::new);
        let action = input::classify(event.keysym, text.as_deref().map(String::as_str), self.ctrl);
        match action {
            KeyAction::Submit => {
                let empty = self.password.is_empty();
                self.handle_event(Event::Submit { empty });
            }
            KeyAction::Backspace => {
                if self.machine.accepts_input() {
                    self.password.backspace();
                }
            }
            KeyAction::Clear => {
                if self.machine.accepts_input() {
                    self.password.clear();
                }
            }
            KeyAction::Insert => {
                if self.machine.accepts_input()
                    && let Some(text) = &text
                {
                    // 入りきらない分は捨てる（Plan 4 は true のときだけ光の柱を出す）
                    self.password.push_str(text);
                }
            }
            KeyAction::Ignore => {}
        }
    }

    /// 認証スレッドからの結果を状態機械に渡す。
    fn handle_auth(&mut self, event: AuthEvent) {
        match event {
            AuthEvent::PasswordChecked { ok: true } => log::info!("password accepted"),
            AuthEvent::PasswordChecked { ok: false } => log::warn!("password rejected"),
            AuthEvent::U2fSucceeded => log::info!("security key accepted"),
        }
        self.handle_event(Event::from(event));
    }

    /// 状態機械にイベントを渡し、返ってきた操作を行う。
    fn handle_event(&mut self, event: Event) {
        let now = self.clock.now_ms();
        let before = self.machine.phase();
        let action = self.machine.handle(event, now);
        let after = self.machine.phase();
        if before != after {
            log::debug!("phase {before:?} -> {after:?} at {now} ms");
        }
        match action {
            Some(Action::CheckPassword) => {
                // handle が CheckPassword を返してから取り出す（取り出したら空になる）
                log::info!("checking password");
                self.checker.check(self.password.take());
            }
            Some(Action::ReleaseLock) => {
                self.release_pending = true;
                self.release_lock();
            }
            None => {}
        }
    }

    /// 解除が決まっていて、`locked` を受け取り済みならロックを外して終わる。
    ///
    /// `unlock_and_destroy` は `locked` の前に送るとプロトコル違反なので、その場合は `locked` で呼び直す。
    fn release_lock(&mut self) {
        if !self.release_pending {
            return;
        }
        let Some(lock) = &self.lock else {
            return;
        };
        if !lock.is_locked() {
            log::debug!("unlock is waiting for the locked event");
            return;
        }
        lock.unlock();
        // unlock_and_destroy をコンポジタに届けてから終わる
        if let Err(e) = self.conn.roundtrip() {
            log::warn!("roundtrip after unlock failed: {e}");
        }
        self.lock = None;
        self.release_pending = false;
        log::info!("unlocked after {} ms", self.clock.now_ms());
        self.exit = Some(Ok(()));
    }
}

impl SessionLockHandler for App {
    fn locked(&mut self, _: &Connection, _: &QueueHandle<Self>, _: SessionLock) {
        log::info!("session locked after {} ms", self.clock.now_ms());
        self.release_lock();
    }

    fn finished(&mut self, _: &Connection, _: &QueueHandle<Self>, session_lock: SessionLock) {
        let error = if session_lock.is_locked() {
            AppError::Other("the compositor ended the lock".into())
        } else {
            AppError::LockDenied
        };
        log::error!("lock finished: {error}");
        self.lock = None;
        self.exit = Some(Err(error));
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: SessionLockSurface,
        configure: SessionLockSurfaceConfigure,
        _serial: u32,
    ) {
        let Some(index) = self
            .outputs
            .iter()
            .position(|o| o.lock_surface.wl_surface() == surface.wl_surface())
        else {
            return;
        };
        let now = self.clock.now_ms();
        if self.first_configure_ms.is_none() {
            self.first_configure_ms = Some(now);
            log::info!("first configure after {now} ms");
        }
        let out = &mut self.outputs[index];
        out.size = configure.new_size;
        log::debug!(
            "configure {}: {}x{}",
            out.meta.label(),
            out.size.0,
            out.size.1
        );
        if out.render.is_none() {
            out.render = Some(OutputRender::Shm(ShmBlack::new()));
        }
        self.draw_output(index);
    }
}

impl KeyboardHandler for App {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
    }

    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
    ) {
        self.ctrl = false;
    }

    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.handle_key(event);
    }

    fn repeat_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
        // キーリピートは使わない（get_keyboard では呼ばれない）
    }

    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        modifiers: Modifiers,
        _: RawModifiers,
        _: u32,
    ) {
        self.ctrl = modifiers.ctrl;
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability != Capability::Keyboard || self.keyboards.iter().any(|(s, _)| *s == seat) {
            return;
        }
        match self.seat_state.get_keyboard(qh, &seat, None) {
            Ok(keyboard) => self.keyboards.push((seat, keyboard)),
            Err(e) => log::error!("cannot get the keyboard: {e}"),
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability != Capability::Keyboard {
            return;
        }
        if let Some(index) = self.keyboards.iter().position(|(s, _)| *s == seat) {
            let (_, keyboard) = self.keyboards.remove(index);
            if keyboard.version() >= 3 {
                keyboard.release();
            }
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        if let Some(out) = self
            .outputs
            .iter_mut()
            .find(|o| o.lock_surface.wl_surface() == surface)
        {
            out.frame_pending = false;
        }
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, output: wl_output::WlOutput) {
        // ロック前（最初の roundtrip 中）は何もしない。ロック後に挿された出力には lock surface を作る
        self.add_output(output);
    }

    fn update_output(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        let meta = OutputMeta::from_info(self.output_state.info(&output).as_ref());
        if let Some(existing) = self.outputs.iter_mut().find(|o| o.output == output) {
            existing.meta = meta;
        }
    }

    fn output_destroyed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        self.remove_output(&output);
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState, SeatState];
}

delegate_registry!(App);
delegate_dispatch2!(App);
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
```

要点:
- `run` は `lock` を要求する前に、失敗しうる準備（接続、bind、イベントループ、チャンネルとタイマーの登録、roundtrip、`WaylandSource` の登録）をすべて済ませる。
- `lock` の直後に、`locked` を待たずに全出力の lock surface を作る。`configure` が来た出力から `ShmBlack` で黒を出す。
- `handle_event` は `Machine::handle` が `CheckPassword` を返してから `PasswordBuffer::take` を呼ぶ。
- `release_lock` は `locked` を受けていなければ待つ（`locked` の中で呼び直す）。
- 時刻はすべて `self.clock.now_ms()`。`Tick` は 50ms ごとのタイマーから送る。

- [ ] **Step 2: ビルドと lint を通す**

Run: `cargo build`
Expected: エラーなし。

Run: `cargo clippy --all-targets -- -D warnings`
Expected: 警告なし。`config` と `user` には `#[allow(dead_code)]` を付けてある（`config` は Plan 3〜5 が読む。`user` は U2F の helper が自分でユーザー名を取ることになったので、Plan 5 Task 6 で外す）。

Run: `cargo fmt --check`
Expected: 差分なし。

Run: `cargo test`
Expected: `test result: ok. 88 passed`（lib）。

- [ ] **Step 3: コミットする**

```bash
git status
git diff
git add src/app.rs
git commit -m "feat: lock every output and unlock through PAM

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 12: コマンド（`main.rs`）と入れ子の sway での確認

**Files:**
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: `app::{run, AppError}`、`config::load`
- Produces: `kabi-lock`（引数なし: ロック、`--version` / `-V`、`--help` / `-h`、それ以外は終了コード 2）。終了コードは契約 7 章どおり（0: 解除して終了、1: ロックできなかった、2: 引数が不正）。

- [ ] **Step 1: 失敗するテストを書く**

今の `src/main.rs`（Plan 1 の「まだロックできない」と出すだけのもの）の末尾に、次のテストモジュールを足す:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Command, String> {
        parse_args(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn no_arguments_runs_the_locker() {
        assert_eq!(parse(&[]), Ok(Command::Run));
    }

    #[test]
    fn version_and_help_flags() {
        assert_eq!(parse(&["--version"]), Ok(Command::Version));
        assert_eq!(parse(&["-V"]), Ok(Command::Version));
        assert_eq!(parse(&["--help"]), Ok(Command::Help));
        assert_eq!(parse(&["-h"]), Ok(Command::Help));
    }

    #[test]
    fn unknown_arguments_are_errors() {
        assert_eq!(
            parse(&["--frobnicate"]),
            Err("unknown argument: --frobnicate".to_string())
        );
        assert_eq!(
            parse(&["--version", "--help"]),
            Err("too many arguments".to_string())
        );
    }

    #[test]
    fn exit_codes_follow_the_contract() {
        assert_eq!(exit_code(&Ok(())), 0);
        assert_eq!(exit_code(&Err(AppError::LockDenied)), 1);
        assert_eq!(exit_code(&Err(AppError::LockUnsupported)), 1);
        assert_eq!(exit_code(&Err(AppError::NoWayland("x".into()))), 1);
        assert_eq!(exit_code(&Err(AppError::Other("x".into()))), 1);
        assert_eq!(USAGE_ERROR, 2);
    }
}
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test --bin kabi-lock`
Expected: コンパイルエラー（`parse_args`、`Command`、`exit_code`、`USAGE_ERROR`、`AppError` が見つからない）。

- [ ] **Step 3: 実装を書く**

`src/main.rs` 全体を次の内容にする（テストモジュールは Step 1 のものと同じ）:

```rust
//! kabi-lock のコマンド。引数を読み、設定を読み込み、ロックして、解除されたら終わる。

use std::process::ExitCode;

use kabi_lock::app::{self, AppError};

/// 引数が不正なときの終了コード。
const USAGE_ERROR: u8 = 2;

const USAGE: &str = "\
Usage: kabi-lock [--help | --version]

Lock the session (ext-session-lock) until the password is entered.
Settings are read from $XDG_CONFIG_HOME/kabi-lock/config.toml.
Logging is controlled by RUST_LOG (default: warn).
";

/// 引数から決まる動作。
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Run,
    Version,
    Help,
}

/// プログラム名を除いた引数を読む。知らない引数や 2 つ以上の引数はエラー。
fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let mut args = args.into_iter();
    let command = match args.next().as_deref() {
        None => Command::Run,
        Some("--version" | "-V") => Command::Version,
        Some("--help" | "-h") => Command::Help,
        Some(other) => return Err(format!("unknown argument: {other}")),
    };
    if args.next().is_some() {
        return Err("too many arguments".to_string());
    }
    Ok(command)
}

/// `app::run` の結果を終了コードにする（0: 解除して終了、1: ロックできなかった）。
fn exit_code(result: &Result<(), AppError>) -> u8 {
    match result {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

fn main() -> ExitCode {
    let command = match parse_args(std::env::args().skip(1)) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("kabi-lock: {message}\n\n{USAGE}");
            return ExitCode::from(USAGE_ERROR);
        }
    };
    match command {
        Command::Version => {
            println!("kabi-lock {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Command::Help => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Command::Run => {
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
                .init();
            let config = kabi_lock::config::load();
            let result = app::run(config);
            if let Err(e) = &result {
                eprintln!("kabi-lock: {e}");
            }
            ExitCode::from(exit_code(&result))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Command, String> {
        parse_args(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn no_arguments_runs_the_locker() {
        assert_eq!(parse(&[]), Ok(Command::Run));
    }

    #[test]
    fn version_and_help_flags() {
        assert_eq!(parse(&["--version"]), Ok(Command::Version));
        assert_eq!(parse(&["-V"]), Ok(Command::Version));
        assert_eq!(parse(&["--help"]), Ok(Command::Help));
        assert_eq!(parse(&["-h"]), Ok(Command::Help));
    }

    #[test]
    fn unknown_arguments_are_errors() {
        assert_eq!(
            parse(&["--frobnicate"]),
            Err("unknown argument: --frobnicate".to_string())
        );
        assert_eq!(
            parse(&["--version", "--help"]),
            Err("too many arguments".to_string())
        );
    }

    #[test]
    fn exit_codes_follow_the_contract() {
        assert_eq!(exit_code(&Ok(())), 0);
        assert_eq!(exit_code(&Err(AppError::LockDenied)), 1);
        assert_eq!(exit_code(&Err(AppError::LockUnsupported)), 1);
        assert_eq!(exit_code(&Err(AppError::NoWayland("x".into()))), 1);
        assert_eq!(exit_code(&Err(AppError::Other("x".into()))), 1);
        assert_eq!(USAGE_ERROR, 2);
    }
}
```

- [ ] **Step 4: テストと lint を通す**

Run: `cargo test --bin kabi-lock`
Expected: `test result: ok. 4 passed`

Run: `cargo test`
Expected: lib が `88 passed`、bin が `4 passed`。

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: 警告なし、差分なし。

- [ ] **Step 5: 引数と終了コードを確かめる（ロックはしない）**

Run: `cargo build && ./target/debug/kabi-lock --version; echo "exit=$?"`
Expected:

```text
kabi-lock 0.1.0
exit=0
```

Run: `./target/debug/kabi-lock --help; echo "exit=$?"`
Expected: `Usage: kabi-lock [--help | --version]` から始まる説明と `exit=0`。

Run: `./target/debug/kabi-lock --bogus; echo "exit=$?"`
Expected: `kabi-lock: unknown argument: --bogus`、続けて使い方、`exit=2`。

存在しないソケット名を指定して、Wayland に接続できないときの終了コードを確かめる（本物のセッションには接続しない）:

Run: `WAYLAND_DISPLAY=kabi-lock-no-such-socket ./target/debug/kabi-lock; echo "exit=$?"`
Expected:

```text
kabi-lock: cannot connect to the Wayland compositor: Could not find wayland compositor
exit=1
```

- [ ] **Step 6: コミットする**

```bash
git status
git diff
git add src/main.rs
git commit -m "feat: run the locker from the command line with exit codes

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 7: 手動確認 A — ロック・黒画面・出力の抜き挿し（画面もキーボードもない入れ子の sway）**

本物のセッションに影響しないよう、`XDG_RUNTIME_DIR` を新しい空のディレクトリに替えて、ヘッドレスの sway を入れ子で動かす。
kabi-lock はその sway の子として起動するので、入れ子の sway のソケットにしか接続できない。ソケットのパスは 108 バイトまでなので、短いパスを使う。
キーボードがないので解除はできない。最後に kabi-lock を止めて、`swaymsg exit` で sway を終える（ロック中でも IPC は効く）。

黒いバッファを本当に付けたかは画面からは分からない（sway はロック中、lock surface に何もないときも黒で覆う）。
そこでこの確認に限り `WAYLAND_DEBUG=client` で要求を記録し、`attach` と `commit` を確かめる。
キーボードのある環境（Step 8）では `WAYLAND_DEBUG` を **使わない**（`wl_keyboard.key` イベントとして打ったキーが記録される）。

```bash
RT=$(mktemp -d /tmp/kabi-rt.XXXX); chmod 700 "$RT"
BIN=$PWD/target/debug/kabi-lock
cat > /tmp/kabi-headless.conf <<EOF
exec sh -c 'WAYLAND_DEBUG=client RUST_LOG=kabi_lock=debug $BIN > /tmp/kabi-headless.log 2>&1; echo "exit=\$?" >> /tmp/kabi-headless.log'
exec sh -c 'sleep 2; swaymsg create_output; sleep 2; swaymsg output HEADLESS-2 unplug; sleep 2; pkill -f "^$BIN"; sleep 1; swaymsg exit'
EOF
env -i HOME="$HOME" PATH="$PATH" USER="$USER" XDG_RUNTIME_DIR="$RT" \
  WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 \
  timeout 40 sway -c /tmp/kabi-headless.conf > /tmp/kabi-headless-sway.log 2>&1
rm -rf "$RT"
grep -E 'kabi_lock|^exit=' /tmp/kabi-headless.log
```

Expected（時刻とミリ秒の数字は環境による）: 次の行がこの順で出る。

```text
INFO  kabi_lock::app] output HEADLESS-1: Headless output 1
INFO  kabi_lock::app] lock requested after 0 ms
INFO  kabi_lock::app] lock surface created for HEADLESS-1
INFO  kabi_lock::app] session locked after 1 ms
INFO  kabi_lock::app] first configure after 1 ms
DEBUG kabi_lock::app] configure HEADLESS-1: 1280x720
INFO  kabi_lock::app] lock surface created for HEADLESS-2
DEBUG kabi_lock::app] configure HEADLESS-2: 1920x1080
INFO  kabi_lock::app] output removed: HEADLESS-2
exit=143
```

（出力が増減したときに `configure HEADLESS-1: 1280x720` がもう一度出ることがある。`exit=143` は `pkill` の SIGTERM による。）

Run: `grep -E 'get_lock_surface|ack_configure|create_buffer|attach|commit' /tmp/kabi-headless.log | head -8`
Expected（`@` の後の番号と時刻は環境による）: lock surface ごとに、configure の ack のあと、configure と同じ大きさで
形式 0（ARGB8888）のバッファを作って attach と commit している。

```text
-> ext_session_lock_v1@11.get_lock_surface(ext_session_lock_surface_v1@13, wl_surface@12, wl_output@6)
-> ext_session_lock_surface_v1@13.ack_configure(2)
-> wl_shm_pool@14.create_buffer(wl_buffer@15, 0, 1280, 720, 5120, 0)
-> wl_surface@12.attach(wl_buffer@15, 0, 0)
-> wl_surface@12.commit()
```

- [ ] **Step 8: 手動確認 B — パスワードで解除する（窓のある入れ子の sway）**

`/etc/pam.d/kabi-lock` は Plan 6 でパッケージから入れる。まだない場合、PAM は `/etc/pam.d/other`（すべて拒否）を使うので解除できない。
`pam_check` は `authenticate` の後に `acct_mgmt` も呼ぶ（`NEW_AUTHTOK_REQD` だけは通す）ので、使う PAM サービスには `auth` と `account` の両方の行が要る。
`/etc/pam.d/swaylock` は Arch では `auth include login` だけで `account` の行がなく、`acct_mgmt` が `PAM_PERM_DENIED` になって正しいパスワードでも失敗するので使わない。
システムを変えずに済むよう、一時的な設定ファイルで `login` の PAM サービスを使う（Plan 6 の `/etc/pam.d/kabi-lock` は `auth include login` と `account include login` なので、同じ規則で確かめられる）。

まず `/etc/pam.d/login` に `auth` と `account` の行があることを確かめる:

```bash
grep -qE '^auth[[:space:]]' /etc/pam.d/login && grep -qE '^account[[:space:]]' /etc/pam.d/login && echo PAM-OK || echo "PAM file missing auth or account line"
```

Expected: `PAM-OK`。`PAM file missing auth or account line` が出たら、ここで止めて BLOCKED として人に報告する（`/etc/pam.d` を `sudo` で変えるのは人の判断。エージェントは `sudo` を実行しない）。

一時的な設定ファイルを作る:

```bash
mkdir -p /tmp/kabi-nested-config/kabi-lock
printf '[auth]\npam_service = "login"\n' > /tmp/kabi-nested-config/kabi-lock/config.toml
```

この回避策（`XDG_CONFIG_HOME` の一時的な設定で `pam_service = "login"`）は、Plan 3〜5 の手動確認でも同じように使う。

契約 9 章の手順で入れ子の sway を起動する:

```bash
BIN=$PWD/target/debug/kabi-lock
cat > /tmp/kabi-nested.conf <<EOF
exec sh -c 'XDG_CONFIG_HOME=/tmp/kabi-nested-config RUST_LOG=kabi_lock=debug $BIN > /tmp/kabi-nested.log 2>&1; echo "exit=\$?" >> /tmp/kabi-nested.log; swaymsg exit'
EOF
WLR_BACKENDS=wayland sway -c /tmp/kabi-nested.conf
```

入れ子の sway の窓が真っ黒になる（赤くならない）ことを確かめる。窓にフォーカスを当て、自分のパスワードを **1 回だけ** 打って Enter を押す。
約 1.6 秒後（解除アニメーションの長さ。Plan 2 では黒いまま待つ）にロックが外れ、sway が終わって窓が閉じる。

Run: `cat /tmp/kabi-nested.log`
Expected（数字は環境による）:

```text
INFO  kabi_lock::app] output WL-1: ...
INFO  kabi_lock::app] lock requested after 0 ms
INFO  kabi_lock::app] lock surface created for WL-1
INFO  kabi_lock::app] session locked after ... ms
INFO  kabi_lock::app] first configure after ... ms
DEBUG kabi_lock::app] configure WL-1: ...x...
DEBUG kabi_lock::app] phase Idle -> Verifying at ... ms
INFO  kabi_lock::app] checking password
INFO  kabi_lock::app] password accepted
DEBUG kabi_lock::app] phase Verifying -> Unlocking { since_ms: ... } at ... ms
DEBUG kabi_lock::app] phase Unlocking { since_ms: ... } -> Done at ... ms
INFO  kabi_lock::app] unlocked after ... ms
exit=0
```

ログにパスワード・打った文字・その長さが出ていないことを確かめる（`grep -c` などで自分のパスワードを探さない。目で見る）。

キー操作の確認（PAM を呼ばない範囲）: もう一度起動し、何文字か打ってから Esc（または Ctrl+U）を押し、そのまま Enter を押す。
バッファが空なので `checking password` が出ないことを確かめる。Backspace で全部消してから Enter を押した場合も同じ。
確かめたら正しいパスワードで解除する（ここでも PAM を呼ぶのは 1 回だけ）。

失敗の確認はここではしない。間違ったパスワードを PAM に送るのは、Plan 2〜6 の手動確認全体で Plan 4 Task 15 Step 4 の **1 回だけ**にする（faillock の上限は既定で 15 分に 3 回。1 回でも余分に送らない）。
参考までに、そのとき出るログは `PAM authentication failed: ...`（`kabi_lock::auth::password` の WARN）、`password rejected`、
`phase Verifying -> Failed { since_ms: ... }`、約 1.2 秒後に `phase Failed { since_ms: ... } -> Idle`
（pam_faildelay などで結果が返るまで 2 秒ほどかかることがある）。失敗時の流れは Task 7 の単体テスト（`wrong` を拒む偽の確認関数）で確かめてある。

- [ ] **Step 9: 手動確認の後片付け**

Run: `rm -f /tmp/kabi-headless.conf /tmp/kabi-headless.log /tmp/kabi-headless-sway.log /tmp/kabi-nested.conf /tmp/kabi-nested.log && rm -rf /tmp/kabi-nested-config`

Step 8 では `/etc/pam.d` を変えていないので、PAM の後片付けはない。

---

## 契約への補足（この計画で足したもの・決めたもの）

契約の公開 API は変えていない。次は契約に書かれていなかったので、この計画で足した。

- `LockOutput` に `pub(crate) output: wl_output::WlOutput` を足した（`output_destroyed` で抜かれた出力を探すため）。
- `ShmBlack::new()` と `Default` を足した（契約に作り方がなかった）。Plan 3 は EGL 失敗時に `OutputRender::Shm(ShmBlack::new())` で作る。
- `AppError` は `Clone, Debug, PartialEq, Eq`、`Display`、`std::error::Error` を実装する。
- `impl From<AuthEvent> for state::Event`、`auth::current_user()` を足した。`App` に `user: String`（Plan 5 の U2F 用に置いたが、Plan 5 の helper はユーザー名を自分で取るので、Plan 5 Task 6 で外す）と `config: Config` を持たせた。
- `OutputMeta::{from_info, from_parts, label, to_output_info}` を足した。
- `input` は `Keysym` を再輸出する。`classify` は制御文字を 1 つでも含む文字列を `Ignore` にする（契約の「制御文字だけ」より厳しい）。
- `App` のフィールドはすべて `pub(crate)`。`App` には `first_configure_ms: Option<u64>`（契約 5 章のフェードの起点）、`release_pending`、`keyboards`、`ctrl`、`exit` がある。
- 描画の入口 `draw_output` は `match out.render.as_mut()` で分岐する。Plan 3 はここに `Some(OutputRender::Gpu(..))` の分岐を足す。
- `acct_mgmt` の `NEW_AUTHTOK_REQD` は成功として扱う。
- 契約 9 章の手順は、`/etc/pam.d/kabi-lock` がまだない（Plan 6 で入れる）ことに触れていない。Task 12 Step 8 で回避策（一時的な設定ファイルで `pam_service = "login"`。`sudo` を使わない）を書いた。`/etc/pam.d/swaylock` は `account` の行がないので使えない。

## 設計書の項目と、この計画の範囲

**Plan 2 で扱うもの:**

- 2 章: ext-session-lock によるロックと全出力への表示（黒）、PAM によるパスワード認証、出力の抜き挿しへの追従
- 4 章: `smithay-client-toolkit` / `calloop` によるイベントループとキー入力、`pam-client`、`zeroize`、`log` / `env_logger`。
  認証スレッドとイベントループを `calloop::channel` でつなぎ、描画側が認証でブロックされない
- 5 章の 5〜6: `lock` を要求し、返事を待たずに全出力の lock surface を作る。configure が届いた出力から黒を出す
- 6 章: 状態（入力待ち・確認中・失敗・解除中）の遷移、パスワードの入力（xkbcommon の文字列、Backspace、Esc / Ctrl+U、Enter）、
  Enter でバッファの中身を認証スレッドへ渡してメイン側を消去、PAM サービス `auth.pam_service`
- 8 章: Wayland に接続できない・ext-session-lock がない・拒否された（`finished`）ときは終了コード 1。ロック中の出力の増減。
  PAM のエラーは失敗として扱う。パスワードをログに出さず、使用後に `zeroize` で消す（pam-client の `CString` の制約は前述）
- 9 章: 単体テストの追加（`clock`、`input`、`auth`、`wl::outputs`、`render::shm`、`app::AppError`、`main` の引数）と、入れ子の sway での手動確認
- Plan 1 の引き継ぎ「早めにやる」3 件と「Plan 2 で必ず守る」7 件

**Plan 3〜6 に残すもの:**

- Plan 3: wlr-screencopy、EGL、背景のぼかし・ビネット・黒からのフェード、解除時のぼかし解除、EGL 失敗時の `wl_shm` フォールバック（`ShmBlack` を使う）、
  フレームコールバックによる描き直し（`frame_pending`）、スケール 1 以外の出力（`buffer_scale`）、`appearance.blur` の範囲の丸め、
  `reveal_progress = 1.0` のフレームを描いてから `ReleaseLock` で外すこと
- Plan 4: egui による時計・日付・入力バー・光の柱・錠、主画面の選択（`select_primary` と sway IPC の `focused`）と、主画面が抜かれたときの選び直し、
  打鍵ごとの光の柱（`push_str` / `backspace` が `true` のときだけ）、`date_format` の検証、フォント
- Plan 5: pam_u2f のタッチ待ちの子プロセスと見張りスレッド（`AuthEvent::U2fSucceeded` を送る）、KeePassXC 連携（zbus）、通知
- Plan 6: `/etc/pam.d/kabi-lock`、U2F 用 PAM の例、PKGBUILD、README、sway の設定例、設定ファイルの例
- 対応しないまま残すもの: キーリピート、IME、SIGUSR1 による解除（設計書 2 章で v1 の対象外）
