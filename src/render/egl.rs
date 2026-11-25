//! EGL の準備（`Gpu`）と、出力ごとの EGL サーフェス（`GpuSurface`）。
//!
//! `Gpu` はロック要求の前に、サーフェスなしで current にして GL の準備まで済ませる。
//! EGL の関数は main スレッドからだけ呼ぶ。

use std::ffi::c_void;
use std::sync::Arc;
use std::time::Instant;

use glow::HasContext;
use khronos_egl as egl;
use log::{debug, warn};
use wayland_client::{Connection, Proxy, protocol::wl_surface};

/// EGL_PLATFORM_WAYLAND_KHR
const PLATFORM_WAYLAND_KHR: egl::Enum = 0x31D8;

/// EGL ディスプレイ・コンテキストと glow の GL。
///
/// `Drop` は実装しない（プロセスの終わりまで使う）。EGL ディスプレイは `Connection` の
/// `wl_display` を指しているので、`Gpu` が `Connection` の複製を持ち、`Gpu` が生きている間は
/// 接続が閉じないようにする。
pub struct Gpu {
    /// EGL ディスプレイが指す wl_display を生かしておくための複製。直接は使わない。
    _conn: Connection,
    egl: egl::Instance<egl::Static>,
    display: egl::Display,
    config: egl::Config,
    context: egl::Context,
    gl: Arc<glow::Context>,
}

impl Gpu {
    /// サーフェスなしで current にし、GL の準備まで済ませる。失敗したら Err（呼び出し側は shm に切り替える）。
    pub fn new(conn: &Connection) -> Result<Gpu, String> {
        let started = Instant::now();
        let egl = egl::Instance::new(egl::Static);
        // SAFETY: display_ptr は conn が生きている間有効な wl_display を指す。
        // 返す Gpu が conn の複製（_conn）を持つので、Gpu と EGL ディスプレイを使う間は閉じない。
        let display = unsafe {
            egl.get_platform_display(
                PLATFORM_WAYLAND_KHR,
                conn.backend().display_ptr() as *mut c_void,
                &[egl::ATTRIB_NONE],
            )
        }
        .map_err(|e| format!("eglGetPlatformDisplay: {e}"))?;
        let (major, minor) = egl
            .initialize(display)
            .map_err(|e| format!("eglInitialize: {e}"))?;
        match Self::create_context(&egl, display) {
            Ok((config, context)) => {
                // SAFETY: コンテキストは current。返る関数ポインタはこのコンテキストで使う。
                let gl = Arc::new(unsafe {
                    glow::Context::from_loader_function(|name| {
                        egl.get_proc_address(name)
                            .map_or(std::ptr::null(), |p| p as *const c_void)
                    })
                });
                // SAFETY: current なコンテキストへの読み取りだけ。
                let (version, renderer) = unsafe {
                    (
                        gl.get_parameter_string(glow::VERSION),
                        gl.get_parameter_string(glow::RENDERER),
                    )
                };
                debug!(
                    "EGL {major}.{minor} ready in {:?}: {version} on {renderer}",
                    started.elapsed()
                );
                Ok(Gpu {
                    _conn: conn.clone(),
                    egl,
                    display,
                    config,
                    context,
                    gl,
                })
            }
            Err(e) => {
                let _ = egl.terminate(display);
                Err(e)
            }
        }
    }

    fn create_context(
        egl: &egl::Instance<egl::Static>,
        display: egl::Display,
    ) -> Result<(egl::Config, egl::Context), String> {
        egl.bind_api(egl::OPENGL_ES_API)
            .map_err(|e| format!("eglBindAPI: {e}"))?;
        let config = egl
            .choose_first_config(
                display,
                &[
                    egl::RED_SIZE,
                    8,
                    egl::GREEN_SIZE,
                    8,
                    egl::BLUE_SIZE,
                    8,
                    egl::ALPHA_SIZE,
                    8,
                    egl::RENDERABLE_TYPE,
                    egl::OPENGL_ES3_BIT,
                    egl::SURFACE_TYPE,
                    egl::WINDOW_BIT,
                    egl::NONE,
                ],
            )
            .map_err(|e| format!("eglChooseConfig: {e}"))?
            .ok_or_else(|| "no RGBA8 OpenGL ES 3 EGL config".to_string())?;
        let context = egl
            .create_context(
                display,
                config,
                None,
                &[egl::CONTEXT_MAJOR_VERSION, 3, egl::NONE],
            )
            .map_err(|e| format!("eglCreateContext: {e}"))?;
        // 画面なしで current にする（EGL_KHR_surfaceless_context）。ロック前にシェーダーを作るため
        if let Err(e) = egl.make_current(display, None, None, Some(context)) {
            let _ = egl.destroy_context(display, context);
            return Err(format!("eglMakeCurrent without surface: {e}"));
        }
        Ok((config, context))
    }

    pub fn gl(&self) -> &Arc<glow::Context> {
        &self.gl
    }
}

/// 1 つの lock surface に結び付いた EGL のウィンドウサーフェス。
///
/// 大きさはバッファの画素数（論理サイズ × buffer_scale）。
/// 捨てるときは EGL サーフェスを先に壊し、その後 `wl_egl_window` を壊す。
/// 元の `wl_surface` より先に捨てること（`LockOutput` のフィールド順で保証する）。
pub struct GpuSurface {
    window: wayland_egl::WlEglSurface,
    surface: egl::Surface,
    display: egl::Display,
    size: (u32, u32),
}

impl GpuSurface {
    pub fn new(
        gpu: &Gpu,
        surface: &wl_surface::WlSurface,
        width: u32,
        height: u32,
    ) -> Result<GpuSurface, String> {
        let (width, height) = (width.max(1), height.max(1));
        let window = wayland_egl::WlEglSurface::new(surface.id(), width as i32, height as i32)
            .map_err(|e| format!("wl_egl_window_create: {e}"))?;
        // SAFETY: window.ptr() は window が生きている間有効な wl_egl_window。
        // EGL サーフェスは Drop で window より先に壊す。
        let egl_surface = unsafe {
            gpu.egl.create_window_surface(
                gpu.display,
                gpu.config,
                window.ptr() as egl::NativeWindowType,
                None,
            )
        }
        .map_err(|e| format!("eglCreateWindowSurface: {e}"))?;
        let this = GpuSurface {
            window,
            surface: egl_surface,
            display: gpu.display,
            size: (width, height),
        };
        gpu.egl
            .make_current(
                gpu.display,
                Some(egl_surface),
                Some(egl_surface),
                Some(gpu.context),
            )
            .map_err(|e| format!("eglMakeCurrent: {e}"))?;
        // 複数の出力を順に描くので、swap でフレームを待たない（待つのはフレームコールバック）
        if let Err(e) = gpu.egl.swap_interval(gpu.display, 0) {
            warn!("eglSwapInterval(0) failed: {e}");
        }
        Ok(this)
    }

    /// 今のバッファの大きさ（画素）。
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// 次の swap からバッファの大きさを変える。
    pub fn resize(&mut self, width: u32, height: u32) {
        let (width, height) = (width.max(1), height.max(1));
        self.window.resize(width as i32, height as i32, 0, 0);
        self.size = (width, height);
    }

    /// このサーフェスを current にする。失敗は呼び出し側で扱う。
    pub fn try_make_current(&self, gpu: &Gpu) -> Result<(), String> {
        gpu.egl
            .make_current(
                gpu.display,
                Some(self.surface),
                Some(self.surface),
                Some(gpu.context),
            )
            .map_err(|e| format!("eglMakeCurrent failed: {e}"))
    }

    /// 描いた内容を出す（wl_surface の commit も含む）。失敗は呼び出し側で扱う。
    pub fn try_swap(&self, gpu: &Gpu) -> Result<(), String> {
        gpu.egl
            .swap_buffers(gpu.display, self.surface)
            .map_err(|e| format!("eglSwapBuffers failed: {e}"))
    }
}

impl Drop for GpuSurface {
    fn drop(&mut self) {
        // Instance<Static> は中身のない型なので、ここで作り直してよい
        let egl = egl::Instance::new(egl::Static);
        // current のまま eglDestroySurface すると破棄が遅延し、wl_egl_window / wl_surface より
        // 後まで残ってしまう。このサーフェスが current なら、先にサーフェスなしに戻す
        if egl.get_current_surface(egl::DRAW) == Some(self.surface) {
            let _ = egl.make_current(self.display, None, None, egl.get_current_context());
        }
        if let Err(e) = egl.destroy_surface(self.display, self.surface) {
            warn!("eglDestroySurface failed: {e}");
        }
        // self.window（wl_egl_window）はこの後フィールドの破棄で壊れる
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 今のセッションの Wayland 接続で EGL を準備できるか（ロックはしない）。
    /// `cargo test -- --ignored` で手動実行。
    #[test]
    #[ignore = "needs a running Wayland compositor and a GPU"]
    fn creates_a_surfaceless_context() {
        let conn = Connection::connect_to_env().expect("wayland");
        let gpu = Gpu::new(&conn).expect("gpu");
        // SAFETY: Gpu::new が current にしたコンテキストへの読み取りだけ。
        let version = unsafe { gpu.gl().get_parameter_string(glow::VERSION) };
        assert!(version.contains("OpenGL ES 3"), "{version}");
    }
}
