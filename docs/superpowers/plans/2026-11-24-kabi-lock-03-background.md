# kabi-lock Plan 3: 背景 実装計画

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** ロック画面の背景を設計どおりにする。ロック直前のスクリーンショットを縮小してぼかし、ビネットをかけ、黒からフェードインし、解除のときにぼかしが晴れて元の画面に戻ってからロックを外す。EGL が使えないときは Plan 2 の `wl_shm` の黒画面でロックを続ける。

**Architecture:** ロック要求の前に、別スレッド・別の Wayland 接続で wlr-screencopy の撮影を始め、並行して main スレッドで EGL をサーフェスなしで準備する（`Gpu::new`）。撮影が終わったら画像を GPU に送り、縮小 → 横のガウス → 縦のガウスの 3 パスでぼかしたテクスチャを出力名ごとに作っておく（`Background::upload`）。ロック後は、出力ごとに `wl_egl_window` の EGL サーフェス（`GpuSurface`）を作り、ぼかし・元画像・フェード・解除の進み具合を 1 回の全画面パスで合成する。フェード中と解除中はフレームコールバックで描き直し、Plan 2 の 50ms の `Tick` で止まった連鎖を再開する。

**Tech Stack:** Rust（edition 2024、rustc 1.100）、`wayland-protocols-wlr` 0.3.12（screencopy）、`khronos-egl` 6.0.0（`static`）、`wayland-egl` 0.32.11、`glow` 0.17（GLSL ES 3.00）、Plan 2 の `smithay-client-toolkit` 0.21.1 / `wayland-client` 0.31.15 / calloop（SCTK の再輸出）

**Spec:** `docs/superpowers/specs/2026-11-24-kabi-lock-design.md`（3 章・5 章・8 章）
**Contract:** `docs/superpowers/plans/2026-11-24-kabi-lock-contract.md`（4 章「Plan 3 が作るもの」、5 章、9 章）

## 前提（Plan 2 が終わっていること）

この計画は Plan 2 の実装の後に実行する。Plan 2 の公開 API は契約 4 章のとおり。`src/app.rs` は Plan 2 の実際のコードに対して書いている。
この計画が使う Plan 2 の名前:

- `App` のフィールド: `conn`、`qh`、`config`、`shm`、`lock`、`release_pending`、`outputs: Vec<LockOutput>`、`machine`、`clock`、`first_configure_ms: Option<u64>`（最初の configure の時刻。フェードはここから測る）
- `LockOutput` のフィールド: `output`、`meta`、`lock_surface`、`size`、`render`、`frame_pending`
- `App` のメソッド: `draw_output`、`add_output`、`release_lock`、`handle_event`。`ShmBlack::new()`
- `run` の中: 出力の名前を受け取る `queue.roundtrip`、`WaylandSource::new(conn, queue)`、`Timer::from_duration(TICK)` のコールバック、`lock_state.lock(&qh)`

## Global Constraints

- Plan 1 と同じ: edition 2024、Conventional Commits（本文は英語）、末尾に書いたモデルの `Co-Authored-By` 行（例の `Claude Opus 5.5` は書いたモデルの名前に置き換える）、原則 1 コミット 1 ファイル、`git commit` の前に `git status` と `git diff`。1 ファイルに別々の変更を入れるときは、変更ごとにコミットを分ける（この計画ではステップごとにコミットする）。
- コード中のコメントは日本語、ログは英語（`log` クレート）。パスワードに関わる情報はログに出さない（この計画では扱わない）。
- `unsafe` は `src/render/` と `src/wl/` の中だけ。`src/wl/screencopy.rs` は `unsafe` を使わないので `#![forbid(unsafe_code)]` を付ける。`unsafe` ブロックには `// SAFETY:` か、ブロックの目的を書いたコメントを付ける。
- **本物のセッションをロックしない。** 動作確認は入れ子の sway（契約 9 章）で行う。`#[ignore]` の付いたテストは今のセッションのスクリーンショットを撮ったり EGL を作ったりするが、ロックはしない（撮った画像はメモリの中だけで使い、保存しない）。
- 契約の公開 API（`Screenshot`、`spawn_capture_all`、`Gpu::{new, gl}`、`GpuSurface::{new, resize, make_current, swap}`、`Background::{new, upload, draw}`、`OutputRender::Gpu`）の形を変えない。この計画で足す公開の補助（`GpuSurface::size`、`background` の純粋関数など）は Plan 3 の中だけで使う。

### 決めたこと（見た目の対応）

- **ぼかしの強さ:** `appearance.blur` は 0.5〜50 に丸める（NaN・負は 7）。swaylock-effects の `--effect-blur <blur>x5`（半径 `blur` の箱型ぼかしを 5 回）と同じ分散のガウスにする。箱 1 回の分散は `((2r+1)² − 1) / 12 = r(r+1)/3` なので、スクリーンショットの画素で測った σ は `sqrt(5·blur·(blur+1)/3)`。初期値 `blur = 7` で σ ≈ 9.66px（以前使っていた `--effect-blur 7x5` と同じ）。設計書 7 章の設定例のコメント「縮小後のガウスの σ 相当」とは違う対応にしている（設計書の意図は「以前の見た目」なので、こちらを優先する）。
- **縮小:** 倍率は `floor(σ / 2)` を 1〜4 に丸めた整数（初期値では 1/4）。元画像のミップマップから箱型に平均して縮小し、縮小後の画素で σ / 倍率 のガウスを横・縦の 2 パスでかける。半径は `ceil(3σ)`（最大 64、`blur = 50` でも 49）。
- **ビネット:** swaylock-effects の `--effect-vignette 0.5:0.5` と同じ式 `clamp(0.5 + 0.5·16·x·y·(1−x)·(1−y), 0, 1)`（中央 1.0、四辺 0.5）。
- **フェード:** `色 = ぼかし × ビネット × fade`（黒の上に重ねるのと同じ）。`fade = anim::fade_alpha(最初の configure からの経過, fade_in_ms)`。最初の configure の時刻は Plan 2 の `App::first_configure_ms` を使う（全体で 1 つ）。
- **解除:** `色 = mix(ぼかし × ビネット × fade, 元画像, reveal)`、`reveal = anim::reveal_progress(解除開始からの経過)`。`reveal = 1` で元画像そのもの（ビネットもフェードも掛からない）になるので、フェード中に U2F で解除されても、最後のフレームは元の画面と同じになる。`ReleaseLock` の直前にこのフレームを 1 枚描いてからロックを外す（Plan 1 の引き継ぎ）。
- **形式:** wl_shm の形式はリトルエンディアンの 32 ビット値なので、`Xrgb8888` / `Argb8888` はメモリ上 B, G, R, X。GLES3 には BGRA の形式がないので RGBA として送り、テクスチャのスウィズルで赤と青を入れ替える。`Xbgr8888` / `Abgr8888`（この開発機の sway が実際に返す形式）はそのまま。10 ビットの `X/A RGB/BGR 2101010` も `RGB10_A2` で扱う。それ以外の形式は警告してその出力を黒にする。
- **向きとスケール:** screencopy の画像は出力の物理的な向き・バッファの画素数で来る。撮影スレッドで出力の transform（8 種類）とフレームの `Y_INVERT` を戻し、論理的な向きにそろえる（grim の出力と headless の sway で画素単位で一致することを確認済み）。lock surface は `buffer_scale` を出力のスケールにし、EGL サーフェスを「論理サイズ × スケール」の画素で作る。スケールは SCTK の `scale_factor_changed` で受け取る（最初の 1 枚はスケール 1 で描くが、フェードの最初なので黒で見えない）。

### 扱わないこと

- 分数スケール（`wp_fractional_scale_v1`）。sway が切り上げた整数スケールで描き、sway が縮小する。
- 出力のモードがロック中に変わった場合の撮り直し。サイズが変わっても古いスクリーンショットを引き伸ばして使う。
- ロック中に増えた出力の背景（黒。設計書 8 章のとおり）。消えた出力のテクスチャは捨てない（小さなリーク）。
- dmabuf だけを返すコンポジター（screencopy v3 の `linux_dmabuf`）。v2 までで接続するので sway では起きない。
- ぼかしの強さを出力のスケールに合わせること（σ はスクリーンショットの画素で測る。スケール 2 の出力では論理ピクセルで見て半分の強さになる）。
- `Gpu` と `Background` の後片付け（プロセスの終わりまで使う）。

## ファイル構成

| ファイル | 責務 | 変更 |
|---|---|---|
| `Cargo.toml`, `Cargo.lock` | `wayland-protocols-wlr`, `khronos-egl`, `wayland-egl`, `glow` を足す | Modify |
| `src/wl/mod.rs` | `pub mod screencopy;` | Modify |
| `src/wl/screencopy.rs` | 撮影スレッド、向きの補正、扱える形式の判定 | Create |
| `src/render/mod.rs` | `pub mod egl; pub mod background;`、`OutputRender::Gpu` | Modify |
| `src/render/egl.rs` | `Gpu`（EGL の準備）と `GpuSurface`（出力ごとの EGL サーフェス） | Create |
| `src/render/background.rs` | ぼかしの計算・形式の対応・フェードの開始時刻（純粋）と、シェーダー・アップロード・描画（GPU） | Create |
| `src/app.rs` | `LockOutput` と `App` の拡張（`render` の位置・`scale`・`gpu`・`background`）、起動順、描画・フレームコールバック・スケール・解除前の最後のフレーム | Modify |

テストは各モジュールの末尾の `#[cfg(test)] mod tests` に置く。GPU と Wayland に触れるテストは `#[ignore]` にして、手動で `cargo test -- --ignored` で実行する。

---

### Task 1: 依存の追加とモジュールの雛形

**Files:**
- Modify: `Cargo.toml`, `Cargo.lock`, `src/wl/mod.rs`, `src/render/mod.rs`
- Create: `src/wl/screencopy.rs`, `src/render/egl.rs`, `src/render/background.rs`

**Interfaces:**
- Consumes: Plan 2 の `src/wl/mod.rs`、`src/render/mod.rs`
- Produces: モジュール `crate::wl::screencopy`、`crate::render::egl`、`crate::render::background`（中身は空）。依存クレート 4 つ。

- [ ] **Step 1: 依存クレートを追加する**

Run（リポジトリのルートで）:

```bash
cargo add wayland-protocols-wlr@0.3.12 --features client
cargo add khronos-egl@6.0.0 --features static
cargo add wayland-egl@0.32.11
cargo add glow@0.17
```

Expected: `Cargo.toml` の `[dependencies]` に次の 4 行が入る（Plan 2 の依存はそのまま）。

```toml
glow = "0.17"
khronos-egl = { version = "6.0.0", features = ["static"] }
wayland-egl = "0.32.11"
wayland-protocols-wlr = { version = "0.3.12", features = ["client"] }
```

`khronos-egl` の `static` は `libEGL.so` にリンクする（Arch では `libglvnd` が入れる）。`wayland-egl` は `wayland-client` の `client_system` を有効にするので、`conn.backend().display_ptr()` が使えるようになる。

- [ ] **Step 2: 空のモジュールファイルを作る**

`src/wl/screencopy.rs`:

```rust
//! ロック前のスクリーンショット（wlr-screencopy）。
```

`src/render/egl.rs`:

```rust
//! EGL の準備（`Gpu`）と、出力ごとの EGL サーフェス（`GpuSurface`）。
```

`src/render/background.rs`:

```rust
//! 背景: スクリーンショットの縮小ぼかし・黒からのフェード・ビネット・解除時のぼかし解除。
```

- [ ] **Step 3: モジュールを宣言する**

`src/wl/mod.rs` の既存の `pub mod` の並びに（アルファベット順の位置に）足す:

```rust
pub mod screencopy;
```

`src/render/mod.rs` の既存の `pub mod shm;` の並びに足す（`OutputRender` の変更は Task 7 で行う）:

```rust
pub mod background;
pub mod egl;
```

- [ ] **Step 4: ビルドを確かめる**

Run: `cargo build && cargo test`
Expected: エラーなくビルドでき、既存のテストがすべて通る（足した依存は Task 3（`wayland-protocols-wlr`）と Task 4（ほかの 3 つ）で使い始める。まだ使っていない依存について cargo は警告しない）。

- [ ] **Step 5: コミットする**

```bash
git status
git diff
git add Cargo.toml Cargo.lock
git commit -m "chore: add screencopy, EGL and glow dependencies

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git add src/wl/mod.rs src/wl/screencopy.rs src/render/mod.rs src/render/egl.rs src/render/background.rs
git commit -m "chore: scaffold screencopy, egl and background modules

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

（2 つ目は複数ファイルだが、空モジュールの作成と宣言という 1 つの操作なのでまとめる。）

---

### Task 2: スクリーンショットの向きと形式（純粋な部分）

**Files:**
- Modify: `src/wl/screencopy.rs`

**Interfaces:**
- Consumes: `wayland_client::protocol::{wl_output, wl_shm}`
- Produces:
  - `pub(crate) fn is_supported(format: wl_shm::Format) -> bool`（1 画素 4 バイトの 8 形式なら `true`）
  - `pub(crate) fn normalize(src: &[u8], width: u32, height: u32, stride: u32, transform: wl_output::Transform, y_invert: bool) -> (u32, u32, u32, Vec<u8>)`（論理的な向きに直した（幅, 高さ, stride, 画素））

- [ ] **Step 1: 失敗するテストを書く**

`src/wl/screencopy.rs` を次の内容にする（実装はまだ書かない）:

```rust
//! ロック前のスクリーンショット（wlr-screencopy）。
//!
//! メインの接続とは別の Wayland 接続を別スレッドで開き、全出力を同時に撮る。
//! 返す画像は「論理的な向き・1 行目が画面の上」にそろえてある
//! （出力の transform と、フレームの Y_INVERT をここで戻す）。
#![forbid(unsafe_code)]

use wayland_client::protocol::{wl_output, wl_shm};

#[cfg(test)]
mod tests {
    use super::*;
    use wl_output::Transform as T;

    /// 幅 3・高さ 2 の画像。画素の 1 バイト目に番号を入れる（a=1 … f=6）。
    ///
    /// ```text
    /// a b c
    /// d e f
    /// ```
    fn image(stride: u32) -> Vec<u8> {
        let mut v = vec![0u8; stride as usize * 2];
        for (i, n) in (1u8..=6).enumerate() {
            let (x, y) = (i % 3, i / 3);
            v[y * stride as usize + x * 4] = n;
        }
        v
    }

    /// 結果の画素番号を行ごとに並べる。
    fn ids(w: u32, h: u32, stride: u32, px: &[u8]) -> Vec<Vec<u8>> {
        (0..h as usize)
            .map(|y| {
                (0..w as usize)
                    .map(|x| px[y * stride as usize + x * 4])
                    .collect()
            })
            .collect()
    }

    fn run(transform: T, y_invert: bool) -> (u32, u32, Vec<Vec<u8>>) {
        let (w, h, s, px) = normalize(&image(12), 3, 2, 12, transform, y_invert);
        (w, h, ids(w, h, s, &px))
    }

    #[test]
    fn normal_keeps_pixels_and_stride() {
        let (w, h, s, px) = normalize(&image(16), 3, 2, 16, T::Normal, false);
        assert_eq!((w, h, s), (3, 2, 16));
        assert_eq!(ids(w, h, s, &px), vec![vec![1, 2, 3], vec![4, 5, 6]]);
    }

    #[test]
    fn y_invert_flips_rows() {
        assert_eq!(
            run(T::Normal, true),
            (3, 2, vec![vec![4, 5, 6], vec![1, 2, 3]])
        );
    }

    #[test]
    fn rotate_90_turns_clockwise() {
        // 上の行 a b c が右の列になる
        assert_eq!(
            run(T::_90, false),
            (2, 3, vec![vec![4, 1], vec![5, 2], vec![6, 3]])
        );
    }

    #[test]
    fn rotate_180() {
        assert_eq!(
            run(T::_180, false),
            (3, 2, vec![vec![6, 5, 4], vec![3, 2, 1]])
        );
    }

    #[test]
    fn rotate_270_turns_counter_clockwise() {
        assert_eq!(
            run(T::_270, false),
            (2, 3, vec![vec![3, 6], vec![2, 5], vec![1, 4]])
        );
    }

    #[test]
    fn flipped_mirrors_horizontally() {
        assert_eq!(
            run(T::Flipped, false),
            (3, 2, vec![vec![3, 2, 1], vec![6, 5, 4]])
        );
    }

    #[test]
    fn flipped_90_is_a_transpose() {
        assert_eq!(
            run(T::Flipped90, false),
            (2, 3, vec![vec![1, 4], vec![2, 5], vec![3, 6]])
        );
    }

    #[test]
    fn flipped_270_is_an_anti_transpose() {
        assert_eq!(
            run(T::Flipped270, false),
            (2, 3, vec![vec![6, 3], vec![5, 2], vec![4, 1]])
        );
    }

    #[test]
    fn y_invert_applies_before_rotation() {
        assert_eq!(
            run(T::_90, true),
            (2, 3, vec![vec![1, 4], vec![2, 5], vec![3, 6]])
        );
    }

    #[test]
    fn supported_formats_are_32_bit() {
        assert!(is_supported(wl_shm::Format::Xrgb8888));
        assert!(is_supported(wl_shm::Format::Argb8888));
        assert!(is_supported(wl_shm::Format::Xbgr2101010));
        assert!(!is_supported(wl_shm::Format::Rgb565));
        assert!(!is_supported(wl_shm::Format::Rgb888));
    }
}
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test screencopy`
Expected: コンパイルエラー（`cannot find function normalize` など）。

- [ ] **Step 3: 実装を書く**

`use` の行とテストモジュールの間に追加する:

```rust
/// 32 ビット / 画素で、背景の描画側（`render::background`）が扱える形式か。
pub(crate) fn is_supported(format: wl_shm::Format) -> bool {
    use wl_shm::Format as F;
    matches!(
        format,
        F::Xrgb8888
            | F::Argb8888
            | F::Xbgr8888
            | F::Abgr8888
            | F::Xrgb2101010
            | F::Argb2101010
            | F::Xbgr2101010
            | F::Abgr2101010
    )
}

/// 撮った 1 画素 4 バイトの画像を、論理的な向き（1 行目が画面の上）に直す。
///
/// screencopy の画像は出力の物理的な向きのままなので、transform の逆をかける。
/// 90 は時計回りに 90°、270 は 270° 回して戻す。flipped の 4 つは左右反転してから
/// 回すが、反転を挟むと回す向きが逆になるので flipped-90 は 270°、flipped-270 は 90° 回す
/// （grim の出力と画素単位で一致することを headless の sway で確かめた）。
/// `y_invert` は回転の前に上下を返す。
/// 返り値は（幅, 高さ, stride, 画素）。向きが変わらなければ元の stride を保つ。
pub(crate) fn normalize(
    src: &[u8],
    width: u32,
    height: u32,
    stride: u32,
    transform: wl_output::Transform,
    y_invert: bool,
) -> (u32, u32, u32, Vec<u8>) {
    use wl_output::Transform as T;
    let (w, h, s) = (width as usize, height as usize, stride as usize);
    let row = w * 4;

    if transform == T::Normal {
        if !y_invert {
            return (width, height, stride, src[..s * h].to_vec());
        }
        let mut out = vec![0u8; s * h];
        for y in 0..h {
            let from = (h - 1 - y) * s;
            out[y * s..y * s + row].copy_from_slice(&src[from..from + row]);
        }
        return (width, height, stride, out);
    }

    let flipped = matches!(
        transform,
        T::Flipped | T::Flipped90 | T::Flipped180 | T::Flipped270
    );
    let quarter_turns = match transform {
        T::_90 | T::Flipped270 => 1,
        T::_180 | T::Flipped180 => 2,
        T::_270 | T::Flipped90 => 3,
        _ => 0,
    };
    let (lw, lh) = if quarter_turns % 2 == 1 {
        (h, w)
    } else {
        (w, h)
    };
    let mut out = vec![0u8; lw * lh * 4];
    for by in 0..h {
        for bx in 0..w {
            let fx = if flipped { w - 1 - bx } else { bx };
            let fy = if y_invert { h - 1 - by } else { by };
            // 画面の座標（y は下向き）で時計回りに 90° ずつ回す
            let (lx, ly) = match quarter_turns {
                1 => (h - 1 - fy, fx),
                2 => (w - 1 - fx, h - 1 - fy),
                3 => (fy, w - 1 - fx),
                _ => (fx, fy),
            };
            let from = by * s + bx * 4;
            let to = (ly * lw + lx) * 4;
            out[to..to + 4].copy_from_slice(&src[from..from + 4]);
        }
    }
    (lw as u32, lh as u32, (lw * 4) as u32, out)
}
```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test screencopy`
Expected: `test result: ok. 10 passed`。`is_supported` と `normalize` が未使用という `dead_code` の警告は Task 3 で消える。

- [ ] **Step 5: コミットする**

```bash
git status
git diff
git add src/wl/screencopy.rs
git commit -m "feat: normalize screenshot orientation and list supported formats

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: スクリーンショットの撮影スレッド

**Files:**
- Modify: `src/wl/screencopy.rs`

**Interfaces:**
- Consumes: Task 2 の `is_supported` / `normalize`、SCTK の `registry::SimpleGlobal` / `shm::raw::RawPool` / `reexports::calloop` / `reexports::calloop_wayland_source::WaylandSource`、`wayland-protocols-wlr` の screencopy
- Produces（契約 4 章）:
  - `pub struct Screenshot { pub output_name: String, pub width: u32, pub height: u32, pub stride: u32, pub format: wl_shm::Format, pub pixels: Vec<u8> }`（1 行目が画面の上、論理的な向き、バッファの画素数）
  - `pub fn spawn_capture_all() -> std::thread::JoinHandle<Vec<Screenshot>>`

**作りの要点:**
- 新しい `Connection` を開き、`wl_output` を v4 で全部 bind して `name` と `transform` を集める（SCTK の `OutputState` は使わない。この接続はこのスレッドだけのもの）。
- screencopy マネージャーは v2 まで（v3 の `buffer_done` / dmabuf を扱わないため）。全出力の `capture_output` を同時に出し、`buffer` イベントを受けたらその場で `RawPool`（memfd + mmap、SCTK 内部で作る）にバッファを作って `copy` する。
- 待つのは calloop の `EventLoop::dispatch` に締め切り（1 秒）を渡して行う。コンポジターが返事をしなくても、撮れた分だけ返してロックを進める。
- 名前のない出力（`wl_output` < v4）・扱えない形式・`failed` は警告してスキップする（呼び出し側でその出力は黒）。

- [ ] **Step 1: `use` を置き換える**

Task 2 で書いた `use wayland_client::protocol::{wl_output, wl_shm};` の 1 行を、次に置き換える:

```rust
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use log::{debug, warn};
use smithay_client_toolkit::reexports::calloop::EventLoop;
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::registry::SimpleGlobal;
use smithay_client_toolkit::shm::raw::RawPool;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_buffer, wl_output, wl_registry, wl_shm};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};
```

- [ ] **Step 2: `Screenshot` と `spawn_capture_all` を足す**

`use` の後、`is_supported` の doc コメントの前に追加する:

```rust
/// 撮影全体の締め切り。過ぎたら撮れた分だけ返す（ロックを遅らせないため）。
const CAPTURE_TIMEOUT: Duration = Duration::from_millis(1000);

/// 1 出力分のスクリーンショット。
///
/// `pixels` の 1 行目が画面の上。1 画素 4 バイトで、並びは `format` のとおり
/// （`Xrgb8888` / `Argb8888` ならメモリ上は B, G, R, X）。`stride` は 1 行のバイト数。
/// `width` / `height` はバッファの画素数（スケール 2 の出力なら論理サイズの 2 倍）。
pub struct Screenshot {
    pub output_name: String,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: wl_shm::Format,
    pub pixels: Vec<u8>,
}

/// 別スレッドで新しい Wayland 接続を開き、全出力を撮る。出力ごとに失敗しうる。
///
/// 失敗した出力は結果に入らない（呼び出し側はその出力の背景を黒にする）。
pub fn spawn_capture_all() -> JoinHandle<Vec<Screenshot>> {
    std::thread::Builder::new()
        .name("kabi-capture".into())
        .spawn(|| {
            let started = Instant::now();
            match capture_all() {
                Ok(shots) => {
                    debug!(
                        "captured {} screenshot(s) in {:?}",
                        shots.len(),
                        started.elapsed()
                    );
                    shots
                }
                Err(e) => {
                    warn!("screenshots unavailable, background will be black: {e}");
                    Vec::new()
                }
            }
        })
        .expect("failed to spawn the screenshot thread")
}
```

- [ ] **Step 3: Wayland の処理を足す**

`normalize` の後、`#[cfg(test)]` の前に追加する:

```rust
// ---- ここから Wayland の処理（撮影スレッドの中だけで使う） ----

struct OutputSlot {
    output: wl_output::WlOutput,
    name: Option<String>,
    transform: wl_output::Transform,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FrameState {
    Waiting,
    Ready,
    Failed,
}

struct FrameSlot {
    output: usize,
    frame: ZwlrScreencopyFrameV1,
    /// (format, width, height, stride)
    info: Option<(wl_shm::Format, u32, u32, u32)>,
    pool: Option<RawPool>,
    buffer: Option<wl_buffer::WlBuffer>,
    y_invert: bool,
    state: FrameState,
}

struct Capture {
    shm: SimpleGlobal<wl_shm::WlShm, 1>,
    outputs: Vec<OutputSlot>,
    frames: Vec<FrameSlot>,
}

fn capture_all() -> Result<Vec<Screenshot>, String> {
    let conn = Connection::connect_to_env().map_err(|e| format!("connect: {e}"))?;
    let (globals, mut queue) =
        registry_queue_init::<Capture>(&conn).map_err(|e| format!("registry: {e}"))?;
    let qh = queue.handle();
    // バージョン 3 の linux_dmabuf / buffer_done は使わないので 2 までにする
    let manager: ZwlrScreencopyManagerV1 = globals
        .bind(&qh, 1..=2, ())
        .map_err(|e| format!("zwlr_screencopy_manager_v1: {e}"))?;
    let shm: wl_shm::WlShm = globals
        .bind(&qh, 1..=1, ())
        .map_err(|e| format!("wl_shm: {e}"))?;

    // 出力の名前（wl_output v4 の name）と transform を集める
    let output_globals: Vec<(u32, u32)> = globals.contents().with_list(|list| {
        list.iter()
            .filter(|g| g.interface == "wl_output")
            .map(|g| (g.name, g.version))
            .collect()
    });
    let mut state = Capture {
        shm: SimpleGlobal::from_bound(shm),
        outputs: Vec::new(),
        frames: Vec::new(),
    };
    for (index, (name, version)) in output_globals.into_iter().enumerate() {
        let output: wl_output::WlOutput = globals.registry().bind(name, version.min(4), &qh, index);
        state.outputs.push(OutputSlot {
            output,
            name: None,
            transform: wl_output::Transform::Normal,
        });
    }
    queue
        .roundtrip(&mut state)
        .map_err(|e| format!("roundtrip: {e}"))?;

    for index in 0..state.outputs.len() {
        if state.outputs[index].name.is_none() {
            warn!("an output has no name (wl_output < v4), skipping its screenshot");
            continue;
        }
        let frame =
            manager.capture_output(0, &state.outputs[index].output, &qh, state.frames.len());
        state.frames.push(FrameSlot {
            output: index,
            frame,
            info: None,
            pool: None,
            buffer: None,
            y_invert: false,
            state: FrameState::Waiting,
        });
    }

    // 締め切りつきでイベントを回す（コンポジターが返事をしなくても止まらない）
    let mut event_loop: EventLoop<Capture> =
        EventLoop::try_new().map_err(|e| format!("event loop: {e}"))?;
    WaylandSource::new(conn.clone(), queue)
        .insert(event_loop.handle())
        .map_err(|e| format!("wayland source: {e}"))?;
    let deadline = Instant::now() + CAPTURE_TIMEOUT;
    while state.frames.iter().any(|f| f.state == FrameState::Waiting) {
        let now = Instant::now();
        if now >= deadline {
            warn!("screenshot timed out, missing outputs will be black");
            break;
        }
        event_loop
            .dispatch(deadline - now, &mut state)
            .map_err(|e| format!("dispatch: {e}"))?;
    }

    let mut shots = Vec::new();
    for slot in &mut state.frames {
        let output = &state.outputs[slot.output];
        let name = output.name.clone().unwrap_or_default();
        if let (FrameState::Ready, Some((format, width, height, stride)), Some(pool)) =
            (slot.state, slot.info, slot.pool.as_mut())
        {
            let len = stride as usize * height as usize;
            let (width, height, stride, pixels) = normalize(
                &pool.mmap()[..len],
                width,
                height,
                stride,
                output.transform,
                slot.y_invert,
            );
            debug!("screenshot of {name}: {width}x{height} {format:?}");
            shots.push(Screenshot {
                output_name: name,
                width,
                height,
                stride,
                format,
                pixels,
            });
        } else if slot.state == FrameState::Failed {
            warn!("screenshot of {name} failed, its background will be black");
        }
        if let Some(buffer) = slot.buffer.take() {
            buffer.destroy();
        }
        slot.frame.destroy();
    }
    manager.destroy();
    let _ = conn.flush();
    Ok(shots)
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Capture {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // 撮影中の出力の増減は扱わない
    }
}

impl Dispatch<wl_output::WlOutput, usize> for Capture {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let slot = &mut state.outputs[*index];
        match event {
            wl_output::Event::Name { name } => slot.name = Some(name),
            wl_output::Event::Geometry {
                transform: WEnum::Value(t),
                ..
            } => slot.transform = t,
            _ => {}
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, usize> for Capture {
    fn event(
        state: &mut Self,
        frame: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        index: &usize,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let output_name = state.outputs[state.frames[*index].output]
            .name
            .clone()
            .unwrap_or_default();
        let slot = &mut state.frames[*index];
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                if slot.buffer.is_some() {
                    return;
                }
                let format = match format {
                    WEnum::Value(f) if is_supported(f) => f,
                    other => {
                        warn!(
                            "screenshot of {output_name}: unsupported format {other:?}, using black"
                        );
                        slot.state = FrameState::Failed;
                        return;
                    }
                };
                if width == 0 || height == 0 || stride < width * 4 {
                    warn!(
                        "screenshot of {output_name}: bad buffer {width}x{height} stride {stride}"
                    );
                    slot.state = FrameState::Failed;
                    return;
                }
                let len = stride as usize * height as usize;
                let mut pool = match RawPool::new(len, &state.shm) {
                    Ok(pool) => pool,
                    Err(e) => {
                        warn!("screenshot of {output_name}: shm pool: {e}");
                        slot.state = FrameState::Failed;
                        return;
                    }
                };
                let buffer = pool.create_buffer(
                    0,
                    width as i32,
                    height as i32,
                    stride as i32,
                    format,
                    (),
                    qh,
                );
                frame.copy(&buffer);
                slot.info = Some((format, width, height, stride));
                slot.pool = Some(pool);
                slot.buffer = Some(buffer);
            }
            zwlr_screencopy_frame_v1::Event::Flags {
                flags: WEnum::Value(flags),
            } => {
                slot.y_invert = flags.contains(zwlr_screencopy_frame_v1::Flags::YInvert);
            }
            zwlr_screencopy_frame_v1::Event::Ready { .. } => slot.state = FrameState::Ready,
            zwlr_screencopy_frame_v1::Event::Failed => slot.state = FrameState::Failed,
            _ => {}
        }
    }
}

wayland_client::delegate_noop!(Capture: ZwlrScreencopyManagerV1);
wayland_client::delegate_noop!(Capture: ignore wl_shm::WlShm);
wayland_client::delegate_noop!(Capture: ignore wl_buffer::WlBuffer);
```

- [ ] **Step 4: 今のセッションで撮れることを確かめるテストを足す**

テストモジュールの末尾（最後の `}` の前）に追加する:

```rust
    /// 今のセッションの全出力を撮る（ロックはしない）。`cargo test -- --ignored` で手動実行。
    #[test]
    #[ignore = "needs a running wlroots compositor"]
    fn captures_the_current_session() {
        let shots = spawn_capture_all().join().expect("thread");
        assert!(!shots.is_empty());
        for s in &shots {
            assert!(!s.output_name.is_empty());
            assert!(s.pixels.len() >= (s.stride * (s.height - 1) + s.width * 4) as usize);
            eprintln!(
                "{}: {}x{} stride {} {:?}",
                s.output_name, s.width, s.height, s.stride, s.format
            );
        }
    }
```

- [ ] **Step 5: ビルド・テスト・clippy**

Run: `cargo build && cargo test screencopy && cargo clippy --all-targets`
Expected: `test result: ok. 10 passed; 0 failed; 1 ignored`。clippy の警告なし（`dead_code` を除く。`spawn_capture_all` は Task 8 で使う）。

- [ ] **Step 6: 今のセッションで撮ってみる（ロックはしない）**

Run（sway のセッションの端末で）: `RUST_LOG=debug cargo test captures_the_current_session -- --ignored --nocapture`
Expected: 出力ごとに `DP-10: 3440x1440 stride 13760 Xbgr8888` のような行が出て `1 passed`。この開発機の sway は `Xbgr8888` を返す。撮影は 3440x1440 + 1920x1080 で 30ms 前後（debug ビルド）。

- [ ] **Step 7: コミットする**

```bash
git status
git diff
git add src/wl/screencopy.rs
git commit -m "feat: capture all outputs with wlr-screencopy on a separate connection

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: EGL（`Gpu` と `GpuSurface`）

**Files:**
- Modify: `src/render/egl.rs`

**Interfaces:**
- Consumes: `khronos-egl`（`Static`）、`wayland-egl`、`glow`、`wayland_client::{Connection, Proxy, protocol::wl_surface}`
- Produces（契約 4 章 + 補助）:
  - `pub struct Gpu`、`Gpu::new(conn: &Connection) -> Result<Gpu, String>`、`Gpu::gl(&self) -> &Arc<glow::Context>`
  - `pub struct GpuSurface`、`GpuSurface::new(gpu: &Gpu, surface: &wl_surface::WlSurface, width: u32, height: u32) -> Result<GpuSurface, String>`（`width` / `height` はバッファの画素数）、`resize(&mut self, width: u32, height: u32)`、`make_current(&self, gpu: &Gpu)`、`swap(&self, gpu: &Gpu)`
  - 補助: `GpuSurface::size(&self) -> (u32, u32)`

**作りの要点:**
- 試作（`docs/superpowers/spikes/2026-11-24-api-spike.rs`）と同じ手順: `get_platform_display(0x31D8, display_ptr)` → `initialize` → `bind_api(OPENGL_ES_API)` → RGBA8・ES3・WINDOW の config → ES 3 のコンテキスト → サーフェスなしで `make_current`（`EGL_KHR_surfaceless_context`）→ `glow::Context::from_loader_function`。途中で失敗したら `terminate` して `Err`。
- `GpuSurface::new` は current にしてから `swap_interval(0)`（複数の出力を順に描くので swap で待たない。待つのはフレームコールバック）。
- `GpuSurface` の `Drop` で EGL サーフェスを先に壊し、その後フィールドの破棄で `wl_egl_window` を壊す。`wl_surface` より先に捨てる必要があるので、Task 7 で `LockOutput` のフィールド順を変える。
- `Gpu` には `Drop` を付けない（EGL ディスプレイは `wl_display` を指しているので、接続を閉じた後に EGL を呼ばないため。プロセスの終わりまで使う）。

- [ ] **Step 1: 実装を書く**

`src/render/egl.rs` を次の内容にする:

```rust
//! EGL の準備（`Gpu`）と、出力ごとの EGL サーフェス（`GpuSurface`）。
//!
//! `Gpu` はロック要求の前に、サーフェスなしで current にして GL の準備まで済ませる。
//! EGL の関数は main スレッドからだけ呼ぶ。

use std::ffi::c_void;
use std::sync::Arc;
use std::time::Instant;

use glow::HasContext;
use khronos_egl as egl;
use log::{debug, warn};
use wayland_client::{Connection, Proxy, protocol::wl_surface};

/// EGL_PLATFORM_WAYLAND_KHR
const PLATFORM_WAYLAND_KHR: egl::Enum = 0x31D8;

/// EGL ディスプレイ・コンテキストと glow の GL。
///
/// `Drop` は実装しない（プロセスの終わりまで使う）。EGL ディスプレイは `Connection` の
/// `wl_display` を指しているので、接続を閉じた後に EGL を呼ばないようにするため。
pub struct Gpu {
    egl: egl::Instance<egl::Static>,
    display: egl::Display,
    config: egl::Config,
    context: egl::Context,
    gl: Arc<glow::Context>,
}

impl Gpu {
    /// サーフェスなしで current にし、GL の準備まで済ませる。失敗したら Err（呼び出し側は shm に切り替える）。
    pub fn new(conn: &Connection) -> Result<Gpu, String> {
        let started = Instant::now();
        let egl = egl::Instance::new(egl::Static);
        // SAFETY: display_ptr は conn が生きている間有効な wl_display を指す。
        // conn（とその複製）はプロセスの終わりまで閉じない。
        let display = unsafe {
            egl.get_platform_display(
                PLATFORM_WAYLAND_KHR,
                conn.backend().display_ptr() as *mut c_void,
                &[egl::ATTRIB_NONE],
            )
        }
        .map_err(|e| format!("eglGetPlatformDisplay: {e}"))?;
        let (major, minor) = egl
            .initialize(display)
            .map_err(|e| format!("eglInitialize: {e}"))?;
        match Self::create_context(&egl, display) {
            Ok((config, context)) => {
                let gl = Arc::new(unsafe {
                    // SAFETY: コンテキストは current。返る関数ポインタはこのコンテキストで使う。
                    glow::Context::from_loader_function(|name| {
                        egl.get_proc_address(name)
                            .map_or(std::ptr::null(), |p| p as *const c_void)
                    })
                });
                // SAFETY: current なコンテキストへの読み取りだけ。
                let (version, renderer) = unsafe {
                    (
                        gl.get_parameter_string(glow::VERSION),
                        gl.get_parameter_string(glow::RENDERER),
                    )
                };
                debug!(
                    "EGL {major}.{minor} ready in {:?}: {version} on {renderer}",
                    started.elapsed()
                );
                Ok(Gpu {
                    egl,
                    display,
                    config,
                    context,
                    gl,
                })
            }
            Err(e) => {
                let _ = egl.terminate(display);
                Err(e)
            }
        }
    }

    fn create_context(
        egl: &egl::Instance<egl::Static>,
        display: egl::Display,
    ) -> Result<(egl::Config, egl::Context), String> {
        egl.bind_api(egl::OPENGL_ES_API)
            .map_err(|e| format!("eglBindAPI: {e}"))?;
        let config = egl
            .choose_first_config(
                display,
                &[
                    egl::RED_SIZE,
                    8,
                    egl::GREEN_SIZE,
                    8,
                    egl::BLUE_SIZE,
                    8,
                    egl::ALPHA_SIZE,
                    8,
                    egl::RENDERABLE_TYPE,
                    egl::OPENGL_ES3_BIT,
                    egl::SURFACE_TYPE,
                    egl::WINDOW_BIT,
                    egl::NONE,
                ],
            )
            .map_err(|e| format!("eglChooseConfig: {e}"))?
            .ok_or_else(|| "no RGBA8 OpenGL ES 3 EGL config".to_string())?;
        let context = egl
            .create_context(
                display,
                config,
                None,
                &[egl::CONTEXT_MAJOR_VERSION, 3, egl::NONE],
            )
            .map_err(|e| format!("eglCreateContext: {e}"))?;
        // 画面なしで current にする（EGL_KHR_surfaceless_context）。ロック前にシェーダーを作るため
        if let Err(e) = egl.make_current(display, None, None, Some(context)) {
            let _ = egl.destroy_context(display, context);
            return Err(format!("eglMakeCurrent without surface: {e}"));
        }
        Ok((config, context))
    }

    pub fn gl(&self) -> &Arc<glow::Context> {
        &self.gl
    }
}

/// 1 つの lock surface に結び付いた EGL のウィンドウサーフェス。
///
/// 大きさはバッファの画素数（論理サイズ × buffer_scale）。
/// 捨てるときは EGL サーフェスを先に壊し、その後 `wl_egl_window` を壊す。
/// 元の `wl_surface` より先に捨てること（`LockOutput` のフィールド順で保証する）。
pub struct GpuSurface {
    window: wayland_egl::WlEglSurface,
    surface: egl::Surface,
    display: egl::Display,
    size: (u32, u32),
}

impl GpuSurface {
    pub fn new(
        gpu: &Gpu,
        surface: &wl_surface::WlSurface,
        width: u32,
        height: u32,
    ) -> Result<GpuSurface, String> {
        let (width, height) = (width.max(1), height.max(1));
        let window = wayland_egl::WlEglSurface::new(surface.id(), width as i32, height as i32)
            .map_err(|e| format!("wl_egl_window_create: {e}"))?;
        // SAFETY: window.ptr() は window が生きている間有効な wl_egl_window。
        // EGL サーフェスは Drop で window より先に壊す。
        let egl_surface = unsafe {
            gpu.egl.create_window_surface(
                gpu.display,
                gpu.config,
                window.ptr() as egl::NativeWindowType,
                None,
            )
        }
        .map_err(|e| format!("eglCreateWindowSurface: {e}"))?;
        let this = GpuSurface {
            window,
            surface: egl_surface,
            display: gpu.display,
            size: (width, height),
        };
        gpu.egl
            .make_current(
                gpu.display,
                Some(egl_surface),
                Some(egl_surface),
                Some(gpu.context),
            )
            .map_err(|e| format!("eglMakeCurrent: {e}"))?;
        // 複数の出力を順に描くので、swap でフレームを待たない（待つのはフレームコールバック）
        if let Err(e) = gpu.egl.swap_interval(gpu.display, 0) {
            warn!("eglSwapInterval(0) failed: {e}");
        }
        Ok(this)
    }

    /// 今のバッファの大きさ（画素）。
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// 次の swap からバッファの大きさを変える。
    pub fn resize(&mut self, width: u32, height: u32) {
        let (width, height) = (width.max(1), height.max(1));
        self.window.resize(width as i32, height as i32, 0, 0);
        self.size = (width, height);
    }

    pub fn make_current(&self, gpu: &Gpu) {
        if let Err(e) = gpu.egl.make_current(
            gpu.display,
            Some(self.surface),
            Some(self.surface),
            Some(gpu.context),
        ) {
            warn!("eglMakeCurrent failed: {e}");
        }
    }

    /// 描いた内容を出す（wl_surface の commit も含む）。
    pub fn swap(&self, gpu: &Gpu) {
        if let Err(e) = gpu.egl.swap_buffers(gpu.display, self.surface) {
            warn!("eglSwapBuffers failed: {e}");
        }
    }
}

impl Drop for GpuSurface {
    fn drop(&mut self) {
        // Instance<Static> は中身のない型なので、ここで作り直してよい
        let egl = egl::Instance::new(egl::Static);
        if let Err(e) = egl.destroy_surface(self.display, self.surface) {
            warn!("eglDestroySurface failed: {e}");
        }
        // self.window（wl_egl_window）はこの後フィールドの破棄で壊れる
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 今のセッションの Wayland 接続で EGL を準備できるか（ロックはしない）。
    /// `cargo test -- --ignored` で手動実行。
    #[test]
    #[ignore = "needs a running Wayland compositor and a GPU"]
    fn creates_a_surfaceless_context() {
        let conn = Connection::connect_to_env().expect("wayland");
        let gpu = Gpu::new(&conn).expect("gpu");
        let version = unsafe { gpu.gl().get_parameter_string(glow::VERSION) };
        assert!(version.contains("OpenGL ES 3"), "{version}");
    }
}
```

- [ ] **Step 2: ビルド・clippy**

Run: `cargo build && cargo clippy --all-targets`
Expected: エラーなし。`dead_code` 以外の警告なし。

- [ ] **Step 3: 今のセッションで EGL を準備できるか確かめる（ロックはしない）**

Run: `cargo test creates_a_surfaceless_context -- --ignored`
Expected: `1 passed`。

Run（EGL が使えない場合の確認）: `__EGL_VENDOR_LIBRARY_FILENAMES=/nonexistent.json cargo test creates_a_surfaceless_context -- --ignored`
Expected: `gpu: "eglGetPlatformDisplay: One or more argument values are invalid."` で失敗する（glvnd がベンダーを見つけられない。Task 10 の shm への切り替えの確認で同じ方法を使う）。

- [ ] **Step 4: コミットする**

```bash
git status
git diff
git add src/render/egl.rs
git commit -m "feat: set up a surfaceless EGL context and per-output EGL surfaces

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: 背景の計算（純粋な部分）

**Files:**
- Modify: `src/render/background.rs`

**Interfaces:**
- Consumes: `crate::anim::{fade_alpha, reveal_progress, REVEAL_START_MS, REVEAL_MS}`、`crate::state::{Phase, UNLOCK_TOTAL_MS}`、Plan 2 の `Phase::elapsed_ms(&self, now_ms: u64) -> Option<f32>`、Task 2 の `crate::wl::screencopy::is_supported`
- Produces:
  - `pub const DEFAULT_BLUR: f32`（= 7）、`pub const MAX_RADIUS: usize`（= 64）、`pub const MAX_DOWNSCALE: u32`（= 4）
  - `pub fn clamp_blur(blur: f32) -> f32`（0.5〜50、NaN・負は 7）
  - `pub fn sigma_for_blur(blur: f32) -> f32`（スクリーンショットの画素での σ）
  - `pub fn downscale_factor(sigma: f32) -> u32`（1〜4）
  - `pub fn scaled_size(width: u32, height: u32, factor: u32) -> (u32, u32)`
  - `pub fn kernel(sigma: f32) -> Vec<f32>`（中心から外への重み、正規化済み）
  - `pub struct TextureLayout { pub internal_format: u32, pub format: u32, pub ty: u32, pub swap_rb: bool }`、`pub fn texture_layout(format: wl_shm::Format) -> Option<TextureLayout>`
  - `pub fn fade_for(first_configure_ms: Option<u64>, now_ms: u64, fade_in_ms: u32) -> f32`（最初の configure からのフェード。始まる前は 0.0）
  - `pub fn reveal_for(phase: Phase, now_ms: u64) -> f32`（`Unlocking` で進み、`Done` は 1.0）
  - `pub fn needs_frames(fade: f32, phase: Phase) -> bool`（フェード中か解除中）

- [ ] **Step 1: 失敗するテストを書く**

`src/render/background.rs` を次の内容にする（実装はまだ書かない）:

```rust
//! 背景: スクリーンショットの縮小ぼかし・黒からのフェード・ビネット・解除時のぼかし解除。
//!
//! ぼかしはロック前に 1 回だけ作る（縮小 → 横のガウス → 縦のガウス、FBO で 3 パス）。
//! 毎フレームの描画は、ぼかし済みと元画像の 2 枚を 1 回のパスで合成するだけ。

use wayland_client::protocol::wl_shm;

use crate::state::Phase;

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn blur_is_clamped_and_defaults_on_garbage() {
        assert_eq!(clamp_blur(7.0), 7.0);
        assert_eq!(clamp_blur(0.0), 0.5);
        assert_eq!(clamp_blur(0.2), 0.5);
        assert_eq!(clamp_blur(100.0), 50.0);
        assert_eq!(clamp_blur(f32::INFINITY), 50.0);
        assert_eq!(clamp_blur(f32::NAN), DEFAULT_BLUR);
        assert_eq!(clamp_blur(-1.0), DEFAULT_BLUR);
        assert_eq!(clamp_blur(f32::NEG_INFINITY), DEFAULT_BLUR);
    }

    #[test]
    fn default_blur_matches_swaylock_effects_7x5() {
        // 半径 7 の箱型ぼかし 5 回: 分散 5 * (15² - 1) / 12 = 93.33
        assert!(close(sigma_for_blur(7.0), 93.333_f32.sqrt(), 1e-3));
    }

    #[test]
    fn sigma_grows_with_blur() {
        let mut last = 0.0;
        for b in [0.5, 1.0, 3.0, 7.0, 20.0, 50.0] {
            let s = sigma_for_blur(b);
            assert!(s > last);
            last = s;
        }
        assert_eq!(sigma_for_blur(f32::NAN), sigma_for_blur(7.0));
    }

    #[test]
    fn downscale_is_between_1_and_4() {
        assert_eq!(downscale_factor(sigma_for_blur(7.0)), 4);
        assert_eq!(downscale_factor(sigma_for_blur(0.5)), 1);
        assert_eq!(downscale_factor(sigma_for_blur(50.0)), 4);
        assert_eq!(downscale_factor(5.0), 2);
    }

    #[test]
    fn scaled_size_rounds_up() {
        assert_eq!(scaled_size(3440, 1440, 4), (860, 360));
        assert_eq!(scaled_size(1366, 768, 4), (342, 192));
        assert_eq!(scaled_size(1, 1, 4), (1, 1));
        assert_eq!(scaled_size(10, 10, 0), (10, 10));
    }

    #[test]
    fn kernel_is_normalized_and_decreasing() {
        for sigma in [0.3, 1.0, 2.4, 16.3] {
            let k = kernel(sigma);
            let sum = k[0] + 2.0 * k[1..].iter().sum::<f32>();
            assert!(close(sum, 1.0, 1e-4), "sigma {sigma}: sum {sum}");
            assert!(k.windows(2).all(|w| w[0] >= w[1]));
            assert!(k.len() - 1 <= MAX_RADIUS);
        }
    }

    #[test]
    fn kernel_radius_is_three_sigma() {
        assert_eq!(kernel(2.0).len(), 7);
        assert_eq!(kernel(0.0).len(), 2);
        assert_eq!(kernel(1000.0).len(), MAX_RADIUS + 1);
    }

    #[test]
    fn strongest_blur_fits_the_shader_array() {
        let sigma = sigma_for_blur(50.0);
        let k = kernel(sigma / downscale_factor(sigma) as f32);
        assert!(k.len() - 1 < MAX_RADIUS, "radius {}", k.len() - 1);
    }

    #[test]
    fn xrgb_is_swizzled_and_xbgr_is_not() {
        let x = texture_layout(wl_shm::Format::Xrgb8888).unwrap();
        assert_eq!(
            (x.internal_format, x.format, x.ty),
            (glow::RGBA8, glow::RGBA, glow::UNSIGNED_BYTE)
        );
        assert!(x.swap_rb);
        assert!(texture_layout(wl_shm::Format::Argb8888).unwrap().swap_rb);
        assert!(!texture_layout(wl_shm::Format::Xbgr8888).unwrap().swap_rb);
    }

    #[test]
    fn ten_bit_formats_use_rgb10_a2() {
        let x = texture_layout(wl_shm::Format::Xrgb2101010).unwrap();
        assert_eq!(
            (x.internal_format, x.ty),
            (glow::RGB10_A2, glow::UNSIGNED_INT_2_10_10_10_REV)
        );
        assert!(x.swap_rb);
        assert!(!texture_layout(wl_shm::Format::Xbgr2101010).unwrap().swap_rb);
    }

    #[test]
    fn other_formats_are_rejected() {
        assert_eq!(texture_layout(wl_shm::Format::Rgb565), None);
        assert_eq!(texture_layout(wl_shm::Format::Rgb888), None);
    }

    #[test]
    fn every_captured_format_has_a_layout() {
        use wl_shm::Format as F;
        for f in [
            F::Xrgb8888,
            F::Argb8888,
            F::Xbgr8888,
            F::Abgr8888,
            F::Xrgb2101010,
            F::Argb2101010,
            F::Xbgr2101010,
            F::Abgr2101010,
        ] {
            assert_eq!(
                crate::wl::screencopy::is_supported(f),
                texture_layout(f).is_some(),
                "{f:?}"
            );
        }
    }

    #[test]
    fn fade_runs_from_the_first_configure() {
        assert_eq!(fade_for(None, 500, 1500), 0.0);
        assert_eq!(fade_for(Some(100), 100, 1500), 0.0);
        assert!(close(fade_for(Some(100), 850, 1500), 0.5, 1e-6));
        assert_eq!(fade_for(Some(100), 5000, 1500), 1.0);
        // 時計が configure の時刻より前を指しても黒のまま
        assert_eq!(fade_for(Some(100), 50, 1500), 0.0);
    }

    #[test]
    fn zero_fade_time_shows_the_background_at_once() {
        assert_eq!(fade_for(Some(10), 10, 0), 1.0);
    }

    #[test]
    fn reveal_runs_only_while_unlocking() {
        assert_eq!(reveal_for(Phase::Idle, 5000), 0.0);
        assert_eq!(reveal_for(Phase::Verifying, 5000), 0.0);
        assert_eq!(reveal_for(Phase::Failed { since_ms: 0 }, 5000), 0.0);
        let unlocking = Phase::Unlocking { since_ms: 1000 };
        assert_eq!(reveal_for(unlocking, 1000), 0.0);
        let mid = 1000 + (crate::anim::REVEAL_START_MS + crate::anim::REVEAL_MS / 2.0) as u64;
        assert!(close(reveal_for(unlocking, mid), 0.5, 1e-3));
        assert_eq!(
            reveal_for(unlocking, 1000 + crate::state::UNLOCK_TOTAL_MS),
            1.0
        );
        assert_eq!(reveal_for(Phase::Done, 0), 1.0);
    }

    #[test]
    fn frames_are_needed_while_fading_or_unlocking() {
        assert!(needs_frames(0.5, Phase::Idle));
        assert!(!needs_frames(1.0, Phase::Idle));
        assert!(!needs_frames(1.0, Phase::Verifying));
        assert!(needs_frames(1.0, Phase::Unlocking { since_ms: 0 }));
        assert!(!needs_frames(1.0, Phase::Done));
    }
}
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test background`
Expected: コンパイルエラー（`cannot find function clamp_blur` など）。

- [ ] **Step 3: 実装を書く**

`use` の後、テストモジュールの前に追加する:

```rust
// ---- 純粋な計算 ----

/// 設定の `blur` が範囲外・NaN・負のときに使う値。
pub const DEFAULT_BLUR: f32 = 7.0;
/// 1 方向のガウスの最大半径（縮小後の画素）。シェーダーの配列の大きさと同じ。
pub const MAX_RADIUS: usize = 64;
/// 縮小の最大倍率。
pub const MAX_DOWNSCALE: u32 = 4;

/// `appearance.blur` を 0.5〜50 に丸める。NaN と負は 7。
pub fn clamp_blur(blur: f32) -> f32 {
    if blur.is_nan() || blur < 0.0 {
        DEFAULT_BLUR
    } else {
        blur.clamp(0.5, 50.0)
    }
}

/// `blur` を、スクリーンショットの画素で測ったガウスの σ に直す。
///
/// swaylock-effects の `--effect-blur <blur>x5`（半径 `blur` の箱型ぼかしを 5 回）と
/// 同じ分散になるようにする: 箱 1 回の分散は ((2r+1)² − 1) / 12 = r(r+1)/3。
/// `blur = 7` で σ ≈ 9.66。
pub fn sigma_for_blur(blur: f32) -> f32 {
    let r = clamp_blur(blur);
    (5.0 * r * (r + 1.0) / 3.0).sqrt()
}

/// σ（元画像の画素）に対する縮小倍率。σ の半分を超えない整数で、1〜4。
pub fn downscale_factor(sigma: f32) -> u32 {
    ((sigma / 2.0).floor() as u32).clamp(1, MAX_DOWNSCALE)
}

/// 縮小後の大きさ（切り上げ、最低 1）。
pub fn scaled_size(width: u32, height: u32, factor: u32) -> (u32, u32) {
    let f = factor.max(1);
    (width.div_ceil(f).max(1), height.div_ceil(f).max(1))
}

/// 1 方向のガウスの重み（中心から外へ、`weights[0]` が中心）。
///
/// 半径は ceil(3σ)（1〜`MAX_RADIUS`）。`weights[0] + 2 * (残りの和) = 1` に正規化する。
pub fn kernel(sigma: f32) -> Vec<f32> {
    let sigma = sigma.max(0.1);
    let radius = ((sigma * 3.0).ceil() as usize).clamp(1, MAX_RADIUS);
    let mut weights: Vec<f32> = (0..=radius)
        .map(|i| (-((i * i) as f32) / (2.0 * sigma * sigma)).exp())
        .collect();
    let sum = weights[0] + 2.0 * weights[1..].iter().sum::<f32>();
    for w in &mut weights {
        *w /= sum;
    }
    weights
}

/// スクリーンショットの形式をテクスチャにするときの指定。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureLayout {
    pub internal_format: u32,
    pub format: u32,
    pub ty: u32,
    /// 赤と青を入れ替えて読む（テクスチャのスウィズル）。
    pub swap_rb: bool,
}

/// wl_shm の形式 → テクスチャの指定。扱えない形式は `None`（その出力は黒）。
///
/// wl_shm の形式はリトルエンディアンの 32 ビット値の並びなので、`Xrgb8888` はメモリ上
/// B, G, R, X。GLES3 には BGRA の形式がないので RGBA として送り、赤と青をスウィズルで入れ替える。
/// アルファは使わない（合成のシェーダーが 1.0 にする）。
pub fn texture_layout(format: wl_shm::Format) -> Option<TextureLayout> {
    use wl_shm::Format as F;
    let (internal_format, ty) = match format {
        F::Xrgb8888 | F::Argb8888 | F::Xbgr8888 | F::Abgr8888 => (glow::RGBA8, glow::UNSIGNED_BYTE),
        F::Xrgb2101010 | F::Argb2101010 | F::Xbgr2101010 | F::Abgr2101010 => {
            (glow::RGB10_A2, glow::UNSIGNED_INT_2_10_10_10_REV)
        }
        _ => return None,
    };
    let swap_rb = matches!(
        format,
        F::Xrgb8888 | F::Argb8888 | F::Xrgb2101010 | F::Argb2101010
    );
    Some(TextureLayout {
        internal_format,
        format: glow::RGBA,
        ty,
        swap_rb,
    })
}

/// 黒 → ぼかしの進み具合（0.0〜1.0）。
///
/// `first_configure_ms` は `App` が最初の configure で記録した時刻（出力ごとではなく全体で 1 つ）。
/// まだ configure を受けていなければ 0.0。
pub fn fade_for(first_configure_ms: Option<u64>, now_ms: u64, fade_in_ms: u32) -> f32 {
    match first_configure_ms {
        None => 0.0,
        Some(start) => {
            crate::anim::fade_alpha(now_ms.saturating_sub(start) as f32, fade_in_ms as f32)
        }
    }
}

/// ぼかし → くっきりの進み具合。解除中だけ進み、ロックを外した後（`Done`）は 1.0。
pub fn reveal_for(phase: Phase, now_ms: u64) -> f32 {
    match phase {
        Phase::Unlocking { .. } => {
            crate::anim::reveal_progress(phase.elapsed_ms(now_ms).unwrap_or(0.0))
        }
        Phase::Done => 1.0,
        _ => 0.0,
    }
}

/// 背景がまだ動いている（フレームコールバックで描き直す必要がある）か。
pub fn needs_frames(fade: f32, phase: Phase) -> bool {
    fade < 1.0 || matches!(phase, Phase::Unlocking { .. })
}
```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test background`
Expected: `test result: ok. 16 passed`。

- [ ] **Step 5: コミットする**

```bash
git status
git diff
git add src/render/background.rs
git commit -m "feat: add blur, pixel format and fade timing helpers for the background

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: 背景のシェーダー（GPU の部分）

**Files:**
- Modify: `src/render/background.rs`

**Interfaces:**
- Consumes: Task 4 の `Gpu`、Task 3 の `Screenshot`、Task 5 の純粋関数
- Produces（契約 4 章）:
  - `pub struct Background`
  - `Background::new(gpu: &Gpu, blur: f32) -> Result<Background, String>`（シェーダー 3 つをコンパイル。`gpu` が current であること）
  - `Background::upload(&mut self, gpu: &Gpu, shot: &Screenshot)`（元画像をミップマップ付きで送り、縮小 → 横 → 縦の 3 パスでぼかしを作る。終わったら framebuffer 0 を戻す）
  - `Background::draw(&self, gpu: &Gpu, output_name: &str, width: u32, height: u32, fade: f32, reveal: f32)`（**今 bind されている framebuffer** に描く。スクショがない出力は黒で塗る。`width` / `height` はバッファの画素数）

**シェーダーの要点（GLSL ES 3.00）:**
- 頂点バッファを使わず、`gl_VertexID` から画面を覆う三角形を 1 枚作る（VAO は空のものを 1 つ作って bind する）。
- テクスチャはどれも「1 行目（t = 0）が画面の上」。FBO への描き込みは `v_uv` をそのまま使い、画面への合成だけ `1.0 - v_uv.y` で上下を返す。
- 縮小は `textureLod(元画像, uv, log2(倍率))`（ミップマップの箱型平均）。ガウスの重みは `uniform float u_weights[65]` に CPU で計算して渡す。
- 合成は 1 パス: `mix(ぼかし × ビネット × fade, 元画像, reveal)`、アルファは 1.0。ブレンドとシザーは切ってから描く（Plan 4 の egui が後から重ねる）。

- [ ] **Step 1: `use` を置き換える**

Task 5 で書いた 2 つの `use`（`wl_shm` と `Phase`）を、次に置き換える:

```rust
use std::collections::HashMap;
use std::time::Instant;

use glow::HasContext;
use log::{debug, warn};
use wayland_client::protocol::wl_shm;

use crate::render::egl::Gpu;
use crate::state::Phase;
use crate::wl::screencopy::Screenshot;
```

- [ ] **Step 2: GPU の部分を足す**

`needs_frames` の後、`#[cfg(test)]` の前に追加する:

```rust
// ---- GPU ----

/// 画面全体を覆う 1 枚の三角形（頂点バッファなし）。`v_uv` は左下が (0, 0)。
const VERTEX_SHADER: &str = r#"#version 300 es
out vec2 v_uv;
void main() {
    vec2 pos = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    v_uv = pos;
    gl_Position = vec4(pos * 2.0 - 1.0, 0.0, 1.0);
}
"#;

/// 縮小: ミップマップの段 `u_lod` を読む（元画像を箱型に平均したもの）。
const DOWNSCALE_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_src;
uniform float u_lod;
in vec2 v_uv;
out vec4 frag;
void main() {
    frag = vec4(textureLod(u_src, v_uv, u_lod).rgb, 1.0);
}
"#;

/// 1 方向のガウス。`u_step` は 1 画素ぶんの uv（横なら (1/w, 0)）。
const BLUR_SHADER: &str = r#"#version 300 es
precision highp float;
const int MAX_RADIUS = 64;
uniform sampler2D u_src;
uniform vec2 u_step;
uniform int u_radius;
uniform float u_weights[MAX_RADIUS + 1];
in vec2 v_uv;
out vec4 frag;
void main() {
    vec3 sum = texture(u_src, v_uv).rgb * u_weights[0];
    for (int i = 1; i <= u_radius; ++i) {
        vec2 offset = u_step * float(i);
        sum += (texture(u_src, v_uv + offset).rgb + texture(u_src, v_uv - offset).rgb) * u_weights[i];
    }
    frag = vec4(sum, 1.0);
}
"#;

/// 合成: ぼかし × ビネット × フェード（黒の上）→ 解除で元画像へ。
const COMPOSITE_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_blurred;
uniform sampler2D u_sharp;
uniform float u_fade;
uniform float u_reveal;
in vec2 v_uv;
out vec4 frag;
void main() {
    // テクスチャは 1 行目（t = 0）が画面の上。画面の下端が v_uv.y = 0 なので上下を返す
    vec2 uv = vec2(v_uv.x, 1.0 - v_uv.y);
    // swaylock-effects の --effect-vignette 0.5:0.5 と同じ式（中央 1.0、四辺 0.5）
    float vignette = clamp(0.5 + 0.5 * 16.0 * uv.x * uv.y * (1.0 - uv.x) * (1.0 - uv.y), 0.0, 1.0);
    vec3 blurred = texture(u_blurred, uv).rgb * vignette * u_fade;
    vec3 sharp = texture(u_sharp, uv).rgb;
    frag = vec4(mix(blurred, sharp, u_reveal), 1.0);
}
"#;

/// 縮小ぼかしの 1 パス。`step` が `None` なら縮小、`Some` なら 1 方向のガウス（1 画素ぶんの uv）。
struct Pass {
    src: glow::Texture,
    dst: glow::Texture,
    step: Option<[f32; 2]>,
}

struct Textures {
    /// 元画像（ミップマップ付き）。解除のときに使う。
    sharp: glow::Texture,
    /// 縮小してぼかしたもの。
    blurred: glow::Texture,
}

pub struct Background {
    vao: glow::VertexArray,
    downscale: glow::Program,
    blur: glow::Program,
    composite: glow::Program,
    /// 元画像の画素で測った σ。
    sigma: f32,
    textures: HashMap<String, Textures>,
}

impl Background {
    /// シェーダーをコンパイルする。`gpu` は current であること（`Gpu::new` の直後はそうなっている）。
    pub fn new(gpu: &Gpu, blur: f32) -> Result<Background, String> {
        let gl = gpu.gl();
        // SAFETY: current な GL コンテキストに対する、引数の正しい GL 呼び出しだけ。
        unsafe {
            let downscale = link(gl, DOWNSCALE_SHADER)?;
            let blur_program = link(gl, BLUR_SHADER)?;
            let composite = link(gl, COMPOSITE_SHADER)?;
            let vao = gl.create_vertex_array()?;
            let sigma = sigma_for_blur(blur);
            debug!(
                "background shaders ready (blur {} -> sigma {sigma:.2}px, downscale 1/{})",
                clamp_blur(blur),
                downscale_factor(sigma)
            );
            Ok(Background {
                vao,
                downscale,
                blur: blur_program,
                composite,
                sigma,
                textures: HashMap::new(),
            })
        }
    }

    /// スクリーンショットを GPU に送り、縮小ぼかしを作っておく。
    ///
    /// 扱えない形式や壊れた大きさなら警告して何もしない（その出力は黒）。
    /// 終わったら framebuffer 0 を戻す。
    pub fn upload(&mut self, gpu: &Gpu, shot: &Screenshot) {
        let started = Instant::now();
        let Some(layout) = texture_layout(shot.format) else {
            warn!(
                "screenshot of {}: unsupported format {:?}, using black",
                shot.output_name, shot.format
            );
            return;
        };
        let (w, h, stride) = (shot.width, shot.height, shot.stride);
        let needed = (stride as usize) * (h as usize).saturating_sub(1) + w as usize * 4;
        if w == 0 || h == 0 || stride % 4 != 0 || stride < w * 4 || shot.pixels.len() < needed {
            warn!(
                "screenshot of {}: bad size {w}x{h} stride {stride}, using black",
                shot.output_name
            );
            return;
        }
        let gl = gpu.gl();
        // SAFETY: current な GL コンテキスト。画素の長さは上で確かめた。
        let result = unsafe { self.make_textures(gl, shot, layout) };
        match result {
            Ok(textures) => {
                if let Some(old) = self.textures.insert(shot.output_name.clone(), textures) {
                    // SAFETY: もう参照しないテクスチャを消す
                    unsafe {
                        gl.delete_texture(old.sharp);
                        gl.delete_texture(old.blurred);
                    }
                }
                debug!(
                    "background for {} ({w}x{h}) uploaded in {:?}",
                    shot.output_name,
                    started.elapsed()
                );
            }
            Err(e) => warn!(
                "background for {} failed, using black: {e}",
                shot.output_name
            ),
        }
    }

    unsafe fn make_textures(
        &self,
        gl: &glow::Context,
        shot: &Screenshot,
        layout: TextureLayout,
    ) -> Result<Textures, String> {
        unsafe {
            let (w, h) = (shot.width as i32, shot.height as i32);

            // 元画像。行の長さは stride から決める
            let sharp = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(sharp));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
            gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, (shot.stride / 4) as i32);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                layout.internal_format as i32,
                w,
                h,
                0,
                layout.format,
                layout.ty,
                glow::PixelUnpackData::Slice(Some(&shot.pixels)),
            );
            gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, 0);
            if layout.swap_rb {
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_SWIZZLE_R, glow::BLUE as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_SWIZZLE_B, glow::RED as i32);
            }
            set_sampling(gl, glow::LINEAR_MIPMAP_LINEAR);
            gl.generate_mipmap(glow::TEXTURE_2D);

            // 縮小用の 2 枚（a に縮小 → b に横 → a に縦）
            let factor = downscale_factor(self.sigma);
            let (dw, dh) = scaled_size(shot.width, shot.height, factor);
            let a = target_texture(gl, dw, dh)?;
            let b = target_texture(gl, dw, dh)?;
            let fbo = gl.create_framebuffer()?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.viewport(0, 0, dw as i32, dh as i32);
            gl.disable(glow::BLEND);
            gl.disable(glow::SCISSOR_TEST);
            gl.bind_vertex_array(Some(self.vao));
            gl.active_texture(glow::TEXTURE0);

            let weights = kernel(self.sigma / factor as f32);
            let mut padded = [0.0f32; MAX_RADIUS + 1];
            padded[..weights.len()].copy_from_slice(&weights);
            let radius = (weights.len() - 1) as i32;

            let passes = [
                Pass {
                    src: sharp,
                    dst: a,
                    step: None,
                },
                Pass {
                    src: a,
                    dst: b,
                    step: Some([1.0 / dw as f32, 0.0]),
                },
                Pass {
                    src: b,
                    dst: a,
                    step: Some([0.0, 1.0 / dh as f32]),
                },
            ];
            let mut status = Ok(());
            for Pass { src, dst, step } in passes {
                gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    Some(dst),
                    0,
                );
                if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
                    status = Err("framebuffer incomplete".to_string());
                    break;
                }
                gl.bind_texture(glow::TEXTURE_2D, Some(src));
                match step {
                    None => {
                        gl.use_program(Some(self.downscale));
                        uniform_i32(gl, self.downscale, "u_src", 0);
                        uniform_f32(gl, self.downscale, "u_lod", (factor as f32).log2());
                    }
                    Some([sx, sy]) => {
                        gl.use_program(Some(self.blur));
                        uniform_i32(gl, self.blur, "u_src", 0);
                        uniform_i32(gl, self.blur, "u_radius", radius);
                        let step_loc = gl.get_uniform_location(self.blur, "u_step");
                        gl.uniform_2_f32(step_loc.as_ref(), sx, sy);
                        let weights_loc = gl.get_uniform_location(self.blur, "u_weights");
                        gl.uniform_1_f32_slice(weights_loc.as_ref(), &padded);
                    }
                }
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
            }

            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.delete_framebuffer(fbo);
            gl.delete_texture(b);
            let error = gl.get_error();
            if status.is_ok() && error != glow::NO_ERROR {
                status = Err(format!("GL error 0x{error:x}"));
            }
            match status {
                Ok(()) => Ok(Textures { sharp, blurred: a }),
                Err(e) => {
                    gl.delete_texture(sharp);
                    gl.delete_texture(a);
                    Err(e)
                }
            }
        }
    }

    /// 今 bind されている framebuffer（通常は出力の EGL サーフェス）に背景を描く。
    ///
    /// `fade` 0..1（黒→ぼかし）、`reveal` 0..1（ぼかし→くっきり）。スクショがない出力は黒。
    /// `width` / `height` はバッファの画素数。
    pub fn draw(
        &self,
        gpu: &Gpu,
        output_name: &str,
        width: u32,
        height: u32,
        fade: f32,
        reveal: f32,
    ) {
        let gl = gpu.gl();
        // SAFETY: current な GL コンテキストに対する、引数の正しい GL 呼び出しだけ。
        unsafe {
            gl.viewport(0, 0, width as i32, height as i32);
            gl.disable(glow::BLEND);
            gl.disable(glow::SCISSOR_TEST);
            let Some(textures) = self.textures.get(output_name) else {
                gl.clear_color(0.0, 0.0, 0.0, 1.0);
                gl.clear(glow::COLOR_BUFFER_BIT);
                return;
            };
            gl.use_program(Some(self.composite));
            gl.bind_vertex_array(Some(self.vao));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(textures.blurred));
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(textures.sharp));
            gl.active_texture(glow::TEXTURE0);
            uniform_i32(gl, self.composite, "u_blurred", 0);
            uniform_i32(gl, self.composite, "u_sharp", 1);
            uniform_f32(gl, self.composite, "u_fade", fade.clamp(0.0, 1.0));
            uniform_f32(gl, self.composite, "u_reveal", reveal.clamp(0.0, 1.0));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
        }
    }
}

/// 頂点シェーダー（共通）と `fragment` をリンクする。
unsafe fn link(gl: &glow::Context, fragment: &str) -> Result<glow::Program, String> {
    unsafe {
        let program = gl.create_program()?;
        let mut shaders = Vec::new();
        for (kind, source) in [
            (glow::VERTEX_SHADER, VERTEX_SHADER),
            (glow::FRAGMENT_SHADER, fragment),
        ] {
            let shader = gl.create_shader(kind)?;
            gl.shader_source(shader, source);
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                let log = gl.get_shader_info_log(shader);
                gl.delete_shader(shader);
                gl.delete_program(program);
                return Err(format!("shader compile failed: {log}"));
            }
            gl.attach_shader(program, shader);
            shaders.push(shader);
        }
        gl.link_program(program);
        for shader in shaders {
            gl.detach_shader(program, shader);
            gl.delete_shader(shader);
        }
        if !gl.get_program_link_status(program) {
            let log = gl.get_program_info_log(program);
            gl.delete_program(program);
            return Err(format!("program link failed: {log}"));
        }
        Ok(program)
    }
}

/// 縮小・ぼかしの描き込み先（RGBA8、ミップマップなし）。
unsafe fn target_texture(
    gl: &glow::Context,
    width: u32,
    height: u32,
) -> Result<glow::Texture, String> {
    unsafe {
        let texture = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            width as i32,
            height as i32,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(None),
        );
        set_sampling(gl, glow::LINEAR);
        Ok(texture)
    }
}

/// bind 中のテクスチャの拡大縮小と端の扱い。
unsafe fn set_sampling(gl: &glow::Context, min_filter: u32) {
    unsafe {
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            min_filter as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MAG_FILTER,
            glow::LINEAR as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_S,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_T,
            glow::CLAMP_TO_EDGE as i32,
        );
    }
}

unsafe fn uniform_i32(gl: &glow::Context, program: glow::Program, name: &str, value: i32) {
    unsafe {
        let location = gl.get_uniform_location(program, name);
        gl.uniform_1_i32(location.as_ref(), value);
    }
}

unsafe fn uniform_f32(gl: &glow::Context, program: glow::Program, name: &str, value: f32) {
    unsafe {
        let location = gl.get_uniform_location(program, name);
        gl.uniform_1_f32(location.as_ref(), value);
    }
}
```

- [ ] **Step 3: 描画を確かめるテストを足す**

テストモジュールの末尾（最後の `}` の前）に追加する:

```rust
    /// 単色の画像を渡して、フェード・ビネット・ぼかし解除と赤青の入れ替えを確かめる。
    /// 今のセッションの Wayland 接続で EGL を作る（ロックはしない）。`cargo test -- --ignored` で手動実行。
    #[test]
    #[ignore = "needs a running Wayland compositor and a GPU"]
    fn draws_fade_vignette_and_reveal() {
        let conn = wayland_client::Connection::connect_to_env().expect("wayland");
        let gpu = Gpu::new(&conn).expect("gpu");
        let mut bg = Background::new(&gpu, 7.0).expect("shaders");
        let (w, h) = (64u32, 32u32);
        // Xrgb8888 はメモリ上 B, G, R, X。赤 200・緑 20・青 10 の単色
        let pixels = [10u8, 20, 200, 0].repeat((w * h) as usize);
        bg.upload(
            &gpu,
            &Screenshot {
                output_name: "TEST-1".into(),
                width: w,
                height: h,
                stride: w * 4,
                format: wl_shm::Format::Xrgb8888,
                pixels,
            },
        );
        let gl = gpu.gl();
        let read = |name: &str, fade: f32, reveal: f32| -> Vec<u8> {
            unsafe {
                let target = target_texture(gl, w, h).unwrap();
                let fbo = gl.create_framebuffer().unwrap();
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
                gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    Some(target),
                    0,
                );
                bg.draw(&gpu, name, w, h, fade, reveal);
                let mut out = vec![0u8; (w * h * 4) as usize];
                gl.read_pixels(
                    0,
                    0,
                    w as i32,
                    h as i32,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelPackData::Slice(Some(&mut out)),
                );
                assert_eq!(gl.get_error(), glow::NO_ERROR);
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                gl.delete_framebuffer(fbo);
                gl.delete_texture(target);
                out
            }
        };
        let at = |px: &[u8], x: u32, y: u32| -> [u8; 3] {
            let i = ((y * w + x) * 4) as usize;
            [px[i], px[i + 1], px[i + 2]]
        };
        let near = |a: [u8; 3], b: [u8; 3]| {
            a.iter()
                .zip(b)
                .all(|(x, y)| (*x as i32 - y as i32).abs() <= 3)
        };

        // フェード 0 は黒
        assert_eq!(at(&read("TEST-1", 0.0, 0.0), 32, 16), [0, 0, 0]);
        // フェード 1: 中央はそのままの色（RGB の順に直っている）、角はビネットで約半分
        let full = read("TEST-1", 1.0, 0.0);
        assert!(
            near(at(&full, 32, 16), [200, 20, 10]),
            "{:?}",
            at(&full, 32, 16)
        );
        let corner = at(&full, 0, 0);
        assert!(near(corner, [100, 10, 5]), "{corner:?}");
        // 解除し切ると、フェードに関係なく元画像（ビネットなし）
        let revealed = read("TEST-1", 0.3, 1.0);
        assert!(
            near(at(&revealed, 0, 0), [200, 20, 10]),
            "{:?}",
            at(&revealed, 0, 0)
        );
        // スクショのない出力は黒
        assert_eq!(at(&read("NONE-1", 1.0, 1.0), 32, 16), [0, 0, 0]);
    }
```

- [ ] **Step 4: ビルド・テスト・clippy**

Run: `cargo build && cargo test background && cargo clippy --all-targets`
Expected: `test result: ok. 16 passed; 0 failed; 1 ignored`。clippy の警告なし（`dead_code` を除く）。

- [ ] **Step 5: シェーダーを実際にコンパイルして描いてみる（ロックはしない）**

シェーダーのエラーは実行時にしか出ないので、必ず実行する。

Run: `cargo test draws_fade_vignette_and_reveal -- --ignored --nocapture`
Expected: `1 passed`。確かめていること: フェード 0 で黒、フェード 1 で中央が元の色（`[200, 20, 10]`。赤と青のスウィズルが効いている）、角がビネットで半分（`[100, 10, 5]`）、`reveal = 1` ならフェード 0.3 でも角まで元の色、スクショのない出力は黒。
失敗して `shader compile failed:` / `program link failed:` が出たら、ログのシェーダーのエラーを直す。

- [ ] **Step 6: コミットする**

```bash
git status
git diff
git add src/render/background.rs
git commit -m "feat: blur screenshots on the GPU and composite fade, vignette and reveal

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: `OutputRender::Gpu` と出力ごとの状態

**Files:**
- Modify: `src/render/mod.rs`, `src/app.rs`

**Interfaces:**
- Consumes: Plan 2 の `OutputRender`、`LockOutput`、`App`、Task 4 の `Gpu` / `GpuSurface`、Task 6 の `Background`
- Produces:
  - `OutputRender::Gpu(egl::GpuSurface)`
  - `LockOutput` の `render` を `lock_surface` より前へ移し、`pub(crate) scale: i32` を足す
  - `App` のフィールド `pub(crate) gpu: Option<Gpu>`、`pub(crate) background: Option<Background>`

- [ ] **Step 1: `OutputRender` に `Gpu` を足す**

`src/render/mod.rs` の `pub enum OutputRender` の中で、Plan 2 のコメント行 `// Plan 3 が追加: Gpu(egl::GpuSurface),` を次の 2 行に置き換える:

```rust
    /// EGL のウィンドウサーフェスに GL で描く（Plan 3）。
    Gpu(egl::GpuSurface),
```

- [ ] **Step 2: `LockOutput` を変える**

`src/app.rs` の `pub(crate) struct LockOutput` を次にする（`output` と `meta` は Plan 2 のまま。`render` を `lock_surface` の前へ移し、`frame_pending` の doc を直し、`scale` を足す）:

```rust
/// 出力 1 つ分のロック画面。
pub(crate) struct LockOutput {
    /// 抜かれたときに探すための wl_output。
    pub(crate) output: wl_output::WlOutput,
    pub(crate) meta: OutputMeta,
    /// configure 前は `None`。
    ///
    /// `lock_surface` より前に置く。フィールドは宣言順に捨てられるので、出力が抜かれたときに
    /// EGL サーフェスと wl_egl_window が wl_surface より先に壊れる。この順番を変えないこと。
    pub(crate) render: Option<OutputRender>,
    pub(crate) lock_surface: SessionLockSurface,
    /// configure の論理サイズ。configure 前は (0, 0)。
    pub(crate) size: (u32, u32),
    /// フレームコールバック待ち。GPU で描いていて、アニメーション中のときだけ要求する。
    pub(crate) frame_pending: bool,
    /// wl_surface の buffer_scale。GPU で描くときだけ使う（shm は常に 1）。
    pub(crate) scale: i32,
}
```

`App::add_output` の `self.outputs.push(LockOutput { .. })` で、`frame_pending: false,` の次の行に足す:

```rust
            scale: 1,
```

- [ ] **Step 3: `App` にフィールドを足す**

`pub struct App` の `first_configure_ms` の次（`exit` の前）に足す:

```rust
    /// EGL。使えなければ `None`（全出力を shm の黒で描く）。
    pub(crate) gpu: Option<Gpu>,
    /// 背景のシェーダーと出力ごとのテクスチャ。`gpu` が `Some` のときだけ `Some`。
    pub(crate) background: Option<Background>,
```

`run` の `let mut app = App { .. }` で、`first_configure_ms: None,` の次の行に足す:

```rust
        gpu: None,
        background: None,
```

フェードの開始時刻は Plan 2 の `first_configure_ms`（`configure` で最初の 1 回だけ `Some(now)` にしている）をそのまま使う。別のフィールドは足さない。

`src/app.rs` の `use crate::render::{OutputRender, shm::ShmBlack};` の前に足す:

```rust
use crate::render::background::Background;
use crate::render::egl::Gpu;
```

- [ ] **Step 4: `draw_output` の `match` に一時的な腕を足す**

`draw_output` の `match out.render.as_mut()` の `None => {}` の前に足す（Task 9 で置き換える）:

```rust
            Some(OutputRender::Gpu(_)) => {}
```

Run: `cargo build && cargo test`
Expected: エラーなし、テストはすべて通る（ignored の 3 つを除く）。`field scale is never read` などの `dead_code` 警告は Task 8・9 で消える。

- [ ] **Step 5: コミットする**

```bash
git status
git diff
git add src/render/mod.rs src/app.rs
git commit -m "feat: add a GPU render variant and per-output buffer scale

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

（`OutputRender` に変種を足すと `app.rs` の `match` を直さないとビルドが通らないので、2 ファイルを 1 つのコミットにする。）

---

### Task 8: 起動順（撮影と EGL を並行してからロック）

**Files:**
- Modify: `src/app.rs`

**Interfaces:**
- Consumes: `crate::wl::screencopy::spawn_capture_all`、`Gpu::new`、`Background::{new, upload}`、`app.config.appearance.blur`
- Produces: `fn prepare_background(conn: &Connection, blur: f32) -> (Option<Gpu>, Option<Background>)`（`app.rs` の中だけ）。`run` の中で `lock` を要求する前に呼ぶ。

契約 4 章の起動順: 設定 → 接続 → roundtrip → `spawn_capture_all` と `Gpu::new` を並行 → join → `Background::upload` → （Plan 5 の keepassxc）→ `lock` → lock surface。

- [ ] **Step 1: `prepare_background` を足す**

`src/app.rs` の `/// ロックして、解除されるまでイベントループを回す。`（`pub fn run` の doc）の前に追加する:

```rust
/// スクリーンショット（別スレッド）と EGL の準備を並行して進め、背景を作る。
///
/// EGL かシェーダーが使えなければ `(None, None)` で、全出力を shm の黒で描く。
/// 撮れなかった出力は `Background` にテクスチャがないので黒になる。
fn prepare_background(conn: &Connection, blur: f32) -> (Option<Gpu>, Option<Background>) {
    let capture = crate::wl::screencopy::spawn_capture_all();
    let gpu = match Gpu::new(conn) {
        Ok(gpu) => Some(gpu),
        Err(e) => {
            log::warn!("EGL unavailable, falling back to a black shm background: {e}");
            None
        }
    };
    let shots = capture.join().unwrap_or_else(|_| {
        log::warn!("screenshot thread panicked, background will be black");
        Vec::new()
    });
    let Some(gpu) = gpu else {
        return (None, None);
    };
    match Background::new(&gpu, blur) {
        Ok(mut background) => {
            for shot in &shots {
                background.upload(&gpu, shot);
            }
            (Some(gpu), Some(background))
        }
        Err(e) => {
            log::warn!("background shaders failed, falling back to shm: {e}");
            (None, None)
        }
    }
}
```

- [ ] **Step 2: `run` から呼ぶ**

`run` の中の、出力の名前を受け取る roundtrip の後にある次の部分を探す:

```rust
        log::info!("output {}: {}", meta.label(), meta.description);
    }
    WaylandSource::new(conn, queue)
```

これを次にする（`for` の後、`conn` が `WaylandSource::new` に move される前に呼ぶ）:

```rust
        log::info!("output {}: {}", meta.label(), meta.description);
    }
    // 背景を用意してからロックする（撮影と EGL の準備は並行。ロックした後は撮れない）
    let (gpu, background) = prepare_background(&conn, app.config.appearance.blur);
    app.gpu = gpu;
    app.background = background;
    WaylandSource::new(conn, queue)
```

`Gpu` は main の接続の `wl_display` を使う。`conn` は `WaylandSource` に move された後もイベントループの中で生きていて、`app.conn` も複製を持つので、`app` が使われている間に接続が閉じることはない。

- [ ] **Step 3: ビルド・テスト**

Run: `cargo build && cargo test`
Expected: エラーなし。残る警告は `field scale is never read` だけ（Task 9 で消える）。

- [ ] **Step 4: コミットする**

```bash
git status
git diff
git add src/app.rs
git commit -m "feat: capture screenshots and set up EGL in parallel before locking

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: 描画・フレームコールバック・スケール・解除前の最後のフレーム

**Files:**
- Modify: `src/app.rs`

**Interfaces:**
- Consumes: Task 5〜8 のすべて、`Clock::now_ms`、`Machine::phase`、`App::first_configure_ms`、SCTK の `compositor::FrameCallbackData`
- Produces（`impl App` の中）:
  - `pub(crate) fn draw_output(&mut self, index: usize)` の `Gpu` の分岐
  - `fn output_index_of(&self, surface: &wl_surface::WlSurface) -> Option<usize>`
  - `fn try_gpu_render(&mut self, index: usize)`（GPU があって `render` が `None` なら `GpuSurface` を作る。失敗したら何もしない）
  - `fn background_animating(&self) -> bool`
  - `fn kick_animation(&mut self)`（アニメーション中で、フレームコールバック待ちでない GPU の出力を描き直す）
  - `fn draw_all_outputs(&mut self)`

**描き直しの流れ:** 最初の configure で Plan 2 が `first_configure_ms` を記録する。その出力に `GpuSurface` を作って 1 枚描く（fade = 0 なので黒）。アニメーション中は描くたびに `wl_surface.frame` を要求してから swap し、`frame` イベントで次を描く。連鎖が切れても（DPMS で画面が消えていた・解除が始まった）50ms の `Tick` の後の `kick_animation` が再開する。フェードが終わって入力待ちになったら描き直しは止まる。

- [ ] **Step 1: `draw_output` と補助の関数を書く**

`src/app.rs` の `use` を 2 か所直す。SCTK の `use` の中の

```rust
    compositor::{CompositorHandler, CompositorState},
```

を

```rust
    compositor::{CompositorHandler, CompositorState, FrameCallbackData},
```

にし、Task 7 で足した 2 行

```rust
use crate::render::background::Background;
use crate::render::egl::Gpu;
```

を次にする:

```rust
use crate::render::background::{self, Background};
use crate::render::egl::{Gpu, GpuSurface};
```

`impl App` の `draw_output`（doc コメント `/// 描画の入口。…` から関数の終わりまで）を、次の `draw_output` と 5 つの補助で置き換える（`Shm` の腕は Plan 2 のまま）:

```rust
    /// 描画の入口。出力 `index` の今の状態を描いて commit する。
    pub(crate) fn draw_output(&mut self, index: usize) {
        let now = self.clock.now_ms();
        let fade = background::fade_for(
            self.first_configure_ms,
            now,
            self.config.appearance.fade_in_ms,
        );
        let phase = self.machine.phase();
        let reveal = background::reveal_for(phase, now);
        let animating = background::needs_frames(fade, phase);
        let Some(out) = self.outputs.get_mut(index) else {
            return;
        };
        let (width, height) = out.size;
        if width == 0 || height == 0 {
            return;
        }
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
            Some(OutputRender::Gpu(surface)) => {
                let (Some(gpu), Some(background)) = (self.gpu.as_ref(), self.background.as_ref())
                else {
                    return;
                };
                let scale = out.scale.max(1);
                let (buffer_width, buffer_height) = (width * scale as u32, height * scale as u32);
                if surface.size() != (buffer_width, buffer_height) {
                    surface.resize(buffer_width, buffer_height);
                }
                surface.make_current(gpu);
                background.draw(
                    gpu,
                    &out.meta.name,
                    buffer_width,
                    buffer_height,
                    fade,
                    reveal,
                );
                // Plan 4: ここで主画面に重ね物を描く
                let wl_surface = out.lock_surface.wl_surface();
                // buffer_scale とフレームコールバックは、swap の commit で一緒に反映される
                wl_surface.set_buffer_scale(scale);
                if animating && !out.frame_pending {
                    wl_surface.frame(&self.qh, FrameCallbackData(wl_surface.clone()));
                    out.frame_pending = true;
                }
                surface.swap(gpu);
            }
            None => {}
        }
    }

    /// wl_surface から出力の番号を引く。
    fn output_index_of(&self, surface: &wl_surface::WlSurface) -> Option<usize> {
        self.outputs
            .iter()
            .position(|o| o.lock_surface.wl_surface() == surface)
    }

    /// GPU が使えて、まだ描画手段がなければ EGL サーフェスを作る。
    /// 失敗したら何もしない（呼び出し側が shm の黒を作る）。
    fn try_gpu_render(&mut self, index: usize) {
        let Some(gpu) = self.gpu.as_ref() else {
            return;
        };
        let out = &mut self.outputs[index];
        if out.render.is_some() {
            return;
        }
        let scale = out.scale.max(1) as u32;
        let (width, height) = (out.size.0 * scale, out.size.1 * scale);
        match GpuSurface::new(gpu, out.lock_surface.wl_surface(), width, height) {
            Ok(surface) => {
                log::debug!("GPU surface for {}: {width}x{height}", out.meta.label());
                out.render = Some(OutputRender::Gpu(surface));
            }
            Err(e) => log::warn!(
                "GPU surface for {} failed, using shm: {e}",
                out.meta.label()
            ),
        }
    }

    /// 背景がまだ動いているか（フェード中・解除中）。GPU がなければ動かない。
    fn background_animating(&self) -> bool {
        if self.gpu.is_none() {
            return false;
        }
        let fade = background::fade_for(
            self.first_configure_ms,
            self.clock.now_ms(),
            self.config.appearance.fade_in_ms,
        );
        background::needs_frames(fade, self.machine.phase())
    }

    /// アニメーション中なのに、フレームコールバック待ちでない GPU の出力を描き直す。
    fn kick_animation(&mut self) {
        if !self.background_animating() {
            return;
        }
        for index in 0..self.outputs.len() {
            let out = &self.outputs[index];
            if !out.frame_pending && matches!(out.render, Some(OutputRender::Gpu(_))) {
                self.draw_output(index);
            }
        }
    }

    /// 全出力を 1 回ずつ描く。
    fn draw_all_outputs(&mut self) {
        for index in 0..self.outputs.len() {
            self.draw_output(index);
        }
    }
```

注意: フレームコールバックの udata は `FrameCallbackData(surface.clone())`（SCTK 0.21 の `delegate_dispatch2!` では `wl_surface` をそのまま渡すとコンパイルできない。試作のコメントは古い）。これで SCTK が `CompositorHandler::frame` を呼ぶ。

Run: `cargo build`
Expected: エラーなし（新しい補助が未使用の警告は Step 2〜4 で消える）。

```bash
git status
git diff
git add src/app.rs
git commit -m "feat: draw the blurred background on GPU outputs

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 2: configure で GPU サーフェスを作る**

`impl SessionLockHandler for App` の `configure` の後半（`let out = &mut self.outputs[index];` から最後の `self.draw_output(index);` まで）を次にする。`first_configure_ms` を記録する前半は Plan 2 のまま:

```rust
        let out = &mut self.outputs[index];
        out.size = configure.new_size;
        log::debug!(
            "configure {}: {}x{}",
            out.meta.label(),
            out.size.0,
            out.size.1
        );
        // GPU が使えれば EGL サーフェスを作る。使えなければ（作れなければ）shm の黒
        self.try_gpu_render(index);
        let out = &mut self.outputs[index];
        if out.render.is_none() {
            out.render = Some(OutputRender::Shm(ShmBlack::new()));
        }
        self.draw_output(index);
```

Run: `cargo build`
Expected: エラーなし。

```bash
git status
git diff
git add src/app.rs
git commit -m "feat: prefer GPU surfaces when a lock surface is configured

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 3: フレームコールバックとスケールの変化を受ける**

`impl CompositorHandler for App` の `scale_factor_changed` と `frame` を次にする（どちらも Plan 2 では空か、`frame_pending` を下ろすだけ）:

```rust
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        let Some(index) = self.output_index_of(surface) else {
            return;
        };
        let out = &mut self.outputs[index];
        let scale = new_factor.max(1);
        if out.scale == scale {
            return;
        }
        out.scale = scale;
        log::debug!("buffer scale of {}: {scale}", out.meta.label());
        // shm は論理サイズ・スケール 1 で描くので、描き直すのは GPU だけ
        if matches!(out.render, Some(OutputRender::Gpu(_))) {
            self.draw_output(index);
        }
    }
```

```rust
    fn frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        let Some(index) = self.output_index_of(surface) else {
            return;
        };
        self.outputs[index].frame_pending = false;
        // フェード中・解除中は次のフレームを描く（時刻は Clock だけを使い、引数の時刻は使わない）
        if self.background_animating() {
            self.draw_output(index);
        }
    }
```

`transform_changed` は変えない（スクリーンショットは撮影時に論理的な向きにしてあり、lock surface の `buffer_transform` は normal のままでよい）。

Run: `cargo build`
Expected: エラーなし。

```bash
git status
git diff
git add src/app.rs
git commit -m "feat: redraw on frame callbacks and follow the output scale

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 4: `Tick` で連鎖を再開し、ロックを外す前にくっきりしたフレームを描く**

(a) `run` の中の `Timer::from_duration(TICK)` のコールバックを次にする:

```rust
        .insert_source(Timer::from_duration(TICK), |_, _, app: &mut App| {
            app.handle_event(Event::Tick);
            // 描き直しの連鎖が止まっている出力があれば再開する（解除の開始・画面の復帰）
            app.kick_animation();
            TimeoutAction::ToDuration(TICK)
        })
```

(b) `App::release_lock` で、`locked` を待つ分岐の後の `lock.unlock();` を次にする:

```rust
        // Phase::Done なので reveal = 1.0。元の画面と同じくっきりしたフレームを出してから外す
        self.draw_all_outputs();
        if let Some(lock) = &self.lock {
            lock.unlock();
        }
```

（`let Some(lock) = &self.lock` の借用は `is_locked()` の確認で終わるので、`&mut self` の `draw_all_outputs` を呼べる。`unlock` のために借り直す。）変更後の `release_lock` 全体:

```rust
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
        // Phase::Done なので reveal = 1.0。元の画面と同じくっきりしたフレームを出してから外す
        self.draw_all_outputs();
        if let Some(lock) = &self.lock {
            lock.unlock();
        }
        // unlock_and_destroy をコンポジタに届けてから終わる
        if let Err(e) = self.conn.roundtrip() {
            log::warn!("roundtrip after unlock failed: {e}");
        }
        self.lock = None;
        self.release_pending = false;
        log::info!("unlocked after {} ms", self.clock.now_ms());
        self.exit = Some(Ok(()));
    }
```

`ReleaseLock` を返した時点で `Machine` は `Phase::Done` なので、`reveal_for` は 1.0 を返し、全出力が元のスクリーンショットそのものになる。`locked` より前に解除が決まった場合は、Plan 2 のとおり `locked` で `release_lock` が呼び直され、そこで描く。swap の commit は `unlock` より先に送られる。

Run:

```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Expected: エラーなし、警告なし、テストはすべて通る（ignored の 3 つを除く）。

```bash
git status
git diff
git add src/app.rs
git commit -m "feat: resume background animation on tick and draw a sharp frame before unlocking

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: 動作確認（ヘッドレスの sway と入れ子の sway）

**Files:** なし（確認だけ。直すことがあれば、その変更を対応するタスクの形でコミットする）

本物のセッションはロックしない。パスワードは自分のもので 1 回だけ試す（契約 1 章・9 章）。

- [ ] **Step 1: 自動のテストと、今のセッションでの GPU の確認**

Run:

```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo test -- --ignored --nocapture
```

Expected: 通常のテストはすべて通る。ignored の 3 つ（`captures_the_current_session`、`creates_a_surfaceless_context`、`draws_fade_vignette_and_reveal`）も通る。これらは今のセッションの画面を撮り、EGL を作るが、ロックはしない。

- [ ] **Step 2: ヘッドレスの sway で、フェードと各出力の描画を確かめる**

Plan 2 の確認と同じく、`XDG_RUNTIME_DIR` を新しい空のディレクトリにしてヘッドレスの sway を動かす（本物のセッションに影響しない。ソケットのパスが 108 バイトを超えないよう `/tmp` に作る）。入力はできないので解除はしない。描画を `grim` で撮って確かめ、最後に kabi-lock を止めて sway ごと終える。

```bash
BIN=$PWD/target/debug/kabi-lock
OUT=$(mktemp -d /tmp/kabi-p3.XXXXXX)
RT=$(mktemp -d /tmp/kabi-rt.XXXXXX)
cat > "$OUT/conf" <<EOF
output HEADLESS-1 resolution 800x500
output HEADLESS-2 resolution 800x600 scale 2
exec alacritty -e sh -c 'printf "kabi-lock background test"; sleep 120'
exec sh -c 'sleep 2; grim -o HEADLESS-2 -t ppm $OUT/before.ppm; RUST_LOG=kabi_lock=debug $BIN > $OUT/kabi.log 2>&1 & K=\$!; sleep 0.25; grim -o HEADLESS-2 -t ppm $OUT/early.ppm; sleep 2.2; grim -o HEADLESS-2 -t ppm $OUT/faded.ppm; kill \$K; sleep 0.5; grim -o HEADLESS-2 -t ppm $OUT/killed.ppm; swaymsg exit'
EOF
env -i HOME="$HOME" PATH="$PATH" USER="$USER" XDG_RUNTIME_DIR="$RT" \
  WLR_BACKENDS=headless WLR_HEADLESS_OUTPUTS=2 WLR_LIBINPUT_NO_DEVICES=1 \
  timeout 40 sway -c "$OUT/conf" > "$OUT/sway.log" 2>&1
grep -v 'sctk\]' "$OUT/kabi.log"
```

（`alacritty` がなければ手元の端末に置き換える。端末が HEADLESS-1 に開いたら、`grim -o` の出力名を入れ替える。）
WLR_RENDERER は指定しない（pixman だとクライアントの EGL が使えないことがある）。

Expected のログ（順番どおり。時間は目安）:

```text
DEBUG kabi_lock::wl::screencopy] screenshot of HEADLESS-2: 800x600 Xbgr8888
DEBUG kabi_lock::wl::screencopy] screenshot of HEADLESS-1: 800x500 Xbgr8888
DEBUG kabi_lock::wl::screencopy] captured 2 screenshot(s) in ...ms
DEBUG kabi_lock::render::egl] EGL 1.5 ready in ...ms: OpenGL ES 3.2 Mesa ...
DEBUG kabi_lock::render::background] background shaders ready (blur 7 -> sigma 9.66px, downscale 1/4)
DEBUG kabi_lock::render::background] background for HEADLESS-2 (800x600) uploaded in ...ms
DEBUG kabi_lock::render::background] background for HEADLESS-1 (800x500) uploaded in ...ms
INFO  kabi_lock::app] lock requested after ... ms
INFO  kabi_lock::app] session locked after ... ms
INFO  kabi_lock::app] first configure after ... ms
DEBUG kabi_lock::app] GPU surface for HEADLESS-2: 400x300
DEBUG kabi_lock::app] GPU surface for HEADLESS-1: 800x500
DEBUG kabi_lock::app] buffer scale of HEADLESS-2: 2
```

`WARN` が出ないこと。撮影と EGL の準備は並行なので、`lock requested` は起動から 60ms 前後。

画像（`$OUT/*.ppm`。画像ビューアで開く）で見ること:
- `early.ppm`（ロック直後 0.25 秒）はほぼ黒、`faded.ppm`（約 2.5 秒）は端末の画面をぼかして四隅を暗くしたもの。文字は読めないが、位置と色は `before.ppm` と同じで、上下・左右に反転していない。
- `killed.ppm` は赤一色（kabi-lock が終わってもロックは残る。設計書 8 章のとおり）。

- [ ] **Step 3: EGL が使えないときに shm の黒でロックすることを確かめる**

glvnd に存在しないベンダーを指定すると `eglGetPlatformDisplay` が失敗する（コードの変更は要らない）。Step 2 と同じやり方で、kabi-lock だけ環境変数を足して起動する:

```bash
OUT=$(mktemp -d /tmp/kabi-p3.XXXXXX)
RT=$(mktemp -d /tmp/kabi-rt.XXXXXX)
cat > "$OUT/conf" <<EOF
exec alacritty -e sh -c 'printf "shm fallback test"; sleep 120'
exec sh -c 'sleep 2; __EGL_VENDOR_LIBRARY_FILENAMES=/nonexistent.json RUST_LOG=kabi_lock=debug $BIN > $OUT/kabi.log 2>&1 & K=\$!; sleep 1; grim -t ppm $OUT/locked.ppm; kill \$K; sleep 0.5; grim -t ppm $OUT/killed.ppm; swaymsg exit'
EOF
env -i HOME="$HOME" PATH="$PATH" USER="$USER" XDG_RUNTIME_DIR="$RT" \
  WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 \
  timeout 40 sway -c "$OUT/conf" > "$OUT/sway.log" 2>&1
grep -v 'sctk\]' "$OUT/kabi.log"
```

Expected: ログに `WARN  kabi_lock::app] EGL unavailable, falling back to a black shm background: eglGetPlatformDisplay: One or more argument values are invalid.` が出て、`GPU surface` の行は出ない（スクリーンショットは撮るが使わない）。`locked.ppm` は黒一色、`killed.ppm` は赤一色。

- [ ] **Step 4: 入れ子の sway で、解除までの見た目を確かめる**

パスワードでの解除には、`auth` と `account` の行がある PAM サービスが要る（Plan 2 の `pam_check` は `acct_mgmt` も呼ぶ）。`/etc/pam.d/kabi-lock` はまだない（Plan 6 で入れる）ので、Plan 2 Task 12 Step 8 と同じく、一時的な設定で `pam_service = "login"` を使う。先に `/etc/pam.d/login` を確かめる:

```bash
grep -qE '^auth[[:space:]]' /etc/pam.d/login && grep -qE '^account[[:space:]]' /etc/pam.d/login && echo PAM-OK || echo "PAM file missing auth or account line"
```

Expected: `PAM-OK`。`PAM file missing auth or account line` が出たら、ここで止めて BLOCKED として人に報告する（エージェントは `/etc/pam.d` を変えない）。

リポジトリの最上位で実行する（`$PWD` を設定ファイルに埋め込むため、ヒアドキュメントの `EOF` は引用符で囲まない）。入れ子の sway のウィンドウにフォーカスを当ててから操作する。

```bash
mkdir -p /tmp/kabi-nested-config/kabi-lock
printf '[auth]\npam_service = "login"\n' > /tmp/kabi-nested-config/kabi-lock/config.toml
cat > /tmp/kabi-nested.conf <<EOF
output WL-1 scale 2
output WL-2 transform 90
exec alacritty -e sh -c 'printf "kabi-lock background test\\n"; ls -l /; sleep 600'
exec sh -c 'sleep 2; XDG_CONFIG_HOME=/tmp/kabi-nested-config RUST_LOG=kabi_lock=debug $PWD/target/debug/kabi-lock > /tmp/kabi-nested.log 2>&1; swaymsg exit'
EOF
WLR_BACKENDS=wayland WLR_WL_OUTPUTS=2 sway -c /tmp/kabi-nested.conf
```

（`grep exec /tmp/kabi-nested.conf` で、`$PWD` がリポジトリの絶対パスに置き換わっていることを確かめられる。）

見ること:
- 両方のウィンドウ（WL-1・WL-2）で、黒からぼかした画面へ約 1.5 秒でなめらかに明るくなる（途中で止まったりカクついたりしない）。四隅・四辺が暗い（ビネット）。ぼかしは以前の swaylock-effects（`--effect-blur 7x5`）と同じくらい。
- WL-2（90° 回転）の背景の文字の向きが、ロック前と同じ。
- 自分のパスワードを **1 回だけ** 正しく入れて解除する（間違ったパスワードは送らない）。解除すると、約 0.9 秒後にぼかしが 0.7 秒で晴れてくっきりした画面になり、そのまま入れ子の sway の画面に戻る（明るさ・位置・細かさが跳ねない。WL-1 のスケール 2 でもぼやけない）。
- ログに `buffer scale of WL-1: 2`、`unlocked after ... ms` が出て、`WARN` が出ない。

- [ ] **Step 5: 片付けと記録**

Run: `rm -rf /tmp/kabi-p3.* /tmp/kabi-rt.* /tmp/kabi-nested.conf /tmp/kabi-nested.log /tmp/kabi-nested-config`

確認した内容（見た目、ログ、気になった点）を作業報告に書く。直したコードがあれば、そのファイルを Conventional Commits でコミットする。

---

## 設計書との対応

### Plan 3 で満たすもの

| 設計書 | 内容 | どこで |
|---|---|---|
| 2 章 | wlr-screencopy によるスクリーンショット、ぼかし、黒からのフェードイン、全出力への背景表示 | Task 2・3・6・9 |
| 3 章「背景」 | ぼかし + ビネット、全出力 | Task 6 |
| 3 章「フェードイン」 | 黒 → ぼかし、`fade_in_ms`（初期値 1500） | Task 5・9 |
| 3 章「錠（解除）」の後半 | ぼかしが晴れてくっきりした画面へ戻ってからロックを外す | Task 5・6・9 |
| 4 章「描画の分担」 | 背景は自前の GL シェーダー | Task 6 |
| 5 章 2・3 | 撮影と EGL の準備を並行、GPU へ送って縮小ぼかし（元画像も残す） | Task 8 |
| 5 章 6 | configure が届いた出力から黒を 1 枚出してフェードを始め、以後はフレームコールバックで描く | Task 9 |
| 7 章 | `appearance.blur`（範囲外は丸める）、`fade_in_ms` | Task 5 |
| 8 章 | 一部の出力の撮影に失敗 → その出力は黒 / EGL 初期化に失敗 → `wl_shm` の黒でロック / ロック中に増えた出力は黒 | Task 3・8・9 |
| 1 章「背景」 | `--effect-scale` のずれを自前のぼかしで避ける、フレームの開始を自分で制御する | Task 6・9 |
| 引き継ぎ（Plan 3） | `blur` の検証、フェード中の U2F 解除（フェードとぼかし解除を同時に扱う）、解除前に `reveal = 1` のフレームを描く | Task 5・6・9 |

### Plan 3 では扱わないもの

| 設計書 | 内容 | 担当 |
|---|---|---|
| 3 章 | 時計・日付・入力バー・光の柱・錠（重ね物の不透明度 `fade_alpha × (1 − reveal_progress)` も） | Plan 4（`draw_output` の `// Plan 4:` の位置に描く。`fade_for` と `reveal_for` を使える） |
| 5 章 1・2 | 主画面の選択（sway IPC）、フォント・egui の準備とアトラスの事前転送 | Plan 4 |
| 5 章 4 | KeePassXC のロック依頼（`prepare_background` の後、`lock` の前） | Plan 5 |
| 6 章 | YubiKey・KeePassXC | Plan 5 |
| 8 章 | 出力が減ったときの主画面の選び直し | Plan 4 |
| — | 分数スケール、ロック中のモード変更での撮り直し、出力のスケールに合わせたぼかしの強さ | v1 では扱わない（「扱わないこと」を参照） |
