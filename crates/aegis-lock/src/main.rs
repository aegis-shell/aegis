mod auth;
mod profile;
mod render;
mod style;

use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::os::fd::{FromRawFd, OwnedFd};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aegis_lock::{AuthResult, LockAction, LockState, PresentationMode};
use profile::Profile;
use render::{Graphics, LockRenderSurface};
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
    session_lock::{
        SessionLock, SessionLockHandler, SessionLockState, SessionLockSurface,
        SessionLockSurfaceConfigure,
    },
};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_surface, wl_touch},
};
use wayland_protocols::wp::{
    fractional_scale::v1::client::{
        wp_fractional_scale_manager_v1::{self, WpFractionalScaleManagerV1},
        wp_fractional_scale_v1::{self, WpFractionalScaleV1},
    },
    viewporter::client::{
        wp_viewport::{self, WpViewport},
        wp_viewporter::{self, WpViewporter},
    },
};

struct OutputSurface {
    output: wl_output::WlOutput,
    lock: SessionLockSurface,
    render: Option<LockRenderSurface>,
    viewport: Option<WpViewport>,
    fractional_scale: Option<WpFractionalScaleV1>,
    logical_size: (u32, u32),
    /// Output density in 120ths (the `wp_fractional_scale_v1` wire unit): the
    /// compositor's fractional hint when it provides one, otherwise the
    /// integer `wl_output` scale × 120.
    scale_120: u32,
}

impl OutputSurface {
    fn scale_factor(&self) -> f32 {
        self.scale_120 as f32 / 120.0
    }
}

struct AppData {
    loop_handle: LoopHandle<'static, Self>,
    connection: Connection,
    registry_state: RegistryState,
    compositor_state: CompositorState,
    output_state: OutputState,
    seat_state: SeatState,
    _session_lock_state: SessionLockState,
    session_lock: Option<SessionLock>,
    fractional_scale_manager: Option<WpFractionalScaleManagerV1>,
    viewporter: Option<WpViewporter>,
    outputs: Vec<OutputSurface>,
    keyboards: Vec<wl_keyboard::WlKeyboard>,
    pointers: Vec<wl_pointer::WlPointer>,
    pointer_positions: HashMap<u32, (wl_surface::WlSurface, (f64, f64))>,
    touches: Vec<wl_touch::WlTouch>,
    graphics: Graphics,
    profile: Profile,
    lock_state: LockState,
    auth_tx: Sender<AuthResult>,
    auth_rx: Receiver<AuthResult>,
    ready_fd: Option<OwnedFd>,
    modifiers: Modifiers,
    active_layout: u32,
    layout_names: Vec<String>,
    touch_points: HashMap<i32, (wl_surface::WlSurface, (f64, f64))>,
    visual_progress: f32,
    feedback_was_animating: bool,
    last_update: Instant,
    last_clock_minute: u64,
    dirty: bool,
    unlock_pending: bool,
    exit: bool,
    fatal: Option<String>,
}

fn main() {
    aegis_logging::init("info");
    if let Err(error) = run() {
        log::error!("lock: {error}");
        eprintln!("aegis-lock: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let ready_fd = parse_args()?;
    unsafe {
        libc::setlocale(libc::LC_TIME, c"".as_ptr());
    }
    let connection = Connection::connect_to_env()?;
    let (globals, event_queue) = registry_queue_init::<AppData>(&connection)?;
    let qh = event_queue.handle();
    let mut event_loop: EventLoop<AppData> = EventLoop::try_new()?;
    let loop_handle = event_loop.handle();
    let (auth_tx, auth_rx) = mpsc::channel();
    let now = Instant::now();

    let session_lock_state = SessionLockState::new(&globals, &qh);
    let session_lock = session_lock_state.lock(&qh)?;
    // Fractional scaling needs both protocols: the manager carries the
    // compositor's exact per-output scale hint and the viewport maps the
    // physical-pixel buffer onto the logical surface size. Either missing
    // keeps every surface on the integer `wl_output.scale` fallback.
    let fractional_scale_manager: Option<WpFractionalScaleManagerV1> =
        globals.bind(&qh, 1..=1, ()).ok();
    let viewporter: Option<WpViewporter> = globals.bind(&qh, 1..=1, ()).ok();
    if fractional_scale_manager.is_none() || viewporter.is_none() {
        log::info!("lock: fractional scaling unavailable; using integer buffer scale");
    }
    let mut app = AppData {
        loop_handle,
        connection: connection.clone(),
        registry_state: RegistryState::new(&globals),
        compositor_state: CompositorState::bind(&globals, &qh)?,
        output_state: OutputState::new(&globals, &qh),
        seat_state: SeatState::new(&globals, &qh),
        _session_lock_state: session_lock_state,
        session_lock: Some(session_lock),
        fractional_scale_manager,
        viewporter,
        outputs: Vec::new(),
        keyboards: Vec::new(),
        pointers: Vec::new(),
        pointer_positions: HashMap::new(),
        touches: Vec::new(),
        graphics: Graphics::new(&connection)?,
        profile: Profile::current()?,
        lock_state: LockState::new(now),
        auth_tx,
        auth_rx,
        ready_fd,
        modifiers: Modifiers::default(),
        active_layout: 0,
        layout_names: Vec::new(),
        touch_points: HashMap::new(),
        visual_progress: 1.0,
        feedback_was_animating: false,
        last_update: now,
        last_clock_minute: clock_minute(),
        dirty: true,
        unlock_pending: false,
        exit: false,
        fatal: None,
    };

    let initial_outputs: Vec<_> = app.output_state.outputs().collect();
    for output in initial_outputs {
        app.ensure_output(output, &qh);
    }
    WaylandSource::new(connection.clone(), event_queue).insert(event_loop.handle())?;

    while !app.exit {
        let timeout = if app.dirty || app.is_animating() {
            Duration::from_millis(16)
        } else {
            Duration::from_secs(1)
        };
        event_loop.dispatch(timeout, &mut app)?;
        app.advance();
        if app.dirty {
            app.render_all();
        }
        app.maybe_unlock();
    }
    if let Some(error) = app.fatal.take() {
        return Err(error.into());
    }
    Ok(())
}

impl AppData {
    fn ensure_output(&mut self, output: wl_output::WlOutput, qh: &QueueHandle<Self>) {
        if self.outputs.iter().any(|entry| entry.output == output) {
            return;
        }
        let Some(lock) = self.session_lock.clone() else {
            return;
        };
        let surface = self.compositor_state.create_surface(qh);
        let lock_surface = lock.create_lock_surface(surface, &output, qh);
        let integer_scale = self
            .output_state
            .info(&output)
            .map(|info| info.scale_factor.max(1))
            .unwrap_or(1);
        // Density negotiation: with fractional scaling the buffer scale stays
        // at 1, the compositor's `preferred_scale` event carries the exact
        // (possibly fractional) output scale, and a `wp_viewport` destination
        // pins the surface's logical size. Without protocol support, fall
        // back to the integer `wl_output.scale` as the buffer scale.
        let (viewport, fractional_scale) = match (&self.viewporter, &self.fractional_scale_manager)
        {
            (Some(viewporter), Some(manager)) => {
                let viewport = viewporter.get_viewport(lock_surface.wl_surface(), qh, ());
                let fractional = manager.get_fractional_scale(
                    lock_surface.wl_surface(),
                    qh,
                    lock_surface.wl_surface().clone(),
                );
                (Some(viewport), Some(fractional))
            }
            _ => {
                lock_surface.wl_surface().set_buffer_scale(integer_scale);
                (None, None)
            }
        };
        // ext-session-lock configures immediately after the role is created.
        // Unlike xdg-shell, an empty initial commit is forbidden: the first
        // commit must follow ack_configure and already carry a full-size
        // buffer. `configure_surface` creates that target and `render_all`
        // performs the first legal commit.
        self.outputs.push(OutputSurface {
            output,
            lock: lock_surface,
            render: None,
            viewport,
            fractional_scale,
            logical_size: (1, 1),
            scale_120: integer_scale as u32 * 120,
        });
        self.dirty = true;
    }

    fn remove_output(&mut self, output: &wl_output::WlOutput) {
        let Some(index) = self
            .outputs
            .iter()
            .position(|entry| &entry.output == output)
        else {
            return;
        };
        let mut removed = self.outputs.remove(index);
        if let Some(render) = removed.render.take() {
            self.graphics.destroy_surface(render);
        }
        if let Some(fractional) = removed.fractional_scale.take() {
            fractional.destroy();
        }
        if let Some(viewport) = removed.viewport.take() {
            viewport.destroy();
        }
    }

    fn update_output(&mut self, output: &wl_output::WlOutput) {
        let scale = self
            .output_state
            .info(output)
            .map(|info| info.scale_factor.max(1))
            .unwrap_or(1);
        if let Some(entry) = self
            .outputs
            .iter_mut()
            .find(|entry| &entry.output == output)
            // Fractional-scale surfaces follow `preferred_scale` instead.
            && entry.fractional_scale.is_none()
            && entry.scale_120 != scale as u32 * 120
        {
            entry.scale_120 = scale as u32 * 120;
            entry.lock.wl_surface().set_buffer_scale(scale);
            let factor = entry.scale_factor();
            if let Some(render) = &mut entry.render
                && let Err(error) = render.resize(entry.logical_size, factor)
            {
                self.fail(error.to_string());
                return;
            }
            self.dirty = true;
        }
    }

    fn configure_surface(
        &mut self,
        connection: &Connection,
        surface: &SessionLockSurface,
        size: (u32, u32),
    ) {
        let Some(entry) = self
            .outputs
            .iter_mut()
            .find(|entry| entry.lock.wl_surface() == surface.wl_surface())
        else {
            self.fail("received configure for an unknown lock surface".into());
            return;
        };
        entry.logical_size = (size.0.max(1), size.1.max(1));
        // The viewport destination pins the surface's logical size so the
        // buffer can carry physical pixels at any fractional density.
        if let Some(viewport) = &entry.viewport {
            viewport.set_destination(entry.logical_size.0 as i32, entry.logical_size.1 as i32);
        }
        let scale = entry.scale_factor();
        let result = if let Some(render) = &mut entry.render {
            render.resize(entry.logical_size, scale)
        } else {
            self.graphics
                .create_surface(
                    connection,
                    entry.lock.wl_surface(),
                    entry.logical_size,
                    scale,
                )
                .map(|render| {
                    entry.render = Some(render);
                })
        };
        if let Err(error) = result {
            self.fail(error.to_string());
        } else {
            self.dirty = true;
        }
    }

    fn handle_key(&mut self, event: KeyEvent) {
        let now = Instant::now();
        let action = match event.keysym {
            Keysym::Return | Keysym::KP_Enter if self.securely_presented() => {
                self.lock_state.submit(now)
            }
            Keysym::Return | Keysym::KP_Enter => {
                self.lock_state.reveal(now);
                LockAction::None
            }
            Keysym::BackSpace | Keysym::Delete | Keysym::KP_Delete => {
                self.lock_state.backspace(now);
                LockAction::None
            }
            Keysym::Escape => {
                self.lock_state.clear(now);
                LockAction::None
            }
            Keysym::u | Keysym::U if self.modifiers.ctrl => {
                self.lock_state.clear(now);
                LockAction::None
            }
            _ => {
                if let Some(text) = event.utf8 {
                    self.lock_state.type_text(&text, now);
                } else {
                    self.lock_state.reveal(now);
                }
                LockAction::None
            }
        };
        self.handle_action(action);
        self.dirty = true;
    }

    fn handle_action(&mut self, action: LockAction) {
        match action {
            LockAction::None => {}
            LockAction::Authenticate(secret) => auth::authenticate_async(
                self.profile.username.clone(),
                secret,
                self.auth_tx.clone(),
            ),
            LockAction::Unlock => self.unlock_pending = true,
        }
    }

    fn pointer_activity(
        &mut self,
        _surface: &wl_surface::WlSurface,
        _position: (f64, f64),
        _submit: bool,
    ) {
        let now = Instant::now();
        if self.lock_state.reveal(now) {
            self.dirty = true;
        }
    }

    fn securely_presented(&self) -> bool {
        self.session_lock
            .as_ref()
            .is_some_and(SessionLock::is_locked)
    }

    fn advance(&mut self) {
        let now = Instant::now();
        if self.graphics.reload_avatar_if_ready() {
            self.dirty = true;
        }
        while let Ok(result) = self.auth_rx.try_recv() {
            let action = self.lock_state.authentication_finished(result, now);
            self.handle_action(action);
            self.dirty = true;
        }
        let (state_changed, action) = self.lock_state.tick(now);
        if state_changed {
            self.dirty = true;
        }
        self.handle_action(action);
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
            // Authentication results arrive through a polled std channel.
            // Keep dispatch latency bounded even when reduced motion turns
            // the visible validation sweep into a static treatment.
            || self.lock_state.validation_pending()
            || self.graphics.composition_animates(&self.lock_state)
            || self
                .graphics
                .feedback_animation_active(&self.lock_state, Instant::now())
    }

    fn render_all(&mut self) {
        self.dirty = false;
        let now = Instant::now();
        for entry in &mut self.outputs {
            let Some(render) = &mut entry.render else {
                continue;
            };
            if let Err(error) = self.graphics.render(
                render,
                &self.lock_state,
                &self.profile,
                self.visual_progress,
                now,
            ) {
                self.fail(error.to_string());
                break;
            }
        }
    }

    fn maybe_unlock(&mut self) {
        if !self.unlock_pending {
            return;
        }
        let Some(lock) = self.session_lock.as_ref() else {
            return;
        };
        if lock.is_locked() {
            log::info!("lock: authentication succeeded; unlocking session");
            lock.unlock();
            let _ = self.connection.flush();
            self.exit = true;
        }
    }

    fn signal_ready(&mut self) {
        let Some(fd) = self.ready_fd.take() else {
            return;
        };
        let mut file = File::from(fd);
        if let Err(error) = file.write_all(b"\n") {
            log::warn!("lock: failed to signal ready fd: {error}");
        }
    }

    fn fail(&mut self, message: String) {
        log::error!("lock: {message}");
        self.fatal.get_or_insert(message);
        self.exit = true;
    }
}

impl Drop for AppData {
    fn drop(&mut self) {
        let outputs = std::mem::take(&mut self.outputs);
        for mut output in outputs {
            if let Some(render) = output.render.take() {
                self.graphics.destroy_surface(render);
            }
        }
    }
}

impl SessionLockHandler for AppData {
    fn locked(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _session_lock: SessionLock,
    ) {
        log::info!("lock: compositor confirmed secure presentation on all outputs");
        self.signal_ready();
        self.dirty = true;
    }

    fn finished(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _session_lock: SessionLock,
    ) {
        if !self.exit {
            self.fail("compositor denied or terminated the session lock".into());
        }
    }

    fn configure(
        &mut self,
        connection: &Connection,
        _qh: &QueueHandle<Self>,
        surface: SessionLockSurface,
        configure: SessionLockSurfaceConfigure,
        _serial: u32,
    ) {
        self.configure_surface(connection, &surface, configure.new_size);
    }
}

impl OutputHandler for AppData {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _connection: &Connection,
        qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        self.ensure_output(output, qh);
    }

    fn update_output(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        self.update_output(&output);
    }

    fn output_destroyed(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        self.remove_output(&output);
    }
}

impl CompositorHandler for AppData {
    fn scale_factor_changed(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        factor: i32,
    ) {
        if let Some(entry) = self
            .outputs
            .iter_mut()
            .find(|entry| entry.lock.wl_surface() == surface)
            // Fractional-scale surfaces follow `preferred_scale` instead.
            && entry.fractional_scale.is_none()
        {
            entry.scale_120 = factor.max(1) as u32 * 120;
            entry.lock.wl_surface().set_buffer_scale(factor.max(1));
            let scale = entry.scale_factor();
            if let Some(render) = &mut entry.render
                && let Err(error) = render.resize(entry.logical_size, scale)
            {
                self.fail(error.to_string());
            }
            self.dirty = true;
        }
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

impl Dispatch<WpFractionalScaleManagerV1, ()> for AppData {
    fn event(
        _state: &mut Self,
        _proxy: &WpFractionalScaleManagerV1,
        _event: wp_fractional_scale_manager_v1::Event,
        _data: &(),
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // The manager has no events.
    }
}

impl Dispatch<WpViewporter, ()> for AppData {
    fn event(
        _state: &mut Self,
        _proxy: &WpViewporter,
        _event: wp_viewporter::Event,
        _data: &(),
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // The viewporter has no events.
    }
}

impl Dispatch<WpViewport, ()> for AppData {
    fn event(
        _state: &mut Self,
        _proxy: &WpViewport,
        _event: wp_viewport::Event,
        _data: &(),
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // The viewport has no events.
    }
}

impl Dispatch<WpFractionalScaleV1, wl_surface::WlSurface> for AppData {
    fn event(
        state: &mut Self,
        _proxy: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        surface: &wl_surface::WlSurface,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            let scale_120 = scale.max(1);
            let Some(entry) = state
                .outputs
                .iter_mut()
                .find(|entry| entry.lock.wl_surface() == surface)
            else {
                return;
            };
            if entry.scale_120 == scale_120 {
                return;
            }
            entry.scale_120 = scale_120;
            let scale = entry.scale_factor();
            if let Some(render) = &mut entry.render
                && let Err(error) = render.resize(entry.logical_size, scale)
            {
                state.fail(error.to_string());
                return;
            }
            state.dirty = true;
        }
    }
}

impl SeatHandler for AppData {
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
                let loop_handle = self.loop_handle.clone();
                let keyboard = self.seat_state.get_keyboard_with_repeat(
                    qh,
                    &seat,
                    None,
                    loop_handle,
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
            Capability::Pointer => {
                self.pointers.clear();
                self.pointer_positions.clear();
            }
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

impl KeyboardHandler for AppData {
    fn enter(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _surface: &wl_surface::WlSurface,
        _serial: u32,
        _raw: &[u32],
        _keysyms: &[Keysym],
    ) {
        if self.lock_state.reveal(Instant::now()) {
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

impl PointerHandler for AppData {
    fn pointer_frame(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        let pointer_id = pointer.id().protocol_id();
        for event in events {
            match &event.kind {
                PointerEventKind::Enter { .. } => {
                    self.pointer_positions
                        .insert(pointer_id, (event.surface.clone(), event.position));
                    self.pointer_activity(&event.surface, event.position, false);
                }
                PointerEventKind::Leave { .. } => {
                    self.pointer_positions.remove(&pointer_id);
                }
                PointerEventKind::Motion { .. } => {
                    let changed = self.pointer_positions.get(&pointer_id).is_none_or(
                        |(surface, position)| {
                            surface != &event.surface || *position != event.position
                        },
                    );
                    self.pointer_positions
                        .insert(pointer_id, (event.surface.clone(), event.position));
                    if changed {
                        self.pointer_activity(&event.surface, event.position, false);
                    }
                }
                PointerEventKind::Press { button, .. } => {
                    self.pointer_activity(&event.surface, event.position, *button == 0x110);
                }
                PointerEventKind::Axis {
                    horizontal,
                    vertical,
                    ..
                } if !horizontal.is_none() || !vertical.is_none() => {
                    self.pointer_activity(&event.surface, event.position, false);
                }
                PointerEventKind::Release { .. } | PointerEventKind::Axis { .. } => {}
            }
        }
    }
}

impl TouchHandler for AppData {
    fn down(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _touch: &wl_touch::WlTouch,
        _serial: u32,
        _time: u32,
        surface: wl_surface::WlSurface,
        id: i32,
        position: (f64, f64),
    ) {
        self.touch_points.insert(id, (surface.clone(), position));
        self.pointer_activity(&surface, position, true);
    }

    fn up(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _touch: &wl_touch::WlTouch,
        _serial: u32,
        _time: u32,
        id: i32,
    ) {
        self.touch_points.remove(&id);
    }

    fn motion(
        &mut self,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
        _touch: &wl_touch::WlTouch,
        _time: u32,
        id: i32,
        position: (f64, f64),
    ) {
        let surface = self
            .touch_points
            .get(&id)
            .map(|(surface, _)| surface.clone());
        if let Some(surface) = surface {
            self.touch_points.insert(id, (surface.clone(), position));
            self.pointer_activity(&surface, position, false);
        }
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
        self.touch_points.clear();
    }
}

impl ProvidesRegistryState for AppData {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState, SeatState];
}

smithay_client_toolkit::delegate_registry!(AppData);
smithay_client_toolkit::delegate_dispatch2!(AppData);

fn parse_args() -> Result<Option<OwnedFd>, Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut ready_fd = None;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--help" | "-h" => {
                println!(
                    "Usage: aegis-lock [--ready-fd FD]\n\n\
                     Locks the current Aegis Wayland session. The ready fd is\n\
                     signalled only after every output has presented securely."
                );
                std::process::exit(0);
            }
            "--version" | "-V" => {
                println!("aegis-lock {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "--ready-fd" => {
                let value = args.next().ok_or("--ready-fd requires a descriptor")?;
                ready_fd = Some(owned_fd(&value)?);
            }
            value if value.starts_with("--ready-fd=") => {
                ready_fd = Some(owned_fd(&value["--ready-fd=".len()..])?);
            }
            _ => return Err(format!("unknown argument: {argument}").into()),
        }
    }
    Ok(ready_fd)
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

fn clock_minute() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 60
}
