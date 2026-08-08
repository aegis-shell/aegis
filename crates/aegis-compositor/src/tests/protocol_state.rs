use super::*;

#[test]
fn session_lock_phase_requires_a_secure_presentation_receipt() {
    let now = std::time::Instant::now();
    let mut phase = SessionLockPhase::Unlocked;
    phase.begin(now);
    assert!(phase.is_active());
    assert!(!phase.is_confirmed());
    assert!(!phase.secure_frame_presented());

    phase.request_secure_frame();
    assert!(phase.frame_pending());
    assert!(phase.secure_frame_presented());
    assert!(phase.is_confirmed());
    assert!(!phase.frame_pending());
}

#[test]
fn session_lock_phase_retires_fallback_frames_without_unlocking() {
    let now = std::time::Instant::now();
    let mut phase = SessionLockPhase::Unlocked;
    phase.begin(now);
    phase.request_secure_frame();
    assert!(phase.secure_frame_presented());

    phase.request_secure_frame();
    assert!(phase.frame_pending());
    assert!(!phase.secure_frame_presented());
    assert!(phase.is_confirmed());

    phase.unlock();
    assert_eq!(phase, SessionLockPhase::Unlocked);
}

#[test]
fn session_lock_surface_grace_only_advances_securing_phase() {
    let now = std::time::Instant::now();
    let grace = std::time::Duration::from_secs(1);
    let mut phase = SessionLockPhase::Unlocked;
    phase.begin(now);
    phase.expire_surface_grace(now + grace / 2, grace);
    assert!(!phase.frame_pending());
    phase.expire_surface_grace(now + grace, grace);
    assert!(phase.frame_pending());
    assert!(phase.secure_frame_presented());

    phase.expire_surface_grace(now + grace * 2, grace);
    assert!(!phase.frame_pending());
}

#[test]
fn destroyed_surfaces_revoke_saved_session_lock_focus() {
    let mut state = State::new(std::ptr::null_mut());
    let destroyed = 0x10usize as *mut ffi::wl_resource;
    let survivor = 0x20usize as *mut ffi::wl_resource;

    state.pre_lock_keyboard_focus = destroyed;
    state.pending_lock_focus = survivor;
    revoke_session_lock_focus(&mut state, destroyed);
    assert!(state.pre_lock_keyboard_focus.is_null());
    assert_eq!(state.pending_lock_focus, survivor);
    assert!(!state.lock_focus_dirty);

    revoke_session_lock_focus(&mut state, survivor);
    assert!(state.pending_lock_focus.is_null());
    assert!(state.lock_focus_dirty);
}

#[test]
fn dmabuf_buffer_ids_are_nonzero_and_monotonic() {
    let first = DmabufBuffer::empty(std::ptr::null_mut());
    let second = DmabufBuffer::empty(std::ptr::null_mut());
    assert_ne!(first.buffer_id, 0);
    assert!(second.buffer_id > first.buffer_id);
}

#[test]
fn interaction_domain_registry_filter_isolates_outputs_and_physical_authority_globals() {
    let mut state = State::new(std::ptr::null_mut());
    let bundle = state
        .authority
        .create_agent_interaction_domain("filter-agent", SeatCapabilities::POINTER_KEYBOARD);
    let client_id = state.authority.register_client(Some(format!(
        "aegis.interaction_domain.{}",
        bundle.interaction_domain.0
    )));
    let interaction_domain_client = 0x10usize as *mut ffi::wl_client;
    let human_client = 0x20usize as *mut ffi::wl_client;
    state
        .clients
        .insert(interaction_domain_client as usize, client_id);
    state
        .client_initial_interaction_domains
        .insert(client_id, bundle.interaction_domain);

    let physical_global = 0x100usize as *mut ffi::wl_global;
    let virtual_global = 0x200usize as *mut ffi::wl_global;
    let session_global = 0x300usize as *mut ffi::wl_global;
    let info = state.output_infos[0].clone();
    state.output_globals.push(Box::new(OutputGlobal {
        state: std::ptr::null_mut(),
        info: info.clone(),
        interaction_domain: None,
        global: physical_global,
        active: true,
    }));
    state.output_globals.push(Box::new(OutputGlobal {
        state: std::ptr::null_mut(),
        info,
        interaction_domain: Some(bundle.interaction_domain),
        global: virtual_global,
        active: true,
    }));
    state
        .interaction_domain_hidden_globals
        .insert(session_global as usize);
    let data = (&mut state as *mut State).cast();

    unsafe {
        assert!(interaction_domain_global_filter(
            interaction_domain_client,
            virtual_global,
            data
        ));
        assert!(!interaction_domain_global_filter(
            interaction_domain_client,
            physical_global,
            data
        ));
        assert!(!interaction_domain_global_filter(
            interaction_domain_client,
            session_global,
            data
        ));
        assert!(interaction_domain_global_filter(
            human_client,
            physical_global,
            data
        ));
        assert!(interaction_domain_global_filter(
            human_client,
            virtual_global,
            data
        ));
        assert!(interaction_domain_global_filter(
            human_client,
            session_global,
            data
        ));
    }
}

#[test]
fn finger_scroll_updates_do_not_emit_stop_or_discrete_steps() {
    let frame = aegis_model::input::PointerAxisFrame::from_values(
        42,
        Some(aegis_model::input::PointerAxisSource::Finger),
        0.0,
        1.25,
    );
    assert_eq!(
        pointer_axis_wire_events(9, frame),
        vec![
            PointerAxisWireEvent::Source(ffi::WL_POINTER_AXIS_SOURCE_FINGER),
            PointerAxisWireEvent::Axis {
                time: 42,
                axis: ffi::WL_POINTER_AXIS_VERTICAL_SCROLL,
                value: 1.25,
            },
            PointerAxisWireEvent::Frame,
        ]
    );
}

#[test]
fn finger_scroll_stop_is_emitted_only_for_the_terminal_frame() {
    let frame = aegis_model::input::PointerAxisFrame {
        time: 55,
        source: Some(aegis_model::input::PointerAxisSource::Finger),
        vertical: aegis_model::input::PointerAxis {
            stop: true,
            ..aegis_model::input::PointerAxis::default()
        },
        ..aegis_model::input::PointerAxisFrame::default()
    };
    assert_eq!(
        pointer_axis_wire_events(9, frame),
        vec![
            PointerAxisWireEvent::Source(ffi::WL_POINTER_AXIS_SOURCE_FINGER),
            PointerAxisWireEvent::Stop {
                time: 55,
                axis: ffi::WL_POINTER_AXIS_VERTICAL_SCROLL,
            },
            PointerAxisWireEvent::Frame,
        ]
    );
}

#[test]
fn wheel_metadata_precedes_axis_and_preserves_direction() {
    let frame = aegis_model::input::PointerAxisFrame {
        time: 77,
        source: Some(aegis_model::input::PointerAxisSource::Wheel),
        vertical: aegis_model::input::PointerAxis {
            value: Some(-10.0),
            discrete: Some(-1),
            value120: Some(-120),
            ..aegis_model::input::PointerAxis::default()
        },
        ..aegis_model::input::PointerAxisFrame::default()
    };
    assert_eq!(
        pointer_axis_wire_events(9, frame),
        vec![
            PointerAxisWireEvent::Source(ffi::WL_POINTER_AXIS_SOURCE_WHEEL),
            PointerAxisWireEvent::Value120 {
                axis: ffi::WL_POINTER_AXIS_VERTICAL_SCROLL,
                value: -120,
            },
            PointerAxisWireEvent::Axis {
                time: 77,
                axis: ffi::WL_POINTER_AXIS_VERTICAL_SCROLL,
                value: -10.0,
            },
            PointerAxisWireEvent::Frame,
        ]
    );
    assert_eq!(
        pointer_axis_wire_events(7, frame),
        vec![
            PointerAxisWireEvent::Source(ffi::WL_POINTER_AXIS_SOURCE_WHEEL),
            PointerAxisWireEvent::Discrete {
                axis: ffi::WL_POINTER_AXIS_VERTICAL_SCROLL,
                value: -1,
            },
            PointerAxisWireEvent::Axis {
                time: 77,
                axis: ffi::WL_POINTER_AXIS_VERTICAL_SCROLL,
                value: -10.0,
            },
            PointerAxisWireEvent::Frame,
        ]
    );
}

#[test]
fn legacy_output_scale_rounds_fractional_values_up() {
    assert_eq!(integer_output_scale(1.0), 1);
    assert_eq!(integer_output_scale(1.25), 2);
    assert_eq!(integer_output_scale(1.5), 2);
    assert_eq!(integer_output_scale(2.0), 2);
}

#[test]
fn interaction_domain_damage_is_clipped_deduplicated_and_bounded() {
    let output = aegis_model::Rect::new(0, 0, 100, 80);
    let mut damage = vec![
        aegis_model::Rect::new(-10, -10, 20, 20),
        aegis_model::Rect::new(-10, -10, 20, 20),
        aegis_model::Rect::new(90, 70, 30, 30),
        aegis_model::Rect::new(150, 150, 10, 10),
    ];
    normalize_interaction_domain_damage(&mut damage, output);
    assert_eq!(
        damage,
        vec![
            aegis_model::Rect::new(0, 0, 10, 10),
            aegis_model::Rect::new(90, 70, 10, 10),
        ]
    );

    let mut many = (0..65)
        .map(|x| aegis_model::Rect::new(x, 0, 1, 1))
        .collect::<Vec<_>>();
    normalize_interaction_domain_damage(&mut many, output);
    assert_eq!(many, vec![aegis_model::Rect::new(0, 0, 65, 1)]);
}

#[test]
fn compositor_resize_edges_map_to_protocol_cursor_shapes() {
    use aegis_model::window::ResizeEdges;

    assert_eq!(resize_cursor_shape(ResizeEdges::LEFT), 25);
    assert_eq!(resize_cursor_shape(ResizeEdges::RIGHT), 18);
    assert_eq!(
        resize_cursor_shape(ResizeEdges(ResizeEdges::TOP.0 | ResizeEdges::LEFT.0)),
        21
    );
    assert_eq!(
        resize_cursor_shape(ResizeEdges(ResizeEdges::BOTTOM.0 | ResizeEdges::RIGHT.0)),
        23
    );
}

#[test]
fn explicit_geometry_size_respects_client_hints() {
    let hints = aegis_model::window::SizeHints {
        min_w: 320,
        min_h: 200,
        max_w: 1920,
        max_h: 1080,
    };
    assert_eq!(
        clamp_size_to_hints(aegis_model::Size { w: 100, h: 2_000 }, hints),
        aegis_model::Size { w: 320, h: 1080 }
    );
    assert_eq!(
        clamp_size_to_hints(
            aegis_model::Size { w: 800, h: 600 },
            aegis_model::window::SizeHints::default(),
        ),
        aegis_model::Size { w: 800, h: 600 }
    );
}

#[test]
fn logical_surface_size_applies_transform_scale_and_viewport_in_order() {
    let mut surface = SurfaceRec::new(std::ptr::null_mut());
    surface.width = 400;
    surface.height = 200;
    surface.buffer_scale = 2;
    assert_eq!(
        surface_logical_size(&surface),
        aegis_model::Size { w: 200, h: 100 }
    );

    surface.buffer_transform = aegis_model::Transform::Rotate90;
    assert_eq!(
        surface_logical_size(&surface),
        aegis_model::Size { w: 100, h: 200 }
    );

    // Viewport source coordinates are after transform and buffer scale,
    // so they are already surface-local and must not be divided again.
    surface.viewport_src = Some(aegis_model::Rect::new(5, 7, 80, 60));
    assert_eq!(
        surface_logical_size(&surface),
        aegis_model::Size { w: 80, h: 60 }
    );

    surface.viewport_dst = Some(aegis_model::Size { w: 123, h: 45 });
    assert_eq!(
        surface_logical_size(&surface),
        aegis_model::Size { w: 123, h: 45 }
    );
}

#[test]
fn viewport_source_unset_uses_wire_encoded_fixed_negative_one() {
    assert_eq!(decode_viewport_source(-256, -256, -256, -256), Ok(None));
    assert!(decode_viewport_source(-1, -1, -1, -1).is_err());
    assert!(decode_viewport_source(-256, -256, 256, 256).is_err());
}

#[test]
fn viewport_source_decodes_positive_fixed_coordinates() {
    assert_eq!(
        decode_viewport_source(384, 576, 2_560, 5_120),
        Ok(Some(aegis_model::Rect::new(2, 2, 10, 20)))
    );
}

#[test]
fn draw_origin_subtracts_window_geometry_insets() {
    let mut surface = SurfaceRec::new(std::ptr::null_mut());
    surface.position = aegis_model::Point { x: 100, y: 60 };

    // No declared geometry: the buffer draws at the window-rect origin.
    assert_eq!(
        surface_draw_origin(&surface),
        aegis_model::Point { x: 100, y: 60 }
    );

    // CSD insets: the buffer extends up-left of the window rect.
    surface.window_geometry = Some(aegis_model::Rect::new(20, 10, 400, 300));
    assert_eq!(
        surface_draw_origin(&surface),
        aegis_model::Point { x: 80, y: 50 }
    );
}

#[test]
fn draw_origin_walks_nested_subsurface_chains() {
    let mut root = SurfaceRec::new(std::ptr::null_mut());
    root.position = aegis_model::Point { x: 100, y: 60 };
    // A CSD root: the chain anchors at the buffer draw origin.
    root.window_geometry = Some(aegis_model::Rect::new(20, 10, 400, 300));

    let mut child = SurfaceRec::new(std::ptr::null_mut());
    child.parent = &mut root;
    child.subsurface_offset = aegis_model::Point { x: 10, y: 5 };
    let mut grandchild = SurfaceRec::new(std::ptr::null_mut());
    grandchild.parent = &mut child;
    grandchild.subsurface_offset = aegis_model::Point { x: 3, y: 2 };

    // 100-20+10+3, 60-10+5+2: offsets accumulate in each parent's
    // buffer space down to the root's draw origin.
    assert_eq!(
        surface_draw_origin(&grandchild),
        aegis_model::Point { x: 93, y: 57 }
    );
    assert_eq!(
        surface_draw_origin(&child),
        aegis_model::Point { x: 90, y: 55 }
    );

    // Detaching (wl_subsurface.destroy / parent destroyed) stops the walk.
    grandchild.parent = std::ptr::null_mut();
    assert_eq!(
        surface_draw_origin(&grandchild),
        aegis_model::Point::default()
    );
}

#[test]
fn accepts_point_uses_buffer_space_for_subsurfaces() {
    let mut root = SurfaceRec::new(std::ptr::null_mut());
    root.position = aegis_model::Point { x: 100, y: 60 };
    let mut child = SurfaceRec::new(std::ptr::null_mut());
    child.parent = &mut root;
    child.subsurface_offset = aegis_model::Point { x: 10, y: 5 };
    child.width = 40;
    child.height = 30;
    child.buffer_scale = 1;

    // Inside the child's buffer rect (anchored at 110,65).
    assert!(surface_accepts_point(&child, 120.0, 70.0));
    // Outside it, but inside the parent's rect — the parent test would
    // catch that point instead.
    assert!(!surface_accepts_point(&child, 105.0, 62.0));

    // An input region further restricts the accepted area, in
    // buffer-local coordinates.
    child.input_region = Some(vec![aegis_model::Rect::new(0, 0, 20, 30)]);
    assert!(surface_accepts_point(&child, 115.0, 70.0));
    assert!(!surface_accepts_point(&child, 135.0, 70.0));
}

#[test]
fn region_subtraction_preserves_the_uncut_area() {
    let pieces = subtract_rect(
        aegis_model::Rect::new(0, 0, 100, 100),
        aegis_model::Rect::new(20, 20, 60, 60),
    );
    assert_eq!(pieces.len(), 4);
    let area: i32 = pieces.iter().map(|rect| rect.size.w * rect.size.h).sum();
    assert_eq!(area, 10_000 - 3_600);
    assert!(
        pieces
            .iter()
            .all(|rect| !rect.contains(aegis_model::Point { x: 50, y: 50 }))
    );
}

#[test]
fn dnd_action_negotiation_honors_preference_then_fallback_order() {
    let all = ffi::WL_DATA_ACTION_COPY | ffi::WL_DATA_ACTION_MOVE | ffi::WL_DATA_ACTION_ASK;
    assert_eq!(
        choose_dnd_action(all, all, ffi::WL_DATA_ACTION_MOVE),
        ffi::WL_DATA_ACTION_MOVE
    );
    assert_eq!(
        choose_dnd_action(all, ffi::WL_DATA_ACTION_COPY | ffi::WL_DATA_ACTION_ASK, 0),
        ffi::WL_DATA_ACTION_COPY
    );
    assert_eq!(
        choose_dnd_action(ffi::WL_DATA_ACTION_MOVE, ffi::WL_DATA_ACTION_COPY, 0),
        ffi::WL_DATA_ACTION_NONE
    );
}

#[test]
fn layout_role_resolution_prefers_rule_then_transient_then_workspace() {
    use aegis_model::layout::LayoutRole;
    // An explicit window rule always wins (even over a transient).
    assert_eq!(
        resolve_layout_role(true, true, Some(LayoutRole::Tiled)),
        LayoutRole::Tiled
    );
    assert_eq!(
        resolve_layout_role(true, false, Some(LayoutRole::Floating)),
        LayoutRole::Floating
    );
    // No rule: a transient (dialog) floats even on a tiled workspace.
    assert_eq!(resolve_layout_role(true, true, None), LayoutRole::Floating);
    assert_eq!(resolve_layout_role(false, true, None), LayoutRole::Floating);
    // No rule, not transient: the workspace's tiled flag decides.
    assert_eq!(resolve_layout_role(true, false, None), LayoutRole::Tiled);
    assert_eq!(
        resolve_layout_role(false, false, None),
        LayoutRole::Floating
    );
}

#[test]
fn initial_size_restores_main_windows_but_not_same_app_transients() {
    let mut state = State::new(std::ptr::null_mut());
    state.window_state_store.update(
        "com.example.App".to_owned(),
        window_state::SavedWindowState {
            size: Some(aegis_model::Size { w: 960, h: 720 }),
            ..Default::default()
        },
    );
    let mut surface = SurfaceRec::new(std::ptr::null_mut());
    surface.state = &mut state;
    surface.window.app_id = Some("com.example.App".to_owned());

    assert_eq!(
        unsafe { initial_toplevel_size(&mut surface) },
        Some(aegis_model::Size { w: 960, h: 720 })
    );

    surface.window.parent = Some(0x1234);
    assert_eq!(unsafe { initial_toplevel_size(&mut surface) }, None);
}

#[test]
fn transient_centering_uses_parent_geometry_and_output_origin() {
    let parent = aegis_model::Rect::new(500, 200, 800, 600);
    let output = aegis_model::Rect::new(320, 100, 1200, 800);
    assert_eq!(
        centered_transient_position(parent, aegis_model::Size { w: 400, h: 200 }, output),
        aegis_model::Point { x: 700, y: 400 }
    );

    // A parent partly beyond the output would center the child off-screen;
    // the compositor keeps the whole child visible instead.
    assert_eq!(
        centered_transient_position(
            aegis_model::Rect::new(1400, 800, 400, 300),
            aegis_model::Size { w: 300, h: 200 },
            output
        ),
        aegis_model::Point { x: 1220, y: 700 }
    );
}

#[test]
fn state_only_configures_preserve_a_mapped_window_size() {
    assert_eq!(
        state_configure_dimensions(aegis_model::Size { w: 1180, h: 760 }),
        (1180, 760)
    );
    assert_eq!(
        state_configure_dimensions(aegis_model::Size { w: 0, h: 0 }),
        (0, 0),
        "an unmapped toplevel still lets the client choose its first size"
    );
}

#[test]
fn newly_mapped_focus_policy_excludes_hidden_read_only_and_minimized_windows() {
    assert!(should_focus_mapped_toplevel(true, true, false));
    assert!(!should_focus_mapped_toplevel(false, true, false));
    assert!(!should_focus_mapped_toplevel(true, false, false));
    assert!(!should_focus_mapped_toplevel(true, true, true));
}

#[test]
fn popup_grab_focus_tracks_the_topmost_popup_and_unwinds_to_its_parent() {
    let mut state = State::new(std::ptr::null_mut());
    let mut root = Box::new(SurfaceRec::new(0x100usize as *mut ffi::wl_resource));
    root.xdg_toplevel = 0x101usize as *mut ffi::wl_resource;
    root.mapped = true;

    let mut parent_popup = Box::new(SurfaceRec::new(0x200usize as *mut ffi::wl_resource));
    parent_popup.xdg_popup = 0x201usize as *mut ffi::wl_resource;
    parent_popup.popup_parent = root.as_mut();
    parent_popup.popup_grabbed = true;
    parent_popup.popup_grab_seat = Some(HUMAN_SEAT);
    parent_popup.mapped = true;

    let mut child_popup = Box::new(SurfaceRec::new(0x300usize as *mut ffi::wl_resource));
    child_popup.xdg_popup = 0x301usize as *mut ffi::wl_resource;
    child_popup.popup_parent = parent_popup.as_mut();
    child_popup.popup_grabbed = true;
    child_popup.popup_grab_seat = Some(HUMAN_SEAT);
    child_popup.mapped = true;

    state.surfaces = vec![root.as_mut(), parent_popup.as_mut(), child_popup.as_mut()];

    assert_eq!(
        topmost_grabbed_popup(&state, HUMAN_SEAT),
        Some(child_popup.as_mut() as *mut SurfaceRec)
    );
    assert_eq!(
        unsafe { popup_keyboard_focus_after_dismissal(child_popup.as_mut()) },
        parent_popup.resource,
        "a nested popup returns keyboard focus to its grabbing parent"
    );

    parent_popup.popup_grabbed = false;
    assert_eq!(
        unsafe { popup_keyboard_focus_after_dismissal(child_popup.as_mut()) },
        root.resource,
        "the popup chain ultimately returns focus to the owning toplevel"
    );
}

#[test]
fn transient_toplevel_unmap_defers_focus_to_its_live_parent() {
    let mut state = State::new(std::ptr::null_mut());
    let mut parent = Box::new(SurfaceRec::new(0x100usize as *mut ffi::wl_resource));
    parent.xdg_toplevel = 0x101usize as *mut ffi::wl_resource;
    parent.mapped = true;

    let mut dialog = Box::new(SurfaceRec::new(0x200usize as *mut ffi::wl_resource));
    dialog.state = &mut state;
    dialog.xdg_toplevel = 0x201usize as *mut ffi::wl_resource;
    dialog.window.parent = Some(parent.as_mut() as *mut SurfaceRec as usize);
    dialog.mapped = false;

    state.keyboard_focus = dialog.resource;
    state.surfaces = vec![parent.as_mut(), dialog.as_mut()];

    unsafe { defer_keyboard_focus_after_toplevel_unmap(dialog.as_mut()) };

    let entry = state
        .pending_keyboard_focus
        .get(&HUMAN_SEAT)
        .expect("a restoration entry is deferred");
    assert_eq!(
        entry.target, parent.resource,
        "closing a focused transient must return focus to its parent"
    );
    assert_eq!(entry.restoring_from, dialog.resource);
}

#[test]
fn transient_focus_fallback_skips_an_unmapped_intermediate_parent() {
    let mut state = State::new(std::ptr::null_mut());
    let mut root = Box::new(SurfaceRec::new(0x100usize as *mut ffi::wl_resource));
    root.xdg_toplevel = 0x101usize as *mut ffi::wl_resource;
    root.mapped = true;

    let mut parent = Box::new(SurfaceRec::new(0x200usize as *mut ffi::wl_resource));
    parent.xdg_toplevel = 0x201usize as *mut ffi::wl_resource;
    parent.window.parent = Some(root.as_mut() as *mut SurfaceRec as usize);
    parent.mapped = false;

    let mut dialog = Box::new(SurfaceRec::new(0x300usize as *mut ffi::wl_resource));
    dialog.state = &mut state;
    dialog.xdg_toplevel = 0x301usize as *mut ffi::wl_resource;
    dialog.window.parent = Some(parent.as_mut() as *mut SurfaceRec as usize);
    dialog.mapped = false;

    state.keyboard_focus = dialog.resource;
    state.surfaces = vec![root.as_mut(), parent.as_mut(), dialog.as_mut()];

    unsafe { defer_keyboard_focus_after_toplevel_unmap(dialog.as_mut()) };

    assert_eq!(
        state
            .pending_keyboard_focus
            .get(&HUMAN_SEAT)
            .map(|entry| entry.target),
        Some(root.resource),
        "nested dialog teardown must fall back to the closest mapped ancestor"
    );
}

fn mapped_toplevel_fixture(
    window: aegis_model::window::WindowId,
    resource: usize,
) -> Box<SurfaceRec> {
    let mut surface = Box::new(SurfaceRec::new(resource as *mut ffi::wl_resource));
    surface.mapped = true;
    surface.xdg_toplevel = resource as *mut ffi::wl_resource;
    surface.window.id = window;
    surface
}

/// Register the windows with the human interaction domain's authority and
/// place them on the current workspace, so the focus fallback's visibility
/// and seat-control filters see a realistic desktop.
fn enroll_windows_on_current_workspace(
    state: &mut State,
    windows: &[aegis_model::window::WindowId],
) {
    let client = state.authority.register_client(None);
    for &window in windows {
        state
            .register_window(client, window)
            .expect("register window");
    }
    let workspace = state
        .workspaces
        .current_workspace(state.output)
        .expect("bootstrap workspace");
    for &window in windows {
        state.workspaces.place_toplevel(workspace, window);
    }
}

#[test]
fn unparented_toplevel_unmap_falls_back_to_the_most_recent_window() {
    let mut state = State::new(std::ptr::null_mut());
    let older = aegis_model::window::WindowId(1);
    let newer = aegis_model::window::WindowId(2);
    let dialog = aegis_model::window::WindowId(3);
    enroll_windows_on_current_workspace(&mut state, &[older, newer, dialog]);

    let mut older_surface = mapped_toplevel_fixture(older, 0x100);
    let mut newer_surface = mapped_toplevel_fixture(newer, 0x200);
    let mut dialog_surface = mapped_toplevel_fixture(dialog, 0x300);
    dialog_surface.state = &mut state;
    // A dialog whose client never called xdg_toplevel.set_parent: there is
    // no transient parent to return to.
    state.keyboard_focus = dialog_surface.resource;
    // Stacking doubles as focus MRU: the focused dialog was raised to the
    // tail, so the window below it is the one the user worked in before.
    state.surfaces = vec![
        older_surface.as_mut(),
        newer_surface.as_mut(),
        dialog_surface.as_mut(),
    ];

    unsafe { defer_keyboard_focus_after_toplevel_unmap(dialog_surface.as_mut()) };

    let entry = state
        .pending_keyboard_focus
        .get(&HUMAN_SEAT)
        .expect("a restoration entry is deferred");
    assert_eq!(
        entry.target, newer_surface.resource,
        "closing an unparented dialog returns focus to the most recently raised window"
    );
    assert_eq!(entry.restoring_from, dialog_surface.resource);
}

#[test]
fn keyboard_focus_fallback_skips_minimized_hidden_and_agent_controlled_windows() {
    let mut state = State::new(std::ptr::null_mut());
    let older = aegis_model::window::WindowId(1);
    let minimized = aegis_model::window::WindowId(2);
    let hidden = aegis_model::window::WindowId(3);
    let agent_mirror = aegis_model::window::WindowId(4);
    enroll_windows_on_current_workspace(&mut state, &[older, minimized]);
    // `hidden` is deliberately not placed on the current workspace.

    // A visible physical mirror owned by an agent interaction domain: the
    // human seat sees it but may not deliver input to it.
    let agent = state
        .authority
        .create_agent_interaction_domain("agent", SeatCapabilities::POINTER_KEYBOARD);
    let agent_client = state.authority.register_client(None);
    let agent_group = state
        .authority
        .create_interaction_group(agent_client, &[agent_mirror], HUMAN_INTERACTION_DOMAIN)
        .expect("agent group");
    state
        .authority
        .transfer_control(
            agent_group,
            agent.interaction_domain,
            TransferOptions {
                retain_source_as_observer: true,
            },
        )
        .expect("transfer to agent interaction domain");
    let workspace = state
        .workspaces
        .current_workspace(state.output)
        .expect("bootstrap workspace");
    state.workspaces.place_toplevel(workspace, agent_mirror);

    let mut older_surface = mapped_toplevel_fixture(older, 0x100);
    let mut minimized_surface = mapped_toplevel_fixture(minimized, 0x200);
    minimized_surface.window.minimized = true;
    let mut hidden_surface = mapped_toplevel_fixture(hidden, 0x300);
    let mut mirror_surface = mapped_toplevel_fixture(agent_mirror, 0x400);
    // Scanning from the tail, every candidate above `older` must be rejected.
    state.surfaces = vec![
        older_surface.as_mut(),
        minimized_surface.as_mut(),
        hidden_surface.as_mut(),
        mirror_surface.as_mut(),
    ];

    assert_eq!(
        unsafe { keyboard_focus_fallback(&state, HUMAN_SEAT, std::ptr::null_mut()) },
        older_surface.resource,
        "the fallback returns the topmost window that is mapped, visible, not minimized, and seat-controlled"
    );
}

#[test]
fn toplevel_unmap_with_focus_on_its_own_popup_still_defers_a_restoration() {
    let mut state = State::new(std::ptr::null_mut());
    let owner = aegis_model::window::WindowId(1);
    let other = aegis_model::window::WindowId(2);
    enroll_windows_on_current_workspace(&mut state, &[owner, other]);

    let mut owner_surface = mapped_toplevel_fixture(owner, 0x100);
    owner_surface.state = &mut state;
    let mut popup = Box::new(SurfaceRec::new(0x200usize as *mut ffi::wl_resource));
    popup.mapped = true;
    popup.xdg_popup = 0x201usize as *mut ffi::wl_resource;
    popup.popup_parent = owner_surface.as_mut();
    popup.popup_grabbed = true;
    popup.popup_grab_seat = Some(HUMAN_SEAT);
    let mut other_surface = mapped_toplevel_fixture(other, 0x300);

    // The seat's focus rests on the toplevel's grabbed popup, not on the
    // toplevel resource itself; the unmap must still notice.
    state.keyboard_focus = popup.resource;
    state.surfaces = vec![
        owner_surface.as_mut(),
        popup.as_mut(),
        other_surface.as_mut(),
    ];

    unsafe { defer_keyboard_focus_after_toplevel_unmap(owner_surface.as_mut()) };

    let entry = state
        .pending_keyboard_focus
        .get(&HUMAN_SEAT)
        .expect("focus on a window's popup makes the seat affected when that window goes away");
    assert_eq!(entry.target, other_surface.resource);
    assert_eq!(entry.restoring_from, owner_surface.resource);
}

#[test]
fn deferred_restoration_is_dropped_once_focus_moves_to_a_new_toplevel() {
    let mut state = State::new(std::ptr::null_mut());
    let mut owner = mapped_toplevel_fixture(aegis_model::window::WindowId(1), 0x100);
    let mut popup = Box::new(SurfaceRec::new(0x200usize as *mut ffi::wl_resource));
    popup.mapped = true;
    popup.xdg_popup = 0x201usize as *mut ffi::wl_resource;
    popup.popup_parent = owner.as_mut();
    let mut fresh = mapped_toplevel_fixture(aegis_model::window::WindowId(2), 0x300);
    state.surfaces = vec![owner.as_mut(), popup.as_mut(), fresh.as_mut()];

    let owner_res = owner.resource;
    let popup_res = popup.resource;
    let fresh_res = fresh.resource;
    let stale_res = 0x999usize as *mut ffi::wl_resource;
    unsafe {
        // Focus still on the dismissed popup: the restoration applies.
        assert!(deferred_focus_restoration_applies(
            &state, popup_res, popup_res, owner_res
        ));
        // Focus anywhere inside the target's own tree (a parent popup in a
        // nested menu after the child was dismissed): applies.
        assert!(deferred_focus_restoration_applies(
            &state, popup_res, stale_res, owner_res
        ));
        // No current focus (the dismissed surface is already gone): applies.
        assert!(deferred_focus_restoration_applies(
            &state,
            std::ptr::null_mut(),
            popup_res,
            owner_res
        ));
        // Unconditional entries (popup grabs) never consult current focus.
        assert!(deferred_focus_restoration_applies(
            &state,
            fresh_res,
            std::ptr::null_mut(),
            owner_res
        ));
        // A dialog mapped in the same dispatch batch claimed focus: the
        // stale restoration to the menu's owner must not steal it back.
        assert!(!deferred_focus_restoration_applies(
            &state, fresh_res, popup_res, owner_res
        ));
        // A null-target restoration (nothing to fall back to) must not clear
        // focus the seat has since moved elsewhere.
        assert!(!deferred_focus_restoration_applies(
            &state,
            fresh_res,
            popup_res,
            std::ptr::null_mut()
        ));
        // Focus on a toplevel that is itself being torn down does not block
        // the restoration.
        fresh.mapped = false;
        assert!(deferred_focus_restoration_applies(
            &state, fresh_res, popup_res, owner_res
        ));
    }
}

#[test]
fn minimize_flight_targets_the_dock_icon_per_style() {
    let mut state = State::new(std::ptr::null_mut());
    let window = aegis_model::window::WindowId(9);
    let icon = aegis_model::Rect::new(500, 1020, 56, 56);
    state.minimize_targets.insert(window, icon);
    let window_rect = aegis_model::Rect::new(100, 100, 800, 600);

    state.minimize_animation = aegis_model::dock::MinimizeAnimationStyle::Scale;
    assert_eq!(minimize_flight_target(&state, window, window_rect), icon);
    state.minimize_animation = aegis_model::dock::MinimizeAnimationStyle::Genie;
    assert_eq!(minimize_flight_target(&state, window, window_rect), icon);

    state.minimize_animation = aegis_model::dock::MinimizeAnimationStyle::Suck;
    let suck = minimize_flight_target(&state, window, window_rect);
    assert_eq!(suck.size, aegis_model::Size { w: 2, h: 2 });
    assert_eq!(suck.origin, aegis_model::Point { x: 527, y: 1047 });

    let transition = minimize_transition(&state, window, window_rect);
    assert_eq!(
        transition.easing,
        aegis_model::transition::Easing::EaseInCubic
    );
    assert_eq!(
        transition.effect,
        Some(aegis_model::transition::TransitionEffect::Minimize {
            style: aegis_model::dock::MinimizeAnimationStyle::Suck,
            target: aegis_model::Point { x: 528, y: 1048 },
        })
    );
}

#[test]
fn minimize_flight_falls_back_to_the_screen_edge_stub_without_an_icon() {
    let state = State::new(std::ptr::null_mut());
    let window = aegis_model::window::WindowId(9);
    let window_rect = aegis_model::Rect::new(100, 100, 800, 600);

    let target = minimize_flight_target(&state, window, window_rect);
    let screen_h = state.output_geometry.logical_rect().size.h;
    assert_eq!(
        target.origin,
        aegis_model::Point {
            x: 300,
            y: screen_h - 20
        }
    );
    assert_eq!(target.size, aegis_model::Size { w: 400, h: 20 });

    let transition = minimize_transition(&state, window, window_rect);
    assert_eq!(transition.effect, None);
}
