use super::*;

#[test]
fn persisted_workspace_uses_output_position_not_stable_id() {
    let mut state = State::new(std::ptr::null_mut());
    let first = state
        .workspaces
        .current_workspace(state.output)
        .expect("bootstrap workspace");
    let first_window = aegis_model::window::WindowId(101);
    state.workspaces.place_toplevel(first, first_window);

    let second = state
        .workspaces
        .output(state.output)
        .and_then(|output| output.workspaces.get(1))
        .copied()
        .expect("placing on the first workspace creates a trailing workspace");
    let second_window = aegis_model::window::WindowId(102);
    state.workspaces.place_toplevel(second, second_window);

    assert_eq!(state.workspace_number_for_window(first_window), Some(1));
    assert_eq!(state.workspace_number_for_window(second_window), Some(2));
}

#[test]
fn backend_outputs_reconcile_workspace_connectors_and_geometry() {
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipping: XDG_RUNTIME_DIR not set");
        return;
    }
    let mut server = Server::new().expect("Server::new");
    let geometry = |x, width| aegis_model::output::OutputGeometry {
        mode: aegis_model::output::OutputMode {
            width,
            height: 1080,
            refresh_mhz: 60_000,
        },
        scale: aegis_model::output::Scale::IDENTITY,
        transform: aegis_model::Transform::Normal,
        logical_origin: aegis_model::Point { x, y: 0 },
    };
    server.set_outputs(vec![
        aegis_model::output::OutputInfo {
            connector: "DP-1".into(),
            geometry: geometry(0, 1920),
            available_modes: Vec::new(),
            color_caps: aegis_model::edid::EdidColorCapabilities::default(),
        },
        aegis_model::output::OutputInfo {
            connector: "HDMI-A-1".into(),
            geometry: geometry(1920, 2560),
            available_modes: Vec::new(),
            color_caps: aegis_model::edid::EdidColorCapabilities::default(),
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
    let geometry = |x, width| aegis_model::output::OutputGeometry {
        mode: aegis_model::output::OutputMode {
            width,
            height: 1080,
            refresh_mhz: 60_000,
        },
        scale: aegis_model::output::Scale::IDENTITY,
        transform: aegis_model::Transform::Normal,
        logical_origin: aegis_model::Point { x, y: 0 },
    };
    server.set_outputs(vec![
        aegis_model::output::OutputInfo {
            connector: "DP-1".into(),
            geometry: geometry(0, 1920),
            available_modes: Vec::new(),
            color_caps: aegis_model::edid::EdidColorCapabilities::default(),
        },
        aegis_model::output::OutputInfo {
            connector: "HDMI-A-1".into(),
            geometry: geometry(1920, 2560),
            available_modes: Vec::new(),
            color_caps: aegis_model::edid::EdidColorCapabilities::default(),
        },
    ]);

    server.set_output_policies(std::collections::HashMap::from([(
        "HDMI-A-1".to_owned(),
        aegis_model::output::OutputPolicy {
            scale: Some(2.0),
            position: Some(aegis_model::Point { x: 1920, y: 0 }),
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
        aegis_model::Point { x: 1920, y: 0 }
    );
    assert_eq!(
        server.output_logical_rect().origin,
        aegis_model::Point { x: 1920, y: 0 },
        "the focused output geometry follows the primary policy"
    );
    // The other output keeps its backend-reported geometry.
    assert_eq!(infos[1].geometry.scale.as_f32(), 1.0);
    assert_eq!(
        infos[1].geometry.logical_origin,
        aegis_model::Point { x: 0, y: 0 }
    );
}

#[test]
fn surface_damage_accumulates_until_present_and_full_is_sticky() {
    let mut surface = SurfaceRec::new(std::ptr::null_mut());
    accumulate_committed_damage(
        &mut surface,
        vec![aegis_model::Rect::new(10, 20, 30, 40)],
        false,
    );
    accumulate_committed_damage(
        &mut surface,
        vec![aegis_model::Rect::new(100, 5, 10, 10)],
        false,
    );
    assert_eq!(
        surface.committed_damage,
        vec![
            aegis_model::Rect::new(10, 20, 30, 40),
            aegis_model::Rect::new(100, 5, 10, 10),
        ]
    );
    assert!(!surface.committed_damage_full);

    accumulate_committed_damage(&mut surface, Vec::new(), true);
    assert!(surface.committed_damage.is_empty());
    assert!(surface.committed_damage_full);

    // Once any commit lacked usable damage, later precise reports cannot
    // narrow the aggregate until presentation acknowledges the full update.
    accumulate_committed_damage(
        &mut surface,
        vec![aegis_model::Rect::new(1, 1, 2, 2)],
        false,
    );
    assert!(surface.committed_damage.is_empty());
    assert!(surface.committed_damage_full);
}

#[test]
fn surface_damage_region_subtracts_overlap_without_inventing_bbox_pixels() {
    let mut surface = SurfaceRec::new(std::ptr::null_mut());
    accumulate_committed_damage(
        &mut surface,
        vec![aegis_model::Rect::new(0, 0, 10, 10)],
        false,
    );
    accumulate_committed_damage(
        &mut surface,
        vec![aegis_model::Rect::new(5, 5, 10, 10)],
        false,
    );

    assert!(aegis_model::Rect::new(0, 0, 10, 10).fully_covered_by(&surface.committed_damage));
    assert!(aegis_model::Rect::new(5, 5, 10, 10).fully_covered_by(&surface.committed_damage));
    assert!(
        !surface
            .committed_damage
            .iter()
            .any(|rect| rect.contains(aegis_model::Point { x: 12, y: 2 })),
        "the overlap union must not dirty the bounding-box-only corner"
    );
}

#[test]
fn surface_damage_region_caps_pathological_client_lists() {
    let mut surface = SurfaceRec::new(std::ptr::null_mut());
    let rects = (0..=MAX_COMMITTED_DAMAGE_RECTS)
        .map(|index| aegis_model::Rect::new((index * 2) as i32, 0, 1, 1))
        .collect();
    accumulate_committed_damage(&mut surface, rects, false);

    assert_eq!(surface.committed_damage.len(), 1);
    assert_eq!(
        surface.committed_damage[0],
        aegis_model::Rect::new(0, 0, (MAX_COMMITTED_DAMAGE_RECTS * 2 + 1) as i32, 1)
    );
}

#[test]
fn surface_damage_region_promotes_unrepresentable_span_to_full() {
    let mut surface = SurfaceRec::new(std::ptr::null_mut());
    let mut rects = (0..MAX_COMMITTED_DAMAGE_RECTS)
        .map(|index| aegis_model::Rect::new(i32::MIN + (index as i32 * 2), 0, 1, 1))
        .collect::<Vec<_>>();
    rects.push(aegis_model::Rect::new(i32::MAX - 1, 0, 1, 1));
    accumulate_committed_damage(&mut surface, rects, false);

    assert!(surface.committed_damage_full);
    assert!(surface.committed_damage.is_empty());
}

#[test]
fn buffer_damage_at_hidpi_scale_rounds_outward_to_surface_coordinates() {
    let mapped = buffer_damage_to_surface(aegis_model::Rect::new(3, 5, 8, 10), 2);
    assert_eq!(mapped, aegis_model::Rect::new(1, 2, 5, 6));
}

#[test]
fn committed_opaque_region_culls_a_fully_covered_window_tree() {
    let mut state = State::new(std::ptr::null_mut());
    state.output_geometry = aegis_model::output::OutputGeometry {
        mode: aegis_model::output::OutputMode {
            width: 100,
            height: 80,
            refresh_mhz: 60_000,
        },
        ..Default::default()
    };
    let background_id = aegis_model::window::WindowId(301);
    let foreground_id = aegis_model::window::WindowId(302);
    let client = state.authority.register_client(None);
    state
        .authority
        .create_interaction_group(
            client,
            &[background_id, foreground_id],
            HUMAN_INTERACTION_DOMAIN,
        )
        .expect("register physical window authority");
    let workspace = state
        .workspaces
        .current_workspace(state.output)
        .expect("bootstrap workspace");
    state.workspaces.place_toplevel(workspace, background_id);
    state.workspaces.place_toplevel(workspace, foreground_id);
    let output = state.output_geometry.logical_rect();

    let mut background = Box::new(SurfaceRec::new(0x301usize as *mut ffi::wl_resource));
    background.mapped = true;
    background.xdg_toplevel = background.resource;
    background.window.id = background_id;
    background.window.size = output.size;
    background.position = output.origin;
    background.width = output.size.w;
    background.height = output.size.h;
    background.pixels = vec![0x20; 4];

    let mut foreground = Box::new(SurfaceRec::new(0x302usize as *mut ffi::wl_resource));
    foreground.mapped = true;
    foreground.xdg_toplevel = foreground.resource;
    foreground.window.id = foreground_id;
    foreground.window.size = output.size;
    foreground.position = output.origin;
    foreground.width = output.size.w;
    foreground.height = output.size.h;
    foreground.pixels = vec![0xE0; 4];
    foreground.opaque_region = Some(vec![aegis_model::Rect::new(
        0,
        0,
        output.size.w - 1,
        output.size.h,
    )]);

    state.surfaces = vec![background.as_mut(), foreground.as_mut()];
    let mut server = std::mem::ManuallyDrop::new(Server {
        state: Box::new(state),
        socket: String::new(),
        interaction_domain_portals: Vec::new(),
        epoch: std::time::Instant::now(),
    });

    assert!(!server.occluded_window_ids().contains(&background_id));
    foreground.opaque_region = Some(vec![aegis_model::Rect::new(
        0,
        0,
        output.size.w,
        output.size.h,
    )]);
    // An actively moving foreground cannot safely occlude the pixels it is
    // moving away from/to.
    foreground.window.transition = Some(aegis_model::transition::WindowTransition {
        from: aegis_model::Rect::new(0, 0, output.size.w / 2, output.size.h / 2),
        started_ms: server.now_ms(),
        duration_ms: 60_000,
        easing: aegis_model::transition::Easing::EaseOutCubic,
        effect: None,
    });
    assert!(!server.occluded_window_ids().contains(&background_id));

    // Once the transition settles it is normalized out of model state. The
    // same opaque foreground must immediately resume contributing coverage,
    // otherwise a covered video window continues rendering indefinitely.
    foreground.window.transition = Some(aegis_model::transition::WindowTransition {
        from: aegis_model::Rect::new(0, 0, output.size.w / 2, output.size.h / 2),
        started_ms: 0,
        duration_ms: 0,
        easing: aegis_model::transition::Easing::EaseOutCubic,
        effect: None,
    });
    assert_eq!(server.settle_finished_transitions(), 1);
    assert!(foreground.window.transition.is_none());
    let occluded = server.occluded_window_ids();
    assert!(occluded.contains(&background_id));
    assert!(!occluded.contains(&foreground_id));

    let physical_windows = server
        .client_surface_frames()
        .into_iter()
        .filter_map(|frame| frame.window)
        .collect::<Vec<_>>();
    assert_eq!(physical_windows, vec![foreground_id]);

    let preview_windows = server
        .client_preview_surface_frames()
        .into_iter()
        .filter_map(|frame| frame.window)
        .collect::<Vec<_>>();
    assert_eq!(preview_windows, vec![background_id, foreground_id]);
    assert_eq!(
        server.client_preview_surface_frame_order(),
        vec![background.resource as usize, foreground.resource as usize]
    );
}

#[test]
fn xdg_unmap_requires_a_fresh_initial_configure() {
    let mut surface = SurfaceRec::new(std::ptr::null_mut());
    surface.xdg_configured = true;
    surface.xdg_configure_acked = true;
    surface.pending_xdg_configures = vec![41, 42];

    reset_xdg_configure_state_after_unmap(&mut surface);

    assert!(!surface.xdg_configured);
    assert!(!surface.xdg_configure_acked);
    assert!(surface.pending_xdg_configures.is_empty());
}

#[test]
fn test_persist_app_geometry_saves_state() {
    let temp_dir = std::env::temp_dir().join(format!("aegis_test_persist_{}", std::process::id()));
    let state_file = temp_dir.join("window_state.json");
    let mut state = State::new(std::ptr::null_mut());
    state.window_state_path = state_file.clone();

    let app_id = "org.test.app";
    let rect = aegis_model::Rect {
        origin: aegis_model::Point { x: 300, y: 400 },
        size: aegis_model::Size { w: 900, h: 600 },
    };

    state.persist_app_geometry(
        app_id,
        rect,
        Some(2),
        Some(aegis_model::layout::LayoutRole::Floating),
    );

    assert_eq!(state.last_app_geometries.get(app_id), Some(&rect));
    let store_entry = state.window_state_store.get(app_id).unwrap();
    assert_eq!(store_entry.position, Some(rect.origin));
    assert_eq!(store_entry.size, Some(rect.size));
    assert_eq!(store_entry.workspace, Some(2));
    assert_eq!(
        store_entry.layout_role,
        Some(aegis_model::layout::LayoutRole::Floating)
    );

    assert!(state_file.exists());
    let loaded = window_state::WindowStateStore::load_from_path(&state_file);
    assert_eq!(loaded.get(app_id).unwrap(), store_entry);

    let _ = std::fs::remove_dir_all(temp_dir);
}
