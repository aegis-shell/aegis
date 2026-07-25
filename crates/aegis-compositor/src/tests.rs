use super::*;

#[test]
fn realm_registry_filter_isolates_outputs_and_physical_authority_globals() {
    let mut state = State::new(std::ptr::null_mut());
    let bundle = state
        .authority
        .create_agent_realm("filter-agent", SeatCapabilities::POINTER_KEYBOARD);
    let client_id = state
        .authority
        .register_client(Some(format!("ass.realm.{}", bundle.realm.0)));
    let realm_client = 0x10usize as *mut ffi::wl_client;
    let human_client = 0x20usize as *mut ffi::wl_client;
    state.clients.insert(realm_client as usize, client_id);
    state.client_initial_realms.insert(client_id, bundle.realm);

    let physical_global = 0x100usize as *mut ffi::wl_global;
    let virtual_global = 0x200usize as *mut ffi::wl_global;
    let session_global = 0x300usize as *mut ffi::wl_global;
    let info = state.output_infos[0].clone();
    state.output_globals.push(Box::new(OutputGlobal {
        state: std::ptr::null_mut(),
        info: info.clone(),
        realm: None,
        global: physical_global,
        active: true,
    }));
    state.output_globals.push(Box::new(OutputGlobal {
        state: std::ptr::null_mut(),
        info,
        realm: Some(bundle.realm),
        global: virtual_global,
        active: true,
    }));
    state.realm_hidden_globals.insert(session_global as usize);
    let data = (&mut state as *mut State).cast();

    unsafe {
        assert!(realm_global_filter(realm_client, virtual_global, data));
        assert!(!realm_global_filter(realm_client, physical_global, data));
        assert!(!realm_global_filter(realm_client, session_global, data));
        assert!(realm_global_filter(human_client, physical_global, data));
        assert!(realm_global_filter(human_client, virtual_global, data));
        assert!(realm_global_filter(human_client, session_global, data));
    }
}

#[test]
fn finger_scroll_updates_do_not_emit_stop_or_discrete_steps() {
    let frame = aegis_core::input::PointerAxisFrame::from_values(
        42,
        Some(aegis_core::input::PointerAxisSource::Finger),
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
    let frame = aegis_core::input::PointerAxisFrame {
        time: 55,
        source: Some(aegis_core::input::PointerAxisSource::Finger),
        vertical: aegis_core::input::PointerAxis {
            stop: true,
            ..aegis_core::input::PointerAxis::default()
        },
        ..aegis_core::input::PointerAxisFrame::default()
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
    let frame = aegis_core::input::PointerAxisFrame {
        time: 77,
        source: Some(aegis_core::input::PointerAxisSource::Wheel),
        vertical: aegis_core::input::PointerAxis {
            value: Some(-10.0),
            discrete: Some(-1),
            value120: Some(-120),
            ..aegis_core::input::PointerAxis::default()
        },
        ..aegis_core::input::PointerAxisFrame::default()
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
fn realm_damage_is_clipped_deduplicated_and_bounded() {
    let output = aegis_core::Rect::new(0, 0, 100, 80);
    let mut damage = vec![
        aegis_core::Rect::new(-10, -10, 20, 20),
        aegis_core::Rect::new(-10, -10, 20, 20),
        aegis_core::Rect::new(90, 70, 30, 30),
        aegis_core::Rect::new(150, 150, 10, 10),
    ];
    normalize_realm_damage(&mut damage, output);
    assert_eq!(
        damage,
        vec![
            aegis_core::Rect::new(0, 0, 10, 10),
            aegis_core::Rect::new(90, 70, 10, 10),
        ]
    );

    let mut many = (0..65)
        .map(|x| aegis_core::Rect::new(x, 0, 1, 1))
        .collect::<Vec<_>>();
    normalize_realm_damage(&mut many, output);
    assert_eq!(many, vec![aegis_core::Rect::new(0, 0, 65, 1)]);
}

#[test]
fn compositor_resize_edges_map_to_protocol_cursor_shapes() {
    use aegis_core::window::ResizeEdges;

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
    let hints = aegis_core::window::SizeHints {
        min_w: 320,
        min_h: 200,
        max_w: 1920,
        max_h: 1080,
    };
    assert_eq!(
        clamp_size_to_hints(aegis_core::Size { w: 100, h: 2_000 }, hints),
        aegis_core::Size { w: 320, h: 1080 }
    );
    assert_eq!(
        clamp_size_to_hints(
            aegis_core::Size { w: 800, h: 600 },
            aegis_core::window::SizeHints::default(),
        ),
        aegis_core::Size { w: 800, h: 600 }
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
        aegis_core::Size { w: 200, h: 100 }
    );

    surface.buffer_transform = aegis_core::Transform::Rotate90;
    assert_eq!(
        surface_logical_size(&surface),
        aegis_core::Size { w: 100, h: 200 }
    );

    // Viewport source coordinates are after transform and buffer scale,
    // so they are already surface-local and must not be divided again.
    surface.viewport_src = Some(aegis_core::Rect::new(5, 7, 80, 60));
    assert_eq!(
        surface_logical_size(&surface),
        aegis_core::Size { w: 80, h: 60 }
    );

    surface.viewport_dst = Some(aegis_core::Size { w: 123, h: 45 });
    assert_eq!(
        surface_logical_size(&surface),
        aegis_core::Size { w: 123, h: 45 }
    );
}

#[test]
fn draw_origin_subtracts_window_geometry_insets() {
    let mut surface = SurfaceRec::new(std::ptr::null_mut());
    surface.position = aegis_core::Point { x: 100, y: 60 };

    // No declared geometry: the buffer draws at the window-rect origin.
    assert_eq!(
        surface_draw_origin(&surface),
        aegis_core::Point { x: 100, y: 60 }
    );

    // CSD insets: the buffer extends up-left of the window rect.
    surface.window_geometry = Some(aegis_core::Rect::new(20, 10, 400, 300));
    assert_eq!(
        surface_draw_origin(&surface),
        aegis_core::Point { x: 80, y: 50 }
    );
}

#[test]
fn draw_origin_walks_nested_subsurface_chains() {
    let mut root = SurfaceRec::new(std::ptr::null_mut());
    root.position = aegis_core::Point { x: 100, y: 60 };
    // A CSD root: the chain anchors at the buffer draw origin.
    root.window_geometry = Some(aegis_core::Rect::new(20, 10, 400, 300));

    let mut child = SurfaceRec::new(std::ptr::null_mut());
    child.parent = &mut root;
    child.subsurface_offset = aegis_core::Point { x: 10, y: 5 };
    let mut grandchild = SurfaceRec::new(std::ptr::null_mut());
    grandchild.parent = &mut child;
    grandchild.subsurface_offset = aegis_core::Point { x: 3, y: 2 };

    // 100-20+10+3, 60-10+5+2: offsets accumulate in each parent's
    // buffer space down to the root's draw origin.
    assert_eq!(
        surface_draw_origin(&grandchild),
        aegis_core::Point { x: 93, y: 57 }
    );
    assert_eq!(
        surface_draw_origin(&child),
        aegis_core::Point { x: 90, y: 55 }
    );

    // Detaching (wl_subsurface.destroy / parent destroyed) stops the walk.
    grandchild.parent = std::ptr::null_mut();
    assert_eq!(surface_draw_origin(&grandchild), aegis_core::Point::default());
}

#[test]
fn accepts_point_uses_buffer_space_for_subsurfaces() {
    let mut root = SurfaceRec::new(std::ptr::null_mut());
    root.position = aegis_core::Point { x: 100, y: 60 };
    let mut child = SurfaceRec::new(std::ptr::null_mut());
    child.parent = &mut root;
    child.subsurface_offset = aegis_core::Point { x: 10, y: 5 };
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
    child.input_region = Some(vec![aegis_core::Rect::new(0, 0, 20, 30)]);
    assert!(surface_accepts_point(&child, 115.0, 70.0));
    assert!(!surface_accepts_point(&child, 135.0, 70.0));
}

#[test]
fn region_subtraction_preserves_the_uncut_area() {
    let pieces = subtract_rect(
        aegis_core::Rect::new(0, 0, 100, 100),
        aegis_core::Rect::new(20, 20, 60, 60),
    );
    assert_eq!(pieces.len(), 4);
    let area: i32 = pieces.iter().map(|rect| rect.size.w * rect.size.h).sum();
    assert_eq!(area, 10_000 - 3_600);
    assert!(
        pieces
            .iter()
            .all(|rect| !rect.contains(aegis_core::Point { x: 50, y: 50 }))
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
    use aegis_core::layout::LayoutRole;
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

/// `Server::new` brings up the display, binds an auto-named socket, and
/// returns a non-empty socket name. The socket lives in `XDG_RUNTIME_DIR`
/// (libwayland's convention) and is removed by `wl_display_destroy`.
#[test]
fn server_new_creates_socket() {
    // Skip on environments without an XDG runtime dir (CI sandboxes).
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipping: XDG_RUNTIME_DIR not set");
        return;
    }
    let server = Server::new().expect("Server::new");
    let socket = server.socket();
    assert!(!socket.is_empty(), "socket name must not be empty");
    let path = std::env::var("XDG_RUNTIME_DIR").unwrap() + "/" + socket;
    assert!(
        std::path::Path::new(&path).exists(),
        "socket file missing: {path}"
    );
    // Drop runs destroy and should remove the socket.
    drop(server);
    assert!(
        !std::path::Path::new(&path).exists(),
        "socket file should be removed after drop: {path}"
    );
}

/// Registry absence is the intentional capability signal for Primary
/// Selection. Keep the standard clipboard visible while guarding against an
/// accidental reintroduction of either primary-capable global.
#[test]
fn registry_exposes_only_the_standard_clipboard() {
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipping: XDG_RUNTIME_DIR not set");
        return;
    }

    let mut server = Server::new().expect("Server::new");
    let socket = server.socket().to_owned();
    let mut child = match std::process::Command::new("wayland-info")
        .env("WAYLAND_DISPLAY", socket)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipping: wayland-info not installed");
            return;
        }
        Err(error) => panic!("could not start wayland-info: {error}"),
    };

    let mut exited = false;
    for _ in 0..2_000 {
        server.dispatch();
        if child.try_wait().expect("poll wayland-info").is_some() {
            exited = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    if !exited {
        let _ = child.kill();
        let _ = child.wait();
        panic!("wayland-info did not finish within two seconds");
    }

    let output = child
        .wait_with_output()
        .expect("collect wayland-info output");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "wayland-info failed: {stderr}");
    assert!(
        stdout.contains("interface: 'wl_data_device_manager'"),
        "standard clipboard global missing:\n{stdout}"
    );
    assert!(
        !stdout.contains("zwp_primary_selection_device_manager_v1"),
        "Primary Selection must not be advertised:\n{stdout}"
    );
    assert!(
        !stdout.contains("ext_data_control_manager_v1"),
        "unimplemented primary-capable data-control must not be advertised:\n{stdout}"
    );
}

#[test]
fn agent_seat_lifecycle_is_fail_closed() {
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipping: XDG_RUNTIME_DIR not set");
        return;
    }
    let mut server = Server::new().expect("Server::new");
    let bundle = server
        .create_agent_realm("test-agent", SeatCapabilities::POINTER_KEYBOARD)
        .expect("create agent realm");
    let portal = server
        .prepare_realm_portal(bundle.realm)
        .expect("prepare Realm portal");
    let _realm_client =
        std::os::unix::net::UnixStream::connect(portal.path()).expect("connect Realm portal");
    std::fs::remove_file(portal.path()).expect("remove ambient portal name");
    std::fs::remove_dir(portal.path().parent().unwrap()).expect("remove portal directory");
    server
        .activate_realm_portal(portal)
        .expect("activate private Realm portal");
    server.dispatch();
    assert_eq!(server.realm_portal_count(), 1);
    assert!(server.realm_snapshot().clients.iter().any(|client| {
        client.connected
            && client.security_context.as_deref()
                == Some(format!("ass.realm.{}", bundle.realm.0).as_str())
    }));
    assert!(
        server
            .realm_snapshot()
            .seats
            .iter()
            .any(|seat| seat.id == bundle.seat && seat.enabled)
    );

    server.pause_realm(bundle.realm).expect("pause");
    assert!(matches!(
        server.forward_agent_input(bundle.seat, &[]),
        Err(RealmRuntimeError::SeatUnavailable(id)) if id == bundle.seat
    ));

    server.resume_realm(bundle.realm).expect("resume");
    server
        .forward_agent_input(bundle.seat, &[])
        .expect("resumed input route");

    server
        .revoke_realm(bundle.realm, HUMAN_REALM)
        .expect("revoke");
    assert_eq!(server.realm_portal_count(), 0);
    assert!(server.realm_snapshot().clients.iter().all(|client| {
        client.security_context.as_deref() != Some(format!("ass.realm.{}", bundle.realm.0).as_str())
            || !client.connected
    }));
    assert!(matches!(
        server.prepare_realm_portal(bundle.realm),
        Err(RealmRuntimeError::Model(RealmError::RealmNotActive(id))) if id == bundle.realm
    ));
    assert!(matches!(
        server.forward_agent_input(bundle.seat, &[]),
        Err(RealmRuntimeError::SeatUnavailable(id)) if id == bundle.seat
    ));
}

#[test]
fn realm_window_registration_schedules_layout_and_damage_observation() {
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipping: XDG_RUNTIME_DIR not set");
        return;
    }
    let mut server = Server::new().expect("Server::new");
    let bundle = server
        .create_agent_realm("damage-agent", SeatCapabilities::POINTER_KEYBOARD)
        .expect("create agent realm");
    // Model the trusted identity assignment performed for a private
    // compositor-mediated launch portal.
    let client = server
        .state
        .authority
        .register_client(Some(format!("ass.realm.{}", bundle.realm.0)));
    server
        .state
        .client_initial_realms
        .insert(client, bundle.realm);
    let window = aegis_core::window::WindowId(4242);
    server
        .state
        .register_window(client, window)
        .expect("register Realm window");
    assert!(server.state.pending_realm_layouts.contains(&bundle.realm));

    server.dispatch();
    assert!(
        server
            .state
            .realm_placements
            .contains_key(&(bundle.realm, window))
    );
    // Discard the full-layout notification, then prove a later surface
    // commit is mapped to only the registered window's placement.
    let _ = server.take_realm_damage();
    server.state.damaged_windows.insert(window);
    let damage = server.take_realm_damage();
    assert_eq!(
        damage.get(&bundle.realm).and_then(|rects| rects.first()),
        server.state.realm_placements.get(&(bundle.realm, window))
    );
}

#[test]
fn single_seat_client_route_follows_atomic_group_authority() {
    let mut state = State::new(std::ptr::null_mut());
    let agent = state
        .authority
        .create_agent_realm("agent", SeatCapabilities::POINTER_KEYBOARD);
    state.seats.insert(
        agent.seat,
        Box::new(SeatRuntime::new(
            agent.seat,
            agent.realm,
            agent.principal,
            SeatCapabilities::POINTER_KEYBOARD,
        )),
    );
    let client = state.authority.register_client(None);
    let raw_client = std::ptr::dangling_mut::<ffi::wl_client>();
    state.clients.insert(raw_client as usize, client);
    let group = state
        .authority
        .create_interaction_group(client, &[aegis_core::window::WindowId(1)], HUMAN_REALM)
        .unwrap();

    unsafe { state.note_client_used_seat(raw_client, HUMAN_SEAT) };
    assert_eq!(state.client_routed_seat(raw_client, HUMAN_SEAT), HUMAN_SEAT);
    state
        .authority
        .transfer_control(group, agent.realm, TransferOptions::default())
        .unwrap();
    assert_eq!(
        state.client_routed_seat(raw_client, HUMAN_SEAT),
        agent.seat,
        "one client-facing seat is a compatibility gateway"
    );

    unsafe { state.note_client_used_seat(raw_client, agent.seat) };
    assert_eq!(
        state.client_routed_seat(raw_client, HUMAN_SEAT),
        HUMAN_SEAT,
        "requesting child resources on two advertised seats proves native multi-seat support"
    );
}

#[test]
fn observers_are_surface_output_members_without_receiving_control() {
    let mut state = State::new(std::ptr::null_mut());
    let agent = state
        .authority
        .create_agent_realm("agent", SeatCapabilities::POINTER_KEYBOARD);
    let window = aegis_core::window::WindowId(7);
    let client = state.authority.register_client(None);
    let group = state
        .authority
        .create_interaction_group(client, &[window], HUMAN_REALM)
        .unwrap();

    state
        .authority
        .set_observer(group, agent.realm, true)
        .unwrap();
    assert_eq!(
        output_realms_for_window(&state, window),
        [HUMAN_REALM, agent.realm].into_iter().collect()
    );

    state
        .authority
        .transfer_control(
            group,
            agent.realm,
            TransferOptions {
                retain_source_as_observer: true,
            },
        )
        .unwrap();
    assert_eq!(
        output_realms_for_window(&state, window),
        [HUMAN_REALM, agent.realm].into_iter().collect(),
        "retaining the source as an observer preserves its output membership"
    );
    assert_eq!(
        state
            .authority
            .interaction_group(group)
            .unwrap()
            .control_realm,
        agent.realm
    );
}

#[test]
fn physical_observer_mirror_blocks_click_through_without_taking_focus() {
    let mut state = State::new(std::ptr::null_mut());
    let agent = state
        .authority
        .create_agent_realm("agent", SeatCapabilities::POINTER_KEYBOARD);
    let bottom_window = aegis_core::window::WindowId(1);
    let mirror_window = aegis_core::window::WindowId(2);
    let bottom_client = state.authority.register_client(None);
    let mirror_client = state.authority.register_client(None);
    state
        .authority
        .create_interaction_group(bottom_client, &[bottom_window], HUMAN_REALM)
        .unwrap();
    let mirror_group = state
        .authority
        .create_interaction_group(mirror_client, &[mirror_window], HUMAN_REALM)
        .unwrap();
    state
        .authority
        .transfer_control(
            mirror_group,
            agent.realm,
            TransferOptions {
                retain_source_as_observer: true,
            },
        )
        .unwrap();

    let workspace = state
        .workspaces
        .current_workspace(state.output)
        .expect("bootstrap workspace");
    state.workspaces.place_toplevel(workspace, bottom_window);
    state.workspaces.place_toplevel(workspace, mirror_window);

    let make_surface = |window: aegis_core::window::WindowId, resource: usize| -> Box<SurfaceRec> {
        let mut surface = Box::new(SurfaceRec::new(resource as *mut ffi::wl_resource));
        surface.mapped = true;
        surface.xdg_toplevel = resource as *mut ffi::wl_resource;
        surface.width = 100;
        surface.height = 100;
        surface.window.id = window;
        surface.window.size = aegis_core::Size { w: 100, h: 100 };
        surface
    };
    let mut bottom = make_surface(bottom_window, 0x100);
    let mut mirror = make_surface(mirror_window, 0x200);
    state.surfaces = vec![bottom.as_mut(), mirror.as_mut()];

    // Avoid Server::drop: these are synthetic resource pointers and
    // there is no wl_display to destroy in this pure hit-test fixture.
    let mut server = std::mem::ManuallyDrop::new(Server {
        state: Box::new(state),
        socket: String::new(),
        realm_portals: Vec::new(),
        epoch: std::time::Instant::now(),
    });
    assert!(
        server.hit_test_focus(10.0, 10.0).is_null(),
        "the visible mirror consumes the visual hit but receives no focus"
    );

    server
        .state
        .authority
        .set_observer(mirror_group, HUMAN_REALM, false)
        .unwrap();
    assert_eq!(
        server.hit_test_focus(10.0, 10.0),
        bottom.resource,
        "a non-presented Realm window must not block the physical scene"
    );
}

#[test]
fn backend_outputs_reconcile_workspace_connectors_and_geometry() {
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipping: XDG_RUNTIME_DIR not set");
        return;
    }
    let mut server = Server::new().expect("Server::new");
    let geometry = |x, width| aegis_core::output::OutputGeometry {
        mode: aegis_core::output::OutputMode {
            width,
            height: 1080,
            refresh_mhz: 60_000,
        },
        scale: aegis_core::output::Scale::IDENTITY,
        transform: aegis_core::Transform::Normal,
        logical_origin: aegis_core::Point { x, y: 0 },
    };
    server.set_outputs(vec![
        aegis_core::output::OutputInfo {
            connector: "DP-1".into(),
            geometry: geometry(0, 1920),
            available_modes: Vec::new(),
        },
        aegis_core::output::OutputInfo {
            connector: "HDMI-A-1".into(),
            geometry: geometry(1920, 2560),
            available_modes: Vec::new(),
        },
    ]);

    assert_eq!(
        server
            .output_infos()
            .iter()
            .map(|output| output.connector.as_str())
            .collect::<Vec<_>>(),
        vec!["DP-1", "HDMI-A-1"]
    );
    assert_eq!(server.output_logical_rect().size.w, 1920);
    assert_eq!(
        server
            .workspace_snapshot()
            .outputs
            .iter()
            .map(|output| output.connector.as_str())
            .collect::<std::collections::HashSet<_>>(),
        std::collections::HashSet::from(["DP-1", "HDMI-A-1"])
    );
}

/// `[[output]]` policy (ADR-0028) overrides the backend-reported
/// geometry: scale and position apply per connector, and a `primary`
/// entry moves its output to the front of the list (index 0 is the
/// focused output whose geometry `output_logical_rect` reports).
#[test]
fn output_policies_apply_scale_position_and_primary() {
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipping: XDG_RUNTIME_DIR not set");
        return;
    }
    let mut server = Server::new().expect("Server::new");
    let geometry = |x, width| aegis_core::output::OutputGeometry {
        mode: aegis_core::output::OutputMode {
            width,
            height: 1080,
            refresh_mhz: 60_000,
        },
        scale: aegis_core::output::Scale::IDENTITY,
        transform: aegis_core::Transform::Normal,
        logical_origin: aegis_core::Point { x, y: 0 },
    };
    server.set_outputs(vec![
        aegis_core::output::OutputInfo {
            connector: "DP-1".into(),
            geometry: geometry(0, 1920),
            available_modes: Vec::new(),
        },
        aegis_core::output::OutputInfo {
            connector: "HDMI-A-1".into(),
            geometry: geometry(1920, 2560),
            available_modes: Vec::new(),
        },
    ]);

    server.set_output_policies(std::collections::HashMap::from([(
        "HDMI-A-1".to_owned(),
        aegis_core::output::OutputPolicy {
            scale: Some(2.0),
            position: Some(aegis_core::Point { x: 1920, y: 0 }),
            primary: true,
            ..Default::default()
        },
    )]));

    let infos = server.output_infos();
    assert_eq!(
        infos
            .iter()
            .map(|output| output.connector.as_str())
            .collect::<Vec<_>>(),
        vec!["HDMI-A-1", "DP-1"],
        "the primary output leads the list"
    );
    assert_eq!(infos[0].geometry.scale.as_f32(), 2.0);
    assert_eq!(
        infos[0].geometry.logical_origin,
        aegis_core::Point { x: 1920, y: 0 }
    );
    assert_eq!(
        server.output_logical_rect().origin,
        aegis_core::Point { x: 1920, y: 0 },
        "the focused output geometry follows the primary policy"
    );
    // The other output keeps its backend-reported geometry.
    assert_eq!(infos[1].geometry.scale.as_f32(), 1.0);
    assert_eq!(
        infos[1].geometry.logical_origin,
        aegis_core::Point { x: 0, y: 0 }
    );
}
