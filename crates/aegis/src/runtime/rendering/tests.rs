use super::*;

fn region(x: f32, y: f32, w: f32, h: f32) -> aegis_shell::BackdropRegion {
    aegis_shell::BackdropRegion {
        x,
        y,
        w,
        h,
        wash: None,
    }
}

#[test]
fn backdrop_refresh_is_driven_by_source_footprint() {
    let input = [
        BackdropCaptureRegion {
            origin: (0, 0),
            extent: (1920, 80),
        },
        BackdropCaptureRegion {
            origin: (0, 1000),
            extent: (1920, 80),
        },
    ];
    let video_above_dock = FrameDamage::Area(vec![aegis_model::Rect::new(100, 100, 800, 450)]);
    assert!(backdrop_refresh_regions(true, false, &video_above_dock, &input,).is_empty());
    let video_under_dock = FrameDamage::Area(vec![aegis_model::Rect::new(100, 1020, 800, 60)]);
    assert_eq!(
        backdrop_refresh_regions(true, false, &video_under_dock, &input),
        vec![input[1]]
    );
    assert_eq!(
        backdrop_refresh_regions(false, false, &FrameDamage::None, &input),
        input
    );
    assert_eq!(
        backdrop_refresh_regions(true, true, &FrameDamage::None, &input),
        input
    );
}

#[test]
fn backdrop_cache_key_tracks_geometry_and_material_exactly() {
    let capture = [BackdropCaptureRegion {
        origin: (0, 1000),
        extent: (1920, 80),
    }];
    let frost = [region(0.0, 1040.0, 1920.0, 40.0)];
    let base = BackdropCacheKey::new(
        (0, 1000),
        (1920, 80),
        (1920, 1080),
        12.0,
        1.0,
        false,
        &capture,
        &[],
        &[],
        None,
    );
    let sigma_changed = BackdropCacheKey::new(
        (0, 1000),
        (1920, 80),
        (1920, 1080),
        12.5,
        1.0,
        false,
        &capture,
        &[],
        &[],
        None,
    );
    assert_ne!(base, sigma_changed);
    assert_eq!(base, base.clone());

    let glass = aegis_shell::LiquidGlassRegion {
        bounds: region(400.0, 1000.0, 320.0, 64.0),
        focus: Some(aegis_shell::LiquidGlassFocus {
            bounds: region(420.0, 1008.0, 96.0, 48.0),
            corner_radius: 12.0,
            strength: 1.0,
        }),
        ..Default::default()
    };
    // Glass parameters live in the material key: a focus-geometry change
    // re-runs the effect composite but leaves the scene capture untouched.
    let with_focus = BackdropMaterialKey::new(&frost, &[glass], [255, 255, 255]);
    let moved_focus = BackdropMaterialKey::new(
        &frost,
        &[aegis_shell::LiquidGlassRegion {
            focus: glass.focus.map(|focus| aegis_shell::LiquidGlassFocus {
                bounds: region(
                    focus.bounds.x + 1.0,
                    focus.bounds.y,
                    focus.bounds.w,
                    focus.bounds.h,
                ),
                ..focus
            }),
            ..glass
        }],
        [255, 255, 255],
    );
    assert_ne!(with_focus, moved_focus);
    // Every material leg participates: material strengths, polarity, the
    // adaptation writeback, and the scheme tint.
    let strengthened = BackdropMaterialKey::new(
        &frost,
        &[aegis_shell::LiquidGlassRegion {
            frost_strength: 5.0,
            ..glass
        }],
        [255, 255, 255],
    );
    assert_ne!(with_focus, strengthened);
    let polarized = BackdropMaterialKey::new(
        &frost,
        &[aegis_shell::LiquidGlassRegion {
            plate_polarity: 0.0,
            ..glass
        }],
        [255, 255, 255],
    );
    assert_ne!(with_focus, polarized);
    let adapted = BackdropMaterialKey::new(
        &frost,
        &[aegis_shell::LiquidGlassRegion {
            adaptation: Some(aegis_shell::LiquidGlassAdaptation {
                plate_luminance: 0.5,
                backdrop_energy: 0.25,
            }),
            ..glass
        }],
        [255, 255, 255],
    );
    assert_ne!(with_focus, adapted);
    let tinted = BackdropMaterialKey::new(&frost, &[glass], [243, 245, 249]);
    assert_ne!(with_focus, tinted);
    // …while the capture key no longer sees any of it.
    let capture_with_glass = BackdropCacheKey::new(
        (0, 1000),
        (1920, 80),
        (1920, 1080),
        12.0,
        1.0,
        false,
        &capture,
        &[],
        &[],
        None,
    );
    assert_eq!(base, capture_with_glass);
}

#[test]
fn focused_preview_content_keeps_one_full_brightness_target() {
    let focused = aegis_model::window::WindowId(7);
    let sibling = aegis_model::window::WindowId(8);
    assert_eq!(
        aegis_shell::preview::content_brightness(Some(focused), focused, 0.74),
        1.0
    );
    assert_eq!(
        aegis_shell::preview::content_brightness(Some(focused), sibling, 0.74),
        0.74
    );
    assert_eq!(
        aegis_shell::preview::content_brightness(None, sibling, 0.74),
        1.0
    );
}

#[test]
fn screenshot_freeze_keeps_the_trigger_cursor_snapshot() {
    let trigger = CaptureCursorState {
        position: (42.25, 73.5),
        shape: 7,
        hidden: true,
        client_surface: true,
    };
    let later = CaptureCursorState {
        position: (900.0, 500.0),
        shape: 1,
        hidden: true,
        client_surface: false,
    };
    let mut freeze = ScreenshotFreeze::new();

    freeze.request_open(Some(trigger));
    freeze.request_open(Some(later));
    assert_eq!(freeze.trigger_cursor(), Some(trigger));

    freeze.disarm();
    assert_eq!(freeze.trigger_cursor(), None);
}

#[test]
fn capture_bounds_cover_top_bar_with_blur_margin() {
    // 32px status bar at the top of a 1920x1080 output, sigma 12: the
    // capture spans the full width but only the bar plus the 3σ margin.
    let (origin, size) = blur_capture_bounds(
        &[region(0.0, 0.0, 1920.0, 32.0)],
        (1920, 1080),
        (1920, 1080),
        1.0,
        12.0,
    );
    assert_eq!(origin, (0, 0));
    assert_eq!(size, (1920, 68));
}

#[test]
fn capture_bounds_union_disjoint_regions() {
    // Top bar + bottom dock: the union covers both, including margins.
    let (origin, size) = blur_capture_bounds(
        &[
            region(0.0, 0.0, 1920.0, 32.0),
            region(400.0, 1040.0, 1120.0, 40.0),
        ],
        (1920, 1080),
        (1920, 1080),
        1.0,
        12.0,
    );
    assert_eq!(origin, (0, 0));
    assert_eq!(size, (1920, 1080));
}

#[test]
fn capture_regions_keep_top_bar_and_bottom_dock_disjoint() {
    let regions = blur_capture_regions(
        &[
            region(0.0, 0.0, 1920.0, 32.0),
            region(400.0, 1040.0, 1120.0, 40.0),
        ],
        (1920, 1080),
        (1920, 1080),
        1.0,
        12.0,
    );
    assert_eq!(
        regions,
        vec![
            BackdropCaptureRegion {
                origin: (0, 0),
                extent: (1920, 68),
            },
            BackdropCaptureRegion {
                origin: (364, 1004),
                extent: (1192, 76),
            },
        ]
    );
}

#[test]
fn capture_regions_merge_overlapping_blur_footprints_transitively() {
    let regions = blur_capture_regions(
        &[
            region(100.0, 100.0, 40.0, 40.0),
            region(170.0, 100.0, 40.0, 40.0),
            region(240.0, 100.0, 40.0, 40.0),
        ],
        (400, 300),
        (400, 300),
        1.0,
        12.0,
    );
    assert_eq!(
        regions,
        vec![BackdropCaptureRegion {
            origin: (64, 64),
            extent: (252, 112),
        }]
    );
}

#[test]
fn backdrop_graph_accumulates_sampling_radius_across_three_layers() {
    use aegis_shell::{BackdropLayer, BackdropLayerId, BackdropLayerSource};

    let cover = BackdropLayer::new(BackdropLayerId(1), BackdropLayerSource::Scene, 4.0)
        .with_frost(vec![region(50.0, 50.0, 20.0, 20.0)]);
    let glass = BackdropLayer::new(
        BackdropLayerId(2),
        BackdropLayerSource::Layer(BackdropLayerId(1)),
        3.0,
    )
    .with_frost(vec![region(50.0, 50.0, 20.0, 20.0)]);
    let glass_again = BackdropLayer::new(
        BackdropLayerId(3),
        BackdropLayerSource::Layer(BackdropLayerId(2)),
        2.0,
    )
    .with_frost(vec![region(50.0, 50.0, 20.0, 20.0)]);

    let plan =
        plan_backdrop_graph(&[glass_again, cover, glass], (200, 200), (200, 200), 1.0).unwrap();
    assert_eq!(plan.order, vec![1, 2, 0]);
    assert_eq!(plan.sources, vec![None, Some(0), Some(1)]);
    assert_eq!(
        plan.layer_regions,
        vec![
            vec![BackdropCaptureRegion {
                origin: (38, 38),
                extent: (44, 44),
            }],
            vec![BackdropCaptureRegion {
                origin: (41, 41),
                extent: (38, 38),
            }],
            vec![BackdropCaptureRegion {
                origin: (44, 44),
                extent: (32, 32),
            }],
        ]
    );
    assert_eq!(
        plan.resolve_regions,
        vec![
            vec![BackdropCaptureRegion {
                origin: (35, 35),
                extent: (50, 50),
            }],
            vec![BackdropCaptureRegion {
                origin: (44, 44),
                extent: (32, 32),
            }],
            vec![BackdropCaptureRegion {
                origin: (50, 50),
                extent: (20, 20),
            }],
        ]
    );
    assert_eq!(
        plan.capture_regions,
        vec![BackdropCaptureRegion {
            // 3σ × (4 + 3 + 2) accumulated along the dependency path.
            origin: (23, 23),
            extent: (74, 74),
        }]
    );
}

#[test]
fn backdrop_graph_rejects_missing_sources_and_cycles() {
    use aegis_shell::{BackdropLayer, BackdropLayerId, BackdropLayerSource};

    let missing = BackdropLayer::new(
        BackdropLayerId(1),
        BackdropLayerSource::Layer(BackdropLayerId(9)),
        4.0,
    )
    .with_frost(vec![region(10.0, 10.0, 20.0, 20.0)]);
    assert!(plan_backdrop_graph(&[missing], (100, 100), (100, 100), 1.0).is_err());

    let a = BackdropLayer::new(
        BackdropLayerId(1),
        BackdropLayerSource::Layer(BackdropLayerId(2)),
        4.0,
    )
    .with_frost(vec![region(10.0, 10.0, 20.0, 20.0)]);
    let b = BackdropLayer::new(
        BackdropLayerId(2),
        BackdropLayerSource::Layer(BackdropLayerId(1)),
        4.0,
    )
    .with_frost(vec![region(10.0, 10.0, 20.0, 20.0)]);
    assert!(plan_backdrop_graph(&[a, b], (100, 100), (100, 100), 1.0).is_err());
}

#[test]
fn capture_regions_map_outward_into_downsampled_target() {
    let mapped = blur_regions_in_capture(
        &[BackdropCaptureRegion {
            origin: (101, 121),
            extent: (81, 41),
        }],
        (50, 100),
        (400, 200),
        (200, 100),
    );
    assert_eq!(
        mapped,
        vec![flux::BlurRegion {
            x: 25,
            y: 10,
            width: 41,
            height: 21,
        }]
    );
}

#[test]
#[allow(clippy::modulo_one)]
fn capture_bounds_align_to_downsample() {
    // A floating region: origin/size land on BACKDROP_DOWNSAMPLE
    // multiples so the capture grid stays exact. With a full-resolution
    // capture the bounds are the exact padded region union.
    let (origin, size) = blur_capture_bounds(
        &[region(100.0, 100.0, 200.0, 50.0)],
        (1920, 1080),
        (1920, 1080),
        1.0,
        12.0,
    );
    assert_eq!(origin, (64, 64));
    assert_eq!(size, (272, 122));
    assert_eq!(origin.0 % BACKDROP_DOWNSAMPLE, 0);
    assert_eq!(origin.1 % BACKDROP_DOWNSAMPLE, 0);
    assert_eq!(size.0 % BACKDROP_DOWNSAMPLE, 0);
    assert_eq!(size.1 % BACKDROP_DOWNSAMPLE, 0);
}

#[test]
fn material_fingerprints_track_each_frame_slot_independently() {
    let frost = [region(0.0, 0.0, 0.0, 0.0)];
    let glass = aegis_shell::LiquidGlassRegion {
        bounds: region(400.0, 100.0, 320.0, 74.0),
        ..Default::default()
    };
    let base = BackdropMaterialKey::new(&frost, &[glass], [255, 255, 255]);
    let restyled = BackdropMaterialKey::new(
        &frost,
        &[aegis_shell::LiquidGlassRegion {
            frost_strength: 2.0,
            ..glass
        }],
        [255, 255, 255],
    );

    // Ring warm-up: three slots each accept the base material once.
    let mut slots = Vec::new();
    for slot in 0..3 {
        assert!(slot_material_changed(&mut slots, slot, &base));
    }
    for slot in 0..3 {
        assert!(!slot_material_changed(&mut slots, slot, &base));
    }

    // A material change lands on slot 0 only. Slots 1 and 2 still serve the
    // old composite: their fingerprints must stay pending so each observes
    // the change on its own next frame instead of being told the change was
    // already consumed (which would present the stale shadow/glass composite
    // every time the ring rotates).
    assert!(slot_material_changed(&mut slots, 0, &restyled));
    assert!(!slot_material_changed(&mut slots, 0, &restyled));
    assert!(slot_material_changed(&mut slots, 1, &restyled));
    assert!(slot_material_changed(&mut slots, 2, &restyled));
    for slot in 0..3 {
        assert!(!slot_material_changed(&mut slots, slot, &restyled));
    }
}

#[test]
fn a_material_change_widens_a_partial_refresh_to_every_capture_region() {
    let full = vec![
        BackdropCaptureRegion {
            origin: (0, 0),
            extent: (1920, 80),
        },
        BackdropCaptureRegion {
            origin: (0, 1000),
            extent: (1920, 80),
        },
    ];
    let partial = vec![full[1]];

    // Without a material change the source-damage plan passes through.
    assert_eq!(
        refresh_regions_covering_material_change(false, partial.clone(), &full),
        partial
    );
    // With one, the refresh must cover every capture region so no composite
    // region keeps the previous material.
    assert_eq!(
        refresh_regions_covering_material_change(true, partial, &full),
        full
    );
    // An already-complete refresh and an empty (Recompute) plan pass through.
    assert_eq!(
        refresh_regions_covering_material_change(true, full.clone(), &full),
        full
    );
    assert!(refresh_regions_covering_material_change(true, Vec::new(), &full).is_empty());
}

#[test]
fn capture_bounds_respect_output_scale() {
    // scale=2: regions are logical, bounds physical (16 logical px bar
    // = 32 physical px; margin is 3σ in physical pixels).
    let (origin, size) = blur_capture_bounds(
        &[region(0.0, 0.0, 960.0, 16.0)],
        (960, 540),
        (1920, 1080),
        2.0,
        12.0,
    );
    assert_eq!(origin, (0, 0));
    assert_eq!(size, (1920, 104));
}

#[test]
fn liquid_glass_geometry_maps_to_the_downsampled_capture_once() {
    let source = aegis_shell::LiquidGlassRegion {
        bounds: region(400.0, 100.0, 320.0, 74.0),
        corner_radius: 18.0,
        opacity: 0.4,
        focus: Some(aegis_shell::LiquidGlassFocus {
            bounds: region(480.0, 112.0, 120.0, 50.0),
            corner_radius: 12.0,
            strength: 0.8,
        }),
        ..Default::default()
    };
    // Output scale 2, capture downsample 1/2, physical capture origin
    // (600, 120): capture coordinates are logical*1 - origin*0.5.
    let groups = liquid_glass_groups(
        &[source],
        (600, 120),
        2.0,
        0.5,
        aegis_model::settings::ColorScheme::Dark,
    );
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].tint_color, [255, 255, 255]);
    assert_eq!(groups[0].primary.x, 100.0);
    assert_eq!(groups[0].primary.y, 40.0);
    assert_eq!(groups[0].primary.width, 320.0);
    assert_eq!(groups[0].primary.height, 74.0);
    assert_eq!(groups[0].primary.corner_radius, 18.0);
    assert_eq!(groups[0].opacity, 0.4);
    assert!(groups[0].merged.is_none());
    let focus = groups[0]
        .focus
        .expect("focus should share the capture mapping");
    assert_eq!(focus.shape.x, 180.0);
    assert_eq!(focus.shape.y, 52.0);
    assert_eq!(focus.shape.width, 120.0);
    assert_eq!(focus.shape.height, 50.0);
    assert_eq!(focus.shape.corner_radius, 12.0);
    assert_eq!(focus.strength, 0.8);
}

#[test]
fn capture_bounds_clamp_to_physical_extent() {
    // Bottom-edge dock: the margin past the screen edge is clamped away.
    let (origin, size) = blur_capture_bounds(
        &[region(400.0, 1048.0, 1120.0, 32.0)],
        (1920, 1080),
        (1920, 1080),
        1.0,
        12.0,
    );
    assert_eq!(origin, (364, 1012));
    assert_eq!(size, (1192, 68));
}

#[test]
fn capture_bounds_fall_back_to_full_frame_without_regions() {
    let (origin, size) = blur_capture_bounds(
        &[region(10.0, 10.0, 0.0, 0.0)],
        (1920, 1080),
        (1920, 1080),
        1.0,
        12.0,
    );
    assert_eq!(origin, (0, 0));
    assert_eq!(size, (1920, 1080));
}

#[test]
fn opaque_frame_fill_replaces_undefined_and_previous_contents() {
    let Ok(device) = flux::Device::new(true, &[], &[], 0) else {
        return;
    };
    let size = (32, 24);
    let surface = flux::Surface::offscreen(&device, size.0, size.1).unwrap();
    let canvas = flux::Canvas::new(&surface).unwrap();

    for expected in [[13, 77, 191, 255], [211, 43, 29, 255]] {
        let frame = surface.begin_frame().unwrap();
        begin_opaque_frame(
            &canvas,
            &frame,
            flux::rgba(expected[0], expected[1], expected[2], expected[3]),
        )
        .unwrap();
        canvas.end_frame_checked().unwrap();
        frame.submit().unwrap().present().unwrap();

        let mut pixels = vec![0; size.0 as usize * size.1 as usize * 4];
        surface.read_pixels(&mut pixels).unwrap();
        // ADR-0069 (optics): the output transform dithers ±1 LSB when
        // quantizing to 8 bit, so an opaque fill is exact up to one
        // quantum per channel, not bit-identical.
        assert!(
            pixels.chunks_exact(4).all(|pixel| {
                pixel
                    .iter()
                    .zip(expected.iter())
                    .all(|(got, want)| got.abs_diff(*want) <= 1)
            }),
            "opaque fill did not replace every output pixel"
        );
    }
}

#[test]
fn damaged_base_and_stencil_overlay_preserve_pixels_outside_the_scissor() {
    let Ok(device) = flux::Device::new(true, &[], &[], 1) else {
        return;
    };
    let size = (32, 24);
    let surface = flux::Surface::offscreen(&device, size.0, size.1).unwrap();
    let canvas = flux::Canvas::new(&surface).unwrap();

    let frame = surface.begin_frame().unwrap();
    begin_opaque_frame(&canvas, &frame, flux::rgba(200, 30, 20, 255)).unwrap();
    canvas.end_frame_checked().unwrap();
    frame.submit().unwrap().present().unwrap();

    let frame = surface.begin_frame().unwrap();
    let repaint = FrameDamage::Area(vec![aegis_model::Rect::new(8, 6, 10, 9)]);
    begin_opaque_frame_repaint(
        &canvas,
        &frame,
        size,
        flux::rgba(10, 80, 220, 255),
        &repaint,
    )
    .unwrap();
    // End the optimized image/base pass, then exercise the exact pass
    // boundary used before Lens. A self-intersecting fill is intentional:
    // Flux rejects it from a no-stencil pass, so successful checked end
    // proves the overlay really has stencil rather than merely being a
    // second no-stencil LOAD pass.
    canvas.end_frame_checked().unwrap();
    begin_stencil_frame_overlay(&canvas, &frame, frame_damage_render_area(&repaint)).unwrap();
    let arena = flux::Arena::with_capacity(4096).unwrap();
    let path = flux::Path::new(&arena).unwrap();
    path.move_to(11.0, 8.0)
        .line_to(15.0, 12.0)
        .line_to(11.0, 12.0)
        .line_to(15.0, 8.0)
        .close();
    canvas.fill_path(&path, &flux::Paint::solid(flux::rgba(20, 220, 70, 255)));
    canvas.end_frame_checked().unwrap();
    frame.submit().unwrap().present().unwrap();

    let mut pixels = vec![0; size.0 as usize * size.1 as usize * 4];
    surface.read_pixels(&mut pixels).unwrap();
    let pixel = |x: usize, y: usize| &pixels[(y * size.0 as usize + x) * 4..][..4];
    assert_eq!(pixel(0, 0), [200, 30, 20, 255]);
    assert_eq!(pixel(9, 7), [10, 80, 220, 255]);
    assert_eq!(pixel(17, 14), [10, 80, 220, 255]);
    assert_eq!(pixel(18, 15), [200, 30, 20, 255]);
}

#[test]
fn frame_damage_render_area_uses_the_exact_union() {
    let damage = FrameDamage::Area(vec![
        aegis_model::Rect::new(11, 7, 5, 9),
        aegis_model::Rect::new(29, 3, 4, 8),
    ]);
    assert_eq!(
        frame_damage_render_area(&damage),
        Some(flux::CanvasRenderArea {
            x: 11,
            y: 3,
            width: 22,
            height: 13,
        })
    );
    assert_eq!(frame_damage_render_area(&FrameDamage::Full), None);
    assert_eq!(frame_damage_render_area(&FrameDamage::None), None);
}
