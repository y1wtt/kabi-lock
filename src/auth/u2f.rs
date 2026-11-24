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
