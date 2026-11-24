// API spike for kabi-lock plans 2-6: session lock + keyboard + EGL + egui + screencopy,
// plus compile-only checks for PAM, zbus (blocking), swayipc and chrono.
#![allow(dead_code)]

use std::ffi::{CStr, CString, c_void};
use std::sync::Arc;
use std::time::{Duration, Instant};

use glow::HasContext;
use khronos_egl as egl;
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{EventLoop, LoopHandle},
        calloop_wayland_source::WaylandSource,
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
    },
    session_lock::{
        SessionLock, SessionLockHandler, SessionLockState, SessionLockSurface,
        SessionLockSurfaceConfigure,
    },
    shm::{Shm, ShmHandler, raw::RawPool},
};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_buffer, wl_keyboard, wl_output, wl_seat, wl_shm, wl_surface},
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};

const PLATFORM_WAYLAND_KHR: egl::Enum = 0x31D8;

struct Gpu {
    egl: egl::Instance<egl::Static>,
    display: egl::Display,
    config: egl::Config,
    context: egl::Context,
    gl: Arc<glow::Context>,
    painter: egui_glow::Painter,
    egui: egui::Context,
}

impl Gpu {
    fn new(conn: &Connection) -> Gpu {
        let t = Instant::now();
        let egl = egl::Instance::new(egl::Static);
        let display = unsafe {
            egl.get_platform_display(
                PLATFORM_WAYLAND_KHR,
                conn.backend().display_ptr() as *mut c_void,
                &[egl::ATTRIB_NONE],
            )
        }
        .expect("egl display");
        egl.initialize(display).expect("egl init");
        egl.bind_api(egl::OPENGL_ES_API).unwrap();
        let config = egl
            .choose_first_config(
                display,
                &[
                    egl::RED_SIZE, 8, egl::GREEN_SIZE, 8, egl::BLUE_SIZE, 8, egl::ALPHA_SIZE, 8,
                    egl::RENDERABLE_TYPE, egl::OPENGL_ES3_BIT,
                    egl::SURFACE_TYPE, egl::WINDOW_BIT,
                    egl::NONE,
                ],
            )
            .unwrap()
            .expect("config");
        let context = egl
            .create_context(display, config, None, &[egl::CONTEXT_MAJOR_VERSION, 3, egl::NONE])
            .expect("context");
        // 画面なしで current にしてシェーダーをコンパイルしておく（surfaceless_context 拡張）
        egl.make_current(display, None, None, Some(context)).expect("make current surfaceless");
        let gl = Arc::new(unsafe {
            glow::Context::from_loader_function(|s| {
                egl.get_proc_address(s).map_or(std::ptr::null(), |p| p as *const _)
            })
        });
        let painter = egui_glow::Painter::new(gl.clone(), "", None, false).expect("painter");
        let egui = egui::Context::default();
        eprintln!("gpu init {:?}", t.elapsed());
        Gpu { egl, display, config, context, gl, painter, egui }
    }
}

struct LockSurface {
    lock: SessionLockSurface,
    egl_window: Option<wayland_egl::WlEglSurface>,
    egl_surface: Option<egl::Surface>,
    size: (u32, u32),
}

struct App {
    loop_handle: LoopHandle<'static, App>,
    conn: Connection,
    compositor: CompositorState,
    outputs: OutputState,
    registry: RegistryState,
    seats: SeatState,
    shm: Shm,
    lock_state: SessionLockState,
    lock: Option<SessionLock>,
    surfaces: Vec<LockSurface>,
    gpu: Gpu,
    started: Instant,
    exit: bool,
    typed: usize,
}

impl App {
    fn draw(&mut self, idx: usize) {
        let s = &self.surfaces[idx];
        let (Some(es), (w, h)) = (s.egl_surface, s.size) else { return };
        let g = &mut self.gpu;
        g.egl.make_current(g.display, Some(es), Some(es), Some(g.context)).unwrap();
        unsafe {
            g.gl.viewport(0, 0, w as i32, h as i32);
            g.gl.clear_color(0.05, 0.06, 0.08, 1.0);
            g.gl.clear(glow::COLOR_BUFFER_BIT);
        }
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w as f32, h as f32))),
            ..Default::default()
        };
        let typed = self.typed;
        let out = g.egui.run_pass(input, |ctx| {
            let p = ctx.layer_painter(egui::LayerId::background());
            p.text(
                egui::pos2(w as f32 / 2.0, h as f32 / 3.0),
                egui::Align2::CENTER_CENTER,
                format!("10:30  typed={typed}  (Esc to unlock)"),
                egui::FontId::proportional(48.0),
                egui::Color32::WHITE,
            );
        });
        let prims = g.egui.tessellate(out.shapes, out.pixels_per_point);
        let mut delta = out.textures_delta;
        g.painter.paint_and_update_textures([w, h], out.pixels_per_point, &prims, &mut delta);
        g.egl.swap_buffers(g.display, es).unwrap();
    }

    fn redraw_all(&mut self) {
        for i in 0..self.surfaces.len() {
            self.draw(i);
        }
    }
}

impl SessionLockHandler for App {
    fn locked(&mut self, _: &Connection, _: &QueueHandle<Self>, _: SessionLock) {
        eprintln!("locked at {:?}", self.started.elapsed());
    }
    fn finished(&mut self, _: &Connection, _: &QueueHandle<Self>, _: SessionLock) {
        eprintln!("lock finished (denied)");
        self.exit = true;
    }
    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: SessionLockSurface,
        configure: SessionLockSurfaceConfigure,
        _: u32,
    ) {
        let (w, h) = configure.new_size;
        let Some(idx) = self.surfaces.iter().position(|s| s.lock.wl_surface() == surface.wl_surface()) else { return };
        let g = &self.gpu;
        let s = &mut self.surfaces[idx];
        if s.egl_window.is_none() {
            let win = wayland_egl::WlEglSurface::new(s.lock.wl_surface().id(), w as i32, h as i32).unwrap();
            let es = unsafe {
                g.egl.create_window_surface(g.display, g.config, win.ptr() as egl::NativeWindowType, None)
            }
            .unwrap();
            g.egl.make_current(g.display, Some(es), Some(es), Some(g.context)).unwrap();
            // 複数の出力を順に描くので、swap でフレームを待たない
            g.egl.swap_interval(g.display, 0).unwrap();
            s.egl_window = Some(win);
            s.egl_surface = Some(es);
        } else if let Some(win) = &s.egl_window {
            win.resize(w as i32, h as i32, 0, 0);
        }
        s.size = (w, h);
        eprintln!("configure {}x{} at {:?}", w, h, self.started.elapsed());
        self.draw(idx);
    }
}

impl KeyboardHandler for App {
    fn enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: &wl_surface::WlSurface, _: u32, _: &[u32], _: &[Keysym]) {}
    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: &wl_surface::WlSurface, _: u32) {}
    fn press_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, event: KeyEvent) {
        if event.keysym == Keysym::Escape {
            if let Some(lock) = self.lock.take() {
                lock.unlock();
                let _ = self.conn.roundtrip();
            }
            self.exit = true;
            return;
        }
        if event.utf8.as_deref().is_some_and(|t| !t.is_empty() && !t.chars().any(char::is_control)) {
            self.typed += 1;
            self.redraw_all();
        }
    }
    fn repeat_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, _: KeyEvent) {}
    fn release_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, _: KeyEvent) {}
    fn update_modifiers(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, _: Modifiers, _: RawModifiers, _: u32) {}
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seats
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: wl_seat::WlSeat, cap: Capability) {
        if cap == Capability::Keyboard {
            self.seats.get_keyboard(qh, &seat, None).expect("keyboard");
        }
    }
    fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat, _: Capability) {}
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl CompositorHandler for App {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: i32) {}
    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState, SeatState];
}

smithay_client_toolkit::delegate_registry!(App);
wayland_client::delegate_noop!(App: ignore wl_buffer::WlBuffer);
wayland_client::delegate_noop!(App: ZwlrScreencopyManagerV1);
smithay_client_toolkit::delegate_dispatch2!(App);

// ---- screencopy (run before locking) ----

#[derive(Default)]
struct Capture {
    format: Option<(wl_shm::Format, u32, u32, u32)>,
    done: bool,
    failed: bool,
}

struct CaptureApp {
    capture: Capture,
}

impl Dispatch<ZwlrScreencopyFrameV1, ()> for CaptureApp {
    fn event(
        state: &mut Self,
        _: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer { format, width, height, stride } => {
                if let wayland_client::WEnum::Value(f) = format {
                    state.capture.format = Some((f, width, height, stride));
                }
            }
            zwlr_screencopy_frame_v1::Event::Ready { .. } => state.capture.done = true,
            zwlr_screencopy_frame_v1::Event::Failed => state.capture.failed = true,
            _ => {}
        }
    }
}
impl Dispatch<wayland_client::protocol::wl_registry::WlRegistry, wayland_client::globals::GlobalListContents> for CaptureApp {
    fn event(
        _: &mut Self,
        _: &wayland_client::protocol::wl_registry::WlRegistry,
        _: wayland_client::protocol::wl_registry::Event,
        _: &wayland_client::globals::GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
wayland_client::delegate_noop!(CaptureApp: ZwlrScreencopyManagerV1);
wayland_client::delegate_noop!(CaptureApp: ignore wl_buffer::WlBuffer);
wayland_client::delegate_noop!(CaptureApp: ignore wayland_client::protocol::wl_shm_pool::WlShmPool);
wayland_client::delegate_noop!(CaptureApp: ignore wl_shm::WlShm);
wayland_client::delegate_noop!(CaptureApp: ignore wl_output::WlOutput);

/// 最初の出力を撮る（返り値: 幅, 高さ, 撮れたか）
fn capture_first_output() -> (u32, u32, bool) {
    let conn = Connection::connect_to_env().unwrap();
    let (globals, mut queue) = registry_queue_init::<CaptureApp>(&conn).unwrap();
    let qh = queue.handle();
    let manager: ZwlrScreencopyManagerV1 = globals.bind(&qh, 1..=3, ()).expect("screencopy");
    let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let output: wl_output::WlOutput = globals.bind(&qh, 1..=4, ()).unwrap();
    let mut app = CaptureApp { capture: Capture::default() };
    let frame = manager.capture_output(0, &output, &qh, ());
    while app.capture.format.is_none() && !app.capture.failed {
        queue.blocking_dispatch(&mut app).unwrap();
    }
    let Some((format, w, h, stride)) = app.capture.format else { return (0, 0, false) };
    let size = (stride * h) as usize;
    let file = rustix_memfd(size);
    let pool = shm.create_pool(std::os::fd::AsFd::as_fd(&file), size as i32, &qh, ());
    let buffer = pool.create_buffer(0, w as i32, h as i32, stride as i32, format, &qh, ());
    frame.copy(&buffer);
    while !app.capture.done && !app.capture.failed {
        queue.blocking_dispatch(&mut app).unwrap();
    }
    (w, h, app.capture.done)
}

fn rustix_memfd(size: usize) -> std::fs::File {
    // 依存を増やさないため /dev/shm 上の一時ファイルで代用（本番は memfd を使う）
    let path = format!("/dev/shm/kabi-spike-{}", std::process::id());
    let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&path).unwrap();
    f.set_len(size as u64).unwrap();
    std::fs::remove_file(&path).unwrap();
    f
}

// ---- compile-only checks ----

struct PasswordConv {
    password: CString,
}

impl pam_client::ConversationHandler for PasswordConv {
    fn prompt_echo_on(&mut self, _: &CStr) -> Result<CString, pam_client::ErrorCode> {
        Err(pam_client::ErrorCode::CONV_ERR)
    }
    fn prompt_echo_off(&mut self, _: &CStr) -> Result<CString, pam_client::ErrorCode> {
        Ok(self.password.clone())
    }
    fn text_info(&mut self, _: &CStr) {}
    fn error_msg(&mut self, _: &CStr) {}
}

fn check_password(service: &str, user: &str, password: &str) -> bool {
    let conv = PasswordConv { password: CString::new(password).unwrap() };
    let Ok(mut ctx) = pam_client::Context::new(service, Some(user), conv) else { return false };
    ctx.authenticate(pam_client::Flag::NONE).is_ok() && ctx.acct_mgmt(pam_client::Flag::NONE).is_ok()
}

fn keepassxc_lock() -> zbus::Result<()> {
    let conn = zbus::blocking::Connection::session()?;
    conn.call_method(
        Some("org.keepassxc.KeePassXC.MainWindow"),
        "/keepassxc",
        Some("org.keepassxc.KeePassXC.MainWindow"),
        "lockAllDatabases",
        &(),
    )?;
    Ok(())
}

fn secret_collections_locked() -> zbus::Result<Vec<bool>> {
    let conn = zbus::blocking::Connection::session()?;
    let reply = conn.call_method(
        Some("org.freedesktop.secrets"),
        "/org/freedesktop/secrets",
        Some("org.freedesktop.DBus.Properties"),
        "Get",
        &("org.freedesktop.Secret.Service", "Collections"),
    )?;
    let value: zbus::zvariant::OwnedValue = reply.body().deserialize()?;
    let paths: Vec<zbus::zvariant::OwnedObjectPath> = value.try_into()?;
    let mut out = Vec::new();
    for p in paths {
        let r = conn.call_method(
            Some("org.freedesktop.secrets"),
            p.as_str(),
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.freedesktop.Secret.Collection", "Locked"),
        )?;
        let v: zbus::zvariant::OwnedValue = r.body().deserialize()?;
        out.push(bool::try_from(v)?);
    }
    Ok(out)
}

fn focused_output() -> Option<String> {
    let mut c = swayipc::Connection::new().ok()?;
    c.get_outputs().ok()?.into_iter().find(|o| o.focused).map(|o| o.name)
}

fn date_text(fmt: &str) -> String {
    use chrono::format::StrftimeItems;
    let valid = StrftimeItems::new(fmt).parse().is_ok();
    let fmt = if valid { fmt } else { "%m/%d" };
    chrono::Local::now().format(fmt).to_string()
}

fn main() {
    env_logger::init();
    let mode = std::env::args().nth(1).unwrap_or_default();
    eprintln!("focused={:?} date={}", focused_output(), date_text("%m/%d"));
    if mode == "capture" {
        let t = Instant::now();
        eprintln!("capture {:?} in {:?}", capture_first_output(), t.elapsed());
        return;
    }
    if mode == "secrets" {
        eprintln!("collections locked: {:?}", secret_collections_locked());
        return;
    }
    let auto = mode == "lock-auto";
    if mode != "lock" && !auto {
        eprintln!("usage: spike lock|capture|secrets");
        return;
    }

    let started = Instant::now();
    let conn = Connection::connect_to_env().unwrap();
    let (globals, event_queue) = registry_queue_init::<App>(&conn).unwrap();
    let qh = event_queue.handle();
    let mut event_loop: EventLoop<App> = EventLoop::try_new().unwrap();
    let gpu = Gpu::new(&conn);
    let mut app = App {
        loop_handle: event_loop.handle(),
        conn: conn.clone(),
        compositor: CompositorState::bind(&globals, &qh).unwrap(),
        outputs: OutputState::new(&globals, &qh),
        registry: RegistryState::new(&globals),
        seats: SeatState::new(&globals, &qh),
        shm: Shm::bind(&globals, &qh).unwrap(),
        lock_state: SessionLockState::new(&globals, &qh),
        lock: None,
        surfaces: Vec::new(),
        gpu,
        started,
        exit: false,
        typed: 0,
    };
    // 出力の情報（名前・説明）を受け取る
    let mut queue = event_queue;
    queue.roundtrip(&mut app).unwrap();
    for o in app.outputs.outputs() {
        let info = app.outputs.info(&o);
        eprintln!("output {:?} {:?}", info.as_ref().and_then(|i| i.name.clone()), info.as_ref().and_then(|i| i.description.clone()));
    }
    let lock = app.lock_state.lock(&qh).expect("ext-session-lock");
    for output in app.outputs.outputs() {
        let surface = app.compositor.create_surface(&qh);
        let ls = lock.create_lock_surface(surface, &output, &qh);
        app.surfaces.push(LockSurface { lock: ls, egl_window: None, egl_surface: None, size: (0, 0) });
    }
    app.lock = Some(lock);
    WaylandSource::new(conn.clone(), queue).insert(event_loop.handle()).unwrap();
    while !app.exit {
        event_loop.dispatch(Duration::from_millis(16), &mut app).unwrap();
        if auto && app.started.elapsed() > Duration::from_secs(3) {
            if let Some(lock) = app.lock.take() {
                lock.unlock();
                let _ = app.conn.roundtrip();
                eprintln!("auto-unlocked at {:?}", app.started.elapsed());
            }
            app.exit = true;
        }
    }
    let _ = RawPool::new(4, &app.shm);
}
