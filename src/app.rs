//! Wayland のイベントループ。ロックを掛け、入力と認証の結果で状態を進め、解除する。

use std::fmt;
use std::time::Duration;

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData},
    delegate_dispatch2, delegate_registry,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{
            EventLoop, channel,
            timer::{TimeoutAction, Timer},
        },
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
    shm::{Shm, ShmHandler},
};
use wayland_client::{
    Connection, Proxy, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_buffer, wl_keyboard, wl_output, wl_seat, wl_surface},
};
use zeroize::Zeroizing;

use crate::auth::{self, AuthEvent, password::PasswordChecker};
use crate::clock::Clock;
use crate::config::Config;
use crate::input::{self, KeyAction};
use crate::password::PasswordBuffer;
use crate::render::background::{self, Background};
use crate::render::egl::{Gpu, GpuSurface};
use crate::render::{OutputRender, shm::ShmBlack};
use crate::state::{Action, Event, Machine};
use crate::wl::outputs::OutputMeta;

/// `Event::Tick` を送る間隔。画面が消えていてフレームコールバックが来なくても状態を進めるため。
const TICK: Duration = Duration::from_millis(50);

/// `run` が返すエラー。どれも終了コード 1 にする。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppError {
    /// Wayland に接続できない、または初期化に失敗した。
    NoWayland(String),
    /// ext-session-lock-v1 がない。
    LockUnsupported,
    /// ロックを拒否された（`finished` が `locked` より先に来た）。
    LockDenied,
    /// そのほか。
    Other(String),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::NoWayland(reason) => {
                write!(f, "cannot connect to the Wayland compositor: {reason}")
            }
            AppError::LockUnsupported => {
                write!(f, "the compositor does not support ext-session-lock-v1")
            }
            AppError::LockDenied => write!(
                f,
                "the compositor refused to lock the session (is another locker running?)"
            ),
            AppError::Other(reason) => write!(f, "{reason}"),
        }
    }
}

impl std::error::Error for AppError {}

/// 出力 1 つ分のロック画面。
pub(crate) struct LockOutput {
    /// 抜かれたときに探すための wl_output。
    pub(crate) output: wl_output::WlOutput,
    pub(crate) meta: OutputMeta,
    /// configure 前は `None`。
    ///
    /// `lock_surface` より前に置く。フィールドは宣言順に捨てられるので、出力が抜かれたときに
    /// EGL サーフェスと wl_egl_window が wl_surface より先に壊れる。この順番を変えないこと。
    pub(crate) render: Option<OutputRender>,
    pub(crate) lock_surface: SessionLockSurface,
    /// configure の論理サイズ。configure 前は (0, 0)。
    pub(crate) size: (u32, u32),
    /// フレームコールバック待ち。GPU で描いていて、アニメーション中のときだけ要求する。
    pub(crate) frame_pending: bool,
    /// wl_surface の buffer_scale。GPU で描くときだけ使う（shm は常に 1）。
    pub(crate) scale: i32,
}

/// ロック中の状態。SCTK の各ハンドラはこの型に実装する。
pub struct App {
    pub(crate) conn: Connection,
    pub(crate) qh: QueueHandle<App>,
    /// Plan 3〜5 で見た目や認証の設定を読む。
    #[allow(dead_code)]
    pub(crate) config: Config,
    /// PAM に渡したユーザー名。Plan 5 の U2F でも使う。
    #[allow(dead_code)]
    pub(crate) user: String,
    pub(crate) registry: RegistryState,
    pub(crate) compositor: CompositorState,
    pub(crate) output_state: OutputState,
    pub(crate) seat_state: SeatState,
    pub(crate) shm: Shm,
    pub(crate) lock_state: SessionLockState,
    /// `lock` を要求してから、解除するか `finished` を受けるまで `Some`。
    pub(crate) lock: Option<SessionLock>,
    /// `ReleaseLock` を受けたが、まだ `unlock` していない。
    pub(crate) release_pending: bool,
    pub(crate) outputs: Vec<LockOutput>,
    pub(crate) keyboards: Vec<(wl_seat::WlSeat, wl_keyboard::WlKeyboard)>,
    pub(crate) ctrl: bool,
    pub(crate) machine: Machine,
    pub(crate) password: PasswordBuffer,
    pub(crate) checker: PasswordChecker,
    pub(crate) clock: Clock,
    /// 最初の configure を受けた時刻。フェードはここから測る（Plan 3）。
    pub(crate) first_configure_ms: Option<u64>,
    /// EGL。使えなければ `None`（全出力を shm の黒で描く）。
    pub(crate) gpu: Option<Gpu>,
    /// 背景のシェーダーと出力ごとのテクスチャ。`gpu` が `Some` のときだけ `Some`。
    pub(crate) background: Option<Background>,
    /// `Some` になったらイベントループを抜けて、これを `run` の結果にする。
    pub(crate) exit: Option<Result<(), AppError>>,
}

/// スクリーンショット（別スレッド）と EGL の準備を並行して進め、背景を作る。
///
/// EGL かシェーダーが使えなければ `(None, None)` で、全出力を shm の黒で描く。
/// 撮れなかった出力は `Background` にテクスチャがないので黒になる。
fn prepare_background(conn: &Connection, blur: f32) -> (Option<Gpu>, Option<Background>) {
    let capture = crate::wl::screencopy::spawn_capture_all();
    let gpu = match Gpu::new(conn) {
        Ok(gpu) => Some(gpu),
        Err(e) => {
            log::warn!("EGL unavailable, falling back to a black shm background: {e}");
            None
        }
    };
    let shots = capture.join().unwrap_or_else(|_| {
        log::warn!("screenshot thread panicked, background will be black");
        Vec::new()
    });
    let Some(gpu) = gpu else {
        return (None, None);
    };
    match Background::new(&gpu, blur) {
        Ok(mut background) => {
            for shot in &shots {
                background.upload(&gpu, shot);
            }
            (Some(gpu), Some(background))
        }
        Err(e) => {
            log::warn!("background shaders failed, falling back to shm: {e}");
            (None, None)
        }
    }
}

/// ロックして、解除されるまでイベントループを回す。
///
/// ext-session-lock が使えない／拒否されたら `Err`。
/// 失敗しうる準備はすべて `lock` の要求より前に済ませる（ロックした後に止まると画面が赤のまま残る）。
pub fn run(config: Config) -> Result<(), AppError> {
    let clock = Clock::start();
    let user = auth::current_user()
        .ok_or_else(|| AppError::Other("cannot determine the current user name".into()))?;
    // サービス定義がないと libpam は全拒否の設定に切り替わる。解除できないロックは取らない
    let service = &config.auth.pam_service;
    if !auth::pam_service_exists(service) {
        return Err(AppError::Other(format!(
            "PAM service {service:?} not found in /etc/pam.d or /usr/lib/pam.d"
        )));
    }

    let conn = Connection::connect_to_env().map_err(|e| AppError::NoWayland(e.to_string()))?;
    let (globals, mut queue) =
        registry_queue_init::<App>(&conn).map_err(|e| AppError::NoWayland(e.to_string()))?;
    let qh = queue.handle();
    let compositor = CompositorState::bind(&globals, &qh)
        .map_err(|e| AppError::NoWayland(format!("wl_compositor: {e}")))?;
    let shm = Shm::bind(&globals, &qh).map_err(|e| AppError::NoWayland(format!("wl_shm: {e}")))?;

    let mut event_loop: EventLoop<'static, App> =
        EventLoop::try_new().map_err(|e| AppError::Other(format!("event loop: {e}")))?;
    let (auth_tx, auth_rx) = channel::channel::<AuthEvent>();
    let checker = PasswordChecker::spawn(config.auth.pam_service.clone(), user.clone(), auth_tx);

    let mut app = App {
        conn: conn.clone(),
        qh: qh.clone(),
        config,
        user,
        registry: RegistryState::new(&globals),
        compositor,
        output_state: OutputState::new(&globals, &qh),
        seat_state: SeatState::new(&globals, &qh),
        shm,
        lock_state: SessionLockState::new(&globals, &qh),
        lock: None,
        release_pending: false,
        outputs: Vec::new(),
        keyboards: Vec::new(),
        ctrl: false,
        machine: Machine::new(),
        password: PasswordBuffer::new(),
        checker,
        clock,
        first_configure_ms: None,
        gpu: None,
        background: None,
        exit: None,
    };

    let handle = event_loop.handle();
    handle
        .insert_source(auth_rx, |event, _, app: &mut App| {
            if let channel::Event::Msg(event) = event {
                app.handle_auth(event);
            }
        })
        .map_err(|e| AppError::Other(format!("auth channel: {}", e.error)))?;
    handle
        .insert_source(Timer::from_duration(TICK), |_, _, app: &mut App| {
            app.handle_event(Event::Tick);
            TimeoutAction::ToDuration(TICK)
        })
        .map_err(|e| AppError::Other(format!("tick timer: {}", e.error)))?;

    // 出力の名前と説明を受け取る
    queue
        .roundtrip(&mut app)
        .map_err(|e| AppError::NoWayland(e.to_string()))?;
    for output in app.output_state.outputs() {
        let meta = OutputMeta::from_info(app.output_state.info(&output).as_ref());
        log::info!("output {}: {}", meta.label(), meta.description);
    }
    // 背景を用意してからロックする（撮影と EGL の準備は並行。ロックした後は撮れない）
    let (gpu, background) = prepare_background(&conn, app.config.appearance.blur);
    app.gpu = gpu;
    app.background = background;
    WaylandSource::new(conn, queue)
        .insert(handle)
        .map_err(|e| AppError::Other(format!("wayland source: {}", e.error)))?;

    let lock = app
        .lock_state
        .lock(&qh)
        .map_err(|_| AppError::LockUnsupported)?;
    log::info!("lock requested after {} ms", app.clock.now_ms());
    app.lock = Some(lock);
    // locked を待たずに、全出力の lock surface を作る
    for output in app.output_state.outputs() {
        app.add_output(output);
    }

    while app.exit.is_none() {
        event_loop
            .dispatch(None, &mut app)
            .map_err(|e| AppError::Other(format!("event loop failed: {e}")))?;
    }
    app.exit.take().unwrap_or(Ok(()))
}

impl App {
    /// 描画の入口。出力 `index` の今の状態を描いて commit する。
    pub(crate) fn draw_output(&mut self, index: usize) {
        let now = self.clock.now_ms();
        let fade = background::fade_for(
            self.first_configure_ms,
            now,
            self.config.appearance.fade_in_ms,
        );
        let phase = self.machine.phase();
        let reveal = background::reveal_for(phase, now);
        let animating = background::needs_frames(fade, phase);
        let Some(out) = self.outputs.get_mut(index) else {
            return;
        };
        let (width, height) = out.size;
        if width == 0 || height == 0 {
            return;
        }
        match out.render.as_mut() {
            Some(OutputRender::Shm(black)) => {
                black.draw(
                    &self.shm,
                    out.lock_surface.wl_surface(),
                    width,
                    height,
                    &self.qh,
                );
            }
            Some(OutputRender::Gpu(surface)) => {
                let (Some(gpu), Some(background)) = (self.gpu.as_ref(), self.background.as_ref())
                else {
                    return;
                };
                let scale = out.scale.max(1);
                let (buffer_width, buffer_height) = (width * scale as u32, height * scale as u32);
                if surface.size() != (buffer_width, buffer_height) {
                    surface.resize(buffer_width, buffer_height);
                }
                surface.make_current(gpu);
                background.draw(
                    gpu,
                    &out.meta.name,
                    buffer_width,
                    buffer_height,
                    fade,
                    reveal,
                );
                // Plan 4: ここで主画面に重ね物を描く
                let wl_surface = out.lock_surface.wl_surface();
                // buffer_scale とフレームコールバックは、swap の commit で一緒に反映される
                wl_surface.set_buffer_scale(scale);
                if animating && !out.frame_pending {
                    wl_surface.frame(&self.qh, FrameCallbackData(wl_surface.clone()));
                    out.frame_pending = true;
                }
                surface.swap(gpu);
            }
            None => {}
        }
    }

    /// wl_surface から出力の番号を引く。
    fn output_index_of(&self, surface: &wl_surface::WlSurface) -> Option<usize> {
        self.outputs
            .iter()
            .position(|o| o.lock_surface.wl_surface() == surface)
    }

    /// GPU が使えて、まだ描画手段がなければ EGL サーフェスを作る。
    /// 失敗したら何もしない（呼び出し側が shm の黒を作る）。
    fn try_gpu_render(&mut self, index: usize) {
        let Some(gpu) = self.gpu.as_ref() else {
            return;
        };
        let out = &mut self.outputs[index];
        if out.render.is_some() {
            return;
        }
        let scale = out.scale.max(1) as u32;
        let (width, height) = (out.size.0 * scale, out.size.1 * scale);
        match GpuSurface::new(gpu, out.lock_surface.wl_surface(), width, height) {
            Ok(surface) => {
                log::debug!("GPU surface for {}: {width}x{height}", out.meta.label());
                out.render = Some(OutputRender::Gpu(surface));
            }
            Err(e) => log::warn!(
                "GPU surface for {} failed, using shm: {e}",
                out.meta.label()
            ),
        }
    }

    /// 背景がまだ動いているか（フェード中・解除中）。GPU がなければ動かない。
    fn background_animating(&self) -> bool {
        if self.gpu.is_none() {
            return false;
        }
        let fade = background::fade_for(
            self.first_configure_ms,
            self.clock.now_ms(),
            self.config.appearance.fade_in_ms,
        );
        background::needs_frames(fade, self.machine.phase())
    }

    /// アニメーション中なのに、フレームコールバック待ちでない GPU の出力を描き直す。
    fn kick_animation(&mut self) {
        if !self.background_animating() {
            return;
        }
        for index in 0..self.outputs.len() {
            let out = &self.outputs[index];
            if !out.frame_pending && matches!(out.render, Some(OutputRender::Gpu(_))) {
                self.draw_output(index);
            }
        }
    }

    /// 全出力を 1 回ずつ描く。
    fn draw_all_outputs(&mut self) {
        for index in 0..self.outputs.len() {
            self.draw_output(index);
        }
    }

    /// 出力に lock surface を作る。ロック前や、作成済みの出力なら情報だけ更新する。
    fn add_output(&mut self, output: wl_output::WlOutput) {
        let meta = OutputMeta::from_info(self.output_state.info(&output).as_ref());
        if let Some(existing) = self.outputs.iter_mut().find(|o| o.output == output) {
            existing.meta = meta;
            return;
        }
        let Some(lock) = &self.lock else {
            return;
        };
        let surface = self.compositor.create_surface(&self.qh);
        let lock_surface = lock.create_lock_surface(surface, &output, &self.qh);
        log::info!("lock surface created for {}", meta.label());
        self.outputs.push(LockOutput {
            output,
            meta,
            lock_surface,
            size: (0, 0),
            render: None,
            frame_pending: false,
            scale: 1,
        });
    }

    /// 抜かれた出力の lock surface とバッファを片付ける。
    fn remove_output(&mut self, output: &wl_output::WlOutput) {
        if let Some(index) = self.outputs.iter().position(|o| &o.output == output) {
            let removed = self.outputs.remove(index);
            log::info!("output removed: {}", removed.meta.label());
        }
    }

    /// キーを 1 回分処理する。
    fn handle_key(&mut self, event: KeyEvent) {
        // xkbcommon が作った文字列も、使い終わったらゼロで消す
        let text = event.utf8.map(Zeroizing::new);
        let action = input::classify(event.keysym, text.as_deref().map(String::as_str), self.ctrl);
        match action {
            KeyAction::Submit => {
                let empty = self.password.is_empty();
                self.handle_event(Event::Submit { empty });
            }
            KeyAction::Backspace => {
                if self.machine.accepts_input() {
                    self.password.backspace();
                }
            }
            KeyAction::Clear => {
                if self.machine.accepts_input() {
                    self.password.clear();
                }
            }
            KeyAction::Insert => {
                if self.machine.accepts_input()
                    && let Some(text) = &text
                {
                    // 入りきらない分は捨てる（Plan 4 は true のときだけ光の柱を出す）
                    self.password.push_str(text);
                }
            }
            KeyAction::Ignore => {}
        }
    }

    /// 認証スレッドからの結果を状態機械に渡す。
    fn handle_auth(&mut self, event: AuthEvent) {
        match event {
            AuthEvent::PasswordChecked { ok: true } => log::info!("password accepted"),
            AuthEvent::PasswordChecked { ok: false } => log::warn!("password rejected"),
            AuthEvent::U2fSucceeded => log::info!("security key accepted"),
        }
        self.handle_event(Event::from(event));
    }

    /// 状態機械にイベントを渡し、返ってきた操作を行う。
    fn handle_event(&mut self, event: Event) {
        let now = self.clock.now_ms();
        let before = self.machine.phase();
        let action = self.machine.handle(event, now);
        let after = self.machine.phase();
        if before != after {
            log::debug!("phase {before:?} -> {after:?} at {now} ms");
        }
        match action {
            Some(Action::CheckPassword) => {
                // handle が CheckPassword を返してから取り出す（取り出したら空になる）
                log::info!("checking password");
                self.checker.check(self.password.take());
            }
            Some(Action::ReleaseLock) => {
                self.release_pending = true;
                self.release_lock();
            }
            None => {}
        }
    }

    /// 解除が決まっていて、`locked` を受け取り済みならロックを外して終わる。
    ///
    /// `unlock_and_destroy` は `locked` の前に送るとプロトコル違反なので、その場合は `locked` で呼び直す。
    fn release_lock(&mut self) {
        if !self.release_pending {
            return;
        }
        let Some(lock) = &self.lock else {
            return;
        };
        if !lock.is_locked() {
            log::debug!("unlock is waiting for the locked event");
            return;
        }
        lock.unlock();
        // unlock_and_destroy をコンポジタに届けてから終わる
        if let Err(e) = self.conn.roundtrip() {
            log::warn!("roundtrip after unlock failed: {e}");
        }
        self.lock = None;
        self.release_pending = false;
        log::info!("unlocked after {} ms", self.clock.now_ms());
        self.exit = Some(Ok(()));
    }
}

impl SessionLockHandler for App {
    fn locked(&mut self, _: &Connection, _: &QueueHandle<Self>, _: SessionLock) {
        log::info!("session locked after {} ms", self.clock.now_ms());
        self.release_lock();
    }

    fn finished(&mut self, _: &Connection, _: &QueueHandle<Self>, session_lock: SessionLock) {
        let error = if session_lock.is_locked() {
            // locked の後に destroy を送るとプロトコルエラーになるので、unlock_and_destroy で閉じる
            session_lock.unlock();
            AppError::Other("the compositor ended the lock".into())
        } else {
            AppError::LockDenied
        };
        log::error!("lock finished: {error}");
        self.lock = None;
        self.exit = Some(Err(error));
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: SessionLockSurface,
        configure: SessionLockSurfaceConfigure,
        _serial: u32,
    ) {
        let Some(index) = self
            .outputs
            .iter()
            .position(|o| o.lock_surface.wl_surface() == surface.wl_surface())
        else {
            return;
        };
        let now = self.clock.now_ms();
        if self.first_configure_ms.is_none() {
            self.first_configure_ms = Some(now);
            log::info!("first configure after {now} ms");
        }
        let out = &mut self.outputs[index];
        out.size = configure.new_size;
        log::debug!(
            "configure {}: {}x{}",
            out.meta.label(),
            out.size.0,
            out.size.1
        );
        if out.render.is_none() {
            out.render = Some(OutputRender::Shm(ShmBlack::new()));
        }
        self.draw_output(index);
    }
}

impl KeyboardHandler for App {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
    }

    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
    ) {
        self.ctrl = false;
    }

    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.handle_key(event);
    }

    fn repeat_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
        // キーリピートは使わない（get_keyboard では呼ばれない）
    }

    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        modifiers: Modifiers,
        _: RawModifiers,
        _: u32,
    ) {
        self.ctrl = modifiers.ctrl;
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability != Capability::Keyboard || self.keyboards.iter().any(|(s, _)| *s == seat) {
            return;
        }
        match self.seat_state.get_keyboard(qh, &seat, None) {
            Ok(keyboard) => self.keyboards.push((seat, keyboard)),
            Err(e) => log::error!("cannot get the keyboard: {e}"),
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability != Capability::Keyboard {
            return;
        }
        if let Some(index) = self.keyboards.iter().position(|(s, _)| *s == seat) {
            let (_, keyboard) = self.keyboards.remove(index);
            if keyboard.version() >= 3 {
                keyboard.release();
            }
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        if let Some(out) = self
            .outputs
            .iter_mut()
            .find(|o| o.lock_surface.wl_surface() == surface)
        {
            out.frame_pending = false;
        }
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, output: wl_output::WlOutput) {
        // ロック前（最初の roundtrip 中）は何もしない。ロック後に挿された出力には lock surface を作る
        self.add_output(output);
    }

    fn update_output(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        let meta = OutputMeta::from_info(self.output_state.info(&output).as_ref());
        if let Some(existing) = self.outputs.iter_mut().find(|o| o.output == output) {
            existing.meta = meta;
        }
    }

    fn output_destroyed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        self.remove_output(&output);
    }
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

delegate_registry!(App);
delegate_dispatch2!(App);
// 黒いバッファの wl_buffer のイベント（release）は使わない
wayland_client::delegate_noop!(App: ignore wl_buffer::WlBuffer);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_explain_themselves() {
        assert_eq!(
            AppError::NoWayland("WAYLAND_DISPLAY is not set".into()).to_string(),
            "cannot connect to the Wayland compositor: WAYLAND_DISPLAY is not set"
        );
        assert_eq!(
            AppError::LockUnsupported.to_string(),
            "the compositor does not support ext-session-lock-v1"
        );
        assert_eq!(
            AppError::LockDenied.to_string(),
            "the compositor refused to lock the session (is another locker running?)"
        );
        assert_eq!(
            AppError::Other("event loop failed".into()).to_string(),
            "event loop failed"
        );
    }
}
