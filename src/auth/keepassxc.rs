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
