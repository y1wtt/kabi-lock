//! kabi-lock のコマンド。引数を読み、設定を読み込み、ロックして、解除されたら終わる。

use std::process::ExitCode;

use kabi_lock::app::{self, AppError};
use kabi_lock::auth::u2f;

/// 引数が不正なときの終了コード。
const USAGE_ERROR: u8 = 2;

const USAGE: &str = "\
Usage: kabi-lock [--help | --version]

Lock the session (ext-session-lock) until the password is entered.
Settings are read from $XDG_CONFIG_HOME/kabi-lock/config.toml.
Logging is controlled by RUST_LOG (default: warn).
";

/// 引数から決まる動作。
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Run,
    Version,
    Help,
    /// 隠しモード: YubiKey のタッチ待ちを 1 回だけ行う子プロセス（Plan 5）。
    U2fHelper(String),
}

/// プログラム名を除いた引数を読む。知らない引数や 2 つ以上の引数はエラー。
fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let mut args = args.into_iter();
    let command = match args.next().as_deref() {
        None => Command::Run,
        Some("--version" | "-V") => Command::Version,
        Some("--help" | "-h") => Command::Help,
        Some(arg) if arg == u2f::HELPER_ARG => match args.next() {
            Some(service) => Command::U2fHelper(service),
            None => return Err(format!("missing PAM service after {arg}")),
        },
        Some(other) => return Err(format!("unknown argument: {other}")),
    };
    if args.next().is_some() {
        return Err("too many arguments".to_string());
    }
    Ok(command)
}

/// `app::run` の結果を終了コードにする（0: 解除して終了、1: ロックできなかった）。
fn exit_code(result: &Result<(), AppError>) -> u8 {
    match result {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

fn main() -> ExitCode {
    let command = match parse_args(std::env::args().skip(1)) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("kabi-lock: {message}\n\n{USAGE}");
            return ExitCode::from(USAGE_ERROR);
        }
    };
    match command {
        Command::Version => {
            println!("kabi-lock {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Command::Help => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Command::Run => {
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
                .init();
            let config = kabi_lock::config::load();
            let result = app::run(config);
            if let Err(e) = &result {
                eprintln!("kabi-lock: {e}");
            }
            ExitCode::from(exit_code(&result))
        }
        Command::U2fHelper(service) => {
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
                .init();
            ExitCode::from(u2f::helper_main(&service))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Command, String> {
        parse_args(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn no_arguments_runs_the_locker() {
        assert_eq!(parse(&[]), Ok(Command::Run));
    }

    #[test]
    fn version_and_help_flags() {
        assert_eq!(parse(&["--version"]), Ok(Command::Version));
        assert_eq!(parse(&["-V"]), Ok(Command::Version));
        assert_eq!(parse(&["--help"]), Ok(Command::Help));
        assert_eq!(parse(&["-h"]), Ok(Command::Help));
    }

    #[test]
    fn unknown_arguments_are_errors() {
        assert_eq!(
            parse(&["--frobnicate"]),
            Err("unknown argument: --frobnicate".to_string())
        );
        assert_eq!(
            parse(&["--version", "--help"]),
            Err("too many arguments".to_string())
        );
    }

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
        assert_eq!(exit_code(&Ok(())), 0);
        assert_eq!(exit_code(&Err(AppError::LockDenied)), 1);
        assert_eq!(exit_code(&Err(AppError::LockUnsupported)), 1);
        assert_eq!(exit_code(&Err(AppError::NoWayland("x".into()))), 1);
        assert_eq!(exit_code(&Err(AppError::Other("x".into()))), 1);
        assert_eq!(USAGE_ERROR, 2);
    }
}
