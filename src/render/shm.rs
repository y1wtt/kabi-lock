//! `wl_shm` の黒い画面。EGL が使えないときの予備にもなる。

use smithay_client_toolkit::shm::{Shm, raw::RawPool};
use wayland_client::QueueHandle;
use wayland_client::protocol::{wl_buffer, wl_shm, wl_surface};

use crate::app::App;

/// 出力 1 つ分の黒い画面。大きさが変わるまで同じバッファを使い回す。
#[derive(Default)]
pub struct ShmBlack {
    buffer: Option<BlackBuffer>,
}

impl ShmBlack {
    pub fn new() -> ShmBlack {
        ShmBlack::default()
    }

    /// 黒い ARGB8888 バッファを作って attach + commit する。
    ///
    /// ext-session-lock では configure の大きさと同じバッファを出さなければならないので、
    /// `width` x `height` が前回と違えば作り直す。作れなければログを出して何もしない。
    pub fn draw(
        &mut self,
        shm: &Shm,
        surface: &wl_surface::WlSurface,
        width: u32,
        height: u32,
        qh: &QueueHandle<App>,
    ) {
        let reusable = self
            .buffer
            .as_ref()
            .is_some_and(|b| b.width == width && b.height == height);
        // 古いバッファは、新しいバッファを commit してから捨てる
        let mut old = None;
        if !reusable {
            match BlackBuffer::new(shm, width, height, qh) {
                Ok(buffer) => old = self.buffer.replace(buffer),
                Err(e) => {
                    log::error!("cannot create a {width}x{height} black buffer: {e}");
                    return;
                }
            }
        }
        let Some(buffer) = &self.buffer else { return };
        surface.attach(Some(&buffer.buffer), 0, 0);
        surface.damage_buffer(0, 0, width as i32, height as i32);
        surface.commit();
        drop(old);
    }
}

/// 黒で塗った wl_buffer と、その元のメモリプール。
struct BlackBuffer {
    // プールより先に wl_buffer を破棄する（Drop を参照）
    buffer: wl_buffer::WlBuffer,
    _pool: RawPool,
    width: u32,
    height: u32,
}

impl BlackBuffer {
    fn new(shm: &Shm, width: u32, height: u32, qh: &QueueHandle<App>) -> Result<Self, String> {
        let len = buffer_len(width, height).ok_or_else(|| "invalid size".to_string())?;
        let mut pool = RawPool::new(len, shm).map_err(|e| e.to_string())?;
        fill_black(&mut pool.mmap()[..len]);
        // buffer_len が i32 に収まることを確かめている
        let buffer = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            (width * 4) as i32,
            wl_shm::Format::Argb8888,
            (),
            qh,
        );
        Ok(BlackBuffer {
            buffer,
            _pool: pool,
            width,
            height,
        })
    }
}

impl Drop for BlackBuffer {
    fn drop(&mut self) {
        self.buffer.destroy();
    }
}

/// ARGB8888 のバッファのバイト数。0 や、wl_shm のプールの上限（`i32::MAX`）を超える大きさは `None`。
fn buffer_len(width: u32, height: u32) -> Option<usize> {
    if width == 0 || height == 0 {
        return None;
    }
    let len = u64::from(width) * u64::from(height) * 4;
    if len > i32::MAX as u64 {
        return None;
    }
    usize::try_from(len).ok()
}

/// 不透明な黒で塗る。ARGB8888 はリトルエンディアンで B, G, R, A の順。
fn fill_black(pixels: &mut [u8]) {
    for pixel in pixels.as_chunks_mut::<4>().0 {
        *pixel = [0, 0, 0, 0xff];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_len_is_four_bytes_per_pixel() {
        assert_eq!(buffer_len(1920, 1080), Some(1920 * 1080 * 4));
        assert_eq!(buffer_len(3440, 1440), Some(3440 * 1440 * 4));
    }

    #[test]
    fn buffer_len_rejects_empty_and_huge_sizes() {
        assert_eq!(buffer_len(0, 1080), None);
        assert_eq!(buffer_len(1920, 0), None);
        // wl_shm のプールの大きさは i32 なので、それを超えるものは作れない
        assert_eq!(buffer_len(65_536, 65_536), None);
        assert_eq!(buffer_len(u32::MAX, 1), None);
    }

    #[test]
    fn fill_black_writes_opaque_black_argb() {
        let mut pixels = vec![0x55u8; 4 * 3];
        fill_black(&mut pixels);
        // ARGB8888 はリトルエンディアンで B, G, R, A の順に並ぶ
        assert_eq!(pixels, [0, 0, 0, 0xff].repeat(3));
    }
}
