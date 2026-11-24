//! 認証。結果は `AuthEvent` としてイベントループへ送る。
//!
//! この下のモジュールでは `unsafe` を使わない。

#![forbid(unsafe_code)]

pub mod password;

use std::path::Path;

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

/// PAM のサービス定義を探すディレクトリ（libpam が見る場所）。
pub const PAM_DIRS: [&str; 2] = ["/etc/pam.d", "/usr/lib/pam.d"];

/// パスワード用の PAM サービスの定義ファイルがあるか。
///
/// 定義がないと libpam は `/etc/pam.d/other`（すべて拒否）に切り替わり、
/// どのパスワードでも解除できないロックになる。ロックを取る前にこれで確かめる。
pub fn pam_service_exists(name: &str) -> bool {
    let dirs: Vec<&Path> = PAM_DIRS.iter().map(Path::new).collect();
    pam_service_exists_in(name, &dirs)
}

/// `pam_service_exists` の本体。探すディレクトリを引数に取る（テスト用）。
///
/// 空の名前、`/` を含む名前、NUL を含む名前は、別の場所のファイルを指しうるので常に `false`。
fn pam_service_exists_in(name: &str, dirs: &[&Path]) -> bool {
    if name.is_empty() || name.contains('/') || name.contains('\0') {
        return false;
    }
    dirs.iter().any(|dir| dir.join(name).is_file())
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

    /// テスト用の一時ディレクトリ。名前はプロセス ID とテスト名で一意にする。
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let path =
                std::env::temp_dir().join(format!("kabi-lock-test-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            TempDir(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn pam_service_is_found_in_any_dir() {
        let first = TempDir::new("pam-first");
        let second = TempDir::new("pam-second");
        std::fs::write(second.0.join("kabi-lock"), "auth required pam_deny.so\n").unwrap();
        let dirs = [first.0.as_path(), second.0.as_path()];
        assert!(pam_service_exists_in("kabi-lock", &dirs));
        assert!(!pam_service_exists_in("missing", &dirs));
    }

    #[test]
    fn pam_service_must_be_a_file() {
        let dir = TempDir::new("pam-dir-entry");
        std::fs::create_dir(dir.0.join("subdir")).unwrap();
        assert!(!pam_service_exists_in("subdir", &[dir.0.as_path()]));
    }

    #[test]
    fn pam_service_rejects_odd_names() {
        let dir = TempDir::new("pam-odd");
        std::fs::write(dir.0.join("ok"), "").unwrap();
        let dirs = [dir.0.as_path()];
        assert!(pam_service_exists_in("ok", &dirs));
        assert!(!pam_service_exists_in("", &dirs));
        assert!(!pam_service_exists_in("../ok", &dirs));
        assert!(!pam_service_exists_in("a/b", &dirs));
        assert!(!pam_service_exists_in("ok\0", &dirs));
    }

    #[test]
    fn pam_service_with_no_dirs_is_missing() {
        assert!(!pam_service_exists_in("ok", &[]));
    }
}
