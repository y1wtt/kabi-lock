# kabi-lock Plan 4: 重ね描き 実装計画

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 主画面に、egui（egui_glow）で時計・日付・光の柱・錠を重ねて描く。錠は Enter で現れ、解除では U 字が跳ね上がってから長い足を軸に丸棒のまま回り、失敗では横に震える。主画面は設定のルールと sway IPC のフォーカス中の出力で選び、出力の抜き挿しで選び直す。時計・日付のフォントは `appearance.font`（初期値 `"Inter"`）を fontconfig で探し、使えなければ埋め込みの Inter を使う。日付に足りない文字（日本語など）は fontconfig のフォントで補う。

**Architecture:** 何をどこに描くかは純粋なモジュール `scene`（`#![forbid(unsafe_code)]`）で決め、すべて単体テストする（状態・時刻・レーン → `Scene`、1920x1080 基準の配置 → `Layout`、柱の色、U 字の折れ線、日付の書式とロケール、主画面の追跡）。フォントの候補の選び方と埋め込みに戻す判断も純粋なモジュール `fonts` に分けてテストする。`render/overlay.rs` は `Scene` を egui の図形にして、Plan 3 が背景を描いた同じ GL サーフェスに重ねるだけにする。App との継ぎ目は `OverlayHost` 1 つにまとめ、Plan 2・3 の `app.rs` には数行ずつ足す。

**Tech Stack:** Rust（edition 2024）、egui / egui_glow 0.37（glow 0.17）、skrifa 0.44（egui が使っているもの。フォントの検査だけに使う）、chrono 0.4.45（`unstable-locales`）、swayipc 4.0.0、Plan 1〜3 のクレート

**Spec:** `docs/superpowers/specs/2026-11-24-kabi-lock-design.md`（3 章の見た目、5 章の起動順、7 章の設定とフォント、8 章の主画面の選び直し）

**Contract:** `docs/superpowers/plans/2026-11-24-kabi-lock-contract.md`（4 章「Plan 4 が作るもの」）。Plan 1 の引き継ぎ（`2026-11-24-kabi-lock-01-core-handoff.md`）の「Plan 4」の項目もこの計画で扱う。

**前の計画:** Plan 2（`2026-11-24-kabi-lock-02-wayland.md`）と Plan 3（`2026-11-24-kabi-lock-03-background.md`）を実行した後のコードに当てる。この計画のコードは、Plan 2・3 を当てたリポジトリの写しで、タスクごとに当ててビルド・テスト・clippy・ヘッドレスの sway での表示まで確かめてある。

## 前提（始める前に確かめる）

Plan 2・3 が終わっていること。次を実行し、すべて見つかることを確かめる。見つからないものがあれば BLOCKED として止まる。

```bash
grep -n 'pub fn elapsed_ms' src/state.rs           # Plan 2: Phase::elapsed_ms
grep -n 'pub struct OutputMeta' src/wl/outputs.rs  # Plan 2
grep -n 'pub fn gl' src/render/egl.rs              # Plan 3: Gpu::gl
grep -n 'pub fn draw' src/render/background.rs     # Plan 3: Background::draw
grep -n 'Plan 4: ここで主画面に重ね物を描く' src/app.rs  # Plan 3 が draw_output に残した印
grep -n 'saturating_add' src/lanes.rs              # Plan 2: 引き継ぎの桁あふれ対策
grep -n 'fn empty_rules_are_ignored' src/config.rs # Plan 2: 空の主画面ルール
grep -n '^glow' Cargo.toml                         # Plan 3 が glow 0.17 を入れている
cargo test
```

Expected: 8 つの grep がそれぞれ 1 行以上を出し、`cargo test` が通る（Plan 3 の ignored の 3 件を除く）。

引き継ぎの「早めにやる」のうち、`lanes` の飽和加算と空の主画面ルールは Plan 2 で済んでいるので、この計画では扱わない。

## Global Constraints

- Rust edition 2024。`unsafe` は `render/` と `wl/` の中だけ（契約 1 章）。`scene.rs` の先頭に `#![forbid(unsafe_code)]`。
- コミットメッセージは Conventional Commits。本文は英語。末尾に、そのコミットを実際に書いたモデルの `Co-Authored-By` 行（例: `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`）。各タスクのコミット例の `Claude Opus 5.5` は書いたモデルの名前に置き換える。
- 1 コミットは原則 1 ファイル（`cargo add` による `Cargo.toml` と `Cargo.lock` は 1 つの操作としてまとめてよい）。1 ファイルに複数の変更を入れるときは部分ごとにコミットを分ける。`git commit` の前に必ず `git status` と `git diff` で変更を確認する。
- コード中のコメントは日本語、ログメッセージは英語。パスワード・入力された文字・その長さはログに出さない。
- 配置は 1920x1080 基準の値（設計書 3 章）を、描画先の高さ / 1080 倍する。レーン数は拡大前の設定値で決める（引き継ぎ）。
- 重ね物の不透明度は `fade_alpha × (1 − reveal_progress)`。`LaneAllocator` と `pillar_at` には同じ `pillar_fade_ms` を渡す（引き継ぎ）。
- クレートの API の細かな違いでコンパイルできないときは、契約の公開 API を変えない範囲で最小限直してよい（直した内容は報告に書く）。
- 本物のセッションはロックしない。動作確認はヘッドレスの sway と入れ子の sway（契約 9 章、Task 15）。

## 設計の決めごと（この計画で決めたもの）

| 項目 | 決めたこと | 理由 |
|---|---|---|
| `scene::build` の引数 | `(machine, now_ms, fade_alpha, pillars, input, wall, date)` | フェードの開始時刻は Plan 3 が持つので、`Background::draw` に渡すのと同じ `fade` を受け取る。ぼかし解除の進み具合は `Machine` の状態から `anim::reveal_progress` で求める（Plan 3 と同じ式）。壁時計の時刻は呼び出し側で取り、`build` は純粋にする |
| 時計の等幅数字 | 1 文字ずつ並べ、数字は一番広い数字の幅の枠の中央に置く | egui 0.37 は harfrust で文字組みするが、OpenType の機能（`tnum`）を指定する口がない |
| 柱のグラデーション | 頂点ごとに色を持つ格子のメッシュ（横 6 列 × 縦 9 行） | 不透明度は「横のグラデーション（両端 0.85・中 0.38）× 縦のマスク（上 0 → 下 1）」の積。三角形の中は直線補間なので、横はグラデーションの区切り（0, 0.22, 0.78, 1）に頂点を置いて正確にし、端の区間は 0.11・0.89 でも分け、縦は 8 段に分けて積の誤差を 1% 未満にする |
| U 字の線 | 3 次ベジェ 2 本を 16 分割した折れ線を egui の線（太さ一定 = 3.5 × アイコン単位）で描き、見える端（短い足の下端）に丸を足す | egui の線の端は平ら。長い足の下端は常に本体に隠れる（Task 1 のテストで確かめる） |
| 真横から見た U 字 | 短い足と長い足の x の差が 0.5 未満なら、頂点から長い足の下端までの 1 本の棒として描く | 行って戻る折れ線は egui の継ぎ目の計算で角が尖る |
| 日付のロケール | `LC_ALL` → `LC_TIME` → `LANG` を chrono の `Locale` にして `format_localized` | 設計書 7 章「曜日などは LC_TIME のロケールに従う」。chrono の `unstable-locales` 機能が要る |
| 時計・日付のフォント | `fc-list --format=… '<appearance.font>'` を子プロセスで呼び、`fonts::pick_faces` で時計用（500）と日付用（400）のフェイスを選ぶ。どちらかが使えなければ両方とも埋め込みの Inter | `fc-match` は無いファミリーでも似たフォントを返してしまい「見つからない」が分からないので、ファミリーの一覧を返す `fc-list` を使う。時計と日付で別のファミリーが混ざらないよう、戻すときは両方戻す |
| 500 / 400 がないとき | 同じファミリーの中で、CSS Fonts 4 の太さの照合と同じ順に一番近い太さを使う（500 が欲しいとき: 500 → 400 → それより細い順 → 500 より太い順。400 が欲しいとき: 400 → 500 → 細い順 → 太い順）。可変フォントは `wght` 軸をちょうどの太さにする。埋め込みに戻すのは、ファミリーがない・直立で egui が読める形式のフェイスがない・読めないときだけ | 利用者はファミリーを選んだのであって、太さが 1 段違うだけで別のフォント（Inter）に替わるより、同じファミリーの近い太さの方が意図に近い。照合の順は、設計のブラウザでの見た目と同じになるよう CSS に合わせる |
| 選んだフォントにない字形 | 字形を探す順を 選んだフォント → 埋め込みの Inter → 日本語などの補助フォント にする | 記号だけのフォントなどを選んでも数字が消えない |
| 足りない文字のフォント | `fc-match --format='%{file}\n%{index}' 'sans-serif:lang=<言語>'` を子プロセスで呼ぶ | C のライブラリ（fontconfig）を直接つながないので `unsafe` もビルド時の `pkg-config` も要らず、デスクトップの他のアプリと同じ規則で選ばれる。呼ぶのはロック前に 1 回だけ。fontconfig を読むだけの純 Rust のクレート（fontdb など）は `lang=` の照合をしない |
| 壊れたフォントへの備え | 読み込んだフォント（`appearance.font` のものも補助フォントも）を `skrifa::FontRef::from_index` で先に確かめる | egui は読めないフォントで panic する（`epaint` の `Family::new`）。egui と同じ `skrifa` で同じ検査をする |
| フォントの準備の時期 | ロック前に `Painter::new`（シェーダー）、`set_fonts`、数字と日付の字形をアトラスに載せて GPU に送る | 設計書 5 章「重いのは初期化なので、ロック要求の前に済ませる」 |
| App との継ぎ目 | `OverlayHost`（`render/overlay.rs`）。`new(&config)`（GL なし）と `attach_gpu(&gpu)` に分ける | 契約の起動順では App は roundtrip のために `Gpu::new` より先に作られる |
| 主画面の追跡 | `scene::PrimaryTracker`。描くたびと `Tick` ごとに出力の一覧を渡し、変わったときだけ `config::select_primary` を呼ぶ | 出力の追加・削除の処理（Plan 2）に手を入れずに済む。主画面が抜かれても次の `Tick`（50ms 以内）で残りの出力に描き直す |
| フレームの続け方 | 柱・震え・解除の間は主画面だけフレームコールバックを続ける（`OverlayHost::animating(index, ..)`）。それ以外の変化（分が変わる、失敗の錠が消える）は `Tick` で拾って主画面を描き直す | 打鍵のたびに全出力を 60fps で描かない |

## ファイル構成

| ファイル | 変更 | 責務 |
|---|---|---|
| `src/anim.rs` | 変更 | 錠の本体の形（角丸 6、y 24〜52）と U 字の太さを定数として公開 |
| `src/config.rs` | 変更 | `date_format` の検証、帯・柱の幅の上限、`appearance.font` |
| `src/scene.rs` | 新規 | 1 フレーム分の内容（`Scene`）、配置、柱の色、U 字の折れ線、日付とロケール、主画面の追跡 |
| `src/fonts.rs` | 新規 | fontconfig の一覧の読み取り、時計・日付のフェイスの選び方、埋め込みに戻す理由（純粋） |
| `src/lib.rs` | 変更 | `pub mod scene;`、`pub mod fonts;`、doc コメントの純粋なモジュールの並びに `scene` `fonts` を足す |
| `assets/fonts/Inter-Medium.ttf`, `Inter-Regular.ttf`, `LICENSE.txt` | 新規 | Inter 4.1（OFL） |
| `src/wl/outputs.rs` | 変更 | `focused_output()`（sway IPC） |
| `src/render/overlay.rs` | 新規 | `Overlay`（フォントの読み込み、egui の準備と描画）と `OverlayHost`（App 用の状態一式） |
| `src/render/mod.rs` | 変更 | `pub mod overlay;` |
| `src/app.rs` | 変更 | `OverlayHost` の作成・描画・打鍵・Tick の結線 |
| `Cargo.toml`, `Cargo.lock` | 変更 | chrono, swayipc, egui, egui_glow, skrifa |

---

### Task 1: 錠の本体の形を `anim` の定数にする

**Files:**
- Modify: `src/anim.rs`

**Interfaces:**
- Consumes: Plan 1 の `anim::{shackle_at, shackle_path, SHACKLE_MS, SHORT_LEG_X_CLOSED}`
- Produces: `anim::{ICON_W, ICON_H, BODY_X, BODY_Y, BODY_W, BODY_H, BODY_RADIUS, SHACKLE_STROKE}: f32`（アイコン座標 64x56）

- [ ] **Step 1: 失敗するテストを書く**

`src/anim.rs` のテストモジュール（`mod tests`）の最後のテストの後に追加する:

```rust
    #[test]
    fn body_matches_the_design() {
        assert_eq!(
            (BODY_X, BODY_Y, BODY_X + BODY_W, BODY_Y + BODY_H),
            (14.0, 24.0, 50.0, 52.0)
        );
        assert_eq!(BODY_RADIUS, 6.0);
        // 本体はアイコンの左右中央にある
        assert_eq!(BODY_X + BODY_W / 2.0, ICON_W / 2.0);
        const { assert!(BODY_Y + BODY_H <= ICON_H) };
    }

    #[test]
    fn closed_shackle_ends_are_hidden_by_the_body() {
        let p = shackle_path(shackle_at(0.0));
        for end in [p.short_leg_bottom, p.long_leg_bottom] {
            assert!(end[0] > BODY_X && end[0] < BODY_X + BODY_W);
            assert!(end[1] > BODY_Y && end[1] < BODY_Y + BODY_H);
        }
    }

    #[test]
    fn long_leg_stays_in_the_body_during_the_whole_unlock() {
        // 一番高く跳ねたとき（-11px）でも、長い足の下端（丸い端を含む）は本体に隠れる
        for ms in (0..=SHACKLE_MS as u32).step_by(10) {
            let p = shackle_path(shackle_at(ms as f32));
            assert!(p.long_leg_bottom[1] - SHACKLE_STROKE / 2.0 > BODY_Y);
        }
    }

    #[test]
    fn shackle_path_stays_finite_and_in_range() {
        for ms in (0..=SHACKLE_MS as u32).step_by(5) {
            let p = shackle_path(shackle_at(ms as f32));
            let points = [
                p.short_leg_bottom,
                p.short_leg_top,
                p.arc1[0],
                p.arc1[1],
                p.arc1[2],
                p.arc2[0],
                p.arc2[1],
                p.arc2[2],
                p.long_leg_bottom,
            ];
            assert!(points.iter().flatten().all(|v| v.is_finite()));
            assert!((SHORT_LEG_X_CLOSED..=62.0).contains(&p.short_leg_top[0]));
        }
    }
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test --lib anim::`
Expected: コンパイルエラー（`cannot find value BODY_X in this scope` など）。

- [ ] **Step 3: 定数を追加する**

`src/anim.rs` の `pub const SHORT_LEG_X_CLOSED: f32 = 22.0;` の行の下に追加する:

```rust
/// 錠のアイコン全体の大きさ（アイコン座標）。設計では 60x52 px の枠に縦横比を保って収める。
pub const ICON_W: f32 = 64.0;
pub const ICON_H: f32 = 56.0;
/// 錠の本体の左上（アイコン座標）。角丸の塗りつぶしで、鍵穴はない。
pub const BODY_X: f32 = 14.0;
pub const BODY_Y: f32 = 24.0;
/// 錠の本体の大きさ（アイコン座標）。y は 24〜52。
pub const BODY_W: f32 = 36.0;
pub const BODY_H: f32 = 28.0;
/// 錠の本体の角丸の半径（アイコン座標）。
pub const BODY_RADIUS: f32 = 6.0;
/// U 字の線の太さ（アイコン座標）。解除中も変えない（丸棒として扱うため）。
pub const SHACKLE_STROKE: f32 = 3.5;
```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test --lib anim::`
Expected: `test result: ok. 16 passed`（Plan 1 の 12 件 + 4 件）

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: 警告・差分なし（出たら直す。`cargo fmt` で直してよい）。

- [ ] **Step 5: コミットする**

```bash
git status
git diff
git add src/anim.rs
git commit -m "feat: export padlock body geometry from anim

The overlay draws the body from these constants instead of
repeating the numbers from the design.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: `date_format` を読み込み時に検証する

**Files:**
- Modify: `Cargo.toml`, `Cargo.lock`, `src/config.rs`

**Interfaces:**
- Consumes: `chrono::format::StrftimeItems::parse`（契約 2 章）
- Produces:
  - `config::DEFAULT_DATE_FORMAT: &str`（= `"%m/%d"`）
  - `config::is_valid_date_format(format: &str) -> bool`
  - `config::parse` が不正な `appearance.date_format` を `DEFAULT_DATE_FORMAT` に戻す（警告を出す）

- [ ] **Step 1: chrono を追加する**

Run:

```bash
cargo add chrono@0.4.45 --features unstable-locales
```

Expected: `Cargo.toml` に `chrono = { version = "0.4.45", features = ["unstable-locales"] }` が入る。`unstable-locales` は `pure-rust-locales` を引き込む（Task 5 で曜日などをロケールに合わせるのに使う）。

- [ ] **Step 2: 失敗するテストを書く**

`src/config.rs` のテストモジュールの `fn wrong_type_is_an_error()` の前に追加する:

```rust
    #[test]
    fn valid_date_formats_are_kept() {
        for format in ["%m/%d", "%Y-%m-%d (%a)", "%-d %B", ""] {
            assert!(is_valid_date_format(format), "{format}");
            let text = format!("[appearance]\ndate_format = {format:?}");
            assert_eq!(parse(&text).unwrap().config.appearance.date_format, format);
        }
    }

    #[test]
    fn invalid_date_format_falls_back_to_default() {
        for format in ["%Q", "%", "%m/%"] {
            assert!(!is_valid_date_format(format), "{format}");
            let text = format!("[appearance]\ndate_format = {format:?}");
            assert_eq!(
                parse(&text).unwrap().config.appearance.date_format,
                DEFAULT_DATE_FORMAT
            );
        }
    }
```

- [ ] **Step 3: テストが失敗することを確認する**

Run: `cargo test --lib config::`
Expected: コンパイルエラー（`cannot find function is_valid_date_format`）。

- [ ] **Step 4: 実装を書く**

`src/config.rs` の `impl Default for Appearance` の中の `date_format: "%m/%d".into(),` を次に置き換える:

```rust
            date_format: DEFAULT_DATE_FORMAT.into(),
```

`impl Default for Appearance { ... }` のブロックの直後に追加する:

```rust
/// `appearance.date_format` の初期値。不正な書式のときもこれに戻す。
pub const DEFAULT_DATE_FORMAT: &str = "%m/%d";

/// chrono の strftime 形式として正しいか。
///
/// 不正な書式のまま `to_string()` すると panic するので、読み込み時に確かめる。
pub fn is_valid_date_format(format: &str) -> bool {
    chrono::format::StrftimeItems::new(format).parse().is_ok()
}

/// 不正な `date_format` を初期値に戻す。
fn sanitize_date_format(appearance: &mut Appearance) {
    if !is_valid_date_format(&appearance.date_format) {
        log::warn!(
            "appearance.date_format {:?} is not a valid strftime format; using {:?}",
            appearance.date_format,
            DEFAULT_DATE_FORMAT
        );
        appearance.date_format = DEFAULT_DATE_FORMAT.into();
    }
}
```

`pub fn parse` の中で、`serde_ignored::deserialize` で設定を作る文を `mut` で型付きの束縛にし、その直後に検証を呼ぶ（Plan 2・3 は `parse` を変えていないので、次の形になる）:

```rust
    let mut config: Config =
        serde_ignored::deserialize(value, |path| unknown_keys.push(path.to_string()))
            .map_err(|e| ConfigError(e.to_string()))?;
    sanitize_date_format(&mut config.appearance);
    Ok(Loaded {
        config,
        unknown_keys,
    })
```

- [ ] **Step 5: テストが通ることを確認する**

Run: `cargo test --lib config::`
Expected: `test result: ok. 19 passed`（Plan 2 の時点の 17 件 + 2 件）

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: 警告・差分なし。

- [ ] **Step 6: コミットする（依存と実装を分ける）**

```bash
git status
git diff
git add Cargo.toml Cargo.lock
git commit -m "build: add chrono with locale support

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git add src/config.rs
git commit -m "fix: fall back to the default date format when it is invalid

An invalid strftime format panics when formatted, so check it once
when the config is loaded.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: 帯と柱の幅に上限を付ける

**Files:**
- Modify: `src/config.rs`

**Interfaces:**
- Consumes: Task 2 の `parse`
- Produces: `config::MAX_BAND_WIDTH: u32`（= 1920）。`parse` は `input.band_width` と `input.pillar_width` をこの値で丸める（警告を出す）

`lanes` の足し算は Plan 2 で飽和加算になっているので panic はしないが、`band_width = 4294967295` だと
レーンが約 9000 万本になり、打鍵のたびに空きレーンの一覧を作る `LaneAllocator::spawn` が重くなる。読み込み時に丸める。

- [ ] **Step 1: 失敗するテストを書く**

`src/config.rs` のテストモジュールの `fn wrong_type_is_an_error()` の前に追加する:

```rust
    #[test]
    fn huge_band_and_pillar_widths_are_clamped() {
        let text = "[input]\nband_width = 4294967295\npillar_width = 99999";
        let input = parse(text).unwrap().config.input;
        assert_eq!(input.band_width, MAX_BAND_WIDTH);
        assert_eq!(input.pillar_width, MAX_BAND_WIDTH);
    }

    #[test]
    fn normal_widths_are_kept() {
        let input = parse("[input]\nband_width = 960\npillar_width = 24")
            .unwrap()
            .config
            .input;
        assert_eq!((input.band_width, input.pillar_width), (960, 24));
    }
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test --lib config::`
Expected: コンパイルエラー（`cannot find value MAX_BAND_WIDTH`）。

- [ ] **Step 3: `config.rs` を直す**

`src/config.rs` の `impl Default for Input { ... }` のブロックの直後に追加する:

```rust
/// `input.band_width` と `input.pillar_width` の上限（1920x1080 基準の px）。
///
/// 巨大な値を入れるとレーン数が膨らみ、打鍵ごとの割り当てが重くなるので丸める。
pub const MAX_BAND_WIDTH: u32 = 1920;

/// 帯と柱の幅を上限で丸める。
fn clamp_input(input: &mut Input) {
    if input.band_width > MAX_BAND_WIDTH {
        log::warn!(
            "input.band_width {} is too large; using {MAX_BAND_WIDTH}",
            input.band_width
        );
        input.band_width = MAX_BAND_WIDTH;
    }
    if input.pillar_width > MAX_BAND_WIDTH {
        log::warn!(
            "input.pillar_width {} is too large; using {MAX_BAND_WIDTH}",
            input.pillar_width
        );
        input.pillar_width = MAX_BAND_WIDTH;
    }
}
```

`pub fn parse` の `sanitize_date_format(&mut config.appearance);` の次の行に追加する:

```rust
    clamp_input(&mut config.input);
```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test --lib config::`
Expected: `test result: ok. 21 passed`

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: 警告・差分なし。

- [ ] **Step 5: コミットする**

```bash
git status
git diff
git add src/config.rs
git commit -m "fix: clamp huge band and pillar widths from the config

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: `appearance.font` を足す

**Files:**
- Modify: `src/config.rs`

**Interfaces:**
- Consumes: Plan 1 の `config::Appearance`
- Produces:
  - `config::Appearance::font: String`（fontconfig のファミリー名。初期値 `"Inter"`。契約 4 章）
  - `config::DEFAULT_FONT: &str`（= `"Inter"`）

値の検証はここではしない（空や存在しない名前でもロックは続ける。Task 12 で fontconfig に尋ね、使えなければ埋め込みの Inter に戻して理由をログに出す）。

- [ ] **Step 1: 失敗するテストを書く**

`src/config.rs` のテストモジュールの `fn wrong_type_is_an_error()` の前に追加する:

```rust
    #[test]
    fn font_defaults_to_inter() {
        let loaded = parse("").unwrap();
        assert_eq!(loaded.config.appearance.font, "Inter");
        assert_eq!(Appearance::default().font, DEFAULT_FONT);
    }

    #[test]
    fn font_can_be_chosen() {
        let loaded = parse("[appearance]\nfont = \"Noto Sans\"").unwrap();
        assert!(loaded.unknown_keys.is_empty());
        assert_eq!(loaded.config.appearance.font, "Noto Sans");
        // ほかの見た目の設定は初期値のまま
        assert_eq!(loaded.config.appearance.fade_in_ms, 1500);
        assert_eq!(loaded.config.appearance.date_format, DEFAULT_DATE_FORMAT);
    }

    #[test]
    fn font_must_be_a_string() {
        assert!(parse("[appearance]\nfont = 500").is_err());
    }
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test --lib config::`
Expected: コンパイルエラー（`cannot find value DEFAULT_FONT`、`no field font on type config::Appearance`）。

- [ ] **Step 3: 実装を書く**

`src/config.rs` の `pub struct Appearance` の `pub date_format: String,` の次に足す:

```rust
    /// 時計・日付のフォント（fontconfig のファミリー名）。見つからなければ埋め込みの Inter。
    pub font: String,
```

`impl Default for Appearance` の `date_format: DEFAULT_DATE_FORMAT.into(),` の次に足す:

```rust
            font: DEFAULT_FONT.into(),
```

`/// \`appearance.date_format\` の初期値。` の doc コメントの前（`pub const DEFAULT_DATE_FORMAT` の上）に足す:

```rust
/// `appearance.font` の初期値。バイナリに埋め込んだフォントと同じファミリー。
pub const DEFAULT_FONT: &str = "Inter";

```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test --lib config::`
Expected: `test result: ok. 24 passed`（既存の `empty_file_gives_defaults` の `Config::default()` との比較もそのまま通る）

Run: `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: すべて通る、警告・差分なし（ほかに `Appearance { .. }` を書いている所はない）。

- [ ] **Step 5: コミットする**

```bash
git status
git diff
git add src/config.rs
git commit -m "feat: add appearance.font to choose the clock and date font

Defaults to Inter, which is also embedded as the fallback.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: `scene` — 日付の書式とロケール

**Files:**
- Create: `src/scene.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: Task 2 の `config::{DEFAULT_DATE_FORMAT, is_valid_date_format}`、chrono（`unstable-locales`）
- Produces:
  - `scene::time_locale_name<'a>(lc_all: Option<&'a str>, lc_time: Option<&'a str>, lang: Option<&'a str>) -> Option<&'a str>`
  - `scene::chrono_locale(name: Option<&str>) -> chrono::Locale`
  - `scene::language(name: Option<&str>) -> Option<String>`（fontconfig の `lang=` 用。`ja_JP.UTF-8` → `ja`）
  - `pub struct scene::DateStyle`（`Clone, Debug, PartialEq`）: `new(format: &str, locale: Locale) -> DateStyle`、`format(&self, t: &DateTime<FixedOffset>) -> String`（panic しない）
  - `scene::inter_covers(c: char) -> bool`、`scene::needs_fallback_font(style: &DateStyle) -> bool`
  - `scene::parse_fc_match(output: &str) -> Option<(String, u32)>`

- [ ] **Step 1: 失敗するテストを書く**

`src/scene.rs` を次の内容で作る（実装はまだ書かない）:

```rust
//! 1 フレーム分の重ね物（時計・日付・光の柱・錠）の内容と配置を計算する。
//!
//! 描画（egui・GL）には依存しない純粋なモジュール。
#![forbid(unsafe_code)]

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn jst(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<FixedOffset> {
        FixedOffset::east_opt(9 * 3600)
            .unwrap()
            .with_ymd_and_hms(y, m, d, h, min, 0)
            .unwrap()
    }

    #[test]
    fn locale_name_follows_glibc_priority() {
        let ja = Some("ja_JP.UTF-8");
        let en = Some("en_US.UTF-8");
        assert_eq!(time_locale_name(Some("de_DE"), ja, en), Some("de_DE"));
        assert_eq!(time_locale_name(Some(""), ja, en), Some("ja_JP.UTF-8"));
        assert_eq!(time_locale_name(None, None, en), Some("en_US.UTF-8"));
        assert_eq!(time_locale_name(None, Some(""), None), None);
    }

    #[test]
    fn locale_name_maps_to_chrono_locale() {
        assert_eq!(chrono_locale(Some("ja_JP.UTF-8")), Locale::ja_JP);
        assert_eq!(chrono_locale(Some("en_DK.UTF-8")), Locale::en_DK);
        assert_eq!(chrono_locale(Some("de_DE.UTF-8@euro")), Locale::de_DE_euro);
        assert_eq!(chrono_locale(Some("C.UTF-8")), Locale::POSIX);
        assert_eq!(chrono_locale(Some("POSIX")), Locale::POSIX);
        assert_eq!(chrono_locale(Some("xx_YY")), Locale::POSIX);
        assert_eq!(chrono_locale(None), Locale::POSIX);
    }

    #[test]
    fn language_is_the_lowercase_prefix() {
        assert_eq!(language(Some("ja_JP.UTF-8")).as_deref(), Some("ja"));
        assert_eq!(language(Some("fil_PH")).as_deref(), Some("fil"));
        assert_eq!(language(Some("C.UTF-8")), None);
        assert_eq!(language(Some("POSIX")), None);
        assert_eq!(language(Some("x:y")), None);
        assert_eq!(language(None), None);
    }

    #[test]
    fn default_date_format_is_month_and_day() {
        let style = DateStyle::new("%m/%d", Locale::POSIX);
        assert_eq!(style.format(&jst(2026, 11, 24, 10, 30)), "11/24");
    }

    #[test]
    fn date_follows_the_locale() {
        let t = jst(2026, 11, 24, 10, 30);
        assert_eq!(DateStyle::new("%a", Locale::ja_JP).format(&t), "火");
        assert_eq!(DateStyle::new("%a", Locale::POSIX).format(&t), "Tue");
    }

    #[test]
    fn invalid_date_format_uses_the_default() {
        let style = DateStyle::new("%Q", Locale::POSIX);
        assert_eq!(style.format(&jst(2026, 1, 2, 0, 0)), "01/02");
    }

    #[test]
    fn fallback_font_is_needed_only_for_uncovered_text() {
        let needs =
            |format: &str, locale: Locale| needs_fallback_font(&DateStyle::new(format, locale));
        assert!(!needs("%m/%d", Locale::ja_JP));
        assert!(!needs("%a %d %b", Locale::en_US));
        assert!(!needs("%A %B", Locale::ru_RU));
        assert!(!needs("%A %B", Locale::de_DE));
        assert!(needs("%m/%d (%a)", Locale::ja_JP));
        assert!(needs("%m月%d日", Locale::POSIX));
    }

    #[test]
    fn fc_match_output_is_parsed() {
        assert_eq!(
            parse_fc_match("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc\n2"),
            Some((
                "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc".into(),
                2
            ))
        );
        assert_eq!(parse_fc_match("/a/b.ttf\n"), Some(("/a/b.ttf".into(), 0)));
        assert_eq!(parse_fc_match(""), None);
        assert_eq!(parse_fc_match("\n0"), None);
    }
}
```

`src/lib.rs` のモジュール宣言の `pub mod render;` の次の行に足す:

```rust
pub mod scene;
```

あわせて、`src/lib.rs` の doc コメントの純粋なモジュールの並びに `scene` を足す。次の行:

```rust
//! `anim` `config` `lanes` `password` `state` `clock` `input` は Wayland にも GPU にも依存しない
```

を、次にする:

```rust
//! `anim` `config` `lanes` `password` `state` `clock` `input` `scene` は Wayland にも GPU にも依存しない
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test --lib scene::`
Expected: コンパイルエラー（`cannot find type DateTime`、`cannot find function time_locale_name` など）。

- [ ] **Step 3: 実装を書く**

`src/scene.rs` の `#![forbid(unsafe_code)]` の行の直後（その下の空行と `#[cfg(test)]` の前）に追加する:

```rust

use std::fmt::Write as _;

use chrono::{DateTime, FixedOffset, Locale};

use crate::config;

// ---------------------------------------------------------------------------
// 日付の書式とロケール

/// glibc と同じ優先順（`LC_ALL` → `LC_TIME` → `LANG`）で、日時のロケール名を選ぶ。
/// 空の値は飛ばす。どれもなければ `None`。
pub fn time_locale_name<'a>(
    lc_all: Option<&'a str>,
    lc_time: Option<&'a str>,
    lang: Option<&'a str>,
) -> Option<&'a str> {
    [lc_all, lc_time, lang]
        .into_iter()
        .flatten()
        .find(|v| !v.is_empty())
}

/// ロケール名（例: `ja_JP.UTF-8`、`de_DE.UTF-8@euro`）を chrono のロケールにする。
/// 文字コードの部分は外す。知らない名前・`C`・`POSIX`・`None` は `POSIX`。
pub fn chrono_locale(name: Option<&str>) -> Locale {
    let Some(name) = name else {
        return Locale::POSIX;
    };
    let (base, modifier) = match name.split_once('@') {
        Some((base, modifier)) => (base, Some(modifier)),
        None => (name, None),
    };
    let base = base.split('.').next().unwrap_or(base);
    if let Some(modifier) = modifier
        && let Ok(locale) = Locale::try_from(format!("{base}@{modifier}").as_str())
    {
        return locale;
    }
    Locale::try_from(base).unwrap_or(Locale::POSIX)
}

/// ロケール名から言語（`ja_JP.UTF-8` → `ja`）を取り出す。fontconfig の `lang=` に使う。
/// `C`・`POSIX`・英小文字 2〜3 字でないものは `None`（fontconfig のパターンに余計な文字を入れない）。
pub fn language(name: Option<&str>) -> Option<String> {
    let name = name?;
    let lang = name.split(['_', '.', '@']).next().unwrap_or("");
    let valid = (2..=3).contains(&lang.len()) && lang.bytes().all(|b| b.is_ascii_lowercase());
    valid.then(|| lang.to_owned())
}

/// 日付の書式とロケール。書式は作るときに確かめ、不正なら初期値に戻す。
#[derive(Clone, Debug, PartialEq)]
pub struct DateStyle {
    format: String,
    locale: Locale,
}

impl DateStyle {
    pub fn new(format: &str, locale: Locale) -> DateStyle {
        let format = if config::is_valid_date_format(format) {
            format
        } else {
            config::DEFAULT_DATE_FORMAT
        };
        DateStyle {
            format: format.to_owned(),
            locale,
        }
    }

    /// 日付を整形する。整形に失敗しても panic せず、初期値の書式で整形し直す。
    pub fn format(&self, t: &DateTime<FixedOffset>) -> String {
        let mut out = String::new();
        if write!(out, "{}", t.format_localized(&self.format, self.locale)).is_ok() {
            return out;
        }
        t.format(config::DEFAULT_DATE_FORMAT).to_string()
    }
}

/// Inter が持っている文字か（ラテン・ギリシャ・キリル文字と主な記号）。
/// ここにない文字（日本語など）は fontconfig で探したフォントで補う。
pub fn inter_covers(c: char) -> bool {
    matches!(
        c as u32,
        0x20..=0x7E
            | 0xA0..=0x24F
            | 0x370..=0x3FF
            | 0x400..=0x52F
            | 0x1E00..=0x1EFF
            | 0x2000..=0x206F
            | 0x20A0..=0x20CF
            | 0x2100..=0x214F
    )
}

/// この書式とロケールの日付に、Inter にない文字が出うるか。
/// 12 か月と 7 曜日、午前・午後を整形して確かめる。
pub fn needs_fallback_font(style: &DateStyle) -> bool {
    let utc = FixedOffset::east_opt(0).expect("zero offset is valid");
    let months = (1..=12).map(|m| (2026, m, 1, 0));
    // 2026-11-23 は月曜日。正午にして午後の表記も確かめる
    let weekdays = (23..=29).map(|d| (2026, 11, d, 12));
    months.chain(weekdays).any(|(y, m, d, h)| {
        let t = chrono::TimeZone::with_ymd_and_hms(&utc, y, m, d, h, 0, 0)
            .single()
            .expect("sample dates are valid");
        !style.format(&t).chars().all(inter_covers)
    })
}

/// `fc-match --format='%{file}\n%{index}'` の出力からファイル名とフェイスの番号を取り出す。
pub fn parse_fc_match(output: &str) -> Option<(String, u32)> {
    let mut lines = output.lines();
    let file = lines.next()?.trim();
    if file.is_empty() {
        return None;
    }
    let index = lines
        .next()
        .and_then(|l| l.trim().parse().ok())
        .unwrap_or(0);
    Some((file.to_owned(), index))
}
```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test --lib scene::`
Expected: `test result: ok. 8 passed`

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: 警告・差分なし。

- [ ] **Step 5: コミットする**

```bash
git status
git diff
git add src/scene.rs
git commit -m "feat: add scene module with locale-aware date formatting

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git add src/lib.rs
git commit -m "feat: register the scene module

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

（1 ファイル 1 コミットに分ける。先に `src/scene.rs` だけをコミットしても、`src/lib.rs` が宣言するまではコンパイルされないので、どちらのコミットでもビルドは通る。`src/lib.rs` のコミットはモジュールの宣言と doc コメントの 1 語だけ。）

---

### Task 6: `scene` — 配置・柱の色・U 字の折れ線

**Files:**
- Modify: `src/scene.rs`

**Interfaces:**
- Consumes: Task 1 の `anim::{ICON_W, ICON_H}`、Plan 1 の `anim::{PillarFrame, ShacklePath}`
- Produces:
  - `pub struct scene::Layout`（`Clone, Copy, Debug, PartialEq`。フィールドは下のコード）と `scene::layout(width: u32, height: u32, band_width: u32) -> Layout`
  - `pub const scene::{REFERENCE_HEIGHT, BAND_H}: f32`
  - `pub struct scene::PillarDraw { pub x: f32, pub width: f32, pub frame: anim::PillarFrame }`（契約どおり。`x`・`width` は帯の左端を 0 とした 1920x1080 基準の px）
  - `scene::pillar_rect(l: &Layout, p: &PillarDraw) -> [f32; 4]`（左, 上, 右, 下）
  - `pub const scene::PILLAR_COLUMNS: [f32; 6]`、`pub const scene::PILLAR_ROWS: usize`、`scene::pillar_alpha(u: f32, v: f32) -> f32`
  - `scene::shackle_polyline(path: &ShacklePath, segments_per_arc: usize) -> Vec<[f32; 2]>`

配置の数値（設計書 3 章と `docs/design/source/Prototype.dc.html` の CSS から。1920x1080 基準）:

| 要素 | 値 |
|---|---|
| 錠の領域 | 上端 130、高さ 52（アイコン 64x56 を 60x52 の枠に縦横比を保って収める → 1 単位 = 52/56 px）。左右中央 |
| 時計 | 錠の下余白 18 + 間隔 14 の下に行の高さ 104（line-height 1）。行の中心 y = 130 + 52 + 18 + 14 + 52 = 266。104px、字間 -2 |
| 日付 | 時計の行の下端 318 + 間隔 14 → 行の上端 332。26px、字間 1。行の高さは Inter の 1.21 倍 ≈ 31.5 |
| 帯 | 時計グループの下端（332 + 31.5 = 363.5）+ 374 → 上端 737.5、高さ 72 → 下端 809.5。幅は `band_width` で中央寄せ |

（`docs/design/images/lock-screen.png` を測ると、柱の下端 809、数字の上端 227・下端 304 で、上の値と合う。）

- [ ] **Step 1: 失敗するテストを書く**

`src/scene.rs` のテストモジュールの `fn jst(...)` の前に追加する:

```rust
    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

```

テストモジュールの最後（`fn fc_match_output_is_parsed` の後）に追加する:

```rust

    // --- 配置

    #[test]
    fn layout_matches_the_design_at_1080p() {
        let l = layout(1920, 1080, 768);
        assert_eq!(l.scale, 1.0);
        assert_eq!(l.center_x, 960.0);
        assert_eq!(l.icon_origin[1], 130.0);
        // アイコン 64 単位の幅が中央にくる
        assert!(close(l.icon_origin[0] + 32.0 * l.icon_unit, 960.0));
        assert!(close(l.icon_unit * 56.0, 52.0));
        assert_eq!(l.clock_center_y, 266.0);
        assert_eq!(l.clock_size, 104.0);
        assert_eq!(l.date_top, 332.0);
        assert_eq!(l.date_size, 26.0);
        assert_eq!(l.band_left, 576.0);
        assert_eq!(l.band_top, 737.5);
        assert_eq!(l.band_bottom, 809.5);
    }

    #[test]
    fn layout_scales_with_the_output_height() {
        let l = layout(3440, 1440, 768);
        let s = 1440.0 / 1080.0;
        assert!(close(l.scale, s));
        assert_eq!(l.center_x, 1720.0);
        assert!(close(l.clock_size, 104.0 * s));
        assert!(close(l.band_bottom, 809.5 * s));
        assert!(close(l.band_left, 1720.0 - 384.0 * s));
        assert!(close(l.clock_letter_spacing, -2.0 * s));
    }

    #[test]
    fn pillar_grows_from_the_band_bottom() {
        let l = layout(1920, 1080, 768);
        let p = PillarDraw {
            x: 8.0,
            width: 32.0,
            frame: PillarFrame {
                scale_y: 0.5,
                opacity: 1.0,
            },
        };
        assert_eq!(pillar_rect(&l, &p), [584.0, 773.5, 616.0, 809.5]);
    }

    #[test]
    fn pillar_alpha_has_bright_edges_and_fades_upward() {
        assert!(close(pillar_alpha(0.0, 1.0), 0.85));
        assert!(close(pillar_alpha(1.0, 1.0), 0.85));
        assert!(close(pillar_alpha(0.22, 1.0), 0.38));
        assert!(close(pillar_alpha(0.5, 1.0), 0.38));
        assert!(close(pillar_alpha(0.78, 1.0), 0.38));
        assert!(close(pillar_alpha(0.11, 1.0), (0.85 + 0.38) / 2.0));
        assert_eq!(pillar_alpha(0.0, 0.0), 0.0);
        assert!(close(pillar_alpha(0.5, 0.5), 0.19));
    }

    #[test]
    fn pillar_columns_include_the_gradient_stops() {
        for stop in [0.0, 0.22, 0.78, 1.0] {
            assert!(PILLAR_COLUMNS.contains(&stop));
        }
        assert!(PILLAR_COLUMNS.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn closed_shackle_polyline_runs_from_short_leg_to_long_leg() {
        let path = anim::shackle_path(anim::shackle_at(0.0));
        let pts = shackle_polyline(&path, 8);
        assert_eq!(pts.len(), 2 + 8 * 2 + 1);
        assert_eq!(pts[0], [22.0, 26.0]);
        assert_eq!(pts[1], [22.0, 18.0]);
        assert!(pts.contains(&[32.0, 8.0]));
        assert_eq!(*pts.last().unwrap(), [42.0, 40.0]);
        for p in &pts {
            assert!((22.0 - 1e-3..=42.0 + 1e-3).contains(&p[0]));
            assert!((8.0 - 1e-3..=40.0).contains(&p[1]));
        }
        // 同じ点が続かない（継ぎ目の計算が壊れないように）
        assert!(pts.windows(2).all(|w| w[0] != w[1]));
    }

    #[test]
    fn edge_on_shackle_is_a_single_rod() {
        let path = anim::shackle_path(anim::shackle_at(530.0));
        let pts = shackle_polyline(&path, 8);
        assert_eq!(pts.len(), 2);
        assert!(close(pts[0][0], anim::LONG_LEG_X));
        assert_eq!(pts[1], path.long_leg_bottom);
        assert!(pts[0][1] < pts[1][1]);
    }

    #[test]
    fn swivelled_shackle_polyline_is_finite() {
        for ms in (0..=800).step_by(20) {
            let path = anim::shackle_path(anim::shackle_at(ms as f32));
            let pts = shackle_polyline(&path, 12);
            assert!(pts.len() >= 2);
            assert!(pts.iter().flatten().all(|v| v.is_finite()));
        }
    }
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test --lib scene::`
Expected: コンパイルエラー（`cannot find function layout`、`cannot find struct PillarDraw` など）。

- [ ] **Step 3: 実装を書く**

`src/scene.rs` の `use crate::config;` の行の前に追加する:

```rust
use crate::anim::{self, PillarFrame, ShacklePath};
```

`pub fn parse_fc_match` 関数の後（テストモジュールの前）に追加する:

```rust
// ---------------------------------------------------------------------------
// 配置（設計書 3 章。1920x1080 基準の値を出力の高さに比例させる）

/// 配置の基準にする画面の高さ。
pub const REFERENCE_HEIGHT: f32 = 1080.0;
/// 錠の領域の上端（時計グループの上端）。
const LOCK_TOP: f32 = 130.0;
/// 錠の領域の高さ。アイコン（64x56）を 60x52 の枠に縦横比を保って収める。
const LOCK_BOX_H: f32 = 52.0;
/// 時計の行の中心。錠 52 + 下余白 18 + 間隔 14 の下に、行の高さ 104（line-height 1）。
const CLOCK_CENTER_Y: f32 = LOCK_TOP + LOCK_BOX_H + 18.0 + 14.0 + CLOCK_SIZE / 2.0;
const CLOCK_SIZE: f32 = 104.0;
const CLOCK_LETTER_SPACING: f32 = -2.0;
/// 日付の行の上端（時計の行の下端 + 間隔 14）。
const DATE_TOP: f32 = CLOCK_CENTER_Y + CLOCK_SIZE / 2.0 + 14.0;
const DATE_SIZE: f32 = 26.0;
const DATE_LETTER_SPACING: f32 = 1.0;
/// 日付の行の高さ。Inter の行の高さ（(ascender + descender) / UPM ≈ 1.21）× 26px。
const DATE_LINE_H: f32 = 31.5;
/// 時計グループの下端から帯の上端まで。
const BAND_GAP: f32 = 374.0;
/// 帯（光の柱）の高さ。
pub const BAND_H: f32 = 72.0;

/// 出力 1 枚分の配置（px、出力の左上が原点）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    /// 1920x1080 基準からの拡大率（出力の高さ / 1080）。
    pub scale: f32,
    pub center_x: f32,
    /// 錠のアイコン座標 (0, 0) の位置。
    pub icon_origin: [f32; 2],
    /// アイコン座標 1 単位の長さ。
    pub icon_unit: f32,
    pub clock_center_y: f32,
    pub clock_size: f32,
    pub clock_letter_spacing: f32,
    pub date_top: f32,
    pub date_size: f32,
    pub date_letter_spacing: f32,
    pub band_left: f32,
    pub band_top: f32,
    pub band_bottom: f32,
}

pub fn layout(width: u32, height: u32, band_width: u32) -> Layout {
    let s = height as f32 / REFERENCE_HEIGHT;
    let center_x = width as f32 / 2.0;
    let icon_unit = LOCK_BOX_H / anim::ICON_H * s;
    let band_top = DATE_TOP + DATE_LINE_H + BAND_GAP;
    Layout {
        scale: s,
        center_x,
        icon_origin: [center_x - anim::ICON_W / 2.0 * icon_unit, LOCK_TOP * s],
        icon_unit,
        clock_center_y: CLOCK_CENTER_Y * s,
        clock_size: CLOCK_SIZE * s,
        clock_letter_spacing: CLOCK_LETTER_SPACING * s,
        date_top: DATE_TOP * s,
        date_size: DATE_SIZE * s,
        date_letter_spacing: DATE_LETTER_SPACING * s,
        band_left: center_x - band_width as f32 * s / 2.0,
        band_top: band_top * s,
        band_bottom: (band_top + BAND_H) * s,
    }
}

/// 表示中の光の柱 1 本。`x`・`width` は帯の左端を 0 とした 1920x1080 基準の px。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PillarDraw {
    pub x: f32,
    pub width: f32,
    pub frame: PillarFrame,
}

/// 柱を描く四角形（左, 上, 右, 下）。下端を基準に縦に `scale_y` 倍する。
pub fn pillar_rect(l: &Layout, p: &PillarDraw) -> [f32; 4] {
    let left = l.band_left + p.x * l.scale;
    let right = left + p.width * l.scale;
    let top = l.band_bottom - BAND_H * l.scale * p.frame.scale_y;
    [left, top, right, l.band_bottom]
}

/// 柱の横方向の色の区切り（0 = 左端、1 = 右端）。両端 0.85 → 22% で 0.38 → 78% まで 0.38 → 右端 0.85。
/// 区切りの間は直線なので、この位置に頂点を置けば横方向は正確になる。
/// 端の区間は縦の変化と掛け合わさるので 2 つに分けて誤差を減らす。
pub const PILLAR_COLUMNS: [f32; 6] = [0.0, 0.11, 0.22, 0.78, 0.89, 1.0];
/// 柱の縦方向の分割数。
pub const PILLAR_ROWS: usize = 8;

/// 柱の中の位置（`u` 横 0〜1、`v` 縦 0 = 上端〜1 = 下端）の不透明度。
/// 横のグラデーションと、上が透明・下が不透明の縦のマスクの積。
pub fn pillar_alpha(u: f32, v: f32) -> f32 {
    const EDGE: f32 = 0.85;
    const MID: f32 = 0.38;
    let u = u.clamp(0.0, 1.0);
    let across = if u < 0.22 {
        EDGE + (MID - EDGE) * (u / 0.22)
    } else if u <= 0.78 {
        MID
    } else {
        MID + (EDGE - MID) * ((u - 0.78) / 0.22)
    };
    across * v.clamp(0.0, 1.0)
}

/// 短い足と長い足がこれより近いときは、真横から見た 1 本の棒として描く。
const EDGE_ON: f32 = 0.5;

/// U 字を折れ線（アイコン座標）にする。短い足の下端から始まり、長い足の下端で終わる。
/// 各ベジェは `segments_per_arc` 個に分ける。描画側は最初の点に丸い端を付ける。
///
/// 短い足が長い足と重なる（真横から見た）ときは、頂点から長い足の下端までの 2 点を返す
/// （行って戻る折れ線は線の継ぎ目が壊れるため）。
pub fn shackle_polyline(path: &ShacklePath, segments_per_arc: usize) -> Vec<[f32; 2]> {
    let apex = path.arc1[2];
    if (path.short_leg_top[0] - path.arc2[2][0]).abs() < EDGE_ON {
        return vec![apex, path.long_leg_bottom];
    }
    let n = segments_per_arc.max(1);
    let mut points = vec![path.short_leg_bottom, path.short_leg_top];
    for (start, [c1, c2, end]) in [(path.short_leg_top, path.arc1), (apex, path.arc2)] {
        for i in 1..=n {
            points.push(cubic(start, c1, c2, end, i as f32 / n as f32));
        }
    }
    points.push(path.long_leg_bottom);
    points
}

fn cubic(p0: [f32; 2], p1: [f32; 2], p2: [f32; 2], p3: [f32; 2], t: f32) -> [f32; 2] {
    let u = 1.0 - t;
    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    [
        a * p0[0] + b * p1[0] + c * p2[0] + d * p3[0],
        a * p0[1] + b * p1[1] + c * p2[1] + d * p3[1],
    ]
}

```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test --lib scene::`
Expected: `test result: ok. 16 passed`

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: 警告・差分なし。

- [ ] **Step 5: コミットする**

```bash
git status
git diff
git add src/scene.rs
git commit -m "feat: compute overlay layout, pillar gradient and shackle polyline

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: `scene` — 1 フレーム分の内容（`Scene` と `build`）

**Files:**
- Modify: `src/scene.rs`

**Interfaces:**
- Consumes: Plan 1 の `state::{Machine, Phase}`（`lock_visible`）、Plan 2 の `Phase::elapsed_ms`、Plan 1 の `anim::{fade 以外の各関数}`、`lanes::{lane_x, Pillar}`、`config::Input`、Task 5・6
- Produces（契約 4 章の形）:
  - `pub struct scene::LockDraw { pub path: anim::ShacklePath, pub shake_x: f32 }`（`shake_x` は 1920x1080 基準の px）
  - `pub struct scene::Scene { pub opacity: f32, pub time_text: String, pub date_text: String, pub pillars: Vec<PillarDraw>, pub lock: Option<LockDraw> }`
  - `scene::build(machine: &Machine, now_ms: u64, fade_alpha: f32, pillars: &[lanes::Pillar], input: &config::Input, wall: &DateTime<FixedOffset>, date: &DateStyle) -> Scene`

`build` の決まり:

- `opacity = clamp(fade_alpha) × (1 − reveal)`。`reveal` は解除中なら `anim::reveal_progress(経過)`、`Done` なら 1、それ以外 0。
- 錠は `machine.lock_visible()` のときだけ。確認中は閉じた形、失敗中は閉じた形を `anim::shake_offset(経過)` だけ横にずらす、解除中は `anim::shackle_path(anim::shackle_at(経過))`。経過は `Phase::elapsed_ms`。
- 柱は入力を受け付けている間だけ。入力待ちはすべて、失敗中は失敗した後に始まった柱だけ（試作どおり Enter で柱を消す。`LaneAllocator` を消す処理を App に足さずに済む）。各柱は `lanes::lane_x` と `anim::pillar_at(経過, pillar_fade_ms)`。
- 時刻は `wall` の `%H:%M`、日付は `DateStyle::format`。

- [ ] **Step 1: 失敗するテストを書く**

`src/scene.rs` のテストモジュールの `use super::*;` の次の行に追加する:

```rust
    use crate::state::{Event, FAIL_HOLD_MS};
```

テストモジュールの最後に追加する:

```rust

    // --- 1 フレーム分の内容

    fn input() -> Input {
        Input::default()
    }

    fn date() -> DateStyle {
        DateStyle::new("%m/%d", Locale::POSIX)
    }

    fn scene(m: &Machine, now: u64, fade: f32, pillars: &[Pillar]) -> Scene {
        build(
            m,
            now,
            fade,
            pillars,
            &input(),
            &jst(2026, 11, 24, 9, 5),
            &date(),
        )
    }

    fn failed_at(t: u64) -> Machine {
        let mut m = Machine::new();
        m.handle(Event::Submit { empty: false }, t);
        m.handle(Event::PasswordChecked { ok: false }, t);
        m
    }

    fn unlocking_at(t: u64) -> Machine {
        let mut m = Machine::new();
        m.handle(Event::U2fSucceeded, t);
        m
    }

    #[test]
    fn idle_shows_clock_and_date_without_lock() {
        let s = scene(&Machine::new(), 0, 1.0, &[]);
        assert_eq!(s.time_text, "09:05");
        assert_eq!(s.date_text, "11/24");
        assert_eq!(s.opacity, 1.0);
        assert!(s.lock.is_none());
        assert!(s.pillars.is_empty());
    }

    #[test]
    fn opacity_follows_the_fade_in() {
        assert_eq!(scene(&Machine::new(), 0, 0.0, &[]).opacity, 0.0);
        assert!(close(scene(&Machine::new(), 0, 0.4, &[]).opacity, 0.4));
        assert_eq!(scene(&Machine::new(), 0, 7.0, &[]).opacity, 1.0);
        assert_eq!(scene(&Machine::new(), 0, f32::NAN, &[]).opacity, 0.0);
    }

    #[test]
    fn opacity_is_fade_times_one_minus_reveal() {
        let m = unlocking_at(1000);
        let half = anim::REVEAL_START_MS + anim::REVEAL_MS / 2.0;
        let s = scene(&m, 1000 + half as u64, 0.8, &[]);
        assert!(close(s.opacity, 0.8 * 0.5));
        let s = scene(&m, 1000 + anim::REVEAL_START_MS as u64, 1.0, &[]);
        assert_eq!(s.opacity, 1.0);
        let s = scene(&m, 1000 + crate::state::UNLOCK_TOTAL_MS, 1.0, &[]);
        assert_eq!(s.opacity, 0.0);
    }

    #[test]
    fn verifying_shows_the_closed_lock() {
        let mut m = Machine::new();
        m.handle(Event::Submit { empty: false }, 10);
        let s = scene(&m, 500, 1.0, &[]);
        let lock = s.lock.unwrap();
        assert_eq!(lock.path, anim::shackle_path(anim::shackle_at(0.0)));
        assert_eq!(lock.shake_x, 0.0);
    }

    #[test]
    fn failure_shakes_the_closed_lock() {
        let m = failed_at(100);
        let at = 100 + (0.12 * anim::SHAKE_MS) as u64;
        let s = scene(&m, at, 1.0, &[]);
        let lock = s.lock.unwrap();
        assert!(close(lock.shake_x, anim::shake_offset(at as f32 - 100.0)));
        assert!(lock.shake_x < -9.0);
        assert_eq!(lock.path, anim::shackle_path(anim::shackle_at(0.0)));
        // 震え終わっても、入力待ちに戻るまでは錠を出したまま
        let s = scene(&m, 100 + FAIL_HOLD_MS - 1, 1.0, &[]);
        assert_eq!(s.lock.unwrap().shake_x, 0.0);
    }

    #[test]
    fn lock_hides_again_after_the_failure_hold() {
        let mut m = failed_at(100);
        m.handle(Event::Tick, 100 + FAIL_HOLD_MS);
        assert!(scene(&m, 100 + FAIL_HOLD_MS, 1.0, &[]).lock.is_none());
    }

    #[test]
    fn unlocking_opens_the_shackle() {
        let m = unlocking_at(1000);
        let s = scene(&m, 1000 + 400, 1.0, &[]);
        let lock = s.lock.unwrap();
        assert_eq!(lock.path, anim::shackle_path(anim::shackle_at(400.0)));
        assert_eq!(lock.shake_x, 0.0);
    }

    #[test]
    fn pillars_use_lane_positions_and_the_pillar_animation() {
        let pillars = [
            Pillar {
                lane: 0,
                started_ms: 1000,
            },
            Pillar {
                lane: 15,
                started_ms: 1100,
            },
        ];
        let s = scene(&Machine::new(), 1108, 1.0, &pillars);
        assert_eq!(s.pillars.len(), 2);
        assert_eq!(s.pillars[0].x, 8.0);
        assert_eq!(s.pillars[1].x, 728.0);
        assert_eq!(s.pillars[0].width, 32.0);
        assert_eq!(s.pillars[0].frame, anim::pillar_at(108.0, 900.0).unwrap());
        assert_eq!(s.pillars[1].frame, anim::pillar_at(8.0, 900.0).unwrap());
    }

    #[test]
    fn expired_pillars_are_not_drawn() {
        let pillars = [Pillar {
            lane: 3,
            started_ms: 0,
        }];
        assert!(
            scene(&Machine::new(), 900, 1.0, &pillars)
                .pillars
                .is_empty()
        );
    }

    #[test]
    fn pillars_hide_while_verifying_and_unlocking() {
        let pillars = [Pillar {
            lane: 3,
            started_ms: 0,
        }];
        let mut m = Machine::new();
        m.handle(Event::Submit { empty: false }, 10);
        assert!(scene(&m, 20, 1.0, &pillars).pillars.is_empty());
        assert!(
            scene(&unlocking_at(10), 20, 1.0, &pillars)
                .pillars
                .is_empty()
        );
    }

    #[test]
    fn after_a_failure_only_new_pillars_are_drawn() {
        let pillars = [
            Pillar {
                lane: 1,
                started_ms: 50,
            },
            Pillar {
                lane: 2,
                started_ms: 150,
            },
        ];
        let s = scene(&failed_at(100), 200, 1.0, &pillars);
        assert_eq!(s.pillars.len(), 1);
        assert_eq!(s.pillars[0].x, lanes::lane_x(2, 768, 32));
    }

    #[test]
    fn pillar_width_comes_from_the_config() {
        let wide = Input {
            pillar_width: 64,
            ..Input::default()
        };
        let pillars = [Pillar {
            lane: 0,
            started_ms: 0,
        }];
        let s = build(
            &Machine::new(),
            10,
            1.0,
            &pillars,
            &wide,
            &jst(2026, 11, 24, 9, 5),
            &date(),
        );
        assert_eq!(s.pillars[0].width, 64.0);
        assert_eq!(s.pillars[0].x, lanes::lane_x(0, 768, 64));
    }
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test --lib scene::`
Expected: コンパイルエラー（`cannot find type Machine`、`cannot find function build` など）。

- [ ] **Step 3: 実装を書く**

`src/scene.rs` の `use` を次のようにする（`use crate::config;` を置き換え、2 行足す）:

```rust
use crate::anim::{self, PillarFrame, ShacklePath};
use crate::config::{self, Input};
use crate::lanes::{self, Pillar};
use crate::state::{Machine, Phase};
```

`fn cubic(...)` の後（テストモジュールの前）に追加する:

```rust
// ---------------------------------------------------------------------------
// 1 フレーム分の内容

/// 錠。`path` はアイコン座標、`shake_x` は 1920x1080 基準の px（右が正）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LockDraw {
    pub path: ShacklePath,
    pub shake_x: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    /// 重ね物全体の不透明度。`fade_alpha * (1 - reveal_progress)`。
    pub opacity: f32,
    /// `HH:MM`。
    pub time_text: String,
    /// `date_format` で整形済みの日付。
    pub date_text: String,
    pub pillars: Vec<PillarDraw>,
    /// 錠を出すときだけ `Some`。
    pub lock: Option<LockDraw>,
}

/// 1 フレーム分の重ね物の内容を作る。
///
/// - `fade_alpha`: 背景と同じ黒からのフェードの不透明度（`anim::fade_alpha`）。
/// - `pillars`: `LaneAllocator::pillars(now_ms)` の結果（`input.pillar_fade_ms` と同じ寿命で割り当てたもの）。
/// - `wall`: 表示する時刻（呼び出し側で `Local::now().fixed_offset()`）。
pub fn build(
    machine: &Machine,
    now_ms: u64,
    fade_alpha: f32,
    pillars: &[Pillar],
    input: &Input,
    wall: &DateTime<FixedOffset>,
    date: &DateStyle,
) -> Scene {
    let phase = machine.phase();
    let elapsed = phase.elapsed_ms(now_ms).unwrap_or(0.0);
    let reveal = match phase {
        Phase::Unlocking { .. } => anim::reveal_progress(elapsed),
        Phase::Done => 1.0,
        _ => 0.0,
    };
    let fade = if fade_alpha.is_nan() {
        0.0
    } else {
        fade_alpha.clamp(0.0, 1.0)
    };

    let closed = anim::shackle_path(anim::shackle_at(0.0));
    let lock = machine.lock_visible().then(|| match phase {
        Phase::Unlocking { .. } => LockDraw {
            path: anim::shackle_path(anim::shackle_at(elapsed)),
            shake_x: 0.0,
        },
        Phase::Failed { .. } => LockDraw {
            path: closed,
            shake_x: anim::shake_offset(elapsed),
        },
        _ => LockDraw {
            path: closed,
            shake_x: 0.0,
        },
    });

    // 柱は入力を受け付けている間だけ。失敗中は、失敗した後に打った柱だけを出す
    // （Enter より前の柱は試作と同じく消す）。
    let shown_since = match phase {
        Phase::Idle => Some(0),
        Phase::Failed { since_ms } => Some(since_ms),
        _ => None,
    };
    let life_ms = input.pillar_fade_ms as f32;
    let pillars = match shown_since {
        Some(since) => pillars
            .iter()
            .filter(|p| p.started_ms >= since)
            .filter_map(|p| {
                let frame = anim::pillar_at(now_ms.saturating_sub(p.started_ms) as f32, life_ms)?;
                Some(PillarDraw {
                    x: lanes::lane_x(p.lane, input.band_width, input.pillar_width),
                    width: input.pillar_width as f32,
                    frame,
                })
            })
            .collect(),
        None => Vec::new(),
    };

    Scene {
        opacity: fade * (1.0 - reveal),
        time_text: wall.format("%H:%M").to_string(),
        date_text: date.format(wall),
        pillars,
        lock,
    }
}

```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test --lib scene::`
Expected: `test result: ok. 28 passed`

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: 警告・差分なし。

- [ ] **Step 5: コミットする**

```bash
git status
git diff
git add src/scene.rs
git commit -m "feat: build the per-frame overlay scene from lock state

Opacity is fade times one minus reveal, the padlock shows only while
the state machine says so, and pillars follow the lane allocator.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: `scene` — 主画面の追跡

**Files:**
- Modify: `src/scene.rs`

**Interfaces:**
- Consumes: Plan 1 の `config::{select_primary, OutputInfo}`（空のルールを無視する修正は Plan 2 Task 1 で済んでいる）
- Produces:
  - `pub struct scene::PrimaryTracker`（`Debug`）
  - `PrimaryTracker::new(rules: Vec<String>, focused: Option<String>) -> PrimaryTracker`
  - `PrimaryTracker::update<'a, I>(&mut self, outputs: I) -> Option<usize> where I: IntoIterator<Item = (&'a str, &'a str)>, I::IntoIter: Clone`（出力名, description。一覧が前回と同じなら選び直さず、文字列も確保しない）
  - `PrimaryTracker::primary(&self) -> Option<usize>`

- [ ] **Step 1: 失敗するテストを書く**

`src/scene.rs` のテストモジュールの最後に追加する:

```rust

    // --- 主画面

    fn outs<'a>(
        list: &'a [(&'a str, &'a str)],
    ) -> impl Iterator<Item = (&'a str, &'a str)> + Clone {
        list.iter().copied()
    }

    #[test]
    fn primary_uses_rules_then_focused_then_first() {
        let list = [
            ("eDP-1", "Lenovo (eDP-1)"),
            ("DP-10", "LG ULTRAWIDE (DP-10)"),
        ];
        let mut by_rule = PrimaryTracker::new(vec!["LG ULTRAWIDE".into()], Some("eDP-1".into()));
        assert_eq!(by_rule.update(outs(&list)), Some(1));
        let mut by_focus = PrimaryTracker::new(vec![], Some("DP-10".into()));
        assert_eq!(by_focus.update(outs(&list)), Some(1));
        let mut first = PrimaryTracker::new(vec![], None);
        assert_eq!(first.update(outs(&list)), Some(0));
        assert_eq!(first.primary(), Some(0));
    }

    #[test]
    fn primary_is_chosen_again_when_outputs_change() {
        let mut t = PrimaryTracker::new(vec!["DP-10".into()], None);
        let both = [("eDP-1", ""), ("DP-10", "")];
        assert_eq!(t.update(outs(&both)), Some(1));
        // 主画面が抜かれたら残りから選び直す
        let laptop = [("eDP-1", "")];
        assert_eq!(t.update(outs(&laptop)), Some(0));
        // 挿し直したら戻る（並びが変わっても名前で選ぶ）
        let again = [("DP-10", ""), ("eDP-1", "")];
        assert_eq!(t.update(outs(&again)), Some(0));
        assert_eq!(t.update(outs(&[])), None);
    }

    #[test]
    fn primary_starts_unknown() {
        assert_eq!(PrimaryTracker::new(vec![], None).primary(), None);
    }
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test --lib scene::`
Expected: コンパイルエラー（`cannot find type PrimaryTracker`）。

- [ ] **Step 3: 実装を書く**

`src/scene.rs` の `use crate::config::{self, Input};` を次に置き換える:

```rust
use crate::config::{self, Input, OutputInfo};
```

`pub fn build(...)` の後（テストモジュールの前）に追加する:

```rust
// ---------------------------------------------------------------------------
// 主画面

/// 主画面の選択を覚えておき、出力の一覧が変わったときだけ選び直す。
#[derive(Debug)]
pub struct PrimaryTracker {
    rules: Vec<String>,
    focused: Option<String>,
    seen: Vec<OutputInfo>,
    primary: Option<usize>,
}

impl PrimaryTracker {
    /// `rules` は `display.primary`、`focused` はロック前にフォーカスしていた出力名（sway IPC）。
    pub fn new(rules: Vec<String>, focused: Option<String>) -> PrimaryTracker {
        PrimaryTracker {
            rules,
            focused,
            seen: Vec::new(),
            primary: None,
        }
    }

    /// 出力の一覧（出力名, description）を渡し、主画面の添字を返す。
    /// 一覧が前回と同じなら選び直さない（毎フレーム呼んでも文字列を確保しない）。
    pub fn update<'a, I>(&mut self, outputs: I) -> Option<usize>
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
        I::IntoIter: Clone,
    {
        let outputs = outputs.into_iter();
        let same = outputs.clone().count() == self.seen.len()
            && outputs
                .clone()
                .zip(&self.seen)
                .all(|((name, desc), seen)| seen.name == name && seen.description == desc);
        if !same {
            self.seen = outputs
                .map(|(name, desc)| OutputInfo {
                    name: name.to_owned(),
                    description: desc.to_owned(),
                })
                .collect();
            self.primary = config::select_primary(&self.rules, &self.seen, self.focused.as_deref());
        }
        self.primary
    }

    /// 最後に `update` で選んだ主画面。
    pub fn primary(&self) -> Option<usize> {
        self.primary
    }
}

```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test --lib scene::`
Expected: `test result: ok. 31 passed`

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: 警告・差分なし。

- [ ] **Step 5: コミットする**

```bash
git status
git diff
git add src/scene.rs
git commit -m "feat: track the primary output across output changes

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Inter を置く

**Files:**
- Create: `assets/fonts/Inter-Medium.ttf`, `assets/fonts/Inter-Regular.ttf`, `assets/fonts/LICENSE.txt`

**Interfaces:**
- Consumes: なし
- Produces: Task 12 の `include_bytes!` が読むフォント（Inter 4.1 の静的 TTF。Medium = weight 500、Regular = weight 400）と OFL のライセンス文

**ダウンロードには実行時にユーザーの承認が要る。** Step 1 で承認を得てから進む。承認が得られなければ BLOCKED として止まる。

- [ ] **Step 1: ダウンロードの承認を得る**

ユーザーに次を伝え、承認を待つ:

- ファイル: `Inter-4.1.zip`（33,707,794 バイト、約 32 MiB）
- 取得元: rsms/inter の GitHub リリース v4.1 `https://github.com/rsms/inter/releases/download/v4.1/Inter-4.1.zip`
- 使うのは中の `extras/ttf/Inter-Medium.ttf`、`extras/ttf/Inter-Regular.ttf`、`LICENSE.txt` だけ

- [ ] **Step 2: ダウンロードして検証する**

Run:

```bash
mkdir -p /tmp/kabi-inter
curl -fL -o /tmp/kabi-inter/Inter-4.1.zip https://github.com/rsms/inter/releases/download/v4.1/Inter-4.1.zip
stat -c %s /tmp/kabi-inter/Inter-4.1.zip
sha256sum /tmp/kabi-inter/Inter-4.1.zip
```

Expected:

```text
33707794
9883fdd4a49d4fb66bd8177ba6625ef9a64aa45899767dde3d36aa425756b11e  /tmp/kabi-inter/Inter-4.1.zip
```

上流はチェックサムを公開していない（GitHub API のこのアセットの `digest` も `null`）。この SHA-256 は Arch Linux の `inter-font` 4.1-1 の PKGBUILD（`https://gitlab.archlinux.org/archlinux/packaging/packages/inter-font/-/raw/main/PKGBUILD`）に載っている値で、別の経路から同じファイルを確かめるために使う。一致しなければ展開せずに BLOCKED として止まり、`/tmp/kabi-inter` を消す。

- [ ] **Step 3: 中身を確かめて取り出す**

Run:

```bash
unzip -l /tmp/kabi-inter/Inter-4.1.zip | grep -E ' (extras/ttf/Inter-(Medium|Regular)\.ttf|LICENSE\.txt)$'
```

Expected: `extras/ttf/Inter-Medium.ttf`、`extras/ttf/Inter-Regular.ttf`、`LICENSE.txt` の 3 行。3 行そろわないときは、Step 2 の SHA-256 と合っている以上は計画の前提（中のパス）が違うので、別のパスを探して使うことはせず、取り出さずに BLOCKED として止まり、`rm -r /tmp/kabi-inter` で消して、`unzip -l` の出力を添えて報告する。

Run（リポジトリのルートで）:

```bash
mkdir -p assets/fonts
unzip -j /tmp/kabi-inter/Inter-4.1.zip extras/ttf/Inter-Medium.ttf extras/ttf/Inter-Regular.ttf LICENSE.txt -d assets/fonts/
fc-scan --format '%{family} %{weight}\n' assets/fonts/Inter-Medium.ttf assets/fonts/Inter-Regular.ttf
head -n 3 assets/fonts/LICENSE.txt
sha256sum assets/fonts/*
```

Expected: `fc-scan` が `Inter,Inter Medium 100`（fontconfig の weight 100 = Medium）と `Inter 80`（80 = Regular）のように出す。`LICENSE.txt` の先頭に `Copyright (c) 2016 The Inter Project Authors` と SIL Open Font License の文がある。最後の `sha256sum` の 3 行は、Step 5 でコミットメッセージに（コマンドで）入れる。

**注意（本物の Inter では未確認）:** この計画を書いたときは Inter をダウンロードせず、Noto Sans を代わりに置いて Task 12・15 まで確かめた。そのため、この Step の `fc-scan` の期待値、Task 12 の `embedded_fonts_are_inter_with_the_right_weights`、Task 6 の日付の行の高さ（Inter の 1.21 倍 ≈ 31.5、`DATE_LINE_H`）は、本物の Inter ではまだ一度も動かしていない。ここや Task 12 で食い違ったときは、コードやテストを結果に合わせて直さない。まず Step 2〜3 で取り出したファイル（`extras/ttf/` の静的な Medium / Regular か。`InterVariable`・`InterDisplay` を取り出していないか）を確かめ直し、ファイルが正しいのに食い違うなら BLOCKED として報告する。

- [ ] **Step 4: 後片付け**

Run: `rm -r /tmp/kabi-inter`

- [ ] **Step 5: コミットする**

```bash
git status
git add assets/fonts/Inter-Medium.ttf assets/fonts/Inter-Regular.ttf assets/fonts/LICENSE.txt
git diff --cached --stat
git commit -m "chore: vendor Inter 4.1 Medium and Regular (OFL-1.1)" \
  -m "From https://github.com/rsms/inter/releases/download/v4.1/Inter-4.1.zip
(sha256 9883fdd4a49d4fb66bd8177ba6625ef9a64aa45899767dde3d36aa425756b11e,
matching the Arch Linux inter-font 4.1-1 PKGBUILD):
extras/ttf/Inter-Medium.ttf, extras/ttf/Inter-Regular.ttf and LICENSE.txt." \
  -m "$(sha256sum assets/fonts/Inter-Medium.ttf assets/fonts/Inter-Regular.ttf assets/fonts/LICENSE.txt)" \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git log -1 --format=%B
```

Expected: `git log` の本文に、置いた 3 つのファイルの `sha256sum` の 3 行（Step 3 の最後の出力と同じ）が入っている。`-m` ごとに段落になる。

（フォント 2 つとライセンス文は「上流の配布物をそのまま置く」1 つの操作なのでまとめる。）

---

### Task 10: `fonts` — 時計・日付のフォントの選び方（純粋）

**Files:**
- Create: `src/fonts.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: なし（文字列の処理だけ）
- Produces:
  - `pub const fonts::{CLOCK_WEIGHT, DATE_WEIGHT}: f32`（500 / 400）、`pub const fonts::FC_LIST_FORMAT: &str`
  - `pub enum fonts::FcWeight { Fixed(f32), Range(f32, f32) }`、`pub struct fonts::FcFace { file, index, families, weight, slant, format }`
  - `pub struct fonts::FacePick { pub file: String, pub face_index: u32, pub weight: f32, pub variable_weight: Option<f32> }`
  - `pub enum fonts::Fallback { EmptyName, FontconfigUnavailable(String), NotFound(String), NoUsableFace(String), Unreadable { file, error }, Unusable { file, error } }`（`Display` は英語の理由）
  - `fonts::fc_pattern(family: &str) -> String`、`fonts::parse_fc_list(output: &str) -> Vec<FcFace>`
  - `fonts::css_weight(fc: f32) -> f32`、`fonts::weight_preference(desired: f32, available: f32) -> (u8, u32)`
  - `fonts::pick_faces(family: &str, faces: &[FcFace]) -> Result<(FacePick, FacePick), Fallback>`（時計, 日付）

決めごと（`fc-list` の出力はこの計画を書いた環境で確かめたもの）:

- `fc-list --format='%{file}\t%{index}\t%{family}\t%{weight}\t%{slant}\t%{fontformat}\n' '<ファミリー>'` は、そのファミリーのフェイスだけを返す（大文字・小文字と空白は無視。見つからなければ何も出さず終了コード 0）。`fc-match` は無いファミリーでも代わりのフォントを返すので使わない。
- `weight` は fontconfig の単位（80 = Regular、100 = Medium、200 = Bold）。可変フォントの本体は `[0 210]` のような範囲、名前付きインスタンスは `index` の上位 16 bit に番号が入る（例: `65536`）。`css_weight` で CSS の太さに直す（fontconfig の `FcWeightToOpenType` と同じ表）。
- **500 / 400 がないとき:** 同じファミリーの中で、CSS Fonts 4 の太さの照合と同じ順に一番近い太さを使う。埋め込みの Inter に戻すのは、名前が空・ファミリーがない・直立（`slant` 0）で egui が読める形式（`TrueType`・`CFF`）のフェイスがない、のときだけ（ファイルが読めない・壊れているは Task 12 の入出力の側で判断する）。理由: 利用者はファミリーを選んだのであり、太さが 1 段違うだけで別のフォントに替わるより、同じファミリーの近い太さの方が意図に近い。順番を CSS に合わせると、設計（ブラウザで作った）と同じ選び方になる。
- 同じ太さなら 静的なフェイス → 名前付きインスタンス → 可変の範囲 の順に選ぶ（ヒンティングと読み込みの軽さ）。可変フォント・名前付きインスタンスは `variable_weight` に `wght` 軸の値を入れる。

- [ ] **Step 1: 失敗するテストを書く**

`src/fonts.rs` を次の内容で作る（実装はまだ書かない）:

```rust
//! 時計・日付のフォントを、fontconfig の一覧から選ぶ。
//!
//! fontconfig を呼ぶ・ファイルを読むのは `render::overlay`。ここは一覧の読み取りと、
//! どのファイルを使うか・いつ埋め込みの Inter に戻すかの判断だけを持つ純粋なモジュール。
#![forbid(unsafe_code)]

#[cfg(test)]
mod tests {
    use super::*;

    const NOTO: &str = "\
/usr/share/fonts/noto/NotoSans-Black.ttf\t0\tNoto Sans,Noto Sans Black\t210\t0\tTrueType
/usr/share/fonts/noto/NotoSans-MediumItalic.ttf\t0\tNoto Sans,Noto Sans Medium\t100\t100\tTrueType
/usr/share/fonts/noto/NotoSans-Medium.ttf\t0\tNoto Sans,Noto Sans Medium\t100\t0\tTrueType
/usr/share/fonts/noto/NotoSans-Italic.ttf\t0\tNoto Sans\t80\t100\tTrueType
/usr/share/fonts/noto/NotoSans-Regular.ttf\t0\tNoto Sans\t80\t0\tTrueType
/usr/share/fonts/noto/NotoSans-Bold.ttf\t0\tNoto Sans\t200\t0\tTrueType
";

    const ADWAITA: &str = "\
/usr/share/fonts/Adwaita/AdwaitaSans-Italic.ttf\t0\tAdwaita Sans\t[0 210]\t100\tTrueType
/usr/share/fonts/Adwaita/AdwaitaSans-Regular.ttf\t0\tAdwaita Sans\t[0 210]\t0\tTrueType
";

    fn face(file: &str, index: u32, weight: FcWeight) -> FcFace {
        FcFace {
            file: file.into(),
            index,
            families: vec!["Test Sans".into()],
            weight,
            slant: 0,
            format: "TrueType".into(),
        }
    }

    fn fixed(list: &[(&str, f32)]) -> Vec<FcFace> {
        list.iter()
            .map(|(file, w)| face(file, 0, FcWeight::Fixed(*w)))
            .collect()
    }

    #[test]
    fn pattern_escapes_special_characters() {
        assert_eq!(fc_pattern("Noto Sans"), "Noto Sans");
        assert_eq!(fc_pattern(r"A-B:C,D\E"), r"A\-B\:C\,D\\E");
    }

    #[test]
    fn fc_list_output_is_parsed() {
        let faces = parse_fc_list(ADWAITA);
        assert_eq!(faces.len(), 2);
        assert_eq!(
            faces[1],
            FcFace {
                file: "/usr/share/fonts/Adwaita/AdwaitaSans-Regular.ttf".into(),
                index: 0,
                families: vec!["Adwaita Sans".into()],
                weight: FcWeight::Range(0.0, 210.0),
                slant: 0,
                format: "TrueType".into(),
            }
        );
        let noto = parse_fc_list(NOTO);
        assert_eq!(noto[0].families, vec!["Noto Sans", "Noto Sans Black"]);
        assert_eq!(noto[0].weight, FcWeight::Fixed(210.0));
    }

    #[test]
    fn broken_lines_are_skipped() {
        let text = "only-a-file\n/a.ttf\tx\tA\t80\t0\tTrueType\n\n/b.ttf\t0\tB\t80\t0\tTrueType\n";
        let faces = parse_fc_list(text);
        assert_eq!(faces.len(), 1);
        assert_eq!(faces[0].file, "/b.ttf");
    }

    #[test]
    fn fontconfig_weights_map_to_css_weights() {
        assert_eq!(css_weight(80.0), 400.0);
        assert_eq!(css_weight(100.0), 500.0);
        assert_eq!(css_weight(200.0), 700.0);
        assert_eq!(css_weight(0.0), 100.0);
        assert_eq!(css_weight(210.0), 900.0);
        assert_eq!(css_weight(90.0), 450.0);
        assert_eq!(css_weight(-5.0), 100.0);
        assert_eq!(css_weight(300.0), 1000.0);
    }

    #[test]
    fn weight_preference_follows_css_matching() {
        let order = |desired: f32, list: &[f32]| {
            let mut v = list.to_vec();
            v.sort_by_key(|w| weight_preference(desired, *w));
            v
        };
        let all = [100.0, 300.0, 400.0, 500.0, 600.0, 700.0];
        assert_eq!(
            order(500.0, &all),
            [500.0, 400.0, 300.0, 100.0, 600.0, 700.0]
        );
        assert_eq!(
            order(400.0, &all),
            [400.0, 500.0, 300.0, 100.0, 600.0, 700.0]
        );
        assert_eq!(
            order(300.0, &all),
            [300.0, 100.0, 400.0, 500.0, 600.0, 700.0]
        );
        assert_eq!(
            order(700.0, &all),
            [700.0, 600.0, 500.0, 400.0, 300.0, 100.0]
        );
    }

    #[test]
    fn static_family_gets_exact_weights() {
        let (clock, date) = pick_faces("Noto Sans", &parse_fc_list(NOTO)).unwrap();
        assert_eq!(
            clock,
            FacePick {
                file: "/usr/share/fonts/noto/NotoSans-Medium.ttf".into(),
                face_index: 0,
                weight: 500.0,
                variable_weight: None,
            }
        );
        assert_eq!(date.file, "/usr/share/fonts/noto/NotoSans-Regular.ttf");
        assert_eq!(date.weight, 400.0);
    }

    #[test]
    fn family_names_ignore_case_and_spaces() {
        let (clock, _) = pick_faces("  notosans ", &parse_fc_list(NOTO)).unwrap();
        assert_eq!(clock.weight, 500.0);
    }

    #[test]
    fn missing_weights_use_the_nearest_in_css_order() {
        // 400 と 700 しかない: 時計（500）は 400、日付は 400
        let faces = fixed(&[("/r.ttf", 80.0), ("/b.ttf", 200.0)]);
        let (clock, date) = pick_faces("Test Sans", &faces).unwrap();
        assert_eq!((clock.file.as_str(), clock.weight), ("/r.ttf", 400.0));
        assert_eq!(date.file, "/r.ttf");
        // 300 と 700: 500 は細い方を先に見る
        let faces = fixed(&[("/l.ttf", 50.0), ("/b.ttf", 200.0)]);
        assert_eq!(pick_faces("Test Sans", &faces).unwrap().0.file, "/l.ttf");
        // 500 と 700: 日付（400）は 500 までの太い方を先に見る
        let faces = fixed(&[("/m.ttf", 100.0), ("/b.ttf", 200.0)]);
        let (clock, date) = pick_faces("Test Sans", &faces).unwrap();
        assert_eq!(
            (clock.file.as_str(), date.file.as_str()),
            ("/m.ttf", "/m.ttf")
        );
        // 1 つの太さしかなければ両方それ
        let faces = fixed(&[("/black.ttf", 210.0)]);
        let (clock, date) = pick_faces("Test Sans", &faces).unwrap();
        assert_eq!((clock.weight, date.weight), (900.0, 900.0));
    }

    #[test]
    fn variable_font_is_set_to_the_exact_weight() {
        let (clock, date) = pick_faces("Adwaita Sans", &parse_fc_list(ADWAITA)).unwrap();
        let file = "/usr/share/fonts/Adwaita/AdwaitaSans-Regular.ttf";
        assert_eq!(
            clock,
            FacePick {
                file: file.into(),
                face_index: 0,
                weight: 500.0,
                variable_weight: Some(500.0),
            }
        );
        assert_eq!(date.file, file);
        assert_eq!(date.variable_weight, Some(400.0));
    }

    #[test]
    fn static_faces_win_over_variable_ones_of_the_same_weight() {
        let faces = vec![
            face("/var.ttf", 0, FcWeight::Range(0.0, 210.0)),
            face("/var.ttf", 4 << 16, FcWeight::Fixed(100.0)),
            face("/medium.ttf", 0, FcWeight::Fixed(100.0)),
        ];
        let (clock, date) = pick_faces("Test Sans", &faces).unwrap();
        assert_eq!(clock.file, "/medium.ttf");
        assert_eq!(clock.variable_weight, None);
        // 400 の静的なフェイスはないので、可変フォントで正確な太さにする
        assert_eq!(date.file, "/var.ttf");
        assert_eq!(date.variable_weight, Some(400.0));
    }

    #[test]
    fn named_instance_uses_its_face_and_weight() {
        let faces = vec![face("/var.ttf", (5 << 16) | 2, FcWeight::Fixed(100.0))];
        let (clock, _) = pick_faces("Test Sans", &faces).unwrap();
        assert_eq!(clock.face_index, 2);
        assert_eq!(clock.variable_weight, Some(500.0));
    }

    #[test]
    fn falls_back_when_the_family_is_unusable() {
        assert_eq!(
            pick_faces("", &parse_fc_list(NOTO)),
            Err(Fallback::EmptyName)
        );
        assert_eq!(
            pick_faces("Nope", &parse_fc_list(NOTO)),
            Err(Fallback::NotFound("Nope".into()))
        );
        assert_eq!(
            pick_faces("Nope", &[]),
            Err(Fallback::NotFound("Nope".into()))
        );
        // イタリックしかない
        let mut italic = fixed(&[("/i.ttf", 80.0)]);
        italic[0].slant = 100;
        assert_eq!(
            pick_faces("Test Sans", &italic),
            Err(Fallback::NoUsableFace("Test Sans".into()))
        );
        // egui が読めない形式しかない
        let mut bitmap = fixed(&[("/b.pcf.gz", 80.0)]);
        bitmap[0].format = "PCF".into();
        assert_eq!(
            pick_faces("Test Sans", &bitmap),
            Err(Fallback::NoUsableFace("Test Sans".into()))
        );
    }

    #[test]
    fn fallback_reasons_are_readable() {
        assert_eq!(
            Fallback::NotFound("Nope".into()).to_string(),
            "no installed font family named \"Nope\""
        );
        assert_eq!(
            Fallback::Unusable {
                file: "/x.ttf".into(),
                error: "bad".into()
            }
            .to_string(),
            "/x.ttf is not a usable font: bad"
        );
    }
}
```

`src/lib.rs` のモジュール宣言の `pub mod config;` の次の行に足す:

```rust
pub mod fonts;
```

あわせて、`src/lib.rs` の doc コメントの純粋なモジュールの並びに `fonts` を足す。次の行（Task 5 で `scene` を足した後）:

```rust
//! `anim` `config` `lanes` `password` `state` `clock` `input` `scene` は Wayland にも GPU にも依存しない
```

を、次にする:

```rust
//! `anim` `config` `lanes` `password` `state` `clock` `input` `scene` `fonts` は Wayland にも GPU にも依存しない
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test --lib fonts::`
Expected: コンパイルエラー（`cannot find type FcWeight`、`cannot find function pick_faces` など）。

- [ ] **Step 3: 実装を書く**

`src/fonts.rs` の `#![forbid(unsafe_code)]` の行の直後（その下の空行と `#[cfg(test)]` の前）に追加する:

```rust

use std::fmt;

/// 時計の太さ（CSS / OpenType の weight）。
pub const CLOCK_WEIGHT: f32 = 500.0;
/// 日付の太さ。
pub const DATE_WEIGHT: f32 = 400.0;

/// `fc-list --format` に渡す書式。`parse_fc_list` はこの形を読む。
pub const FC_LIST_FORMAT: &str =
    "%{file}\t%{index}\t%{family}\t%{weight}\t%{slant}\t%{fontformat}\n";

/// fontconfig の weight。可変フォントは範囲（例: `[0 210]`）になる。値は fontconfig の単位。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FcWeight {
    Fixed(f32),
    Range(f32, f32),
}

/// `fc-list` が返したフェイス 1 つ。
#[derive(Clone, Debug, PartialEq)]
pub struct FcFace {
    pub file: String,
    /// fontconfig の index。下位 16 bit がファイルの中のフェイスの番号、
    /// 上位 16 bit が可変フォントの名前付きインスタンスの番号（0 ならインスタンスではない）。
    pub index: u32,
    /// ファミリー名（別名を含む。例: `Noto Sans`, `Noto Sans Medium`）。
    pub families: Vec<String>,
    pub weight: FcWeight,
    /// 0 なら直立（イタリック・斜体でない）。
    pub slant: i32,
    /// `TrueType`, `CFF` など。
    pub format: String,
}

/// 使うと決めたフォントファイル。
#[derive(Clone, Debug, PartialEq)]
pub struct FacePick {
    pub file: String,
    /// ファイルの中のフェイスの番号（egui の `FontData::index`）。
    pub face_index: u32,
    /// このフェイスで出る太さ（CSS の weight）。
    pub weight: f32,
    /// 可変フォントの `wght` 軸に設定する値。静的なフォントなら `None`。
    pub variable_weight: Option<f32>,
}

/// 埋め込みの Inter に戻した理由。ログに出す。
#[derive(Clone, Debug, PartialEq)]
pub enum Fallback {
    EmptyName,
    FontconfigUnavailable(String),
    NotFound(String),
    NoUsableFace(String),
    Unreadable { file: String, error: String },
    Unusable { file: String, error: String },
}

impl fmt::Display for Fallback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fallback::EmptyName => write!(f, "appearance.font is empty"),
            Fallback::FontconfigUnavailable(e) => write!(f, "fontconfig is unavailable: {e}"),
            Fallback::NotFound(family) => write!(f, "no installed font family named {family:?}"),
            Fallback::NoUsableFace(family) => write!(
                f,
                "font family {family:?} has no upright TrueType or OpenType face"
            ),
            Fallback::Unreadable { file, error } => write!(f, "cannot read {file}: {error}"),
            Fallback::Unusable { file, error } => write!(f, "{file} is not a usable font: {error}"),
        }
    }
}

/// fontconfig のパターンで特別な意味を持つ文字（`\` `-` `:` `,`）をエスケープする。
pub fn fc_pattern(family: &str) -> String {
    let mut out = String::with_capacity(family.len());
    for c in family.chars() {
        if matches!(c, '\\' | '-' | ':' | ',') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `fc-list --format=FC_LIST_FORMAT` の出力を読む。読めない行は飛ばす。
pub fn parse_fc_list(output: &str) -> Vec<FcFace> {
    output.lines().filter_map(parse_fc_line).collect()
}

fn parse_fc_line(line: &str) -> Option<FcFace> {
    let mut fields = line.split('\t');
    let file = fields.next()?.trim();
    let index = fields.next()?.trim().parse().ok()?;
    let families = fields
        .next()?
        .split(',')
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let weight = parse_weight(fields.next()?.trim())?;
    let slant = fields.next()?.trim().parse().ok()?;
    let format = fields.next()?.trim().to_owned();
    if file.is_empty() || families.is_empty() {
        return None;
    }
    Some(FcFace {
        file: file.to_owned(),
        index,
        families,
        weight,
        slant,
        format,
    })
}

fn parse_weight(text: &str) -> Option<FcWeight> {
    if let Some(range) = text.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
        let mut ends = range.split_whitespace().map(|v| v.parse::<f32>().ok());
        let (lo, hi) = (ends.next()??, ends.next()??);
        return Some(FcWeight::Range(lo.min(hi), lo.max(hi)));
    }
    text.parse().ok().map(FcWeight::Fixed)
}

/// fontconfig の weight を CSS / OpenType の weight にする（fontconfig の `FcWeightToOpenType` と同じ表）。
pub fn css_weight(fc: f32) -> f32 {
    const MAP: [(f32, f32); 12] = [
        (0.0, 100.0),
        (40.0, 200.0),
        (50.0, 300.0),
        (55.0, 350.0),
        (75.0, 380.0),
        (80.0, 400.0),
        (100.0, 500.0),
        (180.0, 600.0),
        (200.0, 700.0),
        (205.0, 800.0),
        (210.0, 900.0),
        (215.0, 1000.0),
    ];
    if fc <= MAP[0].0 {
        return MAP[0].1;
    }
    for w in MAP.windows(2) {
        let ((f0, c0), (f1, c1)) = (w[0], w[1]);
        if fc <= f1 {
            return c0 + (c1 - c0) * (fc - f0) / (f1 - f0);
        }
    }
    MAP[MAP.len() - 1].1
}

/// 欲しい太さ `desired` に対して、太さ `available` がどれだけ好ましいか（小さいほど良い）。
///
/// CSS Fonts 4 の「font-weight の照合」と同じ順番にする（ブラウザで設計を見たときと同じ選び方）:
/// 同じ太さ → 400〜500 を欲しいときは 500 までの太い方 → 細い方 → 500 より太い方。
/// 400 未満なら細い方 → 太い方、500 より上なら太い方 → 細い方。それぞれ近い順。
pub fn weight_preference(desired: f32, available: f32) -> (u8, u32) {
    let distance = ((available - desired).abs() * 10.0).round() as u32;
    let group = if available == desired {
        0
    } else if (400.0..=500.0).contains(&desired) {
        if available > desired && available <= 500.0 {
            1
        } else if available < desired {
            2
        } else {
            3
        }
    } else if desired < 400.0 {
        if available < desired { 1 } else { 2 }
    } else if available > desired {
        1
    } else {
        2
    };
    (group, distance)
}

/// ファミリー名の比較（fontconfig と同じく大文字・小文字と空白を無視する）。
fn same_family(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        s.chars()
            .filter(|c| !c.is_whitespace())
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    norm(a) == norm(b)
}

/// フェイスの種類の好み（静的 → 名前付きインスタンス → 可変の範囲）。
fn kind_rank(face: &FcFace) -> u8 {
    match face.weight {
        FcWeight::Fixed(_) if face.index >> 16 == 0 => 0,
        FcWeight::Fixed(_) => 1,
        FcWeight::Range(..) => 2,
    }
}

/// フェイス 1 つで出せる、`desired` に一番近い太さ（CSS の weight）。
fn provided_weight(face: &FcFace, desired: f32) -> f32 {
    match face.weight {
        FcWeight::Fixed(w) => css_weight(w),
        FcWeight::Range(lo, hi) => desired.clamp(css_weight(lo), css_weight(hi)),
    }
}

fn pick_one(faces: &[&FcFace], desired: f32) -> Option<FacePick> {
    let best = faces.iter().min_by(|a, b| {
        let key = |f: &FcFace| {
            (
                weight_preference(desired, provided_weight(f, desired)),
                kind_rank(f),
            )
        };
        key(a)
            .cmp(&key(b))
            .then_with(|| a.file.cmp(&b.file))
            .then_with(|| a.index.cmp(&b.index))
    })?;
    let weight = provided_weight(best, desired);
    let variable_weight = (kind_rank(best) > 0).then_some(weight);
    Some(FacePick {
        file: best.file.clone(),
        face_index: best.index & 0xFFFF,
        weight,
        variable_weight,
    })
}

/// `family` の中から時計用（500）と日付用（400）のフェイスを選ぶ。
///
/// 直立で、egui が読める形式（TrueType・CFF）のフェイスだけを使う。その太さがなければ
/// 同じファミリーの一番近い太さ（`weight_preference` の順）を使う。ファミリーがない・使える
/// フェイスがないときは `Err` で、呼び出し側は埋め込みの Inter を使う。
pub fn pick_faces(family: &str, faces: &[FcFace]) -> Result<(FacePick, FacePick), Fallback> {
    let family = family.trim();
    if family.is_empty() {
        return Err(Fallback::EmptyName);
    }
    let in_family: Vec<&FcFace> = faces
        .iter()
        .filter(|f| f.families.iter().any(|n| same_family(n, family)))
        .collect();
    if in_family.is_empty() {
        return Err(Fallback::NotFound(family.to_owned()));
    }
    let usable: Vec<&FcFace> = in_family
        .into_iter()
        .filter(|f| f.slant == 0 && matches!(f.format.as_str(), "TrueType" | "CFF"))
        .collect();
    match (
        pick_one(&usable, CLOCK_WEIGHT),
        pick_one(&usable, DATE_WEIGHT),
    ) {
        (Some(clock), Some(date)) => Ok((clock, date)),
        _ => Err(Fallback::NoUsableFace(family.to_owned())),
    }
}
```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test --lib fonts::`
Expected: `test result: ok. 13 passed`

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: 警告・差分なし。

- [ ] **Step 5: コミットする**

```bash
git status
git diff
git add src/fonts.rs
git commit -m "feat: choose clock and date faces from fontconfig results

Missing weights use the nearest one in CSS font matching order;
the embedded Inter is used only when the family has no usable face.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git add src/lib.rs
git commit -m "feat: register the fonts module

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

（Task 5 と同じく 1 ファイル 1 コミットに分ける。`src/fonts.rs` は `src/lib.rs` が宣言するまでコンパイルされないので、どちらのコミットでもビルドは通る。）

---

### Task 11: ロック前にフォーカスしていた出力（sway IPC）

**Files:**
- Modify: `Cargo.toml`, `Cargo.lock`, `src/wl/outputs.rs`

**Interfaces:**
- Consumes: `swayipc` 4.0.0（`Connection::new()`、`get_outputs()` の `focused` と `name`。契約 2 章）
- Produces: `wl::outputs::focused_output() -> Option<String>`（失敗したら `None`。設計書 5 章 1）

- [ ] **Step 1: swayipc を追加する**

Run: `cargo add swayipc@4.0.0`
Expected: `Cargo.toml` に `swayipc = "4.0.0"` が入る。

- [ ] **Step 2: 実装を書く**

`src/wl/outputs.rs` のテストモジュール（`#[cfg(test)]`）の前に追加する（Plan 2 の `OutputMeta` などはそのまま。テストモジュールの後ろに置くと clippy の `items_after_test_module` になる）:

```rust
/// ロック前に sway でフォーカスしていた出力の名前（sway IPC の `get_outputs` の `focused`）。
///
/// sway でない・`SWAYSOCK` がない・応答が不正なときは `None`（主画面は設定か最初の出力で決める）。
/// ロックの後はフォーカスの意味が変わるので、ロック前に 1 回だけ呼ぶ。
pub fn focused_output() -> Option<String> {
    let mut conn = match swayipc::Connection::new() {
        Ok(conn) => conn,
        Err(e) => {
            log::debug!("sway IPC unavailable: {e}");
            return None;
        }
    };
    match conn.get_outputs() {
        Ok(outputs) => outputs.into_iter().find(|o| o.focused).map(|o| o.name),
        Err(e) => {
            log::debug!("sway IPC get_outputs failed: {e}");
            None
        }
    }
}

```

（単体テストはない。sway に問い合わせる入出力だけの関数で、Task 15 でログを見て確かめる。）

- [ ] **Step 3: ビルドと lint**

Run: `cargo build && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: エラー・警告・差分なし。

- [ ] **Step 4: コミットする（依存と実装を分ける）**

```bash
git status
git diff
git add Cargo.toml Cargo.lock
git commit -m "build: add swayipc

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git add src/wl/outputs.rs
git commit -m "feat: ask sway which output was focused before locking

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 12: `Overlay` — フォントの読み込み、egui の準備と描画

**Files:**
- Modify: `Cargo.toml`, `Cargo.lock`, `src/render/mod.rs`
- Create: `src/render/overlay.rs`

**Interfaces:**
- Consumes: Plan 3 の `render::egl::Gpu::gl() -> &Arc<glow::Context>`、Task 5〜7 の `scene`、Task 10 の `fonts`、Task 1 の `anim` の定数、Task 4 の `config::DEFAULT_FONT`、Task 9 のフォント
- Produces（契約 4 章）:
  - `pub struct render::overlay::Overlay`
  - `Overlay::new(gpu: &Gpu, font_family: &str, date_needs_fallback: bool) -> Result<Overlay, String>`（GL が current であること。フォントを選んで登録し、シェーダー・数字と日付の字形のアトラス転送まで済ませる）
  - `Overlay::draw(&mut self, gpu: &Gpu, width: u32, height: u32, scene: &Scene, input: &config::Input)`（描画先は current にし、背景は描いておく。`width`・`height` は描画先のピクセル数）
  - `impl Drop for Overlay`（`Painter::destroy`）

フォントの読み込み（入出力の側。判断は Task 10 の `fonts`）:

1. `appearance.font` が空なら `Fallback::EmptyName`。
2. `fc-list --format=<FC_LIST_FORMAT> <fc_pattern(font)>` を実行する。起動できない・失敗したら `Fallback::FontconfigUnavailable`。
3. `fonts::pick_faces` で時計用・日付用のフェイスを選ぶ（同じファイルなら 1 回だけ読む）。
4. ファイルが読めなければ `Fallback::Unreadable`、`skrifa::FontRef::from_index` が失敗したら `Fallback::Unusable`（egui は読めないフォントで panic するため）。可変フォントは `wght` 軸を選んだ太さにする。
5. どれかで失敗したら、時計・日付とも埋め込みの Inter にする。ログ: 成功は `INFO clock font: <family> weight 500 (<file> face <n>); date font: weight 400 (<file> face <n>)`、戻したときは `WARN font "<family>" is not used (<理由>); using the embedded Inter`。初期値の `"Inter"` が入っていないだけのときは普通のことなので `INFO font "Inter" is not installed; using the embedded Inter`。
6. 字形を探す順は 選んだフォント → 埋め込みの Inter → 日本語などの補助フォント。

確認済みの API（egui / epaint / egui_glow 0.37.0、skrifa 0.44.0）:

- `egui_glow::Painter::new(Arc<glow::Context>, &str, Option<ShaderVersion>, bool) -> Result<Painter, PainterError>`（`PainterError: Display`）、`paint_and_update_textures([u32; 2], f32, &[ClippedPrimitive], &mut TexturesDelta)`、`destroy()`。`paint_primitives` の終わりに `SCISSOR_TEST` は戻すが、`BLEND`（`ONE, ONE_MINUS_SRC_ALPHA`）とプログラムは残す
- `egui::FontDefinitions::empty()`、`font_data: BTreeMap<String, Arc<FontData>>`、`families: BTreeMap<FontFamily, Vec<String>>`、`FontData::{from_static, from_blob(Blob, index)}`、`FontFamily::Name(Arc<str>)`、`Context::set_fonts`
- `epaint` は読めないフォントを `Family::new` で `panic!` する（`skrifa::FontRef::from_index` の失敗）
- egui の文字組みは harfrust だが OpenType の機能は指定できない。`TextFormat::extra_letter_spacing` で字間を足せる
- 可変フォントは `FontData::tweak(FontTweak { coords: VariationCoords::new([(b"wght", 500.0)]), .. })` で太さを決める（`epaint::text::VariationCoords`。`FontTweak::coords` は字形の作成と文字組みの両方に効く。Adwaita Sans で 100 / 400 / 500 / 900 の幅が変わることを確かめた）
- `Shape::{mesh, line, circle_filled, rect_filled, galley_with_override_text_color}`、`CornerRadius::same(u8)`、`Mesh::{colored_vertex, add_triangle}`、`Color32` は乗算済みアルファ（`from_white_alpha`）

- [ ] **Step 1: 依存を追加する**

Run:

```bash
cargo add egui@0.37 egui_glow@0.37 skrifa@0.44
cargo tree -i skrifa --depth 1
```

Expected: `skrifa` が 1 つの版（egui 0.37.0 の `epaint` と同じ 0.44.x）だけ出る。2 つの版が出たら、`epaint` が使っている版に `Cargo.toml` の `skrifa` を合わせる（`cargo add skrifa@<その版>`）。`glow` は Plan 3 で入っている（前提）。

- [ ] **Step 2: 失敗するテストを書く**

`src/render/mod.rs` の `pub mod egl;` の次の行に足す:

```rust
pub mod overlay;
```

`src/render/overlay.rs` を次の内容で作る（実装はまだ書かない）:

```rust
//! egui（egui_glow）による重ね描き: 時計・日付・光の柱・錠。主画面だけに描く。
//!
//! 何をどこに描くかは `scene`（純粋）で決め、ここでは egui の図形にして GL で描くだけにする。

#[cfg(test)]
mod tests {
    use super::*;
    use egui::epaint::text::{Fonts, TextOptions};
    use skrifa::MetadataProvider as _;
    use skrifa::attribute::Weight;
    use skrifa::string::StringId;

    #[test]
    fn embedded_fonts_are_inter_with_the_right_weights() {
        for (bytes, weight) in [
            (INTER_MEDIUM, Weight::MEDIUM),
            (INTER_REGULAR, Weight::NORMAL),
        ] {
            let font = skrifa::FontRef::new(bytes).unwrap();
            assert_eq!(font.attributes().weight, weight);
            let family: String = font
                .localized_strings(StringId::FAMILY_NAME)
                .english_or_first()
                .unwrap()
                .chars()
                .collect();
            assert!(
                family.starts_with("Inter") && !family.contains("Display"),
                "{family}"
            );
        }
    }

    #[test]
    fn egui_can_lay_out_the_clock_and_date() {
        let mut fonts = Fonts::new(TextOptions::default(), font_definitions(None, None));
        let mut view = fonts.with_pixels_per_point(1.0);
        let l = scene::layout(1920, 1080, 768);
        assert!(view.has_glyphs(&clock_font(&l), "0123456789:"));
        assert!(view.has_glyphs(&date_font(&l), "0123456789/ Tue Nov"));
        assert!(!view.has_glyphs(&date_font(&l), "火"));
    }

    #[test]
    fn embedded_inter_is_used_without_a_chosen_font() {
        let defs = font_definitions(None, None);
        assert_eq!(
            defs.families[&FontFamily::Name(CLOCK_FONT.into())],
            [INTER_MEDIUM_NAME]
        );
        assert_eq!(
            defs.families[&FontFamily::Name(DATE_FONT.into())],
            [INTER_REGULAR_NAME]
        );
    }

    #[test]
    fn chosen_font_comes_first_and_inter_fills_the_gaps() {
        let chosen = (
            FontData::from_static(INTER_MEDIUM),
            FontData::from_static(INTER_REGULAR),
        );
        let defs = font_definitions(Some(chosen), Some(FontData::from_static(INTER_REGULAR)));
        assert_eq!(
            defs.families[&FontFamily::Name(CLOCK_FONT.into())],
            [CHOSEN_CLOCK, INTER_MEDIUM_NAME, FALLBACK_FONT]
        );
        assert_eq!(
            defs.families[&FontFamily::Name(DATE_FONT.into())],
            [CHOSEN_DATE, INTER_REGULAR_NAME, FALLBACK_FONT]
        );
    }

    #[test]
    fn variable_faces_get_the_weight_axis() {
        let pick = FacePick {
            file: "embedded".into(),
            face_index: 0,
            weight: 500.0,
            variable_weight: Some(500.0),
        };
        let data = face_data(&pick, Arc::new(INTER_REGULAR.to_vec())).unwrap();
        assert_eq!(data.tweak.coords, VariationCoords::new([(b"wght", 500.0)]));
        let fixed = FacePick {
            variable_weight: None,
            ..pick
        };
        let data = face_data(&fixed, Arc::new(INTER_REGULAR.to_vec())).unwrap();
        assert_eq!(data.tweak.coords, VariationCoords::default());
    }

    #[test]
    fn unusable_font_files_are_rejected() {
        let pick = FacePick {
            file: "/broken.ttf".into(),
            face_index: 0,
            weight: 400.0,
            variable_weight: None,
        };
        let err = face_data(&pick, Arc::new(b"not a font".to_vec())).unwrap_err();
        assert!(matches!(err, Fallback::Unusable { ref file, .. } if file == "/broken.ttf"));
        assert!(matches!(
            read_font_file("/nonexistent/kabi-lock.ttf"),
            Err(Fallback::Unreadable { .. })
        ));
    }

    #[test]
    fn white_is_premultiplied() {
        assert_eq!(white(1.0), Color32::WHITE);
        assert_eq!(white(0.0), Color32::TRANSPARENT);
        assert_eq!(
            white(0.5),
            Color32::from_rgba_premultiplied(128, 128, 128, 128)
        );
        assert_eq!(white(2.0), Color32::WHITE);
    }
}
```

- [ ] **Step 3: テストが失敗することを確認する**

Run: `cargo test --lib render::overlay::`
Expected: コンパイルエラー（`cannot find value INTER_MEDIUM`、`cannot find function font_definitions` など）。

- [ ] **Step 4: 実装を書く**

`src/render/overlay.rs` の doc コメントの後の空行と、テストモジュール（`#[cfg(test)]`）の間に追加する:

```rust
use std::process::Command;
use std::sync::Arc;

use egui::epaint::text::VariationCoords;
use egui::text::LayoutJob;
use egui::{
    Color32, Context, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, FontTweak,
    LayerId, Mesh, Painter, RawInput, Rect, Shape, Stroke, TextFormat, pos2, vec2,
};
use glow::HasContext as _;

use crate::anim;
use crate::config;
use crate::fonts::{self, FacePick, Fallback};
use crate::render::egl::Gpu;
use crate::scene::{self, Layout, Scene};

/// 埋め込みの Inter（weight 500、時計用）。OFL。ライセンスは `assets/fonts/LICENSE.txt`。
/// `appearance.font` が使えないときの予備で、使えるときも字形が足りない分を補う。
static INTER_MEDIUM: &[u8] = include_bytes!("../../assets/fonts/Inter-Medium.ttf");
/// 埋め込みの Inter（weight 400、日付用）。
static INTER_REGULAR: &[u8] = include_bytes!("../../assets/fonts/Inter-Regular.ttf");

/// egui のファミリー名（時計・日付）。
const CLOCK_FONT: &str = "clock";
const DATE_FONT: &str = "date";
/// egui のフォント名。
const CHOSEN_CLOCK: &str = "chosen-clock";
const CHOSEN_DATE: &str = "chosen-date";
const INTER_MEDIUM_NAME: &str = "inter-medium";
const INTER_REGULAR_NAME: &str = "inter-regular";
const FALLBACK_FONT: &str = "fallback";

/// 時計と日付の色（白の不透明度。重ね物全体の不透明度を掛ける前の値）。
const CLOCK_ALPHA: f32 = 0.94;
const DATE_ALPHA: f32 = 0.74;
/// U 字のベジェ 1 本あたりの分割数。
const SHACKLE_SEGMENTS: usize = 16;

/// 時計・日付・光の柱・錠を描く。GL の準備（シェーダー・フォント・アトラス）はロック前に済ませる。
pub struct Overlay {
    ctx: Context,
    painter: egui_glow::Painter,
}

impl Overlay {
    /// 時計・日付のフォントを選んで登録し、`date_needs_fallback` なら fontconfig の
    /// 日本語などのフォントを補いに足す。
    ///
    /// フォントは fontconfig で `font_family`（`appearance.font`）を探し、太さ 500（時計）と
    /// 400（日付）のファイルを読む。使えなければ埋め込みの Inter にする（理由はログに出す）。
    /// 呼ぶときは `gpu` のコンテキストが current であること（`Gpu::new` の直後はサーフェスなしで current）。
    pub fn new(gpu: &Gpu, font_family: &str, date_needs_fallback: bool) -> Result<Overlay, String> {
        let painter = egui_glow::Painter::new(gpu.gl().clone(), "", None, false)
            .map_err(|e| format!("egui_glow painter: {e}"))?;
        let ctx = Context::default();
        let chosen = match system_fonts(font_family) {
            Ok((clock, date)) => Some((clock, date)),
            Err(reason) => {
                if reason == Fallback::NotFound(config::DEFAULT_FONT.into()) {
                    // 初期値の Inter が入っていないのは普通のこと
                    log::info!("font \"Inter\" is not installed; using the embedded Inter");
                } else {
                    log::warn!(
                        "font {font_family:?} is not used ({reason}); using the embedded Inter"
                    );
                }
                None
            }
        };
        let fallback = if date_needs_fallback {
            fallback_font()
        } else {
            None
        };
        ctx.set_fonts(font_definitions(chosen, fallback));
        let mut overlay = Overlay { ctx, painter };
        overlay.warm_up();
        Ok(overlay)
    }

    /// 1920x1080 基準の配置を、出力の高さに合わせて拡大して描く。
    ///
    /// `width`・`height` は描画先のピクセル数。呼ぶ前に描画先のサーフェスを current にし、
    /// 背景を描いておく（重ね物は上に重ねるだけで、画面を消さない）。
    pub fn draw(
        &mut self,
        gpu: &Gpu,
        width: u32,
        height: u32,
        scene: &Scene,
        input: &config::Input,
    ) {
        if scene.opacity <= 0.0 || width == 0 || height == 0 {
            return;
        }
        let layout = scene::layout(width, height, input.band_width);
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                vec2(width as f32, height as f32),
            )),
            ..Default::default()
        };
        let out = self.ctx.run_pass(raw, |ctx| {
            paint_scene(&ctx.layer_painter(LayerId::background()), &layout, scene);
        });
        let primitives = self.ctx.tessellate(out.shapes, out.pixels_per_point);
        let mut textures = out.textures_delta;
        self.painter.paint_and_update_textures(
            [width, height],
            out.pixels_per_point,
            &primitives,
            &mut textures,
        );
        // egui が変えた状態のうち、背景の描画に響くものを既定に戻す
        // （SCISSOR_TEST は egui_glow が自分で戻す）
        unsafe {
            gpu.gl().disable(glow::BLEND);
            gpu.gl().use_program(None);
        }
    }

    /// フォントを読み込み、時計の数字と日付の字形をアトラスに載せて GPU へ送っておく。
    /// 1080p の大きさで作るので、それ以外の高さの主画面では最初のフレームで足りない字形を作る。
    fn warm_up(&mut self) {
        let layout = scene::layout(1920, 1080, 768);
        let out = self.ctx.run_pass(RawInput::default(), |ctx| {
            let painter = ctx.layer_painter(LayerId::background());
            let _ =
                painter.layout_no_wrap("0123456789:".into(), clock_font(&layout), Color32::WHITE);
            let _ =
                painter.layout_no_wrap("0123456789/".into(), date_font(&layout), Color32::WHITE);
        });
        let mut textures = out.textures_delta;
        // 何も描かずにテクスチャだけ送る（サーフェスなしの current でも描画命令は出さない）
        self.painter
            .paint_and_update_textures([1, 1], out.pixels_per_point, &[], &mut textures);
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        // GL の後片付け。終了時にコンテキストがなくても GL 呼び出しが無視されるだけで害はない
        self.painter.destroy();
    }
}

fn clock_font(l: &Layout) -> FontId {
    FontId::new(l.clock_size, FontFamily::Name(CLOCK_FONT.into()))
}

fn date_font(l: &Layout) -> FontId {
    FontId::new(l.date_size, FontFamily::Name(DATE_FONT.into()))
}

/// 白に不透明度を付けた色（egui の色は乗算済みアルファ）。
fn white(alpha: f32) -> Color32 {
    Color32::from_white_alpha((alpha.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// egui に登録するフォント。
///
/// 時計は 選んだ太さ 500 → 埋め込みの Inter 500 → `fallback`、日付は 選んだ太さ 400 → Inter 400 →
/// `fallback` の順に字形を探す（選んだフォントにない文字は Inter で補う）。
fn font_definitions(
    chosen: Option<(FontData, FontData)>,
    fallback: Option<FontData>,
) -> FontDefinitions {
    let mut fonts = FontDefinitions::empty();
    let mut insert = |name: &str, data: FontData| {
        fonts.font_data.insert(name.to_owned(), Arc::new(data));
    };
    insert(INTER_MEDIUM_NAME, FontData::from_static(INTER_MEDIUM));
    insert(INTER_REGULAR_NAME, FontData::from_static(INTER_REGULAR));
    let mut clock = vec![INTER_MEDIUM_NAME.to_owned()];
    let mut date = vec![INTER_REGULAR_NAME.to_owned()];
    if let Some((clock_data, date_data)) = chosen {
        insert(CHOSEN_CLOCK, clock_data);
        insert(CHOSEN_DATE, date_data);
        clock.insert(0, CHOSEN_CLOCK.to_owned());
        date.insert(0, CHOSEN_DATE.to_owned());
    }
    if let Some(data) = fallback {
        insert(FALLBACK_FONT, data);
        clock.push(FALLBACK_FONT.to_owned());
        date.push(FALLBACK_FONT.to_owned());
    }
    fonts
        .families
        .insert(FontFamily::Name(CLOCK_FONT.into()), clock);
    fonts
        .families
        .insert(FontFamily::Name(DATE_FONT.into()), date.clone());
    // egui が内部で使う既定のファミリーも空にしない
    fonts
        .families
        .insert(FontFamily::Proportional, date.clone());
    fonts.families.insert(FontFamily::Monospace, date);
    fonts
}

/// `appearance.font` のファミリーの、時計用（500）と日付用（400）のフォントを読む。
///
/// fontconfig の `fc-list` で一覧を取り（`fc-match` は似たフォントで代用してしまうので使わない）、
/// どれを使うかは `fonts::pick_faces` が決める。読めない・egui が扱えないファイルなら `Err`。
fn system_fonts(family: &str) -> Result<(FontData, FontData), Fallback> {
    if family.trim().is_empty() {
        return Err(Fallback::EmptyName);
    }
    let output = Command::new("fc-list")
        .arg(format!("--format={}", fonts::FC_LIST_FORMAT))
        .arg(fonts::fc_pattern(family.trim()))
        .output()
        .map_err(|e| Fallback::FontconfigUnavailable(e.to_string()))?;
    if !output.status.success() {
        return Err(Fallback::FontconfigUnavailable(format!(
            "fc-list exited with {}",
            output.status
        )));
    }
    let faces = fonts::parse_fc_list(&String::from_utf8_lossy(&output.stdout));
    let (clock, date) = fonts::pick_faces(family, &faces)?;
    // 可変フォントなどで同じファイルを 2 回読まない
    let clock_bytes = read_font_file(&clock.file)?;
    let date_bytes = if date.file == clock.file {
        clock_bytes.clone()
    } else {
        read_font_file(&date.file)?
    };
    let clock_data = face_data(&clock, clock_bytes)?;
    let date_data = face_data(&date, date_bytes)?;
    log::info!(
        "clock font: {family} weight {} ({} face {}); date font: weight {} ({} face {})",
        clock.weight,
        clock.file,
        clock.face_index,
        date.weight,
        date.file,
        date.face_index
    );
    Ok((clock_data, date_data))
}

fn read_font_file(file: &str) -> Result<Arc<Vec<u8>>, Fallback> {
    std::fs::read(file)
        .map(Arc::new)
        .map_err(|e| Fallback::Unreadable {
            file: file.to_owned(),
            error: e.to_string(),
        })
}

/// 選んだフェイスを egui のフォントにする。egui は読めないフォントで panic するので、
/// 同じ方法（skrifa）で先に確かめる。可変フォントは `wght` 軸を選んだ太さにする。
fn face_data(pick: &FacePick, bytes: Arc<Vec<u8>>) -> Result<FontData, Fallback> {
    if let Err(e) = skrifa::FontRef::from_index(&bytes, pick.face_index) {
        return Err(Fallback::Unusable {
            file: pick.file.clone(),
            error: e.to_string(),
        });
    }
    let data = FontData::from_blob(bytes, pick.face_index);
    Ok(match pick.variable_weight {
        Some(weight) => data.tweak(FontTweak {
            coords: VariationCoords::new([(b"wght", weight)]),
            ..Default::default()
        }),
        None => data,
    })
}

/// 日時のロケールの言語で fontconfig にフォントを尋ね、読み込む。
///
/// `fc-match` を子プロセスで呼ぶ（C のライブラリを直接つながないので unsafe もビルド時の依存も要らず、
/// デスクトップの他のアプリと同じ規則で選ばれる）。見つからない・読めない・壊れているときは `None`
/// （その文字は表示されないが、ロックは続ける）。
fn fallback_font() -> Option<FontData> {
    let lc_all = std::env::var("LC_ALL").ok();
    let lc_time = std::env::var("LC_TIME").ok();
    let lang = std::env::var("LANG").ok();
    let locale = scene::time_locale_name(lc_all.as_deref(), lc_time.as_deref(), lang.as_deref());
    let pattern = match scene::language(locale) {
        Some(lang) => format!("sans-serif:lang={lang}"),
        None => "sans-serif".to_owned(),
    };
    let output = match Command::new("fc-match")
        .arg("--format=%{file}\n%{index}")
        .arg(&pattern)
        .output()
    {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            log::warn!("fc-match {pattern} failed: {}", output.status);
            return None;
        }
        Err(e) => {
            log::warn!("cannot run fc-match: {e}");
            return None;
        }
    };
    let (file, index) = scene::parse_fc_match(&String::from_utf8_lossy(&output.stdout))?;
    let bytes = match std::fs::read(&file) {
        Ok(bytes) => bytes,
        Err(e) => {
            log::warn!("cannot read fallback font {file}: {e}");
            return None;
        }
    };
    // egui は読めないフォントで panic するので、同じ方法（skrifa）で先に確かめる
    if let Err(e) = skrifa::FontRef::from_index(&bytes, index) {
        log::warn!("fallback font {file} (index {index}) is not usable: {e}");
        return None;
    }
    log::debug!("fallback font for {pattern}: {file} (index {index})");
    Some(FontData::from_blob(Arc::new(bytes), index))
}

/// 1 フレーム分の図形を積む。描く順は 光の柱 → 時計 → 日付 → 錠（U 字 → 本体）。
fn paint_scene(painter: &Painter, l: &Layout, scene: &Scene) {
    for pillar in &scene.pillars {
        let [left, top, right, bottom] = scene::pillar_rect(l, pillar);
        let rect = Rect::from_min_max(pos2(left.round(), top), pos2(right.round(), bottom));
        painter.add(Shape::mesh(pillar_mesh(
            rect,
            scene.opacity * pillar.frame.opacity,
        )));
    }
    paint_clock(
        painter,
        l,
        &scene.time_text,
        white(CLOCK_ALPHA * scene.opacity),
    );
    paint_date(
        painter,
        l,
        &scene.date_text,
        white(DATE_ALPHA * scene.opacity),
    );
    if let Some(lock) = &scene.lock {
        paint_lock(painter, l, lock, white(scene.opacity));
    }
}

/// 柱を 1 本、頂点ごとに色を持つ格子のメッシュで作る。
///
/// 不透明度は「横のグラデーション × 縦のマスク」の積で、三角形 1 枚の中では直線補間しかできない。
/// 横は区切り（`PILLAR_COLUMNS`）に頂点を置くので正確、縦は `PILLAR_ROWS` 段に分けて
/// 積の誤差を 1% 未満にする。
fn pillar_mesh(rect: Rect, opacity: f32) -> Mesh {
    let cols = scene::PILLAR_COLUMNS;
    let rows = scene::PILLAR_ROWS;
    let mut mesh = Mesh::default();
    for r in 0..=rows {
        let v = r as f32 / rows as f32;
        let y = rect.top() + v * rect.height();
        for u in cols {
            let x = rect.left() + u * rect.width();
            mesh.colored_vertex(pos2(x, y), white(opacity * scene::pillar_alpha(u, v)));
        }
    }
    let w = cols.len() as u32;
    for r in 0..rows as u32 {
        for c in 0..w - 1 {
            let i = r * w + c;
            mesh.add_triangle(i, i + 1, i + w);
            mesh.add_triangle(i + 1, i + w + 1, i + w);
        }
    }
    mesh
}

/// 時計。数字は等幅（一番広い数字の幅の枠の中央に置く）、字間は -2px（1080p 基準）。
///
/// egui は OpenType の機能（`tnum`）を指定できないので、1 文字ずつ並べて等幅数字にする。
fn paint_clock(painter: &Painter, l: &Layout, text: &str, color: Color32) {
    let font = clock_font(l);
    let glyph = |c: char| painter.layout_no_wrap(c.to_string(), font.clone(), Color32::WHITE);
    let digit_w = ('0'..='9')
        .map(|c| glyph(c).size().x)
        .fold(0.0_f32, f32::max);
    let glyphs: Vec<_> = text
        .chars()
        .map(|c| {
            let g = glyph(c);
            let cell = if c.is_ascii_digit() {
                digit_w
            } else {
                g.size().x
            };
            (g, cell)
        })
        .collect();
    let gaps = glyphs.len().saturating_sub(1) as f32;
    let total: f32 =
        glyphs.iter().map(|(_, cell)| cell).sum::<f32>() + l.clock_letter_spacing * gaps;
    let mut x = l.center_x - total / 2.0;
    for (g, cell) in glyphs {
        let pos = pos2(
            x + (cell - g.size().x) / 2.0,
            l.clock_center_y - g.size().y / 2.0,
        );
        painter.add(Shape::galley_with_override_text_color(pos, g, color));
        x += cell + l.clock_letter_spacing;
    }
}

/// 日付。字間 1px（1080p 基準）、行の上端を `date_top` に合わせて中央寄せ。
fn paint_date(painter: &Painter, l: &Layout, text: &str, color: Color32) {
    let job = LayoutJob::single_section(
        text.to_owned(),
        TextFormat {
            font_id: date_font(l),
            extra_letter_spacing: l.date_letter_spacing,
            color: Color32::WHITE,
            ..Default::default()
        },
    );
    let galley = painter.layout_job(job);
    let pos = pos2(l.center_x - galley.size().x / 2.0, l.date_top);
    painter.add(Shape::galley_with_override_text_color(pos, galley, color));
}

/// 錠。U 字を太さ一定の線で描き、その上に本体（角丸の塗りつぶし）を重ねて足の下端を隠す。
fn paint_lock(painter: &Painter, l: &Layout, lock: &scene::LockDraw, color: Color32) {
    let unit = l.icon_unit;
    let origin = [l.icon_origin[0] + lock.shake_x * l.scale, l.icon_origin[1]];
    let to_screen = |p: [f32; 2]| pos2(origin[0] + p[0] * unit, origin[1] + p[1] * unit);

    let points: Vec<_> = scene::shackle_polyline(&lock.path, SHACKLE_SEGMENTS)
        .into_iter()
        .map(to_screen)
        .collect();
    let stroke = anim::SHACKLE_STROKE * unit;
    // egui の線は端が平らなので、見える方の端（最初の点）に丸を足す（設計の stroke-linecap: round）
    painter.add(Shape::circle_filled(points[0], stroke / 2.0, color));
    painter.add(Shape::line(points, Stroke::new(stroke, color)));

    let body = Rect::from_min_max(
        to_screen([anim::BODY_X, anim::BODY_Y]),
        to_screen([anim::BODY_X + anim::BODY_W, anim::BODY_Y + anim::BODY_H]),
    );
    let radius = (anim::BODY_RADIUS * unit).round().clamp(0.0, 255.0) as u8;
    painter.add(Shape::rect_filled(body, CornerRadius::same(radius), color));
}

```

- [ ] **Step 5: テストが通ることを確認する**

Run: `cargo test --lib render::overlay::`
Expected: `test result: ok. 7 passed`。`embedded_fonts_are_inter_with_the_right_weights` が失敗したら、Task 9 で違うファイル（`InterDisplay` や可変フォント）を取り出している。Task 9 をやり直す。

**注意（本物の Inter では未確認）:** この計画を書いたときは Noto Sans を Inter の代わりに置いて確かめた（そのときのテストは `starts_with("Noto Sans")` で通した）。`embedded_fonts_are_inter_with_the_right_weights`、Task 9 Step 3 の `fc-scan` の期待値、日付の行の高さ（Inter の 1.21 倍 ≈ 31.5、`DATE_LINE_H`）は本物の Inter ではまだ動かしていない。食い違ったら、テストや定数を結果に合わせて直さず、Task 9 で取り出したファイルを確かめ直す（Task 9 Step 3 の注意を参照）。

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: 警告・差分なし。

- [ ] **Step 6: コミットする（依存と実装を分ける）**

```bash
git status
git diff
git add Cargo.toml Cargo.lock
git commit -m "build: add egui, egui_glow and skrifa

skrifa is the version egui already uses; it checks font files
before handing them to egui, which panics on unreadable fonts.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git add src/render/overlay.rs
git commit -m "feat: draw clock, date, pillars and padlock with egui

The clock and date use appearance.font from fontconfig and fall back
to the embedded Inter; glyphs they lack come from Inter and then from
the font fontconfig picks for the LC_TIME language.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git add src/render/mod.rs
git commit -m "feat: register the overlay renderer module

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

（1 ファイル 1 コミットに分ける。`src/render/overlay.rs` は `src/render/mod.rs` が宣言するまでコンパイルされないので、どちらのコミットでもビルドは通る。依存の追加は先にコミットしてあるので、`overlay.rs` だけのコミットでも `Cargo.toml` と食い違わない。）

---

### Task 13: `OverlayHost` — App 用の状態一式

**Files:**
- Modify: `src/render/overlay.rs`

**Interfaces:**
- Consumes: Task 12 の `Overlay`、Task 5〜8 の `scene`、Task 11 の `wl::outputs::focused_output`、Plan 1 の `lanes::{lane_count, LaneAllocator}`、`state::{Machine, Phase}`、`config::Config`
- Produces（`render::overlay::OverlayHost`。Task 14 が使う）:
  - `OverlayHost::new(config: &Config) -> OverlayHost`（GL なし。ロケール・日付の書式・`appearance.font`・レーン・主画面のルールと、sway のフォーカス中の出力を用意）
  - `attach_gpu(&mut self, gpu: &Gpu)`（`Overlay::new(gpu, &font, ..)` を呼ぶ。失敗したら警告して重ね物なしで続ける）
  - `keystroke(&mut self, now_ms: u64)`（柱を 1 本出す）
  - `refresh_primary<'a, I>(&mut self, outputs: I) -> Option<usize>`（`I: IntoIterator<Item = (&'a str, &'a str)>, I::IntoIter: Clone`）
  - `animating(&self, index: usize, machine: &Machine, now_ms: u64) -> bool`（出力 `index` が主画面で、柱・震え・解除のどれかが動いている）
  - `wants_frame(&self, machine: &Machine, now_ms: u64) -> bool`（主画面の重ね物が動いている、または時刻・日付・状態・主画面が前に描いたときと違う）
  - `draw(&mut self, gpu: &Gpu, index: usize, size: (u32, u32), machine: &Machine, now_ms: u64, fade: f32)`（主画面のときだけ描く）

この型は GL を要するので単体テストはない（`animating`・`wants_frame`・`draw` は `Overlay` がないと何もしない）。中身の計算は `scene` でテスト済み。Task 15 で動きを確かめる。

- [ ] **Step 1: 実装を書く**

`src/render/overlay.rs` の `use crate::…` の行を次にする（`config`・`scene` の行を置き換え、`lanes`・`state` を足す）:

```rust
use crate::anim;
use crate::config::{self, Config};
use crate::fonts::{self, FacePick, Fallback};
use crate::lanes::{self, LaneAllocator};
use crate::render::egl::Gpu;
use crate::scene::{self, DateStyle, Layout, PrimaryTracker, Scene};
use crate::state::{Machine, Phase};
```

`fn paint_lock(...)` の後（テストモジュールの前）に追加する:

```rust
/// 重ね物を描くのに要る状態一式（App が 1 つ持つ）。
///
/// EGL が使えないとき（`overlay` が `None`）も、柱のレーンなどは動かしてよい（描かないだけ）。
pub struct OverlayHost {
    overlay: Option<Overlay>,
    lanes: LaneAllocator,
    primary: PrimaryTracker,
    date: DateStyle,
    input: config::Input,
    /// `appearance.font`。`attach_gpu` で fontconfig に尋ねる。
    font: String,
    /// 最後に柱を出した時刻。これから `pillar_fade_ms` の間は柱が動いている。
    last_keystroke_ms: Option<u64>,
    /// 最後に描いた内容（変わったときだけ描き直すため）。
    last: Option<Drawn>,
}

/// 最後に描いた内容のうち、時間で変わるもの。
struct Drawn {
    phase: Phase,
    time_text: String,
    date_text: String,
}

impl OverlayHost {
    /// App を作るときに呼ぶ（ロックの前）。GL はまだ使わない。
    /// ロック前にフォーカスしていた出力を sway IPC で尋ねるのもここ。
    pub fn new(config: &Config) -> OverlayHost {
        let lc_all = std::env::var("LC_ALL").ok();
        let lc_time = std::env::var("LC_TIME").ok();
        let lang = std::env::var("LANG").ok();
        let locale = scene::chrono_locale(scene::time_locale_name(
            lc_all.as_deref(),
            lc_time.as_deref(),
            lang.as_deref(),
        ));
        let focused = crate::wl::outputs::focused_output();
        log::info!("focused output before locking: {focused:?}");
        let input = config.input.clone();
        // レーン数は拡大前の設定値で決める（主画面の大きさでレーン数が変わらないように）
        let count = lanes::lane_count(input.band_width, input.pillar_width);
        OverlayHost {
            overlay: None,
            lanes: LaneAllocator::new(count, u64::from(input.pillar_fade_ms), fastrand::Rng::new()),
            primary: PrimaryTracker::new(config.display.primary.clone(), focused),
            date: DateStyle::new(&config.appearance.date_format, locale),
            input,
            font: config.appearance.font.clone(),
            last_keystroke_ms: None,
            last: None,
        }
    }

    /// GPU が使えるようになったら（ロックの前に）呼ぶ。シェーダー・フォント・アトラスを準備する。
    /// 失敗したら重ね物なしで続ける（背景とパスワード入力は動く）。
    pub fn attach_gpu(&mut self, gpu: &Gpu) {
        match Overlay::new(gpu, &self.font, scene::needs_fallback_font(&self.date)) {
            Ok(overlay) => self.overlay = Some(overlay),
            Err(e) => log::warn!("overlay disabled: {e}"),
        }
    }

    /// パスワードバッファが打鍵を受け付けたとき（`push_str` / `backspace` が `true`）に呼ぶ。
    pub fn keystroke(&mut self, now_ms: u64) {
        self.lanes.spawn(now_ms);
        self.last_keystroke_ms = Some(now_ms);
    }

    /// 出力の一覧（出力名, description）を渡して主画面の添字を返す。一覧が変わったときだけ選び直す。
    pub fn refresh_primary<'a, I>(&mut self, outputs: I) -> Option<usize>
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
        I::IntoIter: Clone,
    {
        let before = self.primary.primary();
        let after = self.primary.update(outputs);
        if after != before {
            log::info!("overlay output index: {after:?}");
            // 新しい主画面には最初から描き直す
            self.last = None;
        }
        after
    }

    /// 出力 `index` の重ね物が動き続けている（毎フレーム描き直す）か:
    /// 主画面で、柱が表示中・錠が震えている・解除のアニメーション中。
    pub fn animating(&self, index: usize, machine: &Machine, now_ms: u64) -> bool {
        if self.overlay.is_none() || self.primary.primary() != Some(index) {
            return false;
        }
        let shaking = matches!(machine.phase(), Phase::Failed { .. })
            && machine
                .phase()
                .elapsed_ms(now_ms)
                .is_some_and(|ms| ms < anim::SHAKE_MS);
        // 一番新しい柱が消えるまでは、どれかの柱が動いている
        let pillars = self
            .last_keystroke_ms
            .is_some_and(|t| now_ms.saturating_sub(t) < u64::from(self.input.pillar_fade_ms));
        shaking || pillars || matches!(machine.phase(), Phase::Unlocking { .. })
    }

    /// 主画面の重ね物が、前に描いたときから変わったか（時刻・日付・状態・主画面）、動き続けているか。
    /// `Tick` ごとに（`refresh_primary` の後で）呼び、`true` なら主画面を描き直す。
    pub fn wants_frame(&self, machine: &Machine, now_ms: u64) -> bool {
        let Some(index) = self.primary.primary() else {
            return false;
        };
        if self.overlay.is_none() {
            return false;
        }
        if self.animating(index, machine, now_ms) {
            return true;
        }
        let Some(last) = &self.last else {
            return true;
        };
        let wall = chrono::Local::now().fixed_offset();
        last.phase != machine.phase()
            || last.time_text != wall.format("%H:%M").to_string()
            || last.date_text != self.date.format(&wall)
    }

    /// 出力 `index` が主画面なら重ね物を描く。描画先は current にしておき、背景は描いておく。
    /// `size` は `Background::draw` に渡したのと同じ幅と高さ、`fade` は同じフェードの不透明度。
    pub fn draw(
        &mut self,
        gpu: &Gpu,
        index: usize,
        size: (u32, u32),
        machine: &Machine,
        now_ms: u64,
        fade: f32,
    ) {
        let Some(overlay) = self.overlay.as_mut() else {
            return;
        };
        if self.primary.primary() != Some(index) {
            return;
        }
        let wall = chrono::Local::now().fixed_offset();
        let pillars = self.lanes.pillars(now_ms);
        let scene = scene::build(
            machine,
            now_ms,
            fade,
            pillars,
            &self.input,
            &wall,
            &self.date,
        );
        overlay.draw(gpu, size.0, size.1, &scene, &self.input);
        self.last = Some(Drawn {
            phase: machine.phase(),
            time_text: scene.time_text,
            date_text: scene.date_text,
        });
    }
}

```

- [ ] **Step 2: ビルド・テスト・lint**

Run: `cargo test --lib render::overlay:: && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: `test result: ok. 7 passed`、警告・差分なし。

- [ ] **Step 3: コミットする**

```bash
git status
git diff
git add src/render/overlay.rs
git commit -m "feat: add OverlayHost to own overlay state for the app

It keeps the lane allocator, the primary output choice, the date style
and the configured font, and tells the app when the primary output
needs a new frame.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 14: App に組み込む

**Files:**
- Modify: `src/app.rs`

**Interfaces:**
- Consumes: Task 13 の `OverlayHost`、Plan 2・3 の `App`・`run`・`draw_output`・`LockOutput { meta, frame_pending, .. }`・`handle_key`・`CompositorHandler::frame`・Tick のタイマー
- Produces: 主画面に時計・日付・光の柱・錠が出るロッカー（契約 8 章の Plan 4 の成果物）。`App` に足すメソッド: `primary_index`、`redraw_primary`、`tick_overlay`（どれも `pub(crate)` か非公開）

下のコードは Plan 2・3 を当てた後の `src/app.rs` に合わせて書いてあり、そのまま当たることを確かめてある。
各変更の位置は、各 Step に引いた行で決まる。始める前に、次の行がそれぞれ `src/app.rs` に 1 か所だけあることを `grep -cF` で確かめる（どれも `1` が出る）:

```bash
for anchor in \
  'use crate::render::egl::{Gpu, GpuSurface};' \
  'pub(crate) gpu: Option<Gpu>,' \
  'let mut app = App {' \
  'first_configure_ms: None,' \
  'app.background = background;' \
  '/// 描画の入口。' \
  '// Plan 4: ここで主画面に重ね物を描く' \
  '/// wl_surface から出力の番号を引く。' \
  'let animating = background::needs_frames(fade, phase);' \
  'if self.background_animating() {' \
  'fn handle_key' \
  'app.kick_animation();'
do
  printf '%s\t%s\n' "$(grep -cF -- "$anchor" src/app.rs)" "$anchor"
done
```

（`let now = self.clock.now_ms();` は `src/app.rs` に 3 か所あるので一覧に入れていない。Step 2 で使うのは `draw_output` の中の 1 か所。）
見つからない・2 か所以上あるときは、別の場所を推測して当てずに BLOCKED として止まり、見つからなかった行を報告する（Plan 2・3 の実装が計画書と違う）。

- [ ] **Step 1: フィールドを足し、作って GPU をつなぐ**

`src/app.rs` の `use crate::render::egl::{Gpu, GpuSurface};` の次の行に足す:

```rust
use crate::render::overlay::OverlayHost;
```

`pub struct App` の `pub(crate) gpu: Option<Gpu>,`（とその doc コメント）の**前**に足す:

```rust
    /// 主画面への重ね描き（時計・日付・光の柱・錠）。Plan 4。
    ///
    /// `gpu` より前に置く。フィールドは宣言順に捨てられるので、egui_glow の GL の後片付けが
    /// EGL のコンテキストより先に行われる。
    pub(crate) overlay: OverlayHost,
```

`run` の `let mut app = App {` の行の**前**に足す（`config` は初期化式で App に move されるので、その前に作る）:

```rust
    // 重ね物の状態（GL はまだ使わない）。ロック前にフォーカスしていた出力もここで sway に尋ねる
    let overlay = OverlayHost::new(&config);
```

初期化式の `first_configure_ms: None,` と `gpu: None,` の間に足す:

```rust
        overlay,
```

`app.background = background;` の次の行（`WaylandSource::new` の前）に足す:

```rust
    // フォント・シェーダー・アトラスをロック前に用意する（失敗しても重ね物なしで続ける）
    if let Some(gpu) = app.gpu.as_ref() {
        app.overlay.attach_gpu(gpu);
    }
```

- [ ] **Step 2: `draw_output` で主画面に重ねる**

`impl App` の `pub(crate) fn draw_output` の doc コメント（`/// 描画の入口。…`）の前に足す:

```rust
    /// 主画面の添字（出力の一覧が変わっていれば選び直す）。Plan 4。
    pub(crate) fn primary_index(&mut self) -> Option<usize> {
        self.overlay.refresh_primary(
            self.outputs
                .iter()
                .map(|o| (o.meta.name.as_str(), o.meta.description.as_str())),
        )
    }

```

`draw_output` の先頭（`let now = self.clock.now_ms();` の前）に足す:

```rust
        // 出力の一覧が変わっていれば主画面を選び直す（Plan 4）
        self.primary_index();
```

Gpu の腕の `// Plan 4: ここで主画面に重ね物を描く` の行を次に置き換える（`background.draw(..)` の直後、`set_buffer_scale` の前）:

```rust
                // 主画面なら時計・日付・光の柱・錠を重ねる（Plan 4）
                self.overlay.draw(
                    gpu,
                    index,
                    (buffer_width, buffer_height),
                    &self.machine,
                    now,
                    fade,
                );
```

この位置では `out`（`self.outputs` の要素）を可変で借りているが、`self.overlay`・`self.machine` は別のフィールドなので借用は衝突しない。
`now`・`fade` は `draw_output` の先頭で Plan 3 が作っているもの、`buffer_width`・`buffer_height` は `background.draw` に渡しているバッファの画素数。

- [ ] **Step 3: ビルドしてコミットする**

Run: `cargo build && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: エラー・警告・差分なし。

（ここで Task 15 の Step 1・2 を行うと、主画面に時計と日付が出ることを確かめられる。柱と時刻の更新はまだ動かない。）

```bash
git status
git diff
git add src/app.rs
git commit -m "feat: draw the overlay on the primary output

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 4: 打鍵で柱を出し、動いている間はフレームを続ける**

`impl App` の `fn output_index_of` の doc コメント（`/// wl_surface から出力の番号を引く。`）の前に足す:

```rust
    /// 主画面を描き直す。フレームコールバック待ちなら、そのコールバックで描かれるので何もしない。
    pub(crate) fn redraw_primary(&mut self) {
        if let Some(index) = self.primary_index()
            && index < self.outputs.len()
            && !self.outputs[index].frame_pending
        {
            self.draw_output(index);
        }
    }

```

`draw_output` の `let animating = background::needs_frames(fade, phase);` を次に置き換える（重ね物が動いている間は、主画面だけフレームコールバックを続ける）:

```rust
        let animating = background::needs_frames(fade, phase)
            || self.overlay.animating(index, &self.machine, now);
```

`impl CompositorHandler for App` の `fn frame` の

```rust
        // フェード中・解除中は次のフレームを描く（時刻は Clock だけを使い、引数の時刻は使わない）
        if self.background_animating() {
```

を次に置き換える:

```rust
        // フェード中・解除中・主画面の重ね物が動いている間は次のフレームを描く
        // （時刻は Clock だけを使い、引数の時刻は使わない）
        if self.background_animating()
            || self
                .overlay
                .animating(index, &self.machine, self.clock.now_ms())
        {
```

`fn handle_key` の `KeyAction::Submit`・`Backspace`・`Insert` の腕を次にする（`Clear` と `Ignore` は変えない）。
`push_str` / `backspace` が `true` を返したとき（パスワードが変わったとき）だけ柱を出す
（引き継ぎ: `push_str` が `true` のときだけ。設計書 6 章「Backspace は 1 文字消して柱を出す」。Esc・Ctrl+U では出さない）:

```rust
            KeyAction::Submit => {
                let empty = self.password.is_empty();
                self.handle_event(Event::Submit { empty });
                // 確認中になったらすぐ錠を出す（Plan 4）
                self.redraw_primary();
            }
            KeyAction::Backspace => {
                if self.machine.accepts_input() && self.password.backspace() {
                    // 1 文字消えたときだけ光の柱を出す（Plan 4）
                    let now_ms = self.clock.now_ms();
                    self.overlay.keystroke(now_ms);
                    self.redraw_primary();
                }
            }
```

```rust
            KeyAction::Insert => {
                if self.machine.accepts_input()
                    && let Some(text) = &text
                    && self.password.push_str(text)
                {
                    // パスワードに入ったときだけ光の柱を出す（Plan 4。入りきらない分は捨て、柱も出さない）
                    let now_ms = self.clock.now_ms();
                    self.overlay.keystroke(now_ms);
                    self.redraw_primary();
                }
            }
```

Run: `cargo build && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: エラー・警告・差分なし。

```bash
git status
git diff
git add src/app.rs
git commit -m "feat: spawn a light pillar for each accepted keystroke

Frames keep coming on the primary output while pillars, the shake or
the unlock animate.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 5: Tick で時刻・状態・主画面の変化を描き直す**

`use crate::state::{Action, Event, Machine};` を次にする:

```rust
use crate::state::{Action, Event, Machine, Phase};
```

`fn redraw_primary` の後（`/// wl_surface から出力の番号を引く。` の前）に足す:

```rust
    /// 主画面の重ね物が前に描いたときから変わっていれば描き直す（`Tick` ごと。Plan 4）。
    ///
    /// 分が変わった・失敗の錠が消えた・主画面の出力が抜かれた、などを拾う。ロックを外した後は描かない。
    fn tick_overlay(&mut self) {
        if self.machine.phase() == Phase::Done {
            return;
        }
        self.primary_index();
        if self.overlay.wants_frame(&self.machine, self.clock.now_ms()) {
            self.redraw_primary();
        }
    }

```

`run` の Tick のタイマーのコールバックで、`app.kick_animation();` の次の行に足す:

```rust
            // 時刻・日付・状態・主画面が変わったら重ね物を描き直す（Plan 4）
            app.tick_overlay();
```

Run: `cargo build && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: エラー・警告・差分なし。テストはすべて通る（Plan 3 の ignored の 3 件を除く。この計画を当て終えた時点で 176 passed）。

```bash
git status
git diff
git add src/app.rs
git commit -m "feat: redraw the overlay when the time or the lock state changes

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 15: 動作確認（ヘッドレスの sway と入れ子の sway）

**Files:** なし（確認だけ。直すことがあれば、その変更を対応するタスクの形でコミットする）

本物のセッションはロックしない。間違ったパスワードは 1 回だけ試す（faillock。契約 1 章・9 章）。

- [ ] **Step 1: 自動のテスト**

Run:

```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Expected: すべて通る（Plan 3 の ignored の 3 件を除く）、警告・差分なし。

- [ ] **Step 2: ヘッドレスの sway で、主画面だけに時計と日付が出ることと、選んだフォントを確かめる**

Plan 3 の Task 10 と同じく、`XDG_RUNTIME_DIR` を新しい空のディレクトリにしてヘッドレスの sway を動かす（本物のセッションに影響しない。ソケットのパスが 108 バイトを超えないよう `/tmp` に作る）。入力はできないので解除はしない。`grim` で撮って確かめ、最後に kabi-lock を止めて sway ごと終える。`Noto Serif` は時計が Inter と見分けやすい例（入っていなければ `fc-list : family` にある別のセリフ体に置き換える）。

```bash
BIN=$PWD/target/debug/kabi-lock
OUT=$(mktemp -d /tmp/kabi-p4.XXXXXX)
RT=$(mktemp -d /tmp/kabi-rt.XXXXXX)
mkdir -p "$OUT/xdg/kabi-lock"
cat > "$OUT/xdg/kabi-lock/config.toml" <<'EOF'
[display]
primary = ["HEADLESS-2"]

[appearance]
date_format = "%m/%d (%a)"
font = "Noto Serif"
EOF
cat > "$OUT/conf" <<EOF
output HEADLESS-1 resolution 800x500
output HEADLESS-2 resolution 1280x720
exec alacritty -e sh -c 'printf "kabi-lock overlay test"; sleep 120'
exec sh -c 'sleep 2; XDG_CONFIG_HOME=$OUT/xdg LC_TIME=ja_JP.UTF-8 RUST_LOG=kabi_lock=debug $BIN > $OUT/kabi.log 2>&1 & K=\$!; sleep 2.5; grim -o HEADLESS-2 -t ppm $OUT/primary.ppm; grim -o HEADLESS-1 -t ppm $OUT/other.ppm; kill \$K; sleep 0.5; swaymsg exit'
EOF
env -i HOME="$HOME" PATH="$PATH" USER="$USER" XDG_RUNTIME_DIR="$RT" \
  WLR_BACKENDS=headless WLR_HEADLESS_OUTPUTS=2 WLR_LIBINPUT_NO_DEVICES=1 \
  timeout 40 sway -c "$OUT/conf" > "$OUT/sway.log" 2>&1
grep -vE 'sctk\]|egui_glow' "$OUT/kabi.log"
```

（`alacritty` がなければ手元の端末に置き換える。）

Expected のログ（抜粋。時間は目安）:

```text
INFO  kabi_lock::render::overlay] focused output before locking: Some("HEADLESS-2")
...
INFO  kabi_lock::render::overlay] clock font: Noto Serif weight 500 (/usr/share/fonts/noto/NotoSerif-Medium.ttf face 0); date font: weight 400 (/usr/share/fonts/noto/NotoSerif-Regular.ttf face 0)
DEBUG kabi_lock::render::overlay] fallback font for sans-serif:lang=ja: /usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc (index ...)
INFO  kabi_lock::app] lock requested after ... ms
INFO  kabi_lock::render::overlay] overlay output index: Some(0)
INFO  kabi_lock::app] session locked after ... ms
```

`WARN` が出ないこと。`lock requested` は Plan 3 の時点より 30ms ほど遅くなる（フォントの読み込みと egui の準備。日本語の補助フォントは約 19MB を読む）。

画像（`$OUT/*.ppm`）で見ること:
- `primary.ppm`（HEADLESS-2）: ぼかした背景の上寄り（上から約 1/4）に `HH:MM`（Noto Serif、数字の幅がそろっている）と、その下に `11/24 (火)` のような日付（曜日は日本語）。打鍵していないので帯と錠は出ない。
- `other.ppm`（HEADLESS-1）: 背景だけで、時計も日付も出ない。

- [ ] **Step 3: フォントが使えないときに埋め込みの Inter に戻ることを確かめる**

Step 2 の `config.toml` の `font` を `"No Such Font"` にしてもう一度動かす。続けて `font = "Noto Serif"` に戻し、kabi-lock だけ `PATH` から `fc-list` を外して動かす（`exec sh -c '…'` の中の `XDG_CONFIG_HOME=…` の前に `PATH=/nonexistent` を足す。kabi-lock は `fc-list`・`fc-match` のほかにコマンドを使わない）。

Expected:
- 1 回目: `WARN … font "No Such Font" is not used (no installed font family named "No Such Font"); using the embedded Inter`。`primary.ppm` の時計は Inter。
- 2 回目: `WARN … font "Noto Serif" is not used (fontconfig is unavailable: No such file or directory (os error 2)); using the embedded Inter` と `WARN … cannot run fc-match: …`。時計は Inter、日付の曜日（`火`）は四角（豆腐）になる（ロックは続く）。
- どちらもロックは掛かり、`overlay output index: Some(0)` が出る。

- [ ] **Step 4: 入れ子の sway で、打鍵・錠・解除を確かめる**

パスワードでの解除には、`auth` と `account` の行がある PAM サービスが要る（Plan 2 の `pam_check` は `acct_mgmt` も呼ぶ）。`/etc/pam.d/kabi-lock` はまだない（Plan 6 で入れる）ので、Plan 2 Task 12 Step 8 と同じく、一時的な設定で `pam_service = "login"` を使う。先に `/etc/pam.d/login` を確かめる:

```bash
grep -qE '^auth[[:space:]]' /etc/pam.d/login && grep -qE '^account[[:space:]]' /etc/pam.d/login && echo PAM-OK || echo "PAM file missing auth or account line"
```

Expected: `PAM-OK`。`PAM file missing auth or account line` が出たら、ここで止めて BLOCKED として人に報告する（エージェントは `/etc/pam.d` を変えない）。

入れ子の sway のウィンドウにフォーカスを当ててから操作する。

```bash
mkdir -p /tmp/kabi-xdg/kabi-lock
cat > /tmp/kabi-xdg/kabi-lock/config.toml <<'EOF'
[display]
primary = ["WL-2"]

[appearance]
date_format = "%m/%d (%a)"

[auth]
pam_service = "login"
EOF
cat > /tmp/kabi-nested.conf <<EOF
exec sh -c 'XDG_CONFIG_HOME=/tmp/kabi-xdg LC_TIME=ja_JP.UTF-8 RUST_LOG=kabi_lock=debug $PWD/target/debug/kabi-lock > /tmp/kabi-nested.log 2>&1; swaymsg exit'
EOF
WLR_BACKENDS=wayland WLR_WL_OUTPUTS=2 sway -c /tmp/kabi-nested.conf
```

- [ ] `WL-2` のウィンドウだけに時計・日付・帯が出る。`WL-1` は背景だけ。
- [ ] 黒から背景と一緒に時計などがフェードインする（約 1.5 秒）。
- [ ] 時計が `HH:MM`（`font` の初期値 `"Inter"`。Inter が入っていなければ埋め込みの Inter。ログに `font "Inter" is not installed; using the embedded Inter`）、その下に日本語の曜日の日付。
- [ ] 文字を打つたびに、帯の空いているレーンに柱が 1 本、下から伸びて（少し伸び過ぎてから戻り）消える。Esc では柱が出ない。空のときの Backspace でも出ない。打った文字は Esc で全部消し、ここでは Enter を押さない（中身のあるバッファで Enter を押すと、それがパスワードとして PAM に送られる）。
- [ ] 間違ったパスワードを **1 回だけ** 打って Enter を押すと、錠が現れ（閉じた形、U 字の上が半円）、PAM の結果が返ると左右に震え（約 0.45 秒）、約 1.2 秒で消える。震えている間に打った文字の柱は出る（打った文字は Esc で消し、Enter は押さない）。これが Plan 2〜6 の手動確認全体で、間違ったパスワードを PAM に送るただ 1 回（faillock）。この Step や Step 5 をやり直すときも、ほかの計画でも、もう送らない。
- [ ] 正しいパスワードで、U 字が跳ね上がってから右の長い足を軸に回り（線の太さは変わらない）、背景がくっきりして時計などが消え、ロックが外れて入れ子の sway が終わる。
- [ ] ログにパスワード・入力した文字・その長さが出ていない（`grep -inE 'password|utf8|keysym' /tmp/kabi-nested.log` にパスワードの中身がない）。

- [ ] **Step 5: 主画面の選び直しを確かめる**

1. `/tmp/kabi-xdg/kabi-lock/config.toml` の `primary` を消し（`[display]` の 2 行を消す。`[auth]` の `pam_service = "login"` は残す）、Step 4 と同じコマンドで入れ子の sway を起動し直す。**間違ったパスワードの確認（震え）はやり直さない**（Step 4 で 1 回送った。faillock）。ロック前にフォーカスしていた出力に重ね物が出る（ログに `focused output before locking: Some("WL-…")`）ことだけを確かめる。
2. そのままロック中に入れ子の sway のウィンドウ `WL-2` を閉じる（出力が減る）。50ms 以内に残った出力に重ね物が移る（ログに `overlay output index: Some(0)`）。
3. 残ったウィンドウにフォーカスを当て、自分のパスワードを **1 回だけ** 正しく入れて解除する。

- [ ] **Step 6: 片付けと記録**

Run: `rm -rf /tmp/kabi-p4.* /tmp/kabi-rt.* /tmp/kabi-xdg /tmp/kabi-nested.conf /tmp/kabi-nested.log`

確認した内容（見た目、ログ、気になった点）を作業報告に書く。

---

## 契約に対する補足（この計画で決めたこと・契約の不足）

- `scene::build` の引数は契約で「Plan 4 で決める」とされていたので、`(machine, now_ms, fade_alpha, pillars, input, wall, date)` にした。フェードは Plan 3 の `background::fade_for(first_configure_ms, now, fade_in_ms)` の値（`draw_output` の `fade`）をそのまま受け取る。
- 純粋なモジュール `src/fonts.rs` を足した（契約 3 章の一覧にない。契約 4 章の「候補の選び方と戻し方の判断は純粋な関数に分けてテストする」のため。ほかの計画からは呼ばない）。`#![forbid(unsafe_code)]` 付き。
- 契約の表にない依存: `skrifa` 0.44（egui が使うものと同じ版。フォントファイルの検査）。chrono には `unstable-locales` 機能を付けた（設計書 7 章の「曜日などは LC_TIME のロケールに従う」に要る）。
- `wl::outputs::focused_output()` を Plan 2 のファイルに足した（契約は「swayipc で取る」とだけ書いている）。
- App への継ぎ目（Task 14）は契約に書かれていない場所にも及ぶ: `CompositorHandler::frame` の続ける条件、`handle_key` の 3 つの腕、Tick のタイマー。どれも数行で、Plan 2・3 の構造は変えない。
- 契約の起動順では App が roundtrip のために `Gpu::new` より先にできるので、`OverlayHost` は `new`（GL なし）と `attach_gpu` に分けた。`Overlay::new(gpu, font_family, date_needs_fallback)` と `Overlay::draw(gpu, width, height, scene, input)` は契約どおり。
- 引き継ぎの「早めにやる」のうち `lanes` の飽和加算と空の主画面ルールは Plan 2 で済んでいるので扱わない。Plan 6 は `docs/config.example.toml` と README に `appearance.font` を足す（契約 4 章）。

## 設計書との対応

### Plan 4 で満たすもの

| 設計書 | 内容 | どこで |
|---|---|---|
| 3 章（表） | 時計（`HH:MM`、500、104px、白 0.94、字間 -2、等幅数字）、日付（下 14px、26px、白 0.74）、錠の領域、入力バー（下 374px、幅 768・高さ 72、16 レーン）、光の柱（幅 32、間隔 16、両端 0.85・中 0.38、上が透明） | Task 1・5〜8・12 |
| 3 章「レイアウト」 | 1920x1080 基準、上から 130px、高さに比例 | Task 6 |
| 3 章「フェードイン」 | 時計なども背景と同じ不透明度で一緒に | Task 7・14 |
| 3 章「光の柱」 | 空いているレーンからランダム、0.35 → 1.06 → 1.0 倍、全レーン使用中は一番古いレーン | Task 7（Plan 1 の `lanes`・`anim`） |
| 3 章「錠（解除）」 | 跳ね上がり → 長い足を軸に回る、太さ一定の丸棒、その後ぼかしが晴れて重ね物が消える（`fade × (1 − reveal)`） | Task 6・7・12 |
| 3 章「錠（失敗）」「錠の形」 | 震え（450ms）、約 1.2 秒で消える、角丸 rx 6 の本体、半円の U 字、右足が長い | Task 1・7・12 |
| 4 章「描画の分担」 | egui（egui_glow）で背景の上に重ねる、キー入力は egui に渡さない | Task 12・14 |
| 5 章 1 | 主画面が設定で決まらないときのため、sway IPC の `focused` を記録 | Task 11・13 |
| 5 章 2 | フォント読み込み・egui の準備・アトラスの事前転送をロック前に | Task 12・14 |
| 6 章「パスワード」 | 打鍵ごとに柱、Backspace でも柱、Esc・Ctrl+U では出さない | Task 14 |
| 7 章 | `appearance.font`（fontconfig、500 / 400、埋め込みの Inter に戻す、理由をログ）、`date_format`（検証・ロケール）、`pillar_fade_ms`・`pillar_width`・`band_width`、`display.primary` | Task 2〜4・10・12・13 |
| 7 章「フォント」 | Inter の埋め込みとライセンス文、日本語などの補助フォント | Task 9・12 |
| 8 章 | 出力が減ったときの主画面の選び直し、EGL が使えないとき重ね物なしで続ける、フォントが使えなくてもロックを続ける | Task 8・13・14 |
| 引き継ぎ（Plan 4） | `date_format` の検証、レーン数は拡大前の設定値、同じ `pillar_fade_ms`、錠の本体の定数、`push_str` が `true` のときだけ柱 | Task 1・2・7・13・14 |

### Plan 4 では扱わないもの

- 背景（ぼかし・ビネット・フェード・ぼかし解除）、`buffer_scale`、EGL 失敗時の黒画面（Plan 3）。重ね物は描画先のピクセルの高さで拡大するので、スケール 2 の出力でも同じ見た目になる。
- YubiKey・KeePassXC（Plan 5）。U2F での解除も `Machine` の状態で錠が出て開く（Plan 5 が `Event::U2fSucceeded` を送れば、この計画の変更なしで動く）。
- PKGBUILD でのフォントのライセンス文のインストール、README・設定例への `appearance.font` の追記（Plan 6）。
- 試作（`Prototype.dc.html`）にあって設計書にないもの: 錠を出し入れするときの 180ms の不透明度の変化、震えのキーフレーム間の ease-in-out（Plan 1 の `shake_offset` は直線補間）、CSS の字間が最後の文字の後にも付くことによる 1px のずれ。
- 時計の等幅数字は、フォントの `tnum` の字形ではなく、通常の数字を同じ幅の枠に並べて作る（egui に OpenType の機能を指定する口がないため）。
- 錠の丸い端は、重ね物全体が半透明になる間（解除の最後の約 0.7 秒）だけ、線と重なる部分が少し明るく見える（egui の線と円を重ねて描くため）。
- 主画面以外の高さの出力が主画面になったとき、最初のフレームで足りない大きさの字形を作る（事前転送は 1080p の大きさだけ）。
