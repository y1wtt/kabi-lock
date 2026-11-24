//! pam_u2f（YubiKey のタッチ）による認証。
//!
//! タッチ待ちは子プロセスで行う。kabi-lock 自身を `--u2f-helper <PAM サービス>` で起動し直し、
//! 子（helper）は pam_u2f だけの PAM サービスで `pam_authenticate` を 1 回呼んで、結果を終了コードで返す。
//! 親では見張りスレッドが子の終了を待ち、成功なら `AuthEvent::U2fSucceeded` を 1 回送って終わる。
//! 失敗なら `RETRY_DELAY` あけて子を起動し直す。ただし PAM サービスが入力（パスワードなど）を
//! 求めたときは設定の誤りとみなし、そのロック中は起動し直さない（`pam_faillock` の失敗回数を
//! 1 秒ごとに積み上げて、アカウントがロックされるのを防ぐ）。
//!
//! `pam_authenticate` は途中で止められないが、子プロセスなら止められる。解除が決まったら
//! `U2fHandle::stop`（`Drop` でも呼ばれる）で子を止める。YubiKey がタッチを待ったまま残ると、
//! 解除後に出す KeePassXC の解除ダイアログのタッチを横取りしてしまうため。

#![forbid(unsafe_code)]

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

use super::{AuthEvent, PAM_DIRS};

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
    /// PAM サービスが入力を求めた（設定の誤り）。このロック中はもう起動しない。
    GiveUp,
}

/// 止められていれば（成功していても）何も知らせない。解除はもう決まっているため。
///
/// 入力を求められたときは再試行しない。そのサービスが `pam_unix` などに落ちる作りだと、
/// 試すたびに `pam_faillock` の失敗として数えられ、すぐにアカウントがロックされてしまう。
fn next_step(exit: HelperExit, stopped: bool) -> Next {
    if stopped {
        Next::Stop
    } else {
        match exit {
            HelperExit::Accepted => Next::Succeed,
            HelperExit::AskedForInput => Next::GiveUp,
            _ => Next::Retry,
        }
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
        HelperExit::SetupFailed => warn!("u2f: the helper could not start a PAM transaction"),
        HelperExit::Other(code) => warn!("u2f: the helper ended unexpectedly (exit code {code:?})"),
        HelperExit::NotStarted => warn!("u2f: the helper could not be started"),
        // 成功は `supervise` が、入力の要求は諦めるときに 1 回だけログに出す
        HelperExit::Accepted | HelperExit::AskedForInput => {}
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
/// `service` はログに出す PAM サービスの名前。
fn start<F, S>(
    service: String,
    make: F,
    on_success: S,
    retry_delay: Duration,
) -> std::io::Result<U2fHandle>
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
        move || supervise(&shared, &service, make, on_success, retry_delay)
    })?;
    Ok(U2fHandle {
        shared,
        thread: Some(thread),
    })
}

/// helper を起動して終わりを待つ、を成功するか止められるか、入力を求められるまで繰り返す。
fn supervise<F, S>(shared: &Shared, service: &str, make: F, on_success: S, retry_delay: Duration)
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
            Next::GiveUp => {
                warn!(
                    "u2f: PAM service {service} asks for a password; \
                     security-key unlock is off until the next lock"
                );
                return;
            }
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
/// PAM サービスが入力を求めたら、警告を 1 回出してこのロック中は再試行しない。
/// PAM サービスの設定ファイルがない、またはスレッドを起動できなければ `None`（タッチでは解除できない）。
pub fn spawn(service: String, events: Sender<AuthEvent>) -> Option<U2fHandle> {
    let Some(file) = find_service_file(&service, &PAM_DIRS) else {
        warn!("u2f: pam service {service:?} not found in {PAM_DIRS:?}; YubiKey unlock is off");
        return None;
    };
    let make = {
        let service = service.clone();
        move || {
            let mut command = Command::new(SELF_EXE);
            command.arg(HELPER_ARG).arg(&service);
            command
        }
    };
    let on_success = move || {
        if events.send(AuthEvent::U2fSucceeded).is_err() {
            debug!("u2f: event loop is gone; dropping the result");
        }
    };
    match start(service, make, on_success, RETRY_DELAY) {
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
        assert_eq!(next_step(HelperExit::SetupFailed, false), Next::Retry);
        assert_eq!(next_step(HelperExit::Other(Some(2)), false), Next::Retry);
        // 入力を求めるサービスで繰り返すと pam_faillock の失敗が積み上がるので、このロック中はやめる
        assert_eq!(next_step(HelperExit::AskedForInput, false), Next::GiveUp);
        assert_eq!(next_step(HelperExit::AskedForInput, true), Next::Stop);
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

    fn test_service() -> String {
        "kabi-lock-test".into()
    }

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
        let mut handle = start(test_service(), sh("exit 0"), on_success, SHORT).unwrap();
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
        let _handle = start(test_service(), make, on_success, SHORT).unwrap();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(*calls.lock().unwrap(), 3);
    }

    #[test]
    fn stop_kills_a_waiting_helper() {
        let (on_success, rx) = success_channel();
        let mut handle = start(test_service(), sh("exec sleep 30"), on_success, SHORT).unwrap();
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
        let handle = start(test_service(), sh("exec sleep 30"), on_success, SHORT).unwrap();
        let pid = wait_for_child(&handle);
        drop(handle);
        assert!(process_gone(pid));
    }

    #[test]
    fn a_helper_ignoring_sigterm_is_killed() {
        let (on_success, _rx) = success_channel();
        let mut handle = start(
            test_service(),
            sh("trap '' TERM; exec sleep 30"),
            on_success,
            SHORT,
        )
        .unwrap();
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
        let mut handle = start(
            test_service(),
            sh("exit 1"),
            on_success,
            Duration::from_secs(60),
        )
        .unwrap();
        // 最初の helper が終わって、再試行の待ちに入るまで待つ
        thread::sleep(Duration::from_millis(200));
        let started = Instant::now();
        handle.stop();
        assert!(started.elapsed() < QUICK);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn a_helper_asking_for_input_is_not_restarted() {
        let calls = Arc::new(Mutex::new(0u32));
        let make = {
            let calls = Arc::clone(&calls);
            move || {
                *calls.lock().unwrap() += 1;
                let mut command = Command::new("sh");
                command.arg("-c").arg("exit 3");
                command
            }
        };
        let (on_success, rx) = success_channel();
        let mut handle = start(test_service(), make, on_success, SHORT).unwrap();
        // 再試行しないなら、見張りスレッドは自分で終わる
        let deadline = Instant::now() + QUICK;
        while !handle.thread.as_ref().unwrap().is_finished() {
            assert!(Instant::now() < deadline, "the supervisor kept retrying");
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(*calls.lock().unwrap(), 1);
        handle.stop();
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn a_command_that_cannot_start_is_retried() {
        let (on_success, rx) = success_channel();
        let mut handle = start(
            test_service(),
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
