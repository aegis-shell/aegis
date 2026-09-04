//! Concentric geometric shapes, discs, rings, and dots.

use tessera_design::materials;
use lens::{Color, Frame, LayoutOpts, Rect};

/// Place a filled circular disc centered at `(center.0, center.1)`.
pub fn render_disc(frame: &mut Frame, id: &str, center: (f32, f32), diameter: f32, color: Color) {
    let rect = Rect {
        x: center.0 - diameter * 0.5,
        y: center.1 - diameter * 0.5,
        w: diameter,
        h: diameter,
    };
    frame.place(
        id,
        &materials::chrome_place(
            rect,
            LayoutOpts {
                bg: color,
                border: Color::TRANSPARENT,
                radius: diameter * 0.5,
                ..materials::surface_layout()
            },
        ),
        |_| {},
    );
}

/// Place a hollow circular ring centered at `(center.0, center.1)`.
pub fn render_ring(
    frame: &mut Frame,
    id: &str,
    center: (f32, f32),
    diameter: f32,
    color: Color,
    border_width: f32,
) {
    let rect = Rect {
        x: center.0 - diameter * 0.5,
        y: center.1 - diameter * 0.5,
        w: diameter,
        h: diameter,
    };
    frame.place(
        id,
        &materials::chrome_place(
            rect,
            LayoutOpts {
                bg: Color::TRANSPARENT,
                border: color,
                border_width,
                radius: diameter * 0.5,
                ..materials::surface_layout()
            },
        ),
        |_| {},
    );
}

/// Place a small solid dot centered at `(center.0, center.1)` with given radius.
pub fn render_dot(frame: &mut Frame, id: &str, center: (f32, f32), radius: f32, color: Color) {
    render_disc(frame, id, center, radius * 2.0, color);
}
