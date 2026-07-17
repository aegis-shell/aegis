//! Depth-tested glTF wallpaper layer with automatic framing and animation.

use std::f32::consts::{PI, TAU};
use std::path::Path;
use std::time::Instant;

use flux::{Camera, Material, MaterialDesc, MaterialKind, SceneColorLoad, SceneLight, Target};
use flux_scene_graph::{Bounds, Scene};

use crate::Error;

const DEPTH_FORMAT: flux::Format = flux::Format::FLUX_FORMAT_D32_SFLOAT;
const FOV_Y: f32 = 48.0 * PI / 180.0;
const ORBIT_SPEED: f32 = 0.10;
const ORBIT_PITCH: f32 = 0.16;
const FRAME_MARGIN: f32 = 1.20;

pub(super) struct ModelLayer {
    scene: Scene,
    material: Material,
    bounds: Bounds,
    /// A frame may render the model twice at different extents (launcher
    /// backdrop capture followed by the full-size desktop). Keep every depth
    /// attachment referenced by that slot's command buffer alive through
    /// submission; replacing a single per-slot target here would destroy an
    /// image while recorded GPU commands still reference it.
    depth_by_slot: Vec<Vec<Target>>,
    started: Instant,
}

impl ModelLayer {
    pub(super) fn from_path(
        device: &flux::Device,
        surface: &flux::Surface,
        path: &Path,
    ) -> Result<Self, Error> {
        let bytes = std::fs::read(path).map_err(|error| Error::Open(path.to_path_buf(), error))?;
        Self::from_bytes(device, surface, &bytes, path)
    }

    pub(super) fn builtin(device: &flux::Device, surface: &flux::Surface) -> Result<Self, Error> {
        let bytes = procedural_knot_glb();
        Self::from_bytes(
            device,
            surface,
            &bytes,
            Path::new("<built-in procedural knot>"),
        )
    }

    fn from_bytes(
        device: &flux::Device,
        surface: &flux::Surface,
        bytes: &[u8],
        label: &Path,
    ) -> Result<Self, Error> {
        let scene = Scene::from_glb(device, bytes)
            .map_err(|error| Error::Gltf(label.to_path_buf(), error))?;
        let bounds = scene
            .bounds()
            .ok_or_else(|| Error::GltfBounds(label.to_path_buf()))?;
        let material = Material::new(
            device,
            MaterialDesc {
                kind: MaterialKind::Phong,
                base_color: [0.30, 0.48, 0.62, 1.0],
                color_format: surface.format(),
                depth_format: DEPTH_FORMAT,
                shininess: 72.0,
                specular: 0.72,
            },
        )?;
        log::info!(
            "wallpaper: 3D model loaded {:?} ({} primitive(s))",
            label,
            scene.primitive_count()
        );
        Ok(Self {
            scene,
            material,
            bounds,
            depth_by_slot: Vec::new(),
            started: Instant::now(),
        })
    }

    pub(super) fn draw(
        &mut self,
        device: &flux::Device,
        frame: &mut flux::Frame<'_>,
        color_target: Option<&flux::Image>,
    ) -> Result<(), flux::Error> {
        let slot = frame.index() as usize;
        if self.depth_by_slot.len() <= slot {
            self.depth_by_slot.resize_with(slot + 1, Vec::new);
        }

        let surface_size = color_target.map_or_else(|| frame.surface_size(), flux::Image::size);
        let target_index = self.depth_by_slot[slot]
            .iter()
            .position(|target| target.size() == surface_size);
        let target_index = if let Some(index) = target_index {
            index
        } else {
            self.depth_by_slot[slot].push(Target::depth(
                device,
                surface_size.0,
                surface_size.1,
                DEPTH_FORMAT,
            )?);
            self.depth_by_slot[slot].len() - 1
        };
        let depth = &self.depth_by_slot[slot][target_index];

        let elapsed = self.started.elapsed().as_secs_f32();
        let aspect = surface_size.0 as f32 / surface_size.1.max(1) as f32;
        let half_diag = self.bounds.half_diagonal().max(0.001);
        let center = self.bounds.center();
        let fov_x = 2.0 * ((FOV_Y * 0.5).tan() * aspect).atan();
        let fov_min = FOV_Y.min(fov_x);
        let distance = half_diag / (fov_min * 0.5).sin() * FRAME_MARGIN;
        let yaw = elapsed * ORBIT_SPEED;
        let pitch = ORBIT_PITCH + (elapsed * 0.07).sin() * 0.035;
        let cp = pitch.cos();
        let eye = [
            center[0] + distance * cp * yaw.sin(),
            center[1] + distance * pitch.sin(),
            center[2] + distance * cp * yaw.cos(),
        ];
        let near = (half_diag * 0.03).max(0.01);
        let far = (distance + half_diag * 2.0) * 5.0 + 1.0;
        let mut camera = Camera::perspective(FOV_Y, aspect, near, far);
        camera.look_at(eye, center, [0.0, 1.0, 0.0]);

        let light_phase = elapsed * -0.23;
        let warmth = (elapsed * 0.19).sin() * 0.5 + 0.5;
        let light = SceneLight {
            direction: [light_phase.cos() * -0.72, -0.82, light_phase.sin() * -0.72],
            color: [
                0.72 + warmth * 0.34,
                0.86 + (1.0 - warmth) * 0.16,
                1.08 - warmth * 0.18,
            ],
            ambient: 0.16,
        };

        let pass = if let Some(target) = color_target {
            frame.begin_image_scene_pass(target, depth, SceneColorLoad::Load)?
        } else {
            frame.begin_scene_pass(depth, SceneColorLoad::Load)?
        };
        self.scene
            .draw(&pass, &camera, &self.material, Some(&light));
        pass.end();
        Ok(())
    }
}

/// Build a compact binary glTF containing a smooth (2,3) torus knot. The
/// asset is generated in memory so the default 3D wallpaper remains original,
/// deterministic, and free of a second binary artifact in the repository.
fn procedural_knot_glb() -> Vec<u8> {
    const PATH_SEGMENTS: usize = 128;
    const TUBE_SEGMENTS: usize = 12;
    const MAJOR_RADIUS: f32 = 1.32;
    const MINOR_RADIUS: f32 = 0.52;
    const TUBE_RADIUS: f32 = 0.18;
    const P: f32 = 2.0;
    const Q: f32 = 3.0;

    let mut positions = Vec::<[f32; 3]>::with_capacity(PATH_SEGMENTS * TUBE_SEGMENTS);
    let mut normals = Vec::<[f32; 3]>::with_capacity(PATH_SEGMENTS * TUBE_SEGMENTS);
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];

    for i in 0..PATH_SEGMENTS {
        let t = i as f32 / PATH_SEGMENTS as f32 * TAU;
        let pt = P * t;
        let qt = Q * t;
        let ring = MAJOR_RADIUS + MINOR_RADIUS * qt.cos();
        let center = [ring * pt.cos(), MINOR_RADIUS * qt.sin(), ring * pt.sin()];
        let tangent = normalize([
            -MINOR_RADIUS * Q * qt.sin() * pt.cos() - ring * P * pt.sin(),
            MINOR_RADIUS * Q * qt.cos(),
            -MINOR_RADIUS * Q * qt.sin() * pt.sin() + ring * P * pt.cos(),
        ]);
        let radial = [pt.cos(), 0.0, pt.sin()];
        let normal = normalize(sub(radial, scale(tangent, dot(radial, tangent))));
        let binormal = normalize(cross(tangent, normal));

        for j in 0..TUBE_SEGMENTS {
            let v = j as f32 / TUBE_SEGMENTS as f32 * TAU;
            let outward = add(scale(normal, v.cos()), scale(binormal, v.sin()));
            let position = add(center, scale(outward, TUBE_RADIUS));
            for axis in 0..3 {
                min[axis] = min[axis].min(position[axis]);
                max[axis] = max[axis].max(position[axis]);
            }
            positions.push(position);
            normals.push(outward);
        }
    }

    let mut indices = Vec::<u32>::with_capacity(PATH_SEGMENTS * TUBE_SEGMENTS * 12);
    for i in 0..PATH_SEGMENTS {
        let ni = (i + 1) % PATH_SEGMENTS;
        for j in 0..TUBE_SEGMENTS {
            let nj = (j + 1) % TUBE_SEGMENTS;
            let a = (i * TUBE_SEGMENTS + j) as u32;
            let b = (i * TUBE_SEGMENTS + nj) as u32;
            let c = (ni * TUBE_SEGMENTS + j) as u32;
            let d = (ni * TUBE_SEGMENTS + nj) as u32;
            // Include both windings. Flux materials cull back faces, so only
            // the outward-facing winding survives while the knot remains
            // robust to coordinate-system handedness in imported cameras.
            indices.extend_from_slice(&[a, c, b, b, c, d, a, b, c, b, d, c]);
        }
    }

    let position_bytes = positions.len() * 12;
    let normal_bytes = normals.len() * 12;
    let index_bytes = indices.len() * 4;
    let binary_len = position_bytes + normal_bytes + index_bytes;
    let json = format!(
        concat!(
            "{{\"asset\":{{\"version\":\"2.0\",\"generator\":\"ass procedural knot\"}},",
            "\"buffers\":[{{\"byteLength\":{}}}],",
            "\"bufferViews\":[",
            "{{\"buffer\":0,\"byteOffset\":0,\"byteLength\":{},\"target\":34962}},",
            "{{\"buffer\":0,\"byteOffset\":{},\"byteLength\":{},\"target\":34962}},",
            "{{\"buffer\":0,\"byteOffset\":{},\"byteLength\":{},\"target\":34963}}],",
            "\"accessors\":[",
            "{{\"bufferView\":0,\"componentType\":5126,\"count\":{},\"type\":\"VEC3\",",
            "\"min\":[{},{},{}],\"max\":[{},{},{}]}},",
            "{{\"bufferView\":1,\"componentType\":5126,\"count\":{},\"type\":\"VEC3\"}},",
            "{{\"bufferView\":2,\"componentType\":5125,\"count\":{},\"type\":\"SCALAR\"}}],",
            "\"meshes\":[{{\"primitives\":[{{\"attributes\":{{\"POSITION\":0,\"NORMAL\":1}},\"indices\":2}}]}}],",
            "\"nodes\":[{{\"mesh\":0}}],\"scenes\":[{{\"nodes\":[0]}}],\"scene\":0}}"
        ),
        binary_len,
        position_bytes,
        position_bytes,
        normal_bytes,
        position_bytes + normal_bytes,
        index_bytes,
        positions.len(),
        min[0],
        min[1],
        min[2],
        max[0],
        max[1],
        max[2],
        normals.len(),
        indices.len(),
    );

    let json_padded = align4(json.len());
    let binary_padded = align4(binary_len);
    let total_len = 12 + 8 + json_padded + 8 + binary_padded;
    let mut glb = Vec::with_capacity(total_len);
    push_u32(&mut glb, 0x4654_6c67);
    push_u32(&mut glb, 2);
    push_u32(&mut glb, total_len as u32);
    push_u32(&mut glb, json_padded as u32);
    push_u32(&mut glb, 0x4e4f_534a);
    glb.extend_from_slice(json.as_bytes());
    glb.resize(12 + 8 + json_padded, b' ');
    push_u32(&mut glb, binary_padded as u32);
    push_u32(&mut glb, 0x004e_4942);
    for value in positions.iter().flatten().chain(normals.iter().flatten()) {
        glb.extend_from_slice(&value.to_le_bytes());
    }
    for index in indices {
        push_u32(&mut glb, index);
    }
    glb.resize(total_len, 0);
    glb
}

fn align4(value: usize) -> usize {
    (value + 3) & !3
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(v: [f32; 3], factor: f32) -> [f32; 3] {
    [v[0] * factor, v[1] * factor, v[2] * factor]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let length = dot(v, v).sqrt();
    if length <= f32::EPSILON {
        [0.0, 1.0, 0.0]
    } else {
        scale(v, length.recip())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn procedural_model_is_aligned_glb_v2() {
        let bytes = procedural_knot_glb();
        assert_eq!(&bytes[0..4], b"glTF");
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 2);
        assert_eq!(
            u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize,
            bytes.len()
        );
        assert_eq!(bytes.len() % 4, 0);
    }

    #[test]
    fn procedural_model_renders_when_vulkan_is_available() {
        let Ok(device) = flux::Device::new(true, &[], &[]) else {
            return;
        };
        let surface = flux::Surface::offscreen(&device, 160, 96).unwrap();
        let canvas = flux::Canvas::new(&surface).unwrap();
        let mut model = ModelLayer::builtin(&device, &surface).unwrap();

        let mut frame = surface.begin_frame().unwrap();
        canvas
            .begin(&frame, Some(flux::rgba(2, 3, 6, 255)))
            .unwrap();
        canvas.end();
        model.draw(&device, &mut frame, None).unwrap();
        frame.submit().unwrap().present().unwrap();

        let mut pixels = vec![0u8; 160 * 96 * 4];
        surface.read_pixels(&mut pixels).unwrap();
        assert!(pixels
            .chunks_exact(4)
            .any(|pixel| { pixel[0] > 30 || pixel[1] > 30 || pixel[2] > 30 }));
    }

    #[test]
    fn procedural_model_participates_in_downsampled_live_blur() {
        let Ok(device) = flux::Device::new(true, &[], &[]) else {
            return;
        };
        let surface = flux::Surface::offscreen(&device, 160, 96).unwrap();
        let canvas = flux::Canvas::new(&surface).unwrap();
        let target = flux::Image::render_target(&device, 80, 48, surface.format()).unwrap();
        let mut blur = flux::BlurFilter::new(&device).unwrap();
        let mut model = ModelLayer::builtin(&device, &surface).unwrap();

        let mut frame = surface.begin_frame().unwrap();
        canvas
            .begin_target(&frame, &target, Some(flux::rgba(2, 3, 6, 255)))
            .unwrap();
        canvas.end_target();
        model.draw(&device, &mut frame, Some(&target)).unwrap();
        let blurred = blur.apply(&frame, &target, 3.0).unwrap();
        canvas
            .begin(&frame, Some(flux::rgba(0, 0, 0, 255)))
            .unwrap();
        blurred.draw(&canvas, 0.0, 0.0, 160.0, 96.0);
        canvas.end();
        frame.submit().unwrap().present().unwrap();

        let mut pixels = vec![0u8; 160 * 96 * 4];
        surface.read_pixels(&mut pixels).unwrap();
        assert!(pixels
            .chunks_exact(4)
            .any(|pixel| pixel[0] > 20 || pixel[1] > 20 || pixel[2] > 20));
    }

    #[test]
    fn procedural_model_keeps_two_same_frame_depth_targets_alive() {
        let Ok(device) = flux::Device::new(true, &[], &[]) else {
            return;
        };
        let surface = flux::Surface::offscreen(&device, 160, 96).unwrap();
        let canvas = flux::Canvas::new(&surface).unwrap();
        let target = flux::Image::render_target(&device, 80, 48, surface.format()).unwrap();
        let mut model = ModelLayer::builtin(&device, &surface).unwrap();

        let mut frame = surface.begin_frame().unwrap();
        canvas
            .begin_target(&frame, &target, Some(flux::rgba(2, 3, 6, 255)))
            .unwrap();
        canvas.end_target();
        model.draw(&device, &mut frame, Some(&target)).unwrap();

        canvas
            .begin(&frame, Some(flux::rgba(2, 3, 6, 255)))
            .unwrap();
        canvas.end();
        model.draw(&device, &mut frame, None).unwrap();
        frame.submit().unwrap().present().unwrap();

        // Both extents remain cached until their slot can be safely reused.
        assert_eq!(model.depth_by_slot[0].len(), 2);
        let mut pixels = vec![0u8; 160 * 96 * 4];
        surface.read_pixels(&mut pixels).unwrap();
        assert!(pixels
            .chunks_exact(4)
            .any(|pixel| pixel[0] > 20 || pixel[1] > 20 || pixel[2] > 20));
    }
}
