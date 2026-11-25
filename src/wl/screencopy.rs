//! ロック前のスクリーンショット（wlr-screencopy）。
//!
//! メインの接続とは別の Wayland 接続を別スレッドで開き、全出力を同時に撮る。
//! 返す画像は「論理的な向き・1 行目が画面の上」にそろえてある
//! （出力の transform と、フレームの Y_INVERT をここで戻す）。
#![forbid(unsafe_code)]

use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use log::{debug, warn};
use smithay_client_toolkit::reexports::calloop::EventLoop;
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::registry::SimpleGlobal;
use smithay_client_toolkit::shm::raw::RawPool;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_buffer, wl_output, wl_registry, wl_shm};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};

/// 撮影全体の締め切り。過ぎたら撮れた分だけ返す（ロックを遅らせないため）。
const CAPTURE_TIMEOUT: Duration = Duration::from_millis(1000);

/// 1 出力分のスクリーンショット。
///
/// `pixels` の 1 行目が画面の上。1 画素 4 バイトで、並びは `format` のとおり
/// （`Xrgb8888` / `Argb8888` ならメモリ上は B, G, R, X）。`stride` は 1 行のバイト数。
/// `width` / `height` はバッファの画素数（スケール 2 の出力なら論理サイズの 2 倍）。
pub struct Screenshot {
    pub output_name: String,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: wl_shm::Format,
    pub pixels: Vec<u8>,
}

/// 別スレッドで新しい Wayland 接続を開き、全出力を撮る。出力ごとに失敗しうる。
///
/// 失敗した出力は結果に入らない（呼び出し側はその出力の背景を黒にする）。
/// スレッドを起動できなければ警告して `None`（スクリーンショットなしで、全出力の背景が黒）。
pub fn spawn_capture_all() -> Option<JoinHandle<Vec<Screenshot>>> {
    let spawned = std::thread::Builder::new()
        .name("kabi-capture".into())
        .spawn(|| {
            let started = Instant::now();
            match capture_all() {
                Ok(shots) => {
                    debug!(
                        "captured {} screenshot(s) in {:?}",
                        shots.len(),
                        started.elapsed()
                    );
                    shots
                }
                Err(e) => {
                    warn!("screenshots unavailable, background will be black: {e}");
                    Vec::new()
                }
            }
        });
    match spawned {
        Ok(handle) => Some(handle),
        Err(e) => {
            warn!("cannot start the screenshot thread, background will be black: {e}");
            None
        }
    }
}

/// 32 ビット / 画素で、背景の描画側（`render::background`）が扱える形式か。
pub(crate) fn is_supported(format: wl_shm::Format) -> bool {
    use wl_shm::Format as F;
    matches!(
        format,
        F::Xrgb8888
            | F::Argb8888
            | F::Xbgr8888
            | F::Abgr8888
            | F::Xrgb2101010
            | F::Argb2101010
            | F::Xbgr2101010
            | F::Abgr2101010
    )
}

/// コンポジタが `buffer` で伝えた大きさの shm バッファのバイト数。使えない大きさなら `None`。
///
/// 幅・高さが 0、stride が 1 行（幅 × 4 バイト）より短い、`stride × height` が i32 を超える
/// （wl_shm のプールの大きさは i32）なら使えない。値はコンポジタから来るので、掛け算は u64 で行う。
/// `Some` なら幅・高さ・stride はどれも i32 に収まる。
pub(crate) fn shm_buffer_len(width: u32, height: u32, stride: u32) -> Option<usize> {
    if width == 0 || height == 0 || u64::from(stride) < u64::from(width) * 4 {
        return None;
    }
    let len = u64::from(stride) * u64::from(height);
    if len > i32::MAX as u64 {
        return None;
    }
    usize::try_from(len).ok()
}

/// 撮った 1 画素 4 バイトの画像を、論理的な向き（1 行目が画面の上）に直す。
///
/// screencopy の画像は出力の物理的な向きのままなので、transform の逆をかける。
/// 90 は時計回りに 90°、270 は 270° 回して戻す。flipped の 4 つは左右反転してから
/// 回すが、反転を挟むと回す向きが逆になるので flipped-90 は 270°、flipped-270 は 90° 回す
/// （grim の出力と画素単位で一致することを headless の sway で確かめた）。
/// `y_invert` は回転の前に上下を返す。
/// 返り値は（幅, 高さ, stride, 画素）。向きが変わらなければ元の stride を保つ。
pub(crate) fn normalize(
    src: &[u8],
    width: u32,
    height: u32,
    stride: u32,
    transform: wl_output::Transform,
    y_invert: bool,
) -> (u32, u32, u32, Vec<u8>) {
    use wl_output::Transform as T;
    let (w, h, s) = (width as usize, height as usize, stride as usize);
    let row = w * 4;

    if transform == T::Normal {
        if !y_invert {
            return (width, height, stride, src[..s * h].to_vec());
        }
        let mut out = vec![0u8; s * h];
        for y in 0..h {
            let from = (h - 1 - y) * s;
            out[y * s..y * s + row].copy_from_slice(&src[from..from + row]);
        }
        return (width, height, stride, out);
    }

    let flipped = matches!(
        transform,
        T::Flipped | T::Flipped90 | T::Flipped180 | T::Flipped270
    );
    let quarter_turns = match transform {
        T::_90 | T::Flipped270 => 1,
        T::_180 | T::Flipped180 => 2,
        T::_270 | T::Flipped90 => 3,
        _ => 0,
    };
    let (lw, lh) = if quarter_turns % 2 == 1 {
        (h, w)
    } else {
        (w, h)
    };
    let mut out = vec![0u8; lw * lh * 4];
    for by in 0..h {
        for bx in 0..w {
            let fx = if flipped { w - 1 - bx } else { bx };
            let fy = if y_invert { h - 1 - by } else { by };
            // 画面の座標（y は下向き）で時計回りに 90° ずつ回す
            let (lx, ly) = match quarter_turns {
                1 => (h - 1 - fy, fx),
                2 => (w - 1 - fx, h - 1 - fy),
                3 => (fy, w - 1 - fx),
                _ => (fx, fy),
            };
            let from = by * s + bx * 4;
            let to = (ly * lw + lx) * 4;
            out[to..to + 4].copy_from_slice(&src[from..from + 4]);
        }
    }
    (lw as u32, lh as u32, (lw * 4) as u32, out)
}

// ---- ここから Wayland の処理（撮影スレッドの中だけで使う） ----

struct OutputSlot {
    output: wl_output::WlOutput,
    name: Option<String>,
    transform: wl_output::Transform,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FrameState {
    Waiting,
    Ready,
    Failed,
}

struct FrameSlot {
    output: usize,
    frame: ZwlrScreencopyFrameV1,
    /// (format, width, height, stride)
    info: Option<(wl_shm::Format, u32, u32, u32)>,
    pool: Option<RawPool>,
    buffer: Option<wl_buffer::WlBuffer>,
    y_invert: bool,
    state: FrameState,
}

struct Capture {
    shm: SimpleGlobal<wl_shm::WlShm, 1>,
    outputs: Vec<OutputSlot>,
    frames: Vec<FrameSlot>,
}

fn capture_all() -> Result<Vec<Screenshot>, String> {
    let conn = Connection::connect_to_env().map_err(|e| format!("connect: {e}"))?;
    let (globals, mut queue) =
        registry_queue_init::<Capture>(&conn).map_err(|e| format!("registry: {e}"))?;
    let qh = queue.handle();
    // バージョン 3 の linux_dmabuf / buffer_done は使わないので 2 までにする
    let manager: ZwlrScreencopyManagerV1 = globals
        .bind(&qh, 1..=2, ())
        .map_err(|e| format!("zwlr_screencopy_manager_v1: {e}"))?;
    let shm: wl_shm::WlShm = globals
        .bind(&qh, 1..=1, ())
        .map_err(|e| format!("wl_shm: {e}"))?;

    // 出力の名前（wl_output v4 の name）と transform を集める
    let output_globals: Vec<(u32, u32)> = globals.contents().with_list(|list| {
        list.iter()
            .filter(|g| g.interface == "wl_output")
            .map(|g| (g.name, g.version))
            .collect()
    });
    let mut state = Capture {
        shm: SimpleGlobal::from_bound(shm),
        outputs: Vec::new(),
        frames: Vec::new(),
    };
    for (index, (name, version)) in output_globals.into_iter().enumerate() {
        let output: wl_output::WlOutput = globals.registry().bind(name, version.min(4), &qh, index);
        state.outputs.push(OutputSlot {
            output,
            name: None,
            transform: wl_output::Transform::Normal,
        });
    }
    queue
        .roundtrip(&mut state)
        .map_err(|e| format!("roundtrip: {e}"))?;

    for index in 0..state.outputs.len() {
        if state.outputs[index].name.is_none() {
            warn!("an output has no name (wl_output < v4), skipping its screenshot");
            continue;
        }
        let frame =
            manager.capture_output(0, &state.outputs[index].output, &qh, state.frames.len());
        state.frames.push(FrameSlot {
            output: index,
            frame,
            info: None,
            pool: None,
            buffer: None,
            y_invert: false,
            state: FrameState::Waiting,
        });
    }

    // 締め切りつきでイベントを回す（コンポジターが返事をしなくても止まらない）
    let mut event_loop: EventLoop<Capture> =
        EventLoop::try_new().map_err(|e| format!("event loop: {e}"))?;
    WaylandSource::new(conn.clone(), queue)
        .insert(event_loop.handle())
        .map_err(|e| format!("wayland source: {e}"))?;
    let deadline = Instant::now() + CAPTURE_TIMEOUT;
    while state.frames.iter().any(|f| f.state == FrameState::Waiting) {
        let now = Instant::now();
        if now >= deadline {
            warn!("screenshot timed out, missing outputs will be black");
            break;
        }
        event_loop
            .dispatch(deadline - now, &mut state)
            .map_err(|e| format!("dispatch: {e}"))?;
    }

    let mut shots = Vec::new();
    for slot in &mut state.frames {
        let output = &state.outputs[slot.output];
        let name = output.name.clone().unwrap_or_default();
        if let (FrameState::Ready, Some((format, width, height, stride)), Some(pool)) =
            (slot.state, slot.info, slot.pool.as_mut())
        {
            let len = stride as usize * height as usize;
            let (width, height, stride, pixels) = normalize(
                &pool.mmap()[..len],
                width,
                height,
                stride,
                output.transform,
                slot.y_invert,
            );
            debug!("screenshot of {name}: {width}x{height} {format:?}");
            shots.push(Screenshot {
                output_name: name,
                width,
                height,
                stride,
                format,
                pixels,
            });
        } else if slot.state == FrameState::Failed {
            warn!("screenshot of {name} failed, its background will be black");
        }
        if let Some(buffer) = slot.buffer.take() {
            buffer.destroy();
        }
        slot.frame.destroy();
    }
    manager.destroy();
    let _ = conn.flush();
    Ok(shots)
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Capture {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // 撮影中の出力の増減は扱わない
    }
}

impl Dispatch<wl_output::WlOutput, usize> for Capture {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let slot = &mut state.outputs[*index];
        match event {
            wl_output::Event::Name { name } => slot.name = Some(name),
            wl_output::Event::Geometry {
                transform: WEnum::Value(t),
                ..
            } => slot.transform = t,
            _ => {}
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, usize> for Capture {
    fn event(
        state: &mut Self,
        frame: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        index: &usize,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let output_name = state.outputs[state.frames[*index].output]
            .name
            .clone()
            .unwrap_or_default();
        let slot = &mut state.frames[*index];
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                if slot.buffer.is_some() {
                    return;
                }
                let format = match format {
                    WEnum::Value(f) if is_supported(f) => f,
                    other => {
                        warn!(
                            "screenshot of {output_name}: unsupported format {other:?}, using black"
                        );
                        slot.state = FrameState::Failed;
                        return;
                    }
                };
                let Some(len) = shm_buffer_len(width, height, stride) else {
                    warn!(
                        "screenshot of {output_name}: bad buffer {width}x{height} stride {stride}"
                    );
                    slot.state = FrameState::Failed;
                    return;
                };
                let mut pool = match RawPool::new(len, &state.shm) {
                    Ok(pool) => pool,
                    Err(e) => {
                        warn!("screenshot of {output_name}: shm pool: {e}");
                        slot.state = FrameState::Failed;
                        return;
                    }
                };
                // shm_buffer_len が i32 に収まることを確かめている
                let buffer = pool.create_buffer(
                    0,
                    width as i32,
                    height as i32,
                    stride as i32,
                    format,
                    (),
                    qh,
                );
                frame.copy(&buffer);
                slot.info = Some((format, width, height, stride));
                slot.pool = Some(pool);
                slot.buffer = Some(buffer);
            }
            zwlr_screencopy_frame_v1::Event::Flags {
                flags: WEnum::Value(flags),
            } => {
                slot.y_invert = flags.contains(zwlr_screencopy_frame_v1::Flags::YInvert);
            }
            zwlr_screencopy_frame_v1::Event::Ready { .. } => slot.state = FrameState::Ready,
            zwlr_screencopy_frame_v1::Event::Failed => slot.state = FrameState::Failed,
            _ => {}
        }
    }
}

wayland_client::delegate_noop!(Capture: ZwlrScreencopyManagerV1);
wayland_client::delegate_noop!(Capture: ignore wl_shm::WlShm);
wayland_client::delegate_noop!(Capture: ignore wl_buffer::WlBuffer);

#[cfg(test)]
mod tests {
    use super::*;
    use wl_output::Transform as T;

    /// 幅 3・高さ 2 の画像。画素の 1 バイト目に番号を入れる（a=1 … f=6）。
    ///
    /// ```text
    /// a b c
    /// d e f
    /// ```
    fn image(stride: u32) -> Vec<u8> {
        let mut v = vec![0u8; stride as usize * 2];
        for (i, n) in (1u8..=6).enumerate() {
            let (x, y) = (i % 3, i / 3);
            v[y * stride as usize + x * 4] = n;
        }
        v
    }

    /// 結果の画素番号を行ごとに並べる。
    fn ids(w: u32, h: u32, stride: u32, px: &[u8]) -> Vec<Vec<u8>> {
        (0..h as usize)
            .map(|y| {
                (0..w as usize)
                    .map(|x| px[y * stride as usize + x * 4])
                    .collect()
            })
            .collect()
    }

    fn run(transform: T, y_invert: bool) -> (u32, u32, Vec<Vec<u8>>) {
        let (w, h, s, px) = normalize(&image(12), 3, 2, 12, transform, y_invert);
        (w, h, ids(w, h, s, &px))
    }

    #[test]
    fn normal_keeps_pixels_and_stride() {
        let (w, h, s, px) = normalize(&image(16), 3, 2, 16, T::Normal, false);
        assert_eq!((w, h, s), (3, 2, 16));
        assert_eq!(ids(w, h, s, &px), vec![vec![1, 2, 3], vec![4, 5, 6]]);
    }

    #[test]
    fn y_invert_flips_rows() {
        assert_eq!(
            run(T::Normal, true),
            (3, 2, vec![vec![4, 5, 6], vec![1, 2, 3]])
        );
    }

    #[test]
    fn rotate_90_turns_clockwise() {
        // 上の行 a b c が右の列になる
        assert_eq!(
            run(T::_90, false),
            (2, 3, vec![vec![4, 1], vec![5, 2], vec![6, 3]])
        );
    }

    #[test]
    fn rotate_180() {
        assert_eq!(
            run(T::_180, false),
            (3, 2, vec![vec![6, 5, 4], vec![3, 2, 1]])
        );
    }

    #[test]
    fn rotate_270_turns_counter_clockwise() {
        assert_eq!(
            run(T::_270, false),
            (2, 3, vec![vec![3, 6], vec![2, 5], vec![1, 4]])
        );
    }

    #[test]
    fn flipped_mirrors_horizontally() {
        assert_eq!(
            run(T::Flipped, false),
            (3, 2, vec![vec![3, 2, 1], vec![6, 5, 4]])
        );
    }

    #[test]
    fn flipped_90_is_a_transpose() {
        assert_eq!(
            run(T::Flipped90, false),
            (2, 3, vec![vec![1, 4], vec![2, 5], vec![3, 6]])
        );
    }

    #[test]
    fn flipped_270_is_an_anti_transpose() {
        assert_eq!(
            run(T::Flipped270, false),
            (2, 3, vec![vec![6, 3], vec![5, 2], vec![4, 1]])
        );
    }

    #[test]
    fn y_invert_applies_before_rotation() {
        assert_eq!(
            run(T::_90, true),
            (2, 3, vec![vec![1, 4], vec![2, 5], vec![3, 6]])
        );
    }

    #[test]
    fn buffer_len_accepts_normal_sizes() {
        assert_eq!(shm_buffer_len(3440, 1440, 3440 * 4), Some(3440 * 4 * 1440));
        // 行の終わりに詰め物があってもよい
        assert_eq!(shm_buffer_len(3, 2, 16), Some(32));
        // ちょうど i32::MAX まで
        assert_eq!(
            shm_buffer_len(1, 1, i32::MAX as u32),
            Some(i32::MAX as usize)
        );
    }

    #[test]
    fn buffer_len_rejects_bad_or_huge_sizes() {
        assert_eq!(shm_buffer_len(0, 10, 40), None);
        assert_eq!(shm_buffer_len(10, 0, 40), None);
        assert_eq!(shm_buffer_len(10, 10, 39), None);
        // width * 4 が u32 で桁あふれする幅（u32 で計算すると stride の確認を通ってしまう）
        assert_eq!(shm_buffer_len(0x4000_0001, 1, 4), None);
        assert_eq!(shm_buffer_len(u32::MAX, 1, u32::MAX), None);
        // stride * height が i32 を超える
        assert_eq!(shm_buffer_len(32768, 16384, 32768 * 4), None);
        assert_eq!(shm_buffer_len(16384, 16384, 16384 * 4), Some(1 << 30));
        assert_eq!(shm_buffer_len(1, 2, i32::MAX as u32), None);
    }

    #[test]
    fn supported_formats_are_32_bit() {
        assert!(is_supported(wl_shm::Format::Xrgb8888));
        assert!(is_supported(wl_shm::Format::Argb8888));
        assert!(is_supported(wl_shm::Format::Xbgr2101010));
        assert!(!is_supported(wl_shm::Format::Rgb565));
        assert!(!is_supported(wl_shm::Format::Rgb888));
    }

    /// 今のセッションの全出力を撮る（ロックはしない）。`cargo test -- --ignored` で手動実行。
    #[test]
    #[ignore = "needs a running wlroots compositor"]
    fn captures_the_current_session() {
        let shots = spawn_capture_all().expect("spawn").join().expect("thread");
        assert!(!shots.is_empty());
        for s in &shots {
            assert!(!s.output_name.is_empty());
            assert!(s.pixels.len() >= (s.stride * (s.height - 1) + s.width * 4) as usize);
            eprintln!(
                "{}: {}x{} stride {} {:?}",
                s.output_name, s.width, s.height, s.stride, s.format
            );
        }
    }
}
