//! Wayland のイベントループ。ロックを掛け、入力と認証の結果で状態を進め、解除する。

use std::fmt;
use std::time::Duration;

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData, Region},
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
use crate::render::overlay::OverlayHost;
use crate::render::{OutputRender, shm::ShmBlack};
use crate::state::{Action, Event, Machine, Phase};
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
    /// GPU のフレームを 1 回でも出せたか。出せる前に失敗したら shm の黒に切り替える。
    pub(crate) presented: bool,
    /// 最後に出したフレームが、アニメーションの終わった後の最終の絵か。
    /// フェード中に画面が眠ってフレームコールバックが遅れても、起きたときに最終の絵を描くため。
    pub(crate) settled: bool,
}

impl LockOutput {
    /// GPU のフレームを出せなかったときの後始末。
    ///
    /// まだ 1 枚も出していなければ、lock surface が map されず（キーボードのフォーカスも来ず）
    /// パスワードを打てないので、shm の黒に切り替えて出す。出したことがあれば、次のフレームか
    /// Tick で描き直す。
    fn gpu_frame_failed(
        &mut self,
        shm: &Shm,
        qh: &QueueHandle<App>,
        requested_frame: bool,
        error: String,
    ) {
        // この描画で要求したフレームコールバックは commit されていないので、待たない
        if requested_frame {
            self.frame_pending = false;
        }
        if self.presented {
            log::warn!(
                "GPU frame for {} failed, retrying later: {error}",
                self.meta.label()
            );
            return;
        }
        log::warn!(
            "GPU frame for {} failed before the first frame, using shm: {error}",
            self.meta.label()
        );
        // 先に GpuSurface を捨てる（EGL サーフェス → wl_egl_window の順に壊れる）
        self.render = None;
        let wl_surface = self.lock_surface.wl_surface();
        // 失敗した描画で buffer_scale が保留になっているかもしれない。shm は論理サイズ・スケール 1
        wl_surface.set_buffer_scale(1);
        let mut black = ShmBlack::new();
        black.draw(shm, wl_surface, self.size.0, self.size.1, qh);
        self.render = Some(OutputRender::Shm(black));
        self.settled = true;
    }
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
    /// 主画面への重ね描き（時計・日付・光の柱・錠）。Plan 4。
    ///
    /// `gpu` より前に置く。フィールドは宣言順に捨てられるので、egui_glow の GL の後片付けが
    /// EGL のコンテキストより先に行われる。
    pub(crate) overlay: OverlayHost,
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

/// 論理サイズ全体を不透明領域にする（次の commit で反映される）。
///
/// GPU の EGL サーフェスは ARGB なので、伝えないとコンポジタが毎フレーム下と混ぜ合わせる。
fn set_opaque_region(
    compositor: &CompositorState,
    surface: &wl_surface::WlSurface,
    (width, height): (u32, u32),
) {
    match Region::new(compositor) {
        Ok(region) => {
            region.add(0, 0, width as i32, height as i32);
            surface.set_opaque_region(Some(region.wl_region()));
            // region は set_opaque_region の後に捨ててよい（ここで destroy される）
        }
        Err(e) => log::warn!("cannot create the opaque region: {e}"),
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

    // 重ね物の状態（GL はまだ使わない）。ロック前にフォーカスしていた出力もここで sway に尋ねる
    let overlay = OverlayHost::new(&config);
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
        overlay,
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
            // フレームコールバック待ちでない出力の描き直しを再開する（解除の開始など）。
            // 眠っている出力はコールバック待ちのままなので、ここでは描かない。コンポジタが
            // 溜めていたコールバックを届けたときに、frame() が settled を見て最終の絵を描く
            app.kick_animation();
            // 時刻・日付・状態・主画面が変わったら重ね物を描き直す（Plan 4）
            app.tick_overlay();
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
    // フォント・シェーダー・アトラスをロック前に用意する（失敗しても重ね物なしで続ける）
    if let Some(gpu) = app.gpu.as_ref() {
        app.overlay.attach_gpu(gpu);
    }
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
    /// 主画面の添字（出力の一覧が変わっていれば選び直す）。Plan 4。
    ///
    /// 選び直して主画面が別の出力に替わったら、前の主画面に残っている重ね物を消すため、
    /// その出力がまだあれば重ね物なしで描き直す。添字は出力が抜かれるとずれるので出力名で探す。
    /// `drawing` はこれから描く出力（そこで描かれるので、ここでは描かない）。
    /// フレームコールバック待ちなら、前の主画面は描き終えていない（settled でない）ので、
    /// そのコールバックで描き直される。
    pub(crate) fn primary_index(&mut self, drawing: Option<usize>) -> Option<usize> {
        let primary = self.overlay.refresh_primary(
            self.outputs
                .iter()
                .map(|o| (o.meta.name.as_str(), o.meta.description.as_str())),
        );
        if let Some(name) = self.overlay.take_previous_primary()
            && let Some(index) = self.outputs.iter().position(|o| o.meta.name == name)
            && Some(index) != drawing
            && !self.outputs[index].frame_pending
        {
            // 一覧は今選び直したばかりなので、draw_output の中では選び直さない（ここに戻ってこない）
            self.draw_output(index);
        }
        primary
    }

    /// 描画の入口。出力 `index` の今の状態を描いて commit する。
    pub(crate) fn draw_output(&mut self, index: usize) {
        // 出力の一覧が変わっていれば主画面を選び直す（Plan 4）。
        // 描くたびにここで選び直すことがあり、主画面が替わったら前の主画面もここで描き直す（primary_index）
        self.primary_index(Some(index));
        let now = self.clock.now_ms();
        let fade = background::fade_for(
            self.first_configure_ms,
            now,
            self.config.appearance.fade_in_ms,
        );
        let phase = self.machine.phase();
        let reveal = background::reveal_for(phase, now);
        let animating = background::needs_frames(fade, phase)
            || self.overlay.animating(index, &self.machine, now);
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
                // shm の黒は動かないので、いつも最終の絵
                out.settled = true;
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
                if let Err(e) = surface.try_make_current(gpu) {
                    out.gpu_frame_failed(&self.shm, &self.qh, false, e);
                    return;
                }
                background.draw(
                    gpu,
                    &out.meta.name,
                    buffer_width,
                    buffer_height,
                    fade,
                    reveal,
                );
                // 主画面なら時計・日付・光の柱・錠を重ねる（Plan 4）
                self.overlay.draw(
                    gpu,
                    index,
                    (buffer_width, buffer_height),
                    &self.machine,
                    now,
                    fade,
                );
                let wl_surface = out.lock_surface.wl_surface();
                // buffer_scale とフレームコールバックは、swap の commit で一緒に反映される
                wl_surface.set_buffer_scale(scale);
                let requested_frame = animating && !out.frame_pending;
                if requested_frame {
                    wl_surface.frame(&self.qh, FrameCallbackData(wl_surface.clone()));
                    out.frame_pending = true;
                }
                match surface.try_swap(gpu) {
                    Ok(()) => {
                        out.presented = true;
                        out.settled = !animating;
                    }
                    Err(e) => out.gpu_frame_failed(&self.shm, &self.qh, requested_frame, e),
                }
            }
            None => {}
        }
    }

    /// 主画面を描き直す。フレームコールバック待ちなら、そのコールバックで描かれるので何もしない。
    pub(crate) fn redraw_primary(&mut self) {
        if let Some(index) = self.primary_index(None)
            && index < self.outputs.len()
            && !self.outputs[index].frame_pending
        {
            self.draw_output(index);
        }
    }

    /// 主画面の重ね物が前に描いたときから変わっていれば描き直す（`Tick` ごと。Plan 4）。
    ///
    /// 分が変わった・失敗の錠が消えた・主画面の出力が抜かれた、などを拾う。ロックを外した後は描かない。
    fn tick_overlay(&mut self) {
        // release_lock の後・ロックが終わった後は描かない（kick_animation と同じ）
        if self.exit.is_some() || self.machine.phase() == Phase::Done {
            return;
        }
        self.primary_index(None);
        if self.overlay.wants_frame(&self.machine, self.clock.now_ms()) {
            self.redraw_primary();
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

    /// フレームコールバック待ちでない GPU の出力のうち、アニメーション中のもの（背景、または
    /// 主画面の重ね物が動いている）と、まだ最終の絵を出していないものを描き直す。
    fn kick_animation(&mut self) {
        // release_lock の後は描かない
        if self.exit.is_some() {
            return;
        }
        let animating = self.background_animating();
        let now = self.clock.now_ms();
        for index in 0..self.outputs.len() {
            let out = &self.outputs[index];
            if !out.frame_pending
                && (animating || !out.settled || self.overlay.animating(index, &self.machine, now))
                && matches!(out.render, Some(OutputRender::Gpu(_)))
            {
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
            presented: false,
            settled: false,
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
                // 確認中になったらすぐ錠を出す（Plan 4）
                self.redraw_primary();
            }
            KeyAction::Backspace => {
                if self.machine.accepts_input() && self.password.backspace() {
                    // 1 文字消えたときだけ光の柱を出す（Plan 4）
                    let now_ms = self.clock.now_ms();
                    self.overlay.keystroke(now_ms);
                    self.redraw_primary();
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
                    && self.password.push_str(text)
                {
                    // パスワードに入ったときだけ光の柱を出す（Plan 4。入りきらない分は捨て、柱も出さない）
                    let now_ms = self.clock.now_ms();
                    self.overlay.keystroke(now_ms);
                    self.redraw_primary();
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
        // Phase::Done なので reveal = 1.0。元の画面と同じくっきりしたフレームを出してから外す
        self.draw_all_outputs();
        if let Some(lock) = &self.lock {
            lock.unlock();
        }
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
        // GPU が使えれば EGL サーフェスを作る。使えなければ（作れなければ）shm の黒
        self.try_gpu_render(index);
        let out = &mut self.outputs[index];
        if out.render.is_none() {
            out.render = Some(OutputRender::Shm(ShmBlack::new()));
        }
        // 大きさが変わることもあるので、configure のたびに設定し直す
        if matches!(out.render, Some(OutputRender::Gpu(_))) {
            set_opaque_region(&self.compositor, out.lock_surface.wl_surface(), out.size);
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
        surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        let Some(index) = self.output_index_of(surface) else {
            return;
        };
        let out = &mut self.outputs[index];
        let scale = new_factor.max(1);
        if out.scale == scale {
            return;
        }
        out.scale = scale;
        log::debug!("buffer scale of {}: {scale}", out.meta.label());
        // shm は論理サイズ・スケール 1 で描くので、描き直すのは GPU だけ
        if matches!(out.render, Some(OutputRender::Gpu(_))) {
            self.draw_output(index);
        }
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
        let Some(index) = self.output_index_of(surface) else {
            return;
        };
        self.outputs[index].frame_pending = false;
        // release_lock の後は描かない
        if self.exit.is_some() {
            return;
        }
        // フェード中・解除中・主画面の重ね物が動いている間は次のフレームを描く
        // （時刻は Clock だけを使い、引数の時刻は使わない）。
        // 最終の絵をまだ出していなければ（フェードの途中で画面が眠っていたなど）それも描く
        if self.background_animating()
            || self
                .overlay
                .animating(index, &self.machine, self.clock.now_ms())
            || !self.outputs[index].settled
        {
            self.draw_output(index);
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
