//! 描画。出力ごとの描画手段を `OutputRender` で持つ。

pub mod shm;

/// 出力 1 つの描画手段。
pub enum OutputRender {
    /// `wl_shm` の黒い画面。
    Shm(shm::ShmBlack),
    // Plan 3 が追加: Gpu(egl::GpuSurface),
}
