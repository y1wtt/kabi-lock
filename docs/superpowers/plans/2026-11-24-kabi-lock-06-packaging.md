# kabi-lock Plan 6: 配布 実装計画

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** kabi-lock を Arch Linux でインストールでき、README と設定例で使い方が分かる状態にする。PAM サービスファイル、U2F 用 PAM の例、sway のキー割り当て例、設定例、PKGBUILD、README を作る。

**Architecture:** コードは変えない。例外は 1 つで、`docs/config.example.toml` が本物のパーサーで読めて初期値と一致することを守る単体テストを `src/config.rs` に足す（Task 3）。それ以外はすべて設定・文書・パッケージ定義のファイル。パッケージは GitHub の `y1wtt/kabi-lock` を git ソースとして取り、`cargo` でビルドして、バイナリ・PAM ファイル・ライセンス・文書・例を入れる。

**Tech Stack:** PKGBUILD（makepkg）、PAM、TOML、Markdown。検証に `bash -n`、`makepkg --printsrcinfo`、`pam_start_confdir`（C）、`sway --validate`、`cargo test`。

**Spec:** `docs/superpowers/specs/2026-11-24-kabi-lock-design.md`（6 章の PAM 設定ファイル、7 章の設定ファイル、10 章の配布）
**契約:** `docs/superpowers/plans/2026-11-24-kabi-lock-contract.md`（4 章「Plan 6 が作るもの」）

## 前提（Plan 2〜5 が終わっていること）

この計画は Plan 2〜5 の実行後に行う。次のことを前提にしている。違っていたら、該当するタスクで直す。

1. バイナリ名は `kabi-lock`、`Cargo.lock` はリポジトリに入っている（`--locked` を使うため）。
2. Plan 4 が `assets/fonts/` に Inter（`Inter-Medium.ttf`、`Inter-Regular.ttf`）と、そのライセンス文（SIL OFL 1.1）**`assets/fonts/LICENSE.txt`** を置く（Plan 4 の Task で確認済み）。
3. `Config` のキーは設計書 7 章のまま（Plan 2〜5 でキーが増えたら、`docs/config.example.toml` とそのテスト（Task 3）のキー一覧にも足す）。
4. Plan 2〜5 の単体テストは Wayland・GPU・D-Bus を使わない（PKGBUILD の `check()` が `cargo test` を素のビルド環境で動かすため）。
5. Plan 4 は実行時に fontconfig の `fc-list` コマンドを呼んで、`appearance.font` のフォントを探す。`fc-list` がなくても埋め込みの Inter で動くので、README では「任意」と書くが、`fc-list` を提供する `fontconfig` は `depends` に入れる（日本語の日付のフォールバック用のフォントも同じ仕組みで探す）。

## 設計上の判断

| 項目 | 決めたこと | 理由 |
|---|---|---|
| パッケージ名 | `kabi-lock-git`（`provides=kabi-lock`、`conflicts=kabi-lock`） | まだリリースタグがなく、1.0 前は `main` の最新を配る方が実情に合う。既存の `swaylock-effects-y1wtt` も同じ流儀。`v0.1.0` を打った後に、固定タグ版 `kabi-lock` を別に作る道は残す（`conflicts` で同時に入らないようにしてある）。 |
| `pkgver()` | `git describe --long --tags --match 'v[0-9]*'` を `0.1.0.r3.gabc1234` の形に直す。タグがなければ `r<コミット数>.g<ハッシュ>` | 既存の PKGBUILD と同じ形。タグ前でもビルドが壊れないように予備を持つ。 |
| ビルドの引数 | `prepare()` で `cargo fetch --locked`、`build()` と `check()` は `cargo ... --frozen --release` | `--frozen` は `--locked` と `--offline` の両方を含む。Arch の Rust パッケージの作法で、ネットワークを使うのを `prepare()` に限る。`Cargo.lock` からずれると失敗するのは `--locked` と同じ。 |
| `makedepends` | `cargo` `clang` `git` | `cargo` は Arch では `rust` が提供する。**`clang` は契約にないが必要**: `pam-client` が使う `pam-sys 1.0.0-alpha5` は `build.rs` で bindgen を動かし、libclang を使う（`~/.cargo/registry` の `pam-sys` の `Cargo.toml` で確認）。 |
| `depends` | `pam` `libxkbcommon` `wayland` `libglvnd` `fontconfig` `gcc-libs` `glibc` | `libglvnd` は `khronos-egl`（static）が `libEGL` を、`glow` が `libGLESv2` を使うため。GPU ドライバー（`mesa` など）は人によって違うので `optdepends`。 |
| `options=('!lto')` | 付ける | `makepkg` の既定の C 向け LTO を Rust のビルドに持ち込まない（Arch の Rust パッケージの作法）。 |
| `/etc/pam.d/kabi-lock` | `auth include login` と `account include login` を入れる | 設計書 6 章・契約どおり。Plan 2 の `pam_check` は `pam_authenticate` の後に `acct_mgmt(Flag::NONE)` を呼ぶ。`auth` の行だけの PAM サービスでは `pam_acct_mgmt` が `PAM_PERM_DENIED`（6）で失敗し、正しいパスワードでも解除できない（Task 1 の Step 4 で確かめる）ので、`account` の行は必須。 |
| README の言語 | 英語を本体にして、末尾に短い日本語の節を付ける | リポジトリは公開の OSS で、Arch のユーザーが読むのは英語が前提。ただし作者が日本語話者で、設計書もデザインも日本語なので、要点（インストール・設定・注意）だけを日本語で添える。全文の二重管理は避ける。 |
| PKGBUILD の置き場所と `.SRCINFO` | `packaging/arch/PKGBUILD` と、同じ場所に `.SRCINFO`、`.gitignore` | 設計書 10 章。`.SRCINFO` は AUR に出すときのために置く。`pkgver` の欄は固定の見本（`0.1.0.r0.g0000000`）で、`makepkg` が実行時に書き換える。 |

## ファイル構成

| ファイル | 責務 |
|---|---|
| `packaging/pam.d/kabi-lock` | パスワード確認用の PAM サービス（パッケージが `/etc/pam.d/kabi-lock` に入れる） |
| `contrib/pam.d/kabi-lock-u2f` | YubiKey 用 PAM サービスの例（入れない） |
| `contrib/sway/kabi-lock.conf` | sway のキー割り当ての例 |
| `docs/config.example.toml` | 全キーと初期値を書いた設定例 |
| `src/config.rs`（変更） | 設定例を検証するテストを 1 つ足す |
| `README.md` | 英語の README（日本語の節つき） |
| `packaging/arch/PKGBUILD` | Arch 用パッケージ定義 |
| `packaging/arch/.gitignore` | `makepkg` が作る作業ファイルを無視する |
| `packaging/arch/.SRCINFO` | `makepkg --printsrcinfo` の出力 |

## 共通の決まり

- コミットは契約 1 章の決まりに従う（Conventional Commits、本文は英語、末尾に書いたモデルの `Co-Authored-By`、原則 1 コミット 1 ファイル）。以下のコミット例の `Claude Opus 5.5` は、書いたモデルの名前に置き換える。
- 各コミットの前に `git status` と `git diff` を実行して変更を確認する。
- コード以外のファイルの中身（コメント・README）は英語。公開リポジトリの利用者が読むため。
- `makepkg` はリポジトリの中では実行しない（`pkgver` が書き換わり、作業ファイルが散らかる）。確認は `/tmp` のコピーで行う。
- 実際のインストール（`sudo pacman -U`）は人が行う。エージェントは実行しない。

---

### Task 1: PAM サービスファイル（パッケージ用と U2F の例）

**Files:**
- Create: `packaging/pam.d/kabi-lock`
- Create: `contrib/pam.d/kabi-lock-u2f`

**Interfaces:**
- Consumes: なし（`login` の PAM サービスが `/etc/pam.d/login` にあること）
- Produces: `/etc/pam.d/kabi-lock` になるファイル（`config.auth.pam_service` の初期値 `kabi-lock` が指すもの）と、`auth.u2f.pam_service` の初期値 `kabi-lock-u2f` に使える例

- [ ] **Step 1: 前提を確認する**

Run（リポジトリのルート `~/oss/kabi-lock` で）:

```bash
git status
git log --oneline | head -5
ls LICENSE docs/design/images/lock-screen.png
ls assets/fonts/
cargo test
```

Expected: 作業ツリーがきれい。`LICENSE` と `lock-screen.png` がある。`assets/fonts/` に `Inter-Medium.ttf`、`Inter-Regular.ttf`、`LICENSE.txt` がある。`cargo test` が全部通る。

`assets/fonts/` に `Inter-Medium.ttf`、`Inter-Regular.ttf`、`LICENSE.txt` の 3 つがあることを確認する（なければ Plan 4 が終わっていない。止まって報告する）。

- [ ] **Step 2: `packaging/pam.d/kabi-lock` を書く**

```text
#%PAM-1.0
# PAM service used by kabi-lock to check your password.
#
# It reuses the stack of a normal console login, so the lock screen follows
# the same policy as logging in (pam_faillock, pam_unix, ...).
# `account` is needed as well: kabi-lock calls pam_acct_mgmt after a
# successful pam_authenticate, and a service with no account line fails there.
auth       include      login
account    include      login
```

- [ ] **Step 3: `contrib/pam.d/kabi-lock-u2f` を書く**

```text
#%PAM-1.0
# Example PAM service for kabi-lock's YubiKey (FIDO U2F) unlock.
#
# This file is NOT installed by the package: the authfile path differs per
# user and per setup. Copy it to /etc/pam.d/kabi-lock-u2f (as root) and fix
# the authfile, or point `auth.u2f.pam_service` in config.toml at a service
# you already have.
#
# WARNING: keep this service to pam_u2f ONLY.
# While the screen is locked, kabi-lock calls pam_authenticate on this service
# again and again (once a second after each failure) until a key is touched.
# Do not add pam_faillock, pam_unix or any other module that asks for a
# password, and do not `include` another stack (such as system-auth or login):
# every failed try would be counted, the failures pile up every second, and
# your account can end up locked.
#
# Touching a registered key unlocks the screen with no password and no PIN.
# Every prompt (password, PIN) is answered with a failure, so PIN-protected
# keys (pinverification=1) cannot be used here.
#
# authfile: one of
#   /etc/u2f_mappings                    system-wide, one "user:keyhandle,..." line per user
#   /home/YOUR_USER/.config/Yubico/u2f_keys   per user (the pam_u2f default if authfile is omitted)
# Register a key with pamu2fcfg (package pam-u2f), see pamu2fcfg(1).
auth       required     pam_u2f.so    authfile=/etc/u2f_mappings
account    required     pam_permit.so
```

- [ ] **Step 4: `pam_acct_mgmt` が通ることを確かめる（コミットしない一時ファイル）**

PAM サービスを `/etc/pam.d` に置かずに確かめるため、`pam_start_confdir` で別のディレクトリを指す。`pam_authenticate` は呼ばないので、パスワードの失敗は数えられない（`pam_faillock` でアカウントがロックされるおそれはない）。

```bash
WORK=$(mktemp -d)
cat > "$WORK/acct_check.c" <<'CEOF'
// 使い方: acct_check <pam.d の代わりのディレクトリ> <サービス名>
// pam_acct_mgmt だけを呼ぶ（pam_authenticate は呼ばないので faillock は増えない）。
#include <security/pam_appl.h>
#include <stdio.h>
#include <pwd.h>
#include <unistd.h>

static int conv(int n, const struct pam_message **m, struct pam_response **r, void *d) {
    return PAM_CONV_ERR;
}

int main(int argc, char **argv) {
    if (argc != 3) { fprintf(stderr, "usage: %s CONFDIR SERVICE\n", argv[0]); return 2; }
    struct pam_conv c = { conv, NULL };
    pam_handle_t *h = NULL;
    struct passwd *pw = getpwuid(getuid());
    int rc = pam_start_confdir(argv[2], pw->pw_name, &c, argv[1], &h);
    if (rc != PAM_SUCCESS) { printf("pam_start=%d\n", rc); return 1; }
    rc = pam_acct_mgmt(h, 0);
    printf("%s: pam_acct_mgmt=%d (%s)\n", argv[2], rc, pam_strerror(h, rc));
    pam_end(h, rc);
    return rc == PAM_SUCCESS ? 0 : 1;
}
CEOF
gcc "$WORK/acct_check.c" -o "$WORK/acct_check" -lpam

CONF=$(mktemp -d)
cp /etc/pam.d/login /etc/pam.d/system-local-login /etc/pam.d/system-login /etc/pam.d/system-auth "$CONF/"
cp packaging/pam.d/kabi-lock contrib/pam.d/kabi-lock-u2f "$CONF/"

"$WORK/acct_check" "$CONF" kabi-lock
"$WORK/acct_check" "$CONF" kabi-lock-u2f

# 比較: account の行がない版
sed -i '/^account/d' "$CONF/kabi-lock"
"$WORK/acct_check" "$CONF" kabi-lock
ls /usr/lib/security/pam_u2f.so
```

Expected:

```text
kabi-lock: pam_acct_mgmt=0 (Success)
kabi-lock-u2f: pam_acct_mgmt=0 (Success)
kabi-lock: pam_acct_mgmt=6 (Permission denied)
/usr/lib/security/pam_u2f.so
```

3 行目（`account` なし）が 6 で失敗することが、`account include login` を足した理由。`pam_u2f.so` がなければ `pam-u2f` が入っていない（例のファイルの検証には必要ないが、手動確認の前に入れる）。確認が終わったら `rm -r "$WORK" "$CONF"`（この 2 つは `mktemp -d` が作った `/tmp` の下のディレクトリ）。

- [ ] **Step 5: コミットする**

```bash
git status
git diff
git add packaging/pam.d/kabi-lock
git commit -m "feat: add PAM service file for the password check

Include the login stack for both auth and account, because kabi-lock
calls pam_acct_mgmt and a service without account lines fails there.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git add contrib/pam.d/kabi-lock-u2f
git commit -m "docs: add example PAM service for YubiKey unlock

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

（`git diff` は未追跡ファイルを出さないので、`git status` で 2 ファイルが untracked と出ることを確認する。）

---

### Task 2: sway のキー割り当ての例

**Files:**
- Create: `contrib/sway/kabi-lock.conf`

**Interfaces:**
- Consumes: `kabi-lock` が `PATH` にあること
- Produces: `bindsym $mod+l exec kabi-lock` の例（パッケージが `/usr/share/kabi-lock/sway/kabi-lock.conf` に入れる）

- [ ] **Step 1: `contrib/sway/kabi-lock.conf` を書く**

```text
# kabi-lock: key binding for sway.
#
# Put this line in ~/.config/sway/config (or in a file included from it).
# $mod is the modifier variable from your sway config (Mod4 = Super / Meta).
#
# Note: the default sway config already uses $mod+l for "focus right".
# A later bindsym for the same key replaces the earlier one, so either remove
# the old binding or pick another key, for example $mod+Shift+l.
bindsym $mod+l exec kabi-lock
```

- [ ] **Step 2: sway の構文確認**

`$mod` は利用者の設定にある変数なので、確認用に定義を足したファイルで見る。

```bash
(echo 'set $mod Mod4'; cat contrib/sway/kabi-lock.conf) > /tmp/kabi-sway-check.conf
sway --validate -c /tmp/kabi-sway-check.conf; echo "rc=$?"
rm /tmp/kabi-sway-check.conf
```

Expected: エラーの出力なしで `rc=0`。（`--validate` は構文を見るだけで、コマンドの中身は実行しない。）

- [ ] **Step 3: コミットする**

```bash
git status
git add contrib/sway/kabi-lock.conf
git commit -m "docs: add sway key binding example

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: 設定例と、それを検証するテスト（TDD）

**Files:**
- Create: `docs/config.example.toml`
- Modify: `src/config.rs`（`tests` モジュールにテストを 1 つ足すだけ）

**Interfaces:**
- Consumes: `config::parse`、`Config::default()`（Plan 1）
- Produces: 全キーと初期値を書いた `docs/config.example.toml`（パッケージが `/usr/share/kabi-lock/config.example.toml` に入れる）。テスト `example_config_matches_defaults_and_lists_every_key`

設定例が実際のパーサーで読めること（未知のキーや型の誤りがないこと）、初期値と同じであること、全 13 キーが書かれていることを、`include_str!` で 1 つのテストにする。後で `Config` のキーや初期値を変えると、このテストが落ちて設定例の更新漏れに気づける。

- [ ] **Step 1: 失敗するテストを書く**

`src/config.rs` の `mod tests` の中の、`fn outputs() -> Vec<OutputInfo> {` の直前に次を足す（その行が見つからないときは、`mod tests` の末尾に足す）:

```rust
    /// 設定例の `docs/config.example.toml` が、実際のパーサーで警告なしに読めて、
    /// 初期値と同じであり、設計書 7 章の全キーを載せていることを確かめる。
    #[test]
    fn example_config_matches_defaults_and_lists_every_key() {
        let text = include_str!("../docs/config.example.toml");
        let loaded = parse(text).expect("example config must parse");
        assert!(
            loaded.unknown_keys.is_empty(),
            "unknown keys in example: {:?}",
            loaded.unknown_keys
        );
        assert_eq!(loaded.config, Config::default());

        // 省略しても初期値になるので、キーが消えても上の比較では気づけない。
        // 例にすべてのキーが書かれているかを、TOML の葉のキーで確かめる。
        fn leaf_keys(prefix: &str, value: &toml::Value, out: &mut Vec<String>) {
            match value {
                toml::Value::Table(table) => {
                    for (key, child) in table {
                        let path = if prefix.is_empty() {
                            key.clone()
                        } else {
                            format!("{prefix}.{key}")
                        };
                        leaf_keys(&path, child, out);
                    }
                }
                _ => out.push(prefix.to_string()),
            }
        }
        let value: toml::Value = toml::from_str(text).unwrap();
        let mut keys = Vec::new();
        leaf_keys("", &value, &mut keys);
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "appearance.blur",
                "appearance.date_format",
                "appearance.fade_in_ms",
                "appearance.font",
                "auth.pam_service",
                "auth.u2f.enabled",
                "auth.u2f.pam_service",
                "display.primary",
                "input.band_width",
                "input.pillar_fade_ms",
                "input.pillar_width",
                "keepassxc.lock_before",
                "keepassxc.unlock_after",
            ]
        );
    }
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test --lib example_config`
Expected: コンパイルエラー。

```text
error: couldn't read `src/../docs/config.example.toml`: No such file or directory (os error 2)
```

- [ ] **Step 3: `docs/config.example.toml` を書く**

```toml
# kabi-lock configuration.
#
# Copy this file to $XDG_CONFIG_HOME/kabi-lock/config.toml
# (usually ~/.config/kabi-lock/config.toml).
#
# Every key is optional. The values below are the defaults, so a file that
# contains exactly this text behaves the same as no file at all.
# A missing, unreadable or invalid file is not fatal: kabi-lock logs a warning
# and uses the defaults. Unknown keys are ignored with a warning.

[display]
# Which monitor shows the clock, date, input bar and padlock. The other
# monitors show only the blurred background.
#
# Candidates are checked from the top; the first one that matches a connected
# output wins. Each candidate is compared for exact equality with either
#   - the output name, e.g. "DP-10"  (see `swaymsg -t get_outputs`)
#   - the monitor description (wl_output description). sway writes it as
#     "make model serial (DP-10)", so the text without the trailing
#     " (DP-10)" is compared as well,
#     e.g. "LG Electronics LG ULTRAWIDE 311NTDV3Z418".
# If nothing matches, the output that was focused when the lock started (read
# from the sway IPC) is used, and if that is unknown, the first output.
#
# Example: primary = ["DP-10", "LG Electronics LG ULTRAWIDE 311NTDV3Z418"]
primary = []

[appearance]
# Time to fade from black to the blurred screenshot, in milliseconds.
# The clock and the other overlays fade in together.
fade_in_ms = 1500
# Blur strength of the background: the radius, in screenshot pixels, of a
# 5-pass box blur (the same look as swaylock-effects' --effect-blur 7x5, which
# is about a 9.7 px Gaussian sigma). Larger is blurrier. Values outside
# 0.5 to 50 are clamped; a negative or NaN value becomes 7.
blur = 7.0
# Date format under the clock, as a chrono (strftime-style) string.
# Month and weekday names follow the LC_TIME locale, for example "%a %m/%d".
# An invalid format falls back to "%m/%d".
date_format = "%m/%d"
# Font family for the clock and date: a fontconfig family name (see
# `fc-list : family`). Within that family the clock uses the weight nearest to
# 500 (Medium) and the date the weight nearest to 400 (Regular). If the family
# is not found, its files cannot be read, or fontconfig (`fc-list`) is not
# available, the Inter font built into kabi-lock is used. Which font was
# chosen (and why the fallback was used) is written to the log.
font = "Inter"

[input]
# How long one light pillar (drawn for each key press) takes to fade out, in ms.
pillar_fade_ms = 900
# Width of a pillar in pixels, on a 1920x1080 layout (scaled with the screen
# height on other sizes). Pillars are 16 px apart. Values above 1920 are
# clamped to 1920.
pillar_width = 32
# Width of the input bar that holds the pillars, in pixels (1920x1080 layout).
# Values above 1920 are clamped to 1920. The number of lanes is
# (band_width + 16) / (pillar_width + 16), rounded down, and at least 1.
# With the defaults that is 16 lanes.
band_width = 768

[auth]
# PAM service that checks the typed password. The package installs
# /etc/pam.d/kabi-lock.
pam_service = "kabi-lock"

[auth.u2f]
# Also accept a YubiKey touch while locked. Needs a PAM service for it
# (see contrib/pam.d/kabi-lock-u2f), which must contain pam_u2f only. If the
# service file exists neither in /etc/pam.d nor in /usr/lib/pam.d, the touch
# check is not started (a warning is logged) and only the password works.
# Set to false to turn this off.
enabled = true
# PAM service for the touch check. You may name one you already have,
# for example "u2f-touch".
pam_service = "kabi-lock-u2f"

[keepassxc]
# Ask KeePassXC to lock all databases before the screen locks (D-Bus
# org.keepassxc.KeePassXC.MainWindow.lockAllDatabases). Does nothing if
# KeePassXC is not running. Locking continues even if it fails.
lock_before = true
# After unlocking, ask KeePassXC (Secret Service) to unlock the databases
# that are locked, which makes its unlock dialog appear.
unlock_after = true
```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test --lib`
Expected: `example_config_matches_defaults_and_lists_every_key` を含め、全部通る（テスト数は Plan 2〜5 で増えている）。

`cargo fmt --check` と `cargo clippy --all-targets -- -D warnings` も実行して、警告・差分が出ないことを確認する。

テストが効くことの確認（任意。確認後は必ず元に戻す）:

```bash
cp docs/config.example.toml /tmp/config.example.toml.bak
sed -i 's/^blur = 7.0/blur = 8.0/' docs/config.example.toml
cargo test --lib example_config   # FAILED（初期値と違う）
cp /tmp/config.example.toml.bak docs/config.example.toml
cargo test --lib example_config   # ok
git status                        # config.example.toml が変更なしに戻っている
rm /tmp/config.example.toml.bak
```

- [ ] **Step 5: コミットする（ファイルごとに分ける）**

```bash
git status
git diff
git add docs/config.example.toml
git commit -m "docs: add example config listing every key with its default

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git add src/config.rs
git commit -m "test: check that the example config parses and matches the defaults

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: README

**Files:**
- Create: `README.md`

**Interfaces:**
- Consumes: `docs/design/images/lock-screen.png`、`docs/config.example.toml`、`contrib/` の 3 ファイル、`packaging/` のファイル（Task 1〜3）
- Produces: 英語の README（日本語の節つき）。パッケージが `/usr/share/doc/kabi-lock/README.md` に入れる

設計書 7 章の設定表と README の表の初期値が一致していること、契約 7 章の終了コードが README の表と一致していることを、書いた後で見比べる。

- [ ] **Step 1: `README.md` を書く**

`Assumption` として、README の次の記述は Plan 2〜5 の実装に依存している。実装を見て違えば直す: 終了コード（契約 7 章）、`RUST_LOG`（契約 6 章）、キー操作（設計書 6 章）、スクリーンショットをディスクに書かないこと（Plan 3）、`assets/fonts/` のライセンス文（Plan 4）。

`````markdown
# kabi-lock

A screen locker for [sway](https://swaywm.org/), written in Rust.
It fades from black into a blurred screenshot of your desktop, shows a clock, and draws a
light pillar for every key you type. You can unlock with your password or by touching a
YubiKey.

![kabi-lock screen](docs/design/images/lock-screen.png)

*Design render of the lock screen. The desktop behind it is drawn as blurred rectangles
instead of a real screenshot.*

日本語の説明は[下](#日本語)にあります。

## Features

- Locks the session with `ext-session-lock-v1`, on every monitor.
- Background is a screenshot taken just before locking, blurred, darkened at the edges, and
  faded in from black (1.5 s by default). It clears up again when you unlock.
- Clock, date, and a 16-lane input bar on one monitor. Each key press raises a light pillar
  in a random free lane. Backspace does too; Esc and Ctrl+U clear the input without one.
- A padlock appears on Enter: it opens on success and shakes on failure.
- Password check through PAM. A YubiKey touch (`pam_u2f`) also unlocks, at the same time as
  the password prompt.
- Optional KeePassXC support: lock the databases before the screen locks, and ask for an unlock
  dialog after you unlock.
- Plug and unplug monitors while locked; new monitors get a locked (black) surface.
- If the GPU cannot be used, it still locks with a plain black screen.
- TOML configuration. Every setting is optional.

## Requirements

- A Wayland compositor with `ext-session-lock-v1` and `wlr-screencopy-unstable-v1`.
  Developed and tested on sway (1.8 or newer has `ext-session-lock-v1`).
- A GPU driver with EGL and OpenGL ES 3 (for example Mesa). Without it you get the black fallback.
- PAM, libxkbcommon, libwayland.
- Optional: fontconfig (`fc-list`), to find the font set by `appearance.font`. Without it kabi-lock uses its built-in Inter.
- A `/etc/pam.d/kabi-lock` service file (the Arch package installs one).
- Optional: [`pam_u2f`](https://developers.yubico.com/pam-u2f/) for YubiKey unlock.
- Optional: KeePassXC with its Secret Service integration enabled, for the KeePassXC features.
- To build: Rust 1.95 or newer (required by egui 0.37), `git`, and `clang` (the `pam-sys` crate runs bindgen).

## Install

### Arch Linux

```sh
mkdir kabi-lock-build && cd kabi-lock-build
curl -O https://raw.githubusercontent.com/y1wtt/kabi-lock/main/packaging/arch/PKGBUILD
makepkg -si
```

This builds the latest `main` and installs the binary, `/etc/pam.d/kabi-lock`, the licenses,
the documentation, and examples under `/usr/share/kabi-lock/`.

### From source

```sh
git clone https://github.com/y1wtt/kabi-lock.git
cd kabi-lock
cargo build --release --locked
sudo install -Dm755 target/release/kabi-lock /usr/local/bin/kabi-lock
sudo install -Dm644 packaging/pam.d/kabi-lock /etc/pam.d/kabi-lock
```

The font (Inter) is built into the binary. The PAM file is required: without
`/etc/pam.d/kabi-lock` the password check cannot succeed. This matters for locking yourself
out, so create it before you test.

If `kabi-lock` is not in the `PATH` that sway uses, write the full path in the key binding.

## Usage

Add a key binding to your sway config (see `contrib/sway/kabi-lock.conf`):

```
bindsym $mod+l exec kabi-lock
```

Then press `$mod+l`. Type your password and press Enter, or touch your YubiKey.

| Key | Action |
|---|---|
| Enter | Check the password |
| Backspace | Delete one character |
| Esc, Ctrl+U | Clear the input |

Run `kabi-lock` with no arguments; the only options are `--help` and `--version`.
Logs go to stderr; the default level is `warn`, and `RUST_LOG=debug` shows more.
The messages never contain the password or its length.

| Exit code | Meaning |
|---|---|
| 0 | Unlocked, and exited normally |
| 1 | Could not lock (no `ext-session-lock-v1`, the request was refused, or no Wayland connection) |
| 2 | Invalid arguments |

To try it without locking your real session, start a nested sway
(`WLR_BACKENDS=wayland sway -c nested.conf`, with `exec kabi-lock` in `nested.conf`) and run
it there.

Idle and suspend integration is not part of kabi-lock. `kabi-lock` stays in the foreground
until you unlock, so do not run it from `swayidle -w` for `before-sleep`: that would hold the
sleep until you unlock.

## Configuration

Settings live in `$XDG_CONFIG_HOME/kabi-lock/config.toml`, or `~/.config/kabi-lock/config.toml`.
A full file with every key and its default is in
[`docs/config.example.toml`](docs/config.example.toml) (installed as
`/usr/share/kabi-lock/config.example.toml`).

All keys are optional. Unknown keys are ignored with a warning. If the file is missing,
unreadable, or invalid, kabi-lock logs a warning and uses the defaults; it always locks.

| Key | Default | Meaning |
|---|---|---|
| `display.primary` | `[]` | Monitors that get the clock, date, input bar and padlock, tried in order (see below) |
| `appearance.fade_in_ms` | `1500` | Fade from black to the blurred background, in ms |
| `appearance.blur` | `7` | Blur strength: radius of a 5-pass box blur in screenshot pixels (clamped to 0.5-50) |
| `appearance.date_format` | `"%m/%d"` | Date under the clock, chrono strftime format; names follow `LC_TIME` |
| `appearance.font` | `"Inter"` | Font family for the clock and date. Looked up with fontconfig (`fc-list`): the weight nearest 500 for the clock and 400 for the date. If the family is not found or unreadable, or fontconfig is missing, the built-in Inter is used |
| `input.pillar_fade_ms` | `900` | Lifetime of one light pillar, in ms |
| `input.pillar_width` | `32` | Pillar width in px (1920x1080 layout, at most 1920) |
| `input.band_width` | `768` | Input bar width in px (1920x1080 layout, at most 1920) |
| `auth.pam_service` | `"kabi-lock"` | PAM service for the password |
| `auth.u2f.enabled` | `true` | Also accept a YubiKey touch |
| `auth.u2f.pam_service` | `"kabi-lock-u2f"` | PAM service for the YubiKey |
| `keepassxc.lock_before` | `true` | Lock KeePassXC databases before locking the screen |
| `keepassxc.unlock_after` | `true` | Ask KeePassXC to unlock its databases after unlocking |

The number of pillar lanes is `(band_width + 16) / (pillar_width + 16)`, rounded down, and at least 1.
The defaults give 16 lanes. Pillars are 16 px apart.

### Choosing the monitor for the clock

`display.primary` is a list of names or descriptions. The first entry that equals a connected
monitor wins. An entry can be an output name such as `"DP-10"`, or the monitor description.
sway describes a monitor as `make model serial (DP-10)`; the text without the trailing
` (DP-10)` is accepted too. Run `swaymsg -t get_outputs` to see them.

```toml
[display]
primary = ["DP-10", "LG Electronics LG ULTRAWIDE 311NTDV3Z418"]
```

If nothing matches, the monitor that was focused when the lock started is used (from the sway IPC),
and if that is unknown, the first monitor. Other monitors show only the blurred background.

## Authentication

### Password

The password is checked by the PAM service `auth.pam_service`. The packaged
`/etc/pam.d/kabi-lock` includes your `login` stack:

```
auth       include      login
account    include      login
```

The `account` line is required, not decoration: after a successful password check kabi-lock
calls `pam_acct_mgmt`, and PAM fails that call (`PAM_PERM_DENIED`) for a service that has no
`account` lines, so with `auth` alone every correct password would be rejected.
If you replace the file, keep an `account` line.

So the lock screen follows your login policy, for example `pam_faillock`. Be careful with
repeated wrong passwords: if your login stack locks the account after a few failures, so will
kabi-lock. Every module in your `login` stack (such as `pam_u2f`, if your system uses it there)
takes part in the password check too. Edit `/etc/pam.d/kabi-lock` if you want a different policy;
pacman keeps your edits when you upgrade.

### YubiKey

While the screen is locked, kabi-lock checks the PAM service `auth.u2f.pam_service` in a helper
process (kabi-lock starts a copy of itself for this). If a try fails, for example because nothing
is touched, a new helper starts one second later. When a touch is accepted, the screen unlocks,
even while you are typing a password. The helper is stopped when the screen unlocks.

**Warning:** this service must contain `pam_u2f` only (plus a `pam_permit` `account` line).
kabi-lock starts a new check one second after each failed one until a key is touched, so a `pam_faillock`, a password
module (`pam_unix`) or an `include` of your login stack would count a failure every second and
can lock your account. Touch-only, no PIN.

The package does not install this service because the `authfile` path differs per user.
Use the example `contrib/pam.d/kabi-lock-u2f` (installed as
`/usr/share/kabi-lock/pam.d/kabi-lock-u2f`):

```sh
sudo cp /usr/share/kabi-lock/pam.d/kabi-lock-u2f /etc/pam.d/kabi-lock-u2f
# then edit authfile= to match where your keys are registered (see pamu2fcfg)
```

You can also set `auth.u2f.pam_service` to a service you already have, for example `"u2f-touch"`
(check that it is `pam_u2f` only, as above). If the service file exists neither in `/etc/pam.d`
nor in `/usr/lib/pam.d`, kabi-lock does not start the touch check and logs a warning.
Set `auth.u2f.enabled = false` to turn it off.

### KeePassXC

- Before the screen locks, kabi-lock calls `lockAllDatabases` on KeePassXC (D-Bus) and does not wait
  for it. It then checks the Secret Service for up to 2 seconds; if a collection is still unlocked,
  it shows a notification. The lock goes ahead either way. If KeePassXC is not running, nothing happens.
- After you unlock, it asks the Secret Service to unlock the collections that are locked, so
  KeePassXC shows its unlock dialog.

Turn these off with `keepassxc.lock_before = false` and `keepassxc.unlock_after = false`.

## Security notes

- kabi-lock is a personal project and has not been audited. Read this section before relying on it.
- **A YubiKey touch alone unlocks the screen** with the supplied PAM example (no PIN, no password).
  Anyone who can reach your desk while the key is plugged in can unlock it. Unplug the key when
  you leave, or set `auth.u2f.enabled = false`. (A PIN cannot be used here: kabi-lock answers
  every PAM prompt with a failure.)
- The lock is `ext-session-lock-v1`: sway itself keeps the session locked and shows a plain
  red screen if the locker dies. If kabi-lock crashes, start it again to unlock (for example from
  a TTY with `WAYLAND_DISPLAY` and `XDG_RUNTIME_DIR` set to your sway session).
- The screenshot is kept only in memory and is never written to disk. The blur is a visual effect,
  not redaction: with a small `blur` value large text may still be readable. Raise `appearance.blur`
  if that matters to you.
- Passwords are never logged. The password buffer is wiped after use, on a best-effort basis; the
  copy that PAM and its libraries hold is outside kabi-lock's control.
- kabi-lock does not stop switching to a text console (Ctrl+Alt+F2 and the like), which is
  governed by your system. Lock the console or disable VT switching if your threat model needs it.
- Wrong passwords go through your PAM stack and can lock your account (`pam_faillock`).
  The YubiKey service is tried again one second after each failure, so it must be `pam_u2f` only: a `pam_faillock`
  or password module there would pile up failures.
- `/etc/pam.d/kabi-lock` needs its `account` line: kabi-lock calls `pam_acct_mgmt` after the
  password check, and PAM fails that for a service without `account` rules (so unlocking would
  never succeed). If you replace the file, keep an `account` line.
- Locking continues even when the configuration is broken, the GPU cannot be used, or a
  screenshot fails; those only change how the screen looks.

## Development

```sh
cargo test                  # unit tests (no Wayland needed)
cargo build --release --locked
```

Do not test by locking your real session. Use a nested sway, as described under Usage.
Logs are controlled with `RUST_LOG`.

## License

kabi-lock is released under the [BSD 3-Clause License](LICENSE).
The [Inter](https://rsms.me/inter/) font built into the binary is licensed under the
SIL Open Font License 1.1 (`assets/fonts/LICENSE.txt`).

---

## 日本語

kabi-lock は sway 向けの画面ロッカーです。ロックすると画面が黒からぼかしたスクリーンショットへ
フェードインし、時計と日付、打鍵ごとに立ち上がる光の柱を表示します。パスワードでも YubiKey の
タッチでも解除できます。

- **必要なもの**: `ext-session-lock-v1` と `wlr-screencopy` が使える sway、EGL と OpenGL ES 3 の
  ドライバー、PAM。YubiKey は `pam_u2f`、KeePassXC 連携は KeePassXC（Secret Service を有効に）が
  あるときだけ使います。
- **インストール（Arch）**: `PKGBUILD` を取って `makepkg -si`（上の「Install」を参照）。
  `/etc/pam.d/kabi-lock` が入ります。ソースから入れるときは、同じファイルを自分で
  `/etc/pam.d/` に置いてください。ないとパスワードが通りません。
- **sway の設定**: `bindsym $mod+l exec kabi-lock`（`contrib/sway/kabi-lock.conf`）。
  sway の初期設定では `$mod+l` が「右へフォーカス」なので、重なる場合はどちらかを変えてください。
- **設定**: `~/.config/kabi-lock/config.toml`（すべて省略可）。全キーと初期値は
  `docs/config.example.toml`（インストール後は `/usr/share/kabi-lock/config.example.toml`）にあります。
  時計を出す画面は `display.primary` に出力名（`DP-10`）かモニターの説明文を並べて選びます。
- **YubiKey**: `authfile` の場所は人ごとに違うので、PAM の設定は自動では入りません。
  `contrib/pam.d/kabi-lock-u2f` を `/etc/pam.d/` にコピーして直すか、
  `auth.u2f.pam_service` に既存の設定（例 `u2f-touch`）を指定します。この PAM サービスは
  **`pam_u2f` だけ**にしてください。1 秒ごとに繰り返し呼ばれるので、`pam_faillock` やパスワードの
  モジュールがあると失敗が毎秒積み上がります（PIN なし、タッチだけ）。
- **注意**: 付属の例では YubiKey に触れるだけで解除されます（PIN なし）。鍵を挿したまま席を離れないでください。
  パスワードを間違えると、`login` の PAM に `pam_faillock` があればアカウントがロックされます。
  kabi-lock が異常終了しても sway はロックしたままなので、もう一度起動すれば解除できます。
- **ライセンス**: BSD-3-Clause。同梱の Inter フォントは SIL OFL 1.1。
`````

- [ ] **Step 2: 内容を確認する**

```bash
# 初期値が設定例（= Config::default()）と合っているか。表の値を目で見比べる:
grep -nE '^(primary|fade_in_ms|blur|date_format|font|pillar_fade_ms|pillar_width|band_width|pam_service|enabled|lock_before|unlock_after) =' docs/config.example.toml
# README が参照するファイルがすべてある（assets/fonts/ は Plan 4 の成果物）:
ls docs/design/images/lock-screen.png docs/config.example.toml contrib/sway/kabi-lock.conf contrib/pam.d/kabi-lock-u2f packaging/pam.d/kabi-lock LICENSE assets/fonts
# 相対リンクの確認:
grep -oE '\]\((docs|contrib|packaging|LICENSE|assets)[^)]*\)' README.md
```

Expected: 設定例の 13 のキー（`font`、2 つの `pam_service`、`enabled`・`lock_before`・`unlock_after` を含む）が README の表と同じ。`ls` がエラーなし。`README.md` 内の相対リンクの先がすべて存在する。

- [ ] **Step 3: コミットする**

```bash
git status
git diff
git add README.md
git commit -m "docs: add README with install, configuration and security notes

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: PKGBUILD

**Files:**
- Create: `packaging/arch/PKGBUILD`
- Create: `packaging/arch/.gitignore`
- Create: `packaging/arch/.SRCINFO`（生成）

**Interfaces:**
- Consumes: `Cargo.lock`、`LICENSE`、`assets/fonts/LICENSE.txt`（Plan 4、前提 2）、`README.md`、`docs/design/images/lock-screen.png`、`docs/config.example.toml`、`contrib/pam.d/kabi-lock-u2f`、`contrib/sway/kabi-lock.conf`、`packaging/pam.d/kabi-lock`
- Produces: パッケージ `kabi-lock-git`。入るもの:

| パス | 内容 |
|---|---|
| `/usr/bin/kabi-lock` | バイナリ |
| `/etc/pam.d/kabi-lock` | PAM サービス（`backup=` で、編集した内容を更新で上書きしない） |
| `/usr/share/licenses/kabi-lock-git/LICENSE` | BSD-3-Clause |
| `/usr/share/licenses/kabi-lock-git/LICENSE-Inter-OFL.txt` | Inter の SIL OFL 1.1 |
| `/usr/share/doc/kabi-lock/README.md`、`.../docs/design/images/lock-screen.png` | 文書とスクリーンショット（README の相対リンクが切れない配置） |
| `/usr/share/kabi-lock/config.example.toml`、`pam.d/kabi-lock-u2f`、`sway/kabi-lock.conf` | 例（有効なものは何もない） |

- [ ] **Step 1: `packaging/arch/PKGBUILD` を書く**

```bash
# Maintainer: yuga iwata <30215831+y1wtt@users.noreply.github.com>
# Builds the latest kabi-lock from the GitHub repository.

_pkgname=kabi-lock
pkgname=kabi-lock-git
pkgver=0.1.0.r0.g0000000
pkgrel=1
pkgdesc="Screen locker for sway: blurred screenshot background, fade-in, PAM and YubiKey unlock"
arch=('x86_64' 'aarch64')
url="https://github.com/y1wtt/kabi-lock"
license=('BSD-3-Clause')
depends=('fontconfig' 'gcc-libs' 'glibc' 'libglvnd' 'libxkbcommon' 'pam' 'wayland')
makedepends=('cargo' 'clang' 'git')
optdepends=('keepassxc: lock and unlock KeePassXC databases around the screen lock'
            'mesa: GPU drivers for the EGL/OpenGL ES rendering (or a proprietary driver)'
            'pam-u2f: unlock with a YubiKey touch')
provides=("$_pkgname")
conflicts=("$_pkgname")
backup=('etc/pam.d/kabi-lock')
options=('!lto')
source=("$_pkgname::git+$url.git")
sha256sums=('SKIP')

pkgver() {
	cd "$_pkgname"
	if git describe --long --tags --match 'v[0-9]*' >/dev/null 2>&1; then
		git describe --long --tags --match 'v[0-9]*' | sed 's/^v//; s/\([^-]*-g\)/r\1/; s/-/./g'
	else
		printf 'r%s.g%s' "$(git rev-list --count HEAD)" "$(git rev-parse --short=7 HEAD)"
	fi
}

prepare() {
	cd "$_pkgname"
	export RUSTUP_TOOLCHAIN=stable
	cargo fetch --locked --target "$(rustc -vV | sed -n 's/host: //p')"
}

build() {
	cd "$_pkgname"
	export RUSTUP_TOOLCHAIN=stable
	export CARGO_TARGET_DIR=target
	cargo build --frozen --release
}

check() {
	cd "$_pkgname"
	export RUSTUP_TOOLCHAIN=stable
	export CARGO_TARGET_DIR=target
	cargo test --frozen --release
}

package() {
	cd "$_pkgname"

	install -Dm755 target/release/kabi-lock "$pkgdir/usr/bin/kabi-lock"

	# PAM service for the password check (kept across upgrades, see backup=)
	install -Dm644 packaging/pam.d/kabi-lock "$pkgdir/etc/pam.d/kabi-lock"

	# kabi-lock is BSD-3-Clause; the embedded Inter font is SIL OFL 1.1
	install -Dm644 LICENSE "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
	install -Dm644 assets/fonts/LICENSE.txt "$pkgdir/usr/share/licenses/$pkgname/LICENSE-Inter-OFL.txt"

	install -Dm644 README.md "$pkgdir/usr/share/doc/$_pkgname/README.md"
	install -Dm644 docs/design/images/lock-screen.png \
		"$pkgdir/usr/share/doc/$_pkgname/docs/design/images/lock-screen.png"

	# examples: copy what you need; none of these are active
	install -Dm644 docs/config.example.toml "$pkgdir/usr/share/$_pkgname/config.example.toml"
	install -Dm644 contrib/pam.d/kabi-lock-u2f "$pkgdir/usr/share/$_pkgname/pam.d/kabi-lock-u2f"
	install -Dm644 contrib/sway/kabi-lock.conf "$pkgdir/usr/share/$_pkgname/sway/kabi-lock.conf"
}
```

インデントは既存の PKGBUILD と同じタブ。

- [ ] **Step 2: `packaging/arch/.gitignore` を書く**

`makepkg` を `packaging/arch/` の中で実行しても作業ファイルがコミットに混ざらないようにする（`.SRCINFO` と `PKGBUILD` は無視しない）。

```text
/src/
/pkg/
/kabi-lock/
*.pkg.tar.*
*.log
```

- [ ] **Step 3: 構文と lint を確認する**

```bash
bash -n packaging/arch/PKGBUILD && echo syntax-ok
command -v namcap && namcap packaging/arch/PKGBUILD
```

Expected: `syntax-ok`。`namcap` が入っているときは、PKGBUILD に対する警告・エラーが出ない（この計画を書いた時点では `namcap` は入っていなかった。入っていなければ 2 行目は何も出さない。パッケージ本体の `namcap` 検査は Task 6 で行う）。

- [ ] **Step 4: `.SRCINFO` を生成する**

```bash
cd packaging/arch
makepkg --printsrcinfo > .SRCINFO
cat .SRCINFO
cd ../..
```

`--printsrcinfo` はソースを取得しない。Expected（この計画を書いたときの出力）:

```text
pkgbase = kabi-lock-git
	pkgdesc = Screen locker for sway: blurred screenshot background, fade-in, PAM and YubiKey unlock
	pkgver = 0.1.0.r0.g0000000
	pkgrel = 1
	url = https://github.com/y1wtt/kabi-lock
	arch = x86_64
	arch = aarch64
	license = BSD-3-Clause
	makedepends = cargo
	makedepends = clang
	makedepends = git
	depends = fontconfig
	depends = gcc-libs
	depends = glibc
	depends = libglvnd
	depends = libxkbcommon
	depends = pam
	depends = wayland
	optdepends = keepassxc: lock and unlock KeePassXC databases around the screen lock
	optdepends = mesa: GPU drivers for the EGL/OpenGL ES rendering (or a proprietary driver)
	optdepends = pam-u2f: unlock with a YubiKey touch
	provides = kabi-lock
	conflicts = kabi-lock
	options = !lto
	backup = etc/pam.d/kabi-lock
	source = kabi-lock::git+https://github.com/y1wtt/kabi-lock.git
	sha256sums = SKIP

pkgname = kabi-lock-git
```

- [ ] **Step 5: `makepkg` の動作を `/tmp` のコピーで確かめる（Plan 2〜5 が終わっていなくてもできる部分）**

GitHub には現在のコミットがまだないかもしれないので、ソースをローカルのリポジトリに差し替えて確かめる。差し替えるのは `/tmp` のコピーだけで、リポジトリの PKGBUILD は変えない。依存パッケージの確認だけ `--nodeps` で省く（`pacman` の `sudo` を使わないため）。

```bash
rm -rf /tmp/kabi-pkgcheck && mkdir /tmp/kabi-pkgcheck
sed "s#^source=.*#source=(\"\$_pkgname::git+file://$PWD\")#" packaging/arch/PKGBUILD > /tmp/kabi-pkgcheck/PKGBUILD
cd /tmp/kabi-pkgcheck
makepkg --nodeps -f --noconfirm
bsdtar -tf kabi-lock-git-*-x86_64.pkg.tar.zst | grep -v '/$' | grep -v '^\.'
cd - >/dev/null
```

`file://` のクローンには**コミット済みの内容**しか入らない。Task 1〜4 の成果物がすべてコミットされていることが前提（されていないと `install` が失敗する）。

Expected: `check()` で `cargo test --frozen --release` が通り、`==> Finished making: kabi-lock-git r<コミット数>.g<ハッシュ>-1`（タグがあれば `0.1.0.r…` の形）と出る。ファイル一覧が次の 9 つ（`kabi-lock-git-debug` は別の空のパッケージで、無視してよい。`gdb-add-index` の `No debugging symbols` のメッセージも無視してよい）:

```text
etc/pam.d/kabi-lock
usr/bin/kabi-lock
usr/share/doc/kabi-lock/README.md
usr/share/doc/kabi-lock/docs/design/images/lock-screen.png
usr/share/kabi-lock/config.example.toml
usr/share/kabi-lock/pam.d/kabi-lock-u2f
usr/share/kabi-lock/sway/kabi-lock.conf
usr/share/licenses/kabi-lock-git/LICENSE
usr/share/licenses/kabi-lock-git/LICENSE-Inter-OFL.txt
```

- [ ] **Step 6: コミットする**

```bash
git status
git diff
git add packaging/arch/PKGBUILD
git commit -m "feat: add Arch PKGBUILD that builds from the GitHub repository

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git add packaging/arch/.gitignore
git commit -m "chore: ignore makepkg work files in packaging/arch

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git add packaging/arch/.SRCINFO
git commit -m "chore: add generated .SRCINFO for the PKGBUILD

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: 最終確認と、パッケージの手動ビルド・インストール

**Files:** なし（確認だけ）。タグ付けとインストールは人が行う。

**Interfaces:**
- Consumes: Plan 2〜5 の成果物と Task 1〜5
- Produces: 配布できる状態の確認

- [ ] **Step 1: リポジトリ全体の確認**

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
cargo build --release --locked
git status
git log --oneline | head -15
```

Expected: すべて成功し（clippy の警告なし）、作業ツリーがきれい。`target/release/kabi-lock` ができている。

- [ ] **Step 2: 配布物と文書の食い違いがないか見る**

```bash
bash -n packaging/arch/PKGBUILD
(cd packaging/arch && makepkg --printsrcinfo | diff - .SRCINFO && echo srcinfo-in-sync)
grep -n "assets/fonts" packaging/arch/PKGBUILD README.md
ls assets/fonts/
```

Expected: `srcinfo-in-sync`（PKGBUILD を直したら `.SRCINFO` も作り直す）。`assets/fonts/LICENSE.txt` の参照が `ls` に出たファイルと一致する。

- [ ] **Step 3: （手動）実際のパッケージをビルドして中身を見る**

Plan 2〜5 の成果が GitHub の `main` に push されていること（`source` が GitHub のため）。push の前は Task 5 の Step 5 と同じように `source` をローカルに差し替える。**`/tmp` のコピーで行い、リポジトリの `packaging/arch/` では `makepkg` を実行しない。**

```bash
rm -rf /tmp/kabi-pkg && mkdir /tmp/kabi-pkg
cp packaging/arch/PKGBUILD /tmp/kabi-pkg/
# push 前だけ: source をローカルに向ける
# sed -i "s#^source=.*#source=(\"\$_pkgname::git+file://$PWD\")#" /tmp/kabi-pkg/PKGBUILD
cd /tmp/kabi-pkg
makepkg -s          # 依存を入れるために sudo の確認が出る。人が応答する
bsdtar -tf kabi-lock-git-*-x86_64.pkg.tar.zst | grep -v '/$' | grep -v '^\.'
pacman -Qip kabi-lock-git-*-x86_64.pkg.tar.zst
command -v namcap && namcap kabi-lock-git-*-x86_64.pkg.tar.zst
```

Expected: ファイル一覧が Task 5 の Step 5 と同じ。`pacman -Qip` の `Depends On` に `pam`、`libxkbcommon` などが並ぶ。`namcap` があるときは、共有ライブラリの検出結果が `depends` と合っている（`namcap` が「依存が足りない／余っている」と言ったら、`depends` を直して Task 5 の `.SRCINFO` も更新する）。`namcap` が入っていなければ `sudo pacman -S namcap`（人が実行する）。

- [ ] **Step 4: （手動）インストールして確かめる — 人が実行する**

エージェントは `sudo` を実行しない。次は利用者が自分で行う。

まず、パッケージが入れるファイルがパッケージの外にすでに置かれていないかを見る（README の「From source」の手順などで置いた `/etc/pam.d/kabi-lock` があると、`pacman -U` が `exists in filesystem` で失敗する）:

```bash
ls -l /etc/pam.d/kabi-lock /usr/local/bin/kabi-lock
pacman -Qo /etc/pam.d/kabi-lock
```

- どちらも `No such file or directory` なら、そのまま `pacman -U` に進む。
- `/etc/pam.d/kabi-lock` があり、`pacman -Qo` が `error: No package owns /etc/pam.d/kabi-lock` を出したら、中身を見てから消す（自分で編集していた場合だけ、先に写しを取り、インストール後に `/etc/pam.d/kabi-lock` へ手で反映する）:

```bash
cat /etc/pam.d/kabi-lock
sudo cp /etc/pam.d/kabi-lock /root/kabi-lock.pam.before-package   # 自分で編集していた場合だけ
sudo rm /etc/pam.d/kabi-lock
```

- `/usr/local/bin/kabi-lock`（ソースから入れたもの）があれば `sudo rm /usr/local/bin/kabi-lock` で消す。`PATH` で `/usr/bin` より先に見つかり、sway の `exec kabi-lock` が古いバイナリを動かしてしまうため。

```bash
sudo pacman -U /tmp/kabi-pkg/kabi-lock-git-*-x86_64.pkg.tar.zst
```

- `conflicts=kabi-lock` は、将来の固定タグ版パッケージとの同時インストールを防ぐためのもの。
- **`/etc/pam.d/kabi-lock` が入っていることを確認する。** ないとパスワードが通らず、解除できなくなるおそれがある。

```bash
ls -l /etc/pam.d/kabi-lock /usr/bin/kabi-lock /usr/share/kabi-lock/
pacman -Ql kabi-lock-git | head -20
```

確認の順番（本物のセッションをロックしない）:

1. 契約 9 章の入れ子の sway で、インストールしたバイナリ（`/usr/bin/kabi-lock`）を起動する。
2. 自分のパスワードを**1 回だけ**入れて解除できることを確認する（失敗を繰り返さない。`pam_faillock` でアカウントがロックされるおそれがある）。
3. `~/.config/kabi-lock/config.toml` に `/usr/share/kabi-lock/config.example.toml` をコピーして、起動時に警告が出ない（`RUST_LOG=warn`）ことを確認する。
4. YubiKey を使うなら、`/usr/share/kabi-lock/pam.d/kabi-lock-u2f` を `/etc/pam.d/kabi-lock-u2f` にコピーして `authfile` を直し、入れ子の sway でタッチ解除を確認する。
5. 最後に、実機の本物のセッションで **一度だけ**（設計書 9 章）、3 枚の出力（DP-10 / DP-8 / eDP-1）をつないだ状態で `$mod+l`（Task 2 の割り当て）を試す。README の Security notes にある異常終了時の手順が使えるよう、先に別の TTY にログインしておく。1 回のロックの中で次を見る:
   - 3 枚とも、黒から約 1.5 秒でぼかした背景へなめらかにフェードインする（赤い画面や、ロック前のデスクトップが一瞬見えることがない）。
   - 3 枚ともぼかしとビネットが掛かり、スケールの違う出力（eDP-1 など）でも背景がずれたりぼやけたりしない。
   - 時計と日付は主画面（`display.primary` の規則、なければロック前にフォーカスしていた出力）の 1 枚だけに出て、ほかの 2 枚は背景だけ。
   - 数文字打つと、主画面の帯に打鍵ごとに光の柱が出る。打った文字は Esc で消す（Enter は押さない）。
   - YubiKey を設定してある（手順 4）なら、YubiKey に触れて解除する: 錠が現れて開き（U 字が跳ね上がって回る）、ぼかしが晴れてロックが外れる。KeePassXC 連携を使っているなら、解除の直後に KeePassXC の解除ダイアログが出て、**最初の**タッチがダイアログ側に効く（kabi-lock の helper がタッチを横取りしない）。
   - YubiKey を設定していなければ、自分のパスワードを 1 回だけ正しく入れて Enter: 錠が現れて開き、ぼかしが晴れてロックが外れる。
   - 錠の震え（間違ったパスワード）は Plan 4 Task 15 で確かめたので、ここでは確かめない。間違ったパスワードを送らない（faillock。Plan 2〜6 の手動確認全体で 1 回だけ、という決まり）。

- [ ] **Step 5: （任意・人が判断する）リリースタグ**

動作確認が済んだら、タグを打つと `pkgver` が `0.1.0.r0.g<ハッシュ>` の形になる。

```bash
git tag -a v0.1.0 -m "kabi-lock 0.1.0"
# push は利用者が自分で: git push origin main v0.1.0
```

タグを打ってもこの計画のファイルは変わらない（`pkgver` は `makepkg` が実行時に決める）。

---

## 設計書の要件との対応

| 設計書の要件 | この計画 |
|---|---|
| 2 章（v1 に含める）「Arch 用 PKGBUILD」 | Task 5 |
| 6 章「PAM 設定ファイル」: `/etc/pam.d/kabi-lock`（`auth include login` と `account include login`）をパッケージで入れる | Task 1、Task 5（`/etc/pam.d/kabi-lock`、`backup=`） |
| 6 章: U2F 用は入れず `contrib/pam.d/kabi-lock-u2f` に例を置く。既存の `u2f-touch` を `auth.u2f.pam_service` で指定してもよい | Task 1（例）、Task 4（README の YubiKey の節）、Task 3（設定例のコメント） |
| 2 章「v1 に含めない」: swayidle 連携は sway 設定から `exec kabi-lock` で呼ぶ想定 | Task 2（キー割り当て）、Task 4（README に `swayidle -w` の注意） |
| 7 章「設定ファイル」: 場所、全キーと初期値、省略可、未知のキーは警告 | Task 3（設定例とテスト）、Task 4（README の設定表） |
| 7 章「フォント」: Inter（OFL）のライセンス文を同梱する | Task 5（`LICENSE-Inter-OFL.txt`）。ライセンス文自体は Plan 4 が `assets/fonts/` に置く |
| 8 章「エラー時の動き」（ロックは必ず掛ける、異常終了時は再起動で解除） | Task 4（README の Security notes） |
| 10 章「配布」: BSD-3-Clause、リポジトリ URL、`packaging/arch/PKGBUILD`、`/etc/pam.d/kabi-lock` | Task 5（`license=('BSD-3-Clause')`、`url`、`source`）、Task 4（License の節） |
| 9 章「テスト」の手動確認（入れ子の sway と、実機の 3 枚の出力 DP-10 / DP-8 / eDP-1 で一度だけ） | Task 6 の Step 4（手順 1〜4 が入れ子の sway、手順 5 が実機） |
| 契約 4 章「Plan 6 が作るもの」6 ファイル | PAM（Task 1）、U2F の例（Task 1）、PKGBUILD（Task 5）、README（Task 4）、sway の例（Task 2）、設定例（Task 3） |

## 契約・他の計画への指摘

1. **`auth include login` だけでは `pam_acct_mgmt` が失敗する**（Task 1 の Step 4 で確認済み: 戻り値 6）。Plan 2 の `pam_check` は `acct_mgmt` を呼ぶ（`NEW_AUTHTOK_REQD` だけを通す）ので、`account include login` は必須。設計書 6 章と契約はすでに 2 行の形に更新されている。Plan 2〜5 の手動確認は、`/etc/pam.d/kabi-lock` がまだないので、一時的な設定の `pam_service = "login"` で行う（`/etc/pam.d/swaylock` は `account` の行がないので使えない）。
2. **`clang` が `makedepends` に要る**（`pam-sys` の bindgen）。契約の依存表にない。
3. Inter のライセンス文は Plan 4 が `assets/fonts/LICENSE.txt` に置く。PKGBUILD はそれを `/usr/share/licenses/kabi-lock-git/LICENSE-Inter-OFL.txt` として入れる（Task 1 の Step 1 で存在を確認する）。
4. `pam_u2f` の PAM サービスは、U2F の helper（子プロセス）の会話関数がすべてのプロンプトに失敗を返す（設計書 6 章、Plan 5）ため、PIN 付き（`pinverification=1`）の構成にはできない。README にそう書いた。
5. **Plan 4 が設定キー `appearance.font`（初期値 `"Inter"`、fontconfig のファミリー名）を足した**（設計書 7 章も更新済み）。設定例・テスト・README の表に入れ、キーは全部で 13 になった。それ以外に新しいキーはない。Plan 3 は `blur` の範囲を丸める（0.5〜50）、Plan 4 は `band_width`・`pillar_width` の上限（1920）と不正な `date_format` の戻しを足すだけで、設定例のコメントにその内容を反映した。Plan 3 の `blur` の意味（半径 `blur` の箱型ぼかし 5 回）は設計書 7 章のコメント（「縮小後のガウスの σ 相当」）と違うので、設定例と README は Plan 3 に合わせた。
6. Plan 5 は U2F を子プロセス（隠し引数 `--u2f-helper <pam-service>`）で動かす。README の引数の説明は `--help` と `--version` だけにして、隠し引数は書かない。
7. kabi-lock は解除までフォアグラウンドで動き続けるので、`swayidle -w ... before-sleep 'kabi-lock'` は解除するまでスリープを止める。v1 の範囲外（設計書 2 章）だが README に注意を書いた。

## 未対応の引き継ぎ

レビューで見つかったが、v1 の計画には入れなかったもの。急がないが、失わないようにここに残す。

- 設定: `XDG_CONFIG_HOME` が相対パスのときに無視していない（XDG Base Directory の仕様では無視する）。
- 設定: TOML の複数行のエラーを 1 つの `warn!` に入れているので、ログが読みにくい。
- テストの追加:
  - `blur = 7`（整数）を読めること。
  - `shackle_path` を 0〜800ms で掃引して、跳びがないこと。
  - 主画面の規則で、先に書いた規則が勝つ（`earlier_rule_wins`）順序。
  - `PasswordBuffer` の `MAX_LEN` を多バイト文字で埋めたとき。
  - すべてのレーンが使用中のときのレーンの期限切れ。
  - 状態機械が無視するイベント（その状態で意味のないイベント）。
  - `config::load()` のファイルの I/O（存在しない・読めない）。
- `Machine`・`LaneAllocator`・`Loaded` に `Debug` を付ける。
- `PasswordBuffer` の `bool` を返すメソッドに `#[must_use]` を付ける。
- `lane_x` が負になりうることを doc コメントに書く。
- `shackle_path` に doc コメントを書く。
