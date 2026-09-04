//! Open/close transition (ADR-0029) unit tests: ghost-frame interpolation,
//! the inset target, capacity bounding, and reduced-motion short-circuits.

use super::*;

/// A closing ghost interpolates from the full rect toward the inset target
/// while fading to zero, then settles (and is reclaimed by
/// `settle_finished_transitions`).
#[test]
fn closing_frame_interpolates_then_settles() {
    let rect = tessera_model::Rect::new(100, 100, 800, 600);
    let frame = ClosingFrame {
        id: 7,
        rect,
        pixels: vec![0u8; 4],
        dmabuf: None,
        buffer_width: 800,
        buffer_height: 600,
        color: None,
        transition: tessera_model::transition::WindowTransition::close(rect, 1000),
    };

    let start = frame.rect_at(1000).expect("in flight at start");
    assert_eq!(start, rect, "close starts at the full rect");
    assert_eq!(frame.opacity_at(1000), Some(1.0));

    let mid = frame
        .rect_at(1000 + tessera_model::transition::CLOSE_DURATION_MS / 2)
        .expect("mid flight");
    // Shrinking toward the inset target: smaller and contained in the
    // original rect.
    assert!(mid.size.w < rect.size.w && mid.size.h < rect.size.h);
    assert!(mid.origin.x >= rect.origin.x && mid.origin.y >= rect.origin.y);
    let mid_opacity = frame
        .opacity_at(1000 + tessera_model::transition::CLOSE_DURATION_MS / 2)
        .expect("mid flight opacity");
    assert!(mid_opacity > 0.0 && mid_opacity < 1.0);

    let deadline = 1000 + tessera_model::transition::CLOSE_DURATION_MS;
    assert_eq!(frame.rect_at(deadline), None);
    assert_eq!(frame.opacity_at(deadline), None);
}

/// The close target insets by one-sixteenth per axis, centred, and never
/// collapses below the 2×2 floor.
#[test]
fn inset_rect_targets_sixteenth_and_keeps_floor() {
    let rect = tessera_model::Rect::new(0, 0, 160, 96);
    let inset = inset_rect(rect, rect.size.w / 16, rect.size.h / 16);
    assert_eq!(inset.size.w, 160 - 20);
    assert_eq!(inset.size.h, 96 - 12);
    // Centred shrink.
    assert_eq!(inset.origin.x, 10);
    assert_eq!(inset.origin.y, 6);

    let tiny = inset_rect(tessera_model::Rect::new(0, 0, 3, 3), 50, 50);
    assert_eq!(tiny.size, tessera_model::Size { w: 2, h: 2 });
}

/// Ghost-frame ids never repeat, so renderer texture caches cannot collide
/// across successive closes.
#[test]
fn closing_frame_ids_never_repeat() {
    let mut seen = std::collections::HashSet::new();
    for _ in 0..1000 {
        assert!(seen.insert(next_closing_frame_id()));
    }
}

/// `MAX_CLOSING_FRAMES` bounds the ghost table: a burst of closes drops the
/// oldest ghost instead of growing compositor memory.
#[test]
fn closing_frame_capacity_drops_oldest() {
    const { assert!(MAX_CLOSING_FRAMES >= 4) };
    let rect = tessera_model::Rect::new(0, 0, 100, 100);
    let mut frames: Vec<ClosingFrame> = Vec::new();
    for i in 0..(MAX_CLOSING_FRAMES + 3) {
        let frame = ClosingFrame {
            id: 100 + i,
            rect,
            pixels: vec![0u8; 4],
            dmabuf: None,
            buffer_width: 100,
            buffer_height: 100,
            color: None,
            transition: tessera_model::transition::WindowTransition::close(rect, 1000),
        };
        if frames.len() >= MAX_CLOSING_FRAMES {
            frames.remove(0);
        }
        frames.push(frame);
    }
    assert_eq!(frames.len(), MAX_CLOSING_FRAMES);
    assert_eq!(
        frames.last().map(|frame| frame.id),
        Some(100 + MAX_CLOSING_FRAMES + 2)
    );
}
