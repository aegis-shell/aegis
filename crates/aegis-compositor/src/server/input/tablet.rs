use crate::*;

impl Server {
    /// Route one tablet tool event (zwp_tablet-unstable-v2). The first
    /// proximity of a physical tool announces the compositor's synthetic
    /// tablet device and the tool to every bound seat; from then on, a pen
    /// over a surface whose client holds a tablet seat receives the full
    /// protocol stream (proximity/axes/tip/button, each burst closed by
    /// `frame`) on that client's tool resource, with surface-local
    /// coordinates computed exactly like `touch_down`. Over any other
    /// surface the pen falls back to emulating the pointer — motion plus
    /// BTN_LEFT for the tip — so tablet-unaware clients still work.
    pub(crate) fn tablet_event(&mut self, event: aegis_model::input::TabletEvent) {
        use aegis_model::input::TabletEvent::*;
        match event {
            Proximity {
                tool,
                info,
                in_proximity: true,
                x,
                y,
                ..
            } => self.tablet_proximity_in(tool, info, x, y),
            Proximity {
                tool,
                in_proximity: false,
                time,
                ..
            } => self.tablet_proximity_out(tool, time),
            Axes {
                tool,
                x,
                y,
                pressure,
                distance,
                tilt,
                rotation,
                slider,
                wheel,
                time,
            } => self.tablet_axes(
                tool, x, y, pressure, distance, tilt, rotation, slider, wheel, time,
            ),
            Tip { tool, state, time } => self.tablet_tip(tool, state, time),
            Button {
                tool,
                button,
                state,
                time,
            } => self.tablet_button(tool, button, state, time),
        }
    }

    /// The `zwp_tablet_tool_v2` resource owned by `client` that proxies
    /// physical tool `tool`, or null.
    pub(crate) fn tablet_tool_for(
        &self,
        client: *mut ffi::wl_client,
        tool: u64,
    ) -> *mut ffi::wl_resource {
        self.state
            .tablet_tools
            .iter()
            .copied()
            .filter(|p| !p.is_null())
            .filter(|p| unsafe { ffi::wl_resource_get_client(*p) == client })
            .find(|p| unsafe {
                let rec = ffi::wl_resource_get_user_data(*p) as *mut extensions::TabletToolRec;
                !rec.is_null() && (*rec).tool == tool
            })
            .unwrap_or(std::ptr::null_mut())
    }

    /// Any live `zwp_tablet_v2` resource owned by `client`. One synthetic
    /// tablet exists, so the tablet/tool pairing is implicit per client.
    pub(crate) fn tablet_device_for(&self, client: *mut ffi::wl_client) -> *mut ffi::wl_resource {
        self.state
            .tablet_devices
            .iter()
            .copied()
            .filter(|p| !p.is_null())
            .find(|p| unsafe { ffi::wl_resource_get_client(*p) == client })
            .unwrap_or(std::ptr::null_mut())
    }

    /// A tool entered proximity: announce the device/tool to late-bound
    /// seats, then either open protocol proximity on the hit surface or
    /// emulate pointer motion when its client has no tablet seat.
    pub(crate) fn tablet_proximity_in(
        &mut self,
        tool: u64,
        info: aegis_model::input::TabletToolInfo,
        x: f32,
        y: f32,
    ) {
        // Clone the seat list so the announce calls can re-borrow `state`.
        let seats: Vec<*mut ffi::wl_resource> = self
            .state
            .tablet_seats
            .iter()
            .copied()
            .filter(|s| !s.is_null())
            .collect();
        // A tool must follow a tablet, so a never-announced device goes out
        // to every seat before the tool itself.
        if !self.state.tablet_device_seen {
            self.state.tablet_device_seen = true;
            for seat in &seats {
                unsafe { extensions::announce_tablet(self.state.as_mut(), *seat) };
            }
        }
        if !self.state.known_tools.iter().any(|(id, _)| *id == tool) {
            self.state.known_tools.push((tool, info));
            for seat in &seats {
                unsafe { extensions::announce_tool(self.state.as_mut(), *seat, tool, &info) };
            }
        }
        // Keep chrome/cursor tracking the pen.
        self.state.pointer_x = x;
        self.state.pointer_y = y;
        let focus = self.hit_test_focus(x, y);
        let focus_client = if focus.is_null() {
            std::ptr::null_mut()
        } else {
            unsafe { ffi::wl_resource_get_client(focus) }
        };
        let has_seat = !focus_client.is_null()
            && seats
                .iter()
                .any(|s| unsafe { ffi::wl_resource_get_client(*s) } == focus_client);
        if !has_seat {
            self.state.tablet_focus = std::ptr::null_mut();
            self.pointer_motion(x, y);
            return;
        }
        self.state.tablet_focus = focus;
        let serial = unsafe { ffi::wl_display_next_serial(self.state.display) };
        let tablet = self.tablet_device_for(focus_client);
        let tool_res = self.tablet_tool_for(focus_client, tool);
        if tablet.is_null() || tool_res.is_null() {
            return;
        }
        unsafe {
            ffi::wl_resource_post_event(
                tool_res,
                ffi::ZWP_TABLET_TOOL_V2_PROXIMITY_IN,
                serial,
                tablet,
                focus,
            );
        }
    }

    /// The tool left proximity: close the burst on the focus client's tool
    /// resource and drop tablet focus.
    pub(crate) fn tablet_proximity_out(&mut self, tool: u64, time: u32) {
        if self.state.tablet_focus.is_null() {
            return;
        }
        let focus_client = unsafe { ffi::wl_resource_get_client(self.state.tablet_focus) };
        let tool_res = self.tablet_tool_for(focus_client, tool);
        self.state.tablet_focus = std::ptr::null_mut();
        if tool_res.is_null() {
            return;
        }
        unsafe {
            ffi::wl_resource_post_event(tool_res, ffi::ZWP_TABLET_TOOL_V2_PROXIMITY_OUT);
            ffi::wl_resource_post_event(tool_res, ffi::ZWP_TABLET_TOOL_V2_FRAME, time);
        }
    }

    /// Axis updates for the in-proximity tool: motion plus whichever of
    /// pressure/distance/tilt/rotation/slider/wheel the backend reported,
    /// closed by `frame`. Pressure and distance are normalized f32 0.0..1.0;
    /// the protocol wants uint 0..65535.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn tablet_axes(
        &mut self,
        tool: u64,
        x: f32,
        y: f32,
        pressure: Option<f32>,
        distance: Option<f32>,
        tilt: Option<(f32, f32)>,
        rotation: Option<f32>,
        slider: Option<f32>,
        wheel: Option<(f32, i32)>,
        time: u32,
    ) {
        if self.state.tablet_focus.is_null() {
            // Emulated path: the pen drives the pointer.
            self.pointer_motion(x, y);
            return;
        }
        self.state.pointer_x = x;
        self.state.pointer_y = y;
        let focus = self.state.tablet_focus;
        let focus_client = unsafe { ffi::wl_resource_get_client(focus) };
        let tool_res = self.tablet_tool_for(focus_client, tool);
        if tool_res.is_null() {
            return;
        }
        let rec = unsafe { ffi::wl_resource_get_user_data(focus) as *mut SurfaceRec };
        let origin = if rec.is_null() {
            aegis_model::Point::default()
        } else {
            unsafe { surface_draw_origin(&*rec) }
        };
        let fx = ffi::wl_fixed_from_f32(x - origin.x as f32);
        let fy = ffi::wl_fixed_from_f32(y - origin.y as f32);
        unsafe {
            ffi::wl_resource_post_event(tool_res, ffi::ZWP_TABLET_TOOL_V2_MOTION, fx, fy);
            if let Some(p) = pressure {
                ffi::wl_resource_post_event(
                    tool_res,
                    ffi::ZWP_TABLET_TOOL_V2_PRESSURE,
                    (p.clamp(0.0, 1.0) * 65535.0) as u32,
                );
            }
            if let Some(d) = distance {
                ffi::wl_resource_post_event(
                    tool_res,
                    ffi::ZWP_TABLET_TOOL_V2_DISTANCE,
                    (d.clamp(0.0, 1.0) * 65535.0) as u32,
                );
            }
            if let Some((tx, ty)) = tilt {
                ffi::wl_resource_post_event(
                    tool_res,
                    ffi::ZWP_TABLET_TOOL_V2_TILT,
                    ffi::wl_fixed_from_f32(tx),
                    ffi::wl_fixed_from_f32(ty),
                );
            }
            if let Some(r) = rotation {
                ffi::wl_resource_post_event(
                    tool_res,
                    ffi::ZWP_TABLET_TOOL_V2_ROTATION,
                    ffi::wl_fixed_from_f32(r),
                );
            }
            if let Some(s) = slider {
                ffi::wl_resource_post_event(
                    tool_res,
                    ffi::ZWP_TABLET_TOOL_V2_SLIDER,
                    ffi::wl_fixed_from_f32(s),
                );
            }
            if let Some((degrees, clicks)) = wheel {
                ffi::wl_resource_post_event(
                    tool_res,
                    ffi::ZWP_TABLET_TOOL_V2_WHEEL,
                    ffi::wl_fixed_from_f32(degrees),
                    clicks,
                );
            }
            ffi::wl_resource_post_event(tool_res, ffi::ZWP_TABLET_TOOL_V2_FRAME, time);
        }
    }

    /// Tip down/up: protocol `down`/`up` on the focus client's tool resource
    /// (with click-to-focus parity on down), or a BTN_LEFT click on the
    /// emulated pointer path.
    pub(crate) fn tablet_tip(
        &mut self,
        tool: u64,
        state: aegis_model::input::ButtonState,
        time: u32,
    ) {
        const BTN_LEFT: u32 = 0x110;
        if self.state.tablet_focus.is_null() {
            self.pointer_button(BTN_LEFT, state);
            return;
        }
        if state.is_pressed() {
            self.change_keyboard_focus(self.state.tablet_focus);
        }
        let focus_client = unsafe { ffi::wl_resource_get_client(self.state.tablet_focus) };
        let tool_res = self.tablet_tool_for(focus_client, tool);
        if tool_res.is_null() {
            return;
        }
        unsafe {
            if state.is_pressed() {
                let serial = ffi::wl_display_next_serial(self.state.display);
                ffi::wl_resource_post_event(tool_res, ffi::ZWP_TABLET_TOOL_V2_DOWN, serial);
            } else {
                ffi::wl_resource_post_event(tool_res, ffi::ZWP_TABLET_TOOL_V2_UP);
            }
            ffi::wl_resource_post_event(tool_res, ffi::ZWP_TABLET_TOOL_V2_FRAME, time);
        }
    }

    /// A stylus button: protocol `button` + `frame` on the focus client's
    /// tool resource. No-op without tablet focus (the emulated pointer path
    /// has no stylus buttons).
    pub(crate) fn tablet_button(
        &mut self,
        tool: u64,
        button: u32,
        state: aegis_model::input::ButtonState,
        time: u32,
    ) {
        if self.state.tablet_focus.is_null() {
            return;
        }
        let focus_client = unsafe { ffi::wl_resource_get_client(self.state.tablet_focus) };
        let tool_res = self.tablet_tool_for(focus_client, tool);
        if tool_res.is_null() {
            return;
        }
        let serial = unsafe { ffi::wl_display_next_serial(self.state.display) };
        unsafe {
            ffi::wl_resource_post_event(
                tool_res,
                ffi::ZWP_TABLET_TOOL_V2_BUTTON,
                serial,
                button,
                u32::from(state.is_pressed()),
            );
            ffi::wl_resource_post_event(tool_res, ffi::ZWP_TABLET_TOOL_V2_FRAME, time);
        }
    }
}
