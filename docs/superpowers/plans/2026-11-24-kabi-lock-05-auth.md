# kabi-lock Plan 5: 認証の追加 実装計画

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** YubiKey のタッチ（pam_u2f）でもロックを解除できるようにし、ロック前の KeePassXC の全 DB ロック（確認と通知つき）と、解除後の KeePassXC の解除要求を、設定で切り替えられる形で入れる。

**Architecture:** YubiKey のタッチ待ちは**子プロセス**で行う。kabi-lock は自分自身を `--u2f-helper <PAM サービス>`（隠し引数）で起動し直し、子（helper）は pam_u2f だけの PAM サービスで `pam_authenticate` を 1 回呼んで、結果を終了コードで返す。親の `src/auth/u2f.rs` の見張りスレッドは、子が 0 で終われば Plan 2 のパスワード確認と同じ calloop のチャンネルに `AuthEvent::U2fSucceeded` を 1 回送り、それ以外なら 1 秒あけて子を起動し直す。`pam_authenticate` はスレッドの中では止められないが、子プロセスなら止められるので、ロックを外すとき（KeePassXC の解除ダイアログを出す前）に `U2fHandle::stop` で子を止める。`src/auth/keepassxc.rs` は zbus（blocking）で KeePassXC と Secret Service を呼ぶ。テストできる判断（引数の読み取り、終了コードの対応、再試行の判断、ロックされていないコレクションの抽出、2 秒までの確認、D-Bus の値の読み取り、通知の引数）は純粋な関数に分けて TDD で作る。子プロセスの起動と停止は、PAM の代わりに `sh -c 'exit 0'` や `sleep` を helper にしたテストで確かめる。D-Bus と PAM を実際に呼ぶ部分は手動確認にする。

**Tech Stack:** Rust（edition 2024）、pam-client 0.5.0（`default-features = false`、Plan 2 が追加済み）、nix 0.32（Plan 2 が `user` で追加済み。`signal` を足す）、zbus 5（`zbus::blocking`）、smithay-client-toolkit 0.21.1 が再輸出する calloop 0.14 の `channel`、log

**Spec:** `docs/superpowers/specs/2026-11-24-kabi-lock-design.md`（6 章が中心）

**Contract:** `docs/superpowers/plans/2026-11-24-kabi-lock-contract.md`（4 章「Plan 5 が作るもの」。この計画で API を変えた。末尾の「契約への追記」を参照）

## 前提（Plan 2〜4 から受け取るもの）

この計画は Plan 2〜4 の実行後に行う。この計画のコードと差し込み位置は、Plan 2〜4 を当てた後の実際のコードに合わせて書き、そのコードに当ててビルド・テスト・clippy・fmt と headless の sway での起動まで確かめてある。

| もの | 場所（Plan 2〜4 の実際の名前） | この計画での使い方 |
|---|---|---|
| `pam-client`（`default-features = false`）、`smithay-client-toolkit`、`nix`（`default-features = false, features = ["user"]`） | `Cargo.toml` | `u2f.rs` で使う。nix に `signal` を足す（`process` も一緒に有効になる） |
| `pub enum AuthEvent { PasswordChecked { ok: bool }, U2fSucceeded }`、`impl From<AuthEvent> for state::Event`、`pub fn current_user() -> Option<String>`、`pub mod password;`、ファイル先頭の `#![forbid(unsafe_code)]` | `src/auth/mod.rs` | helper はユーザー名を `super::current_user()` で取る。`mod.rs` に `pub mod keepassxc;` と `pub mod u2f;` を足す |
| `let (auth_tx, auth_rx) = channel::channel::<AuthEvent>();` と、`auth_tx` を move する `PasswordChecker::spawn(config.auth.pam_service.clone(), user.clone(), auth_tx)` | `src/app.rs` の `run` | その行の前に `auth_tx.clone()` を取り、U2F に渡す |
| `App { config, user, checker, lock_state, lock, release_pending, exit, .. }`、`App::handle_auth`（`AuthEvent::U2fSucceeded` をログに出して `Event::from` で状態機械へ） | `src/app.rs` | `App` に `u2f: Option<U2fHandle>` を足す。どこからも読まれない `user` は外す（Task 6）。`handle_auth` は変えない |
| `app.lock_state.lock(&qh)`（ロック要求）→ `for output in app.output_state.outputs() { app.add_output(output); }` → `while app.exit.is_none() { event_loop.dispatch(..) }` | `src/app.rs` の `run` | ロック要求の直前に KeePassXC、`add_output` のループの後に U2F |
| `App::release_lock`（`lock.unlock()` → `roundtrip` → `self.lock = None` → `self.exit = Some(Ok(()))`） | `src/app.rs` | ロックを外した直後に U2F の helper を止める |
| `parse_args` / `enum Command { Run, Version, Help }` / `main`（知らない引数は終了コード 2） | `src/main.rs` | 隠しモード `--u2f-helper <サービス>` を足す。`Command::Run` で `app::run` が `Ok` を返した後に `keepassxc::unlock_after` を呼ぶ |
| `state::Machine` の U2F の扱い（入力待ち・確認中・失敗のどこからでも解除中へ、解除中に来たパスワードの結果は捨てる） | `src/state.rs`（Plan 1） | そのまま使う |

## Global Constraints

- コミットは Conventional Commits、本文は英語、末尾に書いたモデルの `Co-Authored-By` 行（例: `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`。各タスクの例の `Claude Opus 5.5` は、実際に書いたモデルの名前に置き換える）。
- 1 コミットは原則 1 ファイル（`Cargo.toml` と `Cargo.lock` は依存の変更という 1 つの操作なのでまとめる）。`git commit` の前に必ず `git status` と `git diff` を見る。
- コード中のコメントは日本語、ログメッセージは英語（`log` クレート）。パスワード・入力された文字・その長さはログに出さない。
- `src/auth/` は `unsafe` を使わない（`auth/mod.rs` の `#![forbid(unsafe_code)]` が効く。`u2f.rs` と `keepassxc.rs` の先頭にも付ける）。シグナル・`waitid`・`prctl` は nix の安全な関数で呼ぶ。
- 本物のセッションをロックしない。動作確認は入れ子の sway で行う（契約 9 章、Task 8）。
- PAM で間違ったパスワードを繰り返し送らない（faillock でアカウントがロックされるおそれがある）。**U2F 用の PAM サービスに、パスワードを求めるモジュールや `pam_faillock` を入れたものを指定しない**（タッチ待ちは失敗するたびに 1 秒ごとに繰り返すので、失敗の回数がすぐに積み上がる）。
- 設定（Plan 1 の `config`）: `auth.u2f.enabled`（初期値 `true`）、`auth.u2f.pam_service`（初期値 `"kabi-lock-u2f"`）、`keepassxc.lock_before`（初期値 `true`）、`keepassxc.unlock_after`（初期値 `true`）。
- U2F 用の PAM サービスのファイル（`contrib/pam.d/kabi-lock-u2f` の例）は Plan 6 が書く。この計画ではサービス名を設定から読むだけにする。

### この計画で決めた振る舞い（設計書の補足）

- **タッチ待ちは子プロセス（helper）で行う**（設計書 4 章の `auth/u2f` と 6 章のとおり）。解除が決まったら、KeePassXC の解除要求より前に子を止める。止めずにタッチ待ちが残ると、解除後に出す KeePassXC の解除ダイアログ（polkit 経由の pam_u2f）のタッチを横取りするおそれがあるため。今の `swaylock-touch` も `u2f-touch-check` を kill してから解除要求を出している。
  - helper は `/proc/self/exe --u2f-helper <サービス>` で起動する（`current_exe()` ではなく `/proc/self/exe`。ロック中にパッケージの更新でバイナリが置き換わっても、起動中と同じ版が動く）。標準入力・標準出力は `/dev/null`、標準エラーは親と同じ（ログ）。
  - helper の終了コード: 0 = 認証できた、1 = 認証できなかった（タッチされない・キーがない）、3 = PAM サービスが入力を求めたので断った（設定の誤り）、4 = ユーザー名が分からない／`pam_start` に失敗。2 は `main.rs` の引数の誤り。親は 0 だけを成功とみなし、ほかは再試行する。
  - helper は起動直後に `prctl(PR_SET_PDEATHSIG, SIGTERM)` を設定する。親が異常終了しても、タッチを待ったまま残らない。
  - 止め方: SIGTERM を送り、0.5 秒で終わらなければ SIGKILL。子は回収（`wait`）するまで pid が再利用されないので、親は `waitid(WEXITED | WNOWAIT)` で終わるのを待ってから、ロックを取って回収する。こうすると `stop` が送るシグナルが別のプロセスに届くことはない。
  - 止めるのは `App::release_lock` でロックを外した直後（`main` が `unlock_after` を呼ぶより前）。`U2fHandle` の `Drop` でも止まるので、エラーで `run` を抜けるときも残らない。
- **U2F は `pam_authenticate` だけを呼ぶ**（`pam_acct_mgmt` は呼ばない）。今の `u2f-touch-check` と同じ。U2F 用のサービスに `account` の行がなくても動く。
- **U2F 用の PAM サービスのファイルが `/etc/pam.d` にも `/usr/lib/pam.d` にもなければ、タッチ待ちを始めない**（警告をログに出す）。ファイルがないと PAM は `other`（Arch では `pam_deny` と `pam_warn`）を使うので、1 秒ごとに失敗して journal に警告を書き続けるだけになるため。`auth.u2f.pam_service` の初期値 `kabi-lock-u2f` はパッケージでは入れない（設計書 6 章）ので、未設定の人でもログが荒れないようにする。
- 同じ失敗が続くときは、最初の 1 回だけ `info`／`warn` で出し、以後は `debug` にする（1 秒ごとにログが流れないように）。
- **`lockAllDatabases` は返事を求めずに送る**（`NoReplyExpected`）。未保存の変更がある DB では KeePassXC が確認ダイアログを出し、返事がダイアログを閉じるまで来ないことがある。返事を待たずに 2 秒の確認へ進み、ロックされなければ通知する。送るのも確認も別スレッドで行い、呼び出し側（ロックの開始）は一切待たない。
- **KeePassXC が起動していなければ何もしない。** `org.freedesktop.secrets` は D-Bus の自動起動の対象になっていることがある（例: `~/.local/share/dbus-1/services/org.freedesktop.secrets.service` が `/usr/bin/keepassxc` を起動する）。呼ぶと KeePassXC が起動してしまうので、先に `GetNameOwner`（自動起動しない）で持ち主を確かめる。
- **Secret Service の確認と解除要求は、Secret Service の持ち主が KeePassXC と同じ接続のときだけ行う**（`org.keepassxc.KeePassXC.MainWindow` と `org.freedesktop.secrets` の一意名が同じ）。gnome-keyring など別のものが Secret Service を出しているときに、関係のない通知や解除ダイアログを出さないため。
- 確認できるのは、KeePassXC が Secret Service に公開している DB だけ（今の `swaylock-touch` と同じ制約）。
- 通知は `org.freedesktop.Notifications.Notify`（`susssasa{sv}i`）を zbus で直接呼ぶ。緊急度は critical（`urgency` = 2）で、ロック解除後も通知センターに残る。文言は英語（ログと同じく配布物として英語にそろえる。今のスクリプトは日本語だった）。
- 解除後の `Unlock(ao)` は `(unlocked: ao, prompt: o)` を返す。`prompt` が `"/"` でなければ `Prompt.Prompt("")` を呼び、`Completed` シグナルは待たない。D-Bus の呼び出しは 1 回 3 秒で打ち切る（KeePassXC が固まっていても kabi-lock の終了が止まらないように）。
- `unlock_after` は `main.rs` で、`app::run` が `Ok` を返した後に呼ぶ。このときロックは外れていて（`release_lock` が `roundtrip` まで済ませている）、U2F の helper も止まっている。ロックできなかった（`Err`）ときは呼ばない。

## ファイル構成

| ファイル | 変更 | 責務 |
|---|---|---|
| `Cargo.toml` | 変更 | nix に `signal` を足す（Task 1）。`zbus = "5"` を足す（Task 4、`Cargo.lock` も） |
| `src/auth/u2f.rs` | 作成 | helper 側（入力を断る会話関数、1 回の `pam_authenticate`、終了コード）と、親側（PAM サービスのファイルの確認、見張りスレッド、`U2fHandle::stop`） |
| `src/auth/keepassxc.rs` | 作成 | KeePassXC の全 DB ロック（裏で確認・通知）と、解除後の解除要求。純粋な判断と D-Bus の呼び出しを分ける |
| `src/auth/mod.rs` | 変更 | `pub mod keepassxc;` と `pub mod u2f;` を足す |
| `src/main.rs` | 変更 | 隠しモード `--u2f-helper <サービス>`（Task 3）、解除後の `keepassxc::unlock_after`（Task 7） |
| `src/app.rs` | 変更 | `App.u2f`、`start_u2f`、ロック要求の前の KeePassXC、ロック後の U2F、`release_lock` での停止、使われない `App.user` の削除 |

テストは各ファイルの末尾の `#[cfg(test)] mod ...` に置く。D-Bus と PAM を実際に呼ぶ部分は単体テストにせず、Task 8 の手動確認で確かめる。

---

### Task 1: U2F の helper（子プロセス側）

**Files:**
- Modify: `Cargo.toml`
- Create: `src/auth/u2f.rs`
- Modify: `src/auth/mod.rs`

**Interfaces:**
- Consumes: `pam_client::{Context, ConversationHandler, ErrorCode, Flag}`、`nix::sys::{prctl::set_pdeathsig, signal::Signal}`、`crate::auth::current_user`（Plan 2）
- Produces:
  - `pub const HELPER_ARG: &str`（= `"--u2f-helper"`）
  - `pub fn helper_main(service: &str) -> u8`（helper の本体。終了コードを返す）
  - 非公開: `TouchOnly`（会話関数）、`enum Attempt { Accepted, Rejected, AskedForInput, SetupFailed }`、`exit_code(Attempt) -> u8`、`judge(authenticated: bool, refused_prompts: u32) -> Attempt`、`touch_once(service, user) -> Attempt`、終了コードの定数 `EXIT_ACCEPTED`（0）/ `EXIT_REJECTED`（1）/ `EXIT_ASKED_FOR_INPUT`（3）/ `EXIT_SETUP_FAILED`（4）

- [ ] **Step 1: nix の `signal` を有効にする**

Run:

```bash
cargo add nix@0.32.0 --no-default-features --features user,signal
```

Expected: `Cargo.toml` の nix の行が `nix = { version = "0.32.0", default-features = false, features = ["signal", "user"] }` になる（`signal` は `process` も有効にする。`waitid` と `prctl` は `process` にある）。`Cargo.lock` は変わらない。

- [ ] **Step 2: 失敗するテストを書く**

`src/auth/u2f.rs` を次の内容で作る（実装はまだ書かない）:

```rust
//! pam_u2f（YubiKey のタッチ）による認証。
//!
//! タッチ待ちは子プロセスで行う。kabi-lock 自身を `--u2f-helper <PAM サービス>` で起動し直し、
//! 子（helper）は pam_u2f だけの PAM サービスで `pam_authenticate` を 1 回呼んで、結果を終了コードで返す。
//! 親では見張りスレッドが子の終了を待ち、成功なら `AuthEvent::U2fSucceeded` を 1 回送って終わる。
//! 失敗なら `RETRY_DELAY` あけて子を起動し直す。
//!
//! `pam_authenticate` は途中で止められないが、子プロセスなら止められる。解除が決まったら
//! `U2fHandle::stop`（`Drop` でも呼ばれる）で子を止める。YubiKey がタッチを待ったまま残ると、
//! 解除後に出す KeePassXC の解除ダイアログのタッチを横取りしてしまうため。

#![forbid(unsafe_code)]

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> CString {
        CString::new(s).unwrap()
    }

    #[test]
    fn refuses_hidden_prompt() {
        let mut conv = TouchOnly::default();
        assert_eq!(
            conv.prompt_echo_off(&text("Password: ")),
            Err(ErrorCode::CONV_ERR)
        );
        assert_eq!(conv.refused_prompts, 1);
    }

    #[test]
    fn refuses_visible_prompt() {
        let mut conv = TouchOnly::default();
        assert_eq!(
            conv.prompt_echo_on(&text("Insert your U2F device, then press ENTER.")),
            Err(ErrorCode::CONV_ERR)
        );
        assert_eq!(conv.refused_prompts, 1);
    }

    #[test]
    fn accepts_info_and_error_messages() {
        let mut conv = TouchOnly::default();
        conv.text_info(&text("Please touch the device."));
        conv.error_msg(&text("Failed to authenticate."));
        assert_eq!(conv.refused_prompts, 0);
    }

    #[test]
    fn judges_attempts() {
        assert_eq!(judge(true, 0), Attempt::Accepted);
        assert_eq!(judge(false, 0), Attempt::Rejected);
        assert_eq!(judge(false, 2), Attempt::AskedForInput);
    }

    #[test]
    fn helper_exit_codes_are_distinct() {
        let codes = [
            exit_code(Attempt::Accepted),
            exit_code(Attempt::Rejected),
            exit_code(Attempt::AskedForInput),
            exit_code(Attempt::SetupFailed),
        ];
        assert_eq!(codes, [0, 1, 3, 4]);
    }
}
```

`src/auth/mod.rs` の `pub mod password;` の行の下に 1 行足す（rustfmt はモジュール宣言を名前順に並べるので、この位置にする）:

```rust
pub mod u2f;
```

- [ ] **Step 3: テストが失敗することを確認する**

Run: `cargo test auth::u2f`
Expected: コンパイルエラー（`cannot find type TouchOnly`、`cannot find function judge` など）。

- [ ] **Step 4: 実装を書く**

`src/auth/u2f.rs` の `#![forbid(unsafe_code)]` の行とテストモジュールの間に、次を入れる（`#![forbid(unsafe_code)]` の後に空行 1 行、テストモジュールの前に空行 1 行）:

```rust
use std::ffi::{CStr, CString};

use log::debug;
use nix::sys::signal::Signal;
use pam_client::{ConversationHandler, ErrorCode, Flag};

/// helper として起動するときの隠し引数（`--help` には出さない）。
pub const HELPER_ARG: &str = "--u2f-helper";

// ---- helper（子プロセス）側 -------------------------------------------------

/// helper の終了コード: タッチで認証できた。
const EXIT_ACCEPTED: u8 = 0;
/// helper の終了コード: 認証できなかった（タッチされない・キーがないなど）。
const EXIT_REJECTED: u8 = 1;
/// helper の終了コード: PAM サービスが入力（パスワードなど）を求めたので断った。設定の誤り。
const EXIT_ASKED_FOR_INPUT: u8 = 3;
/// helper の終了コード: ユーザー名が分からない、または `pam_start` に失敗した。
const EXIT_SETUP_FAILED: u8 = 4;

/// 入力（パスワードなど）を求められても答えない会話関数。
///
/// pam_u2f の案内（「タッチしてください」など）は受け取るだけにする。
/// 入力を求められた回数を数え、PAM サービスの設定違いを終了コードで知らせるのに使う。
#[derive(Default)]
struct TouchOnly {
    refused_prompts: u32,
}

impl ConversationHandler for TouchOnly {
    fn prompt_echo_on(&mut self, _prompt: &CStr) -> Result<CString, ErrorCode> {
        self.refused_prompts += 1;
        Err(ErrorCode::CONV_ERR)
    }

    fn prompt_echo_off(&mut self, _prompt: &CStr) -> Result<CString, ErrorCode> {
        self.refused_prompts += 1;
        Err(ErrorCode::CONV_ERR)
    }

    fn text_info(&mut self, msg: &CStr) {
        debug!("u2f helper: pam info: {}", msg.to_string_lossy());
    }

    fn error_msg(&mut self, msg: &CStr) {
        debug!("u2f helper: pam error message: {}", msg.to_string_lossy());
    }
}

/// 1 回のタッチ待ちの結果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Attempt {
    Accepted,
    Rejected,
    AskedForInput,
    SetupFailed,
}

/// 結果を helper の終了コードにする。
fn exit_code(attempt: Attempt) -> u8 {
    match attempt {
        Attempt::Accepted => EXIT_ACCEPTED,
        Attempt::Rejected => EXIT_REJECTED,
        Attempt::AskedForInput => EXIT_ASKED_FOR_INPUT,
        Attempt::SetupFailed => EXIT_SETUP_FAILED,
    }
}

/// `pam_authenticate` の結果と、入力を断った回数から、1 回の結果を決める。
fn judge(authenticated: bool, refused_prompts: u32) -> Attempt {
    if authenticated {
        Attempt::Accepted
    } else if refused_prompts > 0 {
        Attempt::AskedForInput
    } else {
        Attempt::Rejected
    }
}

/// タッチ待ちを 1 回行う。
fn touch_once(service: &str, user: &str) -> Attempt {
    let Ok(mut ctx) = pam_client::Context::new(service, Some(user), TouchOnly::default()) else {
        return Attempt::SetupFailed;
    };
    let authenticated = ctx.authenticate(Flag::NONE).is_ok();
    judge(authenticated, ctx.conversation().refused_prompts)
}

/// `kabi-lock --u2f-helper <PAM サービス>` の本体。タッチ待ちを 1 回だけ行い、終了コードを返す。
///
/// パスワードは扱わない。ログには結果の種類だけを出す。
pub fn helper_main(service: &str) -> u8 {
    // 親が異常終了したら、タッチを待ったまま残らないように自分も終わる
    if let Err(e) = nix::sys::prctl::set_pdeathsig(Signal::SIGTERM) {
        debug!("u2f helper: cannot set the parent death signal: {e}");
    }
    let attempt = match super::current_user() {
        Some(user) => touch_once(service, &user),
        None => Attempt::SetupFailed,
    };
    debug!("u2f helper: {attempt:?}");
    exit_code(attempt)
}
```

要点:
- helper はタッチ待ちを 1 回だけ行い、結果を終了コードで返す。繰り返しと停止は親（Task 2）が受け持つ。
- 会話関数が返す `Err(CONV_ERR)` で、pam_u2f の `interactive` オプション（「挿して Enter」）や、パスワードを求めるモジュールには答えない。断ったら終了コード 3 にして、親が設定の誤りとしてログに出す。
- ユーザー名は親と同じ `auth::current_user()` で取る（引数で渡さない）。

- [ ] **Step 5: テストが通ることを確認する**

Run: `cargo test auth::u2f`
Expected: `test result: ok. 5 passed`

Run: `cargo clippy --all-targets -- -D warnings`
Expected: 警告なし。

Run: `cargo fmt --check`
Expected: 差分なし。差分が出たら `cargo fmt` を実行する。

- [ ] **Step 6: コミットする（ファイルごとに分ける）**

```bash
git status
git diff
git add Cargo.toml
git commit -m "build: enable nix signal and process wrappers

Needed to signal and wait for the pam_u2f helper process and to set its
parent death signal.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git add src/auth/u2f.rs
git commit -m "feat: add the pam_u2f touch helper mode

Runs a single pam_authenticate on a pam_u2f-only service, refusing any
prompt, and reports the result as the exit code.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git add src/auth/mod.rs
git commit -m "feat: register u2f auth module

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: U2F の見張りスレッド（親側）

**Files:**
- Modify: `src/auth/u2f.rs`

**Interfaces:**
- Consumes: Task 1 の終了コード、`nix::sys::{signal::kill, wait::waitid}`、`nix::unistd::Pid`、`smithay_client_toolkit::reexports::calloop::channel::Sender`、`crate::auth::AuthEvent`
- Produces:
  - `pub const RETRY_DELAY: Duration`（= 1 秒）
  - `pub struct U2fHandle`、`U2fHandle::stop(&mut self)`（何度呼んでもよい。`Drop` でも呼ばれる）
  - `pub fn spawn(service: String, events: Sender<AuthEvent>) -> Option<U2fHandle>`（PAM サービスのファイルがない・スレッドを起動できないときは `None`）
  - 非公開: `enum HelperExit`、`classify_exit(Option<i32>) -> HelperExit`、`enum Next { Succeed, Stop, Retry }`、`next_step(HelperExit, stopped: bool) -> Next`、`FailureLog`、`find_service_file(service, dirs) -> Option<PathBuf>`、`start(make, on_success, retry_delay) -> io::Result<U2fHandle>`（helper のコマンドを作る関数を引数で受け取る。テストで差し替える）、`supervise`、`wait_and_reap`

- [ ] **Step 1: 失敗するテストを書く**

`src/auth/u2f.rs` の末尾（`mod tests` の後、空行 1 行あけて）に、次のテストモジュールを足す:

```rust
#[cfg(test)]
mod supervisor_tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Instant;

    #[test]
    fn exit_codes_round_trip() {
        for (attempt, exit) in [
            (Attempt::Accepted, HelperExit::Accepted),
            (Attempt::Rejected, HelperExit::Rejected),
            (Attempt::AskedForInput, HelperExit::AskedForInput),
            (Attempt::SetupFailed, HelperExit::SetupFailed),
        ] {
            assert_eq!(classify_exit(Some(i32::from(exit_code(attempt)))), exit);
        }
    }

    #[test]
    fn only_zero_means_accepted() {
        assert_eq!(exit_code(Attempt::Accepted), 0);
        // 2 は引数の誤り（main.rs）。知らないコードとして扱う
        assert_eq!(classify_exit(Some(2)), HelperExit::Other(Some(2)));
        assert_eq!(classify_exit(None), HelperExit::Other(None));
    }

    #[test]
    fn next_step_table() {
        assert_eq!(next_step(HelperExit::Accepted, false), Next::Succeed);
        assert_eq!(next_step(HelperExit::Rejected, false), Next::Retry);
        assert_eq!(next_step(HelperExit::NotStarted, false), Next::Retry);
        assert_eq!(next_step(HelperExit::Other(None), false), Next::Retry);
        assert_eq!(next_step(HelperExit::Accepted, true), Next::Stop);
        assert_eq!(next_step(HelperExit::Other(None), true), Next::Stop);
    }

    #[test]
    fn failure_log_suppresses_repeats() {
        let mut log = FailureLog::default();
        assert!(log.is_new(HelperExit::Rejected));
        assert!(!log.is_new(HelperExit::Rejected));
        assert!(log.is_new(HelperExit::AskedForInput));
        assert!(log.is_new(HelperExit::Rejected));
    }

    /// テスト用の一時ディレクトリ（テストごとに別の名前）。
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kabi-u2f-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn finds_service_file_in_later_dir() {
        let first = temp_dir("first");
        let second = temp_dir("second");
        std::fs::write(second.join("kabi-lock-u2f"), "auth required pam_u2f.so\n").unwrap();
        let dirs = [first.to_str().unwrap(), second.to_str().unwrap()];
        assert_eq!(
            find_service_file("kabi-lock-u2f", &dirs),
            Some(second.join("kabi-lock-u2f"))
        );
        std::fs::remove_dir_all(first).unwrap();
        std::fs::remove_dir_all(second).unwrap();
    }

    #[test]
    fn missing_service_file_is_none() {
        let dir = temp_dir("missing");
        let dirs = [dir.to_str().unwrap()];
        assert_eq!(find_service_file("kabi-lock-u2f", &dirs), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_empty_or_path_like_service_names() {
        let dir = temp_dir("names");
        std::fs::write(dir.join("x"), "").unwrap();
        let dirs = [dir.to_str().unwrap()];
        assert_eq!(find_service_file("", &dirs), None);
        assert_eq!(find_service_file("../x", &dirs), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    // ---- 見張りスレッド（PAM の代わりに true / false / sleep を helper にする） ----

    const SHORT: Duration = Duration::from_millis(20);
    /// 「すぐ止まる」の上限。負荷の高いビルド機（PKGBUILD の check()）でも揺れないよう広めにとる。
    /// helper の `sleep 30` や再試行の待ち 60 秒よりは十分に短いので、止まらないときは見分けられる。
    const QUICK: Duration = Duration::from_secs(5);

    fn sh(script: &'static str) -> impl Fn() -> Command + Send + 'static {
        move || {
            let mut command = Command::new("sh");
            command.arg("-c").arg(script);
            command
        }
    }

    /// 成功の知らせを受け取るチャンネル。
    fn success_channel() -> (impl FnOnce() + Send + 'static, mpsc::Receiver<()>) {
        let (tx, rx) = mpsc::channel();
        (
            move || {
                let _ = tx.send(());
            },
            rx,
        )
    }

    /// helper が起動するまで待って、その pid を返す。
    fn wait_for_child(handle: &U2fHandle) -> i32 {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(child) = &handle.shared.lock().child {
                return child.id() as i32;
            }
            assert!(Instant::now() < deadline, "helper did not start");
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// 回収済みで、もう存在しないか。
    fn process_gone(pid: i32) -> bool {
        kill(Pid::from_raw(pid), None) == Err(Errno::ESRCH)
    }

    #[test]
    fn success_is_reported_once_and_the_thread_ends() {
        let (on_success, rx) = success_channel();
        let mut handle = start(sh("exit 0"), on_success, SHORT).unwrap();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let started = Instant::now();
        handle.stop();
        assert!(started.elapsed() < QUICK);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn failures_are_retried_until_success() {
        let calls = Arc::new(Mutex::new(0u32));
        let make = {
            let calls = Arc::clone(&calls);
            move || {
                let mut calls = calls.lock().unwrap();
                *calls += 1;
                let mut command = Command::new("sh");
                command
                    .arg("-c")
                    .arg(if *calls < 3 { "exit 1" } else { "exit 0" });
                command
            }
        };
        let (on_success, rx) = success_channel();
        let _handle = start(make, on_success, SHORT).unwrap();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(*calls.lock().unwrap(), 3);
    }

    #[test]
    fn stop_kills_a_waiting_helper() {
        let (on_success, rx) = success_channel();
        let mut handle = start(sh("exec sleep 30"), on_success, SHORT).unwrap();
        let pid = wait_for_child(&handle);
        let started = Instant::now();
        handle.stop();
        assert!(started.elapsed() < QUICK);
        assert!(process_gone(pid));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn drop_stops_the_helper() {
        let (on_success, _rx) = success_channel();
        let handle = start(sh("exec sleep 30"), on_success, SHORT).unwrap();
        let pid = wait_for_child(&handle);
        drop(handle);
        assert!(process_gone(pid));
    }

    #[test]
    fn a_helper_ignoring_sigterm_is_killed() {
        let (on_success, _rx) = success_channel();
        let mut handle = start(sh("trap '' TERM; exec sleep 30"), on_success, SHORT).unwrap();
        let pid = wait_for_child(&handle);
        // sh が trap を設定し終えるまで待つ（その前の SIGTERM では普通に終わってしまう）
        thread::sleep(Duration::from_millis(200));
        let started = Instant::now();
        handle.stop();
        let elapsed = started.elapsed();
        assert!(elapsed >= KILL_GRACE, "{elapsed:?}");
        assert!(elapsed < KILL_GRACE + QUICK, "{elapsed:?}");
        assert!(process_gone(pid));
    }

    #[test]
    fn stop_during_the_retry_delay_is_quick() {
        let (on_success, rx) = success_channel();
        let mut handle = start(sh("exit 1"), on_success, Duration::from_secs(60)).unwrap();
        // 最初の helper が終わって、再試行の待ちに入るまで待つ
        thread::sleep(Duration::from_millis(200));
        let started = Instant::now();
        handle.stop();
        assert!(started.elapsed() < QUICK);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn a_command_that_cannot_start_is_retried() {
        let (on_success, rx) = success_channel();
        let mut handle = start(
            || Command::new("/nonexistent/kabi-lock-helper"),
            on_success,
            SHORT,
        )
        .unwrap();
        thread::sleep(Duration::from_millis(100));
        handle.stop();
        assert!(rx.try_recv().is_err());
    }
}
```

見張りスレッドのテストは PAM を使わない。`sh -c 'exit 0'`（成功）、`sh -c 'exit 1'`（失敗）、`exec sleep 30`（タッチ待ちの代わり）、`trap '' TERM; exec sleep 30`（SIGTERM を無視する helper）を helper にする。

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test auth::u2f`
Expected: コンパイルエラー（`cannot find function classify_exit`、`cannot find function start` など）。

- [ ] **Step 3: `use` を置き換える**

`src/auth/u2f.rs` の次の `use` の並び:

```rust
use std::ffi::{CStr, CString};

use log::debug;
use nix::sys::signal::Signal;
use pam_client::{ConversationHandler, ErrorCode, Flag};
```

を、次に置き換える:

```rust
use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use log::{debug, info, warn};
use nix::errno::Errno;
use nix::sys::signal::{Signal, kill};
use nix::sys::wait::{Id, WaitPidFlag, waitid};
use nix::unistd::Pid;
use pam_client::{ConversationHandler, ErrorCode, Flag};
use smithay_client_toolkit::reexports::calloop::channel::Sender;

use super::AuthEvent;
```

- [ ] **Step 4: 実装を書く**

`src/auth/u2f.rs` の `mod tests` の前（`#[cfg(test)]` の行の直前。`helper_main` との間と、`#[cfg(test)]` との間に空行 1 行）に、次を入れる:

```rust
// ---- 親（見張りスレッド）側 -------------------------------------------------

/// 失敗してから次の helper を起動するまでの間隔（キーが挿さっていないときの空回り防止）。
pub const RETRY_DELAY: Duration = Duration::from_secs(1);

/// `stop` で SIGTERM を送ってから、SIGKILL に切り替えるまでの猶予。
const KILL_GRACE: Duration = Duration::from_millis(500);

/// helper として起動する実行ファイル。自分自身の実行ファイルを指す。
///
/// `current_exe()` ではなくこちらを使う。ロック中にパッケージの更新でバイナリが置き換わっても、
/// 起動中と同じ版（引数の形が同じ）の helper が起動する。
const SELF_EXE: &str = "/proc/self/exe";

/// PAM がサービスの設定ファイルを探す場所（Linux-PAM: `/etc/pam.d`、次にベンダーの `/usr/lib/pam.d`）。
const PAM_DIRS: [&str; 2] = ["/etc/pam.d", "/usr/lib/pam.d"];

/// 親から見た helper の終わり方。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HelperExit {
    Accepted,
    Rejected,
    AskedForInput,
    SetupFailed,
    /// 知らない終了コード、またはシグナルで終わった（`None`）。
    Other(Option<i32>),
    /// 起動できなかった。
    NotStarted,
}

/// 終了コードから終わり方を読む。`None` はシグナルで終わったとき。
fn classify_exit(code: Option<i32>) -> HelperExit {
    match code {
        Some(c) if c == i32::from(EXIT_ACCEPTED) => HelperExit::Accepted,
        Some(c) if c == i32::from(EXIT_REJECTED) => HelperExit::Rejected,
        Some(c) if c == i32::from(EXIT_ASKED_FOR_INPUT) => HelperExit::AskedForInput,
        Some(c) if c == i32::from(EXIT_SETUP_FAILED) => HelperExit::SetupFailed,
        other => HelperExit::Other(other),
    }
}

/// helper が終わった後にすること。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Next {
    /// 成功を知らせて終わる。
    Succeed,
    /// 止められたので、何も知らせずに終わる。
    Stop,
    /// 間をあけて起動し直す。
    Retry,
}

/// 止められていれば（成功していても）何も知らせない。解除はもう決まっているため。
fn next_step(exit: HelperExit, stopped: bool) -> Next {
    if stopped {
        Next::Stop
    } else if exit == HelperExit::Accepted {
        Next::Succeed
    } else {
        Next::Retry
    }
}

/// 同じ失敗が 1 秒ごとに続くとき、ログを繰り返さないための記録。
#[derive(Default)]
struct FailureLog {
    last: Option<HelperExit>,
}

impl FailureLog {
    /// 直前と違う失敗なら `true`（そのときだけ目立つレベルでログに出す）。
    fn is_new(&mut self, exit: HelperExit) -> bool {
        let new = self.last != Some(exit);
        self.last = Some(exit);
        new
    }
}

fn report(log: &mut FailureLog, exit: HelperExit) {
    if !log.is_new(exit) {
        debug!("u2f: attempt failed again: {exit:?}");
        return;
    }
    match exit {
        HelperExit::Rejected => {
            info!("u2f: no touch accepted; retrying every {RETRY_DELAY:?}");
        }
        HelperExit::AskedForInput => warn!(
            "u2f: the pam service asked for input and was refused; it should contain only pam_u2f"
        ),
        HelperExit::SetupFailed => warn!("u2f: the helper could not start a PAM transaction"),
        HelperExit::Other(code) => warn!("u2f: the helper ended unexpectedly (exit code {code:?})"),
        HelperExit::NotStarted => warn!("u2f: the helper could not be started"),
        HelperExit::Accepted => {}
    }
}

/// PAM サービスの設定ファイルを `dirs` から探す。見つからなければ `None`。
///
/// ファイルがないと PAM は `other`（Arch では拒否して警告ログを出す）を使うため、
/// 1 秒ごとに失敗とログを繰り返すだけになる。その場合はタッチ待ちを始めない。
fn find_service_file(service: &str, dirs: &[&str]) -> Option<PathBuf> {
    if service.is_empty() || service.contains('/') {
        return None;
    }
    dirs.iter()
        .map(|dir| Path::new(dir).join(service))
        .find(|path| path.is_file())
}

/// 見張りスレッドと `U2fHandle` が共有する状態。
struct State {
    stopped: bool,
    /// 起動中の helper。回収（wait）するまで `Some` なので、その間は pid が別のプロセスに使われない。
    child: Option<Child>,
}

struct Shared {
    state: Mutex<State>,
    /// `stopped` か `child` が変わったら知らせる。
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// YubiKey のタッチ待ちの見張りスレッド。`stop` するか捨てると、待っている helper を止める。
pub struct U2fHandle {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl U2fHandle {
    /// 待っている helper を止め（SIGTERM、`KILL_GRACE` で終わらなければ SIGKILL）、
    /// 見張りスレッドの終わりを待つ。何度呼んでもよい。
    pub fn stop(&mut self) {
        let Some(thread) = self.thread.take() else {
            return;
        };
        {
            let mut state = self.shared.lock();
            state.stopped = true;
            if let Some(child) = &state.child {
                signal(child, Signal::SIGTERM);
            }
            self.shared.changed.notify_all();
            let (mut state, _) = self
                .shared
                .changed
                .wait_timeout_while(state, KILL_GRACE, |s| s.child.is_some())
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(child) = state.child.as_mut() {
                warn!("u2f: the helper ignored SIGTERM; killing it");
                if let Err(e) = child.kill() {
                    warn!("u2f: cannot kill the helper: {e}");
                }
            }
        }
        if thread.join().is_err() {
            warn!("u2f: the supervisor thread panicked");
        }
        debug!("u2f: stopped");
    }
}

impl Drop for U2fHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 回収前の子にシグナルを送る。
fn signal(child: &Child, signal: Signal) {
    let Ok(pid) = i32::try_from(child.id()) else {
        return;
    };
    if let Err(e) = kill(Pid::from_raw(pid), signal) {
        debug!("u2f: cannot send {signal} to the helper: {e}");
    }
}

/// 見張りスレッドを起動する。`make` は helper を起動するコマンドを作る（テストでは別のコマンドに替える）。
fn start<F, S>(make: F, on_success: S, retry_delay: Duration) -> std::io::Result<U2fHandle>
where
    F: Fn() -> Command + Send + 'static,
    S: FnOnce() + Send + 'static,
{
    let shared = Arc::new(Shared {
        state: Mutex::new(State {
            stopped: false,
            child: None,
        }),
        changed: Condvar::new(),
    });
    let thread = thread::Builder::new().name("kabi-u2f".into()).spawn({
        let shared = Arc::clone(&shared);
        move || supervise(&shared, make, on_success, retry_delay)
    })?;
    Ok(U2fHandle {
        shared,
        thread: Some(thread),
    })
}

/// helper を起動して終わりを待つ、を成功するか止められるまで繰り返す。
fn supervise<F, S>(shared: &Shared, make: F, on_success: S, retry_delay: Duration)
where
    F: Fn() -> Command,
    S: FnOnce(),
{
    let mut log = FailureLog::default();
    loop {
        let pid = {
            let mut state = shared.lock();
            if state.stopped {
                return;
            }
            let spawned = make().stdin(Stdio::null()).stdout(Stdio::null()).spawn();
            match spawned {
                Ok(child) => {
                    let pid = child.id();
                    state.child = Some(child);
                    Some(pid)
                }
                Err(e) => {
                    debug!("u2f: cannot start the helper: {e}");
                    None
                }
            }
        };
        let exit = match pid {
            Some(pid) => classify_exit(wait_and_reap(shared, pid)),
            None => HelperExit::NotStarted,
        };
        let stopped = shared.lock().stopped;
        match next_step(exit, stopped) {
            Next::Succeed => {
                info!("u2f: touch accepted");
                on_success();
                return;
            }
            Next::Stop => return,
            Next::Retry => {
                report(&mut log, exit);
                let state = shared.lock();
                let _ = shared
                    .changed
                    .wait_timeout_while(state, retry_delay, |s| !s.stopped);
            }
        }
    }
}

/// helper の終わりを待って回収し、終了コードを返す（シグナルで終わったら `None`）。
///
/// まず `WNOWAIT` で終わるのを待つだけにして、回収はロックを取ってから行う。回収前なら pid は
/// 再利用されないので、その間に `stop` がシグナルを送っても別のプロセスに届くことはない。
fn wait_and_reap(shared: &Shared, pid: u32) -> Option<i32> {
    let target = Pid::from_raw(i32::try_from(pid).ok()?);
    loop {
        match waitid(Id::Pid(target), WaitPidFlag::WEXITED | WaitPidFlag::WNOWAIT) {
            Ok(_) => break,
            Err(Errno::EINTR) => continue,
            Err(e) => {
                warn!("u2f: cannot wait for the helper: {e}");
                // 回収できる状態に確実にするため、止めてから回収する
                if let Some(child) = shared.lock().child.as_mut() {
                    let _ = child.kill();
                }
                break;
            }
        }
    }
    let mut state = shared.lock();
    let status = state.child.take().and_then(|mut child| child.wait().ok());
    shared.changed.notify_all();
    status.and_then(|s| s.code())
}

/// pam_u2f の PAM サービスで、成功するまでタッチ待ちを繰り返す（helper を起動し直す）。
///
/// 成功したら `AuthEvent::U2fSucceeded` を 1 回送って見張りスレッドが終わる。失敗したら 1 秒あけて再試行。
/// PAM サービスの設定ファイルがない、またはスレッドを起動できなければ `None`（タッチでは解除できない）。
pub fn spawn(service: String, events: Sender<AuthEvent>) -> Option<U2fHandle> {
    let Some(file) = find_service_file(&service, &PAM_DIRS) else {
        warn!("u2f: pam service {service:?} not found in {PAM_DIRS:?}; YubiKey unlock is off");
        return None;
    };
    let make = move || {
        let mut command = Command::new(SELF_EXE);
        command.arg(HELPER_ARG).arg(&service);
        command
    };
    let on_success = move || {
        if events.send(AuthEvent::U2fSucceeded).is_err() {
            debug!("u2f: event loop is gone; dropping the result");
        }
    };
    match start(make, on_success, RETRY_DELAY) {
        Ok(handle) => {
            info!("u2f: waiting for a touch (pam service {})", file.display());
            Some(handle)
        }
        Err(e) => {
            warn!("u2f: cannot start the supervisor thread: {e}");
            None
        }
    }
}
```

要点:
- `spawn` は PAM サービスのファイルを確かめてから見張りスレッドを起動し、すぐ戻る。イベントループは PAM を待たない。
- 見張りスレッドは helper を起動 → 終わるまで待つ → 0 なら `on_success`（`AuthEvent::U2fSucceeded` を送る）を 1 回呼んで終わる。それ以外なら `RETRY_DELAY` 待って起動し直す。待っている間に `stop` されたらすぐ終わる。
- `stop` は SIGTERM → 最大 `KILL_GRACE`（0.5 秒）待つ → まだ生きていれば SIGKILL → 見張りスレッドの終わりを待つ。止められた後に helper が 0 で終わっていても、成功は知らせない（解除はもう決まっている）。
- 子の回収は見張りスレッドだけが行う。`waitid` に `WNOWAIT` を付けて「終わった」ことだけを先に知り、回収はロックを取ってから行うので、`stop` が回収済みの pid にシグナルを送ることはない。

- [ ] **Step 5: テストが通ることを確認する**

Run: `cargo test auth::u2f`
Expected: `test result: ok. 19 passed`（1 秒弱かかる。SIGTERM を無視する helper のテストが 0.5 秒待つため）

Run: `pgrep -fa 'sleep 30'`
Expected: 何も出ない（テストで起動した helper の代わりが残っていない）。

Run: `cargo clippy --all-targets -- -D warnings`
Expected: 警告なし。

Run: `cargo fmt --check`
Expected: 差分なし。差分が出たら `cargo fmt` を実行する。

- [ ] **Step 6: コミットする**

```bash
git status
git diff
git add src/auth/u2f.rs
git commit -m "feat: supervise the u2f helper and stop it on demand

A supervisor thread re-runs the helper process until it succeeds and
sends U2fSucceeded once. U2fHandle::stop (also on drop) terminates a
waiting helper so no touch request outlives the lock.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: 隠しモード `--u2f-helper`（`main.rs`）

**Files:**
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: `kabi_lock::auth::u2f::{HELPER_ARG, helper_main}`
- Produces: `Command::U2fHelper(String)`。`kabi-lock --u2f-helper <サービス>` で helper として動き、`helper_main` の値を終了コードにする。`--help` の表示には出さない。

- [ ] **Step 1: 失敗するテストを書く**

`src/main.rs` のテストモジュールで、次の部分:

```rust
    #[test]
    fn exit_codes_follow_the_contract() {
```

を、次に置き換える:

```rust
    #[test]
    fn hidden_u2f_helper_mode() {
        assert_eq!(
            parse(&["--u2f-helper", "u2f-touch"]),
            Ok(Command::U2fHelper("u2f-touch".to_string()))
        );
        assert_eq!(
            parse(&["--u2f-helper"]),
            Err("missing PAM service after --u2f-helper".to_string())
        );
        assert_eq!(
            parse(&["--u2f-helper", "u2f-touch", "extra"]),
            Err("too many arguments".to_string())
        );
    }

    #[test]
    fn helper_mode_is_not_in_the_usage() {
        assert!(!USAGE.contains(u2f::HELPER_ARG));
    }

    #[test]
    fn exit_codes_follow_the_contract() {
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test --bin kabi-lock`
Expected: コンパイルエラー（`no variant named U2fHelper`、`failed to resolve: use of undeclared ... u2f` など）。

- [ ] **Step 3: 実装を書く**

`src/main.rs` を次の 4 か所で変える。

(a) `use` を足す。次の行:

```rust
use kabi_lock::app::{self, AppError};
```

を、次にする:

```rust
use kabi_lock::app::{self, AppError};
use kabi_lock::auth::u2f;
```

(b) `enum Command` に隠しモードを足す。次の部分:

```rust
    Version,
    Help,
}
```

を、次にする:

```rust
    Version,
    Help,
    /// 隠しモード: YubiKey のタッチ待ちを 1 回だけ行う子プロセス（Plan 5）。
    U2fHelper(String),
}
```

(c) `parse_args` で読む。次の行:

```rust
        Some("--help" | "-h") => Command::Help,
```

を、次にする:

```rust
        Some("--help" | "-h") => Command::Help,
        Some(arg) if arg == u2f::HELPER_ARG => match args.next() {
            Some(service) => Command::U2fHelper(service),
            None => return Err(format!("missing PAM service after {arg}")),
        },
```

(d) `main` の `match command` の最後に腕を足す。`Command::Run` の腕の終わりから関数の終わりまでの次の部分:

```rust
            ExitCode::from(exit_code(&result))
        }
    }
}
```

を、次にする:

```rust
            ExitCode::from(exit_code(&result))
        }
        Command::U2fHelper(service) => {
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
                .init();
            ExitCode::from(u2f::helper_main(&service))
        }
    }
}
```

`USAGE` は変えない（隠しモードは利用者向けではない）。

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test --bin kabi-lock`
Expected: `test result: ok. 6 passed`

Run: `cargo build && ./target/debug/kabi-lock --u2f-helper; echo "exit=$?"`
Expected: `kabi-lock: missing PAM service after --u2f-helper` と使い方が出て `exit=2`（PAM は呼ばれない）。

Run: `./target/debug/kabi-lock --help | grep -c u2f`
Expected: `0`

Run: `cargo clippy --all-targets -- -D warnings`
Expected: 警告なし。

Run: `cargo fmt --check`
Expected: 差分なし。差分が出たら `cargo fmt` を実行する。

- [ ] **Step 5: コミットする**

```bash
git status
git diff
git add src/main.rs
git commit -m "feat: add the hidden --u2f-helper mode

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: zbus の追加と、KeePassXC 連携の判断（純粋な部分）

**Files:**
- Modify: `Cargo.toml`, `Cargo.lock`, `src/auth/mod.rs`
- Create: `src/auth/keepassxc.rs`

**Interfaces:**
- Consumes: `zbus::message::{Flags, Message}`、`zbus::zvariant::{OwnedObjectPath, OwnedValue, Value}`
- Produces（すべて非公開。Task 5 の D-Bus 呼び出しから使う）:
  - 定数 `KEEPASSXC_NAME` / `KEEPASSXC_PATH` / `KEEPASSXC_IFACE`、`pub const CHECK_INTERVAL: Duration`（200ms）、`pub const CHECK_TIMEOUT: Duration`（2 秒）、`CHECK_RETRIES`（10）、`URGENCY_CRITICAL`（2）
  - `struct Collection { path: String, locked: bool }`
  - `type UnlockReply = (Vec<OwnedObjectPath>, OwnedObjectPath)`（`aoo`）、`type NotifyArgs<'a>`（`susssasa{sv}i`）
  - `keepassxc_serves_secrets(Option<&str>, Option<&str>) -> bool`
  - `still_unlocked(&[Collection]) -> Vec<String>`、`locked_paths(&[Collection]) -> Vec<String>`
  - `wait_until_locked(read: impl FnMut() -> Option<Vec<Collection>>, sleep: impl FnMut(Duration)) -> Vec<String>`
  - `prompt_to_open(&str) -> Option<&str>`、`collection_label(&str) -> &str`、`notification_text(&[String]) -> String`、`notify_args(&str) -> NotifyArgs<'_>`
  - `parse_paths(OwnedValue) -> Option<Vec<String>>`、`parse_bool(OwnedValue) -> Option<bool>`
  - `lock_all_message() -> zbus::Result<Message>`（`lockAllDatabases` を `NoReplyExpected` で呼ぶメッセージ。送らずに作るだけ）

- [ ] **Step 1: 依存クレートを確かめて、zbus を追加する**

Run: `grep -E '^(pam-client|smithay-client-toolkit|zbus) ' Cargo.toml`
Expected: `pam-client = { version = "0.5.0", default-features = false }` と `smithay-client-toolkit = "0.21.1"` がある（Plan 2 が追加済み）。`zbus` はまだない。

Run:

```bash
cargo add zbus@5
```

Expected: `Cargo.toml` の `[dependencies]` に `zbus = "5"` が入る（zbus 6 は blocking API がないので 5 に固定する。契約 2 章）。

- [ ] **Step 2: 失敗するテストを書く**

`src/auth/keepassxc.rs` を次の内容で作る（実装はまだ書かない）:

```rust
//! KeePassXC と Secret Service の連携。
//!
//! - ロック前: KeePassXC に全 DB のロックを頼み（返事は待たない）、別スレッドで Secret Service の
//!   各コレクションの `Locked` を最大 2 秒確かめる。ロックされないものが残れば通知を出す。
//! - 解除後: Secret Service でロック中のコレクションがあれば `Unlock` を呼び、返ったプロンプトの
//!   `Prompt` を呼ぶ（KeePassXC の解除ダイアログが出る）。プロンプトの完了は待たない。
//!
//! KeePassXC が起動していなければ何もしない。`org.freedesktop.secrets` は D-Bus の自動起動の
//! 対象になっていることがあるので、名前の持ち主を先に確かめ、いないときは呼ばない。
//! D-Bus の呼び出しと、テストできる判断（純粋な関数）を分けて書く。

#![forbid(unsafe_code)]

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use zbus::zvariant::{ObjectPath, Type};

    fn col(path: &str, locked: bool) -> Collection {
        Collection {
            path: path.into(),
            locked,
        }
    }

    const A: &str = "/org/freedesktop/secrets/collection/Passwords";
    const B: &str = "/org/freedesktop/secrets/collection/Work";

    #[test]
    fn same_owner_serves_secrets() {
        assert!(keepassxc_serves_secrets(Some(":1.10"), Some(":1.10")));
    }

    #[test]
    fn other_owner_does_not_serve_secrets() {
        assert!(!keepassxc_serves_secrets(Some(":1.10"), Some(":1.11")));
        assert!(!keepassxc_serves_secrets(Some(":1.10"), None));
        assert!(!keepassxc_serves_secrets(None, Some(":1.11")));
        assert!(!keepassxc_serves_secrets(None, None));
    }

    #[test]
    fn splits_locked_and_unlocked() {
        let cols = [col(A, true), col(B, false)];
        assert_eq!(still_unlocked(&cols), vec![B.to_string()]);
        assert_eq!(locked_paths(&cols), vec![A.to_string()]);
    }

    #[test]
    fn stops_at_once_when_all_locked() {
        let reads = Cell::new(0);
        let mut slept = Duration::ZERO;
        let open = wait_until_locked(
            || {
                reads.set(reads.get() + 1);
                Some(vec![col(A, true)])
            },
            |d| slept += d,
        );
        assert!(open.is_empty());
        assert_eq!(reads.get(), 1);
        assert_eq!(slept, Duration::ZERO);
    }

    #[test]
    fn no_collections_means_nothing_to_wait_for() {
        let mut slept = Duration::ZERO;
        let open = wait_until_locked(|| Some(Vec::new()), |d| slept += d);
        assert!(open.is_empty());
        assert_eq!(slept, Duration::ZERO);
    }

    #[test]
    fn waits_until_the_lock_takes_effect() {
        let reads = Cell::new(0);
        let mut slept = Duration::ZERO;
        let open = wait_until_locked(
            || {
                reads.set(reads.get() + 1);
                Some(vec![col(A, reads.get() >= 4)])
            },
            |d| slept += d,
        );
        assert!(open.is_empty());
        assert_eq!(reads.get(), 4);
        assert_eq!(slept, CHECK_INTERVAL * 3);
    }

    #[test]
    fn gives_up_after_the_timeout() {
        let mut slept = Duration::ZERO;
        let open = wait_until_locked(|| Some(vec![col(A, true), col(B, false)]), |d| slept += d);
        assert_eq!(open, vec![B.to_string()]);
        assert_eq!(slept, CHECK_TIMEOUT);
    }

    #[test]
    fn read_failure_ends_the_check_quietly() {
        let reads = Cell::new(0);
        let open = wait_until_locked(
            || {
                reads.set(reads.get() + 1);
                if reads.get() == 1 {
                    Some(vec![col(A, false)])
                } else {
                    None
                }
            },
            |_| {},
        );
        assert!(open.is_empty());
        assert_eq!(reads.get(), 2);
    }

    #[test]
    fn slash_prompt_means_no_prompt() {
        assert_eq!(prompt_to_open("/"), None);
        assert_eq!(prompt_to_open(""), None);
        assert_eq!(
            prompt_to_open("/org/freedesktop/secrets/prompt/p1"),
            Some("/org/freedesktop/secrets/prompt/p1")
        );
    }

    #[test]
    fn label_is_the_last_path_segment() {
        assert_eq!(collection_label(A), "Passwords");
        assert_eq!(collection_label("Passwords"), "Passwords");
    }

    #[test]
    fn notification_names_the_unlocked_databases() {
        let text = notification_text(&[A.to_string(), B.to_string()]);
        assert!(text.contains("Passwords, Work"), "{text}");
    }

    #[test]
    fn notify_args_match_the_dbus_signature() {
        assert_eq!(NotifyArgs::SIGNATURE.to_string(), "(susssasa{sv}i)");
    }

    #[test]
    fn unlock_reply_matches_the_dbus_signature() {
        assert_eq!(UnlockReply::SIGNATURE.to_string(), "(aoo)");
    }

    #[test]
    fn lock_all_does_not_wait_for_a_reply() {
        let msg = lock_all_message().unwrap();
        let header = msg.header();
        assert!(
            msg.primary_header()
                .flags()
                .contains(Flags::NoReplyExpected)
        );
        assert_eq!(header.member().unwrap().as_str(), "lockAllDatabases");
        assert_eq!(header.path().unwrap().as_str(), KEEPASSXC_PATH);
        assert_eq!(header.interface().unwrap().as_str(), KEEPASSXC_IFACE);
        assert_eq!(header.destination().unwrap().to_string(), KEEPASSXC_NAME);
    }

    #[test]
    fn notify_args_are_critical_and_new() {
        let (app, replaces, _, summary, body, actions, hints, timeout) = notify_args("x");
        assert_eq!(app, "kabi-lock");
        assert_eq!(replaces, 0);
        assert_eq!(summary, "KeePassXC");
        assert_eq!(body, "x");
        assert!(actions.is_empty());
        assert_eq!(hints.get("urgency"), Some(&Value::U8(URGENCY_CRITICAL)));
        assert_eq!(timeout, -1);
    }

    #[test]
    fn parses_collection_paths() {
        let value = OwnedValue::try_from(Value::from(vec![
            ObjectPath::try_from(A).unwrap(),
            ObjectPath::try_from(B).unwrap(),
        ]))
        .unwrap();
        assert_eq!(parse_paths(value), Some(vec![A.to_string(), B.to_string()]));
    }

    #[test]
    fn rejects_paths_of_the_wrong_type() {
        let value = OwnedValue::try_from(Value::from(vec!["not", "paths"])).unwrap();
        assert_eq!(parse_paths(value), None);
    }

    #[test]
    fn parses_locked_flag() {
        assert_eq!(parse_bool(OwnedValue::from(true)), Some(true));
        assert_eq!(parse_bool(OwnedValue::from(false)), Some(false));
        assert_eq!(parse_bool(OwnedValue::from(7u32)), None);
    }
}
```

`src/auth/mod.rs` の `pub mod password;` の行の上に 1 行足す:

```rust
pub mod keepassxc;
```

- [ ] **Step 3: テストが失敗することを確認する**

Run: `cargo test auth::keepassxc`
Expected: コンパイルエラー（`cannot find type Collection`、`cannot find function wait_until_locked` など）。

- [ ] **Step 4: 実装を書く**

`src/auth/keepassxc.rs` の `#![forbid(unsafe_code)]` の行とテストモジュールの間に、次を入れる（前後に空行 1 行）。
先頭の `#![allow(dead_code)]` は、Task 5 で D-Bus の呼び出しから使うまでの間だけ警告を止めるためのもので、Task 5 の Step 1 で外す:

```rust
// Task 5 で D-Bus の呼び出しから使うまでの一時的な許可。Task 5 の Step 1 で外す。
#![allow(dead_code)]

use std::collections::HashMap;
use std::time::Duration;

use zbus::message::{Flags, Message};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const KEEPASSXC_NAME: &str = "org.keepassxc.KeePassXC.MainWindow";
const KEEPASSXC_PATH: &str = "/keepassxc";
const KEEPASSXC_IFACE: &str = "org.keepassxc.KeePassXC.MainWindow";

/// ロック後の確認の間隔。
pub const CHECK_INTERVAL: Duration = Duration::from_millis(200);
/// ロック後の確認を続ける時間。
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(2);
/// 確認の回数（最初の 1 回を除く）。`CHECK_INTERVAL` ごとに `CHECK_TIMEOUT` まで。
const CHECK_RETRIES: u32 = 10;
/// 通知の緊急度（0: low, 1: normal, 2: critical）。critical はロック解除後も消えずに残る。
const URGENCY_CRITICAL: u8 = 2;

/// Secret Service のコレクション 1 つの状態。
#[derive(Clone, Debug, PartialEq, Eq)]
struct Collection {
    path: String,
    locked: bool,
}

/// `Unlock` の返事（D-Bus の型は `aoo`）: すぐ解除できたコレクションと、プロンプトのパス。
type UnlockReply = (Vec<OwnedObjectPath>, OwnedObjectPath);

/// `Notify` の引数（D-Bus の型は `susssasa{sv}i`）。
type NotifyArgs<'a> = (
    &'a str,
    u32,
    &'a str,
    &'a str,
    &'a str,
    Vec<&'a str>,
    HashMap<&'a str, Value<'a>>,
    i32,
);

// ---- 純粋な判断 -------------------------------------------------------------

/// Secret Service を KeePassXC 自身が提供しているか（2 つの名前の持ち主が同じ接続か）。
///
/// 別のもの（gnome-keyring など）が提供しているときは、KeePassXC の DB の状態は分からないので
/// 確認も解除要求もしない。
fn keepassxc_serves_secrets(keepassxc_owner: Option<&str>, secrets_owner: Option<&str>) -> bool {
    matches!((keepassxc_owner, secrets_owner), (Some(a), Some(b)) if a == b)
}

/// ロックされていないコレクションのパス。
fn still_unlocked(collections: &[Collection]) -> Vec<String> {
    collections
        .iter()
        .filter(|c| !c.locked)
        .map(|c| c.path.clone())
        .collect()
}

/// ロック中のコレクションのパス。
fn locked_paths(collections: &[Collection]) -> Vec<String> {
    collections
        .iter()
        .filter(|c| c.locked)
        .map(|c| c.path.clone())
        .collect()
}

/// 全コレクションがロックされるまで、`CHECK_INTERVAL` ごとに `CHECK_TIMEOUT` まで確かめる。
///
/// 戻り値は、時間切れの時点でまだロックされていないコレクション（全部ロックされたら空）。
/// `read` が `None` を返したら（読めなかったら）確認をやめて空を返す（通知は出さない）。
fn wait_until_locked(
    mut read: impl FnMut() -> Option<Vec<Collection>>,
    mut sleep: impl FnMut(Duration),
) -> Vec<String> {
    let mut retries = 0;
    loop {
        let Some(collections) = read() else {
            return Vec::new();
        };
        let open = still_unlocked(&collections);
        if open.is_empty() || retries >= CHECK_RETRIES {
            return open;
        }
        sleep(CHECK_INTERVAL);
        retries += 1;
    }
}

/// `Unlock` が返したプロンプトのパス。`"/"` はプロンプト不要（もう解除できた）という意味。
fn prompt_to_open(prompt: &str) -> Option<&str> {
    if prompt == "/" || prompt.is_empty() {
        None
    } else {
        Some(prompt)
    }
}

/// コレクションのパスの最後の部分（KeePassXC では DB の名前）。
fn collection_label(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// ロックできなかった DB を知らせる通知の本文。
fn notification_text(unlocked: &[String]) -> String {
    let names: Vec<&str> = unlocked.iter().map(|p| collection_label(p)).collect();
    format!(
        "Could not lock: {}. Check for unsaved changes or open dialogs in KeePassXC.",
        names.join(", ")
    )
}

/// `Notify` に渡す引数を組み立てる。
fn notify_args(body: &str) -> NotifyArgs<'_> {
    let mut hints = HashMap::new();
    hints.insert("urgency", Value::U8(URGENCY_CRITICAL));
    (
        "kabi-lock",
        0,           // replaces_id: 新しい通知
        "keepassxc", // app_icon
        "KeePassXC",
        body,
        Vec::new(), // actions なし
        hints,
        -1, // expire_timeout: サーバーに任せる
    )
}

/// `Properties.Get` の `Collections`（`ao`）を読む。型が違えば `None`。
fn parse_paths(value: OwnedValue) -> Option<Vec<String>> {
    let paths: Vec<OwnedObjectPath> = value.try_into().ok()?;
    Some(paths.into_iter().map(|p| p.as_str().to_owned()).collect())
}

/// `Properties.Get` の `Locked`（`b`）を読む。型が違えば `None`。
fn parse_bool(value: OwnedValue) -> Option<bool> {
    bool::try_from(value).ok()
}

/// `lockAllDatabases` の呼び出し。返事は求めない（未保存の DB があると KeePassXC が確認ダイアログを
/// 出し、返事が遅れることがあるため）。
fn lock_all_message() -> zbus::Result<Message> {
    Message::method_call(KEEPASSXC_PATH, "lockAllDatabases")?
        .destination(KEEPASSXC_NAME)?
        .interface(KEEPASSXC_IFACE)?
        .with_flags(Flags::NoReplyExpected)?
        .build(&())
}
```

要点:
- `wait_until_locked` は最初に 1 回読み、ロックされていないものが残っていれば `CHECK_INTERVAL` 眠ってまた読む。これを `CHECK_RETRIES`（10）回まで繰り返すので、眠る時間の合計は最大で `CHECK_TIMEOUT`（2 秒）。今の `swaylock-touch` の `lock_keepassxc`（0.2 秒 × 10 回）と同じ。
- 読めなかったら（`read` が `None`）確認をやめ、通知は出さない。原因は Task 5 の読み出し側がログに出す。
- `lock_all_message` はメッセージを作るだけで送らない。テストでは宛先とフラグだけを確かめる。

- [ ] **Step 5: テストが通ることを確認する**

Run: `cargo test auth::keepassxc`
Expected: `test result: ok. 18 passed`

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: 警告・差分なし（Task 5 で使う関数と定数の `dead_code` は、一時的な `#![allow(dead_code)]` で止めてある）。fmt の差分が出たら `cargo fmt` を実行する。

- [ ] **Step 6: コミットする（ファイルごとに分ける）**

```bash
git status
git diff
git add Cargo.toml Cargo.lock
git commit -m "chore: add zbus dependency

zbus 5 still provides the blocking API used for KeePassXC and Secret
Service calls.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git add src/auth/keepassxc.rs
git commit -m "feat: add keepassxc lock checks and d-bus message helpers

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git add src/auth/mod.rs
git commit -m "feat: register keepassxc auth module

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: KeePassXC と Secret Service の D-Bus 呼び出し

**Files:**
- Modify: `src/auth/keepassxc.rs`

**Interfaces:**
- Consumes: Task 4 の純粋な関数、`zbus::blocking::{Connection, connection::Builder}`
- Produces（契約どおり）:
  - `pub fn lock_before_in_background()`
  - `pub fn unlock_after()`

- [ ] **Step 1: 一時的な `allow` を外し、`use` をまとめて置き換える**

`src/auth/keepassxc.rs` の、Task 4 で入れた一時的な許可と次の `use` の並び:

```rust
// Task 5 で D-Bus の呼び出しから使うまでの一時的な許可。Task 5 の Step 1 で外す。
#![allow(dead_code)]

use std::collections::HashMap;
use std::time::Duration;

use zbus::message::{Flags, Message};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
```

を、次に置き換える:

```rust
use std::collections::HashMap;
use std::thread;
use std::time::Duration;

use log::{debug, info, warn};
use zbus::blocking::Connection;
use zbus::message::{Flags, Message};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};
```

- [ ] **Step 2: D-Bus の宛先の定数を足す**

`const KEEPASSXC_IFACE: &str = "org.keepassxc.KeePassXC.MainWindow";` の行の下（空行 1 行あけて）に、次を足す（後ろにも空行 1 行）:

```rust
const DBUS_NAME: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";
const PROPERTIES_IFACE: &str = "org.freedesktop.DBus.Properties";

const SECRETS_NAME: &str = "org.freedesktop.secrets";
const SECRETS_PATH: &str = "/org/freedesktop/secrets";
const SERVICE_IFACE: &str = "org.freedesktop.Secret.Service";
const COLLECTION_IFACE: &str = "org.freedesktop.Secret.Collection";
const PROMPT_IFACE: &str = "org.freedesktop.Secret.Prompt";

const NOTIFY_NAME: &str = "org.freedesktop.Notifications";
const NOTIFY_PATH: &str = "/org/freedesktop/Notifications";
const NOTIFY_IFACE: &str = "org.freedesktop.Notifications";

/// D-Bus の 1 回の呼び出しを待つ上限。KeePassXC が固まっていても終了処理が止まらないようにする。
const CALL_TIMEOUT: Duration = Duration::from_secs(3);
```

- [ ] **Step 3: D-Bus の呼び出しと公開する入口を書く**

テストモジュール（`#[cfg(test)]` の行）の直前に、次を足す（`lock_all_message` との間と、テストモジュールとの間に空行 1 行）:

```rust
// ---- D-Bus の呼び出し -------------------------------------------------------

fn connect() -> zbus::Result<Connection> {
    zbus::blocking::connection::Builder::session()?
        .method_timeout(CALL_TIMEOUT)
        .build()
}

/// 名前の持ち主（`:1.23` のような一意名）。持ち主がいなければ `None`。自動起動はしない。
fn name_owner(conn: &Connection, name: &str) -> Option<String> {
    let reply = conn
        .call_method(
            Some(DBUS_NAME),
            DBUS_PATH,
            Some(DBUS_NAME),
            "GetNameOwner",
            &(name,),
        )
        .ok()?;
    reply.body().deserialize::<String>().ok()
}

/// KeePassXC の D-Bus 名の持ち主と、Secret Service も KeePassXC が提供しているかどうか。
struct Owners {
    keepassxc: Option<String>,
    serves_secrets: bool,
}

fn owners(conn: &Connection) -> Owners {
    let keepassxc = name_owner(conn, KEEPASSXC_NAME);
    let secrets = name_owner(conn, SECRETS_NAME);
    let serves_secrets = keepassxc_serves_secrets(keepassxc.as_deref(), secrets.as_deref());
    Owners {
        keepassxc,
        serves_secrets,
    }
}

fn get_secret_property(
    conn: &Connection,
    path: &str,
    iface: &str,
    name: &str,
) -> zbus::Result<OwnedValue> {
    let reply = conn.call_method(
        Some(SECRETS_NAME),
        path,
        Some(PROPERTIES_IFACE),
        "Get",
        &(iface, name),
    )?;
    reply.body().deserialize::<OwnedValue>()
}

/// Secret Service の全コレクションと、それぞれの `Locked`。読めないコレクションは飛ばす。
fn collections(conn: &Connection) -> zbus::Result<Vec<Collection>> {
    let value = get_secret_property(conn, SECRETS_PATH, SERVICE_IFACE, "Collections")?;
    let paths = parse_paths(value)
        .ok_or_else(|| zbus::Error::Failure("Collections is not an object path array".into()))?;
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        match get_secret_property(conn, &path, COLLECTION_IFACE, "Locked").map(parse_bool) {
            Ok(Some(locked)) => out.push(Collection { path, locked }),
            Ok(None) => debug!("keepassxc: {path}: Locked is not a boolean; skipped"),
            Err(e) => debug!("keepassxc: {path}: could not read Locked ({e}); skipped"),
        }
    }
    Ok(out)
}

fn notify(conn: &Connection, body: &str) -> zbus::Result<()> {
    conn.call_method(
        Some(NOTIFY_NAME),
        NOTIFY_PATH,
        Some(NOTIFY_IFACE),
        "Notify",
        &notify_args(body),
    )?;
    Ok(())
}

// ---- 公開する入口 -----------------------------------------------------------

/// lockAllDatabases を呼び、別スレッドで最大 2 秒 Secret Service の Locked を確かめる。
/// ロックされないものが残れば通知を出す。呼び出し側は待たない。
pub fn lock_before_in_background() {
    let started = thread::Builder::new()
        .name("kabi-keepassxc".into())
        .spawn(|| {
            if let Err(e) = lock_and_watch() {
                warn!("keepassxc: lock request failed: {e}");
            }
        });
    if let Err(e) = started {
        warn!("keepassxc: could not start the lock thread: {e}");
    }
}

fn lock_and_watch() -> zbus::Result<()> {
    let conn = connect()?;
    let owners = owners(&conn);
    if owners.keepassxc.is_none() {
        info!("keepassxc: not running; nothing to lock");
        return Ok(());
    }
    conn.send(&lock_all_message()?)?;
    info!("keepassxc: asked to lock all databases");
    if !owners.serves_secrets {
        info!("keepassxc: Secret Service is not provided by KeePassXC; cannot verify the lock");
        return Ok(());
    }
    let open = wait_until_locked(
        || match collections(&conn) {
            Ok(c) => Some(c),
            Err(e) => {
                warn!("keepassxc: could not read collections: {e}");
                None
            }
        },
        thread::sleep,
    );
    if open.is_empty() {
        info!("keepassxc: all exposed databases are locked");
        return Ok(());
    }
    warn!(
        "keepassxc: {} collection(s) still unlocked after {CHECK_TIMEOUT:?}",
        open.len()
    );
    notify(&conn, &notification_text(&open))?;
    info!("keepassxc: sent a desktop notification");
    Ok(())
}

/// ロック中のコレクションがあれば Unlock → Prompt を呼ぶ。終了処理の中で呼ぶ（待ってよい）。
pub fn unlock_after() {
    if let Err(e) = request_unlock() {
        warn!("keepassxc: unlock request failed: {e}");
    }
}

fn request_unlock() -> zbus::Result<()> {
    let conn = connect()?;
    if !owners(&conn).serves_secrets {
        info!("keepassxc: not serving Secret Service; nothing to unlock");
        return Ok(());
    }
    let locked = locked_paths(&collections(&conn)?);
    if locked.is_empty() {
        info!("keepassxc: no locked collections; nothing to unlock");
        return Ok(());
    }
    let targets = locked
        .iter()
        .map(|p| ObjectPath::try_from(p.as_str()))
        .collect::<Result<Vec<_>, _>>()?;
    let reply = conn.call_method(
        Some(SECRETS_NAME),
        SECRETS_PATH,
        Some(SERVICE_IFACE),
        "Unlock",
        &(targets,),
    )?;
    let (unlocked, prompt): UnlockReply = reply.body().deserialize()?;
    debug!(
        "keepassxc: Unlock returned {} collection(s) already unlocked",
        unlocked.len()
    );
    match prompt_to_open(prompt.as_str()) {
        None => info!("keepassxc: collections unlocked without a prompt"),
        Some(path) => {
            conn.call_method(
                Some(SECRETS_NAME),
                path,
                Some(PROMPT_IFACE),
                "Prompt",
                &("",),
            )?;
            info!("keepassxc: opened the unlock prompt");
        }
    }
    Ok(())
}
```

要点:
- `lock_before_in_background` はスレッドを起動してすぐ戻る。接続・`GetNameOwner`・`lockAllDatabases` の送信・2 秒の確認・通知は、すべてそのスレッドで行う。kabi-lock が 2 秒以内に解除されて終わったときは、確認の途中でもプロセスごと終わる（それでよい）。
- `unlock_after` は呼び出し元のスレッドで動き、1 回の呼び出しは最大 3 秒（`CALL_TIMEOUT`）で打ち切る。
- 失敗はすべて `warn!` でログに出すだけで、呼び出し元には返さない（ロックの進行も終了も止めない）。

- [ ] **Step 4: ビルドとテスト、lint を通す**

Run: `cargo test auth::keepassxc`
Expected: `test result: ok. 18 passed`

Run: `cargo clippy --all-targets -- -D warnings`
Expected: 警告なし（Step 1 で Task 4 の一時的な `#![allow(dead_code)]` を外したので、Task 4 の関数と定数がすべて使われていることもここで確かめられる）。

Run: `cargo fmt --check`
Expected: 差分なし。差分が出たら `cargo fmt` を実行する。

- [ ] **Step 5: 読み取りだけの D-Bus で、型の読み方を確かめる（KeePassXC が動いているとき）**

ここでは lockAllDatabases・Unlock・Notify は呼ばない。`busctl` で、コードが読むのと同じプロパティを見て、`parse_paths`（`ao`）と `parse_bool`（`b`）が前提にしている型であることを確かめる。

```bash
busctl --user call org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus GetNameOwner s org.keepassxc.KeePassXC.MainWindow
busctl --user call org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus GetNameOwner s org.freedesktop.secrets
busctl --user get-property org.freedesktop.secrets /org/freedesktop/secrets org.freedesktop.Secret.Service Collections
COL=$(busctl --user get-property org.freedesktop.secrets /org/freedesktop/secrets org.freedesktop.Secret.Service Collections | grep -o '"/org/freedesktop/secrets/collection/[^"]*"' | head -n 1 | tr -d '"')
echo "COL=$COL"
busctl --user get-property org.freedesktop.secrets "$COL" org.freedesktop.Secret.Collection Locked
busctl --user introspect org.freedesktop.Notifications /org/freedesktop/Notifications org.freedesktop.Notifications | grep Notify
```

Expected:
- 2 つの `GetNameOwner` が同じ一意名（例: `s ":1.1510"`）を返す（KeePassXC の Secret Service 連携が有効な場合）。違えば、`lock_before_in_background` は DB のロックだけ頼んで確認はしない（ログ `Secret Service is not provided by KeePassXC`）。
- `Collections` が `ao 1 "/org/freedesktop/secrets/collection/Passwords"` のような `ao`。`COL=` の行に、その最初のパス（例: `COL=/org/freedesktop/secrets/collection/Passwords`）が出る。
- `Locked` が `b true` か `b false`（`COL=` が空なら、Secret Service に公開している DB がない。KeePassXC の設定で DB を公開してからやり直す）。
- `Notify` の行の型が `susssasa{sv}i`（`notify_args_match_the_dbus_signature` のテストと同じ）。

（この計画を書くときに、同じ読み取りを zbus から行い、`Collection { path: "/org/freedesktop/secrets/collection/Passwords", locked: false }` が得られることを確かめてある。）

- [ ] **Step 6: コミットする**

```bash
git status
git diff
git add src/auth/keepassxc.rs
git commit -m "feat: lock and unlock keepassxc over d-bus

Sends lockAllDatabases without waiting for a reply, then checks Secret
Service collections for up to 2 seconds in a background thread and posts
a critical notification if some stay unlocked. After unlocking, asks
Secret Service to unlock locked collections and opens the prompt.
Nothing is called when KeePassXC is not running, so D-Bus activation
never starts it.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: `app.rs` への結線（ロック前の KeePassXC、ロック後の U2F、解除時の停止）

**Files:**
- Modify: `src/app.rs`

**Interfaces:**
- Consumes: `crate::auth::u2f::{spawn, U2fHandle}`、`crate::auth::keepassxc::lock_before_in_background`、`run` の中の `auth_tx` と `app.config`
- Produces（`app.rs` の中だけ）:
  - `App.u2f: Option<U2fHandle>`（あわせて、どこからも読まれない `App.user` を外す）
  - `fn start_u2f(config: &Config, events: channel::Sender<AuthEvent>) -> Option<U2fHandle>`（`auth.u2f.enabled` が `false` なら `None`）

- [ ] **Step 1: 失敗するテストを書く**

`src/app.rs` のテストモジュールの最後の次の部分:

```rust
            "event loop failed"
        );
    }
}
```

を、次に置き換える（`errors_explain_themselves` の後に 2 つ足す）:

```rust
            "event loop failed"
        );
    }

    #[test]
    fn u2f_is_not_started_when_disabled() {
        let mut config = Config::default();
        config.auth.u2f.enabled = false;
        let (tx, _rx) = channel::channel::<AuthEvent>();
        assert!(start_u2f(&config, tx).is_none());
    }

    #[test]
    fn u2f_is_not_started_without_a_pam_service_file() {
        let mut config = Config::default();
        config.auth.u2f.pam_service = "kabi-lock-test-no-such-service".into();
        let (tx, _rx) = channel::channel::<AuthEvent>();
        assert!(start_u2f(&config, tx).is_none());
    }
}
```

2 つ目のテストは、存在しない PAM サービス名を使うので helper は起動しない（PAM は呼ばれない）。初期値の `kabi-lock-u2f` を使うと、そのファイルがある環境では本物のタッチ待ちが始まってしまうので使わない。

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test app::tests`
Expected: コンパイルエラー（`cannot find function start_u2f`）。

- [ ] **Step 3: `start_u2f` と `App.u2f` を足す**

(a) `use` を置き換える。次の行:

```rust
use crate::auth::{self, AuthEvent, password::PasswordChecker};
```

を、次にする:

```rust
use crate::auth::{
    self, AuthEvent, keepassxc,
    password::PasswordChecker,
    u2f::{self, U2fHandle},
};
```

(b) `App` に U2F の見張りを足す。次の行:

```rust
    pub(crate) checker: PasswordChecker,
```

を、次にする:

```rust
    pub(crate) checker: PasswordChecker,
    /// YubiKey のタッチ待ち（Plan 5）。`release_lock` で止める。`None` ならタッチでは解除しない。
    pub(crate) u2f: Option<U2fHandle>,
```

(c) `start_u2f` を `run` の前（`prepare_background` の後）に足す。次の行:

```rust
/// ロックして、解除されるまでイベントループを回す。
```

を、次にする:

```rust
/// 設定で有効なら、YubiKey のタッチ待ちを始める（Plan 5）。
///
/// PAM サービスの設定ファイルがなければ始めない（`u2f::spawn` が `None` を返す）。
fn start_u2f(config: &Config, events: channel::Sender<AuthEvent>) -> Option<U2fHandle> {
    if !config.auth.u2f.enabled {
        log::info!("u2f: disabled in config");
        return None;
    }
    u2f::spawn(config.auth.u2f.pam_service.clone(), events)
}

/// ロックして、解除されるまでイベントループを回す。
```

- [ ] **Step 4: `run` と `release_lock` から呼ぶ**

(d) `run` で、`PasswordChecker::spawn` に `auth_tx` が move される前に複製を取る。次の行:

```rust
    let checker = PasswordChecker::spawn(config.auth.pam_service.clone(), user.clone(), auth_tx);
```

を、次にする:

```rust
    // YubiKey のタッチ待ち（Plan 5）にも同じチャンネルを使う。auth_tx は次の行で move される
    let u2f_tx = auth_tx.clone();
    let checker = PasswordChecker::spawn(config.auth.pam_service.clone(), user.clone(), auth_tx);
```

(e) `App` の初期化に足す。次の部分:

```rust
        checker,
        clock,
```

を、次にする:

```rust
        checker,
        u2f: None,
        clock,
```

(f) ロック要求の直前で KeePassXC のロックを頼む。次の部分:

```rust
    let lock = app
        .lock_state
        .lock(&qh)
```

を、次にする:

```rust
    // KeePassXC の全 DB のロックを頼む（Plan 5。待たない。確認は裏のスレッドで 2 秒まで）
    if app.config.keepassxc.lock_before {
        keepassxc::lock_before_in_background();
    }
    let lock = app
        .lock_state
        .lock(&qh)
```

(g) lock surface を作った後、イベントループに入る前に U2F を始める。次の部分:

```rust
        app.add_output(output);
    }
```

を、次にする:

```rust
        app.add_output(output);
    }
    // YubiKey のタッチ待ちを始める（Plan 5。成功したら AuthEvent::U2fSucceeded が届く）
    app.u2f = start_u2f(&app.config, u2f_tx);
```

(h) `release_lock` で、ロックを外した直後に helper を止める。次の部分:

```rust
        self.lock = None;
        self.release_pending = false;
```

を、次にする:

```rust
        // タッチ待ちの helper を止める（Plan 5）。解除後に main が KeePassXC の解除ダイアログを
        // 出す前に止めて、そのダイアログ用の YubiKey のタッチを横取りしないようにする
        if let Some(mut u2f) = self.u2f.take() {
            u2f.stop();
        }
        self.lock = None;
        self.release_pending = false;
```

`handle_auth` は変えない（Plan 2 がすでに `AuthEvent::U2fSucceeded` を `security key accepted` とログに出し、`Event::from` で `Event::U2fSucceeded` にして状態機械へ渡している）。`Machine` は入力待ち・確認中・失敗のどこからでも U2F の成功で解除中へ進み、解除中に届いたパスワードの結果を捨てる（Plan 1 の `u2f_unlocks_from_idle_failed_and_verifying` と `late_password_result_is_ignored_while_unlocking`）。

U2F で解除したときは、見張りスレッドは成功を送った時点で終わっているので、(h) の `stop` はすぐ戻る。パスワードで解除したときは、タッチを待っている helper が SIGTERM で終わる。`run` がエラーで抜けるとき（`finished`、Wayland の切断など）は、`App` が捨てられるときに `U2fHandle` の `Drop` が同じことをする。

- [ ] **Step 5: 使われない `App.user` を外す**

Plan 2 は `App` に `user` を「Plan 5 の U2F でも使う」として `#[allow(dead_code)]` 付きで置いたが、U2F の helper はユーザー名を自分で `auth::current_user()` から取る（Task 1）ので、親の `App.user` はどこからも読まれない。フィールドを外す。

(i) `App` の次の 3 行を消す:

```rust
    /// PAM に渡したユーザー名。Plan 5 の U2F でも使う。
    #[allow(dead_code)]
    pub(crate) user: String,
```

(j) `run` の `App` の初期化から、次の 1 行を消す（`config,` の次の行）:

```rust
        user,
```

(k) `run` の、(d) で変えた後の次の行:

```rust
    let checker = PasswordChecker::spawn(config.auth.pam_service.clone(), user.clone(), auth_tx);
```

を、次にする（`user` はもう `App` に入れないので、複製せずに渡す）:

```rust
    let checker = PasswordChecker::spawn(config.auth.pam_service.clone(), user, auth_tx);
```

`let user = auth::current_user()...` の行は残す（パスワード確認と、ユーザー名が分からないときの `AppError::Other` に使う）。

- [ ] **Step 6: テストが通ることを確認する**

Run: `cargo test app::tests`
Expected: `test result: ok. 3 passed`

Run: `cargo build && cargo test`
Expected: エラーなし。すべてのテストが通る（Plan 1〜4 のテストに加え、`auth::u2f` 19 件、`auth::keepassxc` 18 件、`app::tests` 2 件、`main.rs` 2 件）。

Run: `cargo clippy --all-targets -- -D warnings`
Expected: 警告なし。

Run: `cargo fmt --check`
Expected: 差分なし。差分が出たら `cargo fmt` を実行する。

- [ ] **Step 7: コミットする**

```bash
git status
git diff
git add src/app.rs
git commit -m "feat: start yubikey and keepassxc integration from the locker

Asks KeePassXC to lock right before the session lock request, starts the
pam_u2f helper supervisor on the password channel once the lock surfaces
exist, and stops it as soon as the lock is released. Drops the unused
App.user field: the helper looks up the user name itself.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: 解除後の KeePassXC 解除要求（`main.rs`）

**Files:**
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: `kabi_lock::app::run`、`kabi_lock::auth::keepassxc::unlock_after`
- Produces: なし（`Command::Run` の腕の中だけを変える）

- [ ] **Step 1: `Command::Run` の腕を変える**

`src/main.rs` の `Command::Run` の腕の中の次の部分:

```rust
            let config = kabi_lock::config::load();
            let result = app::run(config);
            if let Err(e) = &result {
                eprintln!("kabi-lock: {e}");
            }
```

を、次にする:

```rust
            let config = kabi_lock::config::load();
            // 解除後に KeePassXC の解除を頼むか（Plan 5）。config は run に move されるので先に読む
            let unlock_keepassxc = config.keepassxc.unlock_after;
            let result = app::run(config);
            match &result {
                Err(e) => eprintln!("kabi-lock: {e}"),
                // ここではロックはもう外れていて、U2F の helper も止まっている
                Ok(()) if unlock_keepassxc => kabi_lock::auth::keepassxc::unlock_after(),
                Ok(()) => log::info!("keepassxc: unlock_after is disabled in config"),
            }
```

`app::run` が `Ok` を返した時点で、ロックは外れていて（`release_lock` が `unlock` の後に `roundtrip` している）、U2F の helper も止まっている（`release_lock` と `App` の `Drop`）。

- [ ] **Step 2: ビルドとテスト、lint を通す**

Run: `cargo build && cargo test`
Expected: エラーなし。すべてのテストが通る。

Run: `cargo clippy --all-targets -- -D warnings`
Expected: 警告なし。

Run: `cargo fmt --check`
Expected: 差分なし。差分が出たら `cargo fmt` を実行する。

- [ ] **Step 3: コミットする**

```bash
git status
git diff
git add src/main.rs
git commit -m "feat: request keepassxc unlock after the lock is released

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: 入れ子の sway での手動確認

**Files:** なし（コードは変えない。問題が見つかったら、どの Task のどのファイルかを特定して直し、そのファイルだけのコミットにする）

本物のセッションはロックしない。入れ子の sway の中の kabi-lock は、**本物のセッションの D-Bus**（KeePassXC・通知）と**本物の YubiKey**を使う。KeePassXC の DB は実際にロックされる（いつもの方法で解除すればよい）。間違ったパスワードは送らない。パスワードでの解除は、自分のパスワードで 1 回だけ試す。

helper は `ps` では `/proc/self/exe --u2f-helper <サービス>` と見える。各 Step の「helper が残っていない」は `pgrep -fa u2f-helper` が何も出さない（`pgrep` 自身の行を除く）ことで確かめる。

- [ ] **Step 1: 準備**

パスワードでの解除（Step 4〜6、8）には、`auth` と `account` の行がある PAM サービスが要る（Plan 2 の `pam_check` は `acct_mgmt` も呼ぶ）。`/etc/pam.d/kabi-lock` はまだない（Plan 6 で入れる）ので、Plan 2 Task 12 Step 8 と同じく、一時的な設定で `pam_service = "login"` を使う。先に `/etc/pam.d/login` を確かめる:

```bash
grep -qE '^auth[[:space:]]' /etc/pam.d/login && grep -qE '^account[[:space:]]' /etc/pam.d/login && echo PAM-OK || echo "PAM file missing auth or account line"
```

Expected: `PAM-OK`。`PAM file missing auth or account line` が出たら、ここで止めて BLOCKED として人に報告する（エージェントは `/etc/pam.d` を変えない）。

U2F 用の PAM サービスには、今ある `/etc/pam.d/u2f-touch`（`auth required pam_u2f.so authfile=/etc/u2f_mappings` と `account required pam_permit.so`）を使う。中身が pam_u2f だけであることを確かめる:

```bash
cat /etc/pam.d/u2f-touch
```

Expected: `pam_u2f.so` の `auth` 行と、`pam_permit.so` の `account` 行だけ（`pam_unix`・`pam_faillock`・`include` がない）。ほかの行がある場合は先に進まない（BLOCKED として人に報告する）。

`/etc/pam.d/u2f-touch` がない（`No such file or directory`）場合は、Plan 6 の `contrib/pam.d/kabi-lock-u2f` と同じ中身のファイルを、**人が** `sudo` で置く（エージェントは `sudo` を実行しない。BLOCKED として人に頼む）。鍵の登録先を確かめてから、どちらか 1 つを実行する:

```bash
ls -l /etc/u2f_mappings ~/.config/Yubico/u2f_keys
# /etc/u2f_mappings に登録してある場合:
printf 'auth       required     pam_u2f.so    authfile=/etc/u2f_mappings\naccount    required     pam_permit.so\n' | sudo tee /etc/pam.d/u2f-touch
# ~/.config/Yubico/u2f_keys に登録してある場合（pam_u2f の既定の場所なので authfile を書かない）:
printf 'auth       required     pam_u2f.so\naccount    required     pam_permit.so\n' | sudo tee /etc/pam.d/u2f-touch
```

どちらのファイルもなければ YubiKey が登録されていないので、YubiKey を使う Step（2、3、5、7〜11）は行わず、その旨を Step 12 で報告する。ここで置いた `/etc/pam.d/u2f-touch` は Task 8 の後に人が `sudo rm /etc/pam.d/u2f-touch` で消す（Plan 6 の手動確認では `kabi-lock-u2f` を使う）。

本物の設定ファイルを変えないよう、確認用の設定を別の場所に置く:

```bash
cargo build
mkdir -p /tmp/kabi-test-config/kabi-lock
cat > /tmp/kabi-test-config/kabi-lock/config.toml <<'EOF'
[auth]
pam_service = "login"

[auth.u2f]
enabled = true
pam_service = "u2f-touch"

[keepassxc]
lock_before = false
unlock_after = false
EOF
cat > /tmp/kabi-nested.conf <<EOF
exec sh -c 'XDG_CONFIG_HOME=/tmp/kabi-test-config RUST_LOG=kabi_lock=debug $PWD/target/debug/kabi-lock > /tmp/kabi-nested.log 2>&1; swaymsg exit'
EOF
```

（`RUST_LOG=kabi_lock=debug` で kabi-lock のログだけを出す。helper のログも同じファイルに出る。各ケースの前に `config.toml` を書き換え、`WLR_BACKENDS=wayland sway -c /tmp/kabi-nested.conf` で入れ子の sway を起動する。kabi-lock が終わると入れ子の sway も閉じる。）

- [ ] **Step 2: YubiKey のタッチで解除できる**

上の設定のまま起動する。ロック画面が出たら、ホスト側の端末で `pgrep -fa u2f-helper` を実行してから、YubiKey に触れる。

Expected:
- `pgrep` に `/proc/self/exe --u2f-helper u2f-touch` が 1 つ出る。
- 触れると、錠が開くアニメーション → ぼかしが晴れる → 入れ子の sway が閉じる。
- `/tmp/kabi-nested.log` に次の順で出る:
  - `u2f: waiting for a touch (pam service /etc/pam.d/u2f-touch)`
  - （pam_u2f が `cue` 付きなら）`u2f helper: pam info: ...`（debug）
  - `u2f helper: Accepted`（debug）
  - `u2f: touch accepted`、`security key accepted`
  - `u2f: stopped`（debug）、`unlocked after ... ms`
  - `keepassxc: unlock_after is disabled in config`
- 閉じた後、`pgrep -fa u2f-helper` が何も出さない。

- [ ] **Step 3: タッチ待ちの間もイベントループが止まらない**

もう一度起動し、YubiKey に触れずに数文字打つ（Enter は押さない）。

Expected: 打つたびに光の柱が出る（Plan 4）。Esc で全消去できる。その後 YubiKey に触れると Step 2 と同じく解除される。

- [ ] **Step 4: パスワードで解除すると、タッチ待ちの helper が止まる**

もう一度起動し、YubiKey には触れずに、自分のパスワードを 1 回だけ正しく入れて Enter。

Expected:
- 解除されて入れ子の sway が閉じる（`u2f: touch accepted` は出ない）。ログに `u2f: stopped`（debug）。
- `pgrep -fa u2f-helper` と `pgrep -x kabi-lock` が何も出さない。
- YubiKey の点滅（タッチ待ち）が止まる。その後触れても何も起きない。

- [ ] **Step 5: YubiKey を抜いた状態でも空回りしない**

YubiKey を抜いて起動し、5 秒ほど待ってから、自分のパスワードを 1 回だけ正しく入れて解除する。

Expected:
- `u2f: no touch accepted; retrying every 1s` が **1 回だけ** 出て、以後は `u2f: attempt failed again: Rejected`（debug）が約 1 秒ごとに出る。
- `top -p $(pgrep -x kabi-lock)` などで見ても、kabi-lock が CPU を使い続けていない（helper は 1 秒ごとに短く起動するだけ）。
- 解除後、`pgrep -fa u2f-helper` が何も出さない。

- [ ] **Step 6: U2F をオフにする／サービスがないとき**

`config.toml` の `[auth.u2f]` を `enabled = false` にして起動し、パスワードで 1 回解除する。

Expected: `u2f: disabled in config` が出て、`u2f: waiting for a touch` は出ない。ロック中に `pgrep -fa u2f-helper` が何も出さない。

`enabled = true`、`pam_service = "kabi-lock-u2f-missing"` にして起動し、パスワードで 1 回解除する。

Expected: `u2f: pam service "kabi-lock-u2f-missing" not found in ["/etc/pam.d", "/usr/lib/pam.d"]; YubiKey unlock is off` が出る。helper は起動せず、PAM も呼ばれない。

- [ ] **Step 7: kabi-lock が異常終了しても helper が残らない（任意）**

Step 1 の設定で起動し、ロック画面が出たらホスト側で `pgrep -fa u2f-helper` で helper があることを確かめてから、`pkill -KILL -x kabi-lock` で kabi-lock だけを強制終了する。

Expected:
- 直後に `pgrep -fa u2f-helper` が何も出さない（helper の `PR_SET_PDEATHSIG` で SIGTERM が届いて終わる）。
- 入れ子の sway はロックしたまま（赤い画面）になる。これは設計書 8 章どおりなので、入れ子の sway のウィンドウを閉じるか、`swaymsg -s <入れ子の SWAYSOCK> exit` で終える。

- [ ] **Step 8: KeePassXC の DB が開いているとき（ロック → 解除要求）**

KeePassXC を起動し、Secret Service 連携を有効にした DB を開いておく。`config.toml` を次にする:

```toml
[auth]
pam_service = "login"

[auth.u2f]
enabled = true
pam_service = "u2f-touch"

[keepassxc]
lock_before = true
unlock_after = true
```

起動する前に、ホスト側の端末で、公開されているコレクションのパスを変数に取っておく（Task 5 の Step 5 と同じ）:

```bash
COL=$(busctl --user get-property org.freedesktop.secrets /org/freedesktop/secrets org.freedesktop.Secret.Service Collections | grep -o '"/org/freedesktop/secrets/collection/[^"]*"' | head -n 1 | tr -d '"')
echo "COL=$COL"
```

Expected: `COL=/org/freedesktop/secrets/collection/...`（空なら DB が Secret Service に公開されていない。KeePassXC の設定で公開してからやり直す）。

起動して、ロック画面が出てから 3 秒以内に、同じ端末で状態を見る:

```bash
busctl --user get-property org.freedesktop.secrets "$COL" org.freedesktop.Secret.Collection Locked
```

Expected: `b true`。ログに次が出る:
- `keepassxc: asked to lock all databases`
- `keepassxc: all exposed databases are locked`

自分のパスワードを 1 回だけ正しく入れて解除する（YubiKey には触れない）。

Expected:
- 入れ子の sway が閉じた直後、ホスト側に KeePassXC の解除ダイアログが出る。ログに `u2f: stopped` の後に `keepassxc: opened the unlock prompt`。
- ダイアログ側の YubiKey のタッチ（クイックアンロック → polkit → pam_u2f）が、**最初のタッチで**効く（kabi-lock の helper が止まっているので、タッチを横取りしない）。

YubiKey のタッチで解除した場合も同じく、解除後にダイアログが出て、続けてのタッチが KeePassXC 側に効くことを確かめる。

- [ ] **Step 9: KeePassXC の DB が最初からロックされているとき**

KeePassXC の DB をロックしておき、Step 8 と同じ設定で起動して、YubiKey で解除する。

Expected: `keepassxc: asked to lock all databases`、`keepassxc: all exposed databases are locked`、解除後に `keepassxc: opened the unlock prompt` とダイアログ。

`[keepassxc]` を `unlock_after = false` にしてもう一度行う。

Expected: 解除後にダイアログが出ない。ログに `keepassxc: unlock_after is disabled in config`。

- [ ] **Step 10: KeePassXC が起動していないとき（任意）**

KeePassXC を終了し、Step 8 と同じ設定で起動して、YubiKey で解除する。

Expected:
- `keepassxc: not running; nothing to lock`、解除後に `keepassxc: not serving Secret Service; nothing to unlock`。
- `pgrep -x keepassxc` が何も返さない（D-Bus の自動起動で KeePassXC が立ち上がっていない）。

- [ ] **Step 11: ロックできない DB があるときの通知（任意・使い捨ての DB で）**

本物の DB は使わない。KeePassXC で使い捨ての DB（例: `/tmp/kabi-test.kdbx`）を作って Secret Service に公開し、KeePassXC の設定で「変更後すぐに自動保存」と「ロック時に保存」を切った状態で、その DB のエントリーを 1 つ追加して保存しないでおく。Step 8 と同じ設定で起動する。

Expected:
- KeePassXC が保存の確認ダイアログを出し、DB はロックされない。
- 約 2 秒後にログに `keepassxc: 1 collection(s) still unlocked after 2s` と `keepassxc: sent a desktop notification`。
- ホスト側に「KeePassXC / Could not lock: <DB 名>. Check for unsaved changes or open dialogs in KeePassXC.」の通知（critical）が出る。
- ロック自体は普通に進み、YubiKey で解除できる。

確認が済んだら KeePassXC のダイアログで変更を破棄し、使い捨ての DB を閉じて消す。

- [ ] **Step 12: 結果を記録する**

各 Step の結果（ログの該当行と `pgrep` の結果）を、実行報告に書く。

---

## 契約への追記（Plan 5 の公開 API の変更）

契約 4 章「Plan 5 が作るもの」の `u2f` を、次に置き換える（`keepassxc` は変えない）。

```rust
// src/auth/u2f.rs
/// helper として起動するときの隠し引数。`kabi-lock --u2f-helper <PAM サービス>`。--help には出さない。
pub const HELPER_ARG: &str = "--u2f-helper";
/// 失敗してから helper を起動し直すまで。
pub const RETRY_DELAY: std::time::Duration; // 1 秒
/// helper の本体（main.rs から呼ぶ）。pam_authenticate を 1 回だけ行い、終了コードを返す。
/// 0 = 成功、1 = 認証できなかった、3 = PAM が入力を求めた（断った）、4 = ユーザー名・pam_start の失敗。
pub fn helper_main(service: &str) -> u8;
/// タッチ待ちの見張り。stop か drop で、待っている helper を止める（SIGTERM → 0.5 秒後 SIGKILL）。
pub struct U2fHandle { /* 見張りスレッドと、起動中の helper */ }
impl U2fHandle {
    pub fn stop(&mut self); // 何度呼んでもよい
}
impl Drop for U2fHandle { /* stop を呼ぶ */ }
/// pam_u2f の PAM サービスで、helper（子プロセス）を成功するまで起動し直す見張りスレッドを起動する。
/// 成功したら AuthEvent::U2fSucceeded を 1 回送って終わる。失敗したら 1 秒あけて再試行。
/// PAM サービスの設定ファイル（/etc/pam.d、/usr/lib/pam.d）がない、またはスレッドを起動できなければ None。
pub fn spawn(service: String, events: calloop::channel::Sender<AuthEvent>) -> Option<U2fHandle>;
```

ほかの計画に関わる変更:

- `user` 引数をなくした（helper が `auth::current_user()` で取る。親と同じ関数なので同じ名前になる）。
- `src/main.rs`: 隠しモード `--u2f-helper <サービス>` を受け付ける（`Command::U2fHelper`）。Plan 6 の README・PKGBUILD で、この引数を利用者向けに書かない。
- `src/app.rs`: `App` に `u2f: Option<U2fHandle>` を足した。`release_lock` で `unlock` の直後に止める。Plan 2 が置いた `App.user` は読まれないので外した（`run` のローカル変数 `user` は `PasswordChecker::spawn` に move する）。
- `Cargo.toml`: nix の features に `signal` を足した。`zbus = "5"` を足した。
- タッチ待ちを子プロセスで行い、ロックを外すときに止めることは、設計書 4 章・6 章に反映済み。

## 設計書との対応

### Plan 5 で扱うもの

| 設計書 | 内容 | この計画 |
|---|---|---|
| 2 章 | pam_u2f（YubiKey タッチ）による認証。パスワードと同時に受け付ける | Task 1〜3、6 |
| 2 章 | KeePassXC 連携（ロック前のロック、解除後の解除要求）。設定でオン／オフ | Task 4〜7 |
| 4 章 | `auth/u2f`（ループ）、`auth/keepassxc`（zbus）。認証とはチャンネルでやりとりし、描画側はブロックされない | Task 1〜6 |
| 5 章 4 | `keepassxc.lock_before` なら全 DB のロックを依頼（完了は待たず、確認は裏で続ける） | Task 5、6 |
| 6 章 状態 | U2F の成功は入力待ち・失敗・確認中のどこからでも解除中へ。確認中ならパスワードの結果は捨てる | Plan 1 の `state`、Plan 2 の `handle_auth` をそのまま使う |
| 6 章 YubiKey | `pam_authenticate` の繰り返し／入力要求に答えない／成功で通知、失敗で 1 秒あけて再試行 | Task 1、2 |
| 6 章 YubiKey | 解除が決まったら、KeePassXC の解除要求より前に子プロセスを止める（SIGTERM、だめなら SIGKILL） | Task 2、6（契約の API の変更は「契約への追記」参照） |
| 6 章 KeePassXC | `lockAllDatabases`、未起動なら何もしない、`Locked` を最大 2 秒確認、残れば通知、ロックの進行は止めない／解除後に `Unlock` → `Prompt` | Task 4、5 |
| 7 章 | `auth.u2f.{enabled, pam_service}`、`keepassxc.{lock_before, unlock_after}` | Task 6、7 |
| 8 章 | PAM がエラーを返したら失敗として扱う（U2F は再試行）。異常終了時に helper を残さない | Task 1、2 |
| 9 章 | 手動確認（入れ子の sway） | Task 8 |

### Plan 5 で扱わないもの

- `contrib/pam.d/kabi-lock-u2f`（U2F 用 PAM サービスの例）、README での説明、設定例（Plan 6）。例のファイルには「pam_u2f の `auth` 行だけ、`pam_faillock` やパスワードのモジュールを入れない」ことを書いてもらう必要がある。
- KeePassXC が Secret Service に公開していない DB の確認（D-Bus から状態が分からない。今のスクリプトと同じ制約）。
- `Prompt` の `Completed` シグナルの待ち合わせ（設計書どおり待たない）。
- 通知の文言の翻訳（英語の固定文）。
- 設計書のその他の章（描画・配布など）は Plan 2〜4、6。

### 残る課題（実装時に気をつけること）

- helper の `PR_SET_PDEATHSIG` は、helper が起動してから設定するまでのごく短い間に親が死ぬと効かない。その場合でも helper は pam_u2f のタイムアウトか 1 回のタッチで終わる（ループはしない）。
- `PR_SET_PDEATHSIG` は「helper を起動したスレッド」が終わったときにも発火する。helper を起動するのは見張りスレッドで、見張りスレッドは helper を回収してから終わるので問題ない。helper の起動を別のスレッドに移すときは気をつける。
- KeePassXC が `org.keepassxc.KeePassXC.MainWindow` と `org.freedesktop.secrets` を別の D-Bus 接続で出す版では、`keepassxc_serves_secrets` が `false` になり、確認と解除要求をしない（ログ `Secret Service is not provided by KeePassXC` / `not serving Secret Service`）。この計画を書いた時点の KeePassXC では同じ一意名だった。
- 本物の pam_u2f と YubiKey での helper の動き（Step 2〜8）は、この計画を書いた時点では確かめていない。headless の sway では、存在しない PAM サービス（helper を起動しない）と、helper を `sleep` に差し替えた一時的な変更（コミットしない）で、起動・成功での解除・終了時の停止（helper が残らないこと）だけを確かめた。
