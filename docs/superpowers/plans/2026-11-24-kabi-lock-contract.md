# kabi-lock Plan 2〜6 の共通契約

Plan 2〜6 は、前の計画の実行を待たずにまとめて書く。そのため、計画をまたいで使う
モジュール・型・関数の形と、使うクレートの API をこの文書で先に固定する。
各計画はこの契約に従って書き、契約にない公開 API を別の計画から呼ばない。

- 設計書: `docs/superpowers/specs/2026-11-24-kabi-lock-design.md`
- Plan 1（実装済み）: `docs/superpowers/plans/2026-11-24-kabi-lock-01-core.md`
- Plan 1 の引き継ぎ: `docs/superpowers/plans/2026-11-24-kabi-lock-01-core-handoff.md`
- API の確認に使った試作: `docs/superpowers/spikes/2026-11-24-api-spike.rs`（`.Cargo.toml` も同じ場所）

## 1. 実装時の決まり（全計画共通）

- 計画に書いたコードが、クレートの API の細かな違い（引数の順番、型名、トレイトの import など）で
  コンパイルできないときは、**この契約の公開 API を変えない範囲で**最小限直してよい。
  直した内容は報告に書く。契約の公開 API を変えないと直せないときは BLOCKED として止まる。
- `unsafe` は `render/` と `wl/` の中だけで使ってよい。Plan 1 のコアモジュール（`anim`, `config`,
  `lanes`, `password`, `state`）と、この契約で「純粋」と書いたモジュールには
  `#![forbid(unsafe_code)]` をファイル先頭に付ける（Plan 2 で付ける）。
- コミット規則は Plan 1 と同じ（Conventional Commits、英語の本文、書いたモデルの Co-Authored-By、
  原則 1 コミット 1 ファイル、同じ操作を複数ファイルに行うときだけまとめてよい）。
- 本物のセッションをロックするテストはしない。動作確認は入れ子の sway で行う（9 章）。
- PAM で実際の認証を試すときは、間違ったパスワードを何度も送らない（faillock でアカウントが
  ロックされるおそれがある）。PAM の結合テストは手動確認の手順として書くだけにする。

## 2. 確認済みのクレートと API（2026-11-24 時点、試作で確認）

| クレート | バージョン | 確認したこと |
|---|---|---|
| `smithay-client-toolkit` | 0.21.1 | `delegate_dispatch2!(App)` 方式。`registry_handlers![OutputState, SeatState]`。`SessionLockState::new` → `.lock(&qh)` → 出力ごとに `create_lock_surface(surface, &output, &qh)`。`SessionLockHandler::{locked, finished, configure}`。`configure.new_size: (u32, u32)`。`SeatHandler::new_capability` で `seat_state.get_keyboard(qh, &seat, None)`。`KeyboardHandler::press_key(.., event: KeyEvent)` の `event.keysym: Keysym` と `event.utf8: Option<String>`。`update_modifiers(.., modifiers: Modifiers, raw: RawModifiers, layout: u32)` |
| `calloop` / `calloop-wayland-source` | 0.14.5 / 0.4.1 | SCTK の `reexports::calloop` と `reexports::calloop_wayland_source::WaylandSource` を使う |
| `wayland-client` | 0.31.15 | `Connection::connect_to_env()`、`registry_queue_init`、`conn.backend().display_ptr()`（`wayland-egl` が `client_system` を有効にするので使える）、`delegate_noop!(App: ignore wl_buffer::WlBuffer)` |
| `wayland-protocols-wlr` | 0.3.12（feature `client`） | `screencopy::v1::client::{zwlr_screencopy_manager_v1, zwlr_screencopy_frame_v1}`。`manager.capture_output(0, &output, &qh, ())` → `Event::Buffer { format: WEnum<wl_shm::Format>, width, height, stride }` → `frame.copy(&buffer)` → `Event::Ready` / `Event::Failed`。3440x1440 で 13.6ms |
| `khronos-egl` | 6.0.0（feature `static`） | `get_platform_display(0x31D8 /* PLATFORM_WAYLAND_KHR */, display_ptr, &[ATTRIB_NONE])`。`make_current(display, None, None, Some(ctx))`（サーフェスなし）でロック前に GL を準備できる。`swap_interval(display, 0)` |
| `wayland-egl` | 0.32.11 | `WlEglSurface::new(wl_surface.id(), w, h)`、`.ptr()`、`.resize(w, h, 0, 0)` |
| `glow` | 0.17（egui_glow 0.37 が要求） | `Context::from_loader_function` |
| `egui` / `egui_glow` | 0.37 | `Context::run_pass`、`tessellate`、`Painter::new(gl, "", None, false)`、`paint_and_update_textures([w, h], ppp, &prims, &mut delta)`。**既定のフォントが入っていない**（`set_fonts` しないと文字が出ない） |
| `pam-client` | 0.5.0（`default-features = false`） | `Context::new(service, Some(user), conv)`、`authenticate(Flag::NONE)`、`acct_mgmt(Flag::NONE)`。`ConversationHandler` は `prompt_echo_on` / `prompt_echo_off`（`CString` を返す）/ `text_info` / `error_msg`。応答用の C 側メモリは解放時にゼロで消されない（pam-client の `ResponseBuffer::drop` は `free` のみ） |
| `zbus` | **5.x**（`zbus::blocking`） | zbus 6 は blocking API を削除したので 5 を使う。`blocking::Connection::session()`、`call_method(Some(dest), path, Some(iface), method, &body)`、`reply.body().deserialize::<OwnedValue>()` |
| `swayipc` | 4.0.0 | `Connection::new()`、`get_outputs()` の `focused` と `name` |
| `chrono` | 0.4.45 | `format::StrftimeItems::new(fmt).parse()` で書式の検証、`Local::now().format(fmt)` |

計測（入れ子の sway、AMD Radeon 860M）: EGL 初期化 26ms、`lock` 要求 → `locked` 29ms、configure 30ms。

## 3. モジュール構成（最終形）

```
src/
  lib.rs            モジュール宣言
  main.rs           引数・設定読み込み・app::run の呼び出し・終了コード
  anim.rs config.rs lanes.rs password.rs state.rs   （Plan 1、純粋）
  clock.rs          Plan 2、純粋: 単調時計
  input.rs          Plan 2、純粋: キー入力の分類
  scene.rs          Plan 4、純粋: 1 フレーム分の重ね物の内容を計算
  app.rs            Plan 2 で作り、Plan 3〜5 が拡張: SCTK の App とイベントループ
  wl/
    mod.rs
    outputs.rs      Plan 2: 出力の情報（名前・説明）
    screencopy.rs   Plan 3: ロック前のスクリーンショット（別スレッド・別接続）
  render/
    mod.rs          Plan 2: OutputRender（描画手段の列挙）
    shm.rs          Plan 2: wl_shm の黒画面（EGL 失敗時の予備としても使う）
    egl.rs          Plan 3: Gpu と GpuSurface
    background.rs   Plan 3: 背景シェーダー（ぼかし・フェード・ビネット・ぼかし解除）
    overlay.rs      Plan 4: egui による時計・日付・光の柱・錠
  auth/
    mod.rs          Plan 2: AuthEvent
    password.rs     Plan 2: PAM のパスワード確認スレッド
    u2f.rs          Plan 5: pam_u2f のタッチ待ちスレッド
    keepassxc.rs    Plan 5: KeePassXC / Secret Service
assets/fonts/       Plan 4: Inter（OFL）とライセンス文
packaging/          Plan 6
```

## 4. 計画をまたぐ公開 API

### Plan 2 が作るもの

```rust
// src/clock.rs（純粋）
pub struct Clock { /* start: Instant */ }
impl Clock {
    pub fn start() -> Clock;
    /// ロック開始からのミリ秒。単調増加。Wayland のタイムスタンプは使わない。
    pub fn now_ms(&self) -> u64;
}

// src/state.rs（Plan 1 のファイルに追加）
impl Phase {
    /// この状態に入ってからの経過ミリ秒（Failed / Unlocking のみ Some）。saturating_sub を使う。
    pub fn elapsed_ms(&self, now_ms: u64) -> Option<f32>;
}

// src/input.rs（純粋）
pub enum KeyAction { Submit, Backspace, Clear, Insert, Ignore }
/// keysym と、xkb が返した文字列と、Ctrl の押下状態から操作を決める。
/// Enter / KP_Enter → Submit、BackSpace → Backspace、Escape と Ctrl+U → Clear、
/// Tab と制御文字だけの文字列と空文字 → Ignore、それ以外で文字列あり → Insert。
pub fn classify(keysym: smithay_client_toolkit::seat::keyboard::Keysym, utf8: Option<&str>, ctrl: bool) -> KeyAction;
// Keysym は SCTK が再輸出している xkbcommon の型。テストでは Keysym::Return などの定数を使う

// src/auth/mod.rs
pub enum AuthEvent { PasswordChecked { ok: bool }, U2fSucceeded }

// src/auth/password.rs
pub struct PasswordChecker { /* 送信側 */ }
impl PasswordChecker {
    /// PAM の確認スレッドを起動する。結果は `events` に必ず 1 回ずつ送る
    /// （PAM エラー・パニック時も `ok: false`。送らないと Verifying のまま止まる）。
    pub fn spawn(service: String, user: String,
                 events: calloop::channel::Sender<AuthEvent>) -> PasswordChecker;
    /// パスワードを渡す。`Zeroizing<String>` のまま move する。
    pub fn check(&self, password: zeroize::Zeroizing<String>);
}

// src/render/mod.rs
pub enum OutputRender {
    Shm(shm::ShmBlack),
    // Plan 3 が追加: Gpu(egl::GpuSurface),
}

// src/render/shm.rs
pub struct ShmBlack { /* buffer */ }
impl ShmBlack {
    /// 黒い ARGB8888 バッファを作って attach + commit する。
    pub fn draw(&mut self, shm: &smithay_client_toolkit::shm::Shm, surface: &wl_surface::WlSurface,
                width: u32, height: u32, qh: &QueueHandle<crate::app::App>);
}

// src/wl/outputs.rs
pub struct OutputMeta { pub name: String, pub description: String }

// src/app.rs
pub struct App { /* SCTK の状態、lock、出力ごとの LockOutput、Machine、PasswordBuffer、
                    PasswordChecker、Clock、Config など。フィールドは Plan 2 で決めてよい */ }
/// ロックして、解除されるまでイベントループを回す。
/// ext-session-lock が使えない／拒否されたら Err。
pub fn run(config: crate::config::Config) -> Result<(), AppError>;
pub enum AppError { NoWayland(String), LockUnsupported, LockDenied, Other(String) }
```

`app.rs` の中で、出力ごとの状態は次の形にする（Plan 3〜5 がここを拡張する）:

```rust
pub(crate) struct LockOutput {
    pub(crate) meta: OutputMeta,
    pub(crate) lock_surface: SessionLockSurface,
    pub(crate) size: (u32, u32),          // configure の論理サイズ
    pub(crate) render: Option<OutputRender>, // configure 前は None
    pub(crate) frame_pending: bool,       // フレームコールバック待ち
}
```

描画の入口は `impl App { pub(crate) fn draw_output(&mut self, index: usize) }` 1 つにまとめる。
Plan 2 は `OutputRender::Shm` だけを描く。Plan 3 は `Gpu` の分岐を足し、Plan 4 はその中で主画面に重ね物を描く。

時間の進め方:
- `Clock::now_ms()` を唯一の時刻にする。
- `state::Event::Tick` は calloop の `Timer` から 50ms ごとに送る（画面が消えていてフレームコールバックが
  来なくても、失敗の解除や `ReleaseLock` が進むように）。
- アニメーション中（Plan 3 以降）はフレームコールバックでも描き直す。

起動順（Plan 2 の時点）: 設定 → Wayland 接続 → roundtrip（出力情報）→ `lock` →
全出力の lock surface を作成 → イベントループ。`locked` を待たずに lock surface を作る。

### Plan 3 が作るもの

```rust
// src/wl/screencopy.rs
pub struct Screenshot { pub output_name: String, pub width: u32, pub height: u32,
                        pub stride: u32, pub format: wl_shm::Format, pub pixels: Vec<u8> }
/// 別スレッドで新しい Wayland 接続を開き、全出力を撮る。出力ごとに失敗しうる。
pub fn spawn_capture_all() -> std::thread::JoinHandle<Vec<Screenshot>>;

// src/render/egl.rs
pub struct Gpu { /* egl, display, config, context, gl: Arc<glow::Context> */ }
impl Gpu {
    /// サーフェスなしで current にし、GL の準備まで済ませる。失敗したら Err（呼び出し側は shm に切り替える）。
    pub fn new(conn: &wayland_client::Connection) -> Result<Gpu, String>;
    pub fn gl(&self) -> &Arc<glow::Context>;
}
pub struct GpuSurface { /* WlEglSurface, egl::Surface, size */ }
impl GpuSurface {
    pub fn new(gpu: &Gpu, surface: &wl_surface::WlSurface, width: u32, height: u32) -> Result<GpuSurface, String>;
    pub fn resize(&mut self, width: u32, height: u32);
    pub fn make_current(&self, gpu: &Gpu);
    pub fn swap(&self, gpu: &Gpu);
}

// src/render/background.rs
pub struct Background { /* シェーダー、出力名ごとのテクスチャ（ぼかし済み・元画像） */ }
impl Background {
    pub fn new(gpu: &Gpu, blur: f32) -> Result<Background, String>;
    /// スクリーンショットを GPU に送り、縮小ぼかしを作っておく。
    pub fn upload(&mut self, gpu: &Gpu, shot: &crate::wl::screencopy::Screenshot);
    /// `fade` 0..1（黒→ぼかし）、`reveal` 0..1（ぼかし→くっきり）。スクショがない出力は黒。
    pub fn draw(&self, gpu: &Gpu, output_name: &str, width: u32, height: u32, fade: f32, reveal: f32);
}
```

`OutputRender` に `Gpu(egl::GpuSurface)` を足す。`Gpu::new` が失敗したら全出力を `Shm` で描く。
起動順: 設定 → 接続 → roundtrip → `spawn_capture_all` と `Gpu::new` を並行 → join →
`Background::upload` → `keepassxc`（Plan 5）→ `lock` → lock surface。
`blur` は 0.5〜50 に丸める（NaN・負は 7）。

### Plan 4 が作るもの

```rust
// src/scene.rs（純粋）
pub struct PillarDraw { pub x: f32, pub width: f32, pub frame: crate::anim::PillarFrame }
pub struct LockDraw { pub path: crate::anim::ShacklePath, pub shake_x: f32 }
pub struct Scene {
    pub opacity: f32,            // fade_alpha * (1 - reveal_progress)
    pub time_text: String,       // "HH:MM"
    pub date_text: String,       // date_format で整形済み
    pub pillars: Vec<PillarDraw>,
    pub lock: Option<LockDraw>,  // 錠を出すときだけ
}
/// 状態・時刻・レーンなどから 1 フレーム分の内容を作る。描画には依存しない。
pub fn build(/* 引数は Plan 4 で決める。Machine, now_ms, LaneAllocator, Config, 時刻文字列など */) -> Scene;

// src/render/overlay.rs
pub struct Overlay { /* egui::Context, egui_glow::Painter */ }
impl Overlay {
    /// 時計・日付のフォントを選んで登録し、必要なら fontconfig の日本語フォントを足す。
    /// フォントの選び方: fontconfig で `font_family`（設定の `appearance.font`）を探し、
    /// 太さ 500（時計）と 400（日付）のファイルを読む。見つからない・読めない（egui が扱えない
    /// 形式を含む）・fontconfig が使えない、のどれかのときは埋め込みの Inter を使う。
    /// 選んだフォントと、埋め込みに戻した理由はログ（英語）に出す。
    /// 候補の選び方と戻し方の判断は純粋な関数に分けてテストする。
    pub fn new(gpu: &Gpu, font_family: &str, date_needs_fallback: bool) -> Result<Overlay, String>;
    /// 1920x1080 基準の配置を、出力の高さに合わせて拡大して描く。
    pub fn draw(&mut self, gpu: &Gpu, width: u32, height: u32, scene: &Scene, input: &crate::config::Input);
}
```

主画面の選び方は `config::select_primary(&config.display.primary, &outputs, focused)` を使い、
`focused` は `swayipc` で取る（失敗したら `None`）。重ね物は主画面だけに描く。
`date_format` は読み込み時に `StrftimeItems::new(fmt).parse()` で検証し、不正なら `%m/%d` に戻す。
設定に `appearance.font: String`（初期値 `"Inter"`）を足すのは Plan 4（`config.rs` の変更とテストを含む）。
Plan 6 は `docs/config.example.toml` とその一致テスト、README の設定の表に `font` を足す。

### Plan 5 が作るもの

```rust
// src/auth/u2f.rs
/// 子プロセス `kabi-lock --u2f-helper <service>`（/proc/self/exe）でタッチ待ちを 1 回ずつ行い、
/// 監視スレッドで終了を待つ。成功したら AuthEvent::U2fSucceeded を 1 回送って終わる。
/// 失敗したら 1 秒あけて子を起動し直す。PAM サービスのファイルがなければ None。
pub fn spawn(service: String, events: calloop::channel::Sender<AuthEvent>) -> Option<U2fHandle>;
pub struct U2fHandle { /* 監視スレッドと現在の子プロセス */ }
impl U2fHandle {
    /// 子プロセスを止め（SIGTERM → 0.5 秒後に SIGKILL）、監視スレッドを終わらせる。Drop でも呼ぶ。
    pub fn stop(&mut self);
}
pub const HELPER_ARG: &str;                       // "--u2f-helper"
/// main.rs から呼ぶ子プロセス側の処理。終了コード: 0 成功、1 拒否、3 入力を求められた、4 準備失敗。
pub fn helper_main(service: &str) -> u8;

// src/auth/keepassxc.rs
/// lockAllDatabases を呼び、別スレッドで最大 2 秒 Secret Service の Locked を確かめる。
/// ロックされないものが残れば通知を出す。呼び出し側は待たない。
pub fn lock_before_in_background();
/// ロック中のコレクションがあれば Unlock → Prompt を呼ぶ。終了処理の中で呼ぶ（待ってよい）。
pub fn unlock_after();
```

`config.auth.u2f.enabled` と `config.keepassxc.{lock_before, unlock_after}` で切り替える。

### Plan 6 が作るもの

`packaging/pam.d/kabi-lock`（`auth include login` と `account include login`）、`contrib/pam.d/kabi-lock-u2f`（例）、
`packaging/arch/PKGBUILD`、`README.md`、`contrib/sway/kabi-lock.conf`（`bindsym $mod+l exec kabi-lock` の例）、
`docs/config.example.toml`。コードの変更はしない（必要ならバグとして報告する）。

### 計画を書いた後に確定した差分

Plan 2〜5 は、前の計画を実際に当てたコードの上に順に当てて確かめた（作業用のリポジトリで
Plan 2 → 3 → 4 → 5 の順に全タスクを適用し、ビルド・テスト・clippy・rustfmt と入れ子の sway での描画を確認）。
その過程で、上の契約から次のように変わった。各計画書の記述が正しい。

- Plan 2: `LockOutput` の先頭に `output: WlOutput` を追加。`ShmBlack::new()` と `Default`。
  `App` のフィールド名は Plan 2 の計画書のとおり（`output_state`、`seat_state`、`outputs`、`machine`、
  `checker`、`clock`、`first_configure_ms` など）。`classify` は制御文字を 1 つでも含む文字列を無視する。
  ユーザー名は `nix` の passwd 検索、なければ `$USER`。キーリピートはしない。
- Plan 3: `LockOutput` の `render` を `lock_surface` より前に移し（EGL サーフェスを先に破棄するため）、
  `scale` を追加。`App` に `gpu`、`background`。フェードは Plan 2 の `first_configure_ms` から測る（`fade_for`）。
  フレームコールバックは SCTK 0.21 の `FrameCallbackData` を使う。`blur` は swaylock-effects の
  `--effect-blur Nx5` に合わせた 5 回のボックスぼかしの半径として扱う（7 で σ 約 9.7px）。
- Plan 4: `src/fonts.rs`（純粋、`fc-list` の出力の解釈と太さの選択）を追加。依存に `skrifa` 0.44 と
  chrono の `unstable-locales`。フォントは選んだファミリーの中で近い太さ（CSS Fonts 4 の順）を使い、
  ファミリーがない・読めない・fontconfig がないときは埋め込みの Inter。`wl/outputs.rs` に `focused_output()`。
- Plan 5: U2F を子プロセスにした（上の `u2f` の API）。`unlock_after` は `main.rs` で `app::run` が
  `Ok` を返した後に呼ぶ。KeePassXC の確認と解除要求は、Secret Service の持ち主が KeePassXC のときだけ行う。
- Plan 6: `/etc/pam.d/kabi-lock` は `auth include login` と `account include login`。
  PKGBUILD の `makedepends` に `clang`（`pam-sys` の bindgen）。
- Plan 2〜5 の手動確認: `/etc/pam.d/kabi-lock` は Plan 6 まで入らないので、9 章の手順で解除するときは
  `XDG_CONFIG_HOME` の一時的な設定に `[auth]` `pam_service = "login"` を書いて使う（`/etc/pam.d/login` に
  `auth` と `account` の行があることを先に確かめる。`/etc/pam.d/swaylock` は `account` の行がないので使えない）。
  間違ったパスワードを PAM に送るのは、Plan 2〜6 の手動確認全体で Plan 4 Task 15 Step 4 の 1 回だけ。
- Plan 4: 重ね物の準備（`OverlayHost::attach_gpu` が行う、フォントの読み込みと検査、egui_glow のシェーダー、
  数字と日付の字形のアトラスの転送）は、`prepare_background` が撮影スレッドを join して `Gpu` を返した後、
  ロック要求の前に行う。設計書 5 章 2 の「撮影と並行して準備する」とは違い、撮影とは並行しない
  （GL のコンテキストを持つ `Gpu` が、撮影の join と同じ `prepare_background` から返るため）。
  そのぶんロック要求が Plan 3 の時点より約 30ms 遅くなる。ロック前に済ませることは変わらない。
  ただし fontconfig の子プロセス（`fc-list`・`fc-match`）は `OverlayHost::new`（`prepare_background` の
  撮影より前）で起動して撮影と並行に走らせ、`attach_gpu` で答えを集める。待つのは集め始めてから合わせて
  1 秒までで、過ぎたら子を kill し、埋め込みの Inter・補助フォントなしで続ける（WARN を出す）。
  ロック前の sway IPC（`focused_output`）は `SWAYSOCK` だけを見て、読み書きに 300ms の期限を付ける。
- Plan 5: Plan 2 が `App` に置いた `user` は、U2F の helper がユーザー名を自分で取るので読まれず、Plan 5 Task 6 で外した。
- Plan 6: 上の「コードの変更はしない」の例外として、`src/config.rs` のテストモジュールにテストを 1 つ
  （`example_config_matches_defaults_and_lists_every_key`。`docs/config.example.toml` が初期値と一致し、
  全キーを含むことを確かめる）足す。製品のコードは変えない。

## 5. 時間と単位

- 時刻は `Clock::now_ms() -> u64`。経過時間は `Phase::elapsed_ms` などで `f32` にしてから `anim` に渡す。
- フェードの経過は「最初の configure を受けた時刻」から測る（出力ごとではなく全体で 1 つ）。
- 画面の座標は lock surface の論理ピクセル。スケール 1 以外の出力は Plan 3 で `buffer_scale` を扱う。

## 6. ログ

- `log` クレートでメッセージは英語。`RUST_LOG` で制御（`env_logger`）。
- パスワード・入力された文字・その長さはログに出さない。

## 7. 終了コード

| コード | 意味 |
|---|---|
| 0 | 解除して終了 |
| 1 | ロックできなかった（ext-session-lock なし、拒否、Wayland に接続できない） |
| 2 | 引数が不正 |

## 8. 計画ごとの成果物（ロードマップの再掲）

| Plan | 完了時にできること |
|---|---|
| 2 | 黒い画面でロックし、パスワード（PAM）で解除できる。出力の抜き挿しに追従 |
| 3 | 背景がスクショのぼかしで、黒からフェードイン、解除時にぼかしが晴れる。EGL 失敗時は黒画面 |
| 4 | 時計・日付・光の柱・錠のアニメーションが主画面に出る |
| 5 | YubiKey タッチでも解除でき、KeePassXC のロック・解除要求が動く |
| 6 | PKGBUILD でインストールでき、README と設定例がある |

## 9. 入れ子の sway での確認手順（全計画共通）

本物のセッションをロックせずに確かめる。`swaybg` がない警告は無視してよい。

```bash
cat > /tmp/kabi-nested.conf <<'EOF'
exec sh -c 'RUST_LOG=debug /path/to/target/debug/kabi-lock > /tmp/kabi-nested.log 2>&1; swaymsg exit'
EOF
WLR_BACKENDS=wayland sway -c /tmp/kabi-nested.conf
```

入れ子の sway のウィンドウにフォーカスを当ててから操作する。パスワード認証の確認は、
PAM サービスを一時的に `/etc/pam.d/kabi-lock` = `auth sufficient pam_permit.so` にするのではなく、
自分のパスワードで 1 回だけ試す（失敗を繰り返さない）。
