//! Development-only xdg-shell host for the production lock-screen renderer.

use std::fs::File;
use std::io::Write;
use std::os::fd::{FromRawFd, OwnedFd};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tessera_lock::{AuthResult, LockAction, LockScreenStyle, LockState, PresentationMode};
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
        keyboard::{
            KeyEvent, KeyboardHandler, Keymap, Keysym, Modifiers, RawModifiers, RepeatInfo,
        },
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
        touch::TouchHandler,
    },
    shell::{
        WaylandSurface,
        xdg::{
            XdgShell,
            window::{Window, WindowConfigure, WindowDecorations, WindowHandler},
        },
    },
};
use wayland_client::{
    Connection, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_surface, wl_touch},
};

use crate::profile::Profile;
use crate::render::{Graphics, GraphicsOptions, LockRenderSurface};

const DEFAULT_SIZE: (u32, u32) = (1280, 800);
const MAX_DIMENSION: u32 = 16_384;
const SIMULATED_AUTH_DELAY: Duration = Duration::from_millis(520);
const APP_ID: &str = "dev.tessera.LockPreview";
const TITLE: &str = "Tessera Lock Preview";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum InitialState {
    #[default]
    Ready,
    Typing,
    Checking,
    Rejected,
    Unavailable,
    Ambient,
}

impl InitialState {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "ready" => Ok(Self::Ready),
            "typing" => Ok(Self::Typing),
            "checking" => Ok(Self::Checking),
            "rejected" => Ok(Self::Rejected),
            "unavailable" => Ok(Self::Unavailable),
            "ambient" => Ok(Self::Ambient),
            _ => Err(format!(
                "invalid preview state {value:?}; expected ready, typing, checking, rejected, unavailable, or ambient"
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SimulatedResult {
    #[default]
    Accepted,
    Rejected,
    Unavailable,
}

impl SimulatedResult {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "accepted" => Ok(Self::Accepted),
            "rejected" => Ok(Self::Rejected),
            "unavailable" => Ok(Self::Unavailable),
            _ => Err(format!(
                "invalid simulated result {value:?}; expected accepted, rejected, or unavailable"
            )),
        }
    }
}

#[derive(Debug)]
struct Options {
    state: InitialState,
    result: SimulatedResult,
    password: Option<tessera_lock::Secret>,
    size: (u32, u32),
    style: Option<LockScreenStyle>,
    background: Option<PathBuf>,
    ready_fd: Option<OwnedFd>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            state: InitialState::Ready,
            result: SimulatedResult::Accepted,
            password: None,
            size: DEFAULT_SIZE,
            style: None,
            background: None,
            ready_fd: None,
        }
    }
}

enum ParsedCommand {
    Run(Options),
    Help,
    Version,
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let options = match parse_args(std::env::args().skip(1))? {
        ParsedCommand::Run(options) => options,
        ParsedCommand::Help => {
            print_help();
            return Ok(());
        }
        ParsedCommand::Version => {
            println!("tessera-lock-preview {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
    };

    unsafe {
        libc::setlocale(libc::LC_TIME, c"".as_ptr());
    }
    let connection = Connection::connect_to_env()?;
    let (globals, event_queue) = registry_queue_init::<PreviewApp>(&connection)?;
    let qh = event_queue.handle();
    let mut event_loop: EventLoop<PreviewApp> = EventLoop::try_new()?;
    let loop_handle = event_loop.handle();

    let compositor_state = CompositorState::bind(&globals, &qh)?;
    let xdg_shell = XdgShell::bind(&globals, &qh)?;
    let surface = compositor_state.create_surface(&qh);
    let window = xdg_shell.create_window(surface, WindowDecorations::RequestServer, &qh);
    window.set_title(TITLE);
    window.set_app_id(APP_ID);
    window.set_min_size(Some(options.size));
    window.set_max_size(Some(options.size));
    window.commit();

    let now = Instant::now();
    let (lock_state, visual_progress, advance_deadlines) = initial_lock_state(options.state, now);
    let profile = Profile::current()?;
    log::debug!(
        "lock preview: rendering the current profile for {:?}",
        profile.username
    );
    let graphics = if options.style.is_none() && options.background.is_none() {
        Graphics::new(&connection)?
    } else {
        Graphics::new_with_options(
            &connection,
            GraphicsOptions {
                style: options.style,
                background: options.background,
            },
        )?
    };
    let mut app = PreviewApp {
        loop_handle,
        registry_state: RegistryState::new(&globals),
        _compositor_state: compositor_state,
        output_state: OutputState::new(&globals, &qh),
        seat_state: SeatState::new(&globals, &qh),
        window,
        keyboards: Vec::new(),
        pointers: Vec::new(),
        touches: Vec::new(),
        graphics,
        render: None,
        profile,
        lock_state,
        simulated_result: options.result,
        expected_password: options.password,
        pending_auth: None,
        logical_size: options.size,
        scale: 1,
        modifiers: Modifiers::default(),
        active_layout: 0,
        layout_names: Vec::new(),
        visual_progress,
        feedback_was_animating: false,
        advance_deadlines,
        last_update: now,
        last_clock_minute: clock_minute(),
        ready_fd: options.ready_fd,
        ready_signaled: false,
        dirty: false,
        exit: false,
        fatal: None,
    };

    WaylandSource::new(connection, event_queue).insert(event_loop.handle())?;
    while !app.exit {
        let timeout = if app.dirty || app.is_animating() {
            Duration::from_millis(16)
        } else {
            Duration::from_secs(1)
        };
        event_loop.dispatch(timeout, &mut app)?;
        app.advance();
        if app.dirty {
            app.render();
        }
    }
    if let Some(error) = app.fatal.take() {
        return Err(error.into());
    }
    Ok(())
}

struct PreviewApp {
    loop_handle: LoopHandle<'static, Self>,
    registry_state: RegistryState,
    _compositor_state: CompositorState,
    output_state: OutputState,
    seat_state: SeatState,
    window: Window,
    keyboards: Vec<wl_keyboard::WlKeyboard>,
    pointers: Vec<wl_pointer::WlPointer>,
    touches: Vec<wl_touch::WlTouch>,
    graphics: Graphics,
    render: Option<LockRenderSurface>,
    profile: Profile,
    lock_state: LockState,
    simulated_result: SimulatedResult,
    expected_password: Option<tessera_lock::Secret>,
    pending_auth: Option<PendingAuth>,
    logical_size: (u32, u32),
    scale: i32,
    modifiers: Modifiers,
    active_layout: u32,
    layout_names: Vec<String>,
    visual_progress: f32,
    feedback_was_animating: bool,
    advance_deadlines: bool,
    last_update: Instant,
    last_clock_minute: u64,
    ready_fd: Option<OwnedFd>,
    ready_signaled: bool,
    dirty: bool,
    exit: bool,
    fatal: Option<String>,
}

struct PendingAuth {
    result: AuthResult,
    complete_at: Instant,
}

impl PreviewApp {
    fn configure(&mut self, connection: &Connection, configure: WindowConfigure) {
        let size = (
            configure
                .new_size
                .0
                .map_or(self.logical_size.0, |value| value.get()),
            configure
                .new_size
                .1
                .map_or(self.logical_size.1, |value| value.get()),
        );
        self.logical_size = (size.0.max(1), size.1.max(1));
        let result = if let Some(render) = &mut self.render {
            render.resize(self.logical_size, self.scale as f32)
        } else {
            self.graphics
                .create_surface(
                    connection,
                    self.window.wl_surface(),
                    self.logical_size,
                    self.scale as f32,
                )
                .map(|render| self.render = Some(render))
        };
        if let Err(error) = result {
            self.fail(error.to_string());
        } else {
            self.dirty = true;
        }
    }

    fn handle_key(&mut self, event: KeyEvent) {
        let now = Instant::now();
        self.advance_deadlines = true;
        match event.keysym {
            Keysym::Escape => self.exit = true,
            Keysym::Return | Keysym::KP_Enter => {
                let action = self.lock_state.submit(now);
                self.handle_action(action, now);
            }
            Keysym::BackSpace | Keysym::Delete | Keysym::KP_Delete => {
                self.lock_state.backspace(now);
            }
            Keysym::u | Keysym::U if self.modifiers.ctrl => {
                self.lock_state.clear(now);
            }
            _ => {
                if let Some(text) = event.utf8 {
                    self.lock_state.type_text(&text, now);
                } else {
                    self.lock_state.reveal(now);
                }
            }
        }
        self.dirty = true;
    }

    fn handle_action(&mut self, action: LockAction, now: Instant) {
        let LockAction::Authenticate(secret) = action else {
            return;
        };
        let result = simulated_auth_result(
            self.simulated_result,
            &secret,
            self.expected_password.as_ref(),
        );
        self.pending_auth = Some(PendingAuth {
            result,
            complete_at: now + SIMULATED_AUTH_DELAY,
        });
    }

    fn pointer_activity(&mut self, _position: (f64, f64), _submit: bool) {
        let now = Instant::now();
        if self.lock_state.reveal(now) {
            self.dirty = true;
        }
    }

    fn advance(&mut self) {
        let now = Instant::now();
        if self.graphics.reload_avatar_if_ready() {
            self.dirty = true;
        }
        let (state_changed, action) = if self.advance_deadlines {
            self.lock_state.tick(now)
        } else {
            (false, LockAction::None)
        };
        if state_changed {
            self.dirty = true;
        }
        self.handle_action(action, now);
        if self
            .pending_auth
            .as_ref()
            .is_some_and(|pending| now >= pending.complete_at)
        {
            let pending = self.pending_auth.take().expect("pending auth checked");
            if matches!(
                self.lock_state.authentication_finished(pending.result, now),
                LockAction::Unlock
            ) {
                log::info!("lock preview: simulated authentication accepted; closing preview");
                self.exit = true;
            }
            self.dirty = true;
        }
        let feedback_animating = self
            .graphics
            .feedback_animation_active(&self.lock_state, now);
        if feedback_animating || self.feedback_was_animating {
            self.dirty = true;
        }
        self.feedback_was_animating = feedback_animating;

        let target = if self.lock_state.presentation() == PresentationMode::Engaged {
            1.0
        } else {
            0.0
        };
        let elapsed = now.duration_since(self.last_update).as_secs_f32();
        self.last_update = now;
        let avatar_visible = self.lock_state.presentation() == PresentationMode::Engaged
            || self.visual_progress > 0.02;
        if avatar_visible {
            match self.graphics.advance_avatar(elapsed) {
                Ok(true) => self.dirty = true,
                Ok(false) => {}
                Err(error) => {
                    self.fail(error.to_string());
                    return;
                }
            }
        }
        if (self.visual_progress - target).abs() > 0.001 {
            let step = elapsed / 0.24;
            if target > self.visual_progress {
                self.visual_progress = (self.visual_progress + step).min(target);
            } else {
                self.visual_progress = (self.visual_progress - step).max(target);
            }
            self.dirty = true;
        }
        let minute = clock_minute();
        if minute != self.last_clock_minute {
            self.last_clock_minute = minute;
            self.dirty = true;
        }
    }

    fn is_animating(&self) -> bool {
        let target = if self.lock_state.presentation() == PresentationMode::Engaged {
            1.0
        } else {
            0.0
        };
        let avatar_visible = self.lock_state.presentation() == PresentationMode::Engaged
            || self.visual_progress > 0.02;
        (self.visual_progress - target).abs() > 0.001
            || (avatar_visible && self.graphics.avatar_is_animated())
            || self.graphics.avatar_reload_pending()
            || self.lock_state.validation_pending()
            || self.graphics.composition_animates(&self.lock_state)
            || self
                .graphics
                .feedback_animation_active(&self.lock_state, Instant::now())
    }

    fn render(&mut self) {
        self.dirty = false;
        let Some(render) = &mut self.render else {
            return;
        };
        if let Err(error) = self.graphics.render(
            render,
            &self.lock_state,
            &self.profile,
            self.visual_progress,
            Instant::now(),
        ) {
            self.fail(error.to_string());
            return;
        }
        self.signal_ready();
    }

    fn signal_ready(&mut self) {
        if self.ready_signaled {
            return;
        }
        self.ready_signaled = true;
        if let Some(fd) = self.ready_fd.take() {
            let mut file = File::from(fd);
            if let Err(error) = file.write_all(b"\n") {
                log::warn!("lock preview: failed to signal ready fd: {error}");
            }
        }
        log::info!("lock preview: first frame presented");
    }

    fn fail(&mut self, message: String) {
        log::error!("lock preview: {message}");
        self.fatal.get_or_insert(message);
        self.exit = true;
    }
}

impl Drop for PreviewApp {
    fn drop(&mut self) {
        if let Some(render) = self.render.take() {
            self.graphics.destroy_surface(render);
        }
    }
}

impl WindowHandler for PreviewApp {
    fn request_close(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _window: &Window,
    ) {
        self.exit = true;
    }

    fn configure(
        &mut self,
        connection: &Connection,
        _qh: &QueueHandle<Self>,
        _window: &Window,
        configure: WindowConfigure,
        _serial: u32,
    ) {
        self.configure(connection, configure);
    }
}

impl CompositorHandler for PreviewApp {
    fn scale_factor_changed(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        factor: i32,
    ) {
        if surface != self.window.wl_surface() {
            return;
        }
        self.scale = factor.max(1);
        self.window.wl_surface().set_buffer_scale(self.scale);
        if let Some(render) = &mut self.render
            && let Err(error) = render.resize(self.logical_size, self.scale as f32)
        {
            self.fail(error.to_string());
            return;
        }
        self.dirty = true;
    }

    fn transform_changed(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _transform: wl_output::Transform,
    ) {
        self.dirty = true;
    }

    fn frame(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
    }

    fn surface_enter(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for PreviewApp {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn update_output(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn output_destroyed(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }
}

impl SeatHandler for PreviewApp {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
    ) {
    }

    fn new_capability(
        &mut self,
        _connection: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        match capability {
            Capability::Keyboard => {
                let keyboard = self.seat_state.get_keyboard_with_repeat(
                    qh,
                    &seat,
                    None,
                    self.loop_handle.clone(),
                    Box::new(|state, _keyboard, event| state.handle_key(event)),
                );
                match keyboard {
                    Ok(keyboard) => self.keyboards.push(keyboard),
                    Err(error) => self.fail(format!("keyboard initialization failed: {error}")),
                }
            }
            Capability::Pointer => match self.seat_state.get_pointer(qh, &seat) {
                Ok(pointer) => self.pointers.push(pointer),
                Err(error) => self.fail(format!("pointer initialization failed: {error}")),
            },
            Capability::Touch => match self.seat_state.get_touch(qh, &seat) {
                Ok(touch) => self.touches.push(touch),
                Err(error) => self.fail(format!("touch initialization failed: {error}")),
            },
            _ => {}
        }
    }

    fn remove_capability(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        match capability {
            Capability::Keyboard => self.keyboards.clear(),
            Capability::Pointer => self.pointers.clear(),
            Capability::Touch => self.touches.clear(),
            _ => {}
        }
    }

    fn remove_seat(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
    ) {
    }
}

impl KeyboardHandler for PreviewApp {
    fn enter(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _serial: u32,
        _raw: &[u32],
        _keysyms: &[Keysym],
    ) {
        if surface == self.window.wl_surface() && self.lock_state.reveal(Instant::now()) {
            self.dirty = true;
        }
    }

    fn leave(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _surface: &wl_surface::WlSurface,
        _serial: u32,
    ) {
    }

    fn press_key(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        self.handle_key(event);
    }

    fn repeat_key(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        self.handle_key(event);
    }

    fn release_key(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        _event: KeyEvent,
    ) {
    }

    fn update_modifiers(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        modifiers: Modifiers,
        _raw_modifiers: RawModifiers,
        layout: u32,
    ) {
        self.modifiers = modifiers;
        self.active_layout = layout;
        let layout = self.layout_names.get(layout as usize).cloned();
        self.lock_state
            .set_keyboard_status(modifiers.caps_lock, layout);
        self.dirty = true;
    }

    fn update_repeat_info(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _info: RepeatInfo,
    ) {
    }

    fn update_keymap(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        keymap: Keymap<'_>,
    ) {
        let context = xkbcommon::xkb::Context::new(xkbcommon::xkb::CONTEXT_NO_FLAGS);
        let Some(keymap) = xkbcommon::xkb::Keymap::new_from_string(
            &context,
            keymap.as_string(),
            xkbcommon::xkb::KEYMAP_FORMAT_TEXT_V1,
            xkbcommon::xkb::KEYMAP_COMPILE_NO_FLAGS,
        ) else {
            self.layout_names.clear();
            self.lock_state
                .set_keyboard_status(self.modifiers.caps_lock, None);
            self.dirty = true;
            return;
        };
        self.layout_names = (0..keymap.num_layouts())
            .map(|index| keymap.layout_get_name(index).to_owned())
            .collect();
        let layout = self.layout_names.get(self.active_layout as usize).cloned();
        self.lock_state
            .set_keyboard_status(self.modifiers.caps_lock, layout);
        self.dirty = true;
    }
}

impl PointerHandler for PreviewApp {
    fn pointer_frame(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            if &event.surface != self.window.wl_surface() {
                continue;
            }
            match &event.kind {
                PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                    self.pointer_activity(event.position, false);
                }
                PointerEventKind::Press { button, .. } => {
                    self.pointer_activity(event.position, *button == 0x110);
                }
                PointerEventKind::Axis {
                    horizontal,
                    vertical,
                    ..
                } if !horizontal.is_none() || !vertical.is_none() => {
                    self.pointer_activity(event.position, false);
                }
                PointerEventKind::Leave { .. }
                | PointerEventKind::Release { .. }
                | PointerEventKind::Axis { .. } => {}
            }
        }
    }
}

impl TouchHandler for PreviewApp {
    fn down(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _touch: &wl_touch::WlTouch,
        _serial: u32,
        _time: u32,
        surface: wl_surface::WlSurface,
        _id: i32,
        position: (f64, f64),
    ) {
        if surface == *self.window.wl_surface() {
            self.pointer_activity(position, true);
        }
    }

    fn up(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _touch: &wl_touch::WlTouch,
        _serial: u32,
        _time: u32,
        _id: i32,
    ) {
    }

    fn motion(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _touch: &wl_touch::WlTouch,
        _time: u32,
        _id: i32,
        position: (f64, f64),
    ) {
        self.pointer_activity(position, false);
    }

    fn shape(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _touch: &wl_touch::WlTouch,
        _id: i32,
        _major: f64,
        _minor: f64,
    ) {
    }

    fn orientation(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _touch: &wl_touch::WlTouch,
        _id: i32,
        _orientation: f64,
    ) {
    }

    fn cancel(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _touch: &wl_touch::WlTouch,
    ) {
    }
}

smithay_client_toolkit::delegate_registry!(PreviewApp);

impl ProvidesRegistryState for PreviewApp {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState, SeatState];
}

smithay_client_toolkit::delegate_dispatch2!(PreviewApp);

fn initial_lock_state(state: InitialState, now: Instant) -> (LockState, f32, bool) {
    let mut lock = LockState::new(now);
    match state {
        InitialState::Ready => (lock, 1.0, true),
        InitialState::Typing => {
            lock.type_text("preview", now);
            (lock, 1.0, false)
        }
        InitialState::Checking => {
            lock.type_text("preview", now);
            let _ = lock.submit(now);
            (lock, 1.0, false)
        }
        InitialState::Rejected => {
            lock.type_text("preview", now);
            let _ = lock.submit(now);
            let _ = lock.authentication_finished(
                AuthResult::Rejected {
                    message: localized(
                        "Incorrect password · Please wait before trying again",
                        "密码错误 · 请稍后重试",
                    ),
                },
                now,
            );
            (lock, 1.0, false)
        }
        InitialState::Unavailable => {
            lock.type_text("preview", now);
            let _ = lock.submit(now);
            let _ = lock.authentication_finished(
                AuthResult::Unavailable {
                    message: localized("Authentication unavailable", "认证服务不可用"),
                },
                now,
            );
            (lock, 1.0, false)
        }
        InitialState::Ambient => {
            let _ = lock.tick(now + Duration::from_secs(60));
            (lock, 0.0, false)
        }
    }
}

fn simulated_auth_result(
    result: SimulatedResult,
    submitted: &tessera_lock::Secret,
    expected: Option<&tessera_lock::Secret>,
) -> AuthResult {
    if let Some(expected) = expected {
        return if submitted.content_eq(expected) {
            AuthResult::Accepted
        } else {
            rejected_result()
        };
    }
    match result {
        SimulatedResult::Accepted => AuthResult::Accepted,
        SimulatedResult::Rejected => rejected_result(),
        SimulatedResult::Unavailable => AuthResult::Unavailable {
            message: localized("Authentication unavailable", "认证服务不可用"),
        },
    }
}

fn rejected_result() -> AuthResult {
    AuthResult::Rejected {
        message: localized(
            "Incorrect password · Please wait before trying again",
            "密码错误 · 请稍后重试",
        ),
    }
}

fn localized(en: &str, zh: &str) -> String {
    let locale = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LC_MESSAGES"))
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_default();
    if locale.starts_with("zh") {
        zh.to_owned()
    } else {
        en.to_owned()
    }
}

fn parse_args(
    args: impl IntoIterator<Item = String>,
) -> Result<ParsedCommand, Box<dyn std::error::Error>> {
    let mut options = Options::default();
    let mut result_explicit = false;
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--help" | "-h" => return Ok(ParsedCommand::Help),
            "--version" | "-V" => return Ok(ParsedCommand::Version),
            "--state" => {
                let value = args.next().ok_or("--state requires a value")?;
                options.state = InitialState::parse(&value)?;
            }
            "--result" => {
                let value = args.next().ok_or("--result requires a value")?;
                options.result = SimulatedResult::parse(&value)?;
                result_explicit = true;
            }
            "--password" => {
                let value = args.next().ok_or("--password requires a value")?;
                options.password = Some(parse_preview_password(&value)?);
            }
            "--size" => {
                let value = args.next().ok_or("--size requires WIDTHxHEIGHT")?;
                options.size = parse_size(&value)?;
            }
            "--composition" | "--style" => {
                let value = args.next().ok_or("--composition requires a value")?;
                options.style = Some(parse_composition(&value)?);
            }
            "--background" => {
                let value = args.next().ok_or("--background requires a path")?;
                options.background = Some(PathBuf::from(value));
            }
            "--ready-fd" => {
                let value = args.next().ok_or("--ready-fd requires a descriptor")?;
                options.ready_fd = Some(owned_fd(&value)?);
            }
            value if value.starts_with("--state=") => {
                options.state = InitialState::parse(&value["--state=".len()..])?;
            }
            value if value.starts_with("--result=") => {
                options.result = SimulatedResult::parse(&value["--result=".len()..])?;
                result_explicit = true;
            }
            value if value.starts_with("--password=") => {
                options.password = Some(parse_preview_password(&value["--password=".len()..])?);
            }
            value if value.starts_with("--size=") => {
                options.size = parse_size(&value["--size=".len()..])?;
            }
            value if value.starts_with("--composition=") => {
                options.style = Some(parse_composition(&value["--composition=".len()..])?);
            }
            value if value.starts_with("--style=") => {
                options.style = Some(parse_composition(&value["--style=".len()..])?);
            }
            value if value.starts_with("--background=") => {
                options.background = Some(PathBuf::from(&value["--background=".len()..]));
            }
            value if value.starts_with("--ready-fd=") => {
                options.ready_fd = Some(owned_fd(&value["--ready-fd=".len()..])?);
            }
            _ => return Err(format!("unknown argument: {argument}").into()),
        }
    }
    if options.password.is_some() && result_explicit {
        return Err("--password cannot be combined with --result".into());
    }
    Ok(ParsedCommand::Run(options))
}

fn parse_preview_password(value: &str) -> Result<tessera_lock::Secret, String> {
    if value.is_empty() {
        return Err("preview password must not be empty".into());
    }
    let mut password = tessera_lock::Secret::default();
    if !password.push_str(value) {
        return Err(format!(
            "preview password must not exceed {} UTF-8 bytes",
            tessera_lock::Secret::MAX_BYTES
        ));
    }
    Ok(password)
}

fn parse_composition(value: &str) -> Result<LockScreenStyle, String> {
    match value {
        "centered" => Ok(LockScreenStyle::Centered),
        "cinematic" => Ok(LockScreenStyle::Cinematic),
        "bsod" => Ok(LockScreenStyle::Bsod),
        _ => Err(format!(
            "invalid lock-screen composition {value:?}; expected centered, cinematic, or bsod"
        )),
    }
}

fn parse_size(value: &str) -> Result<(u32, u32), String> {
    let separator = value
        .find(['x', 'X'])
        .ok_or_else(|| format!("invalid preview size {value:?}; expected WIDTHxHEIGHT"))?;
    let width = value[..separator]
        .parse::<u32>()
        .map_err(|_| format!("invalid preview width in {value:?}"))?;
    let height = value[separator + 1..]
        .parse::<u32>()
        .map_err(|_| format!("invalid preview height in {value:?}"))?;
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(format!(
            "preview size must be between 1x1 and {MAX_DIMENSION}x{MAX_DIMENSION}"
        ));
    }
    Ok((width, height))
}

fn owned_fd(value: &str) -> Result<OwnedFd, Box<dyn std::error::Error>> {
    let fd: i32 = value.parse()?;
    if fd < 0 {
        return Err("ready fd must be non-negative".into());
    }
    if unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn print_help() {
    print!(
        "{}",
        concat!(
            "Usage: tessera-lock-preview [OPTIONS]\n\n",
            "Preview the production lock-screen renderer in an ordinary Wayland window.\n\n",
            "Options:\n",
            "  --state STATE       Initial state: ready, typing, checking, rejected,\n",
            "                      unavailable, or ambient [default: ready]\n",
            "  --result RESULT     Submission result: accepted, rejected, or unavailable\n",
            "                      [default: accepted]\n",
            "  --password TEXT     Accept only this development-only fake password\n",
            "                      (mutually exclusive with --result)\n",
            "  --size WIDTHxHEIGHT Fixed logical window size [default: 1280x800]\n",
            "  --composition NAME  UI composition: centered, cinematic, or bsod\n",
            "  --style NAME        Compatibility alias for --composition\n",
            "  --background PATH   Independent lock-screen image override\n",
            "  --ready-fd FD       Signal FD after the first frame is presented\n",
            "  -h, --help          Print help\n",
            "  -V, --version       Print version\n\n",
            "Escape closes the preview. No session lock or PAM authentication is used.\n",
            "Never pass a real account password: command arguments may be visible.\n",
        )
    );
}

fn clock_minute() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 60
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scoped_preview_options() {
        let ParsedCommand::Run(options) = parse_args([
            "--state=rejected".to_owned(),
            "--result".to_owned(),
            "unavailable".to_owned(),
            "--size".to_owned(),
            "1440x900".to_owned(),
            "--composition=cinematic".to_owned(),
            "--background".to_owned(),
            "lock.png".to_owned(),
        ])
        .unwrap() else {
            panic!("expected run options");
        };
        assert_eq!(options.state, InitialState::Rejected);
        assert_eq!(options.result, SimulatedResult::Unavailable);
        assert_eq!(options.size, (1440, 900));
        assert_eq!(options.style, Some(LockScreenStyle::Cinematic));
        assert_eq!(options.background, Some(PathBuf::from("lock.png")));
    }

    #[test]
    fn rejects_unknown_state_and_unsafe_dimensions() {
        assert!(InitialState::parse("accepted").is_err());
        assert!(parse_size("0x800").is_err());
        assert!(parse_size("20000x800").is_err());
        assert!(parse_size("wide").is_err());
        assert!(parse_composition("glass").is_err());
        assert_eq!(parse_composition("bsod"), Ok(LockScreenStyle::Bsod));
        let ParsedCommand::Run(alias) = parse_args(["--style=centered".to_owned()]).unwrap() else {
            panic!("expected run options");
        };
        assert_eq!(alias.style, Some(LockScreenStyle::Centered));
    }

    #[test]
    fn parses_fake_password_and_rejects_ambiguous_result_policy() {
        let ParsedCommand::Run(options) = parse_args(["--password=0000".to_owned()]).unwrap()
        else {
            panic!("expected run options");
        };
        let expected = parse_preview_password("0000").unwrap();
        assert!(
            options
                .password
                .as_ref()
                .is_some_and(|password| password.content_eq(&expected))
        );

        assert!(parse_args(["--password=".to_owned()]).is_err());
        assert!(
            parse_args(["--password=0000".to_owned(), "--result=rejected".to_owned(),]).is_err()
        );
    }

    #[test]
    fn fake_password_accepts_only_an_exact_match() {
        let expected = parse_preview_password("0000").unwrap();
        let matching = parse_preview_password("0000").unwrap();
        let wrong = parse_preview_password("0001").unwrap();

        assert!(matches!(
            simulated_auth_result(SimulatedResult::Accepted, &matching, Some(&expected)),
            AuthResult::Accepted
        ));
        assert!(matches!(
            simulated_auth_result(SimulatedResult::Accepted, &wrong, Some(&expected)),
            AuthResult::Rejected { .. }
        ));
    }

    #[test]
    fn constructs_each_visual_state_through_the_product_state_machine() {
        let now = Instant::now();
        let (ready, _, _) = initial_lock_state(InitialState::Ready, now);
        assert_eq!(ready.password_len(), 0);
        assert!(!ready.checking());

        let (typing, _, _) = initial_lock_state(InitialState::Typing, now);
        assert!(typing.password_len() > 0);

        let (checking, _, _) = initial_lock_state(InitialState::Checking, now);
        assert!(checking.checking());
        assert_eq!(checking.password_len(), "preview".chars().count());

        let (rejected, _, _) = initial_lock_state(InitialState::Rejected, now);
        assert!(rejected.message().is_some());

        let (unavailable, _, _) = initial_lock_state(InitialState::Unavailable, now);
        assert!(unavailable.message().is_some());

        let (ambient, progress, _) = initial_lock_state(InitialState::Ambient, now);
        assert_eq!(ambient.presentation(), PresentationMode::Ambient);
        assert_eq!(progress, 0.0);
    }
}
