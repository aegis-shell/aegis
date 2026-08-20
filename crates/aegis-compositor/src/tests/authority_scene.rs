use super::*;

#[test]
fn agent_seat_lifecycle_is_fail_closed() {
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipping: XDG_RUNTIME_DIR not set");
        return;
    }
    let mut server = Server::new().expect("Server::new");
    let bundle = server
        .create_agent_interaction_domain("test-agent", SeatCapabilities::POINTER_KEYBOARD)
        .expect("create agent interaction_domain");
    let portal = server
        .prepare_interaction_domain_portal(bundle.interaction_domain)
        .expect("prepare InteractionDomain portal");
    let _interaction_domain_client = std::os::unix::net::UnixStream::connect(portal.path())
        .expect("connect InteractionDomain portal");
    std::fs::remove_file(portal.path()).expect("remove ambient portal name");
    std::fs::remove_dir(portal.path().parent().unwrap()).expect("remove portal directory");
    server
        .activate_interaction_domain_portal(portal)
        .expect("activate private InteractionDomain portal");
    server.dispatch();
    assert_eq!(server.interaction_domain_portal_count(), 1);
    assert!(
        server
            .interaction_domain_snapshot()
            .clients
            .iter()
            .any(|client| {
                client.connected
                    && client.security_context.as_deref()
                        == Some(
                            format!("aegis.interaction_domain.{}", bundle.interaction_domain.0)
                                .as_str(),
                        )
            })
    );
    assert!(
        server
            .interaction_domain_snapshot()
            .seats
            .iter()
            .any(|seat| seat.id == bundle.seat && seat.enabled)
    );

    server
        .pause_interaction_domain(bundle.interaction_domain)
        .expect("pause");
    assert!(matches!(
        server.forward_agent_input(bundle.seat, &[]),
        Err(InteractionDomainRuntimeError::SeatUnavailable(id)) if id == bundle.seat
    ));

    server
        .resume_interaction_domain(bundle.interaction_domain)
        .expect("resume");
    server
        .forward_agent_input(bundle.seat, &[])
        .expect("resumed input route");

    server
        .revoke_interaction_domain(bundle.interaction_domain, HUMAN_INTERACTION_DOMAIN)
        .expect("revoke");
    assert_eq!(server.interaction_domain_portal_count(), 0);
    assert!(
        server
            .interaction_domain_snapshot()
            .clients
            .iter()
            .all(|client| {
                client.security_context.as_deref()
                    != Some(
                        format!("aegis.interaction_domain.{}", bundle.interaction_domain.0)
                            .as_str(),
                    )
                    || !client.connected
            })
    );
    assert!(matches!(
        server.prepare_interaction_domain_portal(bundle.interaction_domain),
        Err(InteractionDomainRuntimeError::Model(InteractionDomainError::InteractionDomainNotActive(id))) if id == bundle.interaction_domain
    ));
    assert!(matches!(
        server.forward_agent_input(bundle.seat, &[]),
        Err(InteractionDomainRuntimeError::SeatUnavailable(id)) if id == bundle.seat
    ));
}

#[test]
fn interaction_domain_window_registration_schedules_layout_and_damage_observation() {
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipping: XDG_RUNTIME_DIR not set");
        return;
    }
    let mut server = Server::new().expect("Server::new");
    let bundle = server
        .create_agent_interaction_domain("damage-agent", SeatCapabilities::POINTER_KEYBOARD)
        .expect("create agent interaction_domain");
    // Model the trusted identity assignment performed for a private
    // compositor-mediated launch portal.
    let client = server.state.authority.register_client(Some(format!(
        "aegis.interaction_domain.{}",
        bundle.interaction_domain.0
    )));
    server
        .state
        .client_initial_interaction_domains
        .insert(client, bundle.interaction_domain);
    let window = aegis_model::window::WindowId(4242);
    server
        .state
        .register_window(client, window)
        .expect("register InteractionDomain window");
    let group = server
        .state
        .authority
        .interaction_group_for_window(window)
        .expect("registered interaction group");
    assert_eq!(group.control_interaction_domain, bundle.interaction_domain);
    assert!(
        group
            .observer_interaction_domains
            .contains(&HUMAN_INTERACTION_DOMAIN),
        "Agent-launched windows are visible physical mirrors without sharing input authority"
    );
    assert!(
        !server
            .state
            .authority
            .seat_controls_window(HUMAN_SEAT, window)
    );
    assert!(
        server
            .state
            .pending_interaction_domain_layouts
            .contains(&bundle.interaction_domain)
    );

    server.dispatch();
    assert!(
        server
            .state
            .interaction_domain_placements
            .contains_key(&(bundle.interaction_domain, window))
    );
    // Discard the full-layout notification, then prove a later surface
    // commit is mapped to only the registered window's placement.
    let _ = server.take_interaction_domain_damage();
    server.state.damaged_windows.insert(window);
    let damage = server.take_interaction_domain_damage();
    assert_eq!(
        damage
            .get(&bundle.interaction_domain)
            .and_then(|rects| rects.first()),
        server
            .state
            .interaction_domain_placements
            .get(&(bundle.interaction_domain, window))
    );
}

#[test]
fn single_seat_client_route_follows_atomic_group_authority() {
    let mut state = State::new(std::ptr::null_mut());
    let agent = state
        .authority
        .create_agent_interaction_domain("agent", SeatCapabilities::POINTER_KEYBOARD);
    state.seats.insert(
        agent.seat,
        Box::new(SeatRuntime::new(
            agent.seat,
            agent.interaction_domain,
            agent.principal,
            SeatCapabilities::POINTER_KEYBOARD,
        )),
    );
    let client = state.authority.register_client(None);
    let raw_client = std::ptr::dangling_mut::<ffi::wl_client>();
    state.clients.insert(raw_client as usize, client);
    let group = state
        .authority
        .create_interaction_group(
            client,
            &[aegis_model::window::WindowId(1)],
            HUMAN_INTERACTION_DOMAIN,
        )
        .unwrap();

    unsafe { state.note_client_used_seat(raw_client, HUMAN_SEAT) };
    assert_eq!(state.client_routed_seat(raw_client, HUMAN_SEAT), HUMAN_SEAT);
    state
        .authority
        .transfer_control(group, agent.interaction_domain, TransferOptions::default())
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
        .create_agent_interaction_domain("agent", SeatCapabilities::POINTER_KEYBOARD);
    let window = aegis_model::window::WindowId(7);
    let client = state.authority.register_client(None);
    let group = state
        .authority
        .create_interaction_group(client, &[window], HUMAN_INTERACTION_DOMAIN)
        .unwrap();

    state
        .authority
        .set_observer(group, agent.interaction_domain, true)
        .unwrap();
    assert_eq!(
        output_interaction_domains_for_window(&state, window),
        [HUMAN_INTERACTION_DOMAIN, agent.interaction_domain]
            .into_iter()
            .collect()
    );

    state
        .authority
        .transfer_control(
            group,
            agent.interaction_domain,
            TransferOptions {
                retain_source_as_observer: true,
            },
        )
        .unwrap();
    assert_eq!(
        output_interaction_domains_for_window(&state, window),
        [HUMAN_INTERACTION_DOMAIN, agent.interaction_domain]
            .into_iter()
            .collect(),
        "retaining the source as an observer preserves its output membership"
    );
    assert_eq!(
        state
            .authority
            .interaction_group(group)
            .unwrap()
            .control_interaction_domain,
        agent.interaction_domain
    );
}

#[test]
fn physical_observer_mirror_blocks_click_through_without_taking_focus() {
    let mut state = State::new(std::ptr::null_mut());
    let agent = state
        .authority
        .create_agent_interaction_domain("agent", SeatCapabilities::POINTER_KEYBOARD);
    let bottom_window = aegis_model::window::WindowId(1);
    let mirror_window = aegis_model::window::WindowId(2);
    let bottom_client = state.authority.register_client(None);
    let mirror_client = state.authority.register_client(None);
    state
        .authority
        .create_interaction_group(bottom_client, &[bottom_window], HUMAN_INTERACTION_DOMAIN)
        .unwrap();
    let mirror_group = state
        .authority
        .create_interaction_group(mirror_client, &[mirror_window], HUMAN_INTERACTION_DOMAIN)
        .unwrap();
    state
        .authority
        .transfer_control(
            mirror_group,
            agent.interaction_domain,
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

    let make_surface =
        |window: aegis_model::window::WindowId, resource: usize| -> Box<SurfaceRec> {
            let mut surface = Box::new(SurfaceRec::new(resource as *mut ffi::wl_resource));
            surface.mapped = true;
            surface.xdg_toplevel = resource as *mut ffi::wl_resource;
            surface.width = 100;
            surface.height = 100;
            surface.window.id = window;
            surface.window.size = aegis_model::Size { w: 100, h: 100 };
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
        interaction_domain_portals: Vec::new(),
        epoch: std::time::Instant::now(),
    });
    assert!(
        server.hit_test_focus(10.0, 10.0).is_null(),
        "the visible mirror consumes the visual hit but receives no focus"
    );

    server
        .state
        .authority
        .set_observer(mirror_group, HUMAN_INTERACTION_DOMAIN, false)
        .unwrap();
    assert_eq!(
        server.hit_test_focus(10.0, 10.0),
        bottom.resource,
        "a non-presented InteractionDomain window must not block the physical scene"
    );
}

#[test]
fn switcher_preview_does_not_restack_and_stationary_rehit_tracks_the_commit() {
    let mut state = State::new(std::ptr::null_mut());
    let selected_window = aegis_model::window::WindowId(1);
    let old_top_window = aegis_model::window::WindowId(2);
    let client = state.authority.register_client(None);
    state
        .authority
        .create_interaction_group(
            client,
            &[selected_window, old_top_window],
            HUMAN_INTERACTION_DOMAIN,
        )
        .unwrap();
    let workspace = state
        .workspaces
        .current_workspace(state.output)
        .expect("bootstrap workspace");
    state.workspaces.place_toplevel(workspace, selected_window);
    state.workspaces.place_toplevel(workspace, old_top_window);

    let make_surface =
        |window: aegis_model::window::WindowId, resource: usize| -> Box<SurfaceRec> {
            let mut surface = Box::new(SurfaceRec::new(resource as *mut ffi::wl_resource));
            surface.mapped = true;
            surface.xdg_toplevel = surface.resource;
            surface.width = 100;
            surface.height = 100;
            surface.window.id = window;
            surface.window.size = aegis_model::Size { w: 100, h: 100 };
            surface
        };
    let mut selected = make_surface(selected_window, 0x100);
    let mut old_top = make_surface(old_top_window, 0x200);
    state.surfaces = vec![selected.as_mut(), old_top.as_mut()];
    state.pointer_x = 10.0;
    state.pointer_y = 10.0;
    state.pointer_focus = old_top.resource;
    state.keyboard_focus = old_top.resource;

    // Avoid Server::drop and focus event posting: these are synthetic
    // resources in a pure stacking/hit-test fixture.
    let mut server = std::mem::ManuallyDrop::new(Server {
        state: Box::new(state),
        socket: String::new(),
        interaction_domain_portals: Vec::new(),
        epoch: std::time::Instant::now(),
    });
    assert_eq!(
        server.stationary_pointer_rehit_target(),
        Some(old_top.resource)
    );

    server.start_window_switcher();
    server.cycle_focus(true);

    assert_eq!(server.focused_toplevel_id(), Some(old_top_window));
    assert_eq!(
        server
            .state
            .surfaces
            .iter()
            .map(|surface| unsafe { (**surface).window.id })
            .collect::<Vec<_>>(),
        vec![selected_window, old_top_window],
        "previewing must not raise or focus the selected candidate"
    );
    assert_eq!(
        server.window_switcher_snapshot().unwrap().1,
        Some(selected_window)
    );
    server.cancel_window_switcher();

    server.raise_toplevel(selected.resource);

    assert_eq!(
        server.stationary_pointer_rehit_target(),
        Some(selected.resource),
        "the next click or axis frame must target the newly raised window"
    );
    server.state.implicit_grab_active = true;
    assert_eq!(server.stationary_pointer_rehit_target(), None);
}

#[test]
fn window_snapshot_drops_an_unmapped_toplevel() {
    let mut state = State::new(std::ptr::null_mut());
    let window = aegis_model::window::WindowId(77);
    let client = state.authority.register_client(None);
    state
        .authority
        .create_interaction_group(client, &[window], HUMAN_INTERACTION_DOMAIN)
        .expect("register physical window authority");
    let workspace = state
        .workspaces
        .current_workspace(state.output)
        .expect("bootstrap workspace");
    state.workspaces.place_toplevel(workspace, window);

    let mut surface = Box::new(SurfaceRec::new(0x100usize as *mut ffi::wl_resource));
    surface.mapped = true;
    surface.xdg_toplevel = 0x200usize as *mut ffi::wl_resource;
    surface.window.id = window;
    surface.window.app_id = Some("org.example.App".into());
    state.surfaces = vec![surface.as_mut()];

    // Avoid Server::drop: this fixture uses synthetic resource pointers and
    // has no wl_display to destroy.
    let server = std::mem::ManuallyDrop::new(Server {
        state: Box::new(state),
        socket: String::new(),
        interaction_domain_portals: Vec::new(),
        epoch: std::time::Instant::now(),
    });

    assert_eq!(server.windows().len(), 1);
    let mapped_signature = server.windows_signature();

    // Closing the last window may only unmap the xdg_toplevel while the
    // application's process and role resource stay alive. It must disappear
    // from the shell snapshot so Dock/Launcher choose Spawn, not stale Focus.
    surface.mapped = false;
    assert!(server.windows().is_empty());
    assert_ne!(server.windows_signature(), mapped_signature);
}

#[test]
fn window_signature_memo_reuses_within_a_millisecond_and_invalidates_on_change() {
    // The frame loop evaluates both signatures from two call sites per frame
    // (damage assessment and the snapshot fan-out). The memo must make those
    // redundant calls return identical values while the surface table is
    // unchanged — including the all-windows variant interleaved with the
    // visible-set one — and must never serve a stale value after the table
    // mutates outside dispatch: the memo caches the per-millisecond walk,
    // never the semantics.
    let mut state = State::new(std::ptr::null_mut());
    let window = aegis_model::window::WindowId(78);
    let client = state.authority.register_client(None);
    state
        .authority
        .create_interaction_group(client, &[window], HUMAN_INTERACTION_DOMAIN)
        .expect("register physical window authority");
    let workspace = state
        .workspaces
        .current_workspace(state.output)
        .expect("bootstrap workspace");
    state.workspaces.place_toplevel(workspace, window);

    let mut surface = Box::new(SurfaceRec::new(0x300usize as *mut ffi::wl_resource));
    surface.mapped = true;
    surface.xdg_toplevel = 0x400usize as *mut ffi::wl_resource;
    surface.window.id = window;
    surface.window.app_id = Some("memo.test.app".into());
    state.surfaces = vec![surface.as_mut()];

    let server = std::mem::ManuallyDrop::new(Server {
        state: Box::new(state),
        socket: String::new(),
        interaction_domain_portals: Vec::new(),
        epoch: std::time::Instant::now(),
    });

    let first = server.windows_signature();
    let first_all = server.all_windows_signature();
    // Same millisecond: the interleaved lookups return the memoized values.
    assert_eq!(server.windows_signature(), first);
    assert_eq!(server.all_windows_signature(), first_all);
    assert_ne!(first, 0, "a real single-window signature must be non-zero");

    // A direct state edit bypasses dispatch, so the memo must be dropped
    // before the new title is observable (the frame loop's consumers all run
    // after dispatch, so no explicit drop is needed there).
    surface.window.title = Some("changed".into());
    server.invalidate_window_signature_memo();
    assert_ne!(server.windows_signature(), first);
    assert_ne!(server.all_windows_signature(), first_all);
}

#[test]
fn physical_window_snapshot_contains_only_controlled_or_observed_windows() {
    let mut state = State::new(std::ptr::null_mut());
    let agent = state
        .authority
        .create_agent_interaction_domain("agent", SeatCapabilities::POINTER_KEYBOARD);
    let window = aegis_model::window::WindowId(78);
    let client = state.authority.register_client(Some("agent".into()));
    let group = state
        .authority
        .create_interaction_group(client, &[window], agent.interaction_domain)
        .expect("register Agent window authority");
    let workspace = state
        .workspaces
        .current_workspace(state.output)
        .expect("bootstrap workspace");
    state.workspaces.place_toplevel(workspace, window);

    let mut surface = Box::new(SurfaceRec::new(0x300usize as *mut ffi::wl_resource));
    surface.mapped = true;
    surface.xdg_toplevel = 0x400usize as *mut ffi::wl_resource;
    surface.window.id = window;
    surface.window.size = aegis_model::Size { w: 100, h: 100 };
    state.surfaces = vec![surface.as_mut()];

    let mut server = std::mem::ManuallyDrop::new(Server {
        state: Box::new(state),
        socket: String::new(),
        interaction_domain_portals: Vec::new(),
        epoch: std::time::Instant::now(),
    });

    assert!(
        server.windows().is_empty(),
        "an Agent-only window must not leak into physical chrome"
    );
    server
        .state
        .authority
        .set_observer(group, HUMAN_INTERACTION_DOMAIN, true)
        .expect("present physical mirror");
    let windows = server.windows();
    assert_eq!(windows.len(), 1);
    assert!(windows[0].read_only);
}

#[test]
fn raising_a_toplevel_keeps_its_surface_tree_together() {
    let mut state = State::new(std::ptr::null_mut());
    let mut settings = Box::new(SurfaceRec::new(0x100usize as *mut ffi::wl_resource));
    settings.xdg_toplevel = settings.resource;
    let mut terminal = Box::new(SurfaceRec::new(0x200usize as *mut ffi::wl_resource));
    terminal.xdg_toplevel = terminal.resource;
    let mut popup = Box::new(SurfaceRec::new(0x300usize as *mut ffi::wl_resource));
    popup.xdg_popup = popup.resource;
    popup.popup_parent = terminal.as_mut();

    state.surfaces = vec![settings.as_mut(), terminal.as_mut(), popup.as_mut()];
    for (index, surface) in state.surfaces.iter().copied().enumerate() {
        unsafe { (*surface).index = index };
    }
    let mut server = std::mem::ManuallyDrop::new(Server {
        state: Box::new(state),
        socket: String::new(),
        interaction_domain_portals: Vec::new(),
        epoch: std::time::Instant::now(),
    });

    server.raise_toplevel(settings.resource);

    assert_eq!(
        server
            .state
            .surfaces
            .iter()
            .map(|surface| unsafe { (**surface).resource as usize })
            .collect::<Vec<_>>(),
        vec![
            terminal.resource as usize,
            popup.resource as usize,
            settings.resource as usize,
        ]
    );
    assert_eq!(terminal.index, 0);
    assert_eq!(popup.index, 1);
    assert_eq!(settings.index, 2);
}

#[test]
fn always_on_top_windows_stay_above_raised_normal_windows() {
    let mut state = State::new(std::ptr::null_mut());
    let window_a = aegis_model::window::WindowId(1);
    let window_b = aegis_model::window::WindowId(2);
    let client = state.authority.register_client(None);
    state
        .authority
        .create_interaction_group(client, &[window_a, window_b], HUMAN_INTERACTION_DOMAIN)
        .unwrap();

    let mut a = Box::new(SurfaceRec::new(0x100usize as *mut ffi::wl_resource));
    a.xdg_toplevel = a.resource;
    a.window.id = window_a;
    let mut a_popup = Box::new(SurfaceRec::new(0x200usize as *mut ffi::wl_resource));
    a_popup.xdg_popup = a_popup.resource;
    a_popup.popup_parent = a.as_mut();
    let mut b = Box::new(SurfaceRec::new(0x300usize as *mut ffi::wl_resource));
    b.xdg_toplevel = b.resource;
    b.window.id = window_b;

    state.surfaces = vec![a.as_mut(), a_popup.as_mut(), b.as_mut()];
    for (index, surface) in state.surfaces.iter().copied().enumerate() {
        unsafe { (*surface).index = index };
    }
    let mut server = std::mem::ManuallyDrop::new(Server {
        state: Box::new(state),
        socket: String::new(),
        interaction_domain_portals: Vec::new(),
        epoch: std::time::Instant::now(),
    });

    let order = |server: &Server| {
        server
            .state
            .surfaces
            .iter()
            .map(|surface| unsafe { (**surface).resource as usize })
            .collect::<Vec<_>>()
    };
    let band_above = vec![
        b.resource as usize,
        a.resource as usize,
        a_popup.resource as usize,
    ];

    // Enabling always-on-top raises the window's whole tree into the band at
    // the top of the stacking order.
    server.set_toplevel_always_on_top(window_a, true);
    assert!(a.window.always_on_top);
    assert_eq!(order(&server), band_above);

    // Raising a normal window must not stack it above the always-on-top
    // band, and the band tree stays contiguous at the Vec tail.
    server.raise_toplevel(b.resource);
    assert_eq!(order(&server), band_above);
    assert_eq!(b.index, 0);
    assert_eq!(a.index, 1);
    assert_eq!(a_popup.index, 2);

    // The setter is idempotent: a repeated enable changes nothing.
    server.set_toplevel_always_on_top(window_a, true);
    assert_eq!(order(&server), band_above);

    // Disabling only clears the flag; the stacking position is untouched.
    server.set_toplevel_always_on_top(window_a, false);
    assert!(!a.window.always_on_top);
    assert_eq!(order(&server), band_above);

    // Normal raise behavior resumes once the flag is cleared.
    server.raise_toplevel(b.resource);
    assert_eq!(
        order(&server),
        vec![
            a.resource as usize,
            a_popup.resource as usize,
            b.resource as usize,
        ]
    );
}

#[test]
fn restack_keeps_unfocused_newcomers_below_the_always_on_top_band() {
    let mut state = State::new(std::ptr::null_mut());
    let window_a = aegis_model::window::WindowId(1);
    let window_c = aegis_model::window::WindowId(3);
    let client = state.authority.register_client(None);
    state
        .authority
        .create_interaction_group(client, &[window_a, window_c], HUMAN_INTERACTION_DOMAIN)
        .unwrap();

    let mut a = Box::new(SurfaceRec::new(0x100usize as *mut ffi::wl_resource));
    a.xdg_toplevel = a.resource;
    a.window.id = window_a;
    a.window.always_on_top = true;
    // A window that maps without taking focus (hidden workspace, observation
    // mirror) keeps its wl_surface creation slot at the Vec tail until the
    // post-dispatch restack runs.
    let mut c = Box::new(SurfaceRec::new(0x300usize as *mut ffi::wl_resource));
    c.xdg_toplevel = c.resource;
    c.window.id = window_c;

    state.surfaces = vec![a.as_mut(), c.as_mut()];
    for (index, surface) in state.surfaces.iter().copied().enumerate() {
        unsafe { (*surface).index = index };
    }
    let mut server = std::mem::ManuallyDrop::new(Server {
        state: Box::new(state),
        socket: String::new(),
        interaction_domain_portals: Vec::new(),
        epoch: std::time::Instant::now(),
    });

    server.restack_always_on_top_band();

    assert_eq!(
        server
            .state
            .surfaces
            .iter()
            .map(|surface| unsafe { (**surface).resource as usize })
            .collect::<Vec<_>>(),
        vec![c.resource as usize, a.resource as usize]
    );
    assert_eq!(c.index, 0);
    assert_eq!(a.index, 1);
}

#[test]
fn client_surface_order_keeps_each_window_tree_occluded_as_a_unit() {
    let mut state = State::new(std::ptr::null_mut());
    let background_window = aegis_model::window::WindowId(10);
    let foreground_window = aegis_model::window::WindowId(20);
    let client = state.authority.register_client(None);
    state
        .authority
        .create_interaction_group(
            client,
            &[background_window, foreground_window],
            HUMAN_INTERACTION_DOMAIN,
        )
        .unwrap();
    let workspace = state
        .workspaces
        .current_workspace(state.output)
        .expect("bootstrap workspace");
    state
        .workspaces
        .place_toplevel(workspace, background_window);
    state
        .workspaces
        .place_toplevel(workspace, foreground_window);

    let mut background = Box::new(SurfaceRec::new(0x100usize as *mut ffi::wl_resource));
    background.mapped = true;
    background.xdg_toplevel = background.resource;
    background.window.id = background_window;
    let mut background_below = Box::new(SurfaceRec::new(0x110usize as *mut ffi::wl_resource));
    background_below.mapped = true;
    background_below.parent = background.as_mut();
    background_below.subsurface_above_parent = false;
    let mut background_chrome = Box::new(SurfaceRec::new(0x120usize as *mut ffi::wl_resource));
    background_chrome.mapped = true;
    background_chrome.parent = background.as_mut();
    background.children = vec![background_below.as_mut(), background_chrome.as_mut()];

    let mut foreground = Box::new(SurfaceRec::new(0x200usize as *mut ffi::wl_resource));
    foreground.mapped = true;
    foreground.xdg_toplevel = foreground.resource;
    foreground.window.id = foreground_window;
    let mut foreground_chrome = Box::new(SurfaceRec::new(0x220usize as *mut ffi::wl_resource));
    foreground_chrome.mapped = true;
    foreground_chrome.parent = foreground.as_mut();
    foreground.children = vec![foreground_chrome.as_mut()];

    // A popup allocated after the foreground surface still belongs to the
    // background stacking unit and must not escape above the foreground.
    let mut background_popup = Box::new(SurfaceRec::new(0x130usize as *mut ffi::wl_resource));
    background_popup.mapped = true;
    background_popup.xdg_popup = background_popup.resource;
    background_popup.popup_parent = background.as_mut();
    let mut background_popup_child = Box::new(SurfaceRec::new(0x131usize as *mut ffi::wl_resource));
    background_popup_child.mapped = true;
    background_popup_child.parent = background_popup.as_mut();
    background_popup_child.width = 1;
    background_popup_child.height = 1;
    background_popup_child.pixels = vec![0, 0, 0, 0xff];
    background_popup.children = vec![background_popup_child.as_mut()];

    state.surfaces = vec![
        background.as_mut(),
        background_below.as_mut(),
        background_chrome.as_mut(),
        foreground.as_mut(),
        foreground_chrome.as_mut(),
        background_popup.as_mut(),
        background_popup_child.as_mut(),
    ];
    let server = std::mem::ManuallyDrop::new(Server {
        state: Box::new(state),
        socket: String::new(),
        interaction_domain_portals: Vec::new(),
        epoch: std::time::Instant::now(),
    });

    let expected_order = vec![
        background_below.resource as usize,
        background.resource as usize,
        background_chrome.resource as usize,
        background_popup.resource as usize,
        background_popup_child.resource as usize,
        foreground.resource as usize,
        foreground_chrome.resource as usize,
    ];
    assert_eq!(server.client_surface_frame_order(), expected_order);
    assert_eq!(
        server.toplevel_frame_order(),
        expected_order,
        "the compatibility API must not expose the old global role order"
    );
    assert_eq!(
        server
            .client_surface_frames()
            .iter()
            .map(|frame| frame.id)
            .collect::<Vec<_>>(),
        vec![background_popup_child.resource as usize],
        "subsurfaces attached to xdg-popups must be included in the scene"
    );
}
