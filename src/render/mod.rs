//! 描画。出力ごとの描画手段を `OutputRender` で持つ。

pub mod background;
pub mod egl;
pub mod shm;

/// 出力 1 つの描画手段。
pub enum OutputRender {
    /// `wl_shm` の黒い画面。
    Shm(shm::ShmBlack),
    /// EGL のウィンドウサーフェスに GL で描く（Plan 3）。
    Gpu(egl::GpuSurface),
}
