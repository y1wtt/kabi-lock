//! 出力の情報（名前・説明）。

use std::ffi::OsString;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use smithay_client_toolkit::output::OutputInfo;

/// ロック中に覚えておく出力の情報。主画面の選択（Plan 4）とログに使う。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputMeta {
    /// 出力名（例: `DP-10`）。コンポジタが送ってこなければ空。
    pub name: String,
    /// wl_output の description。送ってこなければ空。
    pub description: String,
}

impl OutputMeta {
    /// SCTK の出力情報から作る。情報がまだ届いていなければ両方とも空。
    pub fn from_info(info: Option<&OutputInfo>) -> OutputMeta {
        OutputMeta::from_parts(
            info.and_then(|i| i.name.clone()),
            info.and_then(|i| i.description.clone()),
        )
    }

    pub fn from_parts(name: Option<String>, description: Option<String>) -> OutputMeta {
        OutputMeta {
            name: name.unwrap_or_default(),
            description: description.unwrap_or_default(),
        }
    }

    /// ログに出す名前。
    pub fn label(&self) -> &str {
        if self.name.is_empty() {
            "(unnamed output)"
        } else {
            &self.name
        }
    }

    /// `config::select_primary` に渡す形にする。
    pub fn to_output_info(&self) -> crate::config::OutputInfo {
        crate::config::OutputInfo {
            name: self.name.clone(),
            description: self.description.clone(),
        }
    }
}

/// sway IPC の読み書きの期限。ロック要求の前なので、応答しない sway で止まらないようにする。
const IPC_TIMEOUT: Duration = Duration::from_millis(300);

/// ロック前に sway でフォーカスしていた出力の名前（sway IPC の `get_outputs` の `focused`）。
///
/// ソケットは `SWAYSOCK` だけを見る（swayipc の `Connection::new` は `I3SOCK` を先に見て、
/// どちらもなければ `sway --get-socketpath` を期限なしで呼ぶので使わない）。読み書きには期限を付ける。
/// sway でない・`SWAYSOCK` がない・つながらない・期限までに答えない・応答が不正なときは `None`
/// （主画面は設定か最初の出力で決める）。ロックの後はフォーカスの意味が変わるので、ロック前に 1 回だけ呼ぶ。
pub fn focused_output() -> Option<String> {
    focused_output_at(std::env::var_os("SWAYSOCK"))
}

fn focused_output_at(socket: Option<OsString>) -> Option<String> {
    let Some(socket) = socket else {
        log::info!("SWAYSOCK is not set; not asking sway for the focused output");
        return None;
    };
    let stream = match UnixStream::connect(&socket) {
        Ok(stream) => stream,
        Err(e) => {
            log::warn!("cannot connect to sway IPC at {}: {e}", socket.display());
            return None;
        }
    };
    if let Err(e) = stream
        .set_read_timeout(Some(IPC_TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(IPC_TIMEOUT)))
    {
        log::warn!("cannot set the sway IPC timeout: {e}");
        return None;
    }
    let mut conn = swayipc::Connection::from(stream);
    match conn.get_outputs() {
        Ok(outputs) => outputs.into_iter().find(|o| o.focused).map(|o| o.name),
        Err(e) => {
            log::warn!("sway IPC get_outputs failed: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_name_and_description() {
        let meta = OutputMeta::from_parts(
            Some("DP-10".into()),
            Some("LG Electronics LG ULTRAWIDE 311NTDV3Z418 (DP-10)".into()),
        );
        assert_eq!(meta.name, "DP-10");
        assert_eq!(
            meta.description,
            "LG Electronics LG ULTRAWIDE 311NTDV3Z418 (DP-10)"
        );
        assert_eq!(meta.label(), "DP-10");
    }

    #[test]
    fn missing_values_become_empty() {
        let meta = OutputMeta::from_parts(None, None);
        assert_eq!(meta.name, "");
        assert_eq!(meta.description, "");
        assert_eq!(meta.label(), "(unnamed output)");
    }

    #[test]
    fn no_swaysock_means_no_focused_output() {
        assert_eq!(focused_output_at(None), None);
        assert_eq!(
            focused_output_at(Some("/nonexistent/kabi-lock-test.sock".into())),
            None
        );
    }

    #[test]
    fn silent_sway_socket_times_out() {
        // 接続は受け付けるが何も返さないソケット（accept もしない）
        let path = std::env::temp_dir().join(format!(
            "kabi-lock-test-{}-{}.sock",
            std::process::id(),
            fastrand::u64(..)
        ));
        let _listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let start = std::time::Instant::now();
        let focused = focused_output_at(Some(path.clone().into()));
        let elapsed = start.elapsed();
        let _ = std::fs::remove_file(&path);
        assert_eq!(focused, None);
        assert!(elapsed < std::time::Duration::from_secs(3), "{elapsed:?}");
    }

    #[test]
    fn converts_to_config_output_info() {
        let meta = OutputMeta::from_parts(Some("eDP-1".into()), Some("panel (eDP-1)".into()));
        let info = meta.to_output_info();
        assert_eq!(info.name, "eDP-1");
        assert_eq!(info.description, "panel (eDP-1)");
    }
}
