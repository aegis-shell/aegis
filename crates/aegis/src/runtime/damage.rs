use super::*;

/// Output-level damage for one presentation frame.
///
/// The pipeline is conservative by design: any uncertainty resolves to
/// [`FrameDamage::Full`] (or to not skipping), because a missed region leaves
/// stale pixels on screen while an over-large region only costs a hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FrameDamage {
    /// Nothing visible changed: rendering and presentation may be skipped.
    None,
    /// Conservative full-output damage.
    Full,
    /// Tight bounding box in physical desktop (framebuffer) pixels.
    Area(aegis_core::Rect),
}

#[derive(Debug, Clone, Copy)]
pub(super) struct DamageAssessment {
    pub had_input: bool,
    pub session_locked: bool,
    pub cursor_hidden: bool,
    pub cursor_shape: u32,
    pub software_cursor: bool,
    pub scale: f32,
    pub physical_size: (u32, u32),
}

fn union_frame_damage(left: FrameDamage, right: FrameDamage) -> FrameDamage {
    match (left, right) {
        (FrameDamage::Full, _) | (_, FrameDamage::Full) => FrameDamage::Full,
        (FrameDamage::None, damage) | (damage, FrameDamage::None) => damage,
        (FrameDamage::Area(left), FrameDamage::Area(right)) => {
            let mut union = Some(left);
            union_bbox(&mut union, right);
            FrameDamage::Area(union.expect("two non-empty rectangles have a union"))
        }
    }
}

/// Damage that must be repainted into `slot`, including every change that
/// happened while another ring image was scanned out.
pub(super) fn composite_repaint_for_slot(
    slots: &mut Vec<FrameDamage>,
    slot: usize,
    current: FrameDamage,
) -> FrameDamage {
    if slots.len() <= slot {
        slots.resize(slot + 1, FrameDamage::Full);
    }
    union_frame_damage(slots[slot], current)
}

/// Advance ring-image damage history only after a successful presentation.
pub(super) fn record_composite_present(
    slots: &mut Vec<FrameDamage>,
    slot: usize,
    current: FrameDamage,
) {
    if slots.len() <= slot {
        slots.resize(slot + 1, FrameDamage::Full);
    }
    for (index, pending) in slots.iter_mut().enumerate() {
        if index == slot {
            *pending = FrameDamage::None;
        } else {
            *pending = union_frame_damage(*pending, current);
        }
    }
}

/// Damage contributed by client surface commits, in compositor logical
/// coordinates.
enum ClientDamage {
    None,
    Area(aegis_core::Rect),
    Full,
}

/// Surface state as of the previous damage assessment. Geometry belongs in
/// the baseline alongside content generation: moving/resizing a surface
/// exposes its old pixels even when the buffer itself did not change.
pub(super) struct SurfaceDamageBaseline {
    generation: u64,
    geometry: aegis_core::SurfaceGeometry,
    width: i32,
    height: i32,
}

/// Union of one surface's changed region in compositor logical coordinates:
/// the (clamped) damage rects translated by the surface's compositor
/// position, or the whole surface rect when the commit carried no usable
/// damage information (`damage` empty). The clamping mirrors the renderer's
/// incremental-upload path so both agree on what "usable damage" means.
fn surface_damage_logical(
    position: aegis_core::Point,
    width: i32,
    height: i32,
    damage: &[aegis_core::Rect],
) -> Option<aegis_core::Rect> {
    if width <= 0 || height <= 0 {
        return None;
    }
    if damage.is_empty() {
        return Some(aegis_core::Rect::new(position.x, position.y, width, height));
    }
    let mut x0 = i32::MAX;
    let mut y0 = i32::MAX;
    let mut x1 = i32::MIN;
    let mut y1 = i32::MIN;
    for d in damage {
        let x = d.origin.x.max(0).min(width - 1);
        let y = d.origin.y.max(0).min(height - 1);
        let w = d.size.w.max(0).min(width - x);
        let h = d.size.h.max(0).min(height - y);
        if w == 0 || h == 0 {
            continue;
        }
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x + w);
        y1 = y1.max(y + h);
    }
    if x0 < x1 && y0 < y1 {
        Some(aegis_core::Rect::new(
            position.x.saturating_add(x0),
            position.y.saturating_add(y0),
            x1.saturating_sub(x0),
            y1.saturating_sub(y0),
        ))
    } else {
        None
    }
}

fn union_bbox(bbox: &mut Option<aegis_core::Rect>, rect: aegis_core::Rect) {
    *bbox = Some(match *bbox {
        Some(b) => {
            let x0 = b.origin.x.min(rect.origin.x);
            let y0 = b.origin.y.min(rect.origin.y);
            let x1 = b
                .origin
                .x
                .saturating_add(b.size.w)
                .max(rect.origin.x.saturating_add(rect.size.w));
            let y1 = b
                .origin
                .y
                .saturating_add(b.size.h)
                .max(rect.origin.y.saturating_add(rect.size.h));
            aegis_core::Rect::new(x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0))
        }
        None => rect,
    });
}

/// Map a logical bounding box onto the physical framebuffer, rounding
/// outward and clamping to the physical extent. `None` means the mapping is
/// not trustworthy (bad scale/extent) and the caller must fall back to full
/// damage.
fn logical_to_physical(
    rect: aegis_core::Rect,
    scale: f32,
    physical: (u32, u32),
) -> Option<aegis_core::Rect> {
    if !scale.is_finite() || scale <= 0.0 || physical.0 == 0 || physical.1 == 0 {
        return None;
    }
    let (pw, ph) = (physical.0 as f32, physical.1 as f32);
    let x0 = (rect.origin.x as f32 * scale).floor().clamp(0.0, pw);
    let y0 = (rect.origin.y as f32 * scale).floor().clamp(0.0, ph);
    let x1 = (rect.origin.x.saturating_add(rect.size.w) as f32 * scale)
        .ceil()
        .clamp(0.0, pw);
    let y1 = (rect.origin.y.saturating_add(rect.size.h) as f32 * scale)
        .ceil()
        .clamp(0.0, ph);
    (x1 > x0 && y1 > y0).then_some(aegis_core::Rect::new(
        x0 as i32,
        y0 as i32,
        (x1 - x0) as i32,
        (y1 - y0) as i32,
    ))
}

/// Wall-clock minute, used to keep the status-bar clock honest: chrome draws
/// `HH:MM` from the system clock, so at least one frame must be presented
/// after each minute rollover even when nothing else changed.
pub(super) fn wall_clock_minute() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() / 60)
        .unwrap_or(0)
}

/// Record one visible surface into the generation diff. Returns `true` when
/// this surface alone forces full damage: it appeared since the last
/// assessment, or its contents changed in a way that cannot be mapped to a
/// rectangle (buffer transform, buffer scale, wp_viewport, or an in-flight
/// window transition — the same conditions under which the renderer falls
/// back to a whole-texture upload).
struct SurfaceDamageSample<'a> {
    id: usize,
    generation: u64,
    damage: Option<&'a [aegis_core::Rect]>,
    geometry: &'a aegis_core::SurfaceGeometry,
    width: i32,
    height: i32,
}

fn accumulate_surface(
    old: &std::collections::HashMap<usize, SurfaceDamageBaseline>,
    new: &mut std::collections::HashMap<usize, SurfaceDamageBaseline>,
    sample: SurfaceDamageSample<'_>,
    bbox: &mut Option<aegis_core::Rect>,
) -> bool {
    let SurfaceDamageSample {
        id,
        generation,
        damage,
        geometry,
        width,
        height,
    } = sample;
    new.insert(
        id,
        SurfaceDamageBaseline {
            generation,
            geometry: *geometry,
            width,
            height,
        },
    );
    let Some(previous) = old.get(&id) else {
        return true;
    };
    let geometry_unchanged = previous.width == width
        && previous.height == height
        && previous.geometry.position == geometry.position
        && previous.geometry.window_geometry == geometry.window_geometry
        && previous.geometry.transform == geometry.transform
        && previous.geometry.buffer_scale == geometry.buffer_scale
        && previous.geometry.viewport_src == geometry.viewport_src
        && previous.geometry.viewport_dst == geometry.viewport_dst
        && previous.geometry.transition_size == geometry.transition_size;
    if !geometry_unchanged {
        // The old rect must be erased as well as the new one painted. Without
        // storing a region history across every stacking/clip case, full
        // damage is the only universally correct answer.
        return true;
    }
    if previous.generation == generation {
        return false;
    }
    let mappable = geometry.transform == aegis_core::Transform::Normal
        && geometry.buffer_scale <= 1
        && geometry.viewport_src.is_none()
        && geometry.viewport_dst.is_none()
        && geometry.transition_size.is_none();
    if !mappable {
        return true;
    }
    if let Some(area) =
        surface_damage_logical(geometry.position, width, height, damage.unwrap_or(&[]))
    {
        union_bbox(bbox, area);
    }
    false
}

impl CompositorRuntime {
    /// Damage from client surface commits since the previous assessment,
    /// tracked by the per-surface content generations the renderer already
    /// uses for texture-upload gating. Covers every surface class the scene
    /// draw pulls: toplevels, subsurfaces, overlays (including the client
    /// cursor surface), and lock surfaces.
    fn client_damage(&mut self) -> ClientDamage {
        let old = std::mem::take(&mut self.last_surface_gens);
        let mut new: std::collections::HashMap<usize, SurfaceDamageBaseline> =
            std::collections::HashMap::with_capacity(old.len());
        let mut bbox: Option<aegis_core::Rect> = None;
        let mut full = false;

        let server = &self.server;
        for frames in [
            server.client_surface_frames(),
            server.overlay_frames(),
            server.lock_frames(),
        ] {
            for frame in &frames {
                full |= accumulate_surface(
                    &old,
                    &mut new,
                    SurfaceDamageSample {
                        id: frame.id,
                        generation: frame.generation,
                        damage: Some(frame.damage),
                        geometry: &frame.geometry,
                        width: frame.width,
                        height: frame.height,
                    },
                    &mut bbox,
                );
            }
        }
        // dma-buf surfaces carry no committed damage rects; a generation
        // change conservatively damages the whole surface.
        for frames in [
            server.client_surface_dmabuf_frames(),
            server.overlay_dmabuf_frames(),
            server.lock_dmabuf_frames(),
        ] {
            for frame in &frames {
                full |= accumulate_surface(
                    &old,
                    &mut new,
                    SurfaceDamageSample {
                        id: frame.id,
                        generation: frame.generation,
                        damage: None,
                        geometry: &frame.geometry,
                        width: frame.width,
                        height: frame.height,
                    },
                    &mut bbox,
                );
            }
        }
        // A surface that vanished since the last assessment uncovers whatever
        // was behind it; only its old rectangle is known to be stale, but its
        // stacking interplay is not tracked, so stay conservative.
        full |= old.keys().any(|id| !new.contains_key(id));
        self.last_surface_gens = new;

        if full {
            ClientDamage::Full
        } else {
            match bbox {
                Some(rect) => ClientDamage::Area(rect),
                None => ClientDamage::None,
            }
        }
    }

    /// Assess this frame's output damage from every change signal the
    /// compositor already tracks. Anything not provably unchanged resolves
    /// to [`FrameDamage::Full`]. [`FrameDamage::None`] is the only verdict
    /// that lets the caller skip rendering entirely, so the burden of proof
    /// is on "nothing changed".
    pub(super) fn assess_frame_damage(&mut self, assessment: DamageAssessment) -> FrameDamage {
        let DamageAssessment {
            had_input,
            session_locked,
            cursor_hidden,
            cursor_shape,
            software_cursor,
            scale,
            physical_size,
        } = assessment;
        let (notif_revision, do_not_disturb) = {
            let queue = self.notif_queue.lock().unwrap();
            (queue.revision(), queue.do_not_disturb())
        };
        // Modal chrome states whose transitions are not otherwise signed:
        // overview, keyboard-grabbing chrome (launcher), and the screenshot
        // selector.
        let chrome_mode = (
            self.shell.overview_active(),
            self.shell.window_switcher_active(),
            self.shell.captures_keyboard(),
            self.shell.screenshot_active(),
        );
        let full = self.frame_count == 0
            || self.force_full_redraw
            // Any input event may start a chrome hover/press redraw or move
            // the software cursor; input is rare while truly idle, so treat
            // it as full damage rather than tracking hover precisely.
            || had_input
            || session_locked != self.last_session_locked
            || (software_cursor
                && self.last_presented_cursor != Some((cursor_shape, cursor_hidden)))
            || self.shell.anim_pending()
            || self.server.transitions_pending()
            // A live 3D model or an animated (video/GIF) wallpaper changes
            // every frame it advances.
            || self
                .wallpaper
                .as_ref()
                .is_some_and(|w| w.has_model() || w.next_frame_in().is_some())
            // Server-side topology: window list/geometry, workspace, output,
            // and Realm model changes all feed both the scene and the chrome.
            || self.last_windows_hash != Some(self.server.windows_signature())
            || self.last_ws_sig != Some(self.server.workspace_signature())
            || self.last_outputs_revision != Some(self.server.outputs_revision())
            || self.last_realm_revision != Some(self.server.realm_revision())
            // Toasts and the do-not-disturb indicator follow the notification
            // queue (arrivals, dismissals, expiry).
            || self.last_notif_revision != Some(notif_revision)
            || self.last_chrome_mode != Some(chrome_mode)
            // Shell mutations applied outside the signed paths (status poller,
            // config reload, app rescan, IPC settings/Realm control).
            || self.chrome_dirty
            // The fanout pushes these into chrome when they drift.
            || self.system_status.do_not_disturb != do_not_disturb
            || self.system_status.tiled != self.server.tiling()
            // The status-bar clock is read from wall time at render; force a
            // frame after each minute rollover.
            || self.last_present_minute != Some(wall_clock_minute());

        self.last_session_locked = session_locked;
        self.last_notif_revision = Some(notif_revision);
        self.last_chrome_mode = Some(chrome_mode);
        self.chrome_dirty = false;

        if full {
            return FrameDamage::Full;
        }
        // Building borrowed views for every shm/dma-buf surface is only
        // worthwhile when client content is the remaining possible source of
        // damage. Input, animations, topology/chrome changes and capture
        // already require a full frame; deferring this scan avoids duplicate
        // scene traversal on the compositor's hottest paths. Any generations
        // skipped here remain different and are conservatively picked up once
        // the full-damage condition clears.
        let client = self.client_damage();
        match client {
            ClientDamage::None => FrameDamage::None,
            ClientDamage::Full => FrameDamage::Full,
            // A logical area that does not map cleanly onto the framebuffer
            // must not silently become "no damage".
            ClientDamage::Area(rect) => match logical_to_physical(rect, scale, physical_size) {
                Some(physical) => FrameDamage::Area(physical),
                None => FrameDamage::Full,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_damage_maps_and_clamps_to_surface() {
        // Tight damage translates by the compositor position.
        let area = surface_damage_logical(
            aegis_core::Point { x: 100, y: 50 },
            800,
            600,
            &[aegis_core::Rect::new(10, 20, 30, 40)],
        );
        assert_eq!(area, Some(aegis_core::Rect::new(110, 70, 30, 40)));
        // Out-of-bounds damage clamps like the renderer's upload path.
        let clamped = surface_damage_logical(
            aegis_core::Point { x: 0, y: 0 },
            800,
            600,
            &[aegis_core::Rect::new(-20, 590, 900, 50)],
        );
        assert_eq!(clamped, Some(aegis_core::Rect::new(0, 590, 800, 10)));
        // No damage information damages the whole surface.
        let whole = surface_damage_logical(aegis_core::Point { x: 5, y: 6 }, 800, 600, &[]);
        assert_eq!(whole, Some(aegis_core::Rect::new(5, 6, 800, 600)));
        // Fully degenerate damage reports nothing.
        let degenerate = surface_damage_logical(
            aegis_core::Point { x: 0, y: 0 },
            800,
            600,
            &[aegis_core::Rect::new(10, 10, 0, 0)],
        );
        assert_eq!(degenerate, None);
    }

    #[test]
    fn union_bbox_grows_to_cover() {
        let mut bbox = None;
        union_bbox(&mut bbox, aegis_core::Rect::new(10, 10, 20, 20));
        union_bbox(&mut bbox, aegis_core::Rect::new(0, 40, 15, 10));
        assert_eq!(bbox, Some(aegis_core::Rect::new(0, 10, 30, 40)));
    }

    #[test]
    fn frame_damage_union_preserves_full_and_joins_areas() {
        let a = FrameDamage::Area(aegis_core::Rect::new(10, 20, 30, 40));
        let b = FrameDamage::Area(aegis_core::Rect::new(0, 50, 20, 20));
        assert_eq!(
            union_frame_damage(a, b),
            FrameDamage::Area(aegis_core::Rect::new(0, 20, 40, 50))
        );
        assert_eq!(union_frame_damage(FrameDamage::None, a), a);
        assert_eq!(union_frame_damage(FrameDamage::Full, a), FrameDamage::Full);
    }

    #[test]
    fn logical_to_physical_rounds_outward_and_clamps() {
        // scale 2: outward rounding keeps every touched physical pixel.
        let mapped = logical_to_physical(aegis_core::Rect::new(5, 5, 11, 11), 2.0, (1920, 1080));
        assert_eq!(mapped, Some(aegis_core::Rect::new(10, 10, 22, 22)));
        // Extending past the framebuffer clamps to the physical extent.
        let clamped =
            logical_to_physical(aegis_core::Rect::new(900, 500, 100, 100), 2.0, (1920, 1080));
        assert_eq!(clamped, Some(aegis_core::Rect::new(1800, 1000, 120, 80)));
        // Fully off-screen and unusable scales report no mapping.
        assert_eq!(
            logical_to_physical(aegis_core::Rect::new(-500, 0, 100, 100), 1.0, (1920, 1080)),
            None
        );
        assert_eq!(
            logical_to_physical(aegis_core::Rect::new(0, 0, 10, 10), 0.0, (1920, 1080)),
            None
        );
    }

    #[test]
    fn ring_slot_damage_accumulates_missed_frames() {
        let first = FrameDamage::Area(aegis_core::Rect::new(10, 10, 20, 20));
        let second = FrameDamage::Area(aegis_core::Rect::new(40, 40, 10, 10));
        let mut slots = Vec::new();

        // Every ring image begins undefined and therefore repaints in full on
        // its first acquisition.
        assert_eq!(
            composite_repaint_for_slot(&mut slots, 0, first),
            FrameDamage::Full
        );
        record_composite_present(&mut slots, 0, first);
        assert_eq!(slots, [FrameDamage::None]);

        assert_eq!(
            composite_repaint_for_slot(&mut slots, 1, second),
            FrameDamage::Full
        );
        record_composite_present(&mut slots, 1, second);
        // Slot zero missed the second frame; when it comes around again, its
        // repaint includes both that history and the new frame's damage.
        assert_eq!(slots, [second, FrameDamage::None]);
        assert_eq!(
            composite_repaint_for_slot(&mut slots, 0, first),
            FrameDamage::Area(aegis_core::Rect::new(10, 10, 40, 40))
        );
    }

    #[test]
    fn geometry_change_forces_full_even_without_new_content() {
        let geometry = aegis_core::SurfaceGeometry {
            position: aegis_core::Point { x: 10, y: 20 },
            ..Default::default()
        };
        let mut old = std::collections::HashMap::new();
        old.insert(
            7,
            SurfaceDamageBaseline {
                generation: 3,
                geometry,
                width: 100,
                height: 80,
            },
        );
        let mut moved = geometry;
        moved.position.x += 1;
        let mut new = std::collections::HashMap::new();
        let mut bbox = None;
        assert!(accumulate_surface(
            &old,
            &mut new,
            SurfaceDamageSample {
                id: 7,
                generation: 3,
                damage: Some(&[]),
                geometry: &moved,
                width: 100,
                height: 80,
            },
            &mut bbox,
        ));
        assert_eq!(bbox, None);
    }

    #[test]
    fn unchanged_geometry_maps_new_content_damage() {
        let geometry = aegis_core::SurfaceGeometry {
            position: aegis_core::Point { x: 10, y: 20 },
            ..Default::default()
        };
        let mut old = std::collections::HashMap::new();
        old.insert(
            7,
            SurfaceDamageBaseline {
                generation: 3,
                geometry,
                width: 100,
                height: 80,
            },
        );
        let mut new = std::collections::HashMap::new();
        let mut bbox = None;
        assert!(!accumulate_surface(
            &old,
            &mut new,
            SurfaceDamageSample {
                id: 7,
                generation: 4,
                damage: Some(&[aegis_core::Rect::new(2, 3, 4, 5)]),
                geometry: &geometry,
                width: 100,
                height: 80,
            },
            &mut bbox,
        ));
        assert_eq!(bbox, Some(aegis_core::Rect::new(12, 23, 4, 5)));
    }
}
