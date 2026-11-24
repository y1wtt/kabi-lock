//! キー入力の分類。

#![forbid(unsafe_code)]

pub use smithay_client_toolkit::seat::keyboard::Keysym;

/// 1 回のキー押下で行う操作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAction {
    /// パスワードを確認に回す。
    Submit,
    /// 最後の 1 文字を消す。
    Backspace,
    /// 全部消す。
    Clear,
    /// xkbcommon が返した文字列をパスワードに足す。
    Insert,
    /// 何もしない。
    Ignore,
}

/// keysym と、xkbcommon が返した文字列と、Ctrl の押下状態から操作を決める。
///
/// 特別なキーは keysym で先に判定する（Enter や Esc は文字列としても `"\r"` や `"\x1b"` を返すため）。
/// 残りは文字列で判定し、空文字と、制御文字を 1 つでも含む文字列（Ctrl+A の `"\x01"` など）は入れない。
pub fn classify(keysym: Keysym, utf8: Option<&str>, ctrl: bool) -> KeyAction {
    match keysym {
        Keysym::Return | Keysym::KP_Enter => return KeyAction::Submit,
        Keysym::BackSpace => return KeyAction::Backspace,
        Keysym::Escape => return KeyAction::Clear,
        Keysym::u | Keysym::U if ctrl => return KeyAction::Clear,
        Keysym::Tab | Keysym::ISO_Left_Tab => return KeyAction::Ignore,
        _ => {}
    }
    match utf8 {
        Some(text) if !text.is_empty() && !text.chars().any(char::is_control) => KeyAction::Insert,
        _ => KeyAction::Ignore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_submits() {
        assert_eq!(
            classify(Keysym::Return, Some("\r"), false),
            KeyAction::Submit
        );
        assert_eq!(
            classify(Keysym::KP_Enter, Some("\r"), false),
            KeyAction::Submit
        );
        assert_eq!(
            classify(Keysym::Return, Some("\r"), true),
            KeyAction::Submit
        );
    }

    #[test]
    fn backspace_deletes() {
        assert_eq!(
            classify(Keysym::BackSpace, Some("\x08"), false),
            KeyAction::Backspace
        );
    }

    #[test]
    fn escape_and_ctrl_u_clear() {
        assert_eq!(
            classify(Keysym::Escape, Some("\x1b"), false),
            KeyAction::Clear
        );
        assert_eq!(classify(Keysym::u, Some("\x15"), true), KeyAction::Clear);
        assert_eq!(classify(Keysym::U, Some("\x15"), true), KeyAction::Clear);
    }

    #[test]
    fn u_without_ctrl_is_text() {
        assert_eq!(classify(Keysym::u, Some("u"), false), KeyAction::Insert);
    }

    #[test]
    fn tab_is_ignored() {
        assert_eq!(classify(Keysym::Tab, Some("\t"), false), KeyAction::Ignore);
        assert_eq!(
            classify(Keysym::ISO_Left_Tab, Some("\t"), false),
            KeyAction::Ignore
        );
    }

    #[test]
    fn control_characters_are_ignored() {
        // Ctrl+A は "\x01"、Delete は "\x7f" になる
        assert_eq!(classify(Keysym::a, Some("\x01"), true), KeyAction::Ignore);
        assert_eq!(
            classify(Keysym::Delete, Some("\x7f"), false),
            KeyAction::Ignore
        );
    }

    #[test]
    fn text_with_an_embedded_control_character_is_ignored() {
        assert_eq!(
            classify(Keysym::a, Some("a\u{1}"), false),
            KeyAction::Ignore
        );
        assert_eq!(
            classify(Keysym::a, Some("\u{7f}a"), false),
            KeyAction::Ignore
        );
    }

    #[test]
    fn multi_character_text_is_inserted() {
        // コンポーズキーや IME は 1 回の押下で複数文字を返すことがある
        assert_eq!(
            classify(Keysym::eacute, Some("é"), false),
            KeyAction::Insert
        );
        assert_eq!(classify(Keysym::a, Some("ab"), false), KeyAction::Insert);
        assert_eq!(
            classify(Keysym::a, Some("e\u{301}"), false),
            KeyAction::Insert
        );
    }

    #[test]
    fn keys_without_text_are_ignored() {
        assert_eq!(classify(Keysym::Shift_L, None, false), KeyAction::Ignore);
        assert_eq!(classify(Keysym::a, Some(""), false), KeyAction::Ignore);
    }

    #[test]
    fn printable_text_is_inserted() {
        assert_eq!(classify(Keysym::a, Some("a"), false), KeyAction::Insert);
        assert_eq!(
            classify(Keysym::adiaeresis, Some("ä"), false),
            KeyAction::Insert
        );
        // Ctrl を押していても文字になるキー（数字など）はそのまま入れる
        assert_eq!(classify(Keysym::_1, Some("1"), true), KeyAction::Insert);
    }
}
