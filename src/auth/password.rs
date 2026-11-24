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
