# Plan 1 からの引き継ぎ事項

Plan 1（コアロジック）の最終レビュー（d437e89..7740dfd）で出た指摘のうち、Plan 2〜6 で扱うもの。
Plan 1 自体は「マージ可」の判定で、Critical / Important の指摘はなし。

## 早めにやる（Plan 2 か 4 の冒頭）

- `lanes::lane_count` / `lane_x` の `band_width + GAP`・`pillar_width + GAP` は `u32` の素の足し算。
  設定に巨大な値が入ると debug で panic、release で桁あふれ。`saturating_add` にするか、設定読み込み時に上限で丸める。
- 経過時間の計算を 1 か所にまとめる。`Phase` は `since_ms: u64`、`anim` は経過 `f32` を取るので、
  `Phase::elapsed_ms(now) -> Option<f32>`（`saturating_sub` を使う）のような補助を足す。
- `primary = [""]` のような空のルールは無視する（description が空の出力に一致してしまう）。

## Plan 2（Wayland・PAM）で必ず守る

- **確認中（Verifying）にはタイムアウトがない。** 認証スレッドは PAM エラーやパニックの場合も含め、
  必ず `PasswordChecked` を送る（失敗を送るドロップガード、または `catch_unwind`）。パニック時の方針は明示的に決める。
- 時刻は、ロック開始時の単調増加する `Instant` からのミリ秒（`u64`）を `now_ms` にする。
  Wayland のキーやフレームのタイムスタンプ（`u32`、桁あふれあり、基準が違う）は使わない。
- `Tick` はフレームコールバックだけでなく calloop のタイマーからも送る。
  画面が消えている（DPMS オフ）出力にはフレームコールバックが来ないため、解除中のまま止まるのを防ぐ。
- キー入力は、Enter・Backspace・Esc・Ctrl+U・Tab をキーシムで先に処理し、
  空文字や制御文字（`"\r"`、`"\x1b"`、`"\x15"` など）はパスワードに入れない。
  光の柱は `push_str` が `true` を返したときだけ出す。xkbcommon が返す一時的な `String` もゼロで消す。
- `take()` は `handle()` が `Action::CheckPassword` を返してから呼び、`Zeroizing<String>` のままチャンネルへ move する。
  pam-client の会話関数が返す `CString` が、解放時にゼロで消されるかを確認する。
- `#![forbid(unsafe_code)]` はクレート全体ではなく、コアのモジュールごとに付ける（Plan 3 の EGL・glow で `unsafe` が要るため）。
- `src/lib.rs` の doc コメントにある「Plan 1 では」を、`wl/` を足すときに直す。

## Plan 3（EGL・背景）

- `appearance.blur` は検証していない（負・NaN・無限大）。読み込み時か描画時に範囲を丸める。
- フェード中に U2F で解除されることがある。重ね物の不透明度は `fade_alpha * (1 - reveal_progress)`、
  背景シェーダーはフェードとぼかし解除を同時に扱う。
- `ReleaseLock` で外す前に、`reveal_progress = 1.0` のフレームを 1 枚描いてからロックを外す。

## Plan 4（egui・見た目）

- `appearance.date_format` は設定読み込み時に chrono の `StrftimeItems` で検証し、不正なら `%m/%d` に戻す（不正な書式は `to_string()` で panic する）。
- レーン数は拡大前の設定値（1920x1080 基準）で決め、位置だけを出力の高さに合わせて拡大する（主画面によってレーン数が変わらないように）。
- `LaneAllocator` と `pillar_at` には同じ `pillar_fade_ms` を渡す。
- 錠の本体の形（角丸 6、アイコン座標で y 24〜52）を `anim` の定数として公開し、描画側で数字を重複させない。

## 手が空いたとき（どれも非破壊・優先度低）

- テストの追加: `shackle_path` を 0〜800ms で掃引し、全点が有限で短い足の x が [22, 62] に収まること／
  `blur = 7`（整数）を `f32` として読めること／`earlier_rule_wins` をルール順と出力順で区別できる形にする
  （`["DP-10", "DP-8"]` → `Some(2)`）／description に接尾辞がない場合や名前の衝突／
  `MAX_LEN` をまたぐ多バイト文字／レーンの順序と、全レーン使用中の期限切れ／状態遷移で無視されるイベント／`load()` の入出力。
- `Machine`・`LaneAllocator`・`Loaded` に `Debug` を足す（`PasswordBuffer` には付けない）。
- `PasswordBuffer` の `bool` を返すメソッドに `#[must_use]`。
- `lane_x` が負になりうること（帯が柱より狭い場合）を doc に書く。
- 相対パスの `XDG_CONFIG_HOME` を無視する（XDG の仕様どおり）。
- 複数行の TOML エラーを 1 つの `warn!` に入れない。
- `shackle_path` に doc コメント。
