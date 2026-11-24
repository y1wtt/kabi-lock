//! パスワードの入力バッファ。

use zeroize::{Zeroize, Zeroizing};

/// 受け付けるパスワードの最大長（UTF-8 のバイト数）。
///
/// 最初にこの容量を確保し、超える入力は捨てる。`String` が再確保すると古い領域に
/// パスワードが残ったまま解放されるため、容量を固定して再確保を起こさない。
pub const MAX_LEN: usize = 256;

/// パスワードの入力バッファ。中身はドロップ時と `clear` 時にゼロで上書きされる。
///
/// 内容が漏れないよう `Debug` は実装しない。
pub struct PasswordBuffer {
    text: Zeroizing<String>,
}

impl PasswordBuffer {
    pub fn new() -> Self {
        Self {
            text: Zeroizing::new(String::with_capacity(MAX_LEN)),
        }
    }

    /// 文字列を末尾に足す。足すと `MAX_LEN` を超える場合は何もせず `false` を返す。
    pub fn push_str(&mut self, s: &str) -> bool {
        if self.text.len() + s.len() > MAX_LEN {
            return false;
        }
        self.text.push_str(s);
        true
    }

    /// 最後の 1 文字を消す。消せたら `true`。
    pub fn backspace(&mut self) -> bool {
        self.text.pop().is_some()
    }

    /// 中身をゼロで上書きして空にする（容量はそのまま）。
    pub fn clear(&mut self) {
        self.text.zeroize();
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// 中身を取り出してバッファを空にする。取り出した値もドロップ時にゼロで上書きされる。
    pub fn take(&mut self) -> Zeroizing<String> {
        let mut out = Zeroizing::new(String::with_capacity(MAX_LEN));
        out.push_str(&self.text);
        self.clear();
        out
    }
}

impl Default for PasswordBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_empty() {
        let buf = PasswordBuffer::new();
        assert!(buf.is_empty());
    }

    #[test]
    fn push_and_take() {
        let mut buf = PasswordBuffer::new();
        assert!(buf.push_str("ab"));
        assert!(buf.push_str("c"));
        let taken = buf.take();
        assert_eq!(taken.as_str(), "abc");
        assert!(buf.is_empty());
    }

    #[test]
    fn backspace_removes_one_char_even_if_multibyte() {
        let mut buf = PasswordBuffer::new();
        buf.push_str("aあ");
        assert!(buf.backspace());
        assert_eq!(buf.take().as_str(), "a");
    }

    #[test]
    fn backspace_on_empty_returns_false() {
        let mut buf = PasswordBuffer::new();
        assert!(!buf.backspace());
    }

    #[test]
    fn clear_empties_the_buffer() {
        let mut buf = PasswordBuffer::new();
        buf.push_str("secret");
        buf.clear();
        assert!(buf.is_empty());
        assert_eq!(buf.take().as_str(), "");
    }

    #[test]
    fn rejects_input_beyond_max_len() {
        let mut buf = PasswordBuffer::new();
        let full = "x".repeat(MAX_LEN);
        assert!(buf.push_str(&full));
        assert!(!buf.push_str("y"));
        assert_eq!(buf.take().len(), MAX_LEN);
    }
}
