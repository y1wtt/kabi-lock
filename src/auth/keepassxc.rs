//! KeePassXC と Secret Service の連携。
//!
//! - ロック前: KeePassXC に全 DB のロックを頼み（返事は待たない）、別スレッドで Secret Service の
//!   各コレクションの `Locked` を最大 2 秒確かめる。ロックされないものが残れば通知を出す。
//! - 解除後: Secret Service でロック中のコレクションがあれば `Unlock` を呼び、返ったプロンプトの
//!   `Prompt` を呼ぶ（KeePassXC の解除ダイアログが出る）。プロンプトの完了は待たない。
//!
//! KeePassXC が起動していなければ何もしない。`org.freedesktop.secrets` は D-Bus の自動起動の
//! 対象になっていることがあるので、名前の持ち主を先に確かめ、いないときは呼ばない。
//! 確かめた後の呼び出しは、よく知られた名前ではなく持ち主の一意名（`:1.23` など）に送る。
//! 確かめてから呼ぶまでの間に名前が手放されても、自動起動や別の持ち主（gnome-keyring など）に
//! 届かないようにするため。
//! D-Bus の呼び出しと、テストできる判断（純粋な関数）を分けて書く。

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::thread;
use std::time::Duration;

use log::{debug, info, warn};
use zbus::blocking::Connection;
use zbus::message::{Flags, Message};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

const KEEPASSXC_NAME: &str = "org.keepassxc.KeePassXC.MainWindow";
const KEEPASSXC_PATH: &str = "/keepassxc";
const KEEPASSXC_IFACE: &str = "org.keepassxc.KeePassXC.MainWindow";

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

/// Secret Service を KeePassXC 自身が提供していれば（2 つの名前の持ち主が同じ接続なら）、その一意名。
///
/// 別のもの（gnome-keyring など）が提供しているときは、KeePassXC の DB の状態は分からないので
/// `None` にして、確認も解除要求もしない。
fn secrets_owner<'a>(
    keepassxc_owner: Option<&'a str>,
    secrets_owner: Option<&str>,
) -> Option<&'a str> {
    match (keepassxc_owner, secrets_owner) {
        (Some(a), Some(b)) if a == b => Some(a),
        _ => None,
    }
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
/// 出し、返事が遅れることがあるため）。`owner` は KeePassXC の一意名。
fn lock_all_message(owner: &str) -> zbus::Result<Message> {
    Message::method_call(KEEPASSXC_PATH, "lockAllDatabases")?
        .destination(owner)?
        .interface(KEEPASSXC_IFACE)?
        .with_flags(Flags::NoReplyExpected)?
        .build(&())
}

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

/// KeePassXC の D-Bus 名の持ち主（一意名）と、Secret Service も KeePassXC が提供していれば、その一意名。
struct Owners {
    keepassxc: Option<String>,
    secrets: Option<String>,
}

fn owners(conn: &Connection) -> Owners {
    let keepassxc = name_owner(conn, KEEPASSXC_NAME);
    let secrets_name_owner = name_owner(conn, SECRETS_NAME);
    let secrets =
        secrets_owner(keepassxc.as_deref(), secrets_name_owner.as_deref()).map(str::to_owned);
    Owners { keepassxc, secrets }
}

/// Secret Service（一意名 `owner`）のプロパティを読む。
fn get_secret_property(
    conn: &Connection,
    owner: &str,
    path: &str,
    iface: &str,
    name: &str,
) -> zbus::Result<OwnedValue> {
    let reply = conn.call_method(
        Some(owner),
        path,
        Some(PROPERTIES_IFACE),
        "Get",
        &(iface, name),
    )?;
    reply.body().deserialize::<OwnedValue>()
}

/// Secret Service（一意名 `owner`）の全コレクションと、それぞれの `Locked`。読めないコレクションは飛ばす。
fn collections(conn: &Connection, owner: &str) -> zbus::Result<Vec<Collection>> {
    let value = get_secret_property(conn, owner, SECRETS_PATH, SERVICE_IFACE, "Collections")?;
    let paths = parse_paths(value)
        .ok_or_else(|| zbus::Error::Failure("Collections is not an object path array".into()))?;
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        match get_secret_property(conn, owner, &path, COLLECTION_IFACE, "Locked").map(parse_bool) {
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
    let Some(keepassxc) = owners.keepassxc else {
        info!("keepassxc: not running; nothing to lock");
        return Ok(());
    };
    conn.send(&lock_all_message(&keepassxc)?)?;
    info!("keepassxc: asked to lock all databases");
    let Some(secrets) = owners.secrets else {
        info!("keepassxc: Secret Service is not provided by KeePassXC; cannot verify the lock");
        return Ok(());
    };
    let open = wait_until_locked(
        || match collections(&conn, &secrets) {
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
    let Some(secrets) = owners(&conn).secrets else {
        info!("keepassxc: not serving Secret Service; nothing to unlock");
        return Ok(());
    };
    let locked = locked_paths(&collections(&conn, &secrets)?);
    if locked.is_empty() {
        info!("keepassxc: no locked collections; nothing to unlock");
        return Ok(());
    }
    let targets = locked
        .iter()
        .map(|p| ObjectPath::try_from(p.as_str()))
        .collect::<Result<Vec<_>, _>>()?;
    let reply = conn.call_method(
        Some(secrets.as_str()),
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
                Some(secrets.as_str()),
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
        assert_eq!(secrets_owner(Some(":1.10"), Some(":1.10")), Some(":1.10"));
    }

    #[test]
    fn other_owner_does_not_serve_secrets() {
        assert_eq!(secrets_owner(Some(":1.10"), Some(":1.11")), None);
        assert_eq!(secrets_owner(Some(":1.10"), None), None);
        assert_eq!(secrets_owner(None, Some(":1.11")), None);
        assert_eq!(secrets_owner(None, None), None);
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
        let msg = lock_all_message(":1.10").unwrap();
        let header = msg.header();
        assert!(
            msg.primary_header()
                .flags()
                .contains(Flags::NoReplyExpected)
        );
        assert_eq!(header.member().unwrap().as_str(), "lockAllDatabases");
        assert_eq!(header.path().unwrap().as_str(), KEEPASSXC_PATH);
        assert_eq!(header.interface().unwrap().as_str(), KEEPASSXC_IFACE);
        // 確かめた一意名に送る（よく知られた名前だと、その間に持ち主が変わると自動起動しうる）
        assert_eq!(header.destination().unwrap().to_string(), ":1.10");
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
