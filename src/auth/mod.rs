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
